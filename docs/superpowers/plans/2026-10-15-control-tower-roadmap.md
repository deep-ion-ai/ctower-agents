# Control Tower — Roadmap de planos

**Spec:** `docs/superpowers/specs/2026-10-15-control-tower-design.md`

A spec cobre quatro subsistemas testáveis isoladamente. Cada um ganha seu próprio plano, escrito quando o anterior estiver concluído (para que o plano seguinte parta do código real, não de suposições).

| # | Plano | Entrega testável | Seções da spec |
|---|---|---|---|
| 1 | **Fundação** (`2026-10-15-plano-1-fundacao.md`) | Distro WSL provisionada; workspace Cargo; `ct-core` (modelo, máquinas de estado, nomes de branch, guarda-corpo); `ct-ledger` (SQLite); `ct-git` (worktrees); CLI `ctower hook` funcionando com guarda-corpo e spool; CI | 4, 5, 6, 7.1 (passo 3), 7.2 (lado do hook), 7.3, 13 (parcial), 14 |
| 2 | **Sessões e servidor** (`2026-10-15-plano-2-sessoes-servidor.md`) | `ct-pty` (PTY + ring buffer), `ct-sessions` (lançar, retomar, backup de transcript, recuperação no boot), `ct-server` (`/hooks`, `/ws`, `/mcp`), `ctowerd` + systemd + instalação dos hooks no `~/.claude/settings.json`; sessão Claude real iniciada e retomada via cliente WS de teste | 7.1, 7.2, 7.4, 8, 9, 11, 12, 13 |
| 3 | **Sincronização GitHub** (`2026-10-15-plano-3-sincronizacao-github.md`, épico #18 — concluído, PR #26) | `ct-gh`: issues, sub-issues, PRs com ETag; gatilhos `PrOpened`/`PrMerged`; `issue_cache` | 7.5, 7.6 (dados) |
| 4 | **UI Tauri** (`2026-10-15-plano-4-ui-tauri.md`, épico #27 — concluído, PR #36; roteiro manual em `docs/ui-roteiro-manual.md`) | Sala de controle, terminal (xterm.js), projeto, recuperação, revisão, configurações, notificações; instalador Windows via GitHub Actions | 7.6, 10, 13 (UI) |

Fase 2 da spec (daemon macOS, Codex, UI macOS) terá planos próprios após o Plano 4.
