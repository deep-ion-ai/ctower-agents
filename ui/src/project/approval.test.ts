import { awaitingApproval, enforcedLabel, hasLabel, isApprovalRefusal } from "./approval";

const exige = { approval: { label: " Aprovada ", required: true } };
const item = (labels: string[], state = "open", status: string | null = null) => ({
  issue: { state, labels },
  work_item: status ? { status } : null,
});

test("exigência: só com a opção ligada e rótulo não vazio; hub ou ambiente anterior não exige", () => {
  expect(enforcedLabel(exige)).toBe("Aprovada");
  expect(enforcedLabel({ approval: { label: "aprovada", required: false } })).toBeNull();
  expect(enforcedLabel({ approval: { label: "  ", required: true } })).toBeNull();
  expect(enforcedLabel({})).toBeNull();
  expect(enforcedLabel(null)).toBeNull();
});

test("selo: issue aberta sem o rótulo (sem diferenciar maiúsculas); fechada ou concluída não", () => {
  expect(hasLabel(["bug", "aprovada "], "Aprovada")).toBe(true);
  expect(awaitingApproval(item(["bug"]), exige)).toBe(true);
  expect(awaitingApproval(item(["APROVADA"]), exige)).toBe(false);
  expect(awaitingApproval(item(["bug"], "closed"), exige)).toBe(false);
  expect(awaitingApproval(item(["bug"], "open", "concluido"), exige)).toBe(false);
  expect(awaitingApproval(item([]), { approval: { label: "aprovada", required: false } })).toBe(false);
});

test("recusa do ambiente reconhecida pelo início fixo da mensagem", () => {
  expect(isApprovalRefusal('aguardando aprovação: ACME-123 não tem o rótulo "aprovada"')).toBe(true);
  expect(isApprovalRefusal("Error: aguardando aprovação: #7")).toBe(true);
  expect(isApprovalRefusal("worktree já existe")).toBe(false);
});
