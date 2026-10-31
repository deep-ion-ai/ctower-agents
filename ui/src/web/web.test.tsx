// Modo web (épico #205, #201): testes de componente com a FakeDaemonApi no
// modo web (`asWeb`), o login e o painel da sessão no lugar do terminal.
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi, FakeWebAuth } from "../api/fake";
import type { HubWebSession } from "../protocol/hub-gen";
import type { Screen } from "../state/nav";
import { hubEnv, hubStatus } from "../test/fixtures";
import { envCalls, hubItem, hubSession, now, webApi } from "../test/web";
import { ENDED_NOTICE, LEFT_NOTICE, WebGate } from "./WebGate";

function renderApp(api: FakeDaemonApi, initial?: Screen) {
  return render(<App api={api} initial={initial} />);
}

describe("modo web: seções exclusivas do desktop ocultas ou desabilitadas", () => {
  it("barra de atividades e de título sem Terminal, Manutenção, sync e versão", async () => {
    const api = webApi();
    renderApp(api);
    const bar = await screen.findByRole("navigation", { name: "Telas" });
    await waitFor(() => expect(screen.getAllByText("WebDaemonApi").length).toBeGreaterThan(0));
    const names = within(bar)
      .getAllByRole("button")
      .map((b) => b.getAttribute("aria-label"));
    expect(names).toEqual(["Sala de controle", "Projetos", "Revisão", "Recuperação", "Hub", "Configurações"]);
    expect(screen.queryByRole("button", { name: /Buscar issues agora/ })).toBeNull();
    const status = screen.getByRole("contentinfo", { name: "Barra de status" });
    expect(within(status).queryByText(/^v/)).toBeNull();
    // Sem conexão com os ambientes: nada de `session.peek`, `dashboard.get` nem `env.info`.
    expect(envCalls(api)).toEqual([]);
    // O tile não tem o fim do terminal.
    expect(screen.queryByLabelText("Últimas linhas do terminal")).toBeNull();
  });

  it("estado do ambiente pelo último envio ao hub", async () => {
    const api = webApi().onHub("hub.envs.list", [hubEnv({ last_seen_at: now - 10 * 60_000 })]);
    renderApp(api);
    expect(await screen.findByText(/wsl-pc sem envio ao hub/)).toBeInTheDocument();
    expect(screen.getAllByText(/sem envio ao hub há 10 min/).length).toBeGreaterThan(0);
  });

  it("Hub com Segurança só no desktop (#26), com Sessões web; o antigo Hub › Projetos cai no índice", async () => {
    const user = userEvent.setup();
    const api = webApi();
    renderApp(api, { name: "hub" });
    const side = await screen.findByRole("region", { name: "Painel lateral" });
    await waitFor(() => expect(within(side).getByText("Sessões web")).toBeInTheDocument());
    // O grupo não some: cada linha avisa que é do desktop e abre a explicação.
    expect(within(side).getByText("Segurança")).toBeInTheDocument();
    for (const s of ["Secrets e variáveis", "Chave de recuperação", "Token do GitHub"]) {
      const row = within(side).getByText(s).closest("button")!;
      expect(row).toHaveTextContent("Só no app desktop");
    }
    await user.click(within(side).getByText("Token do GitHub"));
    expect(await screen.findByText(/fica só no app desktop/)).toBeInTheDocument();
    expect(api.hubCallsTo("hub.secrets.list")).toEqual([]);
    // Projetos e atribuições saiu do Hub (#18).
    expect(within(side).queryByText("Projetos e atribuições")).toBeNull();
  });

  it("endereço antigo do Hub › Projetos (#18): o índice de Projetos, só leitura", async () => {
    const api = webApi();
    renderApp(api, { name: "hub", section: "projects", register: { url: "" } });
    expect(await screen.findByRole("heading", { level: 1, name: "Projetos" })).toBeInTheDocument();
    await screen.findByRole("button", { name: "ctower-agents" });
    // O pedido de cadastro não abre nada na web; o botão fica, desabilitado com o motivo (#26).
    const novo = screen.getByRole("button", { name: "+ Novo projeto" });
    expect(novo).toBeDisabled();
    expect(novo).toHaveAccessibleDescription("Cadastro e edição no app desktop");
    expect(screen.queryByRole("form", { name: "Novo projeto" })).toBeNull();
    // `hub.secrets.list` está fora do /web/ws.
    expect(api.hubCallsTo("hub.secrets.list")).toEqual([]);
  });

  it("Hub (#26): gestor, cadastrar e descartar da quarentena desabilitados com o motivo", async () => {
    const q = {
      project_key: "deep-ion-ai/perdido",
      env_id: "daemon-wsl",
      env_name: "wsl-pc",
      rows: 12,
      first_received_at: now - 3_600_000,
      last_received_at: now - 60_000,
    };
    const api = webApi().onHub("hub.status", hubStatus({ quarantine: [q] }));
    const { unmount } = renderApp(api, { name: "hub" });
    const gestor = await screen.findByRole("button", { name: "Abrir sessão do gestor" });
    expect(gestor).toBeDisabled();
    expect(gestor).toHaveAccessibleDescription("Iniciar o gestor fica no app desktop");
    unmount();
    renderApp(api, { name: "hub", section: "quarantine" });
    const group = await screen.findByRole("group", { name: "Quarentena de deep-ion-ai/perdido" });
    for (const name of ["Cadastrar projeto", "Descartar"]) {
      const b = within(group).getByRole("button", { name });
      expect(b).toBeDisabled();
      expect(b).toHaveAccessibleDescription("Cadastro e edição no app desktop");
    }
    expect(api.hubCallsTo("hub.quarantine.discard")).toEqual([]);
  });

  it("seção sensível aberta por endereço: só o aviso, sem chamar o hub", async () => {
    const api = webApi();
    renderApp(api, { name: "hub", section: "secrets" });
    expect(await screen.findByText(/fica só no app desktop/)).toBeInTheDocument();
    expect(api.hubCallsTo("hub.secrets.list")).toEqual([]);
  });

  it("Configurações: só Aparência, sem Este app nem Ambiente", async () => {
    const api = webApi();
    renderApp(api, { name: "settings" });
    expect(await screen.findByRole("heading", { name: "Aparência" })).toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: /Este app/ })).toBeNull();
    expect(screen.queryByRole("tab", { name: /Ambiente/ })).toBeNull();
    for (const h of ["Conexões", "Sessões", "Terminal", "Notificações"]) {
      expect(screen.queryByRole("heading", { name: h })).toBeNull();
    }
  });

  it("Manutenção por endereço: indisponível na web", async () => {
    renderApp(webApi(), { name: "maintenance" });
    expect(await screen.findByText(/Manutenção fica só no app desktop/)).toBeInTheDocument();
  });

  it("Projeto pela visão do hub com o ambiente offline: sem iniciar sessão", async () => {
    const api = webApi().onHub("hub.envs.list", [hubEnv({ last_seen_at: now, online: false })]);
    renderApp(api, { name: "project", owner: "deep-ion-ai", repo: "ctower-agents" });
    expect(await screen.findByText("visão do hub")).toBeInTheDocument();
    expect(await screen.findByText(/Nenhum ambiente com este projeto está online/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Sessão de planejamento/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Secrets e variáveis" })).toBeNull();
    // A atribuição fica no desktop (#7): o aviso não tem o atalho.
    expect(screen.queryByRole("button", { name: "Atribuir a um ambiente" })).toBeNull();
  });

  it("Configurações do projeto (#8, #17, #26): só leitura, Editar, secrets e exclusão desabilitados", async () => {
    const user = userEvent.setup();
    const api = webApi();
    renderApp(api, { name: "project", owner: "deep-ion-ai", repo: "ctower-agents" });
    await user.click(await screen.findByRole("button", { name: "Configurações do projeto" }));
    await screen.findByRole("group", { name: "Geral" });
    expect(screen.getByRole("tab", { name: /ctower-agents · Configurações/ })).toHaveAttribute("aria-selected", "true");
    const editar = screen.getByRole("button", { name: /^Editar/ });
    expect(editar).toBeDisabled();
    expect(editar).toHaveAccessibleDescription("Cadastro e edição no app desktop");
    const secoes = screen.getByRole("navigation", { name: "Seções das configurações do projeto" });
    for (const s of ["Secrets e variáveis", "Limpeza", "Zona de perigo"]) {
      const b = within(secoes).getByRole("button", { name: s });
      expect(b).toBeDisabled();
      expect(b).toHaveAccessibleDescription("Secrets, limpeza e exclusão do projeto no app desktop");
    }
    // A seção do desktop pedida pelo endereço cai em Geral; Ambientes, sem atribuir nem remover.
    await user.click(within(secoes).getByRole("button", { name: "Ambientes e atribuições" }));
    await screen.findByRole("group", { name: "Ambientes e atribuições" });
    expect(screen.getByRole("button", { name: /^Atribuir a ambiente/ })).toBeDisabled();
    expect(screen.queryByRole("button", { name: /^Remover/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Gerenciar secrets do projeto" })).toBeNull();
    expect(api.hubCallsTo("hub.secrets.list")).toEqual([]);
  });

  it("pedido de atribuição por endereço (#7, #18): Configurações › Ambientes, só leitura, sem o formulário", async () => {
    const api = webApi().onHub("hub.envs.list", [
      hubEnv({ last_seen_at: now, online: true }),
      hubEnv({ env_id: "daemon-mac", name: "mac", last_seen_at: now, online: true }),
    ]);
    renderApp(api, { name: "hub", section: "projects", project: { owner: "deep-ion-ai", repo: "ctower-agents" }, assign: true });
    await screen.findByRole("group", { name: "Ambientes e atribuições" });
    await waitFor(() => expect(api.hubCallsTo("hub.envs.list").length).toBeGreaterThan(0));
    expect(screen.queryByRole("form", { name: /^Atribuir/ })).toBeNull();
  });

  it("Projetos na Sala de controle (#10, #26): '+' desabilitado e sem Atribuir a ambiente no menu", async () => {
    renderApp(webApi());
    const side = await screen.findByRole("complementary", { name: "Ambientes e projetos" });
    const linha = await within(side).findByRole("button", { name: "ctower-agents" });
    const mais = within(side).getByRole("button", { name: "Novo projeto" });
    expect(mais).toBeDisabled();
    expect(mais).toHaveAccessibleDescription("Cadastro e edição no app desktop");
    fireEvent.contextMenu(linha.closest("li")!, { clientX: 10, clientY: 10 });
    const menu = screen.getByRole("menu", { name: "Ações do projeto ctower-agents" });
    expect(within(menu).getByRole("menuitem", { name: "Configurações" })).toBeInTheDocument();
    expect(within(menu).queryByRole("menuitem", { name: "Atribuir a ambiente" })).toBeNull();
  });

  it("Recuperação com o ambiente offline: só leitura, pelas interrompidas do hub", async () => {
    const api = webApi({ live_sessions: [], interrupted_sessions: [hubSession({ state: "interrupted" })] }).onHub(
      "hub.envs.list",
      [hubEnv({ last_seen_at: now, online: false })],
    );
    renderApp(api, { name: "recovery" });
    expect(await screen.findByRole("heading", { name: "Recuperação · 1" })).toBeInTheDocument();
    expect(await screen.findByText(/ambiente offline/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Retomar/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /VS Code/ })).toBeNull();
  });
});

describe("modo web: painel da sessão no lugar do terminal", () => {
  it("abre pela Sala de controle e mostra Abrir no Remote Control", async () => {
    const api = webApi({ live_sessions: [hubSession({ rc_session_id: "session_abc" })] }, [
      hubItem({ status: "aguardando_voce" }),
    ]);
    const user = userEvent.setup();
    renderApp(api);
    // O tile da grade (o destaque "Aguardando você" abre o Remote Control direto; mobile.test.tsx).
    await screen.findByRole("region", { name: "Aguardando você · 1" });
    const grid = document.querySelector<HTMLElement>(".tiles-section")!;
    const tile = within(grid).getByRole("button", { name: /WebDaemonApi/ });
    await user.click(tile);
    expect(await screen.findByRole("heading", { name: /#201 WebDaemonApi/ })).toBeInTheDocument();
    expect(screen.getByText("Aguardando você")).toBeInTheDocument();
    expect(screen.getByText("wsl-pc")).toBeInTheDocument();
    // Sem terminal.
    expect(document.querySelector(".xterm")).toBeNull();
    expect(screen.queryByRole("button", { name: /Responder no terminal/ })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Abrir no Remote Control" }));
    expect(api.opened).toEqual(["https://claude.ai/code/session_abc"]);
    await user.click(screen.getByRole("button", { name: "Copiar link" }));
    expect(api.clipboard).toBe("https://claude.ai/code/session_abc");
    expect(envCalls(api)).toEqual([]);
  });

  it("sem Remote Control: estado e o botão de ligar, sem o de abrir", async () => {
    const api = webApi();
    renderApp(api);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /WebDaemonApi/ }));
    expect(await screen.findByText(/Remote Control desligado/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Ligar Remote Control" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Abrir no Remote Control" })).toBeNull();
  });
});

describe("modo web: login, sair e revogação", () => {
  function gate(auth: FakeWebAuth, api = webApi()) {
    return render(
      <WebGate auth={auth}>
        <App api={api} />
      </WebGate>,
    );
  }

  it("sem sessão: login com o token, que não fica guardado", async () => {
    const auth = new FakeWebAuth();
    const user = userEvent.setup();
    localStorage.clear();
    gate(auth);
    const input = await screen.findByLabelText("Token de UI do hub");
    expect(auth.starts).toBe(0);

    await user.type(input, "errado");
    await user.click(screen.getByRole("button", { name: "Entrar" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Token inválido.");
    expect(screen.getByLabelText("Token de UI do hub")).toHaveValue("");

    await user.type(screen.getByLabelText("Token de UI do hub"), "tok-ui");
    await user.click(screen.getByRole("button", { name: "Entrar" }));
    expect(await screen.findByRole("navigation", { name: "Telas" })).toBeInTheDocument();
    expect(auth.attempts).toEqual(["errado", "tok-ui"]);
    expect(auth.starts).toBe(1);
    expect(JSON.stringify({ ...localStorage })).not.toContain("tok-ui");
    expect(JSON.stringify({ ...sessionStorage })).not.toContain("tok-ui");
  });

  it("com sessão: direto no shell; Sair volta ao login", async () => {
    const auth = new FakeWebAuth().signedIn();
    const user = userEvent.setup();
    gate(auth);
    await user.click(await screen.findByRole("button", { name: "Sair" }));
    expect(auth.logouts).toBe(1);
    expect(await screen.findByText(LEFT_NOTICE)).toBeInTheDocument();
    expect(screen.queryByRole("navigation", { name: "Telas" })).toBeNull();
  });

  it("4401 (sessão revogada ou vencida) leva à tela de login", async () => {
    const auth = new FakeWebAuth().signedIn();
    gate(auth);
    await screen.findByRole("navigation", { name: "Telas" });
    act(() => auth.revoke());
    expect(await screen.findByText(ENDED_NOTICE)).toBeInTheDocument();
    expect(screen.getByLabelText("Token de UI do hub")).toBeInTheDocument();
  });

  it("Configurações › Acesso web mostra a identidade e sai", async () => {
    const auth = new FakeWebAuth().signedIn("eu@exemplo.com");
    const user = userEvent.setup();
    render(
      <WebGate auth={auth}>
        <App api={webApi()} initial={{ name: "settings", section: "web-access" }} />
      </WebGate>,
    );
    expect(await screen.findByText(/Identidade do proxy: eu@exemplo.com/)).toBeInTheDocument();
    const box = screen.getByText(/Identidade do proxy/).closest("div")!;
    await user.click(within(box).getByRole("button", { name: "Sair" }));
    expect(await screen.findByText(LEFT_NOTICE)).toBeInTheDocument();
  });

  it("Hub › Sessões web: lista e revoga", async () => {
    const list: HubWebSession[] = [
      {
        id: "w1",
        created_at: now - 86_400_000,
        last_used_at: now - 60_000,
        expires_at: now + 29 * 86_400_000,
        identity: "eu@exemplo.com",
        user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) Version/18.0 Mobile Safari/604.1",
        current: true,
      },
      {
        id: "w2",
        created_at: now - 2 * 86_400_000,
        last_used_at: now - 3600_000,
        expires_at: now + 28 * 86_400_000,
        identity: null,
        user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/130.0 Safari/537.36",
        current: false,
      },
    ];
    let rows = list;
    const api = webApi()
      .onHub("hub.web.sessions.list", () => rows)
      .onHub("hub.web.sessions.revoke", (p: { id?: string }) => {
        rows = rows.filter((r) => r.id !== p.id);
        return { revoked: 1 };
      });
    const user = userEvent.setup();
    renderApp(api, { name: "hub", section: "web-sessions" });
    expect(await screen.findByText("Safari · iPhone")).toBeInTheDocument();
    expect(screen.getByText("este navegador")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /^Revogar sessão Chrome · macOS/ }));
    await waitFor(() => expect(screen.queryByText("Chrome · macOS")).toBeNull());
    expect(api.hubCallsTo("hub.web.sessions.revoke").map((c) => c.params)).toEqual([{ id: "w2" }]);
    await user.click(screen.getByRole("button", { name: "Revogar todas…" }));
    await user.click(screen.getByRole("button", { name: "Revogar todas" }));
    expect(api.hubCallsTo("hub.web.sessions.revoke").map((c) => c.params)).toEqual([{ id: "w2" }, { all: true }]);
  });
});

describe("modo web: backup dos docs só leitura (#444)", () => {
  it("Visão geral do hub mostra o último pacote dos docs, sem botões de ação", async () => {
    const api = webApi().onHub("hub.backup.get", {
      remote: { kind: "b2", encrypt: true, bucket: "b", prefix: "", region: "", endpoint: "", drive_folder_id: "" },
      status: {},
      credential_fields: [],
      docs: {
        docs_dir: "/srv/ct-docs",
        interval_hours: 24,
        keep: 7,
        max_mb: 1024,
        newest_at: now - 3_600_000,
        newest_name: "docs-20261029-0300.tar.zst",
        last_ok_at: now - 3_600_000,
        last_size: 2048,
        last_error: null,
        last_skipped: null,
        too_large_bytes: null,
        remote: { last_ok_at: null, last_name: null, last_size: null, last_error: "HTTP 503", failures: 2 },
      },
    });
    renderApp(api, { name: "hub" });
    const card = await screen.findByRole("region", { name: "Backup dos docs" });
    expect(card).toHaveTextContent("docs-20261029-0300.tar.zst · 2,0 KB");
    expect(card).toHaveTextContent("falhou (2×): HTTP 503");
    expect(card).toHaveTextContent("Manutenção do app desktop");
    expect(within(card).queryByRole("button")).not.toBeInTheDocument();
    expect(api.hubCallsTo("hub.backup.run")).toHaveLength(0);
  });
});
