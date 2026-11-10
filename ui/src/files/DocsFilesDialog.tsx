// Lista dos `.md` da pasta de docs do projeto ou do épico (#167), ao lado
// do "Abrir pasta de docs": `docs.list` no ambiente, filtro por nome e um
// clique abre no visualizador (escopo do projeto: só a pasta de docs).
import "../project/issue-body.css";
import "./files.css";
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { DocsFiles } from "../protocol/gen";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import { useNow } from "../App";
import { basename, fileTab, sizeText } from "./open";

export function DocsFilesDialog({
  env,
  projectId,
  epic,
  title,
  onClose,
}: {
  env: string;
  projectId: number;
  /** Número do épico: a pasta dele; sem ele, a do projeto. */
  epic?: number;
  title: string;
  onClose: () => void;
}) {
  const { api } = useTower();
  const nav = useNav();
  const now = useNow();
  const [list, setList] = useState<DocsFiles | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const dialog = useRef<HTMLElement>(null);

  useEffect(() => {
    let live = true;
    api.call<DocsFiles>(env, "docs.list", { project_id: projectId, ...(epic !== undefined ? { epic } : {}) }).then(
      (l) => live && setList(l),
      (e) => live && setError(e instanceof Error ? e.message : String(e)),
    );
    return () => {
      live = false;
    };
  }, [api, env, projectId, epic]);

  useEffect(() => {
    const before = document.activeElement as HTMLElement | null;
    dialog.current?.querySelector<HTMLElement>("input")?.focus();
    return () => before?.focus();
  }, []);

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    e.preventDefault();
    e.stopPropagation();
    onClose();
  };

  const q = filter.trim().toLowerCase();
  const files = (list?.files ?? []).filter((f) => !q || f.rel.toLowerCase().includes(q));

  return (
    <div className="ib-scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <section
        ref={dialog}
        className="ib-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="df-title"
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <div className="ib-head">
          <h2 id="df-title">{title}</h2>
          <button type="button" className="ibtn" aria-label="Fechar" title="Fechar (Esc)" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="ib-body">
          {list && <p className="mono muted small">{list.root}</p>}
          <input
            className="df-filter"
            type="search"
            placeholder="Filtrar por nome ou pasta"
            aria-label="Filtrar arquivos"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
          {error !== null ? (
            <p className="ib-error" role="alert">
              Não foi possível listar a pasta de docs: {error}
            </p>
          ) : !list ? (
            <p className="muted" role="status">
              Listando a pasta de docs…
            </p>
          ) : files.length === 0 ? (
            <p className="muted">{q ? "Nenhum arquivo com esse nome." : "Nenhum arquivo .md na pasta de docs."}</p>
          ) : (
            <ul className="df-list sf-list">
              {files.map((f) => (
                <li key={f.path}>
                  <button
                    type="button"
                    className="sf-item"
                    onClick={() => {
                      nav.openTab(fileTab(env, f.path, { projectId }, basename(f.path)));
                      onClose();
                    }}
                  >
                    <span className="sf-name mono">{f.rel}</span>
                    <span className="sf-path">
                      {sizeText(f.size)}
                      {f.modified_at != null && ` · modificado ${ago(f.modified_at, now)}`}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
          {list?.truncated && (
            <p className="muted small">Lista cortada: a pasta tem mais arquivos que o limite mostrado.</p>
          )}
        </div>
      </section>
    </div>
  );
}
