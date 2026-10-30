import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import { StatusMenu } from "./StatusMenu";

const options = async (subIssue: boolean, current: "backlog" | "aguardando_epico" = "backlog") => {
  render(<StatusMenu current={current} enabled busy={false} subIssue={subIssue} onSet={async () => true} />);
  await userEvent.setup().click(screen.getByRole("button", { name: /Mudar status/ }));
  return within(screen.getByRole("group", { name: "Mudar status" }))
    .getAllByRole("button")
    .map((b) => b.textContent);
};

test("'aguardando o épico' (#457) só entre as opções da sub-issue, ou quando já é o status", async () => {
  expect(await options(true)).toContain("aguardando o épico");
  cleanup();
  expect(await options(false)).not.toContain("aguardando o épico");
  cleanup();
  expect(await options(false, "aguardando_epico")).toContain("aguardando o épicoatual");
});
