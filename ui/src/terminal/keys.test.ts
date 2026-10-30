import { contextAction, keyAction, NEWLINE, runAction, type KeyLike, type KeyTarget } from "./keys";

const k = (key: string, mods: Partial<KeyLike> = {}): KeyLike => ({
  key,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  metaKey: false,
  ...mods,
});

test.each([
  // [tecla, modificadores, com seleção, esperado]
  ["c", { ctrlKey: true }, true, "copy"],
  ["c", { ctrlKey: true }, false, null],
  ["C", { ctrlKey: true, shiftKey: true }, false, "copy"],
  ["C", { ctrlKey: true, shiftKey: true }, true, "copy"],
  ["v", { ctrlKey: true }, false, "paste"],
  ["V", { ctrlKey: true, shiftKey: true }, true, "paste"],
  ["Enter", { ctrlKey: true }, false, "newline"],
  ["Enter", { shiftKey: true }, false, "newline"],
  ["Enter", {}, false, null],
  ["Enter", { altKey: true }, false, null],
  ["c", {}, true, null],
  ["v", {}, false, null],
  ["c", { ctrlKey: true, altKey: true }, true, null],
  ["d", { ctrlKey: true }, true, null],
  ["c", { metaKey: true }, true, null],
] as const)("Windows/Linux: %s %o (seleção: %s) → %s", (key, mods, sel, want) => {
  expect(keyAction(k(key, mods), sel, false)).toBe(want);
});

test.each([
  ["c", { metaKey: true }, true, "copy"],
  ["c", { metaKey: true }, false, "copy"],
  ["v", { metaKey: true }, false, "paste"],
  ["c", { ctrlKey: true }, true, null],
  ["Enter", { shiftKey: true }, false, "newline"],
] as const)("macOS: %s %o (seleção: %s) → %s", (key, mods, sel, want) => {
  expect(keyAction(k(key, mods), sel, true)).toBe(want);
});

function target(selection = "") {
  const log: string[] = [];
  let sel = selection;
  const t: KeyTarget = {
    hasSelection: () => sel !== "",
    getSelection: () => sel,
    clearSelection: () => {
      sel = "";
      log.push("clear");
    },
    paste: (text) => log.push(`paste:${text}`),
    input: (data) => log.push(`input:${JSON.stringify(data)}`),
  };
  const clip = { text: "", read: async () => clip.text, write: async (t: string) => void (clip.text = t) };
  return { t, log, clip };
}

test("copiar leva a seleção e limpa; colar usa o paste do xterm (bracketed)", async () => {
  const { t, log, clip } = target("linha 1\nlinha 2");
  await runAction("copy", t, clip);
  expect(clip.text).toBe("linha 1\nlinha 2");
  expect(log).toEqual(["clear"]);
  await runAction("paste", t, clip);
  expect(log).toEqual(["clear", "paste:linha 1\nlinha 2"]);
});

test("copiar sem seleção não mexe na área de transferência; colar vazio não manda nada", async () => {
  const { t, log, clip } = target();
  clip.text = "antes";
  await runAction("copy", t, clip);
  expect(clip.text).toBe("antes");
  clip.text = "";
  await runAction("paste", t, clip);
  expect(log).toEqual([]);
});

test("nova linha manda ESC+CR", async () => {
  const { t, log, clip } = target();
  await runAction("newline", t, clip);
  expect(log).toEqual([`input:${JSON.stringify(NEWLINE)}`]);
  expect(NEWLINE).toBe("\x1b\r");
});

test("clique direito: link copia o endereço; seleção copia; senão cola", async () => {
  const a = target("trecho");
  await contextAction(a.t, "https://github.com/o/r", a.clip);
  expect(a.clip.text).toBe("https://github.com/o/r");
  expect(a.log).toEqual([]);
  await contextAction(a.t, null, a.clip);
  expect(a.clip.text).toBe("trecho");
  await contextAction(a.t, null, a.clip);
  expect(a.log).toEqual(["clear", "paste:trecho"]);
});

test("Ctrl+F (Cmd+F no macOS) abre a busca", () => {
  expect(keyAction(k("f", { ctrlKey: true }), false, false)).toBe("search");
  expect(keyAction(k("F", { ctrlKey: true, shiftKey: true }), false, false)).toBeNull();
  expect(keyAction(k("f", { metaKey: true }), false, true)).toBe("search");
  expect(keyAction(k("f", { ctrlKey: true }), false, true)).toBeNull();
});

test("Ctrl+= / Ctrl++ / Ctrl+- / Ctrl+0 fazem o zoom", () => {
  expect(keyAction(k("=", { ctrlKey: true }), false, false)).toBe("zoomIn");
  expect(keyAction(k("+", { ctrlKey: true, shiftKey: true }), false, false)).toBe("zoomIn");
  expect(keyAction(k("-", { ctrlKey: true }), false, false)).toBe("zoomOut");
  expect(keyAction(k("0", { ctrlKey: true }), false, false)).toBe("zoomReset");
  expect(keyAction(k("0", {}), false, false)).toBeNull();
  expect(keyAction(k("=", { metaKey: true }), false, true)).toBe("zoomIn");
});
