// Tela Logs (#65): hub, ambiente e a própria UI com a mesma interface, os
// atalhos que levam a ela e o modo web.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import { logsTab } from "../maintenance/sections";
import type { Screen } from "../state/nav";
import {
  HUB_DASHBOARD,
  NO_HEALTH,
  NO_PROPOSALS,
  dashboard,
  hubEnv,
  hubInfo,
  hubStatus,
} from "../test/fixtures";
import { ENV, webApi } from "../test/web";
import type { LogLine, TailPage } from "./types";
import { uiLog } from "./uiLog";
import { resetFollowers } from "./useLogs";

const WSL = { ...FakeDaemonApi.wslEnv(), name: "wsl" };
const T0 = Date.UTC(2026, 9, 31, 14, 0, 0);

const line = (id: number, level: string, message: string, target = "cthubd::sync"): LogLine => ({
  id,
  ts: T0 + id,
  level,
  target,
  message,
});

const page = (lines: LogLine[], more = false): TailPage => ({ lines, more, file: "/home/u/.local/state/ctower/logs/cthubd.jsonl" });

beforeEach(() => {
  uiLog.reset();
  resetFollowers();
});

function desktop(initial?: Screen) {
  const api = new FakeDaemonApi([WSL])
    .on("dashboard.get", dashboard({ projects: [] }))
    .on("env.config.get", [])
    .on("logs.tail", page([line(10, "warn", "ambiente: aviso", "ct_server::hub_sync")]))
    .on("logs.follow", {})
    .on("logs.unfollow", {})
    .withHub()
    .onHub("hub.info", hubInfo())
    .onHub("hub.status", hubStatus())
    .onHub("hub.projects.list", [])
    .onHub("hub.envs.list", [hubEnv()])
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("hub.proposals.list", NO_PROPOSALS)
    .onHub("hub.health.get", NO_HEALTH)
    .onHub("hub.logs.tail", (p: { before?: number }) =>
      p.before ? page([line(1, "info", "bem antiga")]) : page([line(5, "info", "hub no ar"), line(6, "error", "sync falhou")], true),
    )
    .onHub("hub.logs.follow", {})
    .onHub("hub.logs.unfollow", {});
  render(<App api={api} initial={initial} />);
  return { api, user: userEvent.setup() };
}

const last = <T,>(a: T[]): T => a[a.length - 1];

const lines = () => within(screen.getByLabelText("Linhas do log")).queryAllByRole("listitem").map((li) => li.textContent);

test("hub: últimas linhas, anteriores sob demanda, seguir ao vivo e pausa", async () => {
  const { api, user } = desktop(logsTab({ kind: "hub" }).screen);
  expect(await screen.findByText("sync falhou")).toBeInTheDocument();
  expect(screen.getByRole("tab", { name: "Manutenção · Logs" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getByLabelText("Fonte do log")).toHaveValue("hub");
  expect(screen.getByText("/home/u/.local/state/ctower/logs/cthubd.jsonl")).toBeInTheDocument();
  expect(api.hubCallsTo("hub.logs.tail")[0].params).toEqual({ limit: 200 });

  // Modo seguir ligado por padrão: as linhas novas chegam pelo push.
  await waitFor(() => expect(api.hubCallsTo("hub.logs.follow")).toHaveLength(1));
  act(() => api.hubPush("hub.logs.lines", { lines: [line(7, "info", "nova linha")] }));
  expect(await screen.findByText("nova linha")).toBeInTheDocument();
  // O push do log não recarrega o hub.
  const reloads = api.hubCallsTo("hub.dashboard.get").length;

  // Pausa: guarda as novas e mostra quantas; ao despausar, entram.
  await user.click(screen.getByRole("button", { name: "Pausar" }));
  act(() => api.hubPush("hub.logs.lines", { lines: [line(8, "warn", "chegou pausado")] }));
  expect(await screen.findByRole("button", { name: "Pausado (1 novas)" })).toBeInTheDocument();
  expect(screen.queryByText("chegou pausado")).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Pausado (1 novas)" }));
  expect(await screen.findByText("chegou pausado")).toBeInTheDocument();
  await act(() => new Promise((r) => setTimeout(r, 600)));
  expect(api.hubCallsTo("hub.dashboard.get")).toHaveLength(reloads);

  // Anteriores: pede com `before` = id da mais antiga.
  await user.click(screen.getByRole("button", { name: "Carregar anteriores" }));
  expect(await screen.findByText("bem antiga")).toBeInTheDocument();
  expect(last(api.hubCallsTo("hub.logs.tail")).params).toEqual({ limit: 200, before: 5 });
  expect(lines()[0]).toContain("bem antiga");

  // Deixar de seguir desliga no hub.
  await user.click(screen.getByRole("button", { name: "Seguir" }));
  await waitFor(() => expect(api.hubCallsTo("hub.logs.unfollow")).toHaveLength(1));
});

test("filtro de nível e busca vão ao daemon; exportar e copiar", async () => {
  const { api, user } = desktop(logsTab({ kind: "hub" }).screen);
  await screen.findByText("sync falhou");
  await user.selectOptions(screen.getByLabelText("Nível do log"), "warn");
  await waitFor(() => expect(last(api.hubCallsTo("hub.logs.tail")).params).toEqual({ limit: 200, level: "warn" }));
  await user.type(screen.getByLabelText("Buscar no log"), "sync");
  await waitFor(() =>
    expect(last(api.hubCallsTo("hub.logs.tail")).params).toEqual({ limit: 200, level: "warn", q: "sync" }),
  );

  await user.click(await screen.findByRole("button", { name: "Exportar .jsonl" }));
  expect(api.saved).toHaveLength(1);
  expect(api.saved[0].name).toMatch(/^ctower-logs-hub-\d{8}-\d{4}\.jsonl$/);
  expect(api.saved[0].content.trim().split("\n").map((l) => JSON.parse(l).message)).toEqual(["hub no ar", "sync falhou"]);
  expect(await screen.findByText(/Exportado para \/home\/u\/Downloads\/ctower-logs-hub-/)).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Copiar" }));
  expect(api.clipboard).toContain("ERROR cthubd::sync: sync falhou");
});

test("ambiente: pelo detalhe do ambiente; desconectado mostra 'ambiente offline'", async () => {
  const { api, user } = desktop({ name: "env", env: "wsl" });
  await user.click(await screen.findByRole("button", { name: "Logs do ambiente" }));
  expect(await screen.findByText("ambiente: aviso")).toBeInTheDocument();
  expect(screen.getByLabelText("Fonte do log")).toHaveValue("env:wsl");
  await waitFor(() => expect(api.callsTo("logs.follow")).toHaveLength(1));
  act(() => api.push("wsl", "logs.lines", { lines: [line(11, "error", "ao vivo do ambiente")] }));
  expect(await screen.findByText("ao vivo do ambiente")).toBeInTheDocument();
  // Push de outro ambiente não entra.
  act(() => api.push("mac", "logs.lines", { lines: [line(12, "error", "de outro")] }));
  expect(screen.queryByText("de outro")).not.toBeInTheDocument();

  act(() => api.setStatus("wsl", false, "conexão recusada"));
  expect(await screen.findByRole("alert")).toHaveTextContent(/^Ambiente offline: wsl está sem conexão agora/);
});

test("hub: atalho na Visão geral; daemon sem a tela Logs pede atualização", async () => {
  const { api, user } = desktop({ name: "hub" });
  api.onHub("hub.logs.tail", () => {
    throw new Error("método desconhecido: hub.logs.tail");
  });
  await user.click(await screen.findByRole("button", { name: "Logs do hub" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("O hub (cthubd) ainda não tem a tela Logs");
});

test("UI: erros da própria UI desde que abriu, ao vivo, pela paleta", async () => {
  uiLog.add("error", "ui.react", "Erro ao renderizar a tela: boom");
  const { user } = desktop();
  await user.click(screen.getByRole("button", { name: "Abrir paleta de comandos" }));
  await user.type(within(screen.getByRole("dialog", { name: "Paleta de comandos" })).getByRole("combobox"), "Logs: esta UI");
  await user.keyboard("{Enter}");
  expect(await screen.findByText("Erro ao renderizar a tela: boom")).toBeInTheDocument();
  expect(screen.getByLabelText("Fonte do log")).toHaveValue("ui");
  act(() => {
    uiLog.add("warn", "ui.protocol", "wsl › logs.tail: falhou token=abc");
  });
  expect(await screen.findByText("wsl › logs.tail: falhou token=***")).toBeInTheDocument();
});

test("web: Logs sem a Manutenção; o hub segue ao vivo e o ambiente só com Atualizar", async () => {
  const api = webApi()
    .onHub("hub.logs.tail", page([line(5, "info", "hub pela web")]))
    .onHub("hub.logs.follow", {})
    .on("logs.tail", page([line(9, "warn", "ambiente pela web", "ct_server::hub_sync")]));
  render(<App api={api} initial={logsTab({ kind: "hub" }).screen} />);
  const user = userEvent.setup();
  expect(await screen.findByText("hub pela web")).toBeInTheDocument();
  expect(screen.getByRole("heading", { level: 1, name: "Logs" })).toBeInTheDocument();
  await waitFor(() => expect(api.hubCallsTo("hub.logs.follow")).toHaveLength(1));

  await user.selectOptions(screen.getByLabelText("Fonte do log"), `env:${ENV}`);
  expect(await screen.findByText("ambiente pela web")).toBeInTheDocument();
  expect(last(api.callsTo("logs.tail")).env).toBe(ENV);
  const seguir = screen.getByRole("button", { name: "Seguir" });
  expect(seguir).toBeDisabled();
  expect(seguir).toHaveAccessibleDescription(/fica no app desktop/);
  expect(api.callsTo("logs.follow")).toHaveLength(0);

  // Exportar na web: o navegador baixa (sem caminho).
  await user.click(screen.getByRole("button", { name: "Exportar .log" }));
  expect(api.saved[0].content).toContain("WARN  ct_server::hub_sync: ambiente pela web");
  expect(await screen.findByText(/Exportado: ctower-logs-/)).toBeInTheDocument();

  // Ambiente sem canal com o hub: "ambiente offline".
  api.offline.add(ENV);
  await user.click(screen.getByRole("button", { name: "Atualizar" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(/^Ambiente offline/);
});
