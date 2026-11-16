//! Templates do prompt inicial por projeto (#166): um override opcional por
//! tipo de sessão (issue, épico, sessão do projeto), no cadastro do projeto
//! no hub e entregue aos ambientes pelo `config_rev`. Vazio = o template
//! global do rastreador do projeto (o `ui.json` do app). Quem resolve e
//! renderiza é a UI (`ui/src/project/templates.ts`).
//!
//! A exceção é a sessão autônoma (#191): o ambiente monta o prompt dela, com
//! o template `autonomous` do projeto ou [`DEFAULT_AUTONOMOUS_TEMPLATE`], e
//! renderiza com [`AutonomousVars`] (o hub inicia a sessão sem a UI).
//!
//! A sessão de análise de dependências do piloto automático (#215) também
//! foge da UI: o hub renderiza o template `analysis` do projeto (ou
//! [`DEFAULT_ANALYSIS_TEMPLATE`]) com [`AnalysisVars`] e manda o prompt
//! pronto no `session.start_project`.

use crate::tracker::{change_label, item_ref, item_short, ForgeKind, TrackerConfig, TrackerRef};
use crate::WorkItemKind;
use serde::{Deserialize, Serialize};

/// Tamanho máximo de cada template, em bytes.
pub const MAX_SESSION_TEMPLATE_BYTES: usize = 64 * 1024;

/// Overrides do projeto (`HubProject.session_templates`). Ausente num hub
/// anterior: nenhum override.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SessionTemplates {
    /// Sessões de issue (avulsa, sub-issue ou sub-tarefa).
    pub issue: String,
    /// Sessões de épico.
    pub epic: String,
    /// Sessão do projeto, sem issue.
    pub project: String,
    /// Sessão autônoma (#191); vazio = [`DEFAULT_AUTONOMOUS_TEMPLATE`]. O
    /// ambiente renderiza, não a UI.
    pub autonomous: String,
    /// Sessão de análise de dependências do piloto automático (#215); vazio
    /// = [`DEFAULT_ANALYSIS_TEMPLATE`]. O hub renderiza, não a UI.
    pub analysis: String,
}

impl SessionTemplates {
    /// JSON da coluna `session_templates`.
    pub fn config_json(&self) -> String {
        serde_json::to_string(self).expect("SessionTemplates serializa")
    }

    /// Template só com espaços vira vazio (herda o global); o resto fica
    /// como veio.
    pub fn normalized(&self) -> SessionTemplates {
        let clean = |s: &str| {
            if s.trim().is_empty() {
                String::new()
            } else {
                s.to_string()
            }
        };
        SessionTemplates {
            issue: clean(&self.issue),
            epic: clean(&self.epic),
            project: clean(&self.project),
            autonomous: clean(&self.autonomous),
            analysis: clean(&self.analysis),
        }
    }

    /// Nenhum override.
    pub fn is_empty(&self) -> bool {
        self.issue.is_empty()
            && self.epic.is_empty()
            && self.project.is_empty()
            && self.autonomous.is_empty()
            && self.analysis.is_empty()
    }

    /// Template da sessão autônoma: o do projeto ou, vazio ou só com
    /// espaços, o padrão.
    pub fn autonomous_template(&self) -> &str {
        if self.autonomous.trim().is_empty() {
            DEFAULT_AUTONOMOUS_TEMPLATE
        } else {
            &self.autonomous
        }
    }

    /// Template da sessão de análise (#215): o do projeto ou, vazio ou só
    /// com espaços, o padrão.
    pub fn analysis_template(&self) -> &str {
        if self.analysis.trim().is_empty() {
            DEFAULT_ANALYSIS_TEMPLATE
        } else {
            &self.analysis
        }
    }

    /// Cada template cabe em [`MAX_SESSION_TEMPLATE_BYTES`].
    pub fn validate(&self) -> Result<(), String> {
        for (label, t) in [
            ("issue", &self.issue),
            ("epic", &self.epic),
            ("project", &self.project),
            ("autonomous", &self.autonomous),
            ("analysis", &self.analysis),
        ] {
            if t.len() > MAX_SESSION_TEMPLATE_BYTES {
                return Err(format!(
                    "session_templates.{label}: no máximo {} KiB ({} bytes informados)",
                    MAX_SESSION_TEMPLATE_BYTES / 1024,
                    t.len()
                ));
            }
        }
        Ok(())
    }
}

/// Template padrão da sessão autônoma (#191). Vale para GitHub e Jira:
/// `{{ref}}` é `owner/repo#12` ou `ACME-123`, `{{change}}` é `PR` ou `MR`, e
/// `{{closes}}` é a linha que o corpo do PR precisa ter: `Closes #12` no
/// GitHub (o guarda-corpo recusa o `gh pr create` sem ela, #375) ou a chave
/// `ACME-123` no Jira.
pub const DEFAULT_AUTONOMOUS_TEMPLATE: &str = "\
Sessão autônoma do Control Tower: trabalhe na {{kind}} {{ref}} ({{title}}) do repositório \
{{owner}}/{{repo}} do começo ao fim, sem perguntar nada: ninguém acompanha esta sessão.
Branch de trabalho: {{branch}}, saída de {{integration_branch}} (a branch de integração do modo \
autônomo). Nunca mescle nem faça push em {{protected}}.
Antes de começar, consulte decisões anteriores com `ledger_list_decisions`.
Diante de uma dúvida, escolha a opção mais conservadora que resolva a issue e siga em frente. \
Registre toda decisão relevante com `ledger_record_decision` e origin `autonoma`: elas são \
revisadas no {{change}}.
Siga as regras do projeto e do épico do system prompt; releia com `rules_get`.
Ao terminar, com os testes passando, abra o {{change}} de {{branch}} para {{integration_branch}} \
(nunca para outra branch), com a linha `{{closes}}` no corpo, exatamente assim, e resuma o que \
foi feito e as decisões tomadas. Não mescle o {{change}}.
Antes de dizer que o {{change}} está pronto, consulte `pr_feedback`: corrija os checks que \
falharam (o log vem com `pr_check_log`) e as threads de review não resolvidas, e faça push de novo.";

/// Variáveis do template da sessão autônoma.
#[derive(Debug, Clone, Copy)]
pub struct AutonomousVars<'a> {
    /// Projeto (owner, repo e rastreador), para `{{ref}}`.
    pub tracker: TrackerRef<'a>,
    /// Forja do repositório, para `{{change}}` (`PR` ou `MR`).
    pub forge: ForgeKind,
    pub kind: WorkItemKind,
    pub number: i64,
    pub title: &'a str,
    /// Branch da sessão.
    pub branch: &'a str,
    /// Branch de integração do modo autônomo (`wip/autonoma`).
    pub integration_branch: &'a str,
    pub protected: &'a [String],
}

impl AutonomousVars<'_> {
    /// Valor da variável; `None` = desconhecida.
    fn value(&self, name: &str) -> Option<String> {
        Some(match name {
            "kind" => match self.kind {
                WorkItemKind::Epic => "épico".into(),
                WorkItemKind::Issue => "issue".into(),
            },
            "number" => self.number.to_string(),
            "ref" => item_ref(&self.tracker, self.number),
            "owner" => self.tracker.owner.into(),
            "repo" => self.tracker.repo.into(),
            "title" => self.title.into(),
            "branch" => self.branch.into(),
            "integration_branch" => self.integration_branch.into(),
            "protected" => self.protected.join(", "),
            "change" => change_label(self.forge).into(),
            "closes" => closes_line(&self.tracker, self.number),
            _ => return None,
        })
    }

    /// O template com as variáveis trocadas.
    pub fn render(&self, template: &str) -> String {
        render_template(template, |name| self.value(name))
    }
}

/// Template padrão da sessão de análise de dependências do piloto
/// automático (#215). `{{issues}}` é a lista das issues da fila a analisar
/// (uma por linha) e `{{in_progress}}` a das que já estão em andamento ou em
/// revisão; `{{ref}}` não existe aqui (a sessão é do projeto).
pub const DEFAULT_ANALYSIS_TEMPLATE: &str = "\
Sessão de análise de dependências do piloto automático do Control Tower, no repositório \
{{owner}}/{{repo}}. Ninguém acompanha esta sessão: não pergunte nada.
Sua tarefa é montar a ordem de execução e as dependências entre as issues da fila do modo \
autônomo. Você não implementa nada, não cria branch, não faz commit nem push e não altera issues \
no rastreador (nada de comentar, rotular ou fechar).
Issues da fila (abertas, aprovadas e com prioridade):
{{issues}}
Issues já em andamento ou em revisão (podem bloquear as da fila):
{{in_progress}}
Dependências já gravadas (as manuais foram corrigidas pela pessoa e prevalecem):
{{dependencies}}
Leia cada issue no rastreador ({{tracker_hint}}): título, corpo, épico, menções como \
“Depende de #N” e as áreas do código citadas. Leia o código na branch de integração \
{{integration_branch}} sem trocar de branch: `git fetch origin {{integration_branch}}` e depois \
`git show origin/{{integration_branch}}:<caminho>` ou `git log origin/{{integration_branch}}`.
Uma issue depende de outra quando precisa do código, da migração ou da decisão dela já \
mesclados para começar sem retrabalho ou conflito. Na dúvida, não crie a dependência.
Grave o resultado com a ferramenta `autopilot_set_dependencies`: para cada issue da fila, a lista \
de issues que a bloqueiam (vazia se nenhuma), com uma justificativa curta para cada uma, e a \
ordem de execução sugerida. Depois disso, fique parado: o hub manda aqui as issues novas ou \
alteradas, e você reanalisa só o necessário e grava de novo.";

/// Variáveis do template da sessão de análise (#215).
#[derive(Debug, Clone, Copy)]
pub struct AnalysisVars<'a> {
    pub owner: &'a str,
    pub repo: &'a str,
    /// Branch de integração do modo autônomo (`wip/autonoma`).
    pub integration_branch: &'a str,
    /// Linhas das issues a analisar (`- #12 Título`).
    pub issues: &'a str,
    /// Linhas das issues em andamento ou em revisão.
    pub in_progress: &'a str,
    /// Linhas das arestas já gravadas (`- #12 bloqueada por #11 (análise)`).
    pub dependencies: &'a str,
    /// Como ler as issues: `gh issue view <n>` no GitHub, a chave no Jira.
    pub tracker_hint: &'a str,
}

impl AnalysisVars<'_> {
    fn value(&self, name: &str) -> Option<String> {
        Some(match name {
            "owner" => self.owner.into(),
            "repo" => self.repo.into(),
            "integration_branch" => self.integration_branch.into(),
            "issues" => self.issues.into(),
            "in_progress" => self.in_progress.into(),
            "dependencies" => self.dependencies.into(),
            "tracker_hint" => self.tracker_hint.into(),
            _ => return None,
        })
    }

    /// O template com as variáveis trocadas.
    pub fn render(&self, template: &str) -> String {
        render_template(template, |name| self.value(name))
    }
}

/// A linha do corpo do PR que liga a issue: `Closes #12` no GitHub (só as
/// palavras-chave em inglês fecham a issue, e o guarda-corpo exige essa
/// forma) e a chave `ACME-123` no Jira.
fn closes_line(t: &TrackerRef, n: i64) -> String {
    match t.tracker {
        TrackerConfig::Github => format!("Closes {}", item_short(t, n)),
        TrackerConfig::Jira(_) => item_short(t, n),
    }
}

/// Troca `{{var}}` (espaços em volta do nome aceitos) por `value(var)`;
/// variável desconhecida fica como está, como no `renderTemplate` da UI.
pub fn render_template(template: &str, value: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let replaced = after.find("}}").and_then(|end| {
            let name = after[..end].trim_matches(|c: char| c.is_ascii_whitespace());
            let word =
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if word {
                value(name).map(|v| (v, end))
            } else {
                None
            }
        });
        match replaced {
            Some((v, end)) => {
                out.push_str(&v);
                rest = &after[end + 2..];
            }
            None => {
                // Não é variável conhecida: um `{` fica, e a busca segue logo
                // depois dele (como a regex da UI, que tenta cada posição).
                out.push('{');
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn so_espacos_vira_vazio_e_o_resto_fica_intacto() {
        let t = SessionTemplates {
            issue: "  \n ".into(),
            epic: "  Épico {{key}}\n".into(),
            project: String::new(),
            autonomous: " \t".into(),
            analysis: "  ".into(),
        }
        .normalized();
        assert_eq!(t.issue, "");
        assert_eq!(t.autonomous, "");
        assert_eq!(t.epic, "  Épico {{key}}\n");
        assert!(!t.is_empty());
        assert!(SessionTemplates::default().is_empty());
    }

    #[test]
    fn json_ausente_ou_parcial_herda() {
        let t: SessionTemplates = serde_json::from_str("{}").unwrap();
        assert!(t.is_empty());
        let t: SessionTemplates = serde_json::from_str(r#"{"issue":"x"}"#).unwrap();
        assert_eq!(t.issue, "x");
        assert_eq!(t.epic, "");
        let back: SessionTemplates = serde_json::from_str(&t.config_json()).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn recusa_template_grande_demais() {
        let t = SessionTemplates {
            project: "x".repeat(MAX_SESSION_TEMPLATE_BYTES + 1),
            ..Default::default()
        };
        assert!(t
            .validate()
            .unwrap_err()
            .contains("session_templates.project"));
        assert!(SessionTemplates::default().validate().is_ok());
    }

    use crate::tracker::{JiraConfig, TrackerConfig};

    fn vars<'a>(
        t: TrackerRef<'a>,
        forge: ForgeKind,
        protected: &'a [String],
    ) -> AutonomousVars<'a> {
        AutonomousVars {
            tracker: t,
            forge,
            kind: WorkItemKind::Issue,
            number: 12,
            title: "Tela de login",
            branch: "feat/12-tela-de-login",
            integration_branch: "wip/autonoma",
            protected,
        }
    }

    #[test]
    fn padrao_renderizado_no_github() {
        let protected = vec!["main".to_string(), "develop".to_string()];
        let v = vars(
            TrackerRef::github("acme", "acme-web"),
            ForgeKind::Github,
            &protected,
        );
        let p = v.render(SessionTemplates::default().autonomous_template());
        assert!(p.contains("issue acme/acme-web#12 (Tela de login)"), "{p}");
        assert!(
            p.contains("Branch de trabalho: feat/12-tela-de-login, saída de wip/autonoma"),
            "{p}"
        );
        assert!(
            p.contains("abra o PR de feat/12-tela-de-login para wip/autonoma"),
            "{p}"
        );
        assert!(p.contains("origin `autonoma`"), "{p}");
        assert!(p.contains("main, develop"), "{p}");
        // O corpo pedido passa no guarda-corpo do `gh pr create` (#375).
        assert!(p.contains("com a linha `Closes #12` no corpo"), "{p}");
        assert!(crate::guard::closes::closes(&p, 12), "{p}");
        assert!(!p.contains("{{"), "variável sem trocar: {p}");
    }

    #[test]
    fn padrao_renderizado_no_jira_com_mr() {
        let jira = TrackerConfig::Jira(JiraConfig {
            url: "https://jira.example.com".into(),
            project_key: "ACME".into(),
            ..Default::default()
        });
        let v = AutonomousVars {
            number: 123,
            ..vars(
                TrackerRef::new("acme", "acme-web", &jira),
                ForgeKind::Gitlab,
                &[],
            )
        };
        let p = v.render(DEFAULT_AUTONOMOUS_TEMPLATE);
        assert!(p.contains("issue ACME-123 (Tela de login)"), "{p}");
        assert!(
            p.contains("abra o MR de feat/12-tela-de-login para wip/autonoma"),
            "{p}"
        );
        assert!(p.contains("com a linha `ACME-123` no corpo"), "{p}");
        assert!(!p.contains("Closes"), "{p}");
        assert!(!p.contains(" PR "), "{p}");
    }

    /// O placeholder da UI (`DEFAULT_AUTONOMOUS_TEMPLATE` em
    /// `ui/src/hub/autonomy.ts`) é o mesmo texto, linha a linha.
    #[test]
    fn padrao_espelhado_na_ui() {
        let ts =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/hub/autonomy.ts");
        let Ok(ts) = std::fs::read_to_string(&ts) else {
            return; // fora do repositório (crate empacotado)
        };
        for line in DEFAULT_AUTONOMOUS_TEMPLATE.lines() {
            assert!(!line.contains(['"', '\\']), "{line}");
            assert!(
                ts.contains(&format!("\"{line}\"")),
                "linha fora do espelho da UI: {line}"
            );
        }
    }

    #[test]
    fn padrao_de_analise_espelhado_na_ui() {
        let ts =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/hub/autonomy.ts");
        let Ok(ts) = std::fs::read_to_string(&ts) else {
            return; // fora do repositório (crate empacotado)
        };
        for line in DEFAULT_ANALYSIS_TEMPLATE.lines() {
            assert!(!line.contains(['"', '\\']), "{line}");
            assert!(
                ts.contains(&format!("\"{line}\"")),
                "linha fora do espelho da UI: {line}"
            );
        }
    }

    #[test]
    fn sobrescrita_do_projeto_e_so_espacos_vale_o_padrao() {
        let mut t = SessionTemplates {
            autonomous: "Faça {{ref}} em {{branch}}".into(),
            ..Default::default()
        };
        assert_eq!(t.autonomous_template(), "Faça {{ref}} em {{branch}}");
        let v = vars(
            TrackerRef::github("acme", "acme-web"),
            ForgeKind::Github,
            &[],
        );
        assert_eq!(
            v.render(t.autonomous_template()),
            "Faça acme/acme-web#12 em feat/12-tela-de-login"
        );
        t.autonomous = "  \n ".into();
        assert_eq!(t.autonomous_template(), DEFAULT_AUTONOMOUS_TEMPLATE);
    }

    #[test]
    fn variaveis_e_desconhecidas_como_na_ui() {
        let protected = vec!["main".to_string()];
        let v = AutonomousVars {
            kind: WorkItemKind::Epic,
            ..vars(
                TrackerRef::github("acme", "acme-web"),
                ForgeKind::Github,
                &protected,
            )
        };
        assert_eq!(
            v.render(
                "{{kind}} {{ number }} {{owner}}/{{repo}} {{integration_branch}} {{protected}} \
                 {{change}} {{nada}} {{ }} {{a-b}} {{ref"
            ),
            "épico 12 acme/acme-web wip/autonoma main PR {{nada}} {{ }} {{a-b}} {{ref"
        );
        assert_eq!(v.render("{{{{ref}}}}"), "{{acme/acme-web#12}}");
        assert_eq!(v.render("{{{ref}}}"), "{acme/acme-web#12}");
        assert_eq!(v.render("sem variáveis"), "sem variáveis");
    }

    #[test]
    fn template_de_analise_do_projeto_ou_o_padrao() {
        let v = AnalysisVars {
            owner: "acme",
            repo: "acme-web",
            integration_branch: "wip/autonoma",
            issues: "- #12 Tela de login",
            in_progress: "(nenhuma)",
            dependencies: "- #12 bloqueada por #11 (manual)",
            tracker_hint: "gh issue view <número>",
        };
        let mut t = SessionTemplates::default();
        assert_eq!(t.analysis_template(), DEFAULT_ANALYSIS_TEMPLATE);
        let out = v.render(t.analysis_template());
        assert!(out.contains("acme/acme-web"), "{out}");
        assert!(out.contains("- #12 Tela de login"), "{out}");
        assert!(out.contains("- #12 bloqueada por #11 (manual)"), "{out}");
        assert!(out.contains("git show origin/wip/autonoma:"), "{out}");
        assert!(out.contains("autopilot_set_dependencies"), "{out}");
        assert!(!out.contains("{{"), "{out}");
        t.analysis = "Analise {{issues}} em {{repo}} {{ref}}".into();
        assert_eq!(
            v.render(t.analysis_template()),
            "Analise - #12 Tela de login em acme-web {{ref}}"
        );
        t.analysis = " \n".into();
        assert_eq!(t.analysis_template(), DEFAULT_ANALYSIS_TEMPLATE);
    }
}
