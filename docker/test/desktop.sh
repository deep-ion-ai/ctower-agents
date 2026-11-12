#!/usr/bin/env bash
# App desktop de teste (#176): roda o app do checkout (`tauri dev`) com outro
# identificador, ligado só ao hub e ao ambiente de docker/test. O app
# instalado continua aberto e com a configuração dele: os dados deste ficam
# em outra pasta (no macOS, ~/Library/Application Support/<identificador>).
#
# Uso (na raiz do repositório, com os containers no ar):
#   docker/test/desktop.sh            # grava a configuração, se faltar, e abre
#   docker/test/desktop.sh --reset    # regrava a configuração (tokens novos) e abre
#
# Variáveis: CT_TEST_HUB_PORT (17879), CT_TEST_ENV_PORT (17878),
# CT_TEST_APP_ID (ai.deep-ion.control-tower.teste).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE="$ROOT/docker/test/compose.yml"
APP_ID="${CT_TEST_APP_ID:-ai.deep-ion.control-tower.teste}"
HUB_PORT="${CT_TEST_HUB_PORT:-17879}"
ENV_PORT="${CT_TEST_ENV_PORT:-17878}"

case "$(uname -s)" in
  Darwin) DATA="$HOME/Library/Application Support/$APP_ID" ;;
  Linux) DATA="${XDG_DATA_HOME:-$HOME/.local/share}/$APP_ID" ;;
  *) echo "sistema não suportado: $(uname -s)" >&2; exit 1 ;;
esac

[[ $APP_ID == ai.deep-ion.control-tower ]] && { echo "CT_TEST_APP_ID não pode ser o do app de produção" >&2; exit 1; }

if docker compose version >/dev/null 2>&1; then
  dc() { docker compose -f "$COMPOSE" "$@"; }
elif command -v docker-compose >/dev/null; then
  dc() { docker-compose -f "$COMPOSE" "$@"; }
else
  echo "precisa do docker compose (ou docker-compose)" >&2
  exit 1
fi

if [[ ${1:-} == --reset || ! -s $DATA/envs.json || ! -s $DATA/hub.json ]]; then
  hub_token="$(dc exec -T hub cat .config/cthub/ui.token | tr -d '[:space:]')"
  env_token="$(dc exec -T env cat .config/ctower/ui.token | tr -d '[:space:]')"
  [[ -n $hub_token && -n $env_token ]] || { echo "suba os containers antes (README.md)" >&2; exit 1; }
  mkdir -p "$DATA"
  chmod 700 "$DATA"
  # Só o ambiente de teste: sem isto o app cairia na lista padrão, que aponta
  # para o ctowerd de verdade em 127.0.0.1:7878.
  ENV_TOKEN="$env_token" HUB_TOKEN="$hub_token" ENV_PORT="$ENV_PORT" HUB_PORT="$HUB_PORT" DATA="$DATA" \
    python3 - <<'PY'
import json, os
data = os.environ["DATA"]
def write(name, value):
    path = os.path.join(data, name)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        json.dump(value, f, indent=2)
write("envs.json", [{
    "id": "ct-test-env",
    "name": "ct-test-env (Docker)",
    "url": f"ws://127.0.0.1:{os.environ['ENV_PORT']}/ws",
    "kind": "remote",
    "distro": None,
    "token": os.environ["ENV_TOKEN"],
}])
write("hub.json", {"url": f"ws://127.0.0.1:{os.environ['HUB_PORT']}/ws", "token": os.environ["HUB_TOKEN"]})
PY
  echo "configuração de teste gravada em $DATA"
fi

cd "$ROOT/ui"
[[ -d node_modules ]] || npm ci --no-audit --no-fund
exec npx tauri dev --config "{\"identifier\":\"$APP_ID\",\"productName\":\"Control Tower (teste)\"}"
