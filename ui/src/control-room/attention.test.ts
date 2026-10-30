import { attention, SYNC_INTERVAL_MS, type AttentionInput } from "./attention";
import { FakeDaemonApi } from "../api/fake";
import { dashboard, envInfo, MIN, NOW, session, sync, workItem } from "../test/fixtures";
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
    action: { to: "terminal", env: "wsl", session: session().id },
  },
  {
    caso: "sessão interrompida",
    p: { dashboards: { wsl: dashboard({ interrupted_sessions: [session({ state: "interrupted" })] }, [workItem()]) } },
    kind: "interrupted",
    title: "ctower-agents #31 Item 31",
    action: { to: "recovery" },
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
    action: { to: "project", env: "wsl", projectId: 1 },
  },
  {
    caso: "ambiente desconectado",
    p: { status: { wsl: { env: "wsl", connected: false, error: "daemon inacessível", since: NOW - 3 * MIN } } },
    kind: "env_down",
    title: "ctower-agents (WSL) desconectado",
    action: { to: "settings" },
  },
  {
    caso: "sync atrasado",
    p: { dashboards: { wsl: dashboard({ sync: [sync({ last_ok_at: NOW - 4 * SYNC_INTERVAL_MS })] }) } },
    kind: "sync_late",
    title: "GitHub de ctower-agents sem sincronizar",
    action: { to: "project", env: "wsl", projectId: 1 },
  },
  {
    caso: "sync que nunca funcionou",
    p: { dashboards: { wsl: dashboard({ sync: [sync({ last_ok_at: null, last_error: "401" })] }) } },
    kind: "sync_late",
    title: "GitHub de ctower-agents sem sincronizar",
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
])("$caso", ({ p, kind, title, action }) => {
  const items = attention(input(p as Partial<AttentionInput>));
  expect(items).toHaveLength(1);
  expect(items[0]).toMatchObject({ kind, title, action });
  expect(items[0].label).not.toBe("");
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
