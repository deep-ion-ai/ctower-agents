//! Sessão de release (#204): a configuração do projeto (`HubProject.release`),
//! as etapas do `ctower release` e o estado que o `status` devolve.
//!
//! O fluxo é o do release-please: PR de promoção `source → target`, o PR de
//! release aberto pelo workflow do `target`, a publicação e o back-merge
//! `target → source`. Os merges e as aprovações são determinísticos: só as
//! etapas do `ctower release` os fazem, sempre com merge commit e presos ao
//! SHA conferido. Só projetos GitHub + GitHub.

use serde::{Deserialize, Serialize};

/// Número do work item sentinela da sessão de release do projeto (a sessão
/// do projeto, sem issue, é o `0`). Negativo: as consultas que olham só
/// issues (`gh_number > 0`) o ignoram.
pub const RELEASE_WORK_ITEM: i64 = -1;

/// Valor de `CTOWER_SESSION_KIND` na sessão de release.
pub const RELEASE_SESSION_KIND: &str = "release";

/// Nome padrão do secret do projeto com o token exclusivo da release.
pub const DEFAULT_RELEASE_TOKEN_SECRET: &str = "CT_RELEASE_GH_TOKEN";

/// Head padrão do PR de release; `{target}` vira a branch de destino.
pub const DEFAULT_RELEASE_PR_BRANCH: &str = "release-please--branches--{target}";

/// Prazo padrão de cada espera, em minutos.
pub const DEFAULT_RELEASE_TIMEOUT_MINUTES: u32 = 30;

/// Prazo máximo aceito para uma espera, em minutos.
pub const MAX_RELEASE_TIMEOUT_MINUTES: u32 = 24 * 60;

/// Fluxo de release. Só o release-please por enquanto.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum ReleaseFlow {
    #[default]
    ReleasePlease,
}

/// Prazo de cada etapa de espera, em minutos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ReleaseTimeouts {
    /// `wait-release-pr`: o workflow do `target` abrir ou atualizar o PR de
    /// release (com o commit dos lockfiles).
    pub release_pr: u32,
    /// `approve-runs` e os checks dos `merge-*`: os runs terminarem.
    pub runs: u32,
    /// `wait-publish`: a tag, os binários e a release fora do rascunho.
    pub publish: u32,
    /// `merge-back`: o PR de back-merge aparecer.
    pub back_merge: u32,
}

impl Default for ReleaseTimeouts {
    fn default() -> Self {
        let m = DEFAULT_RELEASE_TIMEOUT_MINUTES;
        ReleaseTimeouts {
            release_pr: m,
            runs: m,
            publish: m,
            back_merge: m,
        }
    }
}

/// Configuração de release do projeto (`HubProject.release`). Desligada por
/// padrão; ausente num hub anterior à #204.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ReleaseConfig {
    /// Mostra o botão "Criar release" e libera a sessão de release.
    pub enabled: bool,
    pub flow: ReleaseFlow,
    /// Branch promovida (`develop`).
    pub source: String,
    /// Branch de release (`main`).
    pub target: String,
    /// Head do PR de release; `{target}` vira [`ReleaseConfig::target`].
    pub release_pr_branch: String,
    /// Espera e mescla o PR `target → source` aberto pelo workflow.
    pub back_merge: bool,
    /// Aprova os runs em `action_required` dos PRs do fluxo.
    pub approve_pending_runs: bool,
    /// Nome do secret do projeto com o token exclusivo da release.
    pub token_secret: String,
    pub timeouts: ReleaseTimeouts,
}

impl Default for ReleaseConfig {
    fn default() -> Self {
        ReleaseConfig {
            enabled: false,
            flow: ReleaseFlow::ReleasePlease,
            source: "develop".into(),
            target: "main".into(),
            release_pr_branch: DEFAULT_RELEASE_PR_BRANCH.into(),
            back_merge: true,
            approve_pending_runs: true,
            token_secret: DEFAULT_RELEASE_TOKEN_SECRET.into(),
            timeouts: ReleaseTimeouts::default(),
        }
    }
}

impl ReleaseConfig {
    /// JSON da coluna `release_config`.
    pub fn config_json(&self) -> String {
        serde_json::to_string(self).expect("ReleaseConfig serializa")
    }

    /// Igual ao padrão (o JSON do pull não leva o campo).
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Sem espaços nas pontas; vazio volta ao padrão.
    pub fn normalized(&self) -> ReleaseConfig {
        let d = ReleaseConfig::default();
        let or = |s: &str, def: String| {
            let t = s.trim();
            if t.is_empty() {
                def
            } else {
                t.to_string()
            }
        };
        ReleaseConfig {
            enabled: self.enabled,
            flow: self.flow,
            source: or(&self.source, d.source),
            target: or(&self.target, d.target),
            release_pr_branch: or(&self.release_pr_branch, d.release_pr_branch),
            back_merge: self.back_merge,
            approve_pending_runs: self.approve_pending_runs,
            token_secret: or(&self.token_secret, d.token_secret),
            timeouts: self.timeouts,
        }
    }

    /// Branches válidas e distintas, nome de secret válido e prazos entre 1
    /// minuto e [`MAX_RELEASE_TIMEOUT_MINUTES`]. Confere a forma normalizada.
    pub fn validate(&self) -> Result<(), String> {
        let c = self.normalized();
        for (label, b) in [
            ("source", &c.source),
            ("target", &c.target),
            ("release_pr_branch", &c.rendered_pr_branch()),
        ] {
            if !crate::branch::valid_branch_name(b) {
                return Err(format!("release.{label}: nome de branch inválido ({b})"));
            }
        }
        if c.source == c.target {
            return Err("release: source e target precisam ser branches diferentes".into());
        }
        let head = c.rendered_pr_branch();
        if head == c.source || head == c.target {
            return Err(
                "release.release_pr_branch: precisa ser diferente de source e target".into(),
            );
        }
        if !valid_secret_name(&c.token_secret) {
            return Err(format!(
                "release.token_secret: use letras maiúsculas, dígitos e _, sem começar com \
                 dígito nem com CTOWER_ ou CTHUB_ ({})",
                c.token_secret
            ));
        }
        let t = c.timeouts;
        for (label, m) in [
            ("release_pr", t.release_pr),
            ("runs", t.runs),
            ("publish", t.publish),
            ("back_merge", t.back_merge),
        ] {
            if m == 0 || m > MAX_RELEASE_TIMEOUT_MINUTES {
                return Err(format!(
                    "release.timeouts.{label}: entre 1 e {MAX_RELEASE_TIMEOUT_MINUTES} minutos"
                ));
            }
        }
        Ok(())
    }

    /// Head do PR de release, com `{target}` trocado.
    pub fn release_pr_head(&self) -> String {
        self.normalized().rendered_pr_branch()
    }

    fn rendered_pr_branch(&self) -> String {
        self.release_pr_branch.replace("{target}", &self.target)
    }

    /// Nome do secret reservado à sessão de release, se a release está
    /// ligada. Desligada, o secret vale como qualquer outro.
    pub fn reserved_secret(&self) -> Option<String> {
        self.enabled.then(|| self.normalized().token_secret)
    }
}

/// Nome de secret aceito: `[A-Z_][A-Z0-9_]*`, sem os prefixos do Control
/// Tower. A lista completa de reservados fica em `ct_secrets`.
fn valid_secret_name(name: &str) -> bool {
    let mut chars = name.chars();
    let first_ok = matches!(chars.next(), Some(c) if c.is_ascii_uppercase() || c == '_');
    first_ok
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !name.starts_with("CTOWER_")
        && !name.starts_with("CTHUB_")
}

/// Etapas do `ctower release`, na ordem do fluxo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum ReleaseStep {
    Status,
    Promote,
    MergePromotion,
    WaitReleasePr,
    ApproveRuns,
    MergeRelease,
    WaitPublish,
    MergeBack,
    /// Nada a fazer: sem diferença de conteúdo e sem PR do fluxo aberto.
    Nothing,
}

impl ReleaseStep {
    pub fn as_str(self) -> &'static str {
        match self {
            ReleaseStep::Status => "status",
            ReleaseStep::Promote => "promote",
            ReleaseStep::MergePromotion => "merge-promotion",
            ReleaseStep::WaitReleasePr => "wait-release-pr",
            ReleaseStep::ApproveRuns => "approve-runs",
            ReleaseStep::MergeRelease => "merge-release",
            ReleaseStep::WaitPublish => "wait-publish",
            ReleaseStep::MergeBack => "merge-back",
            ReleaseStep::Nothing => "nothing",
        }
    }
}

/// Um PR do fluxo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ReleasePr {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub number: i64,
    pub title: String,
    pub url: String,
    pub head: String,
    pub base: String,
    pub head_sha: String,
}

/// Um run do GitHub Actions num head do fluxo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ReleaseRun {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub id: i64,
    pub name: String,
    pub head_sha: String,
    /// `queued`, `in_progress`, `completed`, `action_required`, ...
    pub status: String,
    pub conclusion: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub attempt: i64,
    pub url: String,
}

impl ReleaseRun {
    /// Esperando aprovação na aba Checks.
    pub fn pending_approval(&self) -> bool {
        self.status == "action_required" || self.conclusion.as_deref() == Some("action_required")
    }

    /// Terminou (com qualquer conclusão diferente de `action_required`).
    pub fn completed(&self) -> bool {
        self.status == "completed" && !self.pending_approval()
    }

    /// Terminou verde (`success`, `skipped` ou `neutral`).
    pub fn green(&self) -> bool {
        self.completed() && green_conclusion(self.conclusion.as_deref())
    }
}

/// Conclusão de run ou check que conta como verde.
pub fn green_conclusion(c: Option<&str>) -> bool {
    matches!(c, Some("success" | "skipped" | "neutral"))
}

/// A release do GitHub mais recente.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ReleaseInfo {
    pub tag: String,
    pub name: String,
    pub url: String,
    pub draft: bool,
}

/// Estado da release (`ctower release status`, `hub.release.status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ReleaseStatus {
    pub owner: String,
    pub repo: String,
    pub source: String,
    pub target: String,
    pub source_sha: String,
    pub target_sha: String,
    /// A árvore de `source` difere da de `target`: há o que lançar. Um
    /// back-merge sozinho (commits sem mudar arquivos) não conta.
    pub content_diff: bool,
    /// Commits de `target` que `source` não tem (back-merge pendente).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub target_ahead_by: i64,
    /// Commits de `source` que `target` não tem.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub source_ahead_by: i64,
    pub promotion_pr: Option<ReleasePr>,
    pub release_pr: Option<ReleasePr>,
    pub back_merge_pr: Option<ReleasePr>,
    /// Runs em `action_required` nos heads dos PRs abertos do fluxo.
    pub pending_runs: Vec<ReleaseRun>,
    /// A release mais recente do repositório.
    pub latest_release: Option<ReleaseInfo>,
    /// Próxima etapa sugerida.
    pub next: ReleaseStep,
}

impl ReleaseStatus {
    /// Próxima etapa pelo estado: um PR aberto do fluxo vem primeiro (o
    /// back-merge antes de tudo), depois a release em rascunho, depois a
    /// diferença de conteúdo.
    pub fn next_step(&self) -> ReleaseStep {
        if self.back_merge_pr.is_some() {
            return ReleaseStep::MergeBack;
        }
        if self.promotion_pr.is_some() {
            return ReleaseStep::MergePromotion;
        }
        if let Some(pr) = &self.release_pr {
            let pending = self.pending_runs.iter().any(|r| r.head_sha == pr.head_sha);
            return if pending {
                ReleaseStep::ApproveRuns
            } else {
                ReleaseStep::MergeRelease
            };
        }
        if self.latest_release.as_ref().is_some_and(|r| r.draft) {
            return ReleaseStep::WaitPublish;
        }
        if self.content_diff {
            return ReleaseStep::Promote;
        }
        ReleaseStep::Nothing
    }

    /// Há back-merge pendente: o PR `target → source` aberto.
    pub fn back_merge_pending(&self) -> bool {
        self.back_merge_pr.is_some()
    }
}

/// Versão do título do PR de release (`chore(main): release 1.2.3` →
/// `1.2.3`).
pub fn version_from_title(title: &str) -> Option<String> {
    let idx = title.rfind("release ")?;
    let v = title[idx + "release ".len()..].trim();
    let v = v.trim_start_matches('v');
    let ok = !v.is_empty()
        && v.chars().next().is_some_and(|c| c.is_ascii_digit())
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
    ok.then(|| v.to_string())
}

/// Resultado de uma etapa (saída JSON do `ctower release` e payload do
/// evento `release_step` no ledger).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepResult {
    /// A etapa fez o que devia.
    Ok,
    /// Já estava feita: nada mudou.
    Noop,
    /// Uma pré-condição não vale; nada foi feito.
    Refused,
    /// Falhou no meio (erro do GitHub, prazo esgotado, run vermelho).
    Failed,
}

/// Desfecho de uma etapa.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepOutcome {
    pub step: ReleaseStep,
    pub result: StepResult,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    /// Detalhes da etapa (runs aprovados, release publicada, estado).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub details: serde_json::Value,
}

impl StepOutcome {
    pub fn new(step: ReleaseStep, result: StepResult, message: impl Into<String>) -> Self {
        StepOutcome {
            step,
            result,
            message: message.into(),
            pr: None,
            url: None,
            sha: None,
            details: serde_json::Value::Null,
        }
    }

    pub fn with_pr(mut self, pr: &ReleasePr) -> Self {
        self.pr = Some(pr.number);
        self.url = Some(pr.url.clone());
        self.sha = Some(pr.head_sha.clone());
        self
    }

    pub fn with_sha(mut self, sha: impl Into<String>) -> Self {
        self.sha = Some(sha.into());
        self
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = details;
        self
    }

    /// Código de saída do `ctower release`: `0` ok/noop, `3` recusada, `1`
    /// falha.
    pub fn exit_code(&self) -> i32 {
        match self.result {
            StepResult::Ok | StepResult::Noop => 0,
            StepResult::Refused => 3,
            StepResult::Failed => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padrao_desligado_e_valido() {
        let c = ReleaseConfig::default();
        assert!(!c.enabled);
        assert!(c.is_default());
        assert!(c.validate().is_ok());
        assert_eq!(c.release_pr_head(), "release-please--branches--main");
        assert_eq!(c.reserved_secret(), None);
        let on = ReleaseConfig {
            enabled: true,
            ..Default::default()
        };
        assert_eq!(on.reserved_secret().as_deref(), Some("CT_RELEASE_GH_TOKEN"));
    }

    #[test]
    fn json_parcial_herda_o_padrao() {
        let c: ReleaseConfig = serde_json::from_str(r#"{"enabled":true,"target":"prod"}"#).unwrap();
        assert!(c.enabled);
        assert_eq!(c.source, "develop");
        assert_eq!(c.release_pr_head(), "release-please--branches--prod");
        assert_eq!(c.timeouts.runs, 30);
        let back: ReleaseConfig = serde_json::from_str(&c.config_json()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn normaliza_espacos_e_vazios() {
        let c = ReleaseConfig {
            source: "  ".into(),
            target: " prod ".into(),
            token_secret: String::new(),
            ..Default::default()
        }
        .normalized();
        assert_eq!(c.source, "develop");
        assert_eq!(c.target, "prod");
        assert_eq!(c.token_secret, DEFAULT_RELEASE_TOKEN_SECRET);
    }

    #[test]
    fn validacao_recusa_o_que_nao_serve() {
        let bad = |c: ReleaseConfig| c.validate().unwrap_err();
        assert!(bad(ReleaseConfig {
            target: "develop".into(),
            ..Default::default()
        })
        .contains("diferentes"));
        assert!(bad(ReleaseConfig {
            source: "a..b".into(),
            ..Default::default()
        })
        .contains("release.source"));
        assert!(bad(ReleaseConfig {
            release_pr_branch: "{target}".into(),
            ..Default::default()
        })
        .contains("release_pr_branch"));
        for name in ["gh_token", "1TOKEN", "CTOWER_X", "CTHUB_X", "A-B"] {
            assert!(bad(ReleaseConfig {
                token_secret: name.into(),
                ..Default::default()
            })
            .contains("token_secret"));
        }
        assert!(bad(ReleaseConfig {
            timeouts: ReleaseTimeouts {
                runs: 0,
                ..Default::default()
            },
            ..Default::default()
        })
        .contains("timeouts.runs"));
    }

    fn pr(n: i64, sha: &str) -> ReleasePr {
        ReleasePr {
            number: n,
            title: format!("PR {n}"),
            url: format!("https://github.com/acme/acme-web/pull/{n}"),
            head: "h".into(),
            base: "b".into(),
            head_sha: sha.into(),
        }
    }

    fn status() -> ReleaseStatus {
        ReleaseStatus {
            owner: "acme".into(),
            repo: "acme-web".into(),
            source: "develop".into(),
            target: "main".into(),
            source_sha: "s".into(),
            target_sha: "t".into(),
            content_diff: false,
            target_ahead_by: 0,
            source_ahead_by: 0,
            promotion_pr: None,
            release_pr: None,
            back_merge_pr: None,
            pending_runs: vec![],
            latest_release: None,
            next: ReleaseStep::Nothing,
        }
    }

    #[test]
    fn proxima_etapa() {
        let mut s = status();
        assert_eq!(s.next_step(), ReleaseStep::Nothing);
        s.content_diff = true;
        assert_eq!(s.next_step(), ReleaseStep::Promote);
        s.latest_release = Some(ReleaseInfo {
            tag: "v1.2.3".into(),
            name: "v1.2.3".into(),
            url: "u".into(),
            draft: true,
        });
        assert_eq!(s.next_step(), ReleaseStep::WaitPublish);
        s.release_pr = Some(pr(2, "abc"));
        assert_eq!(s.next_step(), ReleaseStep::MergeRelease);
        s.pending_runs = vec![ReleaseRun {
            id: 1,
            name: "ci".into(),
            head_sha: "abc".into(),
            status: "action_required".into(),
            conclusion: None,
            attempt: 1,
            url: "u".into(),
        }];
        assert_eq!(s.next_step(), ReleaseStep::ApproveRuns);
        s.promotion_pr = Some(pr(1, "def"));
        assert_eq!(s.next_step(), ReleaseStep::MergePromotion);
        s.back_merge_pr = Some(pr(3, "ghi"));
        assert_eq!(s.next_step(), ReleaseStep::MergeBack);
    }

    #[test]
    fn versao_do_titulo() {
        assert_eq!(
            version_from_title("chore(main): release 1.2.3").as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            version_from_title("chore(main): release v1.2.3-rc.1").as_deref(),
            Some("1.2.3-rc.1")
        );
        assert_eq!(version_from_title("chore: back-merge"), None);
        assert_eq!(version_from_title("release notes"), None);
    }

    #[test]
    fn run_verde_e_pendente() {
        let mut r = ReleaseRun {
            id: 1,
            name: "ci".into(),
            head_sha: "a".into(),
            status: "completed".into(),
            conclusion: Some("action_required".into()),
            attempt: 1,
            url: "u".into(),
        };
        assert!(r.pending_approval());
        assert!(!r.completed());
        r.conclusion = Some("success".into());
        assert!(r.green());
        r.conclusion = Some("failure".into());
        assert!(r.completed() && !r.green());
    }

    #[test]
    fn codigo_de_saida() {
        let o = |r| StepOutcome::new(ReleaseStep::Promote, r, "x").exit_code();
        assert_eq!(o(StepResult::Ok), 0);
        assert_eq!(o(StepResult::Noop), 0);
        assert_eq!(o(StepResult::Refused), 3);
        assert_eq!(o(StepResult::Failed), 1);
    }
}
