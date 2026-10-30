import "./settings.css";
import { useEffect, useRef, useState, type ReactNode } from "react";
import type { NotifyPrefs, ThemePref, UiConfig } from "../api/types";
import { hubTab } from "../hub/sections";
import { maintenanceTab } from "../maintenance/sections";
import type { SessionMode } from "../protocol/gen";
import { useEnvChoice } from "../state/env-choice";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { useTermPrefs } from "../terminal/prefs";
import { WEB_SETTINGS } from "../web/mode";
import { useWebSession } from "../web/WebGate";
import { CLEANUP_KEYS, CleanupRules } from "./CleanupRules";
import { DEFAULT_PROMPTS, DEFAULT_UI_CONFIG } from "./defaults";
import { promptsChanged, ReadyPromptsControl } from "./ReadyPrompts";
import {
  RollbackControl,
  SyncIntervalControl,
  UpdateRefControl,
  configChanged,
  useEnvConfig,
} from "./EnvConfigSettings";
import { EnvsSettings } from "./EnvsSettings";
import { ENV_TOKEN_HINT, GithubSettings } from "./GithubSettings";
import { HubSettings } from "./HubSettings";
import { ProjectsSettings } from "./ProjectsSettings";
import {
  SCOPE_SECTIONS,
  SECTION_LABEL,
  TEMPLATE_KEYS,
  TEMPLATE_LABEL,
  matchesSetting,
  scopeOf,
  settingsTab,
  templateTab,
  type Searchable,
  type SettingsScope,
  type SettingsScreen as SettingsAddress,
  type SettingsSection,
  type TemplateKey,
} from "./sections";
import { CLAUDE_MODELS, DEFAULT_MODEL_VALUE, SESSION_MODES } from "./session-options";
import {
  TermCloseControl,
  TermFontControl,
  TermSizeControl,
  restoreTermClose,
  restoreTermFont,
  restoreTermSize,
  termCloseChanged,
  termFontChanged,
  termSizeChanged,
} from "./TerminalSettings";

/**
 * Uma configuração do editor (#315): "Categoria: **Nome**", descrição,
 * controle e o marcador de alterada (diferente do padrão), com "Restaurar
 * padrão" quando o padrão é conhecido e gravável daqui.
 */
interface Item extends Searchable {
  id: string;
  section: SettingsSection;
  /** Entrada do índice abaixo da seção (padrão: a categoria). */
  toc?: string;
  /** Chave mostrada ao lado do título (ex. `update_ref`). */
  shownKey?: string;
  /** Descrição com marcação; a busca usa `description`. */
  desc?: ReactNode;
  control: ReactNode;
  modified?: boolean;
  restore?: () => void;
}

/** Configurações (#315): editor com busca, índice e escopos; ou o editor de um template (aba própria). */
export function SettingsScreen({ screen = { name: "settings" } }: { screen?: SettingsAddress }) {
  if (screen.template) return <TemplateEditor k={screen.template} />;
  return <SettingsEditor screen={screen} />;
}

function SettingsEditor({ screen }: { screen: SettingsAddress }) {
  const nav = useNav();
  const { envs, web } = useTower();
  const [query, setQuery] = useState("");
  const [tocActive, setTocActive] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);

  // Web (épico #205 §10.2): só as preferências deste navegador e o acesso
  // web; sem o escopo Ambiente (só leitura do daemon) nem o resto de "Este app".
  const scope: SettingsScope = web ? "app" : (screen.scope ?? (screen.section ? scopeOf(screen.section) : "app"));
  const chosen = useEnvChoice(screen.env);
  const env = web ? undefined : chosen;
  const envName = envs.find((e) => e.id === env)?.name;

  const allAppItems = useAppItems();
  const appItems = web ? allAppItems.filter((i) => WEB_SETTINGS.has(i.section)) : allAppItems;
  const envItems = useEnvItems(env);
  const items = scope === "app" ? appItems : envItems;
  const shown = items.filter((i) => matchesSetting(i, query));
  const appHits = appItems.filter((i) => matchesSetting(i, query)).length;
  const envHits = envItems.filter((i) => matchesSetting(i, query)).length;
  const modified = [...appItems, ...envItems].filter((i) => i.modified).length;

  // O escopo e o ambiente ficam no endereço da aba (só a aba ativa fica montada).
  const go = (to: { scope: SettingsScope; env?: string }) =>
    nav.openTab({ ...settingsTab({ scope: to.scope, env: to.env ?? env }), id: nav.current });

  // Cada pedido de seção (objeto novo, ex. pela paleta) rola até ela.
  useEffect(() => {
    if (screen.section) scrollTo(listRef.current, `set-sec-${screen.section}`);
  }, [screen]);

  const sections = SCOPE_SECTIONS[scope]
    .map((s) => ({ s, items: shown.filter((i) => i.section === s) }))
    .filter((x) => x.items.length > 0);
  const other = scope === "app" ? envHits : appHits;

  return (
    <main className="settings">
      <h1 className="sr-only">Configurações</h1>
      <div className="set-head">
        <div className="set-search">
          <input
            type="search"
            aria-label="Buscar configurações"
            placeholder="Buscar configurações (ex.: modelo, fonte, token, update_ref)"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <span className="muted set-count" role="status">
            {modified === 1 ? "1 alterada" : `${modified} alteradas`}
          </span>
        </div>
        {!web && (
          <div className="set-scopes">
            <div role="tablist" aria-label="Escopo" className="set-scope-tabs">
              <button
                type="button"
                role="tab"
                id="set-scope-app"
                aria-selected={scope === "app"}
                aria-controls="set-scope-panel"
                className="set-scope"
                onClick={() => go({ scope: "app" })}
              >
                Este app{query && ` (${appHits})`}
              </button>
              <button
                type="button"
                role="tab"
                id="set-scope-env"
                aria-selected={scope === "env"}
                aria-controls="set-scope-panel"
                className="set-scope"
                disabled={!env}
                onClick={() => go({ scope: "env" })}
              >
                Ambiente{envName ? `: ${envName}` : ""}
                {query && ` (${envHits})`}
              </button>
            </div>
            {envs.length > 1 && (
              <select
                aria-label="Ambiente das configurações"
                className="set-env-select"
                value={env}
                onChange={(e) => go({ scope: "env", env: e.target.value })}
              >
                {envs.map((e) => (
                  <option key={e.id} value={e.id}>
                    {e.name}
                  </option>
                ))}
              </select>
            )}
            <button type="button" className="link-btn set-hub-link" onClick={() => nav.openTab(hubTab("projects"))}>
              Projetos e secrets ficam no Hub ›
            </button>
          </div>
        )}
      </div>
      <div
        className="set-body"
        role="tabpanel"
        id="set-scope-panel"
        aria-labelledby={scope === "app" ? "set-scope-app" : "set-scope-env"}
      >
        <nav className="set-toc" aria-label="Índice das configurações">
          {sections.map(({ s, items: list }) => {
            const subs = [...new Set(list.map((i) => i.toc ?? i.category))];
            return (
              <div key={s}>
                <button
                  type="button"
                  className="set-toc-item"
                  aria-current={tocActive === s ? "true" : undefined}
                  onClick={() => {
                    setTocActive(s);
                    scrollTo(listRef.current, `set-sec-${s}`);
                  }}
                >
                  {SECTION_LABEL[s]}
                </button>
                {subs.length > 1 &&
                  subs.map((t) => (
                    <button
                      key={t}
                      type="button"
                      className="set-toc-item set-toc-sub"
                      aria-current={tocActive === `${s}:${t}` ? "true" : undefined}
                      onClick={() => {
                        setTocActive(`${s}:${t}`);
                        scrollTo(listRef.current, `set-${list.find((i) => (i.toc ?? i.category) === t)!.id}`);
                      }}
                    >
                      {t}
                    </button>
                  ))}
              </div>
            );
          })}
        </nav>
        <div className="set-list" ref={listRef}>
          {scope === "env" && env && <EnvIntro env={env} />}
          {sections.map(({ s, items: list }) => (
            <div key={s} className="set-group">
              <h2 className="set-cat" id={`set-sec-${s}`}>
                {SECTION_LABEL[s]}
              </h2>
              {list.map((i) => (
                <SettingItem key={i.id} item={i} />
              ))}
            </div>
          ))}
          {sections.length === 0 && (
            <div className="set-empty">
              <p className="muted">
                {query ? `Nenhuma configuração encontrada para "${query}" neste escopo.` : "Nenhum ambiente configurado."}
              </p>
              {!web && query && other > 0 && (
                <button type="button" className="btn" onClick={() => go({ scope: scope === "app" ? "env" : "app" })}>
                  Ver {other} em {scope === "app" ? `Ambiente${envName ? `: ${envName}` : ""}` : "Este app"}
                </button>
              )}
            </div>
          )}
        </div>
      </div>
    </main>
  );
}

/**
 * Rola só o painel de conteúdo até o alvo (#431). `scrollIntoView` rolaria
 * também os ancestrais (até com `overflow: hidden`) e deslocaria o app inteiro
 * quando a seção fica perto do fim e não dá para alinhá-la ao topo.
 */
function scrollTo(list: HTMLElement | null, id: string) {
  const target = document.getElementById(id);
  if (!list || !target) return;
  const top = target.getBoundingClientRect().top - list.getBoundingClientRect().top + list.scrollTop;
  list.scrollTo?.({ top, behavior: "smooth" });
}

function SettingItem({ item }: { item: Item }) {
  const tid = `set-${item.id}-t`;
  return (
    <section className="set" id={`set-${item.id}`} aria-labelledby={tid} data-modified={item.modified || undefined}>
      <div className="set-mod">{item.modified && <span className="set-dot" title="Alterada" />}</div>
      <div className="set-b">
        <div className="set-t">
          <span id={tid}>
            {item.category}: <b>{item.name}</b>
          </span>
          {item.shownKey && <span className="mono set-key">{item.shownKey}</span>}
          {item.modified && <span className="set-flag">alterada</span>}
        </div>
        <div className="set-d">{item.desc ?? item.description}</div>
        <div className="set-c">{item.control}</div>
        {item.modified && item.restore && (
          <button type="button" className="link-btn set-restore" onClick={item.restore}>
            Restaurar padrão<span className="sr-only"> de {item.name}</span>
          </button>
        )}
      </div>
    </section>
  );
}

/** Daemon do ambiente: versão, protocolo e máquina; e de onde vêm estas configurações. */
function EnvIntro({ env }: { env: string }) {
  const { envs, infos, status } = useTower();
  const name = envs.find((e) => e.id === env)?.name ?? env;
  const i = infos[env];
  return (
    <div className="set-intro">
      <p className="muted">
        Configurações do <span className="mono">ctowerd</span> de {name}, no <span className="mono">config.toml</span>{" "}
        dele: valem para todas as UIs ligadas ao ambiente. As notificações do ambiente ficam neste app.
      </p>
      {status[env]?.connected && i ? (
        <p className="muted hint">
          daemon {i.version} · protocolo {i.protocol} · {i.hostname}
        </p>
      ) : (
        <p className="state-off hint">sem conexão</p>
      )}
    </div>
  );
}

const THEMES: { value: ThemePref; label: string }[] = [
  { value: "system", label: "Seguir o sistema" },
  { value: "light", label: "Claro" },
  { value: "dark", label: "Escuro" },
];

const NOTIFS: { key: Exclude<keyof NotifyPrefs, "muted_envs">; label: string; hint: string }[] = [
  { key: "needs_human", label: "Sessão aguardando você", hint: "Permissão ou pergunta pendente" },
  { key: "session_error", label: "Sessão saiu com erro", hint: "Processo terminou com código diferente de 0" },
  { key: "env_down", label: "Ambiente desconectado", hint: "Daemon fora do ar ou rede caiu" },
];

/** Sessão web deste navegador: identidade, validade, sair e a lista no Hub (só na web). */
function WebAccess() {
  const session = useWebSession();
  const nav = useNav();
  const [error, setError] = useState<string | null>(null);
  if (!session) return null;
  return (
    <div className="set-checks">
      <p className="muted">
        {session.identity
          ? `Identidade do proxy: ${session.identity}. `
          : "Sem proxy (Cloudflare Access ou Tailscale). "}
        {session.expiresAt ? `Vale até ${new Date(session.expiresAt).toLocaleString("pt-BR")}.` : ""}
      </p>
      <div className="form-actions">
        <button
          type="button"
          className="btn"
          onClick={() => {
            setError(null);
            session.logout().catch((e) => setError(e instanceof Error ? e.message : String(e)));
          }}
        >
          Sair
        </button>
        <button type="button" className="btn" onClick={() => nav.openTab(hubTab("web-sessions"))}>
          Ver sessões web no Hub
        </button>
      </div>
      {error && (
        <p role="alert" className="start-error">
          {error}
        </p>
      )}
    </div>
  );
}

/** Configurações deste app (ui.json e, as do terminal, localStorage desta máquina). */
function useAppItems(): Item[] {
  const webSession = useWebSession();
  const { config, setConfig } = useTower();
  const nav = useNav();
  const term = useTermPrefs();
  const save = (patch: Partial<UiConfig>) => void setConfig({ ...config, ...patch });
  const session = (patch: Partial<UiConfig["session"]>) => save({ session: { ...config.session, ...patch } });
  const notifyOff = NOTIFS.some((n) => !config.notify[n.key]);

  return [
    {
      id: "hub",
      section: "connections",
      category: "Hub",
      name: "Conexão",
      toc: "Hub",
      description: "Endereço e token de UI do hub (cthubd). Projetos, atribuições e a visão consolidada vêm dele.",
      keys: ["hub.url", "hub.token"],
      keywords: "url token cthubd 7879 ui.token",
      control: <HubSettings />,
    },
    {
      id: "envs",
      section: "connections",
      category: "Conexões",
      name: "Ambientes",
      toc: "Ambientes",
      description: "Daemons (ctowerd) que este app acompanha: nome, URL, token de UI e estado da conexão.",
      keywords: "ws url token distro wsl remoto daemon ctowerd adicionar editar remover",
      control: <EnvsSettings />,
    },
    {
      id: "session-model",
      section: "sessions",
      category: "Início de sessão",
      name: "Modelo padrão",
      description: "Modelo do Claude Code pré-selecionado ao iniciar uma sessão; dá para trocar a cada início.",
      keys: ["session.model"],
      keywords: "fable opus sonnet haiku",
      shownKey: "session.model",
      modified: config.session.model !== DEFAULT_UI_CONFIG.session.model,
      restore: () => session({ model: DEFAULT_UI_CONFIG.session.model }),
      control: (
        <select
          id="padrao-modelo"
          aria-label="Modelo padrão"
          className="set-input set-select"
          value={config.session.model ?? DEFAULT_MODEL_VALUE}
          onChange={(e) => session({ model: e.target.value || null })}
        >
          <option value={DEFAULT_MODEL_VALUE}>Padrão do Claude Code</option>
          {CLAUDE_MODELS.map((m) => (
            <option key={m.value} value={m.value}>
              {m.label}
            </option>
          ))}
        </select>
      ),
    },
    {
      id: "session-mode",
      section: "sessions",
      category: "Início de sessão",
      name: "Modo padrão",
      description: "native conversa direto; subagents delega cada tarefa a subagentes.",
      keys: ["session.mode"],
      shownKey: "session.mode",
      modified: config.session.mode !== DEFAULT_UI_CONFIG.session.mode,
      restore: () => session({ mode: DEFAULT_UI_CONFIG.session.mode }),
      control: (
        <select
          id="padrao-modo"
          aria-label="Modo padrão"
          className="set-input set-select"
          value={config.session.mode}
          onChange={(e) => session({ mode: e.target.value as SessionMode })}
        >
          {SESSION_MODES.map((m) => (
            <option key={m.value} value={m.value}>
              {m.label} ({m.hint})
            </option>
          ))}
        </select>
      ),
    },
    {
      id: "session-remote-control",
      section: "sessions",
      category: "Início de sessão",
      name: "Remote Control",
      description:
        "Pré-marca o Remote Control do Claude Code na modal de início, para continuar a sessão pelo celular. Exige o login do claude.ai no ambiente.",
      keys: ["session.remote_control"],
      keywords: "celular claude.ai code remote control qr",
      shownKey: "session.remote_control",
      modified: config.session.remote_control !== DEFAULT_UI_CONFIG.session.remote_control,
      restore: () => session({ remote_control: DEFAULT_UI_CONFIG.session.remote_control }),
      control: (
        <div className="notif-row">
          <input
            type="checkbox"
            id="padrao-remote-control"
            checked={config.session.remote_control}
            onChange={(e) => session({ remote_control: e.target.checked })}
          />
          <label htmlFor="padrao-remote-control">
            <span>Ligar o Remote Control ao iniciar</span>
          </label>
        </div>
      ),
    },
    ...TEMPLATE_KEYS.map(
      (k): Item => ({
        id: `template-${k}`,
        section: "sessions",
        category: "Templates",
        name: TEMPLATE_LABEL[k],
        toc: "Templates de prompt",
        description: `Prompt inicial da sessão (${TEMPLATE_LABEL[k].toLowerCase()}). Edite em aba própria.`,
        keys: [`templates.${k}`],
        keywords: "prompt inicial variáveis",
        shownKey: `templates.${k}`,
        modified: config.templates[k] !== DEFAULT_UI_CONFIG.templates[k],
        restore: () => save({ templates: { ...config.templates, [k]: DEFAULT_UI_CONFIG.templates[k] } }),
        control: (
          <div className="set-inline">
            <button
              type="button"
              className="btn"
              aria-label={`Editar o template ${TEMPLATE_LABEL[k]} em aba própria`}
              onClick={() => nav.openTab(templateTab(k))}
            >
              Editar em aba própria
            </button>
            <span className="mono muted set-preview">{config.templates[k].split("\n")[0]}</span>
          </div>
        ),
      }),
    ),
    {
      id: "prompts",
      section: "sessions",
      category: "Prompts prontos",
      name: "Menu do console",
      toc: "Prompts prontos",
      description:
        "Prompts que o botão \"Enviar prompt pronto\" do cabeçalho da sessão (e a paleta) manda ao agente, com as variáveis dos templates.",
      keys: ["prompts"],
      keywords: "prompt pronto continuar testes ledger pr resumo console enviar",
      shownKey: "prompts",
      modified: promptsChanged(config.prompts),
      restore: () => save({ prompts: DEFAULT_PROMPTS }),
      control: <ReadyPromptsControl />,
    },
    {
      id: "theme",
      section: "appearance",
      category: "Aparência",
      name: "Tema",
      description:
        "Cores da interface: claro, escuro ou as do sistema (muda junto com ele). O terminal fica escuro em todos.",
      keys: ["appearance.theme"],
      keywords: "claro escuro sistema dark light modo noturno",
      shownKey: "appearance.theme",
      modified: config.appearance.theme !== DEFAULT_UI_CONFIG.appearance.theme,
      restore: () => save({ appearance: DEFAULT_UI_CONFIG.appearance }),
      control: (
        <select
          id="tema"
          aria-label="Tema"
          className="set-input set-select"
          value={config.appearance.theme}
          onChange={(e) => save({ appearance: { theme: e.target.value as ThemePref } })}
        >
          {THEMES.map((t) => (
            <option key={t.value} value={t.value}>
              {t.label}
            </option>
          ))}
        </select>
      ),
    },
    {
      id: "term-font",
      section: "terminal",
      category: "Terminal",
      name: "Fonte",
      description: "Lista de fontes do CSS, da preferida para as alternativas. Salva nesta máquina.",
      keywords: "font family console",
      modified: termFontChanged(term),
      restore: restoreTermFont,
      control: <TermFontControl />,
    },
    {
      id: "term-size",
      section: "terminal",
      category: "Terminal",
      name: "Tamanho da fonte",
      description:
        "Em px, nesta máquina. Ctrl+= e Ctrl+- fazem o zoom em todas as abas; Ctrl+0 volta ao tamanho escolhido aqui.",
      keywords: "font size zoom console",
      modified: termSizeChanged(term),
      restore: restoreTermSize,
      control: <TermSizeControl />,
    },
    {
      id: "term-close-on-exit",
      section: "terminal",
      category: "Terminal",
      name: "Fechar ao sair",
      description:
        "Quando o agente sai sem erro (/exit ou Ctrl+D, código 0), a aba fecha depois de 1 s; teclar, clicar ou rolar o console antes disso a mantém aberta. Saída com erro, por sinal ou interrompida mantém a aba. Nesta máquina.",
      keywords: "fechar aba exit encerrar sessão console",
      modified: termCloseChanged(term),
      restore: restoreTermClose,
      control: <TermCloseControl />,
    },
    {
      id: "notify",
      section: "notifications",
      category: "Notificações",
      name: "Avisar quando",
      description:
        "Notificações do sistema, mesmo com a janela minimizada. Valem para todos os ambientes; silencie um ambiente no escopo Ambiente.",
      keys: NOTIFS.map((n) => `notify.${n.key}`),
      keywords: NOTIFS.map((n) => n.label).join(" "),
      modified: notifyOff,
      restore: () => save({ notify: { ...config.notify, needs_human: true, session_error: true, env_down: true } }),
      control: (
        <div className="set-checks">
          {NOTIFS.map((n) => (
            <div key={n.key} className="notif-row">
              <input
                type="checkbox"
                id={`n-${n.key}`}
                checked={config.notify[n.key]}
                onChange={(e) => save({ notify: { ...config.notify, [n.key]: e.target.checked } })}
              />
              <label htmlFor={`n-${n.key}`}>
                <span>{n.label}</span>
                <span className="muted hint">{n.hint}</span>
              </label>
            </div>
          ))}
        </div>
      ),
    },
    ...(webSession
      ? [
          {
            id: "web-session",
            section: "web-access" as const,
            category: "Acesso web",
            name: "Sessão deste navegador",
            description:
              "Entrou com o token de UI do hub; o navegador guarda só o cookie da sessão. Sair revoga a sessão no hub.",
            keywords: "sair logout cookie revogar",
            control: <WebAccess />,
          },
        ]
      : []),
  ];
}

/** Configurações do daemon do ambiente `env` (e as notificações dele, guardadas neste app). */
function useEnvItems(env: string | undefined): Item[] {
  const { config, setConfig, infos, status, api, refresh } = useTower();
  const nav = useNav();
  const [syncing, setSyncing] = useState(false);
  const cfg = useEnvConfig(env);
  if (!env) return [];
  const info = infos[env];
  const on = !!status[env]?.connected;
  // Canal seguido pelo daemon (`update_ref`, padrão `main`); `null` = sem dado.
  const ref = info?.update?.ref ?? null;
  // Editáveis pelo `env.config.*`; sem ele (daemon antigo), só leitura com a chave do config.toml.
  const refItem = cfg.items?.update_ref;
  const syncItem = cfg.items?.gh_sync_interval_secs;
  const readOnlyHint = cfg.error ? ` Este daemon não edita daqui (${cfg.error}): edite no config.toml do ambiente.` : "";
  const muted = config.notify.muted_envs ?? [];
  const isMuted = muted.includes(env);
  const setMuted = (m: boolean) =>
    void setConfig({
      ...config,
      notify: { ...config.notify, muted_envs: m ? [...muted.filter((x) => x !== env), env] : muted.filter((x) => x !== env) },
    });

  return [
    {
      id: "gh-token",
      section: "github",
      category: "GitHub",
      name: "Token do ambiente",
      description:
        "Usado pelo daemon para sincronizar e clonar. Fica em ~/.config/ctower/github.token na distro; sem ele, o daemon usa gh auth token.",
      desc: ENV_TOKEN_HINT,
      keys: ["github.token"],
      keywords: "gh auth token pat personal access login conta",
      control: <GithubSettings env={env} />,
    },
    {
      id: "gh-sync",
      section: "github",
      category: "GitHub",
      name: "Intervalo de sincronização",
      shownKey: "gh_sync_interval_secs",
      keys: ["gh_sync_interval_secs"],
      keywords: "sync sincronizar segundos laço",
      description: `Segundos entre os ciclos do sync com o GitHub (padrão ${syncItem?.default ?? 180}); 0 desliga o laço e o Sincronizar continua funcionando. Vale na hora, sem reiniciar o daemon.${readOnlyHint}`,
      modified: configChanged(syncItem),
      restore: syncItem && (() => void cfg.set("gh_sync_interval_secs", syncItem.default)),
      control: (
        <div className="set-inline">
          {syncItem && <SyncIntervalControl cfg={cfg} />}
          <button
            type="button"
            className="btn"
            disabled={!on || syncing}
            onClick={async () => {
              setSyncing(true);
              try {
                await api.call(env, "sync.now");
                await refresh(env);
              } catch {
                // O estado do sync aparece no painel Sincronização.
              } finally {
                setSyncing(false);
              }
            }}
          >
            {syncing ? "Sincronizando…" : "Sincronizar agora"}
          </button>
        </div>
      ),
    },
    {
      id: "update-ref",
      section: "daemon-update",
      category: "Atualização",
      name: "Canal",
      shownKey: "update_ref",
      keys: ["update_ref", "update_repo"],
      keywords: "branch tag main develop",
      description: `Branch ou tag de onde o ctowerd é recompilado (padrão ${refItem?.default ?? "main"}). Vale na hora: a próxima verificação já segue o canal novo.${readOnlyHint}`,
      modified: refItem ? configChanged(refItem) : ref !== null && ref !== "main",
      restore: refItem && (() => void cfg.set("update_ref", refItem.default)),
      control: refItem ? (
        <UpdateRefControl cfg={cfg} />
      ) : (
        <span className="mono" aria-label="Canal de atualização">
          {ref ?? (on ? "daemon sem suporte a atualização" : "sem conexão")}
        </span>
      ),
    },
    {
      id: "update-interval",
      section: "daemon-update",
      category: "Atualização",
      name: "Intervalo de verificação",
      shownKey: "update_check_interval_hours",
      keys: ["update_check_interval_hours"],
      keywords: "verificar horas",
      description:
        "Horas entre as verificações de versão nova do ctowerd (padrão 6). Edite no config.toml do ambiente e reinicie o daemon.",
      control: <span className="muted">no config.toml do ambiente</span>,
    },
    {
      id: "update-rollback",
      section: "daemon-update",
      category: "Atualização",
      name: "Reverter",
      keywords: "rollback versão anterior desfazer voltar",
      description:
        "Volta o ctowerd à versão de antes da última atualização (env.rollback). O daemon reinicia; sessões vivas são encerradas e voltam como interrompidas.",
      control: <RollbackControl env={env} />,
    },
    // Ponte (#336): versão, verificar e atualizar ficam em Manutenção; a busca por eles cai aqui.
    {
      id: "update-maintenance",
      section: "daemon-update",
      category: "Atualização",
      name: "Versão e atualização",
      keywords: "atualizar versão instalado disponível verificar commits log sessões vivas reverter rollback manutenção",
      description:
        "Versão instalada e disponível, verificar, atualizar e o fim do log de cada ambiente ficam em Manutenção › Atualizações › Ambientes.",
      control: (
        <button type="button" className="link-btn" onClick={() => nav.openTab(maintenanceTab("env-updates"))}>
          Ver e atualizar em Manutenção ›
        </button>
      ),
    },
    {
      id: "projects",
      section: "local-projects",
      category: "Projetos",
      name: "Clones neste ambiente",
      keys: ["repos_root", "hub_url", "config_pull_interval_secs"],
      keywords: "clone caminho distro atribuição configuração rev",
      description:
        "Só leitura: cadastro e atribuição ficam no Hub. Aqui ficam o caminho dos clones e a configuração vinda do hub; o que saiu dele fica em Manutenção › Limpeza.",
      control: <ProjectsSettings env={env} />,
    },
    {
      id: "worktrees",
      section: "local-projects",
      category: "Projetos",
      name: "Raiz dos worktrees",
      shownKey: "worktrees_root",
      keys: ["worktrees_root"],
      description: "Onde o daemon cria o worktree de cada sessão.",
      control: <span className="mono">{info?.worktrees_root ?? (on ? "—" : "sem conexão")}</span>,
    },
    {
      id: "cleanup",
      section: "cleanup",
      category: "Limpeza",
      name: "Regras",
      shownKey: "[cleanup]",
      keys: ["cleanup", ...CLEANUP_KEYS],
      keywords: "worktree target branch sessões transcripts automática intervalo",
      description:
        "Quando o ambiente verifica e o que pode limpar sozinho, com o valor aplicado. Edite [cleanup] no config.toml do ambiente e reinicie o daemon.",
      control: <CleanupRules env={env} />,
    },
    // Ponte (#337): plano, verificar e limpar ficam em Manutenção; a busca por eles cai aqui.
    {
      id: "cleanup-maintenance",
      section: "cleanup",
      category: "Limpeza",
      name: "Plano e limpeza",
      keywords: "limpar pendentes bloqueados verificar concluídos sessões abertas fora do hub restos clones manutenção",
      description:
        "O plano de limpeza (pendentes, bloqueados e por quê), verificar, limpar e o que ficou fora do hub ficam em Manutenção › Limpeza.",
      control: (
        <button type="button" className="link-btn" onClick={() => nav.openTab(maintenanceTab("cleanup", env))}>
          Ver e limpar em Manutenção ›
        </button>
      ),
    },
    {
      id: "env-notify",
      section: "env-notifications",
      category: "Notificações",
      name: "Notificar deste ambiente",
      keys: ["notify.muted_envs"],
      keywords: "silenciar mudo avisos",
      description:
        "Desligado, nenhuma notificação do sistema vem deste ambiente (nem as que estão ligadas em Este app). Guardado neste app.",
      modified: isMuted,
      restore: () => setMuted(false),
      control: (
        <div className="notif-row">
          <input type="checkbox" id={`n-env-${env}`} checked={!isMuted} onChange={(e) => setMuted(!e.target.checked)} />
          <label htmlFor={`n-env-${env}`}>
            <span>Notificações deste ambiente</span>
          </label>
        </div>
      ),
    },
  ];
}

/** Editor de um template de prompt inicial, em aba própria. */
function TemplateEditor({ k }: { k: TemplateKey }) {
  const { config, setConfig } = useTower();
  const [text, setText] = useState(config.templates[k]);
  const [saved, setSaved] = useState(false);
  useEffect(() => setText(config.templates[k]), [config.templates, k]);
  const dirty = text !== config.templates[k];
  // Editar fixa a aba provisória (como no VS Code), para o próximo item aberto
  // não a substituir e levar a edição junto. Só na primeira alteração; abrir/ler não fixa.
  const nav = useNav();
  const pinned = useRef(false);
  const edit = (value: string) => {
    if (!pinned.current) {
      pinned.current = true;
      nav.pin(templateTab(k).id ?? nav.current);
    }
    setText(value);
    setSaved(false);
  };

  return (
    <main className="settings set-template">
      <h1 className="set-template-title">Template: {TEMPLATE_LABEL[k]}</h1>
      <div className="field">
        <label htmlFor={`tpl-${k}`}>{TEMPLATE_LABEL[k]}</label>
        <textarea
          id={`tpl-${k}`}
          rows={14}
          className="mono"
          value={text}
          onChange={(e) => edit(e.target.value)}
        />
      </div>
      <span className="muted hint">
        {k === "gestor" ? (
          <>
            Variáveis: <span className="mono">{"{{env}} {{scope}} {{owner}} {{repo}}"}</span> (os limites do gestor são
            fixos e não entram no template).
          </>
        ) : k.startsWith("jira_") ? (
          <>
            Variáveis: <span className="mono">{"{{kind}} {{key}} {{number}} {{title}} {{branch}} {{base}} {{protected}}"}</span>,{" "}
            <span className="mono">{"{{jira_url}} {{jira_project}} {{forge_path}}"}</span> e{" "}
            <span className="mono">{"{{jira}} {{glab}}"}</span> (o jira-cli e o glab com o comando de credencial do projeto,
            nunca a senha). Usado nos projetos Jira + GitLab; as convenções de cada projeto vão nas regras do agente.
          </>
        ) : (
          <>
            Variáveis: <span className="mono">{"{{kind}} {{number}} {{owner}} {{repo}} {{branch}} {{protected}}"}</span>.
          </>
        )}
      </span>
      <div className="form-actions">
        {saved && <span className="state-on">Template salvo.</span>}
        <button
          type="button"
          className="btn"
          disabled={text === DEFAULT_UI_CONFIG.templates[k]}
          onClick={() => edit(DEFAULT_UI_CONFIG.templates[k])}
        >
          Restaurar padrão
        </button>
        <button
          type="button"
          className="btn btn-primary"
          disabled={!dirty}
          onClick={async () => {
            await setConfig({ ...config, templates: { ...config.templates, [k]: text } });
            setSaved(true);
          }}
        >
          Salvar template
        </button>
      </div>
    </main>
  );
}
