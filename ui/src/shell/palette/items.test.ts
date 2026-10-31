import { FakeDaemonApi } from "../../api/fake";
import type { HubDashboard } from "../../protocol/hub-gen";
import { dashboard, hubProject, project, session, workItem } from "../../test/fixtures";
import { paletteItems, SYNC_PROJECT_PREFIX, SYNC_WEB_NOTE, type PaletteItem, type PaletteSources } from "./items";

const HUB_OFF: PaletteSources["hub"] = { up: false, dashboard: null, connected: false };

function src(p: Partial<PaletteSources> = {}): PaletteSources {
  return { envs: [], status: {}, dashboards: {}, hub: HUB_OFF, uiUpdate: null, liveGestor: [], ...p };
}

const byKind = (items: ReturnType<typeof paletteItems>, kind: string) => items.filter((i) => i.kind === kind);

test("sem hub e sem ambiente: telas e comandos", () => {
  const items = paletteItems(src());
  expect(byKind(items, "place").map((i) => i.label)).toEqual([
    "Sala de controle",
    "Terminal",
    "Projetos",
    "Revisão",
    "Recuperação",
    "Hub",
    "Manutenção",
    "Configurações",
    // Índice dos ambientes (#23).
    "Ambientes",
    // Seções do Hub (#313).
    "Hub › Visão geral",
    "Hub › Propostas do gestor",
    "Hub › Saúde dos projetos",
    "Hub › Quarentena",
    "Hub › Projetos e atribuições",
    "Hub › Ambientes",
    "Hub › Secrets e variáveis",
    "Hub › Chave de recuperação",
    "Hub › Token do GitHub",
    "Hub › Sessões web",
    "Hub › Dispositivos",
    // Seções da Manutenção (#335).
    "Manutenção › Visão geral",
    "Manutenção › Atualizações › Este app",
    "Manutenção › Atualizações › Ambientes",
    "Manutenção › Atualizações › Hub",
    "Manutenção › Limpeza",
    "Manutenção › Backup e snapshots",
    "Manutenção › Logs",
    // Escopos e seções das Configurações (#315).
    "Configurações › Este app",
    "Configurações › Ambiente",
    "Configurações › Este app › Conexões",
    "Configurações › Este app › Sessões",
    "Configurações › Este app › Aparência",
    "Configurações › Este app › Terminal",
    "Configurações › Este app › Notificações",
    "Configurações › Ambiente › GitHub",
    "Configurações › Ambiente › Atualização do daemon",
    "Configurações › Ambiente › Projetos locais",
    "Configurações › Ambiente › Notificações",
  ]);
  expect(byKind(items, "command").map((i) => i.label)).toEqual([
    "Buscar issues agora",
    "Abrir gestor",
    "Atualizar app",
    "Retomar interrompidas",
    "Atalhos e gestos",
    "Projeto: Novo…",
    // Tela Logs (#65): a UI na lista inicial; hub e ambientes só buscando.
    "Logs: esta UI",
  ]);
  expect(items.some((i) => i.kind === "issue" || i.kind === "session" || i.kind === "project")).toBe(false);
});

test("dados locais: sessões, projetos e issues sem repetir entre ambientes; #0 fica de fora", () => {
  const wsl = FakeDaemonApi.wslEnv("wsl");
  const mac = { ...FakeDaemonApi.wslEnv("mac"), name: "Mac mini" };
  const items0 = [
    workItem({ gh_number: 0, title: null }),
    workItem({ gh_number: 31, title: "ui: Sala", status: "aguardando_voce" }),
    workItem({ gh_number: 27, kind: "epic", title: "Plano 4", status: "concluido" }),
  ];
  const live = session({ id: "s1", work_item_id: 31 });
  const gone = session({ id: "s2", work_item_id: 27, state: "interrupted" });
  const items = paletteItems(
    src({
      envs: [wsl, mac],
      dashboards: {
        wsl: dashboard({ live_sessions: [live], interrupted_sessions: [gone] }, items0),
        mac: dashboard({}, [workItem({ gh_number: 31, title: "ui: Sala" })]),
      },
    }),
  );
  const sess = byKind(items, "session");
  expect(sess.map((s) => [s.label, s.detail])).toEqual([
    ["#31 ui: Sala", "ctower-agents · ctower-agents (WSL) · aguardando você"],
    ["#27 Plano 4", "ctower-agents · ctower-agents (WSL) · interrompida"],
  ]);
  expect(sess[0].action).toEqual({ type: "terminal", env: "wsl", session: "s1" });
  expect(sess[1].action).toEqual({ type: "screen", screen: { name: "recovery", env: "wsl", session: "s2" } });

  expect(byKind(items, "project")).toHaveLength(1);
  const issues = items.filter((i) => i.kind === "issue" || i.kind === "epic");
  expect(issues.map((i) => [i.label, i.kind, !!i.closed])).toEqual([
    ["#31 ui: Sala", "issue", false],
    ["#27 Plano 4", "epic", true],
  ]);
  // O item abre em aba própria (#317).
  expect(issues[0].action).toEqual({
    type: "tab",
    tab: {
      id: "project:deep-ion-ai/ctower-agents#31",
      title: "#31 ui: Sala",
      screen: {
        name: "project",
        env: "wsl",
        projectId: 1,
        owner: "deep-ion-ai",
        repo: "ctower-agents",
        item: { ghNumber: 31 },
      },
    },
  });
  // Iniciar só para as abertas.
  const starts = items.filter((i) => i.action.type === "start");
  expect(starts.map((s) => s.label)).toEqual(["Iniciar sessão: #31 ui: Sala"]);
  expect(starts[0].action).toMatchObject({ target: { ghNumber: 31, env: "wsl", projectId: 1 } });
  // Interrompidas aparecem no comando.
  expect(items.find((i) => i.id === "cmd:resume")!.detail).toBe("1 na Recuperação");
});

test("hub no ar: issues do hub completadas com o projeto local; projeto do gestor fica de fora", () => {
  const hd: HubDashboard = {
    projects: [
      {
        project: hubProject({ gh_owner: "Deep-Ion-AI" }),
        work_items: [{ ...workItem({ gh_number: 40, title: "Só no hub" }), priority: null, rank: null }],
      },
      { project: hubProject({ id: 2, gh_repo: "outro", name: "outro" }), work_items: [] },
    ],
    live_sessions: [],
    interrupted_sessions: [],
    failed_sessions: [],
    sync: [],
    envs: [],
    gestor_work_item_id: null,
  };
  const local = dashboard({}, [workItem({ gh_number: 40, title: "Só no hub" })]);
  local.projects.push({ project: project({ id: 9, kind: "gestor", gh_owner: "_ctower", gh_repo: "gestor" }), work_items: [] });
  const items = paletteItems(
    src({ envs: [FakeDaemonApi.wslEnv()], dashboards: { wsl: local }, hub: { up: true, dashboard: hd, connected: true } }),
  );
  expect(byKind(items, "project").map((p) => p.label)).toEqual(["ctower-agents", "outro"]);
  const i40 = items.filter((i) => i.num === 40 && i.kind === "issue");
  expect(i40).toHaveLength(1);
  expect(i40[0].action).toMatchObject({
    tab: { screen: { owner: "Deep-Ion-AI", repo: "ctower-agents", env: "wsl", projectId: 1 } },
  });
  expect(items.find((i) => i.id === "cmd:gestor")!.detail).toBe("Iniciar o gestor de projetos na tela Hub");
});

test("gestor vivo: um comando por sessão, que abre o terminal", () => {
  const items = paletteItems(
    src({ liveGestor: [{ env: "wsl", name: "WSL", session: "g1", started_at: 0, scope: { kind: "portfolio" } }] }),
  );
  const g = items.filter((i) => i.id.startsWith("cmd:gestor"));
  expect(g).toHaveLength(1);
  expect(g[0].label).toMatch(/^Abrir gestor · WSL · Portfólio · /);
  expect(g[0].action).toEqual({ type: "terminal", env: "wsl", session: "g1" });
});

test("web: sem os lugares e comandos exclusivos do desktop (#201)", () => {
  const items = paletteItems({ ...src(), web: true });
  const places = byKind(items, "place").map((i) => i.label);
  expect(places).toEqual([
    "Sala de controle",
    "Projetos",
    "Revisão",
    "Recuperação",
    "Hub",
    "Configurações",
    "Ambientes",
    "Hub › Visão geral",
    "Hub › Propostas do gestor",
    "Hub › Saúde dos projetos",
    "Hub › Quarentena",
    "Hub › Projetos e atribuições",
    "Hub › Ambientes",
    "Hub › Sessões web",
    "Hub › Dispositivos",
    "Logs",
    "Configurações › Aparência",
    "Configurações › Acesso web",
  ]);
  expect(byKind(items, "command").map((i) => i.label)).toEqual([
    "Ver interrompidas",
    "Atalhos e gestos",
    "Logs: esta UI",
    "Logs: hub",
  ]);
});

test("shell (#423): comando com a aba ativa no work item; sem ela ou na web, nada", () => {
  const target = { env: "wsl", workItemId: 32 };
  const cmd = (p: Partial<PaletteSources>) => paletteItems(src(p)).find((i) => i.id === "cmd:shell-new");
  expect(cmd({ shellTarget: target })).toMatchObject({
    label: "Shell: novo no worktree da sessão ativa",
    action: { type: "shell", env: "wsl", workItemId: 32 },
  });
  expect(cmd({ shellTarget: null })).toBeUndefined();
  expect(cmd({ shellTarget: target, web: true })).toBeUndefined();
});

test("ambientes (#23): o detalhe de cada um e, só buscando, cada seção; na web, sem as do desktop", () => {
  const envs = [{ ...FakeDaemonApi.wslEnv(), name: "WSL" }];
  const status: PaletteSources["status"] = { wsl: { env: "wsl", connected: true, since: 0, error: null } };
  const items = paletteItems(src({ envs, status }));
  const env = items.filter((i) => i.id.startsWith("env:"));
  expect(env.map((i) => [i.label, i.browse ?? true])).toEqual([
    ["Ambiente: WSL", true],
    ["Ambiente: Hub — WSL", false],
    ["Ambiente: Projetos atribuídos — WSL", false],
    ["Ambiente: Atualização — WSL", false],
    ["Ambiente: Configuração do daemon — WSL", false],
    ["Ambiente: Limpeza — WSL", false],
  ]);
  expect(env[0]).toMatchObject({ detail: "conectado", action: { type: "tab", tab: { screen: { name: "env", env: "wsl" } } } });
  expect(env[3].action).toEqual({
    type: "tab",
    tab: { screen: { name: "env", env: "wsl", section: "update" }, title: "WSL · Atualização" },
  });

  const web = paletteItems(src({ envs, status, web: true })).filter((i) => i.id.startsWith("env:"));
  expect(web.map((i) => i.label)).toEqual(["Ambiente: WSL", "Ambiente: Hub — WSL", "Ambiente: Projetos atribuídos — WSL"]);
});

test("atalhos (#27): o comando abre a folha, no desktop e na web", () => {
  for (const web of [false, true]) {
    const it = paletteItems(src({ web })).find((i) => i.id === "cmd:shortcuts");
    expect(it).toMatchObject({ kind: "command", label: "Atalhos e gestos", action: { type: "shortcuts" } });
  }
});

test("Logs (#65): a UI sempre; o hub com ele no ar; cada ambiente só buscando", () => {
  const wsl = FakeDaemonApi.wslEnv("wsl");
  const items = paletteItems(src({ envs: [wsl], hub: { ...HUB_OFF, connected: true } }));
  const logs = items.filter((i) => i.id.startsWith("cmd:logs:"));
  expect(logs.map((i) => [i.label, i.browse])).toEqual([
    ["Logs: esta UI", undefined],
    ["Logs: hub", false],
    ["Logs: ctower-agents (WSL)", false],
  ]);
  expect(logs[2].action).toEqual({
    type: "tab",
    tab: { id: "maintenance:logs", screen: { name: "maintenance", section: "logs", source: "env", env: "wsl" } },
  });
  expect(paletteItems(src()).some((i) => i.id === "cmd:logs:hub")).toBe(false);
});

test("sync por projeto (#90): este projeto, 'Sincronizar projeto…' e um por projeto; na web, desabilitados", () => {
  const sources = (web: boolean) =>
    src({
      web,
      envs: [FakeDaemonApi.wslEnv()],
      dashboards: { wsl: dashboard() },
      activeProject: { screen: { name: "project", env: "wsl", projectId: 1 }, name: "ctower-agents" },
    });
  const sync = (items: PaletteItem[]) =>
    items.filter((i) => i.action.type === "sync-project" || i.id === "cmd:sync-project");
  const desk = sync(paletteItems(sources(false)));
  expect(desk.map((i) => [i.label, i.browse ?? true, i.disabled ?? false])).toEqual([
    ["Sincronizar este projeto", true, false],
    ["Sincronizar projeto…", true, false],
    // Por projeto, só buscando (a lista inicial não cresce com os projetos).
    [`${SYNC_PROJECT_PREFIX} ctower-agents`, false, false],
  ]);
  expect(desk[0].action).toEqual({
    type: "sync-project",
    screen: { name: "project", env: "wsl", projectId: 1 },
    open: false,
  });
  // "Sincronizar projeto…" troca a busca para a lista dos projetos.
  expect(desk[1].action).toEqual({ type: "query", query: `${SYNC_PROJECT_PREFIX} ` });
  expect(desk[2].action).toMatchObject({ type: "sync-project", open: true });

  const web = sync(paletteItems(sources(true)));
  expect(web.map((i) => [i.label, i.disabled, i.detail])).toEqual([
    ["Sincronizar este projeto", true, SYNC_WEB_NOTE],
    ["Sincronizar projeto…", true, SYNC_WEB_NOTE],
    [`${SYNC_PROJECT_PREFIX} ctower-agents`, true, SYNC_WEB_NOTE],
  ]);

  // Sem aba de projeto ativa: sem "Sincronizar este projeto".
  const sem = paletteItems(src({ envs: [FakeDaemonApi.wslEnv()], dashboards: { wsl: dashboard() } }));
  expect(sem.find((i) => i.id === "cmd:sync-this")).toBeUndefined();
});
