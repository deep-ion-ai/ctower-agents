// UI neutra de provedor (#352, spec do Jira e GitLab §11): com um projeto
// Jira + GitLab, cada ponto que mostrava `#n`, "GitHub", "PR" ou montava link
// do GitHub usa a chave do Jira, o link `/browse/`, "MR" e `!iid`; com um
// projeto GitHub, tudo fica como antes.
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "./App";
import { FakeDaemonApi } from "./api/fake";
import type { CleanupStatus, Decision, IssueRow, PrRow, ProjectTree, RecoveryItem, Review, TrackerReadiness, TreeNode } from "./protocol/gen";
import type { HubDashboard, HubHistory, HubProjectView, HubTree, HubTreeNode, HubWorkItem } from "./protocol/hub-gen";
import {
  HUB_CONFIG_UNLINKED,
  NO_HEALTH,
  NO_PROPOSALS,
  dashboard,
  healthSignal,
  hubAssignment,
  hubEnv,
  hubInfo,
  hubProjectView,
  hubProposal,
  hubStatus,
  jiraHubProject,
  jiraProject,
  project,
  recoveryStatus,
  session,
  sync,
  workItem,
} from "./test/fixtures";

const NOW = Date.now();
const H = 3_600_000;

/**
 * Nada de GitHub, github.com nem PR no texto e nos atributos que o leitor de
 * tela ou o mouse mostram. O prompt da modal de início vem do template do
 * Jira (#351) e entra na conferência pelo valor do textarea.
 */
const FORBIDDEN = /GitHub|github\.com|\bPRs?\b/;
function expectNeutral(root: Element = document.body) {
  const el = root.cloneNode(true) as Element;
  for (const t of root.querySelectorAll("textarea")) expect(t.value).not.toMatch(FORBIDDEN);
  const hit = (el.textContent ?? "").match(new RegExp(`.{0,60}(${FORBIDDEN.source}).{0,60}`));
  expect(hit?.[0] ?? null).toBeNull();
  for (const n of el.querySelectorAll("*")) {
    for (const a of ["aria-label", "title", "href", "placeholder", "aria-description"]) {
      const v = n.getAttribute(a);
      if (v) expect(`${a}="${v}"`).not.toMatch(FORBIDDEN);
    }
  }
}

// Projeto Jira + GitLab do exemplo no ambiente (id 7): épico PROJ-5400 com a sub-issue PROJ-5450.
const P = jiraProject();
const BROWSE = (n: number) => `https://jira.example.com/browse/PROJ-${n}`;
const MR_URL = "https://gitlab.example.com/grupo/projeto/app/-/merge_requests/7";

const EPIC = workItem({ gh_number: 5400, project_id: 7, kind: "epic", title: "Épico do plugin", branch: "wip/PROJ-5400" });
const WI = workItem({
  gh_number: 5450,
  project_id: 7,
  status: "em_revisao",
  title: "Portar o plugin",
  parent_number: 5400,
  branch: "PROJ-5450",
  worktree_path: "/home/user/worktrees/app/PROJ-5450",
  pending_decisions: 1,
});
const LIVE = session({ id: "54505450-0000-0000-0000-000000005450", work_item_id: WI.id });

const mr = (p: Partial<PrRow> = {}): PrRow => ({
  number: 7,
  work_item_id: WI.id,
  head: "PROJ-5450",
  base: "develop",
  state: "open",
  url: MR_URL,
  updated_at: 0,
  head_sha: null,
  ...p,
});

function issue(n: number, title: string, p: Partial<IssueRow> = {}): IssueRow {
  return { gh_number: n, title, state: "open", labels: [], parent_number: null, has_sub_issues: false, url: BROWSE(n), updated_at: 0, ...p };
}

const summary = (pr: PrRow | null) => ({ pr, decisions: { total: 0, pending_autonomous: 0 }, last_activity_at: NOW - H, live_envs: [] });

function tree(pr: PrRow | null): ProjectTree {
  const sub: TreeNode = { issue: issue(5450, "Portar o plugin", { parent_number: 5400 }), work_item: WI, summary: summary(pr), children: [] };
  // Sub-issue ainda sem work item: a branch da modal sai do padrão do projeto.
  const todo: TreeNode = { issue: issue(5460, "Ajustar o build", { parent_number: 5400 }), work_item: null, summary: summary(null), children: [] };
  return {
    project: P,
    sync: sync({ project_id: 7, last_ok_at: NOW }),
    roots: [{ issue: issue(5400, "Épico do plugin", { has_sub_issues: true }), work_item: EPIC, summary: summary(null), children: [sub, todo] }],
    untracked: [],
  };
}

function decision(id: number, title: string): Decision {
  return {
    id,
    uid: `d-${id}`,
    work_item_id: WI.id,
    session_id: null,
    origin: "autonoma",
    title,
    context: "O exemplo não usa co-author.",
    decision: "Commit [KEY] título.",
    consequences: null,
    reviewed_at: null,
    reviewed_in_pr: null,
    created_at: NOW,
  };
}

/** Prontidão do ambiente para o projeto do exemplo (`env.info.trackers`, #349); `p` muda o que falta. */
function readiness(p: { jira?: Partial<TrackerReadiness["jira"]>; gitlab?: Partial<TrackerReadiness["gitlab"]> } = {}): TrackerReadiness {
  return {
    owner: "grupo/projeto",
    repo: "app",
    checked_at: NOW,
    jira: { cli: "1.7.0", config_present: true, server_matches: true, credential_ok: true, me_ok: true, server_version: "8.5.2", error: null, custom_fields: [], ...p.jira },
    gitlab: { cli: "1.124.0", credential_ok: true, api_ok: true, error: null, ...p.gitlab },
  };
}

const ENV_INFO = { protocol: "2.3", version: "0.1.0", hostname: "ct", worktrees_root: "/home/user/worktrees", trackers: [readiness()] };

/** Ambiente com o projeto Jira, uma sessão viva no PROJ-5450 e o MR !7 mesclado na base (issue aberta). */
function envApi(opts: { pr?: PrRow | null; review?: Review; syncError?: boolean; trackers?: TrackerReadiness[] | null } = {}) {
  const pr = opts.pr === undefined ? mr({ state: "merged" }) : opts.pr;
  return new FakeDaemonApi()
    .on(
      "dashboard.get",
      dashboard(
        {
          projects: [{ project: P, work_items: [EPIC, WI] }],
          live_sessions: [LIVE],
          sync: [sync({ project_id: 7, ...(opts.syncError ? { last_ok_at: null, last_error: "jira me falhou" } : { last_ok_at: NOW }) })],
        },
      ),
    )
    .on("session.peek", { data: "" })
    .on("tree.get", tree(pr))
    .on("env.info", opts.trackers === undefined ? ENV_INFO : { ...ENV_INFO, trackers: opts.trackers ?? undefined })
    .on("sync.now", [])
    .on("review.get", opts.review ?? { work_item: WI, prs: pr ? [pr] : [], decisions: [] })
    .on("review.mark_reviewed", { marked: 1 })
    .on("github.pr.comment", () => {
      throw new Error("não pode comentar pelo GitHub num projeto GitLab");
    });
}

const side = () => screen.getByRole("complementary", { name: "Árvore do projeto" });
/** Espera a linha do item na árvore do painel lateral. */
const treeRow = async (name: RegExp) =>
  within(await screen.findByRole("complementary", { name: "Árvore do projeto" })).findByRole("button", { name });

describe("projeto Jira + GitLab", () => {
  test("árvore do Projeto: chave do Jira, MR, ícone do provedor e links do Jira", async () => {
    const api = envApi();
    render(<App api={api} initial={{ name: "project", env: "wsl", projectId: P.id }} />);
    const user = userEvent.setup();
    await treeRow(/^PROJ-5450 Portar o plugin/);
    // Ícone do provedor na árvore e no cabeçalho do projeto.
    expect(within(side()).getByRole("img", { name: "Jira + GitLab" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "app" }).parentElement!).toContainElement(
      screen.getAllByRole("img", { name: "Jira + GitLab" })[1],
    );
    const epico = within(side()).getByRole("button", { name: /^Épico PROJ-5400 Épico do plugin/ });
    expect(epico).toBeInTheDocument();
    const row = within(side()).getByRole("button", { name: /^PROJ-5450 Portar o plugin, em revisão.*MR !7 mesclado, MR mesclado, issue aberta/ });
    expect(row).toHaveTextContent("PROJ-5450");
    expect(within(side()).getByRole("button", { name: "Iniciar sessão para PROJ-5450" })).toBeInTheDocument();
    expect(within(side()).getByTitle("MR mesclado, issue aberta: o MR !7 não fechou a issue (faltou Closes PROJ-5450)")).toBeInTheDocument();
    expectNeutral();

    // Menu de contexto: abrir e fechar a issue no Jira.
    fireEvent.contextMenu(row.closest("li")!);
    const menu = screen.getByRole("menu", { name: "Ações do item" });
    expect(within(menu).getByRole("menuitem", { name: "Abrir no Jira" })).toBeInTheDocument();
    expectNeutral(menu);
    await user.click(within(menu).getByRole("menuitem", { name: "Fechar a issue no Jira (MR mesclado)" }));
    expect(api.opened).toEqual([BROWSE(5450)]);

    // Quadro: o card leva a chave e o MR.
    await user.click(within(side()).getByRole("button", { name: "Quadro" }));
    expect((await screen.findAllByText("MR !7 mesclado")).length).toBeGreaterThan(0);
    expect(screen.getAllByText("épico PROJ-5400").length).toBeGreaterThan(0);
    expectNeutral();

    // Detalhe do item, em aba própria com a chave no título.
    await user.click(row);
    const panel = await screen.findByRole("region", { name: "Detalhe do item" });
    expect(within(panel).getByRole("heading", { name: "PROJ-5450 Portar o plugin" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "PROJ-5450 Portar o plugin" })).toBeInTheDocument();
    expect(within(panel).getByRole("list", { name: "Trilha" })).toHaveTextContent("Épico PROJ-5400");
    expect(within(panel).getByText("MR mesclado, issue aberta")).toHaveAttribute(
      "title",
      "O MR !7 foi mesclado em develop sem fechar a issue (faltou Closes PROJ-5450)",
    );
    await user.click(within(panel).getByRole("button", { name: "Fechar a issue no Jira ↗" }));
    await user.click(within(panel).getByRole("button", { name: "Abrir MR !7 no GitLab ↗" }));
    expect(api.opened.slice(1)).toEqual([BROWSE(5450), MR_URL]);
    expectNeutral();

    // O épico lista as sub-issues com a chave e o MR.
    await user.click(within(panel).getByRole("button", { name: "Épico PROJ-5400" }));
    const epicPanel = await screen.findByRole("region", { name: "Detalhe do item" });
    const subs = await within(epicPanel).findByRole("region", { name: "Sub-issues" });
    expect(within(subs).getByRole("columnheader", { name: "MR" })).toBeInTheDocument();
    expect(within(subs).getByRole("button", { name: "Abrir MR !7 de PROJ-5450 no GitLab (mesclado)" })).toHaveTextContent("!7");
    expectNeutral();
  });

  test("busca na árvore pela chave do Jira", async () => {
    const api = envApi();
    render(<App api={api} initial={{ name: "project", env: "wsl", projectId: P.id }} />);
    const user = userEvent.setup();
    await treeRow(/^PROJ-5450 Portar o plugin/);
    await user.type(within(side()).getByRole("searchbox"), "PROJ-5450");
    expect(within(side()).getByRole("button", { name: /^PROJ-5450 Portar o plugin/ })).toBeInTheDocument();
  });

  test("modal de início: chave no título, MR no escopo do épico e a branch pelo padrão do projeto", async () => {
    const api = envApi({ pr: null });
    render(<App api={api} initial={{ name: "project", env: "wsl", projectId: P.id }} />);
    const user = userEvent.setup();
    await treeRow(/^PROJ-5450 Portar o plugin/);
    await user.click(within(side()).getByRole("button", { name: "Iniciar sessão para PROJ-5400" }));
    const modal = screen.getByRole("dialog");
    expect(modal).toHaveAccessibleName("PROJ-5400 Épico do plugin");
    expect(within(modal).getByText(/cada uma em MR para/, { selector: "p" })).toBeInTheDocument();
    await user.click(within(modal).getByRole("button", { name: "Iniciar só a sub-issue PROJ-5460 Ajustar o build" }));
    expect(modal).toHaveAccessibleName("PROJ-5460 Ajustar o build");
    // Padrão do exemplo (`{key}`): a branch é a chave.
    expect(within(modal).getByText("PROJ-5460")).toBeInTheDocument();
    expect(within(modal).getByRole("button", { name: /Voltar para PROJ-5400/ })).toBeInTheDocument();
    // Prompt do template de issue do Jira (#351), com o jira-cli e o glab
    // pelo comando de credencial do cadastro.
    const prompt = within(modal).getByLabelText(/Prompt inicial · template de issue do Jira/) as HTMLTextAreaElement;
    expect(prompt.value).toContain("Trabalhe na issue PROJ-5460 do Jira (https://jira.example.com/browse/PROJ-5460): Ajustar o build.");
    expect(prompt.value).toContain('`JIRA_API_TOKEN="$(pass show jira | head -1)" jira issue view PROJ-5460 --comments 20 --plain`');
    expect(prompt.value).toContain(
      '`GITLAB_HOST=gitlab.example.com GITLAB_TOKEN="$(pass show gitlab | head -1)" glab mr create -R grupo/projeto/app -s PROJ-5460 -b develop',
    );
    expect(prompt.value).toContain("issue comment add PROJ-5460 'MR aberto: <url do MR>' --no-input");
    // Tudo pronto no ambiente: sem aviso de prontidão.
    expect(within(modal).queryByRole("status", { name: "Prontidão do Jira e do GitLab" })).not.toBeInTheDocument();
    expectNeutral(modal);
    await user.click(within(modal).getByRole("button", { name: /Voltar para PROJ-5400/ }));
    expect((within(modal).getByLabelText(/template de épico do Jira/) as HTMLTextAreaElement).value).toContain(
      "issue view PROJ-5400 --comments 20 --plain` e liste as issues dele com",
    );
  });

  test("modal de início avisa o que falta no jira-cli e no glab do ambiente, sem bloquear", async () => {
    const api = envApi({ pr: null, trackers: [readiness({ jira: { credential_ok: false, me_ok: false, error: "pass: gpg-agent sem a chave" }, gitlab: { cli: null } })] });
    api.on("session.start", session({ id: "54605460-0000-0000-0000-000000005460", work_item_id: 99 }));
    render(<App api={api} initial={{ name: "project", env: "wsl", projectId: P.id }} />);
    const user = userEvent.setup();
    await treeRow(/^PROJ-5450 Portar o plugin/);
    await user.click(within(side()).getByRole("button", { name: "Iniciar sessão para PROJ-5450" }));
    const modal = screen.getByRole("dialog");
    const note = await within(modal).findByRole("status", { name: "Prontidão do Jira e do GitLab" });
    expect(note).toHaveTextContent("O Jira ou o GitLab não estão prontos em");
    expect(note).toHaveTextContent('comando de credencial do Jira ok: falta (rode "pass show jira | head -1" num terminal do ambiente');
    expect(note).toHaveTextContent("jira me ok: falta");
    expect(note).toHaveTextContent("glab instalado: falta");
    expect(note).toHaveTextContent("pass: gpg-agent sem a chave");
    expectNeutral(modal);
    // Só avisa: a sessão inicia.
    await user.click(within(modal).getByRole("button", { name: /^Iniciar sessão/ }));
    await waitFor(() => expect(api.callsTo("session.start")).toHaveLength(1));
    expect(api.callsTo("session.start")[0].params.prompt).toContain("Trabalhe na issue PROJ-5450 do Jira");
  });

  test("modal de início avisa quando o ambiente ainda não conferiu o jira-cli", async () => {
    const api = envApi({ pr: null, trackers: null });
    render(<App api={api} initial={{ name: "project", env: "wsl", projectId: P.id }} />);
    const user = userEvent.setup();
    await treeRow(/^PROJ-5450 Portar o plugin/);
    await user.click(within(side()).getByRole("button", { name: "Iniciar sessão para PROJ-5450" }));
    const note = await within(screen.getByRole("dialog")).findByRole("status", { name: "Prontidão do Jira e do GitLab" });
    expect(note).toHaveTextContent("ainda não conferiu o jira-cli e o glab deste projeto");
    expect(within(screen.getByRole("dialog")).getByRole("button", { name: /^Iniciar sessão/ })).toBeEnabled();
  });

  test("Sala de controle e Terminal: chave no cartão, no rodapé, na aba e no MR de criação do GitLab", async () => {
    const api = envApi({ pr: null });
    render(<App api={api} />);
    const user = userEvent.setup();
    const tile = (await screen.findAllByRole("button", { name: /Portar o plugin/ })).find((b) => b.classList.contains("tile"))!;
    expect(tile).toHaveTextContent("app PROJ-5450");
    const footer = screen.getByRole("region", { name: /Em revisão/ }).closest("footer")!;
    expect(footer).toHaveTextContent("app PROJ-5450");
    // Título e status do sync dizem Jira.
    expect(screen.getByRole("button", { name: "Buscar issues agora" })).toHaveAttribute("title", "Buscar agora as issues do Jira em todos os ambientes conectados");
    expect(screen.getByRole("contentinfo", { name: "Barra de status" })).toHaveTextContent(/Jira sincronizado/);
    expectNeutral();

    await user.click(tile);
    await waitFor(() => expect(api.attached.has(LIVE.id)).toBe(true));
    expect(screen.getByRole("treeitem", { name: /PROJ-5450 Portar o plugin/ })).toBeInTheDocument();
    const bar = screen.getByRole("region", { name: "Sessão" });
    expect(within(bar).getByText("PROJ-5450 Portar o plugin")).toBeInTheDocument();
    expectNeutral();
    // Sem MR: abre a criação do MR no GitLab, da branch para a base.
    await user.click(within(bar).getByRole("button", { name: "Mais ações" }));
    await user.click(screen.getByRole("menuitem", { name: "Abrir MR" }));
    await waitFor(() =>
      expect(api.opened).toEqual([
        "https://gitlab.example.com/grupo/projeto/app/-/merge_requests/new?merge_request[source_branch]=PROJ-5450&merge_request[target_branch]=develop",
      ]),
    );
  });

  test("Sala de controle: sync atrasado do Jira", async () => {
    render(<App api={envApi({ syncError: true })} />);
    expect(await screen.findByText("Jira de app sem sincronizar")).toBeInTheDocument();
    expectNeutral();
  });

  test("Revisão: MR !7, Comentar no MR copia o texto e abre o MR (sem o comentário do GitHub)", async () => {
    const api = envApi({ review: { work_item: WI, prs: [mr()], decisions: [decision(1, "Commit sem corpo")] } });
    render(<App api={api} initial={{ name: "review", env: "wsl", workItemId: WI.id }} />);
    const user = userEvent.setup();
    const card = await screen.findByRole("article", { name: "Commit sem corpo" });
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("PROJ-5450 Portar o plugin");
    expect(screen.getByText(/MR !7 aberto/)).toBeInTheDocument();
    const lateral = screen.getByRole("complementary", { name: "Itens com decisões a revisar" });
    expect(within(lateral).getByRole("region", { name: /Com MR aberto/ })).toHaveTextContent("MR !7 → develop");
    expectNeutral();

    await user.click(within(card).getByRole("button", { name: "Comentar no MR" }));
    expect(within(card).getByLabelText("Comentário no MR")).toBeInTheDocument();
    await user.click(within(card).getByRole("button", { name: "Copiar e abrir o MR" }));
    expect(await within(card).findByText(/Comentário copiado: cole na conversa do MR/)).toBeInTheDocument();
    expect(api.clipboard).toContain("Commit sem corpo");
    expect(api.opened).toEqual([MR_URL]);
    expect(api.callsTo("github.pr.comment")).toHaveLength(0);
    await user.click(screen.getByRole("button", { name: "Abrir MR !7" }));
    expect(api.opened).toEqual([MR_URL, MR_URL]);
    expectNeutral();
  });

  test("Revisão sem MR: abre a criação no GitLab", async () => {
    const api = envApi({ review: { work_item: WI, prs: [], decisions: [decision(1, "Commit sem corpo")] } });
    render(<App api={api} initial={{ name: "review", env: "wsl", workItemId: WI.id }} />);
    const user = userEvent.setup();
    expect(await screen.findByText(/Sem MR no espelho do GitLab \(sincronize depois de abrir o MR\)\./)).toBeInTheDocument();
    expectNeutral();
    await user.click(screen.getByRole("button", { name: "Abrir MR no GitLab" }));
    expect(api.opened[0]).toMatch(/^https:\/\/gitlab\.example\.com\/grupo\/projeto\/app\/-\/merge_requests\/new\?/);
  });

  test("Recuperação: a sessão interrompida aparece pela chave", async () => {
    const S = session({ id: "54505450-1111-0000-0000-000000005450", work_item_id: WI.id, state: "interrupted" });
    const item: RecoveryItem = {
      session: S,
      work_item: WI,
      project_name: "app",
      last_event: null,
      stopped_for_ms: 30 * 60_000,
      transcript: "original",
      worktree_status: "## PROJ-5450",
    };
    const api = envApi()
      .on("dashboard.get", dashboard({ projects: [{ project: P, work_items: [WI] }], interrupted_sessions: [S], sync: [] }))
      .on("recovery.list", [item])
      .on("recovery.archived", [])
      .on("transcript.tail", { source: "original", entries: [] });
    render(<App api={api} initial={{ name: "recovery" }} />);
    const lateral = await screen.findByRole("complementary", { name: "Sessões interrompidas" });
    const user = userEvent.setup();
    await user.click(await within(lateral).findByRole("button", { name: /PROJ-5450 Portar o plugin/ }));
    expect(await screen.findByRole("tab", { name: /Recuperação · PROJ-5450/ })).toBeInTheDocument();
    expectNeutral();
  });

  test("paleta ⌘K: issue pela chave e Buscar issues agora no Jira", async () => {
    const api = envApi();
    render(<App api={api} />);
    const user = userEvent.setup();
    await screen.findAllByRole("button", { name: /Portar o plugin/ });
    await user.click(screen.getByRole("button", { name: "Abrir paleta de comandos" }));
    const dialog = screen.getByRole("dialog", { name: "Paleta de comandos" });
    await user.type(within(dialog).getByRole("combobox"), "PROJ-5450");
    const opts = within(screen.getByRole("listbox", { name: "Resultados" })).getAllByRole("option");
    expect(opts.some((o) => o.textContent?.includes("PROJ-5450 Portar o plugin"))).toBe(true);
    await user.clear(within(dialog).getByRole("combobox"));
    await user.type(within(dialog).getByRole("combobox"), ">buscar issues");
    expect(within(screen.getByRole("listbox", { name: "Resultados" })).getAllByRole("option")[0]).toHaveTextContent(
      "Jira, nos ambientes conectados",
    );
    expectNeutral(dialog);
  });

  test("Manutenção › Limpeza: o sinal de conclusão diz MR mesclado e o item, a chave", async () => {
    const st: CleanupStatus = {
      config: {
        check_interval_hours: 6,
        auto: false,
        min_age_hours: 24,
        min_idle_hours: 24,
        end_idle_sessions_hours: 0,
        remove_agent_transcripts: true,
        target_idle_days: 14,
      },
      plan: {
        checked_at: NOW - 60_000,
        items: [
          {
            work_item_id: WI.id,
            project_id: 7,
            owner: "grupo/projeto",
            repo: "app",
            gh_number: 5450,
            title: "Portar o plugin",
            branch: "PROJ-5450",
            worktree_path: "/w/PROJ-5450",
            completion: { signal: "pr_mesclado", since: 1, pr: 7 },
            last_activity_at: null,
            steps: [{ kind: "worktree", state: "pendente", reason: null, bytes: 1024, targets: ["/w/PROJ-5450"] }],
            live_sessions: [],
          },
        ],
        pending_items: 1,
        pending_bytes: 1024,
      },
      check_error: null,
      last_run: null,
      busy: false,
    };
    const api = envApi()
      .on("env.info", { ...ENV_INFO, commit: "", env_id: "id-wsl", env_name: "wsl", hub_sync: null, hub_config: HUB_CONFIG_UNLINKED, update: null })
      .on("github.status", { source: "gh", login: "x", error: null })
      .on("cleanup.status", st);
    render(<App api={api} initial={{ name: "maintenance", section: "cleanup" }} />);
    const pend = await screen.findByRole("list", { name: /Limpezas pendentes em/ });
    expect(pend).toHaveTextContent("PROJ-5450");
    expect(pend).toHaveTextContent("MR mesclado");
    expectNeutral(pend);
  });
});

// Hub: projeto Jira + GitLab do exemplo (`grupo/projeto/app`).
const JP = jiraHubProject();

function hubApi(opts: { projects?: HubProjectView[]; health?: unknown; proposals?: unknown; status?: unknown } = {}) {
  const projects = opts.projects ?? [hubProjectView(JP, [hubAssignment({ project_id: 7, gh_owner: "grupo/projeto", gh_repo: "app" })])];
  const hubDash: HubDashboard = {
    projects: projects.map((p) => ({ project: p.project, work_items: [] })),
    live_sessions: [],
    interrupted_sessions: [],
    failed_sessions: [],
    sync: [],
    envs: [],
    gestor_work_item_id: null,
  };
  return new FakeDaemonApi([{ ...FakeDaemonApi.wslEnv(), name: "WSL" }])
    .on("dashboard.get", dashboard({ projects: [] }))
    .on("env.info", { ...ENV_INFO, commit: "", env_id: "daemon-wsl", env_name: "wsl-pc", hub_sync: null, hub_config: HUB_CONFIG_UNLINKED, update: null })
    .on("session.peek", { data: "" })
    .withHub()
    .onHub("hub.info", hubInfo())
    .onHub("hub.status", opts.status ?? hubStatus())
    .onHub("hub.projects.list", projects)
    .onHub("hub.envs.list", [hubEnv()])
    .onHub("hub.dashboard.get", hubDash)
    .onHub("hub.proposals.list", opts.proposals ?? NO_PROPOSALS)
    .onHub("hub.health.get", opts.health ?? NO_HEALTH)
    .onHub("github.status", { source: "token", login: "hub-bot", error: null })
    .onHub("hub.secrets.list", { status: { entries: 0, store_rev: 0, recovery_configured: true }, items: [], delivery: [] })
    .onHub("hub.recovery.status", recoveryStatus());
}

const hubItem = (n: number, p: Partial<HubWorkItem> = {}): HubWorkItem => ({
  id: 100 + n,
  project_id: 7,
  gh_number: n,
  kind: "issue",
  status: "em_revisao",
  branch: `PROJ-${n}`,
  created_at: 0,
  closed_at: null,
  priority: null,
  rank: null,
  ...p,
});

describe("hub com projeto Jira + GitLab", () => {
  test("Saúde e Visão geral: referência do hub, MR sem andamento e Jira sem sync", async () => {
    const health = {
      generated_at: NOW,
      signals: [
        healthSignal({ kind: "pr_sem_andamento", severity: "aviso", owner: "grupo/projeto", repo: "app", number: 5450, ref: "PROJ-5450", title: "Portar o plugin" }),
        healthSignal({ kind: "rastreador_sem_sync", severity: "critico", owner: "grupo/projeto", repo: "app", number: null, ref: null, title: null }),
      ],
      counts: { critico: 1, aviso: 1, info: 0 },
    };
    render(<App api={hubApi({ health })} initial={{ name: "hub" }} />);
    const user = userEvent.setup();
    const saude = await screen.findByRole("region", { name: /^Saúde dos projetos/ });
    const row = await within(saude).findByRole("row", { name: /MR sem andamento/ });
    expect(row).toHaveTextContent("PROJ-5450 Portar o plugin");
    expect(within(row).getByRole("button", { name: "Abrir PROJ-5450" })).toBeInTheDocument();
    expect(within(saude).getByRole("row", { name: /Jira sem sync/ })).toBeInTheDocument();
    expectNeutral(saude);

    await user.click(screen.getByRole("button", { name: /^Saúde dos projetos/ }));
    const panel = await screen.findByRole("region", { name: /^Saúde e riscos/ });
    expect(await within(panel).findByRole("button", { name: "PROJ-5450" })).toBeInTheDocument();
    expect(panel).toHaveTextContent("MR sem andamento");
    expectNeutral(panel);
  });

  test("Propostas: a chave do Jira no item e na posição", async () => {
    const proposals = {
      proposals: [
        hubProposal({ owner: "grupo/projeto", repo: "app", number: 5450, ref: "PROJ-5450", title: "Portar o plugin", proposed: { priority: "P0", before: 5460 } }),
      ],
      truncated: false,
    };
    const api = hubApi({ proposals }).onHub("hub.ranking.get", { items: [], truncated: false });
    render(<App api={api} initial={{ name: "hub" }} />);
    const user = userEvent.setup();
    const props = await screen.findByRole("region", { name: /^Propostas do gestor/ });
    const first = await within(props).findByRole("listitem", { name: "Proposta #1" });
    expect(first).toHaveTextContent("PROJ-5450 Portar o plugin → P0 · antes de PROJ-5460");
    expect(within(first).getByRole("button", { name: "PROJ-5450" })).toHaveAttribute("title", "PROJ-5450");
    expectNeutral(props);

    await user.click(screen.getByRole("button", { name: /^Propostas do gestor/ }));
    const sec = await screen.findByRole("region", { name: /^Propostas do gestor/ });
    await waitFor(() => expect(sec).toHaveTextContent("PROJ-5450"));
    expect(sec).toHaveTextContent("antes de PROJ-5460");
    expectNeutral(sec);
  });

  test("Quarentena de projeto GitLab: descarta pela última barra e cadastra como Jira + GitLab", async () => {
    const q = { project_key: "grupo/projeto/app", env_id: "daemon-wsl", env_name: "wsl-pc", rows: 4, first_received_at: NOW - H, last_received_at: NOW - 60_000 };
    const api = hubApi({ projects: [], status: hubStatus({ quarantine: [q] }) }).onHub("hub.quarantine.discard", { discarded: 4 });
    render(<App api={api} initial={{ name: "hub", section: "quarantine" }} />);
    const user = userEvent.setup();
    const group = await screen.findByRole("group", { name: "Quarentena de grupo/projeto/app" });
    expectNeutral(group);
    await user.click(within(group).getByRole("button", { name: "Descartar" }));
    await user.click(within(group).getByRole("button", { name: "Descartar linhas" }));
    await waitFor(() =>
      expect(api.hubCallsTo("hub.quarantine.discard")[0].params).toEqual({ owner: "grupo/projeto", repo: "app", confirm: true }),
    );
    await user.click(within(group).getByRole("button", { name: "Cadastrar projeto" }));
    const form = await screen.findByRole("form", { name: "Novo projeto" });
    expect(within(form).getByLabelText("Repositório do código-fonte")).toHaveValue("gitlab");
    expect(within(form).getByLabelText("Repositório de demandas (issues)")).toHaveValue("jira");
    expect(within(form).queryByLabelText("URL do GitHub")).not.toBeInTheDocument();
  });

  test("Configurações do projeto: atribuição pela última barra, pelo pedido do endereço antigo (#18)", async () => {
    const api = hubApi({ projects: [hubProjectView(JP, [])] }).onHub("hub.assignments.add", {
      assignment: hubAssignment({ project_id: 7, gh_owner: "grupo/projeto", gh_repo: "app" }),
      changed: true,
      sync_env: false,
    });
    // O endereço antigo (Hub › Projetos com `assign`) cai nas Configurações › Ambientes, com o formulário aberto.
    render(
      <App api={api} initial={{ name: "hub", section: "projects", project: { owner: "grupo/projeto", repo: "app" }, assign: true }} />,
    );
    const user = userEvent.setup();
    const detail = await screen.findByRole("group", { name: "Ambientes e atribuições" });
    const env = await within(detail).findByLabelText("Ambiente");
    await user.selectOptions(env, "daemon-wsl");
    await user.click(within(detail).getByRole("button", { name: /^Atribuir$/ }));
    await waitFor(() => expect(api.hubCallsTo("hub.assignments.add")[0].params).toMatchObject({ owner: "grupo/projeto", repo: "app" }));
  });

  test("tela Projeto pelo hub: MRs no histórico e decisão revisada no MR", async () => {
    const MR_HUB: PrRow = mr({ work_item_id: 150, state: "open" });
    const node = (i: IssueRow, wi: HubWorkItem | null, pr: PrRow | null = null): HubTreeNode => ({
      issue: i,
      work_item: wi,
      summary: { ...summary(pr), live_envs: [] },
      children: [],
    });
    const TREE: HubTree = {
      project: JP,
      sync: sync({ project_id: 7, last_ok_at: NOW }),
      roots: [node(issue(5450, "Portar o plugin"), hubItem(5450, { id: 150 }), MR_HUB)],
      untracked: [],
    };
    const HISTORY: HubHistory = {
      work_item: hubItem(5450, { id: 150 }),
      locations: [],
      sessions: [],
      events: [],
      next_before_hlc: null,
      sub_work_items: [],
      decisions: [
        {
          id: 1,
          uid: "d-1",
          env_id: "daemon-wsl",
          work_item_id: 150,
          session_id: null,
          origin: "autonoma",
          title: "Revisada antes",
          context: "",
          decision: "x",
          consequences: null,
          reviewed_at: 5,
          reviewed_in_pr: 7,
          created_at: 1,
        },
      ],
      prs: [MR_HUB],
    };
    const api = hubApi().onHub("hub.tree.get", TREE).onHub("hub.workitem.history", HISTORY);
    render(<App api={api} initial={{ name: "project", owner: "grupo/projeto", repo: "app" }} />);
    const user = userEvent.setup();
    await treeRow(/^PROJ-5450 Portar o plugin/);
    expect(within(side()).getByRole("img", { name: "Jira + GitLab" })).toBeInTheDocument();
    await user.click(within(side()).getByRole("button", { name: /^PROJ-5450 Portar o plugin/ }));
    const panel = await screen.findByRole("region", { name: "Detalhe do item" });
    const mrs = await within(panel).findByRole("region", { name: "MRs" });
    expect(within(mrs).getByRole("button", { name: "Abrir MR !7 no GitLab (aberto)" })).toBeInTheDocument();
    expect(within(panel).getByText("revisada no MR !7")).toBeInTheDocument();
    expect(api.hubCallsTo("hub.workitem.history")[0].params).toMatchObject({ owner: "grupo/projeto", repo: "app", number: 5450 });
    expectNeutral(panel);
  });
});

describe("projeto GitHub continua igual", () => {
  const GH = project();
  const GWI = workItem({ gh_number: 28, status: "em_revisao", title: "ct-server: lacunas", parent_number: null, branch: "feat/28-ct-server", pending_decisions: 1 });
  const GPR: PrRow = {
    number: 36,
    work_item_id: GWI.id,
    head: "feat/28-ct-server",
    base: "develop",
    state: "merged",
    url: "https://github.com/deep-ion-ai/ctower-agents/pull/36",
    updated_at: 0,
    head_sha: null,
  };
  const GTREE: ProjectTree = {
    project: GH,
    sync: sync({ last_ok_at: NOW }),
    roots: [
      {
        issue: { ...issue(28, "ct-server: lacunas"), url: "https://github.com/deep-ion-ai/ctower-agents/issues/28" },
        work_item: GWI,
        summary: summary(GPR),
        children: [],
      },
    ],
    untracked: [],
  };

  test("árvore e detalhe: #n, PR, ícone do GitHub e links do github.com", async () => {
    const api = new FakeDaemonApi()
      .on("dashboard.get", dashboard({ sync: [sync({ last_ok_at: NOW })] }, [GWI]))
      .on("session.peek", { data: "" })
      .on("tree.get", GTREE)
      .on("env.info", ENV_INFO);
    render(<App api={api} initial={{ name: "project", env: "wsl", projectId: GH.id }} />);
    const user = userEvent.setup();
    await treeRow(/^#28 ct-server: lacunas/);
    expect(within(side()).getByRole("img", { name: "GitHub" })).toBeInTheDocument();
    const row = within(side()).getByRole("button", { name: /^#28 ct-server: lacunas, em revisão.*PR #36 mesclado, PR mesclado, issue aberta/ });
    expect(screen.getByRole("contentinfo", { name: "Barra de status" })).toHaveTextContent(/GitHub sincronizado/);
    fireEvent.contextMenu(row.closest("li")!);
    expect(screen.getByRole("menuitem", { name: "Abrir no GitHub" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Fechar a issue no GitHub (PR mesclado)" })).toBeInTheDocument();
    act(() => fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" }));
    await user.click(row);
    const panel = await screen.findByRole("region", { name: "Detalhe do item" });
    expect(within(panel).getByRole("heading", { name: "#28 ct-server: lacunas" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "#28 ct-server: lacunas" })).toBeInTheDocument();
    await user.click(within(panel).getByRole("button", { name: "Fechar a issue no GitHub ↗" }));
    await user.click(within(panel).getByRole("button", { name: "Abrir PR #36 no GitHub ↗" }));
    expect(api.opened).toEqual(["https://github.com/deep-ion-ai/ctower-agents/issues/28", GPR.url]);
  });

  test("Revisão: Comentar no PR vai pelo github.pr.comment e Sem PR leva ao compare do GitHub", async () => {
    const api = new FakeDaemonApi()
      .on("dashboard.get", dashboard({ sync: [sync({ last_ok_at: NOW })] }, [GWI]))
      .on("session.peek", { data: "" })
      .on("review.get", { work_item: GWI, prs: [{ ...GPR, state: "open" }], decisions: [{ ...decision(1, "Tipos com ts-rs"), work_item_id: GWI.id }] })
      .on("github.pr.comment", { url: "https://github.com/deep-ion-ai/ctower-agents/pull/36#issuecomment-1" });
    render(<App api={api} initial={{ name: "review", env: "wsl", workItemId: GWI.id }} />);
    const user = userEvent.setup();
    const card = await screen.findByRole("article", { name: "Tipos com ts-rs" });
    expect(screen.getByText(/PR #36 aberto/)).toBeInTheDocument();
    await user.click(within(card).getByRole("button", { name: "Comentar no PR" }));
    await user.click(within(card).getByRole("button", { name: "Enviar comentário" }));
    expect(await within(card).findByText(/Comentário publicado no PR/)).toBeInTheDocument();
    expect(within(card).getByRole("button", { name: "Ver no GitHub" })).toBeInTheDocument();
    expect(api.callsTo("github.pr.comment")).toHaveLength(1);
    expect(api.clipboard).toBe("");
  });

  test("hub: sinais e propostas com owner/repo#n e #n", async () => {
    const health = { generated_at: NOW, signals: [healthSignal({ kind: "pr_sem_andamento", severity: "aviso" })], counts: { critico: 0, aviso: 1, info: 0 } };
    render(<App api={hubApi({ projects: [hubProjectView(jiraHubProject({ id: 1, gh_owner: "deep-ion-ai", gh_repo: "ctower-agents", name: "ctower-agents", tracker: { kind: "github" }, forge: { kind: "github" } }))], health, proposals: { proposals: [hubProposal()], truncated: false } })} initial={{ name: "hub" }} />);
    const saude = await screen.findByRole("region", { name: /^Saúde dos projetos/ });
    const row = await within(saude).findByRole("row", { name: /PR sem andamento/ });
    expect(row).toHaveTextContent("#176 atribuições");
    expect(within(row).getByRole("button", { name: "Abrir deep-ion-ai/ctower-agents#176" })).toBeInTheDocument();
    const props = screen.getByRole("region", { name: /^Propostas do gestor/ });
    expect(await within(props).findByRole("listitem", { name: "Proposta #1" })).toHaveTextContent("#215 prioridade e ordem → P0");
  });
});
