import "../settings/settings.css";
import "../hub/hub.css";
import { useCallback, useEffect, useState } from "react";
import { useNow } from "../App";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import type { BackupConfigView, RemoteKind, RemoteSettings } from "../protocol/hub-gen";
import { errText } from "../hub/calls";
import { DocsBackupFacts, DocsBackupWarnings } from "./DocsBackup";

const KINDS: { value: RemoteKind; label: string }[] = [
  { value: "none", label: "Disco local" },
  { value: "drive", label: "Google Drive" },
  { value: "b2", label: "Backblaze B2" },
  { value: "s3", label: "Amazon S3 e compatíveis" },
];

const CRED_LABEL: Record<string, string> = {
  access_key: "Access key (key id)",
  secret_key: "Secret key (application key)",
  client_id: "Client ID do OAuth",
  client_secret: "Client secret do OAuth",
  refresh_token: "Refresh token",
};

const CRED_FIELDS: Record<RemoteKind, string[]> = {
  none: [],
  s3: ["access_key", "secret_key"],
  b2: ["access_key", "secret_key"],
  drive: ["client_id", "client_secret", "refresh_token"],
};

const bytes = (n: number) => (n >= 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`);

/**
 * Backup do hub (issue #263): destino (disco local, Drive, B2, S3), agenda,
 * retenção e credenciais. As credenciais vão para o store de secrets do hub
 * e nunca voltam: a tela só sabe se estão configuradas.
 */
export function Backup() {
  const { api, hub } = useTower();
  const now = useNow();
  const [view, setView] = useState<BackupConfigView | null>(null);
  const [kind, setKind] = useState<RemoteKind>("none");
  const [remote, setRemote] = useState<RemoteSettings | null>(null);
  const [dir, setDir] = useState("");
  const [hours, setHours] = useState("6");
  const [keep, setKeep] = useState("28");
  const [creds, setCreds] = useState<Record<string, string>>({});
  // Backup dos docs (#444): agenda, retenção e limite, salvos à parte.
  const [docsHours, setDocsHours] = useState("24");
  const [docsKeep, setDocsKeep] = useState("7");
  const [docsMax, setDocsMax] = useState("1024");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  /** Onde mostrar o resultado da última ação: no hub.db ou no bloco dos docs. */
  const [where, setWhere] = useState<"hub" | "docs">("hub");

  const apply = useCallback((v: BackupConfigView) => {
    setView(v);
    setKind(v.remote.kind);
    setRemote(v.remote);
    setDir(v.status.dir);
    setHours(String(v.status.interval_hours));
    setKeep(String(v.status.keep));
    setCreds({});
    // Hub anterior à #442 não manda `docs`.
    if (v.docs) {
      setDocsHours(String(v.docs.interval_hours));
      setDocsKeep(String(v.docs.keep));
      setDocsMax(String(v.docs.max_mb));
    }
  }, []);

  const load = useCallback(async () => {
    try {
      apply(await api.hubCall<BackupConfigView>("hub.backup.get"));
    } catch (e) {
      setError(errText(e));
    }
  }, [api, apply]);

  useEffect(() => {
    void load();
  }, [load]);

  if (!view || !remote) {
    return (
      <section aria-labelledby="hub-backup" className="panel settings-section">
        <h2 id="hub-backup">Backup do hub</h2>
        {error ? (
          <p role="alert" className="start-error">
            {error}
          </p>
        ) : (
          <p className="muted">Carregando…</p>
        )}
      </section>
    );
  }

  const st = view.status;
  const set = (p: Partial<RemoteSettings>) => setRemote({ ...remote, ...p });
  const payload = () => ({
    dir,
    interval_hours: Number(hours),
    keep: Number(keep),
    remote: { ...remote, kind },
    credentials: Object.fromEntries(Object.entries(creds).filter(([, v]) => v !== "")),
  });
  const valid = Number.isInteger(Number(hours)) && Number(hours) >= 0 && Number.isInteger(Number(keep)) && Number(keep) >= 1;
  const remoteOn = kind !== "none";
  const keptCreds = kind === st.remote.kind && st.remote.credentials_configured;

  const run = async (fn: () => Promise<void>, at: "hub" | "docs" = "hub") => {
    setWhere(at);
    setBusy(true);
    setError(null);
    setDone(null);
    try {
      await fn();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const save = () =>
    run(async () => {
      apply(await api.hubCall<BackupConfigView>("hub.backup.set", payload()));
      setDone("Configuração do backup salva.");
    });
  const test = () =>
    run(async () => {
      const p = payload();
      const r = await api.hubCall<{ ok: boolean; error?: string }>("hub.backup.test", {
        remote: p.remote,
        credentials: p.credentials,
      });
      if (r.ok) setDone("Conexão ok: o hub gravou e apagou um arquivo de prova no destino.");
      else setError(`Teste falhou: ${r.error ?? "erro desconhecido"}`);
    });
  // Com o backup dos docs (#442), cada botão roda só a sua parte.
  const docs = view.docs ?? null;
  const backupNow = () =>
    run(async () => {
      apply(await api.hubCall<BackupConfigView>("hub.backup.run", docs ? { what: "hub" } : {}));
      setDone("Backup feito.");
    });
  const int = (v: string) => (/^\d+$/.test(v.trim()) ? Number(v) : NaN);
  const docsValid = int(docsHours) >= 0 && int(docsHours) <= 24 * 90 && int(docsKeep) >= 1 && int(docsMax) >= 1 && int(docsMax) <= 4096;
  const saveDocs = () =>
    run(async () => {
      apply(
        await api.hubCall<BackupConfigView>("hub.backup.set", {
          docs_interval_hours: int(docsHours),
          docs_keep: int(docsKeep),
          docs_max_mb: int(docsMax),
        }),
      );
      setDone("Configuração do backup dos docs salva.");
    }, "docs");
  const docsNow = () =>
    run(async () => {
      const v = await api.hubCall<BackupConfigView>("hub.backup.run", { what: "docs" });
      apply(v);
      // A falha do pacote não vira erro da chamada: vem no estado.
      if (v.docs?.last_error) throw new Error(`Backup dos docs falhou: ${v.docs.last_error}`);
      setDone(v.docs?.last_skipped ? `Backup dos docs não rodou: ${v.docs.last_skipped}` : "Backup dos docs feito.");
    }, "docs");

  const message = (
    <>
      {error && (
        <p role="alert" className="start-error">
          {error}
        </p>
      )}
      {done && <p role="status">{done}</p>}
    </>
  );

  const textField = (id: string, label: string, value: string, onChange: (v: string) => void, placeholder = "") => (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <input id={id} type="text" value={value} placeholder={placeholder} onChange={(e) => onChange(e.target.value)} />
    </div>
  );

  return (
    <section aria-labelledby="hub-backup" className="panel settings-section">
      <div className="settings-head">
        <h2 id="hub-backup">Backup do hub</h2>
        <div className="env-actions">
          <button type="button" className="btn" disabled={busy} onClick={backupNow}>
            Fazer backup agora
          </button>
        </div>
      </div>

      <div className="env-row">
        <div className="fact">
          <span className="label">Último snapshot local</span>
          <span>{st.last_ok_at ? ago(st.last_ok_at, now) : st.newest_at ? ago(st.newest_at, now) : "nenhum"}</span>
        </div>
        <div className="fact">
          <span className="label">Pasta local</span>
          <span className="mono">{st.dir}</span>
        </div>
        <div className="fact">
          <span className="label">Destino remoto</span>
          <span className="mono">{st.remote.kind === "none" ? "—" : st.remote.target}</span>
        </div>
        {st.remote.kind !== "none" && (
          <div className="fact">
            <span className="label">Último envio</span>
            <span className={st.remote.last_error ? "state-off" : "state-on"}>
              {st.remote.last_error
                ? `falhou (${st.remote.failures}×): ${st.remote.last_error}`
                : st.remote.last_ok_at
                  ? `${ago(st.remote.last_ok_at, now)} · ${st.remote.last_name ?? ""} · ${bytes(st.remote.last_size ?? 0)}`
                  : "ainda não enviado"}
            </span>
          </div>
        )}
      </div>

      <form
        className="env-form"
        aria-label="Configurar backup do hub"
        onSubmit={(ev) => {
          ev.preventDefault();
          void save();
        }}
      >
        <div className="field">
          <label htmlFor="bk-kind">Destino</label>
          <select id="bk-kind" value={kind} onChange={(e) => setKind(e.target.value as RemoteKind)}>
            {KINDS.map((k) => (
              <option key={k.value} value={k.value}>
                {k.label}
              </option>
            ))}
          </select>
        </div>
        {textField("bk-dir", "Pasta dos snapshots locais", dir, setDir)}
        {textField("bk-hours", "Intervalo (horas, 0 desliga)", hours, setHours)}
        {textField("bk-keep", "Snapshots mantidos", keep, setKeep)}

        {(kind === "s3" || kind === "b2") && (
          <>
            {textField("bk-bucket", "Bucket", remote.bucket, (v) => set({ bucket: v }))}
            {textField("bk-prefix", "Prefixo (pasta no bucket)", remote.prefix, (v) => set({ prefix: v }), "cthub/")}
            {textField("bk-region", "Região", remote.region, (v) => set({ region: v }), "us-east-1")}
            {textField(
              "bk-endpoint",
              kind === "b2" ? "Endpoint do B2" : "Endpoint (vazio = AWS; MinIO, Wasabi, R2)",
              remote.endpoint,
              (v) => set({ endpoint: v }),
              kind === "b2" ? "https://s3.us-west-004.backblazeb2.com" : "https://",
            )}
          </>
        )}
        {kind === "drive" &&
          textField("bk-folder", "ID da pasta do Drive (dedicada)", remote.drive_folder_id, (v) => set({ drive_folder_id: v }))}

        {CRED_FIELDS[kind].map((f) => (
          <div className="field" key={f}>
            <label htmlFor={`bk-${f}`}>{CRED_LABEL[f]}</label>
            <input
              id={`bk-${f}`}
              type="password"
              autoComplete="off"
              placeholder={keptCreds ? "configurado · vazio mantém o atual" : ""}
              value={creds[f] ?? ""}
              onChange={(e) => setCreds({ ...creds, [f]: e.target.value })}
            />
          </div>
        ))}

        {remoteOn && (
          <div className="field">
            <label htmlFor="bk-encrypt">
              <input
                id="bk-encrypt"
                type="checkbox"
                checked={remote.encrypt}
                onChange={(e) => set({ encrypt: e.target.checked })}
              />{" "}
              Cifrar com age antes de enviar
            </label>
            {!remote.encrypt && (
              <p role="status" className="state-off">
                Sem cifra, o hub.db (histórico dos projetos) sobe em texto claro para o destino.
              </p>
            )}
          </div>
        )}

        {where === "hub" && message}
        <div className="form-actions">
          {remoteOn && (
            <button type="button" className="btn" disabled={busy} onClick={test}>
              Testar conexão
            </button>
          )}
          <button type="submit" className="btn btn-primary" disabled={busy || !valid}>
            Salvar backup
          </button>
        </div>
      </form>
      {docs && (
        <section aria-labelledby="hub-docs-backup" className="settings-subsection">
          <div className="settings-head">
            <h3 id="hub-docs-backup">Docs dos projetos</h3>
            <div className="env-actions">
              <button
                type="button"
                className="btn"
                aria-label="Fazer backup dos docs agora"
                disabled={busy || !docs.docs_dir}
                title={docs.docs_dir ? undefined : "docs_dir não configurado no hub"}
                onClick={docsNow}
              >
                Fazer backup agora
              </button>
            </div>
          </div>
          <DocsBackupFacts docs={docs} now={now} />
          <DocsBackupWarnings warnings={hub.health?.warnings ?? []} />
          {where === "docs" && message}
          {docs.docs_dir && (
            <form
              className="env-form"
              aria-label="Configurar backup dos docs"
              onSubmit={(ev) => {
                ev.preventDefault();
                void saveDocs();
              }}
            >
              {textField("bk-docs-hours", "Intervalo dos docs (horas, 0 desliga)", docsHours, setDocsHours)}
              {textField("bk-docs-keep", "Pacotes dos docs mantidos", docsKeep, setDocsKeep)}
              {textField("bk-docs-max", "Limite dos docs (MB, até 4096)", docsMax, setDocsMax)}
              <div className="form-actions">
                <button type="submit" className="btn btn-primary" disabled={busy || !docsValid}>
                  Salvar backup dos docs
                </button>
              </div>
            </form>
          )}
          <p className="muted hint">
            Um pacote completo do <span className="mono">docs_dir</span> por rodada, no mesmo{" "}
            <span className="mono">backup_dir</span> e no mesmo destino remoto do hub.db (cifrado com age se a cifra
            estiver ligada). Para recuperar um arquivo:{" "}
            <span className="mono">cthubd restore --remote latest --to ~/docs-restaurados</span>.
          </p>
        </section>
      )}

      <p className="muted hint">
        As credenciais ficam no store de secrets do hub (<span className="mono">secrets.age</span>, cifrado) e nunca
        voltam para esta tela. O snapshot local continua sendo gerado; o envio remoto é uma etapa a mais e, se falhar,
        vira aviso sem afetar o hub. No Google Drive, use uma pasta só para os backups e o escopo{" "}
        <span className="mono">drive.file</span>; no B2 e no S3, uma chave restrita ao bucket. Para restaurar:{" "}
        <span className="mono">cthubd restore --remote latest</span>.
      </p>
    </section>
  );
}
