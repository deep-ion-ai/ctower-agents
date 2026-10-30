# Console (terminal embutido) — melhorias (brainstorm da issue #78)

Resultado do brainstorm da issue #78. Implementação no épico #86 (sub-issues #79–#85). Complementa a spec principal (`2026-10-15-control-tower-design.md`). As decisões estão registradas no ledger do work item #78.

## 1. Situação e diagnóstico

O console usa o **xterm.js** 5.5 (`ui/src/terminal/xterm.ts`) só com o addon `fit`. O daemon abre o PTY com o `portable-pty` e guarda os últimos 4 MB de **bytes crus** num buffer circular (`ct-pty/src/ring.rs`). Ao anexar, esses bytes voltam como snapshot e a UI faz `reset()` + `write`.

**Fica o xterm.js.** É o padrão de mercado (VS Code, Hyper, Tabby, JupyterLab). As alternativas avaliadas ainda são imaturas: o ghostty-web é pré-1.0 e chegou a suportar só o Chrome, o que é um problema porque o Tauri usa WebKit no macOS e no Linux; o W-Term é novo e lento com saída longa. O "Oh My Zsh" é do shell, não do emulador, e só aparece numa aba de shell (§4).

Problemas relatados no app Windows, com a causa encontrada:

| # | Problema | Causa |
|---|---|---|
| P1 | Texto embaralhado ao trocar para uma aba em que o usuário ainda não interagiu | Toda aba monta o xterm e anexa já escondida (`display: none`). O xterm não mede a fonte e fica em 80×24; o `ResizeObserver` ignora abas escondidas (`TerminalView.tsx`), e o PTY nasce com 120×40 (`ct-sessions/src/launch.rs`). O replay, gerado para outra largura, é escrito no tamanho errado. Ao aparecer, a aba se ajusta e o Claude Code redesenha só a parte viva da tela. |
| P2 | Copiar e colar não funcionam direito | Não há tratamento de teclado nem de clipboard. Ctrl+C e Ctrl+V vão para o Claude Code, e o modo mouse dele captura o arrasto. |
| P3 | O link mostra um alerta e, mesmo aceitando, não abre nem deixa copiar | Sem `linkHandler`, o xterm usa o tratador padrão de OSC 8: `confirm("…could potentially be dangerous")` e depois `window.open()`, que o Tauri bloqueia. |
| P4 | Ctrl+Enter não quebra linha | O xterm manda Ctrl+Enter como `\r`, igual ao Enter. |

## 2. Primeira entrega (épico)

### 2.1 Tamanho certo antes do replay (P1)

- **Anexar na primeira exibição:** a aba só anexa quando fica visível pela primeira vez. Antes, `fit()` e `resize` (sem debounce) com o tamanho real; depois, `attach` e o replay.
- **Redesenho forçado:** depois do attach de uma sessão já em andamento, a UI faz um resize de ida e volta (`rows-1` e depois `rows`) para o Claude Code redesenhar a tela inteira com o tamanho certo.
- **Tamanho inicial do PTY:** `session.start` e `session.start_project` aceitam `cols` e `rows` opcionais. A UI manda o tamanho da área do terminal; sem eles, o PTY continua em 120×40.
- Depois do primeiro attach, a aba fica montada como hoje (o histórico continua nas abas escondidas). Ao voltar a ficar visível, ela ajusta o tamanho como já faz.
- **Fora desta entrega:** um emulador no daemon (`alacritty_terminal` ou `vt100`) que guarda o estado da tela e o manda pronto ao anexar, como o tmux. Só entra se o histórico rolado continuar com lixo depois desta correção.

### 2.2 Links (P3)

- `linkHandler` do xterm (OSC 8) e addon `web-links` (URLs em texto puro) abrem pelo `api.openUrl` (plugin opener do Tauri), **sem confirmação**. Só abrem `http` e `https`.
- Ctrl+clique abre o link, e um clique simples não abre. É o padrão do VS Code e evita abrir algo por engano ao selecionar texto.
- O menu do clique direito sobre um link ganha "Copiar link".

### 2.3 Copiar e colar (P2)

Mesmo modelo do Windows Terminal e do VS Code:

| Atalho | Ação |
|---|---|
| Ctrl+C **com** seleção | copia e limpa a seleção (não manda ^C) |
| Ctrl+C **sem** seleção | ^C para o agente (como hoje) |
| Ctrl+Shift+C | copia sempre |
| Ctrl+V / Ctrl+Shift+V | cola (bracketed paste do xterm, para colar várias linhas sem enviar) |
| Clique direito | copia, se houver seleção; senão, cola |
| Shift+arrastar | seleciona mesmo com o modo mouse do Claude Code ligado |

- O clipboard passa pelo plugin `tauri-plugin-clipboard-manager`, sem depender do `navigator.clipboard` da WebView.
- No macOS, o mesmo vale com Cmd no lugar de Ctrl.

### 2.4 Quebra de linha (P4)

- Ctrl+Enter e Shift+Enter mandam a sequência de nova linha que o Claude Code reconhece (ESC+CR, a mesma do `/terminal-setup` no VS Code). A sequência final deve ser confirmada no app ao implementar.

### 2.5 Busca no buffer

- Ctrl+F (Cmd+F no macOS) abre uma barra de busca sobre o terminal, com o addon `search`: próximo e anterior (Enter e Shift+Enter), diferenciar maiúsculas, contador de ocorrências e Esc para fechar.

### 2.6 Fonte, zoom e tema

- Uma seção **Terminal** nas Configurações com a família e o tamanho da fonte. Fica por máquina, em `localStorage`, porque depende das fontes instaladas.
- Ctrl+= e Ctrl+- fazem o zoom e Ctrl+0 volta ao padrão. Valem para todas as abas.
- O tema ganha a paleta ANSI completa (16 cores), derivada dos tokens de `theme.css`.
- O addon `unicode11` acerta a largura de emojis e símbolos usados pelo Claude Code.

### 2.7 Desempenho

- O addon `webgl` substitui o renderizador DOM. Se o contexto WebGL cair (`onContextLoss`), o terminal volta ao renderizador padrão sem perder o buffer.
- O scrollback do xterm continua em 10.000 linhas.

### 2.8 Teclado: uma regra só

Os atalhos de 2.3 a 2.6 ficam num único `attachCustomKeyEventHandler`, com uma tabela testável (tecla → ação), para não haver conflito entre eles nem com as teclas do Claude Code.

## 3. Segunda entrega (issue #87)

O desenho está em `2026-10-29-console-segunda-entrega.md`. Os pontos abaixo foram o ponto de partida.

- **Ações:** reiniciar ou retomar a sessão encerrada, abrir a issue no GitHub e enviar um prompt pronto.
- **Contexto:** painel recolhível ao lado do console, com decisões do ledger, status do work item, PR e eventos.
- **Aba de shell livre no worktree:** o shell do usuário (bash/zsh do WSL) na pasta da issue, onde aparece o Oh My Zsh.
- A organização com muitas abas e a aba da sessão do projeto sem issue (#43) também ficam para essa entrega.

## 4. Testes

- Unitários (vitest), com o fake de xterm atual: anexar só ao aparecer, a ordem resize → attach → replay, a tabela de atalhos (cada combinação, com e sem seleção), o `linkHandler` (só http/https e chamada ao `openUrl`).
- Rust: `session.start`/`start_project` com e sem `cols`/`rows`.
- Roteiro manual (`docs/ui-roteiro-manual.md`) no app Windows: abrir três sessões e trocar de aba sem interagir; copiar e colar; links OSC 8 e em texto puro; Ctrl+Enter; Ctrl+F; zoom; saída longa.
