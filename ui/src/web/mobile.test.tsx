// Shell no celular (épico #205, #202, spec §10.3): modo web abaixo de 768 px,
// nas larguras de 360 e 414 px. Barra inferior, gaveta, lista de abas, sino
// de avisos, ações da fase 1 pelo hub e "Aguardando você" em destaque.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, vi } from "vitest";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { HubHistory, HubTree } from "../protocol/hub-gen";
import type { Screen } from "../state/nav";
import { hubEnv, hubProject, hubProposal, sync } from "../test/fixtures";
import { setViewport } from "../test/viewport";
import { envCalls, hubItem, hubSession, now, webApi } from "../test/web";

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
});

const TREE: HubTree = {
  project: hubProject(),
  sync: sync({ project_id: 1, last_ok_at: now }),
  roots: [
    {
      issue: {
        gh_number: 201,
        title: "WebDaemonApi",
        state: "open",
        labels: [],
        parent_number: null,
        has_sub_issues: false,
        url: "https://github.com/deep-ion-ai/ctower-agents/issues/201",
        updated_at: 0,
      },
      work_item: {
        id: 11,
        project_id: 1,
        gh_number: 201,
        kind: "issue",
        status: "em_execucao",
        branch: "feat/201-ui-web-daemon-api",
        created_at: 0,
        closed_at: null,
        priority: "P2",
        rank: 1,
      },
      summary: { pr: null, decisions: { total: 0, pending_autonomous: 0 }, last_activity_at: now, live_envs: [] },
      children: [],
    },
  ],
  untracked: [],
};

function mobileApi(...args: Parameters<typeof webApi>) {
  return webApi(...args)
    .onHub("hub.tree.get", TREE)
    .onHub("hub.workitem.history", {
      work_item: TREE.roots[0].work_item!,
      prs: [],
      locations: [],
      sessions: [],
      events: [],
      next_before_hlc: null,
      sub_work_items: [],
      decisions: [],
    } satisfies HubHistory)
    .onHub("hub.workitem.set_status", { ...TREE.roots[0].work_item, status: "bloqueado" })
    .onHub("hub.workitem.set_priority", { ...TREE.roots[0].work_item, priority: "P0" });
}

function renderApp(api: FakeDaemonApi, initial?: Screen) {
  render(<App api={api} initial={initial} />);
  return userEvent.setup();
}

const side = () => screen.queryByRole("region", { name: "Painel lateral" });

describe.each([360, 414])("celular, %i px", (width) => {
  beforeEach(() => setViewport(width));

  it("barra inferior com as telas da web e os selos; sem abas, painel inferior nem barra de status", async () => {
    const api = mobileApi({}, [hubItem({ status: "aguardando_voce" })]);
    const user = renderApp(api);
    const bar = await screen.findByRole("navigation", { name: "Telas" });
    expect(bar).toHaveClass("bottombar");
    expect(document.querySelector(".shell")).toHaveAttribute("data-mobile", "true");
    // Depois da área de trabalho (embaixo), não ao lado.
    expect(document.querySelector(".shell")!.lastElementChild).toBe(bar);
    const names = within(bar)
      .getAllByRole("button")
      .map((b) => b.getAttribute("aria-label"));
    expect(names).toEqual(["Sala de controle", "Projetos", "Revisão", "Recuperação", "Hub", "Configurações"]);
    await waitFor(() =>
      expect(within(bar).getByRole("button", { name: "Sala de controle" })).toHaveAccessibleDescription(
        "1 aguardando você",
      ),
    );
    expect(screen.queryByRole("contentinfo", { name: "Barra de status" })).toBeNull();
    expect(screen.queryByRole("tablist", { name: "Abas abertas" })).toBeNull();
    expect(screen.queryByRole("region", { name: "Painel inferior" })).toBeNull();
    // A barra inferior navega.
    await user.click(within(bar).getByRole("button", { name: "Hub" }));
    expect(within(bar).getByRole("button", { name: "Hub" })).toHaveAttribute("aria-current", "page");
    expect(envCalls(api)).toEqual([]);
  });

  it("painel lateral em gaveta: fechado de início, abre pelo menu e fecha ao escolher", async () => {
    const api = mobileApi();
    // Com o item aberto, a árvore do projeto segue na gaveta (sem item, ela vem ao corpo, #25).
    const user = renderApp(api, {
      name: "project",
      owner: "deep-ion-ai",
      repo: "ctower-agents",
      item: { ghNumber: 201 },
    });
    await screen.findByRole("region", { name: "Detalhe do item" });
    const menu = await screen.findByRole("button", { name: "Abrir painel lateral" });
    expect(side()).toBeNull();
    await user.click(menu);
    const drawer = await screen.findByRole("region", { name: "Painel lateral" });
    expect(screen.getByRole("button", { name: "Fechar painel lateral" })).toHaveAttribute("aria-expanded", "true");
    expect(within(drawer).getByRole("complementary", { name: "Árvore do projeto" })).toBeInTheDocument();
    // Escolher outro lugar (a trilha da gaveta) fecha a gaveta.
    await user.click(within(drawer).getByRole("button", { name: "Projetos" }));
    expect(await screen.findByRole("heading", { name: "Projetos" })).toBeInTheDocument();
    await waitFor(() => expect(side()).toBeNull());
    // Escape também fecha.
    await user.click(screen.getByRole("button", { name: "Abrir painel lateral" }));
    expect(side()).not.toBeNull();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(side()).toBeNull());
  });

  it("uma aba por vez: as abertas numa lista no cabeçalho", async () => {
    const api = mobileApi();
    const user = renderApp(api, { name: "project", owner: "deep-ion-ai", repo: "ctower-agents" });
    await user.click(await screen.findByRole("button", { name: /^#201 WebDaemonApi/ }));
    await screen.findByRole("region", { name: "Detalhe do item" });
    const trigger = screen.getByRole("button", { name: /^Abas abertas \(2\): .*#201/ });
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    await user.click(trigger);
    const list = screen.getByRole("list", { name: "Abas abertas" });
    const rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(2);
    expect(within(list).getByRole("button", { name: /^(?!Fechar).*#201/ })).toHaveAttribute("aria-current", "page");
    // Trocar de aba pela lista: a área de editor mostra só a escolhida.
    await user.click(within(list).getByRole("button", { name: "ctower-agents" }));
    expect(screen.queryByRole("list", { name: "Abas abertas" })).toBeNull();
    await waitFor(() => expect(screen.queryByRole("region", { name: "Detalhe do item" })).toBeNull());
    expect(screen.getByRole("button", { name: /^Abas abertas \(2\): ctower-agents$/ })).toBeInTheDocument();
    // Fechar a aba do item pela lista.
    await user.click(screen.getByRole("button", { name: /^Abas abertas/ }));
    await user.click(screen.getByRole("button", { name: /^Fechar aba .*#201/ }));
    expect(screen.getByRole("button", { name: /^Abas abertas \(1\)/ })).toBeInTheDocument();
  });

  it("Projetos na barra inferior: o índice no corpo, sem a gaveta (#25)", async () => {
    const api = mobileApi();
    const user = renderApp(api);
    const bar = await screen.findByRole("navigation", { name: "Telas" });
    await user.click(within(bar).getByRole("button", { name: "Projetos" }));
    const main = await screen.findByRole("main");
    expect(within(main).getByRole("heading", { name: "Projetos" })).toBeInTheDocument();
    expect(await within(main).findByRole("button", { name: "ctower-agents" })).toBeInTheDocument();
    expect(side()).toBeNull();
  });

  it("abrir um projeto: os itens no corpo, sem abrir a gaveta, e as abas segmentadas (#25)", async () => {
    const api = mobileApi();
    const user = renderApp(api, { name: "project" });
    await user.click(await screen.findByRole("button", { name: "ctower-agents" }));
    const items = await screen.findByRole("region", { name: "Itens do projeto" });
    expect(side()).toBeNull();
    // A árvore não fica também na gaveta (fechada): só no corpo.
    expect(screen.queryByRole("complementary", { name: "Árvore do projeto", hidden: true })).toBeNull();
    expect(within(items).getByRole("group", { name: "Visão" })).toBeInTheDocument();
    expect(within(items).getByRole("searchbox", { name: "Buscar por número ou título" })).toBeInTheDocument();
    // Abas do projeto como controle segmentado, com a trilha de volta ao índice.
    const tabs = screen.getByRole("navigation", { name: "Abas do projeto" });
    expect(tabs).toHaveAttribute("data-segmented", "true");
    expect(within(tabs).getByRole("button", { name: "Itens" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("navigation", { name: "Trilha do projeto" })).toBeInTheDocument();
    // Filtros pelo botão da barra dos itens.
    await user.click(within(items).getByRole("button", { name: "Filtros" }));
    expect(within(items).getByRole("group", { name: "Filtrar por status" })).toBeInTheDocument();
    // Quadro no corpo também.
    await user.click(within(items).getByRole("button", { name: "Quadro" }));
    expect(within(items).getByRole("button", { name: "Quadro" })).toHaveAttribute("aria-pressed", "true");
    await user.click(within(items).getByRole("button", { name: "Árvore" }));
    // Outra aba e de volta: os itens só na aba Itens.
    await user.click(within(tabs).getByRole("button", { name: "Sessões" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Itens do projeto" })).toBeNull());
    await user.click(within(screen.getByRole("navigation", { name: "Abas do projeto" })).getByRole("button", { name: "Itens" }));
    // Tocar na linha abre o item.
    const again = await screen.findByRole("region", { name: "Itens do projeto" });
    await user.click(within(again).getByRole("button", { name: /^#201 WebDaemonApi/ }));
    expect(await screen.findByRole("region", { name: "Detalhe do item" })).toBeInTheDocument();
    expect(side()).toBeNull();
    expect(envCalls(api)).toEqual([]);
  });

  it("nenhum texto manda usar atalho de teclado no projeto (#25)", async () => {
    const api = mobileApi();
    renderApp(api, { name: "project", owner: "deep-ion-ai", repo: "ctower-agents" });
    const items = await screen.findByRole("region", { name: "Itens do projeto" });
    await within(items).findByRole("button", { name: /^#201 WebDaemonApi/ });
    const body = document.querySelector(".shell-main")!.textContent ?? "";
    expect(body).not.toMatch(/Ctrl|⌘|paleta|atalho|painel lateral/i);
  });

  it("avisos no sino do cabeçalho, numa folha", async () => {
    const api = mobileApi().onHub("hub.envs.list", [hubEnv({ last_seen_at: now - 10 * 60_000 })]);
    const user = renderApp(api, { name: "hub" });
    const bell = await screen.findByRole("button", { name: /^Notificações, 1 aviso/ });
    await user.click(bell);
    const sheet = screen.getByRole("dialog", { name: /^Avisos/ });
    expect(within(sheet).getByText(/wsl-pc sem envio ao hub/)).toBeInTheDocument();
    await user.click(within(sheet).getByRole("button", { name: "Fechar avisos" }));
    expect(screen.queryByRole("dialog", { name: /^Avisos/ })).toBeNull();
    // A ação do aviso leva à tela e fecha a folha.
    await user.click(bell);
    await user.click(within(screen.getByRole("dialog", { name: /^Avisos/ })).getByRole("button", { name: "Ver hub" }));
    expect(screen.queryByRole("dialog", { name: /^Avisos/ })).toBeNull();
  });

  it("mudar status e prioridade do item pelo hub (sem reordenar)", async () => {
    const api = mobileApi();
    const user = renderApp(api, { name: "project", owner: "deep-ion-ai", repo: "ctower-agents" });
    await user.click(await screen.findByRole("button", { name: /^#201 WebDaemonApi/ }));
    const panel = await screen.findByRole("region", { name: "Detalhe do item" });

    await user.click(within(panel).getByRole("button", { name: /Mudar status/ }));
    await user.click(within(within(panel).getByRole("group", { name: "Mudar status" })).getByRole("button", { name: "bloqueado" }));
    await waitFor(() => expect(api.hubCallsTo("hub.workitem.set_status")).toHaveLength(1));
    expect(api.hubCallsTo("hub.workitem.set_status")[0].params).toEqual({
      owner: "deep-ion-ai",
      repo: "ctower-agents",
      number: 201,
      status: "bloqueado",
    });

    await user.click(within(panel).getByRole("button", { name: /^Prioridade: P2/ }));
    const prio = within(panel).getByRole("group", { name: "Mudar prioridade" });
    // `hub.workitem.move` fica fora do /web/ws.
    expect(within(prio).queryByRole("button", { name: /Subir na ordem/ })).toBeNull();
    expect(within(prio).queryByRole("button", { name: /Descer na ordem/ })).toBeNull();
    await user.click(within(prio).getByRole("button", { name: "P0" }));
    await waitFor(() => expect(api.hubCallsTo("hub.workitem.set_priority")).toHaveLength(1));
    expect(api.hubCallsTo("hub.workitem.set_priority")[0].params).toEqual({
      owner: "deep-ion-ai",
      repo: "ctower-agents",
      number: 201,
      priority: "P0",
    });
    expect(envCalls(api)).toEqual([]);
  });

  it("aprovar e rejeitar propostas do gestor pelo hub", async () => {
    let pending = [hubProposal(), hubProposal({ id: 2, number: 12 })];
    const api = mobileApi()
      .onHub("hub.proposals.list", () => ({ proposals: pending, truncated: false }))
      .onHub("hub.proposals.decide", (p: { id: number; approve: boolean }) => {
        const x = pending.find((y) => y.id === p.id)!;
        pending = pending.filter((y) => y.id !== p.id);
        return { ...x, state: p.approve ? "aprovada" : "rejeitada", decided_at: now };
      });
    const user = renderApp(api, { name: "hub", section: "proposals" });
    await user.click(await screen.findByRole("button", { name: "Aprovar a proposta #1" }));
    await waitFor(() => expect(api.hubCallsTo("hub.proposals.decide")).toHaveLength(1));
    await user.click(await screen.findByRole("button", { name: "Rejeitar a proposta #2" }));
    await waitFor(() => expect(api.hubCallsTo("hub.proposals.decide")).toHaveLength(2));
    expect(api.hubCallsTo("hub.proposals.decide").map((c) => c.params)).toEqual([
      { id: 1, approve: true },
      { id: 2, approve: false },
    ]);
    expect(envCalls(api)).toEqual([]);
  });

  it("Aguardando você em destaque no topo: com o Remote Control, um toque abre o claude.ai", async () => {
    const api = mobileApi({ live_sessions: [hubSession({ rc_session_id: "session_abc" })] }, [
      hubItem({ status: "aguardando_voce" }),
    ]);
    const user = renderApp(api);
    const box = await screen.findByRole("region", { name: "Aguardando você · 1" });
    // No topo: antes da faixa e da grade.
    const main = document.querySelector(".control-main")!;
    const head = main.querySelector(".control-head")!;
    expect(head.nextElementSibling).toBe(box);
    expect(screen.queryByRole("region", { name: /^Precisa de você/ })).toBeNull();
    await user.click(within(box).getByRole("button", { name: /^Abrir no Remote Control: .*#201 WebDaemonApi/ }));
    expect(api.opened).toEqual(["https://claude.ai/code/session_abc"]);
    expect(envCalls(api)).toEqual([]);
  });

  it("Aguardando você sem Remote Control: o toque leva ao painel da sessão", async () => {
    const api = mobileApi({}, [hubItem({ status: "aguardando_voce" })]);
    const user = renderApp(api);
    const box = await screen.findByRole("region", { name: "Aguardando você · 1" });
    await user.click(within(box).getByRole("button", { name: /^Ver sessão: .*#201 WebDaemonApi/ }));
    expect(await screen.findByRole("heading", { name: /#201 WebDaemonApi/ })).toBeInTheDocument();
    expect(api.opened).toEqual([]);
  });

  it("telas da web pela barra inferior: Revisão, Recuperação e Hub sem Segurança", async () => {
    const api = mobileApi({ interrupted_sessions: [hubSession({ id: "s-int", state: "interrupted" })] });
    const user = renderApp(api);
    const bar = await screen.findByRole("navigation", { name: "Telas" });
    await user.click(within(bar).getByRole("button", { name: "Revisão" }));
    expect(await screen.findByRole("heading", { name: /^Revisão · / })).toBeInTheDocument();
    await user.click(within(bar).getByRole("button", { name: "Recuperação" }));
    expect(await screen.findByRole("heading", { name: "Recuperação · 1" })).toBeInTheDocument();
    // Ambiente online (canal reverso): retomar está no celular também.
    expect(screen.getByRole("button", { name: /^Retomar/ })).toBeInTheDocument();
    await user.click(within(bar).getByRole("button", { name: "Hub" }));
    // Tocar na tela atual abre a gaveta, com as seções do Hub.
    await user.click(within(bar).getByRole("button", { name: "Hub" }));
    const drawer = await screen.findByRole("region", { name: "Painel lateral" });
    expect(within(drawer).queryByText("Segurança")).toBeNull();
    expect(within(drawer).getByText("Propostas do gestor")).toBeInTheDocument();
  });
});

describe("fora do celular", () => {
  it("web em janela larga: shell do desktop", async () => {
    setViewport(1280);
    renderApp(mobileApi());
    const bar = await screen.findByRole("navigation", { name: "Telas" });
    expect(bar).not.toHaveClass("bottombar");
    expect(screen.getByRole("contentinfo", { name: "Barra de status" })).toBeInTheDocument();
    expect(screen.getByRole("tablist", { name: "Abas abertas" })).toBeInTheDocument();
  });

  it("app desktop em janela estreita continua com a barra de atividades ao lado", async () => {
    setViewport(360);
    render(<App api={new FakeDaemonApi()} />);
    const bar = await screen.findByRole("navigation", { name: "Telas" });
    expect(bar).not.toHaveClass("bottombar");
    expect(within(bar).getByRole("button", { name: "Terminal" })).toBeInTheDocument();
  });
});

/**
 * CSS lido do disco: com `css: false`, o Vitest entrega os .css vazios (até
 * com `?raw`), e o projeto não tem os tipos do Node.
 */
async function readCss(path: string): Promise<string> {
  const fs = (await import(/* @vite-ignore */ "node:" + "fs")) as {
    readFileSync(p: string, enc: "utf8"): string;
  };
  return fs.readFileSync(path, "utf8");
}

describe("alvos de toque e tokens", () => {
  let shellCss = "";
  let controlCss = "";
  let themeCss = "";
  let mobileBlock = "";
  beforeAll(async () => {
    shellCss = await readCss("src/shell/shell.css");
    controlCss = await readCss("src/control-room/control-room.css");
    themeCss = await readCss("src/theme.css");
    mobileBlock = shellCss.slice(shellCss.indexOf('.shell[data-mobile="true"] .titlebar'));
  });

  it("controles do shell no celular com pelo menos var(--touch), definido como 44 px", () => {
    expect(themeCss).toMatch(/--touch:\s*44px;/);
    // Todo botão dentro do shell (painel da sessão, Sessões web, menus).
    expect(mobileBlock).toMatch(/\.shell\[data-mobile="true"\] :is\(button[^{]*\{\s*min-height: var\(--touch\);/);
    expect(mobileBlock).toMatch(/\.shell\[data-mobile="true"\] \.ibtn \{\s*width: var\(--touch\);\s*height: var\(--touch\);/);
    expect(mobileBlock).toMatch(/\.bottombar \.ab-item \{[^}]*min-width: var\(--touch\);/);
    expect(controlCss).toMatch(/\.waiting-now-card \{[^}]*min-height: var\(--touch\);/);
  });

  it("modal de início no celular (fora do .shell): tela cheia e alvos de 44 px", async () => {
    const modalCss = await readCss("src/project/start-modal.css");
    const block = modalCss.slice(modalCss.indexOf("@media (max-width: 767px)"));
    expect(block).toMatch(/\.sm-modal :is\(button[^{]*\.sm-check \{\s*min-height: var\(--touch\);/);
    expect(block).not.toMatch(/#[0-9a-fA-F]{3,8}\b|rgba?\(/);
  });

  it("só tokens de theme.css: nenhuma cor literal nas regras do celular", () => {
    expect(mobileBlock).not.toMatch(/#[0-9a-fA-F]{3,8}\b|rgba?\(/);
    const waiting = controlCss.slice(controlCss.indexOf(".waiting-now {"));
    expect(waiting).not.toMatch(/#[0-9a-fA-F]{3,8}\b|rgba?\(/);
  });
});
