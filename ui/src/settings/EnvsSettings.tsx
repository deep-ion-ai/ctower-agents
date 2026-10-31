import { useState } from "react";
import type { EnvInput, EnvView } from "../api/types";
import { envTab } from "../env/sections";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";

const toInput = (e: EnvView): EnvInput => ({ id: e.id, name: e.name, url: e.url, kind: e.kind, distro: e.distro });

function slugId(name: string, taken: string[]): string {
  const base =
    name
      .toLowerCase()
      .normalize("NFD")
      .replace(/[̀-ͯ]/g, "")
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "") || "ambiente";
  let id = base;
  for (let i = 2; taken.includes(id); i++) id = `${base}-${i}`;
  return id;
}

/**
 * Ambientes (daemons `ctowerd`) que este app acompanha: lista, editar,
 * adicionar e remover. Cada um leva ao detalhe dele (#23); com `only`, só
 * esse ambiente (a seção Conexão do detalhe), sem adicionar.
 */
export function EnvsSettings({ only }: { only?: string } = {}) {
  const { envs, status, api, reloadEnvs } = useTower();
  const nav = useNav();
  const shown = only === undefined ? envs : envs.filter((e) => e.id === only);
  const [editing, setEditing] = useState<EnvInput | null>(null);
  const [isNew, setIsNew] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const save = async () => {
    if (!editing) return;
    const e = isNew ? { ...editing, id: slugId(editing.name, envs.map((x) => x.id)) } : editing;
    const all = isNew ? [...envs.map(toInput), e] : envs.map((x) => (x.id === e.id ? e : toInput(x)));
    try {
      await api.saveEnvs(all);
      setEditing(null);
      setError(null);
      await reloadEnvs();
    } catch (err) {
      setError(String(err));
    }
  };

  const remove = async (id: string) => {
    try {
      await api.saveEnvs(envs.filter((x) => x.id !== id).map(toInput));
      await reloadEnvs();
    } catch (err) {
      setError(String(err));
    }
  };

  return (
    <div className="set-stack">
      {only === undefined && (
        <div className="settings-head">
          <button
            type="button"
            className="btn"
            onClick={() => {
              setIsNew(true);
              setEditing({ id: "", name: "", url: "ws://", kind: "remote", distro: null, token: "" });
            }}
          >
            + Adicionar ambiente
          </button>
        </div>
      )}
      {shown.map((e) => {
        const st = status[e.id];
        return (
          <div key={e.id} className="env-row">
            <Fact label="Nome" value={e.name} strong />
            <Fact label="URL" value={e.url} mono />
            <Fact
              label="Token de UI"
              value={e.kind === "wsl" ? `lido da distro ${e.distro}` : e.has_token ? "•••• definido" : "não definido"}
              mono
            />
            <div className="fact">
              <span className="label">Estado</span>
              <span className={st?.connected ? "state-on" : "state-off"}>
                {!st ? "conectando…" : st.connected ? "conectado" : `desconectado · ${st.error ?? "tentando de novo"}`}
              </span>
            </div>
            <div className="env-actions">
              {only === undefined && (
                <button type="button" className="btn" onClick={() => nav.openTab(envTab(e.id, e.name))}>
                  Detalhes <span className="sr-only">de {e.name}</span>
                </button>
              )}
              <button
                type="button"
                className="btn"
                onClick={() => {
                  setIsNew(false);
                  setEditing({ ...toInput(e), token: "" });
                }}
              >
                Editar <span className="sr-only">{e.name}</span>
              </button>
              {envs.length > 1 && (
                <button type="button" className="btn btn-danger" onClick={() => remove(e.id)}>
                  Remover <span className="sr-only">{e.name}</span>
                </button>
              )}
            </div>
          </div>
        );
      })}
      {editing && (
        <form
          className="env-form"
          aria-label={isNew ? "Novo ambiente" : `Editar ${editing.name}`}
          onSubmit={(ev) => {
            ev.preventDefault();
            void save();
          }}
        >
          <div className="field">
            <label htmlFor="env-name">Nome</label>
            <input id="env-name" type="text" value={editing.name} onChange={(e) => setEditing({ ...editing, name: e.target.value })} />
          </div>
          <div className="field">
            <label htmlFor="env-kind">Tipo</label>
            <select
              id="env-kind"
              value={editing.kind}
              onChange={(e) => setEditing({ ...editing, kind: e.target.value as EnvInput["kind"] })}
            >
              <option value="wsl">WSL (token lido da distro)</option>
              <option value="remote">Remoto (token informado aqui)</option>
            </select>
          </div>
          <div className="field">
            <label htmlFor="env-url">URL</label>
            <input id="env-url" type="text" value={editing.url} onChange={(e) => setEditing({ ...editing, url: e.target.value })} />
          </div>
          {editing.kind === "wsl" ? (
            <div className="field">
              <label htmlFor="env-distro">Distro</label>
              <input
                id="env-distro"
                type="text"
                value={editing.distro ?? ""}
                onChange={(e) => setEditing({ ...editing, distro: e.target.value })}
              />
            </div>
          ) : (
            <div className="field">
              <label htmlFor="env-token">Token de UI</label>
              <input
                id="env-token"
                type="password"
                autoComplete="off"
                placeholder={isNew ? "" : "vazio mantém o atual se host e porta não mudarem"}
                value={editing.token ?? ""}
                onChange={(e) => setEditing({ ...editing, token: e.target.value })}
              />
            </div>
          )}
          {error && (
            <p role="alert" className="start-error">
              {error}
            </p>
          )}
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setEditing(null)}>
              Cancelar
            </button>
            <button type="submit" className="btn btn-primary">
              Salvar ambiente
            </button>
          </div>
        </form>
      )}
      <p className="muted hint">
        Ambiente WSL: ao abrir, o app roda <span className="mono">wsl -d &lt;distro&gt; --exec true</span> e lê o token
        de <span className="mono">~/.config/ctower/ui.token</span> na distro; ele nunca é gravado no Windows.
      </p>
    </div>
  );
}

function Fact({ label, value, mono, strong }: { label: string; value: string; mono?: boolean; strong?: boolean }) {
  return (
    <div className="fact">
      <span className="label">{label}</span>
      <span className={[mono ? "mono" : "", strong ? "strong" : ""].join(" ")}>{value}</span>
    </div>
  );
}

