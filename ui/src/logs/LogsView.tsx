import "./logs.css";
import { Fragment, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTower } from "../state/tower";
import { DesktopOnly } from "../web/DesktopOnly";
import { mask } from "./mask";
import {
  classifyError,
  clock,
  exportContent,
  exportName,
  fieldsText,
  lineText,
  parseSourceKey,
  sourceKey,
  type LogFilter,
  type LogSource,
  type SourceProblem,
} from "./sources";
import { LEVEL_LABEL, LEVELS, type LogLevel, type LogLine } from "./types";
import { useLogs } from "./useLogs";

/** Espera da busca antes de pedir de novo ao daemon. */
export const SEARCH_DEBOUNCE_MS = 300;

const LEVEL_OPTIONS: { value: LogLevel | ""; label: string }[] = [
  { value: "", label: "Todos os níveis" },
  { value: "error", label: "Só erros" },
  { value: "warn", label: "Avisos e erros" },
  { value: "info", label: "Info e acima" },
  { value: "debug", label: "Debug e acima" },
];

/** Fontes do seletor: o hub (se houver), cada ambiente e esta UI. */
export function useSourceOptions(): { key: string; label: string }[] {
  const { hub, envs, web } = useTower();
  const out: { key: string; label: string }[] = [];
  if (hub.conn || web) out.push({ key: "hub", label: "Hub" });
  for (const e of envs) out.push({ key: `env:${e.id}`, label: `Ambiente: ${e.name}` });
  out.push({ key: "ui", label: "Esta UI" });
  return out;
}

/** Fonte padrão sem endereço: o hub, senão o primeiro ambiente, senão a UI. */
export function useDefaultSource(): LogSource {
  const { hub, envs, web } = useTower();
  if (hub.conn || web) return { kind: "hub" };
  if (envs[0]) return { kind: "env", env: envs[0].id };
  return { kind: "ui" };
}

/**
 * Tela Logs (#65): a mesma interface para o hub, cada ambiente e a própria
 * UI — seletor de fonte, nível, busca, modo seguir, pausa, quebra de linha,
 * cópia do trecho selecionado e Exportar (`.log`/`.jsonl`). Ambiente sem
 * conexão mostra "ambiente offline"; na web, seguir um ambiente fica no app
 * desktop (o canal do hub não mantém assinatura).
 */
export function LogsView({ source, onSource }: { source: LogSource; onSource: (s: LogSource) => void }) {
  const { api, hub, envs, status, web } = useTower();
  const options = useSourceOptions();
  const key = sourceKey(source);
  const label = options.find((o) => o.key === key)?.label ?? key;

  const [level, setLevel] = useState<LogLevel | "">("");
  const [text, setText] = useState("");
  const [q, setQ] = useState("");
  useEffect(() => {
    const t = setTimeout(() => setQ(text), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [text]);
  const filter: LogFilter = { level, q };

  const canFollow = !(web && source.kind === "env");
  const [followWanted, setFollowWanted] = useState(true);
  const follow = canFollow && followWanted;
  const [paused, setPaused] = useState(false);
  const [wrap, setWrap] = useState(true);
  const [note, setNote] = useState<string | null>(null);

  // A fonte está alcançável agora?
  let problem: SourceProblem | { kind: "hub-unconfigured" } | { kind: "hub-down"; detail: string | null } | null = null;
  let ready = true;
  if (source.kind === "hub") {
    if (!hub.conn && !web) problem = { kind: "hub-unconfigured" };
    else if (!hub.connected) problem = { kind: "hub-down", detail: hub.status?.error ?? null };
    ready = problem === null;
  } else if (source.kind === "env") {
    const st = status[source.env];
    if (!envs.some((e) => e.id === source.env)) problem = { kind: "error", message: "ambiente desconhecido neste app" };
    else if (st && !st.connected) problem = { kind: "offline" };
    ready = problem === null && !!st?.connected;
  }

  const logs = useLogs(source, filter, { follow, paused, ready });
  if (!problem && logs.error) problem = classifyError(logs.error);

  // Rolagem: colada no fim enquanto segue, até o usuário subir.
  const listRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  useLayoutEffect(() => {
    const el = listRef.current;
    if (el && stick.current && !paused) el.scrollTop = el.scrollHeight;
  }, [logs.lines, paused]);
  useEffect(() => {
    stick.current = true;
  }, [key]);
  const onScroll = () => {
    const el = listRef.current;
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
  };

  const flash = (msg: string) => setNote(msg);

  const copy = async () => {
    const sel = window.getSelection();
    const inList = !!sel && !sel.isCollapsed && !!listRef.current && listRef.current.contains(sel.anchorNode);
    const body = inList ? sel!.toString() : logs.lines.map(lineText).join("\n");
    try {
      await api.writeClipboard(mask(body));
      flash(inList ? "Trecho selecionado copiado." : `${logs.lines.length} linha(s) copiada(s).`);
    } catch (e) {
      flash(`Não deu para copiar: ${e instanceof Error ? e.message : String(e)}`);
    }
  };

  const exportAs = async (format: "log" | "jsonl") => {
    const name = exportName(label, format);
    const mime = format === "log" ? "text/plain" : "application/x-ndjson";
    try {
      const path = await api.saveFile(name, exportContent(logs.lines, format), mime);
      flash(path ? `Exportado para ${path}` : `Exportado: ${name}`);
    } catch (e) {
      flash(`Falha ao exportar: ${e instanceof Error ? e.message : String(e)}`);
    }
  };

  return (
    <section className="logs" aria-label={`Log: ${label}`}>
      <div className="logs-bar" role="toolbar" aria-label="Controles do log">
        <select
          aria-label="Fonte do log"
          className="set-env-select"
          value={key}
          onChange={(e) => {
            const s = parseSourceKey(e.target.value);
            if (s) onSource(s);
          }}
        >
          {!options.some((o) => o.key === key) && <option value={key}>{label}</option>}
          {options.map((o) => (
            <option key={o.key} value={o.key}>
              {o.label}
            </option>
          ))}
        </select>
        <select aria-label="Nível do log" className="set-env-select" value={level} onChange={(e) => setLevel(e.target.value as LogLevel | "")}>
          {LEVEL_OPTIONS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
        <input
          type="search"
          className="logs-search"
          aria-label="Buscar no log"
          placeholder="Buscar…"
          value={text}
          onChange={(e) => setText(e.target.value)}
        />
        <div role="group" aria-label="Acompanhamento" className="logs-seg">
          {canFollow ? (
            <button type="button" className="btn btn-sm" aria-pressed={follow} onClick={() => setFollowWanted(!followWanted)}>
              Seguir
            </button>
          ) : (
            <DesktopOnly labels="Seguir" className="btn btn-sm" note="Seguir o log de um ambiente fica no app desktop; use Atualizar" />
          )}
          <button type="button" className="btn btn-sm" aria-pressed={paused} disabled={!follow} onClick={() => setPaused(!paused)}>
            {paused && logs.pending ? `Pausado (${logs.pending} novas)` : "Pausar"}
          </button>
          <button type="button" className="btn btn-sm" aria-pressed={wrap} onClick={() => setWrap(!wrap)}>
            Quebrar linhas
          </button>
        </div>
        <div role="group" aria-label="Ações do log" className="logs-seg">
          <button type="button" className="btn btn-sm" onClick={logs.reload}>
            Atualizar
          </button>
          <button type="button" className="btn btn-sm" onClick={() => void copy()} disabled={!logs.lines.length}>
            Copiar
          </button>
          <button type="button" className="btn btn-sm" onClick={() => void exportAs("log")} disabled={!logs.lines.length}>
            Exportar .log
          </button>
          <button type="button" className="btn btn-sm" onClick={() => void exportAs("jsonl")} disabled={!logs.lines.length}>
            Exportar .jsonl
          </button>
        </div>
      </div>

      {problem && <Problem problem={problem} source={source} />}

      <p className="logs-status muted small" role="status">
        {logs.loading
          ? "Carregando…"
          : `${logs.lines.length} linha(s)${follow ? (paused ? " · pausado" : " · seguindo") : ""}`}
        {logs.missed > 0 && ` · ${logs.missed} linha(s) perdida(s) no acompanhamento`}
        {logs.file && (
          <>
            {" · "}
            <span className="mono">{logs.file}</span>
          </>
        )}
        {source.kind === "ui" && (web ? " · só nesta aba, desde que ela abriu" : " · desde que o app abriu, com a execução anterior")}
        {note && <> · {note}</>}
      </p>

      <div
        ref={listRef}
        className="logs-list"
        data-wrap={wrap ? "on" : "off"}
        onScroll={onScroll}
        tabIndex={0}
        aria-label="Linhas do log"
      >
        {logs.more && (
          <button type="button" className="btn btn-sm logs-older" disabled={logs.loading} onClick={() => void logs.loadOlder()}>
            Carregar anteriores
          </button>
        )}
        {!logs.loading && !logs.lines.length && !problem && <p className="empty">Nenhuma linha com este filtro.</p>}
        <ol className="logs-lines mono">
          {logs.lines.map((l, i) => (
            <Fragment key={l.id}>
              {source.kind === "ui" && boundary(logs.lines, i)}
              <Line line={l} />
            </Fragment>
          ))}
        </ol>
      </div>
    </section>
  );
}

/** Separador entre a execução anterior do app e esta (só na fonte UI). */
function boundary(lines: LogLine[], i: number) {
  const l = lines[i];
  const prev = lines[i - 1];
  if (l.previous && (!prev || !prev.previous))
    return (
      <li className="logs-sep">
        Execução anterior do app
      </li>
    );
  if (!l.previous && prev?.previous)
    return <li className="logs-sep">Esta execução (app aberto às {clock(l.ts).slice(0, 5)})</li>;
  return null;
}

function Line({ line }: { line: LogLine }) {
  const lvl = line.level.toLowerCase();
  const f = fieldsText(line.fields);
  return (
    <li className="log-line" data-level={lvl}>
      <time dateTime={new Date(line.ts).toISOString()} title={new Date(line.ts).toLocaleString()}>
        {clock(line.ts)}
      </time>{" "}
      <span className="log-lvl">{(LEVELS as string[]).includes(lvl) ? LEVEL_LABEL[lvl as LogLevel] : lvl}</span>{" "}
      <span className="log-target">{line.target}</span> <span className="log-msg">{line.message}</span>
      {f && <span className="log-fields"> {f}</span>}
    </li>
  );
}

function Problem({
  problem,
  source,
}: {
  problem: SourceProblem | { kind: "hub-unconfigured" } | { kind: "hub-down"; detail: string | null };
  source: LogSource;
}) {
  const { envs } = useTower();
  const envName = source.kind === "env" ? (envs.find((e) => e.id === source.env)?.name ?? source.env) : "";
  const what = source.kind === "hub" ? "o hub (cthubd)" : source.kind === "env" ? `o daemon de ${envName} (ctowerd)` : "a UI";
  let text: string;
  switch (problem.kind) {
    case "hub-unconfigured":
      text = "A conexão com o hub não está configurada (Configurações › Conexões).";
      break;
    case "hub-down":
      text = `Hub indisponível: o log volta quando ele responder${problem.detail ? ` (${problem.detail})` : ""}.`;
      break;
    case "offline":
      text =
        source.kind === "env"
          ? `Ambiente offline: ${envName} está sem conexão agora (ctowerd parado ou sem rede). O log volta quando ele reconectar.`
          : "Fonte offline agora.";
      break;
    case "disabled":
      text = `O log em arquivo está desligado em ${what} ([logs] max_file_mb = 0 ou pasta inacessível): veja o journald ou o ~/Library/Logs da máquina.`;
      break;
    case "outdated":
      text = `${what.charAt(0).toUpperCase()}${what.slice(1)} ainda não tem a tela Logs: atualize em Manutenção › Atualizações.`;
      break;
    default:
      text = `Falha ao ler o log: ${problem.message}`;
  }
  return (
    <p className="logs-problem" role="alert" data-kind={problem.kind}>
      {text}
    </p>
  );
}
