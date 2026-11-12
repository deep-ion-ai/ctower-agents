#!/usr/bin/env bash
# Entrada do ambiente autônomo (#179). Uso: ct-autonomo serve|verificar [owner/repo]|<comando>
#
# serve      prepara o HOME (volume) e sobe o ctowerd em primeiro plano.
#            Na primeira subida, liga o ambiente ao hub como o
#            `provision.sh --env --hub-url`: grava hub_url/env_name no
#            config.toml e espera o token de sync, gravado pelo
#            `docker compose exec -T autonomo ctower hub set-token` (ou vindo
#            de CT_HUB_TOKEN). Depois disso, rodar de novo não troca o que já
#            foi gravado.
# verificar  confere login do Claude, do gh, conta do plano, statusline e
#            ligação ao hub. Com owner/repo, confere que o gh NÃO alcança esse
#            repositório (um fora do escopo do token).
#
# Credenciais, de /run/secrets/<nome em minúsculas> (secrets do compose) ou da
# variável de mesmo nome:
#   CLAUDE_CODE_OAUTH_TOKEN  login do Claude Code (`claude setup-token`); obrigatório
#   GH_TOKEN                 token fine-grained do gh, só com os repositórios
#                            dos projetos autônomos; obrigatório
#   CT_HUB_TOKEN             token de sync (`cthubd add-env <nome>`); opcional, só
#                            na 1ª subida: sem ele, o entrypoint espera o set-token
#   CT_HUB_AGENT_TOKEN       token de agente (`cthubd agent-token <nome>`); opcional
# O login do Claude e o do gh ficam só no ambiente do processo: nunca vão para
# arquivo no HOME. Os tokens do hub vão para ~/.config/ctower (0600), como em
# qualquer ambiente, e não passam para as sessões.
#
# Outras variáveis: CT_HUB_URL (1ª subida), CT_ENV_NAME (padrão
# linux-autonomo), CT_GIT_NAME e CT_GIT_EMAIL (autor dos commits),
# CT_AUTONOMO_SEM_PROMPTS (1 = sessões sem prompts de permissão; padrão 1),
# CTOWER_CLAUDE_ORG_ID e CTOWER_CLAUDE_EMAIL (conta do plano; veja
# docs/ambiente-autonomo.md). CT_SECRETS_DIR troca /run/secrets (testes).
#
# Nunca imprime tokens.
set -euo pipefail

ENV_NAME="${CT_ENV_NAME:-linux-autonomo}"
SECRETS_DIR="${CT_SECRETS_DIR:-/run/secrets}"
CT_CONF="${CTOWER_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/ctower}"
CLAUDE_DIR="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
CTOWER_BIN="$(command -v ctower || echo /usr/local/bin/ctower)"

log() { echo "[ct-autonomo] $*" >&2; }
die() { log "erro: $*"; exit 1; }

# Exporta NOME com o conteúdo de $SECRETS_DIR/<nome> (sem \r e sem as quebras
# de linha do fim), se o arquivo existir e não for vazio; senão fica a variável.
load_secret() {
  local name="$1" file value
  file="$SECRETS_DIR/$(tr '[:upper:]' '[:lower:]' <<< "$name")"
  if [[ -s $file ]]; then
    value="$(tr -d '\r' < "$file")"
    printf -v "$name" '%s' "$value"
  fi
  if [[ -n ${!name:-} ]]; then
    export "${name?}"
  else
    unset "$name"
  fi
}

valid_env_name() { [[ $1 =~ ^[A-Za-z0-9._-]+$ ]]; }

# Mescla um JSON (filtro do jq) num arquivo JSON do HOME, criando se faltar.
json_merge() {
  local file="$1" filter="$2" tmp
  mkdir -p "$(dirname "$file")"
  [[ -s $file ]] || echo '{}' > "$file"
  tmp="$(mktemp "$file.XXXXXX")"
  jq "$filter" "$file" > "$tmp"
  chmod 600 "$tmp"
  mv "$tmp" "$file"
}

load_all_secrets() {
  local n
  for n in CLAUDE_CODE_OAUTH_TOKEN GH_TOKEN CT_HUB_TOKEN CT_HUB_AGENT_TOKEN; do
    load_secret "$n"
  done
}

link_hub() {
  local cfg="$CT_CONF/config.toml"
  mkdir -p "$CT_CONF"
  chmod 700 "$CT_CONF"
  if [[ ! -f $cfg ]]; then
    local url="${CT_HUB_URL:-}"
    url="${url%/}"
    [[ $url =~ ^https?://[^[:space:]\"]+$ ]] \
      || die "CT_HUB_URL ausente ou inválida na primeira subida (ex.: http://hub.example.com:7879)"
    local tmp
    tmp="$(mktemp "$CT_CONF/.config.toml.XXXXXX")"
    cat > "$tmp" <<EOF
# Gerado por docker/autonomo/entrypoint.sh na primeira subida (#179).
env_name = "$ENV_NAME"
hub_url = "$url"
# Dentro do container; o compose.yml publica a porta só em 127.0.0.1 do host.
bind = "0.0.0.0"
port = 7878
EOF
    chmod 600 "$tmp"
    mv "$tmp" "$cfg"
    log "config do ambiente gravado em $cfg (hub $url)"
  fi
  if [[ ! -s $CT_CONF/hub.token && -n ${CT_HUB_TOKEN:-} ]]; then
    printf '%s\n' "$CT_HUB_TOKEN" | "$CTOWER_BIN" hub set-token > /dev/null \
      || die "o ctower recusou o token de sync (veja a mensagem acima)"
    log "token de sync gravado em $CT_CONF/hub.token"
  fi
  # Primeira subida sem o token: espera o set-token pelo `docker compose exec`.
  # Avisa de novo a cada 30 voltas (1 min com o passo padrão de 2 s).
  local n=0
  while [[ ! -s $CT_CONF/hub.token ]]; do
    if (( n++ % 30 == 0 )); then
      log "esperando o token de sync. No hub: cthubd add-env $ENV_NAME | docker compose exec -T autonomo ctower hub set-token (docs/ambiente-autonomo.md)"
    fi
    sleep "${CT_WAIT_STEP:-2}"
  done
  if [[ ! -s $CT_CONF/hub-agent.token && -n ${CT_HUB_AGENT_TOKEN:-} ]]; then
    if printf '%s\n' "$CT_HUB_AGENT_TOKEN" | "$CTOWER_BIN" hub set-agent-token > /dev/null; then
      log "token de agente gravado em $CT_CONF/hub-agent.token"
    else
      log "aviso: o ctower recusou o token de agente; o gestor de projetos fica sem ele"
    fi
  fi
}

# Claude Code sem as telas da primeira execução e, por padrão, sem prompts de
# permissão (o container é o isolamento). O statusline leva o uso ao hub.
# `hasSeenAutoDefaultNudge` cala a oferta "Make auto mode your default
# permission mode?", que o Claude Code mostra depois da primeira resposta
# quando o `defaultMode` do usuário não é `auto`: numa sessão sem ninguém no
# terminal, ela prenderia a sessão.
setup_claude() {
  json_merge "$HOME/.claude.json" '.hasCompletedOnboarding = true | .hasSeenAutoDefaultNudge = true'
  if [[ ${CT_AUTONOMO_SEM_PROMPTS:-1} == 1 ]]; then
    json_merge "$CLAUDE_DIR/settings.json" \
      '.permissions.defaultMode = "bypassPermissions" | .skipDangerousModePermissionPrompt = true'
  else
    json_merge "$CLAUDE_DIR/settings.json" \
      'if .permissions.defaultMode == "bypassPermissions" then del(.permissions.defaultMode) else . end | del(.skipDangerousModePermissionPrompt)'
  fi
  "$CTOWER_BIN" statusline install --ctower-bin "$CTOWER_BIN" > /dev/null \
    || log "aviso: não instalei o statusline; rode 'ct-autonomo verificar'"
}

setup_git() {
  # O helper chama `gh auth git-credential`, que lê o GH_TOKEN do ambiente:
  # o ~/.gitconfig não guarda token.
  gh auth setup-git > /dev/null 2>&1 || log "aviso: 'gh auth setup-git' falhou"
  if [[ -n ${CT_GIT_NAME:-} && -z $(git config --global user.name || true) ]]; then
    git config --global user.name "$CT_GIT_NAME"
  fi
  if [[ -n ${CT_GIT_EMAIL:-} && -z $(git config --global user.email || true) ]]; then
    git config --global user.email "$CT_GIT_EMAIL"
  fi
}

serve() {
  valid_env_name "$ENV_NAME" || die "CT_ENV_NAME inválido: \"$ENV_NAME\" (use só letras, números, '.', '_' e '-')"
  load_all_secrets
  [[ -n ${CLAUDE_CODE_OAUTH_TOKEN:-} ]] \
    || die "falta CLAUDE_CODE_OAUTH_TOKEN (gere com 'claude setup-token'; docs/ambiente-autonomo.md)"
  [[ -n ${GH_TOKEN:-} ]] \
    || die "falta GH_TOKEN (token fine-grained só com os repositórios dos projetos autônomos)"
  [[ -n ${CTOWER_CLAUDE_ORG_ID:-} ]] \
    || log "aviso: sem CTOWER_CLAUDE_ORG_ID, a conta do plano fica desconhecida no hub"
  link_hub
  setup_claude
  setup_git
  # Os tokens do hub já estão no config; as sessões não precisam deles.
  unset CT_HUB_TOKEN CT_HUB_AGENT_TOKEN
  log "ctowerd $(cat /usr/local/share/ctower-autonomo/VERSION 2>/dev/null || echo '?') como \"$ENV_NAME\""
  exec ctowerd
}

verificar() {
  local denied="${1:-}" rc=0 st
  load_all_secrets
  unset CT_HUB_TOKEN CT_HUB_AGENT_TOKEN
  # Só os campos sem dado pessoal.
  st="$(claude auth status 2>/dev/null)" || true
  jq -e . > /dev/null 2>&1 <<< "$st" || st='{}'
  echo "claude: $(jq -c '{loggedIn, authMethod}' <<< "$st")"
  [[ $(jq -r '.loggedIn // false' <<< "$st") == true ]] || rc=1
  if [[ -n $(jq -r '.orgId // empty' <<< "$st") ]]; then
    echo "conta do plano: a do claude auth status"
  elif [[ -n ${CTOWER_CLAUDE_ORG_ID:-} ]]; then
    echo "conta do plano: declarada (CTOWER_CLAUDE_ORG_ID${CTOWER_CLAUDE_EMAIL:+ e CTOWER_CLAUDE_EMAIL})"
    [[ -n ${CTOWER_CLAUDE_EMAIL:-} ]] || echo "  aviso: sem CTOWER_CLAUDE_EMAIL, a chave só junta com ambientes sem e-mail"
  else
    echo "conta do plano: desconhecida (defina CTOWER_CLAUDE_ORG_ID e CTOWER_CLAUDE_EMAIL)"
    rc=1
  fi
  local login
  if login="$(gh api user --jq .login 2>/dev/null)"; then
    echo "gh: autenticado como $login"
  else
    echo "gh: o GitHub recusou o GH_TOKEN (ou ele falta)"
    rc=1
  fi
  if [[ -n $denied ]]; then
    if gh api "repos/$denied" --silent 2>/dev/null; then
      echo "gh: ALCANÇA $denied (o token não está restrito)"
      rc=1
    else
      echo "gh: sem acesso a $denied (esperado)"
    fi
  fi
  if [[ -s $CT_CONF/hub.token ]]; then echo "hub: token de sync gravado"; else echo "hub: sem token de sync"; rc=1; fi
  echo "hub: $(grep -E '^(hub_url|env_name)' "$CT_CONF/config.toml" 2>/dev/null | tr '\n' ' ' || echo 'sem config')"
  if "$CTOWER_BIN" statusline install --ctower-bin "$CTOWER_BIN" > /dev/null 2>&1; then
    echo "statusline: instalado"
  else
    echo "statusline: falhou"
    rc=1
  fi
  "$CTOWER_BIN" status 2>/dev/null | head -5 || { echo "ctowerd: não respondeu"; rc=1; }
  return "$rc"
}

case "${1:-serve}" in
  serve) serve ;;
  verificar) shift; verificar "$@" ;;
  *)
    # Shell ou comando avulso (`docker compose exec autonomo ct-autonomo bash`):
    # com os logins do Claude e do gh, sem os tokens do hub.
    load_all_secrets
    unset CT_HUB_TOKEN CT_HUB_AGENT_TOKEN
    exec "$@"
    ;;
esac
