// Configurações › Este app › Plano da conta (#108): limiares de aviso e
// crítico por janela (o padrão e os de cada conta), o prazo de leitura velha
// demais e a notificação. Ficam no `ui.json` (por máquina), em `plan`.
import { useEffect, useState } from "react";
import { useTower } from "../state/tower";
import { accountLabel } from "../plan/format";
import { usePlan, usePlanPrefs } from "../plan/usePlan";
import {
  MAX_STALE_MINUTES,
  MIN_STALE_MINUTES,
  PLAN_WINDOWS,
  sameThresholds,
  staleError,
  thresholdsError,
  type PlanPrefs,
  type PlanWindowKind,
  type PlanWindowThresholds,
} from "../plan/thresholds";

const WINDOW_LABEL: Record<PlanWindowKind, string> = { five_hour: "Janela de 5h", seven_day: "Janela semanal" };

/** Grava as preferências do plano no `ui.json`. */
function useSavePlan(): (next: PlanPrefs) => void {
  const { config, setConfig } = useTower();
  return (next) => void setConfig({ ...config, plan: next });
}

type Draft = Record<PlanWindowKind, { warn: string; critical: string }>;

const toDraft = (t: PlanWindowThresholds): Draft => ({
  five_hour: { warn: String(t.five_hour.warn), critical: String(t.five_hour.critical) },
  seven_day: { warn: String(t.seven_day.warn), critical: String(t.seven_day.critical) },
});

const fromDraft = (d: Draft): PlanWindowThresholds => ({
  five_hour: { warn: Number(d.five_hour.warn), critical: Number(d.five_hour.critical) },
  seven_day: { warn: Number(d.seven_day.warn), critical: Number(d.seven_day.critical) },
});

/**
 * Aviso e crítico das duas janelas. Grava ao sair do campo (ou Enter)
 * quando as duas janelas valem; senão, mostra o erro e não grava.
 */
export function ThresholdsEditor({
  value,
  onSave,
  label,
}: {
  value: PlanWindowThresholds;
  onSave(t: PlanWindowThresholds): void;
  /** Prefixo dos rótulos acessíveis ("Padrão", "fulano@example.com"). */
  label: string;
}) {
  const [draft, setDraft] = useState<Draft>(() => toDraft(value));
  const key = JSON.stringify(value);
  useEffect(() => setDraft(toDraft(JSON.parse(key) as PlanWindowThresholds)), [key]);
  const next = fromDraft(draft);
  const errors = Object.fromEntries(PLAN_WINDOWS.map((k) => [k, thresholdsError(next[k])])) as Record<PlanWindowKind, string | null>;
  const valid = PLAN_WINDOWS.every((k) => errors[k] === null);
  const commit = () => {
    if (valid && !sameThresholds(next, value)) onSave(next);
  };
  const input = (k: PlanWindowKind, f: "warn" | "critical") => (
    <input
      aria-label={`${label}: ${WINDOW_LABEL[k]}, ${f === "warn" ? "aviso" : "crítico"} (%)`}
      className="set-input set-input-num"
      type="number"
      min={1}
      max={100}
      value={draft[k][f]}
      aria-invalid={errors[k] !== null}
      onChange={(e) => setDraft((d) => ({ ...d, [k]: { ...d[k], [f]: e.target.value } }))}
      onBlur={commit}
      onKeyDown={(e) => e.key === "Enter" && commit()}
    />
  );
  return (
    <div className="set-checks plan-thresholds" role="group" aria-label={`Limiares: ${label}`}>
      {PLAN_WINDOWS.map((k) => (
        <div key={k} className="set-inline">
          <span className="plan-thr-name">{WINDOW_LABEL[k]}</span>
          <span className="muted hint">aviso</span>
          {input(k, "warn")}
          <span className="muted hint">crítico</span>
          {input(k, "critical")}
          {errors[k] && (
            <span className="state-off hint" role="alert">
              {errors[k]}
            </span>
          )}
        </div>
      ))}
    </div>
  );
}

/** Limiares padrão (contas sem limiar próprio). */
export function PlanDefaultControl() {
  const prefs = usePlanPrefs();
  const save = useSavePlan();
  return <ThresholdsEditor label="Padrão" value={prefs.default} onSave={(t) => save({ ...prefs, default: t })} />;
}

/** Limiares próprios de cada conta conhecida (e os de contas que sumiram, para remover). */
export function PlanAccountsControl() {
  const prefs = usePlanPrefs();
  const save = useSavePlan();
  const { data } = usePlan(Date.now());
  const setAccount = (key: string, t: PlanWindowThresholds | null) => {
    const { [key]: _omit, ...rest } = prefs.accounts;
    save({ ...prefs, accounts: t ? { ...rest, [key]: t } : rest });
  };
  const known = new Set(data.accounts.map((a) => a.key));
  const gone = Object.keys(prefs.accounts).filter((k) => !known.has(k));
  if (!data.accounts.length && !gone.length) {
    return <p className="muted">Nenhuma conta conhecida ainda: as contas aparecem quando um ambiente informa o login.</p>;
  }
  return (
    <div className="set-checks">
      {data.accounts.map((a) => {
        const label = accountLabel(a);
        const own = prefs.accounts[a.key];
        const id = `plan-own-${a.key}`;
        return (
          <div key={a.key} className="plan-thr-account">
            <div className="notif-row">
              <input
                type="checkbox"
                id={id}
                checked={!!own}
                onChange={(e) => setAccount(a.key, e.target.checked ? prefs.default : null)}
              />
              <label htmlFor={id}>
                <span>{label}</span>
                <span className="muted hint">{own ? "limiares próprios" : "usa o padrão"}</span>
              </label>
            </div>
            {own && <ThresholdsEditor label={label} value={own} onSave={(t) => setAccount(a.key, t)} />}
          </div>
        );
      })}
      {gone.map((k) => (
        <div key={k} className="set-inline">
          <span className="mono">{k}</span>
          <span className="muted hint">conta sem ambiente conectado agora</span>
          <button type="button" className="btn btn-sm" aria-label={`Remover os limiares da conta ${k}`} onClick={() => setAccount(k, null)}>
            Remover
          </button>
        </div>
      ))}
    </div>
  );
}

/** Prazo, em minutos, acima do qual a leitura não alerta. */
export function PlanStaleControl() {
  const prefs = usePlanPrefs();
  const save = useSavePlan();
  const [text, setText] = useState(String(prefs.stale_minutes));
  useEffect(() => setText(String(prefs.stale_minutes)), [prefs.stale_minutes]);
  const n = Number(text);
  const error = staleError(n);
  const commit = () => {
    if (error === null && n !== prefs.stale_minutes) save({ ...prefs, stale_minutes: n });
  };
  return (
    <div className="set-inline">
      <input
        id="plan-stale"
        aria-label="Leitura velha demais (min)"
        className="set-input set-input-num"
        type="number"
        min={MIN_STALE_MINUTES}
        max={MAX_STALE_MINUTES}
        value={text}
        aria-invalid={error !== null}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.key === "Enter" && commit()}
      />
      <span className="muted hint">min</span>
      {error && (
        <span className="state-off hint" role="alert">
          {error}
        </span>
      )}
    </div>
  );
}

/** Notificação do sistema ao cruzar um limiar. */
export function PlanNotifyControl() {
  const prefs = usePlanPrefs();
  const save = useSavePlan();
  return (
    <div className="notif-row">
      <input type="checkbox" id="n-plan" checked={prefs.notify} onChange={(e) => save({ ...prefs, notify: e.target.checked })} />
      <label htmlFor="n-plan">
        <span>Plano da conta perto do limite</span>
        <span className="muted hint">Ao cruzar o aviso ou o crítico, uma vez por conta</span>
      </label>
    </div>
  );
}
