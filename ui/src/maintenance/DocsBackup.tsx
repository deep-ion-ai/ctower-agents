// Backup dos docs dos projetos (#444, backend na #442): o último pacote
// (data, tamanho, local e remoto), a agenda, a retenção e o limite. A
// leitura (`docsBackupFacts`, `DocsBackupFacts`) serve também à PWA; editar
// e "Fazer backup agora" ficam na Manutenção do desktop.
import { useEffect, useState } from "react";
import type { DocsBackupStatus, Warning } from "../protocol/hub-gen";
import { useTower } from "../state/tower";
import { humanBytes } from "../util/bytes";
import { ago } from "../util/time";

export type FactTone = "ok" | "error" | "neutral";

export interface DocsBackupFact {
  label: string;
  value: string;
  tone?: FactTone;
  mono?: boolean;
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** Aviso de `hub.status` do backup dos docs (vai para este bloco, não para o do hub.db). */
export const isDocsBackupWarning = (kind: string) => kind.startsWith("docs_backup_");

/** Os fatos do bloco, em texto (a PWA e a Manutenção mostram os mesmos). */
export function docsBackupFacts(d: DocsBackupStatus, now: number): DocsBackupFact[] {
  if (!d.docs_dir) {
    return [{ label: "Pasta dos docs", value: "docs_dir não configurado no hub: sem backup dos docs", tone: "neutral" }];
  }
  const local: DocsBackupFact = d.last_error
    ? { label: "Último pacote", value: `falhou: ${d.last_error}`, tone: "error" }
    : d.last_ok_at !== null
      ? {
          label: "Último pacote",
          value: [ago(d.last_ok_at, now), d.newest_name, d.last_size !== null ? humanBytes(d.last_size) : null]
            .filter(Boolean)
            .join(" · "),
          tone: "ok",
        }
      : d.newest_at !== null
        ? { label: "Último pacote", value: `${ago(d.newest_at, now)} · ${d.newest_name ?? ""}`, tone: "ok" }
        : { label: "Último pacote", value: "nenhum ainda", tone: "neutral" };
  const skipped: DocsBackupFact[] = d.last_skipped
    ? [
        {
          label: "Última rodada",
          value:
            d.too_large_bytes !== null
              ? `não rodou: ${humanBytes(d.too_large_bytes)} de docs, acima do limite de ${d.max_mb} MB`
              : `não rodou: ${d.last_skipped}`,
          tone: d.too_large_bytes !== null ? "error" : "neutral",
        },
      ]
    : [];
  const r = d.remote;
  const remote: DocsBackupFact = r.last_error
    ? { label: "Envio remoto", value: `falhou (${r.failures}×): ${r.last_error}`, tone: "error" }
    : r.last_ok_at !== null
      ? {
          label: "Envio remoto",
          value: [ago(r.last_ok_at, now), r.last_name, r.last_size !== null ? humanBytes(r.last_size) : null]
            .filter(Boolean)
            .join(" · "),
          tone: "ok",
        }
      : { label: "Envio remoto", value: "ainda não enviado (ou sem destino remoto)", tone: "neutral" };
  return [
    { label: "Pasta dos docs", value: d.docs_dir, mono: true },
    local,
    ...skipped,
    remote,
    {
      label: "Agenda",
      value: d.interval_hours === 0 ? "desligado" : `a cada ${plural(d.interval_hours, "hora", "horas")}`,
      tone: d.interval_hours === 0 ? "error" : undefined,
    },
    { label: "Retenção", value: `${plural(d.keep, "pacote", "pacotes")} mais novos, local e remoto` },
    { label: "Limite", value: `${d.max_mb} MB de docs` },
  ];
}

const TONE_CLASS: Record<FactTone, string> = { ok: "state-on", error: "state-off", neutral: "muted" };

/** Os fatos, só leitura. */
export function DocsBackupFacts({ docs, now }: { docs: DocsBackupStatus; now: number }) {
  return (
    <div className="env-row" aria-label="Estado do backup dos docs" role="group">
      {docsBackupFacts(docs, now).map((f) => (
        <div className="fact" key={f.label}>
          <span className="label">{f.label}</span>
          <span className={[f.mono ? "mono" : "", f.tone ? TONE_CLASS[f.tone] : ""].filter(Boolean).join(" ") || undefined}>
            {f.value}
          </span>
        </div>
      ))}
    </div>
  );
}

/** Avisos `docs_backup_*` de `hub.status`, no bloco. */
export function DocsBackupWarnings({ warnings }: { warnings: Warning[] }) {
  const mine = warnings.filter((w) => isDocsBackupWarning(w.kind));
  if (mine.length === 0) return null;
  return (
    <ul className="hub-warnings" aria-label="Avisos do backup dos docs">
      {mine.map((w, i) => (
        <li key={`${w.kind}:${i}`} data-kind={w.kind} className="state-off">
          {w.message}
        </li>
      ))}
    </ul>
  );
}

/**
 * PWA (#444): o estado do backup dos docs, só leitura (`hub.backup.get` é da
 * lista da web). Some com o hub sem docs ou anterior à #442.
 */
export function DocsBackupReadOnly({ now }: { now: number }) {
  const { api, hub } = useTower();
  const [docs, setDocs] = useState<DocsBackupStatus | null>(null);
  const connected = hub.connected;
  useEffect(() => {
    if (!connected) return;
    let alive = true;
    api
      .hubCall<{ docs?: DocsBackupStatus }>("hub.backup.get")
      .then((v) => alive && setDocs(v.docs ?? null))
      .catch(() => alive && setDocs(null));
    return () => {
      alive = false;
    };
  }, [api, connected]);
  if (!docs?.docs_dir) return null;
  return (
    <section className="hub-card" aria-labelledby="hub-ov-docs-backup">
      <div className="hub-card-head">
        <h2 id="hub-ov-docs-backup">Backup dos docs</h2>
      </div>
      <div className="hub-card-form">
        <DocsBackupFacts docs={docs} now={now} />
        <DocsBackupWarnings warnings={hub.health?.warnings ?? []} />
        <p className="muted hint">Agenda, retenção e “Fazer backup agora” ficam na Manutenção do app desktop.</p>
      </div>
    </section>
  );
}
