#!/usr/bin/env bash
# Provisiona uma máquina do Control Tower. Idempotente: pode rodar várias vezes.
#
# Uso: provision.sh [--env] [--hub] [--env-name NOME] [--hub-url URL] [--skip-packages] [--from-source]
#                     [--docs-dir PASTA] [--docs-samba]
#
#   (sem opção)   só o ambiente (ctowerd + ctower), como sempre foi
#   --env         o ambiente (ctowerd + ctower): sessões, hooks e MCP
#   --hub         o hub (cthubd): projetos, atribuições e hub.db
#   --hub --env   máquina única: os dois, já ligados. O ambiente recebe um
#                 token de sync (`cthubd add-env <nome>`) em hub.token, o
#                 token de agente (`cthubd agent-token <nome>`, para o
#                 gestor de projetos) em hub-agent.token e
#                 `hub_url = "http://127.0.0.1:7879"` no config.toml
#   --hub-url     (com --env, sem --hub) liga o ambiente a um hub em OUTRA máquina,
#                 ex.: http://192.0.2.10:7879. O token de sync, gerado no hub com
#                 `cthubd add-env <nome>`, vem da variável CT_HUB_TOKEN ou de
#                 --hub-token-file ARQ ('-' lê do stdin); nunca como argumento.
#                 Grava hub.token e hub_url/env_name no config.toml sem trocar o que já existe
#   --hub-token-file ARQ  arquivo (ou '-') com o token de sync do ambiente
#   --env-name    nome do ambiente no hub (padrão: env_name do config.toml
#                 do ctowerd ou o nome da máquina)
#   --from-source compila ctowerd/ctower do fonte (cargo install) em vez de baixar
#                 o binário da release (sha256 conferido); é também o fallback
#                 quando a release não tem asset para a plataforma
#   --skip-packages  não instala pacotes do sistema (apt, gh, rustup, node,
#                 claude); só compila, instala e liga os serviços. O hub ainda
#                 exige Node ≥ 20 com npm (o build da UI web embutida, #199)
#   --docs-dir PASTA  (com --hub) cria a pasta única dos docs dos projetos (#443)
#                 e grava docs_dir no config.toml do hub, se ainda vazio. Com
#                 --hub --env, grava também docs_root = PASTA no ambiente daqui
#   --docs-samba  (com --hub, só Linux) compartilha o docs_dir pelo Samba como
#                 [ctdocs]: usuário próprio do Samba (ctdocs), arquivos gravados
#                 como o usuário do hub e acesso só da rede local (hosts allow).
#                 A senha vem de CT_DOCS_SAMBA_PASSWORD ou é pedida no terminal;
#                 nunca como argumento. Um usuário que já existe no Samba fica
#                 com a senha que tem. Veja docs/hub.md, "Docs dos projetos"
#
# No macOS os serviços são LaunchAgents (packaging/launchd) e o binário do ambiente vem da release
# (sem Rust); o hub compila com o rustup. Valem --hub e --env (ctowerd). O LaunchAgent sobe no login:
# para voltar sozinho depois de um reboot, ligue o login automático do usuário
# (docs/ambiente-macos.md). Detalhes do hub em docs/hub.md.
#
# Variáveis: CTOWER_REPO (clone deste repositório, padrão
# ~/projetos/control-tower). Para testar sem mexer no sistema: CT_SUDO,
# CT_SYSTEMCTL, CT_LAUNCHCTL e CT_CARGO trocam sudo, systemctl, launchctl e
# cargo por outros comandos, e CT_OS (Linux/Darwin) troca o sistema detectado.
# Samba: CT_DOCS_SAMBA_USER (padrão ctdocs), CT_DOCS_SAMBA_HOSTS (hosts allow,
# padrão: loopback e faixas privadas IPv4/IPv6; some 100.64.0.0/10 para a
# tailnet) e, para testes, CT_SAMBA_DIR (padrão /etc/samba).
set -euo pipefail

WANT_ENV=0
WANT_HUB=0
SKIP_PACKAGES=0
FROM_SOURCE=0
ENV_NAME_ARG=""
HUB_URL_ARG=""
HUB_TOKEN_FILE=""
DOCS_DIR_ARG=""
WANT_DOCS_SAMBA=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --env) WANT_ENV=1 ;;
    --from-source) FROM_SOURCE=1 ;;
    --hub) WANT_HUB=1 ;;
    --env-name) ENV_NAME_ARG="${2:?--env-name precisa de um nome}"; shift ;;
    --hub-url) HUB_URL_ARG="${2:?--hub-url precisa de uma URL}"; shift ;;
    --hub-token-file) HUB_TOKEN_FILE="${2:?--hub-token-file precisa de um arquivo}"; shift ;;
    --skip-packages) SKIP_PACKAGES=1 ;;
    --docs-dir) DOCS_DIR_ARG="${2:?--docs-dir precisa de uma pasta}"; shift ;;
    --docs-samba) WANT_DOCS_SAMBA=1 ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) echo "opção desconhecida: $1 (veja --help)" >&2; exit 2 ;;
  esac
  shift
done
if [[ -n $ENV_NAME_ARG && ! $ENV_NAME_ARG =~ ^[A-Za-z0-9._-]+$ ]]; then
  echo "--env-name inválido: \"$ENV_NAME_ARG\" (use só letras, números, '.', '_' e '-')." >&2
  exit 2
fi
if [[ $WANT_ENV -eq 0 && $WANT_HUB -eq 0 ]]; then
  WANT_ENV=1
fi
if [[ -n $HUB_URL_ARG ]]; then
  if [[ $WANT_HUB -eq 1 ]]; then
    echo "--hub-url é para um hub em outra máquina; com --hub o ambiente liga ao hub local." >&2
    exit 2
  fi
  if [[ ! $HUB_URL_ARG =~ ^https?://[^[:space:]\"]+$ ]]; then
    echo "--hub-url inválida: \"$HUB_URL_ARG\" (use http://<ip>:7879)." >&2
    exit 2
  fi
  HUB_URL_ARG="${HUB_URL_ARG%/}"
elif [[ -n $HUB_TOKEN_FILE ]]; then
  echo "--hub-token-file só vale com --hub-url." >&2
  exit 2
fi

if [[ ( -n $DOCS_DIR_ARG || $WANT_DOCS_SAMBA -eq 1 ) && $WANT_HUB -eq 0 ]]; then
  echo "--docs-dir e --docs-samba são do hub: use com --hub (os docs ficam na máquina do cthubd)." >&2
  exit 2
fi

if [[ $EUID -eq 0 ]]; then
  echo "Rode como usuário comum (o script usa sudo quando precisa)." >&2
  exit 1
fi

OS="${CT_OS:-$(uname -s)}"

SUDO="${CT_SUDO:-sudo}"
SYSTEMCTL="${CT_SYSTEMCTL:-systemctl}"
LAUNCHCTL="${CT_LAUNCHCTL:-launchctl}"
CARGO="${CT_CARGO:-$HOME/.cargo/bin/cargo}"
# Node e npm do build da UI web que o cthubd embute (#199).
NODE="${CT_NODE:-node}"
NPM="${CT_NPM:-npm}"
MIN_NODE_MAJOR=20
GH="${CT_GH:-gh}"
RELEASE_REPO="${CTOWER_RELEASE_REPO:-deep-ion-ai/ctower-agents}"
BIN="$HOME/.cargo/bin"
REPO="${CTOWER_REPO:-$HOME/projetos/control-tower}"
# Os mesmos diretórios que ctowerd/ctower e cthubd usam.
CT_CONF="${CTOWER_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/ctower}"
CT_DATA="${CTOWER_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/ctower}"
HUB_CONF="${CTHUB_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/cthub}"
LAUNCHD_LABEL="ai.deep-ion.cthubd"
# Compartilhamento Samba da pasta de docs (#443).
SAMBA_DIR="${CT_SAMBA_DIR:-/etc/samba}"
DOCS_SHARE="ctdocs"
DOCS_SMB_USER="${CT_DOCS_SAMBA_USER:-ctdocs}"
DOCS_SMB_HOSTS="${CT_DOCS_SAMBA_HOSTS:-127.0.0.1 ::1 10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 fe80::/10 fd00::/8}"
ENV_LAUNCHD_LABEL="ai.deep-ion.ctowerd"

# Valor de uma chave de topo (`chave = "valor"` ou `chave = 123`) de um TOML
# simples; vazio se não houver. Para na primeira tabela ([...]).
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

# O ctowerd do #64 com `hub = true` recusa subir: a troca é o `cthubd migrate`,
# que tira o `hub = true` do config. Depois dele, este script sobe tudo
# normalmente (o ambiente já vem ligado: hub_url e hub.token gravados), mas
# troca os binários do ctowerd sem guardar os anteriores: na migração, o
# roteiro usa `ctower update`, que permite `ctower update --rollback`.
legacy_hub() {
  [[ "$(toml_get "$CT_CONF/config.toml" hub)" == "true" ]]
}

install_packages_linux() {
  # Isola a distro do PATH do Windows (evita usar gh.exe/node.exe do host). Vale após `wsl --terminate`.
  if grep -qsi microsoft /proc/version && ! grep -qs '^appendWindowsPath' /etc/wsl.conf; then
    printf '[interop]\nappendWindowsPath=false\n' | $SUDO tee -a /etc/wsl.conf >/dev/null
  fi

  $SUDO apt-get update -y
  $SUDO apt-get install -y build-essential pkg-config git curl ca-certificates unzip jq

  if ! command -v gh >/dev/null; then
    $SUDO mkdir -p -m 755 /etc/apt/keyrings
    curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
      | $SUDO tee /etc/apt/keyrings/githubcli-archive-keyring.gpg >/dev/null
    $SUDO chmod go+r /etc/apt/keyrings/githubcli-archive-keyring.gpg
    echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
      | $SUDO tee /etc/apt/sources.list.d/github-cli.list >/dev/null
    $SUDO apt-get update -y
    $SUDO apt-get install -y gh
  fi

  # O rustup só serve ao hub e ao build do fonte; o ambiente baixa o binário.
  if [[ $WANT_HUB -eq 1 || $FROM_SOURCE -eq 1 ]]; then
    install_rustup
  fi

  # Node no ambiente (Claude Code) e no hub (build da UI web embutida, #199).
  if [[ $WANT_ENV -eq 1 || $WANT_HUB -eq 1 ]] && ! node_ok; then
    curl -fsSL https://deb.nodesource.com/setup_22.x | $SUDO -E bash -
    $SUDO apt-get install -y nodejs
  fi
  # Claude Code só no ambiente (o hub não roda agentes).
  if [[ $WANT_ENV -eq 1 ]]; then
    if ! command -v claude >/dev/null && [[ ! -x "$HOME/.local/bin/claude" ]]; then
      curl -fsSL https://claude.ai/install.sh | bash
    fi
  fi
}

install_packages_macos() {
  if ! xcode-select -p >/dev/null 2>&1; then
    echo "Instale as Xcode Command Line Tools (xcode-select --install) e rode de novo." >&2
    exit 1
  fi
  # O rustup só serve ao hub e ao build do fonte; o ambiente baixa o binário.
  if [[ $WANT_HUB -eq 1 || $FROM_SOURCE -eq 1 ]]; then
    install_rustup
  fi
  if ! command -v gh >/dev/null; then
    echo "Aviso: gh não encontrado (brew install gh). Sem ele, grave o token do GitHub do hub pela UI (tela Hub)." >&2
  fi
  # Node no ambiente (Claude Code) e no hub (build da UI web embutida, #199).
  if [[ $WANT_ENV -eq 1 || $WANT_HUB -eq 1 ]] && ! node_ok; then
    if command -v brew >/dev/null; then
      brew install node
    else
      echo "Aviso: Node ≥ $MIN_NODE_MAJOR não encontrado (brew install node); o Claude Code, os jobs e o build do hub dependem dele." >&2
    fi
  fi
  # Claude Code só no ambiente (o hub não roda agentes).
  if [[ $WANT_ENV -eq 1 ]]; then
    if ! command -v claude >/dev/null && [[ ! -x "$HOME/.local/bin/claude" ]]; then
      curl -fsSL https://claude.ai/install.sh | bash
    fi
  fi
}

install_rustup() {
  if [[ ! -x "$HOME/.cargo/bin/cargo" ]]; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal -c clippy -c rustfmt
  fi
}

# Node ≥ MIN_NODE_MAJOR e npm presentes.
node_ok() {
  local v major
  v="$("$NODE" --version 2>/dev/null || true)"
  major="${v#v}"
  major="${major%%.*}"
  [[ $major =~ ^[0-9]+$ ]] && ((major >= MIN_NODE_MAJOR)) && "$NPM" --version >/dev/null 2>&1
}

# O hub embute a UI web: sem Node, para antes de instalar qualquer coisa.
require_node_for_hub() {
  if ! node_ok; then
    echo "O hub (cthubd) embute a UI web, e o build dela precisa do Node $MIN_NODE_MAJOR ou mais novo com o npm" >&2
    echo "(encontrado: $("$NODE" --version 2>/dev/null || echo nenhum)). Instale o Node (brew install node, ou o" >&2
    echo "NodeSource no Linux; rode sem --skip-packages para instalar) e rode de novo. Veja docs/hub.md." >&2
    exit 1
  fi
}

# Hub: bundle web, binário cthubd e serviço (systemd de usuário ou LaunchAgent).
install_hub() {
  (cd "$REPO/ui" && "$NPM" ci --no-audit --no-fund && "$NPM" run build:web)
  CTHUB_WEB_REQUIRED=1 "$CARGO" install --locked --path "$REPO/crates/cthubd"
  if [[ $OS == Darwin ]]; then
    local agents="$HOME/Library/LaunchAgents" plist
    plist="$agents/$LAUNCHD_LABEL.plist"
    mkdir -p "$agents" "$HOME/Library/Logs"
    sed "s|@HOME@|$HOME|g" "$REPO/packaging/launchd/$LAUNCHD_LABEL.plist" > "$plist"
    if legacy_hub; then
      HUB_DEFERRED=1
    elif ! $LAUNCHCTL print "gui/$(id -u)/$LAUNCHD_LABEL" >/dev/null 2>&1; then
      $LAUNCHCTL bootstrap "gui/$(id -u)" "$plist"
    fi
  else
    mkdir -p "$HOME/.config/systemd/user"
    install -m 644 "$REPO/packaging/systemd/cthubd.service" "$HOME/.config/systemd/user/cthubd.service"
    $SUDO loginctl enable-linger "$USER"
    $SYSTEMCTL --user daemon-reload
    if legacy_hub; then
      # Subir um hub vazio agora atrapalharia o `cthubd migrate` (#182).
      HUB_DEFERRED=1
    else
      # Não reinicia um cthubd já rodando: para aplicar um binário novo,
      # use `cthubd update` (ou `systemctl --user restart cthubd`).
      $SYSTEMCTL --user enable --now cthubd
    fi
  fi
}

# Baixa os binários do ambiente da release da versão do checkout (#279),
# conferindo o sha256, e os instala em $BIN. Falha (retorno 1) se não há
# asset para a plataforma ou o download não confere; quem chama cai para o
# `cargo install`.
install_env_from_release() {
  local ver arch os tag name tmp
  ver="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$REPO/Cargo.toml" | head -1)"
  [[ -n $ver ]] || return 1
  arch="$(uname -m)"
  [[ $arch == arm64 ]] && arch=aarch64
  os=linux
  [[ $OS == Darwin ]] && os=macos
  tag="v$ver"
  name="ctower-env-$tag-$arch-$os"
  command -v "$GH" >/dev/null || return 1
  tmp="$(mktemp -d)"
  if ! "$GH" release download "$tag" --repo "$RELEASE_REPO" --dir "$tmp" \
      --pattern "$name.tar.gz" --pattern "$name.tar.gz.sha256" >/dev/null 2>&1; then
    rm -rf "$tmp"
    return 1
  fi
  if ! (cd "$tmp" && { command -v sha256sum >/dev/null && sha256sum -c "$name.tar.gz.sha256" \
      || shasum -a 256 -c "$name.tar.gz.sha256"; } >/dev/null 2>&1); then
    echo "Aviso: sha256 de $name.tar.gz não confere; descartei o download." >&2
    rm -rf "$tmp"
    return 1
  fi
  if ! tar -xzf "$tmp/$name.tar.gz" -C "$tmp" || [[ ! -f $tmp/$name/ctowerd || ! -f $tmp/$name/ctower ]]; then
    rm -rf "$tmp"
    return 1
  fi
  mkdir -p "$BIN"
  install -m 755 "$tmp/$name/ctowerd" "$BIN/ctowerd"
  install -m 755 "$tmp/$name/ctower" "$BIN/ctower"
  rm -rf "$tmp"
  echo "ctowerd e ctower $tag instalados da release."
}

# Ambiente: binários ctowerd/ctower e a unidade (ainda sem subir). Da
# release; compila do fonte com --from-source ou sem asset para a plataforma.
install_env_bins() {
  if [[ $FROM_SOURCE -eq 1 ]] || ! install_env_from_release; then
    [[ $FROM_SOURCE -eq 1 ]] || echo "Sem binário de release para esta plataforma; compilando do fonte." >&2
    [[ -x $CARGO ]] || install_rustup
    "$CARGO" install --locked --path "$REPO/crates/ctowerd"
    "$CARGO" install --locked --path "$REPO/crates/ctower"
  fi
  if [[ $OS == Darwin ]]; then
    mkdir -p "$HOME/Library/LaunchAgents" "$HOME/Library/Logs"
    sed "s|@HOME@|$HOME|g" "$REPO/packaging/launchd/$ENV_LAUNCHD_LABEL.plist" \
      > "$HOME/Library/LaunchAgents/$ENV_LAUNCHD_LABEL.plist"
  else
    mkdir -p "$HOME/.config/systemd/user"
    install -m 644 "$REPO/packaging/systemd/ctowerd.service" "$HOME/.config/systemd/user/ctowerd.service"
  fi
}

# Porta do hub: `port` do config do cthubd, depois CTHUB_PORT, por fim 7879.
hub_port() {
  local port
  port="$(toml_get "$HUB_CONF/config.toml" port)"
  echo "${CTHUB_PORT:-${port:-7879}}"
}

# Máquina única: liga o ambiente ao hub local. Não troca um token que já
# existe nem um hub_url já configurado; ENV_LINK_CHANGED=1 se gravou algo.
link_local_env() {
  local cfg="$CT_CONF/config.toml" tokf="$CT_CONF/hub.token" agentf="$CT_CONF/hub-agent.token"
  local port url cur name
  port="$(hub_port)"
  url="http://127.0.0.1:$port"
  cur="$(toml_get "$cfg" hub_url)"
  cur="${cur%/}"
  if [[ -n $cur && $cur != "$url" && $cur != "http://localhost:$port" ]]; then
    echo "Aviso: o ambiente já aponta para outro hub ($cur); não o liguei ao hub local." >&2
    return 0
  fi
  name="$(toml_get "$cfg" env_name)"
  if [[ -n $ENV_NAME_ARG && -n $name && $name != "$ENV_NAME_ARG" ]]; then
    echo "Aviso: env_name = \"$name\" no config do ctowerd; mantido (ignorei --env-name $ENV_NAME_ARG)." >&2
  fi
  local name_in_cfg="$name"
  name="${name:-${ENV_NAME_ARG:-$(hostname -s 2>/dev/null || hostname)}}"
  if ! valid_env_name "$name"; then
    echo "Nome de ambiente inválido: \"$name\" (use só letras, números, '.', '_' e '-'; passe --env-name ou ajuste env_name em $cfg)." >&2
    return 1
  fi
  ct_mkdir "$CT_CONF"

  if [[ -s $tokf ]]; then
    echo "hub.token do ambiente já existe; mantido (se o hub recusar, gere outro com 'cthubd add-env $name')."
  else
    local token err rc=0
    err="$(mktemp)"
    # --no-replace: um nome que já existe no hub é de outro ambiente (ou de
    # um token perdido); trocá-lo cortaria o envio dele.
    # O token nasce preso a um env_id (#195): o do ambiente, se o ctowerd já
    # tem um ledger aqui; senão o hub gera um. A saída tem duas linhas (token
    # e env_id) e o `ctower hub set-token` grava as duas.
    local idarg=()
    [[ -s $CT_CONF/env_id ]] && idarg=(--env-id "$(tr -d '[:space:]' < "$CT_CONF/env_id")")
    token="$("$BIN/cthubd" add-env --no-replace ${idarg[@]+"${idarg[@]}"} "$name" 2>"$err")" || rc=$?
    if [[ $rc -eq 3 ]]; then
      rm -f "$err"
      echo "Aviso: o hub já tem um ambiente \"$name\"; não troquei o token dele nem liguei este ambiente." >&2
      echo "  Use outro nome (--env-name) ou, se for este mesmo ambiente, gere outro token com" >&2
      echo "  'cthubd add-env $name | ctower hub set-token' e configure hub_url = \"$url\" (docs/hub.md)." >&2
      return 0
    elif [[ $rc -ne 0 ]]; then
      cat "$err" >&2
      rm -f "$err"
      return 1
    fi
    rm -f "$err"
    printf '%s\n' "$token" | "$BIN/ctower" hub set-token >/dev/null # token + env_id
    echo "Token de sync do ambiente \"$name\" gerado no hub e gravado em $tokf."
    ENV_LINK_CHANGED=1
  fi

  # Token de agente (gestor de projetos): o ctowerd lê o arquivo a cada
  # chamada, então gravá-lo não pede restart. Já gravado = mantido.
  if [[ -s $agentf ]]; then
    echo "hub-agent.token do ambiente já existe; mantido."
  else
    local agent
    if agent="$("$BIN/cthubd" agent-token "$name" 2>/dev/null)"; then
      printf '%s\n' "$agent" | "$BIN/ctower" hub set-agent-token >/dev/null
      echo "Token de agente do ambiente \"$name\" gerado no hub e gravado em $agentf."
    else
      echo "Aviso: não gerei o token de agente; no hub, rode 'cthubd agent-token $name | ctower hub set-agent-token'." >&2
    fi
  fi

  if [[ -z $cur ]]; then
    # Chaves de topo vão no começo do arquivo (antes de qualquer [tabela]).
    local tmp
    tmp="$(mktemp "$CT_CONF/.config.toml.XXXXXX")"
    {
      echo "# Ligado ao hub local por provision.sh --hub --env."
      [[ -z $name_in_cfg ]] && printf 'env_name = "%s"\n' "$name"
      printf 'hub_url = "%s"\n' "$url"
      if [[ -f $cfg ]]; then
        echo
        cat "$cfg"
      fi
    } > "$tmp"
    chmod 600 "$tmp"
    mv "$tmp" "$cfg"
    echo "hub_url = \"$url\" gravado em $cfg."
    ENV_LINK_CHANGED=1
  fi
}

# Hub em outra máquina: grava o token de sync (gerado no hub) e hub_url/env_name.
# Não troca token nem hub_url que já existem; ENV_LINK_CHANGED=1 se gravou algo.
link_remote_env() {
  local cfg="$CT_CONF/config.toml" tokf="$CT_CONF/hub.token"
  local cur name name_in_cfg token=""
  cur="$(toml_get "$cfg" hub_url)"
  cur="${cur%/}"
  if [[ -n $cur && $cur != "$HUB_URL_ARG" ]]; then
    echo "Aviso: o ambiente já aponta para outro hub ($cur); não o religuei a $HUB_URL_ARG." >&2
    return 0
  fi
  name_in_cfg="$(toml_get "$cfg" env_name)"
  name="${name_in_cfg:-${ENV_NAME_ARG:-$(hostname -s 2>/dev/null || hostname)}}"
  if ! valid_env_name "$name"; then
    echo "Nome de ambiente inválido: \"$name\" (passe --env-name ou ajuste env_name em $cfg)." >&2
    return 1
  fi
  ct_mkdir "$CT_CONF"
  if [[ -s $tokf ]]; then
    echo "hub.token do ambiente já existe; mantido."
  else
    if [[ -n $HUB_TOKEN_FILE ]]; then
      if [[ $HUB_TOKEN_FILE == - ]]; then token="$(cat)"; else token="$(cat "$HUB_TOKEN_FILE")"; fi
    else
      token="${CT_HUB_TOKEN:-}"
    fi
    token="${token//[$'\r\n ']/}"
    if [[ -z $token ]]; then
      echo "Falta o token de sync: no hub, 'cthubd add-env $name'; aqui, CT_HUB_TOKEN=<token> ou --hub-token-file." >&2
      return 1
    fi
    printf '%s\n' "$token" | "$BIN/ctower" hub set-token >/dev/null
    echo "Token de sync gravado em $tokf."
    ENV_LINK_CHANGED=1
  fi
  if [[ -z $cur ]]; then
    local tmp
    tmp="$(mktemp "$CT_CONF/.config.toml.XXXXXX")"
    {
      echo "# Ligado ao hub $HUB_URL_ARG por provision.sh --env --hub-url."
      [[ -z $name_in_cfg ]] && printf 'env_name = "%s"\n' "$name"
      printf 'hub_url = "%s"\n' "$HUB_URL_ARG"
      if [[ -f $cfg ]]; then
        echo
        cat "$cfg"
      fi
    } > "$tmp"
    chmod 600 "$tmp"
    mv "$tmp" "$cfg"
    echo "hub_url = \"$HUB_URL_ARG\" gravado em $cfg."
    ENV_LINK_CHANGED=1
  fi
}

# Nome de ambiente aceito na ligação automática (vai cru no TOML e no hub).
valid_env_name() {
  [[ $1 =~ ^[A-Za-z0-9._-]+$ ]]
}

ct_mkdir() {
  mkdir -p "$1"
  chmod 700 "$1"
}

start_env_macos() {
  local domain="gui/$(id -u)" svc was_active=0
  svc="$domain/$ENV_LAUNCHD_LABEL"
  if $LAUNCHCTL print "$svc" >/dev/null 2>&1; then
    was_active=1
  else
    $LAUNCHCTL bootstrap "$domain" "$HOME/Library/LaunchAgents/$ENV_LAUNCHD_LABEL.plist"
  fi
  if [[ $was_active -eq 1 && ${ENV_LINK_CHANGED:-0} -eq 1 ]]; then
    # Não reinicia um ctowerd já rodando sem checar: `ctower restart` recusa com sessões vivas.
    if ! "$BIN/ctower" restart; then
      echo "Aviso: rode 'ctower restart' quando puder, para o ctowerd usar o hub local." >&2
    fi
  fi
}

start_env() {
  if [[ $OS == Darwin ]]; then
    start_env_macos
    return
  fi
  local was_active=0
  if $SYSTEMCTL --user is-active --quiet ctowerd 2>/dev/null; then
    was_active=1
  fi
  # Mantém o serviço de usuário vivo sem sessão de login aberta.
  $SUDO loginctl enable-linger "$USER"
  $SYSTEMCTL --user daemon-reload
  # Não reinicia um ctowerd já rodando (mataria sessões): para aplicar um
  # binário novo, use `ctower restart`.
  $SYSTEMCTL --user enable --now ctowerd
  if [[ $was_active -eq 1 && ${ENV_LINK_CHANGED:-0} -eq 1 ]]; then
    # O config mudou com o daemon no ar: reinicia, mas recusa com sessões vivas.
    if ! CTOWER_SYSTEMCTL="$SYSTEMCTL" "$BIN/ctower" restart; then
      echo "Aviso: rode 'ctower restart' quando puder, para o ctowerd usar o hub local." >&2
    fi
  fi
}

# --- docs dos projetos (#443) -------------------------------------------------

# `~` no começo vira o HOME (o cthubd e o ctowerd expandem igual).
# shellcheck disable=SC2088 # o til literal é o que se procura
expand_tilde() {
  local p="$1"
  if [[ $p == "~" ]]; then
    p="$HOME"
  elif [[ $p == "~/"* ]]; then
    p="$HOME/${p#"~/"}"
  fi
  printf '%s' "$p"
}

# Caminho aceito para a pasta de docs: absoluto, sem espaços nem aspas (vai
# cru no TOML e no smb.conf) e nunca a raiz.
valid_docs_dir() {
  [[ $1 =~ ^/[A-Za-z0-9._/+@-]+$ && $1 != / ]]
}

# Grava `chave = "valor"` no topo de um TOML simples: troca a linha da chave
# se ela já existe antes da primeira [tabela] (ex.: `docs_dir = ""`), senão
# põe no começo do arquivo, com um comentário. O arquivo fica com 0600.
toml_set_top() {
  local file="$1" key="$2" val="$3" note="$4" dir tmp n=""
  dir="$(dirname "$file")"
  mkdir -p "$dir"
  if [[ -f $file ]]; then
    n="$(awk -v k="$key" '
      /^[[:space:]]*\[/ { exit }
      {
        l = $0
        sub(/^[[:space:]]+/, "", l)
        if (index(l, k) == 1 && substr(l, length(k) + 1) ~ /^[[:space:]]*=/) { print NR; exit }
      }' "$file")"
  fi
  tmp="$(mktemp "$dir/.config.toml.XXXXXX")"
  if [[ -n $n ]]; then
    awk -v n="$n" -v line="$key = \"$val\"" 'NR == n { print line; next } { print }' "$file" > "$tmp"
  else
    {
      echo "# $note"
      printf '%s = "%s"\n' "$key" "$val"
      if [[ -f $file ]]; then
        echo
        cat "$file"
      fi
    } > "$tmp"
  fi
  chmod 600 "$tmp"
  mv "$tmp" "$file"
}

# Pasta de docs do hub: cria a de --docs-dir e grava docs_dir no config do
# cthubd se ele ainda não tem um. Um docs_dir diferente já configurado fica
# como está (com aviso). Deixa em DOCS_DIR a pasta em uso.
setup_docs_dir() {
  local cfg="$HUB_CONF/config.toml" cur
  cur="$(expand_tilde "$(toml_get "$cfg" docs_dir)")"
  if [[ -z $DOCS_DIR_ARG ]]; then
    DOCS_DIR="$cur"
  elif [[ -n $cur && $cur != "$DOCS_DIR_ARG" ]]; then
    echo "Aviso: docs_dir = \"$cur\" no config do hub; mantido (ignorei --docs-dir $DOCS_DIR_ARG)." >&2
    DOCS_DIR="$cur"
  else
    DOCS_DIR="$DOCS_DIR_ARG"
  fi
  DOCS_DIR="${DOCS_DIR%/}"
  if ! valid_docs_dir "$DOCS_DIR"; then
    echo "docs_dir inválido: \"$DOCS_DIR\" (caminho absoluto, só letras, números e . _ / + @ -)." >&2
    return 1
  fi
  if [[ ! -d $DOCS_DIR ]]; then
    # Fora do HOME (ex.: /srv/ct-docs) o usuário comum não cria: via sudo,
    # já com dono e grupo do usuário do hub.
    if ! mkdir -p "$DOCS_DIR" 2>/dev/null; then
      $SUDO install -d -o "$USER" -g "$(id -gn)" -m 750 "$DOCS_DIR"
    else
      chmod 750 "$DOCS_DIR"
    fi
    echo "Pasta dos docs criada: $DOCS_DIR."
  fi
  # Marcador da raiz (decisão 106): o ambiente só vê docs_root acessível com
  # ele, então uma pasta de montagem vazia (montagem caída) não passa. O
  # cthubd também o cria ao subir; aqui, só se faltar (nunca sobrescreve).
  local marker="$DOCS_DIR/.ct-docs-root"
  if [[ ! -e $marker ]]; then
    if ! (set -o noclobber; : > "$marker") 2>/dev/null; then
      $SUDO install -o "$USER" -g "$(id -gn)" -m 644 /dev/null "$marker"
    fi
    echo "Marcador $marker criado."
  fi
  if [[ -z $cur ]]; then
    toml_set_top "$cfg" docs_dir "$DOCS_DIR" "Pasta dos docs dos projetos, por provision.sh --docs-dir."
    echo "docs_dir = \"$DOCS_DIR\" gravado em $cfg."
    HUB_DOCS_CHANGED=1
  fi
}

# Máquina única: o ambiente daqui lê os docs direto do docs_dir (sem
# montagem). Não troca um docs_root já configurado.
link_env_docs() {
  local cfg="$CT_CONF/config.toml" cur
  cur="$(toml_get "$cfg" docs_root)"
  if [[ -n $cur ]]; then
    if [[ "$(expand_tilde "$cur")" != "$DOCS_DIR" ]]; then
      echo "Aviso: docs_root = \"$cur\" no config do ctowerd; mantido (o docs_dir do hub é $DOCS_DIR)." >&2
    fi
    return 0
  fi
  ct_mkdir "$CT_CONF"
  toml_set_top "$cfg" docs_root "$DOCS_DIR" "Docs dos projetos: o docs_dir do hub, nesta mesma máquina (provision.sh --docs-dir)."
  echo "docs_root = \"$DOCS_DIR\" gravado em $cfg."
  ENV_LINK_CHANGED=1
}

# Senha do usuário do Samba: CT_DOCS_SAMBA_PASSWORD ou pedida no terminal
# (duas vezes). Nunca vem como argumento.
docs_samba_password() {
  local pw="${CT_DOCS_SAMBA_PASSWORD:-}" pw2
  if [[ -z $pw ]]; then
    if ! { : < /dev/tty; } 2>/dev/null; then
      echo "Falta a senha do usuário $DOCS_SMB_USER do Samba: rode num terminal ou passe CT_DOCS_SAMBA_PASSWORD." >&2
      return 1
    fi
    read -rs -p "Senha do Samba para $DOCS_SMB_USER (mín. 8 caracteres): " pw < /dev/tty
    echo >&2
    read -rs -p "Repita a senha: " pw2 < /dev/tty
    echo >&2
    if [[ $pw != "$pw2" ]]; then
      echo "As senhas não conferem." >&2
      return 1
    fi
  fi
  if [[ ${#pw} -lt 8 ]]; then
    echo "Senha do Samba curta demais (mínimo 8 caracteres)." >&2
    return 1
  fi
  printf '%s' "$pw"
}

# Bloco [ctdocs] do Samba. Os arquivos ficam com o dono do hub (force user),
# que é quem roda o cthubd (backup dos docs) e o ctowerd desta máquina.
docs_samba_conf() {
  cat <<CONF
# Gerado por provision.sh --docs-samba (Control Tower, #443). Rodar o script
# de novo regrava este arquivo: mude pelas opções e variáveis dele.
[$DOCS_SHARE]
   comment = Docs dos projetos do Control Tower
   path = $DOCS_DIR
   browseable = yes
   read only = no
   guest ok = no
   valid users = $DOCS_SMB_USER
   force user = $USER
   force group = $(id -gn)
   create mask = 0664
   directory mask = 0775
   hosts allow = $DOCS_SMB_HOSTS
   hosts deny = ALL
   vfs objects = catia fruit streams_xattr
   fruit:metadata = stream
   fruit:posix_rename = yes
   veto files = /.DS_Store/
   delete veto files = yes
CONF
}

# Compartilhamento Samba do docs_dir (Ubuntu): pacote, usuário do sistema
# sem login, usuário do Samba, o bloco [ctdocs] num arquivo próprio incluído
# no smb.conf e o smbd no ar. Idempotente: só regrava e recarrega o que mudou.
setup_docs_samba() {
  local conf="$SAMBA_DIR/$DOCS_SHARE.conf" smb="$SAMBA_DIR/smb.conf" tmp pw changed=0 r
  if [[ $SKIP_PACKAGES -eq 0 ]]; then
    $SUDO apt-get install -y samba samba-vfs-modules
  fi
  # O Samba exige um usuário do sistema com o mesmo nome; ele não loga nem
  # tem home, e não precisa de permissão na pasta (force user).
  if ! id -u "$DOCS_SMB_USER" >/dev/null 2>&1; then
    $SUDO useradd --system --no-create-home --shell /usr/sbin/nologin "$DOCS_SMB_USER"
  fi
  if $SUDO pdbedit -L 2>/dev/null | cut -d: -f1 | grep -qx "$DOCS_SMB_USER"; then
    echo "Usuário $DOCS_SMB_USER já existe no Samba; senha mantida (para trocar: sudo smbpasswd $DOCS_SMB_USER)."
  else
    pw="$(docs_samba_password)" || return 1
    # printf é builtin: a senha não aparece na lista de processos.
    printf '%s\n%s\n' "$pw" "$pw" | $SUDO smbpasswd -s -a "$DOCS_SMB_USER" >/dev/null
    pw=""
    echo "Usuário $DOCS_SMB_USER criado no Samba."
  fi
  tmp="$(mktemp)"
  docs_samba_conf > "$tmp"
  if ! cmp -s "$tmp" "$conf"; then
    $SUDO install -m 644 "$tmp" "$conf"
    changed=1
  fi
  rm -f "$tmp"
  if ! grep -qsxF "include = $conf" "$smb"; then
    printf '\n# Docs dos projetos do Control Tower (provision.sh --docs-samba)\ninclude = %s\n' "$conf" \
      | $SUDO tee -a "$smb" >/dev/null
    changed=1
  fi
  if ! $SUDO testparm -s "$smb" >/dev/null 2>&1; then
    echo "testparm recusou $smb (o Samba está instalado? sem --skip-packages o script instala). Confira com 'sudo testparm -s'." >&2
    return 1
  fi
  $SUDO systemctl enable --now smbd
  if [[ $changed -eq 1 ]]; then
    $SUDO systemctl reload smbd
    echo "Compartilhamento [$DOCS_SHARE] gravado em $conf (smbd recarregado)."
  else
    echo "Compartilhamento [$DOCS_SHARE] já configurado; nada mudou."
  fi
  # Com o ufw ligado, libera o Samba só para as mesmas faixas (regra
  # repetida o ufw ignora).
  if $SUDO ufw status 2>/dev/null | grep -q '^Status: active'; then
    for r in $DOCS_SMB_HOSTS; do
      [[ $r == 127.* || $r == ::1 ]] && continue
      $SUDO ufw allow from "$r" to any app Samba >/dev/null
    done
    echo "ufw: Samba liberado para $DOCS_SMB_HOSTS."
  fi
}

# --- execução -----------------------------------------------------------------

# Docs (#443): confere tudo antes de instalar qualquer coisa.
DOCS_DIR=""
if [[ -n $DOCS_DIR_ARG ]]; then
  DOCS_DIR_ARG="$(expand_tilde "$DOCS_DIR_ARG")"
  DOCS_DIR_ARG="${DOCS_DIR_ARG%/}"
  if ! valid_docs_dir "$DOCS_DIR_ARG"; then
    echo "--docs-dir inválido: \"$DOCS_DIR_ARG\" (caminho absoluto, só letras, números e . _ / + @ -)." >&2
    exit 2
  fi
fi
if [[ $WANT_DOCS_SAMBA -eq 1 ]]; then
  if [[ $OS == Darwin ]]; then
    echo "--docs-samba é só para Linux (Ubuntu). No macOS, use Ajustes do Sistema › Geral › Compartilhamento (docs/hub.md)." >&2
    exit 2
  fi
  if [[ -z $DOCS_DIR_ARG && -z "$(toml_get "$HUB_CONF/config.toml" docs_dir)" ]]; then
    echo "--docs-samba precisa da pasta: passe --docs-dir PASTA (ou grave docs_dir no config do hub)." >&2
    exit 2
  fi
  if [[ ! $DOCS_SMB_USER =~ ^[a-z_][a-z0-9_-]{0,31}$ ]]; then
    echo "CT_DOCS_SAMBA_USER inválido: \"$DOCS_SMB_USER\" (minúsculas, números, _ e -)." >&2
    exit 2
  fi
  if [[ ! $DOCS_SMB_HOSTS =~ ^[0-9A-Fa-f.:/\ ]+$ ]]; then
    echo "CT_DOCS_SAMBA_HOSTS inválido: \"$DOCS_SMB_HOSTS\" (endereços e faixas IP separados por espaço)." >&2
    exit 2
  fi
fi
WANT_DOCS=0
if [[ -n $DOCS_DIR_ARG || $WANT_DOCS_SAMBA -eq 1 ]]; then
  WANT_DOCS=1
fi

if [[ $SKIP_PACKAGES -eq 0 ]]; then
  if [[ $OS == Darwin ]]; then
    install_packages_macos
  else
    install_packages_linux
  fi
fi

if [[ $WANT_ENV -eq 1 ]]; then
  rc="$HOME/.bashrc"
  [[ $OS == Darwin ]] && rc="$HOME/.zshrc"
  if ! grep -qs 'HOME/.local/bin' "$rc"; then
    # shellcheck disable=SC2016 # literal: expande no .bashrc/.zshrc
    echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$rc"
  fi
  mkdir -p "$HOME/projetos" "$HOME/worktrees" "$HOME/.local/share/ctower"
  ct_mkdir "$CT_CONF"
fi

HUB_DEFERRED=0
ENV_LINK_CHANGED=0
ENV_SKIPPED=0
HUB_DOCS_CHANGED=0

# Docs antes do hub: o cthubd lê o docs_dir ao subir.
if [[ $WANT_DOCS -eq 1 ]]; then
  setup_docs_dir
  if [[ $WANT_DOCS_SAMBA -eq 1 ]]; then
    setup_docs_samba
  fi
fi

# Control Tower: binários e serviços de usuário (precisa do repositório clonado).
if [[ -f "$REPO/Cargo.toml" ]]; then
  # Hub primeiro: na máquina única, o token do ambiente vem dele. Os tokens
  # do hub (~/.config/cthub/ui.token e hub_id) são gerados pelo cthubd na 1ª subida.
  if [[ $WANT_HUB -eq 1 ]]; then
    require_node_for_hub
    install_hub
  fi

  # Ambiente: tokens (~/.config/ctower/{ui,hook}.token) e env_id gerados pelo
  # ctowerd na 1ª subida. Na máquina única, o config é ligado ao hub antes.
  if [[ $WANT_ENV -eq 1 ]]; then
    if legacy_hub; then
      # O ctowerd novo recusa subir com hub = true: não troca os binários
      # da máquina do hub antigo antes do `cthubd migrate` (#182).
      ENV_SKIPPED=1
    else
      install_env_bins
      if [[ $WANT_HUB -eq 1 && $HUB_DEFERRED -eq 0 ]]; then
        link_local_env
        if [[ $WANT_DOCS -eq 1 ]]; then
          link_env_docs
        fi
      elif [[ -n $HUB_URL_ARG ]]; then
        link_remote_env
      fi
      start_env
    fi
  fi
else
  echo "Repositório não encontrado em $REPO; pulei a instalação dos serviços." >&2
fi

echo
echo "Provisionamento concluído. Passos manuais restantes:"
if [[ $ENV_SKIPPED -eq 1 ]]; then
  echo "  * O ctowerd daqui tem 'hub = true' (hub antigo): não troquei os binários dele, porque a"
  echo "    versão nova recusa subir assim. Migre antes (docs/hub.md, seção 5)."
fi
if [[ $HUB_DEFERRED -eq 1 ]]; then
  echo "  * O ctowerd daqui ainda é o hub antigo (hub = true): o cthubd foi instalado, mas não"
  echo "    subiu nem foi ligado. Migre (docs/hub.md, seção 5):"
  echo "      systemctl --user stop ctowerd"
  echo "      $BIN/cthubd migrate --from $CT_DATA"
  echo "      systemctl --user enable --now cthubd"
  echo "      ctower update    # guarda o binário anterior para 'ctower update --rollback'"
  if [[ -z ${CT_FROM_MIGRATE:-} ]]; then
    echo "    ou, de uma vez (passos 2 a 6 do roteiro): $REPO/scripts/migrate-to-cthubd.sh"
  fi
elif [[ $ENV_SKIPPED -eq 1 ]]; then
  echo "    Instale o cthubd e migre: $0 --hub --env (docs/hub.md, seção 5)."
fi
if [[ $WANT_ENV -eq 1 ]]; then
  echo "  1. gh auth login && gh auth setup-git"
  echo "  2. claude   (fazer login)"
  echo "  3. git config --global user.name / user.email"
  if [[ $WANT_HUB -eq 0 && -z $HUB_URL_ARG ]]; then
    echo "  4. ligar a um hub: 'cthubd add-env <nome>' no hub, 'ctower hub set-token' e hub_url aqui (docs/hub.md)"
  fi
fi
if [[ $WANT_HUB -eq 1 ]]; then
  echo "  * UI: em Configurações → Hub, informe http://<máquina>:$(hub_port) e o token de $HUB_CONF/ui.token"
  echo "  * Hub em outra máquina da tailnet: bind = \"<IP Tailscale>\" em $HUB_CONF/config.toml"
  echo "  * Token do GitHub do hub: gh auth login (ou pela tela Hub da UI)"
  echo "  * Atualizar o hub depois: cthubd update"
fi
if [[ $HUB_DOCS_CHANGED -eq 1 ]]; then
  if [[ $OS == Darwin ]]; then
    echo "  * docs_dir gravado: se o cthubd já estava no ar, reinicie-o (launchctl kickstart -k gui/\$(id -u)/$LAUNCHD_LABEL)"
  else
    echo "  * docs_dir gravado: se o cthubd já estava no ar, reinicie-o (systemctl --user restart cthubd)"
  fi
fi
if [[ $WANT_DOCS_SAMBA -eq 1 ]]; then
  echo "  * Docs: monte smb://$DOCS_SMB_USER@<máquina>/$DOCS_SHARE nos outros ambientes e aponte docs_root"
  echo "    para a montagem (docs/hub.md, \"Docs dos projetos\")"
fi
