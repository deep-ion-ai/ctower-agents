import "./issue-body.css";
import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type MouseEvent } from "react";
import type { HubWorkItemBody } from "../protocol/hub-gen";
import { useTower } from "../state/tower";
import { itemShort, trackerLabel, type ProviderProject } from "../util/item";
import { linkedIssue, renderIssueMarkdown } from "./issue-markdown";

const focusables = (root: HTMLElement) =>
  Array.from(root.querySelectorAll<HTMLElement>("button, a[href], [tabindex]:not([tabindex='-1'])"));

/**
 * "Ver issue" (#434): a descrição da issue lida agora no rastreador, pelo hub
 * (`hub.workitem.body`), sem guardar nada: vive só enquanto a modal está
 * aberta e é buscada de novo a cada abertura. Fecha com Esc, ✕ ou clique
 * fora. `#n` do mesmo projeto abre a aba do item; link externo, o navegador.
 */
export function IssueBodyModal({
  project,
  number,
  title,
  url,
  onClose,
  onOpenItem,
}: {
  project: ProviderProject;
  number: number;
  /** Título do espelho, até a resposta chegar. */
  title: string;
  /** Link da issue no rastreador (saída no erro). */
  url: string;
  onClose: () => void;
  onOpenItem?: (ghNumber: number) => void;
}) {
  const { api } = useTower();
  const [body, setBody] = useState<HubWorkItemBody | null>(null);
  const [error, setError] = useState<string | null>(null);
  const dialog = useRef<HTMLElement>(null);
  const tracker = trackerLabel(project);

  useEffect(() => {
    let live = true;
    api
      .hubCall<HubWorkItemBody>("hub.workitem.body", {
        owner: project.gh_owner,
        repo: project.gh_repo,
        number,
      })
      .then(
        (b) => live && setBody(b),
        (e) => live && setError(String(e)),
      );
    return () => {
      live = false;
    };
  }, [api, project.gh_owner, project.gh_repo, number]);

  useEffect(() => {
    const before = document.activeElement as HTMLElement | null;
    dialog.current?.focus();
    return () => before?.focus();
  }, []);

  const html = useMemo(() => (body?.body.trim() ? renderIssueMarkdown(body.body, project) : ""), [body, project]);

  const open = (u: string) => api.openUrl(u).catch((e) => setError(String(e)));

  const onClick = (e: MouseEvent<HTMLDivElement>) => {
    const a = (e.target as HTMLElement).closest("a");
    if (!a) return;
    e.preventDefault();
    const n = linkedIssue(a, project);
    if (n !== null && onOpenItem) {
      onClose();
      onOpenItem(n);
      return;
    }
    const href = a.getAttribute("href");
    if (href && /^(https?:|mailto:)/i.test(href)) void open(href);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
      return;
    }
    if (e.key === "Tab" && dialog.current) {
      // Foco preso: do último volta ao primeiro (e vice-versa com Shift).
      const list = focusables(dialog.current);
      if (list.length === 0) return;
      const first = list[0];
      const last = list[list.length - 1];
      const at = document.activeElement;
      if (e.shiftKey && (at === first || at === dialog.current)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && at === last) {
        e.preventDefault();
        first.focus();
      }
    }
  };

  return (
    <div className="ib-scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <section
        ref={dialog}
        className="ib-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="ib-title"
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <div className="ib-head">
          <h2 id="ib-title">
            <span className="mono muted">{itemShort(project, number)}</span> {body?.title || title}
          </h2>
          <button type="button" className="ibtn" aria-label="Fechar" title="Fechar (Esc)" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="ib-body">
          {error !== null ? (
            <div className="ib-error" role="alert">
              <p>Não foi possível ler a issue: {error}</p>
              <button type="button" className="btn" onClick={() => void open(url)}>
                Abrir no {tracker} ↗
              </button>
            </div>
          ) : body === null ? (
            <p className="muted" role="status">
              Carregando a descrição…
            </p>
          ) : html === "" ? (
            <p className="muted">Sem descrição.</p>
          ) : (
            // HTML do marked já sanitizado pelo DOMPurify (renderIssueMarkdown).
            <div className="ib-md" onClick={onClick} dangerouslySetInnerHTML={{ __html: html }} />
          )}
        </div>
      </section>
    </div>
  );
}
