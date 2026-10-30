// DaemonApi do modo web (PWA servida pelo cthubd, épico #205, spec §10.1).
// Fala só com o `/web/ws` da própria origem, autenticado pelo cookie da
// sessão web (`ct_web`, HttpOnly): o token de UI do hub nunca fica no
// JavaScript. Ambientes, estado e dashboards vêm do hub (`hub.envs.list`,
// com `online` do canal reverso); os métodos de ambiente da lista do canal
// vão por `hub.env.call` (fase 2, #343), e os pushes do ambiente chegam como
// `env.push`. O bundle `vite build --mode web` sai sem nada de
// `@tauri-apps/*`.

import type { DaemonApi } from "./daemon";
import type { EnvStatus, EnvView, HubConnView, PushEvent, UiConfig, UiUpdate, Unsubscribe } from "./types";
import type { HubEnv, HubInfo } from "../protocol/hub-gen";
import { DEFAULT_UI_CONFIG } from "../settings/defaults";
import { OFFLINE, envStatusesFromHub, envViewsFromHub } from "../web/hubData";

/** `env` dos pushes e do estado do hub, como no app desktop. */
const HUB = "hub";
const UI_CONFIG_KEY = "ct.web.ui";
export const MIN_BACKOFF_MS = 1000;
export const MAX_BACKOFF_MS = 30_000;
/** Fechamento do `/web/ws` quando a sessão web é revogada ou vence (cthubd). */
export const CLOSE_SESSION_ENDED = 4401;
/** Major do protocolo do hub que esta UI fala (o mesmo do app desktop). */
export const HUB_PROTOCOL_MAJOR = 1;
/**
 * Métodos do ambiente que a web chama pelo hub (`hub.env.call`): a lista do
 * canal reverso (`ct_http::sync::CHANNEL_METHODS`, spec §5.2). O hub confere
 * de novo; aqui só evita a ida.
 */
export const ENV_METHODS: ReadonlySet<string> = new Set([
  "env.info",
  "projects.list",
  "dashboard.get",
  "tree.get",
  "review.get",
  "events.list",
  "review.mark_reviewed",
  "recovery.list",
  "recovery.archived",
  "transcript.tail",
  "session.context",
  "session.get",
  "session.start",
  "session.start_project",
  "session.start_gestor",
  "session.resume",
  "session.kill",
  "session.archive",
  "session.remote_control.enable",
  "hub.sync.now",
  "tracker.issue_body",
]);

/** O método do ambiente existe na web? */
export const envMethodOnWeb = (method: string) => ENV_METHODS.has(method);

type Pending = { resolve: (v: any) => void; reject: (e: Error) => void };

/** `GET /web/me`. */
export interface WebMe {
  authenticated: boolean;
  /** Identidade do proxy (e-mail do Access ou login do Tailscale). */
  identity: string | null;
  /** Validade absoluta da sessão (ms). */
  expires_at: number | null;
}

/**
 * Login web (spec §3.1): a tela de login e o botão de sair usam isto. Na
 * implementação real é a própria `WebDaemonApi`.
 */
export interface WebAuth {
  me(): Promise<WebMe>;
  /** `POST /web/login { token }`; rejeita com a mensagem para o usuário. O token não é guardado. */
  login(token: string): Promise<void>;
  /** `POST /web/logout`: revoga a sessão atual e para a conexão. */
  logout(): Promise<void>;
  /** Conecta o `/web/ws` (depois de confirmar a sessão). */
  start(): void;
  /** A sessão web terminou (fechamento 4401, ou o hub recusou o cookie ao reconectar). */
  onLoggedOut(cb: () => void): Unsubscribe;
}

function unavailable(what: string): Promise<never> {
  return Promise.reject(new Error(`${what} indisponível na web`));
}

/** URL do `/web/ws` na mesma origem da página. */
export function webWsUrl(loc: Pick<Location, "protocol" | "host"> = window.location): string {
  return `${loc.protocol === "https:" ? "wss:" : "ws:"}//${loc.host}/web/ws`;
}

/** Mensagem do `POST /web/login` recusado, para a tela de login. */
export function loginError(status: number, body: { error?: string; retry_after?: number } | null): string {
  switch (status) {
    case 401:
      return "Token inválido.";
    case 429: {
      const s = body?.retry_after;
      return s ? `Muitas tentativas: aguarde ${s} s e tente de novo.` : "Muitas tentativas: aguarde e tente de novo.";
    }
    case 403:
      return "Acesso negado: confira o proxy (Cloudflare Access ou Tailscale) e a origem permitida no hub.";
    case 404:
      return "Acesso web desligado no hub ([web] enabled = false).";
    default:
      return body?.error ? `Erro do hub: ${body.error}` : `O hub respondeu ${status}.`;
  }
}

export interface WebDaemonOptions {
  url?: string;
  makeSocket?: (url: string) => WebSocket;
  fetch?: typeof fetch;
  /** Só para os testes: em vez de `setTimeout`. */
  schedule?: (f: () => void, ms: number) => void;
}

export class WebDaemonApi implements DaemonApi, WebAuth {
  readonly web = true;
  private readonly url: string;
  private readonly makeSocket: (url: string) => WebSocket;
  private readonly http: typeof fetch;
  private readonly schedule: (f: () => void, ms: number) => void;
  private ws: WebSocket | null = null;
  private next = 0;
  private pending = new Map<number, Pending>();
  private queue: string[] = [];
  private state: EnvStatus = { env: HUB, connected: false, error: null, since: Date.now() };
  private statusSubs = new Set<(s: EnvStatus) => void>();
  private pushSubs = new Set<(e: PushEvent) => void>();
  private envPushSubs = new Set<(e: PushEvent) => void>();
  private envStatusSubs = new Set<(s: EnvStatus) => void>();
  private outSubs = new Set<() => void>();
  private backoff = MIN_BACKOFF_MS;
  /** Conectando ou conectado (entre `start` e o fim da sessão). */
  private running = false;
  /** Identifica a conexão: respostas e fechamentos de uma antiga não valem. */
  private gen = 0;

  constructor(opts: WebDaemonOptions = {}) {
    this.url = opts.url ?? webWsUrl();
    this.makeSocket = opts.makeSocket ?? ((u) => new WebSocket(u));
    this.http = opts.fetch ?? ((...a) => fetch(...a));
    this.schedule = opts.schedule ?? ((f, ms) => void setTimeout(f, ms));
  }

  // --- Login web (spec §3.1) ---

  async me(): Promise<WebMe> {
    const r = await this.http("/web/me", { credentials: "same-origin", cache: "no-store" });
    if (r.status === 401) return { authenticated: false, identity: null, expires_at: null };
    if (!r.ok) throw new Error(loginError(r.status, await r.json().catch(() => null)));
    return (await r.json()) as WebMe;
  }

  async login(token: string): Promise<void> {
    const r = await this.http("/web/login", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ token }),
    });
    if (!r.ok) throw new Error(loginError(r.status, await r.json().catch(() => null)));
  }

  async logout(): Promise<void> {
    const r = await this.http("/web/logout", { method: "POST", credentials: "same-origin" });
    // 401: a sessão já tinha acabado; o resultado é o mesmo.
    if (!r.ok && r.status !== 401) throw new Error(`Não foi possível sair: o hub respondeu ${r.status}.`);
    this.end("você saiu");
  }

  onLoggedOut(cb: () => void): Unsubscribe {
    this.outSubs.add(cb);
    return () => this.outSubs.delete(cb);
  }

  start(): void {
    if (this.running) return;
    this.running = true;
    this.backoff = MIN_BACKOFF_MS;
    this.connect();
  }

  /** Para a conexão de vez: rejeita o que estava pendente e avisa quem ouve. */
  private end(reason: string) {
    const was = this.running;
    this.running = false;
    this.gen++;
    const ws = this.ws;
    this.ws = null;
    try {
      ws?.close();
    } catch {
      // já fechado
    }
    this.failPending(reason);
    this.setState(false, reason);
    if (was) for (const cb of this.outSubs) cb();
  }

  private failPending(reason: string) {
    for (const p of this.pending.values()) p.reject(new Error(reason));
    this.pending.clear();
    this.queue = [];
  }

  // --- Conexão ---

  private setState(connected: boolean, error: string | null) {
    if (this.state.connected === connected && this.state.error === error) return;
    this.state = { env: HUB, connected, error, since: Date.now() };
    for (const cb of this.statusSubs) cb(this.state);
  }

  private connect() {
    const gen = ++this.gen;
    let opened = false;
    const ws = this.makeSocket(this.url);
    this.ws = ws;
    ws.onopen = () => {
      if (gen !== this.gen) return;
      opened = true;
      this.backoff = MIN_BACKOFF_MS;
      for (const m of this.queue.splice(0)) ws.send(m);
      void this.checkProtocol(gen);
    };
    ws.onmessage = (ev) => {
      if (gen === this.gen) this.onMessage(String(ev.data));
    };
    ws.onclose = (ev?: CloseEvent) => {
      if (gen !== this.gen) return;
      this.ws = null;
      this.failPending("hub desconectado");
      if (ev?.code === CLOSE_SESSION_ENDED) {
        this.end("sessão web encerrada");
        return;
      }
      this.setState(false, "hub indisponível");
      // O navegador não mostra o 401 do upgrade: sem abrir, confere a sessão.
      if (!opened) {
        void this.me().then(
          (m) => (m.authenticated ? this.retry(gen) : this.end("sessão web encerrada")),
          () => this.retry(gen),
        );
      } else this.retry(gen);
    };
  }

  private retry(gen: number) {
    if (!this.running || gen !== this.gen) return;
    const wait = this.backoff;
    this.backoff = Math.min(this.backoff * 2, MAX_BACKOFF_MS);
    this.schedule(() => {
      if (this.running && gen === this.gen) this.connect();
    }, wait);
  }

  /** Confere o major do protocolo em `hub.info` (spec §9.2) antes de dar o hub por conectado. */
  private async checkProtocol(gen: number) {
    let info: HubInfo | null = null;
    try {
      info = await this.send<HubInfo>("hub.info", {});
    } catch {
      // Sem resposta: a conexão caiu (o onclose cuida) ou o hub recusou; segue.
    }
    if (gen !== this.gen || !this.running) return;
    const major = info ? parseInt(info.protocol, 10) : HUB_PROTOCOL_MAJOR;
    if (major !== HUB_PROTOCOL_MAJOR) {
      this.setState(false, `protocolo do hub incompatível (${info!.protocol}); esta UI fala ${HUB_PROTOCOL_MAJOR}.x`);
      return;
    }
    this.setState(true, null);
  }

  private onMessage(text: string) {
    let msg: any;
    try {
      msg = JSON.parse(text);
    } catch {
      return;
    }
    if (typeof msg.event === "string") {
      // Push de um ambiente pelo canal reverso: entregue como o do ambiente.
      if (msg.event === "env.push" && typeof msg.data?.env_id === "string") {
        const e: PushEvent = { env: msg.data.env_id, event: String(msg.data.event), data: msg.data.data };
        for (const cb of this.envPushSubs) cb(e);
        return;
      }
      if (msg.event === "env.online" && typeof msg.data?.env_id === "string") {
        const online = !!msg.data.online;
        const s: EnvStatus = {
          env: msg.data.env_id,
          connected: online,
          error: online ? null : OFFLINE,
          since: Date.now(),
        };
        for (const cb of this.envStatusSubs) cb(s);
      }
      const e: PushEvent = { env: HUB, event: msg.event, data: msg.data };
      for (const cb of this.pushSubs) cb(e);
      return;
    }
    const p = this.pending.get(msg.id);
    if (!p) return;
    this.pending.delete(msg.id);
    if (msg.error) p.reject(new Error(msg.error.message ?? "erro do hub"));
    else p.resolve(msg.result);
  }

  private send<T>(method: string, params: object): Promise<T> {
    if (!this.running) return Promise.reject(new Error("hub desconectado"));
    const id = ++this.next;
    const frame = JSON.stringify({ id, method, params });
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      if (this.ws && this.ws.readyState === 1) this.ws.send(frame);
      else this.queue.push(frame);
    });
  }

  // --- DaemonApi: hub ---

  hubCall<T>(method: string, params: object = {}): Promise<T> {
    return this.send<T>(method, params);
  }
  /** A conexão é fixa: a própria origem. */
  getHub(): Promise<HubConnView | null> {
    return Promise.resolve({ url: this.url, has_token: false });
  }
  saveHub(): Promise<HubConnView | null> {
    return unavailable("configurar o hub");
  }
  hubStatus(): Promise<EnvStatus | null> {
    return Promise.resolve(this.state);
  }
  onHubStatus(cb: (s: EnvStatus) => void): Unsubscribe {
    this.statusSubs.add(cb);
    return () => this.statusSubs.delete(cb);
  }
  onHubPush(cb: (e: PushEvent) => void): Unsubscribe {
    this.pushSubs.add(cb);
    return () => this.pushSubs.delete(cb);
  }

  // --- DaemonApi: ambientes, pelo hub ---

  async listEnvs(): Promise<EnvView[]> {
    return envViewsFromHub(await this.hubCall<HubEnv[]>("hub.envs.list"));
  }
  saveEnvs(): Promise<EnvView[]> {
    return unavailable("editar ambientes");
  }
  async statuses(): Promise<EnvStatus[]> {
    return envStatusesFromHub(await this.hubCall<HubEnv[]>("hub.envs.list"), Date.now());
  }
  /** Método do ambiente pelo canal reverso (`hub.env.call`, #343); fora da lista, recusa sem ir ao hub. */
  call<T>(env: string, method: string, params: object = {}): Promise<T> {
    if (!envMethodOnWeb(method)) return unavailable(method);
    return this.send<T>("hub.env.call", { env, method, params });
  }
  /** Pushes do ambiente (`env.push`), já filtrados pela lista do canal. */
  onPush(cb: (e: PushEvent) => void): Unsubscribe {
    this.envPushSubs.add(cb);
    return () => this.envPushSubs.delete(cb);
  }
  /** Canal do ambiente abriu ou fechou (`env.online`). */
  onStatus(cb: (s: EnvStatus) => void): Unsubscribe {
    this.envStatusSubs.add(cb);
    return () => this.envStatusSubs.delete(cb);
  }

  // --- Sem terminal, VS Code nem atualizador na web (spec §10.1) ---

  attach(): Promise<void> {
    return unavailable("terminal");
  }
  detach(): Promise<void> {
    return unavailable("terminal");
  }
  input(): Promise<void> {
    return unavailable("terminal");
  }
  resize(): Promise<void> {
    return unavailable("terminal");
  }
  openVsCode(): Promise<void> {
    return unavailable("VS Code");
  }
  openFolder(): Promise<void> {
    return unavailable("abrir pasta");
  }
  uiUpdateCheck(): Promise<UiUpdate> {
    return Promise.resolve({ current: "web", available: null, error: null });
  }
  uiUpdateInstall(): Promise<void> {
    return unavailable("atualizar o app");
  }
  onUiUpdateProgress(): Unsubscribe {
    return () => {};
  }

  openUrl(url: string): Promise<void> {
    window.open(url, "_blank", "noopener");
    return Promise.resolve();
  }
  readClipboard(): Promise<string> {
    return navigator.clipboard.readText();
  }
  writeClipboard(text: string): Promise<void> {
    return navigator.clipboard.writeText(text);
  }

  // Preferências da web no navegador (tema, telas), nada sensível.
  getUiConfig(): Promise<UiConfig> {
    try {
      const raw = localStorage.getItem(UI_CONFIG_KEY);
      if (raw) return Promise.resolve({ ...structuredClone(DEFAULT_UI_CONFIG), ...JSON.parse(raw) });
    } catch {
      // armazenamento indisponível: padrões
    }
    return Promise.resolve(structuredClone(DEFAULT_UI_CONFIG));
  }
  saveUiConfig(cfg: UiConfig): Promise<void> {
    try {
      localStorage.setItem(UI_CONFIG_KEY, JSON.stringify(cfg));
    } catch {
      // armazenamento indisponível: fica só na memória da página
    }
    return Promise.resolve();
  }
}
