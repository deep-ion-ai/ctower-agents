import type { ReadyPrompt, UiConfig } from "../api/types";

// Templates da spec §7.1 (variáveis: {{kind}} {{number}} {{owner}} {{repo}}
// {{branch}} {{protected}}). Espelham src-tauri/src/envs.rs (padrões do ui.json).
export const DEFAULT_ISSUE_TEMPLATE = [
  "Trabalhe na {{kind}} #{{number}} do repositório {{owner}}/{{repo}} (leia com `gh issue view {{number}}`).",
  "Branch de trabalho: {{branch}}. Você pode mesclar apenas em {{branch}} — nunca em {{protected}}.",
  "Antes de começar, consulte decisões anteriores com `ledger_list_decisions`.",
  "Registre toda decisão relevante com `ledger_record_decision`.",
  'No corpo do PR, escreva `Closes #{{number}}`: o GitHub só fecha a issue com Closes, Fixes ou Resolves, e "Fecha #{{number}}" não fecha.',
].join("\n");

export const DEFAULT_EPIC_TEMPLATE = [
  "Trabalhe no épico #{{number}} de {{owner}}/{{repo}}: uma sub-issue por vez, cada uma em PR para {{branch}}.",
  "Nunca mescle em {{protected}}.",
  "Consulte `ledger_list_decisions` com scope epic antes de começar.",
  "Ao começar cada sub-issue, chame `ledger_start_sub_issue` com o número dela.",
  "Registre toda decisão relevante com `ledger_record_decision`.",
  'No PR do épico para a base, escreva `Closes #{{number}}` e um `Closes #<n>` por sub-issue: o GitHub só fecha com Closes, Fixes ou Resolves, e "Fecha #n" não fecha.',
].join("\n");

export const DEFAULT_PROJECT_TEMPLATE = [
  "Sessão de planejamento no clone principal de {{owner}}/{{repo}}, sem issue associada.",
  "Use-a para entender o projeto e criar issues e épicos com `gh issue create` (sub-issues ligadas ao épico).",
  "Não faça commits nem mescle em {{protected}}: código vai em sessões de issue.",
  "Consulte decisões anteriores com `ledger_list_decisions` e registre as relevantes com `ledger_record_decision`.",
].join("\n");

// Gestor de projetos (spec do gestor §10.1): {{env}}, {{scope}} (a linha de
// foco do escopo escolhido), {{owner}} e {{repo}} (escopo de projeto). Os
// limites fixos vão à parte, no system prompt da sessão.
export const DEFAULT_GESTOR_TEMPLATE = [
  "Você é o gestor de projetos do Control Tower, numa sessão sem repositório no ambiente {{env}}.",
  "{{scope}}",
  "Leia o hub só pelas ferramentas portfolio_*. Para ler uma issue, use o `issue_view` do projeto em portfolio_overview (`gh issue view <n> -R owner/repo` no GitHub; o jira-cli com o comando de credencial no Jira).",
  "Mudanças de prioridade e de status: proponha com portfolio_propose, com justificativa; o usuário aprova na tela Hub.",
  "Issues novas: proponha na conversa e crie com `gh issue create -R owner/repo` (no Jira, `jira issue create`) só depois do meu ok.",
  "Consulte suas decisões anteriores com ledger_list_decisions e registre as relevantes com ledger_record_decision.",
].join("\n");

// Projetos Jira + GitLab (spec do Jira e GitLab §9, #351), escolhidos pelo
// rastreador do projeto. Além das variáveis de sempre: {{key}} (PROJ-5450),
// {{title}}, {{base}}, {{jira_url}}, {{jira_project}}, {{jira}} e {{glab}}
// (o jira-cli e o glab com o comando de credencial do projeto, nunca a
// senha) e {{forge_path}}. Só o mecânico de todo projeto Jira + GitLab: as
// convenções de cada projeto e épico (formato do commit e do título do MR,
// restrições) vêm das regras do agente, no system prompt.
export const DEFAULT_JIRA_ISSUE_TEMPLATE = [
  "Trabalhe na {{kind}} {{key}} do Jira ({{jira_url}}/browse/{{key}}): {{title}}. Repositório GitLab {{forge_path}}.",
  "Leia a issue com `{{jira}} issue view {{key}} --comments 20 --plain`.",
  "Branch de trabalho: {{branch}}. Nunca mescle nem faça push em {{protected}}.",
  "Ao terminar, abra o MR para {{base}} com `{{glab}} mr create -R {{forge_path}} -s {{branch}} -b {{base}} -t '<título>' -d '<descrição>' -y` e, logo depois, comente o link na issue com `{{jira}} issue comment add {{key}} 'MR aberto: <url do MR>' --no-input`.",
  "Nunca mescle o MR nem mude o status da issue no Jira (nada de `jira issue move`): o merge e a transição são do usuário.",
  "Outros comentários no Jira e issues novas, só depois do meu ok.",
  "Siga as regras do projeto e do épico do system prompt (formato do commit e do MR, por exemplo); releia com `rules_get` e proponha mudanças com `rules_propose`.",
  "Antes de começar, consulte decisões anteriores com `ledger_list_decisions`.",
  "Registre toda decisão relevante com `ledger_record_decision`.",
].join("\n");

export const DEFAULT_JIRA_EPIC_TEMPLATE = [
  "Trabalhe no épico {{key}} do Jira ({{jira_url}}/browse/{{key}}): {{title}}. Repositório GitLab {{forge_path}}.",
  "Leia o épico com `{{jira}} issue view {{key}} --comments 20 --plain` e liste as issues dele com `{{jira}} epic list {{key}} --plain`.",
  "Uma issue por vez, cada uma em MR para {{branch}} com `{{glab}} mr create -R {{forge_path}} -s <branch da issue> -b {{branch}} -t '<título>' -d '<descrição>' -y`; logo depois de abrir cada MR, comente o link na issue com `{{jira}} issue comment add <chave> 'MR aberto: <url do MR>' --no-input`.",
  "Ao concluir o épico, abra o MR de {{branch}} para {{base}} do mesmo jeito e comente o link em {{key}}.",
  "Nunca mescle MR nem faça push em {{protected}}, e não mude o status de issues no Jira (nada de `jira issue move`): o merge e a transição são do usuário.",
  "Issue nova no épico, só depois do meu ok: `{{jira}} issue create -p {{jira_project}} -t <tipo> -s '<título>' -b '<descrição>' -P {{key}} --no-input` (o -P liga ao épico pelo Epic Link; numa sub-tarefa, `-P <chave da issue>`).",
  "Siga as regras do projeto e do épico do system prompt; releia com `rules_get` e proponha mudanças com `rules_propose`.",
  "Consulte `ledger_list_decisions` com scope epic antes de começar.",
  "Registre toda decisão relevante com `ledger_record_decision`.",
].join("\n");

export const DEFAULT_JIRA_PROJECT_TEMPLATE = [
  "Sessão de planejamento no clone principal de {{forge_path}} (GitLab), com as issues no projeto {{jira_project}} do Jira ({{jira_url}}), sem issue associada.",
  "Use-a para entender o projeto e propor issues e épicos. Leia com `{{jira}} issue list -p {{jira_project}} --plain` e `{{jira}} issue view <chave> --comments 20 --plain`.",
  "Crie só depois do meu ok: issue com `{{jira}} issue create -p {{jira_project}} -t <tipo> -s '<título>' -b '<descrição>' -P <chave do épico> --no-input` (o -P liga ao épico pelo Epic Link; numa sub-tarefa, `-P <chave da issue>`) e épico com `{{jira}} epic create -p {{jira_project}} -n '<nome>' -s '<título>' -b '<descrição>' --no-input`.",
  "Comente com `{{jira}} issue comment add <chave> '<texto>' --no-input`, também só depois do meu ok. Nunca mude o status de issues (nada de `jira issue move`).",
  "Não faça commits nem mescle em {{protected}}: código vai em sessões de issue.",
  "Siga as regras do projeto do system prompt; releia com `rules_get` e proponha mudanças com `rules_propose`.",
  "Consulte decisões anteriores com `ledger_list_decisions` e registre as relevantes com `ledger_record_decision`.",
].join("\n");

// Prompts prontos do console (#425; spec da #87, §2.4), com as mesmas
// variáveis dos templates. Espelham `default_prompts` de src-tauri/src/envs.rs.
export const DEFAULT_PROMPTS: ReadyPrompt[] = [
  { id: "continue", name: "Continuar", text: "continue" },
  {
    id: "tests",
    name: "Rodar os testes e corrigir",
    text: [
      "Rode os testes do projeto (e o typecheck e o lint, se houver).",
      "Corrija as falhas e rode de novo até ficar tudo verde; no fim, diga o que quebrou e o que você mudou.",
    ].join("\n"),
  },
  {
    id: "ledger",
    name: "Registrar as decisões no ledger",
    text: [
      "Revise o que foi decidido nesta sessão e registre com `ledger_record_decision` cada decisão relevante que ainda não está no ledger.",
      'Use origin "autonoma" para o que você decidiu sozinho e "orientada" para o que eu defini.',
    ].join("\n"),
  },
  {
    id: "pr",
    name: "Abrir o PR",
    text: [
      "Se o trabalho estiver pronto, faça o commit do que falta, dê push na branch {{branch}} e abra o PR (ou MR) como pedem as instruções do início da sessão.",
      "Não mescle: me mande o link.",
    ].join("\n"),
  },
  {
    id: "summary",
    name: "Resumir o estado atual",
    text: "Resuma o estado atual do trabalho: o que já está feito, o que falta, o que está bloqueado e o próximo passo. Não mude nada agora.",
  },
];

export const DEFAULT_UI_CONFIG: UiConfig = {
  templates: {
    issue: DEFAULT_ISSUE_TEMPLATE,
    epic: DEFAULT_EPIC_TEMPLATE,
    project: DEFAULT_PROJECT_TEMPLATE,
    gestor: DEFAULT_GESTOR_TEMPLATE,
    jira_issue: DEFAULT_JIRA_ISSUE_TEMPLATE,
    jira_epic: DEFAULT_JIRA_EPIC_TEMPLATE,
    jira_project: DEFAULT_JIRA_PROJECT_TEMPLATE,
  },
  prompts: DEFAULT_PROMPTS,
  notify: { needs_human: true, session_error: true, env_down: true },
  session: { model: null, mode: "native", remote_control: false },
  appearance: {
    theme: "dark",
    tabs: {
      dark: { terminal: {}, shell: {}, issue: {}, screen: {} },
      light: { terminal: {}, shell: {}, issue: {}, screen: {} },
    },
  },
};
