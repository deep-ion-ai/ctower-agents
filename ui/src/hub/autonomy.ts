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
  "Ao terminar, com os testes passando, abra o {{change}} de {{branch}} para {{integration_branch}} (nunca para outra branch), cite {{ref}} no corpo e resuma o que foi feito e as decisões tomadas. Não mescle o {{change}}.",
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
}

/** O template autônomo do projeto; `null` = hub anterior à #191 (sem o campo). */
export function autonomousTemplateOf(p: HubProject): string | null {
  const t = (p as Partial<HubProject>).session_templates as Partial<SessionTemplates> | undefined;
  return typeof t?.autonomous === "string" ? t.autonomous : null;
}

/**
 * `session_templates` do `hub.projects.update` com o template autônomo do
 * formulário (o objeto inteiro: o hub substitui). Só espaços = o padrão
 * (vazio). `null` se nada mudou ou o hub não tem o campo.
 */
export function autonomyTemplatesPatch(f: AutonomyForm, p: HubProject): SessionTemplates | null {
  const cur = autonomousTemplateOf(p);
  if (cur === null || f.template === null) return null;
  const next = f.template.trim() ? f.template : "";
  if (next === cur) return null;
  return { ...p.session_templates, autonomous: next };
}

export function autonomyForm(a: AutonomyConfig, p: HubProject): AutonomyForm {
  return {
    enabled: a.enabled,
    envId: a.env_id ?? "",
    branch: autonomyBranch(a),
    base: autonomyBase(a, p),
    strategy: a.strategy,
    template: autonomousTemplateOf(p),
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
  return null;
}
