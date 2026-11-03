import { useCallback, useEffect, useState } from "react";
import type { EnvCredential } from "../protocol/gen";
import { useTower } from "../state/tower";

// Comandos de credencial do Jira e do GitLab por ambiente (#143):
// `env.credentials.get` lista os projetos Jira/GitLab atribuídos ao ambiente
// com o comando do cadastro (hub) e o deste ambiente; `env.credentials.set`
// grava em `[credentials."owner/repo"]` do config.toml e vale no próximo uso,
// sem restart. Vazio = o comando do cadastro. Os outros ambientes e o
// cadastro do hub não mudam.

/** Mesmo limite do daemon (`MAX_CREDENTIAL_CMD`). */
export const MAX_CREDENTIAL_CMD = 1000;

export function validCredentialCmd(c: string): boolean {
  return !/[\r\n]/.test(c) && c.length <= MAX_CREDENTIAL_CMD;
}

export interface TrackerCredentials {
  /** `null` = carregando, sem conexão ou daemon sem `env.credentials.*` (ver `error`). */
  rows: EnvCredential[] | null;
  error: string | null;
  save(row: EnvCredential, tracker_cmd: string, forge_cmd: string): Promise<string | null>;
}

/** `env.credentials.get` do ambiente, relido ao (re)conectar. */
export function useTrackerCredentials(env: string | undefined): TrackerCredentials {
  const { api, status } = useTower();
  const on = !!env && !!status[env]?.connected;
  const [rows, setRows] = useState<EnvCredential[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setRows(null);
    setError(null);
    if (!env || !on) return;
    let alive = true;
    api.call<EnvCredential[]>(env, "env.credentials.get").then(
      (list) => alive && setRows(list),
      (e) => alive && setError(String(e)),
    );
    return () => {
      alive = false;
    };
  }, [api, env, on]);

  const save = useCallback(
    async (row: EnvCredential, tracker_cmd: string, forge_cmd: string) => {
      if (!env) return "sem ambiente";
      try {
        const list = await api.call<EnvCredential[]>(env, "env.credentials.set", {
          owner: row.owner,
          repo: row.repo,
          tracker_cmd: tracker_cmd.trim(),
          forge_cmd: forge_cmd.trim(),
        });
        setRows(list);
        return null;
      } catch (e) {
        return String(e);
      }
    },
    [api, env],
  );

  return { rows, error, save };
}

/** Projetos com sobrescrita neste ambiente (marcador de alterada). */
export const credentialsChanged = (c: TrackerCredentials) =>
  !!c.rows?.some((r) => r.tracker_cmd.trim() !== "" || r.forge_cmd.trim() !== "");

/** Situação da última leitura com a credencial neste ambiente. */
export function credentialStatus(r: EnvCredential): { ok: boolean; text: string } | null {
  const source = (fromEnv: boolean | undefined) => (fromEnv ? "do ambiente" : "do cadastro");
  const failures = r.sync?.failures ?? 0;
  if (r.readiness) {
    const j = r.readiness.jira;
    const g = r.readiness.gitlab;
    const parts: string[] = [];
    if (r.tracker === "jira") {
      parts.push(
        j.credential_ok
          ? `Jira: credencial ${source(j.credential_from_env)} ok`
          : `Jira: credencial ${source(j.credential_from_env)} falhou${j.error ? `: ${j.error}` : ""}`,
      );
    }
    if (r.forge === "gitlab") {
      parts.push(
        g.credential_ok
          ? `GitLab: credencial ${source(g.credential_from_env)} ok`
          : `GitLab: credencial ${source(g.credential_from_env)} falhou${g.error ? `: ${g.error}` : ""}`,
      );
    }
    const ok = (r.tracker !== "jira" || j.credential_ok) && (r.forge !== "gitlab" || g.credential_ok);
    if (failures > 0) parts.push(`sync falhando há ${failures} ciclo(s)`);
    return { ok: ok && failures === 0, text: parts.join(" · ") };
  }
  if (r.sync?.last_error && failures > 0) {
    return { ok: false, text: `sync falhando há ${failures} ciclo(s): ${r.sync.last_error}` };
  }
  return null;
}

function CredentialRow({ row, save }: { row: EnvCredential; save: TrackerCredentials["save"] }) {
  const [tracker, setTracker] = useState(row.tracker_cmd);
  const [forge, setForge] = useState(row.forge_cmd);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => setTracker(row.tracker_cmd), [row.tracker_cmd]);
  useEffect(() => setForge(row.forge_cmd), [row.forge_cmd]);
  const jira = row.tracker === "jira";
  const gitlab = row.forge === "gitlab";
  const valid = validCredentialCmd(tracker.trim()) && validCredentialCmd(forge.trim());
  const dirty = tracker.trim() !== row.tracker_cmd || forge.trim() !== row.forge_cmd;
  const status = credentialStatus(row);

  const run = async (t: string, f: string) => {
    setBusy(true);
    setError(await save(row, t, f));
    setBusy(false);
  };

  return (
    <fieldset className="set-stack" aria-label={`Credenciais de ${row.name}`}>
      <legend>
        <strong>{row.name}</strong>{" "}
        <span className="muted mono">
          {row.owner}/{row.repo}
        </span>
      </legend>
      {jira && (
        <label className="set-stack">
          <span>Comando de credencial do Jira neste ambiente</span>
          <input
            aria-label={`Comando do Jira de ${row.name}`}
            className="set-input mono"
            value={tracker}
            placeholder={row.project_tracker_cmd || "o comando do cadastro"}
            aria-invalid={!validCredentialCmd(tracker.trim())}
            onChange={(e) => setTracker(e.target.value)}
          />
          <span className="muted hint">
            Do cadastro: <span className="mono">{row.project_tracker_cmd || "(vazio)"}</span>
          </span>
        </label>
      )}
      {gitlab && (
        <label className="set-stack">
          <span>Comando de credencial do GitLab neste ambiente</span>
          <input
            aria-label={`Comando do GitLab de ${row.name}`}
            className="set-input mono"
            value={forge}
            placeholder={row.project_forge_cmd || "o comando do cadastro"}
            aria-invalid={!validCredentialCmd(forge.trim())}
            onChange={(e) => setForge(e.target.value)}
          />
          <span className="muted hint">
            Do cadastro: <span className="mono">{row.project_forge_cmd || "(vazio)"}</span>
          </span>
        </label>
      )}
      {!valid && <span className="state-off hint">Uma linha só, até {MAX_CREDENTIAL_CMD} caracteres.</span>}
      <div className="set-inline">
        <button type="button" className="btn btn-primary" disabled={busy || !dirty || !valid} onClick={() => run(tracker, forge)}>
          Salvar
        </button>
        <button
          type="button"
          className="btn"
          disabled={busy || (row.tracker_cmd === "" && row.forge_cmd === "")}
          title="Volta a usar os comandos do cadastro do projeto"
          onClick={() => run("", "")}
        >
          Usar o do cadastro
        </button>
        {status && (
          <span role="status" className={status.ok ? "muted" : "state-off"}>
            {status.text}
          </span>
        )}
      </div>
      {error && (
        <span role="alert" className="state-off hint">
          {error}
        </span>
      )}
    </fieldset>
  );
}

/** Lista dos projetos Jira/GitLab do ambiente com os comandos editáveis. */
export function TrackerCredentialsControl({ creds, connected }: { creds: TrackerCredentials; connected: boolean }) {
  if (!connected) return <span className="muted">sem conexão</span>;
  if (creds.error)
    return (
      <span className="muted">
        Este daemon não edita daqui ({creds.error}): use <span className="mono">[credentials."owner/repo"]</span> no
        config.toml do ambiente.
      </span>
    );
  if (!creds.rows) return <span className="muted">carregando…</span>;
  if (creds.rows.length === 0)
    return <span className="muted">Nenhum projeto Jira ou GitLab atribuído a este ambiente.</span>;
  return (
    <div className="set-stack">
      {creds.rows.map((r) => (
        <CredentialRow key={`${r.owner}/${r.repo}`} row={r} save={creds.save} />
      ))}
    </div>
  );
}
