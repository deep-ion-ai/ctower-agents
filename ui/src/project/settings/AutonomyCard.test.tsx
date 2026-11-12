import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../../App";
import { DEFAULT_AUTONOMOUS_TEMPLATE } from "../../hub/autonomy";
import { FakeDaemonApi } from "../../api/fake";
import type { HubProject, HubProjectView } from "../../protocol/hub-gen";
import {
  HUB_DASHBOARD,
  NO_HEALTH,
  NO_PROPOSALS,
  dashboard,
  hubAssignment,
  hubEnv,
  hubInfo,
  hubProject,
  hubProjectView,
  hubStatus,
} from "../../test/fixtures";

const ASSIGNMENTS = [
  hubAssignment({ env_id: "env-auto", env_name: "linux-autonomo" }),
  hubAssignment({ env_id: "daemon-mac", env_name: "mac-mini" }),
];

function setup(project: HubProject) {
  const projects: HubProjectView[] = [hubProjectView(project, ASSIGNMENTS)];
  const api = new FakeDaemonApi([{ ...FakeDaemonApi.wslEnv(), name: "WSL" }])
    .on("dashboard.get", dashboard({ projects: [] }))
    .withHub("ws://mac-mini:7879/ws", true)
    .onHub("hub.info", hubInfo({ protocol: "1.15" }))
    .onHub("hub.status", hubStatus())
    .onHub("hub.projects.list", () => projects)
    .onHub("hub.envs.list", [
      hubEnv({ env_id: "env-auto", name: "linux-autonomo", autonomous: true }),
      hubEnv({ env_id: "daemon-mac", name: "mac-mini" }),
    ])
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("hub.proposals.list", NO_PROPOSALS)
    .onHub("hub.health.get", NO_HEALTH)
    .onHub("github.status", { source: "token", login: "hub-bot", error: null })
    .onHub("hub.projects.update", () => project);
  render(
    <App
      api={api}
      initial={{ name: "project", owner: project.gh_owner, repo: project.gh_repo, view: "settings", section: "autonomy" }}
    />,
  );
  return { api, user: userEvent.setup() };
}

test("ligar o modo: só ambientes autônomos atribuídos, e o objeto inteiro vai ao hub", async () => {
  const { api, user } = setup(hubProject());
  const card = await screen.findByRole("region", { name: "Modo autônomo" });
  expect(within(card).getByText("desligado")).toBeInTheDocument();
  await user.click(within(card).getByRole("button", { name: /Editar/ }));
  const form = screen.getByRole("form", { name: /Modo autônomo de/ });
  const env = within(form).getByLabelText(/Ambiente/);
  expect(within(env).getAllByRole("option").map((o) => o.textContent)).toEqual(["—", "linux-autonomo"]);
  await user.click(within(form).getByLabelText("Ligar o modo autônomo"));
  expect(within(form).getByRole("button", { name: "Salvar modo autônomo" })).toBeDisabled();
  await user.selectOptions(env, "env-auto");
  await user.selectOptions(within(form).getByLabelText(/Alinhamento/), "rebase");
  await user.click(within(form).getByRole("button", { name: "Salvar modo autônomo" }));
  expect(await screen.findByText(/ligado em linux-autonomo/)).toBeInTheDocument();
  expect(api.hubCallsTo("hub.projects.update")[0].params).toEqual({
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    autonomy: { enabled: true, env_id: "env-auto", branch: "", base_branch: "", strategy: "rebase" },
  });
});

test("conflito com a base: o aviso aparece com os arquivos", async () => {
  setup(
    hubProject({
      autonomy: { enabled: true, env_id: "env-auto", branch: "", base_branch: "", strategy: "merge" },
      autonomy_report: {
        state: "conflict",
        branch: "wip/autonoma",
        base: "develop",
        message: "wip/autonoma em conflito com a base",
        conflicts: ["src/app.ts"],
        pr_url: null,
        pr_pending: false,
        at: 1,
      },
    }),
  );
  const alert = await screen.findByRole("alert");
  expect(alert).toHaveTextContent("wip/autonoma em conflito com a base");
  expect(alert).toHaveTextContent("src/app.ts");
});

test("template autônomo (#191): padrão como placeholder e o objeto inteiro de session_templates vai ao hub", async () => {
  const templates = { issue: "Issue {{key}}", epic: "", project: "", autonomous: "" };
  const { api, user } = setup(hubProject({ session_templates: templates }));
  const card = await screen.findByRole("region", { name: "Modo autônomo" });
  expect(within(card).getByText("template padrão")).toBeInTheDocument();
  await user.click(within(card).getByRole("button", { name: /Editar/ }));
  const form = screen.getByRole("form", { name: /Modo autônomo de/ });
  const tpl = within(form).getByLabelText("Template do prompt da sessão autônoma");
  expect(tpl).toHaveAttribute("placeholder", DEFAULT_AUTONOMOUS_TEMPLATE);
  expect(tpl).toHaveValue("");
  await user.type(tpl, "Faça {{{{ref}} sem perguntar");
  await user.click(within(form).getByRole("button", { name: "Salvar modo autônomo" }));
  expect(await screen.findByText(/desligado\./)).toBeInTheDocument();
  expect(api.hubCallsTo("hub.projects.update")[0].params).toMatchObject({
    session_templates: { ...templates, autonomous: "Faça {{ref}} sem perguntar" },
  });
});

test("hub sem o template autônomo: o campo não aparece", async () => {
  const { user } = setup(hubProject({ session_templates: { issue: "", epic: "", project: "" } as never }));
  const card = await screen.findByRole("region", { name: "Modo autônomo" });
  await user.click(within(card).getByRole("button", { name: /Editar/ }));
  const form = screen.getByRole("form", { name: /Modo autônomo de/ });
  expect(within(form).queryByLabelText("Template do prompt da sessão autônoma")).toBeNull();
});
