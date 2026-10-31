import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import type { JiraFieldInfo } from "../../protocol/hub-gen";
import type { JiraExtraFieldForm } from "../../hub/tracker";
import { JiraExtraFields } from "./JiraExtraFields";

const CATALOG: JiraFieldInfo[] = [
  { id: "customfield_10002", name: "Story Points", datatype: "number" },
  {
    id: "customfield_10099",
    name: "Responsável pela implementação",
    datatype: "user",
  },
  { id: "customfield_10098", name: "Responsável pela revisão", datatype: "user" },
  { id: "customfield_10097", name: "Sistema", datatype: "option" },
];

/** O componente com o estado do formulário; `rows()` lê o que ele gravou. */
function setup(catalog: JiraFieldInfo[], initial: JiraExtraFieldForm[] = []) {
  let current = initial;
  function Host() {
    const [rows, setRows] = useState(initial);
    current = rows;
    return (
      <JiraExtraFields
        prefix="t"
        rows={rows}
        catalog={catalog}
        onChange={(r) => {
          current = r;
          setRows(r);
        }}
      />
    );
  }
  render(<Host />);
  return { user: userEvent.setup(), rows: () => current };
}

const group = () => screen.getByRole("group", { name: "Campos do projeto na tela da issue" });

test("combobox com catálogo: busca sem acento, escolha preenche o rótulo e mostra o ID e o tipo", async () => {
  const { user, rows } = setup(CATALOG);
  const g = group();
  expect(g).toHaveTextContent("Situação, Tipo, Prioridade, Responsável e Criador já aparecem sempre.");
  expect(g).not.toHaveTextContent(/jira init/);
  expect(within(g).getByText("0 de 20")).toBeInTheDocument();
  await user.click(within(g).getByRole("button", { name: "+ Adicionar campo" }));
  const combo = within(g).getByRole("combobox", { name: "Campo do Jira 1" });
  // O foco vai para a linha nova.
  expect(combo).toHaveFocus();
  expect(combo).toHaveAttribute("aria-expanded", "false");
  expect(combo).toHaveAccessibleDescription(/Busque pelo nome ou pelo ID/);
  await user.type(combo, "IMPLEMENTACAO");
  expect(combo).toHaveAttribute("aria-expanded", "true");
  const list = within(g).getByRole("listbox", {
    name: "Campos do Jira (linha 1)",
  });
  expect(combo).toHaveAttribute("aria-controls", list.id);
  const opts = within(list).getAllByRole("option");
  expect(opts).toHaveLength(1);
  expect(opts[0]).toHaveTextContent("Responsável pela implementação");
  expect(opts[0]).toHaveTextContent("customfield_10099 · user");
  await user.click(opts[0]);
  expect(combo).toHaveValue("Responsável pela implementação");
  expect(combo).toHaveAttribute("aria-expanded", "false");
  expect(within(g).getByLabelText("Rótulo na tela 1")).toHaveValue("Responsável pela implementação");
  expect(
    within(g).getByText("customfield_10099 · user", {
      selector: ".hub-jx-chosen",
    }),
  ).toBeInTheDocument();
  expect(rows()).toEqual([{ id: "customfield_10099", label: "Responsável pela implementação" }]);
  // Busca pelo ID também.
  await user.click(within(g).getByRole("button", { name: "+ Adicionar campo" }));
  const combo2 = within(g).getByRole("combobox", { name: "Campo do Jira 2" });
  await user.type(combo2, "10097");
  expect(within(g).getByRole("option", { name: /Sistema/ })).toBeInTheDocument();
});

test("rótulo já preenchido não é trocado ao escolher o campo", async () => {
  const { user, rows } = setup(CATALOG, [{ id: "", label: "Dev" }]);
  const combo = screen.getByRole("combobox", { name: "Campo do Jira 1" });
  await user.type(combo, "implem");
  await user.click(screen.getByRole("option", { name: /implementação/ }));
  expect(rows()).toEqual([{ id: "customfield_10099", label: "Dev" }]);
});

test("teclado: setas pulam o campo já na lista, Enter escolhe, Esc fecha", async () => {
  const { user, rows } = setup(CATALOG, [{ id: "customfield_10099", label: "Implementação" }]);
  // A linha existente mostra o nome do campo do catálogo.
  expect(screen.getByRole("combobox", { name: "Campo do Jira 1" })).toHaveValue("Responsável pela implementação");
  await user.click(screen.getByRole("button", { name: "+ Adicionar campo" }));
  const combo = screen.getByRole("combobox", { name: "Campo do Jira 2" });
  await user.type(combo, "respons");
  const used = screen.getByRole("option", {
    name: /Responsável pela implementação/,
  });
  expect(used).toHaveAttribute("aria-disabled", "true");
  expect(used).toHaveTextContent("já na lista");
  // Clicar no desabilitado não faz nada.
  await user.click(used);
  expect(rows()[1].id).toBe("");
  await user.keyboard("{ArrowDown}");
  const teste = screen.getByRole("option", { name: /Responsável pela revisão/ });
  expect(teste).toHaveAttribute("aria-selected", "true");
  expect(combo).toHaveAttribute("aria-activedescendant", teste.id);
  await user.keyboard("{ArrowDown}");
  expect(teste).toHaveAttribute("aria-selected", "true");
  await user.keyboard("{Enter}");
  expect(rows()[1]).toEqual({
    id: "customfield_10098",
    label: "Responsável pela revisão",
  });
  expect(combo).toHaveAttribute("aria-expanded", "false");
  // Esc fecha a lista aberta pela seta.
  await user.keyboard("{ArrowDown}");
  expect(combo).toHaveAttribute("aria-expanded", "true");
  await user.keyboard("{Escape}");
  expect(combo).toHaveAttribute("aria-expanded", "false");
  expect(combo).not.toHaveAttribute("aria-activedescendant");
});

test("ID digitado fora do catálogo vira o ID; nome sem escolher não", async () => {
  const { user, rows } = setup(CATALOG, [{ id: "", label: "Manual" }]);
  const combo = screen.getByRole("combobox", { name: "Campo do Jira 1" });
  await user.type(combo, "Respons");
  expect(rows()[0].id).toBe("");
  // Só o rótulo: a linha avisa, de forma ligada ao campo.
  expect(combo).toHaveAttribute("aria-invalid", "true");
  expect(combo).toHaveAccessibleDescription(/Escolha um campo da lista ou digite o ID/);
  await user.clear(combo);
  await user.type(combo, "customfield_777");
  expect(rows()[0].id).toBe("customfield_777");
  expect(screen.getByText(/Nenhum campo com “customfield_777”\. O ID digitado será usado\./)).toBeInTheDocument();
  await user.keyboard("{Escape}");
  expect(screen.getByText("customfield_777 · fora do catálogo")).toBeInTheDocument();
  expect(combo).not.toHaveAttribute("aria-invalid");
});

test("sem catálogo: a ajuda fala do jira init e o texto digitado é o ID", async () => {
  const { user, rows } = setup([]);
  const g = group();
  expect(g).toHaveTextContent(/vem do .config.yml do jira-cli/);
  expect(g).toHaveTextContent(/rodar jira init no ambiente atualiza/);
  expect(g).toHaveTextContent(/digite o ID \(customfield_…\)/);
  await user.click(within(g).getByRole("button", { name: "+ Adicionar campo" }));
  const combo = within(g).getByRole("combobox", { name: "Campo do Jira 1" });
  await user.type(combo, "customfield_10099");
  expect(combo).toHaveAttribute("aria-expanded", "false");
  await user.type(within(g).getByLabelText("Rótulo na tela 1"), "Implementação");
  expect(rows()).toEqual([{ id: "customfield_10099", label: "Implementação" }]);
  // Repetido: a segunda linha avisa.
  await user.click(within(g).getByRole("button", { name: "+ Adicionar campo" }));
  await user.type(within(g).getByRole("combobox", { name: "Campo do Jira 2" }), "CUSTOMFIELD_10099");
  expect(within(g).getByRole("combobox", { name: "Campo do Jira 2" })).toHaveAccessibleDescription(
    /Campo repetido: CUSTOMFIELD_10099 já está na linha 1\./,
  );
});

test("remover tira a linha certa e devolve o foco", async () => {
  const { user, rows } = setup(
    [],
    [
      { id: "customfield_1", label: "A" },
      { id: "customfield_2", label: "B" },
      { id: "customfield_3", label: "C" },
    ],
  );
  expect(screen.getByText("3 de 20")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Remover campo 2" }));
  expect(rows()).toEqual([
    { id: "customfield_1", label: "A" },
    { id: "customfield_3", label: "C" },
  ]);
  // O texto de cada combobox segue a sua linha.
  expect(screen.getByRole("combobox", { name: "Campo do Jira 2" })).toHaveValue("customfield_3");
  expect(screen.getByRole("combobox", { name: "Campo do Jira 2" })).toHaveFocus();
  await user.click(screen.getByRole("button", { name: "Remover campo 2" }));
  await user.click(screen.getByRole("button", { name: "Remover campo 1" }));
  expect(rows()).toEqual([]);
  expect(screen.getByRole("button", { name: "+ Adicionar campo" })).toHaveFocus();
  expect(screen.getByText("0 de 20")).toBeInTheDocument();
});

test("no máximo 20 campos", async () => {
  const rows20 = Array.from({ length: 19 }, (_, i) => ({
    id: `customfield_${i}`,
    label: `C${i}`,
  }));
  const { user, rows } = setup([], rows20);
  const add = screen.getByRole("button", { name: "+ Adicionar campo" });
  await user.click(add);
  expect(rows()).toHaveLength(20);
  expect(screen.getByText(/20 de 20/)).toHaveTextContent("20 de 20 (limite)");
  expect(add).toBeDisabled();
});

test("muitos resultados: até 50, com aviso para refinar a busca", async () => {
  const big: JiraFieldInfo[] = Array.from({ length: 60 }, (_, i) => ({
    id: `customfield_${20000 + i}`,
    name: `Campo ${String(i).padStart(2, "0")}`,
    datatype: "string",
  }));
  const { user } = setup(big, [{ id: "", label: "" }]);
  const combo = screen.getByRole("combobox", { name: "Campo do Jira 1" });
  await user.type(combo, "campo");
  expect(screen.getAllByRole("option")).toHaveLength(50);
  expect(screen.getByText("Mais 10 campos: refine a busca.")).toBeInTheDocument();
  await user.type(combo, " 5");
  expect(screen.getAllByRole("option")).toHaveLength(10);
  expect(screen.queryByText(/refine a busca/)).not.toBeInTheDocument();
});
