#!/usr/bin/env bash
# Instala (ou reinstala) o app Control Tower no macOS a partir do .dmg da
# release, sem o bloqueio do Gatekeeper na primeira abertura.
#
# Uso: install-app.sh [--version vX.Y.Z] [--dir DIR] [--open]
#
#   --version  tag da release (padrão: a mais recente)
#   --dir      pasta de destino (padrão: /Applications)
#   --open     abre o app no fim
#
# O app não tem Developer ID nem notarização: baixado pelo navegador, o .dmg
# vem com o atributo com.apple.quarantine e o Gatekeeper recusa abrir o app.
# Aqui o download é pelo gh (o repositório é privado), que não grava esse
# atributo; por garantia ele é removido do app instalado. As versões seguintes
# chegam pelo updater do próprio app, que também não grava a quarentena.
#
# Sem clone do repositório:
#   gh api repos/deep-ion-ai/ctower-agents/contents/scripts/install-app.sh \
#     -H 'Accept: application/vnd.github.raw' | bash
#
# Variáveis: CT_GH troca o gh; CTOWER_RELEASE_REPO troca o repositório
# (padrão deep-ion-ai/ctower-agents).
set -euo pipefail

GH="${CT_GH:-gh}"
RELEASE_REPO="${CTOWER_RELEASE_REPO:-deep-ion-ai/ctower-agents}"
APP_NAME="Control Tower.app"
TAG=""
DEST="/Applications"
OPEN=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) TAG="${2:?--version precisa de uma tag}"; shift ;;
    --dir) DEST="${2:?--dir precisa de um diretório}"; shift ;;
    --open) OPEN=1 ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) echo "opção desconhecida: $1 (veja --help)" >&2; exit 2 ;;
  esac
  shift
done

if [[ "$(uname -s)" != Darwin ]]; then
  echo "Só macOS (o app do Linux/WSL é a UI web do hub)." >&2
  exit 1
fi
if [[ "$(uname -m)" != arm64 ]]; then
  echo "A release só tem o app para Apple Silicon (aarch64)." >&2
  exit 1
fi
if [[ $EUID -eq 0 ]]; then
  echo "Rode como usuário comum; o sudo só é pedido se $DEST não for gravável." >&2
  exit 1
fi
if ! command -v "$GH" >/dev/null; then
  echo "gh não encontrado: brew install gh && gh auth login" >&2
  exit 1
fi
if ! "$GH" auth status >/dev/null 2>&1; then
  echo "gh sem login: rode gh auth login (o repositório é privado)." >&2
  exit 1
fi
if [[ ! -d $DEST ]]; then
  echo "Destino não existe: $DEST" >&2
  exit 1
fi

if [[ -z $TAG ]]; then
  TAG="$("$GH" release view --repo "$RELEASE_REPO" --json tagName -q .tagName)"
fi

# pwd -P: o mktemp devolve /var/..., mas o mount lista /private/var/...
tmp="$(cd "$(mktemp -d)" && pwd -P)"
mnt="$tmp/mnt"
cleanup() {
  if mount | grep -qF " on $mnt "; then
    hdiutil detach -quiet "$mnt" || hdiutil detach -quiet -force "$mnt" || {
      echo "Aviso: não consegui desmontar $mnt; ficou em $tmp." >&2
      return
    }
  fi
  rm -rf "$tmp"
}
trap cleanup EXIT

echo "Baixando o app $TAG de $RELEASE_REPO..."
"$GH" release download "$TAG" --repo "$RELEASE_REPO" --dir "$tmp" --pattern '*_aarch64.dmg'
dmg="$(find "$tmp" -maxdepth 1 -name '*_aarch64.dmg' | head -1)"
if [[ -z $dmg ]]; then
  echo "A release $TAG não tem o .dmg do app." >&2
  exit 1
fi

mkdir -p "$mnt"
hdiutil attach -quiet -nobrowse -readonly -noautoopen -mountpoint "$mnt" "$dmg"
if [[ ! -d $mnt/$APP_NAME ]]; then
  echo "O .dmg não tem $APP_NAME." >&2
  exit 1
fi

target="$DEST/$APP_NAME"
# Com o app aberto, a troca do bundle deixa o processo antigo rodando sobre
# arquivos apagados: pede para fechar e espera até 10 s. Casa pelo caminho
# para não pegar o app de `tauri dev`, que tem o mesmo executável.
running() { pgrep -f "^$target/Contents/MacOS/" >/dev/null; }
if running; then
  echo "Fechando o Control Tower aberto..."
  osascript -e 'tell application id "ai.deep-ion.control-tower" to quit' >/dev/null 2>&1 || true
  for _ in $(seq 20); do
    running || break
    sleep 0.5
  done
  if running; then
    echo "O Control Tower continua aberto; feche-o e rode de novo." >&2
    exit 1
  fi
fi

SUDO=""
[[ -w $DEST ]] || SUDO=sudo
# Copia ao lado e só então troca, para uma falha no meio não deixar o
# destino sem app.
staging="$DEST/.$APP_NAME.new"
$SUDO rm -rf "$staging"
$SUDO ditto "$mnt/$APP_NAME" "$staging"
$SUDO xattr -dr com.apple.quarantine "$staging" 2>/dev/null || true
# O build assina só o executável (assinatura do linker), sem selar o bundle, e
# o codesign recusa o app ("code has no resources but signature indicates they
# must be present"). Nesse caso refaz a assinatura ad-hoc do bundle inteiro;
# um app já assinado direito (Developer ID) fica como veio.
if ! codesign --verify --deep --strict "$staging" 2>/dev/null; then
  $SUDO codesign --force --deep --sign - "$staging" 2>/dev/null
  if ! codesign --verify --deep --strict "$staging" 2>/dev/null; then
    $SUDO rm -rf "$staging"
    echo "A assinatura de $APP_NAME não confere nem depois de refeita; nada foi instalado." >&2
    exit 1
  fi
fi
$SUDO rm -rf "$target"
$SUDO mv "$staging" "$target"

ver="$(defaults read "$target/Contents/Info" CFBundleShortVersionString 2>/dev/null || echo "?")"
echo "Control Tower $ver instalado em $target."
if [[ $OPEN -eq 1 ]]; then
  open "$target"
fi
