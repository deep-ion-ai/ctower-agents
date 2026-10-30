import { mainTabId } from "../state/nav";
import { envAt, envSectionOf, envTab, envTitle, isEnvSection } from "./sections";

test("envAt: Conexão sai sem `section`", () => {
  expect(envAt("wsl")).toEqual({ name: "env", env: "wsl" });
  expect(envAt("wsl", "connection")).toEqual({ name: "env", env: "wsl" });
  expect(envAt("wsl", "update")).toEqual({ name: "env", env: "wsl", section: "update" });
});

test("envSectionOf: padrão Conexão; a web não abre as seções do desktop", () => {
  expect(envSectionOf({})).toBe("connection");
  expect(envSectionOf({ section: "cleanup" })).toBe("cleanup");
  expect(envSectionOf({ section: "cleanup" }, true)).toBe("connection");
  expect(envSectionOf({ section: "config" }, true)).toBe("connection");
  expect(envSectionOf({ section: "hub" }, true)).toBe("hub");
});

test("envTab e mainTabId: uma aba por ambiente; o índice fica na aba `env`", () => {
  expect(envTab("wsl", "WSL", "hub")).toEqual({ screen: { name: "env", env: "wsl", section: "hub" }, title: "WSL · Hub" });
  expect(envTab("wsl")).toEqual({ screen: { name: "env", env: "wsl" } });
  expect(mainTabId(envAt("wsl", "hub"))).toBe("env:wsl");
  expect(mainTabId({ name: "env" })).toBe("env");
  expect(envTitle("WSL")).toBe("WSL");
  expect(isEnvSection("update")).toBe(true);
  expect(isEnvSection("x")).toBe(false);
});
