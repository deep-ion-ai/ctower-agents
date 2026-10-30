# Control Tower — Plano 4: UI Tauri — Implementation Plan

> **For agentic workers:** execute task a task, em TDD (teste falhando → implementação → teste passando → commit). Cada task corresponde a uma sub-issue do épico; o PR leva `Closes #N` de cada uma.

**Goal:** Entregar o app de Windows (Tauri) que conecta aos daemons e mostra a sala de controle: o que está rodando, o que espera o humano e o que terminou; terminal embutido; árvore do projeto com ▶ Iniciar; recuperação; revisão de decisões; configurações e notificações nativas. O instalador é gerado no GitHub Actions.

**Architecture:** A conexão com os daemons fica em Rust. O novo crate `ct-client` (tokio + tokio-tungstenite, no workspace) implementa o protocolo `/ws`: token no handshake, requisição/resposta, pushes, frames de terminal, reconexão com backoff e re-attach. Ele é testado contra o `ct-server` real no CI de Linux e macOS. O app em `ui/` tem duas partes. O lado Rust (`ui/src-tauri`) é uma casca fina sobre o `ct-client`, que expõe comandos e eventos Tauri e dispara as notificações nativas. O lado web (React + TypeScript + xterm.js) só apresenta. Os tipos TypeScript do protocolo são gerados dos tipos Rust (`ts-rs`).

**Tech Stack:**
- **App:** Tauri 2 com `tauri-plugin-notification`, `tauri-plugin-opener` e `tauri-plugin-store`.
- **Frontend:** React 18, TypeScript 5, Vite 6, `@xterm/xterm` 5 com `@xterm/addon-fit`, vitest e `@testing-library/react`.
- **Rust:** `ts-rs` 10 e `tokio-tungstenite` 0.27, o mesmo do workspace.
- Versões exatas fixadas no lockfile da Task 3.

**Épico:** #27 (sub-issues #28–#34)

**Spec:** `docs/superpowers/specs/2026-10-15-control-tower-design.md`:
- §7.6: revisão.
- §9: protocolo.
- §10: telas.
- §12: reconexão da UI.
- §13: instalador e `wsl --exec true`.
- §14: contrato tipado e testes de UI.

**Mockup das telas:** canvas "Control Tower — UI" (link no PR do plano). Ele é a referência visual das Tasks 4–7.

## Restrições verificadas no ambiente

- **WebSocket do navegador:** não envia cabeçalho `Authorization`, que o `/ws` exige (spec §9). Por isso a conexão é feita no lado Rust do Tauri. O token nunca chega ao JavaScript.
- **Máquina Windows do usuário:** tem Node 24, mas não tem Rust nem MSVC. O build do app Tauri acontece no GitHub Actions (`windows-latest`). Na distro rodam o `ct-client` e os testes do frontend, com Node 22.

## Decisões tomadas neste plano

1. **`ct-client` no workspace, `ui/src-tauri` fora dele** (`exclude` no `Cargo.toml` raiz, com `Cargo.lock` próprio). O crate do Tauri precisa de WebView2/webkit para compilar. Por isso fica só no job Windows, e a lógica testável fica toda no `ct-client`.
2. **Ponte Tauri:**
   - comandos `envs_list`, `envs_save`, `call(env, method, params)`, `attach(env, session, channel)`, `detach`, `input`, `resize`, `open_url`, `open_vscode(env, path)`;
   - eventos `daemon://push` (`{env, event, data}`) e `daemon://status` (`{env, connected, since}`).
   - A saída de terminal vai por `tauri::ipc::Channel<Vec<u8>>`, um por attach.
3. **Ambientes:**
   - ficam em `envs.json` no diretório de dados do app (`tauri-plugin-store`);
   - para o tipo `wsl`, o token é lido da distro a cada conexão (`wsl.exe -d <distro> -- cat ~/.config/ctower/ui.token`) e não é gravado no Windows;
   - para o tipo `remote` (fase 2), o token fica no `envs.json`.
   - Ao abrir, o app roda `wsl.exe -d <distro> --exec true` (spec §13).
4. **Reconexão:** backoff de 1 s, dobrando até 30 s. Ao reconectar, o `ct-client` refaz o `attach` de cada sessão anexada e entrega um novo snapshot. O xterm é limpo antes, para não duplicar a saída.
5. **Tiles com as últimas linhas:** usam o método novo `session.peek` (até 4 KiB do scrollback, sem anexar), consultado a cada 2 s só enquanto a Sala de controle está visível. Anexar todas as sessões transmitiria 4 MiB de snapshot por tile.
6. **Faixa de atenção:** é uma função pura `attention(dashboard, envStatus, now)`, testada à parte. Sinais de entrada:
   - sessões em `idle`/`running` com work item `aguardando_voce`;
   - sessões `interrupted`;
   - sessões `exited` com `exit_code ≠ 0` nas últimas 24 h;
   - alertas (`transcript_volatile`, `trust_failed`);
   - ambiente desconectado;
   - sync atrasado (último sucesso há mais de 3 intervalos).
7. **Notificações nativas:** disparadas no lado Rust, a partir dos pushes `needs_human`, `session_exited` (código ≠ 0) e da queda de ambiente. Assim funcionam com a janela minimizada (spec §10). Há uma preferência por tipo.
8. **Templates de prompt:** ficam na configuração da UI, com as variáveis da spec (`{{kind}}`, `{{number}}`, `{{owner}}`, `{{repo}}`, `{{branch}}`, `{{protected}}`). A prévia da branch usa a mesma regra do `ct_core::branch_for`, reimplementada em TS e testada com os mesmos casos.
9. **Contrato tipado:** `ts-rs` atrás da feature `ts` em `ct-core`, `ct-ledger` e `ct-sessions`. Um teste exporta os tipos para `ui/src/protocol/gen/`, e o CI falha se `git diff --exit-code` acusar diferença.
10. **Instalador:** NSIS, sem assinatura de código no MVP. Fica como artefato do workflow, sem release automático.

## Global Constraints

- Todas as dos planos anteriores: PT-BR, nunca commitar em `develop`, CI verde em Ubuntu e macOS para o Rust.
- A UI não tem estado de domínio próprio (spec §3). Tudo vem do daemon, exceto ambientes, templates e preferências de notificação.
- O protocolo é versionado: a UI recusa daemon com versão major diferente (`env.info`) e mostra um aviso.
- Acessibilidade:
  - botões e links reais;
  - foco visível;
  - contraste mínimo de 4,5:1;
  - estados distinguíveis também por texto, não só por cor.
- Visual do mockup:
  - fundo escuro, com IBM Plex Sans/Mono empacotadas no app;
  - âmbar só para o que exige o humano.

## Review Focus

1. **Reconexão com terminal aberto:** o daemon reinicia com o xterm anexado. A UI deve mostrar "desconectado", reconectar sozinha e voltar a exibir a sessão sem saída duplicada. Testes nas Tasks 2 e 5.
2. **Token fora do JavaScript:** nenhum comando ou evento Tauri devolve o token. Teste na Task 3, que inspeciona os payloads.
3. **Daemon com protocolo incompatível:** major diferente gera aviso e nenhuma chamada. Teste na Task 2.
4. **Faixa de atenção vazia:** com tudo andando, a faixa some, em vez de mostrar uma caixa vazia. Teste na Task 4.
5. **Sessão encerrada com a aba aberta:** a aba mostra "sessão encerrada (código N)" e para de aceitar entrada. Teste na Task 5.

---

## File Structure

```
crates/
├─ ct-core, ct-ledger, ct-sessions   # + derive(TS) atrás da feature "ts"
├─ ct-server/src/ws.rs               # + projects.update, session.peek, dashboard ampliado
└─ ct-client/src/
   ├─ lib.rs                         # Client: connect, call, events(), attach/detach/input/resize
   └─ reconnect.rs                   # supervisor: backoff, status, re-attach
ui/
├─ package.json, vite.config.ts, tsconfig.json, index.html
├─ src/
│  ├─ protocol/gen/                  # tipos gerados (ts-rs)
│  ├─ api/daemon.ts                  # DaemonApi (invoke/listen) + FakeDaemonApi para testes
│  ├─ control-room/                  # Sidebar, AttentionStrip (+ attention.ts), TileGrid, Footer
│  ├─ terminal/                      # TerminalTabs, TerminalView, adapter.ts
│  ├─ project/                       # ProjectTree, StartSessionPanel, templates.ts, branch.ts
│  ├─ recovery/, review/, settings/
│  └─ App.tsx, theme.css, fonts/
└─ src-tauri/
   ├─ Cargo.toml, tauri.conf.json, capabilities/default.json
   └─ src/{main.rs, bridge.rs, envs.rs, notify.rs, wsl.rs}
.github/workflows/ui.yml             # Ubuntu: typecheck + vitest + vite build; Windows: tauri build → artefato
```

---

### Task 1: lacunas do daemon e tipos gerados (#28)

**Interfaces:**
- `projects.update {id, name?, repo_path?, base_branch?, protected_branches?}`, que usa um novo `Ledger::update_project`.
- `session.peek {session_id, bytes?≤4096}` → `{ data: <string UTF-8 com substituição> }`. É feito pelo novo `PtySession::tail(n)` no `ct-pty`.
- `dashboard.get`: cada work item ganha `title` (do cache, quando houver) e `pending_decisions` (autônomas não revisadas, somando as sub-issues no caso de épico).
- `#[cfg_attr(feature = "ts", derive(ts_rs::TS))]` nos tipos serializados para a UI. Um teste `export_ts` grava em `ui/src/protocol/gen/`.

**Testes:**
- `update_project` parcial;
- `peek` de sessão viva, desconhecida e com limite;
- dashboard com título e contagem;
- tipos gerados atualizados (CI: `cargo test -p ct-server --features ts export_ts && git diff --exit-code ui/src/protocol/gen`).

### Task 2: `ct-client` (#29)

**Interfaces:**
```rust
pub struct EnvConfig { pub name: String, pub url: String, pub token: String }
pub enum ClientEvent { Status { connected: bool }, Push { event: String, data: Value }, Output { session: String, bytes: Vec<u8> } }
pub struct Client;   // Clone; supervisor de reconexão interno
impl Client {
    pub fn spawn(cfg: EnvConfig) -> (Self, tokio::sync::mpsc::UnboundedReceiver<ClientEvent>);
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, ClientError>;
    pub async fn attach(&self, session: &str) -> Result<(), ClientError>;   // snapshot chega como Output
    pub async fn detach(&self, session: &str) -> Result<(), ClientError>;
    pub fn input(&self, session: &str, bytes: &[u8]) -> Result<(), ClientError>;
    pub async fn resize(&self, session: &str, cols: u16, rows: u16) -> Result<(), ClientError>;
}
pub enum ClientError { Disconnected, Rpc { code: i64, message: String }, Incompatible { daemon: String }, Timeout }
```

**Testes** (`ct-server` real numa porta efêmera, com `FakeClaude`):
- token errado → `Status{connected:false}` sem loop apertado;
- `call` com resposta e com erro;
- pushes;
- attach com snapshot e entrada;
- servidor derrubado e religado → `Status false/true` e re-attach com novo snapshot;
- `call` durante a queda → `Disconnected`;
- daemon com `protocol` `2.0` → `Incompatible`.

### Task 3: esqueleto do app e CI (#30)

- **Projeto:** `ui/` criado com Vite (React + TS) e `src-tauri`. Fontes Plex empacotadas, `theme.css` com os tokens do mockup.
- **`bridge.rs`:** um `Client` por ambiente; repassa `ClientEvent` como eventos Tauri e terminal por `Channel`.
- **`wsl.rs`:** bootstrap e leitura do token. **`envs.rs`:** persistência dos ambientes.
- **`api/daemon.ts`:** interface `DaemonApi` com a implementação Tauri e um `FakeDaemonApi`, usado em todos os testes de componente.
- **Workflow `ui.yml`:**
  - job `ui-web` (Ubuntu): `npm ci`, `tsc --noEmit`, `vitest run`, `vite build`;
  - job `ui-windows`: `npm ci`, `npx tauri build`, upload do instalador NSIS.
- **Testes:**
  - `envs.rs`: ida e volta do arquivo; o token do tipo `wsl` não é gravado;
  - `bridge.rs`: os payloads serializados não contêm o token;
  - frontend: `DaemonApi` fake e roteamento das telas.

### Task 4: Sala de controle (#31)

- **Componentes:** `Sidebar`, `AttentionStrip`, `TileGrid` (com `session.peek` a cada 2 s enquanto visível) e `Footer`, conforme o mockup.
- **Atualização ao vivo:** pushes `session_*`, `work_item_status`, `needs_human` e `sync_status` disparam `dashboard.get` com debounce de 300 ms.
- **Testes:**
  - `attention.ts`: tabela de casos com todos os sinais da decisão 6 e a faixa vazia;
  - componentes: tile abre o terminal; contadores da lateral.

### Task 5: terminal (#32)

- **Componentes:** `TerminalTabs` e `TerminalView` (xterm + fit), mais `adapter.ts`, que faz a ponte entre os bytes do `Channel` e o `term.write` e entre o `onData` e o `input`, com resize debounced.
- **Barra de ações:**
  - "Abrir no VS Code" roda `code --remote wsl+<distro> <worktree>` no lado Rust;
  - "Abrir PR" usa o `pr_cache` ou, sem PR, a URL de criação;
  - "Encerrar" pede confirmação na própria UI (sem `confirm()` nativo).
- **Testes** (adapter com xterm fake):
  - snapshot limpa antes de escrever;
  - entrada vai para a sessão certa;
  - sessão encerrada bloqueia a entrada e mostra o código;
  - re-attach após reconexão não duplica a saída.

### Task 6: Projeto, Recuperação e Revisão (#33)

- **Projeto:**
  - `tree.get`, com o épico expansível e status por cor e texto;
  - ▶ Iniciar abre o painel com ferramenta, prévia de branch/worktree e prompt do template;
  - `session.start` só com número e prompt.
- **Recuperação:** `recovery.list`; abre sozinha no boot se houver `interrupted`; Retomar (com prompt editável) e Arquivar; transcript `perdido` oferece "Nova sessão com contexto".
- **Revisão:** `review.get`, "Marcar N como revisadas" (`review.mark_reviewed`, com o PR do `pr_cache`) e "Abrir PR no GitHub".
- **Testes:** `branch.ts` com os mesmos casos do `ct_core::branch`; `templates.ts`; componentes de Recuperação (abre sozinha; perdido sem Retomar) e de Revisão (contagem no botão, lista vazia após marcar).

### Task 7: Configurações, notificações e roteiro manual (#34)

- **Configurações:** ambientes (adicionar e editar; token oculto), projetos (`projects.add`/`projects.update`), templates e preferências de notificação.
- **`notify.rs`:** regras da decisão 7, respeitando as preferências.
- **`docs/ui-roteiro-manual.md`:** instalar o artefato do CI, conectar, iniciar sessão, responder permissão pelo terminal, derrubar o daemon e ver a reconexão, recuperar, revisar.
- README e roadmap, com os Planos 3 e 4 concluídos.
- **Testes:** `notify.rs` (regras e preferências); componentes de Configurações. Roteiro manual executado no Windows, com o resultado no PR.

---

## Fora deste plano

Daemon no macOS, Codex e UI no macOS (fase 2); assinatura de código e auto-update do instalador; controle de custo e tokens (fora de escopo da spec).
