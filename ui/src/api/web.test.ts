import type { DaemonApi } from "./daemon";
import type { EnvStatus, PushEvent } from "./types";
import { CLOSE_SESSION_ENDED, loginError, WebDaemonApi, webWsUrl } from "./web";

/** WebSocket em memória: guarda os frames e entrega mensagens do "hub". */
class FakeSocket {
  static all: FakeSocket[] = [];
  static get last() {
    return FakeSocket.all[FakeSocket.all.length - 1];
  }
  readyState = 0;
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  onclose: ((ev: { code: number }) => void) | null = null;
  constructor(public url: string) {
    FakeSocket.all.push(this);
  }
  send(m: string) {
    this.sent.push(m);
  }
  close() {
    this.readyState = 3;
  }
  open() {
    this.readyState = 1;
    this.onopen?.();
  }
  hub(msg: object) {
    this.onmessage?.({ data: JSON.stringify(msg) });
  }
  /** Responde o último pedido do método. */
  reply(method: string, result: unknown) {
    const req = this.sent
      .map((f) => JSON.parse(f))
      .filter((f) => f.method === method)
      .pop();
    this.hub({ id: req.id, result });
  }
  drop(code = 1006) {
    this.readyState = 3;
    this.onclose?.({ code });
  }
}

const flush = () => new Promise((r) => setTimeout(r, 0));

interface Req {
  url: string;
  init?: RequestInit;
}

/** `fetch` com respostas por rota. */
function fakeFetch(routes: Record<string, { status: number; body?: unknown }>) {
  const reqs: Req[] = [];
  const f = (async (url: string, init?: RequestInit) => {
    reqs.push({ url, init });
    const r = routes[url] ?? { status: 404 };
    return new Response(r.body === undefined ? null : JSON.stringify(r.body), { status: r.status });
  }) as unknown as typeof fetch;
  return { f, reqs, routes };
}

function api(routes: Record<string, { status: number; body?: unknown }> = {}) {
  const timers: { ms: number; run: () => void }[] = [];
  const http = fakeFetch(routes);
  const a = new WebDaemonApi({
    url: "ws://hub/web/ws",
    makeSocket: (u) => new FakeSocket(u) as unknown as WebSocket,
    fetch: http.f,
    schedule: (run, ms) => timers.push({ ms, run }),
  });
  return { a, timers, http };
}

/** Abre a conexão e responde o `hub.info` (protocolo 1.3). */
async function connected(a: WebDaemonApi, protocol = "1.3") {
  a.start();
  const s = FakeSocket.last;
  s.open();
  s.reply("hub.info", { protocol, version: "0.11.0", commit: "", name: "mac-mini", config_rev: 1, backup: {} });
  await flush();
  return s;
}

beforeEach(() => {
  FakeSocket.all = [];
});

describe("WebDaemonApi (#201)", () => {
  it("monta a URL do /web/ws na mesma origem", () => {
    expect(webWsUrl({ protocol: "https:", host: "hub.ts.net" })).toBe("wss://hub.ts.net/web/ws");
    expect(webWsUrl({ protocol: "http:", host: "127.0.0.1:7879" })).toBe("ws://127.0.0.1:7879/web/ws");
  });

  it("só conecta depois de start; hubCall com fila até abrir, erro e pushes", async () => {
    const { a } = api();
    expect(FakeSocket.all).toHaveLength(0);
    const statuses: boolean[] = [];
    a.onHubStatus((st) => statuses.push(st.connected));
    const pushes: string[] = [];
    a.onHubPush((e) => pushes.push(`${e.env}:${e.event}`));

    a.start();
    const s = FakeSocket.last;
    const p = a.hubCall("hub.dashboard.get");
    expect(s.sent).toEqual([]);
    s.open();
    // Na abertura: a fila e a conferência do protocolo.
    expect(s.sent.map((f) => JSON.parse(f).method)).toEqual(["hub.dashboard.get", "hub.info"]);
    expect(statuses).toEqual([]);
    s.reply("hub.info", { protocol: "1.3" });
    await flush();
    expect(statuses).toEqual([true]);

    s.hub({ event: "hub.proposals.changed", data: {} });
    s.reply("hub.dashboard.get", { envs: [] });
    await expect(p).resolves.toEqual({ envs: [] });
    expect(pushes).toEqual(["hub:hub.proposals.changed"]);

    const q = a.hubCall("hub.secrets.set", { name: "X" });
    const id = JSON.parse(s.sent[s.sent.length - 1]).id;
    s.hub({ id, error: { code: -32601, message: "método indisponível na web" } });
    await expect(q).rejects.toThrow("método indisponível na web");
  });

  it("protocolo do hub com outro major: fica desconectado com o motivo", async () => {
    const { a } = api();
    await connected(a, "2.0");
    const st = await a.hubStatus();
    expect(st?.connected).toBe(false);
    expect(st?.error).toMatch(/incompatível \(2\.0\)/);
  });

  it("reconecta com backoff quando a conexão cai", async () => {
    const { a, timers } = api({
      "/web/me": { status: 200, body: { authenticated: true, identity: null, expires_at: 1 } },
    });
    const s = await connected(a);
    const p = a.hubCall("hub.tree.get");
    s.drop();
    await expect(p).rejects.toThrow("hub desconectado");
    expect((await a.hubStatus())?.connected).toBe(false);
    expect(timers.map((t) => t.ms)).toEqual([1000]);
    timers[0].run();
    const s2 = FakeSocket.last;
    expect(s2).not.toBe(s);
    // Cai antes de abrir: confere a sessão (o 401 do upgrade não aparece) e espera o dobro.
    s2.drop();
    await flush();
    await flush();
    expect(timers.map((t) => t.ms)).toEqual([1000, 2000]);
  });

  it("4401: sessão revogada leva ao login, sem reconectar", async () => {
    const { a, timers } = api();
    const out: number[] = [];
    a.onLoggedOut(() => out.push(1));
    const s = await connected(a);
    const p = a.hubCall("hub.dashboard.get");
    s.drop(CLOSE_SESSION_ENDED);
    await expect(p).rejects.toThrow();
    expect(out).toEqual([1]);
    expect(timers).toEqual([]);
    expect((await a.hubStatus())?.connected).toBe(false);
    await expect(a.hubCall("hub.info")).rejects.toThrow("hub desconectado");
  });

  it("401 no upgrade (cookie recusado): /web/me sem sessão leva ao login", async () => {
    const { a, timers } = api({ "/web/me": { status: 401 } });
    const out: number[] = [];
    a.onLoggedOut(() => out.push(1));
    a.start();
    FakeSocket.last.drop();
    await flush();
    await flush();
    expect(out).toEqual([1]);
    expect(timers).toEqual([]);
  });

  it("login manda o token só no corpo do POST e não o guarda", async () => {
    const { a, http } = api({ "/web/login": { status: 200, body: { authenticated: true, expires_at: 1 } } });
    localStorage.clear();
    sessionStorage.clear();
    await a.login("segredo-do-hub");
    const req = http.reqs[0];
    expect(req.url).toBe("/web/login");
    expect(req.init?.method).toBe("POST");
    expect(JSON.parse(String(req.init?.body))).toEqual({ token: "segredo-do-hub" });
    expect(req.init?.credentials).toBe("same-origin");
    expect(JSON.stringify({ ...localStorage })).not.toContain("segredo-do-hub");
    expect(JSON.stringify({ ...sessionStorage })).not.toContain("segredo-do-hub");
    // A conexão não leva o token: só o cookie.
    a.start();
    expect(FakeSocket.last.url).toBe("ws://hub/web/ws");
  });

  it("login recusado: mensagens para a tela", async () => {
    const {
      a,
      http: { routes },
    } = api({ "/web/login": { status: 401, body: { error: "token inválido" } } });
    await expect(a.login("x")).rejects.toThrow("Token inválido.");
    routes["/web/login"] = { status: 429, body: { error: "muitas tentativas: aguarde", retry_after: 60 } };
    await expect(a.login("x")).rejects.toThrow("aguarde 60 s");
    expect(loginError(403, null)).toMatch(/Acesso negado/);
    expect(loginError(404, null)).toMatch(/desligado/);
  });

  it("me e logout", async () => {
    const { a, http } = api({
      "/web/me": { status: 200, body: { authenticated: true, identity: "eu@exemplo.com", expires_at: 5 } },
      "/web/logout": { status: 200, body: { revoked: 1 } },
    });
    await expect(a.me()).resolves.toEqual({ authenticated: true, identity: "eu@exemplo.com", expires_at: 5 });
    const out: number[] = [];
    a.onLoggedOut(() => out.push(1));
    await connected(a);
    await a.logout();
    expect(http.reqs.map((r) => `${r.init?.method ?? "GET"} ${r.url}`)).toEqual(["GET /web/me", "POST /web/logout"]);
    expect(out).toEqual([1]);
    expect((await a.hubStatus())?.connected).toBe(false);
  });

  it("ambientes pelo hub, sem terminal", async () => {
    const { a } = api();
    const s = await connected(a);
    const d: DaemonApi = a;
    expect(d.web).toBe(true);
    const envs = d.listEnvs();
    s.reply("hub.envs.list", [
      { env_id: "d-wsl", name: "wsl", created_at: 0, last_seen_at: Date.now(), last_seq: 1, pending: 0, cleanup: null },
    ]);
    await expect(envs).resolves.toEqual([
      { id: "d-wsl", name: "wsl", url: "", kind: "remote", distro: null, has_token: false },
    ]);
    const st = d.statuses();
    s.reply("hub.envs.list", [
      { env_id: "d-wsl", name: "wsl", created_at: 0, last_seen_at: null, last_seq: 0, pending: 0, cleanup: null },
    ]);
    await expect(st).resolves.toMatchObject([{ env: "d-wsl", connected: false, error: "nunca enviou ao hub" }]);
    await expect(d.attach("d-wsl", "s", () => {})).rejects.toThrow("indisponível na web");
    await expect(d.openVsCode("d-wsl", "/x")).rejects.toThrow("indisponível na web");
    await expect(d.getHub()).resolves.toEqual({ url: "ws://hub/web/ws", has_token: false });
  });

  it("fase 2: métodos do ambiente por hub.env.call, só os da lista do canal", async () => {
    const { a } = api();
    const s = await connected(a);
    const r = a.call<{ id: string }>("d-wsl", "session.start", { gh_number: 7, remote_control: true });
    await flush();
    const req = JSON.parse(s.sent[s.sent.length - 1]);
    expect(req.method).toBe("hub.env.call");
    expect(req.params).toEqual({ env: "d-wsl", method: "session.start", params: { gh_number: 7, remote_control: true } });
    s.hub({ id: req.id, result: { id: "s-9" } });
    await expect(r).resolves.toEqual({ id: "s-9" });

    // Erro do hub (ambiente offline) chega com a mensagem dele.
    const off = a.call("d-mac", "session.kill", { session_id: "x" });
    await flush();
    const req2 = JSON.parse(s.sent[s.sent.length - 1]);
    s.hub({ id: req2.id, error: { code: -32030, message: "ambiente offline: mac está sem canal com o hub agora" } });
    await expect(off).rejects.toThrow("ambiente offline: mac");

    // Fora da lista: recusa sem ir ao hub.
    const before = s.sent.length;
    for (const m of ["session.input", "session.attach", "session.permission.respond", "cleanup.run", "sync.now"]) {
      await expect(a.call("d-wsl", m)).rejects.toThrow("indisponível na web");
    }
    expect(s.sent.length).toBe(before);
  });

  it("fase 2: env.push vira push do ambiente e env.online, estado do ambiente", async () => {
    const { a } = api();
    const s = await connected(a);
    const env: PushEvent[] = [];
    const hub: PushEvent[] = [];
    const st: EnvStatus[] = [];
    a.onPush((e) => env.push(e));
    a.onHubPush((e) => hub.push(e));
    a.onStatus((x) => st.push(x));
    s.hub({ event: "env.push", data: { env_id: "d-wsl", event: "session_started", data: { session_id: "s-9" } } });
    s.hub({ event: "env.online", data: { env_id: "d-wsl", online: false } });
    expect(env).toEqual([{ env: "d-wsl", event: "session_started", data: { session_id: "s-9" } }]);
    // `env.online` também relê o hub (a UI pega o `HubEnv.online`).
    expect(hub.map((e) => e.event)).toEqual(["env.online"]);
    expect(st).toMatchObject([{ env: "d-wsl", connected: false }]);
  });
});
