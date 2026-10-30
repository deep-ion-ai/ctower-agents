import "../settings/settings.css";
import "./hub.css";
import { useCallback, useEffect, useRef, useState } from "react";
import { useNow } from "../App";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import type { RecoveryGenerated, RecoveryStatus } from "../protocol/hub-gen";
import { errText } from "./calls";

/** Etapa do fluxo: a chave gerada só existe no estado durante `show`. */
type Step = { name: "idle" } | { name: "show"; key: string; copied: boolean } | { name: "verify" };

const DAY_MS = 24 * 60 * 60 * 1000;

/** Texto curto de uma chave pública (`age1abc…xyz`). */
const shortKey = (k: string) => (k.length > 20 ? `${k.slice(0, 10)}…${k.slice(-6)}` : k);

/**
 * Chave de recuperação do store de secrets (`hub.recovery.*`, issue #169).
 * Gerar mostra a chave privada uma única vez, para o usuário guardar no
 * gerenciador de senhas; depois ele cola a chave de volta e o hub confere
 * que ela abre o store antes de considerá-la configurada. A chave sai do
 * estado do React ao deixar a etapa em que é mostrada, e a colada, ao
 * verificar ou cancelar. O hub guarda só a pública.
 */
export function HubRecovery() {
  const { api, hub, refreshHub } = useTower();
  const [status, setStatus] = useState<RecoveryStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [step, setStep] = useState<Step>({ name: "idle" });
  const [paste, setPaste] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const now = useNow();
  const seq = useRef(0);

  const load = useCallback(async () => {
    const n = ++seq.current;
    try {
      const s = await api.hubCall<RecoveryStatus>("hub.recovery.status");
      if (n !== seq.current) return;
      setStatus(s);
      setLoadError(null);
    } catch (e) {
      if (n === seq.current) setLoadError(errText(e));
    }
  }, [api]);

  // Relê junto com os avisos do hub (o lembrete vence com o tempo).
  useEffect(() => {
    void load();
  }, [load, hub.health]);

  const go = (s: Step) => {
    setStep(s);
    setPaste("");
    setError(null);
  };

  const generate = async () => {
    setBusy(true);
    setError(null);
    setDone(null);
    try {
      const r = await api.hubCall<RecoveryGenerated>("hub.recovery.generate");
      seq.current++;
      setStatus(r.status);
      go({ name: "show", key: r.private_key, copied: false });
      void refreshHub();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const copy = async () => {
    if (step.name !== "show") return;
    try {
      await api.writeClipboard(step.key);
      setStep({ ...step, copied: true });
    } catch (e) {
      setError(errText(e));
    }
  };

  const verify = async () => {
    const key = paste.trim();
    if (!key) return;
    setBusy(true);
    setError(null);
    setDone(null);
    try {
      const s = await api.hubCall<RecoveryStatus>("hub.recovery.verify", { key });
      seq.current++;
      setStatus(s);
      go({ name: "idle" });
      setDone("Chave verificada: ela abre o store de secrets. A recuperação está configurada.");
      void refreshHub();
    } catch (e) {
      setPaste("");
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const state = (s: RecoveryStatus) => {
    if (!s.configured)
      return s.pending ? (
        <span className="badge" data-tone="attention">
          gerada · falta verificar
        </span>
      ) : (
        <span className="badge" data-tone="error">
          não configurada
        </span>
      );
    return s.reverify_due ? (
      <span className="badge" data-tone="attention">
        configurada · verificação vencida
      </span>
    ) : (
      <span className="badge" data-tone="done">
        configurada
      </span>
    );
  };

  const verified = (s: RecoveryStatus) => {
    if (!s.configured) return null;
    const last = s.last_verified_at ? `verificada ${ago(s.last_verified_at, now)}` : "nunca verificada";
    const next =
      s.next_verify_at && s.reverify_days > 0
        ? s.next_verify_at > now
          ? `; próxima verificação em ${Math.ceil((s.next_verify_at - now) / DAY_MS)} dia(s)`
          : "; refaça a verificação"
        : "";
    return `${last}${next}`;
  };

  return (
    <section aria-labelledby="hub-recuperacao" className="panel settings-section">
      <div className="settings-head">
        <h2 id="hub-recuperacao">Chave de recuperação</h2>
        {status && state(status)}
      </div>
      {loadError && (
        <p role="alert" className="start-error">
          {loadError}
        </p>
      )}
      {status && (
        <div className="env-row">
          <div className="fact">
            <span className="label">Estado</span>
            <span>
              {status.configured
                ? verified(status)
                : status.pending
                  ? "gerada, mas ainda não verificada: o store segue cifrado só para a chave do hub"
                  : "não configurada: se a máquina do hub se perder, os secrets não voltam"}
            </span>
          </div>
          {status.public_key && (
            <div className="fact">
              <span className="label">Chave pública</span>
              <span className="mono" title={status.public_key}>
                {shortKey(status.public_key)}
              </span>
            </div>
          )}
          {status.configured && status.pending && status.pending_public_key && (
            <div className="fact">
              <span className="label">Troca em andamento</span>
              <span className="mono" title={status.pending_public_key}>
                {shortKey(status.pending_public_key)} · vale depois de verificar
              </span>
            </div>
          )}
        </div>
      )}

      {step.name === "show" && (
        <div role="group" aria-label="Chave de recuperação gerada" className="confirm-box">
          <p className="strong">Guarde no seu gerenciador de senhas.</p>
          <p>
            Esta chave aparece só agora: o hub guarda apenas a chave pública e não tem como mostrá-la de novo. Com ela, os
            snapshots dos secrets podem ser restaurados se esta máquina for perdida.
          </p>
          <code className="mono recovery-key" aria-label="Chave privada de recuperação">
            {step.key}
          </code>
          <p className="muted">
            Atenção à dependência circular: a senha mestra do gerenciador de senhas e o recovery dele não podem depender só
            desta máquina. É bom testar o acesso ao gerenciador pelo celular antes de seguir.
          </p>
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => void copy()}>
              {step.copied ? "Copiada" : "Copiar"}
            </button>
            <button type="button" className="btn" onClick={() => go({ name: "idle" })}>
              Cancelar
            </button>
            <button type="button" className="btn btn-primary" onClick={() => go({ name: "verify" })}>
              Já guardei: verificar
            </button>
          </div>
        </div>
      )}

      {step.name === "verify" && (
        <form
          className="env-form"
          aria-label="Verificar a chave de recuperação"
          onSubmit={(ev) => {
            ev.preventDefault();
            void verify();
          }}
        >
          <div className="field hub-wide">
            <label htmlFor="hub-recuperacao-chave">Cole a chave de recuperação</label>
            <input
              id="hub-recuperacao-chave"
              type="password"
              className="mono"
              autoComplete="off"
              spellCheck={false}
              placeholder="AGE-SECRET-KEY-1…"
              value={paste}
              onChange={(e) => setPaste(e.target.value)}
            />
          </div>
          <p className="muted hub-wide">
            Cole a chave a partir do gerenciador de senhas: o hub confere que ela abre o store de secrets e não a grava.
          </p>
          {error && (
            <p role="alert" className="start-error hub-wide">
              {error}
            </p>
          )}
          <div className="form-actions hub-wide">
            <button type="button" className="btn" onClick={() => go({ name: "idle" })}>
              Cancelar
            </button>
            <button type="submit" className="btn btn-primary" disabled={busy || paste.trim() === ""}>
              Verificar
            </button>
          </div>
        </form>
      )}

      {step.name === "idle" && status && (
        <div className="form-actions">
          {(status.pending || status.configured) && (
            <button type="button" className="btn" onClick={() => go({ name: "verify" })}>
              {status.pending ? "Verificar a chave gerada" : "Verificar de novo"}
            </button>
          )}
          <button
            type="button"
            className={status.configured ? "btn" : "btn btn-primary"}
            disabled={busy}
            onClick={() => void generate()}
          >
            {status.configured ? "Trocar a chave" : status.pending ? "Gerar outra" : "Gerar chave de recuperação"}
          </button>
        </div>
      )}
      {step.name !== "verify" && error && (
        <p role="alert" className="start-error">
          {error}
        </p>
      )}
      {done && (
        <p role="status" className="state-on">
          {done}
        </p>
      )}
      <p className="muted hint">
        A chave de recuperação fica fora desta máquina, no seu gerenciador de senhas (qualquer um). O store de secrets é
        cifrado para a chave do hub e para ela, e o hub lembra de refazer a verificação
        {status && status.reverify_days > 0 ? ` a cada ${status.reverify_days} dias` : ""}. A senha mestra do gerenciador e
        o recovery dele não podem depender só desta máquina.
      </p>
    </section>
  );
}
