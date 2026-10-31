import { useEffect, useRef, useState } from "react";
import type { DaemonApi } from "./api/daemon";
import { TowerProvider, useTower } from "./state/tower";
import { NavProvider, hasProjectKey, useNav, type Screen } from "./state/nav";
import { Shell } from "./shell/Shell";
import { interruptedCount } from "./shell/summary";
import { ErrorBoundary } from "./ErrorBoundary";
import { ControlRoom } from "./control-room/ControlRoom";
import { TerminalScreen } from "./terminal/TerminalScreen";
import { ProjectScreen } from "./project/ProjectScreen";
import { HubProjectScreen } from "./project/HubProjectScreen";
import { ProjectIndex } from "./project/ProjectIndex";
import { RecoveryScreen } from "./recovery/RecoveryScreen";
import { ReviewScreen } from "./review/ReviewScreen";
import { SettingsScreen } from "./settings/SettingsScreen";
import { HubScreen } from "./hub/HubScreen";
import { MaintenanceScreen } from "./maintenance/MaintenanceScreen";
import { EnvScreen } from "./env/EnvScreen";
import { maintenanceTab } from "./maintenance/sections";
import { settingsTab } from "./settings/sections";
import { SessionScreen } from "./web/SessionPanel";
import { WebRecovery, WebReview } from "./web/WebScreens";
import { usePwa } from "./web/pwa";

export function App({ api, initial }: { api: DaemonApi; initial?: Screen }) {
  return (
    <TowerProvider api={api}>
      <NavProvider initial={initial}>
        <Main />
      </NavProvider>
    </TowerProvider>
  );
}

/** Relógio de parede para textos "há N min" (atualiza a cada 30 s). */
export function useNow(intervalMs = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);
  return now;
}

/** Major do protocolo que esta UI fala (o do `ct-client`). */
const UI_PROTOCOL_MAJOR = 2;

/**
 * Daemon com protocolo incompatível: oferece atualizar o lado defasado. O
 * `env.update` funciona mesmo assim (método estável, conexão avulsa).
 */
function Incompatible({ env, name, error }: { env: string; name: string; error: string }) {
  const { api } = useTower();
  const { openTab } = useNav();
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [refused, setRefused] = useState(false);
  const daemon = /incompatível \(([^)]+)\)/.exec(error)?.[1] ?? "?";
  const older = parseInt(daemon, 10) < UI_PROTOCOL_MAJOR;

  const update = async (force: boolean) => {
    setBusy(true);
    try {
      await api.call(env, "env.update", { force });
      setRefused(false);
      setMsg("Atualização iniciada; o app reconecta quando o daemon voltar.");
    } catch (e) {
      const s = String(e);
      setRefused(s.includes("Recusado"));
      setMsg(s);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="banner" role="alert">
      {name}: {error}. Nenhuma outra chamada é feita a este ambiente.{" "}
      {older ? (
        <>
          O daemon está defasado.{" "}
          <button type="button" className="btn" disabled={busy} onClick={() => update(false)}>
            Atualizar {name}
          </button>
          {refused && (
            <button type="button" className="btn btn-danger" disabled={busy} onClick={() => update(true)}>
              Atualizar e encerrar sessões
            </button>
          )}
        </>
      ) : (
        <>
          O app está defasado.{" "}
          <button type="button" className="btn" onClick={() => openTab(maintenanceTab("app-updates"))}>
            Atualizar o app
          </button>
        </>
      )}
      {msg && <span className="banner-msg"> {msg}</span>}
    </div>
  );
}

function Main() {
  const { screen, go, openTab, current, editors } = useNav();
  const { dashboards, status, envs, hub, web, api } = useTower();
  const now = useNow();
  // PWA (#203): deep link das notificações e a inscrição presa à sessão web.
  usePwa(api, !!web, openTab);
  const interrupted = interruptedCount(dashboards);
  const incompatible = Object.values(status).filter((s) => s.error?.includes("incompatível"));

  // Spec §10: a Recuperação abre sozinha se, ao abrir o app, houver sessões
  // interrompidas (uma vez; depois o contador na barra de atividades basta).
  const autoOpened = useRef(false);
  useEffect(() => {
    // Web: a Recuperação é só leitura; não toma a tela inicial.
    if (web || autoOpened.current || Object.keys(dashboards).length === 0) return;
    autoOpened.current = true;
    if (interrupted > 0 && screen.name === "control") go({ name: "recovery" });
  }, [web, dashboards, interrupted, screen.name, go]);

  const banners = (
    <>
      {incompatible.map((s) => (
        <Incompatible
          key={s.env}
          env={s.env}
          name={envs.find((e) => e.id === s.env)?.name ?? s.env}
          error={s.error ?? ""}
        />
      ))}
      {hub.partial && (
        <div className="banner" role="status">
          hub indisponível: visão parcial. As sessões seguem nos ambientes com a última configuração recebida; as
          mudanças esperam no outbox e seguem quando o hub voltar.
        </div>
      )}
      {hub.unconfigured && (
        <div className="banner" role="status">
          Conexão com o hub não configurada: projetos, atribuições e a visão consolidada ficam no hub.{" "}
          <button type="button" className="btn" onClick={() => openTab(settingsTab({ section: "connections" }))}>
            Configurar o hub
          </button>
        </div>
      )}
    </>
  );

  return (
    <Shell banners={banners} now={now}>
      {/* `key` por aba: ao trocar de aba, o erro de uma tela não segue para a
          próxima. As abas de sessão são todas da tela Terminal (que mantém os
          consoles montados): trocar entre elas não remonta a tela. */}
      <ErrorBoundary key={(editors.find((e) => e.id === current)?.kind ?? "screen") !== "screen" ? "terminal" : current}>
        <Current screen={screen} />
      </ErrorBoundary>
    </Shell>
  );
}

function Current({ screen }: { screen: Screen }) {
  const { web } = useTower();
  // Web (épico #205 §10.2): no lugar do terminal, o painel da sessão;
  // Recuperação e Revisão só leitura; sem Manutenção.
  if (web) {
    switch (screen.name) {
      case "terminal":
        return <SessionScreen />;
      case "recovery":
        return <WebRecovery />;
      case "review":
        return <WebReview />;
      case "maintenance":
        return (
          <main className="page">
            <h1 className="sr-only">Manutenção</h1>
            <p className="muted">Manutenção fica só no app desktop (atualizações, limpeza, backup e snapshots).</p>
          </main>
        );
    }
  }
  switch (screen.name) {
    case "control":
      return <ControlRoom />;
    case "terminal":
      return <TerminalScreen />;
    case "project":
      return <ProjectRoute screen={screen} />;
    case "recovery":
      return <RecoveryScreen env={screen.env} session={screen.session} />;
    case "review":
      return <ReviewScreen env={screen.env} workItemId={screen.workItemId} />;
    case "settings":
      return <SettingsScreen screen={screen} />;
    case "hub":
      return <HubScreen screen={screen} />;
    case "maintenance":
      return <MaintenanceScreen screen={screen} />;
    case "env":
      return <EnvScreen screen={screen} />;
  }
}

/**
 * Projeto pela visão do hub, quando ele está no ar; senão, pelos dados
 * locais do ambiente (visão parcial). Aceita as duas formas de endereço.
 */
function ProjectRoute({ screen }: { screen: Extract<Screen, { name: "project" }> }) {
  const { hub, envs, dashboards } = useTower();
  // Sem chave, o índice de projetos (#16): não abre mais o primeiro projeto.
  if (!hasProjectKey(screen)) return <ProjectIndex register={screen.register} notice={screen.notice} />;
  const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();
  let key: { owner: string; repo: string } | undefined =
    screen.owner && screen.repo ? { owner: screen.owner, repo: screen.repo } : undefined;
  if (!key && screen.env && screen.projectId !== undefined) {
    const p = dashboards[screen.env]?.projects.find((x) => x.project.id === screen.projectId)?.project;
    if (p) key = { owner: p.gh_owner, repo: p.gh_repo };
  }
  if (hub.up && hub.dashboard) {
    if (key)
      return (
        <HubProjectScreen
          owner={key.owner}
          repo={key.repo}
          item={screen.item}
          view={screen.view}
          section={screen.section}
          assign={screen.assign}
        />
      );
  }
  let { env, projectId } = screen;
  if (key && (env === undefined || projectId === undefined)) {
    // Endereço do hub com o hub fora: o projeto local de algum ambiente.
    for (const e of envs) {
      const p = dashboards[e.id]?.projects.find(
        (x) => same(x.project.gh_owner, key!.owner) && same(x.project.gh_repo, key!.repo),
      );
      if (p) {
        env = e.id;
        projectId = p.project.id;
        break;
      }
    }
  }
  return <ProjectScreen env={env} projectId={projectId} item={screen.item} view={screen.view} />;
}
