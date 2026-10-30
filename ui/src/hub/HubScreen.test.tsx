import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { EnvInfo } from "../protocol/gen";
import type { HubHealth, HubProjectView, HubProposals, HubStatus } from "../protocol/hub-gen";
import {
  HUB_BACKUP,
  HUB_CONFIG_UNLINKED,
  HUB_DASHBOARD,
  NO_HEALTH,
  NO_PROPOSALS,
  dashboard,
  healthSignal,
  hubAssignment,
  hubEnv,
  hubInfo,
  hubProject,
  hubProjectView,
  hubProposal,
  hubStatus,
  recoveryStatus,
  RC_AVAILABLE,
} from "../test/fixtures";
import type { HubSection } from "./sections";

const NOW = Date.now();

function info(): EnvInfo {
  return {
    protocol: "2.0",
    version: "0.1.0",
    commit: "",
    hostname: "wsl",
    worktrees_root: "/w",
    env_id: "daemon-wsl",
    env_name: "wsl-pc",
    hub_sync: null,
    hub_config: { ...HUB_CONFIG_UNLINKED, linked: true, config_rev: 7, pulled_at: NOW },
    update: null,
    remote_control: RC_AVAILABLE,
    docs: { root: null, reachable: false },
  };
}

const CT = hubProject();
const DEMO = hubProject({ id: 2, gh_repo: "demo", name: "demo" });

function setup(
  opts: {
    projects?: HubProjectView[];
    status?: HubStatus;
    connected?: boolean;
    section?: HubSection;
    proposals?: () => HubProposals;
    health?: HubHealth;
  } = {},
) {
  let projects = opts.projects ?? [
    hubProjectView(CT, [
      hubAssignment(),
      hubAssignment({
        env_id: "daemon-mac",
        env_name: "mac-mini",
        state: "error",
        error: "não é um clone de deep-ion-ai/ctower-agents",
        reported_path: null,
      }),
    ]),
    // Pendente com a mensagem de um relato antigo: a mensagem não aparece.
    hubProjectView(DEMO, [hubAssignment({ project_id: 2, gh_repo: "demo", state: "pending", error: "erro velho" })]),
  ];
  const api = new FakeDaemonApi([{ ...FakeDaemonApi.wslEnv(), name: "WSL" }])
    .on("dashboard.get", dashboard({ projects: [] }))
    .on("env.info", info())
    .withHub("ws://mac-mini:7879/ws", opts.connected ?? true)
    .onHub("hub.info", hubInfo({ backup: { ...HUB_BACKUP, newest_at: NOW - 30 * 60_000 } }))
    .onHub("hub.status", opts.status ?? hubStatus())
    .onHub("hub.projects.list", () => projects)
    .onHub("hub.envs.list", [
      hubEnv(),
      hubEnv({
        env_id: "daemon-mac",
        name: "mac-mini",
        last_seen_at: null,
        pending: 3,
        cleanup: { checked_at: NOW - 60_000, items: 2, bytes: 3 * 1024 ** 3 },
      }),
      hubEnv({ env_id: "daemon-note", name: "notebook", cleanup: { checked_at: NOW, items: 0, bytes: 0 } }),
    ])
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("hub.proposals.list", () => opts.proposals?.() ?? NO_PROPOSALS)
    .onHub("hub.health.get", opts.health ?? NO_HEALTH)
    .onHub("github.status", { source: "token", login: "hub-bot", error: null })
    .onHub("hub.backup.get", {
      remote: { kind: "none", encrypt: true, bucket: "", prefix: "", region: "us-east-1", endpoint: "", drive_folder_id: "" },
      status: HUB_BACKUP,
      credential_fields: [],
    })
    .onHub("hub.secrets.list", { status: { entries: 0, store_rev: 0, recovery_configured: true }, items: [], delivery: [] })
    .onHub("hub.recovery.status", recoveryStatus());
  render(<App api={api} initial={{ name: "hub", section: opts.section }} />);
  return {
    api,
    user: userEvent.setup(),
    setProjects: (p: HubProjectView[]) => (projects = p),
  };
}

const section = (name: RegExp) => screen.getByRole("region", { name });
/** Linha da seção no painel lateral. */
const sideRow = (name: string) => screen.getByRole("button", { name });

test("sem conexão com o hub: explica e leva às Configurações", async () => {
  const api = new FakeDaemonApi().on("dashboard.get", dashboard());
  render(<App api={api} initial={{ name: "hub" }} />);
  const user = userEvent.setup();
  expect(await screen.findByText(/A conexão com o hub não está configurada/)).toBeInTheDocument();
  // Estado vazio em qualquer seção; o painel lateral diz o estado.
  expect(screen.getByText("não configurado")).toBeInTheDocument();
  await user.click(sideRow("Secrets e variáveis"));
  expect(await screen.findByText(/A conexão com o hub não está configurada/)).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Configurar o hub" }));
  expect(screen.getByRole("button", { name: "Configurações" })).toHaveAttribute("aria-current", "page");
  expect(api.calls.filter((c) => c.env === "hub")).toHaveLength(0);
});

test("hub fora: aviso e as sessões seguem nos ambientes", async () => {
  const { api } = setup({ connected: false });
  expect(await screen.findByText(/Hub indisponível: cadastro, atribuições/)).toBeInTheDocument();
  expect(screen.getByText(/indisponível · hub inacessível/)).toBeInTheDocument();
  expect(api.calls.filter((c) => c.env === "hub")).toHaveLength(0);
});

test("informações, avisos de hub.status e token do GitHub do hub", async () => {
  const { api, user } = setup({
    status: hubStatus({
      warnings: [
        { kind: "backup_same_device", message: "backup_dir está no mesmo disco do hub.db" },
        { kind: "backup_stale", message: "último snapshot há 14 h" },
      ],
    }),
  });
  expect(await screen.findByText("Avisos · 2")).toBeInTheDocument();
  expect(screen.getByText("backup_dir está no mesmo disco do hub.db")).toBeInTheDocument();
  expect(screen.getByText("último snapshot há 14 h")).toBeInTheDocument();
  // Nome e conexão no topo do painel lateral; versão e snapshot no rodapé.
  expect(screen.getByText("Hub · mac-mini")).toBeInTheDocument();
  expect(screen.getByText("conectado")).toBeInTheDocument();
  const facts = screen.getByText("Revisão da configuração").closest("dl")!;
  expect(within(facts).getByText(/há 30 min em \/Volumes\/Backup\/cthub/)).toBeInTheDocument();
  expect(within(facts).getByText("Revisão da configuração")).toBeInTheDocument();
  // Contador no menu e na Visão geral do painel lateral.
  expect(screen.getByRole("button", { name: "Hub" })).toHaveAccessibleDescription("2 avisos");
  expect(sideRow("Visão geral")).toHaveAccessibleDescription("2 avisos");
  // Backup e snapshots ficam em Manutenção (#338): o Hub não tem a seção nem o grupo.
  const lateral = screen.getByRole("region", { name: "Painel lateral" });
  expect(within(lateral).queryByRole("button", { name: "Backup e snapshots" })).not.toBeInTheDocument();
  expect(within(lateral).queryByText("Manutenção")).not.toBeInTheDocument();
  // Cada aviso de backup leva a Manutenção › Backup e snapshots.
  const avisos = section(/^Avisos/);
  expect(within(avisos).getAllByRole("button", { name: "Configurar backup" })).toHaveLength(2);
  await user.click(within(avisos).getAllByRole("button", { name: "Configurar backup" })[0]);
  expect(screen.getByRole("tab", { name: "Manutenção · Backup e snapshots" })).toHaveAttribute("aria-selected", "true");
  expect(await screen.findByRole("heading", { name: "Backup do hub" })).toBeInTheDocument();
  await user.click(screen.getByRole("tab", { name: "Hub · Visão geral" }));
  // O token do GitHub é o do hub, não o de um ambiente.
  await user.click(sideRow("Token do GitHub"));
  expect(await screen.findByText("@hub-bot")).toBeInTheDocument();
  expect(api.hubCallsTo("github.status")).toHaveLength(1);
  expect(api.calls.filter((c) => c.method === "github.status" && c.env !== "hub")).toHaveLength(0);
});

test("atribuições com estado; a mensagem só aparece no estado de erro", async () => {
  const { user } = setup({ section: "projects" });
  const ct = await screen.findByRole("group", { name: "Projeto ctower-agents" });
  expect(within(ct).getByText("aplicada")).toBeInTheDocument();
  expect(within(ct).getByText("com erro: não é um clone de deep-ion-ai/ctower-agents")).toBeInTheDocument();
  expect(within(ct).getByText("/home/user/projetos/ctower-agents")).toBeInTheDocument();
  // Na lista, o estado de cada projeto; o detalhe é o do projeto escolhido.
  const list = screen.getByRole("region", { name: "Projetos cadastrados" });
  expect(within(list).getByRole("button", { name: "ctower-agents" })).toHaveAccessibleDescription(
    "deep-ion-ai/ctower-agents 1 atribuição com erro",
  );
  expect(within(list).getByRole("button", { name: "demo" })).toHaveAccessibleDescription(
    "deep-ion-ai/demo 1 atribuição pendente",
  );
  expect(screen.queryByRole("group", { name: "Projeto demo" })).not.toBeInTheDocument();
  await user.click(within(list).getByRole("button", { name: "demo" }));
  const demo = await screen.findByRole("group", { name: "Projeto demo" });
  expect(within(demo).getByText("pendente")).toBeInTheDocument();
  expect(within(list).getByRole("button", { name: "demo" })).toHaveAttribute("aria-current", "true");
  expect(screen.getByRole("navigation", { name: "Trilha" })).toHaveTextContent("deep-ion-ai/demo");
  expect(screen.queryByText(/erro velho/)).not.toBeInTheDocument();
});

test("cadastrar projeto pela URL: só os campos preenchidos vão ao hub", async () => {
  const { api, user, setProjects } = setup({ projects: [], section: "projects" });
  api.onHub("hub.projects.add", () => {
    setProjects([hubProjectView(CT)]);
    return { project: CT, rev: 8, reapplied: 4, restored: false };
  });
  await user.click(await screen.findByRole("button", { name: "Cadastrar projeto" }));
  const form = screen.getByRole("form", { name: "Novo projeto" });
  await user.type(within(form).getByLabelText("URL do GitHub"), "https://github.com/deep-ion-ai/ctower-agents");
  await user.type(within(form).getByLabelText(/^Protegidas/), "develop, main");
  const before = api.hubCallsTo("hub.projects.list").length;
  await user.click(within(form).getByRole("button", { name: "Cadastrar" }));
  expect(await screen.findByText(/deep-ion-ai\/ctower-agents cadastrado no hub\. 4 linha/)).toBeInTheDocument();
  expect(api.hubCallsTo("hub.projects.add")[0].params).toEqual({
    url: "https://github.com/deep-ion-ai/ctower-agents",
    protected_branches: ["develop", "main"],
  });
  expect(api.hubCallsTo("hub.projects.list").length).toBeGreaterThan(before);
  expect(await screen.findByRole("group", { name: "Projeto ctower-agents" })).toBeInTheDocument();
  expect(screen.queryByRole("form", { name: "Novo projeto" })).not.toBeInTheDocument();
});

test("erro do hub no cadastro fica na tela", async () => {
  const { api, user } = setup({ projects: [], section: "projects" });
  api.onHub("hub.projects.add", () => {
    throw new Error("branch base develop não existe em deep-ion-ai/x");
  });
  await user.click(await screen.findByRole("button", { name: "Cadastrar projeto" }));
  const form = screen.getByRole("form", { name: "Novo projeto" });
  await user.type(within(form).getByLabelText("URL do GitHub"), "https://github.com/deep-ion-ai/x");
  await user.click(within(form).getByRole("button", { name: "Cadastrar" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("branch base develop não existe em deep-ion-ai/x");
  expect(screen.getByRole("form", { name: "Novo projeto" })).toBeInTheDocument();
});

test("editar manda só o que mudou", async () => {
  const { api, user } = setup({ section: "projects" });
  api.onHub("hub.projects.update", { ...CT, base_branch: "main" });
  const ct = await screen.findByRole("group", { name: "Projeto ctower-agents" });
  await user.click(within(ct).getByRole("button", { name: "Editar ctower-agents" }));
  const form = screen.getByRole("form", { name: "Editar ctower-agents" });
  await user.clear(within(form).getByLabelText("Branch base"));
  await user.type(within(form).getByLabelText("Branch base"), "main");
  await user.click(within(form).getByRole("button", { name: "Salvar projeto" }));
  await waitFor(() => expect(api.hubCallsTo("hub.projects.update")).toHaveLength(1));
  expect(api.hubCallsTo("hub.projects.update")[0].params).toEqual({
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    base_branch: "main",
  });
});

test("editar os docs do projeto manda o objeto inteiro, com as pastas uma por linha (#444)", async () => {
  const docs = { enabled: false, project_dirs: ["analises"], epic_dirs: [] };
  const { api, user } = setup({ section: "projects", projects: [hubProjectView(hubProject({ docs }))] });
  api.onHub("hub.projects.update", hubProject({ docs }));
  const ct = await screen.findByRole("group", { name: "Projeto ctower-agents" });
  expect(within(ct).getByText("desligados")).toBeInTheDocument();
  await user.click(within(ct).getByRole("button", { name: "Editar ctower-agents" }));
  const form = screen.getByRole("form", { name: "Editar ctower-agents" });
  const area = within(form).getByLabelText("Pastas do projeto (em _projeto/)");
  expect(area).toHaveValue("analises");
  expect(area).toBeDisabled();
  await user.click(within(form).getByLabelText("Pasta de docs do projeto"));
  await user.type(area, "\n specs/telas/ \n\n");
  await user.type(within(form).getByLabelText(/Pastas de cada épico/), "prompts");
  await user.click(within(form).getByRole("button", { name: "Salvar projeto" }));
  await waitFor(() => expect(api.hubCallsTo("hub.projects.update")).toHaveLength(1));
  expect(api.hubCallsTo("hub.projects.update")[0].params).toEqual({
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    docs: { enabled: true, project_dirs: ["analises", "specs/telas"], epic_dirs: ["prompts"] },
  });
});

test("hub sem docs (anterior à #440): a edição avisa e não manda docs", async () => {
  const old = { ...hubProject(), docs: undefined as never };
  const { api, user } = setup({ section: "projects", projects: [hubProjectView(old)] });
  api.onHub("hub.projects.update", { ...old, name: "CT" });
  const ct = await screen.findByRole("group", { name: "Projeto ctower-agents" });
  expect(within(ct).queryByText("Docs")).not.toBeInTheDocument();
  await user.click(within(ct).getByRole("button", { name: "Editar ctower-agents" }));
  const form = screen.getByRole("form", { name: "Editar ctower-agents" });
  expect(within(form).getByRole("note")).toHaveTextContent(/não tem docs dos projetos/);
  expect(within(form).queryByLabelText("Pasta de docs do projeto")).not.toBeInTheDocument();
  await user.clear(within(form).getByLabelText("Nome"));
  await user.type(within(form).getByLabelText("Nome"), "CT");
  await user.click(within(form).getByRole("button", { name: "Salvar projeto" }));
  await waitFor(() => expect(api.hubCallsTo("hub.projects.update")).toHaveLength(1));
  expect(api.hubCallsTo("hub.projects.update")[0].params).toEqual({ owner: "deep-ion-ai", repo: "ctower-agents", name: "CT" });
});

test("excluir: impacto exato do hub e, com atribuições, um clique com confirm: true", async () => {
  const { api, user } = setup({ section: "projects" });
  api
    .onHub("hub.projects.impact", {
      project: CT,
      assignments: [hubAssignment(), hubAssignment({ env_id: "daemon-mac", env_name: "mac-mini" })],
      live_sessions: [{ id: "s1", env_id: "daemon-wsl", work_item_id: 50 }],
    })
    .onHub("hub.projects.delete", { rev: 9, snapshot: "/b/hub-20261018-1200-pre-delete.db", assignments: [], live_sessions: [] });
  const ct = await screen.findByRole("group", { name: "Projeto ctower-agents" });
  await user.click(within(ct).getByRole("button", { name: "Excluir projeto… ctower-agents" }));
  const box = screen.getByRole("group", { name: "Confirmar exclusão de ctower-agents" });
  expect(box).toHaveTextContent(/apaga o histórico do projeto no hub/);
  // Impacto (DeleteImpact) do hub antes de confirmar: atribuições, sessões e secrets.
  const impact = await within(box).findByRole("list", { name: "Impacto da exclusão" });
  expect(impact).toHaveTextContent("2 atribuições ativas: wsl-pc, mac-mini.");
  expect(impact).toHaveTextContent("1 sessão viva, que os ambientes não encerram.");
  expect(impact).not.toHaveTextContent("estimado");
  expect(api.hubCallsTo("hub.projects.impact").map((c) => c.params)).toEqual([{ owner: "deep-ion-ai", repo: "ctower-agents" }]);
  expect(api.hubCallsTo("hub.projects.delete")).toHaveLength(0);
  await user.click(within(box).getByRole("button", { name: "Excluir projeto" }));
  expect(await screen.findByText(/excluído do hub\. Snapshot anterior: \/b\/hub-20261018-1200-pre-delete\.db/)).toBeInTheDocument();
  expect(api.hubCallsTo("hub.projects.delete").map((c) => c.params)).toEqual([
    { owner: "deep-ion-ai", repo: "ctower-agents", confirm: true },
  ]);
});

test("excluir: sem impacto vai sem confirm; se o hub recusar, vale a mensagem dele", async () => {
  const { api, user } = setup({ section: "projects" });
  api.onHub("hub.projects.impact", { project: CT, assignments: [], live_sessions: [] });
  // Uma atribuição chegou entre a consulta e a exclusão.
  api.onHub("hub.projects.delete", (p: { confirm?: boolean }) => {
    if (!p.confirm) {
      throw new Error(
        "o projeto deep-ion-ai/ctower-agents está atribuído a 1 ambiente(s): wsl-pc; nenhuma sessão viva relatada. A exclusão apaga o histórico do projeto no hub; repita com confirm: true para excluir",
      );
    }
    return { rev: 9, snapshot: "/b/s.db", assignments: [], live_sessions: [] };
  });
  const ct = await screen.findByRole("group", { name: "Projeto ctower-agents" });
  await user.click(within(ct).getByRole("button", { name: "Excluir projeto… ctower-agents" }));
  const box = screen.getByRole("group", { name: "Confirmar exclusão de ctower-agents" });
  const impact = await within(box).findByRole("list", { name: "Impacto da exclusão" });
  expect(impact).toHaveTextContent("Nenhuma atribuição ativa.");
  expect(impact).toHaveTextContent("Nenhuma sessão viva relatada.");
  await user.click(within(box).getByRole("button", { name: "Excluir projeto" }));
  expect(await within(box).findByText(/atribuído a 1 ambiente\(s\): wsl-pc/)).toBeInTheDocument();
  await user.click(within(box).getByRole("button", { name: "Excluir mesmo assim" }));
  expect(await screen.findByText(/excluído do hub\. Snapshot anterior: \/b\/s\.db/)).toBeInTheDocument();
  expect(api.hubCallsTo("hub.projects.delete").map((c) => c.params)).toEqual([
    { owner: "deep-ion-ai", repo: "ctower-agents" },
    { owner: "deep-ion-ai", repo: "ctower-agents", confirm: true },
  ]);
});

test("atribuir a um ambiente sem atribuição, com caminho opcional", async () => {
  const { api, user } = setup({ section: "projects" });
  api.onHub("hub.assignments.add", {
    assignment: hubAssignment({ env_id: "daemon-note", env_name: "notebook", state: "pending" }),
    changed: true,
    reassigned: false,
  });
  const ct = await screen.findByRole("group", { name: "Projeto ctower-agents" });
  await user.click(within(ct).getByRole("button", { name: /Atribuir a ambiente/ }));
  const form = screen.getByRole("form", { name: "Atribuir ctower-agents" });
  // Só os ambientes que ainda não executam o projeto.
  const select = within(form).getByLabelText("Ambiente");
  expect(within(select).getAllByRole("option").map((o) => o.textContent)).toEqual(["notebook"]);
  await user.type(within(form).getByLabelText(/Caminho do clone/), "~/src/ct");
  await user.click(within(form).getByRole("button", { name: "Atribuir" }));
  expect(await screen.findByText(/atribuído a notebook; o ambiente aplica no próximo pull/)).toBeInTheDocument();
  expect(api.hubCallsTo("hub.assignments.add")[0].params).toEqual({
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    env: "daemon-note",
    repo_path: "~/src/ct",
  });
});

test("remover atribuição com sessões vivas pede a confirmação do hub", async () => {
  const { api, user } = setup({ section: "projects" });
  api.onHub("hub.assignments.remove", (p: { confirm?: boolean }) => {
    if (!p.confirm) {
      throw new Error(
        "deep-ion-ai/ctower-agents tem sessões vivas relatadas: s1 em wsl-pc (running). O ambiente wsl-pc não encerra as sessões: elas seguem até terminarem; repita com confirm: true para remover a atribuição",
      );
    }
    return { rev: 10, assignment: hubAssignment(), live_sessions: [{}] };
  });
  const ct = await screen.findByRole("group", { name: "Projeto ctower-agents" });
  await user.click(within(ct).getByRole("button", { name: "Remover atribuição a wsl-pc" }));
  const box = await screen.findByRole("group", { name: "Confirmar remoção de wsl-pc" });
  expect(box).toHaveTextContent(/não encerra as sessões/);
  await user.click(within(box).getByRole("button", { name: "Remover mesmo assim" }));
  expect(await screen.findByText(/1 sessão\(ões\) viva\(s\) seguem até terminarem/)).toBeInTheDocument();
  expect(api.hubCallsTo("hub.assignments.remove").map((c) => c.params)).toEqual([
    { owner: "deep-ion-ai", repo: "ctower-agents", env: "daemon-wsl" },
    { owner: "deep-ion-ai", repo: "ctower-agents", env: "daemon-wsl", confirm: true },
  ]);
});

test("ambientes do hub com as atribuições, as limpezas pendentes e se esta UI os conhece", async () => {
  setup({ section: "envs" });
  const envs = await screen.findByRole("region", { name: /^Ambientes · 3/ });
  const rows = within(envs).getAllByRole("row");
  const wsl = rows.find((r) => r.textContent?.includes("daemon-wsl"))!;
  expect(within(wsl).getByText("WSL · conectado")).toBeInTheDocument();
  expect(within(wsl).getByText("deep-ion-ai/ctower-agents")).toBeInTheDocument();
  expect(within(wsl).getByText("deep-ion-ai/demo")).toBeInTheDocument();
  expect(within(wsl).getByText("pendente")).toBeInTheDocument();
  // Limpezas pendentes que cada ambiente informou (#287).
  expect(within(wsl).getByText("não informado")).toBeInTheDocument();
  const mac = rows.find((r) => r.textContent?.includes("daemon-mac"))!;
  expect(within(mac).getByText("nunca")).toBeInTheDocument();
  expect(within(mac).getByText("não configurado")).toBeInTheDocument();
  expect(within(mac).getByText(/com erro: não é um clone/)).toBeInTheDocument();
  expect(within(mac).getByText("2 pendente(s) · 3,0 GB")).toBeInTheDocument();
  const note = rows.find((r) => r.textContent?.includes("daemon-note"))!;
  expect(within(note).getByText("nenhuma")).toBeInTheDocument();
  expect(within(note).getByText("nenhuma pendente")).toBeInTheDocument();
});

test("quarentena: descartar com confirmação e cadastrar a partir dela", async () => {
  const q = {
    project_key: "deep-ion-ai/perdido",
    env_id: "daemon-wsl",
    env_name: "wsl-pc",
    rows: 12,
    first_received_at: NOW - 3_600_000,
    last_received_at: NOW - 60_000,
  };
  const { api, user } = setup({
    status: hubStatus({ quarantine: [q, { ...q, env_id: "daemon-mac", env_name: "mac-mini", rows: 3 }] }),
    section: "quarantine",
  });
  api.onHub("hub.quarantine.discard", { discarded: 15 });
  let group = await screen.findByRole("group", { name: "Quarentena de deep-ion-ai/perdido" });
  expect(within(group).getByText("15 · wsl-pc 12, mac-mini 3")).toBeInTheDocument();
  // Um projeto em quarentena (com linhas de dois ambientes).
  expect(sideRow("Quarentena")).toHaveAccessibleDescription("1 projeto");

  // "Cadastrar projeto" abre a seção Projetos com a URL preenchida.
  await user.click(within(group).getByRole("button", { name: "Cadastrar projeto" }));
  const form = await screen.findByRole("form", { name: "Novo projeto" });
  expect(within(form).getByLabelText("URL do GitHub")).toHaveValue("https://github.com/deep-ion-ai/perdido");
  expect(screen.getByRole("tab", { name: "Hub · Projetos e atribuições" })).toHaveAttribute("aria-selected", "true");
  await user.click(within(form).getByRole("button", { name: "Cancelar" }));
  expect(screen.queryByRole("form", { name: "Novo projeto" })).not.toBeInTheDocument();

  // A aba da Quarentena continua aberta.
  await user.click(screen.getByRole("tab", { name: "Hub · Quarentena" }));
  group = await screen.findByRole("group", { name: "Quarentena de deep-ion-ai/perdido" });

  await user.click(within(group).getByRole("button", { name: "Descartar" }));
  const box = within(group).getByRole("group", { name: "Confirmar descarte de deep-ion-ai/perdido" });
  expect(api.hubCallsTo("hub.quarantine.discard")).toHaveLength(0);
  await user.click(within(box).getByRole("button", { name: "Descartar linhas" }));
  expect(await screen.findByText("15 linha(s) de deep-ion-ai/perdido descartada(s).")).toBeInTheDocument();
  expect(api.hubCallsTo("hub.quarantine.discard")[0].params).toEqual({
    owner: "deep-ion-ai",
    repo: "perdido",
    confirm: true,
  });
});

test("push do hub relê projetos e avisos", async () => {
  const { api } = setup({ section: "quarantine" });
  await screen.findByRole("region", { name: /^Quarentena/ });
  await waitFor(() => expect(api.hubCallsTo("hub.projects.list").length).toBeGreaterThan(0));
  const before = api.hubCallsTo("hub.status").length;
  act(() => api.hubPush("hub_sync_status", { env_id: "daemon-wsl" }));
  await waitFor(() => expect(api.hubCallsTo("hub.status").length).toBeGreaterThan(before));
  expect(section(/^Quarentena/)).toHaveTextContent("Nada em quarentena.");
});

test("seções: cada uma abre como aba com trilha; a Visão geral é a aba da tela", async () => {
  const { user } = setup();
  expect(await screen.findByRole("heading", { level: 1, name: "Visão geral" })).toBeInTheDocument();
  expect(screen.getByRole("tab", { name: "Hub · Visão geral" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getByRole("navigation", { name: "Trilha" })).toHaveTextContent("HubOperaçãoVisão geral");
  expect(sideRow("Visão geral")).toHaveAttribute("aria-current", "true");

  await user.click(sideRow("Secrets e variáveis"));
  expect(await screen.findByRole("region", { name: "Secrets e variáveis" })).toBeInTheDocument();
  expect(screen.getByRole("navigation", { name: "Trilha" })).toHaveTextContent("HubSegurançaSecrets e variáveis");
  expect(sideRow("Secrets e variáveis")).toHaveAttribute("aria-current", "true");
  // As duas abas ficam abertas; vindo de outra tela, a barra de atividades volta à Visão geral.
  expect(screen.getByRole("tab", { name: "Hub · Secrets e variáveis" })).toHaveAttribute("aria-selected", "true");
  const telas = screen.getByRole("navigation", { name: "Telas" });
  await user.click(within(telas).getByRole("button", { name: "Configurações" }));
  await user.click(within(telas).getByRole("button", { name: "Hub" }));
  expect(screen.getByRole("tab", { name: "Hub · Visão geral" })).toHaveAttribute("aria-selected", "true");
  // Clicar de novo na seção foca a aba que já existe.
  await user.click(sideRow("Secrets e variáveis"));
  expect(screen.getAllByRole("tab", { name: "Hub · Secrets e variáveis" })).toHaveLength(1);
});

test("contadores do painel lateral batem com avisos, propostas, sinais e quarentena", async () => {
  const q = {
    project_key: "deep-ion-ai/perdido",
    env_id: "daemon-wsl",
    env_name: "wsl-pc",
    rows: 12,
    first_received_at: NOW - 3_600_000,
    last_received_at: NOW - 60_000,
  };
  setup({
    status: hubStatus({
      warnings: [
        { kind: "push_quarantine", message: "deep-ion-ai/perdido em quarentena" },
        { kind: "push_quarantine", message: "deep-ion-ai/outro em quarentena" },
        { kind: "recovery_not_configured", message: "chave de recuperação não configurada" },
      ],
      quarantine: [q, { ...q, env_id: "daemon-mac" }, { ...q, project_key: "deep-ion-ai/outro" }],
    }),
    proposals: () => ({ proposals: [hubProposal(), hubProposal({ id: 2, number: 12 })], truncated: false }),
    health: {
      generated_at: NOW,
      signals: [
        healthSignal(),
        healthSignal({ kind: "parado", severity: "aviso", number: 9 }),
        healthSignal({ kind: "nao_triado", severity: "info", number: null }),
      ],
      counts: { critico: 1, aviso: 1, info: 1 },
    },
  });
  await waitFor(() => expect(sideRow("Propostas do gestor")).toHaveAccessibleDescription("2 pendentes"));
  expect(sideRow("Visão geral")).toHaveAccessibleDescription("3 avisos");
  await waitFor(() => expect(sideRow("Saúde dos projetos")).toHaveAccessibleDescription("2 sinais"));
  expect(sideRow("Quarentena")).toHaveAccessibleDescription("2 projetos");
  expect(sideRow("Chave de recuperação")).toHaveAccessibleDescription("pede atenção");
  expect(sideRow("Projetos e atribuições")).toHaveAccessibleDescription("2 projetos");
  expect(sideRow("Ambientes")).toHaveAccessibleDescription("3 ambientes");
  expect(sideRow("Token do GitHub")).toHaveAccessibleDescription("");
  // Os mesmos números nos títulos da Visão geral.
  expect(section(/^Avisos · 3/)).toBeInTheDocument();
  expect(section(/^Propostas do gestor · 2/)).toBeInTheDocument();
  expect(section(/^Saúde dos projetos · 2 sinais/)).toBeInTheDocument();
});

test("Visão geral: aprovar e recusar ali mesmo, sinais em tabela, ambientes e gestor no cabeçalho", async () => {
  let pending = [hubProposal(), hubProposal({ id: 2, kind: "status", number: 12, current: { status: "backlog" }, proposed: { status: "pausado" } })];
  const { api, user } = setup({
    proposals: () => ({ proposals: pending, truncated: false }),
    health: { generated_at: NOW, signals: [healthSignal()], counts: { critico: 1, aviso: 0, info: 0 } },
  });
  api.onHub("hub.proposals.decide", (p: { id: number; approve: boolean }) => {
    const x = pending.find((y) => y.id === p.id)!;
    pending = pending.filter((y) => y.id !== p.id);
    return { ...x, state: p.approve ? "aprovada" : "rejeitada", decided_at: NOW };
  });
  expect(await screen.findByRole("button", { name: "Abrir sessão do gestor" })).toBeInTheDocument();

  const props = section(/^Propostas do gestor/);
  const first = await within(props).findByRole("listitem", { name: "Proposta #1" });
  expect(first).toHaveTextContent("#215 prioridade e ordem → P0");
  expect(first).toHaveTextContent("bloqueia o resto do épico");
  expect(within(props).getByRole("listitem", { name: "Proposta #2" })).toHaveTextContent("→ pausado");
  await user.click(within(first).getByRole("button", { name: "Aprovar a proposta #1" }));
  expect(await within(props).findByText("Proposta #1 aprovada.")).toBeInTheDocument();
  await user.click(within(props).getByRole("button", { name: "Recusar a proposta #2" }));
  expect(await within(props).findByText("Proposta #2 rejeitada.")).toBeInTheDocument();
  expect(api.hubCallsTo("hub.proposals.decide").map((c) => c.params)).toEqual([
    { id: 1, approve: true },
    { id: 2, approve: false },
  ]);
  expect(await within(props).findByText("Nenhuma proposta pendente.")).toBeInTheDocument();
  await waitFor(() => expect(sideRow("Propostas do gestor")).toHaveAccessibleDescription(""));

  const saude = section(/^Saúde dos projetos/);
  const row = await within(saude).findByRole("row", { name: /bloqueado/ });
  expect(row).toHaveTextContent("#176 atribuições");
  expect(row).toHaveTextContent("8 dias · 7 dias");

  const envs = section(/^Ambientes · 3/);
  expect(within(envs).getByText("mac-mini")).toBeInTheDocument();
  expect(within(envs).getByText(/último envio nunca · 3 pendências/)).toBeInTheDocument();
  await user.click(within(envs).getByRole("button", { name: "Gerenciar" }));
  expect(await screen.findByRole("tab", { name: "Hub · Ambientes" })).toHaveAttribute("aria-selected", "true");

  // Abrir o item do sinal leva à tela do projeto.
  await user.click(screen.getByRole("tab", { name: "Hub · Visão geral" }));
  const again = await screen.findByRole("region", { name: /^Saúde dos projetos/ });
  await user.click(within(again).getByRole("button", { name: "Abrir deep-ion-ai/ctower-agents#176" }));
  // O item abre em aba própria, com a árvore do projeto no painel lateral.
  expect(await screen.findByRole("complementary", { name: "Árvore do projeto" })).toBeInTheDocument();
  expect(screen.getByRole("navigation", { name: "Telas" }).querySelector('[aria-current="page"]')).toHaveAccessibleName(
    "Projetos",
  );
});

test("lista: filtro, reordenar por arrasto e por Alt+↑/↓", async () => {
  const { api, user } = setup({ section: "projects" });
  api.onHub("hub.projects.move", { ...CT, rank: 2 });
  const list = await screen.findByRole("region", { name: "Projetos cadastrados" });
  const items = () => within(list).getAllByRole("listitem");
  // A região aparece antes do hub.projects.list responder.
  await waitFor(() =>
    expect(items().map((li) => within(li).getByRole("button").getAttribute("aria-label"))).toEqual(["ctower-agents", "demo"]),
  );
  expect(within(items()[1]).getByText("#2")).toBeInTheDocument();

  // Filtro por nome ou owner/repo.
  await user.type(within(list).getByLabelText("Filtrar projetos"), "dem");
  expect(items()).toHaveLength(1);
  await user.clear(within(list).getByLabelText("Filtrar projetos"));
  await user.type(within(list).getByLabelText("Filtrar projetos"), "nada");
  expect(within(list).getByText(/Nenhum projeto com/)).toBeInTheDocument();
  await user.clear(within(list).getByLabelText("Filtrar projetos"));

  // Arrastar ctower-agents para a metade de baixo de demo: depois dele.
  const [ct, demo] = items();
  fireEvent.dragStart(ct);
  fireEvent.dragOver(demo, { clientY: 10 });
  expect(demo).toHaveAttribute("data-drop", "after");
  fireEvent.drop(demo, { clientY: 10 });
  await waitFor(() => expect(api.hubCallsTo("hub.projects.move")).toHaveLength(1));
  expect(api.hubCallsTo("hub.projects.move")[0].params).toEqual({
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    after: { owner: "deep-ion-ai", repo: "demo" },
  });
  expect(await screen.findByText("ctower-agents desceu na ordem dos projetos.")).toBeInTheDocument();

  // Soltar sobre si mesmo não muda nada.
  fireEvent.dragStart(items()[0]);
  fireEvent.drop(items()[0]);
  // Teclado: Alt+↑ em demo sobe para antes de ctower-agents.
  within(items()[1]).getByRole("button").focus();
  await user.keyboard("{Alt>}{ArrowUp}{/Alt}");
  await waitFor(() => expect(api.hubCallsTo("hub.projects.move")).toHaveLength(2));
  expect(api.hubCallsTo("hub.projects.move")[1].params).toEqual({
    owner: "deep-ion-ai",
    repo: "demo",
    before: { owner: "deep-ion-ai", repo: "ctower-agents" },
  });
});

test("detalhe: repositório, sessões vivas, secrets com entrega e atalhos", async () => {
  const { api, user } = setup();
  api
    .onHub("hub.dashboard.get", {
      ...HUB_DASHBOARD,
      projects: [{ project: CT, work_items: [{ id: 50 } as never] }],
      live_sessions: [
        { id: "s1", env_id: "daemon-wsl", work_item_id: 50 },
        { id: "s2", env_id: "daemon-wsl", work_item_id: 50 },
        { id: "s3", env_id: "daemon-mac", work_item_id: 99 },
      ] as never,
    })
    .onHub("hub.secrets.list", {
      status: { entries: 2, store_rev: 12, recovery_configured: true },
      items: [
        { scope: { project: "deep-ion-ai/ctower-agents" }, name: "GH_RELEASE_TOKEN", kind: "secret", rev: 12, updated_at: NOW },
        { scope: { project: "deep-ion-ai/ctower-agents" }, name: "RUST_LOG", kind: "variable", rev: 8, updated_at: NOW, value: "info" },
        { scope: "global", name: "GLOBAL_X", kind: "secret", rev: 3, updated_at: NOW },
      ],
      delivery: [
        { env_id: "daemon-wsl", env_name: "wsl-pc", state: "delivered", applied_rev: 12, required_rev: 12, projects: ["deep-ion-ai/ctower-agents"] },
        { env_id: "daemon-mac", env_name: "mac-mini", state: "pending", applied_rev: 9, required_rev: 12, projects: ["deep-ion-ai/ctower-agents"] },
        { env_id: "daemon-note", env_name: "notebook", state: "no_key", applied_rev: null, required_rev: 0, projects: [] },
      ],
      rotate_pending: null,
    });
  await user.click(await screen.findByRole("button", { name: "Projetos e atribuições" }));
  const ct = await screen.findByRole("group", { name: "Projeto ctower-agents" });
  expect(within(ct).getByText("ainda não sincronizado")).toBeInTheDocument();
  expect(within(ct).getByText("#1 de 2")).toBeInTheDocument();
  // Sessões vivas por ambiente, só as do projeto.
  const atr = within(ct).getByRole("table", { name: "Atribuições de ctower-agents" });
  await waitFor(() => expect(within(atr).getByRole("row", { name: /wsl-pc/ })).toHaveTextContent("2 ativas"));
  expect(within(atr).getByRole("row", { name: /mac-mini/ })).toHaveTextContent("nenhuma");
  // Secrets do projeto (não os globais), com a entrega por ambiente.
  const sec = await within(ct).findByRole("table", { name: "Secrets e variáveis de ctower-agents" });
  expect(within(sec).getByRole("row", { name: /GH_RELEASE_TOKEN/ })).toHaveTextContent("secret1/2 ambientes · rev 12");
  expect(within(sec).getByRole("row", { name: /RUST_LOG/ })).toHaveTextContent("variável2/2 ambientes · rev 8");
  expect(within(ct).queryByText("GLOBAL_X")).not.toBeInTheDocument();
  const entrega = within(ct).getByRole("list", { name: "Entrega dos secrets de ctower-agents por ambiente" });
  expect(entrega).toHaveTextContent("wsl-pcentregue");
  expect(entrega).toHaveTextContent("mac-minipendente");
  // A exclusão mostra as sessões e os secrets atingidos; sem hub.projects.impact
  // (hub antigo), o impacto é o que a UI calcula.
  await user.click(within(ct).getByRole("button", { name: "Excluir projeto… ctower-agents" }));
  const impact = await within(ct).findByRole("list", { name: "Impacto da exclusão" });
  expect(impact).toHaveTextContent("Impacto estimado pela UI");
  expect(impact).toHaveTextContent("2 sessões vivas");
  expect(impact).toHaveTextContent("2 secrets e variáveis do projeto");
  await user.click(within(ct).getByRole("button", { name: "Cancelar" }));
  // Fixada, a aba dos Projetos não é substituída pela seção seguinte.
  await user.click(screen.getByRole("button", { name: "Fixar aba" }));
  // Atalho para a seção de Secrets.
  await user.click(within(ct).getByRole("button", { name: "Abrir Secrets e variáveis" }));
  expect(await screen.findByRole("tab", { name: "Hub · Secrets e variáveis" })).toHaveAttribute("aria-selected", "true");
  // De volta: o projeto escolhido continua, e "Abrir árvore" leva à tela do projeto.
  await user.click(screen.getByRole("tab", { name: "Hub · Projetos e atribuições" }));
  const again = await screen.findByRole("group", { name: "Projeto ctower-agents" });
  await user.click(within(again).getByRole("button", { name: "Abrir árvore" }));
  expect(await screen.findByRole("heading", { level: 1, name: /ctower-agents/ })).toBeInTheDocument();
});

test("cadastrar seleciona o projeto novo", async () => {
  const { api, user, setProjects } = setup({ section: "projects" });
  const NEW = hubProject({ id: 3, gh_repo: "novo", name: "novo" });
  api.onHub("hub.projects.add", () => {
    setProjects([hubProjectView(CT), hubProjectView(DEMO), hubProjectView(NEW)]);
    return { project: NEW, rev: 8, reapplied: 0, restored: false };
  });
  await user.click(await screen.findByRole("button", { name: "Cadastrar projeto" }));
  const form = screen.getByRole("form", { name: "Novo projeto" });
  await user.type(within(form).getByLabelText("URL do GitHub"), "https://github.com/deep-ion-ai/novo");
  await user.click(within(form).getByRole("button", { name: "Cadastrar" }));
  expect(await screen.findByRole("group", { name: "Projeto novo" })).toBeInTheDocument();
  expect(screen.getByText("deep-ion-ai/novo cadastrado no hub.")).toBeInTheDocument();
});
