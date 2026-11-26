import "../settings/settings.css";
import "./hub.css";
import { useState } from "react";
import { useTower } from "../state/tower";
import type { Connection } from "../protocol/gen";
import { errText } from "./calls";
import {
  CMD_HINT,
  CONNECTION_KINDS,
  CRED_KINDS,
  ENDPOINT_HINT,
  EMPTY_CONNECTION_FORM,
  KIND_LABEL,
  connectionFormOf,
  connectionParams,
  credentialText,
  readinessText,
  type ConnectionForm,
  type ConnectionTest,
  type CredKind,
} from "./connections";
import { useHubConnections } from "./useConnections";

interface Editing {
  /** Conexão existente (`undefined` = nova). */
  id?: number;
  form: ConnectionForm;
}

const CRED_TEXT: Record<CredKind, string> = {
  command: "Comando que imprime o token",
  secret: "Nome do secret do hub",
  cli: "Login da CLI",
  file: "Caminho do arquivo com o token",
  none: "Sem autenticação",
};

/**
 * Conexões do hub (#238): lista, criação, edição, teste e remoção
 * (`hub.connections.*`, desktop). O teste confere só o cadastro, sem rede; a
 * identidade na rede ("quem sou eu") fica com o ambiente, que tem a credencial.
 * Os projetos escolhem a conexão no cadastro, em vez de digitar URL e credencial.
 */
export function HubConnections() {
  const { api } = useTower();
  const { connections, error: loadError, reload } = useHubConnections();
  const [editing, setEditing] = useState<Editing | null>(null);
  const [tests, setTests] = useState<Record<number, ConnectionTest | string>>({});
  const [confirm, setConfirm] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);

  const save = async () => {
    if (!editing) return;
    const p = connectionParams(editing.form, editing.id);
    if ("error" in p) {
      setError(p.error);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const method = editing.id === undefined ? "hub.connections.add" : "hub.connections.update";
      await api.hubCall(method, p.params);
      setDone(`Conexão ${editing.form.name.trim()} salva.`);
      setEditing(null);
      await reload();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const test = async (c: Connection) => {
    setError(null);
    try {
      const r = await api.hubCall<ConnectionTest>("hub.connections.test", { id: c.id });
      setTests((t) => ({ ...t, [c.id]: r }));
    } catch (e) {
      setTests((t) => ({ ...t, [c.id]: errText(e) }));
    }
  };

  const remove = async (c: Connection) => {
    setBusy(true);
    setError(null);
    try {
      await api.hubCall("hub.connections.delete", { id: c.id });
      setConfirm(null);
      setDone(`Conexão ${c.name} removida.`);
      await reload();
    } catch (e) {
      // O hub recusa a conexão em uso e diz quais projetos a usam.
      setError(errText(e));
      setConfirm(null);
    } finally {
      setBusy(false);
    }
  };

  return (
    <section aria-labelledby="hub-conexoes" className="panel settings-section">
      <h2 id="hub-conexoes">Conexões</h2>
      <p className="muted hint">
        Conexões com GitHub, GitLab e Jira, usadas pelos projetos. O hub guarda só como obter o token (nome de secret,
        comando ou caminho de arquivo): o valor nunca aparece aqui. Os ambientes recebem as conexões dos projetos
        atribuídos a eles.
      </p>
      <p className="muted hint">
        Quem sou eu (a conta na rede) não entra no teste: o teste confere só o cadastro, sem rede. A conferência na
        rede é do ambiente, que tem a credencial.
      </p>

      {error && (
        <p role="alert" className="start-error">
          {error}
        </p>
      )}
      {done && !error && (
        <p role="status" className="muted">
          {done}
        </p>
      )}
      {loadError && (
        <p role="alert" className="start-error">
          {loadError}
        </p>
      )}

      {connections === null && !loadError && <p className="muted">Carregando conexões…</p>}
      {connections?.length === 0 && <p className="muted">Nenhuma conexão ainda. Crie a primeira para usar nos projetos.</p>}

      {connections && connections.length > 0 && (
        <ul className="set-stack" aria-label="Conexões cadastradas">
          {connections.map((c) => {
            const t = tests[c.id];
            const r = t && typeof t !== "string" ? readinessText(t.readiness) : null;
            return (
              <li key={c.id} className="hub-card">
                <div className="hub-card-head">
                  <h3>{c.name}</h3>
                  <span className="muted">{KIND_LABEL[c.kind]}</span>
                </div>
                <dl className="hub-kv">
                  <div>
                    <dt>Endereço</dt>
                    <dd className="mono">{c.endpoint || "padrão do tipo"}</dd>
                  </div>
                  <div>
                    <dt>Credencial</dt>
                    <dd>{credentialText(c.credential)}</dd>
                  </div>
                  {typeof c.options?.config_file === "string" && c.options.config_file && (
                    <div>
                      <dt>Configuração do jira-cli</dt>
                      <dd className="mono">{c.options.config_file}</dd>
                    </div>
                  )}
                </dl>
                <div className="form-actions">
                  <button type="button" className="btn" disabled={busy} onClick={() => void test(c)}>
                    Testar
                  </button>
                  <button
                    type="button"
                    className="btn"
                    disabled={busy}
                    onClick={() => setEditing({ id: c.id, form: connectionFormOf(c) })}
                  >
                    Editar
                  </button>
                  {confirm === c.id ? (
                    <button type="button" className="btn btn-danger" disabled={busy} onClick={() => void remove(c)}>
                      Confirmar exclusão de {c.name}
                    </button>
                  ) : (
                    <button type="button" className="btn" disabled={busy} onClick={() => setConfirm(c.id)}>
                      Excluir
                    </button>
                  )}
                </div>
                {r && (
                  <p role="status" className={r.ok ? "muted" : "start-error"}>
                    Prontidão: {r.text}
                  </p>
                )}
                {typeof t === "string" && (
                  <p role="alert" className="start-error">
                    {t}
                  </p>
                )}
              </li>
            );
          })}
        </ul>
      )}

      {editing ? (
        <ConnectionFormView
          editing={editing}
          busy={busy}
          onChange={setEditing}
          onCancel={() => {
            setEditing(null);
            setError(null);
          }}
          onSave={() => void save()}
        />
      ) : (
        <div className="form-actions">
          <button
            type="button"
            className="btn btn-primary"
            disabled={busy}
            onClick={() => setEditing({ form: { ...EMPTY_CONNECTION_FORM } })}
          >
            Nova conexão
          </button>
        </div>
      )}
    </section>
  );
}

function ConnectionFormView({
  editing,
  busy,
  onChange,
  onCancel,
  onSave,
}: {
  editing: Editing;
  busy: boolean;
  onChange: (e: Editing) => void;
  onCancel: () => void;
  onSave: () => void;
}) {
  const { id, form: f } = editing;
  const prefix = id === undefined ? "conn-new" : `conn-${id}`;
  const set = (patch: Partial<ConnectionForm>) => onChange({ ...editing, form: { ...f, ...patch } });
  // Trocar o tipo troca a forma de credencial que ele aceita.
  const kinds = CRED_KINDS[f.kind];
  const credKind = kinds.includes(f.cred) ? f.cred : kinds[0];
  const valueLabel =
    credKind === "secret"
      ? "Nome do secret do hub"
      : credKind === "file"
        ? "Caminho do arquivo com o token"
        : credKind === "command"
          ? "Comando que imprime o token"
          : null;

  return (
    <form
      className="env-form hub-card-form"
      aria-label={id === undefined ? "Nova conexão" : `Editar ${f.name}`}
      onSubmit={(e) => {
        e.preventDefault();
        onSave();
      }}
    >
      {id === undefined ? (
        <div className="field">
          <label htmlFor={`${prefix}-kind`}>Tipo</label>
          <select
            id={`${prefix}-kind`}
            value={f.kind}
            onChange={(e) => set({ kind: e.target.value as ConnectionForm["kind"], cred: CRED_KINDS[e.target.value as ConnectionForm["kind"]][0] })}
          >
            {CONNECTION_KINDS.map((k) => (
              <option key={k} value={k}>
                {KIND_LABEL[k]}
              </option>
            ))}
          </select>
        </div>
      ) : (
        <div className="field">
          <span className="label">Tipo (não muda)</span>
          <span>{KIND_LABEL[f.kind]}</span>
        </div>
      )}
      <div className="field">
        <label htmlFor={`${prefix}-name`}>Nome</label>
        <input
          id={`${prefix}-name`}
          type="text"
          placeholder={`${f.kind}-empresa`}
          value={f.name}
          onChange={(e) => set({ name: e.target.value })}
        />
        <span className="hint muted">Único no hub: minúsculas, dígitos, `-`, `_` ou `.`.</span>
      </div>
      <div className="field">
        <label htmlFor={`${prefix}-endpoint`}>{f.kind === "gitlab" ? "Host" : f.kind === "jira" ? "URL" : "API"}</label>
        <input
          id={`${prefix}-endpoint`}
          type="text"
          placeholder={ENDPOINT_HINT[f.kind]}
          value={f.endpoint}
          onChange={(e) => set({ endpoint: e.target.value })}
        />
      </div>
      <div className="field">
        <label htmlFor={`${prefix}-cred`}>Credencial</label>
        <select id={`${prefix}-cred`} value={credKind} onChange={(e) => set({ cred: e.target.value as CredKind, cred_value: "" })}>
          {kinds.map((k) => (
            <option key={k} value={k}>
              {CRED_TEXT[k]}
            </option>
          ))}
        </select>
      </div>
      {valueLabel && (
        <div className="field hub-wide">
          <label htmlFor={`${prefix}-value`}>{valueLabel}</label>
          <input
            id={`${prefix}-value`}
            type="text"
            autoComplete="off"
            placeholder={credKind === "command" ? CMD_HINT[f.kind] : credKind === "file" ? "~/.config/…/token" : "NOME_DO_SECRET"}
            value={f.cred_value}
            onChange={(e) => set({ cred_value: e.target.value })}
          />
          <span className="hint muted">O comando roda no ambiente: a senha e o token nunca passam pelo hub.</span>
        </div>
      )}
      {f.kind === "jira" && (
        <div className="field hub-wide">
          <label htmlFor={`${prefix}-config`}>Configuração do jira-cli (opcional)</label>
          <input
            id={`${prefix}-config`}
            type="text"
            placeholder="vazio = ~/.config/.jira/.config.yml"
            value={f.config_file}
            onChange={(e) => set({ config_file: e.target.value })}
          />
        </div>
      )}
      <div className="form-actions">
        <button type="button" className="btn" onClick={onCancel}>
          Cancelar
        </button>
        <button type="submit" className="btn btn-primary" disabled={busy}>
          {busy ? "Salvando…" : "Salvar conexão"}
        </button>
      </div>
    </form>
  );
}
