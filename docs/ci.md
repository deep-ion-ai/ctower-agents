# CI: quando cada job roda e quais runners usa

O CI roda nos **runners hospedados do GitHub**, grátis em repositório público: `ubuntu-22.04` (Linux x86_64) e `macos-15` (Apple Silicon). Não há Windows. Runners próprios continuam possíveis por variável do repositório (veja "Escolher o runner"), mas só em repositório privado.

| Evento | Linux (`rust`, tipos TS, `web`) | macOS (`rust`) | macOS (`macos`, Tauri) |
|---|---|---|---|
| PR para `wip/*` ou `develop` | sim, conforme os arquivos mudados | sim, conforme os arquivos mudados | não |
| PR `develop → main` | sim, sempre | sim | sim |
| Push em `develop`/`main` | sim | sim | não |
| Release (`ui` chamado pelo `release`, ou disparo manual do `ui` com `tag`) | `web`; `cthubd` (binário do hub, só no `release`) | não | sim, gera o app e publica a release |
| Disparo manual (`workflow_dispatch`) | sim | sim | sim |
| PR de release do release-please | só com aprovação manual (*action_required*), não exigido | não | não |

O job `rust` também roda o `scripts/provision/test-provision.sh`, que executa o `provision.sh --hub --env` com HOME temporário e `sudo`, `systemctl` e `cargo` falsos (usa os binários de `target/debug`). O teste confere que o ambiente sai ligado ao hub local (o token gravado vale no `/sync/config` de um `cthubd` de verdade) e que rodar de novo não muda nada. Ele também roda à mão, sem tocar no sistema: `cargo build -p cthubd -p ctowerd -p ctower && scripts/provision/test-provision.sh`. Mudanças em `scripts/provision/**` e `packaging/**` disparam o job.

O job `cthubd` do `release` (`release.yml`) compila o hub na tag e anexa `cthubd-vX.Y.Z-<arch>-linux.tar.gz` e o `.sha256` à release em rascunho, antes do `publish`. O `publish` (o workflow `ui`, que tira a release do rascunho) depende dele. Se o job `cthubd` falhar, a release fica em rascunho, sem o app. Corrija a causa (falha de build, runner indisponível) e rode **Re-run failed jobs** no run do `release`: o job reenvia o binário com `--clobber`, e em seguida o `publish` roda. Não crie tag nem release à mão. Ele roda no `ubuntu-22.04`, então o binário exige glibc 2.35 ou mais nova (Ubuntu 22.04 em diante). O job `envbins` anexa `ctower-env-vX.Y.Z-<arch>-<os>.tar.gz` (+ `.sha256`) com `ctowerd`, `ctower` e a unidade de serviço, para macOS aarch64 (`macos-15`) e Linux x86_64; `provision.sh --env` e `ctower update` os usam, com `cargo install` de fallback (`--from-source` força). Não há binário do hub para macOS; num Mac, o hub é compilado localmente (`docs/hub.md`).

Um job que não roda aparece como *skipped*, não como pendente. Quando um PR mexer em código de plataforma (PTY, área de transferência, abrir URL), dispare as plataformas à mão antes de mesclar:

```sh
gh workflow run ci --ref <branch>
gh workflow run ui --ref <branch>
```

## App de teste de uma branch

Para testar o app de uma branch antes do merge (ex.: a de um épico), sem criar tag nem release:

```sh
gh workflow run ui --ref <branch> -f preview=true
gh run download <run-id> -n control-tower-macos-preview -D <pasta>
```

O job `macos` gera o `.dmg` sem os artefatos do updater e o deixa como artefato `control-tower-macos-preview` do run (14 dias). A versão é a do `tauri.conf.json`. Como o app não é notarizado, na primeira abertura libere-o em Ajustes do Sistema → Privacidade e Segurança, ou rode `xattr -dr com.apple.quarantine "/Applications/Control Tower.app"`.

## PR de release (release-please)

O release-please abre o PR de release com o `GITHUB_TOKEN`. Os runs de `ci` e `ui` que ele dispara ficam em *action_required* e só rodam se alguém aprovar na aba Checks. Eles não são exigidos (issue #159). O PR só muda versão, `CHANGELOG.md` e lockfiles, e o job `lockfiles` do `release` já roda o `scripts/check-versions.py` na branch do PR. Num repositório público a proteção de branch e os rulesets estão disponíveis, então a `main` pode exigir checks; nesse caso exclua o PR de release da regra (veja abaixo).

Se a `main` passar a exigir checks, exclua o PR de release da regra ou reavalie dar ao release-please um token que dispare o CI (GitHub App ou PAT). Hoje, o épico #140 decidiu não usar nenhum dos dois.

## Escolher o runner: variáveis do repositório

Cada plataforma lê uma variável. Sem ela, o job usa o runner hospedado do GitHub.

| Variável | Jobs | Sem a variável |
|---|---|---|
| `CI_RUNNER_LINUX` | `changes`, `rust` (Linux), `web`, `release` (inclusive o `cthubd`) | `"ubuntu-22.04"` |
| `CI_RUNNER_MACOS` | `rust` (macOS), `macos` (Tauri), `envbins` (macOS) | `"macos-15"` |

O valor é **JSON**: um label entre aspas ou uma lista de labels que o runner precisa ter. Um valor que não seja JSON válido (ex.: `macos` sem aspas) quebra o workflow.

## Segurança em repositório público

- **Não registre runner self-hosted** num repositório público: o runner executa o código de qualquer PR, inclusive de forks, com acesso à rede e aos arquivos da máquina. Use as variáveis acima apenas em repositório privado.
- Em *Settings → Actions → General → Fork pull request workflows*, exija aprovação para colaboradores externos.
- PRs de fork não recebem *secrets*. Os secrets `TAURI_SIGNING_PRIVATE_KEY` e `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` só chegam a jobs de `push`, `workflow_dispatch` e release.

## Runners próprios (opcional, só repositório privado)

Registre o runner **no repositório** (Settings → Actions → Runners → *New self-hosted runner*), não na organização. A página mostra o download e o token de registro (vale 1 hora). O runner já recebe os labels `self-hosted`, o sistema (`linux`, `macos`) e a arquitetura; acrescente `ctower` e aponte a variável correspondente:

```sh
gh variable set CI_RUNNER_MACOS --body '["self-hosted","macos","ctower"]'
gh variable set CI_RUNNER_LINUX --body '["self-hosted","linux","ctower"]'
```

Cuidados:
- O runner executa o código de qualquer PR. Rode o serviço com um usuário sem privilégio de administrador (no Linux, um `ghrunner` sem sudo).
- Os workflows usam `cache-bin: false` no `rust-cache` porque, num runner próprio, o `CARGO_HOME` é o do usuário e o pós-job do cache apagaria de `~/.cargo/bin` o que não foi instalado no job (rustup, `ctowerd`, `ctower`).
- Linux: `build-essential`, `pkg-config`, `libssl-dev`, `libwebkit2gtk-4.1-dev`, Git e Python 3.11+; no WSL, o systemd ligado. macOS: Xcode Command Line Tools e rustup; o job `macos` gera só `darwin-aarch64`.
- Um runner desligado deixa os jobs dele na fila.
