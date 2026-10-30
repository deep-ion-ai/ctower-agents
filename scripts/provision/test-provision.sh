#!/usr/bin/env bash
# Teste do provision.sh sem tocar no sistema: HOME temporário, sudo,
# systemctl, launchctl, gh e cargo falsos (o cargo falso copia os binários já
# compilados). Testa o caminho do systemd (CT_OS=Linux) em qualquer SO: no
# macOS, o launchctl de verdade reiniciaria o ctowerd do usuário (#283).
# Prova que `--hub --env` liga o ambiente ao hub local e que rodar de novo
# não gera outro token nem reescreve o config.
#
# Uso: scripts/provision/test-provision.sh [DIR_DOS_BINÁRIOS]   (padrão: target/debug)
# Precisa de cthubd, ctowerd e ctower compilados (cargo build -p cthubd -p ctowerd -p ctower).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BINS="$(cd "${1:-$ROOT/target/debug}" && pwd)"
for b in cthubd ctowerd ctower; do
  [[ -x "$BINS/$b" ]] || { echo "falta $BINS/$b (cargo build -p $b)" >&2; exit 1; }
done

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
H="$T/home"
F="$T/fake"
mkdir -p "$H" "$F"

cat > "$F/sudo" <<'EOF'
#!/bin/sh
echo "sudo $*" >> "$FAKE_LOG"
EOF
cat > "$F/systemctl" <<'EOF'
#!/bin/sh
echo "systemctl $*" >> "$FAKE_LOG"
case "$2" in
  is-active) [ -f "$FAKE_ACTIVE" ] ;;
  *) exit 0 ;;
esac
EOF
cat > "$F/cargo" <<'EOF'
#!/bin/sh
# cargo install --locked --path <crate>: copia o binário já compilado.
while [ $# -gt 0 ]; do case "$1" in --path) P="$2"; shift;; esac; shift; done
B="$(basename "$P")"
mkdir -p "$HOME/.cargo/bin"
cp "$FAKE_BINS/$B" "$HOME/.cargo/bin/$B"
echo "cargo install $B" >> "$FAKE_LOG"
EOF
# Nunca deve ser chamado (CT_OS=Linux): falha alto em vez de tocar no launchd.
cat > "$F/launchctl" <<'EOF'
#!/bin/sh
echo "launchctl $*" >> "$FAKE_LOG.launchctl"
echo "launchctl chamado no teste: $*" >&2
exit 1
EOF
# Node e npm falsos do build da UI web do hub (#199): só registram.
cat > "$F/node" <<'EOF'
#!/bin/sh
echo v22.11.0
EOF
cat > "$F/npm" <<'EOF'
#!/bin/sh
[ "$1" = --version ] && { echo 10.9.0; exit 0; }
echo "npm $* ($(basename "$PWD"))" >> "$FAKE_LOG"
EOF
# Sem release: o provision cai para o cargo falso.
cat > "$F/gh" <<'EOF'
#!/bin/sh
exit 1
EOF
chmod +x "$F"/*

# Ambiente comum dos runs (falta HOME e FAKE_ACTIVE): falsos para tudo que
# mexe no sistema, inclusive o `ctower restart` que o provision chama.
FAKE_ENV=(
  PATH="/usr/local/bin:/usr/bin:/bin" USER="${USER:-teste}"
  FAKE_LOG="$T/log" FAKE_BINS="$BINS" CT_OS=Linux
  CT_SUDO="$F/sudo" CT_SYSTEMCTL="$F/systemctl" CT_LAUNCHCTL="$F/launchctl"
  CT_CARGO="$F/cargo" CT_GH="$F/gh" CT_NODE="$F/node" CT_NPM="$F/npm"
  CTOWER_SERVICE_MANAGER=systemd CTOWER_SYSTEMCTL="$F/systemctl" CTOWER_LAUNCHCTL="$F/launchctl"
  CTOWER_REPO="$ROOT" CTOWER_PORT=1
)

run() {
  run_in "$H" "$@"
}

run_in() {
  local home="$1"
  shift
  env -i "${FAKE_ENV[@]}" HOME="$home" FAKE_ACTIVE="$T/active" \
    bash "$ROOT/scripts/provision/provision.sh" --skip-packages "$@"
}

fail() { echo "FALHOU: $*" >&2; exit 1; }

CFG="$H/.config/ctower/config.toml"
TOK="$H/.config/ctower/hub.token"
AGENT="$H/.config/ctower/hub-agent.token"

# 1. Máquina única do zero.
run --hub --env --env-name maquina > "$T/out1"
[[ -s $TOK ]] || fail "hub.token não gravado"
[[ "$({ stat -c %a "$TOK" 2>/dev/null || stat -f %Lp "$TOK"; })" == 600 ]] || fail "hub.token sem 0600"
# #195: o token nasceu preso a um env_id, gravado junto no ambiente.
[[ -s "$H/.config/ctower/env_id" ]] || fail "env_id do hub não gravado"
HUB_ENV_ID="$(python3 -c 'import sqlite3, sys; print(sqlite3.connect(sys.argv[1]).execute("SELECT env_id FROM env_token").fetchone()[0])' "$H/.local/share/cthub/hub.db")"
[[ "$(tr -d '[:space:]' < "$H/.config/ctower/env_id")" == "$HUB_ENV_ID" ]] || fail "env_id do ambiente difere do preso ao token"
[[ -s $AGENT ]] || fail "hub-agent.token não gravado"
[[ "$({ stat -c %a "$AGENT" 2>/dev/null || stat -f %Lp "$AGENT"; })" == 600 ]] || fail "hub-agent.token sem 0600"
grep -q '^cta_' "$AGENT" || fail "hub-agent.token não é token de agente"
grep -qx 'hub_url = "http://127.0.0.1:7879"' "$CFG" || fail "hub_url não gravado: $(cat "$CFG")"
grep -qx 'env_name = "maquina"' "$CFG" || fail "env_name não gravado"
# O token gravado é um token válido do hub para "maquina".
HUBDB="$H/.local/share/cthub/hub.db"
[[ -f $HUBDB ]] || fail "hub.db não criado"
grep -q "systemctl --user enable --now cthubd" "$T/log" || fail "cthubd não habilitado"
grep -q "systemctl --user enable --now ctowerd" "$T/log" || fail "ctowerd não habilitado"
[[ -f "$H/.config/systemd/user/cthubd.service" ]] || fail "unidade do cthubd não instalada"
[[ -f "$H/.config/systemd/user/ctowerd.service" ]] || fail "unidade do ctowerd não instalada"
# O hub vem antes do ambiente.
first_hub=$(grep -n "enable --now cthubd" "$T/log" | cut -d: -f1)
first_env=$(grep -n "enable --now ctowerd" "$T/log" | cut -d: -f1)
(( first_hub < first_env )) || fail "ctowerd subiu antes do cthubd"
"$H/.cargo/bin/cthubd" --help >/dev/null
# O bundle web sai antes do cargo do cthubd (#199).
grep -q "npm run build:web (ui)" "$T/log" || fail "não gerou o bundle web: $(cat "$T/log")"
first_web=$(grep -n "npm run build:web" "$T/log" | head -1 | cut -d: -f1)
first_hubbin=$(grep -n "cargo install cthubd" "$T/log" | head -1 | cut -d: -f1)
(( first_web < first_hubbin )) || fail "o cthubd compilou antes do bundle web"
TOKEN1="$(cat "$TOK")"
AGENT1="$(cat "$AGENT")"
CFG1="$(cat "$CFG")"

# O config gerado é TOML válido (tomllib: Python 3.11+; o do macOS é mais velho).
if python3 -c 'import tomllib' 2>/dev/null; then
  python3 -c 'import sys, tomllib; c = tomllib.load(open(sys.argv[1], "rb")); assert c["hub_url"] == "http://127.0.0.1:7879", c' "$CFG" \
    || fail "config.toml inválido"
fi

# O token gravado vale no hub: sobe o cthubd numa porta livre e pede a
# configuração como o ambiente faria.
PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
env -i HOME="$H" PATH="/usr/bin:/bin" CTHUB_PORT="$PORT" RUST_LOG=warn "$H/.cargo/bin/cthubd" serve > "$T/hub.log" 2>&1 &
HUB_PID=$!
trap 'kill "$HUB_PID" 2>/dev/null || true; rm -rf "$T"' EXIT
for _ in $(seq 50); do
  curl -fsS "http://127.0.0.1:$PORT/healthz" >/dev/null 2>&1 && break
  sleep 0.2
done
code="$(curl -s -o "$T/config.json" -w '%{http_code}' \
  -H "Authorization: Bearer $TOKEN1" -H "X-Ctower-Env-Id: $HUB_ENV_ID" \
  "http://127.0.0.1:$PORT/sync/config")"
[[ $code == 200 ]] || fail "hub recusou o token gravado ($code): $(cat "$T/config.json") $(cat "$T/hub.log")"
# #195: o mesmo token com outro env_id é recusado já no primeiro uso.
code="$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $TOKEN1" \
  -H "X-Ctower-Env-Id: env-teste" "http://127.0.0.1:$PORT/sync/config")"
[[ $code == 403 ]] || fail "hub aceitou o token com outro env_id ($code)"
code="$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer errado" \
  -H "X-Ctower-Env-Id: env-teste" "http://127.0.0.1:$PORT/sync/config")"
[[ $code == 401 ]] || fail "hub aceitou um token errado ($code)"
# O token de agente vale em /agent/v1 (o env_id já ficou preso pelo pull acima)
# e não no sync.
code="$(curl -s -o "$T/overview.json" -w '%{http_code}' \
  -H "Authorization: Bearer $AGENT1" -H "X-Ctower-Env-Id: $HUB_ENV_ID" \
  "http://127.0.0.1:$PORT/agent/v1/overview")"
[[ $code == 200 ]] || fail "hub recusou o token de agente ($code): $(cat "$T/overview.json")"
code="$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $AGENT1" \
  -H "X-Ctower-Env-Id: env-teste" "http://127.0.0.1:$PORT/sync/config")"
[[ $code == 401 ]] || fail "o sync aceitou o token de agente ($code)"
kill "$HUB_PID"
wait "$HUB_PID" 2>/dev/null || true

# 2. De novo, com o ctowerd no ar: nada muda e não reinicia.
touch "$T/active"
: > "$T/log"
run --hub --env --env-name maquina > "$T/out2"
[[ "$(cat "$TOK")" == "$TOKEN1" ]] || fail "rodar de novo trocou o token"
[[ "$(cat "$AGENT")" == "$AGENT1" ]] || fail "rodar de novo trocou o token de agente"
[[ "$(cat "$CFG")" == "$CFG1" ]] || fail "rodar de novo reescreveu o config"
grep -q "restart" "$T/log" && fail "reiniciou sem mudar nada"
grep -q "já existe; mantido" "$T/out2" || fail "não avisou do token existente"

# 3. Config de antes sem hub_url (com uma tabela): ganha hub_url no topo,
#    mantém o resto e o token; com o ctowerd no ar, reinicia via ctower restart.
printf 'port = 7878\n\n[algo]\nx = 1\n' > "$CFG"
: > "$T/log"
run --hub --env > "$T/out3"
[[ "$(cat "$TOK")" == "$TOKEN1" ]] || fail "trocou o token existente"
head -4 "$CFG" | grep -qx 'hub_url = "http://127.0.0.1:7879"' || fail "hub_url fora do topo: $(cat "$CFG")"
grep -qx 'port = 7878' "$CFG" || fail "perdeu a chave existente"
grep -qx 'x = 1' "$CFG" || fail "perdeu a tabela"
grep -q "systemctl --user restart ctowerd" "$T/log" || fail "não reiniciou o ctowerd depois de ligar"

# 4. Ambiente apontando para outro hub: não mexe.
printf 'hub_url = "http://outro:7879"\n' > "$CFG"
rm -f "$TOK"
run --hub --env > "$T/out4" 2>&1
[[ ! -e $TOK ]] || fail "gerou token para um ambiente de outro hub"
grep -q "outro hub" "$T/out4" || fail "não avisou do outro hub"

# 5. Hub antigo (hub = true): instala o cthubd, mas não sobe nem liga, e
#    não troca os binários do ambiente.
printf 'hub = true\n' > "$CFG"
rm -f "$H/.cargo/bin/ctowerd"
: > "$T/log"
run --hub --env > "$T/out5" 2>&1
grep -q "enable --now cthubd" "$T/log" && fail "subiu o cthubd sobre o hub antigo"
[[ ! -e "$H/.cargo/bin/ctowerd" ]] || fail "trocou o ctowerd da máquina do hub antigo"
[[ ! -e $TOK ]] || fail "ligou o ambiente do hub antigo"
grep -q "migre\|Migre" "$T/out5" || fail "não indicou a migração"

# 6. Só o ambiente (padrão, como antes): sem cthubd.
H2="$T/home2"; mkdir -p "$H2"
H="$H2" CFG="$H2/.config/ctower/config.toml"
: > "$T/log"
env -i "${FAKE_ENV[@]}" HOME="$H2" FAKE_ACTIVE="$T/none" \
  bash "$ROOT/scripts/provision/provision.sh" --skip-packages > "$T/out6"
grep -q "cargo install cthubd" "$T/log" && fail "o padrão instalou o hub"
grep -q "enable --now ctowerd" "$T/log" || fail "o padrão não subiu o ctowerd"
[[ ! -e "$H2/.config/ctower/hub.token" ]] || fail "o padrão gerou token"

# 7. Nome inválido: recusa antes de instalar qualquer coisa.
H3="$T/home3"; mkdir -p "$H3"
: > "$T/log"
if run_in "$H3" --hub --env --env-name 'a"b c' > "$T/out7" 2>&1; then
  fail "aceitou --env-name inválido"
fi
grep -q "inválido" "$T/out7" || fail "não explicou o nome inválido"
[[ ! -s "$T/log" ]] || fail "instalou algo com nome inválido: $(cat "$T/log")"
printf 'env_name = "com espaço"\n' > "$T/cfg7"
mkdir -p "$H3/.config/ctower"; cp "$T/cfg7" "$H3/.config/ctower/config.toml"
if run_in "$H3" --hub --env > "$T/out7b" 2>&1; then
  fail "aceitou env_name inválido do config"
fi
grep -q "Nome de ambiente inválido" "$T/out7b" || fail "não explicou o env_name inválido"
[[ ! -e "$H3/.config/ctower/hub.token" ]] || fail "gerou token com nome inválido"

# 8. Nome que já existe no hub (outro ambiente): não troca o token dele
#    nem liga este.
H4="$T/home4"; mkdir -p "$H4"
env -i HOME="$H4" PATH="/usr/bin:/bin" "$BINS/cthubd" add-env maquina >/dev/null 2>&1
hash_of() {
  python3 -c 'import sqlite3, sys; print(sqlite3.connect(sys.argv[1]).execute("SELECT token_hash FROM env_token WHERE name = ?", (sys.argv[2],)).fetchone()[0])' \
    "$H4/.local/share/cthub/hub.db" "$1"
}
OTHER="$(hash_of maquina)"
run_in "$H4" --hub --env --env-name maquina > "$T/out8" 2>&1
[[ ! -e "$H4/.config/ctower/hub.token" ]] || fail "ligou com um nome que já existe"
grep -q "já tem um ambiente" "$T/out8" || fail "não avisou do nome existente: $(cat "$T/out8")"
grep -qs hub_url "$H4/.config/ctower/config.toml" && fail "gravou hub_url sem token"
env -i HOME="$H4" PATH="/usr/bin:/bin" "$BINS/cthubd" add-env --no-replace maquina >/dev/null 2>&1 && fail "--no-replace trocou"
[[ "$(hash_of maquina)" == "$OTHER" ]] || fail "o token do outro ambiente mudou"

# 9. Hub antigo migrado com `cthubd migrate`: rodar o script de novo sobe o
#    cthubd e o ctowerd, sem gerar outro token nem reescrever o config.
H5="$T/home5"
mkdir -p "$H5/.config/ctower" "$H5/.local/share/ctower"
free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
CT_PORT5="$(free_port)"
HUB_PORT5="$(free_port)"
printf 'env_name = "hub-antigo"\nhub = true\nhub_backup_keep = 3\nport = %s\n' "$CT_PORT5" > "$H5/.config/ctower/config.toml"
printf '0b6c1f7e-1a2b-4c3d-8e9f-000000000001' > "$H5/.config/ctower/env_id"
python3 -c 'import sqlite3, sys; sqlite3.connect(sys.argv[1]).executescript(open(sys.argv[2]).read())' \
  "$H5/.local/share/ctower/hub.db" "$ROOT/crates/cthubd/tests/fixtures/hub-antigo.sql"
: > "$T/log"
run_in "$H5" --hub --env > "$T/out9a" 2>&1
grep -q "enable --now cthubd" "$T/log" && fail "subiu o cthubd antes da migração"
grep -q "cthubd migrate --from $H5/.local/share/ctower" "$T/out9a" || fail "não mostrou o comando da migração: $(cat "$T/out9a")"
env -i HOME="$H5" PATH="/usr/bin:/bin" CTHUB_PORT="$HUB_PORT5" \
  "$H5/.cargo/bin/cthubd" migrate --from "$H5/.local/share/ctower" > "$T/out9m" 2>&1 \
  || fail "cthubd migrate falhou: $(cat "$T/out9m")"
CFG5="$H5/.config/ctower/config.toml"
TOK5="$H5/.config/ctower/hub.token"
grep -qx "hub_url = \"http://127.0.0.1:$HUB_PORT5\"" "$CFG5" || fail "migrate não gravou hub_url: $(cat "$CFG5")"
[[ -s $TOK5 ]] || fail "migrate não gravou o hub.token"
[[ -f "$CFG5.pre-cthub" ]] || fail "migrate não guardou o config antigo"
TOKEN5="$(cat "$TOK5")"
MIG5="$(cat "$CFG5")"
: > "$T/log"
run_in "$H5" --hub --env > "$T/out9b" 2>&1 || fail "provision depois da migração falhou: $(cat "$T/out9b")"
grep -q "enable --now cthubd" "$T/log" || fail "não subiu o cthubd depois da migração"
grep -q "cargo install ctowerd" "$T/log" || fail "não instalou o ctowerd depois da migração"
grep -q "enable --now ctowerd" "$T/log" || fail "não subiu o ctowerd depois da migração"
[[ "$(cat "$TOK5")" == "$TOKEN5" ]] || fail "trocou o token gerado pela migração"
[[ "$(cat "$CFG5")" == "$MIG5" ]] || fail "reescreveu o config migrado"
grep -q "hub antigo" "$T/out9b" && fail "ainda trata como hub antigo"

# 10. Ambiente ligado a um hub em outra máquina (--hub-url): token por
#     CT_HUB_TOKEN ou arquivo, sem trocar o que já existe.
H6="$T/home6"; mkdir -p "$H6"
CFG6="$H6/.config/ctower/config.toml"; TOK6="$H6/.config/ctower/hub.token"
run_in "$H6" --env --hub-url http://192.0.2.10:7879 --env-name mac > "$T/out10a" 2>&1 \
  && fail "aceitou --hub-url sem token"
grep -q "Falta o token" "$T/out10a" || fail "não avisou do token: $(cat "$T/out10a")"
[[ ! -e $TOK6 ]] || fail "gravou hub.token vazio"
printf 'cte_remoto\n' > "$T/tok"
run_in "$H6" --env --hub-url http://192.0.2.10:7879/ --env-name mac --hub-token-file "$T/tok" > "$T/out10b" 2>&1 \
  || fail "ligação remota falhou: $(cat "$T/out10b")"
[[ "$(cat "$TOK6")" == cte_remoto ]] || fail "token remoto não gravado"
[[ "$({ stat -c %a "$TOK6" 2>/dev/null || stat -f %Lp "$TOK6"; })" == 600 ]] || fail "hub.token remoto sem 0600"
grep -qx 'hub_url = "http://192.0.2.10:7879"' "$CFG6" || fail "hub_url remoto não gravado: $(cat "$CFG6")"
grep -qx 'env_name = "mac"' "$CFG6" || fail "env_name remoto não gravado"
CFG6_1="$(cat "$CFG6")"
CT_HUB_TOKEN=outro run_in "$H6" --env --hub-url http://192.0.2.10:7879 > "$T/out10c" 2>&1 \
  || fail "segunda rodada falhou: $(cat "$T/out10c")"
[[ "$(cat "$TOK6")" == cte_remoto ]] || fail "trocou o token na segunda rodada"
[[ "$(cat "$CFG6")" == "$CFG6_1" ]] || fail "reescreveu o config na segunda rodada"
run_in "$H6" --env --hub-url http://10.0.0.9:7879 > "$T/out10d" 2>&1 || fail "outro hub_url falhou"
grep -q "já aponta para outro hub" "$T/out10d" || fail "não avisou do outro hub"
run_in "$H6" --hub --env --hub-url http://x:1 > /dev/null 2>&1 && fail "aceitou --hub com --hub-url"

# #246: a mensagem final usa a porta do hub, não a 7879 fixa.
H7="$T/home7"
mkdir -p "$H7/.config/cthub"
printf 'port = 7889\n' > "$H7/.config/cthub/config.toml"
run_in "$H7" --hub --env --env-name porta > "$T/out11" 2>&1 || fail "provision com porta própria falhou: $(cat "$T/out11")"
grep -q 'informe http://<máquina>:7889 ' "$T/out11" || fail "mensagem final sem a porta 7889: $(grep 'UI:' "$T/out11")"
grep -q 'informe http://<máquina>:7879 ' "$T/out11" && fail "mensagem final com a porta fixa"
grep -qx 'hub_url = "http://127.0.0.1:7889"' "$H7/.config/ctower/config.toml" || fail "hub_url sem a porta 7889"

# #199: o hub embute a UI web; sem Node (ou com um antigo), para antes de
# instalar qualquer coisa, com a mensagem do requisito.
H8="$T/home8"; mkdir -p "$H8"
: > "$T/log"
printf '#!/bin/sh\necho v18.19.0\n' > "$F/node18"; chmod +x "$F/node18"
for n in "$T/sem-node" "$F/node18"; do
  if env -i "${FAKE_ENV[@]}" CT_NODE="$n" HOME="$H8" FAKE_ACTIVE="$T/none" \
    bash "$ROOT/scripts/provision/provision.sh" --skip-packages --hub > "$T/out12" 2>&1; then
    fail "instalou o hub sem Node ≥ 20 ($n)"
  fi
  grep -q "Node 20 ou mais novo" "$T/out12" || fail "não explicou o requisito do Node: $(cat "$T/out12")"
  grep -q "docs/hub.md" "$T/out12" || fail "não apontou o docs/hub.md"
done
grep -q "cargo install cthubd\|npm " "$T/log" && fail "instalou algo sem Node: $(cat "$T/log")"

# #443: pasta de docs e compartilhamento Samba. O sudo falso deste bloco
# executa install/tee (em CT_SAMBA_DIR, uma pasta do teste) e simula o
# pdbedit e o smbpasswd; o resto (apt, useradd, testparm, systemctl, ufw) só
# registra.
cat > "$F/sudo-docs" <<'SH'
#!/bin/sh
echo "sudo $*" >> "$FAKE_LOG"
case "$1" in
  install|tee) exec "$@" ;;
  pdbedit) [ -f "$FAKE_SMB_DB" ] && cat "$FAKE_SMB_DB"; exit 0 ;;
  smbpasswd) cat >> "$FAKE_SMB_PW"; echo "ctdocs:999:" > "$FAKE_SMB_DB"; exit 0 ;;
  *) exit 0 ;;
esac
SH
chmod +x "$F/sudo-docs"
H9="$T/home9"; S9="$T/samba"; D9="$T/docs"
mkdir -p "$H9/.config/cthub" "$S9"
printf '[global]\n   workgroup = WORKGROUP\n' > "$S9/smb.conf"
# Um config do hub que já tem a chave vazia: trocada, não repetida.
printf 'docs_dir = ""\nport = 7879\n' > "$H9/.config/cthub/config.toml"
run_docs() {
  env -i "${FAKE_ENV[@]}" CT_SUDO="$F/sudo-docs" CT_SAMBA_DIR="$S9" \
    FAKE_SMB_DB="$T/smb.db" FAKE_SMB_PW="$T/smb.pw" \
    HOME="$H9" FAKE_ACTIVE="$T/none" "$@" \
    bash "$ROOT/scripts/provision/provision.sh" --skip-packages --hub --env --env-name docs \
    --docs-dir "$D9" --docs-samba
}
# Sem senha e sem terminal: para antes de compilar o hub.
: > "$T/log"
run_docs < /dev/null > "$T/out13a" 2>&1 && fail "aceitou --docs-samba sem senha"
grep -q "CT_DOCS_SAMBA_PASSWORD" "$T/out13a" || fail "não explicou a falta da senha: $(cat "$T/out13a")"
grep -q "cargo install cthubd" "$T/log" && fail "compilou o hub sem a senha do Samba"
: > "$T/log"
run_docs CT_DOCS_SAMBA_PASSWORD=senha-de-teste > "$T/out13" 2>&1 || fail "provision com docs falhou: $(cat "$T/out13")"
[[ -d $D9 ]] || fail "não criou a pasta dos docs"
# Decisão 106: o marcador da raiz, vazio e do usuário do hub.
[[ -f $D9/.ct-docs-root && ! -s $D9/.ct-docs-root ]] || fail "não criou o marcador .ct-docs-root"
[[ -O $D9/.ct-docs-root ]] || fail "marcador com outro dono"
HCFG9="$H9/.config/cthub/config.toml"
[[ "$(grep -c '^docs_dir' "$HCFG9")" == 1 ]] || fail "docs_dir repetido ou ausente: $(cat "$HCFG9")"
grep -qx "docs_dir = \"$D9\"" "$HCFG9" || fail "docs_dir não gravado: $(cat "$HCFG9")"
grep -qx "docs_root = \"$D9\"" "$H9/.config/ctower/config.toml" || fail "docs_root não gravado no ambiente"
if python3 -c 'import tomllib' 2>/dev/null; then
  for c in "$HCFG9" "$H9/.config/ctower/config.toml"; do
    python3 -c 'import sys, tomllib; tomllib.load(open(sys.argv[1], "rb"))' "$c" || fail "TOML inválido: $(cat "$c")"
  done
fi
C9="$S9/ctdocs.conf"
grep -q '^\[ctdocs\]' "$C9" || fail "sem o bloco [ctdocs]"
grep -qx "   path = $D9" "$C9" || fail "path errado: $(cat "$C9")"
grep -qx "   valid users = ctdocs" "$C9" || fail "sem valid users"
grep -qx "   force user = ${USER:-teste}" "$C9" || fail "sem force user"
grep -q "^   hosts allow = 127.0.0.1 .*192.168.0.0/16" "$C9" || fail "sem hosts allow da rede local"
grep -qx "   guest ok = no" "$C9" || fail "aceita convidado"
[[ "$(grep -c "^include = $C9\$" "$S9/smb.conf")" == 1 ]] || fail "include ausente ou repetido: $(cat "$S9/smb.conf")"
grep -q "sudo useradd --system .* ctdocs" "$T/log" || fail "não criou o usuário do sistema"
grep -q "sudo smbpasswd -s -a ctdocs" "$T/log" || fail "não criou o usuário do Samba"
[[ "$(cat "$T/smb.pw")" == $'senha-de-teste\nsenha-de-teste' ]] || fail "senha não foi pelo stdin"
grep -q "senha-de-teste" "$T/log" "$T/out13" && fail "a senha vazou no log ou na saída"
grep -q "sudo systemctl reload smbd" "$T/log" || fail "não recarregou o smbd"
grep -q "Docs: monte smb://ctdocs@" "$T/out13" || fail "mensagem final sem a montagem"
HCFG9_1="$(cat "$HCFG9")"; ECFG9_1="$(cat "$H9/.config/ctower/config.toml")"; SMB9_1="$(cat "$S9/smb.conf")"
# De novo: nada muda, a senha não é pedida e o smbd não recarrega.
: > "$T/log"
run_docs < /dev/null > "$T/out13b" 2>&1 || fail "segunda rodada com docs falhou: $(cat "$T/out13b")"
[[ "$(cat "$HCFG9")" == "$HCFG9_1" ]] || fail "reescreveu o config do hub"
[[ "$(cat "$H9/.config/ctower/config.toml")" == "$ECFG9_1" ]] || fail "reescreveu o config do ambiente"
[[ "$(cat "$S9/smb.conf")" == "$SMB9_1" ]] || fail "mexeu no smb.conf de novo"
grep -q "smbpasswd" "$T/log" && fail "trocou a senha do Samba"
grep -q "reload smbd" "$T/log" && fail "recarregou o smbd sem mudança"
grep -q "senha mantida" "$T/out13b" || fail "não avisou da senha mantida"
# O marcador existente (o cthubd também o cria) não é sobrescrito.
printf 'do cthubd\n' > "$D9/.ct-docs-root"
run_docs < /dev/null > "$T/out13d" 2>&1 || fail "rodada com marcador existente falhou: $(cat "$T/out13d")"
[[ "$(cat "$D9/.ct-docs-root")" == "do cthubd" ]] || fail "sobrescreveu o marcador"
grep -q "Marcador" "$T/out13d" && fail "recriou o marcador existente"
# Outra pasta com docs_dir já gravado: mantém a gravada, com aviso.
env -i "${FAKE_ENV[@]}" CT_SUDO="$F/sudo-docs" CT_SAMBA_DIR="$S9" FAKE_SMB_DB="$T/smb.db" FAKE_SMB_PW="$T/smb.pw" \
  HOME="$H9" FAKE_ACTIVE="$T/none" bash "$ROOT/scripts/provision/provision.sh" --skip-packages --hub \
  --docs-dir "$T/outra" > "$T/out13c" 2>&1 || fail "--docs-dir com docs_dir gravado falhou"
grep -q "mantido (ignorei --docs-dir" "$T/out13c" || fail "não avisou do docs_dir existente"
[[ "$(cat "$HCFG9")" == "$HCFG9_1" ]] || fail "trocou o docs_dir gravado"
[[ ! -e "$T/outra" ]] || fail "criou a pasta ignorada"
# Opções recusadas antes de instalar qualquer coisa.
H10="$T/home10"; mkdir -p "$H10"
: > "$T/log"
run_in "$H10" --env --docs-dir "$T/d" > "$T/out14" 2>&1 && fail "aceitou --docs-dir sem --hub"
grep -q "use com --hub" "$T/out14" || fail "não explicou o --hub"
run_in "$H10" --hub --docs-dir relativo > "$T/out14" 2>&1 && fail "aceitou --docs-dir relativo"
grep -q "docs-dir inválido" "$T/out14" || fail "não explicou o --docs-dir inválido"
run_in "$H10" --hub --docs-samba > "$T/out14" 2>&1 && fail "aceitou --docs-samba sem pasta"
grep -q "precisa da pasta" "$T/out14" || fail "não explicou a falta da pasta"
env -i "${FAKE_ENV[@]}" CT_OS=Darwin HOME="$H10" bash "$ROOT/scripts/provision/provision.sh" \
  --skip-packages --hub --docs-dir "$T/d" --docs-samba > "$T/out14" 2>&1 && fail "aceitou --docs-samba no macOS"
grep -q "só para Linux" "$T/out14" || fail "não explicou o macOS"
[[ ! -s "$T/log" ]] || fail "instalou algo com opção de docs inválida: $(cat "$T/log")"

# Em nenhum passo o teste pode chegar ao launchd do usuário (#283).
[[ ! -e "$T/log.launchctl" ]] || fail "chamou o launchctl: $(cat "$T/log.launchctl")"
echo "provision.sh: ok"
