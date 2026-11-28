// Cards por projeto (#358): regras puras do card, com dados fictícios.
import type { ControlRoomEpicRun, ControlRoomPr, ControlRoomProject, ProjectRelease } from "../protocol/hub-gen";
import type { ProjectRow } from "../project/rows";
import { dashboard, hubProject, MIN, project, session, sync, workItem } from "../test/fixtures";
import type { AttentionItem, HubRoom } from "./attention";
import {
  decisionsOf,
  epicStep,
  filterCards,
  flowOf,
  prsOf,
  projectCards,
  releaseOf,
  syncLine,
  type CardsInput,
} from "./cards";

const NOW = Date.UTC(2026, 10, 27, 15, 0, 0);
const DAY = 86_400_000;

function pr(p: Partial<ControlRoomPr> = {}, checks: Partial<ControlRoomPr["checks"]> = {}): ControlRoomPr {
  return {
    number: 40,
    title: "Exportar relatório",
    issue: 12,
    issue_ref: "acme/acme-web#12",
    url: "https://github.com/acme/acme-web/pull/40",
    head: "feat/12-exportar",
    base: "develop",
    updated_at: NOW - MIN,
    checks: {
      state: "success",
      total: 3,
      failed: 0,
      pending: 0,
      text: "3 checks ok",
      failed_names: [],
      head_sha: "abc",
      synced_at: NOW - MIN,
      ...checks,
    },
    ...p,
  };
}

function run(r: Partial<ControlRoomEpicRun> = {}): ControlRoomEpicRun {
  return {
    run_id: 1,
    epic: 30,
    epic_ref: "acme/acme-web#30",
    title: "Relatórios v2",
    status: "executando",
    env_id: "env-1",
    env_online: true,
    plan_version: 1,
    done: 2,
    total: 6,
    current: [{ number: 31, item_ref: "acme/acme-web#31", title: null, state: "em_execucao", pr: null }],
    next: { number: 33, item_ref: "acme/acme-web#33", title: null, state: "pronta", pr: null },
    pending_decisions: 0,
    stop: null,
    updated_at: NOW,
    ...r,
  };
}

function release(r: Partial<ProjectRelease> = {}): ProjectRelease {
  return {
    project_id: 1,
    owner: "acme",
    repo: "acme-web",
    supported: true,
    synced: true,
    release: { tag: "v1.4.2", name: "v1.4.2", published_at: NOW - DAY, url: "https://github.com/acme/acme-web/releases/v1.4.2" },
    unreleased: 2,
    deliveries: [
      { number: 41, title: "Filtro por data", merged_at: NOW - MIN },
      { number: 39, title: "Ajuste no menu", merged_at: NOW - 2 * MIN },
    ],
    truncated: false,
    synced_at: NOW - MIN,
    ...r,
  };
}

const acme = project({ id: 1, name: "acme-web", gh_owner: "acme", gh_repo: "acme-web" });
const other = project({ id: 2, name: "acme-api", gh_owner: "acme", gh_repo: "acme-api" });

function row(p = acme, env = "env-1"): ProjectRow {
  return {
    key: `${env}:${p.id}`,
    name: p.name,
    running: 0,
    waiting: 0,
    project: p,
    start: null,
    screen: { name: "project", env, projectId: p.id, owner: p.gh_owner, repo: p.gh_repo },
    env: { id: env, name: "servidor" },
    view: null,
    sync: sync({ project_id: p.id, last_ok_at: NOW - MIN, last_attempt_at: NOW - MIN }),
    syncAddr: { env, projectId: p.id },
  };
}

const item = (repo: string, env = "env-1"): AttentionItem => ({
  key: `k:${repo}:${Math.random()}`,
  kind: "waiting",
  env,
  label: "Aguardando você",
  title: "x",
  repo,
  detail: "",
  since: NOW,
  actionLabel: "Responder",
  action: { to: "settings" },
});

function input(i: Partial<CardsInput> = {}): CardsInput {
  return { rows: [row()], items: [], dashboards: {}, hubDashboard: null, room: undefined, releases: null, now: NOW, ...i };
}

test("fluxo: só issues; backlog, em andamento, em revisão e feitas em 7 dias", () => {
  const f = flowOf(
    [
      workItem({ gh_number: 1, status: "backlog" }),
      workItem({ gh_number: 2, status: "em_execucao" }),
      workItem({ gh_number: 3, status: "aguardando_voce" }),
      workItem({ gh_number: 4, status: "em_revisao" }),
      workItem({ gh_number: 5, status: "aguardando_epico" }),
      workItem({ gh_number: 6, status: "concluido", closed_at: NOW - DAY }),
      workItem({ gh_number: 7, status: "concluido", closed_at: NOW - 8 * DAY }),
      workItem({ gh_number: 8, status: "backlog", kind: "epic" }),
      workItem({ gh_number: 0, status: "em_execucao" }),
    ],
    NOW,
  );
  expect(f).toEqual({ backlog: 1, doing: 2, review: 2, done: 1 });
});

test("decisões: o épico não soma de novo as das sub-issues", () => {
  expect(
    decisionsOf([
      workItem({ gh_number: 30, kind: "epic", pending_decisions: 4 }),
      workItem({ gh_number: 31, parent_number: 30, pending_decisions: 3 }),
      workItem({ gh_number: 40, pending_decisions: 2 }),
    ]),
  ).toBe(6);
});

test("PRs: nenhum, checks ok, rodando, vermelho com o nome e MRs no GitLab", () => {
  expect(prsOf(undefined, false)).toMatchObject({ label: "PRs abertos", text: "Nenhum", count: 0 });
  const room = (prs: ControlRoomPr[], forge: "github" | "gitlab" = "github"): ControlRoomProject => ({
    project_id: 1,
    owner: "acme",
    repo: "acme-web",
    name: "acme-web",
    tracker: "github",
    forge,
    prs,
    epic_runs: [],
  });
  expect(prsOf(room([pr(), pr({ number: 41 })]), false)).toMatchObject({ text: "2 · checks ok", tone: "neutral" });
  expect(prsOf(room([pr({}, { state: "pending" })]), false)).toMatchObject({ text: "1 · checks rodando", tone: "running" });
  const red = prsOf(room([pr(), pr({ number: 41 }, { state: "failure", failed_names: ["e2e"], text: "3 checks, 1 falhou" })]), false);
  expect(red).toMatchObject({ text: "2 · e2e vermelho", tone: "error" });
  expect(red.lines).toEqual(["#40 Exportar relatório — 3 checks ok", "#41 Exportar relatório — 3 checks, 1 falhou"]);
  expect(prsOf(room([pr({}, { state: "none", text: null })], "gitlab"), true)).toMatchObject({
    label: "MRs abertos",
    text: "1",
    lines: ["!40 Exportar relatório"],
  });
});

test("release: tag e entregas sem release; sem release; sem leitura; GitLab e não suportado com —", () => {
  expect(releaseOf(release(), "develop", false)).toMatchObject({
    text: "v1.4.2",
    note: "2 entregas na develop sem release",
    warn: true,
    lines: ["#41 Filtro por data", "#39 Ajuste no menu"],
  });
  expect(releaseOf(release({ unreleased: 1, deliveries: [] }), "main", false).note).toBe("1 entrega na main sem release");
  expect(releaseOf(release({ unreleased: 0, deliveries: [] }), "develop", false)).toMatchObject({
    note: "Nada na develop sem release",
    warn: false,
  });
  expect(releaseOf(release({ release: null, unreleased: 250, truncated: true }), "develop", false)).toMatchObject({
    text: "Sem release",
    note: "250+ entregas na develop sem release",
  });
  expect(releaseOf(release({ synced: false, release: null }), "develop", false)).toMatchObject({ text: "—", note: "Ainda sem leitura" });
  expect(releaseOf(release(), "develop", true)).toEqual({ text: "—", note: null, warn: false, lines: [] });
  expect(releaseOf(release({ supported: false }), "develop", false).text).toBe("—");
  expect(releaseOf(undefined, "develop", false).text).toBe("—");
});

test("épico: andamento e próxima, ou o motivo de estar parado", () => {
  expect(epicStep(run())).toBe("Em andamento: #31 · próxima #33");
  expect(epicStep(run({ current: [] }))).toBe("Próxima: #33");
  expect(epicStep(run({ current: [], next: null, status: "integrando" }))).toBe("Integrando as sub-issues");
  const stop = (kind: NonNullable<ControlRoomEpicRun["stop"]>["kind"], s: Partial<NonNullable<ControlRoomEpicRun["stop"]>> = {}) =>
    epicStep(run({ stop: { kind, reason: null, number: null, until: null, since: null, ...s } }));
  expect(stop("pausado", { reason: "revisar o plano" })).toBe("Pausada: revisar o plano");
  expect(stop("estacionado")).toBe("Estacionada");
  expect(stop("aguardando_decisao", { number: 32 })).toBe("Parada em #32: aguardando sua decisão");
  expect(stop("aguardando_aprovacao")).toBe("Plano aguardando sua aprovação");
  expect(stop("falha", { reason: "worktree ocupado" })).toBe("Falha do motor: worktree ocupado");
  expect(stop("ambiente_fora")).toBe("Parada: o ambiente da execução está fora do ar");
});

test("sync: sincronizado, atrasado, com erro e falhando", () => {
  expect(syncLine(sync({ last_ok_at: NOW - MIN }), acme, NOW)).toEqual({ text: "GitHub · sincronizado há 1 min", warn: false });
  expect(syncLine(sync({ last_ok_at: NOW - 22 * MIN }), acme, NOW)).toEqual({
    text: "GitHub · sync atrasado (último há 22 min)",
    warn: true,
  });
  expect(syncLine(sync({ last_error: "rede", last_attempt_at: NOW - 2 * MIN }), acme, NOW).warn).toBe(true);
  expect(syncLine(sync({ failures: 3 }), acme, NOW).text).toBe("GitHub · sync falhando há 3 ciclos");
  expect(syncLine(null, acme, NOW)).toEqual({ text: "GitHub · ainda não sincronizado", warn: false });
});

test("cards: ordem pelo que pede você, badge, sessões e filtros; sem hub, sem PRs, release e épicos", () => {
  const d1 = dashboard(
    {
      projects: [
        {
          project: acme,
          work_items: [
            workItem({ id: 31, gh_number: 31, status: "aguardando_voce", title: "Exportar relatório" }),
            workItem({ id: 32, gh_number: 32, status: "em_execucao", title: "Filtro por data" }),
            workItem({ id: 33, gh_number: 33, status: "em_revisao", title: "Menu", pending_decisions: 2 }),
          ],
        },
        { project: other, work_items: [workItem({ id: 50, project_id: 2, gh_number: 50, status: "backlog" })] },
      ],
      live_sessions: [
        session({ id: "s-31", work_item_id: 31 }),
        session({ id: "s-32", work_item_id: 32 }),
      ],
      failed_sessions: [session({ id: "s-33", work_item_id: 33, state: "exited", exit_code: 1, ended_at: NOW - 10 * MIN })],
      interrupted_sessions: [],
    },
  );
  const cards = projectCards(
    input({
      rows: [row(other), row(acme)],
      dashboards: { "env-1": d1 },
      items: [item("acme/acme-web"), item("ACME/acme-web"), item("acme/acme-web", "env-2")],
    }),
  );
  // acme-web pede você (2 itens deste ambiente): sobe antes da ordem das linhas.
  expect(cards.map((c) => c.name)).toEqual(["acme-web", "acme-api"]);
  const [web, api] = cards;
  expect(web.badge).toEqual({ text: "2 pedem você", tone: "attention" });
  expect(web.sessions.map((s) => [s.ref, s.title, s.state])).toEqual([
    ["#31", "Exportar relatório", "waiting"],
    ["#33", "Menu", "failed"],
    ["#32", "Filtro por data", "running"],
  ]);
  expect(web.sessions[0].terminal).toEqual({ env: "env-1", session: "s-31" });
  expect(web.sessions[1].terminal).toBeNull();
  expect(web.flow).toEqual({ backlog: 0, doing: 2, review: 1, done: 0 });
  expect(web.decisions).toBe(2);
  expect(web.prs).toBeUndefined();
  expect(web.release).toBeUndefined();
  expect(web.epics).toBeUndefined();
  expect(api.badge).toEqual({ text: "Parado", tone: "idle" });
  expect(api.idleText).toBe("Nenhuma sessão rodando");
  expect(api.flow.backlog).toBe(1);

  expect(filterCards(cards, "attention").map((c) => c.name)).toEqual(["acme-web"]);
  expect(filterCards(cards, "running").map((c) => c.name)).toEqual(["acme-web"]);
  expect(filterCards(cards, "all")).toHaveLength(2);
});

test("cards: sem sessão há N dias, pela sessão encerrada mais recente", () => {
  const d = dashboard({
    projects: [{ project: acme, work_items: [workItem({ id: 31, gh_number: 31, status: "backlog" })] }],
    interrupted_sessions: [session({ id: "s", work_item_id: 31, state: "interrupted", last_event_at: NOW - 6 * DAY })],
  });
  const [c] = projectCards(input({ dashboards: { "env-1": d } }));
  expect(c.idleText).toBe("Sem sessão há 6 dias");
});

test("cards com hub: PRs, release e épicos; autônomo; GitLab com MRs e —", () => {
  const hp = hubProject({ id: 1, name: "acme-web", gh_owner: "acme", gh_repo: "acme-web", autonomy: { enabled: true, env_id: "env-1", branch: "", base_branch: "", strategy: "merge" } });
  const gl = hubProject({
    id: 2,
    name: "acme-mobile",
    gh_owner: "grupo",
    gh_repo: "acme-mobile",
    forge: { kind: "gitlab", path: "grupo/acme-mobile", clone_url: "https://gitlab.example.com/grupo/acme-mobile.git", connection: 1 },
  });
  const hubRow = (p: typeof hp): ProjectRow => ({
    ...row(),
    key: `hub:${p.id}`,
    name: p.name,
    project: p,
    env: null,
    sync: null,
    view: { project: p, assignments: [], locations: [], sync: sync({ last_ok_at: NOW - 2 * MIN }), tracker_status: null, wip: null },
    screen: { name: "project", owner: p.gh_owner, repo: p.gh_repo },
  });
  const room: HubRoom = {
    projects: [
      { project_id: 1, owner: "acme", repo: "acme-web", name: "acme-web", tracker: "github", forge: "github", prs: [pr()], epic_runs: [run()] },
      { project_id: 2, owner: "grupo", repo: "acme-mobile", name: "acme-mobile", tracker: "github", forge: "gitlab", prs: [pr({}, { state: "none", text: null })], epic_runs: [] },
    ],
    pilotSessions: [],
    blocked: [],
    blockedAt: 0,
  };
  const cards = projectCards(
    input({
      rows: [hubRow(hp), hubRow(gl)],
      hubDashboard: {
        projects: [
          { project: hp, work_items: [] },
          { project: gl, work_items: [] },
        ],
        live_sessions: [],
        interrupted_sessions: [],
        failed_sessions: [],
        sync: [],
        envs: [],
        gestor_work_item_id: null,
      },
      room,
      releases: [release(), release({ project_id: 2, owner: "grupo", repo: "acme-mobile", supported: false, synced: false, release: null })],
    }),
  );
  const [web, mobile] = cards;
  expect(web.autonomous).toBe(true);
  expect(web.prs).toMatchObject({ label: "PRs abertos", text: "1 · checks ok" });
  expect(web.release).toMatchObject({ text: "v1.4.2", note: "2 entregas na develop sem release" });
  expect(web.epics).toEqual([
    expect.objectContaining({ ref: "#30", title: "Relatórios v2", progress: "2 de 6 sub-issues", step: "Em andamento: #31 · próxima #33" }),
  ]);
  // Épico andando, sem sessão no card: tudo andando.
  expect(web.badge.text).toBe("Tudo andando");
  expect(web.sync).toEqual({ text: "GitHub · sincronizado há 2 min", warn: false });
  expect(mobile.prs).toMatchObject({ label: "MRs abertos", text: "1" });
  expect(mobile.release).toMatchObject({ text: "—", note: null });
  expect(mobile.epics).toEqual([]);
});
