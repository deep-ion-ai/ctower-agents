#!/usr/bin/env bash
# Lint de processos externos (docs/ci.md, issue #226).
#
# Todo processo do daemon sai por ct_proc (prazo, stdin nulo, sessão e grupo
# próprios). Por isso `Command::new` só pode aparecer em crates/ct-proc (que
# monta o processo e roda com prazo) e crates/ct-pty (sessões de terminal),
# e nos arquivos da lista abaixo, cada um com o motivo.
#
# Fica de fora o código de teste: as pastas `tests/` e tudo depois do primeiro
# `#[cfg(test)]` de cada arquivo (os módulos de teste ficam no fim).
#
# A lista também falha se uma exceção deixar de ter `Command::new`, para ela
# não envelhecer. Para tirar uma exceção, migre o arquivo para ct_proc e
# remova a linha.
set -euo pipefail
cd "$(dirname "$0")/.."

EXCECOES=(
  "crates/ct-core/build.rs|script de build: git do commit na compilação, sem daemon"
  "crates/ct-jira/src/cli.rs|monta o Command do jira-cli e roda com ct_proc::run (prazo por chamada)"
  "crates/ct-gitlab/src/cli.rs|monta o Command do glab e roda com ct_proc::run (prazo por chamada)"
  "crates/ct-jira/src/fake.rs|dublê de teste do jira-cli, compilado no crate"
  "crates/ct-gitlab/src/fake.rs|dublê de teste do glab, compilado no crate"
  "crates/ct-sessions/src/test_support.rs|suporte aos testes do ct-sessions"
  "crates/ct-sessions/src/usage.rs|statusLine encadeado e claude auth status, com prazo próprio (wait_output), rodam dentro do statusline do Claude Code"
  "crates/ctower/src/hook.rs|hooks do Claude Code, com prazo próprio (run_with_deadline) e stdin nulo"
  "crates/ct-gh/src/lib.rs|gh auth token, com prazo próprio e nova tentativa de ETXTBSY"
  "crates/ct-sessions/src/update.rs|monta o comando do gerenciador de serviço (systemctl/launchctl); quem roda é o atualizador ou o ctower restart"
  "crates/cthubd/src/update.rs|atualização do hub: builds e npm longos e o gerenciador de serviço"
  "crates/ct-update/src/lib.rs|helper git() do atualizador; as chamadas ainda não têm prazo próprio (a migrar para ct_proc)"
  "crates/ct-update/src/runner.rs|pipeline de atualização: builds e downloads longos, cada passo registrado"
  "crates/ct-update/src/launch.rs|dispara o atualizador destacado; não espera o processo"
)

declare -A permitido=()
for item in "${EXCECOES[@]}"; do
  arq="${item%%|*}"
  motivo="${item#*|}"
  if [ -z "$motivo" ]; then
    echo "::error::exceção sem motivo: $arq"
    exit 1
  fi
  permitido["$arq"]="$motivo"
done

falhas=0
usados=" "
while IFS= read -r arq; do
  achado=$(awk '/^#\[cfg\(test\)\]/ { exit } /Command::new\(/ { print FNR": "$0 }' "$arq")
  [ -n "$achado" ] || continue
  if [ -n "${permitido[$arq]:-}" ]; then
    usados+="$arq "
    continue
  fi
  echo "::error file=$arq::Command::new fora de ct-proc/ct-pty e das exceções (use ct_proc::process::exec ou run)"
  echo "$achado" | sed "s|^|$arq:|"
  falhas=1
done < <(git ls-files 'crates/*.rs' | grep -v -E '^crates/(ct-proc|ct-pty)/' | grep -v '/tests/')

for arq in "${!permitido[@]}"; do
  if [[ "$usados" != *" $arq "* ]]; then
    echo "::error file=$arq::exceção sem Command::new (ou arquivo inexistente): remova-a da lista em scripts/lint-processos.sh"
    falhas=1
  fi
done

if [ "$falhas" -ne 0 ]; then
  exit 1
fi
echo "lint-processos: ok ($(echo "$usados" | wc -w) exceções em uso)"
