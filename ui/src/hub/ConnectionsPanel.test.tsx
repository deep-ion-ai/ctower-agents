import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { Connection } from "../protocol/gen";
import type { HubProjectView } from "../protocol/hub-gen";
import {
  HUB_DASHBOARD,
  NO_HEALTH,
  NO_PROPOSALS,
  dashboard,
  hubEnv,
  hubInfo,
  hubProjectView,
  hubStatus,
  jiraHubProject,
} from "../test/fixtures";

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

function setup(list: Connection[] = [JIRA], projects: HubProjectView[] = []) {
  let conns = list;
  const api = new FakeDaemonApi([{ ...FakeDaemonApi.wslEnv(), name: "WSL" }])
    .on("dashboard.get", dashboard({ projects: [] }))
    .withHub("ws://mac-mini:7879/ws", true)
    .onHub("hub.info", hubInfo({ protocol: "1.5" }))
    .onHub("hub.status", hubStatus())
    .onHub("hub.projects.list", projects)
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

test("conexão do Jira com secret do hub (#274): a forma aparece no formulário e vai ao hub", async () => {
  const { api, user } = setup();
  await screen.findByRole("list", { name: "Conexões cadastradas" });
  await user.click(screen.getByRole("button", { name: "Nova conexão" }));
  const form = screen.getByRole("form", { name: "Nova conexão" });
  const cred = within(form).getByLabelText("Credencial");
  expect(within(cred).getAllByRole("option").map((o) => o.getAttribute("value"))).toEqual(["secret", "command", "file"]);
  await user.selectOptions(cred, "secret");
  await user.type(within(form).getByLabelText("Nome"), "jira-beta");
  await user.type(within(form).getByLabelText("URL"), "https://jira.example.com");
  await user.type(within(form).getByLabelText("Nome do secret do hub"), "JIRA_TOKEN_ACME");
  expect(within(form).getByText(/o do projeto vale sobre o global/)).toBeInTheDocument();
  await user.click(within(form).getByRole("button", { name: "Salvar conexão" }));
  expect(await screen.findByText("Conexão jira-beta salva.")).toBeInTheDocument();
  expect(api.hubCallsTo("hub.connections.add")[0].params).toMatchObject({
    kind: "jira",
    credential: { kind: "secret", name: "JIRA_TOKEN_ACME" },
  });
});

test("cria uma conexão do Jira pelo formulário e manda o cadastro ao hub", async () => {
  const { api, user } = setup();
  await screen.findByRole("list", { name: "Conexões cadastradas" });
  await user.click(screen.getByRole("button", { name: "Nova conexão" }));
  const form = screen.getByRole("form", { name: "Nova conexão" });
  await user.type(within(form).getByLabelText("Nome"), "jira-beta");
  await user.type(within(form).getByLabelText("URL"), "https://jira.example.com");
  // A credencial começa no secret do hub, a fonte padrão (#281).
  expect(within(form).getByLabelText("Credencial")).toHaveValue("secret");
  await user.type(within(form).getByLabelText("Nome do secret do hub"), "JIRA_TOKEN_BETA");
  await user.click(within(form).getByRole("button", { name: "Salvar conexão" }));
  expect(await screen.findByText("Conexão jira-beta salva.")).toBeInTheDocument();
  expect(api.hubCallsTo("hub.connections.add")[0].params).toEqual({
    name: "jira-beta",
    endpoint: "https://jira.example.com",
    credential: { kind: "secret", name: "JIRA_TOKEN_BETA" },
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

test("conexão em uso mostra os projetos e não deixa renomear (#271)", async () => {
  // O projeto Jira do exemplo usa a conexão 7 no rastreador.
  const { api, user } = setup([JIRA], [hubProjectView(jiraHubProject())]);
  const list = await screen.findByRole("list", { name: "Conexões cadastradas" });
  expect(await within(list).findByText("grupo/projeto/app")).toBeInTheDocument();
  await user.click(within(list).getByRole("button", { name: "Editar" }));
  const form = screen.getByRole("form", { name: "Editar jira-acme" });
  const name = within(form).getByLabelText("Nome");
  expect(name).toBeDisabled();
  expect(name).toHaveAccessibleDescription(/Em uso por grupo\/projeto\/app/);
  expect(name).toHaveAccessibleDescription(/\[connections\."jira-acme"\]/);
  expect(api.hubCallsTo("hub.connections.update")).toHaveLength(0);
  // Excluir também fica desabilitado, com o motivo (#281).
  const del = within(list).getByRole("button", { name: "Excluir" });
  expect(del).toBeDisabled();
  expect(del).toHaveAccessibleDescription(/Em uso por grupo\/projeto\/app: troque a conexão/);
});

test("conexão sem projeto pode ser excluída, com confirmação (#281)", async () => {
  const { user } = setup([JIRA], []);
  const list = await screen.findByRole("list", { name: "Conexões cadastradas" });
  const del = await within(list).findByRole("button", { name: "Excluir" });
  expect(del).toBeEnabled();
  await user.click(del);
  expect(within(list).getByRole("button", { name: "Confirmar exclusão de jira-acme" })).toBeEnabled();
});

test("a recusa do hub ao renomear aparece junto do formulário (#271)", async () => {
  const { api, user } = setup();
  api.onHub("hub.connections.update", () => {
    throw new Error(
      'a conexão jira-acme está em uso pelos projetos acme/acme-web; renomear desligaria as sobrescritas [connections."jira-acme"] dos ambientes',
    );
  });
  const list = await screen.findByRole("list", { name: "Conexões cadastradas" });
  expect(within(list).getByText("nenhum projeto")).toBeInTheDocument();
  await user.click(within(list).getByRole("button", { name: "Editar" }));
  const form = screen.getByRole("form", { name: "Editar jira-acme" });
  const name = within(form).getByLabelText("Nome");
  expect(name).toBeEnabled();
  await user.clear(name);
  await user.type(name, "jira-empresa");
  await user.click(within(form).getByRole("button", { name: "Salvar conexão" }));
  expect(await within(form).findByRole("alert")).toHaveTextContent("em uso pelos projetos acme/acme-web");
  expect(api.hubCallsTo("hub.connections.update")[0].params).toMatchObject({ id: 7, name: "jira-empresa" });
});
