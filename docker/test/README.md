# Hub e ambiente de teste em Docker

Um par hub (`cthubd`) e ambiente (`ctowerd`) isolado, compilado do checkout atual, para testar uma branch de ponta a ponta sem tocar no hub e no ambiente de uso diário (#176).

- Cada serviço tem seu próprio volume de HOME: config, `hub.db`, ledger, worktrees e o login do Claude Code e do `gh` ficam ali. Nada do HOME do host é montado.
- Na primeira subida, o hub gera o token de sync e o token de agente do ambiente, e o ambiente se liga a ele (`hub_url = "http://hub:7879"`), como o `provision.sh --hub --env` faz numa máquina só.
- Os dois sobem com `test_mode = true`: a UI pinta a barra do topo de roxo e mostra o selo **Ambiente de teste**, e o título da janela ganha `[TESTE]`.
- As portas ficam só em `127.0.0.1` do host e são diferentes das padrão, para conviver com os daemons de verdade.

| Serviço | No host | No container |
|---|---|---|
| hub (`cthubd`, PWA e `/ws`) | `127.0.0.1:17879` | `7879` |
| ambiente (`ctowerd`, `/ws`) | `127.0.0.1:17878` | `7878` |

As portas do host mudam com `CT_TEST_HUB_PORT` e `CT_TEST_ENV_PORT`; o nome do ambiente no hub, com `CT_TEST_ENV_NAME` (padrão `ct-test-env`).

## Subir

Na raiz do repositório, com Docker (Docker Desktop, Colima ou OrbStack):

```sh
docker compose -f docker/test/compose.yml up --build -d
docker compose -f docker/test/compose.yml logs -f
```

O build compila a UI web e os três binários em modo release e leva alguns minutos. Ele funciona com e sem BuildKit (Colima sem o plugin `buildx`, por exemplo). Se o código não mudou, o Docker reaproveita as camadas; se mudou, os binários são compilados do zero. Para testar outra branch, troque de branch (ou de worktree) e rode o `up --build` de novo: os volumes continuam.

Sem o plugin `compose` no Docker, use o binário `docker-compose` com os mesmos argumentos.

## Tokens de UI

Cada daemon gera o seu `ui.token` na primeira subida. Para ler sem deixar no histórico do shell:

```sh
docker compose -f docker/test/compose.yml exec hub cat .config/cthub/ui.token | pbcopy   # macOS
docker compose -f docker/test/compose.yml exec env cat .config/ctower/ui.token | pbcopy
```

- **PWA do hub:** abra `http://127.0.0.1:17879` no Chrome, Edge ou Firefox e cole o token de UI do hub. O Safari não serve aqui: o cookie da sessão web é `Secure`, e o Safari não o guarda em `http://`, nem em `127.0.0.1`. O login é aceito e a sessão se perde em seguida.
- **App desktop:** em **Configurações → Hub**, use `ws://127.0.0.1:17879/ws` com o token do hub. Em **Configurações → Conexões**, adicione o ambiente como **Remoto (token informado aqui)**, com `ws://127.0.0.1:17878/ws` e o token do ambiente.

O app desktop guarda uma conexão de hub só: apontar o app instalado para o hub de teste o tira do hub de verdade. Para não trocar, use o app de teste abaixo ou a PWA.

## App desktop de teste, ao lado do instalado

```sh
docker/test/desktop.sh            # grava a configuração de teste, se faltar, e abre
docker/test/desktop.sh --reset    # regrava (depois de um down -v, por exemplo)
```

O script roda o app do checkout (`tauri dev`) com outro identificador, `ai.deep-ion.control-tower.teste`, e janela "Control Tower (teste)". Os dados dele (ambientes, hub, tokens e preferências) ficam em `~/Library/Application Support/ai.deep-ion.control-tower.teste` no macOS, separados dos do app instalado, que continua aberto e sem mudança. Antes de abrir, o script grava ali só o ambiente e o hub de teste, com os tokens lidos dos containers. Sem isso, a lista padrão do app apontaria para o `ctowerd` de verdade em `127.0.0.1:7878`.

O primeiro `tauri dev` compila o app e leva alguns minutos. Para apagar os dados do app de teste, remova a pasta acima.

## Claude Code e GitHub dentro do ambiente

O ambiente de teste não enxerga o login do host. Faça o login dentro do container, que fica no volume do ambiente:

```sh
docker compose -f docker/test/compose.yml exec env claude          # login do Claude Code
docker compose -f docker/test/compose.yml exec env gh auth login   # login do gh
```

O hub também precisa de um token do GitHub para validar o repositório ao cadastrar um projeto e para sincronizar as issues. Sem ele, o cadastro falha com "token do GitHub indisponível". O jeito mais simples é passar o token aos dois containers só na subida, sem gravar em arquivo:

```sh
CT_TEST_GH_TOKEN="$(gh auth token)" docker compose -f docker/test/compose.yml up -d
```

**Use um repositório só de teste**, de preferência com um token restrito a ele. O sync, os PRs e a limpeza de branches falam com o GitHub de verdade: as travas do Control Tower bloqueiam o merge, mas criar PR, fechar issue e apagar branch remota acontecem de fato.

## Shell, logs e dados

```sh
docker compose -f docker/test/compose.yml exec env bash      # shell no ambiente
docker compose -f docker/test/compose.yml exec hub bash      # shell no hub
docker compose -f docker/test/compose.yml logs -f env
```

Dentro dos containers, os comandos de sempre funcionam (`ctower`, `cthubd add-env`, `sqlite3`).

## Apagar tudo

```sh
docker compose -f docker/test/compose.yml down -v
```

O `-v` apaga os volumes: `hub.db`, ledger, worktrees, tokens e logins. A próxima subida começa do zero e liga os dois de novo. Sem o `-v`, os containers param e os dados ficam.

## Limites

- Os daemons rodam em primeiro plano, sem systemd nem launchd: `cthubd update`, `ctowerd`/`ctower update` e o rollback não se aplicam aqui. Para testar uma versão nova, rode o `up --build`.
- A pasta de docs dos projetos (`docs_dir`/`docs_root`) vem desligada.
- O `test_mode` só sinaliza na UI; não muda nenhum comportamento dos daemons.
