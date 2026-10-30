import { act, fireEvent, render, waitFor } from "@testing-library/react";
import { FakeDaemonApi } from "../api/fake";
import { TowerProvider } from "../state/tower";
import { dashboard } from "../test/fixtures";
import { lastTermSize } from "./size";
import { setTermPrefs } from "./prefs";
import { CLOSE_DELAY_MS, TerminalView } from "./TerminalView";

// O xterm falso mede sempre 100×30.

const S = "cccccccc-0000-0000-0000-000000000003";

function view(api: FakeDaemonApi, visible: boolean) {
  return (
    <TowerProvider api={api}>
      <TerminalView env="wsl" pty={S} visible={visible} onState={() => {}} />
    </TowerProvider>
  );
}

test("aba escondida não anexa; ao aparecer, ajusta o PTY e só então anexa", async () => {
  const api = new FakeDaemonApi().on("dashboard.get", dashboard());
  const { rerender } = render(view(api, false));
  await act(() => new Promise((r) => setTimeout(r, 20)));
  expect(api.attached.has(S)).toBe(false);
  expect(api.resizes).toEqual([]);

  rerender(view(api, true));
  await waitFor(() => expect(api.attached.has(S)).toBe(true));
  expect(api.resizes).toEqual([{ session: S, cols: 100, rows: 30 }]);
  expect(lastTermSize()).toEqual({ cols: 100, rows: 30 });

  // Esconder e mostrar de novo não anexa outra vez.
  let attaches = 0;
  api.attach = async () => {
    attaches++;
  };
  rerender(view(api, false));
  rerender(view(api, true));
  await act(() => new Promise((r) => setTimeout(r, 20)));
  expect(attaches).toBe(0);
});

// Fim da sessão (#432): saída limpa fecha a aba depois de um instante.
async function exitedView(data: Record<string, unknown>) {
  const api = new FakeDaemonApi().on("dashboard.get", dashboard());
  const onClose = vi.fn();
  const r = render(
    <TowerProvider api={api}>
      <TerminalView env="wsl" pty={S} visible onState={() => {}} onClose={onClose} />
    </TowerProvider>,
  );
  await waitFor(() => expect(api.attached.has(S)).toBe(true));
  vi.useFakeTimers();
  act(() => api.push("wsl", "session_exited", { session_id: S, ...data }));
  return { ...r, onClose };
}

afterEach(() => vi.useRealTimers());

test("sessão que sai com código 0 fecha a aba depois do atraso", async () => {
  const { onClose } = await exitedView({ exit_code: 0, state: "exited" });
  act(() => vi.advanceTimersByTime(CLOSE_DELAY_MS - 1));
  expect(onClose).not.toHaveBeenCalled();
  act(() => vi.advanceTimersByTime(1));
  expect(onClose).toHaveBeenCalledTimes(1);
});

test.each([
  ["código ≠ 0", { exit_code: 1, state: "exited" }],
  ["sinal", { exit_code: null, state: "exited" }],
  ["interrompida", { exit_code: 0, state: "interrupted" }],
])("sessão que sai com %s mantém a aba", async (_, data) => {
  const { onClose } = await exitedView(data);
  act(() => vi.advanceTimersByTime(CLOSE_DELAY_MS * 5));
  expect(onClose).not.toHaveBeenCalled();
});

test("attach perdido mantém a aba", async () => {
  const api = new FakeDaemonApi().on("dashboard.get", dashboard());
  const onClose = vi.fn();
  render(
    <TowerProvider api={api}>
      <TerminalView env="wsl" pty={S} visible onState={() => {}} onClose={onClose} />
    </TowerProvider>,
  );
  await waitFor(() => expect(api.attached.has(S)).toBe(true));
  vi.useFakeTimers();
  act(() => api.push("wsl", "attach_lost", { session_id: S, reason: "x" }));
  act(() => vi.advanceTimersByTime(CLOSE_DELAY_MS * 5));
  expect(onClose).not.toHaveBeenCalled();
});

test("interagir com o console no intervalo cancela o fechamento", async () => {
  const { onClose, getByRole } = await exitedView({ exit_code: 0, state: "exited" });
  fireEvent.keyDown(getByRole("region", { name: "Terminal da sessão" }), { key: "a" });
  act(() => vi.advanceTimersByTime(CLOSE_DELAY_MS * 5));
  expect(onClose).not.toHaveBeenCalled();
});

test("aba escondida (em segundo plano) também fecha", async () => {
  const api = new FakeDaemonApi().on("dashboard.get", dashboard());
  const onClose = vi.fn();
  const v = (visible: boolean) => (
    <TowerProvider api={api}>
      <TerminalView env="wsl" pty={S} visible={visible} onState={() => {}} onClose={onClose} />
    </TowerProvider>
  );
  const { rerender } = render(v(true));
  await waitFor(() => expect(api.attached.has(S)).toBe(true));
  rerender(v(false));
  vi.useFakeTimers();
  act(() => api.push("wsl", "session_exited", { session_id: S, exit_code: 0, state: "exited" }));
  act(() => vi.advanceTimersByTime(CLOSE_DELAY_MS));
  expect(onClose).toHaveBeenCalledTimes(1);
});

test("com a preferência desligada, a aba fica", async () => {
  setTermPrefs({ closeOnExit: false });
  const { onClose } = await exitedView({ exit_code: 0, state: "exited" });
  act(() => vi.advanceTimersByTime(CLOSE_DELAY_MS * 5));
  expect(onClose).not.toHaveBeenCalled();
});

test("shell que sai com código 0 mantém a aba com \"shell encerrado\" (#432 × #423)", async () => {
  const api = new FakeDaemonApi().on("dashboard.get", dashboard());
  const onClose = vi.fn();
  const states: string[] = [];
  render(
    <TowerProvider api={api}>
      <TerminalView
        env="wsl"
        pty={S}
        kind="shell"
        visible
        onState={(s) => s.kind === "ended" && states.push(s.message)}
        onClose={onClose}
      />
    </TowerProvider>,
  );
  await waitFor(() => expect(api.attached.has(S)).toBe(true));
  vi.useFakeTimers();
  act(() => api.push("wsl", "shell_exited", { shell_id: S, code: 0 }));
  act(() => vi.advanceTimersByTime(CLOSE_DELAY_MS * 5));
  expect(states).toEqual(["shell encerrado (código 0)"]);
  expect(onClose).not.toHaveBeenCalled();
});
