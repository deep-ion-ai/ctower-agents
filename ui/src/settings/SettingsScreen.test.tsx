import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { DEFAULT_UI_CONFIG } from "./defaults";
import { FakeDaemonApi } from "../api/fake";
import type { Screen } from "../state/nav";
import { DEFAULT_FONT, termPrefs, zoom } from "../terminal/prefs";
import { dashboard, HUB_CONFIG_UNLINKED, project, session, sync } from "../test/fixtures";

const NOW = Date.now();
const ENV: Screen = { name: "settings", scope: "env" };
const item = (name: string | RegExp) => screen.getByRole("region", { name });

function setup(initial: Screen = { name: "settings" }) {
  const api = new FakeDaemonApi([
    FakeDaemonApi.wslEnv(),
    { id: "mac", name: "mac-mini", url: "ws://mac:7878/ws", kind: "remote", distro: null, has_token: true },
  ])
    .on("dashboard.get", dashboard({ sync: [sync({ last_ok_at: NOW })] }))
    .on("env.info", { protocol: "2.0", version: "0.1.0", hostname: "ct", worktrees_root: "/home/user/worktrees" })
    .on("sync.now", [])
    .on("github.status", { source: "gh", login: "octocat", error: null });
  render(<App api={api} initial={initial} />);
  return { api, user: userEvent.setup() };
}

test("ambientes sem token visível; token remoto novo vai só no salvar", async () => {
  const { api, user } = setup();
  expect(await screen.findByText("lido da distro ctower-agents")).toBeInTheDocument();
  expect(screen.getByText("•••• definido")).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Editar mac-mini" }));
  const form = screen.getByRole("form", { name: "Editar mac-mini" });
  const token = within(form).getByLabelText("Token de UI") as HTMLInputElement;
  expect(token.value).toBe("");
  expect(token.type).toBe("password");
  await user.click(within(form).getByRole("button", { name: "Salvar ambiente" }));
  // Token vazio = mantém o atual (o lado Rust preserva).
  expect(api.savedEnvs[0].find((e) => e.id === "mac")?.token).toBe("");

  await user.click(screen.getByRole("button", { name: "+ Adicionar ambiente" }));
  const novo = screen.getByRole("form", { name: "Novo ambiente" });
  await user.type(within(novo).getByLabelText("Nome"), "Servidor Ação");
  await user.clear(within(novo).getByLabelText("URL"));
  await user.type(within(novo).getByLabelText("URL"), "ws://srv:7878/ws");
  await user.type(within(novo).getByLabelText("Token de UI"), "novo-token");
  await user.click(within(novo).getByRole("button", { name: "Salvar ambiente" }));
  await waitFor(() => expect(api.savedEnvs).toHaveLength(2));
  const salvo = api.savedEnvs[1].find((e) => e.name === "Servidor Ação")!;
  expect(salvo).toMatchObject({ id: "servidor-acao", kind: "remote", token: "novo-token" });
  expect(await screen.findByRole("button", { name: "Editar Servidor Ação" })).toBeInTheDocument();
});

test("projetos do ambiente são somente leitura: cadastro e edição ficam no hub", async () => {
  const { api } = setup(ENV);
  api.on(
    "dashboard.get",
    dashboard({
      projects: [
        {
          project: project({ config_rev: 3, location_state: "error", location_error: "não é um clone" }),
          work_items: [],
        },
      ],
      sync: [sync({ last_ok_at: NOW })],
    }),
  );
  act(() => api.push("wsl", "session_started"));
  const row = (await screen.findByText("deep-ion-ai/ctower-agents")).closest("tr")!;
  expect(await within(row).findByText("com erro: não é um clone")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "+ Cadastrar projeto" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Editar ctower-agents" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Excluir ctower-agents" })).not.toBeInTheDocument();
  expect(screen.getByText(/cadastrados, editados e atribuídos aos ambientes no hub/)).toBeInTheDocument();
  for (const m of ["projects.add", "projects.update", "projects.delete", "projects.parse_url"]) {
    expect(api.callsTo(m)).toHaveLength(0);
  }
});

test("ambiente sem hub avisa que não está ligado", async () => {
  const { api } = setup(ENV);
  api.on("env.info", {
    protocol: "2.0",
    version: "0.1.0",
    hostname: "ct",
    worktrees_root: "/home/user/worktrees",
    hub_config: HUB_CONFIG_UNLINKED,
  });
  act(() => api.push("wsl", "session_started"));
  expect(await screen.findByText(/Ambiente não ligado a um hub/)).toBeInTheDocument();
});

test("projetos do ambiente: revisão e idade da configuração; o que ficou fora do hub está na Manutenção", async () => {
  const { api } = setup(ENV);
  api
    .on("env.info", {
      protocol: "2.0",
      version: "0.1.0",
      hostname: "ct",
      worktrees_root: "/home/user/worktrees",
      env_id: "daemon-wsl",
      hub_config: { ...HUB_CONFIG_UNLINKED, linked: true, config_rev: 42, pulled_at: NOW - 3 * 3_600_000 },
    })
    .on("projects.list", [project({ id: 7, name: "legado", gh_repo: "legado", repo_path: "/p/legado" })]);
  act(() => api.push("wsl", "session_started"));
  expect(await screen.findByText("Configuração rev 42 · há 3 h")).toBeInTheDocument();
  // Os restos fora do hub saíram para Manutenção › Limpeza (#337).
  expect(screen.queryByRole("table", { name: "Projetos fora do hub" })).not.toBeInTheDocument();
  expect(api.callsTo("projects.list")).toHaveLength(0);
});

test("Limpeza nas Configurações: só as regras e a ponte para a Manutenção", async () => {
  const { api, user } = setup({ name: "settings", section: "cleanup" });
  api.on("cleanup.status", {
    config: {
      check_interval_hours: 6,
      auto: false,
      min_age_hours: 24,
      min_idle_hours: 24,
      end_idle_sessions_hours: 0,
      remove_agent_transcripts: true,
      target_idle_days: 14,
    },
    plan: null,
    check_error: null,
    last_run: null,
    busy: false,
  });
  act(() => api.push("wsl", "session_started"));
  const regras = await screen.findByRole("table", { name: "Regras de limpeza" });
  const linha = (k: string) => within(regras).getByText(k).closest("tr")!;
  expect(await within(linha("check_interval_hours")).findByText("6 h")).toBeInTheDocument();
  expect(within(linha("auto")).getByText("desligado")).toBeInTheDocument();
  expect(within(linha("end_idle_sessions_hours")).getByText("desligado")).toBeInTheDocument();
  expect(within(linha("target_idle_days")).getByText("14 dia(s)")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /Verificar agora/ })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /^Limpar/ })).not.toBeInTheDocument();
  await user.click(within(item("Limpeza: Plano e limpeza")).getByRole("button", { name: "Ver e limpar em Manutenção ›" }));
  expect(screen.getByRole("tab", { name: "Manutenção · Limpeza" })).toHaveAttribute("aria-selected", "true");
});

test("busca por limpeza, limpar ou fora do hub cai na ponte para a Manutenção", async () => {
  const { user } = setup({ name: "settings", scope: "env" });
  for (const q of ["limpeza", "limpar", "fora do hub"]) {
    await user.clear(screen.getByRole("searchbox", { name: "Buscar configurações" }));
    await user.type(screen.getByRole("searchbox", { name: "Buscar configurações" }), q);
    expect(await screen.findByRole("region", { name: "Limpeza: Plano e limpeza" })).toBeInTheDocument();
  }
});

test("conexão com o hub: separada dos ambientes, token não volta e dá para remover", async () => {
  const { api, user } = setup();
  const hub = item("Hub: Conexão");
  expect(within(hub).getByText("não configurado")).toBeInTheDocument();
  await user.click(within(hub).getByRole("button", { name: "Configurar hub" }));
  const form = within(hub).getByRole("form", { name: "Conexão com o hub" });
  await user.clear(within(form).getByLabelText("URL do hub"));
  await user.type(within(form).getByLabelText("URL do hub"), "http://mac-mini:7879");
  const token = within(form).getByLabelText("Token de UI do hub") as HTMLInputElement;
  expect(token.type).toBe("password");
  await user.type(token, "ui-do-hub");
  await user.click(within(form).getByRole("button", { name: "Salvar hub" }));
  await waitFor(() => expect(api.savedHubs).toEqual([{ url: "http://mac-mini:7879", token: "ui-do-hub" }]));
  // As conexões dos ambientes não mudam.
  expect(api.savedEnvs).toHaveLength(0);
  expect(await within(hub).findByText("http://mac-mini:7879")).toBeInTheDocument();
  expect(within(hub).getAllByText("•••• definido")).toHaveLength(1);
  expect(screen.queryByText("ui-do-hub")).not.toBeInTheDocument();

  await user.click(within(hub).getByRole("button", { name: "Editar conexão" }));
  const edit = within(hub).getByRole("form", { name: "Conexão com o hub" });
  expect(within(edit).getByLabelText("Token de UI do hub")).toHaveValue("");
  await user.click(within(edit).getByRole("button", { name: "Cancelar" }));
  await user.click(within(hub).getByRole("button", { name: "Remover conexão" }));
  await waitFor(() => expect(api.savedHubs[api.savedHubs.length - 1]).toBeNull());
  expect(await within(hub).findByText("não configurado")).toBeInTheDocument();
});

test("ambiente ligado a um hub sem a conexão na UI: aviso", async () => {
  const { api } = setup();
  api.on("env.info", {
    protocol: "2.0",
    version: "0.1.0",
    hostname: "ct",
    worktrees_root: "/home/user/worktrees",
    hub_config: { ...HUB_CONFIG_UNLINKED, linked: true },
  });
  act(() => api.push("wsl", "session_started"));
  expect(await screen.findByText(/Há ambientes ligados a um hub, mas a conexão com ele não está configurada/)).toBeInTheDocument();
  expect(screen.getByText(/Conexão com o hub não configurada: projetos/)).toBeInTheDocument();
});

test("token do GitHub: mostra conta e origem, nunca o token", async () => {
  const { api, user } = setup(ENV);
  expect(await screen.findByText("@octocat")).toBeInTheDocument();
  expect(screen.getByText(/login do gh na distro/)).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Remover token" })).not.toBeInTheDocument();
  api.on("github.set_token", { source: "token", login: "octocat", error: null });
  const campo = screen.getByLabelText("Novo token (personal access token)");
  expect(campo).toHaveAttribute("type", "password");
  await user.type(campo, "ghp_segredo123");
  await user.click(screen.getByRole("button", { name: "Salvar token" }));
  await waitFor(() => expect(api.callsTo("github.set_token")[0].params).toEqual({ token: "ghp_segredo123" }));
  expect(await screen.findByText(/token configurado no Control Tower/)).toBeInTheDocument();
  expect(campo).toHaveValue("");
  expect(document.body.textContent).not.toContain("ghp_segredo123");
  api.on("github.clear_token", { source: "gh", login: "octocat", error: null });
  await user.click(screen.getByRole("button", { name: "Remover token" }));
  await waitFor(() => expect(api.callsTo("github.clear_token")).toHaveLength(1));
  expect(await screen.findByText(/login do gh na distro/)).toBeInTheDocument();
});

test("token recusado mostra o erro", async () => {
  const { api, user } = setup(ENV);
  api.on("github.set_token", () => {
    throw "GitHub respondeu 401: Bad credentials";
  });
  await user.type(await screen.findByLabelText("Novo token (personal access token)"), "errado");
  await user.click(screen.getByRole("button", { name: "Salvar token" }));
  expect(await screen.findByText("GitHub respondeu 401: Bad credentials")).toBeInTheDocument();
});

test("preferência do Remote Control vai para session.remote_control no ui.json (#197)", async () => {
  const { api, user } = setup();
  const caixa = (await screen.findByLabelText("Ligar o Remote Control ao iniciar")) as HTMLInputElement;
  expect(caixa.checked).toBe(false);
  await user.click(caixa);
  await waitFor(() => expect(api.uiConfig.session.remote_control).toBe(true));
  expect(within(item("Início de sessão: Remote Control")).getByTitle("Alterada")).toBeInTheDocument();
});

test("modelo e modo padrão vão para a configuração da UI ao escolher, com Restaurar padrão", async () => {
  const { api, user } = setup();
  const modelo = (await screen.findByLabelText("Modelo padrão")) as HTMLSelectElement;
  expect(modelo.value).toBe("");
  expect((screen.getByLabelText("Modo padrão") as HTMLSelectElement).value).toBe("native");
  expect(screen.getByText("0 alteradas")).toBeInTheDocument();
  await user.selectOptions(modelo, "fable");
  await waitFor(() => expect(api.uiConfig.session).toEqual({ model: "fable", mode: "native", remote_control: false }));
  await user.selectOptions(screen.getByLabelText("Modo padrão"), "subagents");
  await waitFor(() => expect(api.uiConfig.session).toEqual({ model: "fable", mode: "subagents", remote_control: false }));
  expect(screen.getByText("2 alteradas")).toBeInTheDocument();
  const sessao = item("Início de sessão: Modelo padrão");
  expect(within(sessao).getByTitle("Alterada")).toBeInTheDocument();
  // De volta ao padrão do Claude Code: `null`, não string vazia.
  await user.click(within(sessao).getByRole("button", { name: "Restaurar padrão de Modelo padrão" }));
  await waitFor(() => expect(api.uiConfig.session.model).toBeNull());
  expect(screen.getByText("1 alterada")).toBeInTheDocument();
  expect(within(sessao).queryByRole("button", { name: /Restaurar padrão/ })).not.toBeInTheDocument();
});

test("template abre em aba própria e salva; notificações vão para a configuração da UI", async () => {
  const { api, user } = setup();
  await user.click(await screen.findByRole("button", { name: "Editar o template Issue avulsa e sub-issue em aba própria" }));
  expect(screen.getByRole("tab", { name: "Template · Issue avulsa e sub-issue" })).toHaveAttribute("aria-selected", "true");
  const issue = screen.getByLabelText("Issue avulsa e sub-issue") as HTMLTextAreaElement;
  await user.clear(issue);
  await user.type(issue, "Faça a issue {{{{number}}.");
  await user.click(screen.getByRole("button", { name: "Salvar template" }));
  await waitFor(() => expect(api.uiConfig.templates.issue).toBe("Faça a issue {{number}}."));
  expect(screen.getByText("Template salvo.")).toBeInTheDocument();

  await user.click(screen.getByRole("tab", { name: "Configurações" }));
  const tpl = item("Templates: Issue avulsa e sub-issue");
  expect(within(tpl).getByText("alterada")).toBeInTheDocument();
  await user.click(within(tpl).getByRole("button", { name: /Restaurar padrão/ }));
  await waitFor(() => expect(api.uiConfig.templates.issue).toBe(DEFAULT_UI_CONFIG.templates.issue));

  await user.click(screen.getByLabelText(/Ambiente desconectado/));
  await waitFor(() => expect(api.uiConfig.notify.env_down).toBe(false));
  expect(api.uiConfig.notify.needs_human).toBe(true);
  await user.click(within(item("Notificações: Avisar quando")).getByRole("button", { name: /Restaurar padrão/ }));
  await waitFor(() => expect(api.uiConfig.notify.env_down).toBe(true));
});

test("editar o template fixa a aba provisória; abrir outro item não a substitui", async () => {
  const { user } = setup();
  await user.click(await screen.findByRole("button", { name: "Editar o template Issue avulsa e sub-issue em aba própria" }));
  // Abrir e ler não fixa.
  expect(screen.getByRole("button", { name: "Fixar aba" })).toHaveAttribute("aria-pressed", "false");
  await user.type(screen.getByLabelText("Issue avulsa e sub-issue"), " mais");
  expect(screen.getByRole("button", { name: "Desafixar aba" })).toHaveAttribute("aria-pressed", "true");

  await user.click(screen.getByRole("tab", { name: "Configurações" }));
  await user.click(screen.getByRole("button", { name: "Editar o template Épico em aba própria" }));
  expect(screen.getByRole("tab", { name: "Template · Issue avulsa e sub-issue" })).toBeInTheDocument();
  expect(screen.getByRole("tab", { name: "Template · Épico" })).toHaveAttribute("aria-selected", "true");
});

test("escopo Ambiente: daemon e raiz dos worktrees de um ambiente por vez", async () => {
  const { user } = setup(ENV);
  expect(await screen.findByRole("tab", { name: "Ambiente: ctower-agents (WSL)" })).toHaveAttribute("aria-selected", "true");
  expect(await screen.findByText("/home/user/worktrees")).toBeInTheDocument();
  expect(screen.getByText(/daemon 0.1.0 · protocolo 2.0/)).toBeInTheDocument();
  await user.selectOptions(screen.getByLabelText("Ambiente das configurações"), "mac");
  expect(screen.getByRole("tab", { name: "Ambiente: mac-mini" })).toHaveAttribute("aria-selected", "true");
  expect(await screen.findByText(/ de mac-mini, no/)).toBeInTheDocument();
  // O escopo fica no endereço da aba: voltar a Este app e ao Ambiente mantém o escolhido.
  await user.click(screen.getByRole("tab", { name: "Este app" }));
  expect(item("Hub: Conexão")).toBeInTheDocument();
  expect(screen.queryByText("/home/user/worktrees")).not.toBeInTheDocument();
  await user.click(screen.getByRole("tab", { name: "Ambiente: mac-mini" }));
  expect(item("Projetos: Raiz dos worktrees")).toBeInTheDocument();
});

test("busca filtra por nome e descrição, sem acento, e conta o outro escopo", async () => {
  const { user } = setup();
  const busca = await screen.findByRole("searchbox", { name: "Buscar configurações" });
  await user.type(busca, "modelo");
  expect(item("Início de sessão: Modelo padrão")).toBeInTheDocument();
  expect(screen.queryByRole("region", { name: "Terminal: Fonte" })).not.toBeInTheDocument();
  expect(screen.queryByRole("region", { name: "Hub: Conexão" })).not.toBeInTheDocument();
  // O índice só mostra as seções com resultado.
  const indice = screen.getByRole("navigation", { name: "Índice das configurações" });
  expect(within(indice).getAllByRole("button").map((b) => b.textContent)).toEqual(["Sessões"]);

  await user.clear(busca);
  await user.type(busca, "SINCRONIZACAO");
  expect(screen.getByText(/Nenhuma configuração encontrada/)).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Ver 1 em Ambiente: ctower-agents (WSL)" }));
  expect(item("GitHub: Intervalo de sincronização")).toBeInTheDocument();
});

test("busca pela chave do config.toml acha a configuração do ambiente", async () => {
  const { api, user } = setup(ENV);
  api.on("env.info", {
    protocol: "2.0",
    version: "0.1.0",
    hostname: "ct",
    worktrees_root: "/home/user/worktrees",
    update: {
      installed: { version: "0.1.0", commit: "" },
      ref: "develop",
      available: null,
      check_error: null,
      state: null,
    },
  });
  act(() => api.push("wsl", "session_started"));
  await user.type(screen.getByRole("searchbox", { name: "Buscar configurações" }), "update_ref");
  expect(screen.getByRole("tab", { name: "Ambiente: ctower-agents (WSL) (1)" })).toBeInTheDocument();
  expect(screen.getByRole("tab", { name: "Este app (0)" })).toBeInTheDocument();
  const canal = item("Atualização: Canal");
  expect(await within(canal).findByText("develop")).toBeInTheDocument();
  // Fora do padrão (main): alterada, mas sem Restaurar (o canal é do config.toml do ambiente).
  expect(within(canal).getByText("alterada")).toBeInTheDocument();
  expect(within(canal).queryByRole("button", { name: /Restaurar padrão/ })).not.toBeInTheDocument();
  expect(screen.queryByRole("region", { name: "GitHub: Token do ambiente" })).not.toBeInTheDocument();

  await user.clear(screen.getByRole("searchbox", { name: "Buscar configurações" }));
  await user.type(screen.getByRole("searchbox", { name: "Buscar configurações" }), "worktrees_root");
  expect(item("Projetos: Raiz dos worktrees")).toBeInTheDocument();
});

/** `env.info` com o atualizador ligado e `env.config.get|set` em memória. */
function withEnvConfig(api: FakeDaemonApi, ref = "main", secs = 180) {
  const items: Record<string, { key: string; value: string | number; default: string | number; configured: boolean }> = {
    update_ref: { key: "update_ref", value: ref, default: "main", configured: ref !== "main" },
    gh_sync_interval_secs: { key: "gh_sync_interval_secs", value: secs, default: 180, configured: secs !== 180 },
  };
  api
    .on("env.info", {
      protocol: "2.2",
      version: "0.1.0",
      hostname: "ct",
      worktrees_root: "/home/user/worktrees",
      update: { installed: { version: "0.1.0", commit: "" }, ref, available: null, check_error: null, state: null },
    })
    .on("env.config.get", () => Object.values(items))
    .on("env.config.set", ({ key, value }: { key: string; value: string | number }) => {
      items[key] = { ...items[key], value, configured: true };
      return items[key];
    });
  // O push relê dashboard e env.info depois do debounce, em qualquer ponto do
  // teste: as asserções procuram a chamada pelo método, não a última.
  act(() => api.push("wsl", "session_started"));
}

test("canal editável pelo env.config.set: vale ao sair do campo, recusa nome inválido e restaura o padrão", async () => {
  const { api, user } = setup(ENV);
  withEnvConfig(api, "develop");
  const canal = await screen.findByRole("region", { name: "Atualização: Canal" });
  const campo = await within(canal).findByDisplayValue("develop");
  expect(within(canal).getByText("alterada")).toBeInTheDocument();

  await user.clear(campo);
  await user.type(campo, "minha branch");
  expect(within(canal).getByText(/Nome de branch ou tag/)).toBeInTheDocument();
  await user.tab();
  // Inválido não vai ao daemon e o campo volta ao valor em uso.
  expect(api.calls.some((c) => c.method === "env.config.set")).toBe(false);
  expect(campo).toHaveValue("develop");

  await user.clear(campo);
  await user.type(campo, "release/1.2{Enter}");
  await waitFor(() =>
    expect(api.calls.filter((c) => c.method === "env.config.set").map((c) => c.params)).toEqual([
      { key: "update_ref", value: "release/1.2" },
    ]),
  );

  await user.click(within(canal).getByRole("button", { name: /Restaurar padrão/ }));
  await waitFor(() =>
    expect(api.calls.filter((c) => c.method === "env.config.set").slice(-1)[0]?.params).toEqual({ key: "update_ref", value: "main" }),
  );
  await waitFor(() => expect(within(canal).queryByText("alterada")).not.toBeInTheDocument());
});

test("intervalo de sincronização: 0 ou ≥ 30, e o erro do daemon aparece no item", async () => {
  const { api, user } = setup(ENV);
  withEnvConfig(api);
  const sync = await screen.findByRole("region", { name: "GitHub: Intervalo de sincronização" });
  const campo = await within(sync).findByLabelText("Intervalo de sincronização (segundos)");
  expect(campo).toHaveValue(180);

  await user.clear(campo);
  await user.type(campo, "10");
  expect(within(sync).getByText("0 (desligado) ou pelo menos 30 segundos.")).toBeInTheDocument();
  await user.keyboard("{Enter}");
  expect(api.calls.some((c) => c.method === "env.config.set")).toBe(false);

  await user.clear(campo);
  await user.type(campo, "0{Enter}");
  await waitFor(() => expect(api.callsTo("env.config.set").slice(-1)[0]?.params).toEqual({ key: "gh_sync_interval_secs", value: 0 }));
  expect(await within(sync).findByText("desligado")).toBeInTheDocument();
  expect(within(sync).getByText("alterada")).toBeInTheDocument();

  api.on("env.config.set", () => {
    throw new Error("config.toml: permissão negada");
  });
  await user.clear(campo);
  await user.type(campo, "60{Enter}");
  expect(await within(sync).findByRole("alert")).toHaveTextContent("config.toml: permissão negada");
});

test("daemon sem env.config.*: canal e intervalo ficam só leitura com a chave do config.toml", async () => {
  const { api } = setup(ENV);
  api.on("env.info", {
    protocol: "2.1",
    version: "0.1.0",
    hostname: "ct",
    worktrees_root: "/w",
    update: { installed: { version: "0.1.0", commit: "" }, ref: "main", available: null, check_error: null, state: null },
  });
  act(() => api.push("wsl", "session_started"));
  const canal = await screen.findByRole("region", { name: "Atualização: Canal" });
  expect(await within(canal).findByLabelText("Canal de atualização")).toHaveTextContent("main");
  expect(within(canal).queryByRole("textbox")).not.toBeInTheDocument();
  expect(within(await screen.findByRole("region", { name: "GitHub: Intervalo de sincronização" })).queryByRole("spinbutton")).not.toBeInTheDocument();
});

test("Reverter pede confirmação e manda force só com sessões vivas", async () => {
  const { api, user } = setup(ENV);
  withEnvConfig(api);
  api
    .on("dashboard.get", dashboard({ live_sessions: [session({ id: "s1" }), session({ id: "s2" })] }))
    .on("env.rollback", { kind: "rollback", phase: "starting" });
  act(() => api.push("wsl", "session_started"));
  const rev = await screen.findByRole("region", { name: "Atualização: Reverter" });
  await user.click(await within(rev).findByRole("button", { name: "Reverter" }));
  const box = within(rev).getByRole("group", { name: "Confirmar reversão" });
  expect(await within(box).findByText(/2 sessão\(ões\) viva\(s\)/)).toBeInTheDocument();
  await user.click(within(box).getByRole("button", { name: "Reverter e encerrar sessões" }));
  await waitFor(() => expect(api.callsTo("env.rollback").map((c) => c.params)).toEqual([{ force: true }]));
  expect(await within(rev).findByText(/revertendo…/)).toBeInTheDocument();
});

test("Reverter mostra a recusa do daemon", async () => {
  const { api, user } = setup(ENV);
  withEnvConfig(api);
  api.on("env.rollback", () => {
    throw new Error("nenhuma versão anterior guardada");
  });
  const rev = await screen.findByRole("region", { name: "Atualização: Reverter" });
  await user.click(await within(rev).findByRole("button", { name: "Reverter" }));
  await user.click(within(rev).getByRole("group", { name: "Confirmar reversão" }).querySelector(".btn-primary")!);
  expect(api.callsTo("env.rollback").map((c) => c.params)).toEqual([{ force: false }]);
  expect(await within(rev).findByRole("alert")).toHaveTextContent("nenhuma versão anterior guardada");
});

test("notificações por ambiente: silenciar guarda o id em notify.muted_envs", async () => {
  const { api, user } = setup(ENV);
  const n = await screen.findByLabelText("Notificações deste ambiente");
  expect(n).toBeChecked();
  await user.click(n);
  await waitFor(() => expect(api.uiConfig.notify.muted_envs).toEqual(["wsl"]));
  expect(api.uiConfig.notify.needs_human).toBe(true);
  await user.click(within(item("Notificações: Notificar deste ambiente")).getByRole("button", { name: /Restaurar padrão/ }));
  await waitFor(() => expect(api.uiConfig.notify.muted_envs).toEqual([]));
});

test("o que é do hub fica no Hub: link abre Hub › Projetos e não há cadastro, secrets nem backup aqui", async () => {
  const { user } = setup();
  for (const name of [/Cadastrar projeto/, /Secrets/, /Backup/]) {
    expect(screen.queryByRole("region", { name })).not.toBeInTheDocument();
  }
  await user.click(screen.getByRole("button", { name: "Projetos e secrets ficam no Hub ›" }));
  expect(screen.getByRole("tab", { name: "Hub · Projetos e atribuições" })).toHaveAttribute("aria-selected", "true");
});

test("seção pelo endereço (paleta): abre no escopo dela", async () => {
  setup({ name: "settings", section: "daemon-update" });
  expect(screen.getByRole("tab", { name: /^Ambiente/ })).toHaveAttribute("aria-selected", "true");
  expect(await screen.findByRole("heading", { name: "Atualização do daemon" })).toBeInTheDocument();
  // Só os ajustes (#336): estado e botões de atualizar ficam em Manutenção, com a ponte até lá.
  expect(item("Atualização: Intervalo de verificação")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /^Atualizar/ })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Verificar agora" })).not.toBeInTheDocument();
  expect(within(item("Atualização: Versão e atualização")).getByRole("button", { name: "Ver e atualizar em Manutenção ›" })).toBeInTheDocument();
});

test("busca por atualizar, versão ou rollback cai na ponte para a Manutenção", async () => {
  const { user } = setup({ name: "settings", scope: "env" });
  for (const q of ["atualizar", "versão", "rollback"]) {
    await user.clear(screen.getByRole("searchbox", { name: "Buscar configurações" }));
    await user.type(screen.getByRole("searchbox", { name: "Buscar configurações" }), q);
    expect(await screen.findByRole("region", { name: "Atualização: Versão e atualização" })).toBeInTheDocument();
  }
});

test("seção Terminal salva fonte e tamanho nesta máquina ao sair do campo e zera o zoom", async () => {
  const { user } = setup();
  act(() => zoom(1));
  const tamanhoItem = await screen.findByRole("region", { name: "Terminal: Tamanho da fonte" });
  expect(within(tamanhoItem).getByText(/Zoom atual: 14 px/)).toBeInTheDocument();
  const fonte = screen.getByLabelText("Fonte");
  await user.clear(fonte);
  await user.type(fonte, "Cascadia Code, monospace{Enter}");
  expect(termPrefs().fontFamily).toBe("Cascadia Code, monospace");
  const tamanho = screen.getByLabelText("Tamanho da fonte");
  await user.clear(tamanho);
  await user.type(tamanho, "40");
  expect(within(tamanhoItem).getByText("Entre 8 e 32.")).toBeInTheDocument();
  await user.clear(tamanho);
  await user.type(tamanho, "15");
  await user.tab();
  expect(termPrefs()).toEqual({ fontFamily: "Cascadia Code, monospace", fontSize: 15, zoom: 0, closeOnExit: true });
  await user.click(within(item("Terminal: Fonte")).getByRole("button", { name: /Restaurar padrão/ }));
  await user.click(within(tamanhoItem).getByRole("button", { name: /Restaurar padrão/ }));
  expect(fonte).toHaveValue(DEFAULT_FONT);
  expect(tamanho).toHaveValue(13);
  expect(termPrefs().fontSize).toBe(13);
});

test("seção Terminal: fechar a aba ao sair sem erro vem ligado e desliga nesta máquina (#432)", async () => {
  const { user } = setup();
  const it = await screen.findByRole("region", { name: "Terminal: Fechar ao sair" });
  const box = within(it).getByRole("checkbox", { name: "Fechar a aba quando a sessão sair sem erro" });
  expect(box).toBeChecked();
  await user.click(box);
  expect(termPrefs().closeOnExit).toBe(false);
  expect(within(it).getByTitle("Alterada")).toBeInTheDocument();
  await user.click(within(it).getByRole("button", { name: /Restaurar padrão/ }));
  expect(termPrefs().closeOnExit).toBe(true);
});

test("tema: Claro, Escuro ou Seguir o sistema vai para o ui.json e vale na hora; persiste na próxima abertura", async () => {
  const { api, user } = setup({ name: "settings", section: "appearance" });
  const tema = (await screen.findByLabelText("Tema")) as HTMLSelectElement;
  expect(tema.value).toBe("dark");
  expect([...tema.options].map((o) => o.text)).toEqual(["Seguir o sistema", "Claro", "Escuro"]);
  await waitFor(() => expect(document.documentElement.dataset.theme).toBe("dark"));

  await user.selectOptions(tema, "light");
  await waitFor(() => expect(api.uiConfig.appearance).toEqual({ theme: "light" }));
  expect(document.documentElement.dataset.theme).toBe("light");
  expect(localStorage.getItem("ct.theme")).toBe("light");
  expect(within(item("Aparência: Tema")).getByTitle("Alterada")).toBeInTheDocument();

  // Próxima abertura: o ui.json manda, mesmo com outro tema no <html>.
  cleanup();
  document.documentElement.dataset.theme = "dark";
  render(<App api={api} initial={{ name: "settings", section: "appearance" }} />);
  expect(((await screen.findByLabelText("Tema")) as HTMLSelectElement).value).toBe("light");
  await waitFor(() => expect(document.documentElement.dataset.theme).toBe("light"));

  await user.click(screen.getByRole("button", { name: "Restaurar padrão de Tema" }));
  await waitFor(() => expect(api.uiConfig.appearance.theme).toBe("dark"));
  expect(document.documentElement.dataset.theme).toBe("dark");
});

test("antes de o ui.json chegar, fica o tema posto pelo boot", async () => {
  document.documentElement.dataset.theme = "light";
  const api = new FakeDaemonApi([FakeDaemonApi.wslEnv()]).on("dashboard.get", dashboard());
  let release!: () => void;
  const gate = new Promise<void>((r) => (release = r));
  const get = api.getUiConfig.bind(api);
  api.getUiConfig = async () => {
    await gate;
    return { ...(await get()), appearance: { theme: "light" } };
  };
  render(<App api={api} initial={{ name: "settings", section: "appearance" }} />);
  await screen.findByLabelText("Tema");
  expect(document.documentElement.dataset.theme).toBe("light");
  await act(async () => release());
  expect(document.documentElement.dataset.theme).toBe("light");
});

test("prompts prontos (#425): editar, restaurar o padrão de um, reordenar, adicionar, remover e restaurar a lista", async () => {
  const { api, user } = setup();
  const sec = await screen.findByRole("region", { name: "Prompts prontos: Menu do console" });
  const card = (name: string) => within(sec).getByRole("listitem", { name });
  const nomes = () => api.uiConfig.prompts.map((p) => p.name);
  expect(within(sec).queryByText("alterada")).toBeNull();

  // Editar o texto de um padrão: só vale no Salvar; "Restaurar padrão" volta o texto.
  const texto = within(card("Continuar")).getByLabelText("Texto");
  await user.clear(texto);
  await user.type(texto, "continue com a {{{{key}}");
  expect(api.uiConfig.prompts[0].text).toBe("continue");
  await user.click(within(sec).getByRole("button", { name: "Salvar prompts" }));
  await waitFor(() => expect(api.uiConfig.prompts[0].text).toBe("continue com a {{key}}"));
  expect(within(sec).getByText("Prompts salvos.")).toBeInTheDocument();
  expect(within(sec).getByText("alterada")).toBeInTheDocument();
  await user.click(within(card("Continuar")).getByRole("button", { name: "Restaurar padrão de Continuar" }));
  expect(within(card("Continuar")).getByLabelText("Texto")).toHaveValue("continue");
  await user.click(within(sec).getByRole("button", { name: "Salvar prompts" }));
  await waitFor(() => expect(api.uiConfig.prompts).toEqual(DEFAULT_UI_CONFIG.prompts));

  // Reordenar, adicionar (nome e texto obrigatórios) e remover.
  await user.click(within(sec).getByRole("button", { name: "Descer Continuar" }));
  await user.click(within(sec).getByRole("button", { name: "Adicionar prompt" }));
  const salvar = within(sec).getByRole("button", { name: "Salvar prompts" });
  expect(salvar).toBeDisabled();
  expect(within(sec).getByText("Todo prompt precisa de um nome.")).toBeInTheDocument();
  const novo = card("Prompt 6");
  await user.type(within(novo).getByLabelText("Nome"), "Revisar");
  await user.type(within(card("Revisar")).getByLabelText("Texto"), "Revise o diff");
  await user.click(within(sec).getByRole("button", { name: "Remover Abrir o PR" }));
  await user.click(salvar);
  await waitFor(() =>
    expect(nomes()).toEqual([
      "Rodar os testes e corrigir",
      "Continuar",
      "Registrar as decisões no ledger",
      "Resumir o estado atual",
      "Revisar",
    ]),
  );
  expect(api.uiConfig.prompts[4]).toMatchObject({ name: "Revisar", text: "Revise o diff" });
  expect(within(card("Revisar")).queryByRole("button", { name: /Restaurar padrão/ })).toBeNull();

  // O "Restaurar padrão" do item volta a lista inteira.
  await user.click(within(sec).getByRole("button", { name: "Restaurar padrão de Menu do console" }));
  await waitFor(() => expect(api.uiConfig.prompts).toEqual(DEFAULT_UI_CONFIG.prompts));
  expect(within(sec).queryByRole("listitem", { name: "Revisar" })).toBeNull();
});

test("índice rola só o painel de conteúdo, nunca o documento (#431)", async () => {
  const scrolled: { el: Element; opts: ScrollToOptions }[] = [];
  const scrollIntoView = vi.fn();
  Element.prototype.scrollTo = function (this: Element, opts?: ScrollToOptions | number) {
    scrolled.push({ el: this, opts: opts as ScrollToOptions });
  } as Element["scrollTo"];
  Element.prototype.scrollIntoView = scrollIntoView;
  try {
    const { user } = setup();
    const toc = await screen.findByRole("navigation", { name: "Índice das configurações" });
    await user.click(within(toc).getByRole("button", { name: "Terminal" }));
    expect(scrolled).toHaveLength(1);
    expect(scrolled[0].el).toHaveClass("set-list");
    expect(scrolled[0].opts).toMatchObject({ behavior: "smooth" });
    expect(scrollIntoView).not.toHaveBeenCalled();
  } finally {
    delete (Element.prototype as Partial<Element>).scrollTo;
    delete (Element.prototype as Partial<Element>).scrollIntoView;
  }
});
