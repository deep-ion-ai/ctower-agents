import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { Dashboard } from "../protocol/gen";
import { NavProvider, useNav } from "../state/nav";
import { TowerProvider } from "../state/tower";
import { dashboard as base, hubStatus, MIN, session, sync, workItem } from "../test/fixtures";
import { Shell } from "./Shell";
import { SideContent, SideGroup, SidePanel, SideRow } from "./SidePanel";

const NOW = Date.now();
const WAIT = session({ id: "aaaaaaaa-0000-0000-0000-000000000001", work_item_id: 31, last_event_at: NOW - MIN });
const RUN = session({ id: "bbbbbbbb-0000-0000-0000-000000000002", work_item_id: 32 });

const dashboard: typeof base = (d = {}, items = []) =>
  base({ sync: [sync({ last_ok_at: NOW - 2 * MIN, last_attempt_at: NOW - 2 * MIN })], ...d }, items);

function live(): Dashboard {
  return dashboard({ live_sessions: [WAIT, RUN] }, [
    workItem({ gh_number: 31, status: "aguardando_voce", title: "ui: Sala de controle" }),
    workItem({ gh_number: 32, status: "em_execucao", title: "ui: terminal embutido" }),
  ]);
}

function setup(d: Dashboard = dashboard(), api = new FakeDaemonApi()) {
  api.on("dashboard.get", d).on("session.peek", { data: "" }).on("sync.now", []);
  render(<App api={api} />);
  return { api, user: userEvent.setup(), nav: screen.getByRole("navigation", { name: "Telas" }) };
}

const tabs = () =>
  within(screen.getByRole("tablist", { name: "Abas abertas" }))
    .getAllByRole("tab")
    .map((t) => t.textContent);

test("barra de atividades: todas as telas, Configurações por último, e Tab percorre os ícones", async () => {
  const { user, nav } = setup();
  const nomes = within(nav)
    .getAllByRole("button")
    .map((b) => b.getAttribute("aria-label"));
  expect(nomes).toEqual(["Sala de controle", "Terminal", "Projetos", "Revisão", "Recuperação", "Hub", "Manutenção", "Configurações"]);
  act(() => within(nav).getByRole("button", { name: "Sala de controle" }).focus());
  await user.tab();
  expect(within(nav).getByRole("button", { name: "Terminal" })).toHaveFocus();
  await user.tab();
  expect(within(nav).getByRole("button", { name: "Projetos" })).toHaveFocus();
  await user.click(within(nav).getByRole("button", { name: "Hub" }));
  expect(within(nav).getByRole("button", { name: "Hub" })).toHaveAttribute("aria-current", "page");
  expect(within(nav).getByRole("button", { name: "Sala de controle" })).not.toHaveAttribute("aria-current");
});

test("contadores refletem o useTower em tempo real", async () => {
  const api = new FakeDaemonApi()
    .withHub()
    .onHub("hub.status", hubStatus({ warnings: [{ kind: "backup_same_device", message: "mesmo disco" }] }));
  const { nav } = setup(live(), api);
  const btn = (name: string) => within(nav).getByRole("button", { name });
  await waitFor(() => expect(btn("Sala de controle")).toHaveAccessibleDescription("1 aguardando você"));
  await waitFor(() => expect(btn("Hub")).toHaveAccessibleDescription("1 aviso"));
  expect(btn("Recuperação")).not.toHaveAccessibleDescription();

  // Push: a sessão foi interrompida e a outra parou de esperar.
  api.on(
    "dashboard.get",
    dashboard({ live_sessions: [RUN], interrupted_sessions: [WAIT] }, [
      workItem({ gh_number: 31, status: "em_execucao" }),
      workItem({ gh_number: 32, status: "em_execucao" }),
    ]),
  );
  act(() => api.push("wsl", "session_state", {}));
  await waitFor(() => expect(btn("Recuperação")).toHaveAccessibleDescription("1 interrompida"));
  expect(btn("Sala de controle")).not.toHaveAccessibleDescription();
  expect(within(nav).getAllByText("1", { selector: ".ab-badge" })).toHaveLength(2);
});

test("abas ficam abertas ao trocar de tela; fechar ativa a vizinha", async () => {
  const { user, nav } = setup(live());
  await user.click(await screen.findByRole("button", { name: /ui: terminal embutido/ }));
  expect(within(nav).getByRole("button", { name: "Terminal" })).toHaveAttribute("aria-current", "page");
  await user.click(within(nav).getByRole("button", { name: "Configurações" }));
  await user.click(within(nav).getByRole("button", { name: "Revisão" }));
  expect(tabs()).toEqual(["Sala de controle", "#32 ui: terminal embutido", "Configurações", "Revisão"]);
  expect(screen.getByRole("tab", { name: "Revisão" })).toHaveAttribute("aria-selected", "true");

  // A aba da sessão continua lá e volta ao terminal dela.
  await user.click(screen.getByRole("tab", { name: "#32 ui: terminal embutido" }));
  expect(within(nav).getByRole("button", { name: "Terminal" })).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("region", { name: "Sessão" })).toHaveTextContent("#32 ui: terminal embutido");

  await user.click(screen.getByRole("button", { name: "Fechar aba Configurações" }));
  await user.click(screen.getByRole("button", { name: "Fechar aba #32 ui: terminal embutido" }));
  expect(tabs()).toEqual(["Sala de controle", "Revisão"]);
  expect(screen.getByRole("tab", { name: "Sala de controle" })).toHaveAttribute("aria-selected", "true");
  // Sem sessões abertas, o Terminal volta ao estado vazio.
  expect(within(nav).getByRole("button", { name: "Terminal" })).not.toHaveAccessibleDescription();
});

test("voltar a uma tela pela barra de atividades reabre a aba dela com o mesmo endereço", async () => {
  const { user, nav } = setup(live());
  await user.click(await screen.findByRole("button", { name: /ui: terminal embutido/ }));
  await user.click(within(nav).getByRole("button", { name: "Hub" }));
  await user.click(within(nav).getByRole("button", { name: "Terminal" }));
  expect(screen.getByRole("tab", { name: "#32 ui: terminal embutido" })).toHaveAttribute("aria-selected", "true");
  expect(tabs()).toHaveLength(3);
});

test("Ctrl+B recolhe o painel lateral e Ctrl+J abre o painel inferior", async () => {
  const { user } = setup();
  const lateral = screen.getByRole("region", { name: "Painel lateral" });
  expect(lateral).toBeVisible();
  await user.keyboard("{Control>}b{/Control}");
  expect(screen.queryByRole("region", { name: "Painel lateral" })).not.toBeInTheDocument();
  await user.keyboard("{Control>}b{/Control}");
  expect(screen.getByRole("region", { name: "Painel lateral" })).toBeInTheDocument();

  expect(screen.queryByRole("region", { name: "Painel inferior" })).not.toBeInTheDocument();
  await user.keyboard("{Control>}j{/Control}");
  const painel = screen.getByRole("region", { name: "Painel inferior" });
  expect(within(painel).getByRole("tab", { name: "Eventos" })).toHaveAttribute("aria-selected", "true");
  await user.click(within(painel).getByRole("tab", { name: "Sincronização" }));
  expect(within(painel).getByText("GitHub · ctower-agents")).toBeInTheDocument();
  expect(within(painel).getByText("sincronizado há 2 min")).toBeInTheDocument();
  await user.keyboard("{Control>}j{/Control}");
  expect(screen.queryByRole("region", { name: "Painel inferior" })).not.toBeInTheDocument();
});

test("painel inferior: eventos de push e avisos com a ação", async () => {
  const { api, user, nav } = setup();
  await waitFor(() => expect(api.callsTo("dashboard.get").length).toBeGreaterThan(0));
  act(() => api.push("wsl", "session_started", { gh_number: 31 }));
  act(() => api.setStatus("wsl", false, "conexão recusada"));
  await user.click(screen.getByRole("button", { name: "Notificações, 1 aviso" }));
  const painel = screen.getByRole("region", { name: "Painel inferior" });
  expect(within(painel).getByRole("tab", { name: "Avisos · 1" })).toHaveAttribute("aria-selected", "true");
  expect(within(painel).getByText("ctower-agents (WSL) desconectado")).toBeInTheDocument();
  await user.click(within(painel).getByRole("button", { name: "Configurações" }));
  expect(within(nav).getByRole("button", { name: "Configurações" })).toHaveAttribute("aria-current", "page");
  await user.click(within(painel).getByRole("tab", { name: "Eventos" }));
  expect(within(painel).getByText("sessão iniciada #31")).toBeInTheDocument();
  expect(within(painel).getByText("desconectado: conexão recusada")).toBeInTheDocument();
});

test("barra de status: hub, ambientes, sync com o GitHub, avisos, versão e app novo", async () => {
  const api = new FakeDaemonApi().withHub();
  api.uiUpdate = { current: "0.8.0", available: { version: "0.8.1", notes: null, date: null }, error: null };
  const { user, nav } = setup(dashboard(), api);
  const barra = screen.getByRole("contentinfo", { name: "Barra de status" });
  expect(await within(barra).findByText("v0.8.0")).toBeInTheDocument();
  expect(await within(barra).findByText("Ambientes 1/1")).toBeInTheDocument();
  expect(within(barra).getByRole("button", { name: "GitHub sincronizado há 2 min" })).toBeInTheDocument();
  expect(await within(barra).findByRole("button", { name: /^Hub/ })).toHaveTextContent(/^Hub/);
  act(() => api.setHubStatus(false, "recusada"));
  expect(await within(barra).findByRole("button", { name: /indisponível$/ })).toBeInTheDocument();
  expect(within(barra).getByRole("button", { name: "1 aviso" })).toBeInTheDocument();
  await user.click(within(barra).getByRole("button", { name: "App 0.8.1 disponível" }));
  // Atualizar o app fica em Manutenção › Atualizações › Este app (#336).
  expect(within(nav).getByRole("button", { name: "Manutenção" })).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("tab", { name: "Manutenção · Atualizações · Este app" })).toHaveAttribute("aria-selected", "true");
});

/** Tela de teste que fornece o próprio painel lateral e abre abas próprias. */
function Probe() {
  const nav = useNav();
  return (
    <main>
      <h1>Probe</h1>
      <SideContent>
        <SidePanel title="Hub">
          <SideGroup title="Operação">
            <SideRow
              label="Propostas"
              trailing={3}
              onClick={() => nav.openTab({ id: "hub:proposals", title: "Hub · Propostas", screen: { name: "hub" } })}
              actions={
                <button type="button" className="ibtn" aria-label="Atualizar propostas">
                  ↻
                </button>
              }
            />
          </SideGroup>
        </SidePanel>
      </SideContent>
    </main>
  );
}

test("a tela fornece o painel lateral e abre abas próprias", async () => {
  const api = new FakeDaemonApi().on("dashboard.get", dashboard());
  render(
    <TowerProvider api={api}>
      <NavProvider>
        <Shell banners={null} now={NOW}>
          <Probe />
        </Shell>
      </NavProvider>
    </TowerProvider>,
  );
  const user = userEvent.setup();
  const lateral = screen.getByRole("region", { name: "Painel lateral" });
  // O conteúdo da tela substitui o padrão (abas abertas).
  expect(await within(lateral).findByRole("button", { name: "Atualizar propostas" })).toBeInTheDocument();
  expect(within(lateral).queryByRole("button", { name: "Aba Sala de controle" })).not.toBeInTheDocument();
  await user.click(within(lateral).getByRole("button", { name: "Operação" }));
  expect(within(lateral).queryByRole("button", { name: /Propostas/ })).not.toBeInTheDocument();
  await user.click(within(lateral).getByRole("button", { name: "Operação" }));
  await user.click(within(lateral).getByRole("button", { name: /Propostas/ }));
  expect(tabs()).toEqual(["Sala de controle", "Hub · Propostas"]);
  expect(screen.getByRole("tab", { name: "Hub · Propostas" })).toHaveAttribute("aria-selected", "true");
});

test("painel lateral redimensionável pelas setas, dentro dos limites", async () => {
  const { user } = setup();
  const sep = screen.getByRole("separator", { name: "Largura do painel lateral" });
  expect(sep).toHaveAttribute("aria-valuenow", "268");
  act(() => sep.focus());
  await user.keyboard("{ArrowRight}");
  expect(sep).toHaveAttribute("aria-valuenow", "284");
  expect(screen.getByRole("region", { name: "Painel lateral" })).toHaveStyle({ width: "284px" });
  await user.keyboard("{Home}");
  expect(sep).toHaveAttribute("aria-valuenow", "180");
});
