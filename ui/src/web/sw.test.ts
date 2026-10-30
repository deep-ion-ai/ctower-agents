// Service worker da PWA (#203): roda o sw.js gerado pelo build (pwa/sw.js
// com a lista do shell) num `self` falso e confere o cache só do shell, a
// Web Push e o deep link do toque na notificação.
import { buildServiceWorker, shellList, shellVersion } from "../../pwa/plugin.mjs";

const ORIGIN = "https://hub.exemplo.ts.net";
const SHELL = shellList(
  ["index.html", "assets/index-abc.js", "assets/index-abc.js.map", "assets/f.woff", "assets/f.woff2", "theme-boot.js"],
  ["manifest.webmanifest", "icons/icon-192.png"],
);

type Listener = (e: any) => void;

function loadSw() {
  const listeners: Record<string, Listener> = {};
  const store = new Map<string, Response>();
  const cache = {
    addAll: vi.fn(async (urls: string[]) => urls.forEach((u) => store.set(u, new Response(`cache:${u}`)))),
    match: vi.fn(async (k: string) => store.get(k)?.clone()),
  };
  const caches = {
    open: vi.fn(async () => cache),
    keys: vi.fn(async () => ["ct-shell-velho", `ct-shell-${shellVersion(SHELL)}`, "outro-cache"]),
    delete: vi.fn(async () => true),
  };
  const windows: { url: string; focus: ReturnType<typeof vi.fn>; postMessage: ReturnType<typeof vi.fn> }[] = [];
  const clients = {
    claim: vi.fn(async () => undefined),
    matchAll: vi.fn(async () => windows),
    openWindow: vi.fn(async () => null),
  };
  const showNotification = vi.fn(async () => undefined);
  const self = {
    addEventListener: (t: string, f: Listener) => (listeners[t] = f),
    location: { origin: ORIGIN },
    skipWaiting: vi.fn(async () => undefined),
    clients,
    registration: { showNotification },
  };
  const net = vi.fn(async (_r: unknown) => new Response("rede"));
  new Function("self", "caches", "fetch", buildServiceWorker(SHELL))(self, caches, net);

  /** Dispara o evento e espera o `waitUntil`/`respondWith`. */
  async function fire(type: string, e: Record<string, unknown>) {
    let pending: Promise<unknown> | undefined;
    let responded: Promise<Response> | undefined;
    listeners[type]({
      ...e,
      waitUntil: (p: Promise<unknown>) => (pending = p),
      respondWith: (p: Promise<Response>) => (responded = p),
    });
    await pending;
    return responded;
  }
  const get = (path: string, mode = "cors", method = "GET") =>
    fire("fetch", { request: { method, url: new URL(path, ORIGIN).href, mode } });
  return { fire, get, cache, caches, clients, windows, showNotification, net, store };
}

test("lista do shell: index como /, sem mapas, sem .woff e com os estáticos da PWA", () => {
  expect(SHELL).toEqual([
    "/",
    "/assets/f.woff2",
    "/assets/index-abc.js",
    "/icons/icon-192.png",
    "/manifest.webmanifest",
    "/theme-boot.js",
  ]);
  expect(shellVersion(SHELL)).toMatch(/^[0-9a-f]{12}$/);
});

test("instala guardando só o shell e apaga só os caches antigos do shell", async () => {
  const sw = loadSw();
  await sw.fire("install", {});
  expect(sw.cache.addAll).toHaveBeenCalledWith(SHELL);
  await sw.fire("activate", {});
  expect(sw.caches.delete.mock.calls).toEqual([["ct-shell-velho"]]);
  expect(sw.clients.claim).toHaveBeenCalled();
});

test("dados e rotas do hub nunca passam pelo cache", async () => {
  const sw = loadSw();
  await sw.fire("install", {});
  for (const path of ["/web/ws", "/web/me", "/web/login", "/ws", "/sync/push", "/agent/v1/overview", "/healthz", "/outra"]) {
    expect(await sw.get(path), path).toBeUndefined();
  }
  expect(await sw.get("/assets/index-abc.js", "cors", "POST")).toBeUndefined();
  expect(await sw.fire("fetch", { request: { method: "GET", url: "https://claude.ai/code/x", mode: "cors" } })).toBeUndefined();
  // Asset do shell: do cache, sem rede.
  const r = await sw.get("/assets/index-abc.js");
  expect(await r!.text()).toBe("cache:/assets/index-abc.js");
  expect(sw.net).not.toHaveBeenCalled();
});

test("navegação: rede primeiro; sem rede, o shell guardado", async () => {
  const sw = loadSw();
  await sw.fire("install", {});
  let r = await sw.get("/?go=hub:envs", "navigate");
  expect(await r!.text()).toBe("rede");
  sw.net.mockRejectedValueOnce(new TypeError("offline"));
  r = await sw.get("/hub", "navigate");
  expect(await r!.text()).toBe("cache:/");
});

test("push mostra a notificação com o deep link", async () => {
  const sw = loadSw();
  const n = {
    event: "needs_human",
    title: "Aguardando você",
    body: "deep-ion-ai/demo #5 · Web Push",
    url: "/?go=item&project=deep-ion-ai/demo&n=5",
    tag: "session:s1",
  };
  await sw.fire("push", { data: { json: () => n } });
  expect(sw.showNotification).toHaveBeenCalledWith("Aguardando você", {
    body: n.body,
    tag: "session:s1",
    icon: "/icons/icon-192.png",
    badge: "/icons/icon-192.png",
    data: { url: n.url, event: "needs_human" },
  });
  // Corpo ilegível: ainda assim uma notificação (o navegador exige).
  await sw.fire("push", { data: { json: () => JSON.parse("{") } });
  expect(sw.showNotification).toHaveBeenLastCalledWith("Control Tower", expect.objectContaining({ data: { url: "/", event: null } }));
});

test("toque: Remote Control em janela nova; tela da PWA focando a janela aberta", async () => {
  const sw = loadSw();
  const click = (url: string) => sw.fire("notificationclick", { notification: { close: vi.fn(), data: { url } } });
  await click("https://claude.ai/code/session_01abc");
  expect(sw.clients.openWindow).toHaveBeenLastCalledWith("https://claude.ai/code/session_01abc");

  // Sem janela aberta: abre a PWA no lugar.
  await click("/?go=hub:proposals");
  expect(sw.clients.openWindow).toHaveBeenLastCalledWith("/?go=hub:proposals");

  // Com a PWA aberta: foca e manda a tela por mensagem.
  const win = { url: `${ORIGIN}/`, focus: vi.fn(), postMessage: vi.fn() };
  sw.windows.push(win);
  sw.clients.openWindow.mockClear();
  await click("/?go=item&project=deep-ion-ai/demo&n=5");
  expect(win.postMessage).toHaveBeenCalledWith({ type: "ct-open", url: "/?go=item&project=deep-ion-ai/demo&n=5" });
  expect(win.focus).toHaveBeenCalled();
  expect(sw.clients.openWindow).not.toHaveBeenCalled();
});
