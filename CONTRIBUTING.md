# Contribuindo

Obrigado pelo interesse no Control Tower. Este guia resume como propor mudanças.

## Fluxo

1. Abra uma issue descrevendo o problema ou a proposta antes de mudanças grandes.
2. Faça um fork e crie a branch a partir da `develop` (`feat/123-descricao`, `fix/123-descricao`).
3. Abra o PR contra a `develop`. A `main` só recebe o merge `develop → main`, que dispara a release.
4. O CI roda nos runners do GitHub. Em PRs de fork, um mantenedor precisa aprovar a execução dos workflows.

## Commits

Usamos [Conventional Commits](https://www.conventionalcommits.org/pt-br/) (`feat:`, `fix:`, `docs:`, `chore:`, `test:`), porque o `release-please` gera as versões e o `CHANGELOG.md` a partir deles. Referencie a issue no corpo (`closes #123`).

## Antes de enviar o PR

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked

cd ui
npm ci
npm run typecheck
npm test
```

Se você mudar tipos Rust expostos à UI, regenere e inclua os tipos TypeScript:

```sh
cargo test -p ct-server --features ts --locked export_ts
cargo test -p cthubd --features ts --locked export_ts
```

Não altere as versões à mão nem edite o `CHANGELOG.md`: o PR de release faz isso. O processo completo de publicar uma versão está em [`docs/release.md`](docs/release.md).

## App desktop e UI web (`ui/`)

- **Frontend:** `cd ui && npm ci && npm test` (vitest) e `npm run typecheck`. Os tipos do protocolo do ambiente em `ui/src/protocol/gen` são gerados com `cargo test -p ct-server --features ts export_ts`, e os do `/ws` do hub em `ui/src/protocol/hub-gen` com `cargo test -p cthubd --features ts export_ts`; o CI falha se estiverem desatualizados.
- **Lado Rust do app:** `cargo test --manifest-path ui/src-tauri/Cargo.toml` (fora do workspace; no Linux precisa de `libwebkit2gtk-4.1-dev`).
- **Rodar o app em desenvolvimento:** `cd ui && npx tauri dev`. Pré-requisitos de cada sistema e como gerar os pacotes: [`docs/app.md`](docs/app.md).
- **Roteiro de teste manual:** [`docs/ui-roteiro-manual.md`](docs/ui-roteiro-manual.md).

## Documentação

O índice da documentação está em [`docs/README.md`](docs/README.md). O desenho do sistema está em `docs/superpowers/specs/`, a operação do ambiente em [`docs/ambiente.md`](docs/ambiente.md), a do hub em [`docs/hub.md`](docs/hub.md), o CI em [`docs/ci.md`](docs/ci.md) e a publicação de versões em [`docs/release.md`](docs/release.md). Mudanças de comportamento devem atualizar o documento correspondente.

## Licença

Ao contribuir, você concorda que sua contribuição será licenciada sob a [Apache-2.0](LICENSE).
