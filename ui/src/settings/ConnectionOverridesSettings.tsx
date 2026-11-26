import { useCallback, useEffect, useState } from "react";
import { CMD_HINT, KIND_LABEL } from "../hub/connections";
import type { EnvConnection, HubSource } from "../protocol/gen";
import { useTower } from "../state/tower";

// Sobrescrita de credencial por conexão neste ambiente (#276):
// `env.connections.get` lista as conexões do hub que o ambiente conhece, com a
// fonte do hub (sem valor) e o comando deste ambiente; `env.connections.set`
// grava em `[connections."nome"] credential_cmd` do config.toml e vale no
// próximo uso, sem reiniciar. Vazio = a fonte do hub. Os outros ambientes e o
// cadastro do hub não mudam. Substitui a edição por projeto (`[credentials]`,
// #143) e o token do GitHub do ambiente (`github.token`).

/** Mesmo limite do daemon (`MAX_CREDENTIAL_CMD`). */
export const MAX_CREDENTIAL_CMD = 1000;

export function validCredentialCmd(c: string): boolean {
  return !/[\r\n]/.test(c) && c.length <= MAX_CREDENTIAL_CMD;
}

export interface EnvConnections {
  /** `null` = carregando, sem conexão ou daemon sem `env.connections.*` (ver `error`). */
  rows: EnvConnection[] | null;
  error: string | null;
  save(name: string, credential_cmd: string): Promise<string | null>;
}

/** `env.connections.get` do ambiente, relido ao (re)conectar. */
export function useEnvConnections(env: string | undefined): EnvConnections {
  const { api, status } = useTower();
  const on = !!env && !!status[env]?.connected;
  const [rows, setRows] = useState<EnvConnection[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setRows(null);
    setError(null);
    if (!env || !on) return;
    let alive = true;
    api.call<EnvConnection[]>(env, "env.connections.get").then(
      (list) => alive && setRows(list),
      (e) => alive && setError(String(e)),
    );
    return () => {
      alive = false;
    };
  }, [api, env, on]);

  const save = useCallback(
    async (name: string, credential_cmd: string) => {
      if (!env) return "sem ambiente";
      try {
        setRows(await api.call<EnvConnection[]>(env, "env.connections.set", { name, credential_cmd: credential_cmd.trim() }));
        return null;
      } catch (e) {
        return String(e);
      }
    },
    [api, env],
  );

  return { rows, error, save };
}

/** Conexões com sobrescrita neste ambiente (marcador de alterada). */
export const connectionsChanged = (c: EnvConnections) => !!c.rows?.some((r) => r.credential_cmd.trim() !== "");

/** De onde vem a credencial no hub, sem valor. */
export function hubSourceText(s: HubSource | null): string {
  if (!s) return "não veio para este ambiente (nenhum projeto daqui usa a conexão)";
  switch (s.kind) {
    case "command":
      return "comando do hub";
    case "secret":
      return `secret do hub ${s.detail ?? ""}`.trim();
    case "file":
      return `arquivo ${s.detail ?? ""}`.trim();
    case "cli":
      return "login da CLI (gh auth token)";
    case "none":
      return "sem autenticação";
  }
}

function ConnectionRow({ row, save }: { row: EnvConnection; save: EnvConnections["save"] }) {
  const [cmd, setCmd] = useState(row.credential_cmd);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => setCmd(row.credential_cmd), [row.credential_cmd]);
  const valid = validCredentialCmd(cmd.trim());
  const dirty = cmd.trim() !== row.credential_cmd;
  const kind = row.kind ? KIND_LABEL[row.kind] : null;

  const run = async (c: string) => {
    setBusy(true);
    setError(await save(row.name, c));
    setBusy(false);
  };

  return (
    <fieldset className="set-stack" aria-label={`Credencial de ${row.name}`}>
      <legend>
        <strong className="mono">{row.name}</strong>
        {kind && <span className="muted"> · {kind}</span>}
        {row.endpoint && <span className="muted mono"> · {row.endpoint}</span>}
      </legend>
      {!row.in_hub ? (
        <span role="alert" className="state-off hint">
          Esta conexão não existe no hub (renomeada ou apagada): a sobrescrita não vale.
        </span>
      ) : (
        <>
          <span className="muted hint">
            Do hub: {hubSourceText(row.hub_source)}
            {row.projects.length > 0 && <> · usada por {row.projects.join(", ")}</>}
          </span>
          {row.file_missing && (
            <span role="alert" className="state-off hint">
              O arquivo {row.hub_source?.detail} não existe neste ambiente: o uso da conexão aqui falha. Configure o
              comando deste ambiente{row.kind === "github" ? " (por exemplo, gh auth token)" : ""}.
            </span>
          )}
          <label className="set-stack">
            <span>Comando de credencial neste ambiente</span>
            <input
              aria-label={`Comando de ${row.name} neste ambiente`}
              className="set-input mono"
              value={cmd}
              placeholder={row.kind ? `vazio = a fonte do hub; ex.: ${CMD_HINT[row.kind]}` : "vazio = a fonte do hub"}
              aria-invalid={!valid}
              onChange={(e) => setCmd(e.target.value)}
            />
          </label>
          {!valid && <span className="state-off hint">Uma linha só, até {MAX_CREDENTIAL_CMD} caracteres.</span>}
        </>
      )}
      <div className="set-inline">
        {row.in_hub && (
          <button type="button" className="btn btn-primary" disabled={busy || !dirty || !valid} onClick={() => run(cmd)}>
            Salvar
          </button>
        )}
        <button
          type="button"
          className="btn"
          disabled={busy || row.credential_cmd === ""}
          title="Apaga o comando deste ambiente: volta a valer a fonte do hub"
          onClick={() => run("")}
        >
          {row.in_hub ? "Usar a do hub" : "Apagar"}
        </button>
      </div>
      {error && (
        <span role="alert" className="state-off hint">
          {error}
        </span>
      )}
    </fieldset>
  );
}

/** As conexões do ambiente com o comando editável. */
export function ConnectionOverridesControl({ conns, connected }: { conns: EnvConnections; connected: boolean }) {
  if (!connected) return <span className="muted">sem conexão</span>;
  if (conns.error)
    return (
      <span className="muted">
        Este daemon não edita daqui ({conns.error}): use <span className="mono">[connections."nome"] credential_cmd</span>{" "}
        no config.toml do ambiente.
      </span>
    );
  if (!conns.rows) return <span className="muted">carregando…</span>;
  if (conns.rows.length === 0)
    return <span className="muted">Nenhuma conexão do hub chegou a este ambiente (Hub › Conexões).</span>;
  return (
    <div className="set-stack">
      {conns.rows.map((r) => (
        <ConnectionRow key={r.name} row={r} save={conns.save} />
      ))}
    </div>
  );
}
