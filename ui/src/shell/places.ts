// Telas e seções navegáveis (paleta #312, barra de atividades). Para uma
// seção nova (ex.: "Hub › Secrets", #313), acrescente um `Place` com
// `tab` (aba própria por `useNav().openTab`) em `PLACES`.
import { HUB_PLACES } from "../hub/sections";
import { MAINTENANCE_PLACES } from "../maintenance/sections";
import { SETTINGS_PLACES } from "../settings/sections";
import type { Nav, OpenTab, Screen } from "../state/nav";
import { SCREEN_LABEL } from "./titles";

export interface Place {
  /** Estável: vai para os recentes da paleta. */
  id: string;
  /** "Hub" ou "Hub › Secrets". */
  label: string;
  /** Outros nomes pelos quais a busca acha o lugar. */
  keywords?: string;
  /** Tela (volta à aba já aberta dela) ou uma aba própria. */
  open: { screen: Screen["name"] } | { tab: OpenTab };
}

const screen = (name: Screen["name"], keywords?: string): Place => ({
  id: `screen:${name}`,
  label: SCREEN_LABEL[name],
  keywords,
  open: { screen: name },
});

export const PLACES: Place[] = [
  screen("control", "início sessões aguardando atenção"),
  screen("terminal", "console sessões"),
  screen("project", "índice lista árvore issues épicos kanban prioridade"),
  screen("review", "decisões pr"),
  screen("recovery", "interrompidas retomar"),
  screen("hub", "cthubd projetos atribuições ambientes secrets quarentena propostas gestor"),
  screen("maintenance", "atualizações limpeza backup snapshots versão daemon worktrees"),
  screen("settings", "configurações preferências ambientes atualizações templates"),
  // Índice dos ambientes (#23); cada um, com as seções, vem da paleta (items.ts).
  screen("env", "ambientes daemons ctowerd conexão hub detalhe"),
  ...HUB_PLACES,
  ...MAINTENANCE_PLACES,
  ...SETTINGS_PLACES,
];

/**
 * Vai para a tela: a aba já aberta dela (com o endereço que tinha), ou uma
 * nova. Mesma regra da barra de atividades.
 */
export function openScreen(nav: Pick<Nav, "editors" | "activate" | "go">, name: Screen["name"]): void {
  // Projetos (#16) é sempre o índice: as abas de projeto (`project:<chave>`)
  // e de item continuam abertas, mas a atividade não cai nelas.
  if (name === "project") return nav.go({ name: "project" });
  // Idem para Ambientes (#23): o índice, não a aba de um ambiente.
  if (name === "env") return nav.go({ name: "env" });
  const tab =
    nav.editors.find((e) => e.kind === "screen" && e.id === name) ??
    [...nav.editors].reverse().find((e) => e.kind === "screen" && e.screen.name === name);
  if (tab) nav.activate(tab.id);
  else nav.go({ name } as Screen);
}

export function openPlace(nav: Pick<Nav, "editors" | "activate" | "go" | "openTab">, p: Place): void {
  if ("screen" in p.open) openScreen(nav, p.open.screen);
  else nav.openTab(p.open.tab);
}
