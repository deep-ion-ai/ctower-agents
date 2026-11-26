import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { toApiError } from "./errors";
import type { DaemonMethods } from "../protocol/gen/methods";
import type { HubMethods } from "../protocol/hub-gen/methods";
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

/**
 * Argumentos de `call` para `M`: com `null` no registro (sem parâmetros), o
 * objeto é opcional; sem `null`, é obrigatório.
 */
export type MethodArgs<M extends keyof DaemonMethods> = null extends DaemonMethods[M]["params"]
  ? [params?: DaemonMethods[M]["params"]]
  : [params: DaemonMethods[M]["params"]];

/** Mesma regra de `MethodArgs`, para os métodos do hub (`HubMethods`). */
export type HubMethodArgs<M extends keyof HubMethods> = null extends HubMethods[M]["params"]
  ? [params?: HubMethods[M]["params"]]
  : [params: HubMethods[M]["params"]];

/** Tudo que a UI faz com os daemons passa por aqui (fácil de trocar nos testes). */
export interface DaemonApi {
  /**
   * Modo web (PWA servida pelo cthubd, épico #205): sem terminal, sem
   * conexão direta com os ambientes e sem os recursos exclusivos do desktop.
   * Ausente = app desktop.
   */
  readonly web?: boolean;
  listEnvs(): Promise<EnvView[]>;
  saveEnvs(envs: EnvInput[]): Promise<EnvView[]>;
  statuses(): Promise<EnvStatus[]>;
  /**
   * Requisição tipada ao daemon do ambiente: `method` é um método do registro
   * (`DaemonMethods`, gerado de `ct_http::methods`). Rejeita com a mensagem de erro.
   */
  call<M extends keyof DaemonMethods>(
    env: string,
    method: M,
    ...args: MethodArgs<M>
  ): Promise<DaemonMethods[M]["result"]>;
  /** Requisição sem tipo ao daemon do ambiente; rejeita com a mensagem de erro. */
  call<T = unknown>(env: string, method: string, params?: object): Promise<T>;
  /** Conexão com o hub (`null` = não configurada). */
  getHub(): Promise<HubConnView | null>;
  /** `null` desliga a conexão com o hub. */
  saveHub(hub: HubConnInput | null): Promise<HubConnView | null>;
  /** Estado da conexão com o hub (`null` = não configurada ou ainda conectando). */
  hubStatus(): Promise<EnvStatus | null>;
  /**
   * Requisição tipada ao `/ws` do hub: `method` é um método do registro do hub
   * (`HubMethods`, gerado de `crates/cthubd/src/methods.rs`). Rejeita com a mensagem de erro.
   */
  hubCall<M extends keyof HubMethods>(
    method: M,
    ...args: HubMethodArgs<M>
  ): Promise<HubMethods[M]["result"]>;
  /**
   * Requisição sem tipo ao `/ws` do hub (método fora do registro, ou com o tipo
   * dado por quem chama); rejeita com a mensagem de erro.
   */
  hubCall<T = unknown>(method: string, params?: object): Promise<T>;
  onHubStatus(cb: (s: EnvStatus) => void): Unsubscribe;
  onHubPush(cb: (e: PushEvent) => void): Unsubscribe;
  attach(env: string, session: string, onChunk: (c: TermChunk) => void): Promise<void>;
  detach(env: string, session: string): Promise<void>;
  input(env: string, session: string, data: string): Promise<void>;
  resize(env: string, session: string, cols: number, rows: number): Promise<void>;
  openUrl(url: string): Promise<void>;
  openVsCode(env: string, path: string): Promise<void>;
  /** Abre a pasta do ambiente (WSL) no gerenciador de arquivos do sistema (#444). */
  openFolder(env: string, path: string): Promise<void>;
  /** Área de transferência do sistema (plugin do Tauri). */
  readClipboard(): Promise<string>;
  writeClipboard(text: string): Promise<void>;
  getUiConfig(): Promise<UiConfig>;
  saveUiConfig(cfg: UiConfig): Promise<void>;
  onPush(cb: (e: PushEvent) => void): Unsubscribe;
  onStatus(cb: (s: EnvStatus) => void): Unsubscribe;
  /** Versão nova do app na última release (nunca rejeita: o erro vem no resultado). */
  uiUpdateCheck(): Promise<UiUpdate>;
  /** Baixa, instala e reinicia o app. */
  uiUpdateInstall(): Promise<void>;
  onUiUpdateProgress(cb: (p: UiUpdateProgress) => void): Unsubscribe;
  /**
   * Exportar (tela Logs, #65): no desktop grava na pasta Downloads e devolve
   * o caminho; na web, o navegador baixa o arquivo (`null`).
   */
  saveFile(name: string, content: string, mime: string): Promise<string | null>;
  /** Log da UI em arquivo (só no desktop, #65): acrescenta linhas JSON já mascaradas. */
  uiLogAppend?(lines: string[]): Promise<void>;
  /** Últimas linhas do log da UI em arquivo, inclusive da execução anterior. */
  uiLogRead?(): Promise<string[]>;
  /**
   * Notificação do sistema pedida pela própria UI (alerta do plano, #108).
   * Desktop: plugin de notificação do Tauri; web: a do navegador, se
   * permitida. Sem suporte, não faz nada.
   */
  notify?(title: string, body: string): Promise<void>;
}

/** Converte `listen` (assíncrono) num cancelamento síncrono. */
function subscribe<T>(event: string, cb: (payload: T) => void): Unsubscribe {
  let off: (() => void) | null = null;
  let cancelled = false;
  listen<T>(event, (e) => cb(e.payload)).then((u) => {
    if (cancelled) u();
    else off = u;
  });
  return () => {
    cancelled = true;
    off?.();
  };
}

/**
 * Métodos sensíveis do ambiente (#231) e o comando Tauri dedicado de cada
 * um. Espelho de `SENSITIVE_ENV` em `ui/src-tauri/src/gate.rs`; o teste
 * `sensitive.test.ts` confere os dois. Um `Map`, não um objeto: `method`
 * vem de fora e não pode achar `constructor` e afins.
 */
export const SENSITIVE_COMMANDS: ReadonlyMap<string, string> = new Map([
  ["github.set_token", "github_set_token"],
  ["github.clear_token", "github_clear_token"],
  ["github.pr.comment", "github_pr_comment"],
  ["env.update", "env_update"],
  ["env.rollback", "env_rollback"],
  ["env.config.set", "env_config_set"],
  ["env.credentials.set", "env_credentials_set"],
  ["env.connections.set", "env_connections_set"],
  ["shell.open", "shell_open"],
  ["shell.close", "shell_close"],
  ["session.input", "session_input"],
  ["session.permission.respond", "session_permission_respond"],
  ["cleanup.run", "cleanup_run"],
]);

/** `invoke` com a rejeição convertida em `ApiError`, igual à da web. */
function invokeApi<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return invoke<T>(cmd, args).catch((e: unknown) => Promise.reject(toApiError(e)));
}

/** Implementação real: comandos e eventos do lado Rust (src-tauri/src/main.rs). */
export class TauriDaemonApi implements DaemonApi {
  listEnvs() {
    return invokeApi<EnvView[]>("envs_list");
  }
  saveEnvs(envs: EnvInput[]) {
    return invokeApi<EnvView[]>("envs_save", { envs });
  }
  statuses() {
    return invokeApi<EnvStatus[]>("env_statuses");
  }
  call<T>(env: string, method: string, params: object = {}) {
    // Sensível (#231): só pelo comando dedicado; o genérico o recusa.
    const dedicated = SENSITIVE_COMMANDS.get(method);
    if (dedicated) return invokeApi<T>(dedicated, { env, params });
    return invokeApi<T>("daemon_call", { env, method, params });
  }
  getHub() {
    return invokeApi<HubConnView | null>("hub_get");
  }
  saveHub(hub: HubConnInput | null) {
    return invokeApi<HubConnView | null>("hub_save", { hub });
  }
  hubStatus() {
    return invokeApi<EnvStatus | null>("hub_status");
  }
  hubCall<T>(method: string, params: object = {}) {
    return invokeApi<T>("hub_call", { method, params });
  }
  onHubStatus(cb: (s: EnvStatus) => void) {
    return subscribe<EnvStatus>("hub://status", cb);
  }
  onHubPush(cb: (e: PushEvent) => void) {
    return subscribe<PushEvent>("hub://push", cb);
  }
  attach(env: string, session: string, onChunk: (c: TermChunk) => void) {
    // Cada mensagem: 1º byte = 1 se snapshot, 0 se ao vivo; o resto é a saída.
    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (buf) => {
      const all = new Uint8Array(buf);
      onChunk({ snapshot: all[0] === 1, bytes: all.subarray(1) });
    };
    return invokeApi<void>("term_attach", { env, session, channel });
  }
  detach(env: string, session: string) {
    return invokeApi<void>("term_detach", { env, session });
  }
  input(env: string, session: string, data: string) {
    return invokeApi<void>("term_input", { env, session, data });
  }
  resize(env: string, session: string, cols: number, rows: number) {
    return invokeApi<void>("term_resize", { env, session, cols, rows });
  }
  openUrl(url: string) {
    return invokeApi<void>("open_url", { url });
  }
  openVsCode(env: string, path: string) {
    return invokeApi<void>("open_vscode", { env, path });
  }
  openFolder(env: string, path: string) {
    return invokeApi<void>("open_folder", { env, path });
  }
  readClipboard() {
    return invokeApi<string>("clipboard_read");
  }
  writeClipboard(text: string) {
    return invokeApi<void>("clipboard_write", { text });
  }
  getUiConfig() {
    return invokeApi<UiConfig>("ui_config_get");
  }
  saveUiConfig(cfg: UiConfig) {
    return invokeApi<void>("ui_config_save", { cfg });
  }
  onPush(cb: (e: PushEvent) => void) {
    return subscribe<PushEvent>("daemon://push", cb);
  }
  onStatus(cb: (s: EnvStatus) => void) {
    return subscribe<EnvStatus>("daemon://status", cb);
  }
  uiUpdateCheck() {
    return invokeApi<UiUpdate>("ui_update_check");
  }
  uiUpdateInstall() {
    return invokeApi<void>("ui_update_install");
  }
  onUiUpdateProgress(cb: (p: UiUpdateProgress) => void) {
    return subscribe<UiUpdateProgress>("ui://update-progress", cb);
  }
  saveFile(name: string, content: string) {
    return invokeApi<string>("save_download", { name, content });
  }
  uiLogAppend(lines: string[]) {
    return invokeApi<void>("ui_log_append", { lines });
  }
  notify(title: string, body: string) {
    return invokeApi<void>("notify_send", { title, body });
  }
  uiLogRead() {
    return invokeApi<string[]>("ui_log_read");
  }
}
