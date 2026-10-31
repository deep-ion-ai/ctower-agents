import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { Screen } from "../state/nav";
import { dashboard } from "../test/fixtures";

const WSL = FakeDaemonApi.wslEnv();
const MAC = { ...FakeDaemonApi.wslEnv("mac"), name: "mac-mini" };

function setup(initial?: Screen, envs = [WSL, MAC]) {
  const api = new FakeDaemonApi(envs).on("dashboard.get", dashboard({ projects: [] })).on("sync.now", []);
  render(<App api={api} initial={initial} />);
  return { api, user: userEvent.setup() };
}

const telas = () => screen.getByRole("navigation", { name: "Telas" });
const trilha = () => screen.getByRole("navigation", { name: "Trilha" });
/** Linha da seção no painel lateral. */
const sideRow = (name: string) =>
  within(screen.getByRole("region", { name: "Painel lateral" })).getByRole("button", { name });

test("Manutenção vem depois do Hub e abre a Visão geral; voltar à tela reabre a última aba dela", async () => {
  const { user } = setup();
  await user.click(within(telas()).getByRole("button", { name: "Manutenção" }));
  expect(within(telas()).getByRole("button", { name: "Manutenção" })).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("tab", { name: "Manutenção · Visão geral" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getByRole("heading", { level: 1, name: "Visão geral" })).toBeInTheDocument();
  expect(sideRow("Visão geral")).toHaveAttribute("aria-current", "true");

  // Seção em aba própria; a Visão geral continua aberta.
  await user.click(sideRow("Backup e snapshots"));
  expect(screen.getByRole("tab", { name: "Manutenção · Backup e snapshots" })).toHaveAttribute("aria-selected", "true");
  expect(trilha()).toHaveTextContent("ManutençãoBackup e snapshots");
  expect(screen.getByRole("tab", { name: "Manutenção · Visão geral" })).toBeInTheDocument();

  // Outra tela e de volta: cai na aba principal da tela, como no Hub.
  await user.click(within(telas()).getByRole("button", { name: "Hub" }));
  await user.click(within(telas()).getByRole("button", { name: "Manutenção" }));
  expect(screen.getByRole("tab", { name: "Manutenção · Visão geral" })).toHaveAttribute("aria-selected", "true");
  // Clicar de novo na seção foca a aba que já existe.
  await user.click(sideRow("Backup e snapshots"));
  expect(screen.getAllByRole("tab", { name: "Manutenção · Backup e snapshots" })).toHaveLength(1);
});

test("seções de ambiente: o primeiro conectado, seletor com mais de um e o ambiente no endereço", async () => {
  const { user } = setup({ name: "maintenance", section: "cleanup" });
  expect(await screen.findByRole("heading", { level: 1, name: "Limpeza" })).toBeInTheDocument();
  expect(trilha()).toHaveTextContent("ManutençãoLimpezactower-agents (WSL)");
  const seletor = screen.getByLabelText("Ambiente da manutenção");
  expect(seletor).toHaveValue("wsl");

  await user.selectOptions(seletor, "mac");
  expect(trilha()).toHaveTextContent("ManutençãoLimpezamac-mini");
  expect(screen.getAllByRole("tab", { name: "Manutenção · Limpeza" })).toHaveLength(1);
  // Atualizações › Ambientes mostra todos numa tabela (#336): sem ambiente na trilha nem seletor.
  await user.click(sideRow("Ambientes"));
  expect(screen.getByRole("tab", { name: "Manutenção · Atualizações · Ambientes" })).toHaveAttribute("aria-selected", "true");
  expect(trilha()).toHaveTextContent(/^ManutençãoAtualizaçõesAmbientes$/);
  expect(screen.queryByLabelText("Ambiente da manutenção")).not.toBeInTheDocument();
  // Seção que não é de ambiente: sem seletor.
  await user.click(sideRow("Este app"));
  expect(screen.queryByLabelText("Ambiente da manutenção")).not.toBeInTheDocument();
});

test("um ambiente só: sem seletor de ambiente; as regras ficam na própria Limpeza (#22)", async () => {
  setup({ name: "maintenance", section: "cleanup" }, [WSL]);
  expect(await screen.findByRole("heading", { level: 1, name: "Limpeza" })).toBeInTheDocument();
  expect(screen.queryByLabelText("Ambiente da manutenção")).not.toBeInTheDocument();
  expect(screen.getByRole("heading", { level: 2, name: "Regras" })).toBeInTheDocument();
  expect(screen.getByRole("table", { name: "Regras de limpeza" })).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Ajustes de limpeza ›" })).not.toBeInTheDocument();
});

test("sem ambiente configurado: a seção de ambiente explica e leva às Configurações", async () => {
  const { user } = setup({ name: "maintenance", section: "cleanup" }, []);
  await user.click(await screen.findByRole("button", { name: "Configurar ambientes" }));
  expect(screen.getByRole("tab", { name: "Configurações" })).toHaveAttribute("aria-selected", "true");
});

test("paleta: Manutenção › Limpeza abre a seção em aba própria", async () => {
  const { user } = setup();
  await user.click(screen.getByRole("button", { name: "Abrir paleta de comandos" }));
  const dialog = screen.getByRole("dialog", { name: "Paleta de comandos" });
  await user.type(within(dialog).getByRole("combobox"), "manutenção limpeza");
  await user.keyboard("{Enter}");
  expect(screen.getByRole("tab", { name: "Manutenção · Limpeza" })).toHaveAttribute("aria-selected", "true");
});

test("Backup e snapshots sem hub configurado: explica e leva às Configurações (#338)", async () => {
  const { api, user } = setup({ name: "maintenance", section: "backup" });
  expect(await screen.findByText(/os snapshots e o backup remoto são do hub/)).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Configurar o hub" }));
  expect(screen.getByRole("tab", { name: "Configurações" })).toHaveAttribute("aria-selected", "true");
  expect(api.calls.filter((c) => c.env === "hub")).toHaveLength(0);
});
