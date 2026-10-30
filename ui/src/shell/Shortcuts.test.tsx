// Folha de atalhos e gestos (#27): `?` fora de campos de texto, a paleta e a
// barra de status; no celular, só os gestos.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, vi } from "vitest";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import { dashboard } from "../test/fixtures";
import { setViewport } from "../test/viewport";
import { webApi } from "../test/web";

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
});

function setup(api: FakeDaemonApi = new FakeDaemonApi()) {
  api.on("dashboard.get", dashboard()).on("session.peek", { data: "" }).on("sync.now", []);
  render(<App api={api} />);
  return userEvent.setup();
}

const sheet = () => screen.queryByRole("dialog", { name: /Atalhos e gestos|Gestos/ });

test("? abre e fecha a folha; Esc fecha e devolve o foco", async () => {
  const user = setup();
  await screen.findByRole("navigation", { name: "Telas" });
  await user.keyboard("?");
  const d = await screen.findByRole("dialog", { name: "Atalhos e gestos" });
  // Agrupada por tela, com os atalhos de verdade.
  expect(within(d).getAllByRole("heading", { level: 3 }).map((h) => h.textContent)).toEqual(
    expect.arrayContaining(["Geral", "Terminal", "Projetos"]),
  );
  const keys = within(d)
    .getAllByText((_, el) => el?.tagName === "KBD")
    .map((k) => k.textContent);
  expect(keys).toEqual(expect.arrayContaining(["Ctrl+K", "Ctrl+B", "Ctrl+Alt+B", "Shift+F10", "Alt+↑", "Ctrl+Shift+`"]));
  expect(within(d).getByRole("button", { name: "Fechar atalhos" })).toHaveFocus();
  await user.keyboard("?");
  expect(sheet()).toBeNull();
  const trigger = screen.getByRole("button", { name: "Abrir paleta de comandos" });
  act(() => trigger.focus());
  await user.keyboard("?");
  await screen.findByRole("dialog", { name: "Atalhos e gestos" });
  await user.keyboard("{Escape}");
  expect(sheet()).toBeNull();
  expect(trigger).toHaveFocus();
});

test("? num campo de texto é texto: não abre a folha", async () => {
  const user = setup();
  await user.click(await screen.findByRole("button", { name: "Abrir paleta de comandos" }));
  const input = screen.getByRole("combobox");
  await user.keyboard("?");
  expect(input).toHaveValue("?");
  expect(sheet()).toBeNull();
});

test("também pela paleta e pelo botão da barra de status", async () => {
  const user = setup();
  await user.click(await screen.findByRole("button", { name: "Abrir paleta de comandos" }));
  await user.keyboard("atalhos");
  await user.click(screen.getByRole("option", { name: /Atalhos e gestos/ }));
  await screen.findByRole("dialog", { name: "Atalhos e gestos" });
  await user.click(screen.getByRole("button", { name: "Fechar atalhos" }));
  expect(sheet()).toBeNull();
  const bar = screen.getByRole("contentinfo", { name: "Barra de status" });
  await user.click(within(bar).getByRole("button", { name: "Atalhos e gestos" }));
  expect(await screen.findByRole("dialog", { name: "Atalhos e gestos" })).toBeInTheDocument();
});

test("celular: a folha mostra só os gestos (sem teclas), e abre pela paleta", async () => {
  setViewport(360);
  const api = webApi();
  render(<App api={api} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: "Abrir paleta de comandos" }));
  await user.keyboard("atalhos");
  await user.click(screen.getByRole("option", { name: /Atalhos e gestos/ }));
  const d = await screen.findByRole("dialog", { name: "Gestos" });
  expect(d).toHaveClass("msheet");
  expect(d.querySelector("kbd")).toBeNull();
  expect(within(d).getByText("Tocar no sino")).toBeInTheDocument();
  expect(within(d).queryByText(/Terminal|Ctrl/)).toBeNull();
  await user.click(within(d).getByRole("button", { name: "Fechar atalhos" }));
  await waitFor(() => expect(sheet()).toBeNull());
});
