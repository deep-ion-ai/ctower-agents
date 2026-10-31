import { mask, maskLine } from "./mask";
import {
  appendLines,
  classifyError,
  exportContent,
  exportName,
  lineText,
  matches,
  parseSourceKey,
  sourceKey,
  tailParams,
} from "./sources";
import type { LogLine } from "./types";
import { UiLogStore, logCalls } from "./uiLog";
import { FakeDaemonApi } from "../api/fake";

const line = (id: number, level: string, message: string, extra: Partial<LogLine> = {}): LogLine => ({
  id,
  ts: Date.UTC(2026, 9, 31, 14, 3, 7, 120),
  level,
  target: "ct_server::teste",
  message,
  ...extra,
});

test("máscara: os mesmos casos do ct_http::logs::mask", () => {
  const hex = "a".repeat(64);
  const cases: [string, string][] = [
    [`ui token ${hex} lido`, "ui token *** lido"],
    ["Authorization: Bearer abc.def-123", "Authorization: Bearer ***"],
    ["gh token ghp_abcdefghijklmnopqrstuvwxyz0123", "gh token ***"],
    ["url https://x/y?token=s3cr3t&a=1", "url https://x/y?token=***&a=1"],
    ['{"password": "hunter2"}', '{"password": "***"}'],
    ["hub_token=abc123 ok", "hub_token=*** ok"],
    ["AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQ", "***"],
    ["glpat-abcdefghijklmnopqrst e sk-ant-api03-abcdefghijklmnop", "*** e ***"],
    ["hub http://eu:s3nha@hub:7879/ws", "hub http://***@hub:7879/ws"],
  ];
  for (const [i, o] of cases) expect(mask(i)).toBe(o);
  const plain = "secrets: chave do ambiente trocada (age1xyz) commit 0123456789abcdef0123456789abcdef01234567";
  expect(mask(plain)).toBe(plain);
  // Duas vezes seguidas dá o mesmo (as regex são globais).
  expect(mask("token=x token=y")).toBe("token=*** token=***");
  expect(mask("token=x token=y")).toBe("token=*** token=***");
});

test("máscara dos campos: nome de secret sai inteiro, o resto passa pela máscara", () => {
  const m = maskLine(line(1, "info", "x", { fields: { hub_token: "abc", env: "dev", url: "http://h/?secret=abc", n: 2 } }));
  expect(m.fields).toEqual({ hub_token: "***", env: "dev", url: "http://h/?secret=***", n: 2 });
});

test("fonte: chave estável e de volta", () => {
  expect(sourceKey({ kind: "env", env: "wsl" })).toBe("env:wsl");
  expect(parseSourceKey("env:wsl")).toEqual({ kind: "env", env: "wsl" });
  expect(parseSourceKey("hub")).toEqual({ kind: "hub" });
  expect(parseSourceKey("env:")).toBeNull();
  expect(parseSourceKey("x")).toBeNull();
});

test("filtro: nível mínimo e texto na mensagem, no target ou nos campos; parâmetros sem vazios", () => {
  const f = { level: "warn" as const, q: " HUB " };
  expect(matches(line(1, "error", "o hub caiu"), f)).toBe(true);
  expect(matches(line(1, "info", "o hub caiu"), f)).toBe(false);
  expect(matches(line(1, "warn", "x", { fields: { alvo: "hub" } }), f)).toBe(true);
  expect(matches(line(1, "warn", "nada"), f)).toBe(false);
  expect(tailParams({ level: "", q: "  " }, { limit: 5 })).toEqual({ limit: 5 });
  expect(tailParams(f, { before: 9 })).toEqual({ before: 9, level: "warn", q: "HUB" });
});

test("juntar linhas: sem repetir id e cortando as mais antigas", () => {
  const cur = [line(1, "info", "a"), line(2, "info", "b")];
  const r = appendLines(cur, [line(2, "info", "b"), line(3, "info", "c"), line(4, "info", "d")], 3);
  expect(r.lines.map((l) => l.id)).toEqual([2, 3, 4]);
  expect(r.dropped).toBe(1);
  expect(appendLines(cur, [line(1, "info", "a")], 10).lines).toBe(cur);
});

test("exportar: .log em texto e .jsonl, mascarados e sem o marcador da execução anterior", () => {
  const ls = [line(1, "warn", "token=abc", { fields: { env: "dev" } }), line(2, "info", "ok", { previous: true })];
  expect(lineText(ls[0])).toBe("2026-10-31T14:03:07.120Z WARN  ct_server::teste: token=*** env=dev");
  const log = exportContent(ls, "log");
  expect(log.trim().split("\n")).toHaveLength(2);
  const jsonl = exportContent(ls, "jsonl").trim().split("\n").map((l) => JSON.parse(l));
  expect(jsonl[0]).toMatchObject({ id: 1, level: "warn", message: "token=***" });
  expect(jsonl[1]).not.toHaveProperty("previous");
  expect(exportName("Ambiente: ctower-agents (WSL)", "jsonl", new Date(2026, 9, 31, 14, 3))).toBe(
    "ctower-logs-ambiente-ctower-agents-wsl-20261031-1403.jsonl",
  );
});

test("erros da fonte: offline, log desligado, daemon antigo e o resto", () => {
  expect(classifyError("ambiente offline: wsl está sem canal com o hub agora").kind).toBe("offline");
  expect(classifyError("log em arquivo desligado neste daemon").kind).toBe("disabled");
  expect(classifyError("método desconhecido: logs.tail").kind).toBe("outdated");
  expect(classifyError("método indisponível pelo hub: logs.tail").kind).toBe("outdated");
  expect(classifyError("boom")).toEqual({ kind: "error", message: "boom" });
});

test("buffer da UI: anel, máscara e avisos a quem acompanha", () => {
  const log = new UiLogStore(3);
  const seen: LogLine[][] = [];
  const off = log.subscribe((l) => seen.push(l));
  for (let i = 0; i < 5; i++) log.add("info", "ui", `linha ${i}`);
  log.add("error", "ui.console", "Bearer xyz falhou", { token: "t" });
  expect(log.snapshot().map((l) => l.message)).toEqual(["linha 3", "linha 4", "Bearer *** falhou"]);
  expect(log.snapshot()[2].fields).toEqual({ token: "***" });
  expect(seen).toHaveLength(6);
  off();
});

test("buffer da UI no desktop: a execução anterior antes desta, e grava as linhas no arquivo", async () => {
  vi.useFakeTimers();
  try {
    const log = new UiLogStore();
    log.add("warn", "ui", "antes de ligar o arquivo");
    const appended: string[][] = [];
    const prev = JSON.stringify(line(7, "error", "da execução anterior token=x"));
    await log.attach({
      uiLogRead: async () => [prev, "lixo"],
      uiLogAppend: async (l) => {
        appended.push(l);
      },
    });
    const snap = log.snapshot();
    expect(snap.map((l) => [l.message, !!l.previous])).toEqual([
      ["da execução anterior token=***", true],
      ["antes de ligar o arquivo", false],
    ]);
    expect(snap[0].id).toBeLessThan(snap[1].id);
    log.add("info", "ui", "depois");
    await vi.advanceTimersByTimeAsync(1000);
    expect(appended.flat().map((l) => JSON.parse(l).message)).toEqual(["antes de ligar o arquivo", "depois"]);
  } finally {
    vi.useRealTimers();
  }
});

test("logCalls: falha do protocolo vai para o log da UI e a promessa continua rejeitada", async () => {
  const api = logCalls(new FakeDaemonApi().withHub());
  const before = (await import("./uiLog")).uiLog.snapshot().length;
  await expect(api.call("wsl", "nada.existe")).rejects.toThrow("método sem resposta");
  await expect(api.hubCall("hub.nada")).rejects.toThrow();
  const { uiLog } = await import("./uiLog");
  const added = uiLog.snapshot().slice(before);
  expect(added.map((l) => [l.level, l.target, l.message.split(":")[0]])).toEqual([
    ["warn", "ui.protocol", "wsl › nada.existe"],
    ["warn", "ui.protocol", "hub › hub.nada"],
  ]);
  api.on("ok", 1);
  await expect(api.call("wsl", "ok")).resolves.toBe(1);
});
