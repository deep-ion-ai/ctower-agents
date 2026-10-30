import { expect, test } from "vitest";
import { HUB_DOCS_BACKUP, NOW } from "../test/fixtures";
import { docsBackupFacts, isDocsBackupWarning } from "./DocsBackup";
import { isBackupWarning } from "./sections";

const text = (f: ReturnType<typeof docsBackupFacts>) => Object.fromEntries(f.map((x) => [x.label, x.value]));

test("sem docs_dir: um fato só, sem agenda", () => {
  const f = docsBackupFacts(HUB_DOCS_BACKUP, NOW);
  expect(f).toHaveLength(1);
  expect(f[0].value).toMatch(/docs_dir não configurado/);
});

test("acima do limite: a rodada não gerou pacote, com o tamanho; agenda desligada é destacada", () => {
  const f = docsBackupFacts(
    { ...HUB_DOCS_BACKUP, docs_dir: "/srv/d", interval_hours: 0, last_skipped: "acima do limite", too_large_bytes: 2 * 1024 ** 3 },
    NOW,
  );
  const t = text(f);
  expect(t["Último pacote"]).toBe("nenhum ainda");
  expect(t["Última rodada"]).toBe("não rodou: 2,0 GB de docs, acima do limite de 1024 MB");
  expect(t["Agenda"]).toBe("desligado");
  expect(f.find((x) => x.label === "Agenda")?.tone).toBe("error");
  expect(t["Retenção"]).toBe("7 pacotes mais novos, local e remoto");
});

test("avisos docs_backup_* são do backup (Manutenção) e do bloco dos docs", () => {
  for (const k of ["docs_backup_failed", "docs_backup_stale", "docs_backup_too_large"]) {
    expect(isBackupWarning(k)).toBe(true);
    expect(isDocsBackupWarning(k)).toBe(true);
  }
  expect(isDocsBackupWarning("backup_failed")).toBe(false);
  expect(isBackupWarning("docs_unreachable")).toBe(false);
});
