import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { HubTree, RotatePending, SecretItem, SecretKind, SecretScope, SecretsList } from "../protocol/hub-gen";
import { dashboard, hubEnv, hubInfo, hubProject, hubProjectView, hubStatus, HUB_DASHBOARD, recoveryStatus, sync } from "../test/fixtures";

const NOW = Date.now();
const CT = "deep-ion-ai/ctower-agents";
const SEGREDO = "valor-secreto-que-nao-volta";

const sameScope = (a: SecretScope, b: SecretScope) => JSON.stringify(a) === JSON.stringify(b);

/** Store do hub em memória: `hub.secrets.list|set|delete` como o servidor (sem valor de secret na listagem). */
function hubStore(rotatePending: RotatePending | null = null) {
  let rev = 10;
  let pending = rotatePending;
  const values = new Map<string, string>();
  const items: SecretItem[] = [];
  const keyOf = (s: SecretScope, n: string) => `${JSON.stringify(s)}:${n}`;
  const set = (p: { scope: SecretScope; name: string; kind: SecretKind; value: string }) => {
    rev++;
    const i = items.findIndex((x) => sameScope(x.scope, p.scope) && x.name === p.name);
    const item: SecretItem = {
      scope: p.scope,
      name: p.name,
      kind: p.kind,
      rev,
      updated_at: NOW - 60_000,
      ...(p.kind === "variable" ? { value: p.value } : {}),
    };
    if (i >= 0) items[i] = item;
    else items.push(item);
    values.set(keyOf(p.scope, p.name), p.value);
    return { rev };
  };
  const del = (p: { scope: SecretScope; name: string }) => {
    const i = items.findIndex((x) => sameScope(x.scope, p.scope) && x.name === p.name);
    if (i < 0) throw new Error(`${p.name} não existe`);
    items.splice(i, 1);
    rev++;
    return { rev };
  };
  const list = (): SecretsList => ({
    status: { entries: items.length, store_rev: rev, recovery_configured: false },
    items: structuredClone(items),
    delivery: [
      { env_id: "daemon-wsl", env_name: "wsl-pc", state: "delivered", applied_rev: rev, required_rev: rev, projects: [CT] },
      { env_id: "daemon-mac", env_name: "mac-mini", state: "pending", applied_rev: 3, required_rev: rev, projects: [] },
      { env_id: "daemon-note", env_name: "notebook", state: "no_key", applied_rev: null, required_rev: rev, projects: [CT] },
    ],
    rotate_pending: structuredClone(pending),
  });
  const dismiss = () => {
    pending = null;
    return {};
  };
  set({ scope: "global", name: "API_TOKEN", kind: "secret", value: SEGREDO });
  set({ scope: "global", name: "RUST_LOG", kind: "variable", value: "info" });
  set({ scope: { project: CT }, name: "API_TOKEN", kind: "secret", value: `${SEGREDO}-projeto` });
  return { set, del, list, dismiss, values };
}

function withHub(api: FakeDaemonApi, store: ReturnType<typeof hubStore>) {
  return api
    .withHub("ws://mac-mini:7879/ws")
    .onHub("hub.info", hubInfo())
    .onHub("hub.status", hubStatus())
    .onHub("hub.projects.list", [hubProjectView(hubProject())])
    .onHub("hub.envs.list", [hubEnv()])
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("github.status", { source: "token", login: "hub-bot", error: null })
    .onHub("hub.secrets.list", () => store.list())
    .onHub("hub.secrets.set", (p: any) => store.set(p))
    .onHub("hub.secrets.delete", (p: any) => store.del(p))
    .onHub("hub.secrets.rotate_pending_dismiss", () => store.dismiss())
    .onHub("hub.recovery.status", recoveryStatus({ configured: false, public_key: null, last_verified_at: null, next_verify_at: null }));
}

function setupGlobal(rotatePending: RotatePending | null = null) {
  const store = hubStore(rotatePending);
  const api = withHub(new FakeDaemonApi().on("dashboard.get", dashboard({ projects: [] })), store);
  render(<App api={api} initial={{ name: "hub", section: "secrets" }} />);
  return { api, store, user: userEvent.setup() };
}

const TREE: HubTree = {
  project: hubProject(),
  sync: sync({ last_ok_at: NOW }),
  roots: [],
  untracked: [],
};

function setupProject() {
  const store = hubStore();
  const api = withHub(new FakeDaemonApi().on("dashboard.get", dashboard({ projects: [] })), store).onHub(
    "hub.tree.get",
    TREE,
  );
  render(<App api={api} initial={{ name: "project", owner: "deep-ion-ai", repo: "ctower-agents" }} />);
  return { api, store, user: userEvent.setup() };
}

test("escopo global: lista sem valor de secret, cria, edita e remove", async () => {
  const { api, store, user } = setupGlobal();
  const sec = await screen.findByRole("region", { name: "Secrets e variáveis" });
  const table = await within(sec).findByRole("table", { name: "Secrets e variáveis globais" });
  const token = within(table).getByText("API_TOKEN", { selector: "td" }).closest("tr")!;
  expect(within(token).getByText("•••• definido")).toBeInTheDocument();
  const log = within(table).getByText("RUST_LOG", { selector: "td" }).closest("tr")!;
  expect(within(log).getByText("info")).toBeInTheDocument();
  expect(within(log).getByText("variável")).toBeInTheDocument();
  // Só os globais: o secret do projeto não aparece aqui.
  expect(within(table).getAllByRole("row")).toHaveLength(3);
  expect(within(sec).getByText(/Recuperação não configurada/)).toBeInTheDocument();

  // Estado de entrega por ambiente.
  const entrega = within(sec).getByRole("list", { name: "Entrega aos ambientes" });
  expect(within(entrega).getByText("wsl-pc").closest("li")).toHaveTextContent("entregue");
  expect(within(entrega).getByText("mac-mini").closest("li")).toHaveTextContent("pendente");
  expect(within(entrega).getByText("notebook").closest("li")).toHaveTextContent("sem chave");

  // Criar: nome validado no cliente, valor mascarado.
  await user.click(within(sec).getByRole("button", { name: "+ Adicionar" }));
  const form = within(sec).getByRole("form", { name: "Novo secret ou variável" });
  const valor = within(form).getByLabelText("Valor") as HTMLInputElement;
  expect(valor.type).toBe("password");
  await user.type(within(form).getByLabelText("Nome"), "db_url");
  expect(within(form).getByRole("alert")).toHaveTextContent(/Nome inválido/);
  await user.type(valor, "postgres://senha");
  expect(within(form).getByRole("button", { name: "Salvar" })).toBeDisabled();
  // Nome reservado: recusado no cliente, com o motivo.
  for (const reservado of ["CTOWER_TOKEN", "PATH", "LD_PRELOAD"]) {
    await user.clear(within(form).getByLabelText("Nome"));
    await user.type(within(form).getByLabelText("Nome"), reservado);
    expect(within(form).getByRole("alert")).toHaveTextContent(`Nome reservado: o ambiente já define ${reservado}`);
    expect(within(form).getByRole("button", { name: "Salvar" })).toBeDisabled();
  }
  await user.clear(within(form).getByLabelText("Nome"));
  await user.type(within(form).getByLabelText("Nome"), "DB_URL");
  expect(within(form).queryByRole("alert")).not.toBeInTheDocument();
  expect(within(form).queryByRole("button", { name: /mostrar/i })).not.toBeInTheDocument();
  await user.click(within(form).getByRole("button", { name: "Salvar" }));
  await waitFor(() =>
    expect(api.hubCallsTo("hub.secrets.set")[0].params).toEqual({
      scope: "global",
      name: "DB_URL",
      kind: "secret",
      value: "postgres://senha",
    }),
  );
  expect(await within(sec).findByText(/DB_URL gravado \(configuração rev 14\)/)).toHaveTextContent(
    "Vale a partir do próximo spawn ou resume, depois que o ambiente receber a configuração; sessões em andamento não mudam.",
  );
  expect(within(sec).getByText("DB_URL", { selector: "td" })).toBeInTheDocument();
  expect(document.body.textContent).not.toContain("postgres://senha");
  expect(document.body.textContent).not.toContain(SEGREDO);

  // Editar um secret: campo vazio, nunca pré-preenchido.
  await user.click(within(sec).getByRole("button", { name: "Editar API_TOKEN" }));
  const edit = within(sec).getByRole("form", { name: "Editar API_TOKEN" });
  const novo = within(edit).getByLabelText("Valor") as HTMLInputElement;
  expect(novo.value).toBe("");
  expect(novo.type).toBe("password");
  expect(within(edit).getByLabelText("Nome")).toHaveAttribute("readonly");
  expect(within(edit).getByRole("button", { name: "Salvar" })).toBeDisabled();
  await user.type(novo, "trocado");
  await user.click(within(edit).getByRole("button", { name: "Salvar" }));
  await waitFor(() => expect(store.values.get('"global":API_TOKEN')).toBe("trocado"));

  // Editar uma variável: valor atual no campo de texto.
  await user.click(within(sec).getByRole("button", { name: "Editar RUST_LOG" }));
  const editLog = within(sec).getByRole("form", { name: "Editar RUST_LOG" });
  const campo = within(editLog).getByLabelText("Valor") as HTMLInputElement;
  expect(campo.type).toBe("text");
  expect(campo.value).toBe("info");
  await user.clear(campo);
  await user.type(campo, "debug");
  await user.click(within(editLog).getByRole("button", { name: "Salvar" }));
  expect(await within(sec).findByText("debug")).toBeInTheDocument();

  // Remover com confirmação.
  await user.click(within(sec).getByRole("button", { name: "Remover RUST_LOG" }));
  const box = within(sec).getByRole("group", { name: "Confirmar remoção de RUST_LOG" });
  expect(api.hubCallsTo("hub.secrets.delete")).toHaveLength(0);
  await user.click(within(box).getByRole("button", { name: "Remover RUST_LOG" }));
  await waitFor(() =>
    expect(api.hubCallsTo("hub.secrets.delete")[0].params).toEqual({ scope: "global", name: "RUST_LOG" }),
  );
  await waitFor(() => expect(within(sec).queryByText("RUST_LOG", { selector: "td" })).not.toBeInTheDocument());
  expect(within(sec).getByRole("status")).toHaveTextContent(/RUST_LOG removido .*sessões em andamento não mudam/);
});

test("erro do hub ao gravar aparece no formulário", async () => {
  const { api, user } = setupGlobal();
  api.onHub("hub.secrets.set", () => {
    throw "store de secrets indisponível neste hub";
  });
  const sec = await screen.findByRole("region", { name: "Secrets e variáveis" });
  await user.click(within(sec).getByRole("button", { name: "+ Adicionar" }));
  const form = within(sec).getByRole("form", { name: "Novo secret ou variável" });
  await user.type(within(form).getByLabelText("Nome"), "X");
  await user.type(within(form).getByLabelText("Valor"), "y");
  await user.click(within(form).getByRole("button", { name: "Salvar" }));
  expect(await within(form).findByRole("alert")).toHaveTextContent("store de secrets indisponível neste hub");
});

test("escopo do projeto: herda os globais, marca a sobrescrita, cria, edita e remove", async () => {
  const { api, store, user } = setupProject();
  // Pelas Configurações do projeto no Hub (#8): o cabeçalho do Projeto não tem mais o toggle.
  expect(await screen.findByText("visão do hub")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Secrets e variáveis" })).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Configurações do projeto" }));
  const toggle = await screen.findByRole("button", { name: "Gerenciar secrets do projeto" });
  expect(toggle).toHaveAttribute("aria-expanded", "false");
  await user.click(toggle);
  expect(toggle).toHaveAttribute("aria-expanded", "true");
  const sec = await screen.findByRole("region", { name: `Secrets e variáveis · ${CT}` });
  const table = await within(sec).findByRole("table", { name: `Secrets e variáveis de ${CT}` });
  const rows = within(table).getAllByRole("row").slice(1);
  expect(rows).toHaveLength(3);
  // O do projeto sobrescreve o global de mesmo nome; o global aparece como sobrescrito.
  expect(rows[0]).toHaveTextContent("API_TOKEN");
  expect(within(rows[0]).getByText("sobrescreve o global")).toBeInTheDocument();
  expect(within(rows[0]).getByRole("button", { name: "Editar API_TOKEN" })).toBeInTheDocument();
  expect(rows[1]).toHaveTextContent("global · sobrescrito pelo projeto");
  expect(within(rows[1]).queryByRole("button")).not.toBeInTheDocument();
  expect(rows[2]).toHaveTextContent("RUST_LOG");
  expect(rows[2]).toHaveTextContent("global · herdado");
  expect(document.body.textContent).not.toContain(SEGREDO);

  // Entrega: só os ambientes com o projeto atribuído.
  const entrega = within(sec).getByRole("list", { name: "Entrega aos ambientes" });
  expect(within(entrega).getAllByRole("listitem")).toHaveLength(2);
  expect(within(entrega).queryByText("mac-mini")).not.toBeInTheDocument();

  // Criar no projeto um nome que existe no global: passa a sobrescrever.
  await user.click(within(sec).getByRole("button", { name: "+ Adicionar" }));
  const form = within(sec).getByRole("form", { name: "Novo secret ou variável" });
  await user.type(within(form).getByLabelText("Nome"), "RUST_LOG");
  await user.selectOptions(within(form).getByLabelText("Tipo"), "variable");
  expect((within(form).getByLabelText("Valor") as HTMLInputElement).type).toBe("text");
  await user.type(within(form).getByLabelText("Valor"), "trace");
  await user.click(within(form).getByRole("button", { name: "Salvar" }));
  await waitFor(() =>
    expect(api.hubCallsTo("hub.secrets.set")[0].params).toEqual({
      scope: { project: CT },
      name: "RUST_LOG",
      kind: "variable",
      value: "trace",
    }),
  );
  const own = (await within(sec).findByText("trace")).closest("tr")!;
  expect(within(own).getByText("sobrescreve o global")).toBeInTheDocument();
  expect(within(table).getAllByText("global · sobrescrito pelo projeto")).toHaveLength(2);

  // Editar o secret do projeto: valor novo, campo vazio.
  await user.click(within(sec).getByRole("button", { name: "Editar API_TOKEN" }));
  const edit = within(sec).getByRole("form", { name: "Editar API_TOKEN" });
  expect(within(edit).getByLabelText("Valor")).toHaveValue("");
  await user.type(within(edit).getByLabelText("Valor"), "novo-do-projeto");
  await user.click(within(edit).getByRole("button", { name: "Salvar" }));
  await waitFor(() => expect(store.values.get(`{"project":"${CT}"}:API_TOKEN`)).toBe("novo-do-projeto"));
  expect(api.hubCallsTo("hub.secrets.set")[1].params.scope).toEqual({ project: CT });

  // Remover do projeto: o global volta a valer.
  await user.click(within(sec).getByRole("button", { name: "Remover API_TOKEN" }));
  const box = within(sec).getByRole("group", { name: "Confirmar remoção de API_TOKEN" });
  expect(box).toHaveTextContent("o valor global volta a valer para o projeto");
  await user.click(within(box).getByRole("button", { name: "Remover API_TOKEN" }));
  await waitFor(() =>
    expect(api.hubCallsTo("hub.secrets.delete")[0].params).toEqual({ scope: { project: CT }, name: "API_TOKEN" }),
  );
  await waitFor(() =>
    expect(within(sec).getByText("API_TOKEN", { selector: "td" }).closest("tr")).toHaveTextContent("global · herdado"),
  );
});

test("secret com menos de 8 caracteres: aviso de que não é mascarado, sem bloquear", async () => {
  const { api, user } = setupGlobal();
  const sec = await screen.findByRole("region", { name: "Secrets e variáveis" });
  await user.click(within(sec).getByRole("button", { name: "+ Adicionar" }));
  const form = within(sec).getByRole("form", { name: "Novo secret ou variável" });
  await user.type(within(form).getByLabelText("Nome"), "PIN");
  const aviso = /menos de 8 caracteres não são mascarados na saída das sessões/;
  expect(within(form).queryByText(aviso)).not.toBeInTheDocument();
  await user.type(within(form).getByLabelText("Valor"), "1234567");
  expect(within(form).getByRole("status")).toHaveTextContent(aviso);
  // 8 bytes: mascarado, sem aviso (acentos contam em bytes, como no ambiente).
  await user.type(within(form).getByLabelText("Valor"), "8");
  expect(within(form).queryByText(aviso)).not.toBeInTheDocument();
  await user.clear(within(form).getByLabelText("Valor"));
  await user.type(within(form).getByLabelText("Valor"), "çãõé");
  expect(within(form).queryByText(aviso)).not.toBeInTheDocument();
  // Variável nunca é mascarada: sem aviso.
  await user.selectOptions(within(form).getByLabelText("Tipo"), "variable");
  await user.type(within(form).getByLabelText("Valor"), "abc");
  expect(within(form).queryByText(aviso)).not.toBeInTheDocument();
  await user.selectOptions(within(form).getByLabelText("Tipo"), "secret");
  await user.type(within(form).getByLabelText("Valor"), "abc");
  expect(within(form).getByRole("status")).toHaveTextContent(aviso);
  // Não bloqueia: grava assim mesmo.
  await user.click(within(form).getByRole("button", { name: "Salvar" }));
  await waitFor(() => expect(api.hubCallsTo("hub.secrets.set")[0].params).toMatchObject({ name: "PIN", value: "abc" }));
});

test("lista de secrets a rotacionar na origem depois da restauração, com Já rotacionei", async () => {
  const { api, user } = setupGlobal({
    restored_at: NOW - 3_600_000,
    items: [
      { scope: "global", name: "API_TOKEN" },
      { scope: { project: CT }, name: "DEPLOY_KEY" },
    ],
  });
  const sec = await screen.findByRole("region", { name: "Secrets e variáveis" });
  const box = await within(sec).findByRole("group", { name: "Secrets a rotacionar na origem" });
  const lista = within(box).getByRole("list", { name: "Secrets a rotacionar" });
  expect(within(lista).getByText("API_TOKEN").closest("li")).toHaveTextContent("global");
  expect(within(lista).getByText("DEPLOY_KEY").closest("li")).toHaveTextContent(CT);
  expect(box).toHaveTextContent("chave de recuperação");

  await user.click(within(box).getByRole("button", { name: "Já rotacionei" }));
  await waitFor(() => expect(api.hubCallsTo("hub.secrets.rotate_pending_dismiss")).toHaveLength(1));
  await waitFor(() => expect(within(sec).queryByRole("group", { name: "Secrets a rotacionar na origem" })).toBeNull());
});

test("sem lista a rotacionar, a seção não aparece", async () => {
  setupGlobal();
  const sec = await screen.findByRole("region", { name: "Secrets e variáveis" });
  await within(sec).findByRole("table", { name: "Secrets e variáveis globais" });
  expect(within(sec).queryByRole("group", { name: "Secrets a rotacionar na origem" })).toBeNull();
});
