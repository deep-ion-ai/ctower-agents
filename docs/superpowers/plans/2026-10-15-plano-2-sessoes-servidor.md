# Control Tower — Plano 2: Sessões e servidor — Implementation Plan

> **For agentic workers:** execute task a task, em TDD (teste falhando → implementação → teste passando → commit). Cada task corresponde a uma sub-issue do épico no GitHub; referencie a issue no commit (`refs #N`) e feche-as pelo PR.

**Goal:** Entregar o daemon `ctowerd`: lança sessões Claude Code em PTY dentro do worktree do work item, observa-as pelos hooks, faz backup de transcripts, marca sessões `interrupted` no boot e permite retomá-las — tudo exposto por HTTP (`/hooks`, `/mcp`, `/ws`) e rodando como serviço systemd na distro.

**Architecture:** Novos crates `ct-pty` (PTY + ring buffer + fan-out, síncrono com threads), `ct-sessions` (orquestração; síncrono, sobre `ct-ledger`/`ct-git`/`ct-pty`), `ct-server` (`axum` 0.8 + `tokio`; único código assíncrono) e o binário `ctowerd`. O `Ledger` (rusqlite, não `Sync`) fica atrás de `Arc<Mutex<Ledger>>` dentro de `ct-sessions`; o servidor chama `ct-sessions` via `spawn_blocking` quando a operação toca disco/processos.

**Tech Stack:** o do Plano 1 + `portable-pty` 0.9, `tokio` 1 (rt-multi-thread, macros, sync, time, signal), `axum` 0.8 (feature `ws`), `tower-http` (trace), `tracing`/`tracing-subscriber`, `rand` (tokens), `tokio-tungstenite` (só testes), `reqwest` ou `ureq` (testes).

**Épico:** #4 (sub-issues #5–#13)

**Spec:** `docs/superpowers/specs/2026-10-15-control-tower-design.md` — seções 7.1, 7.2, 7.4, 8, 9, 11, 12, 13.

## Verificado contra o Claude Code 2.1.301 (spec §16)

- `--session-id <uuid>` existe: o daemon **gera o `tool_session_id`** e o passa no lançamento. O hook `SessionStart` apenas confirma (e registra `transcript_path`).
- `--resume <id>`, `--mcp-config <arquivo>` e `--settings <arquivo>` existem.
- **Desvio da spec §7.2 (para revisão):** em vez de instalar hooks no `~/.claude/settings.json` global (com backup/merge), o daemon gera `<data_dir>/sessions/<id>/settings.json` com os hooks e passa `--settings`. Vantagens: zero alteração em arquivo do usuário, nada a desfazer, hooks só existem em sessões gerenciadas. O comportamento "hook sem `CTOWER_SESSION` sai com 0" continua valendo.
- Payload comum dos hooks: `session_id`, `transcript_path`, `cwd`, `hook_event_name`; `Notification` traz `message`; `SessionEnd` traz `reason`. A Task 9 confere isso numa sessão real.

## Global Constraints

- Todas as do Plano 1 continuam valendo (status/estados como strings exatas, append-only, PT-BR, Unix apenas, nunca commitar em `develop`).
- Daemon escuta só em `127.0.0.1` (porta padrão `7878`, configurável).
- Dois tokens em `~/.config/ctower/` com permissão `0600`: `ui.token` (só `/ws`) e `hook.token` (só `/hooks` e `/mcp`). Token errado → `401`; token de hook em `/ws` → `401`.
- Diretórios: config `~/.config/ctower/` (`config.toml`, tokens), dados `~/.local/share/ctower/` (`ledger.db`, `transcripts/`, `hooks-spool/`, `sessions/<id>/`). Todos sobrescrevíveis por `CTOWER_CONFIG_DIR`/`CTOWER_DATA_DIR` (testes).
- Binário do agente configurável (`claude_bin`, padrão `claude`; testes usam um script falso).
- Ring buffer de scrollback: 4 MiB por sessão.
- Backup de transcript: em todo `Stop` e a cada 5 min para sessões `running`; cópia atômica (tmp + rename) para `<data_dir>/transcripts/<tool_session_id>.jsonl`.
- **Gravar antes de agir:** sessão `starting` + evento gravados antes do spawn; PID gravado logo após.
- Retomada nunca é automática.
- Protocolo WS versão `1.0` (`env.info`).

## Review Focus

1. **Queda do daemon** — matar o daemon com sessões vivas e subir de novo: todas viram `interrupted` uma única vez; nenhuma `exited` muda. Teste na Task 8/9.
2. **Retomar com transcript só no backup** — restaura no caminho original antes do `--resume`; sessão antiga → `archived` com evento `resumed_as`. Task 6.
3. **Isolamento de tokens** — token de hook não abre `/ws`; sem token nada responde além de `401`. Task 7.
4. **Hook de sessão desconhecida/encerrada** — não derruba o servidor nem cria registros órfãos; responde `202` e grava evento informativo quando houver projeto. Task 4/7.
5. **Ring buffer** — snapshot nunca excede 4 MiB e preserva os bytes mais recentes; vários assinantes recebem a mesma saída. Task 2.

---

## File Structure

```
crates/
├─ ct-pty/src/lib.rs              # PtySession: spawn, write, resize, kill, wait, subscribe, snapshot
├─ ct-pty/src/ring.rs             # RingBuffer
├─ ct-sessions/src/
│  ├─ lib.rs                      # Sessions (fachada), SessionsConfig
│  ├─ paths.rs                    # Dirs (config/data), tokens
│  ├─ launch.rs                   # start/resume: worktree, mcp.json, settings.json, comando, spawn
│  ├─ hooks.rs                    # apply_hook(HookEnvelope), spool
│  ├─ transcripts.rs              # backup atômico, integridade, local volátil
│  └─ recovery.rs                 # boot, recovery_list, archive
├─ ct-server/src/
│  ├─ lib.rs                      # router(state), AppState, auth
│  ├─ hooks.rs                    # POST /hooks
│  ├─ mcp.rs                      # POST /mcp (JSON-RPC 2.0, streamable HTTP sem SSE)
│  └─ ws.rs                       # /ws: métodos, push de eventos, frames de terminal
├─ ctowerd/src/main.rs            # config, boot, tarefas periódicas, serve, sinais
└─ ctower/src/status.rs           # `ctower status`, `ctower restart [--force]`
packaging/systemd/ctowerd.service
scripts/provision/provision.sh    # + build/instalação de ctowerd/ctower, tokens, serviço
```

Ajustes em crates existentes (feitos na task que precisar, com teste):
- `ct-ledger`: `list_sessions(work_item_id)`, `find_session_by_tool_id`, `dashboard()` (work items com sessão viva/aguardando), `archive_session_resumed_as(old, new)`.

---

### Task 1: `ct-pty` — ring buffer (#5)

**Files:** Create `crates/ct-pty/{Cargo.toml,src/lib.rs,src/ring.rs}`.

**Interface:**
```rust
pub struct RingBuffer { /* VecDeque<u8>, cap */ }
impl RingBuffer {
    pub fn new(cap: usize) -> Self;
    pub fn push(&mut self, bytes: &[u8]);      // descarta os mais antigos ao exceder cap
    pub fn snapshot(&self) -> Vec<u8>;
    pub fn len(&self) -> usize;
}
```

**Testes:** push menor que cap; push que ultrapassa (mantém os últimos `cap` bytes); push único maior que cap; snapshot vazio.

- [ ] Testes falhando → implementar → passar → `cargo clippy -D warnings` → commit `feat(pty): ring buffer de scrollback`.

### Task 2: `ct-pty` — sessão em PTY com fan-out (#6)

**Interface:**
```rust
pub struct SpawnSpec { pub program: String, pub args: Vec<String>, pub cwd: PathBuf,
                       pub env: Vec<(String, String)>, pub cols: u16, pub rows: u16 }
pub struct PtySession;  // Clone (Arc interno)
impl PtySession {
    pub fn spawn(spec: SpawnSpec, scrollback: usize) -> io::Result<Self>;
    pub fn pid(&self) -> Option<u32>;
    pub fn write(&self, bytes: &[u8]) -> io::Result<()>;
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()>;
    pub fn kill(&self) -> io::Result<()>;
    /// Snapshot do ring + receiver ao vivo, atômicos entre si (sem perder nem duplicar bytes).
    pub fn attach(&self) -> (Vec<u8>, std::sync::mpsc::Receiver<Vec<u8>>);
    /// Bloqueia até sair; Some(código) ou None se morto por sinal.
    pub fn wait(&self) -> Option<i32>;
    pub fn on_exit(&self, f: impl FnOnce(Option<i32>) + Send + 'static);
}
```
Thread leitora: lê do master, sob um único `Mutex` faz `ring.push` + envia a todos os assinantes (remove os desconectados).

**Testes:** `bash -c 'echo ola; exit 3'` → saída contém `ola`, `wait()==Some(3)`; dois `attach` recebem os mesmos bytes; `write("echo x\n")` em `bash` interativo ecoa; `kill()` encerra; env e cwd aplicados (`pwd`, `echo $FOO`).

- [ ] TDD → commit `feat(pty): sessões em PTY com scrollback e fan-out`.

### Task 3: `ct-sessions` — diretórios, tokens e lançamento (#7)

**Interface:**
```rust
pub struct SessionsConfig { pub dirs: Dirs, pub port: u16, pub claude_bin: String,
                            pub worktrees_root: PathBuf, pub scrollback: usize, pub hook_token: String }
pub struct Dirs { pub config: PathBuf, pub data: PathBuf }   // Dirs::from_env()
pub fn ensure_token(path: &Path) -> io::Result<String>;      // cria 0600 se não existe (32 bytes hex)
pub struct Sessions { ledger: Arc<Mutex<Ledger>>, live: Mutex<HashMap<String, PtySession>>, cfg, events: broadcast-like callback }
pub struct StartRequest { pub project_id: i64, pub gh_number: i64, pub kind: WorkItemKind,
                          pub title: String, pub labels: Vec<String>, pub prompt: String }
impl Sessions {
    pub fn start(&self, req: StartRequest) -> Result<Session>;
    pub fn pty(&self, session_id: &str) -> Option<PtySession>;
    pub fn kill(&self, session_id: &str) -> Result<()>;
}
```
Fluxo de `start` (spec §7.1): `ensure_work_item` → branch (`branch_for`) → `ensure_worktree` em `<worktrees_root>/<projeto>/<worktree_dir_name>` + `set_work_item_branch` → gera `tool_session_id` (uuid v4) → `create_session(starting)` + evento → escreve `sessions/<id>/mcp.json` (`{"mcpServers":{"ctower":{"type":"http","url":"http://127.0.0.1:<porta>/mcp","headers":{"Authorization":"Bearer <hook>","X-Ctower-Session":"<id>"}}}}`) e `settings.json` (hooks dos 6 eventos chamando `ctower hook <evento>`; `PreToolUse` com matcher `Bash`) → spawn `claude --session-id <uuid> --mcp-config <mcp.json> --settings <settings.json> "<prompt>"` com `CTOWER_SESSION/URL/TOKEN/PROTECTED` → `set_session_pid` → `apply_trigger(SessionActive)` → `on_exit`: `transition_session(exited, code)` + evento `session_exit` se código ≠ 0.

**Testes** (repo git temporário, `claude_bin` = script que imprime argv/env e dorme): worktree e branch criados; arquivos `mcp.json`/`settings.json` corretos e `0600`; argv contém `--session-id` com o uuid gravado; env exportado; sessão `starting` com pid; processo saindo com 3 → `exited`, `exit_code=3`; worktree ocupado por outra branch → erro explícito e **nenhuma** sessão gravada (o worktree é validado/criado antes de `create_session`; a criação do worktree é ela mesma precedida do evento `worktree_requested` no work item, para cumprir "gravar antes de agir").

- [ ] TDD → commit `feat(sessions): lançamento de sessões em worktree com hooks por sessão`.

### Task 4: `ct-sessions` — aplicação de hooks e spool (#8)

**Interface:**
```rust
#[derive(Deserialize)] pub struct HookEnvelope { pub session: String, pub event: String, pub payload: Value, pub ts: i64 }
impl Sessions {
    pub fn apply_hook(&self, h: &HookEnvelope) -> Result<HookOutcome>;  // HookOutcome::{Applied, UnknownSession, Ignored}
    pub fn drain_spool(&self) -> Result<usize>;  // processa e remove arquivos de hooks-spool/ em ordem de nome
}
```
Efeitos (spec §7.2): `SessionStart` → `set_session_tool_info` + `running` + alerta `transcript_volatile` se sob `/tmp`, `/run` ou tmpfs (`findmnt`/`/proc/mounts`); `UserPromptSubmit` → `running` + `SessionActive`; `Notification` → `NeedsHuman` + evento `needs_human {message}`; `Stop` → `idle` + backup (Task 5); `SessionEnd` → `exited`; `GuardBlocked` → evento `guard_blocked`. Transição inválida (ex.: hook atrasado de sessão `exited`) → `Ignored`, sem erro. Spool com JSON inválido → arquivo movido para `hooks-spool/bad/`.

**Testes:** cada evento; ordem fora do esperado; sessão desconhecida; spool com 3 arquivos (um inválido).

- [ ] TDD → commit `feat(sessions): aplicação de hooks e processamento do spool`.

### Task 5: `ct-sessions` — backup de transcripts (#9)

**Interface:** `pub fn backup_transcript(src: &Path, dst_dir: &Path, tool_session_id: &str) -> io::Result<PathBuf>`; `pub enum TranscriptIntegrity { Original, RestorableFromBackup, Lost }`; `pub fn integrity(s: &Session) -> TranscriptIntegrity`; `Sessions::backup_running()` (chamado a cada 5 min pelo daemon).

**Testes:** cópia atômica (sem `.tmp` residual; conteúdo igual); origem ausente → erro sem apagar backup anterior; integridade nos 3 casos; `set_session_backup_path` gravado.

- [ ] TDD → commit `feat(sessions): backup atômico de transcripts`.

### Task 6: `ct-sessions` — boot, recuperação, retomada e arquivamento (#10)

**Interface:**
```rust
impl Sessions {
    pub fn boot(&self) -> Result<Vec<String>>;               // mark_interrupted(pid vivo via kill(pid,0)) + drain_spool
    pub fn recovery_list(&self) -> Result<Vec<RecoveryItem>>; // issue, branch, worktree, último evento, parado há, integridade, git status --short resumido
    pub fn resume(&self, session_id: &str, prompt: Option<String>) -> Result<Session>;
    pub fn archive(&self, session_id: &str) -> Result<()>;
}
```
`resume` (spec §7.4): exige `interrupted` (ou `exited`); restaura transcript do backup se o original sumiu; cria **nova** sessão (mesmo `tool_session_id`, `cwd`) com `claude --resume <tool_session_id> ... "<prompt de retomada>"`; antiga → `archived` + evento `resumed_as {new}` (`archive_session_resumed_as` no ledger, numa transação).

**Testes:** boot com pid morto / vivo / sem pid; boot duas vezes; resume com original, com backup (restaura) e com `Lost` (erro explícito); archive.

- [ ] TDD → commit `feat(sessions): recuperação no boot, retomada e arquivamento`.

### Task 7: `ct-server` — HTTP, autenticação, `/hooks` e `/mcp` (#11)

**Interface:** `pub fn router(state: AppState) -> axum::Router`; `AppState { sessions: Arc<Sessions>, ui_token, hook_token, events: tokio::sync::broadcast::Sender<PushEvent> }`.

- `POST /hooks` (token de hook): corpo `HookEnvelope` → `apply_hook` em `spawn_blocking` → `202` sempre que autenticado (mesmo `UnknownSession`).
- `POST /mcp` (token de hook + `X-Ctower-Session`): JSON-RPC 2.0, resposta `application/json` (sem SSE). Métodos: `initialize` (protocolVersion ecoada, `capabilities.tools`, `instructions` orientando `orientada` × `autonoma`), `notifications/initialized` (`202`), `tools/list`, `tools/call` para `ledger_record_decision`, `ledger_list_decisions`, `ledger_set_status`, `ledger_get_context` (spec §8). `GET /mcp` → `405`.

**Testes** (`tower::ServiceExt::oneshot`): sem token/ token errado/ token de UI em `/hooks` → `401`; hook aplicado muda o ledger; MCP: initialize, tools/list com 4 tools e schemas, record+list decision (origem `orientada` nasce revisada), set_status `bloqueado`, get_context; sessão desconhecida → erro JSON-RPC.

- [ ] TDD → commit `feat(server): endpoints de hooks e MCP do ledger`.

### Task 8: `ct-server` — `/ws` (controle + terminal) (#12)

Protocolo (spec §9): handshake com `Authorization: Bearer <ui-token>`; texto = JSON `{id,method,params}` → `{id,result|error}`; push `{event,data}`; binário `[canal u8][uuid 16 bytes][bytes]` (canal 0 = saída, 1 = entrada).

Métodos neste plano: `env.info` (`{protocol:"1.0", version, hostname}`), `projects.list|add`, `dashboard.get`, `session.start|resume|archive|kill|attach|detach|input|resize`, `recovery.list`. (`tree.get`, `review.*`, `sync.now` ficam para os Planos 3/4.) `attach` envia o snapshot como frame binário e depois o fluxo ao vivo; `detach`/desconexão encerram o encaminhamento.

**Testes** (servidor real em porta efêmera + `tokio-tungstenite`): handshake sem token → 401; `env.info`; `projects.add/list`; `session.start` com `claude_bin=bash` script → push `session_state`; `attach` recebe snapshot + saída nova; `input` chega ao processo; dois clientes anexados recebem a mesma saída; método desconhecido → erro.

- [ ] TDD → commit `feat(server): websocket da UI com controle e terminal`.

### Task 9: `ctowerd`, systemd, `ctower status/restart` e teste de queda (#13)

- `ctowerd`: lê `config.toml` (porta, `claude_bin`, `worktrees_root`), garante tokens, abre ledger, `boot()`, tarefas periódicas (spool a cada 30 s, backup a cada 5 min), `serve` em `127.0.0.1:<porta>`, encerra limpo em SIGTERM.
- `packaging/systemd/ctowerd.service` (user unit, `Restart=on-failure`, `KillMode=control-group` documentado); `provision.sh` passa a fazer `cargo install --path crates/ctowerd --path crates/ctower`, gerar tokens e `systemctl --user enable --now ctowerd` (com `loginctl enable-linger`).
- `ctower status` (GET `/healthz` + resumo via token de UI) e `ctower restart [--force]` (recusa se houver sessões vivas, listando-as — spec §12).
- **Teste de integração "queda":** sobe `ctowerd` como processo filho com dirs temporários, inicia sessão (`claude_bin` falso de longa duração), mata o daemon e os filhos (`SIGKILL` no grupo), sobe de novo → sessão `interrupted` e listada em `recovery.list`; `session.resume` cria nova sessão com `--resume <uuid>`.
- **Smoke manual com Claude real** (registrar resultado no PR): iniciar sessão numa issue de teste, verificar `SessionStart` com `transcript_path`, `Stop` → `idle` + backup, `Notification` → `aguardando_voce`, decisão via MCP, guarda-corpo bloqueando `git push origin develop`.

- [ ] TDD → commit `feat(ctowerd): daemon, serviço systemd e recuperação após queda`.

---

## Fora deste plano

`ct-gh` e gatilhos de PR (Plano 3); `tree.get`, `review.*`, `sync.now`, UI e notificações nativas (Plano 4); geração de tipos TS (`ts-rs`) entra junto com a UI.
