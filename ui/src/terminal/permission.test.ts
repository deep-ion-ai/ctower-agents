import { permissionArg, permissionText, remaining } from "./permission";

test("argumento principal por ferramenta, JSON nas desconhecidas", () => {
  expect(permissionArg({ tool_name: "Bash", tool_input: { command: "ls -la", description: "x" } })).toBe("ls -la");
  expect(permissionArg({ tool_name: "Edit", tool_input: { file_path: "/a/b.ts", old_string: "x" } })).toBe("/a/b.ts");
  expect(permissionArg({ tool_name: "mcp__x__y", tool_input: { a: 1 } })).toBe('{"a":1}');
  expect(permissionArg({ tool_name: "Bash", tool_input: {} })).toBe("");
  expect(permissionArg({ tool_name: null, tool_input: null })).toBe("");
});

test("texto do pedido junta espaços e corta no limite", () => {
  expect(permissionText({ tool_name: "Bash", tool_input: { command: "git\n  status" } })).toBe("Bash: git status");
  expect(permissionText({ tool_name: null, tool_input: {} })).toBe("Ferramenta pede permissão");
  expect(permissionText({ tool_name: "Bash", tool_input: { command: "x".repeat(50) } }, 10)).toBe("Bash: xxxxxxxxx…");
});

test("tempo restante em m:ss, nunca negativo", () => {
  expect(remaining(65_000, 0)).toBe("1:05");
  expect(remaining(0, 5_000)).toBe("0:00");
});
