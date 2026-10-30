# Console — segunda entrega: ações, contexto e aba de shell (brainstorm da issue #87)

Resultado do brainstorm da issue #87, que é a segunda entrega separada na #78 (`2026-10-16-console-melhorias.md`, §3). Complementa a spec principal (`2026-10-15-control-tower-design.md`). As decisões estão no ledger do work item #87.

## 1. Ponto de partida

Desde a #78 e o #310, a tela Terminal (`ui/src/terminal/TerminalScreen.tsx`) já tem:

- a árvore de sessões no painel lateral, agrupada por projeto e por épico (`SessionTree.tsx`);
- uma aba de editor por sessão, com até duas lado a lado ("Dividir à direita");
- o cabeçalho do painel (`PaneBar`), com Buscar, Abrir item no projeto, Abrir PR (ou Revisar decisões do gestor), Abrir no VS Code, dividir e Encerrar;
- "Abrir Recuperação" na sessão interrompida recuperável;
- a faixa de pedido pendente (`AskStrip`) e o uso de contexto na barra de status.

No daemon, também já existem `session.resume` (retomar com `claude --resume`), `session.start_project` (#43, work item `#0`), `review.get` (work item, PRs e decisões) e o `ct-pty`, que é genérico (não sabe o que roda no PTY). Os frames binários de entrada e saída são identificados pelo UUID da sessão.

Esta entrega acrescenta ações, um painel de contexto, uma aba de shell livre, organização de muitas abas e o tratamento da sessão do projeto.

## 2. Ações

### 2.1 Cabeçalho do painel

As ações frequentes ficam como botões de ícone. As demais vão para um menu **⋯**, para o cabeçalho caber no split.

| Ação | Onde | Sessão viva | Sessão encerrada |
|---|---|---|---|
| Buscar no terminal | botão | sim | sim |
| Enviar prompt pronto | botão (menu) | sim | — |
| Contexto (painel, §3) | botão | sim | sim |
| Abrir shell no worktree (§4) | botão | sim | sim, se o worktree existir |
| Retomar | botão primário | — | sim, com `tool_session_id` |
| Abrir issue no GitHub | ⋯ | sim | sim |
| Abrir PR / Revisar decisões do gestor | ⋯ | sim | sim |
| Abrir item no projeto | ⋯ | sim | sim |
| Abrir no VS Code | ⋯ | sim | sim |
| Reiniciar | ⋯ | sim, com confirmação | sim |
| Dividir à direita / Fechar divisão | botão | sim | sim |
| Encerrar sessão | botão | sim | — |

Todas as ações também entram na paleta de comandos ("Sessão: Retomar", "Sessão: Enviar prompt — Rodar os testes"…), aplicadas à sessão ativa.

### 2.2 Retomar e Reiniciar

- **Retomar** chama `session.resume` direto, sem modal: continua a conversa com o mesmo modelo e modo, como na Recuperação. A sessão nova **ocupa a aba da antiga**, na mesma coluna do split, para o usuário não perder o lugar. Se o transcript tiver sido perdido, o erro do daemon aparece no cabeçalho com o link "Abrir Recuperação".
- **Reiniciar** começa uma conversa nova no mesmo work item. Ele abre a modal de início (`StartSessionModal`) com o prompt do template do tipo (issue, épico, projeto ou gestor), o modelo e o modo da sessão anterior. Com a sessão viva, a confirmação avisa que o processo atual será encerrado. A UI faz `session.kill` e depois `session.start`, `session.start_project` ou `session.start_gestor`. A sessão nova também ocupa a aba da antiga.
- O par existe porque retomar preserva o contexto e reiniciar o limpa (contexto cheio, conversa que se perdeu).

### 2.3 Abrir issue no GitHub

- Issue ou épico: `https://github.com/{owner}/{repo}/issues/{n}`, pelo `api.openUrl`.
- Sessão do projeto (`#0`): vira "Abrir repositório no GitHub".
- Gestor: a ação não aparece.

### 2.4 Prompts prontos

- Uma lista editável (nome + texto) nas Configurações, na seção dos templates, guardada no mesmo `UiConfig` (`config.prompts`). Cada item tem "Restaurar padrão", como os templates.
- Padrões: "Continuar" (`continue`), "Rodar os testes e corrigir", "Registrar as decisões no ledger", "Abrir o PR", "Resumir o estado atual".
- O texto aceita as mesmas variáveis dos templates (`{{number}}`, `{{branch}}`…), com `renderTemplate`.
- O envio é por **bracketed paste** (`ESC[200~` … `ESC[201~`) seguido de `\r`, por `session.input`. Assim um prompt de várias linhas chega inteiro ao Claude Code antes do Enter.
- O menu fica desligado com a sessão encerrada. Com um pedido de permissão pendente, o menu avisa que o agente está esperando a resposta e não envia.
- Não há campo livre nesta entrega: um prompt avulso é digitado no terminal.

## 3. Painel de contexto

### 3.1 Lugar e comportamento

- É um painel recolhível **à direita do console**, como a barra lateral secundária do VS Code, com uns 320 px. O botão "Contexto" do cabeçalho e o atalho Ctrl+Alt+B (Cmd+Alt+B no macOS) abrem e fecham. O estado aberto/fechado fica por máquina, em `localStorage`.
- Mostra o contexto da **sessão ativa** (coluna 1). No split, segue a coluna que tem o foco.
- Em janela estreita, o painel passa a sobrepor o console em vez de dividir a largura. Assim o PTY não fica estreito demais.
- Os dados vêm ao abrir o painel e ao trocar de sessão. Depois, são recarregados quando chega um push do mesmo work item (`work_item_status`, `session_state`, `session_exited`, `needs_human`, `sync_status`), com um debounce de 1 s.

### 3.2 Seções

1. **Work item:** status (com o mesmo seletor da tela Projeto quando o ambiente fala com o hub), branch, worktree e as sessões anteriores do item (retomáveis vão para a Recuperação).
2. **PR:** número, estado (aberto, mesclado, fechado) e link; sem PR, "Criar PR" (a URL de compare de `prUrl`). Vem de `review.get`.
3. **Decisões:** as do ledger do item (`review.get`), da mais nova para a mais antiga, com a origem (autônoma ou orientada) e a marca de revisada. Há o link "Revisar decisões", que leva à tela de revisão.
4. **Eventos:** a linha do tempo do work item no ledger, paginada como no `WorkItemPanel` (`EVENTS_PAGE`, com "mostrar mais").

### 3.3 Daemon

- Método novo **`events.list { work_item_id, before?, limit? }`**, que devolve os eventos do ledger do item, do mais novo para o mais antigo (`limit` padrão 20, máximo 100), para a paginação. Ele é só leitura e pode entrar na allowlist web do #205.
- Os outros dados já vêm de `review.get` e do dashboard.

## 4. Aba de shell livre

### 4.1 Onde roda

- O shell é um **PTY no `ctowerd`**, ao lado das sessões do agente. Por isso funciona igual no WSL e no Mac mini remoto, e sobrevive ao app fechado.
- O programa é o shell de login do usuário do daemon (`$SHELL`, ou o do `passwd`, com `-l`), no worktree da issue ou no `repo_path` da sessão do projeto. É ali que aparecem o Oh My Zsh e o prompt do usuário.
- **Sem hooks, sem MCP e sem ledger:** o shell não cria sessão, work item, evento nem decisão. As variáveis do Control Tower que o agente recebe (id da sessão, token do MCP, e as pastas de docs `CT_DOCS_DIR`/`CT_EPIC_DOCS_DIR` do épico #445) **não** são passadas, nem herdadas do ambiente do daemon. O ambiente tem `TERM=xterm-256color` e `COLORTERM=truecolor`.
- O guarda-corpo das branches protegidas não se aplica ao shell: ele é do usuário, como um terminal fora do app.
- O shell fica **só no app desktop.** O PWA (#205) fala com os ambientes pela allowlist do hub, que já deixa de fora o fluxo de terminal. Os métodos `shell.*` também ficam fora dela.

### 4.2 Protocolo

| Método | Parâmetros | Resposta |
|---|---|---|
| `shell.open` | `work_item_id`, `cols?`, `rows?` | `Shell` |
| `shell.list` | — | `Shell[]` |
| `shell.close` | `shell_id` | `{}` |

- `Shell = { id, work_item_id, cwd, program, started_at, busy }`, com `id` UUID. `busy` diz se há um processo em primeiro plano diferente do próprio shell (grupo de processos do terminal ≠ pid do shell, pelo `process_group_leader` do `portable-pty`).
- O shell usa o **mesmo caminho de terminal das sessões**: `session.attach|detach|input|resize` e os frames binários aceitam o id do shell (a busca do PTY passa a olhar também o mapa de shells). O adaptador do xterm (`adapter.ts`) não muda.
- Push `shell_exited { shell_id, code }` quando o processo termina (por `exit` no shell ou por `shell.close`).
- `shell.open` recusa com erro claro: work item sem worktree no disco, worktree removido, gestor (não há pasta de item) e o **limite de 8 shells por ambiente**.
- Os shells ficam só na memória do daemon. No restart do daemon, eles morrem. A limpeza do worktree (#287) encerra antes os shells que estão naquela pasta.

### 4.3 Ciclo de vida

- **Abrir:** pelo botão "Abrir shell no worktree" do cabeçalho, pela paleta ("Shell: novo no worktree da sessão ativa"), pelo atalho Ctrl+Shift+` (como "novo terminal" no VS Code) e pelo item na tela Projeto. Cada clique abre um shell novo.
- **Fechar a aba encerra o shell** (`shell.close`), como no VS Code. Se `busy`, a confirmação diz que há um comando rodando.
- **App fechado ou recarregado:** o shell continua no daemon. A árvore lista os shells vivos por `shell.list`, e um clique reabre a aba com o replay do buffer.
- **Fim do processo:** a aba mostra "shell encerrado (código n)", com "Abrir novo shell" e "Fechar".

### 4.4 Como se distingue da sessão do agente

- Na árvore, o shell é um **filho da linha da sessão do item**, ou do item quando não há sessão, com ícone de prompt (`>_`) e rótulo `zsh · #87`.
- Título da aba: "Shell · #87" (ou "Shell · sessão do projeto"), com o mesmo ícone.
- O cabeçalho do shell é enxuto: pasta, programa e duração; ações Buscar, Abrir no VS Code, dividir e Fechar. Não tem selo de estado, faixa de pedido, uso de contexto, prompts prontos nem painel de contexto.
- Um shell pode ficar no split ao lado da sessão do agente do mesmo item. Esse é o uso esperado: o agente à esquerda, o shell à direita.

### 4.5 UI

- `EditorTab` ganha o tipo `shell` (`{ kind: "shell", env, shell, workItemId }`), e o reducer de `nav` trata o shell como o terminal no split e no fechamento.
- `TerminalView` recebe o id do PTY e um indicador de tipo, só para o texto do fim ("sessão encerrada" ou "shell encerrado").

## 5. Muitas abas

A árvore já agrupa por projeto e por épico. A barra de abas não ganha agrupamento próprio, porque dois agrupamentos competiriam. Esta entrega dá meios de **limpar** e de **achar** abas:

- **Menu de contexto da aba** (clique direito): Fechar, Fechar outras, Fechar à direita, Fechar encerradas e Fechar as do projeto. "Fechar encerradas" fecha as sessões encerradas sem volta e os shells encerrados. A interrompida recuperável fica.
- **Árvore:** a ação "Fechar encerradas" no cabeçalho do painel lateral e o menu de contexto de um grupo, com "Fechar as abas deste projeto".
- **Lista de abas abertas:** com mais abas do que cabem, a barra mostra um botão ⌄ com a lista filtrável (como "Show Opened Editors" no VS Code), com o projeto de cada aba.
- **Dica de projeto:** o tooltip da aba mostra projeto e ambiente. Abas do mesmo projeto ficam lado a lado: uma aba nova entra logo depois da última do mesmo projeto, e não no fim.
- Fica de fora o fechamento automático ao concluir o work item. O "Fechar encerradas" cobre esse caso sem esconder nada do usuário.

## 6. Sessão do projeto (sem issue, #43)

- **Aba:** "Sessão do projeto · <projeto>"; na árvore, a linha fica no topo do grupo do projeto, antes das issues.
- **Ações:** "Abrir repositório no GitHub" no lugar de "Abrir issue". "Abrir PR" e "Abrir item no projeto" não aparecem (não há branch nem item). "Reiniciar" usa `session.start_project` com o template "projeto".
- **Shell:** abre no `repo_path` (clone principal). O cabeçalho do shell avisa "clone principal", para lembrar que ali não é um worktree isolado.
- **Contexto:** sem a seção PR; as decisões e os eventos são os do work item `#0` do projeto.

## 7. Entregas (épico)

A implementação vira um épico com estas sub-issues, na ordem de dependência:

1. **feat(ctowerd): shell livre no PTY do daemon.** `shell.open|list|close`, o attach/input/resize com o id do shell, o push `shell_exited`, o limite, o fim na limpeza do worktree e os tipos TS.
2. **feat(ctowerd): `events.list` do work item.**
3. **feat(ui): aba de shell.** O tipo de aba, a árvore, o cabeçalho enxuto, abrir/fechar com confirmação de `busy`, reabrir pela árvore e o atalho.
4. **feat(ui): ações do console.** O menu ⋯, Retomar e Reiniciar na mesma aba, Abrir issue/repositório e as entradas na paleta.
5. **feat(ui): prompts prontos.** A lista nas Configurações, o menu do cabeçalho, a paleta e o envio por bracketed paste.
6. **feat(ui): painel de contexto.** O painel recolhível e as seções da §3 (depende da 2).
7. **feat(ui): muitas abas e sessão do projeto.** O menu de contexto da aba e da árvore, a lista de abas, a ordem por projeto e os ajustes da §6.

## 8. Testes

- **Rust:** `shell.open` no worktree, com o shell certo e sem as variáveis do Control Tower; recusas (sem worktree, gestor, limite); attach/input/resize com o id do shell; `shell_exited`; `busy` com um `sleep` em primeiro plano; a limpeza do worktree encerra o shell; `events.list` com paginação.
- **UI (vitest):**
  - Retomar e Reiniciar mantêm a aba na mesma posição e coluna;
  - o bracketed paste do prompt e a recusa com permissão pendente;
  - a lista de prompts nas Configurações;
  - o painel de contexto: seções, recarga por push e coluna com foco;
  - o reducer com abas de shell: split, fechar e "Fechar encerradas";
  - a árvore com shells;
  - os rótulos da sessão do projeto.
- **Roteiro manual** (`docs/ui-roteiro-manual.md`), no Windows (WSL) e no macOS com o Mac mini remoto:
  - abrir um shell e ver o Oh My Zsh;
  - deixar o shell no split ao lado do agente;
  - fechar o app e reabrir o shell pela árvore;
  - fechar a aba com um comando rodando;
  - retomar uma sessão encerrada;
  - mandar um prompt de várias linhas;
  - "Fechar encerradas" com dez abas.
