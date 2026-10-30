// Faixa "Precisa de você" (spec §10): só o que exige o humano. Função pura,
// testada à parte; vazia = tudo andando.
import type { EnvStatus, EnvView } from "../api/types";
import type { Dashboard, DashboardWorkItem, EnvInfo, GestorScope, PermissionRequest, Project, Session } from "../protocol/gen";
import { scopeText } from "../project/templates";
import type { HubUpdateStatus, Warning } from "../protocol/hub-gen";
import { isBackupWarning, type MaintenanceSection } from "../maintenance/sections";
import { askKey, type Alert, type Ask } from "../state/tower";
import { duration } from "../util/time";
import { isJiraProject, itemShort, PROJECT_ITEM, trackerLabel } from "../util/item";
import { permissionText } from "../terminal/permission";
import { rcOn, rcUrl } from "../terminal/remote";
import { DOCS_HELP_URL } from "../project/docs";

/** Intervalo padrão do sync com o GitHub (`gh_sync_interval_secs` = 180). */
export const SYNC_INTERVAL_MS = 180_000;
/** Intervalo padrão do sync com o Jira (`jira_sync_interval_secs` = 300, spec §6.5). */
export const JIRA_SYNC_INTERVAL_MS = 300_000;
/** Sync atrasado: último sucesso há mais de 3 intervalos. */
export const SYNC_LATE_FACTOR = 3;
export const FAILED_WINDOW_MS = 24 * 3_600_000;

export type AttentionKind = "waiting" | "failed" | "interrupted" | "env_down" | "docs" | "hub" | "alert" | "sync_late";

export type AttentionAction =
  | { to: "terminal"; env: string; session: string }
  | { to: "recovery" }
  | { to: "settings" }
  | { to: "hub" }
  | { to: "maintenance"; section: MaintenanceSection }
  | { to: "project"; env: string; projectId: number }
  /** Documentação no navegador (ex.: a seção "Docs dos projetos" de docs/hub.md). */
  | { to: "url"; url: string };

export interface AttentionItem {
  key: string;
  kind: AttentionKind;
  env: string;
  /** Rótulo do tipo, escrito (não só cor). */
  label: string;
  title: string;
  detail: string;
  /** Desde quando (ms). */
  since: number;
  actionLabel: string;
  action: AttentionAction;
  /** Alerta de push pode ser dispensado. */
  alert?: Alert;
  /**
   * Web: link do Remote Control da sessão que espera você, quando ligado
   * (`claude.ai/code/<rc_session_id>`); o toque abre direto (spec do épico #205 §10.3).
   */
  remote?: string;
}

export interface AttentionInput {
  envs: EnvView[];
  status: Record<string, EnvStatus>;
  dashboards: Record<string, Dashboard>;
  alerts: Alert[];
  now: number;
  /**
   * Conexão com o hub (só se configurada), os avisos de `hub.status` e a
   * atualização do hub (`hub.update.get`, #421).
   */
  hub?: { status: EnvStatus | null; warnings: Warning[]; update?: HubUpdateStatus | null };
  /** `env.info` de cada ambiente (aviso `docs_unreachable`, #441/#444). */
  infos?: Record<string, EnvInfo>;
  /** Texto do pedido de cada sessão (push `needs_human`), se recebido. */
  asks?: Record<string, Ask>;
  /** Pedidos de permissão pendentes por ambiente (`session.permission.watch`). */
  permissions?: Record<string, PermissionRequest[]>;
  /**
   * Modo web (#201): o estado do ambiente é o do último envio ao hub, e
   * Configurações e Manutenção não existem; os avisos levam ao Hub e o
   * pedido pendente, ao painel da sessão (Remote Control).
   */
  web?: boolean;
}

/** Texto padrão do pedido pendente, sem a mensagem do agente. */
export const ASK_FALLBACK = "Permissão ou pergunta pendente no terminal";

const ORDER: AttentionKind[] = ["waiting", "failed", "interrupted", "env_down", "docs", "hub", "alert", "sync_late"];

/** Título de cada aviso de `hub.status` (`kind` estável do `cthubd`). */
const HUB_WARNING_TITLE: Record<string, string> = {
  backup_disabled: "Snapshots do hub desligados",
  backup_same_device: "Backup no mesmo disco do hub",
  backup_failed: "Snapshot do hub falhou",
  backup_stale: "Snapshot do hub atrasado",
  backup_remote_failed: "Envio do backup para o destino remoto falhando",
  backup_remote_credentials: "Credencial do backup remoto inválida ou expirada",
  backup_remote_stale: "Backup remoto atrasado",
  backup_remote_unencrypted: "Backup remoto sem cifra",
  docs_backup_failed: "Backup dos docs falhou",
  docs_backup_stale: "Backup dos docs atrasado",
  docs_backup_too_large: "Docs acima do limite do backup",
  push_quarantine: "Linhas em quarentena no hub",
  recovery_not_configured: "Chave de recuperação não configurada",
  recovery_reverify_due: "Verificar a chave de recuperação",
  secrets_snapshot_stale: "Snapshot dos secrets desatualizado",
  secrets_snapshot_unreachable: "Destino dos snapshots de secrets inacessível",
  secrets_snapshot_same_device: "Snapshots de secrets no mesmo disco do hub",
  secrets_rotate_pending: "Secrets a rotacionar na origem",
};

const ALERT_TEXT: Record<string, string> = {
  transcript_volatile: "Transcript em local volátil: pode se perder se o ambiente cair",
  trust_failed: "Não foi possível pré-aceitar a confiança do Claude Code no worktree",
  secrets_stale: "Sessão iniciada com secrets de uma configuração desatualizada",
  secrets_unavailable: "Sessão iniciada sem secrets: cache do ambiente ilegível",
  docs_unreachable: "Sessão iniciada sem a pasta de docs: docs_root inacessível",
};

/** Rótulo das sessões do gestor de projetos (sem projeto nem issue). */
export const GESTOR_LABEL = "Gestor de projetos";

/** "Gestor de projetos · <escopo>": distingue as sessões do gestor vivas ao mesmo tempo. */
export function gestorLabel(scope?: GestorScope): string {
  return scope ? `${GESTOR_LABEL} · ${scopeText(scope)}` : GESTOR_LABEL;
}

/**
 * Work item e projeto de uma sessão, procurados no dashboard. `gestor` =
 * sessão do gestor de projetos (o sentinela fica fora de `projects`), com
 * o escopo dela em `scope`, se o ambiente o informou.
 */
export function lookup(
  d: Dashboard | undefined,
  s: Session,
): { wi?: DashboardWorkItem; project?: Project; gestor?: boolean; scope?: GestorScope } {
  if (d && d.gestor_work_item_id !== null && s.work_item_id === d.gestor_work_item_id) {
    // `?.`: ambiente anterior ao campo não manda `gestor_scopes`.
    return { gestor: true, scope: d.gestor_scopes?.[s.id] };
  }
  for (const p of d?.projects ?? []) {
    const wi = p.work_items.find((w) => w.id === s.work_item_id);
    if (wi) return { wi, project: p.project };
  }
  return {};
}

/** "control-tower #31 Título" ou "app PROJ-31 Título" (ou só a referência, sem título no cache). */
export function itemLabel(project?: Project, wi?: DashboardWorkItem, gestor = false, scope?: GestorScope): string {
  if (gestor) return gestorLabel(scope);
  if (!wi) return "sessão sem work item";
  if (wi.gh_number === PROJECT_ITEM) return `${project ? project.name + " · " : ""}sessão do projeto`;
  const t = wi.title ? ` ${wi.title}` : "";
  return `${project ? project.name + " " : ""}${itemShort(project, wi.gh_number)}${t}`;
}

export function attention(input: AttentionInput): AttentionItem[] {
  const { envs, status, dashboards, alerts, now, hub, asks, permissions, web, infos } = input;
  const out: AttentionItem[] = [];

  if (hub?.status && !hub.status.connected) {
    out.push({
      key: "hub:down",
      kind: "hub",
      env: "hub",
      label: "Hub",
      title: "Hub indisponível",
      detail: `${hub.status.error ?? "Reconectando…"}. As sessões seguem nos ambientes com a última configuração.`,
      since: hub.status.since,
      actionLabel: "Ver hub",
      action: { to: "hub" },
    });
  }
  for (const [i, w] of (hub?.warnings ?? []).entries()) {
    // Backup e snapshots do hub se resolvem em Manutenção (#338); o resto, na tela do hub.
    const backup = isBackupWarning(w.kind);
    out.push({
      key: `hub:${w.kind}:${i}`,
      kind: "hub",
      env: "hub",
      label: "Hub",
      title: HUB_WARNING_TITLE[w.kind] ?? "Aviso do hub",
      detail: w.message,
      since: now,
      actionLabel: backup && !web ? "Configurar backup" : "Ver hub",
      action: backup && !web ? { to: "maintenance", section: "backup" } : { to: "hub" },
    });
  }

  // Hub atrás do `update_ref` (#421): atualiza-se em Manutenção (na web, só se vê).
  const next = hub?.status?.connected && hub.update?.available?.update_available ? hub.update.available : null;
  if (next) {
    const behind = next.behind ?? next.commits.length;
    out.push({
      key: "hub:update",
      kind: "hub",
      env: "hub",
      label: "Hub",
      title: "Atualização do hub disponível",
      detail:
        `${next.build.version} em ${next.ref}` +
        (behind > 0 ? ` · ${behind} commit(s) novo(s)` : "") +
        ". Atualize o hub antes dos ambientes.",
      since: next.checked_at,
      actionLabel: web ? "Ver hub" : "Atualizar o hub",
      action: web ? { to: "hub" } : { to: "maintenance", section: "hub-updates" },
    });
  }

  for (const env of envs) {
    const st = status[env.id];
    if (st && !st.connected) {
      out.push({
        key: `env:${env.id}`,
        kind: "env_down",
        env: env.id,
        label: "Ambiente",
        title: web ? `${env.name} sem envio ao hub` : `${env.name} desconectado`,
        detail: st.error ?? "Reconectando…",
        since: st.since,
        actionLabel: web ? "Ver hub" : "Configurações",
        action: web ? { to: "hub" } : { to: "settings" },
      });
    }
    // `docs_root` inacessível (#441): as sessões novas sobem sem a pasta de
    // docs. Resolve-se na montagem do compartilhamento (docs/hub.md).
    const docs = st?.connected ? infos?.[env.id]?.docs : undefined;
    if (docs?.warning === "docs_unreachable") {
      out.push({
        key: `docs:${env.id}`,
        kind: "docs",
        env: env.id,
        label: "Docs",
        title: `${env.name}: pasta de docs inacessível`,
        detail: `docs_root ${docs.root ?? ""} inacessível (montagem caída?): as sessões novas sobem sem a pasta de docs.`,
        since: st!.since,
        actionLabel: "Ver documentação",
        action: { to: "url", url: DOCS_HELP_URL },
      });
    }
    const d = dashboards[env.id];
    if (!d) continue;

    for (const s of d.live_sessions) {
      const { wi, project, gestor, scope } = lookup(d, s);
      // Pedido de permissão segurado pelo daemon: responde-se na faixa do terminal.
      const req = permissions?.[env.id]?.find((p) => p.session_id === s.id);
      if (!req && wi?.status !== "aguardando_voce") continue;
      out.push({
        key: `wait:${env.id}:${s.id}`,
        kind: "waiting",
        env: env.id,
        label: req ? "Permissão" : "Aguardando você",
        title: itemLabel(project, wi, gestor, scope),
        detail: req ? permissionText(req) : asks?.[askKey(env.id, s.id)]?.message || ASK_FALLBACK,
        since: req?.created_at ?? s.last_event_at,
        // O pedido se responde no terminal (faixa logo abaixo do console);
        // na web, pelo Remote Control, a partir do painel da sessão.
        actionLabel: web ? (rcOn(s) ? "Abrir no Remote Control" : "Ver sessão") : "Responder",
        action: { to: "terminal", env: env.id, session: s.id },
        ...(web && rcOn(s) ? { remote: rcUrl(s.rc_session_id!) } : {}),
      });
    }

    for (const s of d.interrupted_sessions) {
      const { wi, project, gestor, scope } = lookup(d, s);
      out.push({
        key: `int:${env.id}:${s.id}`,
        kind: "interrupted",
        env: env.id,
        label: "Interrompida",
        title: itemLabel(project, wi, gestor, scope),
        detail:
          s.exit_code === null
            ? "A sessão parou no meio do trabalho"
            : `O agente saiu com código ${s.exit_code} no meio do trabalho`,
        since: s.last_event_at,
        actionLabel: "Recuperar",
        action: { to: "recovery" },
      });
    }

    for (const s of d.failed_sessions) {
      const ended = s.ended_at ?? s.last_event_at;
      if (now - ended > FAILED_WINDOW_MS) continue;
      const { wi, project, gestor, scope } = lookup(d, s);
      out.push({
        key: `fail:${env.id}:${s.id}`,
        kind: "failed",
        env: env.id,
        label: "Saiu com erro",
        title: itemLabel(project, wi, gestor, scope),
        detail: `Processo terminou com código ${s.exit_code}`,
        since: ended,
        actionLabel: "Ver projeto",
        action: { to: "project", env: env.id, projectId: project?.id ?? wi?.project_id ?? 0 },
      });
    }

    for (const sync of d.sync) {
      const ref = sync.last_ok_at ?? null;
      const project = d.projects.find((p) => p.project.id === sync.project_id)?.project;
      // Projeto fora da lista (desatribuído ou excluído): ninguém o sincroniza mais.
      if (!project) continue;
      const interval = isJiraProject(project) ? JIRA_SYNC_INTERVAL_MS : SYNC_INTERVAL_MS;
      const late = ref === null ? sync.last_error !== null : now - ref > SYNC_LATE_FACTOR * interval;
      if (!late) continue;
      out.push({
        key: `sync:${env.id}:${sync.project_id}`,
        kind: "sync_late",
        env: env.id,
        label: "Sync atrasado",
        title: `${trackerLabel(project)} de ${project.name} sem sincronizar`,
        detail:
          sync.last_error ??
          (ref === null ? "Ainda não sincronizou" : `Último sucesso há ${duration(now - ref)}`),
        since: ref ?? sync.last_attempt_at,
        actionLabel: "Ver projeto",
        action: { to: "project", env: env.id, projectId: sync.project_id },
      });
    }
  }

  for (const a of alerts) {
    const live = dashboards[a.env]?.live_sessions.find((s) => s.id === a.session_id);
    out.push({
      key: `alert:${a.env}:${a.kind}:${a.session_id}:${a.at}`,
      kind: "alert",
      env: a.env,
      label: "Alerta",
      title: ALERT_TEXT[a.kind] ?? a.kind,
      detail: a.detail,
      since: a.at,
      actionLabel: live ? "Ver terminal" : "Configurações",
      action: live ? { to: "terminal", env: a.env, session: live.id } : { to: "settings" },
      alert: a,
    });
  }

  return out.sort(
    (a, b) => ORDER.indexOf(a.kind) - ORDER.indexOf(b.kind) || a.since - b.since,
  );
}
