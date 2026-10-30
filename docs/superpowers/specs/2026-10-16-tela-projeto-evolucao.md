# Tela Projeto — evolução (brainstorm da issue #44)

Resultado do brainstorm da issue #44. Complementa a spec principal (`2026-10-15-control-tower-design.md`, §10 item 2). As decisões estão registradas no ledger do work item #44.

## 1. Layout

```
┌ cabeçalho: projeto · repo · base · protegidas · sync ─────────────────────────┐
│ [busca #/título]  [chips de status]  [ordenar: atividade|número]  [□ concluídos]│
├──────────────────────────────────────────────┬────────────────────────────────┤
│ ÁRVORE                                       │ PAINEL DE DETALHE (item clicado)│
│ ▾ ÉPICO #38 Cadastro…   3/5  ● em execução   │ #43 Sessão do projeto           │
│     #41 …  PR #50 aberto  ◆2  há 3h  [status]│ [▶ Iniciar] [GitHub ↗] [Status▾]│
│     #43 …                 ◆1! há 5m  [status]│ ── Sessões ──                   │
│ ▸ ÉPICO #20 …   (sem atividade: recolhido)   │ ── Eventos (linha do tempo) ──  │
│ ISSUES AVULSAS                               │ ── Decisões (pendentes 1º) ──   │
│     #44 …                                    │ ── PRs ──                       │
└──────────────────────────────────────────────┴────────────────────────────────┘
```

## 2. Árvore

### Visibilidade (padrão)

- Mostra só issues em aberto.
- Um épico aparece quando ele mesmo ou pelo menos uma sub-issue está aberto. Quando aparece, traz **todas** as sub-issues, inclusive as concluídas. Exemplo: 9 concluídas e 1 aberta → as 10 aparecem.
- Toggle **"mostrar concluídos"**: traz também as issues avulsas fechadas e os épicos totalmente fechados.

### Expansão inicial

- Começam expandidos só os épicos com sessão viva ou com atividade recente (`last_activity_at` nos últimos 7 dias).
- Os demais começam recolhidos. A escolha manual de abrir ou fechar continua valendo durante a sessão da tela.

### Linha do item

O que a linha já mostra hoje: `#número`, título, branch, badge de status e o botão de ação.

Novos elementos:

- **PR ligado:** número e estado do PR mais recente, com link.
- **Decisões:** contador `◆N`, com destaque `!` quando há decisões autônomas pendentes de revisão.
- **Última atividade:** "há X", tirado de `last_activity_at`.
- A linha inteira é clicável e abre o painel de detalhe.

### Busca, filtro e ordenação (no cliente)

- **Busca:** por número (`#43` ou `43`) e por trecho do título. Um épico aparece quando ele ou alguma sub-issue casa com a busca.
- **Chips de status** (multisseleção): backlog, em execução, aguardando você, em revisão, bloqueado, pausado, concluído.
- **Ordenação:** atividade recente (padrão) ou número. Os itens sem work item vão para o fim.

## 3. Painel de detalhe

O painel abre à direita, no lugar do painel Iniciar atual, que passa a ser uma ação dentro dele.

**Ações:**
- **▶ Iniciar** ou **Abrir terminal**, como hoje.
- **Abrir no GitHub:** a issue e, se houver, o PR.
- **Mudar status:** menu com todos os status. O motivo é opcional.

**Seções:**
- **Sessões:** ferramenta, estado, início e fim, duração e código de saída. Uma sessão viva abre o terminal.
- **Eventos:** linha do tempo do work item (mais recentes primeiro, com paginação simples).
- **Decisões:** as autônomas pendentes vêm primeiro, depois as demais; cada uma mostra origem, título, decisão e consequências.
- **PRs:** número, head → base, estado e link.

Item sem work item (issue que nunca teve sessão): o painel mostra só as ações e o aviso "sem histórico".

## 4. Contrato com o daemon

Extensões compatíveis: só entram campos e métodos novos.

- **`tree.get`:** cada `TreeNode` ganha `summary`:
  `{ pr: PrRow | null, decisions: { total, pending_autonomous }, last_activity_at: number | null }`.
  `last_activity_at` é o `ts` do último evento do work item. Isso evita uma chamada por linha.
- **`workitem.history { work_item_id }`:** devolve `{ sessions, events, decisions, prs }`. É chamado ao abrir o painel e recarregado quando chega um push que envolve o item.
- **`workitem.set_status { work_item_id, status, reason? }`** (daemon local) e **`hub.workitem.set_status { owner, repo, number, status, reason? }`** (hub, entregue em #47): gravam um evento `status_set_manual` (payload `{ from, to, cause: "manual", reason? }`, com HLC). Segue a §6: o status manual vale até o próximo evento de status mais novo, isto é, um gatilho automático de outra natureza. No hub, `status_set_manual` entra na mesma regra de status por HLC do `status_changed`. Para uma issue sem work item, cria o work item com o status informado (`epic` se a issue tem sub-issues no espelho do hub, senão `issue`). Pedir o status que o item já tem não grava evento. A resposta é o `HubWorkItem` atualizado. Erros (`-32000`): projeto ou issue fora do hub; `-32602`: status fora dos 7 valores. O status local do ambiente não muda (limitação aceita na spec do ledger centralizado, §4).

## 5. Fora deste desenho

- **Criar issue ou épico pela UI:** o app não escreve no GitHub (spec §2). Isso fica com a sessão do projeto (#43).
- **Retomar sessão ou abrir o VS Code pela árvore:** continuam no terminal e na tela Recuperação.

## 6. Próximos passos

Épico **#51**, com as sub-issues:

1. #45 — mockup da nova tela, validado com o usuário (vem primeiro; as demais esperam a aprovação);
2. #46 — daemon: `summary` no `tree.get`;
3. #47 — daemon: `workitem.history` e `workitem.set_status`;
4. #48 — UI: visibilidade, expansão, busca, filtro e ordenação;
5. #49 — UI: linha enriquecida;
6. #50 — UI: painel de detalhe com as ações.

Depois do mockup aprovado: escrever o plano e implementar.
