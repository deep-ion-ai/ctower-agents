#!/usr/bin/env bash
# Migra a máquina do hub antigo (ctowerd com `hub = true`, porta 7878 por
# padrão) para o cthubd (porta 7879): roda os passos 2 a 6 do roteiro de
# docs/hub.md, seção 5.
# Os passos 1, 7 e 8 (outros ambientes e UI) e o "Voltar atrás" ficam à mão.
#
# Uso: migrate-to-cthubd.sh [--repo DIR] [--env-name NOME] [--force] [--yes] [--dry-run]
#
#   --repo DIR     clone deste repositório (padrão: CTOWER_REPO ou
#                  ~/projetos/control-tower)
#   --env-name     nome do ctowerd desta máquina no hub, repassado ao
#                  `cthubd migrate` (padrão: env_name do config ou o nome da máquina)
#   --force        repassado ao `cthubd migrate`: refaz a migração a partir do
#                  hub.db antigo (nova tentativa depois de voltar atrás)
#   --yes          segue mesmo com sessões vivas no ctowerd (parar o daemon as
#                  encerra; elas voltam como interrompidas); vai como --force ao
#                  ctower update
#   --dry-run      só mostra os comandos, sem rodar nada
#
# Passos (cada um para no primeiro erro; rodar de novo é seguro):
#   2. git pull no clone e provision.sh --hub --skip-packages (instala o cthubd sem subir)
#   3. confere que o ctower update vê versão nova (ctower update --check) e as
#      sessões vivas (ctower status), para o ctowerd e confere a porta dele
#   4. cthubd migrate --from ~/.local/share/ctower e confere os arquivos gravados
#   5. systemctl --user enable --now cthubd, /healthz e /sync/config com o hub.token
#   6. ctower update (com --force se --yes; já no ar e em dia, pula) e ctower hub sync
# Já migrado (o ctowerd ligado ao cthubd local), pula os passos 3 e 4.
#
# Só Linux (o ctowerd roda no Linux/WSL). Variáveis para testar sem mexer no
# sistema: CT_SYSTEMCTL troca o systemctl (também repassado ao ctower como
# CTOWER_SYSTEMCTL); MIGRATE_WAIT_SECS é a espera pelas portas (padrão 30).
set -euo pipefail

REPO="${CTOWER_REPO:-$HOME/projetos/control-tower}"
ENV_NAME_ARG=""
FORCE=0
YES=0
DRY_RUN=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo) REPO="${2:?--repo precisa de um diretório}"; shift ;;
    --env-name) ENV_NAME_ARG="${2:?--env-name precisa de um nome}"; shift ;;
    --force) FORCE=1 ;;
    --yes) YES=1 ;;
    --dry-run) DRY_RUN=1 ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) echo "opção desconhecida: $1 (veja --help)" >&2; exit 2 ;;
  esac
  shift
done
if [[ -n $ENV_NAME_ARG && ! $ENV_NAME_ARG =~ ^[A-Za-z0-9._-]+$ ]]; then
  echo "--env-name inválido: \"$ENV_NAME_ARG\" (use só letras, números, '.', '_' e '-')." >&2
  exit 2
fi

if [[ $EUID -eq 0 ]]; then
  echo "Rode como usuário comum (o mesmo do ctowerd)." >&2
  exit 1
fi
if [[ "$(uname -s)" != Linux ]]; then
  echo "Só Linux: o ctowerd que era o hub roda no Linux/WSL (docs/hub.md, seção 5)." >&2
  exit 2
fi

SYSTEMCTL="${CT_SYSTEMCTL:-systemctl}"
WAIT_SECS="${MIGRATE_WAIT_SECS:-30}"
BIN="$HOME/.cargo/bin"
# Os mesmos diretórios que ctowerd/ctower e cthubd usam.
CT_CONF="${CTOWER_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/ctower}"
CT_DATA="${CTOWER_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/ctower}"
HUB_CONF="${CTHUB_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/cthub}"
HUB_DATA="${CTHUB_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/cthub}"
CT_CFG="$CT_CONF/config.toml"
STEP=""
# 1 depois que este script parou o ctowerd e até ele voltar a responder.
CT_STOPPED=0

# Valor de uma chave de topo (`chave = "valor"` ou `chave = 123`) de um TOML
# simples; vazio se não houver. Para na primeira tabela ([...]). (Igual ao
# de provision.sh.)
toml_get() {
  local file="$1" key="$2"
  [[ -f $file ]] || return 0
  awk -v k="$key" '
    /^[[:space:]]*\[/ { exit }
    {
      line = $0
      sub(/^[[:space:]]+/, "", line)
      if (index(line, k) != 1) next
      rest = substr(line, length(k) + 1)
      if (rest !~ /^[[:space:]]*=/) next
      sub(/^[[:space:]]*=[[:space:]]*/, "", rest)
      sub(/[[:space:]]*#.*$/, "", rest)
      gsub(/^"|"$/, "", rest)
      print rest
      exit
    }' "$file"
}

# Portas: as mesmas regras do ctowerd (CTOWER_PORT ou `port`, padrão 7878) e
# do cthubd (CTHUB_PORT ou `port`, padrão 7879).
ct_port() {
  local p
  p="$(toml_get "$CT_CFG" port)"
  echo "${CTOWER_PORT:-${p:-7878}}"
}
hub_port() {
  local p
  p="$(toml_get "$HUB_CONF/config.toml" port)"
  echo "${CTHUB_PORT:-${p:-7879}}"
}
# Porta do hub antigo, que os outros ambientes ainda têm em hub_url (passo 7):
# a do ctowerd daqui (CTOWER_PORT ou `port`, padrão 7878). Depois da
# conversão, vale a `port` do config.toml.pre-cthub (o config do hub antigo).
old_hub_port() {
  local p=""
  if ! legacy_hub && [[ -f "$CT_CFG.pre-cthub" ]]; then
    p="$(toml_get "$CT_CFG.pre-cthub" port)"
  fi
  [[ -n $p ]] || p="$(toml_get "$CT_CFG" port)"
  echo "${CTOWER_PORT:-${p:-7878}}"
}

say() { printf '%s\n' "$*"; }

# Sai explicando qual passo falhou e onde está o caminho de volta.
die() {
  echo >&2
  echo "ERRO no passo $STEP: $*" >&2
  if [[ $CT_STOPPED -eq 1 ]]; then
    echo "O ctowerd daqui ficou parado. Para voltar ao hub antigo: docs/hub.md, seção 5," >&2
    echo "\"Voltar atrás\" (systemctl --user disable --now cthubd, o config.toml.pre-cthub de volta" >&2
    echo "e systemctl --user start ctowerd, ou ctower update --rollback se ele já foi atualizado)." >&2
  fi
  echo "Corrija e rode este script de novo (é seguro), ou volte atrás pelo roteiro" >&2
  echo "de docs/hub.md, seção 5, \"Voltar atrás\"." >&2
  STEP=""
  exit 1
}

# Falha inesperada (set -e) também diz o passo.
# shellcheck disable=SC2317 # chamada pelo trap EXIT
on_exit() {
  local rc=$?
  if [[ $rc -ne 0 && -n $STEP ]]; then
    echo >&2
    echo "ERRO no passo $STEP (saída $rc)." >&2
    echo "Corrija e rode este script de novo (é seguro), ou volte atrás pelo roteiro" >&2
    echo "de docs/hub.md, seção 5, \"Voltar atrás\"." >&2
  fi
}

# Mostra o comando e o roda (no --dry-run, só mostra).
run() {
  say "+ $*"
  [[ $DRY_RUN -eq 1 ]] && return 0
  "$@"
}

step() {
  STEP="$1"
  echo
  say "== Passo $1: $2"
}

# /healthz responde em 127.0.0.1:<porta>? Imprime o corpo.
healthz() {
  curl -fsS --max-time 2 "http://127.0.0.1:$1/healthz" 2>/dev/null
}

# Espera até WAIT_SECS pela condição ("up" ou "down") do /healthz.
wait_healthz() {
  local port="$1" want="$2" i
  for ((i = 0; i <= WAIT_SECS; i++)); do
    if healthz "$port" >/dev/null; then
      [[ $want == up ]] && return 0
    else
      [[ $want == down ]] && return 0
    fi
    [[ $i -lt $WAIT_SECS ]] && sleep 1
  done
  return 1
}

# O ctowerd daqui ainda é o hub antigo?
legacy_hub() {
  [[ "$(toml_get "$CT_CFG" hub)" == "true" ]]
}

# A migração já terminou: o ctowerd daqui virou ambiente ligado ao cthubd
# local (o que o `cthubd migrate` grava) e o hub.db do cthubd existe.
converted() {
  local url
  url="$(toml_get "$CT_CFG" hub_url)"
  url="${url%/}"
  ! legacy_hub \
    && [[ $url == "http://127.0.0.1:$(hub_port)" ]] \
    && [[ -f "$CT_CFG.pre-cthub" && -f "$HUB_DATA/hub.db" ]]
}

# --- passos -------------------------------------------------------------------

step2_install() {
  step 2 "instalar o cthubd sem subi-lo, do clone atualizado"
  [[ -e "$REPO/.git" || $DRY_RUN -eq 1 ]] \
    || die "$REPO não é um clone do control-tower (use --repo)."
  run git -C "$REPO" pull --ff-only || die "git pull --ff-only em $REPO falhou."
  # CT_FROM_MIGRATE: o provision.sh não manda rodar este script de novo.
  if ! run env CTOWER_REPO="$REPO" CT_FROM_MIGRATE=1 \
    bash "$REPO/scripts/provision/provision.sh" --hub --skip-packages; then
    if converted; then
      die "provision.sh --hub --skip-packages falhou (ele sobe o cthubd já migrado: confira
  journalctl --user -u cthubd)."
    fi
    die "provision.sh --hub --skip-packages falhou."
  fi
  if [[ $DRY_RUN -eq 0 && ! -x "$BIN/cthubd" ]]; then
    die "o provision.sh não instalou $BIN/cthubd."
  fi
}

# Antes de parar o ctowerd: o passo 6 precisa de uma versão nova no
# update_ref (um ctowerd com hub = true é sempre um build antigo; "em dia" quer
# dizer que o update_ref aponta para um ref sem o cthubd, e o ctower update não
# o reiniciaria), e o --env-name não pode brigar com o env_name do config (o
# cthubd migrate recusaria com o ctowerd já parado).
preflight() {
  local cfg_name out
  STEP="3 (conferência antes de parar o ctowerd)"
  cfg_name="$(toml_get "$CT_CFG" env_name)"
  if [[ -n $ENV_NAME_ARG && -n $cfg_name && $cfg_name != "$ENV_NAME_ARG" ]]; then
    die "env_name = \"$cfg_name\" em $CT_CFG e --env-name $ENV_NAME_ARG: use um só
  (o cthubd migrate recusa os dois diferentes)."
  fi
  if [[ $DRY_RUN -eq 1 ]]; then
    say "+ $BIN/ctower update --check    # precisa de \"Atualização disponível\""
    return 0
  fi
  say "+ $BIN/ctower update --check"
  out="$("$BIN/ctower" update --check 2>&1)" || {
    say "$out"
    die "ctower update --check falhou: sem ele, o passo 6 (ctower update) não tem como ser conferido."
  }
  say "$out"
  grep -q '^Atualização disponível' <<<"$out" \
    || die "o ctower update não vê versão nova: o passo 6 não subiria o ctowerd com o cthubd.
  Aponte update_ref em $CT_CFG para um ref que traz o cthubd (docs/hub.md, seção 5:
  \"Use a mesma versão (a que traz o cthubd) em todas as máquinas\") e rode de novo."
}

step3_stop() {
  local port st n
  port="$(ct_port)"
  step 3 "parar o ctowerd (hub antigo, porta $port)"
  if [[ $DRY_RUN -eq 1 ]]; then
    say "+ $BIN/ctower status    # recusa com sessões vivas (salvo --yes)"
    say "+ $SYSTEMCTL --user stop ctowerd"
    say "+ curl -s http://127.0.0.1:$port/healthz    # esperado: sem resposta"
    return 0
  fi
  if healthz "$port" >/dev/null; then
    say "+ $BIN/ctower status"
    if st="$("$BIN/ctower" status 2>&1)"; then
      say "$st"
      n="$(grep -oE '[0-9]+ sessão\(ões\) viva' <<<"$st" | head -1 | cut -d' ' -f1 || true)"
      [[ -n $n ]] || n="?"
    else
      say "$st"
      n="?"
    fi
    if [[ $n != 0 && $YES -eq 0 ]]; then
      if [[ $n == "?" ]]; then
        die "não consegui ver as sessões vivas do ctowerd (ctower status). Confira à mão e,
  para parar o daemon mesmo assim, rode de novo com --yes."
      fi
      die "o ctowerd tem $n sessão(ões) viva(s); parar o daemon as encerra (voltam como
  interrompidas). Espere terminarem ou rode de novo com --yes."
    fi
  else
    say "ctowerd fora do ar em 127.0.0.1:$port."
  fi
  run "$SYSTEMCTL" --user stop ctowerd || die "systemctl --user stop ctowerd falhou."
  CT_STOPPED=1
  wait_healthz "$port" down \
    || die "o ctowerd ainda responde em 127.0.0.1:$port/healthz depois de ${WAIT_SECS}s."
  say "ctowerd parado (127.0.0.1:$port/healthz sem resposta)."
}

step4_migrate() {
  local args=(migrate --from "$CT_DATA") url
  [[ -n $ENV_NAME_ARG ]] && args+=(--env-name "$ENV_NAME_ARG")
  [[ $FORCE -eq 1 ]] && args+=(--force)
  step 4 "migrar o hub.db e ligar o ctowerd daqui ao cthubd"
  run "$BIN/cthubd" "${args[@]}" || die "cthubd migrate falhou (o cthubd não foi iniciado)."
  [[ $DRY_RUN -eq 1 ]] && return 0
  url="http://127.0.0.1:$(hub_port)"
  say "+ grep hub_url $CT_CFG"
  grep hub_url "$CT_CFG" || true
  [[ "$(toml_get "$CT_CFG" hub_url)" == "$url" ]] \
    || die "esperava hub_url = \"$url\" em $CT_CFG."
  legacy_hub && die "$CT_CFG ainda tem hub = true."
  [[ -f "$CT_CFG.pre-cthub" ]] || die "falta $CT_CFG.pre-cthub (o config de antes)."
  [[ -f "$HUB_DATA/hub.db" ]] || die "falta $HUB_DATA/hub.db."
  [[ -s "$CT_CONF/hub.token" ]] || die "falta $CT_CONF/hub.token."
  [[ -f "$HUB_CONF/config.toml" ]] || die "falta $HUB_CONF/config.toml."
  say "+ cat $HUB_CONF/config.toml"
  cat "$HUB_CONF/config.toml"
}

step5_start_hub() {
  local port body
  port="$(hub_port)"
  step 5 "subir o cthubd (porta $port) e conferir o token do ctowerd daqui"
  # enable --now não reinicia um cthubd já no ar.
  run "$SYSTEMCTL" --user enable --now cthubd || die "systemctl --user enable --now cthubd falhou."
  if [[ $DRY_RUN -eq 1 ]]; then
    say "+ curl -s http://127.0.0.1:$port/healthz    # \"role\":\"hub\""
    say "+ curl -s -H \"Authorization: Bearer \$(cat $CT_CONF/hub.token)\" \\"
    say "       -H \"X-Ctower-Env-Id: \$(cat $CT_CONF/env_id)\" http://127.0.0.1:$port/sync/config"
    return 0
  fi
  wait_healthz "$port" up \
    || die "o cthubd não respondeu em 127.0.0.1:$port/healthz em ${WAIT_SECS}s (journalctl --user -u cthubd)."
  body="$(healthz "$port")" || die "o cthubd parou de responder em 127.0.0.1:$port/healthz."
  say "+ curl -s http://127.0.0.1:$port/healthz"
  say "$body"
  grep -q '"role":"hub"' <<<"$body" \
    || die "127.0.0.1:$port/healthz não é do cthubd (sem \"role\":\"hub\")."
  [[ -s "$CT_CONF/env_id" ]] || die "falta $CT_CONF/env_id."
  say "+ curl -s -H \"Authorization: Bearer \$(cat $CT_CONF/hub.token)\" -H \"X-Ctower-Env-Id: \$(cat $CT_CONF/env_id)\" http://127.0.0.1:$port/sync/config"
  body="$(curl -fsS --max-time 10 \
    -H "Authorization: Bearer $(tr -d '[:space:]' < "$CT_CONF/hub.token")" \
    -H "X-Ctower-Env-Id: $(tr -d '[:space:]' < "$CT_CONF/env_id")" \
    "http://127.0.0.1:$port/sync/config" 2>&1)" \
    || die "o cthubd recusou o hub.token do ctowerd daqui em /sync/config: $body"
  say "$body"
}

step6_update_env() {
  local port out args=(update) skip=0
  port="$(ct_port)"
  step 6 "atualizar e subir o ctowerd daqui"
  if [[ $DRY_RUN -eq 1 ]]; then
    say "+ $BIN/ctower update --check    # só se o ctowerd já estiver no ar (rodar de novo)"
  elif healthz "$port" >/dev/null; then
    # Rodando de novo, com o ctowerd já no ar: só atualiza se houver versão
    # nova (o ctower update recusa com sessões vivas, mesmo sem nada a fazer).
    say "+ $BIN/ctower update --check"
    out="$("$BIN/ctower" update --check 2>&1)" || { say "$out"; die "ctower update --check falhou."; }
    say "$out"
    grep -q '^Atualização disponível' <<<"$out" || skip=1
  fi
  if [[ $skip -eq 1 ]]; then
    say "ctowerd no ar e sem versão nova; pulei o ctower update."
  else
    # ctower update (e não o provision.sh): guarda o binário anterior e o
    # snapshot do ledger.db para `ctower update --rollback`.
    [[ $YES -eq 1 ]] && args+=(--force)
    run env CTOWER_SYSTEMCTL="$SYSTEMCTL" "$BIN/ctower" "${args[@]}" \
      || die "ctower update falhou. Se recusou por sessões vivas, espere terminarem ou rode
  de novo com --yes (o restart as encerra)."
    if [[ $DRY_RUN -eq 0 ]] && ! wait_healthz "$port" up; then
      # "Já está em dia" com o daemon parado não o sobe.
      echo "Aviso: o ctowerd não respondeu depois do ctower update; subindo-o." >&2
      run "$SYSTEMCTL" --user start ctowerd || die "systemctl --user start ctowerd falhou."
      wait_healthz "$port" up \
        || die "o ctowerd não respondeu em 127.0.0.1:$port/healthz (journalctl --user -u ctowerd)."
    fi
  fi
  CT_STOPPED=0
  run "$BIN/ctower" hub sync || die "ctower hub sync falhou (veja a mensagem acima; docs/hub.md, seção 3)."
}

main() {
  trap on_exit EXIT
  if [[ $DRY_RUN -eq 1 ]]; then
    say "(--dry-run: só mostra os comandos; nada é executado)"
  fi
  if ! legacy_hub && ! converted; then
    echo "O ctowerd daqui não é o hub antigo (sem hub = true em $CT_CFG) nem foi migrado" >&2
    echo "por este roteiro (hub_url = \"http://127.0.0.1:$(hub_port)\", $CT_CFG.pre-cthub e" >&2
    echo "$HUB_DATA/hub.db). Nada a migrar; veja docs/hub.md, seção 5." >&2
    exit 1
  fi

  step2_install
  if converted; then
    echo
    say "== Passos 3 e 4: já feitos (o ctowerd daqui já está ligado ao cthubd local); pulei."
    if [[ $FORCE -eq 1 ]]; then
      say "   --force ignorado: só vale com o ctowerd ainda em hub = true (depois de voltar atrás)."
    fi
  else
    preflight
    step3_stop
    step4_migrate
  fi
  step5_start_hub
  step6_update_env
  STEP=""

  local hub old
  hub="$(hub_port)"
  old="$(old_hub_port)"
  echo
  if [[ $DRY_RUN -eq 1 ]]; then
    say "(--dry-run: nada foi executado)"
  else
    say "Passos 2 a 6 concluídos nesta máquina."
  fi
  cat <<EOF

Faltam os passos à mão (docs/hub.md, seção 5):

  7. Nos outros ambientes, troque a porta de hub_url (a do hub antigo, $old) para $hub e reinicie:

       sed -i -E "s#^(hub_url[[:space:]]*=[[:space:]]*[\"']https?://[^:/\"']+):$old(/?[\"'])#\1:$hub\2#" ~/.config/ctower/config.toml
       grep '^hub_url' ~/.config/ctower/config.toml   # esperado: hub_url = "http://<máquina do hub>:$hub"
       ctower restart
       ctower hub sync

  8. Ligue a UI ao hub (Configurações → Hub, URL http://<máquina do hub>:$hub e o
     conteúdo de $HUB_CONF/ui.token) e confira na tela Hub: os ambientes com
     0 pendências, as atribuições aplicadas e a quarentena.

Até conferir o passo 8, não use sessões nesta máquina: se precisar voltar atrás, o
que elas gravarem não chega ao hub antigo (docs/hub.md, seção 5, "Voltar atrás").
EOF
}

# Tudo dentro de main: o git pull do passo 2 pode trocar este arquivo no meio.
main "$@"; exit
