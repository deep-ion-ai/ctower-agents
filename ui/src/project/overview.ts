// Regras das ações da Visão geral do projeto (#19): quais sinais de saúde
// são do projeto, em que ambiente o "Buscar issues agora" roda e como o
// resultado vira uma linha. Funções puras, testadas em overview.test.ts;
// os cartões ficam em OverviewCards.tsx.
import type { ProjectSync } from "../protocol/gen";
import type { HealthSignal, HubHealth } from "../protocol/hub-gen";
import { plural } from "../shell/summary";
import type { Clone } from "./clones";

const same = (a: string | null | undefined, b: string) => !!a && a.toLowerCase() === b.toLowerCase();

const RANK = { critico: 0, aviso: 1, info: 2 } as const;

/**
 * Sinais de aviso e críticos do projeto `owner/repo` (críticos primeiro). O
 * hub calcula os sinais do portfólio inteiro (`hub.health.get` não filtra
 * por projeto) e marca cada um com `owner`/`repo`: o corte é aqui, sem
 * diferenciar caixa (o endereço pode vir com outra caixa que a do hub). Os
 * informativos ficam fora, como na Visão geral do Hub: só o que pede ação.
 * Sinal sem projeto (`ambiente_mudo`) nunca entra.
 */
export function projectSignals(h: HubHealth | null, owner: string, repo: string): HealthSignal[] {
  return (h?.signals ?? [])
    .filter((s) => s.severity !== "info" && same(s.owner, owner) && same(s.repo, repo))
    .sort((a, b) => RANK[a.severity] - RANK[b.severity]);
}

/** Onde roda o `sync.now` do projeto: o ambiente e o id do projeto no ambiente. */
export interface SyncTarget {
  env: string;
  name: string;
  projectId: number;
}

/** Ambientes do "Buscar issues agora"; sem nenhum, `reason` diz por quê. */
export interface SyncPlan {
  targets: SyncTarget[];
  reason: string | null;
}

/**
 * Em que ambientes o "Buscar issues agora" do projeto roda (#19).
 *
 * `sync.now` do ambiente aceita `project_id` (o id local do projeto, não o
 * do hub) e sincroniza só ele, pelo jira-cli se o projeto é Jira, senão pelo
 * GitHub; o hub não tem pedido sob demanda (o sync do GitHub dele é só
 * periódico) e o canal reverso da web recusa `sync.now`. Então o botão
 * chama `sync.now` com o `project_id` de cada clone:
 *
 * - projeto Jira (`jira` informado): só no ambiente de sync, o único que
 *   alcança o Jira e alimenta o espelho do hub; sem ele escolhido ou
 *   conectado, nada roda e `reason` diz o que falta;
 * - GitHub (e a visão local, hub fora): em cada ambiente conectado com o
 *   projeto (`clones`), quase sempre um só.
 */
export function syncPlan(clones: Clone[], jira: { env: string | null; name: string | null } | null): SyncPlan {
  const target = (c: Clone): SyncTarget => ({ env: c.env, name: c.name, projectId: c.project.id });
  if (jira) {
    if (!jira.name) {
      return { targets: [], reason: "Projeto sem ambiente de sync: escolha um em Configurações › Repositório e rastreador." };
    }
    const c = jira.env ? clones.find((x) => x.env === jira.env) : undefined;
    return c
      ? { targets: [target(c)], reason: null }
      : { targets: [], reason: `O ambiente de sync ${jira.name} não está conectado com este projeto.` };
  }
  if (clones.length === 0) return { targets: [], reason: "Nenhum ambiente conectado tem este projeto." };
  return { targets: clones.map(target), reason: null };
}

/** Resposta de `sync.now` de um ambiente, ou o erro da chamada. */
export type SyncOutcome = { name: string; result: ProjectSync[] } | { name: string; error: string };

/**
 * Feedback do "Buscar issues agora" em uma linha: o total buscado nos
 * ambientes que deram certo e o erro de cada um que falhou (na chamada ou
 * no `ProjectSync.error`, quando o rastreador recusou). `change` é "PR"
 * ou "MR", pela forja do projeto.
 */
export function syncFeedback(outcomes: SyncOutcome[], change: string): { text: string; error: boolean } {
  const okNames: string[] = [];
  const errors: string[] = [];
  let issues = 0;
  let prs = 0;
  let moves = 0;
  for (const o of outcomes) {
    if ("error" in o) {
      errors.push(`${o.name}: ${o.error}`);
      continue;
    }
    const failed = o.result.filter((r) => !r.ok);
    if (failed.length > 0) {
      errors.push(`${o.name}: ${failed.map((r) => r.error ?? "falhou").join("; ")}`);
      continue;
    }
    okNames.push(o.name);
    for (const r of o.result) {
      issues += r.issues;
      prs += r.prs;
      moves += r.status_changes;
    }
  }
  const parts: string[] = [];
  if (okNames.length > 0) {
    const what = [
      plural(issues, "issue", "issues"),
      plural(prs, change, `${change}s`),
      ...(moves > 0 ? [plural(moves, "mudança de status", "mudanças de status")] : []),
    ].join(", ");
    parts.push(`Sincronizado por ${okNames.join(", ")}: ${what}.`);
  }
  if (errors.length > 0) parts.push(`Falhou em ${errors.join(" · ")}`);
  return { text: parts.join(" "), error: errors.length > 0 };
}
