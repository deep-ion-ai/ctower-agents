// Tipos da ponte Tauri (src-tauri/src/bridge.rs, envs.rs). Os do protocolo
// do daemon ficam em ../protocol/gen, gerados dos tipos Rust.

import type { SessionMode } from "../protocol/gen";

/** Ambiente como a UI o vê: o token nunca vem para o JavaScript. */
export interface EnvView {
  id: string;
  name: string;
  url: string;
  kind: "wsl" | "remote";
  /** Distro WSL (tipo `wsl`). */
  distro: string | null;
  /** Há token gravado (tipo `remote`). */
  has_token: boolean;
}

/** Edição de ambiente. `token` vazio/ausente mantém o atual; ignorado no tipo `wsl`. */
export interface EnvInput {
  id: string;
  name: string;
  url: string;
  kind: "wsl" | "remote";
  distro: string | null;
  token?: string | null;
}

/**
 * Conexão com o hub (`cthubd`), separada dos ambientes: URL do `/ws` do hub
 * e se há token de UI do hub gravado. O token nunca vem para o JavaScript.
 */
export interface HubConnView {
  url: string;
  has_token: boolean;
}

/** Edição da conexão com o hub. `token` vazio/ausente mantém o atual. */
export interface HubConnInput {
  url: string;
  token?: string | null;
}

export interface EnvStatus {
  env: string;
  connected: boolean;
  error: string | null;
  /** Desde quando (ms) está neste estado. */
  since: number;
}

/** Push do daemon (`{event, data}`), com o ambiente de origem. */
export interface PushEvent {
  env: string;
  event: string;
  data: any;
}

export interface Templates {
  issue: string;
  epic: string;
  /** Sessão do projeto, sem issue. */
  project: string;
  /** Gestor de projetos (tela Hub). */
  gestor: string;
  /** Projetos Jira + GitLab (#351): issue e sub-tarefa, épico e sessão do projeto. */
  jira_issue: string;
  jira_epic: string;
  jira_project: string;
}

export interface NotifyPrefs {
  needs_human: boolean;
  session_error: boolean;
  env_down: boolean;
  /** Ambientes (por `id`) silenciados (Configurações › Ambiente, #315); ausente = nenhum. */
  muted_envs?: string[];
}

/** Modelo, modo e Remote Control pré-selecionados ao iniciar uma sessão. */
export interface SessionDefaults {
  /** Alias do modelo; `null` = padrão do Claude Code. */
  model: string | null;
  mode: SessionMode;
  /** Pré-marca "Remote Control" na modal de início (#197); desligado por padrão. */
  remote_control: boolean;
}

/** Tema da UI (#290); `system` segue o `prefers-color-scheme`. O terminal é sempre escuro. */
export type ThemePref = "system" | "light" | "dark";

export interface Appearance {
  theme: ThemePref;
}

/** Preferências da UI (ui.json no diretório de dados do app). */
/**
 * Prompt pronto do console (#425; spec da #87, §2.4): nome no menu e texto
 * com as variáveis dos templates. `id` estável: o dos padrões
 * (`DEFAULT_PROMPTS`) permite o "Restaurar padrão" de cada um.
 */
export interface ReadyPrompt {
  id: string;
  name: string;
  text: string;
}

export interface UiConfig {
  templates: Templates;
  /** Prompts prontos do console, na ordem do menu (#425). */
  prompts: ReadyPrompt[];
  notify: NotifyPrefs;
  session: SessionDefaults;
  appearance: Appearance;
}

/** Verificação de versão nova do app (src-tauri/src/updater.rs). */
export interface UiUpdate {
  current: string;
  /** Versão nova; `null` = em dia. */
  available: { version: string; notes: string | null; date: string | null } | null;
  error: string | null;
}

/** Progresso do download da versão nova do app. */
export interface UiUpdateProgress {
  downloaded: number;
  total: number | null;
}

export type Unsubscribe = () => void;

/** Saída de terminal: `snapshot` = scrollback do attach (limpar antes). */
export type TermChunk = { bytes: Uint8Array; snapshot: boolean };
