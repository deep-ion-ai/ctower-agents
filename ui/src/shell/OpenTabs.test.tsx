import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { filterTabs, OpenTabsList, type OpenTabItem } from "./OpenTabs";

// Lista de abas abertas (#427).

const ITEMS: OpenTabItem[] = [
  { id: "control", title: "Sala de controle" },
  { id: "t1", title: "#32 ui: terminal embutido", project: "ctower-agents", tone: "ok" },
  { id: "t2", title: "#44 docs novas", project: "docs-site" },
  { id: "s1", title: "Shell · #32", project: "ctower-agents", icon: "prompt" },
];

const options = () => screen.getAllByRole("option").map((o) => o.textContent);

test("filtro por título ou projeto, sem diferenciar maiúsculas", () => {
  expect(filterTabs(ITEMS, "").map((t) => t.id)).toEqual(["control", "t1", "t2", "s1"]);
  expect(filterTabs(ITEMS, "DOCS").map((t) => t.id)).toEqual(["t2"]);
  expect(filterTabs(ITEMS, "ctower-agents").map((t) => t.id)).toEqual(["t1", "s1"]);
  expect(filterTabs(ITEMS, "#32").map((t) => t.id)).toEqual(["t1", "s1"]);
});

test("lista com o projeto de cada aba; filtra, setas movem e Enter ativa", async () => {
  const picked: string[] = [];
  const closed: boolean[] = [];
  render(<OpenTabsList items={ITEMS} current="t2" onPick={(id) => picked.push(id)} onClose={(r) => closed.push(r)} />);
  const user = userEvent.setup();
  const campo = screen.getByRole("combobox", { name: "Filtrar abas abertas" });
  expect(campo).toHaveFocus();
  expect(options()).toEqual(["Sala de controle", "#32 ui: terminal embutidoctower-agents", "#44 docs novasdocs-site", "Shell · #32ctower-agents"]);
  // A seleção começa na aba ativa.
  expect(screen.getByRole("option", { name: /#44 docs novas/ })).toHaveAttribute("aria-selected", "true");
  await user.keyboard("{ArrowDown}");
  expect(screen.getByRole("option", { name: /Shell/ })).toHaveAttribute("aria-selected", "true");
  expect(campo).toHaveAttribute("aria-activedescendant", "otab-s1");
  await user.keyboard("{ArrowDown}");
  expect(screen.getByRole("option", { name: "Sala de controle" })).toHaveAttribute("aria-selected", "true");
  await user.keyboard("{ArrowUp}");
  expect(screen.getByRole("option", { name: /Shell/ })).toHaveAttribute("aria-selected", "true");
  await user.type(campo, "tower");
  expect(options()).toEqual(["#32 ui: terminal embutidoctower-agents", "Shell · #32ctower-agents"]);
  await user.keyboard("{ArrowDown}{Enter}");
  expect(picked).toEqual(["s1"]);
  expect(closed).toEqual([false]);
});

test("sem resultado mostra o aviso; Esc fecha devolvendo o foco; clique escolhe", async () => {
  const picked: string[] = [];
  const closed: boolean[] = [];
  const { unmount } = render(
    <OpenTabsList items={ITEMS} current="control" onPick={(id) => picked.push(id)} onClose={(r) => closed.push(r)} />,
  );
  const user = userEvent.setup();
  await user.type(screen.getByRole("combobox"), "zzz");
  expect(screen.queryAllByRole("option")).toHaveLength(0);
  expect(screen.getByText("Nenhuma aba encontrada.")).toBeInTheDocument();
  await user.keyboard("{Enter}");
  expect(picked).toEqual([]);
  await user.keyboard("{Escape}");
  expect(closed).toEqual([true]);
  unmount();
  render(<OpenTabsList items={ITEMS} current="control" onPick={(id) => picked.push(id)} onClose={(r) => closed.push(r)} />);
  await user.click(within(screen.getByRole("listbox", { name: "Abas" })).getByRole("option", { name: /#44/ }));
  expect(picked).toEqual(["t2"]);
});
