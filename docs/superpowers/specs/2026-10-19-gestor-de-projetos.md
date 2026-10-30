# Gestor de projetos sob demanda (issue #214)

Spec do épico #221. Parte da spec do hub como plano de controle (`2026-10-18-hub-plano-de-controle.md`), que continua valendo: o `cthubd` é a origem de projetos e a única cópia do histórico; o `ctowerd` executa sessões, PTY, hooks e MCP. Esta spec acrescenta uma sessão que não pertence a projeto nenhum, uma prioridade gravada no hub, sinais de saúde, uma API de leitura do hub para agentes e propostas aprovadas na UI.

As decisões confirmadas pelo usuário estão no épico #221 e não são reabertas aqui. As escolhas que esta spec acrescenta estão na §13 e precisam da revisão do usuário antes das sub-issues #215 a #220.

## 1. Problema e direção

Hoje toda sessão está presa a um work item de um projeto: `session.work_item_id` é `NOT NULL` no `ledger.db` e no `hub.db`, e a sessão de planejamento usa o work item de número `PROJECT_WORK_ITEM = 0` (`crates/ct-sessions/src/launch.rs`, `Sessions::start_project`). O MCP do ambiente (`crates/ct-server/src/mcp.rs`) só enxerga o work item da sessão; o único acesso dele ao hub é `POST /sync/query/decisions`, para `ledger_list_decisions` com escopo `epic`/`project`. Não existe prioridade nem ordem no esquema.

Direção:

```
 UI (tela Hub)
  ├─ botão "Gestor de projetos" ── session.start_gestor ──▶ ctowerd escolhido
  │                                                          sessão do gestor em <data>/gestor
  │                                                          (projeto sentinela _ctower/gestor, #0)
  │                                                            │ MCP /mcp: portfolio_* e ledger_*
  │                                                            ▼
  │                                     ctowerd ── GET/POST /agent/v1/* (token de agente) ──▶ cthubd
  │                                                                                          hub.db:
  └─ /ws do hub: hub.proposals.list|decide, hub.workitem.set_priority|move, hub.health.get   prioridade, ordem,
                 (token de UI do hub) ──────────────────────────────────────────────────────▶ propostas, eventos
```

- O gestor **lê** o hub inteiro pelas ferramentas `portfolio_*`, que o `ctowerd` repassa ao hub.
- O gestor **propõe** mudanças de prioridade e de status. A proposta fica pendente no `hub.db`; o usuário aprova ou rejeita na UI, e o hub aplica.
- O gestor **cria issues** com `gh` só depois do ok do usuário na conversa, como a sessão de planejamento. O app continua sem escrever no GitHub.

## 2. Sessão do gestor (#219)

### 2.1 Modelagem: projeto sentinela

A sessão do gestor é uma sessão comum de um **projeto sentinela**, com chave reservada `_ctower/gestor`, e o work item dela é o de número `0` desse projeto (o mesmo `PROJECT_WORK_ITEM` da sessão de planejamento).

Mudanças de esquema (novas):

```
hub.db     project + kind TEXT NOT NULL DEFAULT 'github'
                     CHECK (kind IN ('github', 'gestor'))
           a migração insere a linha key = '_ctower/gestor', gh_owner = '_ctower',
           gh_repo = 'gestor', name = 'Gestor de projetos', kind = 'gestor', rev = 0
ledger.db  project + kind TEXT NOT NULL DEFAULT 'github'  (mesmo CHECK)
```

- Por que funciona: `session`, `decision`, `event`, hooks, recuperação, outbox e ingestão no hub seguem sem mudança, porque todos chegam ao projeto pela chave `owner/repo`. As decisões do gestor ficam em `decision.work_item_id` = work item `_ctower/gestor#0`, que existe nos dois bancos.
- O sentinela **precisa existir no hub**: o push de linhas de projeto desconhecido vai para `push_quarantine` (spec do hub §5). Por isso a migração do `hub.db` cria a linha. No ambiente, `Sessions::start_gestor` cria a linha local sob demanda (`ensure_gestor_project`), com `repo_path` = a pasta do gestor (§2.2).
- `_ctower` não é um dono válido no GitHub (nomes não começam com `_`), então a chave não colide com um repositório real.
- Quem precisa ignorar `kind = 'gestor'`:
  - no hub: sync do GitHub (`crates/cthubd/src/github.rs`), `project_views`, `tree`, `dashboard().projects`, `env_config` (o sentinela nunca é atribuído nem sai no `/sync/config`), `hub.projects.update|delete` (recusam), saúde (§5) e ranking (§4). As sessões do gestor continuam em `dashboard().live_sessions`, para a UI mostrar onde ele roda;
  - no ambiente: a tarefa de pull (não marca o sentinela com `unassigned_at`, apesar de ele não vir em `projects`), o relato `project_location` (não enfileira), o sync do GitHub (`crates/ct-sessions/src/github.rs`), `projects.list` e `dashboard.get`, e o guarda-corpo (§10.2).
- Poda (spec do hub §6.1): o work item `#0` do sentinela nunca fica `concluido`, então nunca é podado. O volume é pequeno (uma conversa por vez).
- Vários ambientes podem rodar o gestor. No hub, as sessões deles caem no mesmo work item `_ctower/gestor#0` (fusão pela chave natural), e as decisões ficam juntas.

Alternativas descartadas:

- **Novo tipo de sessão sem work item:** exigiria tornar `session.work_item_id` e `decision.work_item_id` opcionais nos dois bancos (recriar tabelas no SQLite, com as triggers de só inclusão) e tratar o caso nulo em hooks, recuperação, gatilhos de status, outbox, ingestão e UI.
- **Novo tipo de work item (`kind = 'gestor'`) num projeto real:** prenderia o gestor a um projeto, o contrário do que o épico pede, e misturaria as decisões dele com as do projeto.
- **Sentinela cadastrado como projeto comum no hub:** apareceria nas listas, o hub tentaria sincronizá-lo com o GitHub e o ambiente tentaria cloná-lo.

### 2.2 Ambiente e pasta dedicada

- O usuário escolhe o ambiente no diálogo do botão (§11). Só servem ambientes ligados a um hub (`env.info.hub_config.linked`) e com token de agente gravado (§6.1).
- Pasta: `gestor_dir` no `config.toml` do ambiente, padrão `<data_dir>/gestor` (`~/.local/share/ctower/gestor`). O daemon cria a pasta (`0700`) se faltar. Não é repositório git nem worktree.
- Todas as sessões do gestor do ambiente usam a mesma pasta. O Claude Code associa transcripts ao `cwd`, e assim `session.resume` funciona como nas outras sessões.
- Sem repositório no `cwd`, o `gh` precisa de `-R owner/repo` em todo comando. O prompt diz isso (§10.1).

Alternativa descartada: um diretório temporário por sessão. A retomada exige o mesmo `cwd`, e a pasta temporária pode sumir.

### 2.3 Início e retomada

Método novo no `/ws` do ambiente:

```
session.start_gestor { prompt, scope, model?, mode?, cols?, rows? }
  scope: { kind: "portfolio" } | { kind: "project", owner, repo } | { kind: "health" }
```

- Recusa sem hub (`ambiente não ligado a um hub...`, como `session.start`) ou sem token de agente (`ambiente sem token de agente do hub: gere com "cthubd agent-token <nome>" e grave com "ctower hub set-agent-token"`).
- `Sessions::start_gestor` (novo, ao lado de `start_project`): garante o sentinela e o work item `#0`, cria a pasta e chama `launch` com `cwd = gestor_dir`. Um evento `gestor_started { scope, tool_session_id }` registra o escopo, ligado à conversa (que a retomada reusa). Várias sessões do gestor podem estar vivas ao mesmo tempo, no mesmo work item e na mesma pasta; o `dashboard.get` do ambiente traz `gestor_scopes` (id da sessão viva → escopo) para a UI distingui-las.
- `launch` acrescenta, só no gestor: `--append-system-prompt` com os limites fixos (§10.2), somado ao do modo `subagents` quando houver, e a variável `CTOWER_SESSION_KIND=gestor` para o hook. Na retomada (`session.resume`), os mesmos argumentos voltam, porque os limites não podem depender do transcript.
- `pre_trust` aceita a pasta do gestor (o `repo_path` do sentinela é a própria pasta).
- O protocolo do ambiente passa a **2.1** (só entram método e campos).

### 2.4 Decisões do gestor

- `ledger_record_decision` grava no work item `_ctower/gestor#0`. Uma decisão sobre um projeto ou issue cita o alvo no texto (`owner/repo#n`). Não há coluna de alvo na `decision`.
- A revisão das autônomas usa o fluxo de hoje (`review.get` e `review.mark_reviewed` no ambiente, sobre o work item do gestor). A UI do gestor oferece "Revisar decisões do gestor" (§11).
- `ledger_set_status` no gestor é recusado (`o gestor não tem status próprio; proponha com portfolio_propose`).
- `ledger_list_decisions` com escopo `work_item` devolve as decisões do gestor; `epic` e `project` são recusados (use `portfolio_list_decisions`).
- `ledger_get_context` no gestor devolve `{ "kind": "gestor", "env": ..., "gestor_dir": ..., "scope": ..., "hub_url": ..., "hub_reachable": bool }`.

## 3. Escopos

O escopo é escolhido no diálogo de início e **só orienta o prompt inicial**. As ferramentas são as mesmas nos três, e todas leem o hub inteiro.

| Escopo | Foco do prompt | Ferramentas que o prompt sugere |
|---|---|---|
| Portfólio | todos os projetos do hub: andamento, prioridades entre projetos, o que fazer a seguir | `portfolio_overview`, `portfolio_ranking`, `portfolio_health` |
| Um projeto | `owner/repo` escolhido: backlog, épicos, ordem, issues que faltam | `portfolio_project`, `portfolio_work_item`, `portfolio_ranking { owner, repo }` |
| Saúde e riscos | o que está parado, bloqueado ou falhando, e o que propor | `portfolio_health`, `portfolio_work_item`, `portfolio_list_decisions` |

Alternativa descartada: restringir as ferramentas ao projeto escolhido. Tudo é leitura, e o usuário costuma mudar de assunto na conversa ("e o outro projeto?"). Restringir obrigaria a abrir outra sessão.

## 4. Prioridade e ordem (#215)

### 4.1 Escala

| Valor | Sentido |
|---|---|
| `P0` | urgente: passa na frente de tudo; poucos itens por vez |
| `P1` | próximo: o que entra quando abrir espaço |
| `P2` | normal |
| `P3` | quando der |
| nulo | não triado |

Nulo é diferente de `P2`. Um item sem prioridade aparece como "não triado" e é a matéria-prima do gestor.

Alternativas descartadas: número livre (1–100), que não dá classes comparáveis entre projetos; `alta/média/baixa`, sem um nível de urgência acima do "alta".

### 4.2 Esquema (hub.db, migração nova)

```
work_item + priority TEXT CHECK (priority IS NULL OR priority IN ('P0','P1','P2','P3'))
          + rank INTEGER            -- ordem dentro de (projeto, prioridade); nulo = fim da classe
          + priority_at INTEGER     -- última mudança de prioridade ou ordem
project   + rank INTEGER            -- ordem entre projetos; nulo = fim
```

- As colunas são **só do hub**. A ingestão (`Hub::ingest`, `apply_work_item`) nunca as escreve: o ambiente não tem prioridade e o push não as traz.
- Issue sem work item no hub: definir prioridade cria o work item, como `set_status_manual` já faz (`epic` se tem sub-issues no espelho, senão `issue`). A maioria das issues do backlog ainda não tem work item.
- A prioridade não vai para o ambiente nem para o GitHub.

### 4.3 Ordem

- **Dentro de um projeto:** `ORDER BY priority NULLS LAST, rank NULLS LAST, gh_number`. O `rank` é denso (1, 2, 3...) dentro de cada (projeto, prioridade).
- **Entre projetos:** a classe vem primeiro, e o projeto só desempata. `ORDER BY priority NULLS LAST, project.rank NULLS LAST, project.key, work_item.rank NULLS LAST, gh_number`. Um P0 de qualquer projeto vem antes de qualquer P1.
- Itens `concluido` e issues fechadas no espelho saem do ranking. Mudar a prioridade leva o item ao fim da nova classe.
- Mover (`hub.workitem.move`) reescreve os `rank` da classe inteira numa transação. É o único escritor, e as classes têm dezenas de itens.

Alternativas descartadas: ordem global única entre todos os itens (cada projeto novo bagunça tudo, e o usuário pensa em "este projeto antes daquele"); `rank` fracionário ou LexoRank (só compensa com vários escritores concorrentes, e o hub é o único).

### 4.4 Métodos e eventos

`/ws` do hub (token de UI):

```
hub.workitem.set_priority { owner, repo, number, priority: "P0".."P3" | null, reason? }
hub.workitem.move         { owner, repo, number, before?: number, after?: number }  -- mesma classe
hub.projects.move         { owner, repo, before?: {owner, repo}, after?: {owner, repo} }
hub.ranking.get           { owner?, repo?, limit? }
```

Cada mudança grava, na mesma transação, um evento no `event` do hub com `env_id = hub_id` (o padrão de `set_status_manual`) e o HLC do hub (`tick_hlc`):

- `priority_set { from, to, rank_from, rank_to, cause: "manual" | "proposal", proposal_id?, reason? }`;
- `rank_moved { priority, rank_from, rank_to, cause, proposal_id? }`;
- `project_rank_moved { rank_from, rank_to }`. Como `event.project_id` é obrigatório, o evento fica no projeto movido.

O histórico do work item (`hub.workitem.history`) passa a mostrar esses eventos. Os `HubWorkItem` e as respostas de `hub.tree.get` e `hub.dashboard.get` ganham `priority` e `rank`. O protocolo do hub passa a **1.1**.

## 5. Sinais de saúde e risco (#216)

Calculados na leitura a partir do `hub.db` (`Hub::health`, novo), sem tabela própria. Os limiares ficam no `config.toml` do `cthubd` (§5.2). O sentinela do gestor fica de fora.

"Atividade" de um work item = o maior entre o `ts` do último evento dele e o `last_event_at` da última sessão (o mesmo `last_activity_at` de `WorkItemSummary`).

### 5.1 Critérios

| Sinal (`kind`) | Critério | Aviso | Crítico |
|---|---|---|---|
| `parado` | status `em_execucao` ou `em_revisao`, sem sessão viva, sem atividade há | 3 dias | 7 dias |
| `esperando_voce` | status `aguardando_voce` há | 24 h | 72 h |
| `bloqueado` | status `bloqueado` (pelo último evento de status) há | 2 dias | 7 dias |
| `pausado_longo` | status `pausado` há | 14 dias (info) | — |
| `pr_sem_andamento` | PR `open` em `pr_cache` com `updated_at` mais velho que | 3 dias | 7 dias |
| `decisoes_sem_revisao` | decisão `autonoma` sem `reviewed_at` mais velha que | 3 dias | 7 dias, ou work item `concluido` com pendência |
| `sessoes_falhas` | sessões `exited` com `exit_code ≠ 0` na janela de 24 h (a de `dashboard`) | 1 | 3 no mesmo work item |
| `sessao_interrompida` | sessão `interrupted` (recuperação pendente) há | 1 h | 24 h |
| `p0_sem_andamento` | `P0` não concluído sem sessão nem atividade há | 24 h | 72 h |
| `nao_triado` | issues abertas sem prioridade no projeto | 10 (info) | — |
| `ambiente_mudo` | ambiente com `pending > 0` e `last_seen_at` há | 1 h | 24 h |
| `github_sem_sync` | `sync_state.last_ok_at` mais velho que 3 × `gh_sync_interval_secs`, ou `last_error` | aviso | — |

A regra geral é aviso no limiar e crítico no dobro ou no valor da tabela. Os números saem da rotina atual: um dia de trabalho sem tocar num item é normal; três já é esquecimento. A janela de 24 h das sessões falhas é a de `FAILED_WINDOW_MS` (`crates/ct-ledger/src/hub/views.rs`).

Resposta (`hub.health.get` e a API de agentes):

```json
{ "generated_at": 1760000000000,
  "signals": [
    { "kind": "bloqueado", "severity": "critico", "owner": "deep-ion-ai", "repo": "control-tower",
      "number": 176, "title": "atribuições", "priority": "P1",
      "since": 1759300000000, "value": "8 dias", "threshold": "7 dias",
      "detail": "bloqueado: aguardando revisão externa" } ],
  "counts": { "critico": 1, "aviso": 4, "info": 2 } }
```

`severity` é `info`, `aviso` ou `critico`. A lista vem ordenada por severidade, prioridade e idade.

### 5.2 Configuração

```toml
# ~/.config/cthub/config.toml
[health]
stale_days = 3
waiting_hours = 24
blocked_days = 2
paused_days = 14
pr_stale_days = 3
decision_review_days = 3
failed_sessions_critical = 3
interrupted_hours = 1
p0_idle_hours = 24
untriaged_info = 10
env_silent_hours = 1
```

Alternativa descartada: guardar os sinais numa tabela atualizada por tarefa periódica. O cálculo é barato para dezenas de projetos e não fica velho.

## 6. API de leitura do hub para agentes (#217)

Rotas novas no `cthubd`, com prefixo `/agent/v1/`. Só o token de agente (§6.1) as acessa; o token de sync e o de UI recebem `401` nelas, e o token de agente recebe `401` em `/sync/*` e `/ws`.

| Rota | Ferramenta MCP (§7) |
|---|---|
| `GET /agent/v1/overview` | `portfolio_overview` |
| `GET /agent/v1/projects/{owner}/{repo}` | `portfolio_project` |
| `GET /agent/v1/work-items/{owner}/{repo}/{number}` | `portfolio_work_item` |
| `GET /agent/v1/ranking` | `portfolio_ranking` |
| `GET /agent/v1/health` | `portfolio_health` |
| `GET /agent/v1/decisions` | `portfolio_list_decisions` |
| `GET /agent/v1/proposals` | `portfolio_list_proposals` |
| `POST /agent/v1/proposals` | `portfolio_propose` |

- O ambiente manda `X-Ctower-Env-Id` (divergente do `env_id` preso ao token = `403`, como no sync) e `X-Ctower-Session` (o id da sessão do gestor, gravado na proposta).
- As respostas só trazem títulos, estados e números. O corpo das issues fica de fora: o gestor lê com `gh issue view -R`. Toda lista tem `limit` (padrão 50, máximo 200) e devolve `truncated: true` quando corta. Isso protege o contexto do agente.
- O único escritor é `POST /agent/v1/proposals`, que só cria proposta pendente (§8).

### 6.1 Token de agente

- **Escopo:** leitura de `/agent/v1/*` e criação de proposta. Não envia (`/sync/push`), não puxa configuração (`/sync/config`), não abre o `/ws` do hub e, portanto, não aprova propostas.
- **Um por ambiente**, preso ao `env_id` do token de sync daquele ambiente. Esquema novo:

```
agent_token  env_name   TEXT PRIMARY KEY      -- = env_token.name
             token_hash TEXT NOT NULL UNIQUE
             created_at INTEGER NOT NULL
             last_used_at INTEGER
```

- **Geração:** `cthubd agent-token <nome>` (novo) gera ou troca e imprime o token em claro uma vez; o hub guarda só o hash (mesmo `token_hash` de `add_env_token`). O formato é o do token de sync com o prefixo `cta_`, para o hub dar um erro claro quando um token é usado na rota errada. Exige que já exista `env_token` com esse nome.
- **No ambiente:** `ctower hub set-agent-token` (novo, lê da entrada padrão como `set-token`) grava em `~/.config/ctower/hub-agent.token` (`0600`, `Dirs::hub_agent_token_file`). O `ctowerd` lê o arquivo a cada chamada repassada, então trocar o token não exige reiniciar. `env.info.hub_config` ganha `agent_token: bool`.
- **Rotação:** rodar `cthubd agent-token <nome>` de novo; o token anterior deixa de valer na hora. `cthubd revoke-agent-token <nome>` corta só o acesso de agente; `cthubd revoke-env <nome>` apaga os dois na mesma transação. Sem validade automática, como os outros tokens (o transporte é o Tailscale).
- **Instalação de máquina única:** `provision.sh --hub --env` gera e grava o token de agente junto com o de sync.
- **O agente não recebe o token.** O `ctowerd` repassa as chamadas das ferramentas `portfolio_*` ao hub; o token não entra em `mcp.json`, nas variáveis da sessão nem no prompt.

Alternativas descartadas:

- **Reusar o token de sync:** ele também escreve no hub (`/sync/push`). Com um token separado dá para cortar o acesso do gestor sem parar a replicação e registrar quem propôs.
- **Entregar o token ao agente (variável ou arquivo) e deixá-lo chamar o hub com `curl`:** o agente lidaria com autenticação e formatos crus, sem o limite de tamanho e as mensagens de erro das ferramentas.
- **Token único do hub para todos os agentes:** não dá para revogar um ambiente sozinho.

## 7. Ferramentas `portfolio_*` do MCP (#219)

Servidas pelo `/mcp` do ambiente só para sessões do sentinela (`tools/list` muda conforme o `kind` do projeto da sessão). O `initialize` devolve instruções próprias do gestor (§10.2). Cada chamada vira uma requisição à rota da §6. Hub fora do ar: `isError: true` com `hub indisponível: <motivo>`, sem resposta parcial, porque o ambiente não tem a visão do portfólio.

Todas são de leitura, menos `portfolio_propose`.

| Ferramenta | Parâmetros | Resposta |
|---|---|---|
| `portfolio_overview` | — | projetos (ordem entre projetos) com contagem por status e por prioridade, PRs abertos, decisões pendentes, contagem de sinais, ambientes atribuídos e estado da atribuição; ambientes com `last_seen_at` e `pending` |
| `portfolio_project` | `owner`, `repo`, `include_closed?` (padrão `false`), `limit?` | árvore do projeto (épicos e sub-issues, como `hub.tree.get`) com `number`, `title`, `status`, `priority`, `rank`, `labels`, PR (`number`, `state`, `updated_at`), `last_activity_at`, `live_envs`, decisões pendentes |
| `portfolio_work_item` | `owner`, `repo`, `number`, `events?` (padrão 30) | item, épico pai, sub-issues, sessões (ambiente, estado, datas), PRs, últimos eventos (status, prioridade), decisões e propostas do item |
| `portfolio_ranking` | `owner?`, `repo?`, `priority?`, `limit?` | lista ordenada pela §4.3: `position`, `owner/repo#number`, `title`, `priority`, `rank`, `status` |
| `portfolio_health` | `owner?`, `repo?`, `min_severity?` (padrão `aviso`) | sinais da §5 |
| `portfolio_list_decisions` | `owner?`, `repo?`, `number?`, `scope?` (`work_item`\|`epic`\|`project`), `origin?`, `unreviewed?`, `since?`, `limit?` | decisões com `owner/repo#number`, `origin`, `title`, `decision`, `reviewed_at`, `created_at` |
| `portfolio_list_proposals` | `state?` (padrão `pendente`), `owner?`, `repo?`, `limit?` | propostas (§8) |
| `portfolio_propose` | `kind` (`priority`\|`status`), `owner`, `repo`, `number`, `proposed`, `justification` | a proposta criada, com `id`, `state: "pendente"` e o valor atual lido pelo hub |

Exemplo (`portfolio_ranking { "limit": 2 }`):

```json
{ "items": [
    { "position": 1, "owner": "deep-ion-ai", "repo": "control-tower", "number": 215,
      "title": "prioridade e ordem dos work items no hub", "priority": "P0", "rank": 1, "status": "backlog" },
    { "position": 2, "owner": "deep-ion-ai", "repo": "outro", "number": 12,
      "title": "corrigir login", "priority": "P1", "rank": 1, "status": "em_execucao" } ],
  "truncated": true }
```

Nomes com prefixo `portfolio_` (e não `ledger_`) separam o que lê o hub inteiro do que age sobre a sessão.

## 8. Propostas (#218)

### 8.1 Tipos e campos

| `kind` | `proposed` | Valores aceitos |
|---|---|---|
| `priority` | `{ "priority": "P1", "before"?: n, "after"?: n }` | `P0`–`P3` ou `null`; `before`/`after` = número de outro item da mesma classe no mesmo projeto, para posicionar |
| `status` | `{ "status": "pausado" }` | `backlog`, `bloqueado`, `pausado`, `concluido` |

Os status automáticos (`em_execucao`, `aguardando_voce`, `em_revisao`) ficam de fora: quem os define são os gatilhos de sessão e de PR (`next_status`), e uma proposta deles seria desfeita no próximo evento.

Tabela nova no `hub.db`:

```
proposal  id              INTEGER PRIMARY KEY
          uid             TEXT NOT NULL UNIQUE
          kind            TEXT NOT NULL CHECK (kind IN ('priority', 'status'))
          project_id      INTEGER NOT NULL REFERENCES project(id)
          gh_number       INTEGER NOT NULL           -- alvo (owner/repo#n)
          current_value   TEXT NOT NULL              -- JSON, lido pelo hub ao criar
          proposed_value  TEXT NOT NULL              -- JSON
          justification   TEXT NOT NULL              -- 1..2000 caracteres
          state           TEXT NOT NULL DEFAULT 'pendente'
                          CHECK (state IN ('pendente', 'aprovada', 'rejeitada', 'obsoleta'))
          env_id          TEXT NOT NULL              -- ambiente do token de agente
          session_id      TEXT                       -- sessão do gestor
          created_at      INTEGER NOT NULL
          decided_at      INTEGER
          decision_note   TEXT                       -- nota do usuário, ou o motivo da obsolescência
          applied_event_uid TEXT                     -- evento que aplicou a mudança
CREATE INDEX proposal_target_idx ON proposal(project_id, gh_number, kind, state);
```

Triggers (o mesmo padrão de `decision_immutable` e `decision_review_once`): `proposal_immutable` impede alterar alvo, valores, justificativa e origem; `proposal_decided_once` impede mudar `state` depois de sair de `pendente`. A exclusão de projeto apaga as propostas dele com o histórico (spec do hub §3.2).

### 8.2 Criação

`POST /agent/v1/proposals` (via `portfolio_propose`):

1. O alvo precisa ser um work item do hub ou uma issue aberta no espelho (`issue_cache`) de um projeto vivo e não sentinela.
2. O hub lê o valor atual (`current_value`) na transação. Proposta igual ao atual é recusada (`nada a mudar`).
3. Uma proposta pendente do mesmo `kind` e alvo vira `obsoleta` com `decision_note = "substituída pela proposta #N"`.
4. Limite de 100 pendentes no hub. Acima disso, a criação é recusada com `muitas propostas pendentes: peça ao usuário para revisar as que existem`.
5. Evento `proposal_created { proposal_id, kind, proposed }` no projeto do alvo, com o `env_id` do token (`work_item_id` nulo se a issue ainda não tem work item).
6. Push `hub.proposals.changed` para as UIs conectadas.

### 8.3 Ciclo

```
pendente ──aprovar (UI)──▶ aprovada   (o hub aplica na mesma transação)
    │ ────rejeitar (UI)──▶ rejeitada
    └─────(hub)──────────▶ obsoleta   (valor atual mudou, substituída ou expirada)
```

`obsoleta` vem do hub, nunca do agente nem da UI:

- **O valor atual mudou:** na mesma transação de toda mudança no alvo (`set_priority`, `move` e `recompute_status`, que cobre status manual, gatilhos dos ambientes e o fechamento da issue no GitHub), as pendentes do alvo cujo `current_value` não vale mais viram `obsoleta`, com a nota `valor atual mudou para X`.
- **Substituída:** passo 3 da §8.2.
- **Expirada:** pendente há mais de 14 dias, verificada na tarefa periódica do sync do GitHub do `cthubd`.

### 8.4 Aprovação aplicada pelo hub

Métodos novos no `/ws` do hub (token de UI):

```
hub.proposals.list   { state?, owner?, repo?, limit? }
hub.proposals.decide { id, approve: bool, note? }
```

`hub.proposals.decide` roda numa transação `IMMEDIATE`:

1. A proposta precisa estar `pendente`; senão, erro com o estado atual.
2. O hub relê o valor atual. Se mudou, grava `obsoleta` e devolve `{ state: "obsoleta" }`. A UI mostra o motivo.
3. **Aprovar:** aplica pelo mesmo código da UI, com `cause: "proposal"` e `proposal_id`: `set_priority` (e `move`, se veio posição; âncora que mudou de classe = fim da classe) ou `set_status_manual`. Grava `aprovada`, `decided_at` e `applied_event_uid`.
4. **Rejeitar:** grava `rejeitada` e a nota.
5. Evento `proposal_decided { proposal_id, state, note? }` com `env_id = hub_id`, e push `hub.proposals.changed`.

O agente não tem como aprovar: nenhuma ferramenta MCP nem rota `/agent/v1/*` decide proposta, e `hub.proposals.decide` só existe no `/ws` do hub, com o token de UI.

Status aprovado no hub segue a regra de hoje: vale até o próximo evento de status mais novo (spec do ledger §4). O ambiente não fica sabendo (limitação já aceita), e uma sessão nova no item volta para `em_execucao`. `concluido` aprovado não fecha a issue no GitHub.

Alternativas descartadas: o agente aplicar direto e o usuário desfazer depois (contraria o épico); aprovar com valor editado na UI (o usuário rejeita e muda à mão, e o histórico fica claro).

## 9. Issues novas no GitHub

- O gestor propõe na conversa título, corpo, labels e épico pai, e só cria depois do ok do usuário naquela conversa, com `gh issue create -R owner/repo` (e `gh sub-issue`/API de sub-issues para ligar ao épico, como a sessão de planejamento).
- Um ok pode cobrir uma lista, se ela foi mostrada por inteiro antes.
- Depois de criar, registra uma decisão `orientada` com os links.
- O app (hub e ambiente) continua sem escrever no GitHub. As issues novas aparecem no espelho do hub no próximo sync.
- Reforço no hook (§10.2): em sessão do gestor, `gh issue create` devolve `ask` ao Claude Code, que pede a confirmação do usuário no terminal antes de rodar.

Não existe proposta do tipo "criar issue". O épico definiu que issues novas são combinadas na conversa, e o app ficaria escrevendo no GitHub.

## 10. Prompt e limites (#219, #220)

### 10.1 Prompt inicial (editável)

Template novo `templates.gestor` na configuração da UI (`ui/src/settings/defaults.ts`, ao lado de `DEFAULT_PROJECT_TEMPLATE`), com variáveis `{{env}}`, `{{scope}}` e, no escopo de projeto, `{{owner}}` e `{{repo}}`. A UI troca o primeiro parágrafo conforme o escopo:

```
Você é o gestor de projetos do Control Tower, numa sessão sem repositório no ambiente {{env}}.
[portfólio]  Foco: o portfólio inteiro. Comece por portfolio_overview e portfolio_ranking.
[projeto]    Foco: {{owner}}/{{repo}}. Comece por portfolio_project e portfolio_ranking com owner e repo.
[saúde]      Foco: saúde e riscos. Comece por portfolio_health e aprofunde com portfolio_work_item.
Leia o hub só pelas ferramentas portfolio_*. Para ler uma issue, use `gh issue view <n> -R owner/repo`.
Mudanças de prioridade e de status: proponha com portfolio_propose, com justificativa; o usuário aprova na tela Hub.
Issues novas: proponha na conversa e crie com `gh issue create -R owner/repo` só depois do meu ok.
Consulte suas decisões anteriores com ledger_list_decisions e registre as relevantes com ledger_record_decision.
```

### 10.2 Limites (fixos)

Vão no `--append-system-prompt` (§2.3) e nas instruções do MCP, e o usuário não os edita:

1. Ler o hub só pelas `portfolio_*`. Não ler arquivos de token (`~/.config/ctower/*.token`, `~/.config/cthub/`) nem chamar o hub ou o ambiente por HTTP.
2. Prioridade e status mudam só por `portfolio_propose`. Nunca dizer que uma mudança foi aplicada antes da aprovação; conferir com `portfolio_list_proposals`.
3. Criar issue só depois do ok explícito (§9). Não editar, fechar, reabrir nem comentar issues; não criar, editar, revisar nem mesclar PRs; não mexer em labels, milestones e repositórios.
4. Não fazer commits nem push, não clonar repositórios para trabalhar neles, não iniciar sessões em ambientes.
5. Registrar com `ledger_record_decision` as recomendações aceitas pelo usuário (`orientada`) e as escolhas próprias relevantes (`autonoma`).

O hook `PreToolUse` reforça os limites quando `CTOWER_SESSION_KIND=gestor` (perfil novo do guarda-corpo em `crates/ct-core/src/guard`):

- **bloqueia:** `git commit`, `git push`; `gh pr` de escrita (`create`, `merge`, `close`, `edit`, `comment`, `review`, `ready`); `gh issue` de escrita exceto `create` (`edit`, `close`, `reopen`, `delete`, `comment`, `transfer`, `lock`, `pin`); `gh label`, `gh repo` de escrita; `gh api` com método diferente de `GET`, exceto a API de sub-issues; comandos que citam `hub.token`, `hub-agent.token`, `hook.token`, `ui.token` ou `.config/cthub`;
- **pede confirmação (`ask`):** `gh issue create` e a API de sub-issues. É um `Verdict::Ask` novo, que o `ctower hook` traduz para `permissionDecision: "ask"`. A implementação confere o suporte na versão instalada do Claude Code.

Como o guarda-corpo de hoje, é uma barreira contra erro, não contra adversário (§12).

Alternativa descartada: limites num `CLAUDE.md` gravado na pasta do gestor. O usuário poderia editá-lo sem querer, e o texto do system prompt não depende de arquivo.

## 11. UI (#220)

- **Tela Hub** (`ui/src/hub/HubScreen.tsx`): botão "Gestor de projetos" no topo. Abre um diálogo com o ambiente (só os ligados e com `agent_token`), o escopo (portfólio, projeto com seletor, saúde e riscos), modelo e modo (como `StartSessionPanel`) e o prompt pré-preenchido. Chama `session.start_gestor` no ambiente e abre o terminal da sessão. O botão sempre inicia uma sessão nova; ao lado dele, cada sessão do gestor viva nos ambientes conectados tem uma ação "Abrir gestor · <ambiente> · <escopo> · <hora de início>". Tile, aba e barra do terminal rotulam a sessão como "Gestor de projetos · <escopo>".
- **Painel de propostas** na tela Hub: pendentes com alvo (link para o item), valor atual → proposto, justificativa, ambiente, sessão e idade; botões aprovar e rejeitar com nota opcional; badge com a contagem. Abaixo, as últimas 50 decididas ou obsoletas, com o motivo. Atualiza com o push `hub.proposals.changed`.
- **Saúde e riscos** na tela Hub: lista de `hub.health.get` por severidade, com link para o item.
- **Board** (`ui/src/project/Board.tsx`) e árvore: badge `P0`–`P3` nos cards; dentro de cada coluna, os cards seguem a ordem da §4.3. O Board continua sem arrastar e soltar. Prioridade e "subir/descer" ficam no menu do painel do item (`WorkItemPanel`), como o status.
- **Projetos** (`HubProjects.tsx`): subir e descer na ordem entre projetos (`hub.projects.move`).
- **Sessão do gestor** no ambiente: aparece na sala de controle com o rótulo "Gestor de projetos" (sem `owner/repo`), e o painel dela oferece "Revisar decisões do gestor" (`review.get` no work item do sentinela).

## 12. Segurança e limites conhecidos

- **"O agente não aprova" é uma garantia do protocolo, não do sistema operacional.** A sessão roda com o mesmo usuário do `ctowerd`. Ela pode ler `~/.config/ctower/hub.token`, `hub-agent.token` e `hook.token`. Na instalação de máquina única (`provision.sh --hub --env`), pode ler também `~/.config/cthub/ui.token`, que abre o `/ws` do hub e `hub.proposals.decide`. O que impede é o prompt, a ausência de ferramenta e o hook, não o acesso a arquivos. Quem quiser isolamento de verdade roda o `cthubd` com outro usuário do sistema (fica documentado em `docs/hub.md`, §6).
- O token de agente só lê e propõe: o pior caso com ele vazado é a leitura do portfólio e propostas pendentes, que o usuário vê na UI.
- O gestor usa o `gh` com a autenticação do usuário no ambiente, que pode escrever no GitHub. A barreira é o hook (§10.2) e o pedido de confirmação.

## 13. Decisões

Escolhas desta spec que não estavam decididas no épico #221, para revisão do usuário:

1. **Sessão do gestor = projeto sentinela `_ctower/gestor`, work item `#0`** (§2.1). Motivo: mantém `session.work_item_id` e `decision.work_item_id` obrigatórios e reaproveita hooks, recuperação, outbox e ingestão sem mudança; `_ctower` não colide com dono do GitHub.
2. **Decisões do gestor vinculadas ao work item `_ctower/gestor#0`, com o alvo citado no texto** (§2.4). Motivo: um gestor decide sobre vários projetos ao mesmo tempo; a revisão usa o fluxo de hoje.
3. **Pasta fixa `<data_dir>/gestor` (`gestor_dir`)** (§2.2). Motivo: a retomada exige o mesmo `cwd`.
4. **Escopo só orienta o prompt; as ferramentas leem tudo** (§3). Motivo: é tudo leitura, e o usuário muda de assunto na conversa.
5. **Escala `P0`–`P3` com nulo = não triado; ordem por classe, depois projeto, depois item, com `rank` denso** (§4). Motivo: classes comparáveis entre projetos sem uma ordem global frágil; o hub é o único escritor.
6. **Prioridade só no `hub.db`, com eventos `priority_set`/`rank_moved` gravados pelo hub; definir prioridade cria o work item se faltar** (§4.2). Motivo: segue o padrão de `set_status_manual`; o ambiente não precisa da prioridade.
7. **Sinais de saúde calculados na leitura, com limiares no `config.toml` do `cthubd` (aviso no limiar, crítico no dobro)** (§5). Motivo: barato, sempre atual e ajustável sem migração.
8. **API `/agent/v1/*` no `cthubd`, repassada pelo `ctowerd`; o agente nunca recebe o token** (§6, §7). Motivo: ferramentas tipadas, respostas com limite de tamanho, token fora do alcance do prompt.
9. **Token de agente separado, um por ambiente, hash no hub, gerado por `cthubd agent-token`, sem validade, rotação por regeneração** (§6.1). Motivo: revoga o gestor sem parar o sync e mantém o padrão dos tokens atuais.
10. **Propostas só de prioridade e de status manual (`backlog`, `bloqueado`, `pausado`, `concluido`)** (§8.1). Motivo: os outros status são de gatilho e seriam desfeitos no próximo evento.
11. **`obsoleta` marcada pelo hub na transação da mudança no alvo, ao substituir e após 14 dias; releitura na aprovação** (§8.3, §8.4). Motivo: o usuário nunca aprova algo cuja premissa mudou.
12. **Limite de 100 propostas pendentes** (§8.2). Motivo: evita que um gestor em laço encha o painel.
13. **Limites fixos no `--append-system-prompt` e reforço no hook com o perfil `gestor`, incluindo `ask` para `gh issue create`** (§10.2). Motivo: o ok do usuário passa a ser confirmado pelo próprio Claude Code, e os limites sobrevivem à retomada e à compactação.
14. **Protocolo do ambiente 2.1 e do hub 1.1** (§2.3, §4.4). Motivo: só entram métodos e campos.

## 14. Próximos passos

Sub-issues do épico #221, nesta ordem (as migrações do `hub.db` usam o próximo número livre na ordem em que entrarem; hoje a próxima é a 9, e a do `ledger.db` é a 8):

1. **#215** — ct-ledger, cthubd: `priority`, `rank` e `project.rank` (§4.2); `set_priority`, `move`, `hub.projects.move`, `hub.ranking.get`; eventos; campos novos nas respostas; protocolo do hub 1.1.
2. **#216** — cthubd: `Hub::health`, `[health]` no config, `hub.health.get` (§5).
3. **#217** — cthubd, ctower: `agent_token`, `cthubd agent-token|revoke-agent-token`, `ctower hub set-agent-token`, rotas `/agent/v1/*` de leitura (§6); `provision.sh`; `docs/hub.md`.
4. **#218** — ct-ledger, cthubd: tabela `proposal` e triggers, `POST /agent/v1/proposals`, obsolescência, `hub.proposals.list|decide` e o push `hub.proposals.changed` (§8).
5. **#219** — ct-ledger, ct-sessions, ct-server, ct-core: `project.kind` e o sentinela nos dois bancos, `gestor_dir`, `session.start_gestor`, ferramentas `portfolio_*` e instruções do MCP, perfil `gestor` do guarda-corpo com `Verdict::Ask` (§2, §7, §10.2); protocolo do ambiente 2.1.
6. **#220** — ui: botão e diálogo do gestor, painel de propostas, saúde e riscos na tela Hub, prioridade no Board e na árvore, ordem entre projetos, template `gestor` (§10.1, §11).
