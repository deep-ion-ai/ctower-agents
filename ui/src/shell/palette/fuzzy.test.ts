import { fold, matchToken, parseQuery, search } from "./fuzzy";
import type { PaletteItem } from "./items";

const item = (id: string, kind: PaletteItem["kind"], label: string, p: Partial<PaletteItem> = {}): PaletteItem => ({
  id,
  kind,
  label,
  action: { type: "sync" },
  ...p,
});

const ITEMS: PaletteItem[] = [
  item("place:control", "place", "Sala de controle"),
  item("place:settings", "place", "Configurações", { keywords: "preferências" }),
  item("cmd:sync", "command", "Sincronizar agora", { detail: "GitHub" }),
  item("cmd:resume", "command", "Retomar interrompidas"),
  item("issue:297", "issue", "#297 ui: paleta de comandos", { num: 297, browse: false, detail: "ctower-agents" }),
  item("issue:29", "issue", "#29 ct-server: lacunas", { num: 29, browse: false, closed: true }),
  item("issue:1297", "epic", "#1297 Épico grande", { num: 1297, browse: false }),
  item("cmd:start:297", "command", "Iniciar sessão: #297 ui: paleta de comandos", { num: 297, browse: false }),
];

const ids = (r: ReturnType<typeof search>) => r.map((x) => x.item.id);

test("prefixos: # = issues e épicos, > = comandos", () => {
  expect(parseQuery("  #297 ")).toEqual({ mode: "issue", text: "297" });
  expect(parseQuery(">sinc")).toEqual({ mode: "command", text: "sinc" });
  expect(parseQuery("sala")).toEqual({ mode: "all", text: "sala" });
  expect(ids(search(ITEMS, "#297"))).toEqual(["issue:297", "issue:1297"]);
  expect(ids(search(ITEMS, ">sinc"))).toEqual(["cmd:sync"]);
  // `>` vazio: os comandos navegáveis (sem a lista de inícios).
  expect(ids(search(ITEMS, ">"))).toEqual(["cmd:sync", "cmd:resume"]);
});

test("número exato da issue vai para o topo; issue antes do comando de início", () => {
  expect(ids(search(ITEMS, "297")).slice(0, 2)).toEqual(["issue:297", "cmd:start:297"]);
  expect(ids(search(ITEMS, "#29"))[0]).toBe("issue:29");
  // Com "iniciar", só o comando casa.
  expect(ids(search(ITEMS, "iniciar 297"))).toEqual(["cmd:start:297"]);
});

test("difusa: subsequência, sem acento e por palavra; trecho contínuo vale mais", () => {
  expect(ids(search(ITEMS, "sincag"))[0]).toBe("cmd:sync");
  expect(ids(search(ITEMS, "configuracoes"))).toEqual(["place:settings"]);
  expect(ids(search(ITEMS, "preferencias"))).toEqual(["place:settings"]);
  // "sc" casa com "Sala de controle" (inícios de palavra) e com outros no meio.
  expect(ids(search(ITEMS, "sc"))[0]).toBe("place:control");
  expect(ids(search(ITEMS, "paleta"))).toContain("issue:297");
  expect(search(ITEMS, "xyzw")).toEqual([]);
  // Contínuo > subsequência.
  const cont = matchToken(fold("retomar interrompidas"), "inter")!;
  const sub = matchToken(fold("retomar interrompidas"), "rtin")!;
  expect(cont.score).toBeGreaterThan(sub.score);
  expect(cont.hits).toEqual([8, 9, 10, 11, 12]);
});

test("destaque: posições casadas no rótulo, prefira inícios de palavra", () => {
  const r = search(ITEMS, "sdc").find((x) => x.item.id === "place:control")!;
  expect(r.hits).toEqual([0, 5, 8]);
  expect(fold("Ação")).toBe("acao");
});

test("busca vazia: recentes no topo, depois os navegáveis; issues só buscando", () => {
  const r = search(ITEMS, "", ["cmd:resume", "issue:297", "sumiu"]);
  expect(ids(r)).toEqual(["cmd:resume", "issue:297", "place:control", "place:settings", "cmd:sync"]);
  expect(r.slice(0, 2).every((x) => x.recent)).toBe(true);
  // `#` vazio: as issues, abertas antes das concluídas.
  expect(ids(search(ITEMS, "#"))).toEqual(["issue:297", "issue:1297", "issue:29"]);
});

test("recentes ganham um pequeno bônus na busca", () => {
  const two = [item("a", "place", "Revisão A"), item("b", "place", "Revisão B")];
  expect(ids(search(two, "revisao"))).toEqual(["a", "b"]);
  expect(ids(search(two, "revisao", ["b"]))).toEqual(["b", "a"]);
});
