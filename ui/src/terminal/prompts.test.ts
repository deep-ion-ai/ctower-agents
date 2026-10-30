import { DEFAULT_PROMPTS } from "../settings/defaults";
import { project, session, workItem } from "../test/fixtures";
import { promptActions, promptIdOf } from "./actions";
import { pasteSequence, PERMISSION_PENDING, promptBlocked, renderPrompt, sendReadyPrompt } from "./prompts";


function recorder() {
  const sent: string[] = [];
  return { sent, api: { input: async (_env: string, _s: string, data: string) => void sent.push(data) } };
}
const noWait = () => Promise.resolve();

test("colagem: marcadores em volta, CRLF vira LF e marcadores no meio saem", () => {
  expect(pasteSequence("a\r\nb\rc\nd")).toBe("\x1b[200~a\nb\nc\nd\x1b[201~");
  expect(pasteSequence("x\x1b[201~y\x1b[200~")).toBe("\x1b[200~xy\x1b[201~");
});

test("envio: a colagem e depois o \\r, em duas escritas, com a espera entre elas", async () => {
  const { sent, api } = recorder();
  const waits: number[] = [];
  await sendReadyPrompt(api, "wsl", "s1", "linha 1\nlinha 2", { live: true, permissionPending: false }, async (ms) => {
    waits.push(ms);
    expect(sent).toHaveLength(1);
  });
  expect(sent).toEqual(["\x1b[200~linha 1\nlinha 2\x1b[201~", "\r"]);
  expect(waits).toHaveLength(1);
});

test("recusa sem enviar nada com permissão pendente ou sessão encerrada", async () => {
  const { sent, api } = recorder();
  await expect(sendReadyPrompt(api, "wsl", "s1", "continue", { live: true, permissionPending: true }, noWait)).rejects.toThrow(
    PERMISSION_PENDING,
  );
  await expect(sendReadyPrompt(api, "wsl", "s1", "continue", { live: false, permissionPending: false }, noWait)).rejects.toThrow(
    "encerrada",
  );
  expect(sent).toEqual([]);
  expect(promptBlocked({ live: true, permissionPending: false })).toBeNull();
});

test("variáveis dos templates na sessão de issue, do Jira e do gestor", () => {
  const wi = workItem({ gh_number: 32, title: "terminal", branch: "feat/32-x" });
  const p = project();
  const text = "{{kind}} {{key}} {{number}} {{branch}} {{base}} {{owner}}/{{repo}} {{title}} {{nada}}";
  expect(renderPrompt(text, { s: session(), wi, project: p }, "WSL")).toBe(
    "issue #32 32 feat/32-x develop deep-ion-ai/ctower-agents terminal {{nada}}",
  );
  const jira = project({
    tracker: { kind: "jira", url: "https://jira.x", project_key: "BN", credential_cmd: "", config_file: "" },
  } as never);
  expect(renderPrompt("{{key}}", { wi, project: jira }, "WSL")).toBe("BN-32");
  expect(renderPrompt("{{env}} {{branch}}", { gestor: true, scope: { kind: "health" } }, "Mac mini")).toBe("Mac mini {{branch}}");
  // Sem work item nem gestor: o texto como está.
  expect(renderPrompt("{{number}}", {}, "WSL")).toBe("{{number}}");
});

test("ações da paleta: uma por prompt, desligadas com o motivo", () => {
  const on = promptActions(DEFAULT_PROMPTS, null);
  expect(on.map((a) => a.label)).toEqual([
    "Enviar prompt — Continuar",
    "Enviar prompt — Rodar os testes e corrigir",
    "Enviar prompt — Registrar as decisões no ledger",
    "Enviar prompt — Abrir o PR",
    "Enviar prompt — Resumir o estado atual",
  ]);
  expect(on.every((a) => !a.disabled && a.place === "prompt")).toBe(true);
  expect(promptIdOf(on[0].id)).toBe("continue");
  expect(promptIdOf("restart")).toBeNull();
  const off = promptActions(DEFAULT_PROMPTS, PERMISSION_PENDING);
  expect(off.every((a) => a.disabled && a.title === PERMISSION_PENDING)).toBe(true);
});

test("o padrão Continuar manda continue", () => {
  expect(DEFAULT_PROMPTS[0]).toEqual({ id: "continue", name: "Continuar", text: "continue" });
});
