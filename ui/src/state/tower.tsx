import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import type { DaemonApi } from "../api/daemon";
import type { EnvStatus, EnvView, HubConnView, PushEvent, UiConfig, UiUpdate } from "../api/types";
import type { Dashboard, EnvInfo, PermissionRequest } from "../protocol/gen";
import type { HubDashboard, HubEnv, HubInfo, HubProjectView, HubStatus, HubUpdateStatus } from "../protocol/hub-gen";
import { DEFAULT_UI_CONFIG } from "../settings/defaults";
import { HUB_LOGS_PUSH } from "../logs/types";
import { useTheme } from "./theme";
import { dashboardsFromHub, envStatusesFromHub, envViewsFromHub } from "../web/hubData";

/** Alerta recebido por push (não fica no ledger): transcript volátil, confiança. */
export interface Alert {
  env: string;
  kind: string;
  session_id: string | null;
  detail: string;
  at: number;
}

/**
 * Último pedido de atenção de uma sessão (push `needs_human`, hook
 * Notification do agente): o texto que o agente mandou, ex. "Claude needs
 * your permission to use Bash". Não fica no ledger nem no dashboard; vale
 * enquanto o work item estiver `aguardando_voce`.
 */
export interface Ask {
  message: string;
  at: number;
}

/** Chave de `asks`: ambiente e sessão. */
export const askKey = (env: string, session: string) => `${env}|${session}`;

/** Respostas a um pedido de permissão; `ask` devolve o pedido ao menu do terminal. */
export type PermissionDecision = "allow" | "allow_session" | "deny" | "ask";

/** Pushes que mudam o dashboard. */
const REFRESH_ON = new Set([
  "session_started",
  // O id do Remote Control chegou (#197): o dashboard traz o `rc_session_id`.
  "session_remote_control",
  "session_state",
  "session_exited",
  "session_resumed",
  "session_archived",
  "sessions_interrupted",
  "work_item_status",
  "needs_human",
  "permission_request",
  "permission_resolved",
  "sync_status",
]);

export const REFRESH_DEBOUNCE_MS = 300;
/** Releitura da visão do hub: os ambientes enviam a cada poucos segundos. */
export const HUB_POLL_MS = 15_000;
/** Pushes do hub (envio de um ambiente) relidos juntos. */
export const HUB_PUSH_DEBOUNCE_MS = 500;
/**
 * Web: depois de um push do ambiente (`env.push`), o hub só tem a mudança no
 * próximo envio do outbox (a cada 5 s): relê a visão do hub de novo depois
 * disso.
 */
export const ENV_SYNC_LAG_MS = 6_000;
/** Verificação de versão nova do app. */
export const UI_UPDATE_EVERY_MS = 6 * 3600 * 1000;

/**
 * Hub (`cthubd`, spec do hub como plano de controle §8): conexão própria,
 * separada dos ambientes, com a visão consolidada, o cadastro de projetos,
 * as atribuições e os avisos.
 */
export interface HubView {
  /** Conexão com o hub configurada (Configurações → Hub); `null` = sem. */
  conn: HubConnView | null;
  status: EnvStatus | null;
  connected: boolean;
  /** Há hub configurado ou algum ambiente ligado a um: a visão completa existe. */
  expected: boolean;
  /** Visão do hub carregada e o hub conectado. */
  up: boolean;
  /** Hub configurado mas fora do ar: as telas dos ambientes seguem com o cache. */
  partial: boolean;
  /** Algum ambiente está ligado a um hub, mas a UI não tem a conexão com ele. */
  unconfigured: boolean;
  info: HubInfo | null;
  /** `hub.status`: avisos (backup, quarentena). */
  health: HubStatus | null;
  /** `hub.projects.list`: projetos com as atribuições. */
  projects: HubProjectView[];
  /** `hub.envs.list`: ambientes ligados ao hub. */
  envs: HubEnv[];
  dashboard: HubDashboard | null;
  /**
   * Atualização do próprio hub (`hub.update.get` e o push
   * `hub.update.status`, #421); `null` = hub anterior ao 1.6 ou fora do ar.
   */
  update: HubUpdateStatus | null;
  /** `env_id` do daemon → id do ambiente na UI. */
  envByDaemon: Record<string, string>;
}

export interface Tower {
  api: DaemonApi;
  /**
   * Modo web (PWA, épico #205): ambientes, estado e dashboards vêm só do
   * hub; as ações nos ambientes vão pelo canal reverso (`hub.env.call`) e só
   * aparecem com o ambiente online; terminal e recursos do desktop ficam
   * ocultos.
   */
  web: boolean;
  envs: EnvView[];
  status: Record<string, EnvStatus>;
  dashboards: Record<string, Dashboard>;
  /** `env.info` de cada ambiente conectado. */
  infos: Record<string, EnvInfo>;
  hub: HubView;
  alerts: Alert[];
  /** Pedidos de atenção por sessão (`askKey`), desde que o app abriu. */
  asks: Record<string, Ask>;
  /**
   * Pedidos de permissão pendentes por ambiente, do mais antigo ao mais novo
   * (#365). A UI acompanha os pedidos (`session.permission.watch`) ao
   * conectar; sem isso o daemon não os segura e eles vão para o terminal.
   */
  permissions: Record<string, PermissionRequest[]>;
  /** `session.permission.respond`; o pedido sai da lista mesmo se já não estava pendente. */
  respondPermission(env: string, requestId: string, decision: PermissionDecision): Promise<void>;
  config: UiConfig;
  /** Última verificação de versão nova do app (`null` = ainda não verificou). */
  uiUpdate: UiUpdate | null;
  checkUiUpdate(): Promise<UiUpdate>;
  /** Recarrega o dashboard do ambiente agora. */
  refresh(env: string): Promise<void>;
  /** Relê tudo do hub agora (depois de uma mudança feita nele). */
  refreshHub(): Promise<void>;
  /** Relê a conexão com o hub (depois de salvá-la). */
  reloadHub(): Promise<void>;
  reloadEnvs(): Promise<void>;
  setConfig(cfg: UiConfig): Promise<void>;
  dismissAlert(a: Alert): void;
}

const Ctx = createContext<Tower | null>(null);

export function useTower(): Tower {
  const t = useContext(Ctx);
  if (!t) throw new Error("useTower fora do TowerProvider");
  return t;
}

export function TowerProvider({ api, children }: { api: DaemonApi; children: ReactNode }) {
  const web = !!api.web;
  const [localEnvs, setEnvs] = useState<EnvView[]>([]);
  const [localStatus, setStatus] = useState<Record<string, EnvStatus>>({});
  const [localDashboards, setDashboards] = useState<Record<string, Dashboard>>({});
  const [infos, setInfos] = useState<Record<string, EnvInfo>>({});
  const [hubConn, setHubConn] = useState<HubConnView | null>(null);
  const [hubStatus, setHubStatus] = useState<EnvStatus | null>(null);
  const [hubDash, setHubDash] = useState<HubDashboard | null>(null);
  const [hubError, setHubError] = useState(false);
  const [hubInfo, setHubInfo] = useState<HubInfo | null>(null);
  const [hubHealth, setHubHealth] = useState<HubStatus | null>(null);
  const [hubProjects, setHubProjects] = useState<HubProjectView[]>([]);
  const [hubEnvs, setHubEnvs] = useState<HubEnv[]>([]);
  const [hubUpdate, setHubUpdate] = useState<HubUpdateStatus | null>(null);
  const [alerts, setAlerts] = useState<Alert[]>([]);
  const [asks, setAsks] = useState<Record<string, Ask>>({});
  const [permissions, setPermissions] = useState<Record<string, PermissionRequest[]>>({});
  const [config, setConfigState] = useState<UiConfig>(DEFAULT_UI_CONFIG);
  const [configLoaded, setConfigLoaded] = useState(false);
  const [uiUpdate, setUiUpdate] = useState<UiUpdate | null>(null);
  const timers = useRef(new Map<string, ReturnType<typeof setTimeout>>());

  const refresh = useCallback(
    async (env: string) => {
      // Web: sem conexão com o ambiente; o dashboard dele vem do hub.
      if (web) return;
      try {
        const d = await api.call<Dashboard>(env, "dashboard.get");
        setDashboards((all) => ({ ...all, [env]: d }));
      } catch {
        // Desconectado: o status do ambiente já mostra; o próximo push recarrega.
      }
      try {
        // Pendências de envio e configuração do hub (mudam a cada envio e pull).
        const i = await api.call<EnvInfo>(env, "env.info");
        setInfos((all) => ({ ...all, [env]: i }));
      } catch {
        // Fora do ar: sem informação de hub.
      }
    },
    [api, web],
  );

  const dropPermission = useCallback((env: string, requestId: string) => {
    setPermissions((all) =>
      all[env] ? { ...all, [env]: all[env].filter((p) => p.request_id !== requestId) } : all,
    );
  }, []);

  // A conexão nova (ou refeita) passa a acompanhar os pedidos; a resposta
  // traz os que já estavam pendentes. Daemon anterior ao método: sem pedidos.
  const watchPermissions = useCallback(
    async (env: string) => {
      try {
        const list = await api.call<PermissionRequest[]>(env, "session.permission.watch");
        setPermissions((all) => ({ ...all, [env]: list }));
      } catch {
        setPermissions((all) => ({ ...all, [env]: [] }));
      }
    },
    [api],
  );

  const respondPermission = useCallback(
    async (env: string, requestId: string, decision: PermissionDecision) => {
      try {
        await api.call(env, "session.permission.respond", { request_id: requestId, decision });
      } finally {
        // Respondido, expirado ou o agente desistiu: em todo caso não está mais pendente.
        dropPermission(env, requestId);
      }
    },
    [api, dropPermission],
  );

  const hubConnected = !!hubConn && !!hubStatus?.connected;

  const refreshHubDashboard = useCallback(async () => {
    if (!hubConnected) {
      setHubDash(null);
      return;
    }
    try {
      setHubDash(await api.hubCall<HubDashboard>("hub.dashboard.get"));
      setHubError(false);
    } catch {
      setHubDash(null);
      setHubError(true);
    }
  }, [api, hubConnected]);

  const refreshHub = useCallback(async () => {
    if (!hubConnected) {
      setHubDash(null);
      setHubInfo(null);
      setHubHealth(null);
      setHubProjects([]);
      setHubEnvs([]);
      setHubUpdate(null);
      return;
    }
    // Cada leitura vale sozinha: uma falha não apaga as outras.
    const keep = <T,>(p: Promise<T>, set: (v: T) => void) => p.then(set, () => {});
    // Hub anterior ao 1.6 não tem `hub.update.get`: sem atualização pela UI.
    const update = api.hubCall<HubUpdateStatus>("hub.update.get").then(setHubUpdate, () => setHubUpdate(null));
    await Promise.all([
      refreshHubDashboard(),
      keep(api.hubCall<HubInfo>("hub.info"), setHubInfo),
      keep(api.hubCall<HubStatus>("hub.status"), setHubHealth),
      keep(api.hubCall<HubProjectView[]>("hub.projects.list"), setHubProjects),
      keep(api.hubCall<HubEnv[]>("hub.envs.list"), setHubEnvs),
      update,
    ]);
  }, [api, hubConnected, refreshHubDashboard]);

  const reloadHub = useCallback(async () => {
    setHubConn(await api.getHub());
    setHubStatus(await api.hubStatus());
  }, [api]);

  // Tudo do hub: ao conectar e periodicamente.
  useEffect(() => {
    void refreshHub();
    if (!hubConnected) return;
    const id = setInterval(() => void refreshHub(), HUB_POLL_MS);
    return () => clearInterval(id);
  }, [refreshHub, hubConnected]);

  // A visão consolidada muda quando qualquer ambiente envia: relê o
  // dashboard do hub junto com o dos ambientes. Na web os dashboards saem do
  // próprio hub (relê-lo aqui seria um laço).
  useEffect(() => {
    if (!web) void refreshHubDashboard();
  }, [refreshHubDashboard, localDashboards, web]);

  useEffect(() => {
    void reloadHub();
    let t: ReturnType<typeof setTimeout> | undefined;
    const offStatus = api.onHubStatus(setHubStatus);
    const offPush = api.onHubPush((e) => {
      // Verificação ou andamento da atualização do hub: só ela muda.
      if (e.event === "hub.update.status") {
        setHubUpdate(e.data as HubUpdateStatus);
        return;
      }
      // Modo seguir da tela Logs (#65): linhas do log, nada do hub mudou.
      if (e.event === HUB_LOGS_PUSH) return;
      clearTimeout(t);
      t = setTimeout(() => void refreshHub(), HUB_PUSH_DEBOUNCE_MS);
    });
    return () => {
      clearTimeout(t);
      offStatus();
      offPush();
    };
  }, [api, reloadHub, refreshHub]);

  const schedule = useCallback(
    (env: string) => {
      const t = timers.current;
      clearTimeout(t.get(env));
      t.set(
        env,
        setTimeout(() => {
          t.delete(env);
          void refresh(env);
        }, REFRESH_DEBOUNCE_MS),
      );
    },
    [refresh],
  );

  const reloadEnvs = useCallback(async () => {
    // Web: a lista vem de `hub.envs.list`, relida com o resto do hub.
    if (web) return refreshHub();
    const list = await api.listEnvs();
    setEnvs(list);
    const st = await api.statuses();
    setStatus(Object.fromEntries(st.map((s) => [s.env, s])));
    for (const s of st) {
      if (!s.connected) continue;
      void refresh(s.env);
      void watchPermissions(s.env);
    }
  }, [api, web, refresh, refreshHub, watchPermissions]);

  useEffect(() => {
    void reloadEnvs();
    void api.getUiConfig().then((cfg) => {
      setConfigState(cfg);
      setConfigLoaded(true);
    });
    // Web: o estado vem de `hub.envs.list` (o `env.online` relê o hub) e as
    // ações nos ambientes vão por `hub.env.call`; não há permissões a acompanhar.
    let webTimers: ReturnType<typeof setTimeout>[] = [];
    const refreshWeb = () => {
      for (const id of webTimers) clearTimeout(id);
      webTimers = [HUB_PUSH_DEBOUNCE_MS, ENV_SYNC_LAG_MS].map((ms) => setTimeout(() => void refreshHub(), ms));
    };
    const offStatus = api.onStatus((s) => {
      if (web) return;
      setStatus((all) => ({ ...all, [s.env]: s }));
      if (s.connected) {
        void refresh(s.env);
        void watchPermissions(s.env);
      } else {
        // A conexão caiu: o daemon parou de segurar os pedidos por esta UI.
        setPermissions((all) => ({ ...all, [s.env]: [] }));
      }
    });
    const offPush = api.onPush((e: PushEvent) => {
      if (e.event === "alert") {
        setAlerts((all) => [
          ...all,
          {
            env: e.env,
            kind: String(e.data?.kind ?? "alerta"),
            session_id: e.data?.session_id ?? null,
            detail: String(e.data?.path ?? e.data?.error ?? ""),
            at: Date.now(),
          },
        ]);
      }
      if (e.event === "needs_human" && typeof e.data?.session_id === "string") {
        const k = askKey(e.env, e.data.session_id);
        setAsks((all) => ({ ...all, [k]: { message: String(e.data?.message ?? ""), at: Date.now() } }));
      }
      if (e.event === "permission_request" && typeof e.data?.request_id === "string") {
        const req = e.data as PermissionRequest;
        setPermissions((all) => {
          const list = (all[e.env] ?? []).filter((p) => p.request_id !== req.request_id);
          return { ...all, [e.env]: [...list, req] };
        });
      }
      if (e.event === "permission_resolved" && typeof e.data?.request_id === "string") {
        dropPermission(e.env, e.data.request_id);
      }
      if (e.event === "update.status") {
        // Verificação ou andamento de atualização: só a parte `update` do env.info muda.
        setInfos((all) => (all[e.env] ? { ...all, [e.env]: { ...all[e.env], update: e.data } } : all));
      }
      if (REFRESH_ON.has(e.event)) {
        if (web) refreshWeb();
        else schedule(e.env);
      }
    });
    const t = timers.current;
    return () => {
      offStatus();
      offPush();
      for (const id of t.values()) clearTimeout(id);
      t.clear();
      for (const id of webTimers) clearTimeout(id);
    };
  }, [api, web, refresh, refreshHub, reloadEnvs, schedule, watchPermissions, dropPermission]);

  const checkUiUpdate = useCallback(async () => {
    // Web: sem atualizador (a versão do bundle é a do cthubd).
    if (web) return { current: "web", available: null, error: null };
    const u = await api.uiUpdateCheck();
    setUiUpdate(u);
    return u;
  }, [api, web]);

  // Versão nova do app: ao abrir e a cada 6 h.
  useEffect(() => {
    if (web) return;
    void checkUiUpdate();
    const id = setInterval(() => void checkUiUpdate(), UI_UPDATE_EVERY_MS);
    return () => clearInterval(id);
  }, [checkUiUpdate, web]);

  // Web: ambientes, estado (canal reverso; hub 1.3, idade do último envio) e
  // dashboards tirados do hub, no formato das telas.
  const webView = useMemo(() => {
    if (!web) return null;
    const list = hubConnected ? hubEnvs : [];
    return {
      envs: envViewsFromHub(list),
      status: Object.fromEntries(envStatusesFromHub(list, Date.now()).map((s) => [s.env, s])),
      dashboards: hubConnected && hubDash ? dashboardsFromHub(hubDash, hubProjects, list) : {},
    };
  }, [web, hubConnected, hubEnvs, hubDash, hubProjects]);
  const envs = webView?.envs ?? localEnvs;
  const status = webView?.status ?? localStatus;
  const dashboards = webView?.dashboards ?? localDashboards;

  useTheme(configLoaded ? config.appearance.theme : null);

  const setConfig = useCallback(
    async (cfg: UiConfig) => {
      await api.saveUiConfig(cfg);
      setConfigState(cfg);
    },
    [api],
  );

  const dismissAlert = useCallback((a: Alert) => {
    setAlerts((all) => all.filter((x) => x !== a));
  }, []);

  const hub = useMemo<HubView>(() => {
    const envByDaemon: Record<string, string> = {};
    for (const [env, i] of Object.entries(infos)) envByDaemon[i.env_id] = env;
    // Web: o id do ambiente na UI já é o `env_id` do daemon.
    if (web) for (const e of hubEnvs) envByDaemon[e.env_id] = e.env_id;
    const linked = Object.values(infos).some((i) => i.hub_config?.linked || i.hub_sync !== null);
    return {
      conn: hubConn,
      status: hubConn ? hubStatus : null,
      connected: hubConnected,
      expected: !!hubConn || linked,
      up: hubConnected && hubDash !== null,
      partial: !!hubConn && (!hubConnected || hubError),
      unconfigured: !hubConn && linked,
      info: hubConnected ? hubInfo : null,
      health: hubConnected ? hubHealth : null,
      projects: hubConnected ? hubProjects : [],
      envs: hubConnected ? hubEnvs : [],
      dashboard: hubConnected ? hubDash : null,
      update: hubConnected ? hubUpdate : null,
      envByDaemon,
    };
  }, [
    web,
    infos,
    hubConn,
    hubStatus,
    hubConnected,
    hubDash,
    hubError,
    hubInfo,
    hubHealth,
    hubProjects,
    hubEnvs,
    hubUpdate,
  ]);

  const value = useMemo<Tower>(
    () => ({
      api,
      web,
      envs,
      status,
      dashboards,
      infos,
      hub,
      alerts,
      asks,
      permissions,
      respondPermission,
      config,
      uiUpdate,
      checkUiUpdate,
      refresh,
      refreshHub,
      reloadHub,
      reloadEnvs,
      setConfig,
      dismissAlert,
    }),
    [
      refreshHub,
      reloadHub,
      api,
      web,
      envs,
      status,
      dashboards,
      infos,
      hub,
      alerts,
      asks,
      permissions,
      respondPermission,
      config,
      uiUpdate,
      checkUiUpdate,
      refresh,
      reloadEnvs,
      setConfig,
      dismissAlert,
    ],
  );
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}
