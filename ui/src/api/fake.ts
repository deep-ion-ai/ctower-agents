import type { DaemonApi } from "./daemon";
import type {
  EnvInput,
  EnvStatus,
  EnvView,
  HubConnInput,
  HubConnView,
  PushEvent,
  TermChunk,
  UiConfig,
  UiUpdate,
  UiUpdateProgress,
  Unsubscribe,
} from "./types";
import { DEFAULT_UI_CONFIG } from "../settings/defaults";
import { envMethodOnWeb, type WebAuth, type WebMe } from "./web";

type Handler = (params: any, env: string) => unknown;

/** `env` das chamadas ao hub registradas em `calls`. */
export const HUB = "hub";

/** DaemonApi em memória para os testes: respostas por método e registro das chamadas. */
export class FakeDaemonApi implements DaemonApi {
  /**
   * Modo web (`asWeb`): ambientes só pelo hub; `call` aceita só os métodos
   * da lista do canal (como `hub.env.call`) e recusa ambiente em `offline`.
   */
  web = false;
  /** Web: ambientes sem canal com o hub (`hub.env.call` responde "ambiente offline"). */
  offline = new Set<string>();
  envs: EnvView[];
  status: Record<string, EnvStatus> = {};
  handlers: Record<string, Handler> = {};
  calls: { env: string; method: string; params: any }[] = [];
  inputs: { env: string; session: string; data: string }[] = [];
  resizes: { session: string; cols: number; rows: number }[] = [];
  opened: string[] = [];
  vscode: { env: string; path: string }[] = [];
  folders: { env: string; path: string }[] = [];
  clipboard = "";
  attached = new Map<string, (c: TermChunk) => void>();
  uiConfig: UiConfig = structuredClone(DEFAULT_UI_CONFIG);
  savedEnvs: EnvInput[][] = [];
  uiUpdate: UiUpdate = { current: "0.1.0", available: null, error: null };
  uiInstalls = 0;
  /** Conexão com o hub (`null` = não configurada). */
  hub: HubConnView | null = null;
  hubState: EnvStatus | null = null;
  /** Respostas do `/ws` do hub, à parte das do ambiente (`github.*` existe nos dois). */
  hubHandlers: Record<string, Handler> = {};
  savedHubs: (HubConnInput | null)[] = [];
  private hubStatusSubs = new Set<(s: EnvStatus) => void>();
  private hubPushSubs = new Set<(e: PushEvent) => void>();
  private progressSubs = new Set<(p: UiUpdateProgress) => void>();
  private pushSubs = new Set<(e: PushEvent) => void>();
  private statusSubs = new Set<(s: EnvStatus) => void>();

  constructor(envs: EnvView[] = [FakeDaemonApi.wslEnv()]) {
    this.envs = envs;
    for (const e of envs) {
      this.status[e.id] = { env: e.id, connected: true, error: null, since: 0 };
    }
  }

  static wslEnv(id = "wsl"): EnvView {
    return {
      id,
      name: "ctower-agents (WSL)",
      url: "ws://127.0.0.1:7878/ws",
      kind: "wsl",
      distro: "ctower-agents",
      has_token: false,
    };
  }

  /** Define a resposta de um método (valor ou função dos params). */
  on(method: string, h: Handler | unknown): this {
    this.handlers[method] = typeof h === "function" ? (h as Handler) : () => h;
    return this;
  }

  /**
   * Modo web (épico #205): como a `WebDaemonApi`, sem conexão direta com os
   * ambientes. A UI monta os ambientes pelo hub (`hub.envs.list`); os
   * métodos de ambiente da lista do canal respondem pelos `on` (o
   * `hub.env.call` da fase 2), os demais rejeitam. Já conecta o hub (a
   * própria origem).
   */
  asWeb(): this {
    this.web = true;
    this.envs = [];
    this.status = {};
    return this.withHub("ws://hub.local/web/ws");
  }

  /** Configura a conexão com o hub, já conectada. */
  withHub(url = "ws://mac-mini:7879/ws", connected = true): this {
    this.hub = { url, has_token: true };
    this.hubState = { env: HUB, connected, error: connected ? null : "hub inacessível", since: 0 };
    return this;
  }

  /** Resposta de um método do `/ws` do hub (registrada com env `hub`). */
  onHub(method: string, h: Handler | unknown): this {
    this.hubHandlers[method] = typeof h === "function" ? (h as Handler) : () => h;
    return this;
  }

  setHubStatus(connected: boolean, error: string | null = null) {
    const s = { env: HUB, connected, error, since: Date.now() };
    this.hubState = s;
    for (const cb of this.hubStatusSubs) cb(s);
  }

  hubPush(event: string, data: any = {}) {
    for (const cb of this.hubPushSubs) cb({ env: HUB, event, data });
  }

  /** Chamadas ao hub. */
  hubCallsTo(method: string) {
    return this.calls.filter((c) => c.env === HUB && c.method === method);
  }

  push(env: string, event: string, data: any = {}) {
    for (const cb of this.pushSubs) cb({ env, event, data });
  }

  setStatus(env: string, connected: boolean, error: string | null = null) {
    const s = { env, connected, error, since: Date.now() };
    this.status[env] = s;
    for (const cb of this.statusSubs) cb(s);
  }

  /** Simula saída de terminal para uma sessão anexada. */
  output(session: string, text: string, snapshot = false) {
    this.attached.get(session)?.({ bytes: new TextEncoder().encode(text), snapshot });
  }

  callsTo(method: string) {
    return this.calls.filter((c) => c.method === method);
  }

  async listEnvs() {
    return this.envs;
  }
  async saveEnvs(envs: EnvInput[]) {
    this.savedEnvs.push(envs);
    this.envs = envs.map((e) => ({
      id: e.id,
      name: e.name,
      url: e.url,
      kind: e.kind,
      distro: e.distro,
      has_token: e.kind === "remote" && !!e.token,
    }));
    return this.envs;
  }
  async statuses() {
    return Object.values(this.status);
  }
  async call<T>(env: string, method: string, params: object = {}): Promise<T> {
    this.calls.push({ env, method, params });
    if (this.web && !envMethodOnWeb(method)) throw new Error(`${method} indisponível na web`);
    if (this.web && this.offline.has(env)) throw new Error("ambiente offline: sem canal com o hub agora");
    const h = this.handlers[method];
    if (!h) throw new Error(`método sem resposta no fake: ${method}`);
    return (await h(params, env)) as T;
  }
  async getHub() {
    return this.hub;
  }
  async saveHub(hub: HubConnInput | null) {
    this.savedHubs.push(hub);
    if (!hub) {
      this.hub = null;
      this.hubState = null;
      return null;
    }
    this.hub = { url: hub.url, has_token: !!hub.token || !!this.hub?.has_token };
    return this.hub;
  }
  async hubStatus() {
    return this.hubState;
  }
  async hubCall<T>(method: string, params: object = {}): Promise<T> {
    this.calls.push({ env: HUB, method, params });
    if (!this.hub || !this.hubState?.connected) throw new Error("hub desconectado");
    const h = this.hubHandlers[method];
    if (!h) throw new Error(`método do hub sem resposta no fake: ${method}`);
    return (await h(params, HUB)) as T;
  }
  onHubStatus(cb: (s: EnvStatus) => void): Unsubscribe {
    this.hubStatusSubs.add(cb);
    return () => this.hubStatusSubs.delete(cb);
  }
  onHubPush(cb: (e: PushEvent) => void): Unsubscribe {
    this.hubPushSubs.add(cb);
    return () => this.hubPushSubs.delete(cb);
  }
  async attach(_env: string, session: string, onChunk: (c: TermChunk) => void) {
    this.attached.set(session, onChunk);
  }
  async detach(_env: string, session: string) {
    this.attached.delete(session);
  }
  async input(env: string, session: string, data: string) {
    this.inputs.push({ env, session, data });
  }
  async resize(_env: string, session: string, cols: number, rows: number) {
    this.resizes.push({ session, cols, rows });
  }
  async openUrl(url: string) {
    this.opened.push(url);
  }
  async openVsCode(env: string, path: string) {
    this.vscode.push({ env, path });
  }
  async openFolder(env: string, path: string) {
    this.folders.push({ env, path });
  }
  async readClipboard() {
    return this.clipboard;
  }
  async writeClipboard(text: string) {
    this.clipboard = text;
  }
  async getUiConfig() {
    return structuredClone(this.uiConfig);
  }
  async saveUiConfig(cfg: UiConfig) {
    this.uiConfig = structuredClone(cfg);
  }
  onPush(cb: (e: PushEvent) => void): Unsubscribe {
    this.pushSubs.add(cb);
    return () => this.pushSubs.delete(cb);
  }
  onStatus(cb: (s: EnvStatus) => void): Unsubscribe {
    this.statusSubs.add(cb);
    return () => this.statusSubs.delete(cb);
  }
  async uiUpdateCheck() {
    return structuredClone(this.uiUpdate);
  }
  async uiUpdateInstall() {
    this.uiInstalls++;
  }
  /** Simula progresso do download da versão nova do app. */
  uiProgress(p: UiUpdateProgress) {
    for (const cb of this.progressSubs) cb(p);
  }
  onUiUpdateProgress(cb: (p: UiUpdateProgress) => void): Unsubscribe {
    this.progressSubs.add(cb);
    return () => this.progressSubs.delete(cb);
  }
}

/** Login web em memória para os testes do modo web (`WebGate`). */
export class FakeWebAuth implements WebAuth {
  /** Token que o "hub" aceita. */
  token = "tok-ui";
  session: WebMe = { authenticated: false, identity: null, expires_at: null };
  /** Tokens tentados (o teste confere que a UI não guarda o token em lugar nenhum). */
  attempts: string[] = [];
  logouts = 0;
  starts = 0;
  /** Próximo login recusado com esta mensagem. */
  refuse: string | null = null;
  private outSubs = new Set<() => void>();

  signedIn(identity: string | null = "eu@exemplo.com"): this {
    this.session = { authenticated: true, identity, expires_at: Date.now() + 30 * 86_400_000 };
    return this;
  }
  async me() {
    return { ...this.session };
  }
  async login(token: string) {
    this.attempts.push(token);
    if (this.refuse) throw new Error(this.refuse);
    if (token !== this.token) throw new Error("Token inválido.");
    this.signedIn();
  }
  async logout() {
    this.logouts++;
    this.session = { authenticated: false, identity: null, expires_at: null };
    for (const cb of this.outSubs) cb();
  }
  start() {
    this.starts++;
  }
  onLoggedOut(cb: () => void) {
    this.outSubs.add(cb);
    return () => this.outSubs.delete(cb);
  }
  /** O hub fechou o `/web/ws` com 4401 (sessão revogada ou vencida). */
  revoke() {
    this.session = { authenticated: false, identity: null, expires_at: null };
    for (const cb of this.outSubs) cb();
  }
}
