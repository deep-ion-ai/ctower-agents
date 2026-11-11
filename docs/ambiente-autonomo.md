# Ambiente autônomo em container (Linux)

As sessões autônomas do piloto automático (épico #178) rodam sem prompts de permissão. Por isso elas ficam num ambiente próprio, longe da máquina de uso diário: um `ctowerd` num container Docker, ligado ao hub como qualquer outro ambiente (#179). Os arquivos estão em [`docker/autonomo/`](../docker/autonomo/).

- **Binários da release:** a imagem baixa `ctower-env-vX.Y.Z-x86_64-linux.tar.gz` da release ([release.md](release.md)) e confere o `.sha256`. Nada é compilado.
- **HOME num volume próprio** (`ct-autonomo_home`): config, ledger, worktrees, clones e as configurações do Claude Code ficam ali. Nada do HOME do host é montado.
- **Subida automática:** `restart: unless-stopped` no compose e, opcional, uma unidade systemd (`ct-autonomo.service`).
- **Login do Claude por token** (`CLAUDE_CODE_OAUTH_TOKEN`) e **`gh` com token fine-grained** (`GH_TOKEN`), só com os repositórios dos projetos autônomos. Os dois entram como secrets do compose e ficam só no ambiente do processo. Eles nunca vão para o volume, para o `.env` nem para o repositório.
- **Statusline** instalado a cada subida, para o uso do plano chegar ao hub.
- **Sem prompts de permissão** nas sessões, por padrão (`CT_AUTONOMO_SEM_PROMPTS=1`): o Claude Code do container sobe com `permissions.defaultMode = "bypassPermissions"`. O isolamento é o container, e o alcance é o dos tokens.

A imagem é `linux/amd64`, porque a release só tem o `ctower-env` para Linux x86_64: use um host x86_64. Num host arm64, o builder antigo (sem BuildKit) gera a imagem na arquitetura do host, e o build para com a mensagem "imagem saiu aarch64". Com BuildKit e emulação pelo QEMU, o instalador do Claude Code (um binário do Bun) cai com falha de segmentação.

## 1. Gerar os tokens

São quatro. Os dois logins não vão para arquivo nenhum: guarde-os no gerenciador de senhas e cole no shell só na hora do `up` (passo 3). Os dois do hub vão direto do `cthubd` para o container, por um pipe, e ficam no config do volume, como em qualquer ambiente.

| Token | De onde vem | Quando |
|---|---|---|
| `CLAUDE_CODE_OAUTH_TOKEN` | `claude setup-token`, numa máquina com navegador, logado na conta do plano. O comando abre o login e imprime um token de longa duração (`sk-ant-oat01-...`). | sempre que o container é criado |
| `GH_TOKEN` | GitHub → **Settings → Developer settings → Fine-grained tokens**. Em **Repository access**, escolha **Only select repositories** e marque só os repositórios dos projetos autônomos (por exemplo, `acme/acme-web`). Permissões: **Contents**, **Pull requests** e **Issues** em leitura e escrita, **Metadata** em leitura e, se o CI dos PRs importa, **Actions** e **Commit statuses** em leitura. | sempre que o container é criado |
| de sync | no hub, `cthubd add-env linux-autonomo` (duas linhas: o token e o `env_id`) | só na primeira subida |
| de agente | no hub, `cthubd agent-token linux-autonomo` | só na primeira subida (opcional: o gestor de projetos usa) |

O nome `linux-autonomo` é o padrão de `CT_ENV_NAME`. Se usar outro nome, use o mesmo nos dois comandos do hub.

### Conta do plano

Com login por token, o `claude auth status` responde `loggedIn: true` e `authMethod: "oauth_token"`, mas sem `orgId` nem `email`. Sem a conta, o hub não junta as leituras do plano do container com as dos outros ambientes. Por isso, o `ctowerd` lê a conta de duas variáveis, que não são segredo:

```sh
# Num ambiente com login normal na mesma conta:
claude auth status | jq -r '.orgId, .email'
```

Ponha os dois valores em `CTOWER_CLAUDE_ORG_ID` e `CTOWER_CLAUDE_EMAIL` no `.env` (passo 2). Com os mesmos valores, a chave da conta (`orgId/email`) é a mesma dos outros ambientes, e o hub junta as leituras. Os detalhes estão em [ambiente.md](ambiente.md), na seção "Uso do plano".

## 2. Configurar

Num host Linux com Docker e o plugin `compose`:

```sh
git clone https://github.com/deep-ion-ai/ctower-agents.git
cd ctower-agents/docker/autonomo
cp exemplo.env .env      # só variáveis sem segredo; o .env fica fora do git
$EDITOR .env
```

No `.env`, ajuste:

- `CT_VERSION`: a versão da release, sem o `v` (por exemplo, `1.8.0`). Use a mesma major do hub.
- `CT_HUB_URL`: a URL do hub, lida só na primeira subida. Para um hub em outra máquina, use `http://<ip ou nome>:7879`. Para um hub nesta mesma máquina, use `http://host.docker.internal:7879`; nesse caso, o `bind` do hub precisa aceitar a rede do Docker (por exemplo, o IP da ponte `docker0`).
- `CTOWER_CLAUDE_ORG_ID` e `CTOWER_CLAUDE_EMAIL`: a conta do plano (acima).
- `CT_GIT_NAME` e `CT_GIT_EMAIL`: o autor dos commits das sessões.

## 3. Subir

Leia os dois logins sem eco, para eles não ficarem no histórico do shell, e suba:

```sh
read -rs CLAUDE_CODE_OAUTH_TOKEN && export CLAUDE_CODE_OAUTH_TOKEN
read -rs GH_TOKEN && export GH_TOKEN
docker compose up -d --build
unset CLAUDE_CODE_OAUTH_TOKEN GH_TOKEN
docker compose logs -f
```

Na primeira subida, o entrypoint grava `env_name`, `hub_url`, `bind = "0.0.0.0"` e `port = 7878` no `config.toml` e fica esperando o token de sync, com a mensagem "esperando o token de sync". Ligue o ambiente ao hub como no `provision.sh --env --hub-url`, com os tokens gerados no hub:

```sh
# Hub alcançável por ssh (o token não passa pelo terminal):
ssh hub.example.com cthubd add-env linux-autonomo | docker compose exec -T autonomo ctower hub set-token
ssh hub.example.com cthubd agent-token linux-autonomo | docker compose exec -T autonomo ctower hub set-agent-token

# Sem ssh: rode o cthubd no hub, e aqui cole a saída e termine com Ctrl-D.
docker compose exec -T autonomo ctower hub set-token
docker compose exec -T autonomo ctower hub set-agent-token
```

Se o `cthubd` não está no `PATH` do ssh, use o caminho completo (`~/.cargo/bin/cthubd`, na instalação do `provision.sh`). O `ctower hub set-token` grava o token e o `env_id` em `~/.config/ctower` (`0600`, no volume). Ele pede para reiniciar o `ctowerd`, mas aqui não precisa: o entrypoint ainda não o subiu. Em seguida, o entrypoint:

1. instala o statusline e marca o onboarding do Claude Code como feito;
2. liga o `gh` como helper de credencial do `git` (`gh auth setup-git`). O helper lê o `GH_TOKEN` do ambiente, e o `~/.gitconfig` não guarda token;
3. sobe o `ctowerd`, e o ambiente aparece online no hub.

Nas subidas seguintes, o que já foi gravado não muda. Quem roda o container sem o compose pode passar os tokens do hub em `CT_HUB_TOKEN` e `CT_HUB_AGENT_TOKEN` (como no `provision.sh`), em vez do pipe: o entrypoint os grava e os tira do ambiente antes de subir o `ctowerd`.

Para conferir:

```sh
docker compose exec autonomo ct-autonomo verificar acme/repo-fora-do-escopo
```

O `verificar` mostra o login do Claude (só `loggedIn` e `authMethod`), de onde vem a conta do plano, o usuário do `gh`, a ligação ao hub, o statusline e o `ctower status`. Com um `owner/repo` fora do escopo do token, ele confere que o `gh` **não** alcança esse repositório. Se alcançar, o token não está restrito, e o comando sai com erro.

Para abrir um shell com os mesmos logins das sessões: `docker compose exec autonomo ct-autonomo bash`.

Depois de ligado, atribua no hub os projetos autônomos a `linux-autonomo` (docs/hub.md, "Atribuições").

### Unidade systemd (opcional)

O Docker sobe o container de novo depois de um reboot, por causa do `restart: unless-stopped`, desde que o serviço do Docker esteja habilitado no boot. Para ter o ambiente sob o systemd (`systemctl start|stop|status ct-autonomo`):

```sh
sed "s|@DIR@|$PWD|g" ct-autonomo.service | sudo tee /etc/systemd/system/ct-autonomo.service
sudo systemctl daemon-reload
sudo systemctl enable ct-autonomo
```

A unidade roda `docker compose up -d --no-recreate --no-build` e não recebe tokens: ela reaproveita o container já criado, com os secrets dele. Criar o container (primeira subida, versão nova ou token novo) é sempre no shell, como no passo 3.

## 4. Atualizar a versão

```sh
cd ctower-agents/docker/autonomo
git pull                                  # o Dockerfile e o entrypoint da versão nova
$EDITOR .env                              # CT_VERSION=<versão nova> (e CLAUDE_CODE_VERSION, se quiser)
read -rs CLAUDE_CODE_OAUTH_TOKEN && export CLAUDE_CODE_OAUTH_TOKEN
read -rs GH_TOKEN && export GH_TOKEN
docker compose up -d --build
unset CLAUDE_CODE_OAUTH_TOKEN GH_TOKEN
```

O container é recriado com a imagem nova, e o volume continua: config, ledger e worktrees ficam. Os tokens do hub não são pedidos de novo. Sem os dois logins no shell, o compose recusa criar o container ("environment variable ... is not set"); o container antigo pode já ter sido removido, então rode de novo com os logins. Antes, confira na UI do hub que não há sessão rodando no ambiente: recriar o container encerra as sessões, que voltam como interrompidas.

O Claude Code dentro da imagem não se atualiza sozinho (`DISABLE_AUTOUPDATER=1`). A versão é a do build: `CLAUDE_CODE_VERSION=stable`, `latest` ou um número. Com `stable` ou `latest`, o Docker reaproveita a camada do build anterior. Para trazer a versão nova, rode `docker compose build --no-cache` antes do `up`.

O `ctower update` e a atualização pela UI não se aplicam ao container: a versão é a da imagem.

Para **trocar um login** (o token do Claude expirou, ou o fine-grained do GitHub), gere o novo e rode `docker compose up -d --force-recreate` com os dois logins no shell. Sem o `--force-recreate`, o compose não recria o container, porque o valor de um secret não entra na comparação.

## 5. Apagar tudo

```sh
cd ctower-agents/docker/autonomo
docker compose down -v --rmi all
sudo systemctl disable --now ct-autonomo 2>/dev/null; sudo rm -f /etc/systemd/system/ct-autonomo.service
```

O `-v` apaga o volume do HOME: ledger, worktrees, clones e os tokens do hub. O `--rmi all` apaga a imagem. Depois, revogue o que ficou fora do host:

- no hub, `cthubd revoke-env linux-autonomo`;
- no GitHub, apague o token fine-grained;
- o token do `claude setup-token` continua valendo até expirar, mesmo com o container apagado: guarde-o como senha ou descarte-o, e, se ele vazou, revogue o acesso pela conta em claude.ai.

## Segurança e limites

- **Onde os logins ficam:** o compose lê os secrets do shell ao criar o container e os copia para `/run/secrets` dentro dele. O Docker guarda essa cópia com o container, em `/var/lib/docker` (só root), até o `down`. Eles não ficam no volume, no `.env` nem no repositório. O teste `docker/autonomo/test-entrypoint.sh` confere que nenhum login fica gravado no HOME e que os tokens do hub não chegam às sessões.
- **Alcance do agente:** o agente tem o `CLAUDE_CODE_OAUTH_TOKEN` e o `GH_TOKEN` no ambiente da sessão, e o `ctowerd` não mascara esses dois na saída (só os secrets do hub). O alcance no GitHub é o do token fine-grained, e por isso ele precisa ser restrito aos repositórios dos projetos autônomos.
- **Rede:** o container sai para a internet (GitHub, Anthropic, npm). A porta do `ctowerd` só é publicada em `127.0.0.1:17880` do host, para o app desktop desta máquina. O hub fala com o ambiente pelo canal reverso, que o próprio ambiente abre.
- **Ferramentas:** a imagem tem `git`, `gh`, Node (com `npm`), `jq` e `sqlite3`. Projetos que precisam de outro toolchain (Rust, Python, Java) pedem uma imagem derivada (`FROM ct-autonomo:<versão>`, com `USER root` para instalar e `USER ct` no fim) e o `image:` do compose apontado para ela.
- **Docs dos projetos** (`docs_root`): desligados, porque a pasta de docs não é montada.
