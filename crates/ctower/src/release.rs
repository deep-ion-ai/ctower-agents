//! `ctower release <etapa>` (#204): as etapas determinísticas da release,
//! só dentro da sessão de release.
//!
//! Cada execução pede ao ctowerd, com o token da sessão, o contexto (o
//! repositório e a configuração de release do projeto) e o token exclusivo
//! da release (o secret de `release.token_secret`), roda a etapa contra a
//! API do GitHub com esse token (`ct_gh::release`), imprime o desfecho em
//! JSON no stdout e o grava no ledger (`POST /session/release/event`).
//!
//! Saída: `0` ok ou nada a fazer, `3` recusada (uma pré-condição não vale),
//! `1` falha (erro do GitHub, prazo esgotado, fora da sessão de release).

use ct_core::release::{ReleaseConfig, ReleaseStep, StepOutcome, StepResult};
use ct_gh::release::{Pace, Release};
use ct_gh::{GhClient, GhConfig};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

/// Prazo das chamadas ao ctowerd.
const DAEMON_TIMEOUT: Duration = Duration::from_secs(10);

/// Etapa pedida na linha de comando.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum ReleaseCmd {
    /// Estado da release em JSON: diferença de conteúdo source×target, PRs
    /// abertos do fluxo, runs pendentes, a release mais recente e a próxima
    /// etapa.
    Status,
    /// Abre (ou reaproveita) o PR source → target. Recusa sem diferença de
    /// conteúdo ou com back-merge pendente.
    Promote {
        /// Arquivo com o corpo do PR (Markdown).
        #[arg(long, value_name = "arquivo")]
        body_file: Option<PathBuf>,
        /// Título do PR (padrão: `release: <source> -> <target>`).
        #[arg(long)]
        title: Option<String>,
    },
    /// Mescla o PR de promoção com merge commit, preso ao SHA, com os
    /// checks obrigatórios verdes.
    MergePromotion {
        /// SHA conferido do head do PR (recusa se mudou).
        #[arg(long)]
        sha: Option<String>,
    },
    /// Espera o workflow do target abrir ou atualizar o PR de release (com
    /// o commit dos lockfiles).
    WaitReleasePr,
    /// Aprova os runs em action_required do head do PR, espera terminarem e
    /// reexecuta uma vez os que falharam.
    ApproveRuns {
        /// Número do PR do fluxo.
        pr: i64,
    },
    /// Mescla o PR de release com merge commit, preso ao SHA, com os runs
    /// aprovados e verdes.
    MergeRelease {
        #[arg(long)]
        sha: Option<String>,
    },
    /// Espera a tag, os jobs de binários e a release sair do rascunho.
    WaitPublish,
    /// Espera o PR target → source, aprova os runs e mescla com merge
    /// commit, preso ao SHA.
    MergeBack {
        #[arg(long)]
        sha: Option<String>,
    },
}

impl ReleaseCmd {
    fn step(&self) -> ReleaseStep {
        match self {
            ReleaseCmd::Status => ReleaseStep::Status,
            ReleaseCmd::Promote { .. } => ReleaseStep::Promote,
            ReleaseCmd::MergePromotion { .. } => ReleaseStep::MergePromotion,
            ReleaseCmd::WaitReleasePr => ReleaseStep::WaitReleasePr,
            ReleaseCmd::ApproveRuns { .. } => ReleaseStep::ApproveRuns,
            ReleaseCmd::MergeRelease { .. } => ReleaseStep::MergeRelease,
            ReleaseCmd::WaitPublish => ReleaseStep::WaitPublish,
            ReleaseCmd::MergeBack { .. } => ReleaseStep::MergeBack,
        }
    }
}

/// Contexto vindo do ctowerd (`POST /session/release/context`).
#[derive(Debug, Deserialize)]
struct Context {
    owner: String,
    repo: String,
    release: ReleaseConfig,
    api_url: String,
}

/// Conexão com o ctowerd pela sessão.
struct Daemon {
    url: String,
    token: String,
    agent: ureq::Agent,
}

impl Daemon {
    fn from_env() -> Result<Daemon, String> {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let token = env(ct_sessions::SESSION_TOKEN_ENV).ok_or_else(|| {
            format!(
                "{} ausente: `ctower release` só roda dentro da sessão de release",
                ct_sessions::SESSION_TOKEN_ENV
            )
        })?;
        let url = env("CTOWER_URL").ok_or("CTOWER_URL ausente")?;
        Ok(Daemon {
            url: url.trim_end_matches('/').to_string(),
            token,
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(2))
                .timeout(DAEMON_TIMEOUT)
                .build(),
        })
    }

    fn post(&self, path: &str, body: &Value) -> Result<String, String> {
        let r = self
            .agent
            .post(&format!("{}{path}", self.url))
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .send_string(&body.to_string());
        match r {
            Ok(r) => r.into_string().map_err(|e| e.to_string()),
            Err(ureq::Error::Status(code, r)) => {
                let msg = r.into_string().unwrap_or_default().trim().to_string();
                Err(if msg.is_empty() {
                    format!("HTTP {code}")
                } else {
                    msg
                })
            }
            Err(ureq::Error::Transport(e)) => Err(format!("ctowerd fora do ar: {e}")),
        }
    }

    fn context(&self) -> Result<Context, String> {
        let body = self.post("/session/release/context", &json!({}))?;
        serde_json::from_str(&body).map_err(|e| format!("contexto inválido do ctowerd: {e}"))
    }

    fn secret(&self, name: &str) -> Result<String, String> {
        self.post("/session/secret", &json!({ "name": name }))
            .map_err(|e| format!("token da release ({name}) indisponível: {e}"))
    }

    fn event(&self, payload: &Value) -> Result<(), String> {
        self.post("/session/release/event", payload).map(|_| ())
    }
}

/// Ritmo das esperas; `CTOWER_RELEASE_POLL_MS` e `CTOWER_RELEASE_MINUTE_MS`
/// encurtam nos testes.
fn pace() -> Pace {
    let ms = |k: &str| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_millis)
    };
    let d = Pace::default();
    Pace {
        interval: ms("CTOWER_RELEASE_POLL_MS").unwrap_or(d.interval),
        minute: ms("CTOWER_RELEASE_MINUTE_MS").unwrap_or(d.minute),
    }
}

/// Roda a etapa e devolve o código de saída.
pub fn run(cmd: ReleaseCmd) -> i32 {
    let step = cmd.step();
    let daemon = match Daemon::from_env() {
        Ok(d) => d,
        Err(e) => return fail(step, &e),
    };
    // Fora da sessão de release o ctowerd recusa o contexto (403).
    let ctx = match daemon.context() {
        Ok(c) => c,
        Err(e) => return fail(step, &e),
    };
    let token = match daemon.secret(&ctx.release.token_secret) {
        Ok(t) if !t.trim().is_empty() => t.trim().to_string(),
        Ok(_) => return fail(step, "token da release vazio"),
        Err(e) => return fail(step, &e),
    };
    // O `gh` não é usado: o token fixo vence e nunca cai para o `gh auth`.
    let client = GhClient::new(GhConfig::new(&ctx.api_url, "gh").with_token(token));
    let rel = Release::new(&client, &ctx.owner, &ctx.repo, &ctx.release).with_pace(pace());
    let outcome = match cmd {
        ReleaseCmd::Status => match rel.status() {
            Ok(st) => {
                print_json(&serde_json::to_value(&st).expect("ReleaseStatus serializa"));
                let o = StepOutcome::new(
                    step,
                    StepResult::Ok,
                    format!("próxima etapa: {}", st.next.as_str()),
                )
                .with_sha(st.target_sha.clone())
                .with_details(json!({
                    "next": st.next,
                    "content_diff": st.content_diff,
                    "target_ahead_by": st.target_ahead_by,
                    "promotion_pr": st.promotion_pr.as_ref().map(|p| p.number),
                    "release_pr": st.release_pr.as_ref().map(|p| p.number),
                    "back_merge_pr": st.back_merge_pr.as_ref().map(|p| p.number),
                    "pending_runs": st.pending_runs.len(),
                }));
                record(&daemon, &o);
                return 0;
            }
            Err(e) => StepOutcome::new(step, StepResult::Failed, format!("erro do GitHub: {e}")),
        },
        ReleaseCmd::Promote { body_file, title } => {
            let body = match body_file {
                Some(p) => match std::fs::read_to_string(&p) {
                    Ok(b) => b,
                    Err(e) => return fail(step, &format!("{}: {e}", p.display())),
                },
                None => String::new(),
            };
            rel.promote(title.as_deref(), &body)
        }
        ReleaseCmd::MergePromotion { sha } => rel.merge_promotion(sha.as_deref()),
        ReleaseCmd::WaitReleasePr => rel.wait_release_pr(),
        ReleaseCmd::ApproveRuns { pr } => rel.approve_runs(pr),
        ReleaseCmd::MergeRelease { sha } => rel.merge_release(sha.as_deref()),
        ReleaseCmd::WaitPublish => rel.wait_publish(),
        ReleaseCmd::MergeBack { sha } => rel.merge_back(sha.as_deref()),
    };
    print_json(&serde_json::to_value(&outcome).expect("StepOutcome serializa"));
    eprintln!("ctower release {}: {}", step.as_str(), outcome.message);
    record(&daemon, &outcome);
    outcome.exit_code()
}

/// Grava o desfecho no ledger; a falha só avisa (a etapa já aconteceu).
fn record(daemon: &Daemon, o: &StepOutcome) {
    let payload = serde_json::to_value(o).expect("StepOutcome serializa");
    if let Err(e) = daemon.event(&payload) {
        eprintln!(
            "aviso: o evento da etapa {} não foi gravado no ledger: {e}",
            o.step.as_str()
        );
    }
}

fn fail(step: ReleaseStep, msg: &str) -> i32 {
    eprintln!("ctower release {}: {msg}", step.as_str());
    1
}

fn print_json(v: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(v).expect("JSON serializa")
    );
}
