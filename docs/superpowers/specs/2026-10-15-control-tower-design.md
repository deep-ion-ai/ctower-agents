# Control Tower — Especificação de design

- **Data:** 2026-10-15
- **Status:** proposta, aguardando revisão
- **Repositório:** `deep-ion-ai/ctower-agents`

## 1. Contexto e objetivo

O desenvolvimento é feito majoritariamente por agentes de código (Claude Code ~80%, Codex ~20%) rodando **em paralelo**, em vários projetos, frequentemente em sessões **longas e autônomas** (até ~10 h, muitas com `ultracode`). As issues vivem no GitHub, mas o GitHub (inclusive Projects) não sabe nada sobre sessões, worktrees, quem está atuando em quê, nem guarda o histórico de decisões.

Dor concreta que motivou o projeto: após uma queda de energia, uma sessão em andamento foi esquecida e todo o trabalho foi refeito do zero.

**Objetivo:** uma aplicação própria (Rust + Tauri) que funcione como **sala de controle**: mostra o que está rodando, onde, em que estado e o que exige atenção humana; inicia e retoma sessões em terminais embutidos; e mantém um **ledger** por projeto (status das issues, decisões e sessões).

### Critérios de sucesso

1. Após reiniciar a máquina (ou o ambiente), o app lista todas as sessões que estavam ativas — com issue, branch, worktree e session-id — e permite retomá-las com um clique.
2. Uma única tela responde "o que está rodando, o que está parado esperando por mim, o que terminou".
3. Agentes registram as decisões que tomam, vinculadas à issue/épico e à sessão, e essas decisões são revisadas no momento do PR para a branch base.
4. Nenhuma sessão gerenciada consegue mesclar/empurrar em branch protegida (`develop`, `main`).

### Princípios

- **80% dashboard.** O app observa e infere estado sozinho; o humano só age para iniciar, retomar, revisar. Quanto menos interação, melhor.
- **GitHub é a fonte da verdade das issues.** O app não duplica nem edita issues; guarda apenas o que o GitHub não guarda.
- **Gravar antes de agir.** Todo efeito colateral relevante (iniciar processo, criar worktree) é precedido de registro no ledger, para que uma queda nunca deixe trabalho "invisível".
- **Hooks nunca atrapalham o agente.** Falha de comunicação com o app não pode travar uma sessão (exceto o guarda-corpo de branch protegida, que bloqueia por design).

## 2. Escopo

### MVP (fase 1)

- Um ambiente: **daemon em uma distro WSL dedicada**, UI no Windows.
- Cadastro de projetos (repositório GitHub + caminho local do repositório no ambiente).
- Sincronização de issues, sub-issues e PRs do GitHub (somente leitura).
- Work items (épico ou issue avulsa) com status de andamento automático.
- Iniciar e retomar sessões **Claude Code** em terminal embutido, com worktree e branch criados automaticamente.
- Hooks do Claude Code: estado da sessão, session-id, transcript, "aguardando você".
- Guarda-corpo de branch protegida.
- Backup de transcripts e tela de recuperação de sessões interrompidas.
- Servidor MCP do ledger (decisões, status).
- UI: Sala de controle, terminal, árvore do projeto, recuperação, revisão, configurações.
- Notificações nativas do Windows.

### Fase 2

- Daemon no **macOS** (Mac mini) acessado remotamente (Tailscale + token).
- Suporte completo ao **Codex**.
- UI rodando também no macOS.

### Fora de escopo

- Controle de uso de tokens/custo.
- Notas livres / diário de bordo.
- Escrita no GitHub pelo app (criar/editar issues, comentar, mesclar). O app só abre a URL de criação de PR no navegador.
- Daemon nativo no Windows (ConPTY). Windows é apenas hospedeiro da UI.
- Retomada automática de sessões interrompidas (sempre decisão humana).

## 3. Arquitetura

```
                ┌───────── ws + token ─────────► ctowerd (WSL dedicada)   [fase 1]
UI Tauri ───────┤                                  ├─ PTYs (claude/codex)
(Windows;       │                                  ├─ SQLite (ledger)
 macOS na f2)   │                                  ├─ git (worktrees/branches)
                │                                  ├─ GitHub sync
                │                                  └─ HTTP: /ws  /hooks  /mcp
                └───────── ws + token ─────────► ctowerd (macOS)          [fase 2]
```

- **`ctowerd` (daemon):** um binário Rust por ambiente (Linux/macOS — apenas Unix). É dono dos processos das sessões, dos terminais e do ledger daquele ambiente. Roda como serviço (systemd / launchd) e **sobrevive ao fechamento da UI**.
- **UI (Tauri + React + TypeScript + xterm.js):** cliente sem estado próprio além da lista de ambientes. Conecta a um ou mais daemons e agrega a visão.
- **Ledger por daemon:** cada ambiente guarda os dados dos seus projetos. A queda de um ambiente não afeta o ledger dos outros. **Revisado em 2026-10-16 (issue #56):** cada daemon continua local-first, mas envia o ledger a um daemon com papel de hub, que guarda a visão consolidada entre ambientes — ver `2026-10-16-ledger-centralizado.md`. **Revisado de novo em 2026-10-18 (épico #183):** o hub virou um binário próprio (`cthubd`), origem dos projetos e da configuração; cada `ctowerd` só executa os projetos atribuídos a ele — ver `2026-10-18-hub-plano-de-controle.md`.
- **`ctower` (CLI):** binário auxiliar instalado junto com o daemon, usado pelos hooks (`ctower hook <evento>`) e para diagnóstico (`ctower status`).

### Por que daemon + UI separados (e não monólito Tauri)

- Fechar/atualizar/travar a UI não mata sessões de horas.
- O mesmo binário atende WSL e macOS; um novo ambiente é só mais uma conexão na UI.
- Linux/macOS têm PTY nativo simples; evita ConPTY.

Custo aceito: protocolo WebSocket e buffer de scrollback no daemon para reconexão.

## 4. Componentes do daemon

Workspace Cargo com crates de responsabilidade única:

| Crate | Responsabilidade | Depende de |
|---|---|---|
| `ct-core` | Tipos de domínio, máquinas de estado (work item, sessão), regras de branch | — |
| `ct-ledger` | Persistência SQLite, migrações, consultas, eventos append-only | `ct-core` |
| `ct-pty` | Spawn de processos em PTY (`portable-pty`), ring buffer de scrollback, fan-out de saída | — |
| `ct-git` | Criação/remoção de worktree e branch, inspeção de estado (via `git` CLI) | `ct-core` |
| `ct-gh` | Cliente GitHub (issues, sub-issues, PRs), sincronização periódica | `ct-core`, `ct-ledger` |
| `ct-sessions` | Orquestra o ciclo de vida: lança, observa, recupera, faz backup de transcript | todos acima |
| `ct-server` | HTTP (`axum`): `/ws` (UI), `/hooks`, `/mcp`; autenticação por token | `ct-sessions`, `ct-ledger` |
| `ctowerd` | Binário: configuração, inicialização, recuperação no boot | `ct-server` |
| `ctower` | Binário CLI: hooks, diagnóstico | `ct-core` |

UI em `ui/` (Tauri), sem lógica de domínio além de apresentação.

## 5. Modelo de dados (SQLite, por daemon)

> Identidade global (`env_id`, `uid`, `hlc`), outbox e `hub.db`: ver `2026-10-16-ledger-centralizado.md` (issue #56).

```
project      id, name, gh_owner, gh_repo, repo_path, base_branch ('develop'),
             protected_branches (json, padrão ["develop","main","master"]),
             created_at

issue_cache  project_id, gh_number, title, state ('open'|'closed'), labels (json),
             parent_number?, has_sub_issues, url, updated_at, synced_at
             PK (project_id, gh_number)
             — espelho do GitHub; pode ser apagado e recriado a qualquer momento.

work_item    id, project_id, gh_number, kind ('epic'|'issue'), status,
             branch, worktree_path, created_at, closed_at?
             UNIQUE (project_id, gh_number)
             — a unidade gerenciada pelo app.

session      id (uuid do app), work_item_id, tool ('claude'|'codex'),
             tool_session_id?, transcript_path?, backup_path?,
             cwd, launch_cmd (json), state, exit_code?, pid?,
             started_at, last_event_at, ended_at?

decision     id, work_item_id, session_id?, origin ('autonoma'|'orientada'|'manual'),
             title, context, decision, consequences?,
             reviewed_at?, reviewed_in_pr?, created_at

event        id, ts, project_id, work_item_id?, session_id?, type, payload (json)
             — append-only; toda mudança de estado gera um evento.
```

Regras:

- `kind = 'epic'` quando a issue tem sub-issues no GitHub (`has_sub_issues`). Sub-issues aparecem como filhas via `issue_cache.parent_number` e só viram `work_item` se receberem sessão própria.
- `decision.origin`:
  - `autonoma` — agente decidiu sem orientação; **pendente de revisão** até ser marcada no fluxo de PR.
  - `orientada` — agente registrou decisão definida pelo humano na conversa; já nasce revisada (`reviewed_at = created_at`).
  - `manual` — registrada pelo humano na UI (raro; já nasce revisada).
- Tabelas `decision` e `event` não sofrem `UPDATE` exceto `decision.reviewed_*`; nada é apagado.

## 6. Máquinas de estado

### Work item (o "andamento")

```
backlog → em_execucao → aguardando_voce → em_revisao → concluido
             ↑   ↓            │
             └───┴────────────┘          + bloqueado, pausado (manuais ou via MCP)
```

| Gatilho | Novo status |
|---|---|
| Sessão iniciada / retomada / hook `UserPromptSubmit` | `em_execucao` |
| Hook `Notification` (permissão ou pergunta pendente) | `aguardando_voce` |
| PR aberto da branch do work item | `em_revisao` |
| PR mesclado | `concluido` |
| MCP `ledger_set_status` | o status informado (`bloqueado`, `pausado`) |
| Humano na UI | qualquer |

Mudanças manuais prevalecem até o próximo gatilho automático de natureza diferente.

### Sessão

```
starting → running ⇄ idle → exited
    │          │       │
    └──────────┴───────┴──→ interrupted → (retomar: nova sessão) | archived
```

- `starting`: registro gravado, processo ainda não confirmado.
- `running`: processo vivo, agente trabalhando.
- `idle`: hook `Stop` — agente encerrou o turno e espera entrada.
- `exited`: processo terminou de forma limpa (hook `SessionEnd`, código 0 ou `session.kill` pedido pelo humano; `exit_code` registrado).
- `interrupted`: processo terminou de forma anormal (código ≠ 0 ou sinal, sem `SessionEnd` antes; `exit_code` registrado) ou, **no boot do daemon**, sessão `starting|running|idle` cujo PTY não pertence ao daemon (o PTY só existe na memória do daemon que a lançou; #283).
- `archived`: humano descartou na tela de recuperação.
- Retomar uma sessão `interrupted` cria uma **nova** `session` com o mesmo `tool_session_id` e `cwd`, e a antiga vira `archived` com evento `resumed_as`.

## 7. Fluxos

### 7.1 Iniciar sessão

1. Humano clica **▶ Iniciar** num épico ou issue; escolhe ferramenta (MVP: claude) e confirma o prompt inicial (pré-preenchido pelo template do tipo).
2. Daemon cria/atualiza o `work_item` e grava `session(state=starting)` + evento.
3. Se não há worktree:
   - Branch: épico → `wip/<n>-<slug>`; issue avulsa → `<prefixo>/<n>-<slug>` com prefixo `feat` por padrão (`fix` se a issue tiver label `bug`, `chore` se tiver `chore`). Base: `project.base_branch`.
   - Worktree em `<worktrees_root>/<projeto>/<branch-sanitizada>` (configurável; padrão `~/worktrees`).
   - Se a branch já existir (local ou remota), reutiliza.
4. Gera arquivo de configuração MCP da sessão (`<data_dir>/sessions/<id>/mcp.json`) apontando para `/mcp` com cabeçalhos `Authorization` e `X-Ctower-Session`.
5. Spawn em PTY, `cwd` = worktree, com ambiente:
   - `CTOWER_SESSION=<id>`, `CTOWER_URL=http://127.0.0.1:<porta>`, `CTOWER_TOKEN=<token-de-hook>`, `CTOWER_PROTECTED=<branches protegidas, separadas por vírgula>`
   - comando: `claude --mcp-config <mcp.json> [opções do template] "<prompt inicial>"`
6. `SessionStart` confirma `tool_session_id` e `transcript_path`; estado → `running`.

Template padrão de prompt inicial (editável por projeto/tipo):

```
Trabalhe na {{kind}} #{{number}} do repositório {{owner}}/{{repo}} (leia com `gh issue view {{number}}`).
Branch de trabalho: {{branch}}. Você pode mesclar apenas em {{branch}} — nunca em {{protected}}.
Antes de começar, consulte decisões anteriores com `ledger_list_decisions`.
Registre toda decisão relevante com `ledger_record_decision`.
```

### 7.2 Hooks do Claude Code

O daemon instala (uma vez, com backup do arquivo original e merge não destrutivo) entradas em `~/.claude/settings.json` do ambiente, todas chamando `ctower hook <evento>`, que lê o JSON do stdin e faz POST em `$CTOWER_URL/hooks`.

**Se `CTOWER_SESSION` não está definido, o hook sai imediatamente com 0** — sessões fora do Control Tower não são afetadas.

| Evento | Efeito |
|---|---|
| `SessionStart` | grava `tool_session_id`, `transcript_path`; sessão → `running`; alerta se transcript em local volátil |
| `UserPromptSubmit` | sessão → `running`; work item → `em_execucao` |
| `Notification` | work item → `aguardando_voce`; notificação nativa |
| `Stop` | sessão → `idle`; backup do transcript |
| `SessionEnd` | sessão → `exited` |
| `PreToolUse` (matcher `Bash`) | guarda-corpo de branch protegida (7.3) |

Falha de rede/timeout (1 s) no POST: o hook escreve em `<data_dir>/hooks-spool/` e sai com 0. O daemon processa o spool ao subir e periodicamente.

### 7.3 Guarda-corpo de branch protegida

Executado localmente por `ctower hook pre-tool-use` (não depende do daemon estar no ar), apenas quando `CTOWER_SESSION` está definido. Bloqueia (saída 2, com mensagem explicativa para o agente) comandos Bash que:

- façam `git push` cujo destino seja uma branch protegida (inclui `HEAD:develop`, `--all`, `--mirror`, force push para protegida);
- façam `git merge`, `git commit`, `git rebase`, `git cherry-pick`, `git reset` com o HEAD do worktree em branch protegida;
- façam `git checkout`/`git switch` para branch protegida dentro do worktree da sessão;
- façam `gh pr merge` de PR cuja base seja branch protegida (o hook consulta `gh pr view --json baseRefName`; se não conseguir determinar a base, bloqueia). Merge de PRs de sub-issues na branch `wip/*` do épico continua permitido.

Em sessão de issue (o daemon exporta `CTOWER_ISSUE=<n>`; épico, planejamento e gestor não têm), bloqueia também o `gh pr create` cujo corpo não traga `Closes|Fixes|Resolves #<n>` (ou `owner/repo#<n>`, ou a URL da issue): o GitHub só fecha a issue com as palavras-chave em inglês, e "Fecha #n" deixa a issue aberta (#375). O hook confere o comando inteiro (corpo por `--body`, `$(cat <<EOF …)` ou heredoc em `--body-file -`) e lê o arquivo de `--body-file`; arquivo ilegível passa.

A lista de branches protegidas vem do projeto e é exportada na sessão (`CTOWER_PROTECTED`). Bloqueios geram evento `guard_blocked` no ledger (via POST best-effort).

Limitação conhecida: é uma barreira contra erro, não contra adversário; recomenda-se também branch protection no GitHub.

### 7.4 Backup de transcripts e recuperação

- `transcript_path` é sempre registrado a partir do hook (nunca presumido). Se o caminho estiver sob `/tmp`, `/run` ou outro `tmpfs`, a sessão recebe alerta visível.
- Backup: em todo `Stop` e a cada 5 min enquanto `running`, o daemon copia o transcript para `<data_dir>/transcripts/<tool_session_id>.jsonl` (cópia atômica: escreve temporário + rename).
- **Boot do daemon:** sessões `starting|running|idle` sem PTY neste daemon → `interrupted` (evento com horário do último sinal), mesmo que o processo ainda exista; o `SessionEnd` tardio dele é ignorado.
- **Tela de recuperação:** para cada `interrupted` mostra issue, branch, worktree, último evento, tempo parado, integridade do transcript (`original` | `restaurável do backup` | `perdido`) e estado do worktree (`git status` resumido).
- **Retomar:** se o transcript original não existe e há backup, restaura no caminho original; depois spawn de `claude --resume <tool_session_id>` **no mesmo `cwd`** (o Claude Code associa transcripts ao diretório). Prompt sugerido de retomada (editável): "A sessão foi interrompida antes de terminar. Verifique o estado do worktree e dos commits antes de continuar."
- Observação: `--resume` restaura a conversa, não a execução de um workflow multi-agente em andamento; o agente precisa reavaliar o estado. Por isso a retomada nunca é automática.

### 7.5 Sincronização com o GitHub

- Token obtido via `gh auth token` no ambiente do daemon (sem armazenar segredo próprio).
- A cada 3 min (e sob demanda pela UI): issues abertas + fechadas nos últimos 30 dias, sub-issues das issues com filhas, PRs cuja head seja branch de algum work item.
- Usa `ETag`/`If-None-Match` para economizar rate limit.
- Eventos derivados: PR aberto → `em_revisao`; PR mesclado → `concluido`; issue fechada sem PR → evento informativo (status não muda sozinho).
- Falha de rede/rate limit: mantém cache, marca "sincronizado há X min" na UI, backoff exponencial até 15 min.

### 7.6 Revisão (PR → base)

- Disponível quando o work item tem PR aberto para a branch base, ou sob demanda.
- Lista decisões `autonoma` não revisadas do work item (e, para épico, das sub-issues com work item próprio), com link para a sessão de origem.
- Ações: **marcar todas como revisadas** (grava `reviewed_at`, `reviewed_in_pr`) e **abrir PR no GitHub** (abre a URL no navegador).

## 8. MCP do ledger

Servidor MCP em `/mcp` (transporte HTTP streamable). A sessão é identificada pelo cabeçalho `X-Ctower-Session`; o work item é inferido dela.

| Tool | Parâmetros | Efeito |
|---|---|---|
| `ledger_record_decision` | `title`, `context`, `decision`, `consequences?`, `origin` (`autonoma`\|`orientada`) | cria `decision` |
| `ledger_list_decisions` | `scope?` (`work_item` padrão \| `epic` \| `project`) | lista decisões |
| `ledger_set_status` | `status` (`bloqueado`\|`pausado`\|`em_execucao`), `reason` | muda status do work item + evento |
| `ledger_get_context` | — | retorna work item, branch, branch protegidas, issue pai (épico) |

Instruções do servidor MCP orientam o agente: usar `orientada` quando a decisão veio do humano na conversa, `autonoma` caso contrário.

## 9. Protocolo UI ⇄ daemon

- Transporte: WebSocket em `/ws`, autenticado por `Authorization: Bearer <token>` no handshake.
- Mensagens de controle em JSON:
  - Requisição: `{ "id": n, "method": "...", "params": {...} }` → resposta `{ "id": n, "result" | "error" }`.
  - Push do servidor: `{ "event": "...", "data": {...} }` (mudanças de estado, novos eventos do ledger, status do sync).
- Terminal: frames binários `[u8 canal][uuid sessão][bytes]` para saída e entrada; mensagem `resize` em JSON.
- `attach(session)`: servidor envia snapshot do ring buffer (padrão 4 MiB por sessão) e passa a transmitir ao vivo. Várias UIs podem estar anexadas à mesma sessão.
- Métodos iniciais: `projects.list|add|update`, `tree.get(project)`, `dashboard.get`, `session.start|resume|archive|kill|attach|detach|input|resize`, `recovery.list`, `review.get|mark_reviewed`, `sync.now`, `env.info`.
- Shell livre (2.9, issue #428; spec da segunda entrega do console §4): `shell.open { work_item_id, cols?, rows? } → Shell`, `shell.list → Shell[]` e `shell.close { shell_id } → {}`, com `Shell = { id, work_item_id, cwd, program, started_at, busy }`. É o shell de login do usuário do daemon (`$SHELL` ou o do `passwd`, com `-l`) no worktree do item, ou no `repo_path` no `#0`, com `TERM=xterm-256color` e `COLORTERM=truecolor` e sem nenhuma variável `CTOWER_*` nem `CT_DOCS_DIR`/`CT_EPIC_DOCS_DIR` (as dos docs do agente, #441). Não passa pelo ledger e fica só na memória do daemon. O terminal é o mesmo das sessões: `session.attach|detach|input|resize` e os frames binários aceitam o id do shell. `busy` = o grupo de processos em primeiro plano não é o do shell. Push `shell_exited { shell_id, code }`. Recusas (`-32000`): item sem worktree no disco, gestor e o limite de 8 shells por ambiente. A limpeza do worktree (#287) encerra os shells da pasta antes de removê-la. Fica fora do canal do hub (PWA).
- Linha do tempo do work item (2.9, issue #422; spec da segunda entrega do console §3.3): `events.list { work_item_id, before?, limit? } → Event[]`, os eventos do ledger do item do mais novo para o mais antigo. `limit` tem padrão 20 e máximo 100 (pedido maior devolve 100; menor que 1 é `-32602`); `before` é o id de um evento e traz só os mais antigos que ele, para a paginação (a página seguinte usa o id do último da anterior; página com menos de `limit` itens é a última). Item inexistente: `-32000` "work_item não encontrado". Só leitura; entra no canal do hub (PWA).
- Versionamento: `env.info` retorna versão do protocolo; UI recusa daemon com versão major diferente e mostra aviso.

## 10. UI

Tela principal — **Sala de controle**:

- **Lateral:** ambientes (conectado/desconectado) e projetos com contadores (em execução, aguardando).
- **Faixa de atenção (topo):** só o que exige o humano — sessões aguardando você (há quanto tempo), sessões interrompidas, sessões que saíram com erro, alertas (transcript volátil, daemon desconectado, sync atrasado). Vazia = tudo andando.
- **Grade central:** um tile por sessão viva com issue, ferramenta, tempo, estado e as últimas linhas do terminal ao vivo. Clique abre o terminal.
- **Rodapé:** em revisão (com contagem de decisões pendentes) e concluídas hoje.

Telas secundárias:

1. **Terminal:** xterm.js em abas; barra com issue, branch, worktree, ferramenta, tempo, estado; ações: abrir no VS Code (`code --remote wsl+<distro> <worktree>`), abrir PR, encerrar.
2. **Projeto:** árvore épico → sub-issues com status; botão ▶ Iniciar em cada item; histórico de sessões e eventos do work item.
3. **Recuperação:** abre automaticamente se houver `interrupted`; ações Retomar / Arquivar por sessão.
4. **Revisão:** ver 7.6.
5. **Configurações:** ambientes (nome, URL, token), projetos, templates de prompt, raiz de worktrees.

Notificações nativas apenas para: aguardando você, saída com erro, ambiente desconectado.

## 11. Segurança

- Daemon escuta apenas em `127.0.0.1` por padrão (o Windows alcança a WSL via `localhost`). Bind em outra interface exige configuração explícita (caso macOS/Tailscale na fase 2).
- Dois tokens gerados na instalação, armazenados com permissão `0600` em `<config_dir>`:
  - **token de UI** — acesso completo ao `/ws`;
  - **token de hook/MCP** — apenas `/hooks` e `/mcp` (exportado para as sessões).
- Token do GitHub: o configurado pela UI (Configurações → GitHub), validado no GitHub e gravado na distro em `<config_dir>/github.token` com permissão `0600`; na falta dele, `gh auth token` sob demanda. A UI só vê a conta e a origem do token, nunca o valor. (Revisado em 2026-10-16, issue #41.)
- Projetos gerenciados e seus worktrees ficam no sistema de arquivos nativo do ambiente (ext4 na WSL), nunca em `/mnt/*`.

## 12. Tratamento de erros

| Situação | Comportamento |
|---|---|
| Processo da sessão termina com código ≠ 0 | sessão `exited`, evento com código, alerta na faixa de atenção |
| Daemon reinicia (atualização, crash) | PTYs morrem junto; no boot, sessões viram `interrupted` → recuperação |
| Atualizar daemon com sessões ativas | `ctower` recusa reiniciar o serviço sem `--force`, listando as sessões |
| Hook sem daemon | spool local, sai com 0 (exceto guarda-corpo, que funciona offline) |
| Worktree já existe com outra branch | erro explícito na UI, sem alterar nada |
| Transcript perdido e sem backup | recuperação mostra `perdido`; oferece iniciar nova sessão com prompt de contexto |
| GitHub indisponível | cache + indicador de atraso; nada bloqueia sessões |
| UI perde conexão | reconecta com backoff; ao reconectar, re-attach com snapshot |

## 13. Configuração e instalação

- **Provisionamento do ambiente WSL** (`scripts/provision/`):
  - lado Windows (`create-distro.ps1`): cria a distro dedicada a partir de uma imagem Ubuntu LTS, com nome configurável;
  - lado Linux (`provision.sh`, idempotente): habilita `systemd` em `/etc/wsl.conf`, instala git, gh, Node (para Claude Code), Claude Code, Rust toolchain, `ctowerd`/`ctower`, cria e habilita `ctowerd.service`, gera tokens, instala hooks.
- **UI:** instalador Tauri para Windows gerado no GitHub Actions (runner Windows). Ao abrir, executa `wsl -d <distro> --exec true` para garantir que a distro (e o serviço) estejam de pé antes de conectar.
- Diretórios do daemon (XDG): config `~/.config/ctower/`, dados `~/.local/share/ctower/` (SQLite, transcripts, spool, sessões).
- Repositório com `.gitattributes` forçando `eol=lf` (scripts e fontes rodam em Linux).

## 14. Testes

- **Unitários (`ct-core`, `ct-ledger`, `ct-git`):** transições de estado, derivação de branch/slug, regras do guarda-corpo (tabela de comandos permitidos/bloqueados), migrações e consultas com SQLite em memória, worktrees em repositório git temporário.
- **Integração (`ct-server`/`ct-sessions`):** daemon em processo de teste; hooks simulados via HTTP; spawn de `bash` no lugar de `claude`; cenário de "queda" (matar daemon + filhos, reiniciar, verificar `interrupted`); restauração de transcript do backup.
- **Contrato do protocolo:** testes de serialização das mensagens WS compartilhando tipos com a UI (geração de tipos TS a partir dos tipos Rust, ex.: `ts-rs`).
- **UI:** testes de componentes no essencial (faixa de atenção, recuperação); roteiro manual curto para terminal e reconexão.
- CI: GitHub Actions — `cargo test` + `clippy` em Linux e macOS; build da UI em Windows.

## 15. Decisões registradas neste design

1. Terminal embutido interativo (não headless) — permite acompanhar e intervir; monitoramento via hooks.
2. Ledger 100% no SQLite do app (projetos têm estruturas heterogêneas; nada é escrito nos repositórios).
3. Worktree por work item; épico usa `wip/*`; agentes só mesclam na branch do work item.
4. Épico = issue com sub-issues nativas do GitHub; issues avulsas também são work items.
5. Recuperação sempre manual ("mostrar e esperar").
6. Decisões registradas pelo agente via MCP, com origem `autonoma`/`orientada`; revisão no PR para a base.
7. Daemon + UI via WebSocket, daemon apenas Unix; fase 1 em WSL dedicada, fase 2 no macOS.
8. Codex adiado para a fase 2.
9. Sem notas livres; foco em dashboard.

## 16. Riscos e pontos a verificar na implementação

- **Superfície do Claude Code muda rápido:** nomes/payloads de hooks (`SessionStart`, `Notification`, `SessionEnd`), flags (`--mcp-config`, `--resume`) e o local de transcripts devem ser verificados contra a versão instalada no início da implementação; o crate de integração isola esses detalhes.
- **Codex (fase 2):** capacidades de hooks/notify, local de sessões e retomada precisam de investigação dedicada antes do design detalhado.
- **Reinício do daemon mata sessões:** mitigado pela recusa de restart com sessões ativas; extrair um supervisor de PTY separado fica como evolução se isso se mostrar doloroso.
- **Rate limit do GitHub** com muitos projetos: ETags e intervalo configurável.
