# Hub e ambiente de teste (#176): uma imagem só, com o cthubd (UI web
# embutida), o ctowerd e o ctower compilados do checkout. O papel sai do
# comando (`hub` ou `env`); veja entrypoint.sh e README.md.
#
# Contexto do build: a raiz do repositório (compose.yml já aponta para ela);
# o .dockerignore da raiz tira target/, node_modules e .git do contexto.

# --- UI web embutida no cthubd ------------------------------------------------
FROM node:22-bookworm-slim AS web
WORKDIR /src/ui
COPY ui/ ./
# O npm ci apaga o node_modules que vier no contexto (o .dockerignore já o
# exclui) e instala o do Linux.
RUN npm ci --no-audit --no-fund && npm run build:web

# --- Binários ----------------------------------------------------------------
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
COPY --from=web /src/ui/dist-web ui/dist-web
# O rust-toolchain.toml pede o canal stable com clippy e rustfmt; o build não
# precisa deles, então o toolchain da imagem basta.
ENV RUSTUP_TOOLCHAIN=stable CTHUB_WEB_REQUIRED=1
# Sem cache mounts, para valer também no builder antigo (sem BuildKit).
RUN cargo build --release --locked -p cthubd -p ctowerd -p ctower \
 && mkdir -p /out \
 && cp target/release/cthubd target/release/ctowerd target/release/ctower /out/

# --- Imagem final ------------------------------------------------------------
FROM debian:bookworm-slim
ARG CLAUDE_CODE=1
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl git gnupg less openssh-client procps sqlite3 tini \
 && install -d -m 0755 /etc/apt/keyrings \
 && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
      -o /etc/apt/keyrings/githubcli-archive-keyring.gpg \
 && chmod go+r /etc/apt/keyrings/githubcli-archive-keyring.gpg \
 && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
      > /etc/apt/sources.list.d/github-cli.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends gh \
 && rm -rf /var/lib/apt/lists/*

# Node para o Claude Code e os jobs do ambiente.
COPY --from=web /usr/local/bin/node /usr/local/bin/node

# /link: volume só do teste, por onde o hub passa os tokens ao ambiente. Criada
# aqui para o volume nascer com o dono certo.
RUN useradd --create-home --shell /bin/bash ct \
 && install -d -o ct -g ct -m 0700 /link
COPY --from=build /out/ /usr/local/bin/
COPY docker/test/entrypoint.sh /usr/local/bin/ct-test-entrypoint

USER ct
WORKDIR /home/ct
ENV PATH=/home/ct/.local/bin:$PATH
# Claude Code no HOME do usuário (o mesmo instalador do provision.sh). Fica
# na imagem; o login vai para o volume do HOME do ambiente.
RUN if [ "$CLAUDE_CODE" = 1 ]; then curl -fsSL https://claude.ai/install.sh | bash; fi \
 && mkdir -p /home/ct/.local/bin

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/ct-test-entrypoint"]
CMD ["env"]
