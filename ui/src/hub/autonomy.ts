// Modo autônomo do projeto (#181) em Configurações › Modo autônomo e no
// aviso da Visão geral: o estado do modo, a branch de integração, o relato
// do ambiente do modo e o `autonomy` do `hub.projects.update`. Funções
// puras, testadas em autonomy.test.ts.
import type { AlignStrategy, AutonomyConfig, HubAssignment, HubEnv, HubProject, SessionTemplates } from "../protocol/hub-gen";

/** Protocolo do hub com o modo autônomo (#181). */
export const AUTONOMY_PROTOCOL = "1.15";

/** Branch de integração padrão. */
export const DEFAULT_AUTONOMY_BRANCH = "wip/autonoma";

/**
 * Template padrão da sessão autônoma (#191), espelho de
 * `ct_core::session_templates::DEFAULT_AUTONOMOUS_TEMPLATE` (o teste
 * `padrao_espelhado_na_ui` confere linha a linha). Quem renderiza é o
 * ambiente; aqui é só o placeholder do campo.
 */
export const DEFAULT_AUTONOMOUS_TEMPLATE = [
  "Sessão autônoma do Control Tower: trabalhe na {{kind}} {{ref}} ({{title}}) do repositório {{owner}}/{{repo}} do começo ao fim, sem perguntar nada: ninguém acompanha esta sessão.",
  "Branch de trabalho: {{branch}}, saída de {{integration_branch}} (a branch de integração do modo autônomo). Nunca mescle nem faça push em {{protected}}.",
  "Antes de começar, consulte decisões anteriores com `ledger_list_decisions`.",
  "Diante de uma dúvida, escolha a opção mais conservadora que resolva a issue e siga em frente. Registre toda decisão relevante com `ledger_record_decision` e origin `autonoma`: elas são revisadas no {{change}}.",
  "Siga as regras do projeto e do épico do system prompt; releia com `rules_get`.",
  "Ao terminar, com os testes passando, abra o {{change}} de {{branch}} para {{integration_branch}} (nunca para outra branch), com a linha `{{closes}}` no corpo, exatamente assim, e resuma o que foi feito e as decisões tomadas. Não mescle o {{change}}.",
  "Antes de dizer que o {{change}} está pronto, consulte `pr_feedback`: corrija os checks que falharam (o log vem com `pr_check_log`) e as threads de review não resolvidas, e faça push de novo.",
].join("\n");

/**
 * Template padrão da sessão de análise de dependências do piloto (#215),
 * espelho de `ct_core::session_templates::DEFAULT_ANALYSIS_TEMPLATE` (o teste
 * `padrao_de_analise_espelhado_na_ui` confere linha a linha). Quem renderiza
 * é o hub; aqui é só o placeholder do campo.
 */
export const DEFAULT_ANALYSIS_TEMPLATE = [
  "Sessão de análise de dependências do piloto automático do Control Tower, no repositório {{owner}}/{{repo}}. Ninguém acompanha esta sessão: não pergunte nada.",
  "Sua tarefa é montar a ordem de execução e as dependências entre as issues da fila do modo autônomo. Você não implementa nada, não cria branch, não faz commit nem push e não altera issues no rastreador (nada de comentar, rotular ou fechar).",
  "Issues da fila (abertas, aprovadas e com prioridade):",
  "{{issues}}",
  "Issues já em andamento ou em revisão (podem bloquear as da fila):",
  "{{in_progress}}",
  "Dependências já gravadas (as manuais foram corrigidas pela pessoa e prevalecem):",
  "{{dependencies}}",
  "Leia cada issue no rastreador ({{tracker_hint}}): título, corpo, épico, menções como “Depende de #N” e as áreas do código citadas. Leia o código na branch de integração {{integration_branch}} sem trocar de branch: `git fetch origin {{integration_branch}}` e depois `git show origin/{{integration_branch}}:<caminho>` ou `git log origin/{{integration_branch}}`.",
  "Uma issue depende de outra quando precisa do código, da migração ou da decisão dela já mesclados para começar sem retrabalho ou conflito. Na dúvida, não crie a dependência.",
  "Grave o resultado com a ferramenta `autopilot_set_dependencies`: para cada issue da fila, a lista de issues que a bloqueiam (vazia se nenhuma), com uma justificativa curta para cada uma, e a ordem de execução sugerida. Depois disso, fique parado: o hub manda aqui as issues novas ou alteradas, e você reanalisa só o necessário e grava de novo.",
].join("\n");

/** Tamanho máximo do template, em bytes (o de `SessionTemplates`). */
export const MAX_AUTONOMOUS_TEMPLATE_BYTES = 64 * 1024;

export const STRATEGY_LABEL: Record<AlignStrategy, string> = {
  merge: "Merge da base na branch",
  rebase: "Rebase da branch sobre a base",
};

/** O modo do projeto; `null` = hub anterior à #181 (sem modo autônomo). */
export function autonomyOf(p: HubProject): AutonomyConfig | null {
  return (p as Partial<HubProject>).autonomy ?? null;
}

/** Branch de integração em vigor. */
export const autonomyBranch = (a: AutonomyConfig): string => a.branch.trim() || DEFAULT_AUTONOMY_BRANCH;

/** Base em vigor: a do modo ou a do projeto. */
export const autonomyBase = (a: AutonomyConfig, p: HubProject): string => a.base_branch.trim() || p.base_branch;

/**
 * Aviso do projeto com o modo ligado e a branch em conflito com a base
 * ("wip/autonoma em conflito com a base"); `null` sem aviso.
 */
export function autonomyWarning(p: HubProject): string | null {
  const a = autonomyOf(p);
  const r = (p as Partial<HubProject>).autonomy_report ?? null;
  if (!a?.enabled || r?.state !== "conflict") return null;
  return r.message ?? `${autonomyBranch(a)} em conflito com a base`;
}

/**
 * Erro da branch de integração com o modo ligado (relato `error` do
 * ambiente do modo, ex.: o push recusado); `null` sem erro. Com erro, o
 * agendador não inicia issues no projeto.
 */
export function autonomyError(p: HubProject): string | null {
  const a = autonomyOf(p);
  const r = (p as Partial<HubProject>).autonomy_report ?? null;
  if (!a?.enabled || r?.state !== "error") return null;
  return r.message ?? `erro em ${autonomyBranch(a)}`;
}

/** Ambientes onde o modo pode rodar: atribuídos ao projeto e marcados como autônomos. */
export function autonomyEnvChoices(assignments: HubAssignment[], envs: HubEnv[]): HubAssignment[] {
  const autonomous = new Set(envs.filter((e) => e.autonomous).map((e) => e.env_id));
  return assignments.filter((a) => autonomous.has(a.env_id));
}

/** Formulário do cartão. */
export interface AutonomyForm {
  enabled: boolean;
  envId: string;
  branch: string;
  base: string;
  strategy: AlignStrategy;
  /** Template autônomo do projeto (#191); `null` = hub sem o campo (não edita). */
  template: string | null;
  /** Template da sessão de análise (#215); `null` = hub sem o campo (não edita). */
  analysisTemplate: string | null;
}

/** O template autônomo do projeto; `null` = hub anterior à #191 (sem o campo). */
export function autonomousTemplateOf(p: HubProject): string | null {
  const t = (p as Partial<HubProject>).session_templates as Partial<SessionTemplates> | undefined;
  return typeof t?.autonomous === "string" ? t.autonomous : null;
}

/** O template da sessão de análise do projeto; `null` = hub anterior à #215. */
export function analysisTemplateOf(p: HubProject): string | null {
  const t = (p as Partial<HubProject>).session_templates as Partial<SessionTemplates> | undefined;
  return typeof t?.analysis === "string" ? t.analysis : null;
}

/**
 * `session_templates` do `hub.projects.update` com os templates autônomo e
 * de análise do formulário (o objeto inteiro: o hub substitui). Só espaços
 * = o padrão (vazio). `null` se nada mudou ou o hub não tem os campos.
 */
export function autonomyTemplatesPatch(f: AutonomyForm, p: HubProject): SessionTemplates | null {
  const clean = (t: string) => (t.trim() ? t : "");
  let out: SessionTemplates | null = null;
  const cur = autonomousTemplateOf(p);
  if (cur !== null && f.template !== null && clean(f.template) !== cur) {
    out = { ...p.session_templates, autonomous: clean(f.template) };
  }
  const curA = analysisTemplateOf(p);
  if (curA !== null && f.analysisTemplate != null && clean(f.analysisTemplate) !== curA) {
    out = { ...(out ?? p.session_templates), analysis: clean(f.analysisTemplate) };
  }
  return out;
}

export function autonomyForm(a: AutonomyConfig, p: HubProject): AutonomyForm {
  return {
    enabled: a.enabled,
    envId: a.env_id ?? "",
    branch: autonomyBranch(a),
    base: autonomyBase(a, p),
    strategy: a.strategy,
    template: autonomousTemplateOf(p),
    analysisTemplate: analysisTemplateOf(p),
  };
}

/**
 * `autonomy` do `hub.projects.update` (o objeto inteiro: o hub substitui).
 * Branch padrão e base do projeto vão vazias, para seguirem o padrão.
 */
export function autonomyPatch(f: AutonomyForm, p: HubProject): AutonomyConfig {
  const branch = f.branch.trim();
  const base = f.base.trim();
  return {
    enabled: f.enabled,
    env_id: f.envId.trim() || null,
    branch: branch === DEFAULT_AUTONOMY_BRANCH ? "" : branch,
    base_branch: base === p.base_branch ? "" : base,
    strategy: f.strategy,
  };
}

/** O que impede salvar o formulário, ou `null`. */
export function autonomyFormError(f: AutonomyForm, p: HubProject): string | null {
  if (f.enabled && !f.envId) return "Escolha o ambiente onde as sessões autônomas rodam.";
  const branch = f.branch.trim() || DEFAULT_AUTONOMY_BRANCH;
  const base = f.base.trim() || p.base_branch;
  if (branch === base) return "A branch de integração não pode ser a própria base.";
  if (p.protected_branches.includes(branch)) return `${branch} é uma branch protegida do projeto: o agente mescla nela.`;
  const bytes = new TextEncoder().encode(f.template ?? "").length;
  if (bytes > MAX_AUTONOMOUS_TEMPLATE_BYTES)
    return `O template da sessão autônoma tem no máximo ${MAX_AUTONOMOUS_TEMPLATE_BYTES / 1024} KiB (${bytes} bytes).`;
  const aBytes = new TextEncoder().encode(f.analysisTemplate ?? "").length;
  if (aBytes > MAX_AUTONOMOUS_TEMPLATE_BYTES)
    return `O template da sessão de análise tem no máximo ${MAX_AUTONOMOUS_TEMPLATE_BYTES / 1024} KiB (${aBytes} bytes).`;
  return null;
}
