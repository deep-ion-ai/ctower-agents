import "../settings/settings.css";
import "./hub.css";
import { useCallback, useEffect, useRef, useState } from "react";
import { useNow } from "../App";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import type {
  DeliveryState,
  SecretChanged,
  SecretItem,
  SecretKind,
  SecretScope,
  SecretsList,
} from "../protocol/hub-gen";
import { errText } from "./calls";

/** Nome válido como variável de ambiente (o hub valida de novo). */
export const SECRET_NAME = /^[A-Z_][A-Z0-9_]*$/;

/**
 * Nomes que o ambiente já define para a sessão e que um secret não pode
 * sobrescrever (o hub recusa de novo). Mesma lista de
 * `ct_secrets::RESERVED_NAMES`/`RESERVED_PREFIXES` (issue #166).
 */
export const RESERVED_NAMES = [
  "PATH",
  "HOME",
  "USER",
  "LOGNAME",
  "SHELL",
  "TERM",
  "COLORTERM",
  "COLUMNS",
  "LINES",
  "PWD",
  "OLDPWD",
  "LANG",
  "CLAUDE_CONFIG_DIR",
  "CLAUDECODE",
];
export const RESERVED_PREFIXES = ["CTOWER_", "CTHUB_", "LC_", "LD_", "DYLD_"];

export function isReservedName(name: string): boolean {
  return RESERVED_NAMES.includes(name) || RESERVED_PREFIXES.some((p) => name.startsWith(p));
}

/**
 * Tamanho mínimo (em bytes UTF-8) de um secret para ser mascarado na saída
 * das sessões e no backup de transcript. Mesmo valor de
 * `ct_secrets::MIN_MASK_LEN` (issue #167).
 */
export const MIN_MASK_LEN = 8;

export const SHORT_SECRET_NOTE =
  "Valores com menos de 8 caracteres não são mascarados na saída das sessões nem no backup do transcript.";

/** O secret é curto demais para ser mascarado (não impede gravar). */
export function isTooShortToMask(value: string): boolean {
  return value !== "" && new TextEncoder().encode(value).length < MIN_MASK_LEN;
}

/** Quando uma mudança chega às sessões (issue #166). */
export const APPLIES_NOTE =
  "Vale a partir do próximo spawn ou resume, depois que o ambiente receber a configuração; sessões em andamento não mudam.";

const KIND_TEXT: Record<SecretKind, string> = { secret: "secret", variable: "variável" };

export const DELIVERY_TEXT: Record<DeliveryState, string> = {
  delivered: "entregue",
  pending: "pendente",
  no_key: "sem chave",
};

const DELIVERY_HINT: Record<DeliveryState, string> = {
  delivered: "o ambiente confirmou a revisão com a última mudança",
  pending: "o ambiente recebe no próximo pull",
  no_key: "o ambiente nunca mandou a chave pública: não recebe secrets",
};

interface Form {
  name: string;
  kind: SecretKind;
  value: string;
  /** Entrada que está sendo editada (`null` = nova). */
  editing: SecretItem | null;
}

const projectOf = (s: SecretScope) => (typeof s === "object" ? s.project : null);

/**
 * Secrets e variáveis do hub (`hub.secrets.*`, issue #165). Sem `project`,
 * o escopo global; com `project`, o do projeto, mostrando também os globais
 * herdados e quando o projeto sobrescreve um deles. A UI só escreve secrets:
 * o hub nunca devolve o valor de um, e editar é digitar um valor novo.
 * Mostra também, por ambiente, se a configuração com os secrets já chegou.
 */
export function HubSecrets({ project }: { project?: { owner: string; repo: string } }) {
  const { api, hub } = useTower();
  const key = project ? `${project.owner}/${project.repo}`.toLowerCase() : null;
  const scope: SecretScope = key ? { project: key } : "global";
  const id = key ? "secrets-projeto" : "secrets-global";
  const [list, setList] = useState<SecretsList | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [form, setForm] = useState<Form | null>(null);
  const [confirm, setConfirm] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const now = useNow();
  const seq = useRef(0);

  const load = useCallback(async () => {
    const n = ++seq.current;
    try {
      const l = await api.hubCall<SecretsList>("hub.secrets.list");
      if (n !== seq.current) return;
      setList(l);
      setLoadError(null);
    } catch (e) {
      if (n === seq.current) setLoadError(errText(e));
    }
  }, [api]);

  // O estado de entrega muda com os pulls dos ambientes: relê junto com a
  // visão do hub (que a UI já consulta periodicamente).
  useEffect(() => {
    void load();
  }, [load, hub.envs]);

  const items = list?.items ?? [];
  const globals = items.filter((i) => i.scope === "global");
  const own = key ? items.filter((i) => projectOf(i.scope) === key) : globals;
  const ownNames = new Set(own.map((i) => i.name));
  const globalNames = new Set(globals.map((i) => i.name));
  const delivery = (list?.delivery ?? []).filter((d) => !key || d.projects.includes(key));

  const nameError =
    form && !form.editing && form.name !== ""
      ? !SECRET_NAME.test(form.name)
        ? "Nome inválido: use letras maiúsculas, dígitos e _, sem começar por dígito (como variável de ambiente)."
        : isReservedName(form.name)
          ? `Nome reservado: o ambiente já define ${form.name} para a sessão (CTOWER_*, PATH, HOME, LC_*, LD_* e afins não podem ser sobrescritos).`
          : null
      : null;
  // Valor obrigatório para secret e para quem era secret (o valor antigo
  // não volta do hub); variável pode ficar vazia.
  const needsValue = form ? form.kind === "secret" || form.editing?.kind === "secret" : false;
  const canSave = !!form && form.name !== "" && !nameError && (!needsValue || form.value !== "");

  const save = async () => {
    if (!form || !canSave) return;
    setBusy(true);
    setError(null);
    setDone(null);
    try {
      const r = await api.hubCall<SecretChanged>("hub.secrets.set", {
        scope,
        name: form.name,
        kind: form.kind,
        value: form.value,
      });
      setDone(`${form.name} gravado (configuração rev ${r.rev}); os ambientes recebem no próximo pull. ${APPLIES_NOTE}`);
      setForm(null);
      await load();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (name: string) => {
    setBusy(true);
    setError(null);
    setDone(null);
    try {
      const r = await api.hubCall<SecretChanged>("hub.secrets.delete", { scope, name });
      setDone(`${name} removido (configuração rev ${r.rev}); os ambientes recebem no próximo pull. ${APPLIES_NOTE}`);
      setConfirm(null);
      await load();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  // Lista "a rotacionar na origem" depois de um `cthubd secrets restore`
  // com a chave de recuperação (issue #170): só no escopo global.
  const pending = !key && list?.rotate_pending && list.rotate_pending.items.length > 0 ? list.rotate_pending : null;

  const dismissPending = async () => {
    setBusy(true);
    setError(null);
    setDone(null);
    try {
      await api.hubCall("hub.secrets.rotate_pending_dismiss");
      setDone("Lista de secrets a rotacionar dispensada.");
      await load();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const changed = (i: SecretItem) => (i.updated_at ? `${ago(i.updated_at, now)} · rev ${i.rev}` : `rev ${i.rev}`);

  const valueCell = (i: SecretItem) =>
    i.kind === "secret" ? <span className="muted">•••• definido</span> : <span className="mono">{i.value ?? ""}</span>;

  // Na tela do projeto: as do projeto e, depois, os globais (herdados ou
  // sobrescritos). Nas Configurações do hub: só os globais.
  const rows: { item: SecretItem; inherited: boolean }[] = [
    ...own.map((item) => ({ item, inherited: false })),
    ...(key ? globals.map((item) => ({ item, inherited: true })) : []),
  ];

  return (
    <section aria-labelledby={id} className="panel settings-section">
      <div className="settings-head">
        <h2 id={id}>{key ? `Secrets e variáveis · ${key}` : "Secrets e variáveis"}</h2>
        <button
          type="button"
          className="btn"
          onClick={() => {
            setForm({ name: "", kind: "secret", value: "", editing: null });
            setError(null);
            setDone(null);
          }}
        >
          + Adicionar
        </button>
      </div>
      {loadError && (
        <p role="alert" className="start-error">
          {loadError}
        </p>
      )}
      {pending && (
        <div role="group" aria-label="Secrets a rotacionar na origem" className="confirm-box">
          <h3 className="hub-sub">Secrets a rotacionar na origem</h3>
          <p>
            O store foi restaurado com a chave de recuperação {ago(pending.restored_at, now)}, depois da perda da máquina do
            hub. Troque o valor destes secrets em quem os emitiu (tokens de API, por exemplo) e grave os novos aqui; a chave
            de recuperação usada deixou de valer.
          </p>
          <ul className="hub-list" aria-label="Secrets a rotacionar">
            {pending.items.map((i) => (
              <li key={`${projectOf(i.scope) ?? "global"}:${i.name}`}>
                <span className="mono strong">{i.name}</span>
                <span className="muted small">{projectOf(i.scope) ?? "global"}</span>
              </li>
            ))}
          </ul>
          <div className="form-actions">
            <button type="button" className="btn" disabled={busy} onClick={() => void dismissPending()}>
              Já rotacionei
            </button>
          </div>
        </div>
      )}
      {list && rows.length === 0 && (
        <p className="muted">
          {key ? "Nenhum secret ou variável neste projeto nem no escopo global." : "Nenhum secret ou variável global."}
        </p>
      )}
      {rows.length > 0 && (
        <div className="table-wrap">
          <table aria-label={key ? `Secrets e variáveis de ${key}` : "Secrets e variáveis globais"}>
            <thead>
              <tr>
                <th>Nome</th>
                <th>Tipo</th>
                <th>Valor</th>
                <th>Alterado</th>
                {key && <th>Origem</th>}
                <th>
                  <span className="sr-only">Ações</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map(({ item: i, inherited }) => (
                <tr key={`${inherited ? "g" : "p"}:${i.name}`} className={inherited && ownNames.has(i.name) ? "muted" : undefined}>
                  <td className="mono strong">{i.name}</td>
                  <td>{KIND_TEXT[i.kind]}</td>
                  <td>{valueCell(i)}</td>
                  <td>{changed(i)}</td>
                  {key && (
                    <td>
                      {!inherited ? (
                        globalNames.has(i.name) ? (
                          <span className="badge" data-tone="attention">
                            sobrescreve o global
                          </span>
                        ) : (
                          "projeto"
                        )
                      ) : ownNames.has(i.name) ? (
                        "global · sobrescrito pelo projeto"
                      ) : (
                        "global · herdado"
                      )}
                    </td>
                  )}
                  <td>
                    {!inherited && (
                      <div className="env-actions">
                        <button
                          type="button"
                          className="btn"
                          onClick={() => {
                            // Secret: campo vazio, nunca pré-preenchido.
                            setForm({ name: i.name, kind: i.kind, value: i.kind === "variable" ? i.value ?? "" : "", editing: i });
                            setError(null);
                            setDone(null);
                          }}
                        >
                          Editar <span className="sr-only">{i.name}</span>
                        </button>
                        <button type="button" className="btn btn-danger" onClick={() => setConfirm(i.name)}>
                          Remover <span className="sr-only">{i.name}</span>
                        </button>
                      </div>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {confirm && (
        <div role="group" aria-label={`Confirmar remoção de ${confirm}`} className="confirm-box">
          <p>
            Remover {confirm} {key ? `do projeto ${key}` : "do escopo global"}? Os ambientes deixam de recebê-lo no próximo
            pull
            {key && globalNames.has(confirm) ? "; o valor global volta a valer para o projeto" : ""}.
          </p>
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setConfirm(null)}>
              Cancelar
            </button>
            <button type="button" className="btn btn-danger" disabled={busy} onClick={() => remove(confirm)}>
              Remover {confirm}
            </button>
          </div>
        </div>
      )}
      {form && (
        <form
          className="env-form"
          aria-label={form.editing ? `Editar ${form.editing.name}` : "Novo secret ou variável"}
          onSubmit={(ev) => {
            ev.preventDefault();
            void save();
          }}
        >
          <div className="field">
            <label htmlFor={`${id}-name`}>Nome</label>
            <input
              id={`${id}-name`}
              type="text"
              className="mono"
              autoComplete="off"
              spellCheck={false}
              placeholder="API_TOKEN"
              readOnly={!!form.editing}
              value={form.name}
              aria-invalid={nameError ? true : undefined}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
            />
          </div>
          <div className="field">
            <label htmlFor={`${id}-kind`}>Tipo</label>
            <select
              id={`${id}-kind`}
              value={form.kind}
              onChange={(e) => {
                const kind = e.target.value as SecretKind;
                // Virar secret apaga o que estava no campo de texto.
                setForm({ ...form, kind, value: kind === "secret" && form.kind !== "secret" ? "" : form.value });
              }}
            >
              <option value="secret">Secret (só escrita)</option>
              <option value="variable">Variável (visível)</option>
            </select>
          </div>
          <div className="field hub-wide">
            <label htmlFor={`${id}-value`}>Valor</label>
            <input
              id={`${id}-value`}
              type={form.kind === "secret" ? "password" : "text"}
              autoComplete={form.kind === "secret" ? "new-password" : "off"}
              spellCheck={false}
              placeholder={
                form.editing?.kind === "secret" ? "digite o novo valor (o atual não é mostrado)" : ""
              }
              value={form.value}
              onChange={(e) => setForm({ ...form, value: e.target.value })}
            />
          </div>
          {form.kind === "secret" && isTooShortToMask(form.value) && (
            <p role="status" className="muted small hub-wide">
              {SHORT_SECRET_NOTE}
            </p>
          )}
          {nameError && (
            <p role="alert" className="start-error hub-wide">
              {nameError}
            </p>
          )}
          {error && (
            <p role="alert" className="start-error hub-wide">
              {error}
            </p>
          )}
          <div className="form-actions hub-wide">
            <button type="button" className="btn" onClick={() => setForm(null)}>
              Cancelar
            </button>
            <button type="submit" className="btn btn-primary" disabled={busy || !canSave}>
              Salvar
            </button>
          </div>
        </form>
      )}
      {!form && error && (
        <p role="alert" className="start-error">
          {error}
        </p>
      )}
      {done && (
        <p role="status" className="state-on">
          {done}
        </p>
      )}
      {list && (
        <>
          <h3 className="hub-sub">Entrega aos ambientes</h3>
          {delivery.length === 0 ? (
            <p className="muted">
              {key ? "Projeto não atribuído a nenhum ambiente." : "Nenhum ambiente ligado ao hub."}
            </p>
          ) : (
            <ul className="hub-list" aria-label="Entrega aos ambientes">
              {delivery.map((d) => (
                <li key={d.env_id}>
                  <span className="strong">{d.env_name}</span>
                  <span className="hub-state" data-state={d.state} title={DELIVERY_HINT[d.state]}>
                    {DELIVERY_TEXT[d.state]}
                  </span>
                  <span className="muted small">
                    {d.state === "no_key"
                      ? DELIVERY_HINT.no_key
                      : `aplicou rev ${d.applied_rev ?? "—"}; última mudança rev ${d.required_rev}`}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
      <p className="muted hint">
        Secrets ficam cifrados no hub e só saem cifrados para a chave de cada ambiente; esta tela grava, mas nunca mostra o
        valor de um secret. Variáveis podem ser lidas de volta. Vale o valor do projeto quando o mesmo nome existe nos dois
        escopos.
        {list && !list.status.recovery_configured && !key && " Recuperação não configurada: o store está cifrado só para a chave do hub (veja Chave de recuperação)."}
      </p>
    </section>
  );
}
