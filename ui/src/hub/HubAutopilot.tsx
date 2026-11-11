// Hub › Orçamento do piloto (#180, épico #178): a política de orçamento do
// piloto automático por conta do Claude Code. Escolhe a conta
// (`hub.plan.accounts`), edita faixas de horário, teto semanal, reserva por
// dia útil, corte e fuso, e mostra a prévia "agora pode usar até X%"
// (`hub.autopilot.policy.preview`, sem gravar). Conta sem política = piloto
// automático desligado. Só no app desktop: na web a conta vem anonimizada.
import { useCallback, useEffect, useId, useState } from "react";
import type {
  AccountPlan,
  AutopilotPolicyView,
  BudgetDecision,
  BudgetPolicy,
  HubPlanAccounts,
  TimeBand,
} from "../protocol/hub-gen";
import { useTower } from "../state/tower";
import {
  AUTOPILOT_PROTOCOL,
  DAY_LABELS,
  OFFSETS,
  bandSummary,
  clonePolicy,
  endInput,
  hhmmToMin,
  minToHHMM,
  newBand,
  nextOpenText,
  offsetLabel,
  previewHeadline,
  reasonText,
} from "./autopilot";
import { errText } from "./calls";

const accountLabel = (a: AccountPlan) => a.account.email ?? a.account.org_name ?? a.account.key;

export function HubAutopilot() {
  const { api } = useTower();
  const id = useId();
  const [accounts, setAccounts] = useState<AccountPlan[] | null>(null);
  const [account, setAccount] = useState<string>("");
  const [view, setView] = useState<AutopilotPolicyView | null>(null);
  const [draft, setDraft] = useState<BudgetPolicy | null>(null);
  const [preview, setPreview] = useState<BudgetDecision | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api
      .hubCall<HubPlanAccounts>("hub.plan.accounts")
      .then((h) => {
        setAccounts(h.accounts);
        setAccount((cur) => cur || h.accounts[0]?.account.key || "");
      })
      .catch((e) => setError(errText(e)));
  }, [api]);

  const load = useCallback(
    async (key: string) => {
      try {
        const v = await api.hubCall<AutopilotPolicyView>("hub.autopilot.policy.get", { account: key });
        setView(v);
        setDraft(clonePolicy(v.policy ?? v.default));
        setError(null);
      } catch (e) {
        const t = errText(e);
        setError(
          t.includes("método desconhecido")
            ? `Este hub não tem o piloto automático (anterior ao protocolo ${AUTOPILOT_PROTOCOL}): atualize o hub.`
            : t,
        );
      }
    },
    [api],
  );

  useEffect(() => {
    if (!account) return;
    setView(null);
    setDraft(null);
    setPreview(null);
    setMsg(null);
    void load(account);
  }, [account, load]);

  // Prévia do rascunho, sem gravar, um instante depois da última mudança.
  useEffect(() => {
    if (!account || !draft) return;
    let live = true;
    const t = setTimeout(() => {
      api
        .hubCall<BudgetDecision>("hub.autopilot.policy.preview", { account, policy: draft })
        .then((d) => {
          if (!live) return;
          setPreview(d);
          setPreviewError(null);
        })
        .catch((e) => {
          if (!live) return;
          setPreview(null);
          setPreviewError(errText(e));
        });
    }, 250);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [api, account, draft]);

  const save = async (policy: BudgetPolicy | null) => {
    setBusy(true);
    setMsg(null);
    setError(null);
    try {
      const v = await api.hubCall<AutopilotPolicyView>("hub.autopilot.policy.set", { account, policy });
      setView(v);
      setDraft(clonePolicy(v.policy ?? v.default));
      setMsg(policy ? "Política salva: o piloto automático está ligado nesta conta." : "Piloto automático desligado nesta conta.");
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const patch = (p: Partial<BudgetPolicy>) => draft && setDraft({ ...draft, ...p });
  const patchBand = (i: number, b: Partial<TimeBand>) =>
    draft && setDraft({ ...draft, bands: draft.bands.map((x, j) => (j === i ? { ...x, ...b } : x)) });

  const enabled = !!view?.policy;

  return (
    <section aria-labelledby="hub-autopilot" className="panel settings-section">
      <h2 id="hub-autopilot">Orçamento do piloto automático</h2>
      <p className="muted hint">
        A cota é da conta do Claude Code: a política limita até onde as sessões autônomas podem levar o uso da conta, não
        quanto elas gastam. Só o início de issues novas é barrado; a sessão em andamento sempre termina. Conta sem política
        fica com o piloto automático desligado.
      </p>
      {error && (
        <p role="alert" className="start-error">
          {error}
        </p>
      )}
      {accounts && accounts.length === 0 && (
        <p className="muted">Nenhuma conta conhecida: os ambientes informam a conta do Claude Code ao enviar ao hub.</p>
      )}
      {accounts && accounts.length > 0 && (
        <div className="field">
          <label htmlFor={`${id}-account`}>Conta</label>
          <select id={`${id}-account`} value={account} onChange={(e) => setAccount(e.target.value)}>
            {accounts.map((a) => (
              <option key={a.account.key} value={a.account.key}>
                {accountLabel(a)}
              </option>
            ))}
          </select>
        </div>
      )}
      {view && draft && (
        <>
          <p className={enabled ? "state-on" : "muted"} role="note">
            {enabled ? "Piloto automático ligado nesta conta." : "Piloto automático desligado nesta conta (sem política)."}
          </p>
          <div role="status" aria-label="Prévia" className="autopilot-preview">
            {preview ? (
              <>
                <p className="strong">{previewHeadline(preview)}</p>
                <p>
                  {preview.can_start
                    ? "Uma sessão autônoma nova pode começar agora."
                    : `Não inicia agora: ${preview.reasons.map(reasonText).join("; ")}.`}
                </p>
                {preview.weekly && (
                  <p className="muted small">
                    Semanal em {preview.weekly_used ?? 0}% · limite = mín(T {draft.weekly_cap}%, reserva{" "}
                    {preview.weekly.reserve_limit.toFixed(0)}% com {preview.weekly.workdays_left} dia(s) útil(eis) até o
                    reset, ritmo {preview.weekly.pace_limit.toFixed(1)}% com {(preview.weekly.pace * 100).toFixed(0)}% das
                    horas elegíveis)
                  </p>
                )}
                {nextOpenText(preview, draft.utc_offset_min) && (
                  <p className="muted small">{nextOpenText(preview, draft.utc_offset_min)}</p>
                )}
              </>
            ) : previewError ? (
              <p className="start-error">{previewError}</p>
            ) : (
              <p className="muted">Calculando a prévia…</p>
            )}
          </div>

          <div className="field-pair">
            <div className="field">
              <label htmlFor={`${id}-cap`}>Teto semanal T (%)</label>
              <input
                id={`${id}-cap`}
                type="number"
                min={0}
                max={100}
                value={draft.weekly_cap}
                onChange={(e) => patch({ weekly_cap: Number(e.target.value) })}
              />
            </div>
            <div className="field">
              <label htmlFor={`${id}-reserve`}>Reserva R por dia útil (%)</label>
              <input
                id={`${id}-reserve`}
                type="number"
                min={0}
                max={100}
                value={draft.reserve_per_workday}
                onChange={(e) => patch({ reserve_per_workday: Number(e.target.value) })}
              />
            </div>
            <div className="field">
              <label htmlFor={`${id}-cutoff`}>Corte de início nos dias úteis</label>
              <input
                id={`${id}-cutoff`}
                type="time"
                value={draft.workday_cutoff_min == null ? "" : minToHHMM(draft.workday_cutoff_min)}
                onChange={(e) => patch({ workday_cutoff_min: e.target.value ? hhmmToMin(e.target.value) : null })}
              />
            </div>
            <div className="field">
              <label htmlFor={`${id}-tz`}>Fuso</label>
              <select
                id={`${id}-tz`}
                value={draft.utc_offset_min}
                onChange={(e) => patch({ utc_offset_min: Number(e.target.value) })}
              >
                {OFFSETS.map((o) => (
                  <option key={o} value={o}>
                    {offsetLabel(o)}
                  </option>
                ))}
              </select>
            </div>
          </div>

          <h3>Faixas de horário</h3>
          <p className="muted hint">
            Cada faixa é fechada ou tem um teto da janela de 5h. Fim antes do início atravessa a meia-noite. Faixas marcadas
            como dia útil contam para a reserva e para o corte. Sobrepostas: vale a primeira da lista.
          </p>
          {draft.bands.map((b, i) => (
            <fieldset key={i} className="autopilot-band" aria-label={`Faixa ${b.label || i + 1}`}>
              <legend className="muted small">{bandSummary(b)}</legend>
              <div className="field-pair">
                <div className="field">
                  <label htmlFor={`${id}-b${i}-label`}>Nome</label>
                  <input
                    id={`${id}-b${i}-label`}
                    type="text"
                    value={b.label}
                    onChange={(e) => patchBand(i, { label: e.target.value })}
                  />
                </div>
                <div className="field">
                  <label htmlFor={`${id}-b${i}-start`}>Início</label>
                  <input
                    id={`${id}-b${i}-start`}
                    type="time"
                    value={minToHHMM(b.start_min)}
                    onChange={(e) => {
                      const v = hhmmToMin(e.target.value);
                      if (v != null) patchBand(i, { start_min: v });
                    }}
                  />
                </div>
                <div className="field">
                  <label htmlFor={`${id}-b${i}-end`}>Fim</label>
                  <input
                    id={`${id}-b${i}-end`}
                    type="time"
                    value={endInput(b.end_min)}
                    onChange={(e) => {
                      const v = hhmmToMin(e.target.value, true);
                      if (v != null) patchBand(i, { end_min: v });
                    }}
                  />
                </div>
                <div className="field">
                  <label htmlFor={`${id}-b${i}-cap`}>Teto da 5h (%)</label>
                  <input
                    id={`${id}-b${i}-cap`}
                    type="number"
                    min={0}
                    max={100}
                    disabled={b.five_hour_cap == null}
                    value={b.five_hour_cap ?? ""}
                    onChange={(e) => patchBand(i, { five_hour_cap: Number(e.target.value) })}
                  />
                </div>
              </div>
              <div className="autopilot-days" role="group" aria-label="Dias da semana">
                {DAY_LABELS.map((d, n) => (
                  <label key={d}>
                    <input
                      type="checkbox"
                      checked={b.days.includes(n)}
                      onChange={(e) =>
                        patchBand(i, {
                          days: e.target.checked ? [...b.days, n].sort((x, y) => x - y) : b.days.filter((x) => x !== n),
                        })
                      }
                    />{" "}
                    {d}
                  </label>
                ))}
              </div>
              <div className="autopilot-days">
                <label>
                  <input
                    type="checkbox"
                    checked={b.five_hour_cap == null}
                    onChange={(e) => patchBand(i, { five_hour_cap: e.target.checked ? null : 60 })}
                  />{" "}
                  Fechada
                </label>
                <label>
                  <input type="checkbox" checked={b.workday} onChange={(e) => patchBand(i, { workday: e.target.checked })} />{" "}
                  Dia útil
                </label>
                <button
                  type="button"
                  className="btn"
                  onClick={() => setDraft({ ...draft, bands: draft.bands.filter((_, j) => j !== i) })}
                >
                  Remover faixa
                </button>
              </div>
            </fieldset>
          ))}
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setDraft({ ...draft, bands: [...draft.bands, newBand()] })}>
              Adicionar faixa
            </button>
            <button type="button" className="btn" onClick={() => setDraft(clonePolicy(view.default))}>
              Restaurar padrão
            </button>
            {enabled && (
              <button type="button" className="btn btn-danger" disabled={busy} onClick={() => void save(null)}>
                Desligar piloto automático
              </button>
            )}
            <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void save(draft)}>
              {enabled ? "Salvar política" : "Ligar com esta política"}
            </button>
          </div>
          {msg && (
            <p role="status" className="state-on">
              {msg}
            </p>
          )}
        </>
      )}
    </section>
  );
}
