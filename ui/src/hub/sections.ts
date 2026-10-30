// Seções do Hub (#313): endereço (aba própria por seção), grupos do painel
// lateral, contadores e o atalho de cada aviso de `hub.status` para a seção
// que o resolve. Funções puras, testadas em sections.test.ts.
import type { HubHealth, QuarantineGroup, Warning } from "../protocol/hub-gen";
import type { OpenTab, Screen } from "../state/nav";
import { plural } from "../shell/summary";
import { isBackupWarning, maintenanceTab } from "../maintenance/sections";

export type HubSection =
  | "overview"
  | "proposals"
  | "health"
  | "quarantine"
  | "projects"
  | "envs"
  | "secrets"
  | "recovery"
  | "github"
  /** Sessões web (#200, #201): listar e revogar. */
  | "web-sessions"
  /** Dispositivos da Web Push (#203): inscrever, eventos, testar e remover. */
  | "devices";

export const HUB_SECTION_LABEL: Record<HubSection, string> = {
  overview: "Visão geral",
  proposals: "Propostas do gestor",
  health: "Saúde dos projetos",
  quarantine: "Quarentena",
  projects: "Projetos e atribuições",
  envs: "Ambientes",
  secrets: "Secrets e variáveis",
  recovery: "Chave de recuperação",
  github: "Token do GitHub",
  "web-sessions": "Sessões web",
  devices: "Dispositivos",
};

/** Outros nomes pelos quais a paleta acha a seção. */
const KEYWORDS: Record<HubSection, string> = {
  overview: "avisos resumo início",
  proposals: "gestor aprovar rejeitar prioridade status",
  health: "sinais riscos parado bloqueado",
  quarantine: "push não cadastrado descartar",
  projects: "cadastro atribuições repositórios",
  envs: "ambientes envio pendências limpezas",
  secrets: "variáveis env tokens chaves",
  recovery: "chave recuperação secrets verificar",
  github: "token gh login",
  "web-sessions": "navegador celular pwa cookie revogar sair acesso",
  devices: "notificações push celular pwa avisos alertas",
};

export interface HubGroup {
  label: string;
  sections: HubSection[];
}

/** Grupos do painel lateral, na ordem. */
export const HUB_GROUPS: HubGroup[] = [
  { label: "Operação", sections: ["overview", "proposals", "health", "quarantine"] },
  { label: "Cadastro", sections: ["projects", "envs"] },
  { label: "Segurança", sections: ["secrets", "recovery", "github"] },
  // Revogar só reduz acesso: fica no desktop e na web (spec §3.1).
  { label: "Acesso", sections: ["web-sessions", "devices"] },
];

export const HUB_SECTIONS: HubSection[] = HUB_GROUPS.flatMap((g) => g.sections);

export function hubGroupOf(s: HubSection): string {
  return HUB_GROUPS.find((g) => g.sections.includes(s))!.label;
}

/** Cadastrar projeto com a URL preenchida (vinda da quarentena). Cada pedido (objeto novo) abre o formulário. */
export interface RegisterIntent {
  url: string;
  /** Tipo do projeto no formulário; ausente = GitHub. Jira + GitLab vem sem URL (#352). */
  kind?: "github" | "jira";
}

/** Projeto pela chave natural: o detalhe da seção Projetos (`hubTab("projects", { project })`, #314). */
export interface ProjectRef {
  owner: string;
  repo: string;
}

export type HubScreen = Extract<Screen, { name: "hub" }>;

/**
 * `id` da aba da seção. A Visão geral é a aba da própria tela (`hub`): a
 * barra de atividades e `go({ name: "hub" })` caem nela.
 */
export const hubTabId = (s: HubSection): string => (s === "overview" ? "hub" : `hub:${s}`);

/** Aba da seção, para `useNav().openTab` (ex.: link "Hub › Secrets" das Configurações). */
export function hubTab(s: HubSection, extra: Omit<HubScreen, "name" | "section"> = {}): OpenTab {
  return {
    id: hubTabId(s),
    screen: s === "overview" ? { name: "hub", ...extra } : { name: "hub", section: s, ...extra },
  };
}

/** Título da aba: "Hub · Propostas do gestor". */
export const hubTitle = (s: HubSection | undefined): string => `Hub · ${HUB_SECTION_LABEL[s ?? "overview"]}`;

/**
 * Lugares da paleta (#312): "Hub › Secrets e variáveis". Backup e
 * snapshots do hub ficam em Manutenção (#338).
 */
export const HUB_PLACES = HUB_SECTIONS.map((s) => ({
  id: `hub:${s}`,
  label: `Hub › ${HUB_SECTION_LABEL[s]}`,
  keywords: `hub ${KEYWORDS[s]}`,
  open: { tab: hubTab(s) },
}));

export interface WarningAction {
  /** Seção do Hub que resolve; `null` quando fica fora do Hub (backup: Manutenção, #338). */
  section: HubSection | null;
  /** Aba que o botão abre. */
  tab: OpenTab;
  /** Texto do botão que resolve. */
  label: string;
}

const inHub = (section: HubSection, label: string): WarningAction => ({ section, tab: hubTab(section), label });

/**
 * Onde se resolve um aviso de `hub.status` (pelo `kind`, estável): uma seção
 * do Hub ou, para backup e snapshots, Manutenção › Backup e snapshots.
 */
export function warningAction(kind: string, web = false): WarningAction | null {
  // Web: Manutenção e o grupo Segurança não existem (spec do épico #205 §10.2).
  if (web && (isBackupWarning(kind) || kind.startsWith("recovery_") || kind === "secrets_rotate_pending")) return null;
  if (isBackupWarning(kind)) return { section: null, tab: maintenanceTab("backup"), label: "Configurar backup" };
  if (kind.startsWith("recovery_")) return inHub("recovery", "Verificar chave");
  if (kind === "push_quarantine") return inHub("quarantine", "Revisar quarentena");
  if (kind === "secrets_rotate_pending") return inHub("secrets", "Revisar secrets");
  return null;
}

export interface SectionCount {
  /** Número, ou "!" quando a seção pede atenção sem contagem. */
  n: number | "!";
  /** Descrição acessível ("2 pendentes"). */
  text: string;
  tone?: "attention" | "error";
}

export interface HubCountsInput {
  warnings: Warning[];
  quarantine: QuarantineGroup[];
  /** Propostas pendentes. */
  pending: number;
  health: HubHealth | null;
  projects: number;
  envs: number;
}

/** Projetos com linhas em quarentena (um aviso `push_quarantine` por projeto). */
export const quarantineProjects = (q: QuarantineGroup[]): number => new Set(q.map((g) => g.project_key)).size;

/** Sinais que pedem ação: os de aviso e os críticos (os informativos ficam só na seção). */
export const actionableSignals = (h: HubHealth | null) => (h?.signals ?? []).filter((s) => s.severity !== "info");

/**
 * Contadores do painel lateral: Visão geral = avisos de `hub.status`,
 * Propostas = pendentes, Saúde = sinais de aviso e críticos, Quarentena =
 * projetos com linhas; Projetos e Ambientes = quantos há. As seções de
 * Segurança mostram "!" quando algum aviso é delas (os de backup contam só
 * na Visão geral: a seção fica em Manutenção).
 */
export function hubCounts(i: HubCountsInput): Partial<Record<HubSection, SectionCount>> {
  const out: Partial<Record<HubSection, SectionCount>> = {};
  if (i.warnings.length > 0) {
    out.overview = { n: i.warnings.length, text: plural(i.warnings.length, "aviso", "avisos"), tone: "attention" };
  }
  if (i.pending > 0) out.proposals = { n: i.pending, text: plural(i.pending, "pendente", "pendentes"), tone: "attention" };
  const signals = actionableSignals(i.health);
  if (signals.length > 0) {
    out.health = {
      n: signals.length,
      text: plural(signals.length, "sinal", "sinais"),
      tone: signals.some((s) => s.severity === "critico") ? "error" : undefined,
    };
  }
  const q = quarantineProjects(i.quarantine);
  if (q > 0) out.quarantine = { n: q, text: plural(q, "projeto", "projetos"), tone: "attention" };
  if (i.projects > 0) out.projects = { n: i.projects, text: plural(i.projects, "projeto", "projetos") };
  if (i.envs > 0) out.envs = { n: i.envs, text: plural(i.envs, "ambiente", "ambientes") };
  for (const w of i.warnings) {
    const a = warningAction(w.kind);
    if (a?.section && a.section !== "quarantine" && !out[a.section]) out[a.section] = { n: "!", text: "pede atenção", tone: "attention" };
  }
  return out;
}
