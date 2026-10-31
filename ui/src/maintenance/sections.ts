// Seções da tela Manutenção (#335): endereço (aba própria por seção), grupos
// do painel lateral e os lugares da paleta. Funções puras, testadas em
// sections.test.ts.
import type { ProjectRef } from "../hub/sections";
import type { OpenTab, Screen } from "../state/nav";

export type MaintenanceSection = "overview" | "app-updates" | "env-updates" | "hub-updates" | "cleanup" | "backup";

/** Nome da seção no painel lateral (dentro do grupo, quando há grupo). */
export const MAINTENANCE_SECTION_LABEL: Record<MaintenanceSection, string> = {
  overview: "Visão geral",
  "app-updates": "Este app",
  "env-updates": "Ambientes",
  "hub-updates": "Hub",
  cleanup: "Limpeza",
  backup: "Backup e snapshots",
};

/** Outros nomes pelos quais a paleta acha a seção. */
const KEYWORDS: Record<MaintenanceSection, string> = {
  overview: "resumo pendências ambientes",
  "app-updates": "versão atualizar app reiniciar notas updater",
  "env-updates": "daemon ctowerd versão atualizar verificar commits log reverter rollback",
  "hub-updates": "cthubd hub versão atualizar verificar protocolo commits log reverter rollback",
  cleanup: "limpeza limpar regras [cleanup] auto worktree target concluídos fora do hub clones projeto",
  backup: "snapshots restaurar remoto s3 drive hub",
};

export interface MaintenanceGroup {
  /** Sem rótulo: as seções ficam soltas no painel. */
  label?: string;
  sections: MaintenanceSection[];
}

/** Painel lateral, na ordem: Atualizações agrupa Este app, Ambientes e Hub. */
export const MAINTENANCE_GROUPS: MaintenanceGroup[] = [
  { sections: ["overview"] },
  { label: "Atualizações", sections: ["app-updates", "env-updates", "hub-updates"] },
  { sections: ["cleanup", "backup"] },
];

export const MAINTENANCE_SECTIONS: MaintenanceSection[] = MAINTENANCE_GROUPS.flatMap((g) => g.sections);

/**
 * Seções de um ambiente: o endereço leva o `env` e a tela mostra o seletor.
 * Atualizações › Ambientes não entra: mostra todos os ambientes numa tabela (#336).
 */
export const ENV_SECTIONS: MaintenanceSection[] = ["cleanup"];

export const isEnvSection = (s: MaintenanceSection): boolean => ENV_SECTIONS.includes(s);

/** Caminho da seção: ["Atualizações", "Este app"] ou ["Limpeza"]. */
export function maintenancePath(s: MaintenanceSection): string[] {
  const g = MAINTENANCE_GROUPS.find((x) => x.sections.includes(s))!;
  return g.label ? [g.label, MAINTENANCE_SECTION_LABEL[s]] : [MAINTENANCE_SECTION_LABEL[s]];
}

export type MaintenanceScreen = Extract<Screen, { name: "maintenance" }>;

/**
 * `id` da aba da seção. A Visão geral é a aba da própria tela
 * (`maintenance`): a barra de atividades e `go({ name: "maintenance" })`
 * caem nela.
 */
export const maintenanceTabId = (s: MaintenanceSection): string =>
  s === "overview" ? "maintenance" : `maintenance:${s}`;

/**
 * Aba da seção, para `useNav().openTab` (`env` só vale nas seções de
 * ambiente; `project`, o filtro por projeto, só na Limpeza, #22).
 */
export function maintenanceTab(s: MaintenanceSection, env?: string, project?: ProjectRef): OpenTab {
  const screen: MaintenanceScreen = { name: "maintenance" };
  if (s !== "overview") screen.section = s;
  if (env !== undefined && isEnvSection(s)) screen.env = env;
  if (project && s === "cleanup") screen.project = { owner: project.owner, repo: project.repo };
  return { id: maintenanceTabId(s), screen };
}

/** `owner/repo` do filtro, sem caixa: a chave que casa item do plano e projeto. */
export const projectFilterKey = (p: ProjectRef): string => `${p.owner}/${p.repo}`.toLowerCase();

/** Título da aba: "Manutenção · Atualizações · Este app". */
export const maintenanceTitle = (s: MaintenanceSection | undefined): string =>
  ["Manutenção", ...maintenancePath(s ?? "overview")].join(" · ");

/** Lugares da paleta: "Manutenção › Limpeza", "Manutenção › Atualizações › Ambientes". */
export const MAINTENANCE_PLACES = MAINTENANCE_SECTIONS.map((s) => ({
  id: `maintenance:${s}`,
  label: ["Manutenção", ...maintenancePath(s)].join(" › "),
  keywords: `manutenção ${KEYWORDS[s]}`,
  open: { tab: maintenanceTab(s) },
}));

/**
 * Aviso de `hub.status` que se resolve em Backup e snapshots (#338): os de
 * backup (inclusive o dos docs, #442) e os dos snapshots de secrets (pelo
 * `kind`, estável).
 */
export const isBackupWarning = (kind: string): boolean =>
  kind.startsWith("backup_") || kind.startsWith("docs_backup_") || kind.startsWith("secrets_snapshot_");
