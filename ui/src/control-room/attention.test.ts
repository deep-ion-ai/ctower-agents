import { attention, SYNC_INTERVAL_MS, type AttentionInput, type HubRoom } from "./attention";
import type { ControlRoomPr, ControlRoomProject } from "../protocol/hub-gen";
import { FakeDaemonApi } from "../api/fake";
import { dashboard, envInfo, MIN, NOW, project, session, sync, workItem } from "../test/fixtures";
import { DOCS_HELP_URL } from "../project/docs";
import { warnings } from "../shell/summary";

const env = FakeDaemonApi.wslEnv();
const ok = { wsl: { env: "wsl", connected: true, error: null, since: 0 } };

function input(p: Partial<AttentionInput> = {}): AttentionInput {
  return { envs: [env], status: ok, dashboards: { wsl: dashboard() }, alerts: [], now: NOW, ...p };
}

test("tudo andando: faixa vazia", () => {
  const d = dashboard({ live_sessions: [session()] }, [workItem({ status: "em_execucao" })]);
  expect(attention(input({ dashboards: { wsl: d } }))).toEqual([]);
  // Sem status ainda (conectando): nada a mostrar.
  expect(attention(input({ status: {} }))).toEqual([]);
  // Dashboard ainda não carregado.
  expect(attention(input({ dashboards: {} }))).toEqual([]);
});

test("sub-issue aguardando o épico não precisa de você", () => {
  const d = dashboard({ live_sessions: [session({ state: "idle" })] }, [workItem({ status: "aguardando_epico" })]);
  expect(attention(input({ dashboards: { wsl: d } }))).toEqual([]);
});

test.each([
  {
    caso: "sessão viva com work item aguardando você",
    p: {
      dashboards: {
        wsl: dashboard({ live_sessions: [session({ state: "idle" })] }, [
          workItem({ status: "aguardando_voce", title: "ui: Sala de controle" }),
        ]),
      },
    },
    kind: "waiting",
    title: "ctower-agents #31 ui: Sala de controle",
    where: { project: "ctower-agents", item: "#31 ui: Sala de controle" },
    action: { to: "terminal", env: "wsl", session: session().id },
  },
  {
    caso: "sessão interrompida",
    p: { dashboards: { wsl: dashboard({ interrupted_sessions: [session({ state: "interrupted" })] }, [workItem()]) } },
    kind: "interrupted",
    title: "ctower-agents #31 Item 31",
    where: { project: "ctower-agents", item: "#31 Item 31" },
    action: { to: "recovery", env: "wsl", session: session().id },
  },
  {
    caso: "saiu com erro nas últimas 24 h",
    p: {
      dashboards: {
        wsl: dashboard(
          { failed_sessions: [session({ state: "exited", exit_code: 1, ended_at: NOW - 25 * MIN })] },
          [workItem()],
        ),
      },
    },
    kind: "failed",
    title: "ctower-agents #31 Item 31",
    where: { project: "ctower-agents", item: "#31 Item 31" },
    action: { to: "project", env: "wsl", projectId: 1 },
  },
  {
    caso: "ambiente desconectado",
    p: { status: { wsl: { env: "wsl", connected: false, error: "daemon inacessível", since: NOW - 3 * MIN } } },
    kind: "env_down",
    title: "ctower-agents (WSL) desconectado",
    where: { project: undefined, item: undefined },
    action: { to: "settings" },
  },
  {
    caso: "sync atrasado",
    p: { dashboards: { wsl: dashboard({ sync: [sync({ last_ok_at: NOW - 4 * SYNC_INTERVAL_MS })] }) } },
    kind: "sync_late",
    title: "GitHub de ctower-agents sem sincronizar",
    where: { project: "ctower-agents", item: "GitHub sem sincronizar" },
    action: { to: "project", env: "wsl", projectId: 1 },
  },
  {
    caso: "sync que nunca funcionou",
    p: { dashboards: { wsl: dashboard({ sync: [sync({ last_ok_at: null, last_error: "401" })] }) } },
    kind: "sync_late",
    title: "GitHub de ctower-agents sem sincronizar",
    where: { project: "ctower-agents", item: "GitHub sem sincronizar" },
    action: { to: "project", env: "wsl", projectId: 1 },
  },
  {
    caso: "alerta de transcript volátil",
    p: {
      dashboards: { wsl: dashboard({ live_sessions: [session()] }, [workItem()]) },
      alerts: [{ env: "wsl", kind: "transcript_volatile", session_id: session().id, detail: "/tmp/x", at: NOW }],
    },
    kind: "alert",
    title: "Transcript em local volátil: pode se perder se o ambiente cair",
    where: { project: "ctower-agents", item: undefined },
    action: { to: "terminal", env: "wsl", session: session().id },
  },
  {
    caso: "sessão com secrets de uma configuração velha",
    p: {
      dashboards: { wsl: dashboard({ live_sessions: [session()] }, [workItem()]) },
      alerts: [{ env: "wsl", kind: "secrets_stale", session_id: session().id, detail: "há 48h", at: NOW }],
    },
    kind: "alert",
    title: "Sessão iniciada com secrets de uma configuração desatualizada",
    action: { to: "terminal", env: "wsl", session: session().id },
  },
  {
    caso: "sessão iniciada com docs_root inacessível",
    p: {
      dashboards: { wsl: dashboard({ live_sessions: [session()] }, [workItem()]) },
      alerts: [{ env: "wsl", kind: "docs_unreachable", session_id: session().id, detail: "/mnt/x", at: NOW }],
    },
    kind: "alert",
    title: "Sessão iniciada sem a pasta de docs: docs_root inacessível",
    action: { to: "terminal", env: "wsl", session: session().id },
  },
  {
    caso: "sessão de issue iniciada sem o épico resolvido (#143)",
    p: {
      dashboards: { wsl: dashboard({ live_sessions: [session()] }, [workItem()]) },
      alerts: [
        { env: "wsl", kind: "epic_unresolved", session_id: session().id, detail: "issue fora do cache", at: NOW },
      ],
    },
    kind: "alert",
    title: "Sessão iniciada sem saber o épico da issue: as regras do épico podem estar faltando",
    action: { to: "terminal", env: "wsl", session: session().id },
  },
])("$caso", ({ p, kind, title, action, ...rest }) => {
  const items = attention(input(p as Partial<AttentionInput>));
  expect(items).toHaveLength(1);
  expect(items[0]).toMatchObject({ kind, title, action });
  expect(items[0].label).not.toBe("");
  // Lista compacta (#357): o projeto numa coluna e o item sem ele.
  const where = (rest as { where?: { project?: string; item?: string } }).where;
  if (where) {
    expect(items[0].project).toBe(where.project);
    expect(items[0].item).toBe(where.item);
  }
});

test("lista compacta (#357): gestor, sessão do projeto e sync falhando têm projeto e item à parte", () => {
  const d = dashboard(
    {
      live_sessions: [
        session({ id: "g", work_item_id: 900, state: "idle" }),
        session({ id: "p", work_item_id: 40, state: "idle" }),
      ],
      gestor_work_item_id: 900,
    },
    [workItem({ id: 40, gh_number: 0, status: "aguardando_voce" })],
  );
  const perms = {
    wsl: [
      { session_id: "g", tool_name: "Bash", tool_input: { command: "ls" }, created_at: NOW } as never,
      { session_id: "p", tool_name: "Bash", tool_input: { command: "ls" }, created_at: NOW } as never,
    ],
  };
  const items = attention(input({ dashboards: { wsl: d }, permissions: perms }));
  const gestor = items.find((i) => i.key.endsWith(":g"))!;
  expect(gestor.project).toBeUndefined();
  expect(gestor.item).toBe("Gestor de projetos");
  const projeto = items.find((i) => i.key.endsWith(":p"))!;
  expect(projeto).toMatchObject({ project: "ctower-agents", item: "Sessão do projeto" });

  const falhando = attention(
    input({ dashboards: { wsl: dashboard({ sync: [sync({ last_ok_at: NOW, failures: 3, last_error: "HTTP 410" })] }) } }),
  );
  expect(falhando[0]).toMatchObject({ kind: "sync_failing", project: "ctower-agents", item: "Sync falhando há 3 ciclos" });
});

test("interrompidas em ambientes diferentes: cada ação leva o ambiente e a sessão dela (#160)", () => {
  const vps = { ...FakeDaemonApi.wslEnv("vps"), name: "Servidor" };
  const items = attention(
    input({
      envs: [env, vps],
      status: { ...ok, vps: { env: "vps", connected: true, error: null, since: 0 } },
      dashboards: {
        wsl: dashboard({ interrupted_sessions: [session({ id: "s-a", state: "interrupted" })] }, [workItem()]),
        vps: dashboard({ interrupted_sessions: [session({ id: "s-b", state: "interrupted" })] }, [workItem()]),
      },
    }),
  );
  expect(items.map((i) => i.action)).toEqual([
    { to: "recovery", env: "wsl", session: "s-a" },
    { to: "recovery", env: "vps", session: "s-b" },
  ]);
});

test("ignora o que não pede o humano", () => {
  const d = dashboard(
    {
      failed_sessions: [session({ state: "exited", exit_code: 1, ended_at: NOW - 25 * 3_600_000 })],
      sync: [sync({ last_ok_at: NOW - 2 * SYNC_INTERVAL_MS })],
    },
    [workItem()],
  );
  expect(attention(input({ dashboards: { wsl: d } }))).toEqual([]);
});

test("sync atrasado de projeto fora da lista não avisa", () => {
  const d = dashboard({ sync: [sync({ project_id: 99, last_ok_at: null, last_error: "timeout" })] }, [workItem()]);
  expect(attention(input({ dashboards: { wsl: d } }))).toEqual([]);
});

test("ordem: aguardando primeiro, mais antigo antes", () => {
  const a = session({ id: "a", work_item_id: 31, last_event_at: NOW - 5 * MIN });
  const b = session({ id: "b", work_item_id: 32, last_event_at: NOW - 30 * MIN });
  const d = dashboard(
    { live_sessions: [a, b], interrupted_sessions: [session({ id: "c", state: "interrupted", last_event_at: NOW - 99 * MIN })] },
    [workItem({ status: "aguardando_voce" }), workItem({ gh_number: 32, status: "aguardando_voce" })],
  );
  const keys = attention(input({ dashboards: { wsl: d } })).map((i) => i.key);
  expect(keys).toEqual(["wait:wsl:b", "wait:wsl:a", "int:wsl:c"]);
});

test("hub: fora do ar e avisos de hub.status, depois dos ambientes caídos", () => {
  const items = attention(
    input({
      status: { wsl: { env: "wsl", connected: false, error: "x", since: NOW - MIN } },
      hub: {
        status: { env: "hub", connected: false, error: "hub inacessível", since: NOW - 5 * MIN },
        warnings: [{ kind: "backup_stale", message: "último snapshot há 14 h" }],
      },
    }),
  );
  expect(items.map((i) => [i.kind, i.title])).toEqual([
    ["env_down", "ctower-agents (WSL) desconectado"],
    ["hub", "Hub indisponível"],
    ["hub", "Snapshot do hub atrasado"],
  ]);
  expect(items[1]).toMatchObject({ actionLabel: "Ver hub", action: { to: "hub" }, since: NOW - 5 * MIN });
  expect(items[2].detail).toBe("último snapshot há 14 h");
  // Aviso de backup leva a Manutenção › Backup e snapshots (#338).
  expect(items[2]).toMatchObject({ actionLabel: "Configurar backup", action: { to: "maintenance", section: "backup" } });
  // Hub conectado e sem avisos: nada.
  expect(
    attention(input({ hub: { status: { env: "hub", connected: true, error: null, since: 0 }, warnings: [] } })),
  ).toEqual([]);
});

test("hub: avisos dos snapshots de secrets têm título próprio", () => {
  const items = attention(
    input({
      hub: {
        status: { env: "hub", connected: true, error: null, since: 0 },
        warnings: [
          { kind: "secrets_snapshot_unreachable", message: "destino inacessível: /mnt/c/x" },
          { kind: "secrets_snapshot_stale", message: "não corresponde ao store" },
          { kind: "secrets_snapshot_same_device", message: "mesmo disco" },
          { kind: "secrets_rotate_pending", message: "2 secret(s) a rotacionar na origem" },
        ],
      },
    }),
  );
  expect(items.map((i) => i.title)).toEqual([
    "Destino dos snapshots de secrets inacessível",
    "Snapshot dos secrets desatualizado",
    "Snapshots de secrets no mesmo disco do hub",
    "Secrets a rotacionar na origem",
  ]);
  expect(items[0].detail).toBe("destino inacessível: /mnt/c/x");
});

test("pedido de permissão segurado vira item com a ferramenta, mesmo antes do status do work item mudar (#365)", () => {
  const s = session();
  const d = dashboard({ live_sessions: [s] }, [workItem({ status: "em_execucao" })]);
  const req = {
    request_id: "r",
    session_id: s.id,
    work_item_id: 31,
    tool_name: "Bash",
    tool_input: { command: "rm -rf target" },
    created_at: NOW - MIN,
    expires_at: NOW + MIN,
  };
  const [item] = attention(input({ dashboards: { wsl: d }, permissions: { wsl: [req] } }));
  expect(item).toMatchObject({
    kind: "waiting",
    label: "Permissão",
    detail: "Bash: rm -rf target",
    since: NOW - MIN,
    action: { to: "terminal", env: "wsl", session: s.id },
  });
});

test("docs_root inacessível no ambiente: item com o link da documentação, também na aba Avisos (#444)", () => {
  const down = { wsl: envInfo({ docs: { root: "/mnt/ct-docs", reachable: false, warning: "docs_unreachable" } }) };
  const items = attention(input({ infos: down, status: { wsl: { env: "wsl", connected: true, error: null, since: NOW - MIN } } }));
  expect(items).toHaveLength(1);
  expect(items[0]).toMatchObject({
    key: "docs:wsl",
    kind: "docs",
    title: "ctower-agents (WSL): pasta de docs inacessível",
    actionLabel: "Ver documentação",
    action: { to: "url", url: DOCS_HELP_URL },
    since: NOW - MIN,
  });
  expect(items[0].detail).toContain("/mnt/ct-docs");
  // Acessível, sem docs_root ou ambiente desconectado: nada de docs.
  expect(attention(input({ infos: { wsl: envInfo({ docs: { root: "/mnt/ct-docs", reachable: true } }) } }))).toEqual([]);
  expect(attention(input({ infos: { wsl: envInfo() } }))).toEqual([]);
  const off = attention(input({ infos: down, status: { wsl: { env: "wsl", connected: false, error: "x", since: 0 } } }));
  expect(off.map((i) => i.kind)).toEqual(["env_down"]);
  // Aba Avisos do painel inferior.
  const t = {
    envs: [env],
    status: ok,
    dashboards: { wsl: dashboard() },
    alerts: [],
    infos: down,
    hub: { conn: null, status: null, health: null, update: null } as never,
  };
  expect(warnings(t, NOW).map((i) => i.kind)).toEqual(["docs"]);
});

test("avisos do backup dos docs levam a Manutenção › Backup (na web, ao hub)", () => {
  const hub = {
    status: { env: "hub", connected: true, error: null, since: 0 },
    warnings: [
      { kind: "docs_backup_failed", message: "pacote falhou" },
      { kind: "docs_backup_stale", message: "nenhum pacote há 50 h" },
      { kind: "docs_backup_too_large", message: "2 GB" },
    ],
  };
  const items = attention(input({ hub }));
  expect(items.map((i) => i.title)).toEqual([
    "Backup dos docs falhou",
    "Backup dos docs atrasado",
    "Docs acima do limite do backup",
  ]);
  for (const i of items) expect(i.action).toEqual({ to: "maintenance", section: "backup" });
  for (const i of attention(input({ hub, web: true }))) expect(i.action).toEqual({ to: "hub" });
});

// #90: sync de um projeto falhando seguidas vezes.
describe("sync falhando (#90)", () => {
  const failing = (failures: number) =>
    dashboard({
      sync: [
        sync({
          last_ok_at: NOW - 60 * MIN,
          last_attempt_at: NOW - MIN,
          last_error: "GitHub: HTTP 410",
          failures,
          failing_since: NOW - 30 * MIN,
        }),
      ],
    });

  test("a partir de 3 falhas seguidas: aviso com o projeto, os ciclos, o erro e o link", () => {
    const items = attention(input({ dashboards: { wsl: failing(3) } }));
    expect(items).toEqual([
      expect.objectContaining({
        kind: "sync_failing",
        label: "Sync falhando",
        title: "ctower-agents: sync falhando há 3 ciclos",
        detail: "GitHub: HTTP 410",
        since: NOW - 30 * MIN,
        actionLabel: "Ver projeto",
        action: { to: "project", env: "wsl", projectId: 1 },
      }),
    ]);
    // Também na aba Avisos.
    const tower = { envs: [env], status: ok, dashboards: { wsl: failing(3) }, alerts: [], hub: { conn: null } };
    expect(warnings(tower as unknown as Parameters<typeof warnings>[0], NOW).map((w) => w.kind)).toEqual(["sync_failing"]);
  });

  test("abaixo do limiar não avisa (queda de rede passageira)", () => {
    const d = dashboard({ sync: [sync({ last_ok_at: NOW - 5 * MIN, last_error: "rede", failures: 2 })] });
    expect(attention(input({ dashboards: { wsl: d } }))).toEqual([]);
    // Duas falhas com o último sucesso antigo: continua o "Sync atrasado" de antes.
    expect(attention(input({ dashboards: { wsl: failing(2) } })).map((i) => i.kind)).toEqual(["sync_late"]);
  });

  test("some na primeira rodada bem-sucedida", () => {
    const ok1 = dashboard({ sync: [sync({ last_ok_at: NOW - MIN, failures: 0 })] });
    expect(attention(input({ dashboards: { wsl: ok1 } }))).toEqual([]);
  });
});

// #360: check vermelho e bloqueada pelo piloto (dados fictícios).
describe("hub: check vermelho e bloqueada pelo piloto (#360)", () => {
  const hub = { status: { env: "hub", connected: true, error: null, since: 0 }, warnings: [] };
  const pr = (over: Partial<ControlRoomPr["checks"]> = {}, issue = 31): ControlRoomPr => ({
    number: 7,
    title: "Item 31",
    issue,
    issue_ref: `acme/acme-web#${issue}`,
    url: "https://git.example.com/acme/acme-web/pull/7",
    head: "feat/x",
    base: "develop",
    updated_at: NOW - 10 * MIN,
    checks: {
      state: "failure",
      total: 3,
      failed: 1,
      pending: 0,
      text: "3 checks, 1 falhou",
      failed_names: ["build"],
      head_sha: "abc",
      synced_at: NOW - 5 * MIN,
      ...over,
    },
  });
  const proj = (prs: ControlRoomPr[]): ControlRoomProject => ({
    project_id: 1,
    owner: "acme",
    repo: "acme-web",
    name: "acme-web",
    tracker: "github",
    forge: "github",
    prs,
    epic_runs: [],
  });
  const room = (over: Partial<HubRoom> = {}): HubRoom => ({
    projects: [proj([pr()])],
    pilotSessions: [],
    blocked: [],
    blockedAt: NOW - 60 * MIN,
    ...over,
  });
  const run = (r: HubRoom | undefined, extra: Partial<AttentionInput> = {}) =>
    attention(input({ hub, room: r, ...extra }));

  test("PR com check falho aparece uma vez, com o nome do check e a ação de abrir", () => {
    const items = run(room());
    expect(items).toEqual([
      expect.objectContaining({
        kind: "ci_failed",
        label: "Check vermelho",
        project: "acme-web",
        detail: "build falhou no último push",
        since: NOW - 5 * MIN,
        actionLabel: "Abrir o check",
        action: { to: "url", url: "https://git.example.com/acme/acme-web/pull/7" },
      }),
    ]);
  });

  test("some quando o check fica verde ou o PR fecha", () => {
    expect(run(room({ projects: [proj([pr({ state: "success", failed: 0, failed_names: [] })])] }))).toEqual([]);
    expect(run(room({ projects: [proj([pr({ state: "pending" })])] }))).toEqual([]);
    expect(run(room({ projects: [proj([])] }))).toEqual([]);
  });

  test("PR que o piloto ainda corrige fica de fora", () => {
    const piloting = room({ pilotSessions: [{ owner: "acme", repo: "acme-web", gh_number: 31 }] });
    expect(run(piloting)).toEqual([]);
    // Sessão do piloto em outra issue não esconde este PR.
    expect(run(room({ pilotSessions: [{ owner: "acme", repo: "acme-web", gh_number: 99 }] }))).toHaveLength(1);
  });

  const blockedRoom = (reason: string | null = "CI falhou 3 vezes") =>
    room({
      projects: [proj([])],
      blocked: [{ owner: "acme", repo: "acme-web", name: "acme-web", blocked: [{ number: 31, title: "Item 31", reason }] }],
    });

  test("issue bloqueada pelo piloto aparece com o motivo e abre a issue", () => {
    expect(run(blockedRoom())).toEqual([
      expect.objectContaining({
        kind: "pilot_blocked",
        label: "Bloqueada pelo piloto",
        project: "acme-web",
        item: "#31 Item 31",
        detail: "CI falhou 3 vezes",
        since: NOW - 60 * MIN,
        actionLabel: "Abrir a issue",
        action: { to: "url", url: "https://github.com/acme/acme-web/issues/31" },
      }),
    ]);
    expect(run(blockedRoom(null))[0].detail).toBe("Sem motivo registrado");
  });

  test("some quando a issue sai de bloqueada no ambiente", () => {
    const acme = (status: "em_execucao" | "bloqueado") =>
      dashboard({ projects: [{ project: project({ gh_owner: "acme", gh_repo: "acme-web" }), work_items: [workItem({ status })] }] });
    expect(run(blockedRoom(), { dashboards: { wsl: acme("em_execucao") } })).toEqual([]);
    expect(run(blockedRoom(), { dashboards: { wsl: acme("bloqueado") } })).toHaveLength(1);
  });

  test("sem hub (ou sem a leitura), os dois tipos não aparecem", () => {
    expect(attention(input({ room: room() }))).toEqual([]);
    expect(attention(input({ room: blockedRoom() }))).toEqual([]);
    expect(run(undefined)).toEqual([]);
  });

  test("ordem: depois das interrompidas e antes do ambiente caído", () => {
    const d = dashboard({ interrupted_sessions: [session({ id: "c", state: "interrupted" })] }, [workItem()]);
    const items = run(room(), {
      dashboards: { wsl: d },
      status: { wsl: { env: "wsl", connected: false, error: "x", since: NOW } },
    });
    expect(items.map((i) => i.kind)).toEqual(["interrupted", "ci_failed", "env_down"]);
  });
});
