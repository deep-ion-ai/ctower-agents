import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FakeDaemonApi } from "../api/fake";
import type { HubRuleVersion, HubRules, HubRulesProposal } from "../protocol/hub-gen";
import { TowerProvider } from "../state/tower";
import { dashboard } from "../test/fixtures";
import { AgentRules, sizeState, sourceText, utf8Bytes } from "./AgentRules";

const NOW = Date.now();
const MIN = 60_000;

function version(p: Partial<HubRuleVersion> = {}): HubRuleVersion {
  const content = p.content ?? "Branch exatamente PROJ-NNNN.";
  return {
    version: 1,
    content,
    bytes: utf8Bytes(content),
    source: "usuario",
    proposal_id: null,
    author_env_id: null,
    author_env_name: null,
    session_id: null,
    note: null,
    created_at: NOW - 30 * MIN,
    ...p,
  };
}

function rules(current: HubRuleVersion | null, p: Partial<HubRules> = {}): HubRules {
  return {
    owner: "grupo/projeto",
    repo: "app",
    epic_number: 0,
    label: "app",
    current,
    pending: 0,
    max_bytes: 16384,
    warn_bytes: 12288,
    ...p,
  };
}

function proposal(p: Partial<HubRulesProposal> = {}): HubRulesProposal {
  return {
    id: 3,
    uid: "u-3",
    owner: "grupo/projeto",
    repo: "app",
    epic_number: 5014,
    label: "PROJ-5014",
    base_version: 1,
    current_version: 2,
    current_content: "Só toque o openapi.yaml.\nAvise se for só frontend.",
    base_changed: true,
    content: "Só toque o openapi.yaml.\nE o CHANGELOG.md.\nAvise se for só frontend.",
    reason: "o bot recusou o MR sem o CHANGELOG",
    state: "pendente",
    env_id: "daemon-wsl",
    env_name: "wsl-pc",
    session_id: "abcdef0123456789",
    created_at: NOW - 5 * MIN,
    decided_at: null,
    decision_note: null,
    applied_version: null,
    ...p,
  };
}

function setup(opts: {
  epic?: number;
  label?: string;
  current?: HubRuleVersion | null;
  proposals?: HubRulesProposal[];
  web?: boolean;
}) {
  const epic = opts.epic ?? 0;
  let current = opts.current === undefined ? version() : opts.current;
  let proposals = opts.proposals ?? [];
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard())
    .withHub("ws://mac-mini:7879/ws", true)
    .onHub("hub.rules.get", () => rules(current, { epic_number: epic, label: opts.label ?? "app" }))
    .onHub("hub.rules.proposals.list", () => ({ proposals, truncated: false }))
    .onHub("hub.rules.set", (p: { content: string; base_version: number }) => {
      current = version({ version: p.base_version + 1, content: p.content });
      return current;
    })
    .onHub("hub.rules.import", (p: { content: string; base_version: number }) => {
      current = version({ version: p.base_version + 1, content: p.content, source: "importacao" });
      return current;
    })
    .onHub("hub.rules.history", () => ({
      versions: [
        version({ version: 3, source: "proposta", proposal_id: 2, author_env_name: "wsl-pc", note: "o bot pediu" }),
        version({ version: 2, source: "usuario", content: "editado" }),
        version({ version: 1, source: "importacao", content: "# CLAUDE.md" }),
      ],
      truncated: false,
    }))
    .onHub("hub.rules.proposals.decide", (p: { id: number; approve: boolean }) => {
      proposals = [];
      return proposal({ id: p.id, state: p.approve ? "aprovada" : "rejeitada", applied_version: p.approve ? 3 : null });
    });
  if (opts.web) api.asWeb();
  render(
    <TowerProvider api={api}>
      <AgentRules
        owner="grupo/projeto"
        repo="app"
        epic={epic}
        label={opts.label ?? "app"}
        now={NOW}
        className="hub-card"
      />
    </TowerProvider>,
  );
  return { api, user: userEvent.setup() };
}

test("tamanho: aviso a partir de 12 KiB e recusa acima de 16 KiB", () => {
  expect(sizeState(100, 12288, 16384)).toBe("ok");
  expect(sizeState(12288, 12288, 16384)).toBe("warn");
  expect(sizeState(16384, 12288, 16384)).toBe("warn");
  expect(sizeState(16385, 12288, 16384)).toBe("over");
  expect(utf8Bytes("é")).toBe(2);
  expect(sourceText(version({ source: "proposta", proposal_id: 9, author_env_name: "wsl-pc" }))).toBe(
    "proposta #9 do agente (wsl-pc), aprovada pelo usuário",
  );
});

test("editor: mostra a vigente, edita em Markdown com contador e grava com a versão base", async () => {
  const { api, user } = setup({});
  const card = await screen.findByRole("region", { name: /Regras do agente do projeto · versão 1/ });
  expect(within(card).getByLabelText("Texto de regras do projeto")).toHaveTextContent(
    "Branch exatamente PROJ-NNNN.",
  );
  expect(within(card).getByText(/edição do usuário/)).toBeInTheDocument();
  await user.click(within(card).getByRole("button", { name: "Editar" }));
  const form = within(card).getByRole("form", { name: "Editar regras do projeto" });
  const area = within(form).getByLabelText("Regras em Markdown");
  await user.clear(area);
  await user.paste("Commit [PROJ-NNNN] título, sem corpo.");
  expect(within(form).getByText(/^38 de 16384 bytes/)).toBeInTheDocument();
  await user.type(within(form).getByLabelText("Nota da versão (opcional)"), "formato do exemplo");
  await user.click(within(form).getByRole("button", { name: "Gravar versão 2" }));
  await waitFor(() => expect(api.hubCallsTo("hub.rules.set")).toHaveLength(1));
  expect(api.hubCallsTo("hub.rules.set")[0].params).toEqual({
    owner: "grupo/projeto",
    repo: "app",
    epic_number: 0,
    content: "Commit [PROJ-NNNN] título, sem corpo.",
    base_version: 1,
    note: "formato do exemplo",
  });
  expect(await within(card).findByText(/Versão 2 gravada/)).toBeInTheDocument();
});

test("editor: perto do limite avisa; acima, desabilita gravar", async () => {
  const { user } = setup({ current: null });
  const card = await screen.findByRole("region", { name: /Regras do agente do projeto/ });
  expect(within(card).getByText(/Sem regras do projeto/)).toBeInTheDocument();
  await user.click(within(card).getByRole("button", { name: "Escrever regras" }));
  const form = within(card).getByRole("form");
  const area = within(form).getByLabelText("Regras em Markdown");
  await user.click(area);
  await user.paste("a".repeat(12_300));
  expect(within(form).getByText(/perto do limite de 16 KiB/)).toBeInTheDocument();
  expect(within(form).getByRole("button", { name: "Gravar versão 1" })).toBeEnabled();
  await user.paste("a".repeat(4_100));
  expect(within(form).getByText(/acima do limite de 16 KiB/)).toBeInTheDocument();
  expect(within(form).getByRole("button", { name: "Gravar versão 1" })).toBeDisabled();
});

test("importar CLAUDE.md: o arquivo vai ao editor e grava como importação", async () => {
  const { api, user } = setup({ current: null, epic: 5014, label: "PROJ-5014" });
  const card = await screen.findByRole("region", { name: /Regras do agente do épico PROJ-5014/ });
  await within(card).findByText(/Sem regras do épico/);
  const file = new File(["# PROJ-5014\n- Só o openapi.yaml.\n"], "CLAUDE.md", { type: "text/markdown" });
  await user.upload(within(card).getByLabelText("Importar CLAUDE.md para regras do épico PROJ-5014"), file);
  const form = await within(card).findByRole("form");
  expect(within(form).getByLabelText("Regras em Markdown")).toHaveValue("# PROJ-5014\n- Só o openapi.yaml.\n");
  expect(within(form).getByRole("checkbox", { name: /Importação de um CLAUDE.md/ })).toBeChecked();
  await user.click(within(form).getByRole("button", { name: "Gravar versão 1" }));
  await waitFor(() => expect(api.hubCallsTo("hub.rules.import")).toHaveLength(1));
  expect(api.hubCallsTo("hub.rules.import")[0].params).toMatchObject({ epic_number: 5014, base_version: 0 });
  expect(api.hubCallsTo("hub.rules.set")).toHaveLength(0);
  expect(api.hubCallsTo("hub.rules.get")[0].params).toEqual({ owner: "grupo/projeto", repo: "app", epic_number: 5014 });
});

test("histórico: lê ao abrir e mostra a origem de cada versão", async () => {
  const { api, user } = setup({});
  const card = await screen.findByRole("region", { name: /Regras do agente do projeto/ });
  expect(api.hubCallsTo("hub.rules.history")).toHaveLength(0);
  await user.click(within(card).getByText("Histórico"));
  const list = await within(card).findByRole("list", { name: "Histórico de regras do projeto" });
  const items = within(list).getAllByRole("listitem");
  expect(items).toHaveLength(3);
  expect(items[0]).toHaveTextContent("v3 proposta #2 do agente (wsl-pc), aprovada pelo usuário");
  expect(items[0]).toHaveTextContent("o bot pediu");
  expect(items[1]).toHaveTextContent("v2 edição do usuário");
  expect(items[2]).toHaveTextContent("v1 importação de CLAUDE.md");
});

test("proposta pendente: diferença contra a vigente, aviso de base antiga e aprovar", async () => {
  const { api, user } = setup({
    epic: 5014,
    label: "PROJ-5014",
    current: version({ version: 2, content: "Só toque o openapi.yaml.\nAvise se for só frontend." }),
    proposals: [proposal()],
  });
  const card = await screen.findByRole("region", { name: /épico PROJ-5014 · versão 2 · 1 proposta pendente/ });
  const p = within(card).getByRole("listitem", { name: "Proposta #3" });
  expect(p).toHaveTextContent("o bot recusou o MR sem o CHANGELOG");
  expect(p).toHaveTextContent("do agente em wsl-pc");
  expect(within(p).getByRole("note")).toHaveTextContent("Feita sobre a versão 1; a vigente é a 2.");
  expect(p).toHaveTextContent("+1 −0 linhas");
  const diff = within(p).getByLabelText("Diferença da proposta #3");
  const added = diff.querySelectorAll('[data-op="add"]');
  expect(added).toHaveLength(1);
  expect(added[0]).toHaveTextContent("+ E o CHANGELOG.md.");
  expect(diff.querySelectorAll('[data-op="same"]')).toHaveLength(2);
  await user.click(within(p).getByRole("button", { name: "Aprovar proposta #3" }));
  await waitFor(() => expect(api.hubCallsTo("hub.rules.proposals.decide")).toHaveLength(1));
  expect(api.hubCallsTo("hub.rules.proposals.decide")[0].params).toEqual({ id: 3, approve: true });
  expect(await within(card).findByText("Proposta #3 aprovada: versão 3 em vigor.")).toBeInTheDocument();
  expect(within(card).queryByRole("listitem", { name: "Proposta #3" })).not.toBeInTheDocument();
});

test("nas regras do projeto, avisa das propostas pendentes de épicos; o push relê", async () => {
  const { api } = setup({ proposals: [proposal()] });
  const card = await screen.findByRole("region", { name: /Regras do agente do projeto/ });
  expect(await within(card).findByText(/Propostas pendentes nas regras de épicos: PROJ-5014/)).toBeInTheDocument();
  expect(within(card).queryByRole("listitem", { name: "Proposta #3" })).not.toBeInTheDocument();
  const before = api.hubCallsTo("hub.rules.get").length;
  api.hubPush("hub.rules.changed");
  await waitFor(() => expect(api.hubCallsTo("hub.rules.get").length).toBe(before + 1));
});

test("web: regras e propostas só para leitura; editar, importar e decidir desabilitados com o motivo", async () => {
  setup({ epic: 5014, label: "PROJ-5014", proposals: [proposal()], web: true });
  const card = await screen.findByRole("region", { name: /Regras do agente/ });
  expect(await within(card).findByText("o bot recusou o MR sem o CHANGELOG")).toBeInTheDocument();
  expect(within(card).getByText(/Branch exatamente PROJ-NNNN\./)).toBeInTheDocument();
  // #26: os botões ficam, desabilitados, e não há campo de arquivo para importar.
  expect(within(card).getByRole("button", { name: "Editar" })).toBeDisabled();
  expect(within(card).getByRole("button", { name: "Importar CLAUDE.md" })).toBeDisabled();
  expect(within(card).queryByLabelText(/Importar CLAUDE.md para/)).toBeNull();
  const aprovar = within(card).getByRole("button", { name: /Aprovar proposta/ });
  expect(aprovar).toBeDisabled();
  expect(aprovar).toHaveAccessibleDescription("Decidir propostas fica no app desktop");
  expect(within(card).getByRole("button", { name: /Rejeitar proposta/ })).toBeDisabled();
});
