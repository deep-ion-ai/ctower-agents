// O que some no modo web (spec do épico #205 §10.2): telas, lugares da
// paleta, seções do Hub e das Configurações exclusivas do desktop. Funções
// puras, testadas em mode.test.ts. A decisão de estar na web vem da
// implementação da DaemonApi (`api.web`, `useTower().web`): o bundle web usa
// a `WebDaemonApi` (`main.web.tsx`), e os testes, a `FakeDaemonApi.asWeb()`.
import type { HubSection } from "../hub/sections";
import { MOBILE_QUERY, useMediaQuery } from "../shell/layout";
import { useTower } from "../state/tower";
import type { SettingsSection } from "../settings/sections";
import type { Place } from "../shell/places";
import type { Screen } from "../state/nav";

/**
 * Telas sem lugar na barra de atividades da web: o Terminal (no lugar
 * dele, a aba da sessão mostra o painel de estado, `SessionPanel`) e a
 * Manutenção inteira (#340), inclusive Backup.
 */
export const WEB_HIDDEN_SCREENS: ReadonlySet<Screen["name"]> = new Set(["terminal", "maintenance"]);

/** Seções do Hub fora da web: o grupo Segurança (secrets, chave de recuperação, token do GitHub). */
export const WEB_HIDDEN_HUB: ReadonlySet<HubSection> = new Set(["secrets", "recovery", "github"]);

/**
 * Configurações na web: só as do navegador (tema) e o acesso web. Somem
 * "Este app" do desktop (conexões, sessões, terminal, notificações) e o
 * escopo Ambiente (só leitura do daemon).
 */
export const WEB_SETTINGS: ReadonlySet<SettingsSection> = new Set(["appearance", "web-access"]);

/**
 * Explicação padrão das ações só do desktop (#26): na web elas aparecem
 * desabilitadas com este aviso (`DesktopOnly`), em vez de sumir.
 */
export const DESKTOP_ONLY_NOTE = "Cadastro e edição no app desktop";

export const hubSectionOnWeb = (s: HubSection) => !WEB_HIDDEN_HUB.has(s);

/** Lugar da paleta que existe na web. */
export function placeOnWeb(p: Place): boolean {
  if ("screen" in p.open) return !WEB_HIDDEN_SCREENS.has(p.open.screen);
  const s = p.open.tab.screen;
  if (WEB_HIDDEN_SCREENS.has(s.name)) return false;
  if (s.name === "hub") return hubSectionOnWeb(s.section ?? "overview");
  if (s.name === "settings") {
    // Os escopos ("Este app", "Ambiente") e os templates são do desktop.
    return !s.template && !!s.section && WEB_SETTINGS.has(s.section);
  }
  return true;
}

/** Rótulo do lugar na web: sem o escopo "Este app", que não existe lá. */
export const webPlaceLabel = (label: string) => label.replace("Configurações › Este app › ", "Configurações › ");

/**
 * Celular (spec do épico #205 §10.3): modo web abaixo de 768 px, o mesmo
 * corte do shell. As telas o usam para trazer ao corpo o que no desktop fica
 * no painel lateral (#25).
 */
export function useMobile(): boolean {
  const narrow = useMediaQuery(MOBILE_QUERY);
  const { web } = useTower();
  return narrow && web;
}
