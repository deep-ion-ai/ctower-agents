import "./project-index.css";
import { useEffect, useState, type MouseEvent } from "react";
import { useNow } from "../App";
import { projectCounts, useHubHealth } from "../hub/HubHealth";
import type { HubHealth } from "../protocol/hub-gen";
import { settingsTab } from "../settings/sections";
import { ContextMenu } from "../shell/ContextMenu";
import { Icon } from "../shell/icons";
import { useStartSession } from "../shell/start";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { NewProjectDialog, type RegisterIntent } from "./NewProjectDialog";
import { useProjectOrder, type ProjectOrder } from "./order";
import { ProviderIcon } from "./ProviderIcon";
import { projectMenuItems, useProjectRows, type ProjectRow } from "./rows";
import { projectAt } from "./sections";
import { keyOf, plural, projectState, syncText, useSettingsRun, type Tone } from "./settings";

/**
 * Índice da atividade Projetos (#16): `{ name: "project" }` sem chave. Lista
 * os projetos (pelo hub, na ordem de prioridade; sem hub, os locais de cada
 * ambiente) com estado, sessões, atribuições e saúde, e as ações de cada um:
 * Abrir (a linha, na aba Itens), ⚙ (a aba Configurações do projeto, #17) e ⋯ (o mesmo menu da Sala de
 * controle). Aqui ficam também o cadastro (+ Novo projeto), o filtro e a
 * reordenação da prioridade (arrastar ou Alt+↑/↓). Abrir um projeto abre a
 * aba dele (`project:<chave>`), e esta aba segue sendo o índice: o projeto
 * aberto continua na faixa de abas ao voltar para cá.
 *
 * `register` (#18) é o pedido de cadastro do endereço, que antes ia ao Hub ›
 * Projetos: o "+" da Sala de controle e a paleta (vazio) e o "Cadastrar
 * projeto" da Quarentena (com a URL, ou em Jira + GitLab). Abre o formulário
 * e sai do endereço, como o `assign` nas Configurações do projeto. `notice`
 * é o aviso de quem chegou aqui ao terminar (a exclusão do projeto, com o
 * snapshot anterior): fica na tela (a mensagem de uma ação seguinte o cobre), fora do endereço.
 */
export function ProjectIndex({ register, notice: noticeReq }: { register?: RegisterIntent; notice?: string } = {}) {
  const { hub, web, api } = useTower();
  const nav = useNav();
  const now = useNow();
  const startSession = useStartSession();
  const rows = useProjectRows();
  const settings = useSettingsRun();
  const { health } = useHubHealth();
  const [filter, setFilter] = useState("");
  /** Cadastro aberto; `id` novo a cada pedido, para o formulário recomeçar. */
  const [add, setAdd] = useState<{ id: number; intent: RegisterIntent } | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; row: ProjectRow; origin: HTMLElement } | null>(null);

  const viaHub = hub.up && !!hub.dashboard;
  // Cadastro, ordem e atribuição: no hub e só no desktop (a web é só leitura).
  const editable = hub.connected && !web;

  // Pedido de cadastro do endereço: abre o formulário (de novo a cada pedido,
  // pelo `id`) e sai do endereço, para voltar à aba não reabrir o formulário.
  // Com o hub ainda conectando (deep link na abertura), espera por ele; na
  // web (só leitura) o pedido só sai.
  useEffect(() => {
    if (!noticeReq) return;
    setNotice(noticeReq);
    nav.go({ name: "project" });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- só o aviso dispara
  }, [noticeReq]);
  useEffect(() => {
    if (!register || (!hub.connected && !web)) return;
    if (editable) setAdd((a) => ({ id: (a?.id ?? 0) + 1, intent: register }));
    nav.go({ name: "project" });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- só o pedido e a conexão disparam
  }, [register, hub.connected]);
  // A ordem é a do hub: só os projetos dele (todos com `view`) reordenam.
  const ranked = viaHub ? rows.flatMap((r) => (r.view ? [r.view.project] : [])) : [];
  const order = useProjectOrder(ranked, settings, viaHub && editable);

  const q = filter.trim().toLowerCase();
  const shown = q
    ? rows.filter(
        (r) => r.name.toLowerCase().includes(q) || keyOf(r.project.gh_owner, r.project.gh_repo).toLowerCase().includes(q),
      )
    : rows;

  const openMenu = (row: ProjectRow, x: number, y: number, origin: HTMLElement) => setMenu({ x, y, row, origin });
  // ⚙: a aba Configurações do projeto (#17), na mesma aba do projeto.
  const settingsOf = (r: ProjectRow) => nav.go(projectAt(r.screen, "settings"));

  const list = (items: ProjectRow[]) => (
    <ul className="pidx-list">
      {items.map((r) => (
        <IndexRow
          key={r.key}
          row={r}
          now={now}
          health={viaHub ? health : null}
          pos={r.view ? ranked.findIndex((p) => p.id === r.view!.project.id) + 1 : null}
          order={r.view && viaHub ? order : null}
          onOpen={() => nav.go(projectAt(r.screen))}
          onSettings={hub.connected ? () => settingsOf(r) : null}
          onMenu={(x, y, origin) => openMenu(r, x, y, origin)}
        />
      ))}
    </ul>
  );

  // Sem hub: um grupo por ambiente (como a Sala de controle sem hub).
  const groups = viaHub
    ? null
    : [...new Map(shown.map((r) => [r.env!.id, r.env!])).values()].map((env) => ({
        env,
        rows: shown.filter((r) => r.env?.id === env.id),
      }));

  return (
    <main className="page pidx">
      <div className="pidx-head">
        <div className="pidx-title">
          <h1>Projetos</h1>
          <span className="badge" title={plural(rows.length, "projeto", "projetos")}>
            {rows.length}
          </span>
        </div>
        <label htmlFor="pidx-filter" className="sr-only">
          Filtrar projetos
        </label>
        <input
          id="pidx-filter"
          type="search"
          className="pidx-filter"
          placeholder="Filtrar projetos"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        {editable && (
          <button
            type="button"
            className="btn btn-primary"
            onClick={() => setAdd((a) => ({ id: (a?.id ?? 0) + 1, intent: { url: "" } }))}
          >
            + Novo projeto
          </button>
        )}
      </div>

      {!viaHub && rows.length > 0 && (
        <p className="muted small" role="note">
          Hub fora do ar: cada ambiente mostra os próprios projetos (visão parcial, sem a ordem de prioridade).
        </p>
      )}

      {add && (
        <div className="panel">
          <NewProjectDialog
            key={add.id}
            intent={add.intent}
            settings={settings}
            onDone={() => setAdd(null)}
          />
        </div>
      )}
      {settings.error && (
        <p role="alert" className="start-error">
          {settings.error}
        </p>
      )}
      {(settings.done ?? notice) && (
        <p role="status" className="state-on">
          {settings.done ?? notice}
        </p>
      )}

      {rows.length === 0 ? (
        <div>
          {/* O cadastro é no Hub: sem ele no ar, o caminho é configurar a conexão. */}
          <p className="empty">
            {hub.connected
              ? web
                ? "Nenhum projeto cadastrado. O cadastro é feito no app desktop."
                : "Nenhum projeto cadastrado. Use + Novo projeto para cadastrar pela URL do repositório."
              : "Nenhum projeto cadastrado. O cadastro fica no Hub, que está fora do ar ou sem conexão configurada."}
          </p>
          {!hub.connected && !web && (
            <button type="button" className="btn btn-primary" onClick={() => nav.openTab(settingsTab({ section: "connections" }))}>
              Configurar o hub
            </button>
          )}
        </div>
      ) : shown.length === 0 ? (
        <p className="muted">Nenhum projeto com “{filter.trim()}”.</p>
      ) : groups ? (
        groups.map((g) => (
          <section key={g.env.id} className="pidx-group" aria-label={`Projetos de ${g.env.name}`}>
            <h2 className="section-title">{g.env.name}</h2>
            {list(g.rows)}
          </section>
        ))
      ) : (
        list(shown)
      )}

      {viaHub && editable && ranked.length > 1 && (
        <p className="muted hint">
          A ordem da lista é a prioridade entre projetos. Arraste para reordenar, ou use Alt+↑/↓ no projeto.
        </p>
      )}

      {menu && (
        <ContextMenu
          label={`Ações do projeto ${menu.row.name}`}
          x={menu.x}
          y={menu.y}
          items={projectMenuItems(menu.row, {
            nav,
            hubConnected: hub.connected,
            editable,
            startSession,
            openUrl: (url) => void api.openUrl(url).catch(() => {}),
            origin: menu.origin,
          })}
          onClose={(restore) => {
            const o = menu.origin;
            setMenu(null);
            if (restore) o.focus();
          }}
        />
      )}
    </main>
  );
}

/** Uma linha do índice: a linha abre o projeto; ⚙ e ⋯ ficam à direita. */
function IndexRow({
  row: r,
  now,
  health,
  pos,
  order,
  onOpen,
  onSettings,
  onMenu,
}: {
  row: ProjectRow;
  now: number;
  health: HubHealth | null;
  /** Posição na prioridade do hub; `null` sem hub. */
  pos: number | null;
  /** Arrasto e Alt+↑/↓; `null` = não reordena (sem hub). */
  order: ProjectOrder | null;
  onOpen(): void;
  /** Configurações do projeto; `null` = sem hub conectado. */
  onSettings: (() => void) | null;
  onMenu(x: number, y: number, origin: HTMLElement): void;
}) {
  const key = keyOf(r.project.gh_owner, r.project.gh_repo);
  const id = `pidx-${r.key.replace(/[^a-z0-9-]/gi, "-")}`;
  const state: { text: string; tone: Tone } = r.view
    ? projectState(r.view, now)
    : r.env
      ? syncText(r.sync, now)
      : { text: "aguardando o hub", tone: "neutral" };
  const sessions =
    r.running + r.waiting === 0
      ? "sem sessões"
      : [r.running > 0 && plural(r.running, "em execução", "em execução"), r.waiting > 0 && `${r.waiting} aguardando`]
          .filter(Boolean)
          .join(", ");
  const where = r.view
    ? r.view.assignments.length === 0
      ? "sem ambiente"
      : r.view.assignments.map((a) => a.env_name).join(", ")
    : r.env?.name ?? null;
  const counts = health ? projectCounts(health, r.project.gh_owner, r.project.gh_repo) : null;
  const hub = r.view?.project;
  const main = (e: MouseEvent<HTMLElement>) => e.currentTarget.closest("li")?.querySelector<HTMLElement>(".pidx-main");

  return (
    <li
      className="pidx-row"
      {...(order && hub ? order.itemProps(hub) : {})}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu(e.clientX, e.clientY, main(e) ?? e.currentTarget);
      }}
    >
      <button
        type="button"
        className="pidx-main"
        aria-label={r.name}
        aria-describedby={`${id}-d`}
        title="Abrir"
        onClick={onOpen}
        onKeyDown={(e) => {
          if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
            e.preventDefault();
            const b = e.currentTarget.getBoundingClientRect();
            onMenu(b.left + 16, b.bottom, e.currentTarget);
            return;
          }
          if (order && hub) order.onKey(e, hub);
        }}
      >
        <span className="pidx-top">
          <ProviderIcon project={r.project} />
          <strong>{r.name}</strong>
          <span className="mono muted small">{key}</span>
          {pos !== null && <span className="badge pidx-pos">#{pos}</span>}
        </span>
        <span id={`${id}-d`} className="pidx-meta">
          <span>
            <span className="hub-dot" data-tone={state.tone} aria-hidden="true" /> {state.text}
          </span>
          <span>{sessions}</span>
          {where && <span>{r.view ? `atribuído a: ${where}` : `ambiente: ${where}`}</span>}
          {counts && (
            <span>
              {counts.critico + counts.aviso === 0
                ? "sem riscos"
                : [counts.critico > 0 && plural(counts.critico, "crítico", "críticos"), counts.aviso > 0 && plural(counts.aviso, "aviso", "avisos")]
                    .filter(Boolean)
                    .join(", ")}
            </span>
          )}
        </span>
      </button>
      <div className="pidx-actions">
        {onSettings && (
          <button type="button" className="ibtn" aria-label={`Configurações de ${r.name}`} title="Configurações" onClick={onSettings}>
            <Icon name="settings" size={15} />
          </button>
        )}
        <button
          type="button"
          className="ibtn"
          aria-label={`Mais ações de ${r.name}`}
          aria-haspopup="menu"
          title="Mais ações"
          onClick={(e) => {
            const b = e.currentTarget.getBoundingClientRect();
            onMenu(b.left, b.bottom, e.currentTarget);
          }}
        >
          <Icon name="more" size={15} />
        </button>
      </div>
    </li>
  );
}

/**
 * Trilha "Projetos › X" no topo do painel do projeto (#16), no lugar do
 * seletor de projeto: "Projetos" volta ao índice (a aba da atividade), e a
 * aba deste projeto continua aberta.
 */
export function ProjectTrail({ name, title }: { name: string; title?: string }) {
  const nav = useNav();
  return (
    <nav aria-label="Trilha do projeto" className="pj-trail">
      <ol className="pj-crumbs">
        <li>
          <button type="button" className="linklike" onClick={() => nav.go({ name: "project" })}>
            Projetos
          </button>
        </li>
        <li>
          <span aria-current="page" title={title}>
            {name}
          </span>
        </li>
      </ol>
    </nav>
  );
}
