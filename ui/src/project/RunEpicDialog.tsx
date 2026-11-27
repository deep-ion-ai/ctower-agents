import "./add-issue.css";
import { useEffect, useId, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import type { EpicRun, HubEnv } from "../protocol/hub-gen";
import { useTower } from "../state/tower";
import { errText } from "../hub/calls";
import { CLAUDE_EFFORTS, CLAUDE_MODELS } from "../settings/session-options";
import { parallelOf } from "../hub/EpicRuns";

const focusables = (root: HTMLElement) =>
  Array.from(
    root.querySelectorAll<HTMLElement>("button, a[href], input, select, [tabindex]:not([tabindex='-1'])"),
  ).filter((e) => !(e as HTMLButtonElement).disabled);

/**
 * "Executar épico" (#304): dispara a execução do épico num ambiente
 * (`hub.epic_run.start`). Uma sessão de plano (modelo forte) grava o plano,
 * que espera a aprovação em Hub › Execução de épicos.
 */
export function RunEpicDialog({
  owner,
  repo,
  epic,
  onClose,
  onStarted,
}: {
  owner: string;
  repo: string;
  epic: number;
  onClose: () => void;
  onStarted: (run: EpicRun) => void;
}) {
  const { api } = useTower();
  const ids = useId();
  const dialog = useRef<HTMLElement>(null);
  const [envs, setEnvs] = useState<HubEnv[] | null>(null);
  const [env, setEnv] = useState("");
  const [parallel, setParallel] = useState("2");
  const [model, setModel] = useState("opus");
  const [effort, setEffort] = useState("high");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api.hubCall<HubEnv[]>("hub.envs.list").then(
      (l) => {
        if (!live) return;
        setEnvs(l);
        setEnv((cur) => cur || (l.find((e) => e.online) ?? l[0])?.env_id || "");
      },
      (e) => live && setError(errText(e)),
    );
    dialog.current?.focus();
    return () => {
      live = false;
    };
  }, [api]);

  const send = async (e: FormEvent) => {
    e.preventDefault();
    if (!env) return;
    setBusy(true);
    setError(null);
    try {
      const run = await api.hubCall<EpicRun>("hub.epic_run.start", {
        owner,
        repo,
        epic,
        env_id: env,
        max_parallel: parallelOf(parallel, 2),
        plan_model: model,
        plan_effort: effort || null,
      });
      onStarted(run);
    } catch (err) {
      setError(errText(err));
    } finally {
      setBusy(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape" && !busy) onClose();
    if (e.key === "Tab" && dialog.current) {
      const list = focusables(dialog.current);
      if (list.length === 0) return;
      const first = list[0];
      const last = list[list.length - 1];
      const at = document.activeElement;
      if (e.shiftKey && (at === first || at === dialog.current)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && at === last) {
        e.preventDefault();
        first.focus();
      }
    }
  };

  return (
    <div className="ai-scrim" onMouseDown={(e) => e.target === e.currentTarget && !busy && onClose()}>
      <section
        ref={dialog}
        className="ai-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby={`${ids}-title`}
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <h2 id={`${ids}-title`}>Executar o épico #{epic}</h2>
        <p className="muted">
          Uma sessão de plano analisa as sub-issues e grava a ordem, o modelo e as áreas de cada uma. O plano espera a sua
          aprovação antes de qualquer sessão de implementação.
        </p>
        <form className="ai-form" aria-labelledby={`${ids}-title`} onSubmit={(e) => void send(e)}>
          <div className="field">
            <label htmlFor={`${ids}-env`}>Ambiente</label>
            <select id={`${ids}-env`} value={env} onChange={(e) => setEnv(e.target.value)} disabled={!envs}>
              {(envs ?? []).map((e) => (
                <option key={e.env_id} value={e.env_id}>
                  {e.name}
                  {e.online ? "" : " (offline)"}
                </option>
              ))}
            </select>
          </div>
          <div className="field">
            <label htmlFor={`${ids}-par`}>Sessões em paralelo</label>
            <input
              id={`${ids}-par`}
              type="number"
              min={1}
              max={8}
              value={parallel}
              onChange={(e) => setParallel(e.target.value)}
            />
          </div>
          <div className="field">
            <label htmlFor={`${ids}-model`}>Modelo da sessão de plano</label>
            <select id={`${ids}-model`} value={model} onChange={(e) => setModel(e.target.value)}>
              {CLAUDE_MODELS.map((m) => (
                <option key={m.value} value={m.value}>
                  {m.label}
                </option>
              ))}
            </select>
          </div>
          <div className="field">
            <label htmlFor={`${ids}-effort`}>Esforço da sessão de plano</label>
            <select id={`${ids}-effort`} value={effort} onChange={(e) => setEffort(e.target.value)}>
              <option value="">padrão</option>
              {CLAUDE_EFFORTS.map((x) => (
                <option key={x.value} value={x.value}>
                  {x.label}
                </option>
              ))}
            </select>
          </div>
          {error && (
            <p role="alert" className="ai-error">
              {error}
            </p>
          )}
          <div className="ai-actions">
            <button type="submit" className="btn btn-primary" disabled={busy || !env}>
              Executar épico
            </button>
            <button type="button" className="btn" onClick={onClose} disabled={busy}>
              Cancelar
            </button>
          </div>
        </form>
      </section>
    </div>
  );
}
