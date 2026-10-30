// PWA (issue #203, spec do épico #205 §7): registro do service worker,
// deep link das notificações (`/?go=...`), detecção do que o aparelho
// suporta (iOS fora da tela de início, sem Web Push) e a inscrição na Web
// Push do hub (`hub.push.*`). As funções puras são testadas em pwa.test.ts.
import { useEffect } from "react";
import type { DaemonApi } from "../api/daemon";
import type { HubPushDevice, PushEvent } from "../protocol/hub-gen";
import { hubTab } from "../hub/sections";
import { itemTab } from "../project/tabs";
import type { OpenTab } from "../state/nav";
import { browserLabel } from "../hub/HubWebSessions";

/** Eventos da Web Push e o texto de cada um, na ordem da tela. */
export const PUSH_EVENTS: { event: PushEvent; label: string; hint: string }[] = [
  { event: "needs_human", label: "Aguardando você", hint: "só de sessões sem Remote Control (as com ele já avisam pelo app Claude)" },
  { event: "session_error", label: "Sessão saiu com erro", hint: "código de saída diferente de 0" },
  { event: "proposal_new", label: "Proposta nova do gestor", hint: "prioridade ou status a decidir" },
  { event: "env_stale", label: "Ambiente fora do ar", hint: "sem canal com o hub há mais de 1 min (ou, num ambiente antigo, sem enviar ao hub)" },
];

/**
 * Aba do deep link da notificação (spec §7.3): `go=item&project=<o>/<r>&n=<n>`,
 * `go=hub:proposals`, `go=hub:envs`, `go=hub:devices` ou `go=control`.
 * `null` = sem `go` ou desconhecido.
 */
export function deepLinkTab(search: string): OpenTab | null {
  const q = new URLSearchParams(search);
  const go = q.get("go");
  switch (go) {
    case "item": {
      const [owner, repo] = (q.get("project") ?? "").split("/");
      const n = Number(q.get("n"));
      if (!owner || !repo || !Number.isInteger(n) || n <= 0) return null;
      const tab = itemTab({ name: "project", owner, repo }, n, "");
      return { ...tab, title: `#${n}` };
    }
    case "hub:proposals":
      return hubTab("proposals");
    case "hub:envs":
      return hubTab("envs");
    case "hub:devices":
      return hubTab("devices");
    case "control":
      return { screen: { name: "control" } };
    default:
      return null;
  }
}

/** Registra o service worker (só no bundle web, servido pelo hub). */
export function registerServiceWorker(): void {
  if (!("serviceWorker" in navigator)) return;
  const register = () =>
    navigator.serviceWorker.register("/sw.js", { scope: "/" }).catch((e) => console.warn("service worker:", e));
  if (document.readyState === "complete") register();
  else window.addEventListener("load", register, { once: true });
}

/** O que o aparelho permite: Web Push, iOS fora da tela de início, bloqueada ou sem suporte. */
export type PushSupport = "ok" | "denied" | "ios-home-screen" | "unsupported";

export interface PushEnv {
  ua: string;
  /** iPad no modo desktop se apresenta como Mac, com toque. */
  touchPoints: number;
  standalone: boolean;
  serviceWorker: boolean;
  pushManager: boolean;
  notification: boolean;
  permission: NotificationPermission | null;
}

export function pushEnv(): PushEnv {
  const nav = navigator as Navigator & { standalone?: boolean };
  const hasNotification = typeof Notification !== "undefined";
  return {
    ua: nav.userAgent,
    touchPoints: nav.maxTouchPoints ?? 0,
    standalone: nav.standalone === true || window.matchMedia?.("(display-mode: standalone)").matches === true,
    serviceWorker: "serviceWorker" in nav,
    pushManager: typeof window !== "undefined" && "PushManager" in window,
    notification: hasNotification,
    permission: hasNotification ? Notification.permission : null,
  };
}

export function isIos(ua: string, touchPoints: number): boolean {
  return /iPhone|iPad|iPod/.test(ua) || (/Macintosh/.test(ua) && touchPoints > 1);
}

/**
 * No iOS a Web Push só existe com a PWA na tela de início (iOS 16.4+,
 * spec §7.4): fora do modo standalone, a tela mostra a instrução.
 */
export function pushSupport(env: PushEnv = pushEnv()): PushSupport {
  if (isIos(env.ua, env.touchPoints) && !env.standalone) return "ios-home-screen";
  if (!env.serviceWorker || !env.pushManager || !env.notification) return "unsupported";
  if (env.permission === "denied") return "denied";
  return "ok";
}

/** Chave VAPID (base64url) no formato do `applicationServerKey`. */
export function vapidKeyBytes(b64url: string): Uint8Array {
  const b64 = b64url.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(b64url.length / 4) * 4, "=");
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** A chave da inscrição existente é a do hub? (sem a informação, vale como a mesma) */
export function sameKey(current: ArrayBuffer | null, key: Uint8Array): boolean {
  if (!current) return true;
  const a = new Uint8Array(current);
  return a.length === key.length && a.every((b, i) => b === key[i]);
}

/** Inscrição atual deste navegador, se houver. */
export async function currentSubscription(): Promise<PushSubscription | null> {
  if (!("serviceWorker" in navigator)) return null;
  const reg = await navigator.serviceWorker.getRegistration();
  return (await reg?.pushManager.getSubscription()) ?? null;
}

function subscribeParams(sub: PushSubscription, label?: string) {
  const j = sub.toJSON();
  return {
    endpoint: j.endpoint ?? sub.endpoint,
    keys: { p256dh: j.keys?.p256dh ?? "", auth: j.keys?.auth ?? "" },
    ...(label ? { label } : {}),
  };
}

/**
 * Pede a permissão e inscreve este dispositivo. Precisa ser chamada direto
 * do toque do usuário (o iOS recusa o pedido fora de um gesto): o
 * `requestPermission` é a primeira coisa feita.
 */
export async function enablePush(api: DaemonApi): Promise<HubPushDevice> {
  const permission = await Notification.requestPermission();
  if (permission !== "granted") {
    throw new Error(
      permission === "denied"
        ? "As notificações foram bloqueadas: libere nas configurações do navegador para este site."
        : "Permissão de notificação não concedida.",
    );
  }
  const reg = await navigator.serviceWorker.ready;
  const { key } = await api.hubCall<{ key: string }>("hub.push.vapid_key");
  const applicationServerKey = vapidKeyBytes(key);
  let sub = await reg.pushManager.getSubscription();
  // Inscrição feita com outra chave VAPID (o hub gerou outra): refaz.
  if (sub && !sameKey(sub.options?.applicationServerKey ?? null, applicationServerKey)) {
    await sub.unsubscribe().catch(() => false);
    sub = null;
  }
  sub ??= await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey });
  return api.hubCall<HubPushDevice>("hub.push.subscribe", subscribeParams(sub, browserLabel(navigator.userAgent)));
}

/** Desinscreve este navegador e remove o dispositivo do hub. */
export async function disablePush(api: DaemonApi, id: number): Promise<void> {
  const sub = await currentSubscription().catch(() => null);
  await sub?.unsubscribe().catch(() => false);
  await api.hubCall("hub.push.remove", { id });
}

/**
 * Deep link da notificação: ao abrir a PWA por `/?go=...` e quando o service
 * worker manda `ct-open` a uma janela já aberta. Também prende de novo a
 * inscrição deste navegador à sessão web atual (depois de um novo login a
 * sessão antiga some, e com ela a inscrição), mantendo os eventos.
 */
export function usePwa(api: DaemonApi, web: boolean, openTab: (t: OpenTab) => void): void {
  useEffect(() => {
    if (!web) return;
    const open = (search: string) => {
      const tab = deepLinkTab(search);
      if (tab) openTab(tab);
    };
    if (window.location.search) {
      open(window.location.search);
      window.history.replaceState(null, "", window.location.pathname);
    }
    const sw = "serviceWorker" in navigator ? navigator.serviceWorker : null;
    const onMessage = (e: MessageEvent) => {
      const d = e.data as { type?: string; url?: string } | null;
      if (d?.type === "ct-open" && typeof d.url === "string") open(new URL(d.url, window.location.origin).search);
    };
    sw?.addEventListener("message", onMessage);
    if (typeof Notification !== "undefined" && Notification.permission === "granted") {
      void currentSubscription()
        .then((sub) => sub && api.hubCall("hub.push.subscribe", subscribeParams(sub)))
        .catch(() => undefined);
    }
    return () => sw?.removeEventListener("message", onMessage);
  }, [api, web, openTab]);
}
