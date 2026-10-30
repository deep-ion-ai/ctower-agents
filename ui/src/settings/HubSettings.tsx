import { useState } from "react";
import type { HubConnInput } from "../api/types";
import { useTower } from "../state/tower";

/**
 * Conexão com o hub (`cthubd`), separada das conexões com os ambientes:
 * URL e token de UI do hub. O token vai para o lado nativo do app e nunca
 * volta para a tela. Controle do item "Hub: Conexão" das Configurações.
 */
export function HubSettings() {
  const { hub, api, reloadHub, refreshHub } = useTower();
  const [editing, setEditing] = useState<HubConnInput | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const save = async (input: HubConnInput | null) => {
    setBusy(true);
    try {
      await api.saveHub(input);
      setEditing(null);
      setError(null);
      await reloadHub();
      void refreshHub();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const state = !hub.conn
    ? "não configurado"
    : !hub.status
      ? "conectando…"
      : hub.status.connected
        ? "conectado"
        : `indisponível · ${hub.status.error ?? "tentando de novo"}`;

  return (
    <div className="set-stack">
      <div className="settings-head">
        <div className="env-actions">
          {hub.conn && (
            <button type="button" className="btn btn-danger" disabled={busy} onClick={() => save(null)}>
              Remover conexão
            </button>
          )}
          <button
            type="button"
            className="btn"
            onClick={() => setEditing({ url: hub.conn?.url ?? "http://", token: "" })}
          >
            {hub.conn ? "Editar conexão" : "Configurar hub"}
          </button>
        </div>
      </div>
      {hub.unconfigured && (
        <p role="status" className="state-off">
          Há ambientes ligados a um hub, mas a conexão com ele não está configurada: o cadastro de projetos, as
          atribuições e a visão consolidada ficam indisponíveis.
        </p>
      )}
      <div className="env-row">
        <div className="fact">
          <span className="label">URL</span>
          <span className="mono">{hub.conn?.url ?? "—"}</span>
        </div>
        <div className="fact">
          <span className="label">Token de UI do hub</span>
          <span className="mono">{hub.conn ? (hub.conn.has_token ? "•••• definido" : "não definido") : "—"}</span>
        </div>
        <div className="fact">
          <span className="label">Estado</span>
          <span className={hub.connected ? "state-on" : "state-off"}>{state}</span>
        </div>
        {hub.info && (
          <div className="fact">
            <span className="label">{hub.info.name}</span>
            <span>
              cthubd {hub.info.version} · protocolo {hub.info.protocol} · configuração rev {hub.info.config_rev}
            </span>
          </div>
        )}
      </div>
      {editing && (
        <form
          className="env-form"
          aria-label="Conexão com o hub"
          onSubmit={(ev) => {
            ev.preventDefault();
            void save(editing);
          }}
        >
          <div className="field">
            <label htmlFor="hub-url">URL do hub</label>
            <input
              id="hub-url"
              type="text"
              placeholder="http://mac-mini.tailnet:7879"
              value={editing.url}
              onChange={(e) => setEditing({ ...editing, url: e.target.value })}
            />
          </div>
          <div className="field">
            <label htmlFor="hub-token">Token de UI do hub</label>
            <input
              id="hub-token"
              type="password"
              autoComplete="off"
              placeholder={hub.conn?.has_token ? "vazio mantém o atual se host e porta não mudarem" : ""}
              value={editing.token ?? ""}
              onChange={(e) => setEditing({ ...editing, token: e.target.value })}
            />
          </div>
          {error && (
            <p role="alert" className="start-error">
              {error}
            </p>
          )}
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setEditing(null)}>
              Cancelar
            </button>
            <button type="submit" className="btn btn-primary" disabled={busy}>
              Salvar hub
            </button>
          </div>
        </form>
      )}
      {!editing && error && (
        <p role="alert" className="start-error">
          {error}
        </p>
      )}
      <p className="muted hint">
        O hub (<span className="mono">cthubd</span>, porta 7879) guarda os projetos, as atribuições e a visão consolidada;
        as sessões, o terminal e a recuperação continuam em cada ambiente. O token de UI do hub fica em{" "}
        <span className="mono">~/.config/cthub/ui.token</span> na máquina do hub e é outro que o dos ambientes; ao
        trocar o host ou a porta da URL, informe o token de novo (ele nunca segue para outro destino). A URL
        aceita <span className="mono">http://host:7879</span> (como o <span className="mono">hub_url</span> do ambiente)
        ou <span className="mono">ws://host:7879/ws</span>.
      </p>
    </div>
  );
}
