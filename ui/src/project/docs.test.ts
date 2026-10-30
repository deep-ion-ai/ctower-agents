import { expect, test } from "vitest";
import { FakeDaemonApi } from "../api/fake";
import type { EnvView } from "../api/types";
import type { EnvInfo } from "../protocol/gen";
import { envInfo, project } from "../test/fixtures";
import { docsFolderState, isLoopbackUrl, type DocsCandidate } from "./docs";

const wsl = FakeDaemonApi.wslEnv();
const remote: EnvView = { id: "mac", name: "mac", url: "ws://mac:7878/ws", kind: "remote", distro: null, has_token: true };
const ON = { enabled: true, project_dirs: ["analises"], epic_dirs: [] };
const UP = { root: "/mnt/ct-docs", reachable: true };

const cand = (env: string, docs = ON, id = 1): DocsCandidate => ({ env, name: env, project: project({ id, docs }) });

test("WSL com docs ligados e docs_root acessível: habilitado, pelo primeiro ambiente em condições", () => {
  expect(docsFolderState([cand("wsl")], [wsl], { wsl: envInfo({ docs: UP }) })).toEqual({
    enabled: true,
    env: "wsl",
    projectId: 1,
  });
  // O remoto vem antes na lista, mas só o WSL abre a pasta nesta máquina.
  expect(
    docsFolderState([cand("mac", ON, 9), cand("wsl", ON, 3)], [remote, wsl], {
      mac: envInfo({ docs: UP }),
      wsl: envInfo({ docs: UP }),
    }),
  ).toEqual({ enabled: true, env: "wsl", projectId: 3 });
});

test.each([
  { caso: "sem ambiente com o projeto", c: [], envs: [wsl], infos: {}, motivo: /Nenhum ambiente conectado/ },
  {
    caso: "docs desligados no projeto",
    c: [cand("wsl", { ...ON, enabled: false })],
    envs: [wsl],
    infos: { wsl: envInfo({ docs: UP }) },
    motivo: /Docs desligados neste projeto/,
  },
  {
    caso: "projeto do cache anterior à #440 (sem docs)",
    c: [{ env: "wsl", name: "wsl", project: { ...project(), docs: undefined as never } }],
    envs: [wsl],
    infos: { wsl: envInfo({ docs: UP }) },
    motivo: /Docs desligados/,
  },
  {
    caso: "só ambiente remoto",
    c: [cand("mac")],
    envs: [remote],
    infos: { mac: envInfo({ docs: UP }) },
    motivo: /só nos ambientes desta máquina/,
  },
  { caso: "env.info ainda não lido", c: [cand("wsl")], envs: [wsl], infos: {}, motivo: /Lendo o estado/ },
  {
    caso: "ambiente anterior ao docs.dir",
    c: [cand("wsl")],
    envs: [wsl],
    infos: { wsl: envInfo({ protocol: "2.7", docs: UP }) },
    motivo: /Atualize o ambiente wsl \(protocolo 2\.7/,
  },
  {
    caso: "docs_root não configurado",
    c: [cand("wsl")],
    envs: [wsl],
    infos: { wsl: envInfo() },
    motivo: /docs_root não configurado no ambiente wsl/,
  },
  {
    caso: "docs_root inacessível",
    c: [cand("wsl")],
    envs: [wsl],
    infos: { wsl: envInfo({ docs: { root: "/mnt/ct-docs", reachable: false, warning: "docs_unreachable" } }) },
    motivo: /docs_root \/mnt\/ct-docs inacessível no ambiente wsl/,
  },
])("desabilitado com dica: $caso", ({ c, envs, infos, motivo }) => {
  const st = docsFolderState(c, envs, infos as Record<string, EnvInfo>);
  expect(st.enabled).toBe(false);
  if (!st.enabled) expect(st.reason).toMatch(motivo);
});

test("ctowerd local do macOS (remoto com URL de loopback) abre a pasta; remoto de verdade não", () => {
  const mac: EnvView = { ...remote, id: "local", url: "ws://127.0.0.1:7878/ws" };
  expect(docsFolderState([cand("local")], [mac], { local: envInfo({ docs: { root: "/Volumes/ctdocs", reachable: true } }) })).toEqual({
    enabled: true,
    env: "local",
    projectId: 1,
  });
  for (const u of ["ws://127.0.0.1:7878/ws", "ws://127.1.2.3/ws", "WS://LOCALHOST:7878/ws", "wss://[::1]:7878/ws"]) {
    expect(isLoopbackUrl(u)).toBe(true);
  }
  for (const u of ["ws://192.168.0.10:7878/ws", "ws://mac-mini:7878/ws", "ws://localhost.example.com/ws", "ws://[::2]/ws", "lixo"]) {
    expect(isLoopbackUrl(u)).toBe(false);
  }
});
