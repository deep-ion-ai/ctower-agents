# Ledger centralizado — base compartilhada entre ambientes (brainstorm da issue #56)

> **Revisada em 2026-10-18 (épico #183):** o hub deixou de ser um `ctowerd` com `hub = true` e virou um binário próprio (`cthubd`), origem de projetos, configuração e atribuições. As §2, §4 (atributos de projeto), §5, §6, §7, §8, §9 e §11 foram substituídas, no todo ou em parte, por `2026-10-18-hub-plano-de-controle.md`. Cada seção afetada tem uma nota.

Resultado do brainstorm da issue #56. Revisa a spec principal (`2026-10-15-control-tower-design.md`): §3 "Ledger por daemon", §5 (modelo de dados), §8 (MCP), §9 (protocolo) e §11 (segurança). As decisões estão registradas no ledger do work item #56.

## 1. Problema e direção

Hoje cada `ctowerd` tem o seu `ledger.db`. Por isso o mesmo repositório em dois ambientes fica com histórico separado, e a perda de um ambiente (ex.: a distro WSL) apaga o histórico dele.

O desenho atende **várias máquinas físicas** (PC com WSL, Mac mini, notebook fora de casa), com rede que pode cair. Direção escolhida: **local-first com hub**.

- Cada daemon continua gravando no próprio SQLite antes de agir (spec §1, "gravar antes de agir").
- Um `ctowerd` com papel de **hub** recebe o que cada ambiente produz e guarda a **visão consolidada**. Só o hub tem essa visão.
- Os ambientes **não** replicam de volta a visão completa: cada um guarda só o que produziu.

```
  ctowerd (WSL, PC)          ctowerd (notebook)         ctowerd hub (Mac mini)
  ledger.db (local)          ledger.db (local)          ledger.db (local) + hub.db (consolidado)
   └─ sync_outbox ──POST /sync/push──┐                        ▲      │
                     sync_outbox ────┴──── Tailscale ─────────┘      └─ snapshots (VACUUM INTO)
                                                                  ▲
  UI ── /ws (sessões, terminal, recuperação) ──► cada daemon      │
     └─ /ws hub.* (árvore, histórico, decisões, dashboard) ───────┘
```

## 2. O que é compartilhado

> **Substituída em parte** (`2026-10-18-hub-plano-de-controle.md` §2, §3 e §6): `project` nasce no hub e chega ao ambiente como cache somente leitura; `project_location` é só o relato do ambiente, e a intenção fica em `assignment`; o ambiente poda os work items inativos depois do ack, e o hub vira a única cópia do histórico.

| Dado | Onde vive | Observação |
|---|---|---|
| `project` | local + hub | No hub, a chave global é `owner/repo`, sem diferenciar maiúsculas. `repo_path` vale por ambiente (`project_location`). |
| `work_item` | local + hub | A chave global é (projeto, `gh_number`). `branch` é global. `worktree_path` vale por ambiente (`work_item_location`). O status no hub é derivado (§4). |
| `session` | local + hub | Só o ambiente dono altera a sessão (processo, PTY, transcript). O hub guarda uma cópia com `env_id` para mostrar onde cada sessão roda. |
| `decision` | local + hub | Só recebe inclusões, com `uid`. A revisão (`reviewed_*`) é replicada: vale a primeira (trigger `decision_review_once`). |
| `event` | local + hub | Só recebe inclusões, com `uid`, `env_id` e `hlc`. |
| `issue_cache`, `pr_cache`, `gh_etag`, `sync_state` | **local** | Espelho do GitHub, não replicado. O hub sincroniza **todos os projetos da base central**, mesmo sem clone na máquina dele. |
| PTYs, transcripts, backups de transcript, spool de hooks, worktrees | **local** | O daemon do ambiente continua dono deles. |

## 3. Identidade

- **`env_id`:** UUID gerado no primeiro boot do daemon e guardado em `<config_dir>/env_id`. O nome do ambiente vem do config.
- **`uid`:** UUIDv7 para `decision` e `event`. `session.id` já é UUID.
- **Eventos derivados do GitHub** recebem um `uid` determinístico, por exemplo `gh:pr:<owner>/<repo>#<n>:merged`. Se dois ambientes perceberem o mesmo PR mesclado, o hub guarda um só evento.
- Os ids `INTEGER` continuam valendo só dentro de cada banco. Entre ambientes, as referências usam chave natural ou `uid`. Por isso a UI inicia uma sessão num ambiente informando (`owner/repo`, número), nunca o id do hub.

## 4. Conflitos

> **Substituída em parte** (nova spec §5): a regra "projeto com atributos divergentes: vale o mais recente" saiu. Atributos de projeto só mudam no hub.

- **Status do work item:** no hub, o status é o do evento de status mais recente pelo **horário de origem**. Cada daemon mantém um relógio lógico híbrido (HLC) persistido e grava o `hlc` em cada evento. O desempate é por `env_id`. Um evento que chega atrasado, vindo de um ambiente que estava offline, não sobrescreve um status mais novo, mas fica no histórico.
- **Projeto com atributos divergentes** (`base_branch`, protegidas, nome): vale o valor mais recente.
- **Sessão:** não há conflito, porque só o ambiente dono a altera. O hub aplica a cópia com `last_event_at` maior.
- **Limitação aceita:** o status local de um ambiente não fica sabendo de mudanças feitas em outro. As automações locais (spec §6) usam o status local. A visão que vale para o humano é a do hub.

## 5. Replicação

> **Substituída em parte** (nova spec §5 e §6): não há mais envio "em processo" (o hub não é ambiente); o `ctower hub resync` só reenvia o que não foi podado. O outbox e o `POST /sync/push` continuam.

- **Outbox transacional:** toda mudança replicável grava uma linha em `sync_outbox (seq, kind, key, payload, created_at)` **na mesma transação** da mudança.
- **Envio:** uma tarefa do daemon manda lotes em `POST <hub_url>/sync/push`. O hub aplica cada lote de forma idempotente (upsert por `uid` ou chave natural) e devolve o último `seq` aplicado. O ambiente apaga do outbox o que foi confirmado.
- **Falha:** quando o hub está inacessível, o outbox cresce e o envio tenta de novo com backoff exponencial (até 5 min). Nada bloqueia as sessões. A UI mostra o atraso ("hub: N pendentes, último envio há X").
- **O próprio hub** também é um ambiente: ele envia pelo mesmo caminho, em processo.
- **`hub.db`** é um arquivo separado do `ledger.db`, com esquema próprio (`env`, `project`, `project_location`, `work_item`, `work_item_location`, `session`, `decision`, `event`). Os triggers de "só inclusão" de `event` e `decision` continuam valendo nele.
- **Resync:** `ctower hub resync` (no ambiente) coloca de novo no outbox tudo o que o ambiente tem. Serve para reconstruir um hub perdido ou trocado.

## 6. Hub fora do ar

> **Complementada** (nova spec §4.3): o ambiente também trabalha com a última configuração do hub em cache.

| Quem | Comportamento |
|---|---|
| Iniciar ou retomar sessão, hooks | Seguem normalmente, só local. As mudanças esperam no outbox. |
| UI | Lê de cada daemon os dados locais (como hoje) e mostra o aviso "hub indisponível: visão parcial". |
| MCP `ledger_list_decisions` (escopo `epic`/`project`) | O daemon local consulta o hub. Se falhar, devolve as decisões locais com `partial: true` e o motivo. Nunca trava o agente. |
| `ledger_record_decision`, `ledger_set_status` | Gravam local (outbox). |

## 7. Protocolo e UI

> **Substituída** (nova spec §8): `env.info` perde `hub: bool`; os `hub.*` saem do `ctowerd` e ficam no `/ws` do `cthubd`; a UI tem uma conexão própria com o hub.

- **`env.info`** passa a devolver `env_id` e `hub: bool`.
- **Novos métodos no `/ws` do hub** (só no daemon com `hub = true`): `hub.projects.list`, `hub.tree.get { owner, repo }`, `hub.workitem.history { owner, repo, number }`, `hub.dashboard.get`, `hub.envs.list` (ambientes, último contato, pendências).
- A UI identifica o ambiente hub pelo `env.info` e pede a ele a visão consolidada (árvore, `summary`, histórico, decisões). Terminal, `attach`, sessões vivas e recuperação continuam em cada daemon.
- **Épico #51 (tela Projeto):** `summary` (#46) e `workitem.history`/`set_status` (#47) passam a ser servidos a partir do `hub.db`. `workitem.set_status` feito na UI vai para o daemon do hub, que grava um evento com `hlc` como qualquer ambiente.
- **MCP local → hub:** endpoint `POST /sync/query/decisions` (token de sync) para o escopo `epic`/`project`.
- Todas as mudanças são compatíveis: só entram campos e métodos novos.

## 8. Segurança

> **Substituída em parte** (nova spec §9): `ctower hub add-env`/`revoke-env` viram `cthubd add-env`/`revoke-env`; o hub tem token de UI próprio; o token de sync também dá acesso a `GET /sync/config`.

- No hub, `ctower hub add-env <nome>` gera um **token de sync** para o ambiente. O hub guarda só o hash. O ambiente grava o token em `<config_dir>/hub.token` (`0600`) e `hub_url` no config.
- O token de sync dá acesso só a `/sync/*`. Para revogar um ambiente, basta apagar o hash no hub.
- O transporte entre máquinas é o Tailscale (WireGuard), sem TLS próprio nesta fase. O bind fora de `127.0.0.1` continua exigindo configuração explícita (spec §11).

## 9. Migração

> **Substituída** para a instalação atual pela nova spec §11 (`cthubd migrate`).

1. A migração 4 do `ledger.db` acrescenta `uid`, `env_id` e `hlc` (eventos antigos usam `ts` como HLC) e cria `sync_outbox`.
2. Na primeira conexão ao hub, o ambiente coloca todo o ledger existente no outbox.
3. O hub funde projetos por `owner/repo` e work items por (projeto, número), une decisões e eventos e recalcula o status pela regra da §4.

## 10. Backup

> **Revisada** (nova spec §6.5 e §10): com a poda, o snapshot passa a ser a defesa principal; as opções viram `backup_dir`, `backup_interval_hours` e `backup_keep` no config do `cthubd`.

- O hub gera snapshots periódicos de `hub.db` com `VACUUM INTO <backup_dir>/hub-<AAAAMMDD-HHMM>.db` e rotação. Os padrões são a cada 6 h, mantendo os 28 últimos.
- `backup_dir` é configurável e pode ficar fora da máquina do hub ou num disco sincronizado.
- Segunda linha de defesa: cada ambiente guarda os próprios dados e pode reenviá-los (`ctower hub resync`).

## 11. Configuração

> **Substituída** (nova spec §10): `hub` e `hub_backup_*` saem do `ctowerd`; o `cthubd` tem config própria em `~/.config/cthub/config.toml`.

```toml
# ~/.config/ctower/config.toml
env_name = "wsl-pc"
hub = false                       # true no daemon que é o hub
hub_url = "http://mac-mini.tailnet:7878"   # ambientes que enviam ao hub
# só no hub:
bind = "100.x.y.z"                # IP Tailscale (explícito)
hub_backup_dir = "/Volumes/Backup/ctower"
hub_backup_interval_hours = 6
hub_backup_keep = 28
```

## 12. Próximos passos

Épico **#64**, com as sub-issues:

1. #57 — ct-ledger: `env_id`, `uid`, HLC, migração 4 e `sync_outbox`;
2. #58 — ct-ledger: `hub.db`, ingestão idempotente, fusão e status por HLC;
3. #59 — ct-server: sync ambiente→hub (`/sync/push`, tokens, backoff, resync);
4. #60 — ct-gh: o hub sincroniza todos os projetos centrais; `uid` determinístico dos eventos do GitHub;
5. #61 — ct-server: métodos `hub.*` no `/ws` e MCP via hub com resposta parcial;
6. #62 — ui: ambiente hub, visão consolidada e aviso de visão parcial;
7. #63 — ctowerd: snapshots do `hub.db` com rotação e docs de configuração.

O épico #51 (tela Projeto) depende deste: #46 e #47 passam a ser servidos a partir do `hub.db`.
