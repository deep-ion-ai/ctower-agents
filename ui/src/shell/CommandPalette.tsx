// Paleta de comandos (#312): ⌘K ou o campo da barra de título. Busca difusa
// sobre sessões, issues e épicos, projetos, telas e comandos; `#` limita às
// issues e `>` aos comandos. Com a busca vazia, os recentes vêm no topo.
import { useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { useLiveGestor } from "../hub/HubGestor";
import { useNav } from "../state/nav";
import { lookup } from "../control-room/attention";
import { dashboardSessions } from "../terminal/SessionTree";
import { shellTarget, useOpenShell } from "../terminal/shells";
import { usePublishedActions } from "../terminal/actions";
import { useTower } from "../state/tower";
import { Icon } from "./icons";
import { search, type PaletteResult } from "./palette/fuzzy";
import { KIND_LABEL, paletteItems, type PaletteAction, type PaletteItem } from "./palette/items";
import { loadRecent, pushRecent } from "./palette/recent";
import { openPlace } from "./places";
import { useShell } from "./Shell";
import { useStartSession } from "./start";
import { projectAt } from "../project/sections";
import { projectSyncInfo, runProjectSync, syncAddrOf } from "../project/sync-now";

const LIST_ID = "palette-list";
const optId = (i: number) => `palette-opt-${i}`;

/** Rótulo com os caracteres casados destacados. */
function Highlight({ text, hits }: { text: string; hits: number[] }) {
  if (hits.length === 0) return <>{text}</>;
  const set = new Set(hits);
  const parts: { s: string; on: boolean }[] = [];
  for (let i = 0; i < text.length; i++) {
    const on = set.has(i);
    const last = parts[parts.length - 1];
    if (last && last.on === on) last.s += text[i];
    else parts.push({ s: text[i], on });
  }
  return <>{parts.map((p, i) => (p.on ? <mark key={i}>{p.s}</mark> : <span key={i}>{p.s}</span>))}</>;
}

export function CommandPalette({ onClose }: { onClose(): void }) {
  const tower = useTower();
  const nav = useNav();
  const shell = useShell();
  const startSession = useStartSession();
  const openShell = useOpenShell();
  const liveGestor = useLiveGestor();
  // Ações da sessão ativa (#424), publicadas pelo cabeçalho dela na tela Terminal.
  const published = usePublishedActions();
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const [recent] = useState(loadRecent);
  // Foco de antes de abrir: volta para lá ao fechar (se ainda estiver na tela).
  const back = useRef<Element | null>(document.activeElement);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    input.current?.focus();
    const el = back.current;
    return () => {
      if (el instanceof HTMLElement && el.isConnected) el.focus();
    };
  }, []);

  const { envs, status, dashboards, hub, uiUpdate, web } = tower;
  // Refeitos a cada render: poucas centenas de itens, e a lista acompanha os pushes.
  // Shell (#423): no work item da aba ativa do Terminal, se ela está no dashboard.
  const target = web
    ? null
    : shellTarget(nav.active, (t) => {
        const s = dashboardSessions(dashboards[t.env]).find((x) => x.id === t.session);
        return s ? lookup(dashboards[t.env], s) : {};
      });
  // Projeto da aba ativa (#90): o "Sincronizar este projeto" usa o mesmo endereço.
  const here = nav.screen.name === "project" ? syncAddrOf(tower, nav.screen) : null;
  const activeProject =
    here && nav.screen.name === "project" ? { screen: nav.screen, name: projectSyncInfo(tower, here).name } : null;
  const items = paletteItems({
    envs,
    status,
    dashboards,
    hub,
    uiUpdate,
    liveGestor,
    web,
    shellTarget: target,
    sessionActions: published,
    activeProject,
  });
  const results = search(items, query, recent);
  const cur = Math.min(active, results.length - 1);

  useEffect(() => {
    if (cur >= 0) document.getElementById(optId(cur))?.scrollIntoView?.({ block: "nearest" });
  }, [cur]);

  const exec = (a: PaletteAction) => {
    switch (a.type) {
      case "screen":
        return nav.go(a.screen);
      case "place":
        return openPlace(nav, a.place);
      case "tab":
        return nav.openTab(a.tab);
      case "terminal":
        return nav.openTerminal({ env: a.env, session: a.session });
      case "start":
        return startSession(a.target);
      case "sync":
        return void shell.syncNow();
      case "shell":
        return void openShell(a.env, a.workItemId);
      case "session-action":
        return published?.run(a.id);
      case "shortcuts":
        return shell.openShortcuts();
      case "sync-project": {
        const addr = syncAddrOf(tower, a.screen);
        if (!addr) return;
        // De outro lugar, abre a Visão geral do projeto: o cartão mostra o resultado.
        if (a.open) nav.go(projectAt(a.screen, "overview"));
        return void runProjectSync(tower.api, projectSyncInfo(tower, addr));
      }
      case "query":
        return;
    }
  };

  const run = (it: PaletteItem) => {
    if (it.disabled) return;
    pushRecent(it.id, recent);
    if (it.action.type === "query") {
      // Fica aberta, com a busca trocada (ex.: a lista de projetos do sync).
      setQuery(it.action.query);
      setActive(0);
      input.current?.focus();
      return;
    }
    onClose();
    exec(it.action);
  };

  const onKeyDown = (e: ReactKeyboardEvent) => {
    const n = results.length;
    switch (e.key) {
      case "ArrowDown":
      case "ArrowUp": {
        e.preventDefault();
        if (n === 0) return;
        const d = e.key === "ArrowDown" ? 1 : -1;
        setActive((cur + d + n) % n);
        return;
      }
      case "PageDown":
      case "PageUp": {
        e.preventDefault();
        if (n === 0) return;
        setActive(Math.max(0, Math.min(n - 1, cur + (e.key === "PageDown" ? 8 : -8))));
        return;
      }
      case "Enter":
        e.preventDefault();
        if (cur >= 0) run(results[cur].item);
        return;
      case "Escape":
        e.preventDefault();
        e.stopPropagation();
        onClose();
        return;
      case "Tab":
        // O foco fica no campo: a lista se percorre pelas setas.
        e.preventDefault();
        return;
    }
  };

  const empty = query.trim() === "" || query.trim() === "#" || query.trim() === ">";
  return (
    <div className="palette-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="palette" role="dialog" aria-modal="true" aria-label="Paleta de comandos">
        <div className="palette-input">
          <Icon name="search" size={14} />
          <input
            ref={input}
            type="text"
            role="combobox"
            aria-label="Buscar sessões, issues, projetos, telas e comandos"
            aria-expanded="true"
            aria-controls={LIST_ID}
            aria-autocomplete="list"
            aria-activedescendant={cur >= 0 ? optId(cur) : undefined}
            placeholder="Buscar… (# issues, > comandos)"
            spellCheck={false}
            autoComplete="off"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setActive(0);
            }}
            onKeyDown={onKeyDown}
          />
        </div>
        <ul className="palette-list" id={LIST_ID} role="listbox" aria-label="Resultados">
          {results.map((r: PaletteResult, i) => (
            <li
              key={r.item.id}
              id={optId(i)}
              role="option"
              aria-selected={i === cur}
              aria-disabled={r.item.disabled || undefined}
              className="palette-opt"
              onMouseMove={() => i !== cur && setActive(i)}
              // Sem tirar o foco do campo.
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => run(r.item)}
            >
              <span className="palette-label">
                <Highlight text={r.item.label} hits={r.hits} />
              </span>
              {r.item.detail && <span className="palette-detail">{r.item.detail}</span>}
              <span className="palette-kind">{r.recent && empty ? "recente" : KIND_LABEL[r.item.kind]}</span>
            </li>
          ))}
        </ul>
        {results.length === 0 && (
          <p className="palette-empty" role="status">
            Nada encontrado.
          </p>
        )}
        <div className="palette-foot" aria-hidden="true">
          <span>
            <kbd className="kbd">↑↓</kbd> navegar
          </span>
          <span>
            <kbd className="kbd">Enter</kbd> abrir
          </span>
          <span>
            <kbd className="kbd">Esc</kbd> fechar
          </span>
          <span>
            <kbd className="kbd">#</kbd> issues
          </span>
          <span>
            <kbd className="kbd">&gt;</kbd> comandos
          </span>
        </div>
      </div>
    </div>
  );
}
