# Control Tower — Plano 3: Sincronização com o GitHub — Implementation Plan

> **For agentic workers:** execute task a task, em TDD (teste falhando → implementação → teste passando → commit). Cada task corresponde a uma sub-issue do épico no GitHub; referencie a issue no commit (`refs #N`).

**Goal:** Espelhar no ledger as issues, sub-issues e PRs dos projetos cadastrados (somente leitura, com ETag), mover os work items para `em_revisao`/`concluido` quando o PR da branch é aberto/mesclado e expor esses dados à UI (`sync.now`, `tree.get`, `review.get`, `review.mark_reviewed`).

**Architecture:** Novo crate `ct-gh` (síncrono, `ureq` com TLS) com o cliente HTTP e a busca de um projeto. A busca roda **sem** o lock do ledger e produz um `GithubSnapshot`; o ledger aplica o snapshot numa única transação (`apply_github`), que também dispara os gatilhos de PR e grava os ETags. A orquestração (`GithubSync`) fica em `ct-sessions`, que já é dono do lock do ledger e do `Notifier`; o `ctowerd` roda o laço periódico com backoff e o `ct-server` expõe os métodos novos.

**Tech Stack:** o dos Planos 1 e 2 + `ureq` 2.12 com features `tls` e `json` (rustls 0.23 + ring, verificado na distro: compila e recebe `304` da API real). Token via `gh auth token`.

**Épico:** #18 (sub-issues #19–#24)

**Spec:** `docs/superpowers/specs/2026-10-15-control-tower-design.md` — seções 7.5, 7.6 (dados), 9 (métodos `sync.now`, `tree.get`, `review.*`) e 11 (token do GitHub nunca armazenado).

## Verificado contra a API do GitHub (2026-10-15)

- `GET /repos/{o}/{r}/issues` traz, em cada issue, `parent_issue_url` (pai, quando é sub-issue) e `sub_issues_summary.total` (filhas). **Não é preciso uma chamada por issue** para montar a árvore.
- A listagem de issues inclui PRs (campo `pull_request`); eles são descartados.
- `GET /repos/{o}/{r}/issues/{n}/sub_issues` lista as filhas de um épico (inclusive as fechadas há mais de 30 dias, que a listagem principal não traz).
- `GET /repos/{o}/{r}/pulls?state=all&head={o}:{branch}` traz os PRs da branch; `merged_at` distingue mesclado de fechado.
- Respostas trazem `ETag`; repetir com `If-None-Match` devolve `304` (o `ureq` trata como sucesso), e respostas `304` autenticadas não consomem rate limit.
- Paginação por cabeçalho `Link: <url>; rel="next"` (URL absoluta).

## Decisões tomadas neste plano

1. **Cliente HTTP próprio (`ureq` + rustls), não `gh api`.** Testável contra um servidor falso local (`api_url` configurável), sem depender do formato de saída do `gh`. O `gh` só fornece o token (`gh auth token`, prazo de 5 s), guardado em memória; `401` descarta o token e tenta de novo uma vez.
2. **Duas fases.** `fetch` (rede, sem lock) → `Ledger::apply_github` (uma transação: caches, gatilhos, eventos, ETags, estado do sync). Nunca se segura o lock do ledger durante a rede.
3. **Gatilhos de PR só na transição.** `PrOpened` dispara quando o PR passa a `open` (novo ou reaberto); `PrMerged` quando passa a `merged`. Se o humano mudar o status manualmente com o PR já aberto, o próximo ciclo não o desfaz (spec §6: "manuais prevalecem até o próximo gatilho de natureza diferente").
4. **Quais PRs:** os da branch de cada work item com `branch` definida e status ≠ `concluido`, qualquer base (o PR de sub-issue para a `wip/*` do épico conta para a sub-issue).
5. **Janela de issues:** duas consultas — `state=open` e `state=closed&since=<dia UTC de 30 dias atrás, 00:00:00Z>`. O `since` é arredondado ao dia para a URL (e o ETag) ficarem estáveis entre ciclos.
6. **ETag por URL da primeira página.** `304` na primeira página = consulta inteira inalterada (nada é reprocessado). O ETag só é gravado na mesma transação que aplica os dados.
7. **Issue fechada sem PR mesclado** (transição `open` → `closed` no cache, com work item não concluído): evento `issue_closed` no work item; status inalterado.
8. **Erros:** falha de um projeto não impede os outros; grava `last_error` em `sync_state`, mantém o cache e o laço aplica backoff exponencial (`intervalo × 2^falhas`, teto 15 min). Rate limit (`403`/`429` com `x-ratelimit-remaining: 0` ou `retry-after`) é erro próprio.
9. **Segurança:** o token só é enviado para URLs que começam com `api_url` (inclusive as do `Link`).
10. **Configuração** (`config.toml`): `gh_sync_interval_secs = 180` (`0` desliga o laço; `sync.now` continua funcionando), `gh_api_url = "https://api.github.com"`, `gh_bin = "gh"`.

## Global Constraints

- Todas as dos Planos 1 e 2 (PT-BR, Unix apenas, status/estados como strings exatas, `event` append-only, nunca commitar em `develop`, CI com `fmt`, `clippy -D warnings --locked` e testes em Ubuntu e macOS).
- O app **nunca escreve** no GitHub (spec §2): só `GET`.
- Nenhum token do GitHub em disco nem em log.
- Testes não acessam a rede: usam o `FakeGitHub` (servidor HTTP local) e um `gh` falso. O smoke real é `#[ignore]`.
- GitHub indisponível nunca bloqueia sessões (spec §12).

## Review Focus

1. **Gatilho reaplicado a cada ciclo** — com o PR aberto, o humano põe o work item em `pausado`; os ciclos seguintes não podem voltar para `em_revisao`. Teste na Task 1 (`apply_github`) e na Task 3.
2. **ETag gravado sem dados aplicados** — se a aplicação falhar, o próximo ciclo não pode receber `304` e perder a mudança. Teste na Task 1 (transação única).
3. **Token vazando para outro host** — `Link` apontando para fora de `api_url` não é seguido. Teste na Task 2.
4. **Lock do ledger durante a rede** — um `FakeGitHub` lento não pode travar `dashboard.get`. Teste na Task 4.
5. **Sync concorrente** — `sync.now` durante o ciclo periódico não aplica o mesmo snapshot duas vezes nem gera eventos duplicados. Teste na Task 4.

---

## File Structure

```
crates/
├─ ct-core/src/model.rs           # + PrState (open|closed|merged), pr_trigger(prev, new)
├─ ct-ledger/src/
│  ├─ schema.rs                   # + migração 2: pr_cache, gh_etag, sync_state
│  ├─ github.rs                   # IssueRow, PrRow, GithubSnapshot, apply_github, leituras
│  └─ tree.rs                     # project_tree (épico → sub-issues + work items)
├─ ct-gh/src/
│  ├─ lib.rs                      # GhConfig, GhClient, GhError, Fetched
│  ├─ time.rs                     # RFC 3339 ⇄ ms, início do dia UTC
│  ├─ sync.rs                     # SyncInput, fetch → GithubSnapshot, caminhos das consultas
│  └─ fake.rs                     # FakeGitHub + fake_gh (feature test-support)
├─ ct-sessions/src/github.rs      # GithubSync: sync_all/sync_project, notificações
├─ ct-sessions/src/config.rs      # + gh_sync_interval_secs, gh_api_url, gh_bin
├─ ct-server/src/ws.rs            # + sync.now, tree.get, review.get, review.mark_reviewed
├─ ct-server/src/mcp.rs           # ledger_get_context com issue e épico pai
├─ ctowerd/src/main.rs            # GithubSync + laço com backoff
├─ ctowerd/tests/github_sync.rs   # ponta a ponta com FakeGitHub
└─ ct-gh/tests/smoke_github.rs    # #[ignore], GitHub real, somente leitura
```

---

### Task 1: `ct-ledger` — migração 2 e API do GitHub (#19)

**Files:** `ct-core/src/model.rs`, `ct-ledger/src/{schema.rs,github.rs,tree.rs,lib.rs}`.

**Migração 2** (acrescentada ao fim de `MIGRATIONS`):
```sql
CREATE TABLE pr_cache (project_id INTEGER NOT NULL REFERENCES project(id), number INTEGER NOT NULL,
  work_item_id INTEGER NOT NULL REFERENCES work_item(id), head TEXT NOT NULL, base TEXT NOT NULL,
  state TEXT NOT NULL, url TEXT NOT NULL, updated_at INTEGER NOT NULL, synced_at INTEGER NOT NULL,
  PRIMARY KEY (project_id, number));
CREATE INDEX pr_cache_work_item_idx ON pr_cache(work_item_id);
CREATE TABLE gh_etag (key TEXT PRIMARY KEY, etag TEXT NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE sync_state (project_id INTEGER PRIMARY KEY REFERENCES project(id),
  last_attempt_at INTEGER NOT NULL, last_ok_at INTEGER, last_error TEXT);
```

**Interfaces:**
```rust
// ct-core
str_enum!(PrState, "pr_state", { Open => "open", Closed => "closed", Merged => "merged" });
/// Gatilho de uma transição de PR (None = sem transição relevante).
pub fn pr_trigger(prev: Option<PrState>, new: PrState) -> Option<WorkItemTrigger>;
// Open com prev ≠ Some(Open) → PrOpened; Merged com prev ≠ Some(Merged) → PrMerged; resto → None.

// ct-ledger::github
pub struct IssueRow { pub gh_number: i64, pub title: String, pub state: String /* open|closed */,
    pub labels: Vec<String>, pub parent_number: Option<i64>, pub has_sub_issues: bool,
    pub url: String, pub updated_at: i64 }                     // Serialize, Deserialize, Clone
pub struct PrRow { pub number: i64, pub work_item_id: i64, pub head: String, pub base: String,
    pub state: PrState, pub url: String, pub updated_at: i64 }
pub struct GithubSnapshot { pub project_id: i64, pub issues: Vec<IssueRow>, pub prs: Vec<PrRow>,
    pub etags: Vec<(String, String)>, pub synced_at: i64 }
pub struct GithubApplied { pub issues: usize, pub prs: usize,
    pub status_changes: Vec<(i64, WorkItemStatus)>, pub issues_closed: Vec<i64> }
pub struct SyncState { pub project_id: i64, pub last_attempt_at: i64,
    pub last_ok_at: Option<i64>, pub last_error: Option<String> }   // Serialize
impl Ledger {
    pub fn apply_github(&self, s: &GithubSnapshot) -> Result<GithubApplied>;   // uma transação
    pub fn record_sync_error(&self, project_id: i64, error: &str) -> Result<()>;
    pub fn sync_states(&self) -> Result<Vec<SyncState>>;
    pub fn list_issues(&self, project_id: i64) -> Result<Vec<IssueRow>>;
    pub fn get_issue(&self, project_id: i64, gh_number: i64) -> Result<Option<IssueRow>>;
    pub fn prs_for_work_item(&self, work_item_id: i64) -> Result<Vec<PrRow>>;
    pub fn etag(&self, key: &str) -> Result<Option<String>>;
}
// ct-ledger::tree
pub struct TreeNode { pub issue: IssueRow, pub work_item: Option<WorkItem>, pub children: Vec<TreeNode> }
pub struct ProjectTree { pub project: Project, pub sync: Option<SyncState>,
    pub roots: Vec<TreeNode>, pub untracked: Vec<WorkItem> }   // untracked = work items sem issue no cache
pub fn project_tree(&self, project_id: i64) -> Result<ProjectTree>;   // método de Ledger
```

`apply_github` numa única transação (sem chamar métodos que abrem transação própria — extrair `set_status_tx` privado de `set_work_item_status`):
1. Para cada issue: lê o estado anterior, faz upsert (`synced_at`); se `open → closed`, há work item não concluído e nenhum PR `merged` dele no cache → evento `issue_closed {gh_number}`.
2. Para cada PR: lê o estado anterior, faz upsert; `pr_trigger(prev, new)` → `next_status` → `set_status_tx` (evento `status_changed` com `cause` do gatilho).
3. Grava os ETags; `sync_state` com `last_attempt_at = last_ok_at = synced_at`, `last_error = NULL`.

**Testes:** migração 2 sobre banco na versão 1 (dados preservados); `pr_trigger` (tabela de casos); snapshot com PR novo aberto → `em_revisao`; reaplicar o mesmo snapshot → nenhuma mudança nem evento novo; work item `pausado` + PR ainda aberto → continua `pausado`; PR `merged` → `concluido` com `closed_at`; issue `open → closed` sem PR → `issue_closed`, status igual; falha no meio (PR com `work_item_id` inexistente → FK) → nada gravado, inclusive ETag; `record_sync_error` preserva `last_ok_at`; `project_tree` monta épico → filhas, filha cujo pai não está no cache vira raiz, work item sem issue em `untracked`.

- [ ] TDD → `cargo clippy -D warnings` → commit `feat(ledger): cache do GitHub, gatilhos de PR e árvore do projeto (refs #19)`.

### Task 2: `ct-gh` — cliente HTTP com ETag (#20)

**Files:** `crates/ct-gh/{Cargo.toml,src/lib.rs,src/time.rs,src/fake.rs}`; workspace `Cargo.toml` (`ct-gh` como dependência de workspace).

**Interfaces:**
```rust
pub struct GhConfig { pub api_url: String, pub gh_bin: String, pub timeout: Duration }
pub enum Fetched<T> { NotModified, Modified { body: T, etag: Option<String> } }
#[derive(Debug, thiserror::Error)]
pub enum GhError { Token(String), Network(String), Http { status: u16, message: String },
                   RateLimited { reset_at: Option<i64> }, Parse(String) }
pub struct GhClient;   // Send + Sync; token em Mutex<Option<String>>
impl GhClient {
    pub fn new(cfg: GhConfig) -> Self;
    /// GET `path` (relativo a api_url, com query). Segue `Link: rel="next"` e concatena os
    /// arrays; o ETag é o da primeira página e só ela recebe `If-None-Match`.
    pub fn get_all(&self, path: &str, etag: Option<&str>) -> Result<Fetched<Vec<Value>>, GhError>;
}
// time.rs
pub fn parse_rfc3339(s: &str) -> Option<i64>;          // "2026-10-15T22:47:50Z" → ms
pub fn day_start_utc(ms: i64) -> String;               // → "2026-09-15T00:00:00Z"
// fake.rs (#[cfg(any(test, feature = "test-support"))])
pub struct FakeGitHub;   // servidor HTTP/1.1 mínimo em 127.0.0.1:0, thread própria
impl FakeGitHub {
    pub fn start(token: &str) -> Self;
    pub fn url(&self) -> String;
    pub fn set(&self, path: &str, body: Value);                  // muda o ETag se o corpo mudou
    pub fn set_pages(&self, path: &str, pages: Vec<Value>);      // Link rel="next" entre páginas
    pub fn fail_next(&self, path: &str, status: u16, headers: &[(&str, &str)]);
    pub fn set_delay(&self, d: Duration);
    pub fn requests(&self) -> Vec<(String, Option<String>)>;     // (path, If-None-Match)
}
pub fn fake_gh(dir: &Path, token: &str) -> PathBuf;              // script que imprime o token
```
Cabeçalhos: `Authorization: Bearer`, `Accept: application/vnd.github+json`, `X-GitHub-Api-Version: 2022-11-28`, `User-Agent: ctowerd/<versão>`. O `FakeGitHub` responde `401` sem o token certo e `404` para caminho desconhecido.

**Testes** (contra o `FakeGitHub`): `200` com ETag → `Modified`; repetição com o ETag → `NotModified` e cabeçalho enviado; três páginas concatenadas; `Link` para outro host → `GhError::Http`/erro explícito sem enviar token; `401` uma vez → renova o token (o `gh` falso é chamado 2×) e sucede; `401` persistente → `Http{401}`; `403` com `x-ratelimit-remaining: 0` e `x-ratelimit-reset` → `RateLimited{reset_at}`; `gh` inexistente → `Token`; servidor fora do ar → `Network`; `time.rs` com casos de virada de mês/ano bissexto.

- [ ] TDD → commit `feat(gh): cliente HTTP do GitHub com ETag e paginação (refs #20)`.

### Task 3: `ct-gh` — busca de um projeto (#21)

**Files:** `crates/ct-gh/src/sync.rs`.

**Interfaces:**
```rust
pub struct SyncInput { pub project: Project, pub work_items: Vec<WorkItem> /* com branch, ≠ concluido */,
    pub open_epics: Vec<i64> /* do cache */, pub etags: HashMap<String, String> }
impl SyncInput { pub fn load(l: &Ledger, project_id: i64) -> ct_ledger::Result<Self>; }
pub fn issues_path(owner: &str, repo: &str, state: &str, since: Option<&str>) -> String;
pub fn sub_issues_path(owner: &str, repo: &str, number: i64) -> String;
pub fn pulls_path(owner: &str, repo: &str, branch: &str) -> String;   // head percent-encoded
pub fn fetch(api: &GhClient, input: &SyncInput, now_ms: i64) -> Result<GithubSnapshot, GhError>;
pub fn issue_row(v: &Value) -> Option<IssueRow>;    // None para PRs
pub fn pr_row(v: &Value, work_item_id: i64) -> Option<PrRow>;
```
`fetch`: (1) `issues_path(open)` e `issues_path(closed, since)`; (2) épicos = `open_epics` ∪ issues abertas com `has_sub_issues` recém-buscadas → `sub_issues_path` de cada (filhas com `parent_number` = épico); (3) `pulls_path` de cada work item. Cada consulta usa o ETag de `input.etags` e devolve o novo em `snapshot.etags`. Issue repetida entre listas: vale a última, e `parent_number` vindo de `sub_issues` prevalece. Caminhos com `per_page=100`.

**Testes** (`FakeGitHub` + ledger em memória, ponta a ponta `fetch` → `apply_github`): issues abertas, fechadas e PR na listagem (descartado); épico com filha fechada só visível em `sub_issues`; `parent_issue_url`/`sub_issues_summary` mapeados; segunda rodada inteira em `304` → snapshot vazio, nenhum evento; PR aberto e depois mesclado → `em_revisao` → `concluido`; branch com `/` codificada corretamente no `head`; erro numa consulta → `Err` e nada aplicado.

- [ ] TDD → commit `feat(gh): sincronização de issues, sub-issues e PRs de um projeto (refs #21)`.

### Task 4: `ct-sessions` + `ctowerd` — orquestração e laço (#22)

**Files:** `ct-sessions/{Cargo.toml,src/github.rs,src/config.rs,src/lib.rs}`, `ctowerd/src/main.rs`.

**Interfaces:**
```rust
#[derive(Serialize)]
pub struct ProjectSync { pub project_id: i64, pub ok: bool, pub error: Option<String>,
    pub issues: usize, pub prs: usize, pub status_changes: usize }
pub struct GithubSync;   // Sessions + GhClient + Mutex<()> que serializa execuções
impl GithubSync {
    pub fn new(sessions: Sessions, client: GhClient) -> Self;
    pub fn sync_all(&self) -> Result<Vec<ProjectSync>>;
    pub fn sync_project(&self, project_id: i64) -> Result<ProjectSync>;
}
// DaemonConfig: + gh_sync_interval_secs: u64 (180), gh_api_url: String, gh_bin: String
```
Por projeto: `SyncInput::load` (lock curto) → `fetch` (sem lock) → `apply_github` (lock curto) → notifica `work_item_status {work_item_id, status}` por mudança e `sync_status {project_id, ok, last_ok_at, error}`. Erro do GitHub → `record_sync_error` + `sync_status` com `ok: false` (não é `Err` de `sync_all`).

`ctowerd`: cria `GithubSync` (sempre) e, se `gh_sync_interval_secs > 0`, o laço: primeira execução logo após o boot; depois espera `intervalo × 2^falhas` (teto 900 s), com `falhas` zerado quando todos os projetos sincronizam.

**Testes:** config com e sem os campos novos; `sync_all` com dois projetos, um com erro (`404`) → o outro sincroniza e o erro fica em `sync_states`; notificações emitidas; `FakeGitHub` com atraso de 2 s → `sessions.ledger()` obtido em < 200 ms durante o sync; duas threads chamando `sync_all` ao mesmo tempo com PR recém-aberto → um único `status_changed`; função pura `next_delay(interval, failures)` (1×, 2×, 4×, teto).

- [ ] TDD → commit `feat(sessions): orquestração do sync do GitHub e laço com backoff (refs #22)`.

### Task 5: `ct-server` — métodos da UI e contexto do épico (#23)

**Files:** `ct-server/src/{lib.rs,ws.rs,mcp.rs}`, `ct-server/Cargo.toml`.

- `AppState` ganha `pub github: Option<Arc<GithubSync>>`.
- `sync.now {project_id?}` → `[ProjectSync]` (`None` → erro `-32000` "sincronização com o GitHub desativada").
- `tree.get {project_id}` → `ProjectTree`.
- `review.get {work_item_id}` → `{ work_item, prs, decisions }` com as decisões `autonoma` não revisadas de `epic_work_items` (o próprio work item e as sub-issues com work item).
- `review.mark_reviewed {work_item_id, pr}` → marca essas decisões → `{ marked }`.
- `session.start`: `title`, `labels` e `kind` passam a ser opcionais; ausentes, vêm do `issue_cache` (`kind = epic` se `has_sub_issues`). Sem cache e sem `title` → `-32602`.
- `dashboard.get` ganha `sync: [SyncState]`.
- MCP `ledger_get_context` ganha `issue: {title, url, state}` e `parent_epic: <número>` quando houver cache.

**Testes:** cada método (com `GithubSync` apontando para um `FakeGitHub`); `sync.now` sem GitHub configurado; `session.start` só com `project_id`, `gh_number` e `prompt` usando o cache; `review.mark_reviewed` não toca decisões `orientada` nem de outros work items; `ledger_get_context` de uma sub-issue informa o épico.

- [ ] TDD → commit `feat(server): sync.now, tree.get, review e contexto do épico (refs #23)`.

### Task 6: ponta a ponta, smoke real e documentação (#24)

- `ctowerd/tests/github_sync.rs`: `ctowerd` real com `gh_api_url` = `FakeGitHub`, `gh_bin` = `gh` falso e `gh_sync_interval_secs = 1`; cadastra projeto, inicia sessão na issue #1 (`claude` falso), publica PR aberto da branch → push `work_item_status` `em_revisao`; marca mesclado → `concluido`; `tree.get` mostra a issue com o work item.
- `ct-gh/tests/smoke_github.rs` (`#[ignore]`): `GhClient` real contra `deep-ion-ai/ctower-agents` — épico #4 com as filhas #5–#13, PR #15 (`feat/plano-2-sessoes-servidor`) `merged`, segunda chamada `304`. Rodar: `cargo test -p ct-gh --test smoke_github -- --ignored --nocapture`.
- README (seção do daemon: sync com o GitHub e campos novos do `config.toml`) e roadmap (Plano 3 concluído).

- [ ] TDD → commit `test(ctowerd): sync do GitHub ponta a ponta e smoke real (refs #24)`.

---

## Fora deste plano

UI Tauri, telas de árvore/revisão e notificações nativas (Plano 4). Escrita no GitHub (fora de escopo da spec). Webhooks do GitHub (o daemon fica atrás de `127.0.0.1`; polling com ETag basta).
