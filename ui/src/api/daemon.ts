import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
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
  /** Requisição ao daemon do ambiente; rejeita com a mensagem de erro. */
  call<T = unknown>(env: string, method: string, params?: object): Promise<T>;
  /** Conexão com o hub (`null` = não configurada). */
  getHub(): Promise<HubConnView | null>;
  /** `null` desliga a conexão com o hub. */
  saveHub(hub: HubConnInput | null): Promise<HubConnView | null>;
  /** Estado da conexão com o hub (`null` = não configurada ou ainda conectando). */
  hubStatus(): Promise<EnvStatus | null>;
  /** Requisição ao `/ws` do hub; rejeita com a mensagem de erro. */
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

/** Implementação real: comandos e eventos do lado Rust (src-tauri/src/main.rs). */
export class TauriDaemonApi implements DaemonApi {
  listEnvs() {
    return invoke<EnvView[]>("envs_list");
  }
  saveEnvs(envs: EnvInput[]) {
    return invoke<EnvView[]>("envs_save", { envs });
  }
  statuses() {
    return invoke<EnvStatus[]>("env_statuses");
  }
  call<T>(env: string, method: string, params: object = {}) {
    return invoke<T>("daemon_call", { env, method, params });
  }
  getHub() {
    return invoke<HubConnView | null>("hub_get");
  }
  saveHub(hub: HubConnInput | null) {
    return invoke<HubConnView | null>("hub_save", { hub });
  }
  hubStatus() {
    return invoke<EnvStatus | null>("hub_status");
  }
  hubCall<T>(method: string, params: object = {}) {
    return invoke<T>("hub_call", { method, params });
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
    return invoke<void>("term_attach", { env, session, channel });
  }
  detach(env: string, session: string) {
    return invoke<void>("term_detach", { env, session });
  }
  input(env: string, session: string, data: string) {
    return invoke<void>("term_input", { env, session, data });
  }
  resize(env: string, session: string, cols: number, rows: number) {
    return invoke<void>("term_resize", { env, session, cols, rows });
  }
  openUrl(url: string) {
    return invoke<void>("open_url", { url });
  }
  openVsCode(env: string, path: string) {
    return invoke<void>("open_vscode", { env, path });
  }
  openFolder(env: string, path: string) {
    return invoke<void>("open_folder", { env, path });
  }
  readClipboard() {
    return invoke<string>("clipboard_read");
  }
  writeClipboard(text: string) {
    return invoke<void>("clipboard_write", { text });
  }
  getUiConfig() {
    return invoke<UiConfig>("ui_config_get");
  }
  saveUiConfig(cfg: UiConfig) {
    return invoke<void>("ui_config_save", { cfg });
  }
  onPush(cb: (e: PushEvent) => void) {
    return subscribe<PushEvent>("daemon://push", cb);
  }
  onStatus(cb: (s: EnvStatus) => void) {
    return subscribe<EnvStatus>("daemon://status", cb);
  }
  uiUpdateCheck() {
    return invoke<UiUpdate>("ui_update_check");
  }
  uiUpdateInstall() {
    return invoke<void>("ui_update_install");
  }
  onUiUpdateProgress(cb: (p: UiUpdateProgress) => void) {
    return subscribe<UiUpdateProgress>("ui://update-progress", cb);
  }
  saveFile(name: string, content: string) {
    return invoke<string>("save_download", { name, content });
  }
  uiLogAppend(lines: string[]) {
    return invoke<void>("ui_log_append", { lines });
  }
  uiLogRead() {
    return invoke<string[]>("ui_log_read");
  }
}
