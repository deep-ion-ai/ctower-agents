#!/usr/bin/env bash
# Teste do migrate-to-cthubd.sh sem tocar no sistema: HOME temporário e git,
# curl, systemctl, provision.sh, ctower e cthubd falsos (não precisa compilar
# nada). Os falsos guardam o estado em arquivos (ctowerd/cthubd no ar, sessões
# vivas) e o cthubd falso imita os arquivos que o `cthubd migrate` grava.
#
# Cobre: caminho feliz, rodar de novo (pula os passos 3 e 4; com sessões vivas,
# só atualiza com --yes), falha no migrate (não sobe o cthubd), recusa com
# sessões vivas (e --yes) ou sem contá-las, update_ref sem versão nova,
# --env-name em conflito, ctower update que não sobe o daemon, --dry-run (não
# roda nada), máquina que não é o hub antigo e hub antigo fora da porta 7878
# (o sed do passo 7 troca a porta dele pela do cthubd).
#
# Uso: scripts/test-migrate-to-cthubd.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCRIPT="$ROOT/scripts/migrate-to-cthubd.sh"

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
F="$T/fake"
mkdir -p "$F"

cat > "$F/git" <<'EOF'
#!/bin/sh
echo "git $*" >> "$FAKE_LOG"
EOF
cat > "$F/systemctl" <<'EOF'
#!/bin/sh
echo "systemctl $*" >> "$FAKE_LOG"
case "$*" in
  "--user stop ctowerd") rm -f "$FAKE_STATE/ctowerd_up" ;;
  "--user start ctowerd") touch "$FAKE_STATE/ctowerd_up" ;;
  "--user enable --now cthubd") touch "$FAKE_STATE/cthubd_up" ;;
esac
exit 0
EOF
# curl: só o que o script usa (/healthz das duas portas e /sync/config).
cat > "$F/curl" <<'EOF'
#!/bin/sh
url="" auth="" envid=""
while [ $# -gt 0 ]; do
  case "$1" in
    -H) case "$2" in
          Authorization:*) auth="$2" ;;
          X-Ctower-Env-Id:*) envid="$2" ;;
        esac; shift ;;
    http*) url="$1" ;;
  esac
  shift
done
echo "curl $url" >> "$FAKE_LOG"
case "$url" in
  "http://127.0.0.1:${FAKE_CT_PORT:-7878}/healthz")
    [ -f "$FAKE_STATE/ctowerd_up" ] || exit 7
    echo '{"ok":true,"version":"0.9.0","commit":"abc"}' ;;
  http://127.0.0.1:7879/healthz)
    [ -f "$FAKE_STATE/cthubd_up" ] || exit 7
    echo '{"ok":true,"role":"hub","version":"0.9.0","commit":"abc","protocol":1}' ;;
  http://127.0.0.1:7879/sync/config)
    [ -f "$FAKE_STATE/cthubd_up" ] || exit 7
    [ "$auth" = "Authorization: Bearer $(cat "$HOME/.config/ctower/hub.token")" ] || { echo "401" >&2; exit 22; }
    [ "$envid" = "X-Ctower-Env-Id: $(cat "$HOME/.config/ctower/env_id")" ] || { echo "403" >&2; exit 22; }
    echo '{"config_rev":1,"changed":true,"projects":[],"removed":[]}' ;;
  *) exit 7 ;;
esac
EOF
cat > "$F/ctower" <<'EOF'
#!/bin/sh
echo "ctower $*" >> "$FAKE_LOG"
case "$1" in
  status)
    [ -f "$FAKE_STATE/ctowerd_up" ] || { echo "ctowerd fora do ar (conexão recusada)" >&2; exit 1; }
    [ -f "$FAKE_STATE/status_weird" ] && { echo "formato desconhecido"; exit 0; }
    echo "ctowerd 0.9.0 no ar — $(cat "$FAKE_STATE/live" 2>/dev/null || echo 0) sessão(ões) viva(s), 0 interrompida(s)" ;;
  update)
    if [ "$2" = --check ]; then
      echo "instalado:  0.9.0 (abc)"
      if [ -f "$FAKE_STATE/check_same" ]; then echo "Em dia."
      else echo "Atualização disponível: 3 commit(s) novo(s)."; fi
      exit 0
    fi
    live="$(cat "$FAKE_STATE/live" 2>/dev/null || echo 0)"
    if [ -f "$FAKE_STATE/ctowerd_up" ] && [ "$live" != 0 ] && [ "$2" != --force ]; then
      echo "Recusado: $live sessão(ões) viva(s) seriam encerradas pelo restart:" >&2; exit 1
    fi
    # update_noop: "já está em dia" com o daemon parado, sem subi-lo.
    [ -f "$FAKE_STATE/update_noop" ] && { echo "já está em dia"; exit 0; }
    touch "$FAKE_STATE/ctowerd_up"
    echo "update concluído" ;;
  hub)
    [ -f "$FAKE_STATE/ctowerd_up" ] && [ -f "$FAKE_STATE/cthubd_up" ] || { echo "hub inacessível" >&2; exit 1; }
    echo "hub confirmou tudo" ;;
esac
EOF
# cthubd migrate: grava o que o de verdade grava (config reescrito,
# config.toml.pre-cthub, hub.token, hub.db e config do cthubd).
cat > "$F/cthubd" <<'EOF'
#!/bin/sh
echo "cthubd $*" >> "$FAKE_LOG"
[ "$1" = migrate ] || exit 0
[ -f "$FAKE_STATE/fail_migrate" ] && { echo "erro: hub.db antigo inválido" >&2; exit 1; }
[ -f "$FAKE_STATE/ctowerd_up" ] && { echo "erro: o ctowerd está no ar" >&2; exit 1; }
C="$HOME/.config/ctower"
if grep -qx 'hub = true' "$C/config.toml"; then
  [ -f "$C/config.toml.pre-cthub" ] || cp "$C/config.toml" "$C/config.toml.pre-cthub"
  { echo 'hub_url = "http://127.0.0.1:7879"'; grep -vx 'hub = true' "$C/config.toml"; } > "$C/config.toml.new"
  mv "$C/config.toml.new" "$C/config.toml"
  echo "tok-$$" > "$C/hub.token"
  mkdir -p "$HOME/.config/cthub" "$HOME/.local/share/cthub"
  touch "$HOME/.local/share/cthub/hub.db"
  [ -f "$HOME/.config/cthub/config.toml" ] || echo 'backup_keep = 3' > "$HOME/.config/cthub/config.toml"
  echo "ui" > "$HOME/.config/cthub/ui.token"
  echo "hub.db migrado"
else
  echo "nada a fazer"
fi
EOF
cat > "$F/provision.sh" <<'EOF'
#!/bin/sh
echo "provision.sh $* (CTOWER_REPO=$CTOWER_REPO)" >> "$FAKE_LOG"
mkdir -p "$HOME/.cargo/bin"
cp "$FAKE_DIR/cthubd" "$HOME/.cargo/bin/cthubd"
EOF
chmod +x "$F"/*

REPO="$T/repo"
mkdir -p "$REPO/.git" "$REPO/scripts/provision"
# Sem o bit de execução, como no git: o script tem de chamá-lo com bash.
cp "$F/provision.sh" "$REPO/scripts/provision/provision.sh"
chmod 644 "$REPO/scripts/provision/provision.sh"
mkdir -p "$F/path"
for b in git curl; do ln -s "$F/$b" "$F/path/$b"; done

fail() { echo "FALHOU: $*" >&2; exit 1; }

# Uma máquina do hub antigo nova: ctowerd no ar com hub = true.
H="" S=""
new_machine() {
  H="$T/$1/home" S="$T/$1/state"
  mkdir -p "$H/.config/ctower" "$H/.local/share/ctower" "$H/.cargo/bin" "$S"
  printf 'env_name = "hub-antigo"\nhub = true\nhub_backup_keep = 3\n' > "$H/.config/ctower/config.toml"
  printf '0b6c1f7e-1a2b-4c3d-8e9f-000000000001\n' > "$H/.config/ctower/env_id"
  touch "$H/.local/share/ctower/hub.db"
  cp "$F/ctower" "$H/.cargo/bin/ctower"
  touch "$S/ctowerd_up"
  : > "$T/log"
}

run() {
  env -i PATH="$F/path:/usr/local/bin:/usr/bin:/bin" HOME="$H" USER="${USER:-teste}" \
    FAKE_LOG="$T/log" FAKE_STATE="$S" FAKE_DIR="$F" FAKE_CT_PORT="${FAKE_CT_PORT:-7878}" \
    CT_SYSTEMCTL="$F/systemctl" MIGRATE_WAIT_SECS=1 \
    bash "$SCRIPT" --repo "$REPO" "$@"
}

# Linha do log em que aparece o padrão (0 = não aparece).
at() { grep -n -m1 -x -F -- "$1" "$T/log" | cut -d: -f1 || true; }

# 1. Caminho feliz: os passos 2 a 6 na ordem do roteiro.
new_machine feliz
run > "$T/out1" 2>&1 || fail "caminho feliz falhou: $(cat "$T/out1")"
prev=0
for p in "git -C $REPO pull --ff-only" "provision.sh --hub --skip-packages (CTOWER_REPO=$REPO)" \
         "ctower update --check" "ctower status" "systemctl --user stop ctowerd" \
         "cthubd migrate --from $H/.local/share/ctower" \
         "systemctl --user enable --now cthubd" "curl http://127.0.0.1:7879/sync/config" \
         "ctower update" "ctower hub sync"; do
  n="$(at "$p")"
  [[ -n $n ]] || fail "não rodou: $p ($(cat "$T/log"))"
  (( n > prev )) || fail "fora de ordem: $p ($(cat "$T/log"))"
  prev=$n
done
CFG="$H/.config/ctower/config.toml"
grep -qx 'hub_url = "http://127.0.0.1:7879"' "$CFG" || fail "config não convertido"
grep -q "Passos 2 a 6 concluídos" "$T/out1" || fail "não disse que concluiu"
grep -qF ':7878(/?[\"'"'"'])#\1:7879\2#" ~/.config/ctower/config.toml' "$T/out1" \
  || fail "não mostrou o sed do passo 7: $(cat "$T/out1")"
grep -q "ctower restart" "$T/out1" || fail "não mostrou o ctower restart do passo 7"
grep -qF "$H/.config/cthub/ui.token" "$T/out1" || fail "não mostrou o ui.token do passo 8"
grep -q "não use sessões nesta máquina" "$T/out1" || fail "não lembrou da janela segura"
CFG1="$(cat "$CFG")"
TOK1="$(cat "$H/.config/ctower/hub.token")"

grep -q "start ctowerd" "$T/log" && fail "subiu o ctowerd à mão sem precisar"

# 2. De novo (já em dia): pula a parada, o migrate e o ctower update.
: > "$T/log"
touch "$S/check_same"
run > "$T/out2" 2>&1 || fail "rodar de novo falhou: $(cat "$T/out2")"
grep -q "stop ctowerd" "$T/log" && fail "parou o ctowerd de novo"
grep -q "cthubd migrate" "$T/log" && fail "rodou o migrate de novo"
grep -q "ctower status" "$T/log" && fail "conferiu sessões sem precisar"
grep -q "Passos 3 e 4: já feitos" "$T/out2" || fail "não disse que pulou os passos 3 e 4"
grep -q "ctower hub sync" "$T/log" || fail "não sincronizou na segunda vez"
grep -qx "ctower update" "$T/log" && fail "rodou o ctower update sem versão nova"
grep -q "pulei o ctower update" "$T/out2" || fail "não disse que pulou o ctower update"

# 2b. De novo, com sessões vivas e versão nova: recusa sugerindo --yes; com
#     --yes, vai --force ao ctower update.
rm "$S/check_same"
echo 1 > "$S/live"
: > "$T/log"
if run > "$T/out2b" 2>&1; then
  fail "atualizou com sessões vivas"
fi
grep -q "ERRO no passo 6" "$T/out2b" || fail "não disse o passo 6: $(cat "$T/out2b")"
grep -q -- "--yes" "$T/out2b" || fail "não sugeriu --yes"
grep -q "ficou parado" "$T/out2b" && fail "disse que o ctowerd ficou parado sem tê-lo parado"
: > "$T/log"
run --yes > "$T/out2c" 2>&1 || fail "--yes não seguiu no passo 6: $(cat "$T/out2c")"
grep -qx "ctower update --force" "$T/log" || fail "--yes não foi como --force: $(cat "$T/log")"
# Sem versão nova, com sessões vivas, termina sem tocar no ctowerd.
touch "$S/check_same"
: > "$T/log"
run > "$T/out2d" 2>&1 || fail "recusou com sessões vivas e sem versão nova: $(cat "$T/out2d")"
[[ "$(cat "$CFG")" == "$CFG1" ]] || fail "rodar de novo mudou o config"
[[ "$(cat "$H/.config/ctower/hub.token")" == "$TOK1" ]] || fail "rodar de novo trocou o token"

# 3. Falha no migrate: para no passo 4, sem subir o cthubd; repassa
#    --env-name e --force.
new_machine falha
touch "$S/fail_migrate"
if run --env-name hub-antigo --force > "$T/out3" 2>&1; then
  fail "seguiu com o migrate falhando"
fi
grep -q "cthubd migrate --from $H/.local/share/ctower --env-name hub-antigo --force" "$T/log" \
  || fail "não repassou --env-name/--force: $(cat "$T/log")"
grep -q "ERRO no passo 4" "$T/out3" || fail "não disse o passo que falhou: $(cat "$T/out3")"
grep -q "ficou parado" "$T/out3" || fail "não avisou que o ctowerd ficou parado"
grep -q "Voltar atrás" "$T/out3" || fail "não apontou o Voltar atrás"
grep -q "enable --now cthubd" "$T/log" && fail "subiu o cthubd depois do migrate falhar"
grep -qx "ctower update" "$T/log" && fail "atualizou o ctowerd depois do migrate falhar"
[[ ! -f $S/cthubd_up ]] || fail "cthubd no ar"
# Corrigido o problema, rodar de novo termina.
rm "$S/fail_migrate"
run > "$T/out3b" 2>&1 || fail "não retomou depois da falha: $(cat "$T/out3b")"
grep -q "Passos 2 a 6 concluídos" "$T/out3b" || fail "retomada não concluiu"

# 4. Sessões vivas: recusa antes de parar o ctowerd; com --yes, segue.
new_machine sessoes
echo 2 > "$S/live"
if run > "$T/out4" 2>&1; then
  fail "parou o ctowerd com sessões vivas"
fi
grep -q "2 sessão(ões) viva(s)" "$T/out4" || fail "não mostrou as sessões vivas: $(cat "$T/out4")"
grep -q -- "--yes" "$T/out4" || fail "não indicou o --yes"
grep -q "ERRO no passo 3" "$T/out4" || fail "não disse o passo 3"
grep -q "stop ctowerd" "$T/log" && fail "parou o ctowerd mesmo recusando"
grep -q "cthubd migrate" "$T/log" && fail "migrou mesmo recusando"
[[ -f $S/ctowerd_up ]] || fail "ctowerd fora do ar depois da recusa"
: > "$T/log"
run --yes > "$T/out4b" 2>&1 || fail "--yes não seguiu: $(cat "$T/out4b")"
grep -q "stop ctowerd" "$T/log" || fail "--yes não parou o ctowerd"

# 4b. ctower status sem contagem legível: recusa sem --yes.
new_machine status
touch "$S/status_weird"
if run > "$T/out4c" 2>&1; then
  fail "parou o ctowerd sem conseguir ver as sessões"
fi
grep -q "não consegui ver as sessões vivas" "$T/out4c" || fail "não explicou: $(cat "$T/out4c")"
grep -q "stop ctowerd" "$T/log" && fail "parou o ctowerd sem ver as sessões"

# 4c. update_ref sem versão nova: recusa antes de parar o ctowerd.
new_machine emdia
touch "$S/check_same"
if run > "$T/out4d" 2>&1; then
  fail "migrou sem versão nova para o passo 6"
fi
grep -q "update_ref" "$T/out4d" || fail "não apontou o update_ref: $(cat "$T/out4d")"
grep -q "stop ctowerd" "$T/log" && fail "parou o ctowerd sem versão nova"

# 4d. --env-name diferente do env_name do config: recusa antes de parar.
new_machine nome
if run --env-name outro > "$T/out4e" 2>&1; then
  fail "aceitou --env-name diferente do config"
fi
grep -q "use um só" "$T/out4e" || fail "não explicou o conflito de nome: $(cat "$T/out4e")"
grep -q "stop ctowerd" "$T/log" && fail "parou o ctowerd com o nome em conflito"

# 4e. ctower update "em dia" sem subir o daemon parado: sobe-o com systemctl.
new_machine noop
touch "$S/update_noop"
run > "$T/out4f" 2>&1 || fail "não se recuperou do update sem restart: $(cat "$T/out4f")"
grep -qx "systemctl --user start ctowerd" "$T/log" || fail "não subiu o ctowerd: $(cat "$T/log")"
grep -q "Aviso: o ctowerd não respondeu" "$T/out4f" || fail "não avisou do ctowerd fora do ar"
grep -q "Passos 2 a 6 concluídos" "$T/out4f" || fail "não concluiu depois de subir o ctowerd"

# 5. --dry-run: mostra os comandos e não roda nada.
new_machine seco
cp "$H/.config/ctower/config.toml" "$T/cfg5"
run --dry-run > "$T/out5" 2>&1 || fail "--dry-run falhou: $(cat "$T/out5")"
[[ ! -s $T/log ]] || fail "--dry-run rodou algo: $(cat "$T/log")"
cmp -s "$H/.config/ctower/config.toml" "$T/cfg5" || fail "--dry-run mudou o config"
for p in "+ git -C $REPO pull --ff-only" "provision.sh --hub --skip-packages" "stop ctowerd" \
         "cthubd migrate --from $H/.local/share/ctower" "enable --now cthubd" \
         "ctower update" "ctower hub sync"; do
  grep -qF -- "$p" "$T/out5" || fail "--dry-run não mostrou: $p"
done
[[ ! -e "$H/.cargo/bin/cthubd" ]] || fail "--dry-run instalou o cthubd"

# 6. Máquina que não é o hub antigo nem foi migrada: recusa sem rodar nada.
new_machine comum
printf 'env_name = "notebook"\nhub_url = "http://mac-mini:7879"\n' > "$H/.config/ctower/config.toml"
if run > "$T/out6" 2>&1; then
  fail "aceitou uma máquina que não é o hub antigo"
fi
grep -q "Nada a migrar" "$T/out6" || fail "não explicou: $(cat "$T/out6")"
[[ ! -s $T/log ]] || fail "rodou algo numa máquina comum: $(cat "$T/log")"

# 7. Hub antigo na porta 7979 (port no config do ctowerd): o sed do passo 7
#    troca :7979 por :7879, também ao rodar de novo (port do .pre-cthub).
new_machine porta
printf 'port = 7979\n' >> "$H/.config/ctower/config.toml"
FAKE_CT_PORT=7979 run > "$T/out7" 2>&1 || fail "hub antigo na 7979 falhou: $(cat "$T/out7")"
grep -qF ':7979(/?[\"'"'"'])#\1:7879\2#" ~/.config/ctower/config.toml' "$T/out7" \
  || fail "o sed do passo 7 não usou a porta 7979: $(cat "$T/out7")"
grep -qF ':7878(' "$T/out7" && fail "o sed do passo 7 usou a 7878 com o hub antigo na 7979"
grep -q "a do hub antigo, 7979" "$T/out7" || fail "não disse a porta do hub antigo"
touch "$S/check_same"
FAKE_CT_PORT=7979 run > "$T/out7b" 2>&1 || fail "rodar de novo (7979) falhou: $(cat "$T/out7b")"
grep -q "Passos 3 e 4: já feitos" "$T/out7b" || fail "não pulou os passos 3 e 4 (7979)"
grep -qF ':7979(/?[\"'"'"'])#\1:7879\2#" ~/.config/ctower/config.toml' "$T/out7b" \
  || fail "rodar de novo não usou a porta 7979: $(cat "$T/out7b")"

echo "migrate-to-cthubd.sh: ok"
