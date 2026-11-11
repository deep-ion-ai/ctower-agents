#!/usr/bin/env bash
# Entrada dos containers de teste (#176). Uso: ct-test-entrypoint hub|env|<comando>
#
# hub: grava o config do cthubd na primeira subida, gera os tokens do ambiente
#      de teste em /link (volume só do teste) e sobe o cthubd.
# env: espera os tokens do hub, liga o ctowerd a http://hub:7879 na primeira
#      subida e sobe o ctowerd.
# Os dois escutam em 0.0.0.0 dentro do container; o compose.yml publica as
# portas só em 127.0.0.1 do host.
#
# Nunca imprime tokens. Rodar de novo não troca o que já foi gravado.
set -euo pipefail

ENV_NAME="${CT_TEST_ENV_NAME:-ct-test-env}"
HUB_HOST_PORT="${CT_TEST_HUB_PORT:-17879}"
HUB_CONF="$HOME/.config/cthub"
ENV_CONF="$HOME/.config/ctower"
LINK=/link

log() { echo "[ct-test] $*" >&2; }

hub() {
  mkdir -p "$HUB_CONF"
  chmod 700 "$HUB_CONF"
  if [[ ! -f $HUB_CONF/config.toml ]]; then
    cat > "$HUB_CONF/config.toml" <<EOF
# Gerado por docker/test/entrypoint.sh (hub de teste).
name = "ct-test-hub"
test_mode = true
bind = "0.0.0.0"
port = 7879

[web]
enabled = true
origins = ["http://127.0.0.1:$HUB_HOST_PORT", "http://localhost:$HUB_HOST_PORT"]
EOF
    log "config do hub gravado em $HUB_CONF/config.toml"
  fi

  # Tokens do ambiente de teste: só na primeira vez. --no-replace sai com 3
  # se o nome já existe no hub (o ambiente já foi ligado antes).
  if [[ ! -s $LINK/env.token ]]; then
    local rc=0 tmp
    tmp="$(mktemp "$LINK/.env.token.XXXXXX")"
    cthubd add-env --no-replace "$ENV_NAME" > "$tmp" || rc=$?
    if [[ $rc -eq 0 ]]; then
      mv "$tmp" "$LINK/env.token"
      log "token de sync do ambiente \"$ENV_NAME\" gerado"
    else
      rm -f "$tmp"
      [[ $rc -eq 3 ]] || exit "$rc"
      log "o hub já tem o ambiente \"$ENV_NAME\"; para ligar de novo, apague os volumes (docker compose down -v)"
    fi
  fi
  if [[ ! -s $LINK/agent.token ]]; then
    local tmp
    tmp="$(mktemp "$LINK/.agent.token.XXXXXX")"
    if cthubd agent-token "$ENV_NAME" > "$tmp"; then
      mv "$tmp" "$LINK/agent.token"
      log "token de agente do ambiente \"$ENV_NAME\" gerado"
    else
      rm -f "$tmp"
      log "aviso: não gerei o token de agente"
    fi
  fi

  exec cthubd
}

env() {
  mkdir -p "$ENV_CONF"
  chmod 700 "$ENV_CONF"
  if [[ ! -s $ENV_CONF/hub.token ]]; then
    log "esperando o token de sync do hub..."
    local i
    for i in $(seq 1 120); do
      [[ -s $LINK/env.token ]] && break
      sleep 1
    done
    [[ -s $LINK/env.token ]] || { log "o hub não gerou o token em 2 min"; exit 1; }
    ctower hub set-token < "$LINK/env.token" > /dev/null
    log "token de sync gravado em $ENV_CONF/hub.token"
  fi
  if [[ ! -s $ENV_CONF/hub-agent.token && -s $LINK/agent.token ]]; then
    ctower hub set-agent-token < "$LINK/agent.token" > /dev/null
    log "token de agente gravado em $ENV_CONF/hub-agent.token"
  fi
  if [[ ! -f $ENV_CONF/config.toml ]]; then
    cat > "$ENV_CONF/config.toml" <<EOF
# Gerado por docker/test/entrypoint.sh (ambiente de teste).
env_name = "$ENV_NAME"
test_mode = true
bind = "0.0.0.0"
port = 7878
hub_url = "http://hub:7879"
EOF
    log "config do ambiente gravado em $ENV_CONF/config.toml"
  fi

  exec ctowerd
}

case "${1:-env}" in
  hub) hub ;;
  env) env ;;
  *) exec "$@" ;;
esac
