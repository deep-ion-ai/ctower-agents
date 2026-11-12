# Publicar uma versão

Para quem mantém o repositório: como uma versão nasce dos commits, sai como release e volta para a `develop`. O CI que roda em cada etapa está em [`ci.md`](ci.md).

A versão segue SemVer e sai dos commits ([Conventional Commits](https://www.conventionalcommits.org/pt-br/)), pelo [release-please](https://github.com/googleapis/release-please). Ninguém sobe versão nem cria tag à mão.

## O fluxo

1. **`develop → main`:** o merge na `main` dispara o workflow `release`, que abre ou atualiza o PR **"chore(main): release X.Y.Z"**. O PR sobe a versão em `Cargo.toml` (workspace), `ui/package.json`, `ui/src-tauri/Cargo.toml` e `ui/src-tauri/tauri.conf.json`, atualiza o `CHANGELOG.md` e, num commit à parte, os três lockfiles. A cada merge novo na `main`, o mesmo PR é refeito.
2. **Mesclar o PR de release** é o botão: nem todo merge na `main` vira release. Ao mesclar, o release-please cria a tag `vX.Y.Z` e a release, ainda em rascunho.
3. **Publicação:** o job `cthubd` do `release` anexa o binário do hub para Linux (`cthubd-vX.Y.Z-<arch>-linux.tar.gz`, com a unidade systemd, e o `.sha256`). Depois, o `release` chama o workflow `ui`, que compila no macOS e envia o `.dmg`, o `.app.tar.gz` assinado (+ `.sig`) e o `latest.json` (`darwin-aarch64`), e só então tira a release do rascunho. Até ali, o updater do app continua vendo a versão anterior. O `ctowerd`, o `ctower` e o `cthubd` têm a versão do workspace. Os atualizadores (`ctower update`, `cthubd update`) compilam do fonte e não usam os binários da release. Não há binário do hub para macOS.
4. **Back-merge:** o `release` abre o PR **"chore: back-merge vX.Y.Z"** (`main → develop`) com o commit de release. Mescle com merge commit (não squash), para o próximo `develop → main` não dar conflito em versão nem no `CHANGELOG.md`.

## Pelo Control Tower: a sessão de release (#204)

Em vez de fazer cada passo à mão, a release pode correr numa **sessão de release** do Control Tower. O agente decide quando avançar, escreve o corpo do PR de promoção, diagnostica falhas e relata. Ele não escolhe como mesclar: os merges e as aprovações só passam pelas etapas do `ctower release`, que conferem pré-condições fixas e usam sempre **merge commit preso ao SHA conferido**, nunca squash, rebase, push direto nem bypass de regras.

**Preparar o projeto (uma vez):**

1. **Token exclusivo.** Crie um token do GitHub só para a release: um fine-grained PAT de uma conta de máquina (ou um GitHub App), restrito ao repositório, com **Contents: read and write**, **Pull requests: read and write**, **Actions: read and write** (aprovar e reexecutar runs) e **Metadata: read**. Os PRs e merges feitos com ele **disparam** os workflows (os feitos pelo `GITHUB_TOKEN` não disparam), e o autor deles é a conta do token.
2. **Secret do projeto.** No hub, cadastre o token como secret do projeto (tela do projeto › **Secrets e variáveis**) com o nome `CT_RELEASE_GH_TOKEN` (ou o nome configurado em `token_secret`). Com a release ligada, esse nome é **reservado**: só a sessão de release o recebe, como variável e também como `GH_TOKEN`. As outras sessões do projeto não o veem em variável de ambiente nem pelo `ctower secret get`.
3. **Ligar a release.** Em Configurações do projeto › **Release**, ligue a sessão de release e confira o fluxo: `source`/`target` (`develop`/`main`), o head do PR de release (`release-please--branches--{target}`), o back-merge, a aprovação dos runs pendentes, o nome do secret e os prazos de cada espera (30 min por padrão). Só projetos GitHub + GitHub têm a seção; o hub recusa o campo nos projetos Jira + GitLab.

**Rodar:** o botão **Criar release** na tela do projeto mostra antes o estado (diferença de conteúdo entre `develop` e `main`, PRs abertos do fluxo, runs pendentes e a última release) e inicia a sessão num ambiente atribuído. O botão fica desabilitado, com o motivo, sem a release ligada, sem o secret ou com outra sessão de release viva no projeto (só uma por projeto).

A sessão roda no clone principal, só para leitura, e segue o template **Sessão de release** (global em Configurações › Templates, ou o do projeto). As etapas, na ordem:

| Etapa | O que faz |
| --- | --- |
| `ctower release status` | Estado em JSON: diferença de **conteúdo** `develop`×`main` (compara as árvores; um back-merge sozinho não conta), PRs abertos do fluxo, runs em `action_required`, a release mais recente e a próxima etapa (`next`). |
| `ctower release promote --body-file <arquivo>` | Abre (ou reaproveita) o PR `develop → main` com o corpo escrito pelo agente. Recusa sem diferença de conteúdo ("nada a lançar") ou com back-merge pendente (`main` à frente da `develop`). |
| `ctower release merge-promotion [--sha <sha>]` | Mescla o PR de promoção com merge commit, preso ao SHA, só com os checks obrigatórios de `main` verdes. |
| `ctower release wait-release-pr` | Espera os runs do workflow `release` no head da `main` terminarem (release-please e commit dos lockfiles) e o PR de release aberto. |
| `ctower release approve-runs <pr>` | Aprova os runs em `action_required` do head atual do PR, espera terminarem e reexecuta uma vez os que falharam na primeira tentativa (a corrida conhecida do check de versões antes do commit dos lockfiles). |
| `ctower release merge-release [--sha <sha>]` | Mescla o PR de release com merge commit, preso ao SHA, com os runs aprovados e verdes. |
| `ctower release wait-publish` | Espera a tag `vX.Y.Z`, os jobs de binários e o `publish` tirarem a release do rascunho; relata o job que falhou. |
| `ctower release merge-back [--sha <sha>]` | Espera o PR `main → develop`, aprova os runs dele e mescla com merge commit, preso ao SHA. |

- **Saída:** JSON no stdout com `step`, `result` (`ok`, `noop`, `refused` ou `failed`), `message`, `pr`, `sha` e `details`; código `0` (ok ou nada a fazer), `3` (recusada: uma pré-condição não vale, nada foi feito) ou `1` (falha: erro do GitHub, prazo esgotado ou fora da sessão de release).
- **Idempotência:** cada etapa confere o estado no GitHub; rodar de novo depois de sucesso devolve `noop`. A sessão pode ser retomada e continua da etapa em que parou (`next` do `status`).
- **Ledger:** cada etapa grava o evento `release_step` no work item da sessão de release, com a etapa, o resultado, o PR e o SHA.
- **Preso ao SHA:** o merge usa o `sha` do head conferido; se o head mudar entre a conferência e o merge (ou não bater com o `--sha` informado), o GitHub recusa e a etapa responde `refused`.
- **Guarda-corpo:** na sessão de release, `git push`, `git merge`, `git commit` (e as outras escritas no clone), `gh pr merge`, `gh pr review`, `gh release create|edit|delete|upload` e toda escrita pela `gh api` ficam bloqueados. Continuam livres a leitura, `gh run rerun <id> --failed`, `gh workflow run` e o `ctower release`. Fora da sessão de release, o `ctower release` recusa.
- **Quando para:** conflito, check obrigatório vermelho, run que falhou de novo ou prazo esgotado. O agente não corrige código nem CI: relata e termina. No fim, resume a versão, os links da release e dos três PRs, os runs aprovados e as decisões registradas.

## Versões, falhas e configuração

Enquanto a versão for `0.x` (`bump-minor-pre-major`):

| Commit | Exemplo | Versão |
| --- | --- | --- |
| `fix:`, `perf:`, `revert:` | `fix(ui): reconecta depois de suspender` | patch (0.3.0 → 0.3.1) |
| `feat:` | `feat(ct-gh): sincroniza milestones` | minor (0.3.1 → 0.4.0) |
| `!` depois do tipo ou rodapé `BREAKING CHANGE:` (em qualquer tipo) | `feat(ct-server)!: remove o método session.peek` | minor (0.4.0 → 0.5.0); a partir da 1.0, major |
| `docs:`, `chore:`, `ci:`, `test:`, `refactor:`, `build:`, `style:`, merge e tipos fora da convenção | `docs: atualiza o README` | nenhuma: sozinhos, não geram release |

- **Forçar uma versão:** um commit com `Release-As: X.Y.Z` no corpo (ex.: `git commit --allow-empty -m "chore: release 1.0.0" -m "Release-As: 1.0.0"`), que chega à `main` pelo fluxo normal. O PR de release passa a propor essa versão.
- **Binário do cthubd que falhou:** o `publish` não roda e a release fica em rascunho. Reexecute os jobs que falharam no run do `release` (**Re-run failed jobs**).
- **Publicação que falhou:** a tag e a release em rascunho já existem. Rode **Actions → ui → Run workflow** com `tag: vX.Y.Z`. O workflow recompila, reenvia os arquivos (`--clobber`) e publica a release. Não crie nem empurre tag à mão: a tag tem de apontar para o commit de release.
- **Versões divergentes:** `python3 scripts/check-versions.py` confere os arquivos de versão e os lockfiles; o CI roda o mesmo check.
- **Configuração:** `release-please-config.json` e `.release-please-manifest.json` (última versão publicada; o release-please o atualiza no PR de release). O repositório precisa de **Settings → Actions → General → Allow GitHub Actions to create and approve pull requests**. O PR de release é aberto pelo `GITHUB_TOKEN`, e os runs de `ci` e `ui` nele ficam em **action_required**, à espera de aprovação. Eles não são exigidos: a versão, o `CHANGELOG.md` e os lockfiles são conferidos pelo check de versões dentro do job `lockfiles` do `release`, e a `main` não tem checks obrigatórios. Mescle o PR de release sem aprovar os runs. Se quiser os checks completos, aprove-os na aba **Checks** do PR (issue #159).
- **Assinatura do updater (sem notarização):** usa os secrets `TAURI_SIGNING_PRIVATE_KEY` e `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, e a chave pública fica em `plugins.updater.pubkey`. Para trocar a chave, gere um par novo com `npx tauri signer generate -w ~/.tauri/control-tower-updater.key`, atualize a pubkey e os secrets e reinstale o app à mão uma vez.
