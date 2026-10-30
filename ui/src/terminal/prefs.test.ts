import { DEFAULT_PREFS, MAX_FONT, MIN_FONT, effectiveSize, resetTermPrefs, setTermPrefs, termPrefs, zoom } from "./prefs";

test("zoom soma ao tamanho, respeita os limites e Ctrl+0 zera", () => {
  zoom(1);
  zoom(1);
  expect(effectiveSize(termPrefs())).toBe(DEFAULT_PREFS.fontSize + 2);
  zoom(0);
  expect(termPrefs().zoom).toBe(0);
  for (let i = 0; i < 50; i++) zoom(1);
  expect(effectiveSize(termPrefs())).toBe(MAX_FONT);
  for (let i = 0; i < 100; i++) zoom(-1);
  expect(effectiveSize(termPrefs())).toBe(MIN_FONT);
});

test("fonte e tamanho ficam no localStorage; valores fora do limite são ajustados", () => {
  setTermPrefs({ fontFamily: "  Cascadia Code  ", fontSize: 99 });
  expect(termPrefs()).toMatchObject({ fontFamily: "Cascadia Code", fontSize: MAX_FONT });
  expect(JSON.parse(localStorage.getItem("ctower.terminal.prefs")!)).toMatchObject({ fontFamily: "Cascadia Code" });
  setTermPrefs({ fontFamily: "", fontSize: 2 });
  expect(termPrefs()).toMatchObject({ fontFamily: DEFAULT_PREFS.fontFamily, fontSize: MIN_FONT });
  resetTermPrefs();
  expect(termPrefs()).toEqual(DEFAULT_PREFS);
  expect(localStorage.getItem("ctower.terminal.prefs")).toBeNull();
});
