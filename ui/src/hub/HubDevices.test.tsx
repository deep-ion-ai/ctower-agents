// Hub › Dispositivos (#203): lista, eventos por dispositivo, testar,
// remover e, na PWA, ativar neste dispositivo e a instrução do iOS.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { HubPushDevice, PushEvent } from "../protocol/hub-gen";
import { dashboard, hubEnv, hubInfo, hubProject, hubProjectView, hubStatus, HUB_DASHBOARD } from "../test/fixtures";
import { now, webApi } from "../test/web";

const ALL: PushEvent[] = ["session_error", "env_stale", "proposal_new", "needs_human"];

function device(d: Partial<HubPushDevice> = {}): HubPushDevice {
  return {
    id: 1,
    label: "Chrome · Android",
    events: ALL,
    service: "fcm.googleapis.com",
    created_at: now - 3600_000,
    last_ok_at: null,
    failures: 0,
    disabled_at: null,
    current: false,
    ...d,
  };
}

/** `hub.push.*` como o hub: a lista muda com update e remove. */
function withDevices<T extends FakeDaemonApi>(api: T, initial: HubPushDevice[]) {
  let rows = initial;
  return api
    .onHub("hub.push.list", () => rows)
    .onHub("hub.push.update", (p: { id: number; events: PushEvent[] }) => {
      rows = rows.map((r) => (r.id === p.id ? { ...r, events: p.events } : r));
      return rows.find((r) => r.id === p.id);
    })
    .onHub("hub.push.remove", (p: { id: number }) => {
      const had = rows.some((r) => r.id === p.id);
      rows = rows.filter((r) => r.id !== p.id);
      return { removed: had };
    })
    .onHub("hub.push.test", () => ({ delivered: true, status: 201, removed: false, error: null }))
    .onHub("hub.push.subscribe", (p: { endpoint: string; label?: string }) => {
      const d = device({ id: 9, label: p.label ?? null, current: true });
      rows = [d, ...rows];
      return d;
    })
    .onHub("hub.push.vapid_key", { key: "BAAA" });
}

const setUa = (ua: string) => Object.defineProperty(navigator, "userAgent", { value: ua, configurable: true });
const IPHONE = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 Version/17.4 Mobile/15E148 Safari/604.1";
const ANDROID = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Chrome/126.0 Mobile Safari/537.36";
const realUa = navigator.userAgent;

afterEach(() => {
  setUa(realUa);
  vi.unstubAllGlobals();
  delete (navigator as unknown as Record<string, unknown>).serviceWorker;
  delete (window as unknown as Record<string, unknown>).PushManager;
});

/** Navegador com Web Push: service worker pronto e PushManager que inscreve. */
function stubPushBrowser(permission: NotificationPermission = "default") {
  const subscription = {
    endpoint: "https://fcm.googleapis.com/fcm/send/xyz",
    toJSON: () => ({ endpoint: "https://fcm.googleapis.com/fcm/send/xyz", keys: { p256dh: "P", auth: "A" } }),
    unsubscribe: vi.fn(async () => true),
  };
  const pushManager = { getSubscription: vi.fn(async () => null), subscribe: vi.fn(async () => subscription) };
  const reg = { pushManager };
  Object.defineProperty(navigator, "serviceWorker", {
    configurable: true,
    value: {
      ready: Promise.resolve(reg),
      getRegistration: async () => reg,
      register: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    },
  });
  Object.defineProperty(window, "PushManager", { configurable: true, value: function PushManager() {} });
  const requestPermission = vi.fn(async () => "granted" as NotificationPermission);
  vi.stubGlobal("Notification", { permission, requestPermission });
  return { pushManager, requestPermission, subscription };
}

describe("Hub › Dispositivos na PWA", () => {
  it("lista, troca eventos, testa e remove", async () => {
    setUa(ANDROID);
    stubPushBrowser();
    const api = withDevices(webApi(), [device(), device({ id: 2, label: "Safari · iPhone", current: true, last_ok_at: now })]);
    const user = userEvent.setup();
    render(<App api={api} initial={{ name: "hub", section: "devices" }} />);
    expect(await screen.findByRole("button", { name: "Testar Chrome · Android" })).toBeInTheDocument();
    expect(screen.getByText("este dispositivo")).toBeInTheDocument();
    expect(screen.getByText("Este dispositivo recebe notificações.")).toBeInTheDocument();

    const list = screen.getByRole("list", { name: "Dispositivos inscritos" });
    const first = within(list).getAllByRole("listitem")[0];
    await user.click(within(first).getByRole("checkbox", { name: /Ambiente fora do ar/ }));
    await waitFor(() =>
      expect(api.hubCallsTo("hub.push.update").map((c) => c.params)).toEqual([
        { id: 1, events: ["session_error", "proposal_new", "needs_human"] },
      ]),
    );
    await waitFor(() => expect(within(first).getByRole("checkbox", { name: /Ambiente fora do ar/ })).not.toBeChecked());

    await user.click(screen.getByRole("button", { name: "Testar Chrome · Android" }));
    expect(await screen.findByText("Notificação de teste enviada.")).toBeInTheDocument();
    expect(api.hubCallsTo("hub.push.test").map((c) => c.params)).toEqual([{ id: 1 }]);

    await user.click(screen.getByRole("button", { name: "Remover Chrome · Android" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Testar Chrome · Android" })).toBeNull());
    expect(api.hubCallsTo("hub.push.remove").map((c) => c.params)).toEqual([{ id: 1 }]);
  });

  it("ativar neste dispositivo pede a permissão no toque e inscreve com a chave VAPID", async () => {
    setUa(ANDROID);
    const b = stubPushBrowser();
    const api = withDevices(webApi(), []);
    const user = userEvent.setup();
    render(<App api={api} initial={{ name: "hub", section: "devices" }} />);
    expect(await screen.findByText("Nenhum dispositivo inscrito.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Ativar notificações neste dispositivo" }));
    expect(await screen.findByText("Notificações ativadas neste dispositivo.")).toBeInTheDocument();
    expect(b.requestPermission).toHaveBeenCalledTimes(1);
    expect(b.pushManager.subscribe).toHaveBeenCalledWith({
      userVisibleOnly: true,
      applicationServerKey: new Uint8Array([4, 0, 0]),
    });
    expect(api.hubCallsTo("hub.push.subscribe").map((c) => c.params)).toEqual([
      {
        endpoint: "https://fcm.googleapis.com/fcm/send/xyz",
        keys: { p256dh: "P", auth: "A" },
        label: "Chrome · Android",
      },
    ]);
    expect(await screen.findByText("este dispositivo")).toBeInTheDocument();

    // Desativar: desinscreve o navegador e remove do hub.
    b.pushManager.getSubscription.mockResolvedValue(b.subscription as never);
    await user.click(screen.getByRole("button", { name: "Desativar neste dispositivo" }));
    expect(await screen.findByText("Notificações desativadas neste dispositivo.")).toBeInTheDocument();
    expect(b.subscription.unsubscribe).toHaveBeenCalled();
    expect(api.hubCallsTo("hub.push.remove").map((c) => c.params)).toEqual([{ id: 9 }]);
  });

  it("permissão negada no pedido: mensagem, sem inscrição", async () => {
    setUa(ANDROID);
    const b = stubPushBrowser();
    b.requestPermission.mockResolvedValue("denied");
    const api = withDevices(webApi(), []);
    const user = userEvent.setup();
    render(<App api={api} initial={{ name: "hub", section: "devices" }} />);
    await user.click(await screen.findByRole("button", { name: "Ativar notificações neste dispositivo" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/bloqueadas/);
    expect(api.hubCallsTo("hub.push.subscribe")).toEqual([]);
  });

  it("iPhone no Safari, fora da tela de início: instrução para instalar, sem botão", async () => {
    setUa(IPHONE);
    stubPushBrowser();
    const api = withDevices(webApi(), []);
    render(<App api={api} initial={{ name: "hub", section: "devices" }} />);
    const note = await screen.findByRole("note", { name: "Notificações no iPhone e no iPad" });
    expect(note).toHaveTextContent(/Adicionar à Tela de Início/);
    expect(note).toHaveTextContent(/iOS\s+16\.4/);
    expect(screen.queryByRole("button", { name: /Ativar notificações/ })).toBeNull();
  });

  it("deep link /?go=hub:devices abre a seção", async () => {
    setUa(ANDROID);
    window.history.replaceState(null, "", "/?go=hub:devices");
    const api = withDevices(webApi(), [device()]);
    render(<App api={api} />);
    expect(await screen.findByRole("button", { name: "Testar Chrome · Android" })).toBeInTheDocument();
    expect(window.location.search).toBe("");
  });
});

describe("Hub › Dispositivos no desktop", () => {
  it("mesma lista, sem inscrever este aparelho", async () => {
    const api = withDevices(
      new FakeDaemonApi()
        .on("dashboard.get", dashboard({ projects: [] }))
        .withHub("ws://mac-mini:7879/ws")
        .onHub("hub.info", hubInfo())
        .onHub("hub.status", hubStatus())
        .onHub("hub.projects.list", [hubProjectView(hubProject())])
        .onHub("hub.envs.list", [hubEnv()])
        .onHub("hub.dashboard.get", HUB_DASHBOARD),
      [device()],
    );
    render(<App api={api} initial={{ name: "hub", section: "devices" }} />);
    expect(await screen.findByRole("button", { name: "Testar Chrome · Android" })).toBeInTheDocument();
    expect(screen.getByText(/Os aparelhos se inscrevem pela PWA/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Ativar notificações/ })).toBeNull();
  });
});
