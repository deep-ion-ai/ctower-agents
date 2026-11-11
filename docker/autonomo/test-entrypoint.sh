#!/usr/bin/env bash
# Teste do entrypoint do ambiente autônomo (#179), sem Docker: HOME temporário,
# o ctower de verdade (target/debug) e ctowerd, claude e gh falsos. Confere que
# (1) sem o login do Claude ou do gh o ambiente não sobe; (2) a primeira subida
# grava o config, espera o token de sync (`ctower hub set-token`), grava o de
# agente, instala o statusline e tira os prompts; (3) o login do Claude e o do gh chegam ao ctowerd, os tokens
# do hub não, e nenhum token fica em arquivo do HOME além dos do hub; (4) a
# segunda subida, sem os tokens do hub, não muda o que foi gravado.
#
# Uso: docker/autonomo/test-entrypoint.sh [pasta dos binários]   (padrão: target/debug)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="$(cd "${1:-$ROOT/target/debug}" && pwd)"
ENTRY="$ROOT/docker/autonomo/entrypoint.sh"
[[ -x $BIN/ctower ]] || { echo "falta $BIN/ctower (cargo build -p ctower)" >&2; exit 1; }
command -v jq >/dev/null || { echo "falta o jq" >&2; exit 1; }

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
F="$T/fake"
mkdir -p "$F" "$T/home" "$T/secrets"
ln -s "$BIN/ctower" "$F/ctower"
# ctowerd falso: guarda o ambiente que recebeu e sai.
cat > "$F/ctowerd" <<EOF
#!/bin/sh
env > "$T/ctowerd.env"
EOF
cat > "$F/claude" <<'EOF'
#!/bin/sh
echo '{"loggedIn": true, "authMethod": "oauth_token", "apiProvider": "firstParty"}'
EOF
cat > "$F/gh" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod +x "$F"/*

CLAUDE_TOK="sk-ant-oat01-falso-$RANDOM$RANDOM"
GH_TOK="github_pat_falso_$RANDOM$RANDOM"
SYNC_TOK="$(printf 'ab%.0s' {1..32})"
AGENT_TOK="cta_falso$RANDOM"

fail() { echo "FALHOU: $*" >&2; exit 1; }

run() {
  env -i PATH="$F:/usr/bin:/bin" HOME="$T/home" CT_SECRETS_DIR="$T/secrets" \
    CT_ENV_NAME=linux-autonomo CT_HUB_URL=http://hub.example.com:7879/ \
    CTOWER_CLAUDE_ORG_ID=conta-a CTOWER_CLAUDE_EMAIL=fulano@example.com \
    CT_GIT_NAME="Fulano de Tal" CT_GIT_EMAIL=fulano@example.com \
    "$@" bash "$ENTRY" serve
}

# (1) Sem os logins, não sobe.
if run > "$T/out" 2>&1; then fail "subiu sem CLAUDE_CODE_OAUTH_TOKEN"; fi
grep -q CLAUDE_CODE_OAUTH_TOKEN "$T/out" || fail "a mensagem não cita CLAUDE_CODE_OAUTH_TOKEN"
printf '%s\n' "$CLAUDE_TOK" > "$T/secrets/claude_code_oauth_token"
if run > "$T/out" 2>&1; then fail "subiu sem GH_TOKEN"; fi
grep -q GH_TOKEN "$T/out" || fail "a mensagem não cita GH_TOKEN"
[[ ! -e $T/home/.config/ctower/config.toml ]] || fail "gravou config sem os logins"

# (2) Primeira subida sem o token de sync: grava o config e espera o
# `ctower hub set-token` (o `docker compose exec` da doc).
C="$T/home/.config/ctower"
run GH_TOKEN="$GH_TOK" CT_HUB_AGENT_TOKEN="$AGENT_TOK" CT_WAIT_STEP=0.2 > "$T/out" 2>&1 &
pid=$!
for _ in $(seq 1 100); do grep -q "esperando o token de sync" "$T/out" && break; sleep 0.1; done
grep -q "cthubd add-env linux-autonomo | docker compose exec -T autonomo ctower hub set-token" "$T/out" \
  || { kill "$pid"; cat "$T/out" >&2; fail "não esperou o token de sync com a instrução"; }
[[ ! -e $T/ctowerd.env ]] || fail "subiu o ctowerd sem o token de sync"
printf '%s\r\n' "$SYNC_TOK" | HOME="$T/home" "$BIN/ctower" hub set-token > /dev/null
wait "$pid" || { cat "$T/out" >&2; fail "a primeira subida falhou depois do set-token"; }
grep -qx 'env_name = "linux-autonomo"' "$C/config.toml" || fail "env_name"
grep -qx 'hub_url = "http://hub.example.com:7879"' "$C/config.toml" || fail "hub_url"
grep -qx 'bind = "0.0.0.0"' "$C/config.toml" || fail "bind"
[[ "$(cat "$C/hub.token")" == "$SYNC_TOK" ]] || fail "hub.token"
[[ "$(cat "$C/hub-agent.token")" == "$AGENT_TOK" ]] || fail "hub-agent.token"
[[ "$(stat -c %a "$C/hub.token" 2>/dev/null || stat -f %Lp "$C/hub.token")" == 600 ]] || fail "hub.token sem 0600"
S="$T/home/.claude/settings.json"
jq -e '.statusLine.command | contains("statusline")' "$S" >/dev/null || fail "statusline não instalado"
jq -e '.permissions.defaultMode == "bypassPermissions" and .skipDangerousModePermissionPrompt' "$S" >/dev/null \
  || fail "prompts de permissão não desligados"
jq -e '.hasCompletedOnboarding' "$T/home/.claude.json" >/dev/null || fail "onboarding"
[[ "$(HOME="$T/home" git config --global user.email)" == fulano@example.com ]] || fail "git user.email"

# (3) O que chega ao ctowerd e o que fica em arquivo.
E="$T/ctowerd.env"
grep -qx "CLAUDE_CODE_OAUTH_TOKEN=$CLAUDE_TOK" "$E" || fail "o ctowerd não recebeu o login do Claude"
grep -qx "GH_TOKEN=$GH_TOK" "$E" || fail "o ctowerd não recebeu o GH_TOKEN"
grep -qx "CTOWER_CLAUDE_ORG_ID=conta-a" "$E" || fail "o ctowerd não recebeu a conta declarada"
if grep -q '^CT_HUB_TOKEN=\|^CT_HUB_AGENT_TOKEN=' "$E"; then fail "os tokens do hub chegaram ao ctowerd"; fi
if grep -rqF -e "$CLAUDE_TOK" -e "$GH_TOK" "$T/home"; then fail "um login ficou gravado no HOME"; fi
if grep -qF -e "$CLAUDE_TOK" -e "$GH_TOK" -e "$SYNC_TOK" -e "$AGENT_TOK" "$T/out"; then fail "um token apareceu na saída"; fi

# (4) Segunda subida sem os tokens do hub e sem os prompts desligados.
rm "$E"
cp "$C/config.toml" "$T/config.antes"
run GH_TOKEN="$GH_TOK" CT_AUTONOMO_SEM_PROMPTS=0 > "$T/out" 2>&1 || { cat "$T/out" >&2; fail "a segunda subida falhou"; }
cmp -s "$C/config.toml" "$T/config.antes" || fail "a segunda subida mudou o config.toml"
[[ "$(cat "$C/hub.token")" == "$SYNC_TOK" ]] || fail "a segunda subida mudou o hub.token"
jq -e '(.permissions.defaultMode // "") != "bypassPermissions" and (has("skipDangerousModePermissionPrompt") | not)' "$S" >/dev/null \
  || fail "CT_AUTONOMO_SEM_PROMPTS=0 não religou os prompts"
jq -e '.statusLine.command | contains("statusline")' "$S" >/dev/null || fail "statusline sumiu"
[[ -s $E ]] || fail "o ctowerd não subiu na segunda vez"

echo "ok: entrypoint do ambiente autônomo"
