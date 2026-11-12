# Ambiente autônomo (#179): o ctowerd num container Linux, para as sessões
# sem prompts de permissão rodarem longe da máquina de uso diário. Variante de
# produção da imagem de docker/test: binários da release (sem compilar nada),
# HOME num volume próprio e subida automática. Uso: docs/ambiente-autonomo.md.
#
# Contexto do build: esta pasta (compose.yml já aponta para ela).
#
#   CT_VERSION           versão da release do ctowerd/ctower (1.8.0) ou latest
#   CLAUDE_CODE_VERSION  versão do Claude Code (2.1.322), stable ou latest
#
# A release só tem o binário Linux x86_64 (docs/release.md): a imagem é
# linux/amd64, para um host x86_64 (docs/ambiente-autonomo.md).

# --- Binários da release, com o sha256 conferido ----------------------------
FROM debian:bookworm-slim AS release
ARG CT_VERSION=latest
ARG CT_RELEASE_REPO=deep-ion-ai/ctower-agents
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
SHELL ["/bin/bash", "-euo", "pipefail", "-c"]
# Só baixa: roda na arquitetura do host, mas pega sempre o binário x86_64.
RUN arch=x86_64; \
    if [[ $CT_VERSION == latest ]]; then \
      tag="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$CT_RELEASE_REPO/releases/latest")"; \
      tag="${tag##*/}"; \
    else \
      tag="v${CT_VERSION#v}"; \
    fi; \
    [[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "versão inválida: $tag" >&2; exit 1; }; \
    name="ctower-env-$tag-$arch-linux"; \
    base="https://github.com/$CT_RELEASE_REPO/releases/download/$tag"; \
    mkdir -p /tmp/rel /out; cd /tmp/rel || exit 1; \
    curl -fsSLO "$base/$name.tar.gz"; \
    curl -fsSLO "$base/$name.tar.gz.sha256"; \
    sha256sum -c "$name.tar.gz.sha256"; \
    tar -xzf "$name.tar.gz"; \
    install -m 0755 "$name/ctowerd" "$name/ctower" /out/; \
    echo "$tag" > /out/VERSION

# --- Imagem final ------------------------------------------------------------
# A base do Node traz node, npm e npx, para o Claude Code e os jobs. A
# plataforma fixa vale também no builder antigo (sem BuildKit), que ignora o
# `platform` do compose.
# hadolint ignore=DL3029
FROM --platform=linux/amd64 node:22-bookworm-slim
ARG CLAUDE_CODE_VERSION=stable
SHELL ["/bin/bash", "-euo", "pipefail", "-c"]
# O builder antigo ignora o --platform acima: num host arm64 sem BuildKit, a
# imagem sairia arm64 com o ctowerd x86_64. Para aqui, com a saída.
RUN [[ $(uname -m) == x86_64 ]] \
 || { echo "imagem saiu $(uname -m): o ctowerd da release é x86_64. Use o BuildKit (docker buildx) ou DOCKER_DEFAULT_PLATFORM=linux/amd64" >&2; exit 1; }
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl git gnupg jq less openssh-client procps sqlite3 tini unzip \
 && install -d -m 0755 /etc/apt/keyrings \
 && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
      -o /etc/apt/keyrings/githubcli-archive-keyring.gpg \
 && chmod go+r /etc/apt/keyrings/githubcli-archive-keyring.gpg \
 && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
      > /etc/apt/sources.list.d/github-cli.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends gh \
 && rm -rf /var/lib/apt/lists/*

# Claude Code fora do HOME: o HOME é um volume, e o que a imagem pusesse nele
# só seria copiado na primeira subida (a versão nova da imagem não chegaria).
# O instalador roda num HOME descartável; fica só o binário, em /usr/local/bin.
RUN tmp="$(mktemp -d)" \
 && curl -fsSL https://claude.ai/install.sh -o "$tmp/install.sh" \
 && HOME="$tmp" bash "$tmp/install.sh" "$CLAUDE_CODE_VERSION" \
 && install -m 0755 "$(readlink -f "$tmp/.local/bin/claude")" /usr/local/bin/claude \
 && rm -rf "$tmp" \
 && claude --version

COPY --from=release /out/ctowerd /out/ctower /usr/local/bin/
# A base é bookworm (glibc 2.36). Um binário compilado numa distro mais nova
# (Ubuntu 24.04, por exemplo) pede uma glibc que não existe aqui, e o loader
# pode só avisar ("weak version GLIBC_2.39 not found") sem mudar o código de
# saída: por isso a saída também é conferida.
RUN for b in ctowerd ctower; do \
      if ! out="$("$b" --version 2>&1)" || grep -q 'GLIBC' <<<"$out"; then \
        echo "$b não roda nesta imagem (glibc 2.36, bookworm): $out" >&2; \
        echo "o ctower-env da release precisa ser compilado numa distro com glibc <= 2.36 (docs/ambiente-autonomo.md)" >&2; \
        exit 1; \
      fi; \
      echo "$out"; \
    done
COPY --from=release /out/VERSION /usr/local/share/ctower-autonomo/VERSION
COPY entrypoint.sh /usr/local/bin/ct-autonomo

# O uid 1000 já é do usuário node da imagem base.
RUN useradd --create-home --uid 1001 --shell /bin/bash ct
USER ct
WORKDIR /home/ct
# A versão do Claude Code é a da imagem: atualizar é reconstruir.
ENV DISABLE_AUTOUPDATER=1 \
    RUST_LOG=info

EXPOSE 7878
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/ct-autonomo"]
CMD ["serve"]
