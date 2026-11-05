import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { TreeControls } from "./TreeControls";
import { MINE_UNKNOWN, NO_FILTERS } from "./tree-view";

test("'Só minhas' desabilitado, com a dica, quando o 'eu' do rastreador é desconhecido", () => {
  render(<TreeControls value={{ ...NO_FILTERS, mine: true }} onChange={() => {}} more={false} me={null} />);
  const box = screen.getByRole("checkbox", { name: "Só minhas" });
  expect(box).toBeDisabled();
  // A escolha guardada não aparece marcada sem o "eu".
  expect(box).not.toBeChecked();
  expect(box).toHaveAccessibleDescription(MINE_UNKNOWN);
  expect(box.closest("label")).toHaveAttribute("title", MINE_UNKNOWN);
});

test("'Só minhas' liga com o 'eu' conhecido", async () => {
  const onChange = vi.fn();
  render(<TreeControls value={NO_FILTERS} onChange={onChange} more={false} me="fulano" />);
  const box = screen.getByRole("checkbox", { name: "Só minhas" });
  expect(box).toBeEnabled();
  expect(box.closest("label")).toHaveAttribute("title", "Só as issues atribuídas a fulano");
  await userEvent.setup().click(box);
  expect(onChange).toHaveBeenCalledWith({ ...NO_FILTERS, mine: true });
});
