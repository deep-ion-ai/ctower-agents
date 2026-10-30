import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { HubProjectView, HubTrackerStatus } from "../protocol/hub-gen";
import {
  HUB_DASHBOARD,
  NO_HEALTH,
  NO_PROPOSALS,
  dashboard,
  hubAssignment,
  hubEnv,
  hubInfo,
  hubProjectView,
  hubStatus,
  jiraHubProject,
} from "../test/fixtures";
import type { Screen } from "../state/nav";
import type { ProjectSettingsSection } from "./sections";

const NOW = Date.now();
const JIRA = jiraHubProject();
const ASSIGNMENTS = [
  hubAssignment({ project_id: 7, gh_owner: "grupo/projeto", gh_repo: "app" }),
  hubAssignment({ project_id: 7, gh_owner: "grupo/projeto", gh_repo: "app", env_id: "daemon-mac", env_name: "mac-mini" }),
];

function status(p: { credential_ok?: boolean } = {}): HubTrackerStatus {
  return {
    env_id: "daemon-wsl",
    env_name: "wsl-pc",
    received_at: NOW - 60_000,
    readiness: {
      owner: "grupo/projeto",
      repo: "app",
      checked_at: NOW - 60_000,
      jira: {
        cli: "1.7.0",
        config_present: true,
        server_matches: true,
        credential_ok: p.credential_ok ?? true,
        me_ok: p.credential_ok ?? true,
        server_version: "8.5.2",
        error: p.credential_ok === false ? "comando de credencial do Jira falhou (saída 2)" : null,
      },
      gitlab: { cli: "1.124.0", credential_ok: true, api_ok: true, error: null },
    },
  };
}

/** Configurações do projeto Jira (#17; antes da #18, o detalhe do Hub › Projetos), na seção pedida. */
const settingsAt = (section: ProjectSettingsSection): Screen => ({
  name: "project",
  owner: "grupo/projeto",
  repo: "app",
  view: "settings",
  section,
});

/**
 * Sem `at`: com projetos, as Configurações › Geral do projeto Jira; sem
 * projetos, o índice de Projetos (onde fica o cadastro desde a #18).
 */
function setup(opts: { projects?: HubProjectView[]; at?: Screen; protocol?: string } = {}) {
  let projects = opts.projects ?? [{ ...hubProjectView(JIRA, ASSIGNMENTS), tracker_status: null }];
  const api = new FakeDaemonApi([{ ...FakeDaemonApi.wslEnv(), name: "WSL" }])
    .on("dashboard.get", dashboard({ projects: [] }))
    .withHub("ws://mac-mini:7879/ws", true)
    .onHub("hub.info", hubInfo({ protocol: opts.protocol ?? "1.5" }))
    .onHub("hub.status", hubStatus())
    .onHub("hub.projects.list", () => projects)
    .onHub("hub.envs.list", [hubEnv(), hubEnv({ env_id: "daemon-mac", name: "mac-mini" })])
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("hub.proposals.list", NO_PROPOSALS)
    .onHub("hub.health.get", NO_HEALTH)
    .onHub("github.status", { source: "token", login: "hub-bot", error: null })
    .onHub("hub.secrets.list", { status: { entries: 0, store_rev: 0, recovery_configured: true }, items: [], delivery: [] });
  const at = opts.at ?? (projects.length === 0 ? { name: "project" } : settingsAt("general"));
  render(<App api={api} initial={at} />);
  return { api, user: userEvent.setup(), setProjects: (p: HubProjectView[]) => (projects = p) };
}

test("cadastrar Jira + GitLab: campos do Jira e do GitLab, padrão com prévia, sem URL do GitHub", async () => {
  const { api, user, setProjects } = setup({ projects: [] });
  api.onHub("hub.projects.add", () => {
    setProjects([hubProjectView(JIRA)]);
    return { project: JIRA, rev: 3, reapplied: 0, restored: false };
  });
  await user.click(await screen.findByRole("button", { name: "+ Novo projeto" }));
  const form = screen.getByRole("form", { name: "Novo projeto" });
  await user.selectOptions(within(form).getByLabelText("Repositório do código-fonte"), "gitlab");
  await user.selectOptions(within(form).getByLabelText("Repositório de demandas (issues)"), "jira");
  expect(within(form).queryByLabelText("URL do GitHub")).not.toBeInTheDocument();
  const submit = within(form).getByRole("button", { name: "Cadastrar" });
  expect(submit).toBeDisabled();
  await user.type(within(form).getByLabelText("URL do Jira"), "https://jira.example.com");
  await user.type(within(form).getByLabelText("Chave do projeto no Jira"), "PROJ");
  // Projeto novo: sprint ativa marcada, sem JQL nem limite; desmarcar mostra os dois.
  const sprint = within(form).getByLabelText("Sincronizar somente a sprint ativa");
  expect(sprint).toBeChecked();
  expect(within(form).queryByLabelText("JQL do sync")).not.toBeInTheDocument();
  expect(within(form).queryByLabelText("Limite de issues sincronizadas")).not.toBeInTheDocument();
  await user.click(sprint);
  await user.type(within(form).getByLabelText("JQL do sync"), "component = web");
  expect(within(form).getByLabelText("JQL do sync")).toHaveAccessibleDescription(
    /Mudar o filtro remove do Control Tower as issues que saem dele, exceto as com work item/,
  );
  const limit = within(form).getByLabelText("Limite de issues sincronizadas");
  expect(limit).toHaveValue(100);
  expect(limit).toHaveAccessibleDescription(/Só as 100 issues mais recentes do JQL .* além das que têm work item/);
  await user.clear(limit);
  await user.type(limit, "250");
  await user.type(within(form).getByLabelText("Comando de credencial do Jira"), "pass show jira | head -1");
  await user.type(
    within(form).getByLabelText("URL do repositório no GitLab (HTTPS ou SSH)"),
    "https://gitlab.example.com/grupo/projeto/app.git",
  );
  await user.type(within(form).getByLabelText("Comando de credencial do GitLab"), "pass show gitlab | head -1");
  expect(within(form).getByLabelText("Campo da sprint (opcional)")).toHaveAttribute(
    "placeholder",
    "customfield_10600 (vazio = detectar)",
  );
  await user.type(within(form).getByLabelText("Campo da sprint (opcional)"), "customfield_10600");
  // Prévia do padrão: o do Jira sem nada digitado, o do exemplo com {key}.
  expect(within(form).getByText("feat/PROJ-5450-portar-o-plugin")).toBeInTheDocument();
  await user.type(within(form).getByLabelText("Padrão de branch"), "{{key}");
  expect(within(form).getByText("PROJ-5450")).toBeInTheDocument();
  await user.click(within(form).getByLabelText("Linhas de atribuição nos commits e MRs"));
  await user.type(within(form).getByLabelText(/^Protegidas/), "develop, master");
  await user.click(submit);
  expect(await screen.findByText(/grupo\/projeto\/app cadastrado no hub\./)).toBeInTheDocument();
  expect(api.hubCallsTo("hub.projects.add")[0].params).toEqual({
    tracker: {
      kind: "jira",
      url: "https://jira.example.com",
      project_key: "PROJ",
      active_sprint_only: false,
      jql: "component = web",
      max_issues: 250,
      credential_cmd: "pass show jira | head -1",
      config_file: "",
      epic_link_field: "",
      epic_name_field: "",
      sprint_field: "customfield_10600",
      type_map: { Epic: "epic", Épico: "epic", Bug: "bug", Erro: "bug" },
    },
    forge: {
      kind: "gitlab",
      clone_url: "https://gitlab.example.com/grupo/projeto/app.git",
      credential_cmd: "pass show gitlab | head -1",
    },
    branch_pattern: "{key}",
    attribution: false,
    protected_branches: ["develop", "master"],
  });
  // Formulário longo, digitado tecla a tecla: folga além dos 5 s com a máquina ocupada.
}, 15_000);

async function jiraForm(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: "+ Novo projeto" }));
  const form = screen.getByRole("form", { name: "Novo projeto" });
  await user.selectOptions(within(form).getByLabelText("Repositório do código-fonte"), "gitlab");
  await user.selectOptions(within(form).getByLabelText("Repositório de demandas (issues)"), "jira");
  await user.type(within(form).getByLabelText("URL do Jira"), "https://jira.exemplo.com");
  await user.type(within(form).getByLabelText("Chave do projeto no Jira"), "PROJ");
  await user.type(within(form).getByLabelText("Comando de credencial do Jira"), "pass show jira/token | head -1");
  await user.type(
    within(form).getByLabelText("URL do repositório no GitLab (HTTPS ou SSH)"),
    "https://gitlab.exemplo.com/grupo/projeto.git",
  );
  await user.type(within(form).getByLabelText("Comando de credencial do GitLab"), "pass show gitlab/token | head -1");
  return form;
}

test("cadastro com a sprint ativa: sem JQL e sem limite obrigatórios", async () => {
  const { api, user, setProjects } = setup({ projects: [] });
  api.onHub("hub.projects.add", () => {
    setProjects([hubProjectView(JIRA)]);
    return { project: JIRA, rev: 3, reapplied: 0, restored: false };
  });
  const form = await jiraForm(user);
  expect(within(form).getByLabelText("Sincronizar somente a sprint ativa")).toHaveAccessibleDescription(
    /só as issues das sprints abertas .* sem limite de quantidade/,
  );
  await user.click(within(form).getByRole("button", { name: "Cadastrar" }));
  await waitFor(() => expect(api.hubCallsTo("hub.projects.add")).toHaveLength(1));
  const t = (api.hubCallsTo("hub.projects.add")[0].params as { tracker: Record<string, unknown> }).tracker;
  expect(t.active_sprint_only).toBe(true);
  expect(t.jql).toBe("");
  expect(t.max_issues).toBe(100);
});

test("cadastro sem a sprint ativa: JQL obrigatório e limite de 1 a 5000", async () => {
  const { api, user } = setup({ projects: [] });
  const form = await jiraForm(user);
  await user.click(within(form).getByLabelText("Sincronizar somente a sprint ativa"));
  await user.click(within(form).getByRole("button", { name: "Cadastrar" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(/informe o JQL do sync/);
  await user.type(within(form).getByLabelText("JQL do sync"), "labels = web");
  const limit = within(form).getByLabelText("Limite de issues sincronizadas");
  await user.clear(limit);
  await user.type(limit, "0");
  // O campo numérico recusa 0 já no navegador (1 a 5000); o hub e o formulário conferem também.
  expect(limit).toBeInvalid();
  await user.click(within(form).getByRole("button", { name: "Cadastrar" }));
  expect(api.hubCallsTo("hub.projects.add")).toHaveLength(0);
});

test("editar projeto Jira mostra o modo, o JQL e o limite atuais", async () => {
  const sprintProject = jiraHubProject();
  if (sprintProject.tracker?.kind === "jira") {
    Object.assign(sprintProject.tracker, { active_sprint_only: false, jql: "labels = web", max_issues: 300 });
  }
  const { user } = setup({ projects: [{ ...hubProjectView(sprintProject, ASSIGNMENTS), tracker_status: null }] });
  const g = await screen.findByRole("group", { name: "Geral" });
  expect(g).toHaveTextContent(/JQL: labels = web · até 300 issues/);
  await user.click(within(g).getByRole("button", { name: /^Editar/ }));
  const form = within(g).getByRole("form", { name: "Editar app" });
  expect(within(form).getByLabelText("Sincronizar somente a sprint ativa")).not.toBeChecked();
  expect(within(form).getByLabelText("JQL do sync")).toHaveValue("labels = web");
  expect(within(form).getByLabelText("Limite de issues sincronizadas")).toHaveValue(300);
  await user.click(within(form).getByLabelText("Sincronizar somente a sprint ativa"));
  expect(within(form).queryByLabelText("JQL do sync")).not.toBeInTheDocument();
  expect(within(form).queryByLabelText("Limite de issues sincronizadas")).not.toBeInTheDocument();
});

test("cadastro: combinação não suportada avisa e não deixa cadastrar", async () => {
  const { user } = setup({ projects: [] });
  await user.click(await screen.findByRole("button", { name: "+ Novo projeto" }));
  const form = screen.getByRole("form", { name: "Novo projeto" });
  expect(within(form).queryByRole("note")).not.toBeInTheDocument();
  await user.selectOptions(within(form).getByLabelText("Repositório de demandas (issues)"), "jira");
  // Código no GitHub, issues no Jira: a URL do GitHub e os campos do Jira aparecem, mas o hub recusaria.
  expect(within(form).getByLabelText("URL do GitHub")).toBeInTheDocument();
  expect(within(form).getByLabelText("URL do Jira")).toBeInTheDocument();
  expect(within(form).queryByLabelText("Comando de credencial do GitLab")).not.toBeInTheDocument();
  expect(within(form).getByRole("note")).toHaveTextContent(
    "Combinação ainda não suportada: use GitHub + GitHub ou GitLab + Jira.",
  );
  await user.type(within(form).getByLabelText("URL do GitHub"), "https://github.com/o/r");
  await user.type(within(form).getByLabelText("URL do Jira"), "https://jira.exemplo.com");
  await user.type(within(form).getByLabelText("Chave do projeto no Jira"), "PROJ");
  expect(within(form).getByRole("button", { name: "Cadastrar" })).toBeDisabled();
  // GitLab + GitHub também não.
  await user.selectOptions(within(form).getByLabelText("Repositório do código-fonte"), "gitlab");
  await user.selectOptions(within(form).getByLabelText("Repositório de demandas (issues)"), "github");
  expect(within(form).getByRole("note")).toHaveTextContent(/Combinação ainda não suportada/);
  expect(within(form).getByRole("button", { name: "Cadastrar" })).toBeDisabled();
});

test("cadastro num hub anterior ao protocolo 1.3: GitLab e Jira desabilitados, com aviso", async () => {
  const { user } = setup({ projects: [], protocol: "1.4" });
  await user.click(await screen.findByRole("button", { name: "+ Novo projeto" }));
  const form = screen.getByRole("form", { name: "Novo projeto" });
  await waitFor(() =>
    expect(within(form).getByRole("note")).toHaveTextContent(
      "O hub (protocolo 1.4) não suporta Jira/GitLab; atualize o hub para 1.5+.",
    ),
  );
  expect(within(form).getByRole("option", { name: "GitLab" })).toBeDisabled();
  expect(within(form).getByRole("option", { name: "Jira" })).toBeDisabled();
  expect(within(form).getAllByRole("option", { name: "GitHub" }).every((o) => !(o as HTMLOptionElement).disabled)).toBe(
    true,
  );
});

test("editar projeto Jira: os dois repositórios mostram o atual, sem trocar", async () => {
  const { user } = setup();
  const g = await screen.findByRole("group", { name: "Geral" });
  await user.click(within(g).getByRole("button", { name: /^Editar/ }));
  const form = within(g).getByRole("form", { name: "Editar app" });
  const code = within(form).getByLabelText("Repositório do código-fonte");
  const issues = within(form).getByLabelText("Repositório de demandas (issues)");
  expect(code).toHaveValue("gitlab");
  expect(issues).toHaveValue("jira");
  expect(code).toBeDisabled();
  expect(issues).toBeDisabled();
  expect(within(form).getByLabelText("URL do Jira")).toHaveValue("https://jira.example.com");
  expect(within(form).getByLabelText("URL do repositório no GitLab (HTTPS ou SSH)")).toHaveValue(
    "https://gitlab.example.com/grupo/projeto/app.git",
  );
});

test("padrão de branch inválido e mapa de tipos inválido não vão ao hub", async () => {
  const { api, user } = setup({ projects: [] });
  await user.click(await screen.findByRole("button", { name: "+ Novo projeto" }));
  const form = screen.getByRole("form", { name: "Novo projeto" });
  await user.selectOptions(within(form).getByLabelText("Repositório do código-fonte"), "gitlab");
  await user.selectOptions(within(form).getByLabelText("Repositório de demandas (issues)"), "jira");
  await user.type(within(form).getByLabelText("Padrão de branch"), "{{prefix}/{{slug}");
  expect(within(form).getByText(/Padrão inválido/)).toBeInTheDocument();
  await user.type(within(form).getByLabelText("URL do Jira"), "https://jira.example.com");
  await user.type(within(form).getByLabelText("Chave do projeto no Jira"), "PROJ");
  await user.type(within(form).getByLabelText("URL do repositório no GitLab (HTTPS ou SSH)"), "git@gitlab.example.com:a/b.git");
  const map = within(form).getByLabelText("Mapa de tipos do Jira (Tipo=papel)");
  await user.clear(map);
  await user.type(map, "Tarefa=trabalho");
  await user.click(within(form).getByRole("button", { name: "Cadastrar" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(/mapa de tipos: "Tarefa=trabalho"/);
  expect(api.hubCallsTo("hub.projects.add")).toHaveLength(0);
});

test("Configurações do projeto Jira: Jira e GitLab, sem GitHub, e aguardando o ambiente de sync", async () => {
  const { user } = setup();
  const nav = await screen.findByRole("navigation", { name: "Seções das configurações do projeto" });
  // Geral: o repositório com o Jira, o GitLab e as credenciais, sem GitHub.
  const g = await screen.findByRole("group", { name: "Geral" });
  expect(g).not.toHaveTextContent(/GitHub/);
  expect(within(g).queryByRole("link", { name: /github\.com/ })).not.toBeInTheDocument();
  expect(g).toHaveTextContent("https://jira.example.com · projeto PROJ");
  expect(g).toHaveTextContent("https://gitlab.example.com/grupo/projeto/app.git");
  expect(g).toHaveTextContent("pass show jira | head -1");
  expect(g).toHaveTextContent("sem linhas de atribuição");
  // Repositório e rastreador: o ambiente de sync, aguardando o relato.
  await user.click(within(nav).getByRole("button", { name: "Repositório e rastreador" }));
  const t = await screen.findByRole("group", { name: "Repositório e rastreador" });
  const card = within(t).getByRole("heading", { name: "Ambiente de sync" }).closest("section")!;
  expect(card).toHaveTextContent(/wsl-pc busca o Jira e o GitLab/);
  expect(within(card).getByRole("status")).toHaveTextContent(/Aguardando o ambiente de sync/);
  // Ambientes: a atribuição do ambiente de sync tem a marca.
  await user.click(within(nav).getByRole("button", { name: "Ambientes e atribuições" }));
  const envs = await screen.findByRole("group", { name: "Ambientes e atribuições" });
  const table = within(envs).getByRole("table", { name: "Atribuições de app" });
  expect(within(table).getByText("sync")).toBeInTheDocument();
});

test("trocar o ambiente de sync entre os atribuídos", async () => {
  const { api, user } = setup({ at: settingsAt("tracker") });
  api.onHub("hub.projects.update", { ...JIRA, sync_env_id: "daemon-mac" });
  const g = await screen.findByRole("group", { name: "Repositório e rastreador" });
  await user.click(within(g).getByRole("button", { name: "Trocar ambiente de sync" }));
  const form = within(g).getByRole("form", { name: "Ambiente de sync de app" });
  const sel = within(form).getByLabelText("Ambiente (entre os atribuídos)");
  expect(within(sel).getAllByRole("option").map((o) => o.textContent)).toEqual(["wsl-pc (atual)", "mac-mini"]);
  expect(sel).toHaveValue("daemon-mac");
  await user.click(within(form).getByRole("button", { name: "Usar como ambiente de sync" }));
  await waitFor(() => expect(api.hubCallsTo("hub.projects.update")).toHaveLength(1));
  expect(api.hubCallsTo("hub.projects.update")[0].params).toEqual({
    owner: "grupo/projeto",
    repo: "app",
    sync_env: "daemon-mac",
  });
  expect(await screen.findByText(/mac-mini é o ambiente de sync de app/)).toBeInTheDocument();
});

test("prontidão do ambiente de sync: o que falta, com a instrução", async () => {
  setup({
    at: settingsAt("tracker"),
    projects: [
      {
        ...hubProjectView(JIRA, ASSIGNMENTS),
        sync: { project_id: 7, last_attempt_at: NOW - 60_000, last_ok_at: NOW - 3_600_000, last_error: "comando de credencial do Jira falhou (saída 2)" },
        tracker_status: status({ credential_ok: false }),
      },
    ],
  });
  const g = await screen.findByRole("group", { name: "Repositório e rastreador" });
  const r = within(g).getByLabelText("Prontidão de wsl-pc");
  expect(r).toHaveTextContent(/2 item\(ns\) a configurar/);
  expect(within(r).getByRole("list", { name: "Jira" })).toHaveTextContent(
    'rode "pass show jira | head -1" num terminal do ambiente',
  );
  expect(within(r).getByRole("list", { name: "Jira" })).toHaveTextContent("1.7.0");
  expect(within(r).getByRole("list", { name: "GitLab" })).not.toHaveTextContent(/falta/);
  // O erro do último sync aparece na Sincronização, no cartão do repositório (Geral).
  const nav = screen.getByRole("navigation", { name: "Seções das configurações do projeto" });
  await userEvent.setup().click(within(nav).getByRole("button", { name: "Geral" }));
  const geral = await screen.findByRole("group", { name: "Geral" });
  expect(geral).toHaveTextContent(/sincronização com erro/);
  expect(geral).toHaveTextContent(/pelo ambiente de sync wsl-pc/);
});

test("prontidão mostra o progresso do sync em andamento", async () => {
  const st = status();
  st.readiness.progress = { done: 400, total: 3800, started_at: NOW - 120_000 };
  setup({ at: settingsAt("tracker"), projects: [{ ...hubProjectView(JIRA, ASSIGNMENTS), tracker_status: st }] });
  const g = await screen.findByRole("group", { name: "Repositório e rastreador" });
  const r = within(g).getByLabelText("Prontidão de wsl-pc");
  expect(r).toHaveTextContent(/Sincronizando… 400 de 3800 issues lidas/);
});

test("prontidão mostra a listagem do sync antes da leitura", async () => {
  const st = status();
  st.readiness.progress = { done: 1500, total: 0, started_at: NOW - 60_000 };
  setup({ at: settingsAt("tracker"), projects: [{ ...hubProjectView(JIRA, ASSIGNMENTS), tracker_status: st }] });
  const g = await screen.findByRole("group", { name: "Repositório e rastreador" });
  const r = within(g).getByLabelText("Prontidão de wsl-pc");
  expect(r).toHaveTextContent(/Sincronizando… listando as issues, 1500 até agora/);
  expect(r).not.toHaveTextContent(/de 0 issues/);
});

test("projeto Jira sem ambiente de sync orienta a escolher", async () => {
  setup({
    at: settingsAt("tracker"),
    projects: [{ ...hubProjectView(jiraHubProject({ sync_env_id: null }), ASSIGNMENTS), tracker_status: null }],
  });
  const g = await screen.findByRole("group", { name: "Repositório e rastreador" });
  expect(g).toHaveTextContent(/Sem ambiente de sync: o hub não recebe o espelho do Jira/);
  expect(within(g).getByRole("button", { name: "Escolher ambiente de sync" })).toBeInTheDocument();
});

test("Visão geral mostra o último sync ou o erro de cada projeto Jira", async () => {
  setup({
    at: { name: "hub" },
    projects: [
      {
        ...hubProjectView(JIRA, ASSIGNMENTS),
        sync: { project_id: 7, last_attempt_at: NOW, last_ok_at: NOW - 3_600_000, last_error: "o Jira recusou a senha (401)" },
        tracker_status: null,
      },
    ],
  });
  const item = await screen.findByRole("listitem", { name: "Sync de app" });
  expect(item).toHaveTextContent(/sincronização com erro/);
  expect(item).toHaveTextContent(/pelo ambiente de sync wsl-pc: o Jira recusou a senha \(401\)/);
});

test("Configurações › Regras do projeto Jira: regras do agente do projeto no hub, com a versão", async () => {
  const { api } = setup({ at: settingsAt("rules") });
  api
    .onHub("hub.rules.get", (p: { owner: string; repo: string; epic_number: number }) => ({
      ...p,
      label: "app",
      current: {
        version: 4,
        content: "Commit e MR: [PROJ-NNNN] título, sem corpo.",
        bytes: 47,
        source: "importacao",
        proposal_id: null,
        author_env_id: null,
        author_env_name: null,
        session_id: null,
        note: null,
        created_at: NOW - 3_600_000,
      },
      pending: 0,
      max_bytes: 16384,
      warn_bytes: 12288,
    }))
    .onHub("hub.rules.proposals.list", { proposals: [], truncated: false });
  const g = await screen.findByRole("group", { name: "Regras do agente" });
  const card = await within(g).findByRole("region", { name: /Regras do agente do projeto · versão 4/ });
  expect(card).toHaveTextContent("importação de CLAUDE.md");
  expect(within(card).getByLabelText("Texto de regras do projeto")).toHaveTextContent("[PROJ-NNNN] título");
  expect(api.hubCallsTo("hub.rules.get")[0].params).toEqual({ owner: "grupo/projeto", repo: "app", epic_number: 0 });
});
