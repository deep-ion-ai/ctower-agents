# Projetos com Jira (jira-cli) e GitLab, com sync no ambiente (issue #346)

Spec do épico #353. Parte da spec do hub como plano de controle (`2026-10-18-hub-plano-de-controle.md`) e da spec do gestor (`2026-10-19-gestor-de-projetos.md`), que continuam valendo: o `cthubd` é a origem dos projetos e a única cópia do histórico; o `ctowerd` executa sessões, PTY, hooks e MCP e envia a execução pelo outbox. Esta spec acrescenta um segundo tipo de projeto, com **issues no Jira** e **repositório no GitLab**, cujo sync roda num ambiente e não no hub.

As decisões confirmadas pelo usuário estão no épico #353 e nas sub-issues, e não são reabertas aqui. As escolhas que esta spec acrescenta estão na §14, marcadas como autônomas, e precisam da revisão do usuário antes das sub-issues #347 a #352 e #357.

## 1. Problema e direção

O Control Tower supõe GitHub em todas as camadas:

- **cadastro:** `hub.projects.add { url }` passa por `ct_core::github_repo_from_url` (`crates/ct-core/src/repo_url.rs`) e valida o repositório e a base com o token do GitHub do hub (`crates/cthubd/src/github.rs`, `validate_repo`/`validate_branch`);
- **modelo:** `HubProject.gh_owner/gh_repo`, `HubWorkItem.gh_number`, `Project.gh_owner/gh_repo` no `ledger.db`, e a chave natural `owner/repo#n` (`ct_ledger::sync::project_key`, `work_item_key`) em todo o outbox;
- **sync:** o hub sincroniza issues, sub-issues e PRs de todos os projetos (`HubGithub::sync_all`), e cada ambiente sincroniza os seus (`ct_sessions::github::GithubSync`), os dois pelo `ct-gh` e aplicando com `Ledger::apply_github`/`Hub` (espelho `issue_cache`, `pr_cache`, `sync_state`, `gh_etag`);
- **clone:** `ct_git::gh_clone` e a conferência do `origin` em `crates/ct-server/src/checkout.rs`;
- **conclusão e limpeza:** `CompletionSignal::PrMesclado/PrFechado` (`crates/ct-sessions/src/cleanup.rs`) e a remoção da branch remota pela API do GitHub (`cleanup_branches.rs`, `GithubSync::delete_remote_branch`);
- **guarda-corpo:** `check_gh` e o `gh pr view` do `ctower hook` (`crates/ct-core/src/guard`, `crates/ctower/src/hook.rs`), e o `Closes #n` exigido no `gh pr create` (`guard/closes.rs`, `CTOWER_ISSUE`);
- **branch:** `ct_core::branch::branch_for` gera sempre `{wip|feat|fix|chore}/<n>-<slug>`;
- **templates:** os quatro templates do `ui.json` (`ui/src/settings/defaults.ts`, espelhados em `ui/src-tauri/src/envs.rs`) citam `gh issue view`, `#{{number}}` e `Closes #`;
- **UI:** `itemRef`/`itemTitle` (`ui/src/util/item.ts`) e links montados à mão para `github.com` (`TerminalScreen.tsx`, `HubProjects.tsx`, `HubScreen.tsx`).

O primeiro projeto novo é o projeto de exemplo: Jira Server 8.5.2 (`PROJ`), legível só pelo [jira-cli](https://github.com/ankitpokhrel/jira-cli), e GitLab self-hosted em `https://gitlab.example.com` (`grupo/projeto/app`). Os dois **só são alcançáveis de um ambiente específico**, não da máquina do hub.

Direção:

```
                         cthubd — hub.db
   projeto github: sync do GitHub no hub (como hoje)
   projeto jira:   não sincroniza; recebe o espelho do ambiente de sync
     │ GET /sync/config (tracker, forge, branch_pattern, regras, sync: bool)
     │                                   ▲ POST /sync/push
     ▼                                   │ issue_mirror · pr_mirror · tracker_status (+ execução)
  ctowerd do ambiente de sync ─── jira-cli (JIRA_API_TOKEN do comando de credencial)
     │  ledger.db: issue_cache, pr_cache  └─ glab (GITLAB_HOST + GITLAB_TOKEN do comando de credencial)
     └─ sessões: o agente usa jira-cli e glab com o mesmo comando de credencial
```

- O modelo fica **neutro de provedor**: o projeto tem um *rastreador* (`github` | `jira`) e uma *forja* (`github` | `gitlab`). Os projetos GitHub não mudam.
- O número do work item continua inteiro: no Jira, é o número da chave (`PROJ-5450` → `5450`). A chave em texto é derivada do projeto, nunca gravada (§3.3).
- O sync do projeto Jira roda nos ambientes atribuídos, e um deles, o **ambiente de sync**, envia o espelho ao hub pelo `/sync/push` que já existe (§5).
- O Control Tower **lê** o Jira, e as sessões **criam issues e comentam**. Ninguém faz transição de status no Jira (§7).

## 2. Escopo

Entra:

- projetos `jira` + `gitlab` (rastreador Jira, forja GitLab, self-hosted, HTTPS ou SSH);
- um projeto Jira por projeto do Control Tower e um repositório por projeto (o exemplo tem só o `grupo/projeto/app`);
- padrão de branch por projeto, regras do agente por projeto e por épico (#357) e linhas de atribuição desligáveis por projeto.

Não entra:

- as combinações `github` + `gitlab` e `jira` + `github`. O modelo as comporta, mas o cadastro as recusa (`combinação de rastreador e forja não suportada`) até haver um projeto real;
- transição de status, edição, atribuição e worklog no Jira;
- espelho dos comentários do Jira (as sessões leem pelo jira-cli);
- vários projetos Jira ou vários repositórios num mesmo projeto.

## 3. Modelo neutro de provedor (#347)

### 3.1 Tipos em `ct-core`

Módulo novo `crates/ct-core/src/tracker.rs`:

```rust
str_enum!(TrackerKind, "tracker_kind", { Github => "github", Jira => "jira" });
str_enum!(ForgeKind,   "forge_kind",   { Github => "github", Gitlab => "gitlab" });

/// `tracker_config` do projeto Jira (JSON na coluna; `{}` no GitHub).
pub struct JiraConfig {
    pub url: String,                 // "https://jira.example.com", sem barra final
    pub project_key: String,         // "PROJ"
    pub jql: String,                 // filtro extra, "" = nenhum
    pub credential_cmd: String,      // "pass show jira | head -1"
    pub config_file: String,         // "" = ~/.config/.jira/.config.yml (JIRA_CONFIG_FILE)
    pub epic_link_field: String,     // "customfield_10601"
    pub epic_name_field: String,     // "customfield_10604"
    pub type_map: BTreeMap<String, JiraTypeRole>,  // nome do tipo → papel (§6.3)
}

/// `forge_config` do projeto GitLab (JSON na coluna; `{}` no GitHub).
pub struct GitlabConfig {
    pub host: String,                // "gitlab.example.com"
    pub path: String,                // "grupo/projeto/app"
    pub clone_url: String,           // como o usuário colou (HTTPS ou SSH)
    pub credential_cmd: String,      // "pass show gitlab | head -1"
}
```

Na serialização do protocolo, o par tipo + configuração vira um objeto marcado:

```json
"tracker": { "kind": "jira", "url": "https://jira.example.com", "project_key": "PROJ", "jql": "",
             "credential_cmd": "pass show jira | head -1", "config_file": "",
             "epic_link_field": "customfield_10601", "epic_name_field": "customfield_10604",
             "type_map": { "Épico": "epic", "Bug": "bug" } },
"forge":   { "kind": "gitlab", "host": "gitlab.example.com", "path": "grupo/projeto/app",
             "clone_url": "https://gitlab.example.com/grupo/projeto/app.git",
             "credential_cmd": "pass show gitlab | head -1" }
```

e `{ "kind": "github" }` nos projetos GitHub. Os campos são `#[serde(default)]`: um hub ou ambiente anterior que não manda `tracker` e `forge` é lido como GitHub.

`ProjectKind` (`github` | `gestor`) continua como está: diz se o projeto é real ou o sentinela do gestor, e não o provedor.

### 3.2 Identidade do projeto

A chave natural continua `owner/repo`, em minúsculas (`project_key`). No GitLab:

- `owner` = o caminho sem o último segmento (`grupo/projeto`), que pode ter barras;
- `repo` = o último segmento (`app`);
- a chave fica `grupo/projeto/app`, o caminho do projeto no GitLab.

Assim `project`, `assignment`, o outbox, `push_quarantine`, o `name` padrão (`app`) e a pasta dos worktrees (`<worktrees_root>/<name>`) não mudam. Quem divide a chave na primeira barra passa a dividir na última (`rsplit_once('/')`): `crates/cthubd/src/ws.rs` (escopo de secret de projeto) e `ct_secrets::Scope::parse_project`, que hoje recusa `repo` com barra.

`ct_core::repo_from_url(url) -> Option<RepoRef { host, path }>` (novo, em `repo_url.rs`) aceita HTTPS, `ssh://` e `git@host:` de qualquer host. `github_repo_from_url` fica como está, para o GitHub, e o `checkout.rs` passa a comparar `host` + `path` do `origin` com o cadastro.

### 3.3 Referência do work item: chave derivada

O work item continua com `gh_number INTEGER` nos dois bancos e no outbox. No Jira, o número é o da chave: `PROJ-5450` → `5450`. A chave em texto **não é gravada**: é sempre `{project_key}-{n}`, do `tracker_config` do projeto.

Uma função única gera as referências, em Rust e na UI:

```rust
// ct_core::tracker
pub fn item_ref(t: &TrackerRef, n: i64) -> String   // "deep-ion-ai/ctower-agents#12" | "PROJ-5450"
pub fn item_short(t: &TrackerRef, n: i64) -> String // "#12" | "PROJ-5450"
pub fn item_key(t: &TrackerRef, n: i64) -> String   // "12" | "PROJ-5450" (o {key} da branch)
pub fn item_url(t: &TrackerRef, n: i64) -> String   // ".../issues/12" | "<jira>/browse/PROJ-5450"
```

`TrackerRef` é o par (`owner/repo`, `TrackerConfig`) que todo `Project`/`HubProject` sabe montar. Na UI, `itemRef`/`itemTitle` (`ui/src/util/item.ts`) passam a receber o projeto, e há `itemUrl` e `changeLabel` ("PR" ou "MR").

Onde a referência aparece:

| Lugar | GitHub (igual a hoje) | Jira |
|---|---|---|
| branch (§4) | `feat/12-slug` | `feat/PROJ-5450-slug` no padrão; `PROJ-5450` no exemplo |
| `ledger_get_context` | `"ref": "owner/repo#12"` | `"ref": "PROJ-5450"`, `"issue.url"` do `/browse/` |
| `portfolio_*` e `hub.health.get` | campo `ref` novo, ao lado de `owner`, `repo`, `number` | idem |
| propostas (`hub.proposals.list`) | `ref` na resposta; o alvo continua `(owner, repo, number)` | idem |
| eventos | `payload.gh_number` como hoje | idem; a UI mostra `ref` |
| UI | `#12` | `PROJ-5450` |

A chave só entra no espelho por um caminho: o sync do Jira descarta a issue cuja chave não começa com `{project_key}-` (issue movida para outro projeto). Com isso, o número identifica a issue sem ambiguidade.

O épico de uma issue continua em `issue_cache.parent_number`, e o "é épico" continua em `has_sub_issues` (§6.3).

### 3.4 Colunas novas

Migração nova do `hub.db` (a próxima livre; hoje a 18) e do `ledger.db` (a próxima livre; hoje a 11), uma de cada, na #347:

```
project + tracker_kind   TEXT NOT NULL DEFAULT 'github' CHECK (tracker_kind IN ('github', 'jira'))
        + tracker_config TEXT NOT NULL DEFAULT '{}'      -- JSON de JiraConfig
        + forge_kind     TEXT NOT NULL DEFAULT 'github' CHECK (forge_kind IN ('github', 'gitlab'))
        + forge_config   TEXT NOT NULL DEFAULT '{}'      -- JSON de GitlabConfig
        + branch_pattern TEXT                            -- NULL = padrão do rastreador (§4)
        + attribution    INTEGER NOT NULL DEFAULT 1      -- 0 = sem linhas de atribuição (§9.3)

só no hub.db:
project + sync_env_id    TEXT REFERENCES env(env_id)     -- ambiente de sync (§5.1); NULL no GitHub

só no ledger.db:
project + sync_role      INTEGER NOT NULL DEFAULT 0      -- 1 = este ambiente envia o espelho (§5.2)
```

- Tudo com valor padrão: as linhas existentes viram GitHub, sem mudança visível. As fixtures antigas (`crates/cthubd/tests/fixtures/hub-antigo.sql`, `ledger-antigo.sql`) continuam passando por `cthubd migrate`, e um teste novo confere que um projeto delas sai com `tracker_kind = 'github'`.
- `gh_owner`/`gh_repo` e `gh_number` **não são renomeados**: renomear colunas no SQLite recria tabelas com triggers de só inclusão (`event`, `decision`, `proposal`), e os nomes viajam no outbox de ambientes antigos. No código, os campos ganham doc dizendo que são o `owner`/`repo` da forja e o número do rastreador.
- As regras do agente (#357) têm tabelas próprias (§10.4).

### 3.5 Protocolo

- `HubProject`, `Project` (ledger), `ConfigProject` (`crates/ct-ledger/src/hub_config.rs`) e `ProjectChange` ganham `tracker`, `forge`, `branch_pattern` e `attribution`; `HubProject` ganha `sync_env_id` e `ConfigProject` ganha `sync: bool`.
- Tipos novos com `ts-rs`: `TrackerKind`, `ForgeKind`, `TrackerConfig`, `ForgeConfig`, `JiraConfig`, `GitlabConfig`, `JiraTypeRole`, `TrackerReadiness` (§8.4), regenerados em `ui/src/protocol/gen` e `ui/src/protocol/hub-gen`.
- Só entram campos e métodos: o protocolo do hub passa de **1.2 para 1.3** (`crates/cthubd/src/lib.rs`) e o do ambiente de **2.2 para 2.3** (`crates/ct-server/src/lib.rs`).
- `SyncChange` ganha três variantes (§5.3). Um hub anterior não as entende, então o hub é atualizado antes dos ambientes, como nas outras mudanças do outbox. Na prática, o ambiente só as emite para um projeto Jira, que só um hub 1.3 cadastra.

## 4. Nome da branch configurável (#347)

`branch_pattern` é um modelo com estes marcadores:

| Marcador | Valor |
|---|---|
| `{prefix}` | `wip` (épico), `fix` (label `bug`), `chore` (label `chore`) ou `feat`, como em `branch_for` |
| `{n}` | o número (`12`, `5450`) |
| `{key}` | `item_key`: `12` no GitHub, `PROJ-5450` no Jira |
| `{slug}` | `slugify(title, 40)` |

- Padrão (`branch_pattern` NULL): `{prefix}/{n}-{slug}` no GitHub, igual a hoje, e `{prefix}/{key}-{slug}` no Jira (`feat/PROJ-5450-portar-o-plugin`).
- exemplo: `{key}`, que dá exatamente `PROJ-5450` (o bot da pipeline fecha o MR com outro nome).
- Slug vazio: o separador antes de `{slug}` cai (`feat/5`, como hoje).
- `ct_core::branch::branch_from_pattern(pattern, kind, number, key, title, labels) -> Result<String>`; `branch_for` vira a chamada com o padrão do GitHub. Validação no cadastro: o padrão tem `{n}` ou `{key}`, só tem marcadores conhecidos, e o resultado de um exemplo passa no `git check-ref-format --branch`.
- `Sessions::start` (`crates/ct-sessions/src/launch.rs`) usa o padrão do projeto. A branch gravada no work item (`work_item.branch`) continua valendo depois: mudar o padrão não renomeia branch existente.
- Ninguém mais supõe o prefixo `wip/`. O guarda-corpo, a detecção de PR/MR e a limpeza usam `work_item.branch`. Em `cleanup_branches.rs`, o `never_touch` deixa de reconhecer o épico pelo nome `wip/<n>-…` e passa a procurar o work item `kind = 'epic'` dono da branch.

## 5. Sync no ambiente (#348, #349)

### 5.1 Ambiente de sync

- Cada projeto Jira tem um `sync_env_id` no hub, escolhido entre os ambientes atribuídos.
- A primeira atribuição de um projeto Jira sem ambiente de sync vira o ambiente de sync, na mesma transação. Remover a atribuição do ambiente de sync deixa `sync_env_id` NULL, sem escolher outro sozinho.
- `hub.projects.update { owner, repo, sync_env }` troca o ambiente de sync, que precisa estar atribuído. É uma mudança de projeto: incrementa `config_rev`.
- No `/sync/config`, `ConfigProject.sync` é `true` só para o ambiente de sync. O ambiente grava em `project.sync_role`.

### 5.2 Quem busca

Todo ambiente atribuído a um projeto Jira roda o sync do Jira e do GitLab para uso local (issues para o título e o tipo ao iniciar sessão, MRs para os gatilhos de status e a limpeza), como hoje cada ambiente sincroniza o GitHub dos seus projetos. Os ambientes atribuídos precisam alcançar o Jira e o GitLab de qualquer forma, porque as sessões usam o jira-cli e o glab.

Só o ambiente de sync **envia o espelho** ao hub. O hub não roda sync para projetos Jira: `HubGithub::sync_all` e `ct_sessions::github::GithubSync::sync_all` pulam `tracker_kind != 'github'`.

Alternativa descartada: só o ambiente de sync buscar, e os outros lerem o espelho do hub. O ambiente não lê `issue_cache` do hub hoje, e a limpeza e os gatilhos de MR ficariam presos à conectividade com o hub.

### 5.3 O que sobe pelo `/sync/push`

Três variantes novas de `SyncChange` (`crates/ct-ledger/src/sync.rs`), todas fotografias que se substituem no outbox (`coalesces`):

```
issue_mirror   { owner, repo, issue: IssueRow }                            chave owner/repo#n
pr_mirror      { owner, repo, gh_number, pr: PrRow sem work_item_id }      chave owner/repo!<iid>
tracker_status { owner, repo, last_attempt_at, last_ok_at?, last_error?,
                 readiness: TrackerReadiness }                             chave owner/repo
```

- O ambiente enfileira `issue_mirror` e `pr_mirror` na mesma transação do `apply_github` (só as linhas que mudaram), e `tracker_status` ao fim de cada ciclo, com ou sem erro. Só faz isso com `sync_role = 1`.
- O hub aplica as três no `issue_cache`, `pr_cache` e `sync_state` dele **só se** o projeto for Jira e o `env_id` do envio for o `sync_env_id` atual. Caso contrário, a linha é descartada com ack (contada em `IngestResult`, com aviso no log), como as de projeto com lápide.
- Os eventos derivados (`pr_opened`, `pr_merged`, `issue_closed` e as mudanças de status) já sobem como `event`, com os `uid` determinísticos de hoje (`gh_pr_uid`, `gh_issue_closed_uid`). Dois ambientes que percebem o mesmo MR mesclado geram o mesmo `uid`, e o hub descarta o repetido.
- Ao virar ambiente de sync (pull que traz `sync: true` com `sync_role = 0`), o ambiente enfileira o espelho completo do projeto, para o hub não depender do histórico do anterior.

### 5.4 Ambiente de sync fora do ar

- O hub mantém o último espelho recebido. A árvore, o ranking e a saúde continuam funcionando com ele.
- Sinal de saúde novo `rastreador_sem_sync` (`crates/ct-ledger/src/hub/health.rs`), ao lado de `github_sem_sync`: projeto Jira sem `sync_env_id`, ou com `sync_state.last_ok_at` mais velho que 3 × `jira_sync_interval_secs` (aviso) ou 24 h (crítico), ou com `last_error`. O texto cita o ambiente e o erro.
- O hub não troca o ambiente de sync sozinho. A UI oferece "trocar ambiente de sync" no aviso. Trocar sozinho exigiria saber se o outro ambiente alcança o Jira, e dois ambientes enviando o espelho do mesmo projeto disputariam as linhas.

## 6. Leitura do Jira pelo jira-cli (#349)

### 6.1 Crate `ct-jira`

Crate novo `crates/ct-jira`, sem rede própria: roda o `jira` como processo externo, com prazo (30 s por chamada, 10 s para o comando de credencial), sem TTY e com `stdin` nulo.

```rust
pub struct JiraCli { bin: String, config_file: Option<PathBuf>, deadline: Duration }
impl JiraCli {
    pub fn version(&self) -> Result<String>;                               // jira version
    pub fn me(&self, token: &Secret) -> Result<String>;                    // jira me
    pub fn server_info(&self, token: &Secret) -> Result<String>;           // jira serverinfo
    pub fn list(&self, token: &Secret, q: &ListQuery) -> Result<Vec<Value>>; // jira issue list --raw
}
pub fn issue_row(v: &Value, cfg: &JiraConfig) -> Option<IssueRow>;        // JSON da API v2 → espelho
```

- O token vai só no ambiente do processo filho: `JIRA_API_TOKEN=<saída do comando de credencial>`, e `JIRA_CONFIG_FILE` quando `config_file` não é vazio. Nunca vai para argv, log, evento ou mensagem de erro. O `stderr` do jira-cli entra na mensagem de erro truncado em 300 caracteres e com o token trocado por `***`, se aparecer.
- `Secret` é um `String` que não implementa `Debug`/`Display` e zera a memória no `Drop`.
- O binário vem de `jira_bin` no `config.toml` do `ctowerd` (padrão `"jira"`), como `claude_bin` e `gh_bin`.

### 6.2 Comandos (verificados com jira-cli 1.7.0)

Ciclo de sync, paginado de 100 em 100 até vir uma página menor:

```
jira issue list -p PROJ -q '<filtro>' --order-by updated --reverse --paginate <from>:100 --raw
```

- `--raw` devolve o JSON da busca da API v2 do Jira Server 8.5.2, com `key`, `fields.summary`, `fields.issuetype { name, subtask }`, `fields.status.statusCategory.key`, `fields.labels`, `fields.updated`, `fields.parent` (sub-task) e os campos do épico (`customfield_10601`, `customfield_10604`).
- `--order-by updated --reverse` dá a ordem crescente de atualização. Se um ciclo cair no meio, o próximo recomeça sem buraco.
- `<filtro>`:
  - primeiro ciclo e reconciliação diária: `(<jql do cadastro>) AND (statusCategory != 3 OR updated >= -30d)`, o mesmo recorte do GitHub (abertas e fechadas nos últimos 30 dias). A categoria vai pelo id (3 = Done), porque o nome da categoria aparece traduzido;
  - ciclos seguintes, incrementais: `(<jql do cadastro>) AND updated >= -<m>m`, com `m` = minutos desde o `last_ok_at` mais 5 de folga. O prazo relativo é calculado pelo próprio Jira, sem depender do fuso do servidor;
  - JQL vazia: o `(...) AND` some.
- Mudar `jql` ou `type_map` no cadastro força a reconciliação no ciclo seguinte. A marca da última reconciliação fica em `gh_etag` (chave `jira:full:<owner>/<repo>`), sem tabela nova.
- A issue que sai do filtro (movida, apagada ou fora da JQL nova) fica no espelho, como acontece hoje no GitHub.

Prontidão (§8.4): `jira version`, `jira me` e `jira serverinfo`.

Sessões (§7): `jira issue view <KEY> --comments 20 --plain` para ler; `jira issue list -p <KEY> -q 'cf[10601] = <EPIC>' --plain` para listar as issues de um épico; `jira issue create` e `jira issue comment add` para escrever.

A #349 confere, contra o Jira real, se o `-q` com `-p` acrescenta o `project = ...` sozinho e o formato do `jira serverinfo`, e registra a saída na issue.

### 6.3 Tipos, épicos e sub-tasks

`issue_row` monta o `IssueRow` do espelho:

| Campo | Origem |
|---|---|
| `gh_number` | número da `key` (`PROJ-5450` → 5450); issue com outro prefixo é descartada |
| `title` | `fields.summary` |
| `state` | `"closed"` se `fields.status.statusCategory.key == "done"`, senão `"open"` |
| `labels` | `fields.labels`, mais `bug` ou `chore` se o papel do tipo for esse |
| `parent_number` | sub-task: número de `fields.parent.key`; senão, o número do Epic Link (`fields[epic_link_field]`); se faltar, `fields.parent` (Jira com épico por `parent`) |
| `has_sub_issues` | papel `epic` (abaixo) ou issue com sub-tasks |
| `url` | `<url>/browse/<key>` |
| `updated_at` | `fields.updated` (`2026-10-28T10:15:30.000-0300`) em ms |

Papel do tipo (`JiraTypeRole`): `epic` | `issue` | `bug` | `chore` | `ignore`.

1. `fields.issuetype.subtask == true` → sub-issue do pai, sempre.
2. `type_map[fields.issuetype.name]`, se houver. `ignore` tira a issue do espelho.
3. Sem entrada no mapa: `epic` se `fields[epic_name_field]` não é nulo (só épicos têm Epic Name), senão `issue`.

O mapa padrão do cadastro é `{ "Epic": "epic", "Épico": "epic", "Bug": "bug", "Erro": "bug" }`. Os tipos do exemplo aparecem em português (`Tarefa`, `Melhoria`), e por isso o reconhecimento do épico não depende do nome: o passo 3 cobre um nome que o mapa não tem.

O "é épico" continua sendo `has_sub_issues`, como no GitHub, onde a issue com sub-issues é o épico: `session.start` escolhe `kind = epic` por ele (`crates/ct-server/src/ws.rs`). Uma Story com sub-tasks vira épico no Control Tower, com as sub-tasks como sub-issues. A #349 confere que a árvore (`crates/ct-ledger/src/tree.rs`, `TreeNode.children` recursivo) mostra os três níveis (épico → Story → sub-task).

### 6.4 Status

- A categoria `done` vira `state = "closed"`, e o `apply_github` já gera `issue_closed` (com o mesmo `uid`) e leva à conclusão como hoje. `new` e `indeterminate` ficam `open`.
- O Control Tower **não faz transição** no Jira. Os status do work item (`em_execucao`, `em_revisao`, ...) continuam vindo das sessões e do MR.
- Issue reaberta no Jira volta a `open` no espelho, com o mesmo tratamento do GitHub.

### 6.5 Laço e erros

- `ct_sessions::tracker_sync::JiraGitlabSync` (novo, ao lado de `github.rs`), criado pelo `ctowerd` (`crates/ctowerd/src/main.rs`) como o `GithubSync`. Para cada projeto Jira atribuído, roda o comando de credencial do Jira (§8.1), busca no Jira (§6.2) e no GitLab (§7.2), monta um `GithubSnapshot` e aplica com `apply_github`.
- Intervalo `jira_sync_interval_secs` no `config.toml` (padrão 300; `0` desliga o laço, e o `sync.now` continua). O Jira é antigo e lento, e o recorte incremental torna o ciclo barato.
- Falha: `record_sync_error` com o motivo e backoff de `next_delay` (até `MAX_BACKOFF`, 15 min), sem laço apertado. Os motivos, sem nunca citar a senha:
  - `jira-cli não encontrado (jira_bin = "jira")`;
  - `configuração do jira-cli ausente em ~/.config/.jira/.config.yml: rode "jira init" no ambiente`;
  - `comando de credencial do Jira falhou (saída 2): o cache do gpg-agent pode ter expirado; rode "pass show jira" num terminal do ambiente`;
  - `comando de credencial do Jira não respondeu em 10 s: o pinentry pode estar pedindo a senha`;
  - `o Jira recusou a senha (401)`;
  - `Jira fora do ar: <erro do jira-cli>`.
- Um ciclo com o Jira em erro não aplica nada do GitLab: a falha vai inteira para o `sync_state`, e o `apply_github` continua atômico.

## 7. GitLab e escrita no Jira (#350, #351)

### 7.1 Clone e push

- Clone por `git clone <clone_url> <repo_path>` (função nova `ct_git::clone_url`), HTTPS ou SSH, com o credential helper do git do ambiente (`osxkeychain` no macOS). O `checkout.rs` confere `host` e `path` do `origin` pelo `repo_from_url`.
- Push pelo mesmo `origin`, pelo agente, como hoje.
- A #350 confere se o `osxkeychain` responde a partir do `ctowerd` no launchd. Se não responder, o relato `project_location` sai com erro e a instrução de clonar à mão.

### 7.2 MRs pelo `glab`

Crate novo `crates/ct-gitlab`, no mesmo molde do `ct-jira`: processo externo com prazo, `GITLAB_HOST` do cadastro e `GITLAB_TOKEN` do comando de credencial só no processo filho, e binário em `glab_bin` (padrão `"glab"`). A configuração global do glab no ambiente não é tocada.

- Leitura, por work item com branch cujo último MR no espelho não está mesclado:

  ```
  glab mr list -R grupo/projeto/app --all -s <branch> -F json -P 100
  ```

  Cada MR vira `PrRow`: `number` = `iid`, `head` = `source_branch`, `base` = `target_branch`, `state` = `opened` → `open`, `merged` → `merged`, `closed`/`locked` → `closed`, `url` = `web_url`, `updated_at`, `head_sha` = `sha`.
- O MR mesclado ou fechado vira o mesmo sinal do PR: `pr_trigger` e `CompletionSignal::PrMesclado/PrFechado` não mudam.
- Escolhemos o `glab` em vez da API REST direta. O glab trata host self-hosted, paginação e autenticação pelo `GITLAB_TOKEN` da mesma forma que o jira-cli trata o Jira, e o daemon continua sem cliente HTTP para terceiros.

### 7.3 Limpeza da branch remota

`BranchRemoteStep` (`cleanup_branches.rs`) ganha o caminho GitLab, atrás de um trait `RemoteBranches` com uma implementação para o GitHub (a de hoje) e outra para o GitLab:

- ponta remota: `glab api projects/<path codificado>/repository/branches/<branch codificada>` → `commit.id`;
- só apaga se a ponta é o `head_sha` do último MR: `glab api -X DELETE projects/<path>/repository/branches/<branch>`, depois `git fetch --prune`.

As mesmas proteções de hoje valem (`never_touch`, com o épico achado pelo work item, §4).

### 7.4 Guarda-corpo

- O `git push` para branch protegida já é bloqueado pelo `check_push`, independente da forja.
- `glab mr merge` é **sempre bloqueado** nos projetos GitLab (`check_glab`, novo em `crates/ct-core/src/guard`), assim como `glab api` de escrita num caminho que termina em `/merge`. O hook não tem como saber a base do MR sem o token, que o agente passa dentro do próprio comando (`GITLAB_TOKEN="$(pass ...)" glab ...`) e o hook não deve avaliar. No exemplo, todo MR vai para `develop` (protegida), e o merge é do usuário de qualquer forma.
- Perfil Jira (`CTOWER_TRACKER=jira`, `check_jira`, novo):
  - **bloqueia** transição e edição: `jira issue move|edit|delete|assign|link|unlink|clone|worklog`, `jira epic add|remove`, `jira sprint add`;
  - **pede confirmação (`ask`)**: `jira issue create` e `jira epic create`, porque criar issue no Jira exige o ok do usuário (regra do exemplo). Usa o `Verdict::Ask` do perfil do gestor;
  - libera: `jira issue list|view`, `jira issue comment add`, `jira me`, `jira serverinfo`.
- O `Closes #n` do `gh pr create` (`CTOWER_ISSUE`, #375) não vale para Jira: o `launch` não define `CTOWER_ISSUE` num projeto Jira.

### 7.5 Variáveis da sessão

O `launch` define, num projeto Jira/GitLab (nenhuma é segredo):

```
CTOWER_TRACKER=jira
CTOWER_ISSUE_KEY=PROJ-5450        (sessão de issue ou épico)
GITLAB_HOST=gitlab.example.com
JIRA_CONFIG_FILE=<config_file>       (só se configurado)
```

O token **não** é injetado: o agente roda o comando de credencial dentro do comando, como o exemplo já faz (§8.2).

### 7.6 Quem escreve o quê no Jira

O daemon **nunca escreve** no Jira nem no GitLab, exceto a remoção da branch remota na limpeza (§7.3). As sessões escrevem, pelos templates (§9):

| Quando | Quem | O quê |
|---|---|---|
| sessão de planejamento ou do épico propõe issue | agente, com `ask` do hook | `jira issue create -p PROJ -t Tarefa -s '<título>' -b '<descrição>' -P <EPIC-KEY> --no-input` (`-P` liga ao épico; numa sub-task, `-t Sub-tarefa -P <ISSUE-KEY>`) |
| sessão de issue abre o MR | agente | `glab mr create -R <path> -s <branch> -b <base> -t '<título>' -d '' -y` |
| logo depois de abrir o MR | agente | `jira issue comment add <KEY> 'MR aberto: <url do MR>' --no-input` |
| decisão que muda o escopo da issue | agente, só com o ok do usuário | `jira issue comment add <KEY> ...` |

Só o comentário com o link do MR é automático. É ele que liga o Jira ao MR, já que o Control Tower não faz transição. Os outros comentários dependem do ok do usuário na conversa.

## 8. Credenciais (#348, #349, #351)

### 8.1 Comando de credencial

- Cada projeto guarda dois comandos: `tracker.credential_cmd` (`pass show jira | head -1`) e `forge.credential_cmd` (`pass show gitlab | head -1`). São texto comum do cadastro, não secret: o hub os vê e os entrega pelo `/sync/config`. A senha e o token nunca passam pelo hub.
- O daemon roda o comando como `/bin/sh -c '<cmd>'`, com `stdin` nulo, sem TTY, com prazo de 10 s, matando o grupo de processos no estouro. Do `stdout`, vale a primeira linha sem espaços nas pontas; vazia é erro.
- O comando roda uma vez por ciclo de sync, e o valor fica só na memória até o fim do ciclo (`Secret`, §6.1). Não há cache em disco.
- No jira-cli, a senha vai como `JIRA_API_TOKEN` (Jira Server com basic auth, `installation: Local`, sem PAT). No glab, o token de acesso vai como `GITLAB_TOKEN`.

### 8.2 Nas sessões

O agente usa o mesmo comando. Os templates recebem o prefixo pronto, sem a senha (§9.2):

```
JIRA_API_TOKEN="$(pass show jira | head -1)" jira issue view PROJ-5450 --plain
GITLAB_TOKEN="$(pass show gitlab | head -1)" glab mr create ...
```

(`GITLAB_HOST` já vem da sessão, §7.5.) O valor aparece no ambiente do subprocesso do agente, não no transcript, porque o `$(...)` é expandido pelo shell.

### 8.3 Limite do `gpg-agent`

O `ctowerd` roda em segundo plano (launchd/systemd) e não consegue abrir o pinentry. O comando de credencial só funciona com a chave do `pass` no cache do `gpg-agent`, com TTL longo (`default-cache-ttl`/`max-cache-ttl`), como o exemplo já configura. Com o cache expirado, o comando falha ou trava. O prazo de 10 s corta a espera, o sync fica em erro com o motivo (§6.5), e o hub mostra `rastreador_sem_sync` com o texto. O usuário roda `pass show jira` num terminal do ambiente, e o ciclo seguinte volta.

A configuração do jira-cli (`~/.config/.jira/.config.yml`, gerada por `jira init`) é local ao ambiente. O Control Tower não a cria nem a altera.

### 8.4 Prontidão do ambiente

Cada ambiente calcula, no início de cada ciclo e no boot, um `TrackerReadiness` por projeto Jira atribuído:

```json
{ "owner": "grupo/projeto", "repo": "app", "checked_at": 1761650000000,
  "jira":   { "cli": "1.7.0", "config_present": true, "server_matches": true,
              "credential_ok": true, "me_ok": true, "server_version": "8.5.2", "error": null },
  "gitlab": { "cli": "1.124.0", "credential_ok": true, "api_ok": true, "error": null } }
```

- `server_matches` compara o `server` do `.config.yml` com a `url` do cadastro.
- `api_ok` vem de `glab api version`.
- `env.info` ganha `trackers: Vec<TrackerReadiness>`, lido do último ciclo, sem rodar nada na hora.
- O ambiente de sync manda o mesmo objeto no `tracker_status` (§5.3), e o cadastro no hub o mostra (§11).

## 9. Templates e convenções (#351)

### 9.1 Templates por rastreador

O `ui.json` ganha três templates: `templates.jira_issue`, `templates.jira_epic` e `templates.jira_project`, com padrões em `ui/src/settings/defaults.ts` e `ui/src-tauri/src/envs.rs`. `templateFor(t, kind, tracker)` (`ui/src/project/templates.ts`) escolhe por rastreador. Os projetos GitHub usam os de hoje, sem mudança.

Padrões (o texto final fica na #351):

```
jira_issue:
Trabalhe na {{kind}} {{key}} do Jira ({{jira_url}}/browse/{{key}}), no repositório GitLab {{forge_path}}.
Leia a issue com `{{jira}} issue view {{key}} --comments 20 --plain`.
Branch de trabalho: {{branch}}. Nunca mescle nem faça push em {{protected}}.
Ao terminar, abra o MR para {{base}} com `{{glab}} mr create -R {{forge_path}} -s {{branch}} -b {{base}} -t '<título>' -y`
e comente o link na issue com `{{jira}} issue comment add {{key}} 'MR aberto: <url>' --no-input`.
Não mude o status da issue no Jira. Crie issues só depois do meu ok.
Consulte decisões anteriores com `ledger_list_decisions` e registre as relevantes com `ledger_record_decision`.
```

`jira_epic` e `jira_project` seguem o mesmo molde. O de épico lista as issues com `{{jira}} issue list -p {{jira_project}} -q 'cf[10601] = {{key}}' --plain`. O de projeto cria issues com `{{jira}} issue create -p {{jira_project}} ... -P <épico> --no-input`, só depois do ok. Texto final (#351) em `ui/src/settings/defaults.ts`: o de épico lista as issues com `{{jira}} epic list {{key}} --plain`, que usa o Epic Link do `.config.yml` e não depende do número do campo; os três pedem as regras do projeto e do épico (`rules_get`/`rules_propose`) em vez de repetir as convenções de um projeto.

### 9.2 Variáveis novas

`renderTemplate` ganha, além das de hoje:

| Variável | Valor |
|---|---|
| `{{key}}` | `item_short`: `#12` no GitHub, `PROJ-5450` no Jira |
| `{{title}}` | título da issue no espelho |
| `{{base}}` | `base_branch` |
| `{{jira_url}}`, `{{jira_project}}` | do `tracker_config` |
| `{{jira}}` | `JIRA_API_TOKEN="$(<credential_cmd>)" jira` (com `JIRA_CONFIG_FILE=...` se configurado) |
| `{{glab}}` | `GITLAB_TOKEN="$(<credential_cmd>)" glab` |
| `{{forge_path}}` | `grupo/projeto/app` |

O `{{jira}}` traz o comando de credencial do cadastro, nunca a senha.

### 9.3 Convenções de commit e MR

- **Formato do commit e do título do MR** (`[PROJ-NNNN] <título do Jira>`, sem corpo): vai nas **regras do projeto** (#357, §10), que são texto livre do usuário, e não num campo novo. O exemplo importa isso do `CLAUDE.md` atual.
- **Linhas de atribuição**: `project.attribution` (§3.4). Com `0`, o `settings.json` da sessão (`hook_settings` em `launch.rs`, já passado por `--settings`) ganha `"attribution": { "commit": "", "pr": "" }` e `"includeCoAuthoredBy": false`, que o Claude Code usa para não acrescentar o `Co-Authored-By` nem o rodapé no PR/MR. A #351 confere o nome das chaves na versão instalada do Claude Code. O padrão é `1`, igual a hoje.
- Avisar quando a sessão não tem o jira-cli pronto: o `StartSessionModal` lê `env.info.trackers` do ambiente escolhido e mostra o motivo antes de iniciar. O daemon não recusa a sessão.

## 10. Regras do agente por projeto e por épico (#357)

Vale para qualquer projeto, inclusive os GitHub.

### 10.1 Injeção: `--append-system-prompt`

As regras vigentes entram no `--append-system-prompt` montado pelo `launch`, no mesmo argumento dos limites do gestor e do modo `subagents` (`AgentOptions::args`). Ordem:

```
Regras do projeto <name> (versão 7, Control Tower):
<conteúdo>

Regras do épico PROJ-5014 (versão 3, Control Tower):
<conteúdo>
```

- Sessão do projeto: só as do projeto. Sessão do épico: as do projeto e as do épico. Sessão de sub-issue: as do projeto e as do épico pai (`issue_cache.parent_number`). Issue sem épico: só as do projeto. O gestor não recebe regras de projeto.
- `session.resume` passa pelo mesmo `launch` e recalcula do cache, então a sessão retomada recebe a versão atual.
- `ledger_get_context` ganha `rules: { project: { version }, epic: { number, version } | null }`, e há a ferramenta `rules_get` (§10.3) para reler o texto vigente sem retomar a sessão.

Alternativas descartadas:

- **arquivo no worktree** (`CLAUDE.md` ou `.claude/`): suja o `git status`, pode ser commitado (num repositório em que um bot olha o nome da branch e o conteúdo do MR) e some quando a sessão roda no clone principal;
- **só pelo `ledger_get_context`**: depende de o agente chamar a ferramenta, e o texto se perde na compactação. O system prompt sobrevive à retomada e à compactação, que é o mesmo motivo dos limites do gestor.

### 10.2 Armazenamento e entrega

Migração nova do `hub.db` (na #357):

```
agent_rules           id, project_id, epic_number (0 = projeto), version, content,
                      source ('usuario' | 'proposta' | 'importacao'), proposal_id?,
                      author_env_id?, created_at
                      UNIQUE (project_id, epic_number, version)   -- só inclusão (trigger)
agent_rules_proposal  id, uid UNIQUE, project_id, epic_number, base_version, content, reason,
                      state ('pendente' | 'aprovada' | 'rejeitada' | 'obsoleta'),
                      env_id, session_id?, created_at, decided_at?, decision_note?, applied_version?
```

- Uma tabela própria em vez do `proposal` do gestor, que tem `CHECK (kind IN ('priority', 'status'))` e triggers. Mudar o `CHECK` no SQLite obrigaria a recriar a tabela.
- `ConfigProject` ganha `rules: { project: RuleText?, epics: [ { number, version, content } ] }`, só com as versões vigentes. Mudar regras incrementa `config_rev`. O ambiente guarda numa tabela de cache do `ledger.db` (`agent_rules_cache`, substituída a cada pull).
- Limite de 16 KiB por escopo (recusa acima disso), com aviso na UI a partir de 12 KiB.

### 10.3 Ferramentas e propostas

- MCP do ledger (`crates/ct-server/src/mcp.rs`): `rules_get {}` devolve as regras vigentes dos dois escopos da sessão; `rules_propose { scope: "project" | "epic", content, reason }` propõe o **texto completo novo** do escopo.
- A proposta sobe pelo outbox (`SyncChange::RulesProposal { uid, owner, repo, epic_number, base_version, content, reason, session_id }`), e por isso funciona com o hub fora do ar. Fica `pendente` no hub.
- `hub.rules.get|set|import|history` e `hub.rules.proposals.list|decide` no `/ws` do hub. Aprovar grava a versão nova (`source = 'proposta'`). Se a versão vigente já não for a `base_version`, a proposta continua aprovável: a lista marca `base_changed`, a UI avisa e mostra a diferença contra o texto vigente, e aprovar grava o texto completo proposto (decisão da #357; o hub nunca descarta sozinho). `obsoleta` fica para a proposta que chega inválida (acima do limite, sem justificativa), com o motivo. `hub.rules.set|import` levam a `base_version` que o editor abriu e recusam se a vigente mudou.
- Importação: `hub.rules.import { owner, repo, epic_number?, content }` grava uma versão com `source = 'importacao'` (colar o `CLAUDE.md` do exemplo).

## 11. UI (#348, #352, #357)

- **Hub › Projetos** (`ui/src/hub/HubProjects.tsx`): o formulário ganha o tipo do projeto (GitHub, ou Jira + GitLab). No Jira + GitLab: URL do Jira, chave do projeto, JQL opcional, comando de credencial do Jira, URL de clone do GitLab, comando de credencial do GitLab, branch base, protegidas, padrão de branch (com prévia), atribuição ligada ou não e o mapa de tipos (com o padrão preenchido). O detalhe mostra o ambiente de sync (seletor entre os atribuídos), o último `tracker_status` com o `TrackerReadiness` e, para cada item que falta, a instrução de configuração (`jira init`, `pass show ...`). Mostra também as regras do projeto (§10).
- **Pontos com `#n`, "GitHub", "PR" e links do GitHub** passam a usar `itemRef`, `itemUrl` e `changeLabel`: árvore do Projeto (`TreeRow.tsx`, `tree-view.ts`, `ProjectView.tsx`, `Board.tsx`, `WorkItemPanel.tsx`), Sala de controle (`TileGrid.tsx`, `attention.ts`, `Footer.tsx`), Terminal (`TerminalScreen.tsx`, `SessionTree.tsx`; o link de comparação do GitLab é `https://<host>/<path>/-/merge_requests/new?merge_request[source_branch]=<b>&merge_request[target_branch]=<base>`), Revisão (`ReviewScreen.tsx`: "Comentar no MR"), Recuperação (`RecoveryScreen.tsx`), Hub (`HubScreen.tsx`, `HubHealth.tsx`, propostas, quarentena), paleta ⌘K (`shell/palette/items.ts`) e títulos (`shell/titles.ts`, `shell/summary.ts`).
- Ícone do provedor (GitHub ou Jira) na árvore e no detalhe do projeto.
- Regras: em Hub › Projetos (projeto) e no painel do item do épico (épico): editor em Markdown, versão, histórico (quem, quando, de qual proposta) e propostas pendentes com a diferença e os botões aprovar e rejeitar.
- Teste de componente com um projeto Jira/GitLab no `FakeDaemonApi`, sem nenhum texto "GitHub", "PR" ou link para `github.com`.

## 12. Fakes para testes

Não existe `CTOWER_CLAUDE`: o `claude` falso dos testes (`FakeClaude`, `crates/ct-sessions/src/test_support.rs`) entra pelo `claude_bin` do `config.toml`, e o `gh` falso pelo `gh_bin` (`ct_gh::fake::fake_gh`). Os novos seguem o mesmo caminho:

- `ct_jira::fake::FakeJira`: script `jira` que grava argv e a presença de `JIRA_API_TOKEN`/`JIRA_CONFIG_FILE` em `<dir>/jira-calls`, e responde `issue list --raw` a partir de páginas JSON gravadas em `<dir>/pages/<n>.json`. Também simula falhas por arquivo: `401`, `jira me` falhando e prazo estourado. Entra pelo `jira_bin`.
- `ct_gitlab::fake::FakeGlab`: script `glab` que responde `mr list -s <branch> -F json` e `api ...` a partir de `<dir>/mrs/<branch>.json` e `<dir>/branches/<branch>.json`, registra os `DELETE` e confere `GITLAB_HOST` e `GITLAB_TOKEN`. Entra pelo `glab_bin`.
- O comando de credencial dos testes é `printf 'senha-de-teste\n'`, ou `sleep 30` para o prazo, ou `exit 2` para a falha do `gpg-agent`.
- O `ctower hook` não chama o glab nem o jira-cli (§7.4), então não precisa de `CTOWER_GLAB`/`CTOWER_JIRA`.

## 13. Sub-issues: o que cada uma entrega

1. **#347 — modelo neutro** (destrava o resto): `ct_core::tracker` (§3.1, §3.3) e `repo_from_url` (§3.2); `branch_from_pattern` (§4); migrações do `hub.db` e do `ledger.db` (§3.4); campos novos em `HubProject`, `Project`, `ConfigProject`, `ProjectChange`; `rsplit_once` nas chaves; `ref` nas respostas de `ledger_get_context`, `portfolio_*`, saúde e propostas; `never_touch` sem `wip/`; tipos `ts-rs` e protocolos hub 1.3 e ambiente 2.3; testes de migração com as fixtures antigas e um work item Jira passando por ledger, outbox e hub.
2. **#348 — cadastro e ambiente de sync:** `hub.projects.add` com `tracker`/`forge` (sem validação pelo token do GitHub; a validação vem do primeiro `tracker_status`); `sync_env_id` automático e `hub.projects.update { sync_env }` (§5.1); ingestão de `issue_mirror`, `pr_mirror` e `tracker_status` só do ambiente de sync (§5.3); sinal `rastreador_sem_sync` (§5.4); formulário e detalhe em Hub › Projetos com o `TrackerReadiness` (§11).
3. **#349 — sync do Jira:** crate `ct-jira` (§6.1, §6.2), `issue_row` com tipos, épicos e sub-tasks (§6.3), status pela categoria (§6.4), `JiraGitlabSync` com `jira_sync_interval_secs` e erros (§6.5), comando de credencial (§8.1), `TrackerReadiness` no `env.info` (§8.4), espelho enfileirado com `sync_role`, `FakeJira` (§12). Verificação manual contra o `PROJ` registrada na issue.
4. **#350 — GitLab:** `ct_git::clone_url` e o `checkout.rs` pelo host e caminho (§7.1); crate `ct-gitlab` com MRs (§7.2) e branch remota (§7.3); `RemoteBranches` em `cleanup_branches.rs`; `check_glab` (§7.4); `FakeGlab`.
5. **#352 — UI neutra:** `itemRef`/`itemUrl`/`changeLabel` e todos os pontos da §11, com ícone do provedor e testes no `FakeDaemonApi`.
6. **#357 — regras do agente:** tabelas e entrega pelo `/sync/config` (§10.2), injeção no `--append-system-prompt` (§10.1), `rules_get`/`rules_propose` e `SyncChange::RulesProposal` (§10.3), `hub.rules.*` e a UI. Testes: ordem projeto → épico na sessão de sub-issue, issue sem épico só com as do projeto, proposta pendente até a aprovação, histórico com a origem.
7. **#351 — templates e sessões** (depois de #349, #350 e #357): templates `jira_*` e variáveis (§9.1, §9.2), variáveis da sessão (§7.5), `check_jira` (§7.4), `attribution` no `settings.json` (§9.3), aviso de prontidão no `StartSessionModal`, gestor com `ref` e `jira issue view` no prompt. Teste com o agente falso (prompt e variáveis do spawn) e verificação manual no exemplo (issue e comentário no Jira real, MR no GitLab real).

## 14. Decisões

Orientadas pelo usuário (épico #353 e sub-issues), registradas aqui para referência:

1. **Jira pelo jira-cli**, em processo externo; o Jira Server 8.5.2 é legível só por ele.
2. **Ler, criar issues e comentar, sem transição de status no Jira.** O status só é lido para saber se a issue fechou.
3. **Sync no ambiente, subindo pelo `/sync/push`.** O hub não sincroniza esses projetos.
4. **GitLab com MRs pelo `glab`**, para conclusão e limpeza, e `glab mr create` nas sessões.
5. **Credenciais pelo comando de credencial do projeto, rodado no ambiente** (`pass show jira | head -1`), cuja saída vai como `JIRA_API_TOKEN`/`GITLAB_TOKEN` só ao processo; nada passa pelo hub; o limite do cache do `gpg-agent` vira aviso.
6. **Projetos GitHub inalterados.**
7. **Branch configurável por projeto; no exemplo, exatamente `PROJ-NNNN`.**
8. **Commit e MR `[KEY] título`, sem corpo e sem co-author no exemplo; linhas de atribuição desligáveis por projeto.**
9. **Regras do agente por projeto e por épico, editadas pelo usuário, com propostas do agente pendentes até a aprovação.**
10. **Status pela categoria (`statusCategory.key`), tipo por mapa configurável, Epic e Sub-task reconhecidos pelos campos, Epic Link `customfield_10601`, Epic Name `customfield_10604`, saída `--raw`** (verificação da #346).

Autônomas (desta spec, para revisão):

11. **Chave Jira derivada, sem coluna `external_key`** (§3.3). O work item guarda o número da chave em `gh_number`, e a chave é `{project_key}-{n}`. Motivo: nenhuma tabela, outbox nem fusão no hub muda; um projeto Control Tower tem um projeto Jira só, e a issue de outro prefixo é descartada no sync.
12. **Projeto GitLab identificado por `owner` = namespace e `repo` = último segmento, com chave igual ao caminho** (§3.2). Motivo: `project_key`, atribuições, outbox e pasta de worktrees continuam iguais; só dois `split_once('/')` viram `rsplit_once`.
13. **Colunas `tracker_kind`/`tracker_config`/`forge_kind`/`forge_config` com JSON, sem renomear `gh_*`** (§3.4). Motivo: renomear exigiria recriar tabelas com triggers de só inclusão, e os nomes viajam no outbox de ambientes antigos.
14. **Só as combinações GitHub + GitHub e Jira + GitLab no cadastro** (§2). Motivo: são as únicas com projeto real; o modelo comporta as outras sem migração.
15. **Padrão de branch com `{prefix}`, `{n}`, `{key}` e `{slug}`; padrão `{prefix}/{n}-{slug}` no GitHub e `{prefix}/{key}-{slug}` no Jira** (§4). Motivo: mantém o GitHub idêntico e dá `feat/PROJ-5450-…` sem configuração; o exemplo usa `{key}`.
16. **Todo ambiente atribuído sincroniza para uso local; só o ambiente de sync envia o espelho, e o hub só o aceita dele** (§5.2, §5.3). Motivo: é o mesmo desenho do GitHub (cada ambiente sincroniza os seus); os gatilhos de MR e a limpeza não dependem do hub; um escritor só por projeto no espelho do hub.
17. **Ambiente de sync automático na primeira atribuição, sem failover automático; aviso `rastreador_sem_sync`** (§5.1, §5.4). Motivo: só o usuário sabe qual ambiente alcança o Jira, e dois emissores disputariam o espelho.
18. **Espelho no outbox como `issue_mirror`, `pr_mirror` e `tracker_status`** (§5.3). Motivo: reaproveita o envio, o ack e a coalescência existentes; o hub aplica nas mesmas tabelas que o sync do GitHub preenche.
19. **Leitura incremental por `updated >= -<m>m` relativo, com reconciliação diária e no primeiro ciclo (`statusCategory != 3 OR updated >= -30d`)** (§6.2). Motivo: o prazo relativo é calculado pelo Jira, sem depender do fuso; o id 3 da categoria Done não depende da tradução; o recorte de 30 dias é o mesmo do GitHub.
20. **Épico = `has_sub_issues`: papel `epic` no mapa, ou Epic Name preenchido, ou issue com sub-tasks** (§6.3). Motivo: é a semântica que o GitHub já usa e que `session.start`, árvore e limpeza entendem; o Epic Name reconhece o épico mesmo com o tipo em português.
21. **Mapa de tipos com papéis `epic`/`issue`/`bug`/`chore`/`ignore`** (§6.3). Motivo: `bug` e `chore` alimentam o `{prefix}` da branch como as labels do GitHub, e `ignore` tira tipos que não são trabalho.
22. **`glab` em vez da API REST do GitLab** (§7.2). Motivo: o mesmo modelo do jira-cli (processo, prazo e token só no processo filho), com o host self-hosted e a paginação já resolvidos.
23. **`glab mr merge` sempre bloqueado nos projetos GitLab** (§7.4). Motivo: o hook não tem o token para descobrir a base do MR; no exemplo, todo MR vai para a `develop` protegida e o merge é do usuário.
24. **Perfil Jira no guarda-corpo: bloqueia transição e edição, pede confirmação para criar issue e épico** (§7.4). Motivo: torna o "sem transição" e o "criar só com o ok" uma barreira do Claude Code, como no gestor.
25. **O daemon nunca escreve no Jira; o agente comenta o link do MR logo depois de abri-lo** (§7.6). Motivo: mantém o daemon só leitor do Jira, sem novos caminhos de escrita com a senha, e o comentário é o elo entre a issue e o MR.
26. **Token nunca injetado na sessão; o template leva o prefixo com o comando de credencial** (§8.2). Motivo: o modelo do exemplo, que já funciona, e a senha fica fora do ambiente do agente e do `launch_cmd`.
27. **Credencial rodada uma vez por ciclo, por `/bin/sh -c`, com 10 s de prazo e só a primeira linha** (§8.1). Motivo: o comando do usuário tem pipe; o prazo corta o pinentry travado; nada fica em disco.
28. **Fakes pelo `jira_bin`/`glab_bin` do `config.toml`, sem `CTOWER_JIRA`/`CTOWER_GLAB`** (§12). Motivo: é como o `claude_bin` e o `gh_bin` já funcionam; o hook não chama esses binários.
29. **Regras do agente injetadas pelo `--append-system-prompt`, recalculadas a cada início e retomada, com `rules_get` para reler** (§10.1). Motivo: sobrevivem à compactação, não sujam o worktree e reaproveitam o mecanismo dos limites do gestor.
30. **Regras e propostas de regras em tabelas próprias, com proposta do texto completo pelo outbox** (§10.2, §10.3). Motivo: o `CHECK` do `proposal` do gestor exigiria recriar a tabela; o texto completo dá a diferença direta na UI e aplica sem merge; o outbox funciona offline.
31. **Formato do commit e do MR nas regras do projeto, e só a atribuição como campo (`attribution`)** (§9.3). Motivo: o formato é texto que o agente segue; só a atribuição precisa de configuração do Claude Code (`settings.json` da sessão).
32. **Templates `jira_issue`/`jira_epic`/`jira_project` no `ui.json`, escolhidos pelo rastreador** (§9.1). Motivo: os de GitHub ficam intocados e o usuário edita os de Jira no mesmo lugar.
33. **Intervalo próprio `jira_sync_interval_secs = 300`** (§6.5). Motivo: o Jira 8.5.2 é lento, e cada ciclo roda processos e o `pass`.
34. **Protocolo do hub 1.3 e do ambiente 2.3** (§3.5). Motivo: só entram campos, métodos e variantes do outbox.

## 15. Riscos e verificações pendentes

- O `-q` do jira-cli com `-p`, o formato do `jira serverinfo` e se o `--raw` da listagem traz os `customfield_*` sem `--columns` (#349, contra o `PROJ`).
- O `osxkeychain` a partir do `ctowerd` no launchd (#350).
- ~~As chaves `attribution`/`includeCoAuthoredBy` na versão instalada do Claude Code (#351).~~ Conferidas no schema de settings do Claude Code 2.1.312: `attribution.commit`/`attribution.pr` (texto vazio esconde) e `includeCoAuthoredBy` (antiga, ainda lida). Falta a conferência numa sessão real.
- Volume do primeiro espelho: as issues abertas do `PROJ` podem passar de mil, e o primeiro envio do ambiente de sync é grande. O outbox já envia em lotes; a #349 mede.
- O `PATH` do `ctowerd` precisa achar `jira` e `glab` (`/opt/homebrew/bin` no macOS); `jira_bin`/`glab_bin` aceitam caminho absoluto.
