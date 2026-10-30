// Manutenção › Visão geral (#339): o estado de manutenção de cada ambiente,
// deste app e do backup do hub, com cada célula levando à seção que resolve.
import type { ReactNode } from "react";
import type { UiUpdate } from "../api/types";
import { useNow } from "../App";
import type { UpdateStatus } from "../protocol/gen";
import { settingsTab } from "../settings/sections";
import { plural } from "../shell/summary";
import { useNav, type OpenTab } from "../state/nav";
import { useTower } from "../state/tower";
import { humanBytes } from "../util/bytes";
import { ago } from "../util/time";
import { useMaintenanceOverview, type MaintenanceEnvRow } from "./overviewRows";
import { isBackupWarning, maintenanceTab } from "./sections";

const short = (commit: string) => commit.slice(0, 7);
const build = (version: string, commit: string) => (commit ? `${version} · ${short(commit)}` : version);

/** O que está disponível, sem a atualização (essa vai em destaque). */
function availableText(u: UpdateStatus): string {
  if (!u.available) return u.check_error ? "verificação falhou" : "ainda não verificado";
  switch (u.available.relation) {
    case "same":
      return "em dia";
    case "ahead":
      return `à frente de ${u.ref}`;
    default:
      return build(u.available.build.version, u.available.build.commit);
  }
}

/**
 * Tabela com uma linha por ambiente (os desta UI e os que só o hub conhece)
 * e uma para este app, mais a linha do backup do hub. Só o que pede ação
 * (atualização disponível, limpeza pendente, aviso de backup) vai em
 * destaque, como na Visão geral do Hub (#313).
 */
export function MaintenanceOverview() {
  const { rows, pending } = useMaintenanceOverview();
  const { uiUpdate } = useTower();
  const now = useNow();

  return (
    <>
      <p className="muted" role="status">
        {pending > 0
          ? `${plural(pending, "pendência", "pendências")}: atualizações disponíveis e ambientes com limpeza pendente.`
          : "Nada pendente: tudo em dia."}
      </p>
      <section className="panel settings-section" aria-labelledby="maint-ov-envs">
        <h2 id="maint-ov-envs">Ambientes · {rows.length}</h2>
        <div className="table-wrap maint-ov">
          <table aria-label="Manutenção por ambiente">
            <thead>
              <tr>
                <th scope="col">Ambiente</th>
                <th scope="col">Versão</th>
                <th scope="col">Limpeza</th>
                <th scope="col">Conexão</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <EnvRow key={r.key} r={r} now={now} />
              ))}
              <AppRow uiUpdate={uiUpdate} />
              <HubRow />
            </tbody>
          </table>
        </div>
      </section>
      <BackupLine now={now} />
    </>
  );
}

/** Célula que leva à seção (e ao ambiente) que resolve; sem destino, só o texto. */
function Cell({ label, to, children }: { label: string; to?: OpenTab; children: ReactNode }) {
  const nav = useNav();
  return (
    <td data-label={label}>
      {to ? (
        <button type="button" className="maint-ov-link" onClick={() => nav.openTab(to)}>
          {children}
        </button>
      ) : (
        children
      )}
    </td>
  );
}

const Attention = ({ children }: { children: ReactNode }) => (
  <span className="badge" data-tone="attention">
    {children}
  </span>
);

const Sub = ({ children }: { children: ReactNode }) => <span className="maint-ov-sub">{children}</span>;

function EnvRow({ r, now }: { r: MaintenanceEnvRow; now: number }) {
  const u = r.info?.update;
  const c = r.hub?.cleanup;
  return (
    <tr aria-label={r.name} data-attention={r.updateAvailable || r.cleanupPending || undefined}>
      <th scope="row" className="strong">
        {r.name}
      </th>
      <Cell label="Versão" to={r.uiEnv !== undefined ? maintenanceTab("env-updates") : undefined}>
        <span className="mono">{r.info ? build(r.info.version, r.info.commit) : "—"}</span>
        {r.uiEnv === undefined ? (
          <Sub>fora desta UI</Sub>
        ) : r.updateAvailable && u?.available ? (
          <Sub>
            <Attention>atualização disponível</Attention>{" "}
            <span className="mono">{build(u.available.build.version, u.available.build.commit)}</span>
          </Sub>
        ) : !r.connected ? (
          <Sub>sem conexão</Sub>
        ) : (
          <Sub>{u ? availableText(u) : r.info ? "sem suporte a atualização" : "lendo…"}</Sub>
        )}
      </Cell>
      <Cell label="Limpeza" to={r.uiEnv !== undefined ? maintenanceTab("cleanup", r.uiEnv) : undefined}>
        {!r.hub ? (
          <span className="muted">sem dado do hub</span>
        ) : !c ? (
          <span className="muted">não informado</span>
        ) : c.items > 0 ? (
          <Attention>
            {c.items} pendente(s) · {humanBytes(c.bytes)}
          </Attention>
        ) : (
          <span className="muted">nenhuma pendente</span>
        )}
        {c && <Sub>verificado {ago(c.checked_at, now)}</Sub>}
      </Cell>
      <Cell label="Conexão" to={r.uiEnv !== undefined ? settingsTab({ section: "connections" }) : undefined}>
        {r.uiEnv === undefined ? (
          <span className="muted">não configurado nesta UI</span>
        ) : (
          <span className={r.connected ? "state-on" : "state-off"}>{r.connected ? "conectado" : "desconectado"}</span>
        )}
        {r.hub && <Sub>último envio ao hub {r.hub.last_seen_at ? ago(r.hub.last_seen_at, now) : "nunca"}</Sub>}
      </Cell>
    </tr>
  );
}

function AppRow({ uiUpdate }: { uiUpdate: UiUpdate | null }) {
  const next = uiUpdate?.available ?? null;
  return (
    <tr aria-label="Este app" data-attention={next ? true : undefined}>
      <th scope="row" className="strong">
        Este app
      </th>
      <Cell label="Versão" to={maintenanceTab("app-updates")}>
        <span className="mono">{uiUpdate?.current ?? "—"}</span>
        {next ? (
          <Sub>
            <Attention>atualização disponível</Attention> <span className="mono">{next.version}</span>
          </Sub>
        ) : (
          <Sub>{!uiUpdate ? "verificando…" : uiUpdate.error ? "verificação falhou" : "em dia"}</Sub>
        )}
      </Cell>
      <Cell label="Limpeza">
        <span className="muted">—</span>
      </Cell>
      <Cell label="Conexão">
        <span className="muted">—</span>
      </Cell>
    </tr>
  );
}

/** O hub (#421): versão no ar e a atualização disponível, se houver. */
function HubRow() {
  const { hub } = useTower();
  if (!hub.conn) return null;
  const u = hub.update;
  const next = u?.available?.update_available ? u.available : null;
  const to = maintenanceTab("hub-updates");
  return (
    <tr aria-label="Hub" data-attention={next ? true : undefined}>
      <th scope="row" className="strong">
        Hub
      </th>
      <Cell label="Versão" to={to}>
        <span className="mono">{hub.info?.version ?? "—"}</span>
        {next ? (
          <Sub>
            <Attention>atualização disponível</Attention> <span className="mono">{next.build.version}</span>
          </Sub>
        ) : (
          <Sub>
            {!hub.connected
              ? "sem conexão"
              : !u
                ? "sem atualização pela UI"
                : u.check_error && !u.available
                  ? "verificação falhou"
                  : u.available
                    ? "em dia"
                    : "ainda não verificado"}
          </Sub>
        )}
      </Cell>
      <Cell label="Limpeza">
        <span className="muted">—</span>
      </Cell>
      <Cell label="Conexão">
        <span className={hub.connected ? "state-on" : "state-off"}>{hub.connected ? "conectado" : "desconectado"}</span>
      </Cell>
    </tr>
  );
}

/**
 * Backup do hub: último snapshot e o estado, de `hub.status` (já lido pela
 * faixa de avisos e pelo selo do Hub); os avisos de backup em destaque.
 */
function BackupLine({ now }: { now: number }) {
  const { hub } = useTower();
  const nav = useNav();
  const b = hub.health?.backup;
  const warnings = (hub.health?.warnings ?? []).filter((w) => isBackupWarning(w.kind));

  let state: ReactNode;
  if (!hub.conn) state = <span className="muted">hub não configurado</span>;
  else if (!hub.connected) state = <span className="muted">hub indisponível</span>;
  else if (!b) state = <span className="muted">lendo…</span>;
  else if (warnings.length > 0)
    state = (
      <ul className="maint-ov-warnings">
        {warnings.map((w, i) => (
          <li key={`${w.kind}:${i}`} data-kind={w.kind}>
            <Attention>aviso</Attention> {w.message}
          </li>
        ))}
      </ul>
    );
  else if (b.interval_hours === 0) state = <span className="muted">snapshots desligados</span>;
  else if (b.last_error) state = <span className="state-off">último snapshot falhou: {b.last_error}</span>;
  else state = <span className="muted">em dia</span>;

  return (
    <section className="panel settings-section" aria-labelledby="maint-ov-backup">
      <h2 id="maint-ov-backup">Backup do hub</h2>
      <div className="maint-ov-backup">
        <div className="maint-ov-backup-text">
          {state}
          {b && (
            <Sub>
              último snapshot {b.newest_at ? ago(b.newest_at, now) : "nenhum ainda"}
              {/* Hub anterior ao #263 não manda `remote`. */}
              {b.remote && b.remote.kind !== "none" &&
                ` · remoto ${b.remote.last_ok_at ? ago(b.remote.last_ok_at, now) : "nunca enviado"}`}
            </Sub>
          )}
        </div>
        <button type="button" className="btn btn-sm" onClick={() => nav.openTab(maintenanceTab("backup"))}>
          Backup e snapshots ›
        </button>
      </div>
    </section>
  );
}
