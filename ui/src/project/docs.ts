// "Abrir pasta de docs" do projeto e do épico (#444, épico #445): só no app
// desktop, por um ambiente desta máquina (WSL, ou o `ctowerd` local do
// macOS, registrado como remoto com URL de loopback) conectado que tenha o
// projeto, com docs ligados e `docs_root` acessível. Funções puras,
// testadas em docs.test.ts; a mesma regra do lado Rust (src-tauri/src/folder.rs).
import type { EnvView } from "../api/types";
import type { EnvInfo, Project } from "../protocol/gen";
import { versionNewer } from "../util/version";

/** Protocolo do ambiente com `docs.dir` (#444). */
export const DOCS_DIR_PROTOCOL = "2.8";

/** Seção "Docs dos projetos" de docs/hub.md: montagem, marcador e `docs_root`. */
export const DOCS_HELP_URL =
  "https://github.com/deep-ion-ai/ctower-agents/blob/develop/docs/hub.md#docs-dos-projetos-fora-do-reposit%C3%B3rio";

/** Ambiente conectado com o projeto (um `Clone` basta). */
export interface DocsCandidate {
  env: string;
  name: string;
  project: Project;
}

export type DocsFolderState =
  | { enabled: true; env: string; projectId: number }
  | { enabled: false; reason: string };

const atLeast = (protocol: string, want: string) => !versionNewer(want, protocol);

/** A URL do ambiente aponta para esta máquina: `localhost`, `127.0.0.0/8` ou `::1`. */
export function isLoopbackUrl(url: string): boolean {
  let host: string;
  try {
    host = new URL(url.trim()).hostname.toLowerCase();
  } catch {
    return false;
  }
  host = host.replace(/^\[|\]$/g, "");
  return host === "localhost" || host === "::1" || /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host);
}

/** A pasta do ambiente abre nesta máquina: WSL, ou "remoto" de loopback (ctowerd local do macOS). */
export const isLocalEnv = (e: EnvView | undefined): boolean =>
  !!e && (e.kind === "wsl" || isLoopbackUrl(e.url));

/**
 * Onde abrir a pasta de docs, ou por que não dá (a dica do botão
 * desabilitado). Vale o primeiro ambiente local em condições; sem nenhum, o
 * motivo do primeiro local com docs ligados.
 */
export function docsFolderState(
  candidates: DocsCandidate[],
  envs: EnvView[],
  infos: Record<string, EnvInfo>,
): DocsFolderState {
  if (candidates.length === 0) return { enabled: false, reason: "Nenhum ambiente conectado tem este projeto." };
  // Cache anterior à #440 não traz `docs`: desligado.
  const on = candidates.filter((c) => c.project.docs?.enabled);
  if (on.length === 0) {
    return { enabled: false, reason: "Docs desligados neste projeto: ligue em Hub › Projetos › Editar." };
  }
  const wsl = on.filter((c) => isLocalEnv(envs.find((e) => e.id === c.env)));
  if (wsl.length === 0) {
    return {
      enabled: false,
      reason:
        "Abrir a pasta só nos ambientes desta máquina (WSL ou local): a pasta de docs de um ambiente remoto fica em outra máquina.",
    };
  }
  const ok = wsl.find((c) => {
    const i = infos[c.env];
    return !!i && atLeast(i.protocol, DOCS_DIR_PROTOCOL) && !!i.docs?.root && i.docs.reachable;
  });
  if (ok) return { enabled: true, env: ok.env, projectId: ok.project.id };
  const c = wsl[0];
  const i = infos[c.env];
  if (!i) return { enabled: false, reason: `Lendo o estado do ambiente ${c.name}…` };
  if (!atLeast(i.protocol, DOCS_DIR_PROTOCOL)) {
    return {
      enabled: false,
      reason: `Atualize o ambiente ${c.name} (protocolo ${i.protocol}; abrir a pasta de docs pede ${DOCS_DIR_PROTOCOL} ou maior).`,
    };
  }
  if (!i.docs?.root) {
    return { enabled: false, reason: `docs_root não configurado no ambiente ${c.name} (config.toml do ctowerd).` };
  }
  return {
    enabled: false,
    reason: `docs_root ${i.docs.root} inacessível no ambiente ${c.name}: confira a montagem do compartilhamento.`,
  };
}
