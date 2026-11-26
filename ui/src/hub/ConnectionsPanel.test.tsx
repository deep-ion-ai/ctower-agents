import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { Connection } from "../protocol/gen";
import { HUB_DASHBOARD, NO_HEALTH, NO_PROPOSALS, dashboard, hubEnv, hubInfo, hubStatus } from "../test/fixtures";

const JIRA: Connection = {
  id: 7,
  name: "jira-acme",
  kind: "jira",
  endpoint: "https://jira.example.com",
  credential: { kind: "command", cmd: "pass show jira | head -1" },
  options: {},
  created_at: 1,
  updated_at: 1,
};

function setup(list: Connection[] = [JIRA]) {
  let conns = list;
  const api = new FakeDaemonApi([{ ...FakeDaemonApi.wslEnv(), name: "WSL" }])
    .on("dashboard.get", dashboard({ projects: [] }))
    .withHub("ws://mac-mini:7879/ws", true)
    .onHub("hub.info", hubInfo({ protocol: "1.5" }))
    .onHub("hub.status", hubStatus())
    .onHub("hub.projects.list", [])
    .onHub("hub.envs.list", [hubEnv()])
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("hub.proposals.list", NO_PROPOSALS)
    .onHub("hub.health.get", NO_HEALTH)
    .onHub("hub.connections.list", () => conns)
    .onHub("hub.connections.test", { id: 7, name: "jira-acme", readiness: { state: "ready" } })
    .onHub("hub.connections.add", () => {
      conns = [...conns, { ...JIRA, id: 9, name: "jira-beta" }];
      return conns[conns.length - 1];
    });
  render(<App api={api} initial={{ name: "hub", section: "connections" }} />);
  return { api, user: userEvent.setup() };
}

test("lista as conexões, testa a prontidão e nunca mostra o valor do token", async () => {
  const { api, user } = setup();
  const list = await screen.findByRole("list", { name: "Conexões cadastradas" });
  expect(within(list).getByText("jira-acme")).toBeInTheDocument();
  expect(within(list).getByText("comando: pass show jira | head -1")).toBeInTheDocument();
  await user.click(within(list).getByRole("button", { name: "Testar" }));
  expect(await screen.findByText("Prontidão: pronta")).toBeInTheDocument();
  expect(api.hubCallsTo("hub.connections.test")[0].params).toEqual({ id: 7 });
});

test("cria uma conexão do Jira pelo formulário e manda o cadastro ao hub", async () => {
  const { api, user } = setup();
  await screen.findByRole("list", { name: "Conexões cadastradas" });
  await user.click(screen.getByRole("button", { name: "Nova conexão" }));
  const form = screen.getByRole("form", { name: "Nova conexão" });
  await user.type(within(form).getByLabelText("Nome"), "jira-beta");
  await user.type(within(form).getByLabelText("URL"), "https://jira.example.com");
  await user.type(within(form).getByLabelText("Comando que imprime o token"), "pass show jira | head -1");
  await user.click(within(form).getByRole("button", { name: "Salvar conexão" }));
  expect(await screen.findByText("Conexão jira-beta salva.")).toBeInTheDocument();
  expect(api.hubCallsTo("hub.connections.add")[0].params).toEqual({
    name: "jira-beta",
    endpoint: "https://jira.example.com",
    credential: { kind: "command", cmd: "pass show jira | head -1" },
    options: {},
    kind: "jira",
  });
});

test("sem conexão, o formulário pede o nome antes de chamar o hub", async () => {
  const { api, user } = setup([]);
  expect(await screen.findByText(/Nenhuma conexão ainda/)).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Nova conexão" }));
  const form = screen.getByRole("form", { name: "Nova conexão" });
  await user.click(within(form).getByRole("button", { name: "Salvar conexão" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("informe o nome da conexão");
  expect(api.hubCallsTo("hub.connections.add")).toHaveLength(0);
});
