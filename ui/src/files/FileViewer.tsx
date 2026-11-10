// Visualizador de markdown (#167): um `.md` do ambiente numa aba, lido pelo
// daemon (`file.read`, validado lá), em qualquer ambiente e também na web
// (pelo hub). Dois modos: formatado (o pipeline sanitizado do corpo da
// issue, com os diagramas Mermaid desenhados por cima) e raw (o texto).
import "../project/issue-body.css";
import "./files.css";
import { useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import type { MdFile } from "../protocol/gen";
import { renderFileMarkdown } from "../project/issue-markdown";
import type { Theme } from "../state/theme";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import { useNow } from "../App";
import { renderDiagram } from "./mermaid";
import { basename, fileTab, linkedMd, readParams, scopeOf, sizeText, type FileScreen } from "./open";

export type ViewMode = "formatted" | "raw";

const MODE_KEY = "ct.files.mode";

export function loadMode(): ViewMode {
  try {
    return localStorage.getItem(MODE_KEY) === "raw" ? "raw" : "formatted";
  } catch {
    return "formatted";
  }
}

function saveMode(m: ViewMode) {
  try {
    localStorage.setItem(MODE_KEY, m);
  } catch {
    // Sem armazenamento: vale só nesta aba.
  }
}

const currentTheme = (): Theme => (document.documentElement.dataset.theme === "light" ? "light" : "dark");

/** Tema da UI (`data-theme` no <html>), acompanhando a troca. */
function useDocTheme(): Theme {
  const [theme, setTheme] = useState(currentTheme);
  useEffect(() => {
    const mo = new MutationObserver(() => setTheme(currentTheme()));
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
    return () => mo.disconnect();
  }, []);
  return theme;
}

export function FileViewer({ screen }: { screen: FileScreen }) {
  const { api, envs, dashboards } = useTower();
  const nav = useNav();
  const now = useNow();
  const scope = scopeOf(screen);
  const [file, setFile] = useState<MdFile | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [tick, setTick] = useState(0);
  const [mode, setMode] = useState<ViewMode>(loadMode);
  const theme = useDocTheme();
  const body = useRef<HTMLDivElement>(null);
  const envName = envs.find((e) => e.id === screen.env)?.name ?? screen.env;

  // Projeto do arquivo, para as referências (`#12`, `PROJ-12`) virarem links.
  const d = dashboards[screen.env];
  const project = useMemo(() => {
    if (!d) return undefined;
    if (screen.projectId !== undefined) return d.projects.find((p) => p.project.id === screen.projectId)?.project;
    const all = [...d.live_sessions, ...d.interrupted_sessions, ...d.failed_sessions];
    const s = all.find((x) => x.id === screen.session);
    return s ? d.projects.find((p) => p.work_items.some((w) => w.id === s.work_item_id))?.project : undefined;
  }, [d, screen.projectId, screen.session]);

  useEffect(() => {
    let live = true;
    setLoading(true);
    api.call<MdFile>(screen.env, "file.read", readParams(scope, screen.path)).then(
      (f) => {
        if (!live) return;
        setFile(f);
        setError(null);
        setLoading(false);
      },
      (e) => {
        if (!live) return;
        setError(e instanceof Error ? e.message : String(e));
        setLoading(false);
      },
    );
    return () => {
      live = false;
    };
    // `scope` deriva do endereço (session/projectId).
  }, [api, screen.env, screen.path, screen.session, screen.projectId, tick]);

  const md = useMemo(() => (file ? renderFileMarkdown(file.content, project) : null), [file, project]);

  // Diagramas: desenha por cima do código de cada marcador; troca de
  // conteúdo, de modo ou de tema desenha de novo.
  useEffect(() => {
    const root = body.current;
    if (mode !== "formatted" || !md || !root || md.diagrams.length === 0) return;
    let live = true;
    root.querySelectorAll<HTMLElement>(".md-mermaid[data-mermaid]").forEach((el) => {
      const code = md.diagrams[Number(el.dataset.mermaid)];
      if (code === undefined) return;
      el.dataset.state = "rendering";
      void renderDiagram(code, theme).then((r) => {
        if (!live || !el.isConnected) return;
        el.replaceChildren();
        if ("svg" in r) {
          el.dataset.state = "ok";
          const box = document.createElement("div");
          box.className = "md-mermaid-svg";
          box.setAttribute("role", "img");
          box.setAttribute("aria-label", "Diagrama Mermaid");
          // SVG já sanitizado (sanitizeSvg).
          box.innerHTML = r.svg;
          el.append(box);
        } else {
          el.dataset.state = "error";
          const msg = document.createElement("p");
          msg.className = "md-mermaid-error";
          msg.setAttribute("role", "alert");
          msg.textContent = `Diagrama Mermaid inválido: ${r.error}`;
          const pre = document.createElement("pre");
          const c = document.createElement("code");
          c.textContent = code;
          pre.append(c);
          el.append(msg, pre);
        }
      });
    });
    return () => {
      live = false;
    };
  }, [md, mode, theme]);

  const choose = (m: ViewMode) => {
    setMode(m);
    saveMode(m);
  };

  const onClick = (e: MouseEvent<HTMLDivElement>) => {
    const a = (e.target as HTMLElement).closest("a");
    if (!a) return;
    const href = a.getAttribute("href") ?? "";
    if (href.startsWith("#")) return;
    e.preventDefault();
    const target = file ? linkedMd(href, file.path) : null;
    if (target) {
      nav.openTab(fileTab(screen.env, target, scope));
      return;
    }
    if (/^(https?:|mailto:)/i.test(href)) void api.openUrl(href).catch((err) => setError(String(err)));
  };

  const title = screen.title || basename(screen.path);
  return (
    <main className="fv" aria-labelledby="fv-title">
      <header className="fv-head">
        <div className="fv-id">
          <h1 id="fv-title" className="fv-title" title={title}>
            {title}
          </h1>
          <p className="fv-where">
            <span className="mono fv-path" title={file?.path ?? screen.path}>
              {file?.path ?? screen.path}
            </span>
            <span className="muted">
              {" · "}
              {envName}
              {file && ` · ${sizeText(file.size)}`}
              {file?.modified_at != null && ` · modificado ${ago(file.modified_at, now)}`}
            </span>
          </p>
        </div>
        <div className="fv-actions">
          <div className="fv-modes" role="group" aria-label="Modo de exibição">
            <button type="button" className="btn btn-sm" aria-pressed={mode === "formatted"} onClick={() => choose("formatted")}>
              Formatado
            </button>
            <button type="button" className="btn btn-sm" aria-pressed={mode === "raw"} onClick={() => choose("raw")}>
              Raw
            </button>
          </div>
          <button
            type="button"
            className="btn btn-sm"
            disabled={loading}
            title="Ler o arquivo de novo no ambiente"
            onClick={() => setTick((t) => t + 1)}
          >
            {loading && file ? "Recarregando…" : "Recarregar"}
          </button>
        </div>
      </header>
      <div className="fv-body">
        {error !== null && (
          <div className="ib-error" role="alert">
            <p>Não foi possível ler o arquivo: {error}</p>
            <button type="button" className="btn" onClick={() => setTick((t) => t + 1)}>
              Tentar de novo
            </button>
          </div>
        )}
        {!file ? (
          error === null && (
            <p className="muted" role="status">
              Lendo o arquivo no ambiente {envName}…
            </p>
          )
        ) : mode === "raw" ? (
          <pre className="fv-raw mono" aria-label="Texto do arquivo">
            {file.content}
          </pre>
        ) : file.content.trim() === "" ? (
          <p className="muted">Arquivo vazio.</p>
        ) : (
          // HTML do marked já sanitizado (renderFileMarkdown).
          <div ref={body} className="ib-md fv-md" onClick={onClick} dangerouslySetInnerHTML={{ __html: md!.html }} />
        )}
      </div>
    </main>
  );
}
