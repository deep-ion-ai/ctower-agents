import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test } from "vitest";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import { dashboard, envInfo, hubInfo } from "../test/fixtures";
import { testDescription } from "./testMode";

// Hub e ambiente de teste (#176): barra do topo em outra cor e um selo.

function api() {
  return new FakeDaemonApi().on("dashboard.get", dashboard()).on("session.peek", { data: "" });
}

const banner = () => document.querySelector("header.titlebar")!;

afterEach(() => {
  document.title = "Control Tower";
});

test("testDescription diz quem está em modo de teste", () => {
  expect(testDescription({ hub: "ct-test-hub", envs: [] })).toBe("Modo de teste: hub ct-test-hub.");
  expect(testDescription({ hub: null, envs: ["a"] })).toBe("Modo de teste: ambiente a.");
  expect(testDescription({ hub: "h", envs: ["a", "b"] })).toBe("Modo de teste: hub h e ambientes a, b.");
});

test("sem modo de teste: barra normal, sem selo", async () => {
  document.title = "Control Tower";
  render(<App api={api().on("env.info", envInfo()).withHub().onHub("hub.info", hubInfo())} />);
  await screen.findByRole("navigation", { name: "Telas" });
  // Espera o hub.info chegar antes de afirmar a ausência.
  await waitFor(() => expect(screen.getByRole("button", { name: /Hub mac-mini/ })).toBeInTheDocument());
  expect(banner()).not.toHaveAttribute("data-test");
  expect(screen.queryByText("Ambiente de teste")).toBeNull();
  expect(document.title).toBe("Control Tower");
});

test("hub de teste: barra marcada, selo e título da janela", async () => {
  document.title = "Control Tower";
  render(<App api={api().on("env.info", envInfo()).withHub().onHub("hub.info", hubInfo({ name: "ct-test-hub", test_mode: true }))} />);
  const badge = await screen.findByText("Ambiente de teste");
  expect(badge).toHaveAccessibleName("Modo de teste: hub ct-test-hub.");
  expect(banner()).toHaveAttribute("data-test", "true");
  expect(document.title).toBe("[TESTE] Control Tower");
});

test("ambiente de teste num app sem hub de teste: o selo cita o ambiente", async () => {
  render(<App api={api().on("env.info", envInfo({ test_mode: true }))} />);
  const badge = await screen.findByText("Ambiente de teste");
  expect(badge).toHaveAccessibleName("Modo de teste: ambiente ctower-agents (WSL).");
  expect(banner()).toHaveAttribute("data-test", "true");
});
