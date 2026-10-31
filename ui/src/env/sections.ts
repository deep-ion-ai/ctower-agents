// Detalhe do ambiente (#23, épico #20): o mesmo padrão do projeto (#17).
// Tudo sobre um ambiente numa tela só, uma seção por vez, cada uma com
// endereço próprio (`Screen` env com `section`), que a paleta, o deep link
// da PWA e os atalhos das outras telas (Configurações › Conexões, Hub ›
// Ambientes, Sala de controle, Manutenção › Atualizações) montam por
// `envAt`. Funções puras, testadas em sections.test.ts.
import type { OpenTab, Screen } from "../state/nav";

export type EnvAddr = Extract<Screen, { name: "env" }>;

/**
 * Seção do detalhe do ambiente. Cada uma compõe o que já existia em outro
 * lugar: Conexão = `EnvsSettings` (só este ambiente) e o `env.info`; Hub =
 * a ligação (`hub_config`), o envio (`hub_sync`, "Enviar ao hub") e o que o
 * hub sabe dele (`hub.envs.list`); Projetos = as atribuições do hub e o
 * `ProjectsSettings`; Atualização = `EnvUpdates` (só este ambiente) e os
 * ajustes de atualização; Configuração = os itens do escopo Ambiente das
 * Configurações; Limpeza = o resumo e o atalho para Manutenção › Limpeza
 * (#22: a limpeza fica só lá).
 */
export type EnvSection = "connection" | "hub" | "projects" | "update" | "config" | "cleanup";

/** Na ordem do índice. */
export const ENV_SECTIONS: EnvSection[] = ["connection", "hub", "projects", "update", "config", "cleanup"];

export const ENV_SECTION_LABEL: Record<EnvSection, string> = {
  connection: "Conexão",
  hub: "Hub",
  projects: "Projetos atribuídos",
  update: "Atualização",
  config: "Configuração do daemon",
  cleanup: "Limpeza",
};

/**
 * Seções que a web (só leitura, épico #205) não mostra: a atualização e a
 * limpeza são da Manutenção, e a configuração do daemon é o escopo
 * Ambiente das Configurações, que a web não tem (`WEB_SETTINGS`).
 */
export const DESKTOP_ONLY_ENV_SECTIONS: ReadonlySet<EnvSection> = new Set(["update", "config", "cleanup"]);

export const isEnvSection = (v: unknown): v is EnvSection =>
  typeof v === "string" && (ENV_SECTIONS as string[]).includes(v);

/** Seção mostrada: a pedida, se existe nesta visão (na web as do desktop caem em Conexão). */
export function envSectionOf(s: Pick<EnvAddr, "section">, web = false): EnvSection {
  const want = s.section;
  if (!want || (web && DESKTOP_ONLY_ENV_SECTIONS.has(want))) return "connection";
  return want;
}

/** Endereço do ambiente `env` na `section` (sem ela, Conexão: sai sem `section`). */
export function envAt(env: string, section?: EnvSection): EnvAddr {
  return { name: "env", env, ...(section && section !== "connection" ? { section } : {}) };
}

/**
 * Aba do detalhe (id `env:<id>`, `mainTabId`): trocar de seção troca o
 * endereço na mesma aba. `name` vira o título ("wsl · Atualização").
 */
export function envTab(env: string, name?: string, section?: EnvSection): OpenTab {
  const screen = envAt(env, section);
  return { screen, ...(name ? { title: envTitle(name, screen.section) } : {}) };
}

/** Título da aba: "wsl" ou "wsl · Atualização". */
export const envTitle = (name: string, section?: EnvSection): string =>
  section && section !== "connection" ? `${name} · ${ENV_SECTION_LABEL[section]}` : name;

/** Outros nomes pelos quais a paleta acha a seção. */
export const ENV_SECTION_KEYWORDS: Record<EnvSection, string> = {
  connection: "conexão url token distro wsl daemon ctowerd versão editar remover",
  hub: "hub ligar ligação envio enviar pendências token gestor agente cthubd add-env set-token",
  projects: "projetos atribuições clones",
  update: "atualizar versão canal update_ref reverter rollback",
  config: "configuração daemon github token sync intervalo worktrees notificações config.toml",
  cleanup: "limpeza limpar worktree target concluídos manutenção",
};
