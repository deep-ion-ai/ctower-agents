import "./agent-rules.css";
import { useCallback, useEffect, useId, useRef, useState, type ChangeEvent } from "react";
import type {
  HubRuleVersion,
  HubRules,
  HubRulesHistory,
  HubRulesProposal,
  HubRulesProposals,
} from "../protocol/hub-gen";
import { useTower } from "../state/tower";
import { diffStats, lineDiff } from "../util/diff";
import { ago } from "../util/time";
import { errText } from "./calls";
import { DesktopOnly } from "../web/DesktopOnly";

/** Tamanho em bytes de UTF-8, o que o hub conta para o limite. */
export const utf8Bytes = (s: string) => new TextEncoder().encode(s).length;

/** Texto de um arquivo escolhido (pelo `FileReader`, que também existe no jsdom dos testes). */
export function readText(f: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result ?? ""));
    r.onerror = () => reject(r.error ?? new Error("falha ao ler o arquivo"));
    r.readAsText(f);
  });
}

/** "edição do usuário", "importação de CLAUDE.md", "proposta #12 do agente (wsl)". */
export function sourceText(v: HubRuleVersion): string {
  switch (v.source) {
    case "usuario":
      return "edição do usuário";
    case "importacao":
      return "importação de CLAUDE.md";
    case "proposta": {
      const who = v.author_env_name ?? v.author_env_id;
      return `proposta #${v.proposal_id ?? "?"} do agente${who ? ` (${who})` : ""}, aprovada pelo usuário`;
    }
  }
}

/** Situação do tamanho em relação ao aviso e ao limite. */
export function sizeState(bytes: number, warn: number, max: number): "ok" | "warn" | "over" {
  return bytes > max ? "over" : bytes >= warn ? "warn" : "ok";
}

interface Draft {
  text: string;
  /** Importação de um `CLAUDE.md` (arquivo ou colado): a versão registra a origem. */
  imported: boolean;
  note: string;
  /** Versão que o editor abriu (o hub recusa se a vigente mudou). */
  base: number;
}

const MAX_DEFAULT = 16 * 1024;
const WARN_DEFAULT = 12 * 1024;

/**
 * Regras do agente de um escopo (#357, spec do Jira e GitLab §10): as do
 * projeto (`epic = 0`, em Hub › Projetos) ou as de um épico (no item do
 * épico, na tela Projeto). Mostra a versão vigente, edita em Markdown (ou
 * importa um `CLAUDE.md`), o histórico com a origem e as propostas
 * pendentes do agente com a diferença contra o texto vigente.
 */
export function AgentRules({
  owner,
  repo,
  epic,
  label,
  now,
  className,
}: {
  owner: string;
  repo: string;
  /** 0 = regras do projeto. */
  epic: number;
  /** Nome do escopo: o do projeto ou a referência do épico. */
  label: string;
  now: number;
  /** Classe do cartão de quem usa (`hub-card` ou `sec iv-card`). */
  className: string;
}) {
  // Na web (#205) as regras são só leitura: o `/web/ws` recusa editar,
  // importar e decidir propostas.
  const { api, hub, web } = useTower();
  const id = useId();
  const [rules, setRules] = useState<HubRules | null>(null);
  const [pending, setPending] = useState<HubRulesProposal[]>([]);
  /** Nas regras do projeto: propostas pendentes de épicos do mesmo projeto. */
  const [epicPending, setEpicPending] = useState<HubRulesProposal[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ text: string; error: boolean } | null>(null);
  const [history, setHistory] = useState<HubRulesHistory | null>(null);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [historyError, setHistoryError] = useState<string | null>(null);
  const seq = useRef(0);

  const scope = { owner, repo, epic_number: epic };
  const scopeKey = `${owner}/${repo}/${epic}`;

  const load = useCallback(async () => {
    const n = ++seq.current;
    try {
      const [r, p] = await Promise.all([
        api.hubCall<HubRules>("hub.rules.get", { owner, repo, epic_number: epic }),
        api.hubCall<HubRulesProposals>("hub.rules.proposals.list", {
          owner,
          repo,
          state: "pendente",
          ...(epic > 0 ? { epic_number: epic } : {}),
        }),
      ]);
      if (n !== seq.current) return;
      setRules(r);
      setPending(p.proposals.filter((x) => x.epic_number === epic));
      setEpicPending(epic === 0 ? p.proposals.filter((x) => x.epic_number > 0) : []);
      setError(null);
    } catch (e) {
      if (n === seq.current) setError(errText(e));
    }
  }, [api, owner, repo, epic]);

  const loadHistory = useCallback(async () => {
    try {
      setHistory(await api.hubCall<HubRulesHistory>("hub.rules.history", { owner, repo, epic_number: epic, limit: 50 }));
      setHistoryError(null);
    } catch (e) {
      setHistoryError(errText(e));
    }
  }, [api, owner, repo, epic]);

  // Outro escopo: começa do zero.
  useEffect(() => {
    setRules(null);
    setDraft(null);
    setMsg(null);
    setHistory(null);
    setHistoryOpen(false);
  }, [scopeKey]);

  useEffect(() => {
    if (!hub.connected) return;
    void load();
    return api.onHubPush((e) => {
      if (e.event === "hub.rules.changed") void load();
    });
  }, [api, load, hub.connected]);

  useEffect(() => {
    if (historyOpen) void loadHistory();
  }, [historyOpen, loadHistory, rules?.current?.version]);

  const max = Number(rules?.max_bytes ?? MAX_DEFAULT);
  const warn = Number(rules?.warn_bytes ?? WARN_DEFAULT);
  const current = rules?.current ?? null;
  const headId = `${id}-h`;
  const title = epic === 0 ? "Regras do agente do projeto" : `Regras do agente do épico ${label}`;
  /** Para os rótulos acessíveis: "regras do projeto", "regras do épico PROJ-5014". */
  const scopeName = epic === 0 ? "regras do projeto" : `regras do épico ${label}`;

  const startEdit = (text?: string, imported = false) =>
    setDraft({ text: text ?? current?.content ?? "", imported, note: "", base: Number(current?.version ?? 0) });

  const onFile = async (e: ChangeEvent<HTMLInputElement>) => {
    const f = e.target.files?.[0];
    e.target.value = "";
    if (!f) return;
    try {
      const text = await readText(f);
      setMsg({ text: `${f.name} carregado no editor: revise e grave.`, error: false });
      setDraft((d) => ({ text, imported: true, note: d?.note ?? "", base: d?.base ?? Number(current?.version ?? 0) }));
    } catch (err) {
      setMsg({ text: `Não foi possível ler ${f.name}: ${errText(err)}`, error: true });
    }
  };

  const save = async () => {
    if (!draft) return;
    setBusy(true);
    try {
      const v = await api.hubCall<HubRuleVersion>(draft.imported ? "hub.rules.import" : "hub.rules.set", {
        ...scope,
        content: draft.text,
        base_version: draft.base,
        ...(draft.note.trim() ? { note: draft.note.trim() } : {}),
      });
      setDraft(null);
      setMsg({ text: `Versão ${v.version} gravada. Vale para as próximas sessões e retomadas.`, error: false });
      await load();
    } catch (e) {
      setMsg({ text: errText(e), error: true });
    } finally {
      setBusy(false);
    }
  };

  const decide = async (p: HubRulesProposal, approve: boolean) => {
    setBusy(true);
    try {
      const r = await api.hubCall<HubRulesProposal>("hub.rules.proposals.decide", { id: p.id, approve });
      setMsg({
        text: approve
          ? `Proposta #${r.id} aprovada: versão ${r.applied_version ?? "?"} em vigor.`
          : `Proposta #${r.id} rejeitada.`,
        error: false,
      });
      await load();
    } catch (e) {
      setMsg({ text: errText(e), error: true });
    } finally {
      setBusy(false);
    }
  };

  if (!hub.connected) {
    return (
      <section className={`${className} rules`} aria-labelledby={headId}>
        <div className="rules-head">
          <h3 id={headId}>{title}</h3>
        </div>
        <p className="muted rules-msg">As regras ficam no hub: conecte-se a ele para ver e editar.</p>
      </section>
    );
  }

  const bytes = draft ? utf8Bytes(draft.text) : Number(current?.bytes ?? 0);
  const size = sizeState(bytes, warn, max);

  return (
    <section className={`${className} rules`} aria-labelledby={headId}>
      <div className="rules-head">
        <h3 id={headId}>
          {title}
          {current && <span className="n"> · versão {current.version}</span>}
          {pending.length > 0 && (
            <span className="rules-pend">
              {" · "}
              {pending.length} {pending.length > 1 ? "propostas pendentes" : "proposta pendente"}
            </span>
          )}
        </h3>
        {/* Web: editar e importar desabilitados com o motivo (#26). */}
        {web && rules && (
          <span className="rules-actions">
            <DesktopOnly className="btn btn-sm" labels={[current ? "Editar" : "Escrever regras", "Importar CLAUDE.md"]} />
          </span>
        )}
        {!web && !draft && rules && (
          <span className="rules-actions">
            <button type="button" className="btn btn-sm" onClick={() => startEdit()}>
              {current ? "Editar" : "Escrever regras"}
            </button>
            <label className="btn btn-sm rules-file">
              Importar CLAUDE.md
              <input
                type="file"
                accept=".md,.markdown,.txt,text/markdown,text/plain"
                aria-label={`Importar CLAUDE.md para ${scopeName}`}
                onChange={onFile}
              />
            </label>
          </span>
        )}
      </div>

      {error && (
        <p role="alert" className="start-error rules-msg">
          Não foi possível carregar as regras: {error}
        </p>
      )}
      {!rules && !error && (
        <p className="muted rules-msg" role="status">
          Carregando regras…
        </p>
      )}
      {msg && (
        <p role={msg.error ? "alert" : "status"} className={`rules-msg ${msg.error ? "start-error" : "muted"}`}>
          {msg.text}
        </p>
      )}

      {rules && draft && (
        <form
          className="rules-editor"
          aria-label={`Editar ${scopeName}`}
          onSubmit={(e) => {
            e.preventDefault();
            void save();
          }}
        >
          <label className="rules-field">
            <span>Regras em Markdown</span>
            <textarea
              value={draft.text}
              rows={14}
              spellCheck={false}
              onChange={(e) => setDraft({ ...draft, text: e.target.value })}
            />
          </label>
          <p className="rules-size" data-state={size} aria-live="polite">
            {bytes} de {max} bytes
            {size === "warn" && " · perto do limite de 16 KiB"}
            {size === "over" && " · acima do limite de 16 KiB: encurte para gravar"}
          </p>
          <label className="rules-check">
            <input
              type="checkbox"
              checked={draft.imported}
              onChange={(e) => setDraft({ ...draft, imported: e.target.checked })}
            />
            Importação de um CLAUDE.md (registra a origem no histórico)
          </label>
          <label className="rules-field">
            <span>Nota da versão (opcional)</span>
            <input type="text" value={draft.note} onChange={(e) => setDraft({ ...draft, note: e.target.value })} />
          </label>
          <div className="rules-buttons">
            <button type="submit" className="btn btn-primary btn-sm" disabled={busy || size === "over"}>
              Gravar versão {draft.base + 1}
            </button>
            <label className="btn btn-sm rules-file">
              Carregar arquivo
              <input
                type="file"
                accept=".md,.markdown,.txt,text/markdown,text/plain"
                aria-label="Carregar arquivo no editor"
                onChange={onFile}
              />
            </label>
            <button type="button" className="btn btn-sm" onClick={() => setDraft(null)} disabled={busy}>
              Cancelar
            </button>
          </div>
        </form>
      )}

      {rules && !draft && (
        <div className="rules-current">
          {current && current.content.trim() ? (
            <>
              <p className="muted rules-meta">
                Versão {current.version} · {sourceText(current)} · {ago(Number(current.created_at), now)} ·{" "}
                <span className="rules-size" data-state={size}>
                  {Number(current.bytes)} de {max} bytes
                  {size === "warn" && " · perto do limite"}
                </span>
              </p>
              <pre className="rules-text" aria-label={`Texto de ${scopeName}`}>
                {current.content}
              </pre>
            </>
          ) : (
            <p className="muted rules-msg">
              {epic === 0
                ? "Sem regras do projeto. O que for escrito aqui vai no system prompt de toda sessão do projeto."
                : "Sem regras do épico. Valem para a sessão do épico e as das suas sub-issues, depois das do projeto."}
            </p>
          )}
        </div>
      )}

      {pending.length > 0 && (
        <ul className="rules-proposals" aria-label={`Propostas pendentes de ${scopeName}`}>
          {pending.map((p) => (
            <ProposalCard key={p.id} p={p} now={now} busy={busy} onDecide={web ? undefined : decide} />
          ))}
        </ul>
      )}

      {epicPending.length > 0 && (
        <p className="muted rules-msg">
          Propostas pendentes nas regras de épicos:{" "}
          {Array.from(new Set(epicPending.map((p) => p.label))).join(", ")}. Abra o épico na tela Projeto para decidir.
        </p>
      )}

      {rules && (
        <details
          className="rules-history"
          open={historyOpen}
          onToggle={(e) => setHistoryOpen((e.target as HTMLDetailsElement).open)}
        >
          <summary>Histórico</summary>
          {historyError ? (
            <p role="alert" className="start-error">
              Não foi possível carregar o histórico: {historyError}
            </p>
          ) : !history ? (
            historyOpen && <p className="muted">Carregando…</p>
          ) : history.versions.length === 0 ? (
            <p className="muted">Nenhuma versão ainda.</p>
          ) : (
            <ol className="rules-versions" aria-label={`Histórico de ${scopeName}`}>
              {history.versions.map((v) => (
                <li key={String(v.version)}>
                  <span className="rules-v">v{v.version}</span> <span>{sourceText(v)}</span>
                  <span className="muted"> · {ago(Number(v.created_at), now)} · {Number(v.bytes)} bytes</span>
                  {v.note && <span className="rules-note">{v.note}</span>}
                  <details>
                    <summary>Ver texto da versão {v.version}</summary>
                    <pre className="rules-text">{v.content || "(vazio)"}</pre>
                  </details>
                </li>
              ))}
            </ol>
          )}
          {history?.truncated && <p className="muted">Mostrando as 50 versões mais recentes.</p>}
        </details>
      )}
    </section>
  );
}

/** Proposta pendente: justificativa, aviso de base antiga, diferença e decisão. */
function ProposalCard({
  p,
  now,
  busy,
  onDecide,
}: {
  p: HubRulesProposal;
  now: number;
  busy: boolean;
  /** Ausente = só leitura (web): os botões ficam, desabilitados com o motivo (#26). */
  onDecide?: (p: HubRulesProposal, approve: boolean) => void;
}) {
  const diff = lineDiff(p.current_content ?? "", p.content);
  const { added, removed } = diffStats(diff);
  return (
    <li className="rules-proposal" aria-label={`Proposta #${p.id}`}>
      <p className="rules-meta">
        <strong>Proposta #{p.id}</strong> do agente em {p.env_name} · {ago(Number(p.created_at), now)}
        {p.session_id && <span className="muted"> · sessão {p.session_id.slice(0, 8)}</span>}
      </p>
      <p className="rules-reason">{p.reason}</p>
      {p.base_changed && (
        <p role="note" className="rules-warn">
          Feita sobre a versão {p.base_version}; a vigente é a {p.current_version}. Aprovar substitui o texto vigente
          inteiro pelo proposto, inclusive o que mudou depois da versão {p.base_version}.
        </p>
      )}
      <p className="muted rules-stats">
        Diferença contra a versão {p.current_version}: +{added} −{removed} linhas
      </p>
      <pre className="rules-diff" aria-label={`Diferença da proposta #${p.id}`}>
        {diff.map((l, i) => (
          <span key={i} className="rules-line" data-op={l.op}>
            {l.op === "add" ? "+ " : l.op === "del" ? "- " : "  "}
            {l.text}
            {"\n"}
          </span>
        ))}
      </pre>
      {onDecide && (
        <div className="rules-buttons">
          <button type="button" className="btn btn-primary btn-sm" disabled={busy} onClick={() => onDecide(p, true)}>
            Aprovar proposta #{p.id}
          </button>
          <button type="button" className="btn btn-sm" disabled={busy} onClick={() => onDecide(p, false)}>
            Rejeitar proposta #{p.id}
          </button>
        </div>
      )}
      {!onDecide && (
        <div className="rules-buttons">
          <DesktopOnly
            className="btn btn-sm"
            labels={[`Aprovar proposta #${p.id}`, `Rejeitar proposta #${p.id}`]}
            note="Decidir propostas fica no app desktop"
          />
        </div>
      )}
    </li>
  );
}
