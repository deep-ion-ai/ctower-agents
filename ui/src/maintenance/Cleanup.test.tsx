import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { CleanupItem, CleanupPlan, CleanupRun, CleanupStatus, StepPlan } from "../protocol/gen";
import type { Screen } from "../state/nav";
import { HUB_CONFIG_UNLINKED, dashboard, project } from "../test/fixtures";
import { itemReason, mergeRuns, pendingText } from "./Cleanup";

const GB = 1024 ** 3;

function step(kind: StepPlan["kind"], state: StepPlan["state"], p: Partial<StepPlan> = {}): StepPlan {
  return { kind, state, reason: null, bytes: 0, targets: [], ...p };
}

function item(n: number, steps: StepPlan[], p: Partial<CleanupItem> = {}): CleanupItem {
  return {
    work_item_id: 100 + n,
    project_id: 1,
    owner: "deep-ion-ai",
    repo: "ctower-agents",
    gh_number: n,
    title: `Issue ${n}`,
    branch: `feat/${n}-x`,
    worktree_path: `/w/feat__${n}-x`,
    completion: { signal: "pr_mesclado", since: 1, pr: 9 },
    last_activity_at: null,
    steps,
    live_sessions: [],
    ...p,
  };
}

function plan(items: CleanupItem[]): CleanupPlan {
  const pending = items.filter((i) => i.steps.some((s) => s.state === "pendente"));
  return {
    checked_at: Date.now() - 5 * 60_000,
    items,
    pending_items: pending.length,
    pending_bytes: pending.flatMap((i) => i.steps).reduce((n, s) => n + (s.state === "pendente" ? s.bytes : 0), 0),
  };
}

const PLANO = plan([
  item(251, [
    step("sessoes", "feita"),
    step("worktree", "pendente", { bytes: 6 * GB, targets: ["/w/feat__251-x"] }),
    step("branch_local", "bloqueada", { reason: "aguarda a etapa worktree" }),
  ]),
  item(263, [step("worktree", "pendente", { bytes: GB })]),
  item(274, [step("sessoes", "bloqueada", { reason: "sessão viva (issue concluída, sessão ainda aberta)" })], {
    live_sessions: ["sess-274"],
  }),
]);

function status(p: CleanupPlan | null, s: Partial<CleanupStatus> = {}): CleanupStatus {
  return {
    config: {
      check_interval_hours: 6,
      auto: false,
      min_age_hours: 24,
      min_idle_hours: 24,
      end_idle_sessions_hours: 0,
      remove_agent_transcripts: true,
      target_idle_days: 14,
    },
    plan: p,
    check_error: null,
    last_run: null,
    busy: false,
    ...s,
  };
}

const RUN: CleanupRun = {
  started_at: 1,
  finished_at: 2,
  dry_run: false,
  scope: { work_item_id: 351, step: null },
  freed_bytes: 6 * GB,
  items: [
    {
      work_item_id: 351,
      owner: "deep-ion-ai",
      repo: "ctower-agents",
      gh_number: 251,
      steps: [
        { kind: "worktree", result: "feita", reason: null, bytes: 6 * GB, removed: ["/w/feat__251-x"], kept: [] },
        { kind: "branch_local", result: "bloqueada", reason: "aguarda", bytes: 0, removed: [], kept: [] },
      ],
    },
  ],
};

function setup(st: CleanupStatus | null, initial: Screen = { name: "maintenance", section: "cleanup" }) {
  let current = st;
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard())
    .on("env.info", {
      protocol: "2.1",
      version: "0.8.0",
      commit: "",
      hostname: "wsl",
      worktrees_root: "/w",
      env_id: "id-wsl",
      env_name: "wsl",
      hub_sync: null,
      hub_config: HUB_CONFIG_UNLINKED,
      update: null,
    })
    .on("github.status", { source: "gh", login: "x", error: null })
    .on("cleanup.status", () => {
      if (!current) throw new Error("método desconhecido: cleanup.status");
      return current;
    })
    .on("cleanup.plan", () => current?.plan)
    .on("cleanup.run", () => RUN)
    .on("session.kill", {});
  render(<App api={api} initial={initial} />);
  return {
    api,
    user: userEvent.setup(),
    set: (s: CleanupStatus) => (current = s),
  };
}

const row = () => screen.getByRole("group", { name: "Limpeza de ctower-agents (WSL)" });

test("texto do indicador", () => {
  expect(pendingText(0, 0)).toBe("nenhuma limpeza pendente");
  expect(pendingText(1, GB)).toBe("1 limpeza pendente · 1,0 GB");
  expect(pendingText(2, 7 * GB)).toBe("2 limpezas pendentes · 7,0 GB");
});

test("motivo do item: conclusão ou worktree inativo em andamento", () => {
  const now = Date.now();
  expect(itemReason(item(1, []), now)).toBe("PR mesclado");
  const inativo = item(2, [step("target", "pendente", { bytes: GB })], {
    completion: null,
    last_activity_at: now - 15 * 24 * 3_600_000,
  });
  expect(itemReason(inativo, now)).toBe("em andamento, worktree inativo há 15 dia(s)");
  expect(itemReason({ ...inativo, last_activity_at: null }, now)).toBe("em andamento, worktree inativo");
});

test("mostra a contagem, o espaço e a lista por work item com os bloqueios", async () => {
  setup(status(PLANO));
  const r = await waitFor(() => row());
  expect(await within(r).findByText("2 limpezas pendentes · 7,0 GB")).toBeInTheDocument();
  expect(within(r).getByText("há 5 min")).toBeInTheDocument();
  expect(within(r).getByText("desligada")).toBeInTheDocument();
  const pend = within(r).getByRole("list", { name: "Limpezas pendentes em ctower-agents (WSL)" });
  expect(within(pend).getByText("deep-ion-ai/ctower-agents#251", { selector: ".mono" })).toBeInTheDocument();
  expect(within(pend).getByText("worktree: 6,0 GB")).toBeInTheDocument();
  expect(within(pend).getByText("branch local: aguarda a etapa worktree")).toBeInTheDocument();
  expect(within(pend).queryByText(/sessões/)).not.toBeInTheDocument();
  const bloq = within(r).getByRole("list", { name: "Limpezas bloqueadas em ctower-agents (WSL)" });
  expect(within(bloq).getByText("sessões: sessão viva (issue concluída, sessão ainda aberta)")).toBeInTheDocument();
  expect(within(r).getByText("1 concluído(s) com etapas bloqueadas")).toBeInTheDocument();
});

test("Verificar agora recalcula e atualiza a lista", async () => {
  const { api, user, set } = setup(status(null));
  const r = await waitFor(() => row());
  expect(await within(r).findByText("ainda não verificado")).toBeInTheDocument();
  expect(within(r).getByRole("button", { name: /^Limpar tudo/ })).toBeDisabled();
  set(status(PLANO));
  await user.click(within(r).getByRole("button", { name: /Verificar agora/ }));
  expect(await within(r).findByText("2 limpezas pendentes · 7,0 GB")).toBeInTheDocument();
  expect(api.callsTo("cleanup.plan")).toHaveLength(1);
  expect(api.callsTo("cleanup.run")).toHaveLength(0);
});

test("Limpar um item confirma, executa só ele e mostra o espaço liberado", async () => {
  const { api, user, set } = setup(status(PLANO));
  const r = await waitFor(() => row());
  await user.click(await within(r).findByRole("button", { name: "Limpar deep-ion-ai/ctower-agents#251" }));
  const box = within(r).getByRole("group", { name: "Confirmar limpeza" });
  expect(box).toHaveTextContent("Limpar deep-ion-ai/ctower-agents#251? Saem as etapas pendentes (6,0 GB).");
  set(status(plan([PLANO.items[1], PLANO.items[2]])));
  await user.click(within(box).getByRole("button", { name: "Limpar" }));
  await waitFor(() =>
    expect(api.callsTo("cleanup.run")).toEqual([{ env: "wsl", method: "cleanup.run", params: { work_item_id: 351 } }]),
  );
  expect(await within(r).findByText(/1 etapa\(s\) feita\(s\), 1 bloqueada\(s\), 0 com falha · liberado: 6,0 GB/)).toBeInTheDocument();
  expect(await within(r).findByText("1 limpeza pendente · 1,0 GB")).toBeInTheDocument();
});

test("Limpar tudo executa o plano inteiro do ambiente", async () => {
  const { api, user } = setup(status(PLANO));
  const r = await waitFor(() => row());
  await user.click(await within(r).findByRole("button", { name: /^Limpar tudo/ }));
  const box = within(r).getByRole("group", { name: "Confirmar limpeza" });
  expect(box).toHaveTextContent("Limpar 2 work item(s) em");
  await user.click(within(box).getByRole("button", { name: "Limpar" }));
  await waitFor(() => expect(api.callsTo("cleanup.run")).toEqual([{ env: "wsl", method: "cleanup.run", params: {} }]));
});

test("push cleanup.status atualiza o indicador; daemon antigo fica sem suporte", async () => {
  const { api } = setup(status(plan([])));
  const r = await waitFor(() => row());
  expect(await within(r).findByText("nenhuma limpeza pendente")).toBeInTheDocument();
  act(() => api.push("wsl", "cleanup.status", status(PLANO)));
  expect(within(r).getByText("2 limpezas pendentes · 7,0 GB")).toBeInTheDocument();
});

test("daemon sem cleanup.status: sem suporte e botões desligados", async () => {
  setup(null);
  const r = await waitFor(() => row());
  expect(await within(r).findByText("daemon sem suporte a limpeza")).toBeInTheDocument();
  expect(within(r).getByRole("button", { name: /Verificar agora/ })).toBeDisabled();
});

test("sessão aberta de issue concluída: aviso e encerramento explícito com confirmação", async () => {
  const { api, user, set } = setup(status(PLANO));
  const r = await waitFor(() => row());
  const open = await within(r).findByRole("list", { name: "Sessões abertas de issues concluídas em ctower-agents (WSL)" });
  expect(within(open).getByText("issue concluída, sessão ainda aberta")).toBeInTheDocument();
  expect(within(open).getByText("deep-ion-ai/ctower-agents#274")).toBeInTheDocument();
  await user.click(within(open).getByRole("button", { name: "Encerrar sessão sess-274" }));
  const box = within(open).getByRole("group", { name: "Confirmar encerramento" });
  // Cancelar não encerra nada.
  await user.click(within(box).getByRole("button", { name: "Cancelar" }));
  expect(api.callsTo("session.kill")).toHaveLength(0);
  await user.click(within(open).getByRole("button", { name: "Encerrar sessão sess-274" }));
  set(status(plan([PLANO.items[0], PLANO.items[1]])));
  await user.click(within(r).getByRole("button", { name: "Confirmar" }));
  await waitFor(() =>
    expect(api.callsTo("session.kill")).toEqual([{ env: "wsl", method: "session.kill", params: { session_id: "sess-274" } }]),
  );
  expect(api.callsTo("cleanup.run")).toHaveLength(0);
  await waitFor(() => expect(within(r).queryByRole("list", { name: /Sessões abertas/ })).not.toBeInTheDocument());
});

test("as regras ficam na própria Limpeza (#22, antes nas Configurações), só leitura", async () => {
  setup(status(PLANO));
  await waitFor(() => row());
  const regras = await screen.findByRole("table", { name: "Regras de limpeza" });
  const linha = (k: string) => within(regras).getByText(k).closest("tr")!;
  expect(await within(linha("check_interval_hours")).findByText("6 h")).toBeInTheDocument();
  expect(within(linha("auto")).getByText("desligado")).toBeInTheDocument();
  expect(within(linha("end_idle_sessions_hours")).getByText("desligado")).toBeInTheDocument();
  expect(within(linha("target_idle_days")).getByText("14 dia(s)")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Ajustes de limpeza ›" })).not.toBeInTheDocument();
});

/** Plano com itens de dois projetos: o ctower-agents (PLANO) e o outro-repo. */
const MISTO = plan([
  ...PLANO.items,
  item(5, [step("worktree", "pendente", { bytes: 2 * GB })], { work_item_id: 905, project_id: 2, repo: "outro-repo" }),
  item(6, [step("target", "pendente", { bytes: GB })], { work_item_id: 906, project_id: 2, repo: "outro-repo" }),
]);
const OUTRO: Screen = {
  name: "maintenance",
  section: "cleanup",
  project: { owner: "deep-ion-ai", repo: "outro-repo" },
};

test("filtro por projeto (#22): só os itens dele, com a contagem e o espaço deles", async () => {
  const { user } = setup(status(MISTO));
  const r = await waitFor(() => row());
  expect(await within(r).findByText("4 limpezas pendentes · 10,0 GB")).toBeInTheDocument();
  const filtro = screen.getByLabelText("Projeto da limpeza");
  expect(filtro).toHaveValue("");

  await user.selectOptions(filtro, "deep-ion-ai/ctower-agents");
  expect(screen.getAllByRole("tab", { name: "Manutenção · Limpeza" })).toHaveLength(1);
  expect(screen.getByRole("navigation", { name: "Trilha" })).toHaveTextContent("ManutençãoLimpezactower-agents (WSL)ctower-agents");
  const r2 = await waitFor(() => row());
  expect(await within(r2).findByText("2 limpezas pendentes · 7,0 GB")).toBeInTheDocument();
  const pend = within(r2).getByRole("list", { name: "Limpezas pendentes em ctower-agents (WSL)" });
  expect(within(pend).queryByText(/outro-repo/)).not.toBeInTheDocument();
  // As regras valem para o ambiente inteiro: seguem lá com o filtro.
  expect(screen.getByRole("table", { name: "Regras de limpeza" })).toBeInTheDocument();

  await user.selectOptions(screen.getByLabelText("Projeto da limpeza"), "");
  expect(await within(row()).findByText("4 limpezas pendentes · 10,0 GB")).toBeInTheDocument();
});

test("filtro pelo endereço: projeto fora do ambiente entra no seletor; Limpar tudo limpa item a item", async () => {
  const { api, user } = setup(status(MISTO), OUTRO);
  const r = await waitFor(() => row());
  expect(await within(r).findByText("2 limpezas pendentes · 3,0 GB")).toBeInTheDocument();
  expect(screen.getByLabelText("Projeto da limpeza")).toHaveValue("deep-ion-ai/outro-repo");
  expect(within(r).queryByRole("list", { name: /Sessões abertas/ })).not.toBeInTheDocument();
  await user.click(within(r).getByRole("button", { name: /^Limpar tudo de deep-ion-ai\/outro-repo/ }));
  const box = within(r).getByRole("group", { name: "Confirmar limpeza" });
  expect(box).toHaveTextContent("Limpar 2 work item(s) de deep-ion-ai/outro-repo em ctower-agents (WSL)? Saem as etapas pendentes (3,0 GB).");
  await user.click(within(box).getByRole("button", { name: "Limpar" }));
  // O daemon só limpa o plano inteiro ou um item: um `cleanup.run` por item do projeto.
  await waitFor(() =>
    expect(api.callsTo("cleanup.run").map((c) => c.params)).toEqual([{ work_item_id: 905 }, { work_item_id: 906 }]),
  );
  expect(await within(r).findByText(/2 etapa\(s\) feita\(s\), 2 bloqueada\(s\), 0 com falha · liberado: 12,0 GB/)).toBeInTheDocument();
});

test("mergeRuns soma as execuções por item", () => {
  expect(mergeRuns([])).toBeNull();
  const m = mergeRuns([RUN, { ...RUN, finished_at: 9, freed_bytes: GB }])!;
  expect(m.items).toHaveLength(2);
  expect(m.freed_bytes).toBe(7 * GB);
  expect(m.finished_at).toBe(9);
});

test("o que ficou fora do hub aparece na Limpeza, para limpar à mão", async () => {
  const NOW = Date.now();
  const { api } = setup(status(plan([])));
  api.on("projects.list", (p: { unassigned?: boolean }) =>
    p.unassigned
      ? [
          project({ id: 7, name: "legado", gh_repo: "legado", repo_path: "/p/legado" }),
          project({ id: 8, name: "antigo", gh_repo: "antigo", repo_path: "/p/antigo", config_rev: 5, unassigned_at: NOW - 2 * 86_400_000 }),
        ]
      : [],
  );
  // Configuração nova do ambiente: a lista é relida.
  api.on("env.info", {
    protocol: "2.1",
    version: "0.8.0",
    commit: "",
    hostname: "wsl",
    worktrees_root: "/w",
    env_id: "id-wsl",
    env_name: "wsl",
    hub_sync: null,
    hub_config: { ...HUB_CONFIG_UNLINKED, linked: true, config_rev: 42, pulled_at: NOW },
    update: null,
  });
  act(() => api.push("wsl", "session_started"));
  const fora = await screen.findByRole("table", { name: "Projetos fora do hub" });
  const legado = within(fora).getByText("deep-ion-ai/legado").closest("tr")!;
  expect(within(legado).getByText("projeto local fora do hub")).toBeInTheDocument();
  expect(within(legado).getByText("/p/legado")).toBeInTheDocument();
  const antigo = within(fora).getByText("deep-ion-ai/antigo").closest("tr")!;
  expect(within(antigo).getByText("atribuição removida há 2 d")).toBeInTheDocument();
  expect(api.callsTo("projects.list").some((c) => c.params.unassigned === true)).toBe(true);
});

test("nada fora do hub: a seção diz que não há o que limpar à mão", async () => {
  setup(status(plan([])));
  expect(await screen.findByText("Nada fora do hub neste ambiente.")).toBeInTheDocument();
});
