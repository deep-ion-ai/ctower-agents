#!/usr/bin/env bash
# Teste do `provision.sh --env` numa máquina sem Rust (#359): HOME limpo, sem
# cargo no PATH, baixando o asset ctower-env da release de verdade (gh real).
# Prova que (1) os binários vêm do asset, com o sha256 conferido; (2) um asset
# adulterado é descartado e cai para o fonte; (3) --from-source compila do
# fonte. A compilação é um cargo falso que só registra a chamada (o build do
# fonte já roda no CI); sudo, systemctl e launchctl também são falsos.
#
# Uso: scripts/provision/test-provision-release.sh [TAG]   (padrão: v<versão do Cargo.toml>)
# Precisa de gh autenticado (ou GH_TOKEN) e de uma release publicada com o asset.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
VER="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
TAG="${1:-v$VER}"
REAL_GH="$(command -v gh)" || { echo "falta o gh" >&2; exit 1; }
REAL_HOME="$HOME"

# Antes da primeira release (ou numa bifurcação sem releases) não há asset para
# baixar: avisa e sai sem falhar, em vez de quebrar o CI.
if ! "$REAL_GH" release view "$TAG" ${GH_REPO:+--repo "$GH_REPO"} >/dev/null 2>&1; then
  echo "AVISO: a release $TAG não existe neste repositório; teste do asset ignorado." >&2
  exit 0
fi

ARCH="$(uname -m)"
[[ $ARCH == arm64 ]] && ARCH=aarch64
OSN=linux
[[ "$(uname -s)" == Darwin ]] && OSN=macos
ASSET="ctower-env-$TAG-$ARCH-$OSN.tar.gz"

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
F="$T/fake"
mkdir -p "$F"

# gh de verdade (credenciais do HOME real); com TAMPER definido, adultera o
# tar.gz baixado sem mexer no .sha256.
cat > "$F/gh" <<GH
#!/bin/sh
HOME="$REAL_HOME" "$REAL_GH" "\$@" || exit \$?
if [ -n "\${TAMPER:-}" ]; then
  dir=""; prev=""
  for a in "\$@"; do [ "\$prev" = --dir ] && dir="\$a"; prev="\$a"; done
  [ -n "\$dir" ] && for t in "\$dir"/*.tar.gz; do [ -f "\$t" ] && printf 'adulterado' >> "\$t"; done
fi
exit 0
GH
for c in sudo systemctl launchctl; do
  printf '#!/bin/sh\necho "%s $*" >> "$FAKE_LOG"\n[ "$2" = is-active ] && exit 1\nexit 0\n' "$c" > "$F/$c"
done
printf '#!/bin/sh\necho "cargo $*" >> "$FAKE_LOG"\n' > "$F/cargo"
chmod +x "$F"/*

fail() { echo "FALHOU: $*" >&2; exit 1; }

BASE_PATH="/usr/local/bin:/usr/bin:/bin"
[[ -z "$(env -i PATH="$BASE_PATH" sh -c 'command -v cargo || true')" ]] \
  || fail "há um cargo no PATH base; o teste precisa de uma máquina sem Rust"

# O gh entra só pelo CT_GH (com o GH_TOKEN do CI); o PATH não tem cargo nem rustup.
run() {
  local home="$1"
  shift
  env -i PATH="$BASE_PATH" USER="${USER:-teste}" HOME="$home" ${GH_TOKEN:+GH_TOKEN="$GH_TOKEN"} \
    FAKE_LOG="$T/log" CTOWER_REPO="$ROOT" CTOWER_PORT=1 \
    CT_SUDO="$F/sudo" CT_SYSTEMCTL="$F/systemctl" CT_LAUNCHCTL="$F/launchctl" \
    CT_CARGO="$F/cargo" CT_GH="$F/gh" \
    CTOWER_SERVICE_MANAGER=systemd CTOWER_SYSTEMCTL="$F/systemctl" CTOWER_LAUNCHCTL="$F/launchctl" \
    ${EXTRA_ENV:-} \
    bash "$ROOT/scripts/provision/provision.sh" --skip-packages --env "$@"
}

# 1. Sem Rust, da release: binários instalados, sem chamar o cargo.
H1="$T/h1"; mkdir -p "$H1"; : > "$T/log"
run "$H1" > "$T/out1" 2>&1 || fail "provision --env falhou: $(cat "$T/out1")"
grep -q "instalados da release" "$T/out1" || fail "não instalou da release: $(cat "$T/out1")"
for b in ctowerd ctower; do
  [[ -x "$H1/.cargo/bin/$b" ]] || fail "$b não instalado"
done
# O ctowerd não tem --help (sobe o servidor): confere só que é binário nativo.
# (magic number ELF ou Mach-O 64 bits; o runner pode não ter o `file`).
[[ "$(od -An -tx1 -N4 "$H1/.cargo/bin/ctowerd" | tr -d ' \n')" =~ ^(7f454c46|cffaedfe)$ ]] || fail "ctowerd instalado não é um binário nativo"
"$H1/.cargo/bin/ctower" --help >/dev/null 2>&1 || fail "ctower instalado não executa"
grep -q "^cargo" "$T/log" && fail "chamou o cargo: $(cat "$T/log")"
[[ ! -d "$H1/.rustup" ]] || fail "instalou o rustup"

# 2. O sha256 publicado confere com o asset, por fora do script.
mkdir -p "$T/dl"
"$F/gh" release download "$TAG" --repo deep-ion-ai/ctower-agents --dir "$T/dl" \
  --pattern "$ASSET" --pattern "$ASSET.sha256" >/dev/null
(cd "$T/dl" && { command -v sha256sum >/dev/null && sha256sum -c "$ASSET.sha256" || shasum -a 256 -c "$ASSET.sha256"; } >/dev/null) \
  || fail "o sha256 publicado não confere com o asset"

# 3. Asset adulterado: descartado, nada instalado dele, cai para o fonte.
H3="$T/h3"; mkdir -p "$H3"; : > "$T/log"
EXTRA_ENV="TAMPER=1" run "$H3" > "$T/out3" 2>&1 || fail "fallback do asset adulterado falhou: $(cat "$T/out3")"
grep -q "sha256 de $ASSET não confere" "$T/out3" || fail "não avisou do sha256: $(cat "$T/out3")"
grep -q "instalados da release" "$T/out3" && fail "instalou o asset adulterado"
[[ ! -e "$H3/.cargo/bin/ctowerd" ]] || fail "binário do asset adulterado instalado"
grep -q "cargo install --locked --path $ROOT/crates/ctowerd" "$T/log" || fail "não caiu para o fonte"

# 4. --from-source: nem tenta a release, compila do fonte.
H4="$T/h4"; mkdir -p "$H4"; : > "$T/log"
run "$H4" --from-source > "$T/out4" 2>&1 || fail "--from-source falhou: $(cat "$T/out4")"
grep -q "instalados da release" "$T/out4" && fail "--from-source baixou a release"
for c in ctowerd ctower; do
  grep -q "cargo install --locked --path $ROOT/crates/$c" "$T/log" || fail "--from-source não compilou o $c"
done

echo "provision.sh --env sem Rust ($TAG, $ARCH-$OSN): ok"
