# Hub e ambientes: plano de controle, execução e backup

O **hub** (`cthubd`) é o plano de controle: é a origem dos projetos, da configuração e das atribuições (qual ambiente executa qual projeto) e guarda a visão consolidada em `hub.db`. Ele não roda sessões, PTY, hooks nem MCP. Cada **ambiente** (`ctowerd`) só executa os projetos atribuídos a ele: puxa a configuração deles do hub, guarda em cache e funciona offline com a última versão. A execução sobe para o hub pelo outbox.

Desenho completo: `docs/superpowers/specs/2026-10-18-hub-plano-de-controle.md` (épico #183), que revisa `2026-10-16-ledger-centralizado.md` (épico #64).

> Este documento descreve o desenho do épico #183, em implementação. A instalação em uso ainda é a do #64 (`ctowerd` com `hub = true`) até rodar o roteiro da seção 5, que faz a troca.
>
> Já existe (#174): o binário `cthubd` (`serve`, `add-env`, `revoke-env`, `agent-token`, `revoke-agent-token`, `restore`), o `/sync/push`, o `/sync/query/decisions`, o `/ws` do hub (`hub.info`, `hub.status`, os `hub.*` de leitura e status manual, `github.*`), os snapshots e o sync com o GitHub de todos os projetos. O `ctowerd` só envia a um `hub_url` e recusa subir com `hub = true`.
>
> Já existe (#175): projetos com origem no hub (`hub.projects.add|update|delete`), lápide na exclusão, `config_rev` e a quarentena de push (`hub.quarantine.list|discard`). O hub ignora os atributos de projeto que os ambientes ainda enviam e não cria projeto a partir de push.
>
> Já existe (#176): atribuições no hub (`hub.assignments.list|add|remove`), com o estado derivado do relato do ambiente; `hub.projects.list` traz as atribuições.
>
> Já existe (#177): o pull da configuração (`GET /sync/config`), o cache dela no `ledger.db` do ambiente, a aplicação das atribuições (clone ou validação em `repos_root`) e o relato ao hub pelo outbox.
>
> Já existe (#178): o ambiente sem cadastro próprio. `projects.add|update|delete` (e `projects.parse_url`) saíram do `/ws` do `ctowerd`, e o hub é obrigatório para iniciar sessões (seção 3).
>
> Já existe (#179): a poda dos dados de execução no ambiente depois do ack do hub (seção 4).
>
> Já existe (#180): a UI com conexão própria com o hub (seção 1, "Ligar a UI ao hub"), a tela **Hub** (cadastro, atribuições, ambientes, avisos, quarentena e o token do GitHub do hub) e, em cada ambiente, a revisão e a idade da configuração e os projetos fora do hub.
>
> Já existe (#181): a instalação do hub (`provision.sh --hub`, ou `--hub --env` na máquina única, já ligados), a unidade systemd e o LaunchAgent do macOS, o binário do `cthubd` para Linux na release e o `cthubd update`, com snapshot do `hub.db` e rollback (seções 1 e "Atualizar o hub").
>
> Já existe (#199, épico #205): o `cthubd` embute a UI web (PWA) e serve, com `[web] enabled = true`, o bundle (`/`, `/assets/*`) e o `/web/ws` com a lista de permissões da web (seção "UI web"). Por isso o **Node ≥ 20 com npm** passou a ser requisito do hub: o `provision.sh --hub` e o `cthubd update` geram o bundle antes de compilar.
>
> Já existe (#200): o login web com o token de UI (cookie `ct_web`, `/web/login|logout|me`), validade, revogação (`hub.web.sessions.*`, `cthubd web revoke-all`), bloqueio após falhas, o Cloudflare Access e o `tailscale serve` como porta a mais e o `404` em `/ws`, `/sync/*` e `/agent/v1/*` para o que vem pela Cloudflare (seção "UI web").
>
> Já existe (#203): a PWA instalável (manifest, ícones e um service worker que guarda só o shell) e as notificações Web Push do hub, com a tela Hub › Dispositivos (seção "Web Push", em "UI web").
>
> Já existe (#198, fase 2 do épico #205): o canal reverso `/sync/channel`, que o `ctowerd` abre com o hub; o hub chama por ele os métodos da lista do canal e recebe os pushes do ambiente em tempo real, e `hub.envs.list` mostra `online` (seção 3, "Canal reverso"). Hub 1.4.
>
> Já existe (#343, fase 2 do épico #205): `hub.env.call` no `/ws` e no `/web/ws`, os pushes `env.push` e `env.online` no navegador e as ações de sessão pela PWA (iniciar, retomar, encerrar, ligar o Remote Control, marcar decisões como revisadas); o `env_stale` da Web Push passa a ser o do canal, em tempo real.
>
> Já existe (épico #353): projetos Jira + GitLab (rastreador e forja no cadastro, ambiente de sync, `tracker_status`, regras do agente e sprints do espelho; seção "Projetos"). Hub 1.5 e ambiente 2.4: a UI só oferece o cadastro Jira + GitLab com hub 1.5 ou maior. Na UI web, as regras do agente são só leitura (`hub.rules.get`, `hub.rules.history`, `hub.rules.proposals.list` e o push `hub.rules.changed`); editar, importar e decidir propostas ficam no desktop.
>
> Já existe (#457, épico #456): o status `aguardando_epico` ("aguardando o épico"), da sub-issue com PR mesclado na branch do épico, à espera do merge do épico na base. Não conta como atenção nem como item parado na saúde. Hub 1.7 e ambiente 2.5: **atualize o hub antes dos ambientes**, porque um hub anterior ao 1.7 não interpreta `aguardando_epico` nos eventos de status vindos do ambiente.
>
> Já existe (#440, épico #445): docs dos projetos fora do repositório — `docs_dir` no hub, `docs` no projeto (`hub.projects.add|update`, entregue pelo `GET /sync/config`), `docs_root` no ambiente e a estrutura `_projeto/` e `epicos/<n>-<slug>/` (seção 2, "Docs dos projetos"). Hub 1.8 (`HubProject.docs`, `HubInfo.docs_dir`) e ambiente 2.6 (`Project.docs`, `env.info.docs`). #441: sessões com `CT_DOCS_DIR`, `CT_EPIC_DOCS_DIR` e `--add-dir`, e o aviso `docs_unreachable` (ambiente 2.7, `env.info.docs.warning`). #444: na UI, os docs na edição do projeto, "Abrir pasta de docs" no projeto e no épico (ambiente 2.8, `docs.dir`), o aviso `docs_unreachable` na faixa "Precisa de você" e o bloco "Docs dos projetos" em Manutenção › Backup. #442: backup dos docs (hub 1.9).
>
> Já existe (#434): "Ver issue" na aba do item — `hub.workitem.body { owner, repo, number }` devolve `{ title, body, format, url }` com a descrição lida no rastreador na hora, sem gravar nada (nem espelho, nem ledger, nem cache). GitHub pela API, com o token do hub; Jira pelo ambiente de sync do projeto (método `tracker.issue_body` do canal reverso, com o `jira issue view`), e o hub converte a descrição (wiki markup do Jira Server/DC ou ADF do Cloud) para Markdown em memória. Também no `/web/ws`. Hub 1.10 e ambiente 2.9: projeto Jira pede o ambiente de sync online e atualizado.
>
> Já existe (#182): `cthubd migrate`, que troca o hub antigo pelo `cthubd` (seção 5). **Não atualize o `ctowerd` da máquina do hub antes de migrar**: o novo recusa subir com `hub = true`.

```
                cthubd (Mac mini) — hub.db, snapshots
        │ GET /sync/config (só o atribuído)     ▲ POST /sync/push (outbox)
        │ /sync/channel: chamadas ↓, pushes ↑    │ (WebSocket aberto pelo ctowerd)
        ▼                                        │
  ctowerd (WSL)        ctowerd (notebook)        ctowerd (Mac mini, ao lado do cthubd)
```

## 1. Instalar o hub

O `cthubd` tem config e dados próprios:

- config: `~/.config/cthub/config.toml` (`CTHUB_CONFIG_DIR`), com `ui.token` e `github.token`;
- dados: `~/.local/share/cthub/` (`CTHUB_DATA_DIR`), com `hub.db` e `backups/`.

```toml
# ~/.config/cthub/config.toml
name = "mac-mini"                    # vazio = nome da máquina
port = 7879                          # padrão; CTHUB_PORT sobrepõe
bind = "100.x.y.z"                   # IP Tailscale; 127.0.0.1 fica sempre no ar
gh_sync_interval_secs = 180          # 0 desliga o sync periódico com o GitHub
backup_dir = "/Volumes/Backup/cthub" # vazio = ~/.local/share/cthub/backups
backup_interval_hours = 6            # 0 desliga
backup_keep = 28
secrets_snapshot_dir = ""            # snapshots cifrados de secrets; vazio = backup_dir em vigor
docs_dir = ""                        # pasta única dos docs dos projetos (#440); vazio = desligada
docs_backup_interval_hours = 24      # pacote completo dos docs (#442); 0 desliga
docs_backup_keep = 7                 # pacotes dos docs mantidos (local e remoto), à parte do backup_keep
docs_backup_max_mb = 1024            # acima disso (soma dos arquivos) o backup dos docs não roda e avisa
recovery_reverify_days = 90          # lembrete para verificar a chave de recuperação; 0 desliga

[logs]                               # log em arquivo para a tela Logs da UI (#65); o stderr segue no journald/launchd
dir = ""                             # vazio = ~/.local/state/ctower/logs/cthubd.jsonl (~/Library/Logs/ctower no macOS; <CTHUB_DATA_DIR>/logs se definido)
max_file_mb = 10                     # tamanho de cada arquivo antes de rotacionar; 0 desliga o arquivo
max_files = 5                        # arquivos guardados, contando o atual

[web]                                # UI web (PWA), #199 e #200; veja "UI web" abaixo
enabled = false                      # `/`, `/assets/*` e `/web/*` desligados (404) por padrão
origins = []                         # Origin aceitos no /web/ws e nos POST /web/*; vazio = http://127.0.0.1:<port> e http://localhost:<port>
session_days = 30                    # validade absoluta da sessão web (cookie ct_web)
idle_days = 7                        # dias sem uso até a sessão web vencer

[web.cloudflare]                     # opcional: Cloudflare Access na frente do hub
team_domain = "minha-equipe.cloudflareaccess.com"
aud = "<AUD da aplicação no Access>"
emails = ["eu@exemplo.com"]

[web.tailscale]                      # opcional: tailscale serve na frente do hub
logins = ["eu@github"]

[web.push]                           # Web Push da PWA (#203); veja "Web Push" abaixo
enabled = true                       # com [web] enabled; a chave VAPID nasce no primeiro serve
env_stale_minutes = 30               # ambiente sem canal reverso e sem sync há mais que isso avisa (com canal: fechado há 1 min); 0 desliga
subject = ""                         # sub do JWT VAPID (mailto: ou https://); vazio = 1º e-mail do Access ou 1º origin https://

[health]                             # limiares dos sinais de saúde (hub.health.get)
stale_days = 3                       # em execução/revisão sem atividade; crítico em stale_critical_days (7)
waiting_hours = 24                   # aguardando você; crítico em waiting_critical_hours (72)
blocked_days = 2                     # bloqueado; crítico em blocked_critical_days (7)
paused_days = 14                     # pausado (só informativo)
pr_stale_days = 3                    # PR aberto sem atualização; crítico em pr_stale_critical_days (7)
decision_review_days = 3             # decisão autônoma sem revisão; crítico em decision_review_critical_days (7)
failed_sessions_critical = 3         # sessões falhas em 24 h no mesmo item (1 já é aviso)
interrupted_hours = 1                # sessão interrompida; crítico em interrupted_critical_hours (24)
p0_idle_hours = 24                   # P0 sem andamento; crítico em p0_idle_critical_hours (72)
untriaged_info = 10                  # issues abertas sem prioridade no projeto (informativo)
env_silent_hours = 1                 # ambiente com pendências sem enviar; crítico em env_silent_critical_hours (24)
tracker_sync_minutes = 15            # projeto Jira sem sync do ambiente de sync; crítico em tracker_sync_critical_hours (24)
```

Os sinais são calculados a cada leitura, sem tabela própria (spec do gestor §5). O GitHub conta como sem sync depois de 3 × `gh_sync_interval_secs` sem sucesso ou com erro no último sync. Um projeto Jira (sincronizado pelo ambiente de sync, não pelo hub) dá `rastreador_sem_sync` quando não tem ambiente de sync, quando o último sync bem-sucedido relatado é mais velho que `tracker_sync_minutes` ou quando o último ciclo terminou com erro; o detalhe cita o ambiente e o erro.

**Requisitos do hub:** Rust (rustup) para compilar e **Node ≥ 20 com npm** para o bundle da UI web que o `cthubd` embute (`npm ci` e `npm run build:web` em `ui/`, que geram `ui/dist-web`). O `provision.sh --hub` instala o Node se faltar (NodeSource no Linux, `brew install node` no macOS) e, com `--skip-packages`, recusa seguir sem ele, antes de instalar qualquer coisa. Um `cargo build` sem `ui/dist-web` (desenvolvimento, `ci.yml`) embute uma página de aviso no lugar da UI; com `CTHUB_WEB_REQUIRED=1` (o `provision.sh`, o `cthubd update` e a release) a falta do bundle faz o build falhar.

### UI web (`[web]`)

Com `[web] enabled = true`, o hub serve a UI web embutida no binário (a mesma versão dele) e o `/web/ws`. Para abrir a PWA no celular fora da máquina do hub (`tailscale serve` ou Cloudflare Tunnel com Access, com o roteiro de verificação), veja [`docs/acesso-remoto.md`](acesso-remoto.md).


- **Bundle:** `/` e qualquer caminho de navegação devolvem o `index.html` (rotas da UI), e os arquivos do bundle saem como estão. Os assets com hash no nome (`/assets/*`) vão com `Cache-Control: public, max-age=31536000, immutable`; o `index.html`, o `sw.js`, o `manifest.webmanifest` e os demais arquivos sem hash, com `no-cache` (e `ETag`). Um `/assets/*` que não existe é `404`, e os caminhos do hub (`/ws`, `/web/*`, `/sync/*`, `/agent/*`, `/healthz`) nunca viram o `index.html`. As respostas levam `Content-Security-Policy`, `X-Content-Type-Options: nosniff` e `Referrer-Policy: no-referrer`.
- **`/web/ws`:** o mesmo JSON-RPC do `/ws`, mas só com os métodos da lista da web (leituras, inclusive `hub.rules.get|history|proposals.list`, `hub.workitem.set_status|set_priority`, `hub.proposals.decide`, `hub.web.sessions.list|revoke`, `hub.push.*` e `hub.env.call`, só com os métodos da lista do canal; spec da PWA §4 e §5.4). Os demais respondem `-32601` "método indisponível na web", inclusive os sensíveis (`hub.secrets.set|delete`, `hub.recovery.generate|verify`, `github.set_token|clear_token`, `hub.projects.delete`), que seguem só no `/ws` com o token de UI; `hub.rules.set|import|proposals.decide` (regras do agente, épico #353) também ficam fora. Os pushes repassados são `hub_sync_status`, `hub.proposals.changed`, `hub.rules.changed`, `env.online` e `env.push`.
- **Upgrade:** exige `Origin` igual a um item de `origins` (ausente ou outro: `403`), recusa `Authorization` (`400`: o caminho com token é o `/ws`), a identidade do proxy, se configurado (`403`), e a sessão web (`401`). A conexão confere a sessão a cada mensagem e a cada 30 s: revogada ou vencida, ela fecha (código `4401`).

#### Login web (#200)

- **Login:** `POST /web/login { "token": "<ui.token>" }` (JSON, até 4 KiB) com `Origin` da lista. O token é o mesmo `~/.config/cthub/ui.token` do app desktop, comparado em tempo constante; ele não volta em resposta, erro nem log. Certo, a resposta é `{ "expires_at", "authenticated": true }` com `Set-Cookie: ct_web=<id>.<segredo>; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=<session_days>`. O `hub.db` guarda só o SHA-256 do segredo (tabela `web_session`). Errado: `401`.
- **Validade:** `session_days` (30) absolutos e `idle_days` (7) sem uso; o último uso é gravado no máximo uma vez por minuto.
- **Bloqueio:** contador global (atrás de proxy, o IP visto é sempre `127.0.0.1`). Da 5ª falha seguida em diante, o login fica bloqueado por 1 min, dobrando a cada nova falha até 1 h (`429` com `Retry-After`; durante o bloqueio, nem o token certo entra). Um acerto zera. Cada falha vai ao log com a identidade do proxy. O bloqueio vale só para o `/web/login`: o `/ws`, o sync e as sessões já abertas seguem.
- **`GET /web/me`:** `{ authenticated, identity, expires_at }`, para a PWA escolher entre a tela de login e o shell.
- **Sair e revogar:** `POST /web/logout` (com `Origin`) revoga a sessão do cookie e o apaga. `hub.web.sessions.list` (id, criação, último uso, expiração, identidade do proxy, agente do navegador e `current`, a da própria conexão) e `hub.web.sessions.revoke { id }` ou `{ all: true }` (`{ revoked }`) ficam no `/ws` e no `/web/ws`. Na linha de comando, `cthubd web revoke-all` revoga todas direto no `hub.db`, com o hub no ar ou parado. **Trocar o `ui.token` não derruba as sessões web:** revogue. O `cthubd restore` revoga todas depois de trocar o banco (o snapshot pode trazer sessões já revogadas).
- **Proxy como porta a mais:** com `[web.cloudflare]` ou `[web.tailscale]`, toda rota web (o bundle, `/web/*` e o `/web/ws`) exige uma identidade válida de um deles (`403`, sem o motivo na resposta; o motivo vai ao log), **e** o `/web/ws` exige também o cookie. O token de UI continua exigido para criar o cookie. Sem nenhum proxy configurado, vale só o cookie (uso local ou em LAN confiável). Um proxy configurado pela metade (sem `aud`, sem `emails`, sem `logins`) impede o hub de subir.
  - **Cloudflare Access:** `Cf-Access-Jwt-Assertion` com um JWT RS256 assinado por uma das chaves de `https://<team_domain>/cdn-cgi/access/certs` (cache de 1 h, recarregado quando aparece um `kid` desconhecido, no máximo a cada 30 s), `iss = https://<team_domain>`, `aud` contendo o `aud` configurado, `exp` e `nbf` válidos e `email` em `emails` (sem diferença de maiúsculas).
  - **`tailscale serve`:** `Tailscale-User-Login` só vale com a conexão vinda do loopback e sem nenhum header `Cf-*` (o `cloudflared` também fala do loopback), e o login precisa estar em `logins`. Fora disso, o header é ignorado.
- **Vindo pela Cloudflare:** com `Cf-Connecting-IP`, `/ws`, `/sync/*` e `/agent/v1/*` respondem `404`, com ou sem `[web.cloudflare]`: o app desktop e os ambientes usam o Tailscale ou a LAN. Só as rotas web respondem.
- Um `cthubd` compilado sem o bundle avisa no log ao subir com a web ligada e serve a página "bundle web ausente".

#### PWA instalável e Web Push (#203)

- **Instalável:** o bundle web leva `manifest.webmanifest` (nome "Control Tower", `display: standalone`, ícones 192 e 512 e o maskable, gerados por `npm run pwa:icons` em `ui/` a partir do desenho de `ui/src-tauri/icons/icon.svg`) e o `sw.js`. O service worker guarda só o shell (o `index.html`, os arquivos do build e os ícones), com uma versão de cache por build; nunca guarda o `/web/*` (login, `/web/ws`), o `/ws`, o `/sync/*`, o `/agent/*` nem dados. Sem rede, a PWA abre o shell e mostra "hub indisponível".
- **Chave VAPID:** par P-256 gerado no primeiro `serve` com `[web] enabled` e `[web.push] enabled`, em `~/.config/cthub/vapid.key` (0600, fora do `hub.db` e dos snapshots). Apagar o arquivo gera outra chave no próximo `serve`: as inscrições antigas param de receber (o serviço recusa e, com 10 falhas, a inscrição é desativada) até cada aparelho ativar de novo em Hub › Dispositivos.
- **Inscrições:** uma por dispositivo no `hub.db` (tabela `push_subscription`), presa à sessão web que a criou. **Revogar a sessão web apaga as inscrições dela** (revogar uma, revogar todas, `/web/logout`, `cthubd web revoke-all` e o `cthubd restore`). Na resposta `404`/`410` do serviço de push a inscrição é apagada; outras falhas contam, e com 10 seguidas ela é desativada (reativar = ativar de novo no aparelho). No máximo 50 dispositivos.
- **Eventos**, escolhidos por dispositivo (todos ligados ao inscrever):

  | Evento | Quando | Toque abre |
  |---|---|---|
  | `needs_human` "Aguardando você" | evento `needs_human` ou `permission_request` de sessão viva **sem** Remote Control | o item na PWA |
  | `session_error` | sessão saiu com código ≠ 0 | o item na PWA |
  | `proposal_new` | proposta nova do gestor | Hub › Propostas do gestor |
  | `env_stale` "Ambiente fora do ar" | ambiente com canal reverso: canal fechado há mais de 1 min (em tempo real); ambiente sem canal desde que o hub subiu (`ctowerd` antigo): sem enviar ao hub há mais de `env_stale_minutes` | Hub › Ambientes |

  Sessão com Remote Control nunca gera "Aguardando você" pela Web Push: o app Claude já avisa ("Push when actions required" no `/config` do Claude Code). Tudo sai do que o sync já traz: o hub varre o `hub.db` a cada 30 s e logo depois de cada `/sync/push` e de cada proposta nova. Cada notificação sai uma vez só (deduplicação em `push_sent`); um `env_stale` só se repete depois que o ambiente voltar. A primeira varredura de um hub só marca o que já existia. O corpo leva só o projeto, o número, o título do item e o tipo do evento, nunca mensagem do agente nem comando; o serviço de push do navegador (Google, Apple, Mozilla) vê só o endpoint e o tamanho (RFC 8291, `aes128gcm`, TTL de 1 h).
- **Deep link:** a notificação leva `/?go=item&project=<owner>/<repo>&n=<número>`, `/?go=hub:proposals` ou `/?go=hub:envs` (e `https://claude.ai/code/<id>` quando a sessão tem o Remote Control ligado). O service worker foca uma janela da PWA já aberta antes de abrir outra.
- **Métodos:** `hub.push.vapid_key` (`{ key }`), `hub.push.subscribe { endpoint, keys: { p256dh, auth }, label?, events? }` (só no `/web/ws`: a inscrição fica presa à sessão web), `hub.push.list` (sem endpoint nem chaves; `current` = a inscrição da própria sessão web), `hub.push.update { id, events }`, `hub.push.remove { id }` e `hub.push.test { id }` (`{ delivered, status, removed, error }`). Com a Web Push desligada, `vapid_key`, `subscribe` e `test` respondem `-32030`.
- **Tela:** Hub › Dispositivos (grupo Acesso), no desktop e na PWA: lista, eventos de cada aparelho, testar e remover. Na PWA, "Ativar notificações neste dispositivo" pede a permissão a partir do toque.
- **iPhone e iPad:** a Web Push exige **iOS 16.4 ou mais novo**, a PWA **adicionada à tela de início** (Safari › Compartilhar › Adicionar à Tela de Início) e aberta pelo ícone, e o pedido de permissão feito por um toque (o botão "Ativar notificações neste dispositivo"). No Safari fora da tela de início a tela Hub › Dispositivos mostra essa instrução no lugar do botão. A permissão e a inscrição valem para o app instalado: remover o ícone da tela de início perde a inscrição no aparelho, e o hub a apaga quando o serviço de push responder `404`/`410`.

### Linux (systemd de usuário)

Com este repositório clonado em `~/projetos/control-tower` (ou `CTOWER_REPO`):

```bash
bash scripts/provision/provision.sh --hub          # só o hub
bash scripts/provision/provision.sh --hub --env    # máquina única: hub e ambiente, já ligados
bash scripts/provision/provision.sh                # só o ambiente (padrão, como antes)
```

`--hub` compila e instala o `cthubd` em `~/.cargo/bin`, instala `packaging/systemd/cthubd.service` em `~/.config/systemd/user/`, liga o `loginctl enable-linger` e sobe o serviço (`systemctl --user enable --now cthubd`). Um `cthubd` já no ar não é reiniciado: para aplicar um binário novo, use `cthubd update`. `--skip-packages` pula o apt, o `gh`, o rustup, o Node e o Claude Code. O hub não precisa do Claude Code, mas precisa do Node ≥ 20 com npm (o build da UI web, #199): com `--hub` o script instala o Node se faltar, gera o bundle (`npm ci` e `npm run build:web` em `ui/`) e compila o `cthubd` com `CTHUB_WEB_REQUIRED=1`. Log: `journalctl --user -u cthubd`.

Sem toolchain de Rust na máquina, use o binário da release: cada release traz `cthubd-vX.Y.Z-x86_64-linux.tar.gz` (o binário e a unidade systemd) e o `.sha256`. Copie o `cthubd` para `~/.cargo/bin/` (o caminho da unidade) e a unidade para `~/.config/systemd/user/`. O binário é compilado no runner Linux do CI (glibc dele), com a UI web embutida. O `cthubd update` compila do fonte, então precisa do Rust e do Node. Sem ele, atualize trocando o binário pelo da release nova.

**Máquina única (`--hub --env`):** o hub sobe primeiro, em `127.0.0.1:7879`, e o ambiente fica em `127.0.0.1:7878`. Antes de subir o `ctowerd`, o script gera o token de sync com `cthubd add-env <nome>` (o nome é o `env_name` do config do `ctowerd`, `--env-name` ou o nome da máquina), grava-o com `ctower hub set-token` e põe `hub_url = "http://127.0.0.1:7879"` (e `env_name`, se faltar) no topo de `~/.config/ctower/config.toml`. Também gera o token de agente (`cthubd agent-token`) e o grava com `ctower hub set-agent-token`. Rodar de novo não gera outros tokens nem reescreve o config. O nome precisa ser só letras, números, `.`, `_` e `-`: outro nome é recusado antes de instalar qualquer coisa. Se o hub já tem um ambiente com esse nome, o script não troca o token dele (`cthubd add-env --no-replace`, que sai com 3) e não liga este ambiente: use outro `--env-name`. Um `hub_url` que já aponta para outro hub fica como está, com aviso. Se o `ctowerd` estava no ar e o config mudou, o script roda `ctower restart`, que recusa com sessões vivas (nesse caso, reinicie depois). Com `hub = true` no config do `ctowerd` (o hub antigo do #64), o script instala o `cthubd` mas não o sobe nem liga o ambiente, e não troca os binários do `ctowerd`: ele mostra os comandos da migração (seção 5). Depois do `cthubd migrate`, o config já não tem `hub = true` e já traz `hub_url` e `hub.token`, e o script volta a funcionar normalmente com `--hub --env`, sem gerar outro token. Na migração, porém, atualize o `ctowerd` com `ctower update` (seção 5, passo 6): o script troca os binários sem guardar os anteriores, e depois dele `ctower update --rollback` não serve para voltar atrás.

### Ambiente com hub remoto (`--env --hub-url`)

Para um ambiente (Linux ou macOS) cujo hub roda em outra máquina: no hub, `cthubd add-env <nome>` imprime o token de sync; no ambiente:

```bash
CT_HUB_TOKEN=<token> bash scripts/provision/provision.sh --env --env-name <nome> --hub-url http://<ip-do-hub>:7879
# ou: ... --hub-token-file arquivo   (use '-' para ler do stdin)
```

O `hub_url` é sempre `http://` direto na porta do hub (na LAN, com `bind` no IP da LAN ou `0.0.0.0`, ou no IP Tailscale): o `ctowerd` não fala HTTPS com o hub, então a URL do `tailscale serve` ou da Cloudflare não serve para o sync. Qual porta e qual token cada caso usa (sync dos ambientes × PWA): [`docs/acesso-remoto.md`](acesso-remoto.md), seção 7.

O token nunca vai como argumento. O script grava `hub.token` (0600) e `env_name`/`hub_url` no topo do `config.toml`. Rodar de novo não troca o token nem reescreve o config, e um `hub_url` que já aponta para outro hub fica como está, com aviso. `--hub-url` não vale junto de `--hub` (aí a ligação é ao hub local).

### macOS (Mac mini como hub)

O release não tem binário de hub para macOS, então o build é local (o CI de macOS está ligado desde o #253, `docs/ci.md`). Pré-requisitos: Xcode Command Line Tools (`xcode-select --install`), Node ≥ 20 (`brew install node`; o script instala com o Homebrew se faltar) e, para o token do GitHub do hub, o `gh` (`brew install gh`). O script instala o rustup se faltar.

```bash
bash scripts/provision/provision.sh --hub
```

No macOS valem `--hub` (hub) e `--env` (ambiente, `docs/ambiente-macos.md`). Para o ambiente ligar a um hub em outra máquina, veja "Ambiente com hub remoto" abaixo. O script compila o `cthubd` e instala o LaunchAgent `packaging/launchd/ai.deep-ion.cthubd.plist` em `~/Library/LaunchAgents/`, com o `@HOME@` trocado pelo seu HOME (o launchd não expande `~`). Depois carrega o agente com `launchctl bootstrap gui/$(id -u) ...`. O agente sobe no login e reinicia se o processo cair com erro. Log: `~/Library/Logs/cthubd.log`. À mão:

```bash
(cd ui && npm ci && npm run build:web)
CTHUB_WEB_REQUIRED=1 cargo install --locked --path crates/cthubd
sed "s|@HOME@|$HOME|g" packaging/launchd/ai.deep-ion.cthubd.plist > ~/Library/LaunchAgents/ai.deep-ion.cthubd.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/ai.deep-ion.cthubd.plist
launchctl kickstart -k gui/$(id -u)/ai.deep-ion.cthubd   # reiniciar
launchctl bootout gui/$(id -u)/ai.deep-ion.cthubd        # parar (antes de um cthubd restore)
```

O LaunchAgent é do usuário e só sobe depois do login. Para o hub voltar sozinho depois de um reboot, ligue o login automático desse usuário (ou mantenha a sessão aberta).

### Primeira subida

O serviço roda `cthubd serve` (o padrão sem subcomando). No primeiro boot ele gera `ui.token` (token de UI do hub, `0600`) e `hub_id` (identidade do hub nos eventos que ele mesmo grava). Também gera `secrets.key` (`0600`), a chave age do hub que abre o store de secrets e variáveis em `secrets.age` (`0600`, criado na primeira gravação). Configure a chave de recuperação na tela **Hub** (veja [Chave de recuperação](#chave-de-recuperação)): com ela, o `secrets.age` abre sem `secrets.key`, então não é preciso guardar a chave do hub à parte. A cada alteração, o hub copia o `secrets.age` (já cifrado) para `secrets_snapshot_dir` (veja [Snapshots de secrets](#snapshots-de-secrets)). Até a recuperação estar configurada, o store é cifrado só para o hub, o log avisa "recuperação não configurada" e a faixa de atenção mostra o aviso. `hub.status` lista os avisos do backup (snapshots desligados, `backup_dir` no mesmo disco do `hub.db`, último snapshot falho ou mais velho que 2 × `backup_interval_hours`, e os do envio remoto, veja [Backup remoto](#backup-remoto-disco-local-google-drive-backblaze-b2-e-s3)), os da chave de recuperação (`recovery_not_configured`, `recovery_reverify_due`), os dos snapshots de secrets (`secrets_snapshot_stale`, `secrets_snapshot_unreachable`, `secrets_snapshot_same_device`) e `secrets_rotate_pending` (secrets a trocar na origem depois de uma restauração com a chave de recuperação, veja [Restaurar e rotacionar as chaves](#restaurar-e-rotacionar-as-chaves-dos-secrets)).

### Ligar a UI ao hub

A UI tem uma conexão com o hub separada das conexões com os ambientes: em **Configurações → Hub**, informe a URL do hub (`http://<máquina>:7879`, a mesma do `hub_url` dos ambientes, ou `ws://<máquina>:7879/ws`) e o token de UI do hub (o conteúdo de `~/.config/cthub/ui.token` na máquina do hub). O token fica em `hub.json` no diretório de dados do app e nunca volta para a tela. A conexão confere o protocolo do hub pelo `hub.info` (major 1), à parte do protocolo dos ambientes (major 2).

Com a conexão, a tela **Hub** cadastra, edita e exclui projetos, atribui e remove atribuições (com o estado pendente, aplicada ou com erro), lista os ambientes ligados (`hub.envs.list`, com as limpezas pendentes que cada um informou no último envio: `cleanup = { checked_at, items, bytes }`, ou `null` se nunca informou, issue #287), mostra os avisos de `hub.status` (também na faixa **Precisa de você** da Sala de controle) e a quarentena, e troca o token do GitHub do hub. Quando o hub pede `confirm: true` (exclusão com atribuições, remoção com sessões vivas), a UI mostra a mensagem dele e pede a confirmação. Terminal, `attach`, sessões vivas e recuperação continuam em cada ambiente; para iniciar uma sessão, a tela Projeto escolhe um ambiente conectado onde o projeto está atribuído (os aplicados primeiro).

Com o hub fora do ar, a UI mostra "hub indisponível" e segue operando as sessões nos ambientes com a configuração em cache.

Numa máquina só, `scripts/provision/provision.sh --hub --env` instala o hub e o ambiente juntos e já os liga (token de sync e `hub_url = "http://127.0.0.1:7879"`; veja "Máquina única" acima).

O hub sincroniza com o GitHub todos os projetos cadastrados, com o token dele (`github.token` ou `gh auth token`).

### Atualizar o hub: pela UI ou `cthubd update`

O hub se atualiza como o ambiente (`ctower update`, no README): build local de um clone dedicado, no próprio binário `cthubd`, sem depender do `ctower`. **Atualize o hub antes dos ambientes**: quando o protocolo muda (ex.: hub 1.7 com ambiente 2.5), o ambiente novo espera um hub que já o entenda.

**Pela UI (#421):** em **Manutenção › Atualizações › Hub** do app desktop aparecem a versão, o commit e o protocolo no ar, a versão disponível no `update_ref` (com os commits novos) e os botões **Verificar agora**, **Atualizar o hub** e **Voltar à versão anterior**. O andamento aparece ao vivo (buscando, compilando, guardando, instalando, reiniciando, conferindo). Durante o restart a conexão com o hub cai; a UI reconecta sozinha e mostra o resultado (inclusive o rollback automático, com o fim do log). A tela também avisa se algum ambiente já está numa versão à frente do hub, e a confirmação de **Atualizar** em Ambientes alerta quando o hub tem atualização pendente. A faixa "Precisa de você" e a Visão geral da Manutenção mostram o hub atrás do `update_ref`. Na web (PWA) é só leitura: o estado aparece nos dados do hub, sem os botões.

- O hub verifica sozinho no boot e a cada `update_check_interval_hours` (padrão 6; `0` desliga). A verificação só busca o ref e compara; nunca instala.
- O atualizador roda fora do serviço, porque o restart o encerraria: no Linux, numa unidade transitória do systemd de usuário (`systemd-run --user --unit cthubd-update`, log em `journalctl --user -u cthubd-update`); no macOS, num processo com sessão própria (`setsid`), que o `launchctl kickstart -k` não leva junto. Os dois rodam o mesmo `cthubd update` do terminal, com o `PATH` do serviço: o Node, o npm e o `cargo` precisam estar nele (ou em `CTHUB_NODE`, `CTHUB_NPM` e `CTHUB_CARGO`).
- O andamento e o resultado ficam em `update/state.json`; o hub novo lê o arquivo no boot e segue acompanhando até a conferência final.
- **Voltar à versão anterior** só aparece com uma versão guardada. Se o `hub.db` vai voltar do snapshot, a UI mostra o que sai do banco em uso (como o `--force` do terminal) e pede uma segunda confirmação.
- Métodos do `/ws` (protocolo 1.6): `hub.update.get` (só lê), `hub.update.check`, `hub.update.start`, `hub.update.rollback { force }` (sem `force` e com o banco voltando, erro `-32025` com o que se perde) e o push `hub.update.status`. Na web, só `hub.update.get` e o push.
- Hub anterior ao 1.6: a UI não mostra os botões; atualize-o uma vez pelo terminal.

**No terminal** da máquina do hub:

```bash
cthubd update --check      # versão no ar × a do update_ref
cthubd update              # compila, guarda, instala, reinicia e confere
cthubd update --rollback   # volta para a versão anterior (com --force se o hub.db migrou)
```

```toml
# ~/.config/cthub/config.toml
update_repo = "https://github.com/deep-ion-ai/ctower-agents.git"
update_ref = "main"        # canal: branch (ex.: "develop") ou tag (ex.: "v0.3.0")
update_check_interval_hours = 6   # verificação periódica (só confere); 0 desliga
```

- **Onde:** `~/.local/share/cthub/update/`: o clone em `src`, o build em `target`, o binário anterior em `prev/`, o snapshot em `db/`, o andamento em `state.json` e o log em `update.log`.
- **Node:** antes de tudo (antes do fetch, de qualquer backup e da troca do binário), confere `node --version` (≥ 20) e `npm --version`. Sem eles, falha com a mensagem do requisito e nada muda.
- **Build:** `npm ci` e `npm run build:web` em `src/ui` e `cargo install --locked` do `crates/cthubd` do commit do `update_ref`, com `CTHUB_WEB_REQUIRED=1`. Se falhar, nada muda.
- **Backup:** antes de instalar, guarda o binário atual e tira um snapshot consistente do `hub.db` com `VACUUM INTO`, com o serviço no ar e o WAL aberto (não é uma cópia crua do arquivo).
- **Saúde:** depois do restart (`systemctl --user restart cthubd` ou `launchctl kickstart -k`), espera até 60 s (`CTHUB_UPDATE_VERIFY_SECS`) o `/healthz` de um hub responder e o `hub.info` responder pelo `/ws` com o token de UI (`ui.token`), com o commit novo e um protocolo válido. A major do protocolo pode mudar entre versões.
- **Rollback automático:** se o hub novo não ficar saudável, o atualizador para o serviço e volta o binário anterior. Se o esquema do `hub.db` migrou (`user_version` maior), ele também:
  - guarda o banco em uso como `hub.db.pre-rollback-<AAAAMMDD-HHMM>` (cópia consistente com `VACUUM INTO`, que inclui o WAL; nunca sobrescreve uma cópia anterior);
  - volta o `hub.db` do snapshot;
  - sorteia outra `config_epoch`, para os ambientes receberem o conjunto completo no próximo pull.

  Depois, sobe o serviço de novo. O que o hub novo aceitou depois do snapshot (pushes, cadastros, tokens) fica só na cópia `pre-rollback`. O log (`update/update.log`) avisa disso. Rode `ctower hub resync` e `ctower hub sync` em cada ambiente para reenviar o que eles ainda não podaram.
- **Rollback manual:** `cthubd update --rollback` faz o mesmo. Se o `hub.db` vai voltar do snapshot, ele recusa sem `--force` e mostra o que sai do banco em uso: a data do snapshot, os esquemas e as linhas a mais por tabela (ex.: `event +120, env_token +1`). Com `--force`, segue como o automático (cópia `pre-rollback`, época nova e o aviso do resync).
- **Primeiro plano:** no terminal, o comando roda fora do cgroup do serviço, então o restart não o interrompe. Numa sessão SSH, rode dentro do `tmux`, porque uma queda da conexão no meio deixaria a atualização sem a conferência final. Reiniciar o hub não encerra sessões de agentes: elas rodam nos ambientes, que esperam no outbox enquanto o hub está fora.
- **Máquina única:** `ctower update` atualiza só o `ctowerd` e o `ctower`, e `cthubd update` atualiza só o `cthubd`. Rode os dois. Cada um tem o próprio clone e o próprio build.

## 2. Projetos e atribuições

Projetos nascem e são editados **só no hub** (UI do hub, métodos `hub.projects.*`). O ambiente não cria nem edita projetos.

- **Cadastrar:** `hub.projects.add { url, name?, base_branch?, protected_branches? }`, pela URL do GitHub (`https://github.com/o/r`, `git@github.com:o/r.git`...), sem clone. O hub valida com o token do GitHub dele que o repositório existe e que `base_branch` (padrão `develop`) existe. Protegidas padrão: `develop`, `main`, `master`.
- **Cadastrar Jira + GitLab (#348):** `hub.projects.add { tracker: { kind: "jira", url, project_key, active_sprint_only?, jql?, max_issues?, credential_cmd, config_file?, epic_link_field?, epic_name_field?, sprint_field?, type_map? }, forge: { kind: "gitlab", clone_url | host + path, credential_cmd }, branch_pattern?, attribution?, name?, base_branch?, protected_branches? }`. O `owner/repo` vem do caminho do GitLab (`grupo/projeto/app` → `grupo/projeto` + `app`). O hub não alcança o Jira nem o GitLab e não usa o token do GitHub: confere só a forma (combinação, campos e `branch_pattern`), e o ambiente de sync confere o resto no primeiro ciclo. Só as combinações GitHub + GitHub e Jira + GitLab são aceitas. As credenciais **não** são secret do hub: o cadastro guarda só o comando de credencial de cada um, que roda no ambiente. Sem `type_map`, o hub preenche o padrão (`Epic`/`Épico` → épico, `Bug`/`Erro` → bug). **O que sincronizar:** com `active_sprint_only: true` ("Sincronizar somente a sprint ativa", marcado por padrão no formulário de projeto novo), o JQL do sync é `sprint in openSprints()`, o `jql` do cadastro é ignorado e não há limite de quantidade (o campo de limite fica oculto e é ignorado no sync). Desmarcado, o `jql` é obrigatório e o `max_issues` (padrão 100, de 1 a 5000) limita o espelho às N issues mais recentes do JQL por `updated` (a listagem vai sem o `--reverse`, que no jira-cli 1.7.0 inverte a ordem padrão decrescente, e para em N chaves), além das que têm work item e dos pais delas; o hub recusa o cadastro sem JQL ou com limite fora da faixa. **Cadastro anterior** (sem os campos): `active_sprint_only = false` e `max_issues = 100`, então a próxima reconciliação poda o espelho para as 100 issues mais recentes do `jql` (ou do projeto, sem JQL), mantendo as com work item; editar o rastreador desse projeto pede o JQL (a edição só do nome, das branches ou do ambiente de sync não confere o filtro).
- **Ambiente de sync (Jira):** a primeira atribuição de um projeto Jira sem ambiente de sync o elege (`hub.assignments.add` responde `sync_env: true`). Só ele envia o espelho (`issue_mirror`, `pr_mirror`, `tracker_status`); o de outro ambiente é descartado com ack e aviso no log. `hub.projects.update { owner, repo, sync_env }` troca, entre os atribuídos. Remover a atribuição dele deixa o projeto sem ambiente de sync, e a saúde avisa (`rastreador_sem_sync`). O sync do GitHub do hub pula projetos Jira.
- **Ciclo do sync do Jira (no ambiente de sync, a cada `jira_sync_interval_secs`):** o jira-cli 1.7.0 `issue list --raw` não traz o status nem o épico, então o ciclo lista as chaves do recorte e lê com `issue view --raw` cada issue nova ou com `updated` diferente do cache (4 em paralelo). O recorte é incremental (`updated >= -<m>m` desde o último sucesso), com reconciliação diária e no primeiro ciclo (abertas e fechadas nos últimos 30 dias); o `jql` do cadastro entra nos dois e limita o que é sincronizado — num projeto grande, use-o para encurtar o primeiro sync (ex.: `statusCategory != Done OR updated >= -365d`). As issues lidas são gravadas em lotes de 200 e sobem ao hub em seguida (`issue_mirror`), com o progresso no log do ctowerd (`jira: <owner>/<repo> N de M issues lidas`) e no `tracker_status` (`readiness.progress = { done, total, started_at }`, no máximo a cada 30 s, mostrado na prontidão como "Sincronizando… N de M"; ausente fora do ciclo e nos ambientes anteriores). O cursor incremental e a marca da reconciliação só avançam quando o ciclo termina: um ciclo interrompido deixa os lotes gravados e o próximo recomeça do mesmo recorte, pulando o que já está no cache. A falha de leitura de uma issue não derruba o ciclo: ele termina com aviso no `last_error` (`N issue(s) não lidas, a ler de novo no próximo ciclo: PROJ-1, ...`) e as chaves são lidas de novo no ciclo seguinte. Senha recusada, jira-cli ausente, configuração ausente ou 10 leituras seguidas falhando (Jira fora do ar) derrubam o ciclo. A listagem (`issue list`, 100 chaves por página) tem prazo próprio por página, `jira_list_timeout_secs` (padrão 120 s; num Jira lento cada página leva 10–20 s), separado do `jira_view_timeout_secs` (padrão 30 s) do `issue view` e das conferências; a página que estoura o prazo ou falha por erro transitório é pedida de novo até 2 vezes (esperas de 2 s e 5 s, log `jira: <owner>/<repo> página <offset> da listagem falhou (<erro>), tentando de novo (n/2)`), e só a terceira falha seguida derruba o ciclo (senha recusada e configuração ausente não são tentadas de novo). Vale para as três listagens: incremental, reconciliação e a do JQL da poda. Durante a listagem, o log mostra `jira: <owner>/<repo> listando… N chaves` a cada 5 páginas e o `readiness.progress` vem com `total = 0` e `done` = chaves listadas (na prontidão: "Sincronizando… listando as issues, N até agora").
- **JQL e poda do espelho:** o filtro (o JQL efetivo, o modo sprint ativa e o limite) define o conjunto espelhado. Mudar qualquer um força uma reconciliação, mas não relê o que já está no cache com o mesmo `updated` (só mudar o mapa de tipos, os campos do épico ou da sprint, a URL ou a chave relê tudo). Em toda reconciliação (a diária, a da mudança do filtro), depois das leituras, o ambiente de sync lista o JQL efetivo puro (com o limite) e tira do espelho as issues que não vêm mais nele, no ambiente e no hub (`issue_mirror` com `removed: true`; o hub anterior ignora o campo e só regrava a issue), com o log `jira: <owner>/<repo> N issue(s) fora do JQL removidas do espelho (M mantidas por ter work item)`. **Exceção:** a issue com work item (de qualquer status, inclusive concluído) e os ancestrais dela (o épico de uma sub-issue com work item) ficam no espelho e continuam atualizados: as chaves vão para `jira:pinned:<owner>/<repo>` no `gh_etag` e são lidas pela chave em todo ciclo, como as que falharam. O hub também não apaga a issue que tem work item no hub (de qualquer ambiente). **Proteções:** sem poda se a listagem do JQL falhar ou vier vazia, nem, com o filtro igual ao do último ciclo, se ela tiraria mais da metade do espelho (provável listagem incompleta; fica o aviso no log). Com o filtro mudado (inclusive a primeira reconciliação de um cadastro anterior ao modo e ao limite), a poda grande é esperada e passa. Entre as reconciliações, o incremental só acrescenta: o espelho pode passar do limite até a reconciliação seguinte.
- **Sprints (Jira Software):** cada issue do espelho guarda as sprints em que está ou esteve (`sprints`: `{ id, name, state: active|closed|future, board_id, start_date, end_date, complete_date }`, datas em ms), da mais antiga para a mais recente, no `issue_cache` do ambiente e do hub e no `issue_mirror` (campo opcional: hub e ambiente anteriores ignoram ou mandam sem ele, e a issue fica sem sprint). O campo vem de `sprint_field` no cadastro (ex.: `customfield_10600`); vazio = **autodetecção** em cada `issue view --raw`: o primeiro `customfield_*` cujo valor é uma lista de textos `com.atlassian.greenhopper.service.sprint.Sprint@...[id=...,state=...,name=...]` (Jira Server/Data Center, como o 8.5.2 do exemplo, inclusive com o `&lt;null&gt;` escapado) ou de objetos com `id`, `name`, `state` e `boardId`/`rapidViewId` (Jira Cloud e versões novas). O nome pode ter vírgula e `=`: o texto é cortado pelas chaves conhecidas. Mudar o `sprint_field` relê o projeto na reconciliação seguinte; a primeira versão com sprints relê o projeto uma vez sozinha (a marca da reconciliação leva a versão do espelho), para preencher as issues já espelhadas.
- **Filtro por sprint (UI):** na tela Projeto (árvore do painel lateral e Quadro, local e do hub), os projetos Jira ganham o seletor **Sprint**: Todas, Sprint ativa (as issues com uma sprint `active`), cada sprint conhecida do projeto (derivada das issues, da mais recente para a mais antiga, com estado e datas: `Sprint 48 · ativa · 26/10–06/11`) e Sem sprint. Uma issue pertence a uma sprint se ela está na lista dela. Com uma sprint escolhida (ou a ativa), os concluídos dela aparecem também. A escolha fica guardada por projeto no `localStorage` (`ct.project.sprint`); a sprint que sumiu do espelho volta a "Todas". Cada item mostra a sprint atual (a ativa, ou a última) num chip discreto. Sem nenhuma issue com sprint no espelho (o sync ainda não as leu, ou o projeto não usa sprints), o filtro mostra o aviso discreto "sprints ainda não lidas (aguarde o próximo sync)"; com "Sprint ativa" escolhida e nada nela, a árvore diz "Nenhum item na sprint ativa" (com o mesmo aviso, se for o caso) no lugar da mensagem genérica de filtro. Projetos GitHub não têm o seletor.
- **Editar:** `hub.projects.update { owner, repo, name?, base_branch?, protected_branches?, tracker?, forge?, branch_pattern?, attribution?, sync_env?, docs? }` (a branch nova é validada no GitHub, só em projeto GitHub; o tipo do rastreador e da forja e o repositório do GitLab não mudam; `branch_pattern: ""` volta ao padrão do rastreador).
- **Excluir:** `hub.projects.delete { owner, repo, confirm? }`. A exclusão **apaga o histórico do projeto no hub** (work items, sessões, decisões, eventos, espelho do GitHub), que é a única cópia. Com atribuições ativas ou sessões vivas relatadas por algum ambiente, o hub recusa sem `confirm: true` e lista os ambientes e as sessões. As atribuições ficam removidas e o relato dos ambientes (`project_location`) é apagado com o histórico. Antes de apagar, tira um snapshot fora da rotação em `backup_dir` (`hub-<AAAAMMDD-HHMM>-pre-delete.db`); para desfazer, `cthubd restore` desse arquivo. O projeto fica como **lápide**: o que um ambiente ainda enviar dele é descartado. Cadastrar de novo o mesmo `owner/repo` limpa a lápide. Para mostrar o que seria atingido antes de pedir a confirmação, `hub.projects.impact { owner, repo }` devolve as mesmas atribuições ativas e sessões vivas que a recusa lista.

- **Docs do projeto (#440):** `hub.projects.add|update { docs: { enabled, project_dirs, epic_dirs } }` (veja [Docs dos projetos](#docs-dos-projetos-fora-do-repositório)). O objeto inteiro substitui o anterior; campo ausente = padrão (`enabled: false`, listas vazias). As pastas são caminhos relativos (`analises`, `specs/telas`), sem `..`, `.`, pasta oculta nem raiz; o hub apara, tira barras das pontas e repetidos, e recusa o resto. Mudar o modelo incrementa o `config_rev`.

Cada mudança em projeto ou atribuição incrementa o `config_rev` do hub (`hub.info`) e grava o valor no `rev` da linha. É a base do pull da configuração pelos ambientes.

### Quarentena de push

O hub nunca cria projeto a partir do que um ambiente envia. O que chega de um projeto que não está cadastrado (e não tem lápide) recebe ack, para não travar o outbox do ambiente, e fica em **quarentena** no `hub.db`, com aviso no log e em `hub.status` (contagem por projeto e por ambiente). Ao cadastrar o projeto, a quarentena dele é reaplicada na ordem em que chegou. `hub.quarantine.list` mostra o que está em quarentena; `hub.quarantine.discard { owner, repo, confirm: true }` descarta.

### Atribuições

Para um ambiente executar um projeto, atribua o projeto a ele. O ambiente recebe a atribuição no próximo pull, clona ou valida o clone e relata o resultado.

- **Atribuir:** `hub.assignments.add { owner, repo, env, repo_path? }`. `env` é o `env_id` ou o nome do ambiente (que precisa ter se ligado ao hub uma vez; com nome repetido, use o `env_id`). `repo_path` vazio = padrão do ambiente (`<repos_root>/<repo>`, `~/projetos/<repo>`); senão, absoluto (`/`, `~` ou `C:\`). Repetir a mesma atribuição não muda nada; mudar o `repo_path` ou reatribuir uma removida gera `rev` novo.
- **Listar:** `hub.assignments.list { env?, owner?, repo? }` (só as ativas). `hub.projects.list` traz as atribuições de cada projeto em `assignments`; `locations` segue com o que os ambientes relataram.
- **Remover:** `hub.assignments.remove { owner, repo, env, confirm? }`. Com sessões vivas relatadas pelo ambiente nesse projeto, o hub avisa e exige `confirm: true`. A linha fica com `removed_at`, para o pull propagar a remoção.

Estado de cada atribuição (campo `state`), cruzando a atribuição (`rev`) com o relato do ambiente (`project_location`):

| Estado | Quando |
|---|---|
| `pending` (pendente) | sem relato com `applied_rev` (ambiente anterior à #177) ou `applied_rev` menor que o `rev` da atribuição |
| `applied` (aplicada) | relato `ok` com `applied_rev >= rev` |
| `error` (com erro) | relato `error` com `applied_rev >= rev`; a mensagem vem em `error` |

O relato chega pela fotografia do projeto no outbox (`/sync/push`), com os campos opcionais `state` (`ok`/`error`), `error` e `applied_rev`. O ambiente (#177) manda os três em toda fotografia do projeto, inclusive nas de erro e nas do `ctower hub resync`. Uma fotografia **sem** `applied_rev` (ambiente anterior à #177) atualiza só o caminho: o relato anterior fica como está, e uma atribuição aplicada não volta para pendente. Ambientes que não os enviam continuam sendo ingeridos normalmente.

O `applied_rev` relatado é o `rev` que o ambiente recebeu no pull para o projeto, o maior entre o da atribuição e o do projeto. Assim, depois de mudar a `base_branch` no hub, `applied_rev` mostra se o ambiente já recebeu a mudança.

Remover a atribuição tira o projeto da lista do ambiente, mas não apaga o clone nem os worktrees, nem encerra sessões.

### Docs dos projetos (fora do repositório)

Os docs de um projeto (análises, especificações, prompts, evidências) não vão para o repositório (épico #445). Ficam numa **única pasta na máquina do `cthubd`** (`docs_dir` no `config.toml` do hub; vazio = desligada; `hub.info.docs_dir`); os outros ambientes a acessam por um compartilhamento de rede montado. Não há sincronização, e o CT não monta nada.

```toml
# ~/.config/ctower/config.toml (de cada ambiente)
docs_root = "/srv/ct-docs"    # na máquina do hub: o mesmo docs_dir; nas outras: a montagem. Vazio = sem docs aqui
```

```text
<docs_root>/<owner>__<repo>/      # no GitLab, as barras do grupo também viram __
  _projeto/<project_dirs>
  epicos/<n>-<slug>/              # slug igual ao da branch do épico (slugify do título, 40)
    README.md                     # gerado uma vez: título, link da issue e sub-issues
    <epic_dirs>
```

- **Modelo:** `docs` do projeto no hub (`enabled`, `project_dirs`, `epic_dirs`), entregue pelo `GET /sync/config` em cada projeto. Hub anterior (sem o campo) = desligado.
- **Quando nasce:** `_projeto/` ao aplicar a atribuição (inclusive depois de uma mudança no modelo, que muda o `rev`) e em toda sessão do projeto; `epicos/<n>-<slug>/` na primeira sessão do épico **ou de uma sub-issue** dele (a pasta do épico pai). O título e as sub-issues vêm do espelho do ambiente.
- **Nunca sobrescreve:** só cria as pastas que faltam; o `README.md` só é escrito se não existe. Título do épico mudou: a pasta existente (`<n>` ou `<n>-*`) é reaproveitada, sem renomear.
- **Marcador `.ct-docs-root`:** ao subir com `docs_dir` configurado e existente, o `cthubd` cria o arquivo `.ct-docs-root` na raiz de `docs_dir` se faltar (nunca o sobrescreve e nunca cria `docs_dir`). O ambiente só considera `docs_root` acessível se ele for pasta **e** tiver o marcador: com a montagem cifs/fstab caída, o ponto de montagem continua ali como pasta local vazia, e sem o marcador o CT não cria a estrutura no disco local. Um `docs_root` apontado à mão para uma pasta que não é o `docs_dir` de um `cthubd` precisa do marcador (`touch <docs_root>/.ct-docs-root`); não apague o arquivo da pasta real.
- **Inacessível:** `docs_root` que não existe, não tem o marcador, não abre ou não responde em 2 s (montagem Samba/CIFS caída ou travada) não cria nada; registra aviso no log e a sessão sobe assim mesmo. `env.info.docs = { root, reachable, warning? }` mostra o estado (`warning: "docs_unreachable"`), e a UI mostra o selo "docs inacessíveis" no ambiente. O teste de acesso roda numa thread à parte: enquanto um teste anterior está preso, os seguintes respondem "inacessível" na hora.
- **Nas sessões (#441):** a sessão de issue, de épico ou de planejamento de um projeto com docs recebe `CT_DOCS_DIR=<docs_root>/<owner>__<repo>` e, se há épico (o próprio ou o pai da sub-issue), `CT_EPIC_DOCS_DIR=<pasta do épico>`; o Claude recebe `--add-dir <CT_DOCS_DIR>` e uma linha no system prompt: artefatos de análise, especificação, prompts e evidências vão em `$CT_EPIC_DOCS_DIR` (sem épico, `$CT_DOCS_DIR/_projeto`), nunca no repositório. A retomada recalcula tudo. Com `docs_root` inacessível, a sessão sobe **sem** as variáveis e o `--add-dir` (para o agente não gravar numa pasta local vazia no lugar da montagem), com o alerta `docs_unreachable` na UI e o evento `docs_unreachable` no log do work item. O gestor não recebe nada disso.
- **Na UI (#444):** em Hub › Projetos › Editar, "Pasta de docs do projeto" liga e desliga e as pastas do modelo vão uma por linha (o hub recebe o `docs` inteiro; hub anterior à 1.8 não mostra os campos). Na tela Projeto, **Abrir pasta de docs** (cabeçalho) e **Abrir pasta de docs do épico** (menu de contexto do épico) pedem ao ambiente `docs.dir { project_id, epic? }`, que cria o que faltar como a sessão faria (reaproveitando a `<n>-*` do épico) e devolve `{ path }`; o app desktop abre a pasta no gerenciador de arquivos (no Windows, `\\wsl.localhost\<distro>\...`). Só ambiente desta máquina, 2.8 ou maior: WSL, ou um "remoto" com URL de loopback (`localhost`, `127.0.0.0/8`, `::1`), como o `ctowerd` local do macOS, que abre o caminho direto (ex.: `/Volumes/ctdocs/...`); o botão fica desabilitado, com a dica do motivo, com docs desligados, sem `docs_root` ou com ele inacessível. `docs.dir` não está no canal reverso: a PWA não abre pastas. Com `docs_unreachable`, o aviso "<ambiente>: pasta de docs inacessível" vai para a faixa "Precisa de você" e para a aba Avisos, com o link para esta seção.
- **Limpeza:** a limpeza do ambiente (`ctower clean`, `cleanup.run`) nunca toca em `docs_root`.
- **Backup:** como o `docs_dir` é a única cópia, o hub gera um pacote completo por dia, no `backup_dir` e no destino remoto (veja [Backup dos docs](#backup-dos-docs)).

#### Onde fica e por que uma cópia só

A pasta mora na máquina do hub (hoje o `hub.local`, Ubuntu; ex.: `/srv/ct-docs`), que é onde roda a maioria das issues (decisão 269 do épico #445). Os outros ambientes (o Mac, o WSL) a acessam por um compartilhamento Samba montado, e o `docs_root` de cada um aponta para a montagem. Sincronização (rsync, unison ou a nativa de cada sistema) foi descartada: acrescenta estado e conflitos. Com uma cópia só, uma exclusão vale para todos (decisão 262), e por isso o backup diário é a proteção (veja "Backup" abaixo). Os docs não vão para o repositório porque são material de trabalho (análises, prompts, evidências) que não deve entrar no histórico do git nem nos PRs, e precisam valer para todos os worktrees e sessões do projeto ao mesmo tempo.

#### Samba no hub (`provision.sh --docs-dir` e `--docs-samba`)

```bash
CT_DOCS_SAMBA_PASSWORD='<senha>' bash scripts/provision/provision.sh --hub --env --docs-dir /srv/ct-docs --docs-samba
# sem a variável, o script pede a senha no terminal (duas vezes)
```

- **`--docs-dir PASTA`** (com `--hub`): cria a pasta (fora do HOME, com `sudo`, já com dono e grupo do usuário do hub; modo 750) e o marcador `.ct-docs-root` na raiz, se faltar (vazio, do usuário do hub; um existente nunca é sobrescrito; o `cthubd` também o cria ao subir), e grava `docs_dir` no topo de `~/.config/cthub/config.toml` (troca um `docs_dir = ""` que já esteja lá). Com `--hub --env`, grava também `docs_root = PASTA` no `config.toml` do ambiente daqui, que lê a pasta direto, sem montagem, e roda `ctower restart` se o `ctowerd` já estava no ar (recusa com sessões vivas, como na ligação ao hub). A pasta é um caminho absoluto só com letras, números e `. _ / + @ -` (vai cru no TOML e no `smb.conf`); `~/` vira o HOME.
- **`--docs-samba`** (com `--hub`, só Linux): instala `samba` e `samba-vfs-modules` (salvo `--skip-packages`), cria o usuário do sistema `ctdocs` (sem login nem home), cria o mesmo usuário no Samba com a senha de `CT_DOCS_SAMBA_PASSWORD` (ou pedida no terminal; mínimo 8 caracteres; nunca como argumento; vai ao `smbpasswd` pelo stdin), grava o bloco `[ctdocs]` em `/etc/samba/ctdocs.conf`, acrescenta `include = /etc/samba/ctdocs.conf` ao `smb.conf`, confere com `testparm`, liga o `smbd` e o recarrega se algo mudou. Com o `ufw` ativo, libera o perfil `Samba` só para as mesmas faixas. Precisa da pasta: `--docs-dir` ou um `docs_dir` já no config do hub.
- **Acesso só da rede local:** `hosts allow` com o loopback e as faixas privadas (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `fe80::/10`, `fd00::/8`) e `hosts deny = ALL`; `guest ok = no` e `valid users = ctdocs`. Para montar também pela tailnet, rode com `CT_DOCS_SAMBA_HOSTS="<as faixas acima> 100.64.0.0/10"`. Outro nome de usuário: `CT_DOCS_SAMBA_USER`.
- **Dono dos arquivos:** `force user` e `force group` são o usuário do hub, que roda o `cthubd` (que lê a pasta no backup) e o `ctowerd` desta máquina. Tudo o que o Mac e o WSL gravam pela montagem fica com esse dono, e os três lados leem e escrevem a mesma coisa. O `ctdocs` não precisa de permissão na pasta.
- **Mac:** `vfs objects = catia fruit streams_xattr` guarda os metadados do Finder em atributos estendidos (sem arquivos `._*`), e `.DS_Store` não é gravado (`veto files`).
- **Idempotente:** rodar de novo não troca a senha de um usuário que já existe no Samba (troque com `sudo smbpasswd ctdocs`), não repete o `include` e só regrava o `ctdocs.conf` e recarrega o `smbd` se o conteúdo mudou. Um `docs_dir` diferente já configurado (no hub) ou um `docs_root` já configurado (no ambiente) fica como está, com aviso.
- **Reinício do hub:** o `cthubd` lê o `docs_dir` ao subir. Se ele já estava no ar, reinicie (`systemctl --user restart cthubd`); o script avisa no fim.

À mão, o mesmo:

```bash
sudo apt-get install -y samba samba-vfs-modules
sudo install -d -o "$USER" -g "$(id -gn)" -m 750 /srv/ct-docs
[ -e /srv/ct-docs/.ct-docs-root ] || touch /srv/ct-docs/.ct-docs-root   # marcador da raiz (o cthubd também cria)
sudo useradd --system --no-create-home --shell /usr/sbin/nologin ctdocs
sudo smbpasswd -a ctdocs                         # pede a senha
sudoedit /etc/samba/ctdocs.conf                  # o bloco abaixo
printf '\ninclude = /etc/samba/ctdocs.conf\n' | sudo tee -a /etc/samba/smb.conf
sudo testparm -s >/dev/null && sudo systemctl enable --now smbd && sudo systemctl reload smbd
```

```ini
[ctdocs]
   comment = Docs dos projetos do Control Tower
   path = /srv/ct-docs
   browseable = yes
   read only = no
   guest ok = no
   valid users = ctdocs
   force user = <usuário do hub>
   force group = <grupo do usuário do hub>
   create mask = 0664
   directory mask = 0775
   hosts allow = 127.0.0.1 ::1 10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 fe80::/10 fd00::/8
   hosts deny = ALL
   vfs objects = catia fruit streams_xattr
   fruit:metadata = stream
   fruit:posix_rename = yes
   veto files = /.DS_Store/
   delete veto files = yes
```

Depois, `docs_dir = "/srv/ct-docs"` no `config.toml` do hub (e reinicie o `cthubd`) e, na mesma máquina, `docs_root = "/srv/ct-docs"` no do ambiente (e `ctower restart`). Teste de outra máquina da rede: `smbclient -L //hub.local -U ctdocs` lista o `ctdocs`. Um hub no macOS compartilha a pasta por Ajustes do Sistema › Geral › Compartilhamento › Compartilhamento de Arquivos; o `--docs-samba` recusa o macOS.

#### Montar no macOS (`/Volumes/ctdocs`, no login)

O macOS guarda a senha nas Chaves (Keychain) e um LaunchAgent monta o compartilhamento em todo login e o remonta se cair. Passo a passo em [`docs/ambiente-macos.md`](ambiente-macos.md), "Docs dos projetos":

1. **Primeira montagem, pelo Finder:** Ir › Conectar ao Servidor (⌘K), `smb://ctdocs@hub.local/ctdocs`, a senha do `ctdocs` e **Lembrar esta senha nas Chaves**. O volume aparece em `/Volumes/ctdocs`.
2. **Script de montagem** (`~/.local/bin/ctdocs-mount`, `chmod +x`): não faz nada se já está montado; senão, `mount volume` do AppleScript, que usa a senha das Chaves sem abrir janela:

   ```sh
   #!/bin/sh
   # Monta o compartilhamento dos docs em /Volumes/ctdocs (senha nas Chaves).
   mount | grep -q ' on /Volumes/ctdocs (' && exit 0
   exec osascript -e 'mount volume "smb://ctdocs@hub.local/ctdocs"'
   ```

3. **LaunchAgent** (`~/Library/LaunchAgents/ai.deep-ion.ctdocs-mount.plist`): roda o script no login (`RunAtLoad`) e a cada 5 min (`StartInterval`), o que remonta depois de uma queda da rede ou do hub. O launchd não expande `~`: use o caminho completo.

   ```xml
   <?xml version="1.0" encoding="UTF-8"?>
   <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
   <plist version="1.0">
   <dict>
     <key>Label</key><string>ai.deep-ion.ctdocs-mount</string>
     <key>ProgramArguments</key>
     <array><string>/bin/sh</string><string>/Users/SEU_USUARIO/.local/bin/ctdocs-mount</string></array>
     <key>RunAtLoad</key><true/>
     <key>StartInterval</key><integer>300</integer>
     <key>StandardErrorPath</key><string>/Users/SEU_USUARIO/Library/Logs/ctdocs-mount.log</string>
   </dict>
   </plist>
   ```

   ```sh
   launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/ai.deep-ion.ctdocs-mount.plist
   ```

   Na primeira vez, o macOS pode perguntar se o `NetAuthAgent` pode usar a senha das Chaves: **Permitir sempre**.
4. **`docs_root = "/Volumes/ctdocs"`** no `~/.config/ctower/config.toml` e `ctower restart`.

Com o compartilhamento desmontado, `/Volumes/ctdocs` não existe (nem o marcador `.ct-docs-root`), e o ambiente vê os docs como inacessíveis (nada é gravado no disco local). Se aparecer `/Volumes/ctdocs-1`, sobrou uma montagem antiga com o mesmo nome: desmonte as duas (`diskutil unmount /Volumes/ctdocs-1`) e rode o script. Alternativa sem script: arrastar o volume montado para Ajustes do Sistema › Geral › Itens de Início monta no login (abrindo uma janela do Finder), mas não remonta se cair.

#### Montar no Windows/WSL (`/mnt/ctdocs` ou `/mnt/<letra>`)

**Recomendado: `cifs` no `/etc/fstab` do WSL**, montado pelo systemd do próprio WSL (o `create-distro.ps1` liga o `systemd=true`), sem depender da sessão do Windows:

```bash
sudo apt-get install -y cifs-utils
sudo install -d -m 755 /mnt/ctdocs
sudo install -m 600 /dev/null /etc/ctdocs.cred
sudoedit /etc/ctdocs.cred              # duas linhas: username=ctdocs e password=<senha>
```

```text
# /etc/fstab — troque 1000 pelo seu uid/gid (id -u, id -g)
//hub.local/ctdocs  /mnt/ctdocs  cifs  credentials=/etc/ctdocs.cred,uid=1000,gid=1000,file_mode=0664,dir_mode=0775,vers=3.0,iocharset=utf8,_netdev,nofail,x-systemd.automount,x-systemd.mount-timeout=15  0  0
```

```bash
sudo systemctl daemon-reload && sudo systemctl restart remote-fs.target
ls /mnt/ctdocs                         # monta no primeiro acesso (x-systemd.automount)
```

O arquivo de credenciais é do root com modo 0600: a senha não fica no `fstab` (que todos leem) nem no histórico do shell. Com o `x-systemd.automount`, a montagem volta sozinha no primeiro acesso depois de uma queda (sem `mount` à mão) e um hub fora do ar faz o acesso falhar em vez de travar; e, como em qualquer montagem caída, a pasta vazia não tem o marcador `.ct-docs-root`, então o ambiente vê os docs como inacessíveis (`docs_unreachable`). Se `getent hosts hub.local` não responder dentro do WSL (o NAT do WSL 2 não resolve `.local`), use o IP do hub (reserva fixa no DHCP) no lugar do nome. Depois, `docs_root = "/mnt/ctdocs"` e `ctower restart`.

**Alternativa: unidade mapeada no Windows**, vista no WSL pelo `drvfs`. No Explorador, Este Computador › Mapear unidade de rede: letra `Z:`, pasta `\\hub.local\ctdocs`, **Reconectar ao entrar** e **Conectar usando credenciais diferentes** (usuário `ctdocs`, com **Lembrar minhas credenciais**: a senha fica no Gerenciador de Credenciais). O WSL não monta sozinho as unidades de rede:

```bash
sudo install -d -m 755 /mnt/z
echo 'Z: /mnt/z drvfs defaults,uid=1000,gid=1000,nofail 0 0' | sudo tee -a /etc/fstab
sudo mount /mnt/z
```

E `docs_root = "/mnt/z"`. A unidade só existe com o usuário logado no Windows e a reconexão feita; se ela não estiver conectada quando o WSL sobe, `/mnt/z` fica vazia: sem o marcador `.ct-docs-root`, o ambiente a vê como inacessível (`docs_unreachable`) e não grava nada ali, mas a montagem só volta com `sudo mount /mnt/z` depois de reconectar a unidade no Windows. Por isso o `cifs` com automount é o recomendado.

#### `docs_root` em cada ambiente e como conferir

| Ambiente | `docs_root` |
| --- | --- |
| Máquina do hub (`hub.local`) | o próprio `docs_dir` (ex.: `/srv/ct-docs`); `provision.sh --hub --env --docs-dir` já grava |
| Mac | `/Volumes/ctdocs` |
| WSL | `/mnt/ctdocs` (`cifs`) ou `/mnt/<letra>` (`drvfs`) |
| Ambiente sem docs | vazio (o padrão): nada é criado ali |

O `ctowerd` lê o `docs_root` ao subir: depois de mudar, `ctower restart`. Para conferir:

- **`env.info.docs`** (`{ root, reachable, warning? }`): `reachable: true` com o `root` esperado é o critério de pronto. `reachable` exige o marcador `<docs_root>/.ct-docs-root` (decisão 106). A UI mostra o selo "docs inacessíveis" no ambiente quando vem `warning: "docs_unreachable"` (#441); pelo hub, `hub.env.call { env, method: "env.info" }` lê o de qualquer ambiente com canal.
- **No terminal:** `ls -a <docs_root>` mostra o `.ct-docs-root` e as pastas `<owner>__<repo>` dos projetos (sem o marcador, a montagem caiu ou aponta para outra pasta); `mount | grep ctdocs` (macOS e Linux) mostra a montagem.
- **No log do ambiente** (`journalctl --user -u ctowerd`, `~/Library/Logs/ctowerd.log`): `docs: docs_root ... inacessível (montagem caída?)` quando uma sessão subiu sem os docs.

`reachable` diz que a pasta abre em até 2 s **e** tem o marcador `.ct-docs-root`, que só existe no `docs_dir` do hub: uma pasta de montagem vazia (montagem caída) não passa, e o ambiente não grava nada no disco local no lugar do compartilhamento. Não apague o marcador; se sumir, o `cthubd` o recria ao subir (ou rode de novo o `provision.sh --hub --docs-dir`).

#### Modelos de pastas (`docs.project_dirs` e `docs.epic_dirs`)

Cada projeto liga os docs e define as pastas que o ambiente cria no campo `docs` do cadastro no hub:

```json
{ "method": "hub.projects.update", "params": { "owner": "deep-ion-ai", "repo": "control-tower",
  "docs": { "enabled": true, "project_dirs": ["analises", "especificacoes"], "epic_dirs": ["analise", "prompts", "evidencias"] } } }
```

O objeto `docs` inteiro substitui o anterior. As pastas são caminhos relativos (`analises/negocio` vale), sem `..`, sem nome oculto e no máximo 64 por lista; `README.md` não vale como pasta do épico. Mudar o modelo muda o `config_rev`: o ambiente recebe no próximo pull e cria as pastas novas (as que existem ficam, nada é renomeado nem apagado).

#### Backup

O `docs_dir` é a única cópia: o hub gera um pacote completo por dia, cifrado, no `backup_dir` e no destino remoto, mantendo os 7 mais novos. Configuração e restauração em [Backup dos docs](#backup-dos-docs) (seção 4); para trazer de volta um arquivo apagado, [Roteiro: recuperar um arquivo dos docs](#roteiro-recuperar-um-arquivo-dos-docs). Os pacotes ficam na máquina do hub e no remoto, nunca nas montagens dos outros ambientes.

### Sessões nos projetos Jira + GitLab

As sessões de um projeto Jira + GitLab (#351, spec `docs/superpowers/specs/2026-10-28-jira-gitlab.md` §7.5, §8.2 e §9) leem e comentam no Jira pelo jira-cli e abrem MR pelo glab, ambos do ambiente. O Control Tower não faz transição de status no Jira nem mescla MR.

- **Templates:** Configurações › Sessões tem três templates próprios (`templates.jira_issue`, `jira_epic` e `jira_project` no `ui.json`), escolhidos pelo rastreador do projeto. Os projetos GitHub continuam com os de sempre. Além de `{{kind}}`, `{{number}}`, `{{owner}}`, `{{repo}}`, `{{branch}}` e `{{protected}}`, todos os templates aceitam `{{key}}` (`#12` ou `PROJ-5450`), `{{title}}`, `{{base}}`, `{{jira_url}}`, `{{jira_project}}`, `{{forge_path}}` (`grupo/projeto/app`), `{{jira}}` e `{{glab}}`.
- **Credencial:** `{{jira}}` vira `JIRA_API_TOKEN="$(<comando de credencial do Jira>)" jira` (com `JIRA_CONFIG_FILE=<arquivo>` antes, quando configurado) e `{{glab}}` vira `GITLAB_HOST=<host> GITLAB_TOKEN="$(<comando de credencial do GitLab>)" glab`. O prompt leva o comando do cadastro, nunca a senha: o shell do agente expande o `$(...)` só no processo do `jira`/`glab`. O token não entra no ambiente da sessão, no `launch_cmd` nem no ledger.
- **O que os templates pedem:** ler com `jira issue view`, abrir o MR com `glab mr create -R <caminho> -s <branch> -b <base>` e, logo depois, comentar o link na issue com `jira issue comment add <KEY> 'MR aberto: <url>'`; criar issue (`jira issue create ... -P <épico>`, que liga ao épico pelo Epic Link, ou `-P <issue>` numa sub-tarefa) só com o ok do usuário. As convenções de cada projeto (formato do commit e do título do MR, branch, restrições de um épico) não ficam no template: vão nas regras do agente do projeto e do épico.
- **Variáveis da sessão:** `CTOWER_TRACKER=jira` em toda sessão do projeto, `CTOWER_ISSUE_KEY=<KEY>` na sessão de issue ou de épico, `GITLAB_HOST` e, se configurado, `JIRA_CONFIG_FILE`. `CTOWER_ISSUE` (o `Closes #n` do `gh pr create`) só existe nos projetos GitHub.
- **Guarda-corpo:** com `CTOWER_TRACKER=jira`, o hook **bloqueia** `jira issue move|edit|delete|assign|link|unlink|clone|worklog|watch` (e os apelidos, como `transition`), `jira epic add|remove` e `jira sprint add|close`, e **pede confirmação** (`permissionDecision: "ask"`) para `jira issue create` e `jira epic create`. Leitura (`view`, `list`, `epic list`, `me`, `serverinfo`) e `jira issue comment add` passam. O `glab mr merge` é sempre bloqueado (#350).
- **Atribuição:** com `attribution` desligada no projeto, o `settings.json` da sessão ganha `"attribution": { "commit": "", "pr": "" }` e `"includeCoAuthoredBy": false`, e o Claude Code não põe o `Co-Authored-By` nem o rodapé no commit e no MR. As duas chaves foram conferidas no schema de settings do Claude Code 2.1.312 (`includeCoAuthoredBy` está marcada como antiga, ainda lida).
- **Prontidão:** ao iniciar a sessão, a modal lê o `env.info.trackers` do ambiente escolhido e avisa o que falta no jira-cli e no glab (ou que o ambiente ainda não conferiu), com a instrução de cada item. Não bloqueia: a sessão inicia mesmo assim.

## 3. Ligar um ambiente ao hub

O ambiente precisa do hub para executar: sem `hub_url` e `hub.token`, o `ctowerd` sobe sem projetos (`projects.list` e `dashboard.get` vazios, `env.info.hub_config.linked = false`, aviso no log e em **Configurações → Projetos**). `session.start` e `session.start_project` são recusados com:

```
ambiente não ligado a um hub: configure hub_url e grave o token com "ctower hub set-token"
```

Hooks, MCP e `session.resume` das sessões que já existiam continuam, para não perder trabalho no meio da migração ou de um erro de config. Ligado ao hub, iniciar sessão num projeto fora do cache (não atribuído ou desatribuído) é recusado com `projeto não atribuído a este ambiente: <owner>/<repo>`. A sessão de planejamento do projeto e as de issue não mudam, só que valem apenas para os projetos atribuídos.

1. **No hub**, gere o token de sync do ambiente (o hub guarda só o hash):

   ```bash
   cthubd add-env notebook     # imprime o token e o env_id (duas linhas); rodar de novo troca o token e mantém o env_id
   cthubd add-env --env-id <uuid> notebook   # prende o token ao env_id que o ambiente já usa (ledger existente)
   cthubd add-env --no-replace notebook   # sai com 3 se "notebook" já tem token
   ```

2. **No ambiente**, grave o token e aponte o hub:

   ```bash
   printf '%s\n%s\n' <token> <env_id> | ctower hub set-token   # grava hub.token (0600) e env_id
   ```

   Um ambiente que já rodou o `ctowerd` tem `env_id` próprio (`~/.config/ctower/env_id`) e o ledger dele foi gravado com esse id: gere o token com `cthubd add-env --env-id <esse id> notebook`. O `set-token` recusa um `env_id` diferente do que o ambiente já usa. Na mesma máquina do hub, `cthubd add-env notebook | ctower hub set-token` grava as duas linhas de uma vez (o `provision.sh` faz isso).

   ```toml
   # ~/.config/ctower/config.toml
   env_name = "notebook"
   hub_url = "http://mac-mini.tailnet:7879"
   retention_days = 30        # poda dos work items inativos; 0 desliga
   ```

3. Reinicie o daemon (`ctower restart`). O token nasce preso ao `env_id` (gerado no hub para um nome novo; o mesmo de antes ao girar o token de um nome existente). Um `X-Ctower-Env-Id` diferente é recusado com `403` já no primeiro uso, então um token girado não pode ser preso a outro ambiente. Só um token de instalação anterior ao #195, sem `env_id`, ainda se prende no primeiro uso, com aviso no log; girá-lo com `add-env` o prende na hora.

4. **Token de agente** (opcional, para o gestor de projetos): no hub, `cthubd agent-token notebook` gera o token (`cta_...`) que só lê `/agent/v1/*`. No ambiente, grave com `echo <token> | ctower hub set-agent-token` (`~/.config/ctower/hub-agent.token`, 0600). O `ctowerd` lê o arquivo a cada chamada, sem restart. O token vale depois do primeiro envio do ambiente (fica preso ao mesmo `env_id` do token de sync). Rodar `agent-token` de novo troca o token, e o anterior deixa de valer na hora. `provision.sh --hub --env` faz isso sozinho.

Para revogar: `cthubd revoke-env notebook` no hub. Isso corta o envio, o pull da configuração e o token de agente. `cthubd revoke-agent-token notebook` corta só o acesso de agente.

### API de leitura para agentes (`/agent/v1`)

Só o token de agente entra (o de sync e o de UI recebem `401`, e o de agente recebe `401` no `/sync/*` e no `/ws`). Toda chamada leva `X-Ctower-Env-Id`. As respostas trazem só títulos, estados e números, e as listas têm `limit` (padrão 50, máximo 200) e `truncated`. Nenhuma rota muda prioridade nem status: a única escrita é criar proposta pendente.

| Rota | Conteúdo |
|---|---|
| `GET /agent/v1/overview` | projetos (na ordem entre projetos) com contagem por status e por prioridade, PRs abertos, decisões pendentes, sinais e atribuições; ambientes |
| `GET /agent/v1/projects/{owner}/{repo}?include_closed&limit` | árvore do projeto (épicos e sub-issues) |
| `GET /agent/v1/work-items/{owner}/{repo}/{number}?events` | item, pai, sub-issues, sessões, PRs, eventos e decisões |
| `GET /agent/v1/ranking?owner&repo&priority&limit` | ranking do portfólio (`hub.ranking.get`) |
| `GET /agent/v1/health?owner&repo&min_severity` | sinais de saúde, por padrão a partir de `aviso` |
| `GET /agent/v1/decisions?owner&repo&number&scope&origin&unreviewed&since&limit` | decisões, da mais nova para a mais velha |
| `GET /agent/v1/proposals?state&owner&repo&limit` | propostas do gestor, por padrão as pendentes (`state=todos` traz todas) |
| `POST /agent/v1/proposals` | cria uma proposta pendente (`kind`, `owner`, `repo`, `number`, `proposed`, `justification`), com a sessão em `X-Ctower-Session` |

### Propostas do gestor

O gestor só propõe mudanças de prioridade (`{ "priority": "P1", "before"?: n, "after"?: n }`) e de status manual (`backlog`, `bloqueado`, `pausado`, `concluido`). Quem aprova ou rejeita é o usuário, na UI do hub (`hub.proposals.list` e `hub.proposals.decide { id | ids, approve, note? }`). O hub aplica a aprovação na mesma transação e registra uma decisão `orientada` no work item. A proposta vira `obsoleta` quando o valor atual do alvo muda, quando chega outra do mesmo tipo para o mesmo alvo e depois de 14 dias pendente. O hub aceita no máximo 100 pendentes. As UIs conectadas recebem `hub.proposals.changed` quando a lista muda.

### Pull da configuração

O ambiente puxa do hub a configuração dos projetos atribuídos a ele, no boot e a cada `config_pull_interval_secs` (padrão 60; `0` desliga o laço), e também em `ctower hub sync` e `hub.sync.now`, que puxam e depois enviam o outbox.

```toml
# ~/.config/ctower/config.toml
config_pull_interval_secs = 60
repos_root = "~/projetos"     # repo_path vazio na atribuição = <repos_root>/<repo>
docs_root = ""                # pasta de docs dos projetos (#440); vazio = sem docs neste ambiente
secrets_max_age_hours = 24    # idade da configuração que dispara o aviso no spawn; 0 desliga
```

- **Pedido:** `GET <hub_url>/sync/config?since=<config_rev local>&epoch=<config_epoch local>`, com o token de sync e o cabeçalho `X-Ctower-Env-Id`. Divergir do `env_id` preso ao token dá `403`, já no primeiro uso; só um token de instalação antiga (sem `env_id`) fica preso ao do cabeçalho, com aviso no log.
- **Resposta:** `{ config_rev, config_epoch, changed: false }` quando nada mudou na mesma época; senão `{ config_rev, changed: true, projects, removed }`. `projects` é o **conjunto completo** atribuído ao ambiente (`owner`, `repo`, `name`, `base_branch`, `protected_branches`, `repo_path`, `rev` e, desde a #440, `docs`; o ambiente trata a falta dele como docs desligados); `removed`, as atribuições deste ambiente removidas depois de `since` (inclusive as de projetos excluídos). Projetos não atribuídos nunca aparecem.
- **Época:** `config_epoch` é aleatória e o `cthubd restore` sorteia outra. Um hub restaurado pode voltar ao mesmo `config_rev` com outro conteúdo; com a época diferente, o hub responde sempre o conjunto completo.
- **Cache:** a tabela `project` do `ledger.db` vira cache **somente leitura**: `config_rev` (nulo = projeto local legado), `unassigned_at` e o último relato. O `/ws` do ambiente não tem métodos para criar, editar ou excluir projetos, e o ambiente não envia mais `project_removed` (o hub ainda aceita e ignora o de ambientes antigos). Gravar o cache nunca põe fotografia de projeto no outbox; só o relato sobe. `projects.list` traz só os atribuídos; `projects.list { unassigned: true }`, os desatribuídos.
- **Aplicar:** o pull só grava o cache; a aplicação roda à parte (uma por vez) e nem o pull seguinte nem o envio do outbox esperam por um clone. `hub.sync.now` valida na hora as atribuições cujo clone já existe (o relato vai no mesmo envio), dispara o clone das demais em segundo plano e devolve em `config.pending_applies` quantas estavam pendentes; o relato de um clone novo vai ao hub no envio seguinte (o laço de envio roda a cada 5 s). Para cada atribuição nova, alterada ou com erro, o ambiente clona com o `gh` (o mesmo fluxo do cadastro por URL) ou valida o clone existente (precisa ser um clone de `owner/repo`) e relata `ok` ou `error` com a mensagem e o `applied_rev`. Atribuição com erro é tentada de novo a cada pull, sem repetir o relato igual. O caminho efetivo é resolvido de novo em todo pull: mudar `repos_root` (ou o `HOME`) no ambiente faz a atribuição voltar a pendente, ser clonada ou validada no caminho novo e relatada. Mudar `base_branch` ou as protegidas no hub só atualiza o cache, sem reiniciar o daemon; as sessões iniciadas depois (e o guarda-corpo delas) usam o cache novo.
- **Remover:** todo projeto do cache que não veio em `projects` ganha `unassigned_at` e sai da lista. O clone, os worktrees e as sessões ficam; a limpeza é à mão. O espelho do GitHub do ambiente (`issue_cache`, `pr_cache`) só sincroniza os atribuídos.
- **Primeiro pull:** projetos locais legados que vieram em `projects` são casados por `owner/repo` e mantêm o `id` local (o histórico continua ligado a eles). Legados que o hub não atribuiu ganham `unassigned_at` ("projeto local fora do hub"), sem nada ser apagado do disco.
- **Hub antigo:** `404` em `/sync/config` (o `ctowerd` com `hub = true` do #64, durante a migração) não é erro: os projetos locais legados continuam valendo até o primeiro pull que responder, e `hub_config.legacy_hub` fica `true`.
- **Concluídos do hub (#298):** o status dos work items não move o `config_rev`, então o `concluido` marcado à mão na tela Hub vem por `GET <hub_url>/sync/done` (mesmos cabeçalhos): o conjunto completo dos work items `concluido` dos projetos atribuídos. O `ctowerd` o pede em todo ciclo do pull e substitui o cache `hub_done` do `ledger.db`; `ct_sessions::cleanup::completion` o usa como sinal `work item concluído` (um PR aberto da branch ainda vence). `404` = hub antigo, ignorado.

### Secrets do ambiente

O hub entrega a cada ambiente os secrets e as variáveis globais e dos projetos atribuídos a ele, junto do pull da configuração (#184). Projetos não atribuídos nunca saem do hub.

- **Chave do ambiente:** o `ctowerd` gera no primeiro pull uma identidade age própria em `~/.config/ctower/env.key` (`0600`) e manda a chave pública em todo `GET /sync/config`, no cabeçalho `X-Ctower-Env-Pubkey`. O hub guarda a chave pelo `env_id` (tabela `env_key` do `hub.db`); só o token preso àquele `env_id` chega a ela. O registro do ambiente é só o `cthubd add-env` no hub, então a chave vai no pull, que roda no boot.
- **Entrega:** quando a resposta é de mudança (`changed: true`, inclusive com a época diferente), ela leva em `secrets` o arquivo age em base64 com o **conjunto completo** das entradas, cifrado só para a chave do ambiente. Se a chave pública mudou desde a última entrega, o hub responde `changed: true` mesmo com `since` em dia. Por dentro, o plaintext é `{"format": 1, "config_rev": N, "entries": [{scope, name, kind, value, rev, updated_at}]}`. Sem o cabeçalho (ambiente antigo), não há `secrets`; hub antigo não manda o campo, e o ambiente segue sem cache.
- **Cache:** o ambiente grava o arquivo recebido como está em `~/.local/share/ctower/secrets-cache.age` (`0600`, escrita atômica), depois de conferir que ele abre com `env.key`. Como o conjunto é completo, remover uma atribuição tira do cache os secrets do projeto no pull seguinte. Com o hub fora do ar, vale o último cache (veja "Secrets nas sessões" para o aviso de idade). `hub.sync.now` e `ctower hub sync` mostram em `config.secrets` quantas entradas chegaram.
- **Revogar:** `cthubd revoke-env` corta o pull (`401`) e apaga a chave pública do ambiente no hub. O cache do ambiente fica como estava; apague `secrets-cache.age` à mão se a máquina saiu de uso. Se a máquina foi perdida, siga o [roteiro](#roteiro-perda-de-uma-máquina-de-ambiente).
- **Trocar a chave:** `ctower secrets rotate-env-key` gera uma identidade nova, recifra o cache para ela (continua usável offline), troca `env.key` e puxa a configuração; o hub reentrega para a chave nova. Com o `ctowerd` fora do ar, o comando troca a chave ali mesmo e o hub recebe a nova no próximo pull.

### Secrets nas sessões

No spawn e no resume de uma sessão (#166), o `ctowerd` lê o cache, resolve as entradas globais e as do projeto da sessão (`owner/repo`; por nome, a do projeto sobrepõe a global) e as passa como variáveis de ambiente do processo do agente, ao lado das `CTOWER_*`.

- **Onde os valores vão:** só no ambiente do processo. Não entram em `session.launch_cmd`, nos eventos do `ledger.db`, nos arquivos da sessão (`mcp.json`, `settings.json`) nem nos logs, que citam só nomes.
- **Nomes reservados:** `CTOWER_*` (o que o daemon exporta: `CTOWER_SESSION`, `CTOWER_URL`, `CTOWER_TOKEN`, `CTOWER_PROTECTED`, `CTOWER_SPOOL_DIR`, `CTOWER_ISSUE`, `CTOWER_SESSION_KIND`, `CTOWER_TRACKER` e `CTOWER_ISSUE_KEY`), `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `TERM`, `COLORTERM`, `COLUMNS`, `LINES`, `PWD`, `OLDPWD`, `LANG`, `LC_*`, `LD_*`, `DYLD_*`, `CLAUDE_CONFIG_DIR` e `CLAUDECODE` (`ct_secrets::is_reserved_name`). O hub recusa gravá-los (`hub.secrets.set` responde `-32602`, "nome reservado") e a UI avisa antes de enviar. Um reservado que já esteja no cache é ignorado no spawn, com aviso no log só com o nome; de qualquer forma, as `CTOWER_*` do daemon vêm por último e sempre vencem.
- **Quando uma mudança vale:** no próximo spawn ou resume depois do pull que atualizou o cache. Sessões em andamento não mudam (o ambiente de um processo não muda depois que ele sobe); para pegar o valor novo, encerre e retome a sessão. A UI do hub repete isso ao gravar ou remover.
- **Sem cache** (ambiente sem hub ou sem entrega ainda): a sessão sobe sem secrets, sem aviso.
- **Cache ilegível** (chave trocada à mão, arquivo corrompido): a sessão sobe sem secrets, com aviso no log e o alerta `secrets_unavailable` na faixa "Precisa de você". Recusar o spawn travaria o trabalho por algo que o próximo pull corrige (o hub reentrega).
- **Idade:** medida desde a última sincronização bem-sucedida com o hub (`config_pulled_at` do `ledger.db`; um pull sem mudança também prova que o cache está em dia) ou, sem pull registrado, desde a gravação do cache. Acima de `secrets_max_age_hours` (padrão 24; `0` desliga), o spawn segue com o cache e avisa no log e com o alerta `secrets_stale`; `env.info.hub_config` traz `max_age_ms` e `stale`, e a linha da configuração do ambiente na UI fica em destaque ("desatualizada: sessões novas usam os secrets do cache").

#### Mascaramento na saída e no backup do transcript

Se o agente imprimir um secret (`env`, `echo $API_TOKEN`), o valor acabaria no scrollback, na tela e no backup do transcript. O `ctowerd` troca cada ocorrência por `***NOME***` (#167). Isso reduz a exposição na UI e nos backups, mas não protege contra o próprio agente, que tem o valor no ambiente.

- **O que é mascarado:** só os **secrets** efetivamente injetados na sessão (depois de filtrar os reservados) com pelo menos 8 bytes (`ct_secrets::MIN_MASK_LEN`); valores mais curtos mascarariam texto comum. A UI do hub avisa ao cadastrar um secret menor ("valores com menos de 8 caracteres não são mascarados..."), sem impedir a gravação. Variáveis não são mascaradas.
- **Saída do PTY:** o filtro roda nos bytes lidos do PTY, antes do scrollback e do broadcast para os clientes. Trabalha em bytes (um valor pode cruzar fronteiras de UTF-8 e de leitura). Um valor partido entre leituras é reconhecido: a cauda que ainda pode ser começo de um valor fica retida até a leitura seguinte (no máximo o tamanho do maior valor − 1 bytes) e o resto sai na hora. Se nada chega em 50 ms, a cauda sai como está, para não congelar a tela (o eco de uma tecla que coincide com o começo de um secret, por exemplo); um valor impresso de uma vez chega em leituras seguidas, bem dentro desse prazo. No fim do processo, a cauda é descarregada. Se dois valores começam no mesmo ponto, vence o mais longo.
- **Backup do transcript:** a cópia em `transcripts/` passa pelo mesmo conjunto, mais a forma escapada em JSON de cada valor (o transcript é JSONL); o arquivo original do Claude não é tocado. Ao retomar uma sessão a partir do backup, o transcript restaurado traz `***NOME***` no lugar do valor.
- **Onde fica o conjunto:** só em memória no `ctowerd`, associado à sessão; nunca é gravado. Sai quando a sessão é retomada (a nova ganha o próprio) ou arquivada. Se um backup acontece sem ele (o daemon reiniciou), o conjunto é recalculado do cache atual pelo projeto da sessão; se o cache mudou nesse meio tempo, valem os valores atuais. Cache ausente ou ilegível: o backup sai sem mascaramento, com aviso no log.
- **Resume:** a sessão retomada recebe os valores atuais do cache e mascara esses. Um valor injetado antes e trocado no hub depois não é mais mascarado.

### Secrets e variáveis pela UI do hub

Os secrets e as variáveis são cadastrados pelo `/ws` do hub (token de UI do hub, #165). A UI só escreve secrets: nenhuma resposta nem erro carrega o valor de um, e editar é digitar um valor novo.

- **Listar:** `hub.secrets.list` devolve `status` (`entries`, `store_rev`, `recovery_configured`), `items` (`{scope, name, kind, rev, updated_at, value?}`, com `value` só para `variable`) e `delivery` (um por ambiente: `state`, `applied_rev`, `required_rev`, `projects`) e `rotate_pending` (`null` ou `{restored_at, items: [{scope, name}]}`, a lista a rotacionar na origem depois de um `cthubd secrets restore` com a chave de recuperação).
- **Dispensar a lista a rotacionar:** `hub.secrets.rotate_pending_dismiss` apaga `secrets.rotate-pending.json` e responde `{}`.
- **Gravar:** `hub.secrets.set { scope, name, kind, value }`. `scope` é `"global"` ou `{"project": "owner/repo"}` (o projeto precisa estar cadastrado no hub); `kind` é `secret` ou `variable` (trocar o tipo de uma entrada existente é permitido); o nome segue `[A-Z_][A-Z0-9_]*` e não pode ser reservado (veja "Secrets nas sessões"); secret não aceita valor vazio. Responde `{ rev }`, o `config_rev` da mudança.
- **Remover:** `hub.secrets.delete { scope, name }` responde `{ rev }`.
- **Erros:** `-32602` para parâmetro inválido, `-32020` para hub sem store de secrets, `-32021` para entrada inexistente na remoção e `-32022` para projeto não cadastrado.
- **Entrega por ambiente:** `no_key` = o ambiente nunca mandou a chave pública; `pending` = a chave mudou desde a última entrega ou o ambiente ainda não confirmou a revisão necessária; `delivered` = confirmou. A confirmação é o `since` do pull, que o ambiente só avança depois de gravar o cache de secrets: o hub o guarda em `env_key.applied_rev` (com a época) quando a chave do pull já recebeu uma entrega. A revisão necessária é o maior `rev` entre as entradas e a última remoção do escopo global, as entradas e a última remoção de cada projeto atribuído e, para projetos que têm ou já tiveram entradas, a última mudança da atribuição (inclusive removida) ou do projeto. É conservador: uma mudança de projeto sem efeito nos secrets também deixa o ambiente pendente até o próximo pull.

Na UI, a tela **Hub** tem a seção **Secrets e variáveis** (escopo global, com a entrega por ambiente), e a tela do projeto pela visão do hub abre a do projeto no botão **Secrets e variáveis**, com os globais herdados e a marca "sobrescreve o global" quando o projeto tem o mesmo nome.

### Chave de recuperação

A chave de recuperação (#169) abre o store de secrets se a máquina do hub for perdida. A chave privada fica fora da máquina, no gerenciador de senhas do usuário (qualquer um, sem integração); o hub guarda só a pública. A senha mestra do gerenciador e o recovery dele não podem depender só desta máquina: teste o acesso ao gerenciador pelo celular.

- **Gerar:** `hub.recovery.generate` cria uma identidade age e devolve `{ private_key, public_key, status }`. A `private_key` (`AGE-SECRET-KEY-1...`) só sai nesta resposta; a pública fica pendente em `secrets.recovery.pending.pub` (`0600`). Gerar de novo troca a pendente. Com uma chave já configurada, gerar inicia uma troca: a atual vale até a nova ser verificada.
- **Verificar:** `hub.recovery.verify { key }` recebe a chave colada. Se a pública dela é a pendente, o hub cifra o conteúdo atual do store para hub + chave colada e decifra em memória (ida e volta), regrava `secrets.age` para os dois destinatários, confere que o arquivo gravado abre com a chave colada e só então promove a pendente a `secrets.recovery.pub`. Com a promoção, o store regravado vira o snapshot mais recente e os snapshots antigos são recifrados para hub + recuperação (mesmo nome e mtime). Se a pública é a já configurada, é a reverificação periódica: confere que o snapshot de secrets mais recente (ou o `secrets.age`, sem snapshot) abre com ela; se não abre, regrava o store, tira um snapshot, recifra os antigos e confere de novo. A chave colada nunca é gravada nem aparece em log ou erro. Responde o `status`.
- **Data da verificação:** `secrets.recovery.verified` (`0600`), JSON `{verified_at, recipient}`; a pública amarra a data à chave, então uma troca começa sem data. `recovery_reverify_days` (padrão 90; `0` desliga) define o lembrete `recovery_reverify_due` em `hub.status`; uma pública posta à mão, nunca verificada, já entra vencida.
- **Estado:** `hub.recovery.status` devolve `configured`, `public_key`, `pending`, `pending_public_key`, `last_verified_at`, `reverify_days`, `next_verify_at` e `reverify_due`.
- **Erros:** `-32602` para chave ausente ou que não é uma identidade age, `-32020` para hub sem store de secrets, `-32023` para verificar sem ter gerado e `-32024` para "a chave não confere" (nada muda).
- **Restaurar** sem o hub: `age -d -i <arquivo com a chave privada> secrets-<AAAAMMDDTHHMMSSZ>.age` (o snapshot mais recente na pasta sincronizada, ou o `secrets.age`) mostra o JSON do store.

Na UI, a tela **Hub** tem a seção **Chave de recuperação**: o estado, o botão para gerar (mostra a chave uma vez, com **Copiar**), o campo para colar e verificar e **Verificar de novo** para o lembrete periódico.

### Snapshots de secrets

O store mestre vive na máquina do hub; perder a máquina leva o store junto. Como o `secrets.age` é cifrado para a chave do hub e para a de recuperação, as cópias dele podem ir para uma pasta sincronizada sem expor nada (#168).

- **Destino:** `secrets_snapshot_dir` no `config.toml` do hub; vazio = o mesmo `backup_dir` em vigor dos snapshots do `hub.db` (o da pasta escolhida na tela **Hub**, gravado no `backup.json`, senão o do `config.toml`), então um destino só leva os dois para fora da máquina. Trocar a pasta pela UI muda também o destino dos snapshots de secrets na hora, com um snapshot já no destino novo; com `secrets_snapshot_dir` preenchido, ele não acompanha. Os nomes não se confundem (`hub-*.db` e `secrets-*.age`) e cada um tem a sua rotação. Exemplos:

  ```toml
  # WSL: OneDrive do Windows
  backup_dir = "/mnt/c/Users/<usuario>/OneDrive/cthub"
  # Mac: iCloud Drive ou OneDrive
  backup_dir = "~/Library/Mobile Documents/com~apple~CloudDocs/cthub"
  backup_dir = "~/Library/CloudStorage/OneDrive-Pessoal/cthub"
  # ou só os secrets num lugar diferente do hub.db:
  secrets_snapshot_dir = "/mnt/c/Users/<usuario>/OneDrive/cthub-secrets"
  ```

- **Quando:** logo depois de cada `hub.secrets.set`/`delete` bem-sucedido (e da promoção da chave de recuperação), uma cópia byte a byte do `secrets.age` como `secrets-<AAAAMMDDTHHMMSSZ>.age` (UTC; `-2`, `-3` no mesmo segundo), `0600`, com escrita atômica. Uma falha no destino nunca desfaz nem falha a alteração: ela fica no store local e vira aviso.
- **Rotação:** ficam os 20 mais recentes e, além deles, o mais recente de cada dia (UTC) dos últimos 30 dias. Só arquivos `secrets-*.age` são apagados.
- **Conferência:** no start e a cada hora, o hub confere se o destino está acessível e gravável e se o snapshot mais recente tem o mesmo SHA-256 do `secrets.age`; se o destino responde e não bate, tira um snapshot na hora (é o que normaliza depois de uma falha). Avisos em `hub.status` (e na faixa **Precisa de você**): `secrets_snapshot_unreachable` (destino inacessível ou última cópia falhou), `secrets_snapshot_stale` (o mais recente não corresponde ao store) e `secrets_snapshot_same_device` (destino no mesmo disco do store; aponte para a pasta sincronizada ou configure um destino remoto: como no `hub.db`, o aviso some enquanto o último envio remoto, que leva a cópia do `secrets.age`, deu certo há menos de 2 × `backup_interval_hours`).
- **Abrir em outra máquina:** um snapshot copiado abre só com a chave de recuperação, com `cthubd secrets restore` (abaixo) ou, só para ler, `age -d -i <arquivo com a chave privada> secrets-<...>.age`. Snapshots tirados antes da recuperação configurada são recifrados quando a chave é verificada.
- **Histórico da pasta sincronizada:** o OneDrive (e o iCloud) guarda versões antigas e a lixeira por um tempo. Um snapshot apagado pela rotação, ou recifrado pelo `rotate-keys`, pode continuar lá na versão anterior, aberto pelas chaves da época. Depois de uma rotação por suspeita de vazamento, apague também o histórico de versões e a lixeira da pasta.

### Restaurar e rotacionar as chaves dos secrets

Dois subcomandos do `cthubd` (#170; a issue escrevia `cthub`, mas o binário é o `cthubd`), os dois **com o serviço parado**: recusam, com a mesma mensagem do `cthubd restore`, se houver algo escutando na porta do hub. O destino dos snapshots é o `secrets_snapshot_dir` do config (vazio = o `backup_dir` em vigor, o do `backup.json` se houver).

**`cthubd secrets restore <arquivo> [--identity <arquivo>] [--yes]`** restaura o store de um snapshot (`secrets-*.age`):

1. Abre o arquivo com a `secrets.key` desta máquina, se ela existir e abrir. Senão, usa a chave de recuperação, lida do arquivo de `--identity` ou colada no terminal (sem eco; fora de um terminal, lê uma linha do stdin). Uma chave errada não muda nada e não aparece na mensagem de erro.
2. Lista escopo, nome e tipo de cada entrada (**nunca valores**) e pede confirmação (`s/N`; `--yes` pula). Sem confirmar, sai com `2` e nada muda.
3. Grava o snapshot como o novo `secrets.age` (escrita atômica). O store anterior, se havia, fica como `secrets.age.pre-restore-<AAAAMMDD-HHMM>` (`0600`).
   - **Aberto com a chave do hub:** recifra para o hub e para a recuperação atual (se configurada). A lista a rotacionar não muda.
   - **Aberto com a chave de recuperação:** a chave foi exposta ao ser colada numa máquina nova, então o hub gera uma `secrets.key` nova (a anterior, se havia, por exemplo de um `serve` que subiu antes, fica como `secrets.key.pre-restore-<...>`) e recifra **só** para ela. Apaga `secrets.recovery.pub`, `secrets.recovery.pending.pub` e `secrets.recovery.verified`: a tela **Hub** volta a mostrar "recuperação não configurada" e pede uma chave nova pelo fluxo de [Chave de recuperação](#chave-de-recuperação). Grava `secrets.rotate-pending.json` (`0600`) com `{restored_at, items: [{scope, name}]}` dos `secret` (sem valores, sem as variáveis).
4. Nos dois casos, o `config_rev` do `hub.db` passa do maior `rev` do store restaurado e sobe mais um (um `hub.db` novo começa em 0). Assim, cada ambiente recebe os secrets de novo no próximo pull. Também sai um snapshot do store restaurado no destino configurado.

A lista vem em `hub.secrets.list` (`rotate_pending`) e vira o aviso `secrets_rotate_pending` em `hub.status` (e na faixa **Precisa de você**). A seção **Secrets e variáveis** da tela **Hub** mostra **Secrets a rotacionar na origem**, com o botão **Já rotacionei** (`hub.secrets.rotate_pending_dismiss`, que apaga o arquivo).

**`cthubd secrets rotate-keys [--hub] [--recovery]`** (sem flags, as duas):

- `--hub`: gera uma `secrets.key` nova, recifra o `secrets.age` para ela e para a recuperação **configurada** e recifra cada snapshot do destino (aberto com a chave antiga ou a nova), com o mesmo nome e mtime. Os snapshots que nenhuma das duas abre, ou que não deu para recifrar, são **apagados** (a contagem sai no terminal). Depois sai um snapshot do store recifrado. A chave antiga só some do disco depois que tudo deu certo. Numa falha no meio, ela fica em `secrets.key.old`, e o erro diz o que faltou. A troca é segura contra queda: o store é gravado primeiro para a antiga e a nova juntas (`secrets.key.new`), depois a chave é trocada e o store regravado só para a nova. O `config_rev` não muda, porque os valores são os mesmos e a entrega aos ambientes usa a chave de cada ambiente.
- `--recovery`: gera uma chave de recuperação nova e imprime a privada **uma vez** no terminal, para guardar no gerenciador de senhas. A pública fica **pendente** (`secrets.recovery.pending.pub`). A atual continua valendo até você colar a nova na tela **Hub** e ela ser verificada, como no fluxo normal da [Chave de recuperação](#chave-de-recuperação). Só então o store e os snapshots são recifrados para a nova, e a antiga deixa de abri-los. Por isso, com as duas flags, o `--hub` recifra para a recuperação atual, e não para a pendente.

Cópias fora do destino (histórico de versões da pasta sincronizada, cópias à mão, `secrets.age.pre-restore-*` e `secrets.key.pre-restore-*`) não são tocadas e continuam abrindo com as chaves da época.

**`cthubd secrets list [--env <nome>]`** lista escopo, nome e tipo das entradas, nunca valores. Com `--env`, mostra só o que o hub entrega a esse ambiente: as globais e as dos projetos atribuídos a ele. Só lê o store e o `hub.db`, então funciona com o serviço no ar.

`env.info.hub_config` traz `linked` (há `hub_url` e `hub.token`), `config_rev` local, `pulled_at` e `age_ms` (idade do cache), `max_age_ms` e `stale` (limite `secrets_max_age_hours` e se ele passou), `last_error`, `failures` e `legacy_hub`.

### Hub fora do ar

Nada para: sessões, hooks e decisões gravam localmente e esperam no outbox, e o ambiente usa a última configuração em cache. Envio e pull tentam de novo com backoff até 5 min. `env.info` e a UI mostram as pendências, o `config_rev` local e a idade dele. O MCP `ledger_list_decisions` com escopo `epic`/`project` devolve as decisões locais com `partial: true` se o hub não responder.

### Canal reverso (`/sync/channel`)

Com `hub_url` e `hub.token`, o `ctowerd` abre e mantém um WebSocket com o hub em `ws://<hub>/sync/channel` (o `http://` do `hub_url` vira `ws://`; sem TLS, como o resto do sync). O hub nunca abre conexão com o ambiente, então um ambiente atrás de NAT (WSL, Mac mini em outra rede) continua alcançável. O sync por `/sync/push` e `/sync/config` não muda: um ambiente sem o canal (versão antiga) continua sincronizando, só fica `online: false`.

- **Autenticação:** a mesma do `/sync/config`: token de sync e `X-Ctower-Env-Id`. Token de outro ambiente (ou `env_id` diferente do preso ao token) dá `403`; token de agente ou inválido, `401`. Vindo pela Cloudflare (`Cf-Connecting-IP`), `404`, como todo `/sync/*`.
- **Um canal por ambiente:** a conexão nova do mesmo `env_id` substitui a anterior, que fecha (as chamadas em voo nela falham como offline).
- **Chamadas:** o hub manda `{id, method, params}` e o ambiente responde `{id, result}` ou `{id, error}` com o mesmo despacho do `/ws` dele. Prazo de 30 s por chamada no hub (`-32031` "ambiente não respondeu"); ambiente sem canal, `-32030` "ambiente offline". Só passam os métodos da lista do canal, conferida nos dois lados (o ambiente recusa com `-32601` mesmo que o hub peça): `env.info`, `projects.list`, `dashboard.get`, `tree.get`, `review.get`, `events.list` (#422), `review.mark_reviewed`, `recovery.list`, `recovery.archived`, `transcript.tail`, `session.context`, `session.get`, `session.start`, `session.start_project`, `session.start_gestor`, `session.resume`, `session.kill`, `session.archive`, `session.remote_control.enable`, `hub.sync.now` e `tracker.issue_body` (descrição da issue do Jira para o "Ver issue", #434). Ficam de fora o terminal (`session.attach|detach|input|resize|peek` e o canal binário), o shell livre (`shell.*`, #428), `session.permission.*`, `env.update|rollback|check_update`, `env.config.*`, `cleanup.*`, `github.*` e `sync.now`.
- **Pushes do ambiente:** só `session_started`, `session_state`, `session_exited`, `session_archived`, `sessions_interrupted`, `needs_human`, `work_item_status`, `projects_changed` e `alert`; `permission_request` (traz o `tool_input`), `update.status`, `cleanup.status` e `shell_exited` não saem do ambiente.
- **Online em tempo real:** `HubEnv.online` em `hub.envs.list` e `hub.dashboard.get` é o canal aberto agora, e o `/ws` e o `/web/ws` do hub recebem `env.online { env_id, online }` quando ele abre ou fecha. O hub manda ping a cada 30 s; sem resposta em 60 s, o ambiente fica offline. O ambiente reconecta com o mesmo backoff do envio (5 s dobrando até 5 min) e, sem nenhum frame do hub em 90 s, fecha e reconecta.
- **`hub.env.call { env, method, params }`** (#343), no `/ws` e no `/web/ws`: chama `method` no ambiente `env` (o `env_id`) pelo canal e devolve o resultado ou o erro dele. `params` é um objeto (ausente = `{}`). Fora da lista do canal, `-32601` "método indisponível pelo hub: …"; ambiente sem canal agora, `-32030` "ambiente offline: <nome> está sem canal com o hub agora (…)"; `env_id` que o hub não conhece, `-32602`. A PWA usa para iniciar sessões (com `remote_control: true`), retomar, encerrar, ligar o Remote Control (`session.remote_control.enable`), marcar decisões como revisadas (`review.mark_reviewed`) e ler `env.info` e `session.get`.
- **`env.push { env_id, event, data }`:** cada push do ambiente da lista acima chega ao `/ws` e ao `/web/ws` assim; a UI relê a visão do hub com ele.
- **`env_stale` da Web Push em tempo real:** ambiente que abriu canal desde que o hub subiu fica "fora do ar" quando o canal fecha há mais de 1 min (uma notificação por queda); os outros seguem pela idade do último sync (`env_stale_minutes`).

### Sincronizar agora: `ctower hub sync`

Puxa a configuração do hub e envia o outbox na hora, sem esperar o ciclo nem o backoff (a UI usa o botão **Sincronizar com o hub**, método `hub.sync.now`; o resultado do pull vem em `config`). Sai com `1` se o hub não confirmou tudo ou se o pull falhou, e mostra o motivo:

- `hub inacessível: ...`: hub fora do ar ou fora da tailnet. Nada se perde.
- `hub recusou o token de sync (401)`: gere outro com `cthubd add-env <nome>` e grave com `ctower hub set-token`.
- `hub recusou o token de sync (403)`: o token está preso ao `env_id` de outro ambiente (confira o `env_id` gravado com o token).
- `ambiente não ligado a um hub`: falta `hub_url` ou o `hub.token`.

## 4. Poda e backup

Depois que o hub confirma o envio, o ambiente apaga os work items inativos mais velhos que `retention_days` (padrão 30; `0` desliga). **O hub passa a ser a única cópia do histórico**, então o backup dele é crítico.

### Poda no ambiente

Um work item é podado quando **tudo** vale:

- **não está ativo:** `concluido` (com `closed_at`) ou o projeto está desatribuído (`unassigned_at`); e nenhuma sessão dele está em `starting`, `running`, `idle` ou `interrupted` (recuperação pendente conta como ativa). `bloqueado`, `pausado`, `backlog` e os demais nunca são podados enquanto o projeto estiver atribuído;
- **confirmado pelo hub:** o `sync_outbox` está vazio, verificado dentro da transação da poda (o outbox só esvazia com o ack), e o backfill do ledger anterior à replicação já foi feito;
- **velho:** `closed_at` (ou `unassigned_at`) e o `last_event_at` da sessão mais recente passaram de `retention_days`, pelo relógio do ambiente;
- **fora da revisão local:** a revisão das decisões só existe no ambiente, então não é podado o work item com decisão `autonoma` ainda não revisada (nem o épico com decisão pendente numa sub-issue), nem a sub-issue cujo épico ainda tem work item não concluído aqui.

Quando roda: logo depois de um envio sem erro que deixou o outbox vazio, ainda dentro do envio em voo (nunca em paralelo a outro). No laço de envio, no máximo uma vez a cada 10 min; em `ctower hub sync` e `hub.sync.now`, sempre.

O que sai, numa transação `IMMEDIATE`: as linhas `event`, `decision`, `pr_cache`, `session` e `work_item` do item, e os ETags dos PRs do projeto (a próxima busca no GitHub vem completa, e um item recriado não perde o PR). As triggers append-only aceitam o `DELETE` só com o id do work item na tabela `pruning`, marcado e removido antes do commit (mesmo padrão da `purging`). Um candidato que não pode sair (ex.: decisão de outro work item apontando a sessão dele) é desfeito sozinho e fica para a próxima poda, com aviso no log. Depois do commit saem `<data>/sessions/<id>` e o backup do transcript em `<data>/transcripts` (só se nenhuma sessão restante usa o mesmo arquivo ou `tool_session_id`). Falha ao apagar um arquivo vai para o log e não desfaz a poda. Worktrees, branches e o transcript original do agente não são tocados.

Projeto desatribuído há mais de `retention_days` e sem work items restantes: saem também a linha do cache (e com ela a faixa "o que restou para limpar à mão" da UI), os eventos do projeto e o espelho do GitHub dele (`issue_cache`, `pr_cache`, `sync_state`, ETags). Se for atribuído de novo, volta pelo pull com um `id` novo.

Se o work item voltar a ter atividade depois da poda (issue reaberta, sessão nova), o ambiente cria uma linha nova pela chave natural (`owner/repo#n`) e o hub funde com o histórico que já tinha. Os eventos do GitHub têm `uid` determinístico (`gh:issue:<owner>/<repo>#<n>:closed`...); a poda guarda em `pruned_uid` o maior sufixo já usado de cada um, e a mesma transição depois da poda sai com o sufixo seguinte (`:2`, `:3`...), para o hub não descartá-la como duplicada.

`env.info.hub_sync.prune` traz `retention_days`, `pruned_work_items` (total já podado neste ambiente, guardado em `ledger_meta`), `last_run_at`, `last_pruned` e `last_error`. O resultado de `hub.sync.now` traz `pruned`.

```toml
# ~/.config/ctower/config.toml
retention_days = 30    # 0 desliga a poda
```

### Backup do hub

- **Snapshots:** `hub-<AAAAMMDD-HHMM>.db` (UTC) em `backup_dir`, com `VACUUM INTO`, mantendo os `backup_keep` mais novos. Ponha `backup_dir` em outro disco ou num disco sincronizado: o `cthubd` avisa no log e na UI quando ele está no mesmo disco do `hub.db`. Os snapshots cifrados de secrets vão, por padrão, para o mesmo `backup_dir` (veja [Snapshots de secrets](#snapshots-de-secrets)).
#### Backup remoto (disco local, Google Drive, Backblaze B2 e S3)

A seção **Backup do hub** da tela **Hub** configura o destino, a agenda (`backup_interval_hours`), a retenção (`backup_keep`) e as credenciais, e tem **Testar conexão** (grava, relê e apaga um arquivo de prova) e **Fazer backup agora**. A configuração vai para `~/.config/cthub/backup.json` (sem credenciais), vale por cima do `config.toml`, que segue como fallback, e sobrevive a `cthubd update`. Os métodos do `/ws` são `hub.backup.get|set|test|run` e `hub.backup.remote.list`.

| Destino | O que informar |
|---|---|
| Disco local | só a pasta (`backup_dir`), como antes |
| Amazon S3 e compatíveis (MinIO, Wasabi, R2) | bucket, prefixo, região, endpoint (vazio = AWS), access key e secret key |
| Backblaze B2 | bucket, prefixo, endpoint S3 do B2 (`https://s3.<região>.backblazeb2.com`), key id e application key (chave restrita ao bucket) |
| Google Drive | ID de uma pasta dedicada, client ID, client secret e refresh token do OAuth com o escopo `drive.file` |

O Drive usa um refresh token que você gera uma vez (OAuth do tipo "desktop app", escopo `drive.file`, pasta criada para os backups). Service account não é suportado.

- **O que sobe:** depois de cada snapshot local, o `hub-<AAAAMMDD-HHMM>.db` e uma cópia do `secrets.age` (`secrets-<AAAAMMDD-HHMM>.age`, que já é cifrado). Os snapshots de secrets usam, portanto, o mesmo destino remoto; `secrets_snapshot_dir` continua valendo para o disco local.
- **Cifra:** ligada por padrão. O `.db` sobe como `hub-<...>.db.age`, cifrado com `age` para a chave do hub e a de recuperação (a de recuperação abre o backup sem a máquina do hub). Desligar é uma escolha explícita na UI e gera o aviso `backup_remote_unencrypted`, porque o `hub.db` tem o histórico dos projetos em texto claro.
- **Credenciais:** ficam no `secrets.age` como entradas `CTHUB_BACKUP_*` (prefixo reservado, só do hub). Nunca vão para um ambiente pelo sync, não aparecem em `hub.secrets.list`, não podem ser criadas por `hub.secrets.set`, nem entram em `config.toml`, `backup.json`, log ou resposta do `/ws`: depois de salvas a UI só vê "configurado". Credencial em branco no formulário mantém a guardada.
- **Falhas:** o snapshot local sai antes e não depende da rede. Falha no envio é registrada, vira aviso e é repetida só no envio, com espera crescente (5 min, 10, 20... até o intervalo), sem atrasar o próximo snapshot nem o hub.
- **Retenção:** os `backup_keep` objetos mais novos de cada tipo (`hub-*` e `secrets-*`) ficam no destino; os outros são apagados.
- **Avisos de `hub.status`:** `backup_remote_failed` (envio falhando), `backup_remote_credentials` (credencial inválida ou revogada, ex.: token do Drive), `backup_remote_stale` (nenhum envio em 2 × `backup_interval_hours`) e `backup_remote_unencrypted`. O aviso `backup_same_device` some enquanto há envio remoto em dia.

**Restaurar do remoto**, com o serviço parado (a máquina nova precisa do `backup.json` e das credenciais; sem o `secrets.age`, exporte `CTHUB_BACKUP_S3_ACCESS_KEY`/`CTHUB_BACKUP_S3_SECRET_KEY` (ou as do B2 e do Drive) no ambiente):

```sh
cthubd restore --remote                    # lista os snapshots do destino
cthubd restore --remote latest             # baixa, decifra e restaura o mais novo
cthubd restore --remote hub-20261023-1300.db.age --identity recuperacao.key
```

Ele baixa para `~/.local/share/cthub/restore-remote/`, decifra com a chave do hub ou com `--identity` (a chave de recuperação) e segue como `cthubd restore <snapshot>`. A cópia remota do store de secrets (`secrets-*.age`) baixa-se com `cthubd restore --remote` e o nome dela, e abre com `cthubd secrets restore`. A restauração fica só na linha de comando, porque exige o serviço parado.

- **`ctower hub resync`** (no ambiente) põe de novo no outbox tudo o que o ambiente ainda tem, que é **só o que não foi podado** (work items ativos e os recentes), e avisa quantos work items já foram podados ali. Serve para completar um snapshot, não para substituí-lo.

### Backup dos docs

O `docs_dir` é a **única cópia** dos docs (decisão 269) e as exclusões se propagam (262): o backup é a única proteção contra uma exclusão errada. Só o cliente nativo do hub fala com o destino (decisão 261), sem restic, rclone ou outra ferramenta.

- **Pacote:** a cada `docs_backup_interval_hours` (padrão 24; `0` desliga), `docs-<AAAAMMDD-HHMM>.tar.zst` (UTC) com o `docs_dir` **inteiro**, em `backup_dir` (0600). Sem incremental: cada pacote é completo. Links simbólicos entram como links (não são seguidos); o `backup_dir`, se estiver dentro do `docs_dir`, fica de fora. No boot, só gera se o mais novo passou do intervalo.
- **Remoto:** sobe para o mesmo destino do `hub.db`, cifrado com `age` para a chave do hub e a de recuperação, como `docs-<...>.tar.zst.age`. Com a cifra desligada sobe como `.tar.zst`, sob o mesmo aviso `backup_remote_unencrypted`. Falha no envio é repetida só no envio, com a mesma espera crescente do `hub.db`.
- **Retenção:** os `docs_backup_keep` (padrão **7**) pacotes mais novos do prefixo `docs-`, no `backup_dir` e no destino, separados do `backup_keep` do `hub.db`.
- **Limite:** o pacote vai inteiro para a memória no envio; com os docs acima de `docs_backup_max_mb` (padrão 1024, até 4096), o backup **não roda** e avisa.
- **Pasta vazia ou ausente:** não roda e não avisa (sem `docs_dir`, o backup dos docs nem sobe).
- **Avisos de `hub.status`:** `docs_backup_failed` (pacote ou envio falhando), `docs_backup_stale` (nenhum pacote em 2 × `docs_backup_interval_hours`) e `docs_backup_too_large`.
- **Na UI (#444):** Manutenção › Backup e snapshots tem o bloco "Docs dos projetos": pasta, último pacote (data, nome, tamanho), último envio remoto, agenda, retenção e limite (editáveis), os avisos `docs_backup_*` e "Fazer backup agora" (`hub.backup.run { what: "docs" }`; o botão do topo passa a mandar `what: "hub"`). Na PWA, o mesmo estado, só leitura, na Visão geral do Hub.
- **`/ws`:** `hub.backup.get|set|run` trazem o estado em `docs` (separado do `status` do `hub.db`); `hub.backup.set` aceita `docs_interval_hours`, `docs_keep` e `docs_max_mb` (gravados no `backup.json`, por cima do `config.toml`); `hub.backup.run { what }` roda `all` (padrão), `hub` ou `docs`.

**Restaurar** nunca escreve no `docs_dir`: o pacote é extraído numa pasta **nova ou vazia** fora dele, e você copia de lá o que precisar. Não toca no `hub.db`, então roda com o serviço no ar. Entradas com `..` ou caminho absoluto recusam o pacote.

```sh
cthubd restore --remote                                         # lista snapshots e pacotes dos docs do destino
cthubd restore --remote latest --to ~/docs-restaurados           # o pacote dos docs mais novo
cthubd restore --remote docs-20261029-0300.tar.zst.age --to /tmp/docs-0300 --identity recuperacao.key
cthubd restore ~/.local/share/cthub/backups/docs-20261029-0300.tar.zst --to /tmp/docs-0300
```

### Roteiro: recuperar um arquivo dos docs

1. Ache o pacote de antes da exclusão: `ls <backup_dir>/docs-*` ou `cthubd restore --remote` (o carimbo é UTC).
2. Extraia numa pasta à parte: `cthubd restore <backup_dir>/docs-<...>.tar.zst --to /tmp/docs-recuperados` (ou `--remote docs-<...>.tar.zst.age`; numa máquina sem a chave do hub, com `--identity` e a chave de recuperação).
3. Copie de volta só o arquivo: `cp -a /tmp/docs-recuperados/<owner>__<repo>/<caminho> <docs_dir>/<owner>__<repo>/<caminho>`.
4. Apague a pasta temporária (`rm -rf /tmp/docs-recuperados`).

### Roteiro: hub perdido ou trocado

1. Na máquina nova, instale o `cthubd` (seção 1).
2. **Com snapshot:** com o serviço parado, `cthubd restore <snapshot>` (ou `cthubd restore --remote latest`, se o backup estava no Drive, B2 ou S3) e suba o `cthubd`. O comando recusa se houver algo escutando na porta do hub, valida o snapshot numa cópia (o arquivo dele não muda) e guarda o `hub.db` atual como `hub.db.pre-restore-<AAAAMMDD-HHMM>`. Os tokens de sync estão no snapshot; se o endereço mudou, ajuste `hub_url` nos ambientes.
   **Secrets:** siga o [roteiro de perda da máquina do hub](#roteiro-perda-da-máquina-do-hub-secrets).
3. **Sem snapshot:** cadastre de novo os projetos no hub e refaça as atribuições (seção 2). O hub nunca cria projeto a partir do que os ambientes enviam.
4. **Para completar o que veio depois do snapshot (ou sem snapshot):** se for o caso, gere tokens novos (`cthubd add-env`) e grave-os nos ambientes. Em cada ambiente, rode `ctower hub resync` e depois `ctower hub sync`. O que chegar de um projeto ainda não cadastrado fica em **quarentena** no `hub.db`, com aviso no log e na UI do hub, e é reaplicado quando você cadastrar o projeto. Sem snapshot, o histórico já podado nos ambientes se perde.
5. Confira na UI (ou `hub.envs.list`) os ambientes com 0 pendências, as atribuições aplicadas e a quarentena vazia.

### Roteiro: perda da máquina do hub (secrets)

1. Na máquina nova, instale o `cthubd` (seção 1) e restaure o `hub.db` (roteiro acima), com o serviço parado.
2. Pegue o snapshot de secrets mais recente (`secrets-<AAAAMMDDTHHMMSSZ>.age`, o maior carimbo) na pasta sincronizada.
3. Com o serviço parado: `cthubd secrets restore <snapshot>`. Sem a `secrets.key` antiga, o comando pede a chave de recuperação: cole a do gerenciador de senhas (não aparece na tela). Confira a lista de nomes e confirme com `s`.
4. Suba o serviço. Na tela **Hub**, em **Chave de recuperação**, gere e verifique uma chave **nova** e guarde-a no gerenciador de senhas, no lugar da antiga. A antiga foi exposta na restauração e deixou de valer.
5. Na seção **Secrets e variáveis**, a lista **Secrets a rotacionar na origem** mostra os secrets que estavam no store. Troque cada valor em quem o emitiu (tokens de API: GitHub, provedores de nuvem...), grave o valor novo no hub e clique **Já rotacionei**. O valor antigo pode ter vazado junto com a máquina.
6. Os ambientes recebem os secrets no próximo pull (o restore incrementou o `config_rev`). `ctower hub sync` em cada um adianta. Confira a **Entrega aos ambientes**.
7. Opcional: os snapshots antigos na pasta continuam cifrados para a chave do hub perdida e para a recuperação exposta. Com a recuperação nova verificada, `cthubd secrets rotate-keys --hub` (serviço parado) apaga os que a chave atual não abre. Limpe também o histórico de versões e a lixeira da pasta sincronizada.

### Roteiro: perda de uma máquina de ambiente

O cache do ambiente (`secrets-cache.age`) é cifrado para a `env.key` dele, que estava na mesma máquina. Quem tiver a máquina tem os secrets que o hub entregou a ela.

1. No hub, **antes** de revogar, liste o que estava no cache: `cthubd secrets list --env <nome>` (as globais e as dos projetos atribuídos ao ambiente; só nomes). Sem o token, um ambiente que nunca enviou nada some da lista do hub.
2. `cthubd revoke-env <nome>`: corta a entrega (o pull passa a receber `401`) e apaga a chave pública do ambiente no hub.
3. Troque na origem os valores dos `secret` listados e grave os novos no hub. Os outros ambientes recebem os valores novos no próximo pull.
4. Na tela **Hub**, remova as atribuições do ambiente perdido.

## 5. Migrar da instalação atual (`ctowerd` com `hub = true`)

A instalação do #64 tem um `ctowerd` com `hub = true` (o hub antigo, na porta 7878 por padrão), o `hub.db` dele em `~/.local/share/ctower/`, os tokens de sync e os ambientes ligados a ele, cada um com os projetos que criou. A migração troca esse hub pelo `cthubd` (porta 7879) sem perder histórico nem tokens. O `ctowerd` da máquina do hub vira um ambiente comum, ligado ao `cthubd` local. Desenho na spec, §11.

### O que o `cthubd migrate` faz

`cthubd migrate --from <data_dir do ctowerd>` (o config do `ctowerd` vem de `CTOWER_CONFIG_DIR` ou `~/.config/ctower`; `--ctower-config <dir>` sobrepõe):

- **Recusa** com o `ctowerd` no ar (a porta do config dele, padrão 7878) ou com algo na porta do `cthubd`. Também recusa se o `cthubd` já tem um `hub.db`: com `--force`, o atual fica ao lado como `hub.db.pre-migrate-<AAAAMMDD-HHMM>`.
- **Copia o `hub.db`** com `VACUUM INTO` para `~/.local/share/cthub/hub.db`. O original não muda (abre só para leitura) e fica como backup. O `cthubd` aplica as migrações novas do esquema na cópia.
- **Mantém** `env`, `env_token` (hashes e `env_id`) e todo o histórico (work items, sessões, decisões, eventos, espelho do GitHub). Os tokens de sync dos outros ambientes continuam valendo, presos ao mesmo `env_id`, e o `seq` de cada um continua de onde o hub antigo parou.
- **Promove os projetos a origem** (cada um ganha um `rev`) e cria uma **atribuição** por `project_location` não removida, com o `repo_path` que o ambiente relatou. Inicializa o `config_rev` (e a `config_epoch`). As atribuições começam **pendentes** e viram **aplicadas** quando cada ambiente puxa a configuração e relata.
- **Cria o `config.toml` do `cthubd`** (se ainda não existe) a partir do config do `ctowerd`: `bind`, `hub_backup_dir` → `backup_dir`, `hub_backup_interval_hours` → `backup_interval_hours`, `hub_backup_keep` → `backup_keep`, e `gh_*`, `update_repo`, `update_ref`, se estiverem lá. `hub_backup_dir` vazio (o padrão antigo, `~/.local/share/ctower/hub-backups`) fica vazio: os snapshots novos vão para `~/.local/share/cthub/backups`, e os antigos ficam onde estão. Gera o token de UI do hub (`~/.config/cthub/ui.token`) e copia o `github.token` do `ctowerd`, se houver e o hub ainda não tiver um.
- **Liga o `ctowerd` local ao `cthubd`:** gera o token de sync dele, preso ao `env_id` que ele já usava (`~/.config/ctower/env_id`). O hub antigo gravava o ledger desse `ctowerd` em processo, com esse `env_id` e sem token, então o histórico dele continua sendo do mesmo ambiente, sem duplicar. O nome é o `env_name` do config, `--env-name` ou o nome da máquina. Grava o `hub.token` (`0600`), guarda o config como `config.toml.pre-cthub` e reescreve o config: sai `hub = true`, saem `hub_backup_*` e um `hub_url` antigo, e entra `hub_url = "http://127.0.0.1:7879"` no topo (e `env_name`, se faltava). Os comentários e as outras chaves ficam.

Tudo o que pode falhar roda antes de tocar nos arquivos finais: o `hub.db` novo é montado ao lado (`hub.db.migrate`) e só troca de nome no fim. Rodar de novo é seguro:

- já migrado, com o `ctowerd` convertido: `nada a fazer`;
- interrompido depois da troca do `hub.db` (o `ctowerd` ainda em `hub = true`): retoma, refazendo só a parte do `ctowerd` (com outro token). Antes, confere se o `hub.db` antigo está igual ao copiado (a migração guarda em `hub_meta` os envios de cada ambiente e as contagens e maiores ids de eventos, decisões, sessões, projetos e tokens). Se ele mudou (você voltou atrás e o hub antigo recebeu algo), recusa: retomar perderia o que ele recebeu;
- `--force`: refaz tudo a partir do `hub.db` antigo (para uma nova tentativa depois de voltar atrás). O `hub.db` do `cthubd` fica ao lado como `hub.db.pre-migrate-<data>`.

Se o config do `ctowerd` não tem `hub = true`, o comando migra o `hub.db` mas não mexe no `ctowerd`, com aviso.

### Roteiro

Ordem obrigatória: os outros ambientes primeiro, e na máquina do hub o `cthubd migrate` **antes** de atualizar o `ctowerd` (o novo recusa subir com `hub = true`, e o atualizador faria rollback). Use a mesma versão (a que traz o `cthubd`) em todas as máquinas.

**Janela segura:** do passo 3 até a conferência do passo 8, não use sessões na máquina do hub. Se for preciso voltar atrás depois do passo 6, o `ctower update --rollback` volta o `ledger.db` dessa máquina ao snapshot de antes da atualização, e o que as sessões gravarem nesse meio-tempo não chega ao hub antigo (veja "Voltar atrás"). Nos outros ambientes, as sessões podem seguir.

**Script:** na máquina do hub, os passos 2 a 6 rodam com `scripts/migrate-to-cthubd.sh` (depois do passo 1 nos outros ambientes):

```bash
~/projetos/control-tower/scripts/migrate-to-cthubd.sh --dry-run   # só mostra os comandos
~/projetos/control-tower/scripts/migrate-to-cthubd.sh
```

Ele roda os mesmos comandos e conferências descritos abaixo, na mesma ordem, e para no primeiro erro dizendo o passo (o caminho de volta é o "Voltar atrás"). Antes de parar o `ctowerd`, confere com `ctower update --check` que o `update_ref` traz versão nova (senão o passo 6 não subiria o `ctowerd`; aponte-o para um ref com o `cthubd`) e recusa com sessões vivas, salvo `--yes` (que também vai como `--force` ao `ctower update`). Repassa `--env-name` e `--force` ao `cthubd migrate`; `--repo` troca o clone (padrão `~/projetos/control-tower`). Rodar de novo é seguro: com o `ctowerd` já ligado ao `cthubd` local, pula os passos 3 e 4. No fim, mostra os passos 7 e 8, que continuam à mão, assim como o "Voltar atrás". Só Linux. Os passos à mão:

1. **Nos outros ambientes**, atualize o `ctowerd`:

   ```bash
   ctower update
   ctower hub sync          # ainda envia ao hub antigo (porta 7878)
   ```

   O ambiente novo continua enviando ao hub antigo. `/sync/config` dá `404` no hub antigo, o que ele trata como "hub antigo": os projetos locais continuam valendo, e `env.info.hub_config.legacy_hub` fica `true`.

2. **Na máquina do hub**, instale o `cthubd` sem subi-lo, do clone atualizado:

   ```bash
   git -C ~/projetos/control-tower pull
   bash ~/projetos/control-tower/scripts/provision/provision.sh --hub --skip-packages
   ```

   Com `hub = true` no config, o script instala o binário e a unidade, mas não sobe o `cthubd` nem mexe no `ctowerd`.

3. **Pare o `ctowerd`** e confira que parou. Parar o daemon encerra as sessões vivas dele, que voltam como interrompidas e podem ser retomadas depois; prefira um momento sem sessões (`ctower status`).

   ```bash
   systemctl --user stop ctowerd
   curl -s http://127.0.0.1:7878/healthz || echo parado
   ```

4. **Migre:**

   ```bash
   cthubd migrate --from ~/.local/share/ctower
   ```

   A saída mostra os projetos e atribuições criados, os tokens de sync preservados, o nome e o `env_id` do `ctowerd` local, os arquivos gravados e os próximos passos. Confira:

   ```bash
   grep hub_url ~/.config/ctower/config.toml             # hub_url = "http://127.0.0.1:7879"
   ls ~/.config/ctower/config.toml.pre-cthub ~/.local/share/cthub/hub.db
   cat ~/.config/cthub/config.toml                       # bind e backup_dir
   ```

5. **Suba o `cthubd`** e confira que o token do `ctowerd` local vale nele:

   ```bash
   systemctl --user enable --now cthubd
   curl -s http://127.0.0.1:7879/healthz                 # "role":"hub"
   curl -s -H "Authorization: Bearer $(cat ~/.config/ctower/hub.token)" \
        -H "X-Ctower-Env-Id: $(cat ~/.config/ctower/env_id)" \
        http://127.0.0.1:7879/sync/config                 # os projetos deste ambiente
   ```

6. **Atualize e suba o `ctowerd` desta máquina** (com `update_ref` numa versão que traz o `cthubd`):

   ```bash
   ctower update
   ctower hub sync          # sai com 0: puxou a configuração e enviou o outbox
   ```

   Use o `ctower update`, e não o `provision.sh`: ele guarda o binário anterior em `update/prev/` e o snapshot do `ledger.db`, que o `ctower update --rollback` usa para voltar atrás.

7. **Nos outros ambientes**, troque a porta do hub antigo em `hub_url` (`7878` por padrão; a `port` que o `ctowerd` do hub antigo usava, se era outra) por `7879` e reinicie. O `migrate-to-cthubd.sh` mostra no fim este `sed` já com as duas portas certas:

   ```bash
   OLD=7878   # a porta do hub antigo (a port do ~/.config/ctower/config.toml.pre-cthub na máquina do hub)
   sed -i -E "s#^(hub_url[[:space:]]*=[[:space:]]*[\"']https?://[^:/\"']+):$OLD(/?[\"'])#\1:7879\2#" ~/.config/ctower/config.toml
   grep '^hub_url' ~/.config/ctower/config.toml   # esperado: hub_url = "http://<máquina do hub>:7879"
   ctower restart
   ctower hub sync
   ```

8. **Ligue a UI ao hub** (Configurações → Hub, URL `http://<máquina do hub>:7879` e o conteúdo de `~/.config/cthub/ui.token`) e confira na tela **Hub**: os ambientes com 0 pendências, as atribuições **aplicadas** e a quarentena. Em cada ambiente, os projetos que o hub não atribuiu aparecem como "projeto local fora do hub".

### Primeiro pull em cada ambiente

No primeiro pull, o ambiente troca os projetos locais pelo cache vindo do hub:

- os projetos locais legados (`config_rev` nulo) que vieram em `projects` são casados por `owner/repo` e mantêm o `id` local, então o histórico local continua ligado a eles;
- os legados que o hub não atribuiu a este ambiente ganham `unassigned_at` e aparecem na UI como "projeto local fora do hub". Nada é apagado do disco (clone e worktrees ficam), e eles entram na poda (seção 4) só depois do ack do hub;
- o que estava no outbox de um projeto que o hub não conhece (ex.: criado num ambiente e nunca enviado) vai para a quarentena do hub. Cadastre o projeto no hub (e atribua) para reaplicar, ou descarte (`hub.quarantine.discard`).

### Voltar atrás

O `hub.db` antigo em `~/.local/share/ctower/` não muda em nenhum passo (a cópia o abre só para leitura; pode aparecer um `hub.db-shm` ao lado, que o SQLite recria e não altera os dados). Para voltar ao hub antigo:

1. Na máquina do hub, pare o `cthubd` e restaure o config do `ctowerd`:

   ```bash
   systemctl --user disable --now cthubd
   cp ~/.config/ctower/config.toml.pre-cthub ~/.config/ctower/config.toml
   ```

2. Se o `ctowerd` desta máquina **ainda não** foi atualizado (passo 6), suba-o: `systemctl --user start ctowerd`. Se já foi, volte o binário (e o `ledger.db`, se o esquema migrou) com `ctower update --rollback`, que reinicia o daemon antigo. Esse rollback devolve o `ledger.db` ao snapshot de antes do passo 6: as sessões e decisões gravadas nesta máquina depois dele saem do `ledger.db` em uso. Elas ficam na cópia `~/.local/share/ctower/ledger.db.pre-rollback-<AAAAMMDD-HHMM>` e, se já foram enviadas, no `hub.db` do `cthubd`, mas não chegam ao hub antigo. Recuperá-las é trabalho à mão (consultar essas cópias e registrar de novo o que importar); por isso a janela segura acima.
3. Nos ambientes que já apontam para `7879`, volte `hub_url` para a porta do hub antigo (`7878` por padrão) e rode `ctower restart`. O que eles enviaram ao `cthubd` depois da migração não está no `hub.db` antigo: rode `ctower hub resync` e `ctower hub sync` em cada um para reenviar. O rollback não desfaz o primeiro pull deles: os projetos que o `cthubd` não atribuiu continuam com `unassigned_at` e aparecem como "fora do hub" na UI do ambiente. Nada se perde (clones, worktrees e histórico ficam); só a lista de projetos do ambiente muda.
4. O que mudou no `cthubd` (`~/.local/share/cthub/hub.db`) fica lá. Para uma nova tentativa depois de usar o hub antigo, rode `cthubd migrate --from ~/.local/share/ctower --force` (o `hub.db` do `cthubd` fica ao lado como `hub.db.pre-migrate-<data>`). Sem `--force`, o comando recusa, porque o `hub.db` antigo mudou depois da primeira cópia.

## 6. Gestor de projetos

O gestor é uma sessão do Claude Code que lê o hub inteiro e propõe mudanças de prioridade e de status (spec `docs/superpowers/specs/2026-10-19-gestor-de-projetos.md`). Ele roda num ambiente escolhido, que precisa estar ligado ao hub (`hub_url` e `hub.token`) e ter o token de agente (`hub-agent.token`, seção 3, passo 4). `env.info.hub_config.agent_token` diz se o arquivo existe.

- **Sessão:** `session.start_gestor { prompt, scope, model?, mode?, cols?, rows? }` no `/ws` do ambiente. `scope` pode ser `{ kind: "portfolio" }`, `{ kind: "project", owner, repo }` ou `{ kind: "health" }` e só orienta o prompt. Sem hub ou sem token de agente, o início é recusado com a mensagem de como resolver.
- **Pasta:** `gestor_dir` no `config.toml` do ambiente (padrão `~/.local/share/ctower/gestor`, criada com 0700). Todas as sessões do gestor usam a mesma pasta, para a retomada funcionar. A pasta não é repositório: o gestor usa `gh ... -R owner/repo`.
- **Modelagem:** as sessões são do projeto sentinela `_ctower/gestor` (work item `#0`), que existe nos dois bancos e fica fora das listas, do sync do GitHub, do ranking e da saúde. As decisões do gestor ficam nesse work item.
- **Secrets:** a sessão do gestor recebe só os secrets e variáveis **globais** do hub, nunca os de um projeto (ela não está presa a um repositório). O mascaramento na saída e no transcript segue a mesma regra das outras sessões.
- **Ferramentas:** o MCP da sessão traz `portfolio_overview`, `portfolio_project`, `portfolio_work_item`, `portfolio_ranking`, `portfolio_health`, `portfolio_list_decisions`, `portfolio_list_proposals` e `portfolio_propose`. O `ctowerd` repassa cada uma a `/agent/v1/*`, e o agente nunca vê o token. `ledger_set_status` não existe no gestor, e `ledger_list_decisions` só tem o escopo `work_item`.
- **Limites:** vão fixos no system prompt (`--append-system-prompt`, inclusive na retomada). O hook aplica o perfil `gestor` (`CTOWER_SESSION_KIND=gestor`):
  - **bloqueia** `git commit` e `git push`, a escrita em PRs, issues existentes, labels, milestones e repositórios, a `gh api` com método diferente de `GET`, e a leitura de arquivos de token e de secrets (`env.key`, `secrets-cache.age`, `secrets.age`, `secrets.key` e os snapshots `secrets-<data>.age`);
  - **pede confirmação** no terminal (`permissionDecision: "ask"`) para `gh issue create` e para ligar sub-issues;
  - nos projetos Jira do portfólio, lê pelo jira-cli, pede confirmação para `jira issue create`/`jira epic create` e bloqueia transição, edição e comentário (`jira issue move|edit|comment`...).
- **Projetos Jira:** `portfolio_overview` e `portfolio_project` trazem `tracker` e `issue_view`, o comando para ler uma issue (`gh issue view <n> -R owner/repo`, ou o jira-cli com o comando de credencial do cadastro), e os itens vêm com `ref` (`owner/repo#n` ou `PROJ-n`).

## 7. Segurança

| Token | Dá acesso a |
|---|---|
| UI do hub (`~/.config/cthub/ui.token`) | `/ws` do hub |
| Sync, um por ambiente (`cthubd add-env`) | `/sync/push`, `/sync/config`, `/sync/done`, `/sync/query/decisions` e `/sync/channel`, só dos projetos atribuídos ao seu `env_id` |
| Agente, um por ambiente (`cthubd agent-token`) | `/agent/v1/*` (leitura do hub inteiro), preso ao `env_id` do token de sync do mesmo ambiente |
| UI do ambiente (`~/.config/ctower/ui.token`) | `/ws`, `/hub/sync` e `/secrets/rotate-env-key` do ambiente |
| Hook/MCP (`~/.config/ctower/hook.token`) | `/hooks` e `/mcp` do ambiente |
| Chave VAPID (`~/.config/cthub/vapid.key`) | assinar a Web Push do hub (não abre rota nenhuma) |

"O gestor não aprova as próprias propostas" é uma garantia do protocolo (nenhuma ferramenta nem rota de agente decide), não do sistema operacional. A sessão roda com o mesmo usuário do `ctowerd` e consegue ler os arquivos de token. Na máquina única, isso inclui `~/.config/cthub/ui.token`, que abre o `/ws` do hub. O hook bloqueia essa leitura, mas é uma barreira contra erro. Para isolamento de verdade, rode o `cthubd` com outro usuário do sistema.

O transporte entre máquinas é o Tailscale (WireGuard), sem TLS próprio nesta fase. Escutar fora de `127.0.0.1` exige `bind` explícito. Na LAN (ambientes em outras máquinas, #252), o sync e o `/ws` vão em HTTP e os tokens passam em claro: só numa rede doméstica confiável. A PWA no celular vai por um proxy HTTPS na frente do loopback, com o `cthubd` sem `bind`: [`docs/acesso-remoto.md`](acesso-remoto.md) (endurecimento, privacidade e a relação com o #252). Secrets ficam para a #171.
