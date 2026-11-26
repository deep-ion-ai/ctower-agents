//! O prompt da sessão de plano da execução de épico (#299): o hub monta e
//! manda pronto no `session.start_project` com `epic_plan`.

use ct_core::{CLAUDE_EFFORTS, CLAUDE_MODELS};

/// Modelo padrão da sessão de plano: forte, porque o plano decide o modelo
/// das demais.
pub const PLAN_MODEL_DEFAULT: &str = "opus";
/// Esforço padrão da sessão de plano.
pub const PLAN_EFFORT_DEFAULT: &str = "high";

/// Uma sub-issue do épico no espelho do hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanIssue {
    pub number: i64,
    pub title: String,
    /// `open` ou `closed`.
    pub state: String,
}

/// O que o prompt da sessão de plano mostra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanVars {
    pub owner: String,
    pub repo: String,
    /// Referência do épico (`owner/repo#n` ou a chave).
    pub epic_ref: String,
    pub epic_title: String,
    /// Branch de integração do épico.
    pub wip: String,
    pub issues: Vec<PlanIssue>,
    /// Arestas corrigidas à mão pelo desenvolvedor: (issue, bloqueante,
    /// removida).
    pub manual: Vec<(i64, i64, bool)>,
}

/// O prompt inicial da sessão de plano.
pub fn plan_prompt(v: &PlanVars) -> String {
    let issues = if v.issues.is_empty() {
        "(nenhuma no espelho)".to_string()
    } else {
        v.issues
            .iter()
            .map(|i| {
                let closed = if i.state == "closed" {
                    " (fechada)"
                } else {
                    ""
                };
                format!("- #{} {}{closed}", i.number, i.title)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let manual = if v.manual.is_empty() {
        "(nenhuma)".to_string()
    } else {
        v.manual
            .iter()
            .map(|(n, b, removed)| {
                if *removed {
                    format!("- #{n} NÃO depende de #{b} (removida pelo desenvolvedor)")
                } else {
                    format!("- #{n} depende de #{b}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "Sessão de plano do Control Tower para o épico {epic} ({title}) do repositório \
{owner}/{repo}.

Seu trabalho é só planejar: não altere arquivos, não crie branches, issues nem PRs. Leia o \
épico, as sub-issues e o código do repositório o quanto precisar para decidir.

Sub-issues do épico:
{issues}

Dependências já corrigidas à mão pelo desenvolvedor (valem sobre as suas):
{manual}

Para cada sub-issue aberta, decida:
- quais issues precisam estar mescladas na {wip} antes dela (blocked_by), com uma \
justificativa curta;
- o modelo do agente ({models}) e o esforço ({efforts}): use o mais barato que resolve com \
segurança, e um forte só onde o risco ou o tamanho pedem;
- as áreas do repositório que ela altera, como caminhos ou globs relativos à raiz (ex.: \
crates/ct-core/, ui/src/**/*.tsx). Sub-issues com áreas sobrepostas não rodam ao mesmo \
tempo, então seja específico;
- a ordem sugerida de execução e uma justificativa curta.
Se faltar uma sub-issue ou uma delas for grande demais, proponha em proposals (kind nova ou \
dividir): não crie a issue.

Grave o plano uma vez com a ferramenta epic_set_plan (uma entrada em nodes por sub-issue \
aberta, na ordem sugerida, mais um resumo em summary) e encerre. Se o hub recusar (ciclo \
de dependências, sub-issue fora do épico), corrija e grave de novo. O desenvolvedor aprova \
ou pede outro plano.",
        epic = v.epic_ref,
        title = v.epic_title,
        owner = v.owner,
        repo = v.repo,
        wip = v.wip,
        models = CLAUDE_MODELS.join(", "),
        efforts = CLAUDE_EFFORTS.join(", "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(issues: Vec<PlanIssue>, manual: Vec<(i64, i64, bool)>) -> PlanVars {
        PlanVars {
            owner: "acme".into(),
            repo: "acme-web".into(),
            epic_ref: "acme/acme-web#10".into(),
            epic_title: "Login novo".into(),
            wip: "wip/10-login-novo".into(),
            issues,
            manual,
        }
    }

    #[test]
    fn prompt_traz_as_sub_issues_as_correcoes_e_a_ferramenta() {
        let issue = |n: i64, t: &str, s: &str| PlanIssue {
            number: n,
            title: t.into(),
            state: s.into(),
        };
        let p = plan_prompt(&vars(
            vec![issue(11, "Tela", "open"), issue(12, "API", "closed")],
            vec![(11, 12, false), (13, 11, true)],
        ));
        for s in [
            "acme/acme-web#10 (Login novo)",
            "- #11 Tela\n",
            "- #12 API (fechada)",
            "- #11 depende de #12",
            "- #13 NÃO depende de #11",
            "epic_set_plan",
            "wip/10-login-novo",
            "opus",
        ] {
            assert!(p.contains(s), "{s}\n{p}");
        }
        let vazio = plan_prompt(&vars(vec![], vec![]));
        assert!(vazio.contains("(nenhuma no espelho)") && vazio.contains("(nenhuma)"));
    }
}
