# Publicar uma versão

Para quem mantém o repositório: como uma versão nasce dos commits, sai como release e volta para a `develop`. O CI que roda em cada etapa está em [`ci.md`](ci.md).

A versão segue SemVer e sai dos commits ([Conventional Commits](https://www.conventionalcommits.org/pt-br/)), pelo [release-please](https://github.com/googleapis/release-please). Ninguém sobe versão nem cria tag à mão.

1. **`develop → main`:** o merge na `main` dispara o workflow `release`, que abre ou atualiza o PR **"chore(main): release X.Y.Z"**. O PR sobe a versão em `Cargo.toml` (workspace), `ui/package.json`, `ui/src-tauri/Cargo.toml` e `ui/src-tauri/tauri.conf.json`, atualiza o `CHANGELOG.md` e, num commit à parte, os três lockfiles. A cada merge novo na `main`, o mesmo PR é refeito.
2. **Mesclar o PR de release** é o botão: nem todo merge na `main` vira release. Ao mesclar, o release-please cria a tag `vX.Y.Z` e a release, ainda em rascunho.
3. **Publicação:** o job `cthubd` do `release` anexa o binário do hub para Linux (`cthubd-vX.Y.Z-<arch>-linux.tar.gz`, com a unidade systemd, e o `.sha256`). Depois, o `release` chama o workflow `ui`, que compila no macOS e envia o `.dmg`, o `.app.tar.gz` assinado (+ `.sig`) e o `latest.json` (`darwin-aarch64`), e só então tira a release do rascunho. Até ali, o updater do app continua vendo a versão anterior. O `ctowerd`, o `ctower` e o `cthubd` têm a versão do workspace. Os atualizadores (`ctower update`, `cthubd update`) compilam do fonte e não usam os binários da release. Não há binário do hub para macOS.
4. **Back-merge:** o `release` abre o PR **"chore: back-merge vX.Y.Z"** (`main → develop`) com o commit de release. Mescle com merge commit (não squash), para o próximo `develop → main` não dar conflito em versão nem no `CHANGELOG.md`.

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
