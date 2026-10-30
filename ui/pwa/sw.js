// Service worker da PWA (issue #203, spec do épico #205 §7.3 e §7.5).
//
// Guarda em cache só o shell: o index.html, os assets com hash do build e os
// arquivos estáticos da PWA. Nunca guarda dados: o /web/ws, o login
// (/web/*) e as rotas do hub passam direto, sem cache. Sem rede, a
// navegação abre o shell guardado, e a UI mostra "hub indisponível".
//
// Também mostra a Web Push do hub e, no toque, abre o deep link: o Remote
// Control (claude.ai) numa janela nova, ou a tela da própria PWA, focando
// uma janela já aberta antes de abrir outra.
//
// O build (`vite build --mode web`, pwa/plugin.mjs) preenche VERSION e SHELL.
const VERSION = "__CT_VERSION__";
const SHELL = __CT_SHELL__;
const CACHE = `ct-shell-${VERSION}`;
const PREFIX = "ct-shell-";

/** Caminhos que nunca passam pelo cache (dados e rotas do hub). */
function neverCached(path) {
  return ["/web/", "/ws", "/sync/", "/agent/", "/healthz"].some((p) => path === p || path.startsWith(p));
}

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((c) => c.addAll(SHELL))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k.startsWith(PREFIX) && k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const req = event.request;
  if (req.method !== "GET") return;
  const url = new URL(req.url);
  if (url.origin !== self.location.origin || neverCached(url.pathname)) return;
  if (req.mode === "navigate") {
    // Rede primeiro (o index.html tem no-cache); sem rede, o shell guardado.
    event.respondWith(
      fetch(req).catch(() => caches.open(CACHE).then((c) => c.match("/")).then((r) => r || Response.error())),
    );
    return;
  }
  if (!SHELL.includes(url.pathname)) return;
  event.respondWith(
    caches.open(CACHE).then((c) => c.match(url.pathname).then((hit) => hit || fetch(req))),
  );
});

self.addEventListener("push", (event) => {
  let n = {};
  try {
    n = event.data ? event.data.json() : {};
  } catch {
    n = {};
  }
  const title = n.title || "Control Tower";
  event.waitUntil(
    self.registration.showNotification(title, {
      body: n.body || "",
      tag: n.tag || undefined,
      icon: "/icons/icon-192.png",
      badge: "/icons/icon-192.png",
      data: { url: n.url || "/", event: n.event || null },
    }),
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const raw = (event.notification.data && event.notification.data.url) || "/";
  const target = new URL(raw, self.location.origin);
  if (target.origin !== self.location.origin) {
    // Remote Control: o sistema entrega ao app Claude, se instalado.
    event.waitUntil(self.clients.openWindow(target.href));
    return;
  }
  const path = target.pathname + target.search;
  event.waitUntil(
    self.clients.matchAll({ type: "window", includeUncontrolled: true }).then((list) => {
      const open = list.find((c) => new URL(c.url).origin === self.location.origin);
      if (open) {
        open.postMessage({ type: "ct-open", url: path });
        return open.focus();
      }
      return self.clients.openWindow(path);
    }),
  );
});
