# PWA e acesso pelo celular, sessões pelo Remote Control (issue #196)

Spec do épico #205. Parte das specs do hub como plano de controle (`2026-10-18-hub-plano-de-controle.md`) e do gestor de projetos (`2026-10-19-gestor-de-projetos.md`), que continuam valendo: o `cthubd` guarda a visão consolidada em `hub.db` e o `ctowerd` executa sessões, PTY, hooks e MCP. Esta spec acrescenta uma PWA servida pelo hub, um login web, a ligação das sessões ao Remote Control do Claude Code, a Web Push e, na fase 2, um canal reverso ambiente → hub.

As decisões confirmadas pelo usuário estão no épico #205 (inclusive a revisão de 2026-10-28) e não são reabertas aqui. As escolhas que esta spec acrescenta estão na §13 e precisam da revisão do usuário antes das sub-issues seguintes.

## 1. Problema e direção

Hoje só o app desktop (Tauri) fala com o hub e com os ambientes. O `/ws` do hub (`crates/cthubd/src/ws.rs`) e o do ambiente (`crates/ct-server/src/ws.rs`) exigem `Authorization: Bearer <token>` (`ct_http::bearer_ok`), que o navegador não manda num upgrade de WebSocket. O lado Rust do Tauri (`ui/src-tauri/src/{envs,hub,bridge,notify}.rs`) guarda URLs e tokens, mantém as conexões, repassa os pushes e dispara as notificações nativas. A UI só vê a interface `DaemonApi` (`ui/src/api/daemon.ts`), com `TauriDaemonApi` e `FakeDaemonApi`.

Direção:

```
 celular (PWA)                              máquina do hub
 ─────────────                              ──────────────
  navegador ──HTTPS── tailscale serve ─┐
            ──HTTPS── Cloudflare Access ── cloudflared ─┤
                                                         ▼  127.0.0.1:7879
                                                       cthubd
                                         GET /, /assets/*  (bundle embutido, rust-embed)
                                         POST /web/login|logout   (token de UI → cookie)
                                         GET  /web/ws  (cookie; lista de permissões, §4)
                                         /ws  (Bearer, app desktop, sem mudança)
                                               │ fase 2: hub.env.call
                                               ▼
                                         /sync/channel (WebSocket aberto pelo ctowerd, §5)
                                               ▲
                                            ctowerd ── PTY ── claude --remote-control "<nome>"
                                                                   │ HTTPS de saída
  app Claude / claude.ai/code  ◀──── Anthropic (Remote Control) ◀───┘
```

- A PWA **acompanha e decide**. Quem conversa com a sessão é o Remote Control, no app Claude ou em `claude.ai/code/<id>`.
- O hub continua na máquina do usuário. Ele não ganha porta nova: o proxy (Tailscale ou Cloudflare) fala com o `127.0.0.1` dele.
- O app desktop não muda de caminho: continua no `/ws` com o token de UI e mantém o acesso aos métodos sensíveis.

## 2. Papéis da PWA

Em nenhuma fase a PWA tem terminal: nada de `session.attach|input|resize|peek` nem do canal binário do `/ws` do ambiente.

### 2.1 Fase 1: só o hub

Tudo vem do `hub.db`, que já tem a visão consolidada pelo sync. O `hub.dashboard.get` (`HubDashboard`) traz `live_sessions`, `interrupted_sessions`, `failed_sessions` (código ≠ 0 nas últimas 24 h), `envs` e os work items com status; o status `aguardando_voce` vem do gatilho `NeedsHuman` dos hooks `Notification` e `PermissionRequest` (`crates/ct-sessions/src/hooks.rs`), e chega ao hub pelo outbox a cada `PUSH_EVERY` (5 s, `crates/ct-server/src/hub_sync.rs`).

- **Acompanhar:** Sala de controle (`hub.dashboard.get`), árvore e item (`hub.tree.get`, `hub.workitem.history`), saúde (`hub.health.get`), ranking (`hub.ranking.get`), ambientes (`hub.envs.list`), avisos (`hub.status`) e decisões, que já vêm em `hub.workitem.history`.
- **Agir no hub:** mudar status e prioridade (`hub.workitem.set_status`, `hub.workitem.set_priority`) e aprovar ou rejeitar propostas do gestor (`hub.proposals.decide`).
- **Abrir no Remote Control:** link `https://claude.ai/code/<rc_session_id>` quando a sessão tem o id (§6.4).
- Recuperação só para leitura: as `interrupted_sessions` aparecem, sem botão de retomar.

### 2.2 Fase 2: com o canal reverso

As ações que só o ambiente executa passam a chegar a ele pelo canal reverso (§5):

- iniciar sessão de issue, do projeto (planejamento) e do gestor; retomar e encerrar;
- ligar o Remote Control numa sessão em andamento (`session.remote_control.enable`) e abrir em seguida;
- marcar decisões como revisadas (`review.mark_reviewed`);
- retomar pela Recuperação.

O estado online/offline de cada ambiente passa a ser o do canal, em tempo real, e não mais a idade do último sync.

## 3. Autenticação web (#200)

### 3.1 Login com o token de UI do hub

- A PWA abre numa tela de login com um campo para o token de UI do hub (`~/.config/cthub/ui.token`). `POST /web/login { token }` compara em tempo constante, como o `bearer_ok`, e responde com `Set-Cookie`.
- O cookie é `ct_web=<id>.<segredo>`, com `HttpOnly`, `Secure`, `SameSite=Strict`, `Path=/` e `Max-Age` igual à validade absoluta. O hub guarda só o hash do segredo (SHA-256), como faz com os tokens de sync e de agente.
- **Validade:** 30 dias absolutos e 7 dias sem uso (a cada uso o hub atualiza `last_used_at`, no máximo uma vez por minuto). Os dois valores ficam em `[web]` no `config.toml` (§3.4).
- **Revogação:** `POST /web/logout` revoga a sessão atual e apaga o cookie. `hub.web.sessions.list` e `hub.web.sessions.revoke { id }` ou `{ all: true }` ficam no `/ws` e no `/web/ws` (revogar só reduz acesso). Na CLI, `cthubd web revoke-all` revoga tudo sem o hub precisar estar de pé (grava direto no `hub.db`). Trocar o token de UI não derruba as sessões web: é preciso revogar.
- **Limite de tentativas:** contador global de falhas (atrás de proxy, o IP visto é sempre `127.0.0.1`). A partir da 5ª falha seguida o login fica bloqueado por 1 min, e o bloqueio dobra a cada nova falha até 1 h; um acerto zera o contador. Cada falha vai ao log com a identidade do proxy, se houver. O bloqueio vale só para `/web/login`: não atinge o `/ws` nem o sync.
- **O token nunca vai para o JavaScript.** Ele é digitado uma vez no formulário, sai no corpo do `POST` e não volta em resposta nem em erro. A PWA não o guarda (nem em `localStorage`, nem no service worker).
- O `GET /web/me` responde `{ authenticated, identity, expires_at }`, para a PWA decidir entre a tela de login e o shell.

### 3.2 Confiança no proxy

A identidade do proxy é uma **porta a mais**, não um substituto do login: com o proxy configurado, as rotas web exigem a identidade válida **e** o cookie. O token de UI continua como segunda camada (#204).

- **Cloudflare Access:** com `[web.cloudflare]` configurado, toda requisição às rotas web precisa do header `Cf-Access-Jwt-Assertion` com um JWT RS256 válido: assinatura pelas chaves de `https://<team_domain>/cdn-cgi/access/certs` (guardadas em cache por 1 h e recarregadas quando aparece um `kid` desconhecido), `iss = https://<team_domain>`, `aud` contendo o `aud` configurado, `exp` e `nbf` válidos, e `email` dentro de `emails`. Falha em qualquer item gera `403`, sem distinguir o motivo na resposta (o motivo vai ao log).
- **`tailscale serve`:** com `[web.tailscale]` configurado, o header `Tailscale-User-Login` só é levado em conta quando a conexão TCP vem do loopback **e** a requisição não tem nenhum header `Cf-*` (o `cloudflared` também fala do loopback). O login precisa estar em `logins`. Fora dessas condições, o header é ignorado (e não vira recusa, para o caso de só a Cloudflare estar configurada).
- **Requisição que veio pela Cloudflare só alcança as rotas web.** Com `Cf-Connecting-IP` presente, o hub responde `404` em `/ws`, `/sync/*` e `/agent/v1/*`: o app desktop e os ambientes usam o Tailscale ou a LAN (#252). O `docs/acesso-remoto.md` (#204) também restringe a rota do `cloudflared`, mas o hub não depende disso.
- Sem nenhum proxy configurado, vale só o cookie (uso local ou em LAN confiável).

### 3.3 Origin e CSRF

- O upgrade de `/web/ws` e todo `POST /web/*` exigem `Origin` igual a um item de `[web] origins`. Sem a lista, o padrão aceita `http://127.0.0.1:<port>` e `http://localhost:<port>`. Origin ausente é recusado nessas rotas.
- `SameSite=Strict` e a checagem de `Origin` cobrem CSRF. O `/web/ws` não aceita `Authorization`: o caminho com token é o `/ws`.
- Respostas web com `Content-Security-Policy: default-src 'self'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'`, `X-Content-Type-Options: nosniff` e `Referrer-Policy: no-referrer`.

### 3.4 Configuração

```
# ~/.config/cthub/config.toml
[web]
enabled = false            # rotas web desligadas por padrão
origins = ["https://hub.minha-tailnet.ts.net", "https://hub.exemplo.com"]
session_days = 30
idle_days = 7

[web.cloudflare]           # opcional
team_domain = "minha-equipe.cloudflareaccess.com"
aud = "<AUD da aplicação no Access>"
emails = ["eu@exemplo.com"]

[web.tailscale]            # opcional
logins = ["eu@github"]

[web.push]
env_stale_minutes = 15     # "ambiente sem sync há mais de N minutos"
```

Com `enabled = false`, `/`, `/assets/*` e `/web/*` respondem `404`. O `bind` não muda: atrás de proxy, o `cthubd` escuta só em `127.0.0.1` (o padrão de `ct_http::serve`).

## 4. Métodos do `/web/ws`

O `/web/ws` usa o mesmo JSON-RPC do `/ws` (`ct_http::rpc`: `{id, method, params}` → `{id, result}` ou `{id, error}`; push `{event, data}`). A diferença é uma **lista de permissões** aplicada antes do despacho: método fora dela responde `-32601` com a mensagem "método indisponível na web", mesmo com sessão válida. A lista é fixa no código (`const WEB_METHODS`), com teste que falha se aparecer um `hub.*` novo sem classificação aqui.

### 4.1 Métodos `hub.*` e `github.*` existentes

Todos os métodos atendidos hoje pelo `call` de `crates/cthubd/src/ws.rs` (protocolo do hub 1.2):

| Método | Na web | Motivo |
|---|---|---|
| `hub.info` | sim | leitura; versão do protocolo |
| `hub.status` | sim | avisos da faixa (só textos e contagens) |
| `hub.projects.list` | sim | leitura |
| `hub.projects.add` | não | cadastro de projeto fica no desktop |
| `hub.projects.update` | não | idem |
| `hub.projects.delete` | **não (sensível)** | apaga histórico |
| `hub.projects.impact` | não | só serve à exclusão |
| `hub.projects.move` | não | ordem entre projetos: sem uso na web (fase 1) |
| `hub.assignments.list` | sim | leitura (onde o projeto roda; usada na fase 2 para escolher o ambiente) |
| `hub.assignments.add` | não | muda onde o código roda |
| `hub.assignments.remove` | não | idem |
| `hub.quarantine.list` | sim | leitura |
| `hub.quarantine.discard` | não | destrutivo, sem volta |
| `hub.tree.get` | sim | leitura |
| `hub.workitem.history` | sim | leitura (inclui decisões) |
| `hub.workitem.set_status` | sim | ação da fase 1 |
| `hub.workitem.set_priority` | sim | ação da fase 1 |
| `hub.workitem.move` | não | reordenar por arrasto: sem uso na web (fase 1) |
| `hub.ranking.get` | sim | leitura |
| `hub.health.get` | sim | leitura |
| `hub.proposals.list` | sim | leitura |
| `hub.proposals.decide` | sim | ação da fase 1 |
| `hub.dashboard.get` | sim | leitura |
| `hub.envs.list` | sim | leitura |
| `hub.secrets.list` | não | devolve o valor das variáveis; a seção Hub › Secrets some na web |
| `hub.secrets.set` | **não (sensível)** | grava secret |
| `hub.secrets.delete` | **não (sensível)** | apaga secret |
| `hub.secrets.rotate_pending_dismiss` | não | parte do fluxo de restauração dos secrets |
| `hub.recovery.status` | não | seção Hub › Chave de recuperação some na web |
| `hub.recovery.generate` | **não (sensível)** | a chave privada sai na resposta |
| `hub.recovery.verify` | **não (sensível)** | recebe a chave privada |
| `hub.backup.get` | não | seção Manutenção › Backup some na web |
| `hub.backup.set` | não | recebe credenciais do destino remoto |
| `hub.backup.test` | não | recebe credenciais |
| `hub.backup.run` | não | Manutenção some na web |
| `hub.backup.remote.list` | não | lista o destino remoto (passo da restauração) |
| `github.status` | não | seção Hub › GitHub some na web |
| `github.set_token` | **não (sensível)** | grava o token do GitHub do hub |
| `github.clear_token` | **não (sensível)** | apaga o token |

"Restaurar backup" não é método do `/ws` hoje: é a CLI `cthubd restore` (com `--remote`) e `cthubd secrets restore`, que exigem o hub parado ou a chave de recuperação. Fica assim; se um método de restauração entrar no `/ws` no futuro, ele nasce fora da lista. Os sensíveis em negrito são os da decisão do épico; os outros "não" são escolhas desta spec (§13).

Regra para o futuro: método novo de leitura entra na lista quando a UI web o usa; método que escreve só entra com decisão registrada.

### 4.2 Métodos novos do hub (fase 1)

| Método | `/ws` | `/web/ws` | Uso |
|---|---|---|---|
| `hub.web.sessions.list` | sim | sim | sessões web: id, criação, último uso, expiração, identidade do proxy, agente do navegador; marca a atual |
| `hub.web.sessions.revoke { id } \| { all: true }` | sim | sim | revogação |
| `hub.push.vapid_key` | sim | sim | chave pública VAPID (base64url) para a inscrição |
| `hub.push.subscribe { endpoint, keys: { p256dh, auth }, label, events }` | não | sim | grava ou atualiza a inscrição deste dispositivo |
| `hub.push.list` | sim | sim | dispositivos inscritos (sem as chaves) |
| `hub.push.update { id, events }` | sim | sim | preferências por evento do dispositivo |
| `hub.push.remove { id }` | sim | sim | remove a inscrição |
| `hub.push.test { id }` | sim | sim | envia uma notificação de teste |

### 4.3 Pushes repassados ao navegador

`hub_sync_status` (sync do hub com o GitHub) e `hub.proposals.changed`, como no `/ws`. Na fase 2, também `env.push` (§5.3) e `env.online { env_id, online }`.

## 5. Canal reverso (fase 2, #198 e #343)

### 5.1 Conexão

- O `ctowerd` abre e mantém `GET /sync/channel` (WebSocket) com o token de sync e o `X-Ctower-Env-Id`, validados como em `/sync/push` (token preso ao `env_id`; outro `env_id` gera `403`). Reconexão com o mesmo backoff exponencial do `hub_sync` (teto `MAX_BACKOFF`, 5 min) e ping a cada 30 s; sem pong em 60 s, o hub considera o ambiente offline.
- Um canal por ambiente. Uma conexão nova do mesmo `env_id` substitui a anterior.
- Hub → ambiente: `{id, method, params}`; ambiente → hub: `{id, result}` ou `{id, error}` (o mesmo `ct_http::rpc`), e pushes `{event, data}`. Prazo de 30 s por chamada no hub; estourado, erro `-32031` "ambiente não respondeu".
- O ambiente responde pelo mesmo despacho do seu `/ws`, depois de aplicar a lista do §5.2. O hub aplica a mesma lista antes de mandar (defesa dupla). O canal só tem frames de texto.

### 5.2 Métodos do ambiente aceitos pelo canal

Métodos do `/ws` do ambiente (protocolo 2.2 hoje, 2.3 com o #197):

| Método | No canal | Observação |
|---|---|---|
| `env.info` | sim | inclui `remote_control` (§6.5) |
| `projects.list` | sim | leitura |
| `dashboard.get` | sim | leitura |
| `tree.get` | sim | leitura |
| `review.get` | sim | leitura |
| `events.list` | sim | leitura (#422, protocolo 2.9) |
| `review.mark_reviewed` | sim | ação da fase 2 |
| `recovery.list` | sim | leitura |
| `recovery.archived` | sim | leitura |
| `transcript.tail` | sim | fim da conversa, já mascarado (Recuperação e painel da sessão) |
| `session.context` | sim | leitura |
| `session.get` | sim | leitura |
| `session.start` | sim | com `remote_control` (§6.2) |
| `session.start_project` | sim | idem |
| `session.start_gestor` | sim | idem |
| `session.resume` | sim | idem |
| `session.kill` | sim | encerrar |
| `session.archive` | sim | descartar da Recuperação |
| `session.remote_control.enable` | sim | novo no #197 |
| `hub.sync.now` | sim | envia o outbox e puxa a configuração |
| `session.attach`, `session.detach` | não | fluxo de terminal |
| `session.input`, `session.resize`, `session.peek` | não | fluxo de terminal |
| `session.permission.watch`, `session.permission.list`, `session.permission.respond` | não | a permissão é respondida pelo Remote Control ou no desktop |
| `env.update`, `env.rollback`, `env.check_update` | não | Manutenção fica no desktop |
| `env.config.get`, `env.config.set` | não | Configurações › Ambiente fica no desktop |
| `cleanup.status`, `cleanup.plan`, `cleanup.run` | não | `cleanup.run` apaga worktrees e branches |
| `github.status`, `github.set_token`, `github.clear_token` | não | token do GitHub do ambiente |
| `github.pr.comment` | não | escreve no GitHub em nome do usuário |
| `sync.now` | não | sync do espelho do GitHub do ambiente; o hub já sincroniza |

O canal binário (`[canal u8][uuid][bytes]`) não existe no canal reverso.

### 5.3 Pushes do ambiente

O ambiente repassa pelo canal só: `session_started`, `session_state`, `session_exited`, `session_archived`, `sessions_interrupted`, `needs_human`, `work_item_status`, `projects_changed` e `alert`. Ficam de fora `permission_request` (traz o `tool_input`), `update.status` e `cleanup.status`. O hub os entrega ao navegador como `env.push { env_id, event, data }`.

### 5.4 No hub

- `hub.env.call { env, method, params }` no `/web/ws` (e no `/ws`, para testes e para o desktop no futuro): roteia ao ambiente, com a lista do §5.2. Ambiente sem canal responde `-32030` "ambiente offline".
- `HubEnv` ganha `online: bool` (canal aberto agora).
- Para iniciar uma sessão, a PWA escolhe entre os ambientes de `hub.assignments.list` do projeto que estão online, como a UI desktop faz hoje.

## 6. Remote Control nas sessões (#197)

### 6.1 Fatos conferidos em 2026-10-28

- Exige login OAuth do claude.ai (Pro, Max, Team ou Enterprise; em Team e Enterprise um Owner precisa ligar o recurso).
- Não funciona com API key, com `ANTHROPIC_BASE_URL` para outro host nem com `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` ou `DISABLE_GROWTHBOOK`.
- Uma conexão por processo; o transcript fica nos servidores da Anthropic enquanto conectado.
- Só faz HTTPS de saída.
- Liga com `claude --remote-control "<nome>"` (ou `--rc`) ao iniciar, ou com `/remote-control <nome>` numa sessão em andamento.
- Não é preciso repetir a flag no `--resume`/`--continue`: a retomada reconecta sozinha.
- O setting `remoteControlAtStartup` liga o recurso em todas as sessões.
- `CLAUDE_CODE_BRIDGE_SESSION_ID` (Claude Code ≥ 2.1.199) chega aos hooks; o link é `claude.ai/code/<id>`, com id no formato `session_…`.
- **Verificado manualmente** (comentários do #197): `--append-system-prompt` funciona junto com `--remote-control`, inclusive em mensagens enviadas pelo celular; `claude --continue` reconecta sem repetir a flag e sem confirmação; `CLAUDE_CODE_BRIDGE_SESSION_ID` tem o mesmo `session_…` do link; a pergunta "Enable Remote Control?" não apareceu, mas pode ter sido aceita antes nesta conta, então o spawn deve tolerá-la.

Consequência: os templates, as regras por projeto e por épico e os limites do gestor continuam no `--append-system-prompt` (`AgentOptions::args` em `crates/ct-sessions/src/launch.rs`).

### 6.2 Ligar ao iniciar

- `StartRequest` ganha `remote_control: bool` (e o mesmo campo em `start_project`, `start_gestor` e `resume`). Com ele, `Sessions::launch` acrescenta `--remote-control "<nome>"` ao `argv`, depois de `--settings`.
- No `--resume`, a flag só entra se a sessão ainda não tinha `rc_session_id` e o pedido veio com `remote_control: true`; se já tinha, a retomada reconecta sozinha.
- `session.start`, `session.start_project`, `session.start_gestor` e `session.resume` aceitam `remote_control` (padrão `false`).
- Preferência global no `ui.json` (`SessionDefaults.remote_control`, desligada), que pré-marca a opção na modal de início do desktop. Na fase 2, a modal de início da PWA vem sempre com a opção marcada, porque pelo celular não há outro jeito de conversar com a sessão.
- O `remoteControlAtStartup` não é usado: ele vale para toda sessão do usuário (inclusive as abertas fora do Control Tower) e não leva o nome.

### 6.3 Ligar numa sessão em andamento

`session.remote_control.enable { session_id }` checa os requisitos (§6.5), escreve `/remote-control <nome>\r` no PTY da sessão e responde `{ requested: true }`. A UI passa a esperar o `rc_session_id` (§6.4) e mostra "conectando" por até 30 s. Sessão já com `rc_session_id` e viva responde sem escrever no PTY.

**Nome:** `<repo>#<n> <título>` nas sessões de issue, `<repo> planejamento` na sessão do projeto e `gestor` na do gestor, truncado em 60 caracteres (por caractere, com `…`), sem quebras de linha nem aspas.

### 6.4 Id da sessão remota

- **Hook:** o `ctower hook` (`crates/ctower/src/hook.rs`) lê `CLAUDE_CODE_BRIDGE_SESSION_ID` do próprio ambiente e o manda no envelope (`HookEnvelope` ganha `rc_session_id: Option<String>`, fora do `payload`). Em `Sessions::apply_hook`, qualquer evento com o campo grava o id quando ele mudou (`Ledger::set_session_rc_id`, que põe a fotografia no outbox).
- **Leitura do PTY, de reserva:** ao ligar pelo `/remote-control`, a variável pode não chegar aos hooks do processo já em andamento. O `ctowerd` procura na saída do PTY, nos 30 s seguintes, o padrão `https://claude.ai/code/(session_[A-Za-z0-9]+)` e grava o mesmo id. O hook vence se os dois divergirem.
- **Banco:** coluna `rc_session_id TEXT` na tabela `session` do `ledger.db` e do `hub.db` (§9). `SessionChange` (`crates/ct-ledger/src/sync.rs`) ganha `rc_session_id: Option<String>` com `#[serde(default)]`, então ambientes e hubs antigos seguem compatíveis.
- **Exposição:** `Session` (ambiente) e `HubSession` (hub) ganham `rc_session_id: string | null`. A PWA e a Web Push consideram o Remote Control **ligado** quando a sessão está viva e tem `rc_session_id`. O id fica gravado depois do fim da sessão (a retomada reconecta na mesma conversa remota).
- **Limite conhecido:** desligar o Remote Control dentro da sessão não gera evento. A UI continua mostrando o link; abrir um link desconectado leva a uma conversa só de leitura no claude.ai.

### 6.5 Requisitos e recusa

- Antes do spawn com `remote_control` e no `session.remote_control.enable`, o `ct-sessions` monta o ambiente efetivo da sessão: o do processo do `ctowerd` mais o que `Sessions::session_secrets` resolveu para o projeto (secrets e variáveis do hub, #166). A sessão é recusada, com o erro `-32032` e o motivo, se esse ambiente tiver:
  - `ANTHROPIC_API_KEY` não vazio;
  - `ANTHROPIC_BASE_URL` com host diferente de `api.anthropic.com`;
  - `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` ou `DISABLE_GROWTHBOOK` não vazios.
- O motivo cita o nome da variável e a origem ("secret do projeto owner/repo", "variável global do hub", "ambiente do ctowerd"), nunca o valor.
- `env.info` ganha `remote_control: { available: bool, reason: string | null }`, calculado com o ambiente do processo e os secrets globais. Secrets por projeto só aparecem na recusa do início.
- O login OAuth não é verificado de antemão. Se faltar, a mensagem do Claude Code aparece no PTY e o `rc_session_id` nunca chega; a UI mostra "não conectou" depois dos 30 s.

### 6.6 Confirmação inicial

Se aparecer na saída do PTY, nos primeiros 30 s depois do spawn com a flag ou do `/remote-control`, uma pergunta que contenha `Enable Remote Control?`, o `ctowerd` responde com `y\r` uma única vez e grava o evento `remote_control_confirmed`. O texto exato não é conhecido (não apareceu nos testes), então o padrão fica numa constante e o teste usa o agente falso (`CTOWER_CLAUDE`) imprimindo a pergunta. Diferente do `trust.rs`, que pré-aceita a confiança na pasta escrevendo `hasTrustDialogAccepted` no `.claude.json`, aqui não há chave conhecida para pré-aceitar.

## 7. Web Push (#203)

### 7.1 O que notifica

O app Claude já manda push nativo quando uma sessão com Remote Control pede ação ("Push when actions required"). A Web Push cobre o resto, e todos os eventos saem do hub, a partir do que o sync entrega:

| Evento (`events`) | Quando | Toque abre |
|---|---|---|
| `session_error` | uma sessão entra em `failed_sessions` (saiu com código ≠ 0) | o item na PWA |
| `env_stale` | `HubEnv.last_seen_at` mais velho que `env_stale_minutes`; na fase 2, canal fechado há mais de 1 min | Hub › Ambientes |
| `proposal_new` | proposta nova do gestor (`POST /agent/v1/proposals`) | Hub › Propostas |
| `needs_human` | evento `needs_human` ou `permission_request` ingerido de uma sessão viva **sem** `rc_session_id` | o item na PWA |

- Sessão com Remote Control nunca gera `needs_human` pela Web Push.
- Deduplicação por chave (`session_error:<session>`, `env_stale:<env>:<desde>`, `proposal_new:<id>`, `needs_human:<session>:<evento>`), gravada no `hub.db`; um `env_stale` só se repete depois que o ambiente voltar.
- O corpo da notificação leva só o projeto, o número, o título e o tipo do evento. Nada de mensagem do agente nem de comando.

### 7.2 VAPID e inscrições

- Par de chaves VAPID (P-256) gerado no primeiro `serve` com a web ligada, em `<config>/vapid.key` (0600, fora do `hub.db` e dos snapshots). O `sub` do JWT VAPID é `mailto:` do primeiro e-mail de `[web.cloudflare] emails` ou, sem ele, `https://<primeiro origin>`.
- Inscrições no `hub.db` (tabela `push_subscription`, §9), uma por dispositivo, ligadas à sessão web que as criou. Revogar a sessão web apaga as inscrições dela.
- Envio cifrado (RFC 8291, `aes128gcm`) com TTL de 1 h. Resposta `404`/`410` do serviço de push apaga a inscrição; outras falhas contam em `failures`, e com 10 seguidas a inscrição é desativada.
- Tela na PWA (Hub › Dispositivos) para ver, testar e remover dispositivos e marcar os eventos de cada um. A mesma lista aparece no desktop.

### 7.3 Deep link

A notificação leva `data.url`:

- sessão com Remote Control ligado: `https://claude.ai/code/<rc_session_id>` (o `notificationclick` do service worker abre em nova janela, e o sistema entrega ao app Claude se ele estiver instalado);
- demais: uma URL da própria PWA, `/?go=<lugar>`, com `go=item&project=<owner>/<repo>&n=<número>`, `go=hub:proposals` ou `go=hub:envs`. O shell traduz `go` para o `Screen` correspondente (`ui/src/state/nav`), e o service worker foca uma janela da PWA já aberta antes de abrir outra.

### 7.4 iOS

A Web Push no iOS exige iOS 16.4 ou mais novo, a PWA adicionada à tela de início e o pedido de permissão feito a partir de um toque do usuário. A PWA mostra essa instrução quando detecta Safari no iOS fora do modo `standalone`, e o `docs/acesso-remoto.md` (#204) repete.

### 7.5 PWA instalável

`manifest.webmanifest` (nome "Control Tower", `display: standalone`, ícones 192 e 512 e o maskable), e um service worker que guarda em cache só o shell (o `index.html` e os assets com hash), nunca respostas do `/web/ws` nem dados. Sem rede, a PWA abre o shell e mostra "hub indisponível".

## 8. Bundle web (#199, #201)

- **Build:** `vite build --mode web` (script `build:web` no `ui/package.json`), com saída em `ui/dist-web`. O modo `web` escolhe a `WebDaemonApi` e não importa nada de `@tauri-apps/*` (o build quebra se importar, por um plugin que recusa o pacote no modo `web`).
- **Embutido no `cthubd`** com `rust-embed`. Um `build.rs` aponta para `ui/dist-web` se ele existir; se não existir, aponta para uma página de aviso ("bundle web ausente: rode `npm run build:web`") gerada no `OUT_DIR`, para o `cargo build` e o `cargo test` do workspace seguirem sem Node. Com `CTHUB_WEB_REQUIRED=1`, a falta do bundle faz o build falhar.
- **Cabeçalhos:** assets com hash no nome com `Cache-Control: public, max-age=31536000, immutable`; `index.html`, `sw.js` e `manifest.webmanifest` com `no-cache`. Qualquer caminho que não seja arquivo do bundle nem rota do hub devolve o `index.html`.
- **`cthubd update`** (`crates/cthubd/src/update.rs`): antes do `cargo build`, confere `node --version` (≥ 20) e `npm`; sem eles, falha com mensagem clara, antes de qualquer backup ou troca de binário. Depois roda `npm ci` e `npm run build:web` em `<data>/update/src/ui` e o `cargo build` com `CTHUB_WEB_REQUIRED=1`.
- **Node vira requisito do hub:** o `provision.sh --hub` (`scripts/provision/provision.sh`) instala ou exige Node, e o `docs/hub.md` lista o requisito.
- **CI e release (#279):** o `ui.yml` gera e testa o build web. O job `cthubd` do `release.yml` ganha `setup-node` (22), `npm ci`, `npm run build:web` e `CTHUB_WEB_REQUIRED=1`, então o binário da release leva o mesmo bundle. O `ci.yml` segue sem Node (página de aviso).
- A versão do bundle é a do binário: não há bundle servido de outra origem nem atualização à parte.

## 9. Mudanças de esquema e de protocolo

### 9.1 Migrações

Migrações novas, cada uma no próximo número livre na ordem em que entrar (hoje, 11 no `ledger.db` e 18 no `hub.db`):

```
ledger.db  session + rc_session_id TEXT                                         (#197)
hub.db     session + rc_session_id TEXT                                         (#197)
hub.db     web_session (id TEXT PRIMARY KEY, secret_hash TEXT NOT NULL,
             created_at INTEGER NOT NULL, last_used_at INTEGER NOT NULL,
             expires_at INTEGER NOT NULL, revoked_at INTEGER,
             identity TEXT, user_agent TEXT)                                    (#200)
hub.db     push_subscription (id INTEGER PRIMARY KEY, web_session_id TEXT
             REFERENCES web_session(id) ON DELETE CASCADE,
             endpoint TEXT NOT NULL UNIQUE, p256dh TEXT NOT NULL,
             auth TEXT NOT NULL, label TEXT, events TEXT NOT NULL,              -- JSON
             created_at INTEGER NOT NULL, last_ok_at INTEGER,
             failures INTEGER NOT NULL DEFAULT 0, disabled_at INTEGER)
           push_sent (key TEXT PRIMARY KEY, sent_at INTEGER NOT NULL)           (#203)
```

A tabela `web_session` entra nos snapshots do `hub.db`; restaurar um snapshot antigo pode trazer de volta sessões já revogadas, então o `cthubd restore` revoga todas as sessões web depois de trocar o banco.

### 9.2 Versões

Pela convenção do repositório, só entram métodos e campos: sobe o minor.

- **Ambiente 2.3** (#197): `remote_control` em `session.start|start_project|start_gestor|resume`, `session.remote_control.enable`, `Session.rc_session_id` e `EnvInfo.remote_control`.
- **Hub 1.3** (fase 1: #197, #199, #200, #203): `HubSession.rc_session_id`, `hub.web.sessions.*` e `hub.push.*`.
- **Hub 1.4** (fase 2: #198, #343): `hub.env.call`, `HubEnv.online` e os pushes `env.push` e `env.online`.
- O contrato do sync ganha `SessionChange.rc_session_id` (opcional) e a rota `/sync/channel`; um ambiente sem o canal continua sincronizando por `/sync/push` e `/sync/config`.
- O `/web/ws` não tem versão própria: é o protocolo do hub com a lista de permissões, e a `WebDaemonApi` confere o major em `hub.info` como o desktop.

## 10. UI (#201, #202)

### 10.1 `WebDaemonApi`

- Implementa `DaemonApi` sobre o `/web/ws`, com o cookie (sem token no JS). `hubCall` vai direto; `getHub`/`saveHub` devolvem a conexão fixa (a própria origem); `hubStatus`/`onHubStatus` refletem a conexão do `/web/ws`, com reconexão e backoff.
- `listEnvs`/`statuses` vêm de `hub.envs.list`: o estado de cada ambiente é o do último sync (fase 1) ou do canal (fase 2), e não de uma conexão direta. `saveEnvs` não existe na web.
- `call(env, method)`: na fase 1, rejeita com "disponível na fase 2" e a UI esconde as ações; na fase 2, vira `hub.env.call`.
- `attach`, `detach`, `input`, `resize`, `openVsCode`, `uiUpdateCheck|Install` e `onUiUpdateProgress` não existem na web (rejeitam), e a UI não chega a chamá-los.
- `openUrl` usa `window.open`; a área de transferência usa `navigator.clipboard`. `getUiConfig`/`saveUiConfig` guardam as preferências da web em `localStorage` (tema, telas), sem nada sensível.
- A escolha da implementação é feita no build (`import.meta.env.MODE === "web"`), não em tempo de execução, para o bundle web não carregar o código do Tauri.

### 10.2 O que some na web

- Terminal (tela `terminal` e o painel inferior de terminal), "Abrir no VS Code", atualizador do app.
- Configurações › Este app (fonte e tamanho do terminal) e Configurações › Ambiente (só leitura do daemon).
- Manutenção inteira (#340), inclusive Backup.
- No Hub, o grupo Segurança inteiro (Secrets, Chave de recuperação, GitHub) e as ações de cadastro (adicionar, editar e excluir projeto, atribuir e remover atribuição, descartar quarentena). A leitura de Projetos, Ambientes, Propostas, Saúde e Quarentena fica.
- No lugar do terminal, o painel da sessão mostra estado, modelo, ambiente e há quanto tempo está aguardando, e o botão **Abrir no Remote Control** quando ele está ligado. Na fase 2, também **Ligar Remote Control**, **Encerrar** e **Retomar**.

### 10.3 Shell no celular

Sobre o shell do #310 (`ui/src/shell`), abaixo de 768 px:

- a barra de atividades vira barra inferior, com as telas da web (Sala de controle, Projeto, Revisão, Recuperação, Hub) e os selos;
- o painel lateral vira gaveta;
- uma aba visível por vez, com as abas abertas numa lista no cabeçalho;
- painel inferior e barra de status ocultos; os avisos (`hub.status`) ficam num ícone no cabeçalho;
- alvos de toque de pelo menos 44 px, só tokens de `theme.css`.

"Aguardando você" fica no topo da Sala de controle; com o Remote Control ligado, o toque abre direto o `claude.ai/code/<id>`.

### 10.4 Desktop (#197)

"Ligar Remote Control" no cabeçalho do painel da sessão e no menu de contexto, indicador de conectado na árvore e na barra de status, e o link com QR code para abrir no celular. Opção "Remote Control" na modal de início, pré-marcada pela preferência do `ui.json`.

## 11. Segurança e privacidade

| Credencial | Onde fica | Dá acesso a |
|---|---|---|
| Token de UI do hub (`~/.config/cthub/ui.token`) | desktop; digitado uma vez no login web | `/ws` inteiro; `/web/login` |
| Cookie `ct_web` | navegador (`HttpOnly`) | `/web/ws`, só a lista do §4 |
| JWT do Cloudflare Access | navegador, emitido pela Cloudflare | porta de entrada das rotas web (não basta sozinho) |
| `Tailscale-User-Login` | header do `tailscale serve` | idem |
| Token de sync (um por ambiente) | `~/.config/ctower/hub.token` | `/sync/*`, incluindo `/sync/channel` (fase 2) |
| Chave VAPID (`<config>/vapid.key`) | hub | assinar a Web Push |

- **Pior caso com o cookie vazado:** ler o portfólio, mudar status e prioridade, decidir propostas e, na fase 2, iniciar e encerrar sessões pelos métodos do §5.2. Nunca ler ou gravar secrets, chave de recuperação, token do GitHub ou backup, nem excluir projeto. Revogar todas as sessões web corta o acesso.
- **Iniciar sessão pela web na fase 2 executa um agente** na máquina do ambiente. É a ação mais forte disponível no celular; por isso a fase 2 tem verificação manual própria (#343) e o login exige o token além do proxy.
- **Cloudflare:** o TLS termina nela, que vê o tráfego decifrado (dashboard, títulos, decisões, `transcript.tail` na fase 2).
- **Remote Control:** o transcript fica na Anthropic enquanto a sessão está conectada.
- **Web Push:** o conteúdo passa cifrado pelo serviço de push do navegador (Google, Apple, Mozilla), que vê só o endpoint e o tamanho.

## 12. Fora do escopo

- Terminal no celular, em qualquer fase.
- Responder pedidos de permissão do agente pela PWA (`session.permission.respond`): é papel do Remote Control.
- Cadastro de projetos, atribuições, secrets, backup e atualização pela web.
- Usuários múltiplos: a web tem uma identidade só, a de quem tem o token de UI.

## 13. Decisões

Escolhas desta spec que não estavam decididas no épico #205, para revisão do usuário (registradas no ledger com origem autônoma):

1. **Rota própria `/web/ws` com cookie, separada do `/ws` com Bearer** (§1, §4). Motivo: a lista de permissões fica presa à rota, o `/ws` do desktop não muda, e um bug na checagem do cookie não abre os métodos sensíveis.
2. **Lista de permissões mínima, além dos sensíveis do épico** (§4.1, §5.2). Fora também: cadastro de projetos e atribuições, `hub.quarantine.discard`, ordem por arrasto, leituras das seções sensíveis (`hub.secrets.list` devolve valores de variáveis) e Manutenção/backup; no canal, `cleanup.*`, `env.update|rollback|config.*`, `github.*` e `session.permission.*`. Motivo: a web expõe o hub para fora da máquina; método entra quando a UI web precisa e, se escreve, com decisão.
3. **Identidade do proxy é porta a mais, não substituto do login; o token de UI é sempre exigido para criar o cookie** (§3.2). Motivo: o #204 trata o token como segunda camada; um erro de configuração do Access não abre o hub.
4. **Requisição vinda pela Cloudflare só alcança rotas web** (`404` em `/ws`, `/sync/*`, `/agent/v1/*` com `Cf-Connecting-IP`) **e o `Tailscale-User-Login` só vale do loopback sem headers `Cf-*`** (§3.2). Motivo: o `cloudflared` também fala do loopback, e o túnel não deve virar caminho para o token de UI nem para o sync.
5. **Validade de 30 dias absolutos e 7 sem uso; bloqueio global após 5 falhas, dobrando de 1 min até 1 h** (§3.1). Motivo: atrás de proxy não há IP útil por cliente; o token tem entropia alta, o bloqueio é contra força bruta e ruído.
6. **`rc_session_id` pelo hook, com leitura do PTY de reserva, e "ligado" = sessão viva com `rc_session_id`** (§6.4). Motivo: ligar pelo `/remote-control` numa sessão em andamento pode não pôr a variável no ambiente dos hooks; desligar não gera evento, então o estado é aproximado.
7. **Flag `--remote-control "<nome>"`, sem `remoteControlAtStartup`; nome truncado em 60 caracteres; confirmação respondida com `y` uma vez por padrão de texto** (§6.2, §6.3, §6.6). Motivo: o setting é global para o usuário e não leva nome; o texto da confirmação não é conhecido.
8. **Recusa do Remote Control com o ambiente efetivo (processo + `session_secrets` do projeto), sem checar o login OAuth de antemão** (§6.5). Motivo: os secrets do hub são a causa mais provável de API key; não há verificação barata de login.
9. **Bundle por `rust-embed` com página de aviso quando falta, obrigatório só com `CTHUB_WEB_REQUIRED=1` (update e release)** (§8). Motivo: `cargo build` e `cargo test` do workspace e o `ci.yml` seguem sem Node; o que vai ao usuário sempre leva o bundle.
10. **Chave VAPID em arquivo no diretório de config, inscrições e deduplicação no `hub.db`; revogar a sessão web apaga as inscrições dela** (§7.2). Motivo: a chave privada não vai para os snapshots remotos; o dispositivo perde o push junto com o acesso.
11. **Canal reverso aceita `transcript.tail` e repassa só os pushes de estado** (`permission_request` fora) (§5.2, §5.3). Motivo: a Recuperação e o painel da sessão mostram o fim da conversa já mascarado; o `tool_input` do pedido de permissão pode carregar comando e caminho.
12. **Protocolo do ambiente 2.3, do hub 1.3 na fase 1 e 1.4 na fase 2** (§9.2). Motivo: só entram métodos e campos.

## 14. O que cada sub-issue entrega

Fase 1:

1. **#196** — esta spec.
2. **#197** — ct-sessions, ct-ledger, ct-server, ctower, ui: `remote_control` no início e na retomada, `session.remote_control.enable`, nome, recusa com o ambiente efetivo, confirmação tolerada, `rc_session_id` (hook, PTY, migração nos dois bancos, `SessionChange`, `Session`, `HubSession`), `EnvInfo.remote_control`; UI desktop (§10.4); ambiente 2.3 (§6, §9).
3. **#199** — cthubd, ui, CI: `rust-embed` com `build.rs`, cabeçalhos de cache, `build:web`, `cthubd update` com Node, `provision.sh` e `docs/hub.md`, job `cthubd` da release; `/web/ws` com a lista do §4.1 e os pushes; checagem de `Origin` (§3.3, §4, §8).
4. **#200** — cthubd: `[web]` no config, `/web/login|logout|me`, cookie, tabela `web_session`, validade, bloqueio, `hub.web.sessions.*`, `cthubd web revoke-all`, Cloudflare Access, Tailscale, restrição das rotas vindas pela Cloudflare, revogação após `restore`; hub 1.3 (§3, §4.2).
5. **#201** — ui: `WebDaemonApi`, modo `web` sem `@tauri-apps/*`, telas e seções ocultas, painel da sessão com **Abrir no Remote Control**, CI do `ui.yml` (§10.1, §10.2).
6. **#202** — ui: shell no celular, ações da fase 1, "aguardando você" em destaque (§10.3).
7. **#203** — cthubd, ui: manifest, ícones, service worker, VAPID, `hub.push.*`, tabelas `push_subscription` e `push_sent`, eventos e deduplicação, deep link, Hub › Dispositivos, instrução do iOS (§7).
8. **#204** — docs: `docs/acesso-remoto.md` com Tailscale e Cloudflare Tunnel com Access, endurecimento, privacidade e a relação com o #252 (§3, §11).

Fase 2:

9. **#198** — ctowerd, cthubd: `/sync/channel`, lista do §5.2 nos dois lados, pushes do §5.3, `HubEnv.online` e `env.online` (§5.1 a §5.3).
10. **#343** — cthubd, ui: `hub.env.call`, `env.push`, ações de sessão, Remote Control, revisão e Recuperação na PWA; `env_stale` em tempo real; hub 1.4 (§2.2, §5.4).
