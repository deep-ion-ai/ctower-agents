import { FakeDaemonApi } from "../api/fake";
import { session } from "../test/fixtures";
import { REDRAW_DELAY_MS, RESIZE_DEBOUNCE_MS, TerminalAdapter, type TermLike } from "./adapter";

/** xterm falso: guarda o texto visível (reset limpa). */
class FakeTerm implements TermLike {
  screen = "";
  resets = 0;
  private cb: ((d: string) => void) | null = null;
  write(d: Uint8Array | string) {
    this.screen += typeof d === "string" ? d : new TextDecoder().decode(d);
  }
  reset() {
    this.screen = "";
    this.resets++;
  }
  onData(cb: (d: string) => void) {
    this.cb = cb;
    return { dispose: () => (this.cb = null) };
  }
  type(d: string) {
    this.cb?.(d);
  }
}

const S = "11111111-1111-1111-1111-111111111111";

async function setup() {
  const api = new FakeDaemonApi();
  const term = new FakeTerm();
  const states: string[] = [];
  const a = new TerminalAdapter(api, "wsl", S, term, (s) => states.push(s.kind));
  await a.start();
  return { api, term, a, states };
}

test("snapshot limpa o terminal antes de escrever", async () => {
  const { api, term, states } = await setup();
  api.output(S, "historico\r\n", true);
  api.output(S, "ao vivo\r\n");
  expect(term.screen).toBe("historico\r\nao vivo\r\n");
  expect(term.resets).toBe(1);
  expect(states).toContain("live");
});

test("re-attach após reconexão não duplica a saída", async () => {
  const { api, term } = await setup();
  api.output(S, "um\r\n", true);
  api.output(S, "dois\r\n");
  // Reconectou: o daemon reenvia o scrollback inteiro como snapshot.
  api.output(S, "um\r\ndois\r\n", true);
  api.output(S, "tres\r\n");
  expect(term.screen).toBe("um\r\ndois\r\ntres\r\n");
});

test("entrada vai para a sessão certa", async () => {
  const { api, term } = await setup();
  term.type("ls\r");
  expect(api.inputs).toEqual([{ env: "wsl", session: S, data: "ls\r" }]);
});

test("sessão encerrada bloqueia a entrada e mostra o código", async () => {
  const { api, term, a, states } = await setup();
  a.exited(1);
  expect(term.screen).toContain("sessão encerrada (código 1)");
  term.type("x");
  expect(api.inputs).toEqual([]);
  expect(states[states.length - 1]).toBe("ended");
  // Encerrada não faz detach (já não há o que soltar).
  a.dispose();
  expect(api.attached.has(S)).toBe(true);
});

test("resize com debounce manda só o último tamanho", async () => {
  vi.useFakeTimers();
  try {
    const { api, a } = await setup();
    a.resize(80, 24);
    a.resize(100, 30);
    a.resize(120, 40);
    vi.advanceTimersByTime(RESIZE_DEBOUNCE_MS);
    await Promise.resolve();
    expect(api.resizes).toEqual([{ session: S, cols: 120, rows: 40 }]);
  } finally {
    vi.useRealTimers();
  }
});

test("attach que falha vira estado encerrado com o motivo", async () => {
  const api = new FakeDaemonApi();
  api.attach = async () => {
    throw "sessão não está rodando";
  };
  const term = new FakeTerm();
  const a = new TerminalAdapter(api, "wsl", S, term);
  await a.start();
  expect(a.state).toEqual({
    kind: "ended",
    message: "não foi possível anexar: sessão não está rodando",
    recoverable: false,
  });
});

test("attach a sessão interrompida mostra o código e aponta a Recuperação, sem o erro cru (#283)", async () => {
  const api = new FakeDaemonApi().on("session.get", session({ id: S, state: "interrupted", exit_code: 137 }));
  api.attach = async () => {
    throw "sessão não está rodando";
  };
  const term = new FakeTerm();
  const a = new TerminalAdapter(api, "wsl", S, term);
  await a.start();
  expect(a.state).toEqual({
    kind: "ended",
    message: "sessão interrompida (código 137). Retome pela Recuperação",
    recoverable: true,
  });
  expect(term.screen).not.toContain("não foi possível anexar");
  expect(api.callsTo("session.get")).toEqual([{ env: "wsl", method: "session.get", params: { session_id: S } }]);
});

test("attach a sessão encerrada normalmente mostra o fim, sem Recuperação", async () => {
  const api = new FakeDaemonApi().on("session.get", session({ id: S, state: "exited", exit_code: 0 }));
  api.attach = async () => {
    throw "sessão não está rodando";
  };
  const a = new TerminalAdapter(api, "wsl", S, new FakeTerm());
  await a.start();
  expect(a.state).toEqual({ kind: "ended", message: "sessão encerrada (código 0)", recoverable: false });
});

test("attach a sessão interrompida de issue concluída não oferece a Recuperação (#297)", async () => {
  const api = new FakeDaemonApi().on(
    "session.get",
    { ...session({ id: S, state: "interrupted", exit_code: 137 }), work_item_concluded: true },
  );
  api.attach = async () => {
    throw "sessão não está rodando";
  };
  const a = new TerminalAdapter(api, "wsl", S, new FakeTerm());
  await a.start();
  expect(a.state).toEqual({
    kind: "ended",
    message: "sessão interrompida (código 137). Issue concluída; a sessão será arquivada pela limpeza",
    recoverable: false,
  });
});

test("saída por push de sessão interrompida de issue concluída não é recuperável (#297)", async () => {
  const { api, a } = await setup();
  api.on("session.get", { ...session({ id: S, state: "interrupted" }), work_item_concluded: true });
  await a.exited(null, "interrupted");
  expect(a.state).toMatchObject({ kind: "ended", recoverable: false });
});

test("saída anormal por push vira interrompida e recuperável", async () => {
  const { api, a, term } = await setup();
  api.on("session.get", session({ id: S, state: "interrupted" }));
  await a.exited(null, "interrupted");
  expect(term.screen).toContain("sessão interrompida (sinal). Retome pela Recuperação");
  expect(a.state).toMatchObject({ kind: "ended", recoverable: true });
});

test("dispose solta a sessão e para de escrever", async () => {
  const { api, term, a } = await setup();
  a.dispose();
  expect(api.attached.has(S)).toBe(false);
  a.chunk({ bytes: new TextEncoder().encode("depois"), snapshot: false });
  expect(term.screen).not.toContain("depois");
});

test("com tamanho, o resize vai antes do attach e sem debounce", async () => {
  const api = new FakeDaemonApi();
  const log: string[] = [];
  const resize = api.resize.bind(api);
  api.resize = async (env, s, cols, rows) => {
    log.push(`resize ${cols}x${rows}`);
    return resize(env, s, cols, rows);
  };
  const attach = api.attach.bind(api);
  api.attach = async (env, s, cb) => {
    log.push("attach");
    return attach(env, s, cb);
  };
  const a = new TerminalAdapter(api, "wsl", S, new FakeTerm());
  await a.start({ cols: 132, rows: 41 });
  expect(log).toEqual(["resize 132x41", "attach"]);
});

test("snapshot com histórico força o redesenho com resize de ida e volta", async () => {
  vi.useFakeTimers();
  try {
    const api = new FakeDaemonApi();
    const term = new FakeTerm();
    const a = new TerminalAdapter(api, "wsl", S, term);
    await a.start({ cols: 132, rows: 41 });
    api.output(S, "historico\r\n", true);
    expect(term.screen).toBe("historico\r\n");
    expect(api.resizes).toEqual([
      { session: S, cols: 132, rows: 41 },
      { session: S, cols: 132, rows: 40 },
    ]);
    vi.advanceTimersByTime(REDRAW_DELAY_MS);
    expect(api.resizes[2]).toEqual({ session: S, cols: 132, rows: 41 });
  } finally {
    vi.useRealTimers();
  }
});

test("snapshot vazio (sessão nova) não força redesenho", async () => {
  const api = new FakeDaemonApi();
  const a = new TerminalAdapter(api, "wsl", S, new FakeTerm());
  await a.start({ cols: 132, rows: 41 });
  api.output(S, "", true);
  expect(api.resizes).toEqual([{ session: S, cols: 132, rows: 41 }]);
});

test("shell (#423): fim com o código do push e, sem o processo no daemon, só \"shell encerrado\"", async () => {
  const api = new FakeDaemonApi();
  const term = new FakeTerm();
  const ended: string[] = [];
  const a = new TerminalAdapter(api, "wsl", S, term, (s) => s.kind === "ended" && ended.push(s.message), "shell");
  await a.start();
  await a.exited(130);
  expect(ended).toEqual(["shell encerrado (código 130)"]);
  expect(term.screen).toContain("[shell encerrado (código 130)]");
  term.type("ls");
  expect(api.inputs).toEqual([]);

  // O shell saiu com o app fechado: o attach falha e não há `session.get` a consultar.
  const gone = new FakeDaemonApi();
  gone.attach = async () => {
    throw new Error("sessão não está rodando");
  };
  const b = new TerminalAdapter(gone, "wsl", S, new FakeTerm(), (s) => s.kind === "ended" && ended.push(s.message), "shell");
  await b.start();
  expect(ended[1]).toBe("shell encerrado");
  expect(gone.callsTo("session.get")).toEqual([]);
});

test("saída limpa por push (exited, código 0) marca o fim como limpo (#432)", async () => {
  const { a, term } = await setup();
  await a.exited(0, "exited");
  expect(term.screen).toContain("sessão encerrada (código 0)");
  expect(a.state).toEqual({ kind: "ended", message: "sessão encerrada (código 0)", recoverable: false, clean: true });
});

test.each([
  ["código ≠ 0", 1, "exited"],
  ["sinal", null, "exited"],
  ["interrompida", 0, "interrupted"],
  ["sem estado", 0, undefined],
] as const)("fim por push com %s não é limpo (#432)", async (_, code, state) => {
  const { api, a } = await setup();
  api.on("session.get", session({ id: S, state: "interrupted" }));
  await a.exited(code, state);
  expect(a.state.kind).toBe("ended");
  expect(a.state).not.toHaveProperty("clean");
});

test("attach a sessão já encerrada com código 0 não é fim limpo (a aba fica) (#432)", async () => {
  const api = new FakeDaemonApi().on("session.get", session({ id: S, state: "exited", exit_code: 0 }));
  api.attach = async () => {
    throw "sessão não está rodando";
  };
  const a = new TerminalAdapter(api, "wsl", S, new FakeTerm());
  await a.start();
  expect(a.state).not.toHaveProperty("clean");
});

test("shell que sai com código 0 não é fim limpo: a aba fica com as ações (#432 × #423)", async () => {
  const api = new FakeDaemonApi();
  const a = new TerminalAdapter(api, "wsl", S, new FakeTerm(), () => {}, "shell");
  await a.start();
  await a.exited(0);
  expect(a.state).toEqual({ kind: "ended", message: "shell encerrado (código 0)", recoverable: false });
});
