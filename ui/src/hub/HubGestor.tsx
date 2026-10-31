import { useEffect, useState } from "react";
import type { GestorScope, Session, SessionMode } from "../protocol/gen";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { renderGestorTemplate, scopeText, type GestorScopeChoice } from "../project/templates";
import { lastTermSize } from "../terminal/size";
import { CLAUDE_MODELS, DEFAULT_MODEL_VALUE, SESSION_MODES } from "../settings/session-options";
import { DesktopOnly } from "../web/DesktopOnly";

/** Sessão viva do gestor num ambiente conectado (para "Abrir gestor"). */
export interface LiveGestor {
  env: string;
  name: string;
  session: string;
  /** Escopo escolhido no início, se o ambiente o informou. */
  scope?: GestorScope;
  started_at: number;
}

/**
 * Sessões vivas do gestor nos ambientes conectados (pelo `gestor_work_item_id`
 * de cada dashboard), da mais antiga para a mais nova.
 */
export function useLiveGestor(): LiveGestor[] {
  const { envs, dashboards, status } = useTower();
  return envs
    .flatMap((e) => {
      const d = dashboards[e.id];
      if (!status[e.id]?.connected || !d || d.gestor_work_item_id === null) return [];
      return d.live_sessions
        .filter((s) => s.work_item_id === d.gestor_work_item_id)
        .map((s) => ({
          env: e.id,
          name: e.name,
          session: s.id,
          // `?.`: ambiente anterior ao campo não manda `gestor_scopes`.
          scope: d.gestor_scopes?.[s.id],
          started_at: s.started_at,
        }));
    })
    .sort((a, b) => a.started_at - b.started_at);
}

/** "Abrir gestor · WSL · Portfólio · 14:32": ambiente, escopo e hora de início. */
export function liveGestorLabel(l: LiveGestor): string {
  const hora = new Date(l.started_at).toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" });
  return ["Abrir gestor", l.name, ...(l.scope ? [scopeText(l.scope)] : []), hora].join(" · ");
}

type ScopeKind = GestorScopeChoice["kind"];

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/** O gestor vivo é deste projeto: escopo "Um projeto" com o mesmo `owner/repo`. */
const scopedTo = (l: LiveGestor, p: { owner: string; repo: string }) =>
  l.scope?.kind === "project" && same(l.scope.owner, p.owner) && same(l.scope.repo, p.repo);

/**
 * Botão "Abrir sessão do gestor" da Visão geral do Hub (spec do gestor §11): abre o
 * diálogo de início (ambiente, escopo, modelo, modo e prompt do template
 * `gestor`) e chama `session.start_gestor` no ambiente escolhido. Sempre
 * inicia uma sessão nova; as vivas ficam ao lado, uma ação "Abrir gestor"
 * por sessão.
 *
 * Com `project` (Visão geral do projeto, #19), o diálogo já abre no escopo
 * "Um projeto" com ele escolhido (o usuário está nele; antes tinha de
 * escolher de novo), o botão vira "Gestor deste projeto" e só as sessões
 * vivas do gestor com o foco nele aparecem. O escopo segue trocável.
 */
export function GestorLauncher({ project }: { project?: { owner: string; repo: string } } = {}) {
  const { envs, status, infos, hub, api, config, web } = useTower();
  const nav = useNav();
  const live = useLiveGestor().filter((l) => !project || scopedTo(l, project));
  const [open, setOpen] = useState(false);
  // Só servem ambientes conectados, ligados a um hub e com token de agente.
  const eligible = envs.filter(
    (e) => status[e.id]?.connected && infos[e.id]?.hub_config.linked && infos[e.id]?.hub_config.agent_token,
  );
  const projects = hub.projects.map((p) => p.project);
  const [env, setEnv] = useState("");
  const [scopeKind, setScopeKind] = useState<ScopeKind>(project ? "project" : "portfolio");
  const [projectKey, setProjectKey] = useState(project ? `${project.owner}/${project.repo}` : "");
  const [model, setModel] = useState(config.session.model ?? DEFAULT_MODEL_VALUE);
  const [mode, setMode] = useState<SessionMode>(config.session.mode);
  const [prompt, setPrompt] = useState("");
  const [edited, setEdited] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const target = eligible.find((e) => e.id === env) ?? eligible[0];
  // Sem caixa: o `owner/repo` pré-preenchido vem do endereço do projeto.
  const found = projects.find((p) => same(`${p.gh_owner}/${p.gh_repo}`, projectKey));
  // Pré-preenchido e ainda fora da lista do hub (carregando): fica o pedido,
  // nunca o primeiro da lista no lugar dele.
  const pick = found ?? (project ? undefined : projects[0]);
  const scope: GestorScopeChoice | null =
    scopeKind === "project"
      ? pick
        ? { kind: "project", owner: pick.gh_owner, repo: pick.gh_repo }
        : project
          ? { kind: "project", owner: project.owner, repo: project.repo }
          : null
      : { kind: scopeKind };
  const rendered = target && scope ? renderGestorTemplate(config.templates.gestor, target.name, scope) : "";

  // O prompt acompanha o escopo e o ambiente até o usuário editá-lo.
  useEffect(() => {
    if (!edited) setPrompt(rendered);
  }, [rendered, edited]);

  const start = async () => {
    if (!target || !scope) return;
    setBusy(true);
    setError(null);
    try {
      const s = await api.call<Session>(target.id, "session.start_gestor", {
        scope,
        prompt,
        ...(model ? { model } : {}),
        mode,
        ...(lastTermSize() ?? {}),
      });
      setOpen(false);
      nav.openTerminal({ env: target.id, session: s.id });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="gestor-launch">
      {live.length > 0 && (
        <ul aria-label="Sessões vivas do gestor" className="gestor-live">
          {live.map((l) => (
            <li key={`${l.env}:${l.session}`}>
              <button
                type="button"
                className="btn"
                onClick={() => nav.openTerminal({ env: l.env, session: l.session })}
              >
                {liveGestorLabel(l)}
              </button>
            </li>
          ))}
        </ul>
      )}
      {/* Web: iniciar o gestor fica no app desktop (a escolha do ambiente depende do token de agente do `env.info`);
          o botão aparece desabilitado com o motivo (#26). */}
      {web && (
        <DesktopOnly
          className="btn btn-primary"
          labels={project ? "Gestor deste projeto" : "Abrir sessão do gestor"}
          note="Iniciar o gestor fica no app desktop"
        />
      )}
      {!web && (
        <button type="button" className="btn btn-primary" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
          {project ? "Gestor deste projeto" : "Abrir sessão do gestor"}
        </button>
      )}
      {open && !web && (
        <section aria-label="Iniciar o gestor de projetos" className="panel gestor-dialog">
          <h3>Gestor de projetos</h3>
          <p className="muted small">
            Uma sessão do Claude Code que lê o hub inteiro e propõe mudanças de prioridade e de status. Você aprova as
            propostas aqui na tela Hub; issues novas só com o seu ok na conversa.
          </p>
          {eligible.length === 0 ? (
            <p role="status" className="banner">
              Nenhum ambiente conectado tem o token de agente do hub. No hub, rode{" "}
              <span className="mono">cthubd agent-token &lt;ambiente&gt;</span> e grave no ambiente com{" "}
              <span className="mono">ctower hub set-agent-token</span>.
            </p>
          ) : (
            <>
              <div className="field">
                <label htmlFor="gestor-env">Ambiente</label>
                <select id="gestor-env" value={target?.id ?? ""} onChange={(e) => setEnv(e.target.value)}>
                  {eligible.map((e) => (
                    <option key={e.id} value={e.id}>
                      {e.name}
                    </option>
                  ))}
                </select>
              </div>
              <fieldset className="field gestor-scope">
                <legend>Escopo</legend>
                {(
                  [
                    ["portfolio", "Portfólio"],
                    ["project", "Um projeto"],
                    ["health", "Saúde e riscos"],
                  ] as const
                ).map(([k, label]) => (
                  <label key={k} className="ctl">
                    <input
                      type="radio"
                      name="gestor-scope"
                      value={k}
                      checked={scopeKind === k}
                      onChange={() => setScopeKind(k)}
                    />
                    {label}
                  </label>
                ))}
                {scopeKind === "project" && (
                  <label className="ctl">
                    <span className="sr-only">Projeto</span>
                    <select
                      aria-label="Projeto do escopo"
                      value={pick ? `${pick.gh_owner}/${pick.gh_repo}` : projectKey}
                      onChange={(e) => setProjectKey(e.target.value)}
                    >
                      {projects.map((p) => (
                        <option key={p.id} value={`${p.gh_owner}/${p.gh_repo}`}>
                          {p.name}
                        </option>
                      ))}
                    </select>
                  </label>
                )}
              </fieldset>
              <div className="field-pair">
                <div className="field">
                  <label htmlFor="gestor-modelo">Modelo</label>
                  <select id="gestor-modelo" value={model} onChange={(e) => setModel(e.target.value)}>
                    <option value={DEFAULT_MODEL_VALUE}>Padrão do Claude Code</option>
                    {CLAUDE_MODELS.map((m) => (
                      <option key={m.value} value={m.value}>
                        {m.label}
                      </option>
                    ))}
                  </select>
                </div>
                <div className="field">
                  <label htmlFor="gestor-modo">Modo</label>
                  <select id="gestor-modo" value={mode} onChange={(e) => setMode(e.target.value as SessionMode)}>
                    {SESSION_MODES.map((m) => (
                      <option key={m.value} value={m.value}>
                        {m.label} ({m.hint})
                      </option>
                    ))}
                  </select>
                </div>
              </div>
              <div className="field">
                <label htmlFor="gestor-prompt">Prompt inicial (template do gestor)</label>
                <textarea
                  id="gestor-prompt"
                  rows={8}
                  value={prompt}
                  onChange={(e) => {
                    setPrompt(e.target.value);
                    setEdited(true);
                  }}
                />
              </div>
            </>
          )}
          {error && (
            <p role="alert" className="start-error">
              {error}
            </p>
          )}
          <div className="start-actions">
            <button type="button" className="btn" onClick={() => setOpen(false)}>
              Cancelar
            </button>
            <button
              type="button"
              className="btn btn-primary"
              disabled={busy || !target || !scope || !prompt.trim()}
              onClick={start}
            >
              {busy ? "Iniciando…" : "▶ Iniciar gestor"}
            </button>
          </div>
        </section>
      )}
    </div>
  );
}
