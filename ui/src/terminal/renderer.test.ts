import { loadWebgl, type WebglLike } from "./renderer";

class FakeWebgl implements WebglLike {
  disposed = false;
  private lost: (() => void) | null = null;
  onContextLoss(cb: () => void) {
    this.lost = cb;
    return { dispose: () => (this.lost = null) };
  }
  dispose() {
    this.disposed = true;
  }
  loseContext() {
    this.lost?.();
  }
}

test("usa o WebGL quando carrega", () => {
  const loaded: FakeWebgl[] = [];
  const addon = new FakeWebgl();
  expect(loadWebgl((a) => loaded.push(a), () => addon)).toBe("webgl");
  expect(loaded).toEqual([addon]);
  expect(addon.disposed).toBe(false);
});

test("sem suporte a WebGL, fica no DOM", () => {
  const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
  expect(
    loadWebgl(
      () => {},
      () => {
        throw new Error("sem contexto webgl2");
      },
    ),
  ).toBe("dom");
  expect(
    loadWebgl(
      () => {
        throw new Error("falhou ao carregar");
      },
      () => new FakeWebgl(),
    ),
  ).toBe("dom");
  expect(warn).toHaveBeenCalledTimes(2);
  warn.mockRestore();
});

test("perda do contexto descarta o addon e volta ao DOM", () => {
  const addon = new FakeWebgl();
  const changes: string[] = [];
  loadWebgl(
    () => {},
    () => addon,
    (r) => changes.push(r),
  );
  addon.loseContext();
  expect(addon.disposed).toBe(true);
  expect(changes).toEqual(["dom"]);
  // Um segundo aviso (já descartado) não faz nada.
  addon.loseContext();
  expect(changes).toEqual(["dom"]);
});
