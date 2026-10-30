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

Não altere as versões à mão nem edite o `CHANGELOG.md`: o PR de release faz isso.

## Documentação

O desenho do sistema está em `docs/superpowers/specs/`, a operação do hub em [`docs/hub.md`](docs/hub.md) e o CI em [`docs/ci.md`](docs/ci.md). Mudanças de comportamento devem atualizar o documento correspondente.

## Licença

Ao contribuir, você concorda que sua contribuição será licenciada sob a [Apache-2.0](LICENSE).
