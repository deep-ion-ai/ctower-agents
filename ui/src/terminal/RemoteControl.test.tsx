import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { RemoteControlStatus } from "../protocol/gen";
import { dashboard, HUB_CONFIG_UNLINKED, MIN, session, sync, workItem } from "../test/fixtures";
import { RC_CONNECT_MS, rcOn, rcView } from "./remote";
import { QrCode } from "./RemoteControl";

// Remote Control do Claude Code nas sessões (#197): cabeçalho do painel,
// árvore, menu de contexto, barra de status e o link com QR code.

const NOW = Date.now();
const S = session({ id: "cccccccc-0000-0000-0000-000000000040", work_item_id: 40, started_at: NOW - 10 * MIN });
const WI = workItem({ id: 40, gh_number: 40, title: "remote control", branch: "feat/40-remote-control" });
const LINK = "https://claude.ai/code/session_01Abc";

const statusBar = () => screen.getByRole("contentinfo", { name: "Barra de status" });
const pane = () => screen.getByRole("region", { name: "Sessão" });
const row = () => screen.getByRole("treeitem", { name: /#40 remote control/ });

function info(rc: RemoteControlStatus = { available: true, reason: null }) {
  return {
    protocol: "2.3",
    version: "0.1.0",
    commit: "",
    hostname: "ct",
    worktrees_root: "/w",
    env_id: "daemon-wsl",
    env_name: "wsl",
    hub_sync: null,
    hub_config: HUB_CONFIG_UNLINKED,
    update: null,
    remote_control: rc,
  };
}

async function openTab(rc?: RemoteControlStatus, s = S) {
  const api = new FakeDaemonApi()
    .on("env.info", info(rc))
    .on("dashboard.get", dashboard({ live_sessions: [s], sync: [sync({ last_ok_at: NOW })] }, [WI]))
    .on("session.peek", { data: "" })
    .on("session.remote_control.enable", { requested: true, rc_session_id: null });
  render(<App api={api} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: /remote control/ }));
  await waitFor(() => expect(api.attached.has(s.id)).toBe(true));
  return { api, user };
}

test("Ligar Remote Control no cabeçalho: pede ao daemon, mostra conectando e, com o id, o link, o QR code e os indicadores", async () => {
  const { api, user } = await openTab();
  expect(within(row()).queryByRole("img", { name: "Remote Control conectado" })).not.toBeInTheDocument();
  await user.click(await within(pane()).findByRole("button", { name: "Ligar Remote Control" }));
  expect(api.callsTo("session.remote_control.enable")[0].params).toEqual({ session_id: S.id });
  expect(within(pane()).getByRole("status")).toHaveTextContent("Remote Control · conectando…");

  // O id chega pelo hook: push e dashboard novo.
  api.on("dashboard.get", dashboard({ live_sessions: [{ ...S, rc_session_id: "session_01Abc" }], sync: [] }, [WI]));
  act(() => api.push("wsl", "session_remote_control", { session_id: S.id, rc_session_id: "session_01Abc" }));
  const painel = await screen.findByRole("dialog", { name: "Remote Control" });
  expect(within(painel).getByText(LINK)).toBeInTheDocument();
  expect(within(painel).getByRole("img", { name: `QR code do link ${LINK}` })).toBeInTheDocument();
  await user.click(within(painel).getByRole("button", { name: "Copiar link" }));
  expect(api.clipboard).toBe(LINK);
  expect(within(painel).getByRole("button", { name: "Link copiado" })).toBeInTheDocument();
  await user.click(within(painel).getByRole("button", { name: "Abrir no navegador" }));
  expect(api.opened).toEqual([LINK]);
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog", { name: "Remote Control" })).not.toBeInTheDocument();

  // Ligado: selo no cabeçalho (reabre o painel), ícone na árvore e item na barra de status.
  expect(within(row()).getByRole("img", { name: "Remote Control conectado" })).toBeInTheDocument();
  expect(within(pane()).queryByRole("button", { name: "Ligar Remote Control" })).not.toBeInTheDocument();
  await user.click(within(statusBar()).getByRole("button", { name: /Remote Control/ }));
  expect(api.opened).toEqual([LINK, LINK]);
  await user.click(within(pane()).getByRole("button", { name: "Remote Control conectado: mostrar link e QR code" }));
  expect(await screen.findByRole("dialog", { name: "Remote Control" })).toBeInTheDocument();
});

test("recusa do daemon aparece no cabeçalho e o botão volta", async () => {
  const { api, user } = await openTab();
  api.on("session.remote_control.enable", () => {
    throw "Remote Control indisponível: ANTHROPIC_API_KEY definida (secret do projeto o/r)";
  });
  await user.click(await within(pane()).findByRole("button", { name: "Ligar Remote Control" }));
  expect(await within(pane()).findByRole("alert")).toHaveTextContent("ANTHROPIC_API_KEY definida (secret do projeto o/r)");
  expect(within(pane()).getByRole("button", { name: "Ligar Remote Control" })).toBeEnabled();
});

test("indisponível no ambiente: botão desabilitado com o motivo", async () => {
  await openTab({ available: false, reason: "Remote Control indisponível: DISABLE_GROWTHBOOK definida (ambiente do ctowerd)" });
  const b = await within(pane()).findByRole("button", { name: "Ligar Remote Control" });
  expect(b).toBeDisabled();
  expect(b).toHaveAttribute("title", expect.stringContaining("DISABLE_GROWTHBOOK"));
});

test("menu de contexto da árvore liga o Remote Control e, ligado, mostra e copia o link", async () => {
  const { api, user } = await openTab();
  await within(pane()).findByRole("button", { name: "Ligar Remote Control" });
  fireEvent.contextMenu(row());
  const menu = screen.getByRole("menu", { name: "Ações da sessão" });
  await user.click(within(menu).getByRole("menuitem", { name: "Ligar Remote Control" }));
  expect(api.callsTo("session.remote_control.enable")).toHaveLength(1);
  expect(screen.queryByRole("menu")).not.toBeInTheDocument();

  api.on("dashboard.get", dashboard({ live_sessions: [{ ...S, rc_session_id: "session_01Abc" }], sync: [] }, [WI]));
  act(() => api.push("wsl", "session_remote_control", { session_id: S.id, rc_session_id: "session_01Abc" }));
  await screen.findByRole("dialog", { name: "Remote Control" });
  await user.keyboard("{Escape}");
  // Pelo teclado (Shift+F10) no item focado.
  row().focus();
  await user.keyboard("{Shift>}{F10}{/Shift}");
  const m = screen.getByRole("menu", { name: "Ações da sessão" });
  expect(within(m).queryByRole("menuitem", { name: "Ligar Remote Control" })).not.toBeInTheDocument();
  await user.click(within(m).getByRole("menuitem", { name: "Copiar link do Remote Control" }));
  expect(api.clipboard).toBe(LINK);
});

test("daemon anterior ao protocolo 2.3: sem controles de Remote Control", async () => {
  const api = new FakeDaemonApi()
    .on("env.info", { ...info(), protocol: "2.2", remote_control: undefined })
    .on("dashboard.get", dashboard({ live_sessions: [S], sync: [] }, [WI]))
    .on("session.peek", { data: "" });
  render(<App api={api} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: /remote control/ }));
  await waitFor(() => expect(api.attached.has(S.id)).toBe(true));
  expect(within(pane()).queryByRole("button", { name: "Ligar Remote Control" })).not.toBeInTheDocument();
});

test("estado do Remote Control: ligado só com a sessão viva; conectando por 30 s", () => {
  const on = { ...S, rc_session_id: "session_01X" };
  expect(rcOn(on)).toBe(true);
  expect(rcOn({ ...on, state: "exited" })).toBe(false);
  expect(rcView(on, undefined, 0)).toEqual({ kind: "on", url: "https://claude.ai/code/session_01X" });
  expect(rcView(S, undefined, 0)).toEqual({ kind: "off" });
  expect(rcView(S, 1000, 1000 + RC_CONNECT_MS - 1)).toEqual({ kind: "connecting" });
  expect(rcView(S, 1000, 1000 + RC_CONNECT_MS)).toEqual({ kind: "failed" });
});

test("QR code: matriz quadrada com margem, só tokens no SVG", () => {
  render(<QrCode value={LINK} label="qr" />);
  const svg = screen.getByRole("img", { name: "qr" });
  const [, , w, h] = svg.getAttribute("viewBox")!.split(" ").map(Number);
  expect(w).toBe(h);
  // Versão 3 (29 módulos) para um link deste tamanho, com nível M, mais 2×4 de margem.
  expect(w).toBeGreaterThanOrEqual(29 + 8);
  expect(svg.querySelector("path")!.getAttribute("d")!.length).toBeGreaterThan(100);
  expect(svg.innerHTML).not.toMatch(/#[0-9a-f]{3,6}/i);
});
