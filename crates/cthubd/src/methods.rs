//! Registro tipado dos métodos RPC do hub (`/ws` e `/web/ws`, #229).
//!
//! Mesmo padrão do registro do ambiente (`ct_http::methods`, #228): cada
//! método tem um [`MethodSpec`] com nome, tipo TypeScript dos parâmetros e do
//! resultado, e os escopos em que vale. O despacho de `ws.rs` consulta este
//! registro e recusa o que não está nele; a lista da web ([`WEB_METHODS`]) sai
//! daqui, não de uma lista à mão.
//!
//! Escopos do hub: [`Scope::Ui`] é o app desktop (token de UI, `/ws`, que
//! atende tudo); [`Scope::Web`] é a UI web (`/web/ws`, sessão web). O hub não
//! tem canal reverso próprio: o canal do ambiente é `ct_http::methods`.
//!
//! `ui/src/protocol/hub-gen/methods.ts` (`HubMethods`, `HUB_METHOD_SCOPES`) é
//! gerado deste registro pelo teste `export_ts` do `cthubd` e entra no CI de
//! drift. Os tipos são expressões escritas à mão, com os nomes exportados
//! pelo ts-rs (`HubInfo`, `HubWorkItem`...); `unknown` marca o que ainda não
//! tem tipo nomeado, e `{ [key: string]: unknown }` os parâmetros que o hub
//! ainda não tipa campo a campo.

pub use ct_http::methods::{count_in, names_in, MethodSpec, Scope};

const UI: &[Scope] = &[Scope::Ui];
const UI_WEB: &[Scope] = &[Scope::Ui, Scope::Web];

/// Os métodos do `/ws` do hub. Todo braço de despacho de `ws.rs`,
/// `autopilot/mod.rs` e `autopilot/runner.rs` tem de estar aqui (teste
/// `todo_metodo_do_hub_esta_no_registro`), e o que não está aqui não é
/// despachado. Escopo `Web` = o que a UI web pode chamar; o resto é do app
/// desktop (escritas de configuração, sensíveis, manutenção).
pub const METHODS: &[MethodSpec] = &[
    MethodSpec { name: "hub.info", params: "null", result: "HubInfo", scopes: UI_WEB },
    MethodSpec { name: "hub.status", params: "null", result: "HubStatus", scopes: UI_WEB },
    // Projetos. A escrita (`add`/`update`) é só do desktop.
    MethodSpec { name: "hub.projects.list", params: "null", result: "HubProjectView[]", scopes: UI_WEB },
    MethodSpec { name: "hub.projects.add", params: "{ [key: string]: unknown }", result: "ProjectAdded", scopes: UI },
    MethodSpec {
        name: "hub.projects.update",
        params: "{ owner: string; repo: string; [key: string]: unknown }",
        result: "HubProject",
        scopes: UI,
    },
    MethodSpec {
        name: "hub.projects.move",
        params: "{ owner: string; repo: string; before?: { owner: string; repo: string } | null; after?: { owner: string; repo: string } | null }",
        result: "HubProject",
        scopes: UI,
    },
    // Sem `confirm`, recusa e lista o impacto; a exclusão é só no desktop.
    MethodSpec {
        name: "hub.projects.delete",
        params: "{ owner: string; repo: string; confirm?: boolean | null }",
        result: "ProjectDeleted",
        scopes: UI,
    },
    MethodSpec { name: "hub.projects.impact", params: "{ owner: string; repo: string }", result: "DeleteImpact", scopes: UI },
    MethodSpec { name: "hub.tree.get", params: "{ owner: string; repo: string }", result: "HubTree", scopes: UI_WEB },
    MethodSpec { name: "hub.dashboard.get", params: "null", result: "HubDashboard", scopes: UI_WEB },
    MethodSpec { name: "hub.envs.list", params: "null", result: "HubEnv[]", scopes: UI_WEB },
    MethodSpec {
        name: "hub.envs.set_autonomous",
        params: "{ env: string; autonomous: boolean }",
        result: "HubEnv | null",
        scopes: UI,
    },
    MethodSpec { name: "hub.plan.accounts", params: "null", result: "HubPlanAccounts", scopes: UI_WEB },
    // Conexões externas (#235): o cadastro é só do desktop; `test` confere a prontidão sem rede.
    MethodSpec { name: "hub.connections.list", params: "null", result: "unknown[]", scopes: UI },
    MethodSpec { name: "hub.connections.add", params: "{ name: string; kind: \"github\" | \"gitlab\" | \"jira\"; endpoint?: string; credential: unknown; options?: { [key: string]: unknown } }", result: "unknown", scopes: UI },
    MethodSpec { name: "hub.connections.update", params: "{ id: number; name?: string | null; endpoint?: string | null; credential?: unknown; options?: { [key: string]: unknown } | null }", result: "unknown", scopes: UI },
    MethodSpec { name: "hub.connections.delete", params: "{ id: number }", result: "unknown", scopes: UI },
    MethodSpec { name: "hub.connections.test", params: "{ id: number }", result: "unknown", scopes: UI },
    // Atribuições e quarentena: escrita só no desktop.
    MethodSpec {
        name: "hub.assignments.list",
        params: "{ env?: string | null; owner?: string | null; repo?: string | null } | null",
        result: "HubAssignment[]",
        scopes: UI_WEB,
    },
    MethodSpec {
        name: "hub.assignments.add",
        params: "{ owner: string; repo: string; env: string; repo_path?: string | null }",
        result: "AssignmentAdded",
        scopes: UI,
    },
    MethodSpec {
        name: "hub.assignments.remove",
        params: "{ owner: string; repo: string; env: string; confirm?: boolean | null }",
        result: "AssignmentRemoved",
        scopes: UI,
    },
    MethodSpec { name: "hub.quarantine.list", params: "null", result: "QuarantineGroup[]", scopes: UI_WEB },
    MethodSpec {
        name: "hub.quarantine.discard",
        params: "{ owner: string; repo: string; confirm?: boolean | null }",
        result: "QuarantineDiscarded",
        scopes: UI,
    },
    // Work items. "Ver issue" (#434) é só leitura, de passagem.
    MethodSpec {
        name: "hub.workitem.history",
        params: "{ owner: string; repo: string; number: number; limit?: number | null; before_hlc?: number | null; include_sub_issues?: boolean | null }",
        result: "HubHistory",
        scopes: UI_WEB,
    },
    MethodSpec {
        name: "hub.workitem.body",
        params: "{ owner: string; repo: string; number: number }",
        result: "HubWorkItemBody",
        scopes: UI_WEB,
    },
    // Feedback de review dos PRs (#130): a tela da revisão entra na web depois.
    MethodSpec {
        name: "hub.workitem.pr_feedback",
        params: "{ owner: string; repo: string; number: number }",
        result: "{ prs: unknown }",
        scopes: UI,
    },
    MethodSpec {
        name: "hub.workitem.set_status",
        params: "{ owner: string; repo: string; number: number; status: string; reason?: string | null }",
        result: "HubWorkItem",
        scopes: UI_WEB,
    },
    MethodSpec {
        name: "hub.workitem.set_priority",
        params: "{ owner: string; repo: string; number: number; priority: \"P0\" | \"P1\" | \"P2\" | \"P3\" | null; reason?: string | null }",
        result: "HubWorkItem",
        scopes: UI_WEB,
    },
    // Modelo e esforço do agente na issue (#182): a mesma classe de escrita da prioridade.
    MethodSpec {
        name: "hub.workitem.set_model",
        params: "{ owner: string; repo: string; number: number; model?: string | null; effort?: string | null; reason?: string | null }",
        result: "HubWorkItem",
        scopes: UI_WEB,
    },
    MethodSpec {
        name: "hub.workitem.move",
        params: "{ owner: string; repo: string; number: number; before?: number | null; after?: number | null }",
        result: "HubWorkItem",
        scopes: UI,
    },
    MethodSpec {
        name: "hub.ranking.get",
        params: "{ owner?: string | null; repo?: string | null; priority?: string | null; limit?: number | null } | null",
        result: "HubRanking",
        scopes: UI_WEB,
    },
    MethodSpec {
        name: "hub.health.get",
        params: "{ owner?: string | null; repo?: string | null; min_severity?: string | null } | null",
        result: "HubHealth",
        scopes: UI_WEB,
    },
    MethodSpec {
        name: "hub.proposals.list",
        params: "{ state?: string | null; owner?: string | null; repo?: string | null; number?: number | null; limit?: number | null } | null",
        result: "HubProposals",
        scopes: UI_WEB,
    },
    // Um `id` devolve a proposta; `ids` devolve uma linha por id.
    MethodSpec {
        name: "hub.proposals.decide",
        params: "{ id?: number | null; ids?: number[] | null; approve: boolean; note?: string | null; value?: unknown }",
        result: "HubProposal | ProposalDecided[]",
        scopes: UI_WEB,
    },
    // Regras do agente (épico #353): a web só lê; editar, importar e decidir ficam no desktop.
    MethodSpec {
        name: "hub.rules.get",
        params: "{ owner: string; repo: string; epic_number?: number | null }",
        result: "HubRules",
        scopes: UI_WEB,
    },
    MethodSpec {
        name: "hub.rules.set",
        params: "{ owner: string; repo: string; epic_number?: number | null; content: string; base_version?: number | null; note?: string | null }",
        result: "HubRuleVersion",
        scopes: UI,
    },
    MethodSpec {
        name: "hub.rules.import",
        params: "{ owner: string; repo: string; epic_number?: number | null; content: string; base_version?: number | null; note?: string | null }",
        result: "HubRuleVersion",
        scopes: UI,
    },
    MethodSpec {
        name: "hub.rules.history",
        params: "{ owner: string; repo: string; epic_number?: number | null; limit?: number | null }",
        result: "HubRulesHistory",
        scopes: UI_WEB,
    },
    MethodSpec {
        name: "hub.rules.proposals.list",
        params: "{ state?: string | null; owner?: string | null; repo?: string | null; epic_number?: number | null; limit?: number | null } | null",
        result: "HubRulesProposals",
        scopes: UI_WEB,
    },
    MethodSpec {
        name: "hub.rules.proposals.decide",
        params: "{ id: number; approve: boolean; note?: string | null }",
        result: "HubRulesProposal",
        scopes: UI,
    },
    // Revogar só reduz acesso (spec §3.1 e §4.2).
    MethodSpec { name: "hub.web.sessions.list", params: "null", result: "HubWebSession[]", scopes: UI_WEB },
    MethodSpec {
        name: "hub.web.sessions.revoke",
        params: "{ id?: string | null; all?: boolean | null }",
        result: "WebSessionsRevoked",
        scopes: UI_WEB,
    },
    // Web Push (#203, spec §4.2): inscrever e escolher os eventos do próprio
    // dispositivo; listar, testar e remover não abrem nada além disso.
    MethodSpec { name: "hub.push.vapid_key", params: "null", result: "PushVapidKey", scopes: UI_WEB },
    MethodSpec {
        name: "hub.push.subscribe",
        params: "{ endpoint: string; keys: { p256dh: string; auth: string }; label?: string | null; events?: unknown }",
        result: "HubPushDevice",
        scopes: UI_WEB,
    },
    MethodSpec { name: "hub.push.list", params: "null", result: "HubPushDevice[]", scopes: UI_WEB },
    MethodSpec {
        name: "hub.push.update",
        params: "{ id: number; events?: unknown }",
        result: "HubPushDevice",
        scopes: UI_WEB,
    },
    MethodSpec { name: "hub.push.remove", params: "{ id: number }", result: "PushRemoved", scopes: UI_WEB },
    MethodSpec { name: "hub.push.test", params: "{ id: number }", result: "PushTestResult", scopes: UI_WEB },
    // Fase 2 (#343, spec §2.2 e §5.4): ações do ambiente pelo canal reverso. O
    // método do ambiente é conferido de novo contra `ct_http::sync::CHANNEL_METHODS`.
    MethodSpec {
        name: "hub.env.call",
        params: "{ env: string; method: string; params?: { [key: string]: unknown } | null }",
        result: "unknown",
        scopes: UI_WEB,
    },
    // Atualização do hub (#421): a web vê só o estado; verificar, disparar e voltar ficam no desktop.
    MethodSpec { name: "hub.update.get", params: "null", result: "HubUpdateStatus", scopes: UI_WEB },
    MethodSpec { name: "hub.update.check", params: "null", result: "HubUpdateStatus", scopes: UI },
    MethodSpec {
        name: "hub.update.start",
        params: "{ force?: boolean | null } | null",
        result: "unknown",
        scopes: UI,
    },
    MethodSpec {
        name: "hub.update.rollback",
        params: "{ force?: boolean | null } | null",
        result: "unknown",
        scopes: UI,
    },
    // Tela Logs (#65): leitura do log do hub, já mascarado; o modo seguir vale só para a conexão.
    MethodSpec {
        name: "hub.logs.tail",
        params: "{ level?: string | null; q?: string | null; since?: number | null; until?: number | null; limit?: number | null; before?: number | null } | null",
        result: "unknown",
        scopes: UI_WEB,
    },
    MethodSpec {
        name: "hub.logs.follow",
        params: "{ level?: string | null; q?: string | null; since?: number | null; until?: number | null }",
        result: "{}",
        scopes: UI_WEB,
    },
    MethodSpec { name: "hub.logs.unfollow", params: "null", result: "{}", scopes: UI_WEB },
    // Piloto automático (#183, painel na PWA #185). Pausar e retomar só reduzem o que o agendador faz.
    MethodSpec { name: "hub.autopilot.status", params: "null", result: "AutopilotStatus", scopes: UI_WEB },
    MethodSpec {
        name: "hub.autopilot.events",
        params: "{ limit?: number | null; before?: number | null } | null",
        result: "AutopilotEvent[]",
        scopes: UI_WEB,
    },
    MethodSpec { name: "hub.autopilot.pause", params: "null", result: "unknown", scopes: UI_WEB },
    MethodSpec { name: "hub.autopilot.resume", params: "null", result: "unknown", scopes: UI_WEB },
    // Resumo da manhã e branch de integração (#185): só leitura, sem a chave da conta.
    MethodSpec { name: "hub.autopilot.digest", params: "{ limit?: number | null } | null", result: "MorningDigest", scopes: UI_WEB },
    MethodSpec {
        name: "hub.autopilot.wip",
        params: "{ owner?: string | null; repo?: string | null } | null",
        result: "AutonomyWip[]",
        scopes: UI_WEB,
    },
    // Dependências entre issues (#215): a correção manual de uma aresta.
    MethodSpec {
        name: "hub.autopilot.deps.set",
        params: "{ owner: string; repo: string; action: string; [key: string]: unknown }",
        result: "{ dependencies: DependencyEdge[] }",
        scopes: UI_WEB,
    },
    MethodSpec { name: "hub.autopilot.set", params: "{ [key: string]: unknown }", result: "unknown", scopes: UI },
    // Execução de épico (#298): disparo, plano, controle e decisões.
    MethodSpec {
        name: "hub.epic_run.list",
        params: "{ owner?: string | null; repo?: string | null; epic?: number | null; ended_since?: number | null } | null",
        result: "EpicRun[]",
        scopes: UI_WEB,
    },
    MethodSpec { name: "hub.epic_run.status", params: "{ run_id: number }", result: "EpicRunView", scopes: UI_WEB },
    MethodSpec {
        name: "hub.epic_run.events",
        params: "{ run_id: number; after?: number | null; limit?: number | null }",
        result: "EpicRunEvent[]",
        scopes: UI_WEB,
    },
    MethodSpec {
        name: "hub.epic_run.start",
        params: "{ owner: string; repo: string; epic: number; env_id: string; max_parallel?: number | null; plan_model?: string | null; plan_effort?: string | null }",
        result: "EpicRun",
        scopes: UI_WEB,
    },
    MethodSpec {
        name: "hub.epic_run.approve",
        params: "{ run_id: number; version: number; max_parallel?: number | null; code_review?: boolean | null }",
        result: "EpicRun",
        scopes: UI_WEB,
    },
    MethodSpec { name: "hub.epic_run.replan", params: "{ run_id: number; reason?: string | null }", result: "EpicRun", scopes: UI_WEB },
    MethodSpec { name: "hub.epic_run.pause", params: "{ run_id: number; reason?: string | null }", result: "EpicRun", scopes: UI_WEB },
    MethodSpec { name: "hub.epic_run.interrupt", params: "{ run_id: number }", result: "EpicRun", scopes: UI_WEB },
    MethodSpec { name: "hub.epic_run.resume", params: "{ run_id: number }", result: "EpicRun", scopes: UI_WEB },
    MethodSpec { name: "hub.epic_run.cancel", params: "{ run_id: number }", result: "EpicRun", scopes: UI_WEB },
    MethodSpec { name: "hub.epic_run.set_parallel", params: "{ run_id: number; max_parallel: number }", result: "EpicRun", scopes: UI_WEB },
    MethodSpec { name: "hub.epic_run.set_code_review", params: "{ run_id: number; code_review: boolean }", result: "EpicRun", scopes: UI_WEB },
    MethodSpec { name: "hub.epic_run.decide", params: "{ decision_id: number; answer: string; blocker?: number | null }", result: "EpicDecision", scopes: UI_WEB },
    MethodSpec {
        name: "hub.epic_run.set_node",
        params: "{ run_id: number; number: number; model?: string | null; effort?: string | null }",
        result: "EpicPlan",
        scopes: UI_WEB,
    },
    MethodSpec { name: "hub.epic_run.handback", params: "{ run_id: number; number: number }", result: "EpicNode", scopes: UI_WEB },
    MethodSpec { name: "hub.autopilot.policy.get", params: "{ account: string }", result: "AutopilotPolicyView", scopes: UI },
    MethodSpec {
        name: "hub.autopilot.policy.set",
        params: "{ account: string; policy?: BudgetPolicy | null }",
        result: "AutopilotPolicyView",
        scopes: UI,
    },
    MethodSpec {
        name: "hub.autopilot.policy.preview",
        params: "{ account: string; policy: BudgetPolicy }",
        result: "BudgetDecision",
        scopes: UI,
    },
    // Backup e recuperação: configuração e segredos só no desktop.
    MethodSpec { name: "hub.backup.get", params: "null", result: "BackupConfigView", scopes: UI },
    MethodSpec { name: "hub.backup.set", params: "{ [key: string]: unknown }", result: "BackupConfigView", scopes: UI },
    MethodSpec {
        name: "hub.backup.test",
        params: "{ [key: string]: unknown } | null",
        result: "{ ok: boolean; error?: string }",
        scopes: UI,
    },
    MethodSpec { name: "hub.backup.run", params: "{ what?: string | null } | null", result: "BackupConfigView", scopes: UI },
    MethodSpec { name: "hub.backup.remote.list", params: "{ [key: string]: unknown } | null", result: "unknown", scopes: UI },
    MethodSpec { name: "hub.recovery.status", params: "null", result: "RecoveryStatus", scopes: UI },
    MethodSpec { name: "hub.recovery.generate", params: "null", result: "RecoveryGenerated", scopes: UI },
    MethodSpec { name: "hub.recovery.verify", params: "{ key: string }", result: "unknown", scopes: UI },
    MethodSpec { name: "hub.secrets.list", params: "null", result: "SecretsList", scopes: UI },
    MethodSpec {
        name: "hub.secrets.set",
        params: "{ scope: \"global\" | { project: string }; name: string; kind: \"secret\" | \"variable\"; value: string }",
        result: "SecretChanged",
        scopes: UI,
    },
    MethodSpec {
        name: "hub.secrets.delete",
        params: "{ scope: \"global\" | { project: string }; name: string }",
        result: "SecretChanged | {}",
        scopes: UI,
    },
    MethodSpec { name: "hub.secrets.rotate_pending_dismiss", params: "null", result: "{}", scopes: UI },
    // "Adicionar issue pelo ID" (#148): escreve só no espelho do Jira.
    MethodSpec {
        name: "hub.tracker.add_issue",
        params: "{ owner: string; repo: string; key: string | number }",
        result: "TrackerIssueAdded",
        scopes: UI_WEB,
    },
    MethodSpec {
        name: "hub.release.status",
        params: "{ owner: string; repo: string }",
        result: "HubReleaseStatus",
        scopes: UI_WEB,
    },
];

/// O registro tem `name`? Método fora dele não é despachado.
pub fn find(name: &str) -> Option<&'static MethodSpec> {
    METHODS.iter().find(|m| m.name == name)
}

/// `name` existe no registro e vale no escopo `scope`?
pub fn allowed_in(name: &str, scope: Scope) -> bool {
    find(name).is_some_and(|m| m.scopes.contains(&scope))
}

/// Métodos que a UI web pode chamar pelo `/web/ws`, derivados de
/// [`Scope::Web`]. Não há lista à mão: o que sai daqui é o que o registro diz.
pub const WEB_METHODS: &[&str] =
    &names_in::<{ count_in(METHODS, Scope::Web) }>(METHODS, Scope::Web);

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn nomes_do_registro() -> BTreeSet<&'static str> {
        METHODS.iter().map(|m| m.name).collect()
    }

    /// Os literais de método nos braços de despacho (`"hub.x" =>` ou `"hub.x" |`)
    /// de `ws.rs`, do piloto automático e da execução de épico, antes dos testes
    /// de cada arquivo.
    fn despachados() -> BTreeSet<String> {
        let fontes = [
            include_str!("ws.rs"),
            include_str!("autopilot/mod.rs"),
            include_str!("autopilot/runner.rs"),
            include_str!("epic_run/rpc.rs"),
        ];
        let mut found = BTreeSet::new();
        for src in fontes {
            let code = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
            for prefix in ["\"hub.", "\"github."] {
                for (i, _) in code.match_indices(prefix) {
                    let lit = &code[i + 1..];
                    let Some(j) = lit.find('"') else { continue };
                    let (lit, after) = (&lit[..j], lit[j + 1..].trim_start());
                    if !lit.ends_with('.') && (after.starts_with("=>") || after.starts_with('|')) {
                        found.insert(lit.to_string());
                    }
                }
            }
        }
        found
    }

    #[test]
    fn nomes_unicos() {
        let mut seen = BTreeSet::new();
        for m in METHODS {
            assert!(seen.insert(m.name), "método repetido: {}", m.name);
        }
    }

    #[test]
    fn todo_metodo_tem_escopo_de_ui() {
        for m in METHODS {
            assert!(m.scopes.contains(&Scope::Ui), "{}", m.name);
        }
    }

    /// Todo método despachado em `ws.rs` e no piloto automático está no
    /// registro, e todo método do registro tem braço de despacho.
    #[test]
    fn todo_metodo_do_hub_esta_no_registro() {
        let found = despachados();
        assert!(found.contains("hub.dashboard.get"), "{found:?}");
        let registro: BTreeSet<String> =
            nomes_do_registro().iter().map(|s| s.to_string()).collect();
        let sem_registro: Vec<_> = found.difference(&registro).collect();
        assert!(
            sem_registro.is_empty(),
            "despacho sem registro: {sem_registro:?}"
        );
        let sem_despacho: Vec<_> = registro.difference(&found).collect();
        assert!(
            sem_despacho.is_empty(),
            "registro sem despacho: {sem_despacho:?}"
        );
    }

    /// As listas de escopo saem do registro: a web é um subconjunto da UI,
    /// e `web_allowed` (usado no `/web/ws`) responde pelo registro.
    #[test]
    fn listas_de_escopo_derivadas() {
        let web: BTreeSet<&str> = METHODS
            .iter()
            .filter(|m| m.scopes.contains(&Scope::Web))
            .map(|m| m.name)
            .collect();
        let lista: BTreeSet<&str> = WEB_METHODS.iter().copied().collect();
        assert_eq!(lista, web);
        assert_eq!(WEB_METHODS.len(), web.len(), "sem repetido");
        for m in WEB_METHODS {
            assert!(allowed_in(m, Scope::Ui), "{m}");
            assert!(crate::web::web_allowed(m), "{m}");
        }
        assert!(!crate::web::web_allowed("hub.projects.delete"));
    }

    #[test]
    fn fora_do_registro_nao_vale() {
        assert!(find("hub.inventado").is_none());
        assert!(!allowed_in("hub.inventado", Scope::Ui));
        assert!(!allowed_in("hub.projects.add", Scope::Web));
        assert!(allowed_in("hub.projects.list", Scope::Web));
    }
}
