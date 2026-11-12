// Sessão de release (#204): a configuração do projeto (`HubProject.release`)
// e os textos da prévia do "Criar release". Funções puras, testadas em
// release.test.ts.
import type { HubProject, HubReleaseStatus, ReleaseConfig, ReleaseStatus, ReleaseStep } from "../protocol/hub-gen";

/** Protocolo do hub com `release` (#204). */
export const RELEASE_PROTOCOL = "1.21";

/** Configuração padrão (desligada); espelha `ReleaseConfig::default` do ct-core. */
export const DEFAULT_RELEASE_CONFIG: ReleaseConfig = {
  enabled: false,
  flow: "release-please",
  source: "develop",
  target: "main",
  release_pr_branch: "release-please--branches--{target}",
  back_merge: true,
  approve_pending_runs: true,
  token_secret: "CT_RELEASE_GH_TOKEN",
  timeouts: { release_pr: 30, runs: 30, publish: 30, back_merge: 30 },
};

/** O projeto aceita release: GitHub + GitHub. */
export const releaseSupported = (p: Pick<HubProject, "tracker" | "forge">): boolean =>
  (p.tracker?.kind ?? "github") === "github" && (p.forge?.kind ?? "github") === "github";

/** Configuração do projeto; `null` = projeto Jira + GitLab ou hub anterior à #204. */
export function releaseOf(p: HubProject): ReleaseConfig | null {
  if (!releaseSupported(p)) return null;
  return (p as Partial<HubProject>).release ?? null;
}

/** Head do PR de release, com `{target}` trocado. */
export const releasePrHead = (c: ReleaseConfig): string => c.release_pr_branch.replace("{target}", c.target);

/** Motivo de o botão "Criar release" ficar desabilitado; `null` = pode iniciar. */
export function startBlocker(s: HubReleaseStatus | null, liveSession: boolean): string | null {
  if (!s) return "carregando o estado da release…";
  if (!s.enabled) return "a release está desligada nas configurações do projeto";
  if (!s.secret_present) return `cadastre o secret ${s.token_secret} no projeto (Secrets e variáveis) com o token exclusivo da release`;
  if (liveSession) return "já há uma sessão de release viva neste projeto";
  return null;
}

const STEP_TEXT: Record<ReleaseStep, string> = {
  status: "conferir o estado",
  promote: "abrir o PR de promoção",
  "merge-promotion": "mesclar o PR de promoção",
  "wait-release-pr": "esperar o PR de release",
  "approve-runs": "aprovar os runs do PR de release",
  "merge-release": "mesclar o PR de release",
  "wait-publish": "esperar a publicação",
  "merge-back": "mesclar o back-merge",
  nothing: "nada a lançar",
};

/** Próxima etapa em texto. */
export const stepText = (s: ReleaseStep): string => STEP_TEXT[s];

/** Resumo de uma linha do estado, para a prévia. */
export function statusSummary(s: ReleaseStatus): string {
  if (s.back_merge_pr) return `back-merge pendente (#${s.back_merge_pr.number}): a sessão começa por ele`;
  if (s.promotion_pr || s.release_pr) return `release em andamento: próxima etapa, ${stepText(s.next)}`;
  if (!s.content_diff) return `nada a lançar: ${s.source} e ${s.target} têm o mesmo conteúdo`;
  return `${s.source_ahead_by} commit(s) de ${s.source} a lançar em ${s.target}`;
}

/** `release` do `hub.projects.update` a partir do formulário; `null` = nada mudou. */
export function releasePatch(cur: ReleaseConfig, next: ReleaseConfig): ReleaseConfig | null {
  const clean: ReleaseConfig = {
    ...next,
    source: next.source.trim(),
    target: next.target.trim(),
    release_pr_branch: next.release_pr_branch.trim(),
    token_secret: next.token_secret.trim().toUpperCase(),
  };
  return JSON.stringify(clean) === JSON.stringify(cur) ? null : clean;
}

/** Erro do formulário, antes de enviar (o hub confere de novo). */
export function releaseFormError(c: ReleaseConfig): string | null {
  const branch = /^[^\s~^:?*[\\]+$/;
  if (!c.source.trim() || !branch.test(c.source.trim())) return "branch de origem inválida";
  if (!c.target.trim() || !branch.test(c.target.trim())) return "branch de destino inválida";
  if (c.source.trim() === c.target.trim()) return "origem e destino precisam ser branches diferentes";
  if (!/^[A-Z_][A-Z0-9_]*$/i.test(c.token_secret.trim())) return "nome do secret: letras, dígitos e _";
  if (/^(CTOWER_|CTHUB_)/i.test(c.token_secret.trim())) return "o secret não pode começar com CTOWER_ nem CTHUB_";
  for (const v of Object.values(c.timeouts)) {
    if (!Number.isInteger(v) || v < 1 || v > 1440) return "prazos entre 1 e 1440 minutos";
  }
  return null;
}
