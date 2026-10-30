//! Remote Control do Claude Code nas sessões (issue #197, spec do épico
//! #205 §6). Pelo celular, quem conversa com a sessão é o Remote Control,
//! no app Claude ou em `claude.ai/code/<id>`.
//!
//! - **ao iniciar:** `--remote-control "<nome>"` no argv, depois de
//!   `--settings` ([`AgentOptions::remote_control`]). Na retomada, a flag só
//!   entra se a sessão ainda não tinha `rc_session_id`: com o id, o
//!   `--resume` reconecta sozinho, e a sessão nova herda o id;
//! - **em andamento:** [`Sessions::enable_remote_control`] escreve
//!   `/remote-control <nome>` no PTY;
//! - **id:** o hook manda o `CLAUDE_CODE_BRIDGE_SESSION_ID` no envelope
//!   (vence sempre); de reserva, o daemon procura o link
//!   `https://claude.ai/code/session_…` na saída do PTY nos 30 s seguintes e
//!   grava o id se o hook ainda não o trouxe;
//! - **confirmação:** se nesse prazo aparecer uma pergunta com
//!   [`CONFIRM_PROMPT`], o daemon responde `y` uma única vez (evento
//!   `remote_control_confirmed`). O texto exato não é conhecido: não há,
//!   como no `trust.rs`, uma chave para pré-aceitar;
//! - **requisitos:** o Remote Control exige o login OAuth do claude.ai e não
//!   funciona com API key, `ANTHROPIC_BASE_URL` de outro host,
//!   `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` nem `DISABLE_GROWTHBOOK`. O
//!   ambiente efetivo da sessão (o do processo do `ctowerd` mais os secrets
//!   e variáveis do hub resolvidos para o projeto) é conferido antes do
//!   spawn e do `/remote-control`; a recusa cita o nome e a origem, nunca o
//!   valor. O login não é conferido de antemão.
//!
//! [`AgentOptions::remote_control`]: crate::AgentOptions::remote_control

use crate::{Result, Sessions, SessionsError};
use ct_core::ProjectKind;
use ct_ledger::{NewEvent, Project, Session, WorkItem};
use ct_pty::PtySession;
use ct_secrets::{is_reserved_name, Kind, Scope};
use serde::Serialize;
use serde_json::json;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

/// Trecho da pergunta de confirmação do Claude Code na primeira vez.
pub const CONFIRM_PROMPT: &str = "Enable Remote Control?";

/// Resposta à confirmação, escrita no PTY.
const CONFIRM_ANSWER: &[u8] = b"y\r";

/// Prazo para a confirmação e para o link aparecerem no PTY.
pub const WATCH_WINDOW: Duration = Duration::from_secs(30);

/// Tamanho máximo do nome da sessão remota, em caracteres.
pub const NAME_MAX: usize = 60;

/// Início do link da sessão remota na saída do Claude Code.
const LINK_PREFIX: &str = "https://claude.ai/code/";

/// Host aceito em `ANTHROPIC_BASE_URL`.
const ANTHROPIC_HOST: &str = "api.anthropic.com";

/// Pausa entre o texto do `/remote-control` e o Enter, para o Claude Code não
/// tratar os dois como uma colagem.
const TYPE_PAUSE: Duration = Duration::from_millis(150);

/// Disponibilidade do Remote Control no ambiente (`env.info`), com o
/// ambiente do processo e os secrets globais do hub. Secrets por projeto só
/// aparecem na recusa ao iniciar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RemoteControlStatus {
    pub available: bool,
    /// Por que não está disponível (nome da variável e origem, nunca o valor).
    pub reason: Option<String>,
}

/// Resposta de `session.remote_control.enable`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RemoteControlEnable {
    /// `true` = `/remote-control` escrito no PTY; a UI espera o
    /// `rc_session_id` (push `session_remote_control`) por até 30 s.
    /// `false` = a sessão já tinha o id, nada foi escrito.
    pub requested: bool,
    pub rc_session_id: Option<String>,
}

/// Nome da sessão remota: `<repo>#<n> <título>` na sessão de issue (no
/// projeto Jira, `<CHAVE>-<n> <título>`, #353), `<repo> planejamento` na do
/// projeto e `gestor` na do gestor; sem quebras
/// de linha nem aspas, até [`NAME_MAX`] caracteres (com `…`).
pub fn session_name(project: &Project, wi: &WorkItem, title: Option<&str>) -> String {
    let raw = if project.kind == ProjectKind::Gestor {
        "gestor".to_string()
    } else if wi.gh_number == crate::PROJECT_WORK_ITEM {
        format!("{} planejamento", project.gh_repo)
    } else {
        let item = match project.tracker {
            ct_core::TrackerConfig::Jira(_) => project.item_ref(wi.gh_number),
            ct_core::TrackerConfig::Github => format!("{}#{}", project.gh_repo, wi.gh_number),
        };
        match title.map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => format!("{item} {t}"),
            None => item,
        }
    };
    let clean: String = raw
        .chars()
        .filter(|c| !matches!(c, '"' | '\'' | '`'))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= NAME_MAX {
        return clean;
    }
    let mut cut: String = clean.chars().take(NAME_MAX - 1).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

/// Variável do ambiente efetivo: nome, valor e de onde veio.
struct EnvVar {
    name: String,
    value: String,
    origin: String,
}

/// Motivo da recusa, se o ambiente efetivo impede o Remote Control. Vale a
/// última ocorrência de cada nome (os secrets sobrepõem o processo).
fn blocker(vars: &[EnvVar]) -> Option<String> {
    let last = |name: &str| vars.iter().rev().find(|v| v.name == name);
    let set = |name: &str| last(name).filter(|v| !v.value.trim().is_empty());
    let why = |v: &EnvVar, what: &str| {
        Some(format!(
            "Remote Control indisponível: {} {what} ({})",
            v.name, v.origin
        ))
    };
    if let Some(v) = set("ANTHROPIC_API_KEY") {
        return why(v, "definida: o Remote Control exige o login do claude.ai");
    }
    if let Some(v) = set("ANTHROPIC_BASE_URL") {
        if url_host(&v.value).as_deref() != Some(ANTHROPIC_HOST) {
            return why(v, "aponta para outro host");
        }
    }
    for name in [
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
        "DISABLE_GROWTHBOOK",
    ] {
        if let Some(v) = set(name) {
            return why(v, "definida");
        }
    }
    None
}

/// Host de uma URL, em minúsculas (sem esquema, credenciais, porta e caminho).
fn url_host(url: &str) -> Option<String> {
    let rest = url.trim();
    let rest = rest.split_once("://").map_or(rest, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = host.split(':').next()?.trim_end_matches('.');
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Tira as sequências CSI (`ESC [ … final`) e OSC (`ESC ] … BEL|ESC \`) da
/// saída do terminal, para procurar texto. O alvo dos links OSC 8 fica.
fn strip_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match it.peek() {
            Some('[') => {
                it.next();
                for c in it.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                // OSC 8: `ESC ] 8 ; ; <url> ST`. Mantém a URL, com um espaço
                // antes e depois para separá-la do texto vizinho.
                it.next();
                let mut body = String::new();
                while let Some(c) = it.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        if it.peek() == Some(&'\\') {
                            it.next();
                        }
                        break;
                    }
                    body.push(c);
                }
                if let Some(url) = body.strip_prefix("8;").and_then(|b| b.split_once(';')) {
                    if !url.1.is_empty() {
                        out.push(' ');
                        out.push_str(url.1);
                        out.push(' ');
                    }
                }
            }
            _ => {
                it.next();
            }
        }
    }
    out
}

/// Id `session_…` do primeiro link `https://claude.ai/code/session_…` no texto.
pub fn rc_id_in(text: &str) -> Option<String> {
    let mut from = 0;
    while let Some(i) = text[from..].find(LINK_PREFIX) {
        let start = from + i + LINK_PREFIX.len();
        let id: String = text[start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if id.len() > "session_".len() && id.starts_with("session_") {
            return Some(id);
        }
        from = start;
    }
    None
}

/// Origem de uma entrada do cache de secrets, para a mensagem de recusa.
fn origin_of(scope: &Scope, kind: Kind) -> String {
    let what = match kind {
        Kind::Secret => "secret",
        Kind::Variable => "variável",
    };
    match scope {
        Scope::Global => format!("{what} global do hub"),
        Scope::Project(p) => format!("{what} do projeto {p}"),
    }
}

impl Sessions {
    /// Ambiente efetivo de uma sessão: o do processo do `ctowerd` e, por
    /// cima, os secrets e variáveis do hub resolvidos para o projeto (só os
    /// globais com `None` ou no gestor), sem os nomes reservados. Cache
    /// ausente ou ilegível: só o processo (é o que a sessão receberia).
    fn effective_env(&self, project: Option<&Project>) -> Vec<EnvVar> {
        let process = match &self.config().process_env {
            Some(env) => env.clone(),
            None => std::env::vars().collect(),
        };
        let mut vars: Vec<EnvVar> = process
            .into_iter()
            .map(|(name, value)| EnvVar {
                name,
                value,
                origin: "ambiente do ctowerd".into(),
            })
            .collect();
        let Ok(Some(secrets)) = self.env_cache().load() else {
            return vars;
        };
        let key = project.and_then(crate::secrets::scope_of);
        for (name, e) in secrets.resolve(key.as_deref()) {
            if is_reserved_name(&name) {
                continue;
            }
            vars.push(EnvVar {
                origin: origin_of(&e.scope, e.kind),
                value: e.value.expose().to_string(),
                name,
            });
        }
        vars
    }

    /// Disponibilidade do Remote Control para `env.info`: o ambiente do
    /// processo e os secrets globais do hub.
    pub fn remote_control_status(&self) -> RemoteControlStatus {
        let reason = blocker(&self.effective_env(None));
        RemoteControlStatus {
            available: reason.is_none(),
            reason,
        }
    }

    /// Recusa ([`SessionsError::RemoteControl`]) se o ambiente efetivo da
    /// sessão no projeto impede o Remote Control.
    pub(crate) fn check_remote_control(&self, project: &Project) -> Result<()> {
        match blocker(&self.effective_env(Some(project))) {
            Some(reason) => Err(SessionsError::RemoteControl(reason)),
            None => Ok(()),
        }
    }

    /// Nome da sessão remota de um work item; o título vem do espelho do
    /// GitHub quando quem chama não o tem.
    pub(crate) fn remote_name(
        &self,
        project: &Project,
        wi: &WorkItem,
        title: Option<&str>,
    ) -> String {
        let cached = match title {
            Some(_) => None,
            None => self
                .ledger()
                .get_issue(project.id, wi.gh_number)
                .ok()
                .flatten()
                .map(|i| i.title),
        };
        session_name(project, wi, title.or(cached.as_deref()))
    }

    /// Liga o Remote Control numa sessão viva deste daemon: confere os
    /// requisitos e escreve `/remote-control <nome>` no PTY. Sessão que já
    /// tem `rc_session_id` responde sem escrever nada.
    pub fn enable_remote_control(&self, session_id: &str) -> Result<RemoteControlEnable> {
        let s = self.ledger().get_session(session_id)?;
        let pty = match self.pty(session_id) {
            Some(p) if s.state.is_live() => p,
            _ => {
                return Err(SessionsError::Invalid(format!(
                    "sessão {session_id} não está rodando neste daemon"
                )))
            }
        };
        if s.rc_session_id.is_some() {
            return Ok(RemoteControlEnable {
                requested: false,
                rc_session_id: s.rc_session_id,
            });
        }
        let (project, wi) = {
            let l = self.ledger();
            let wi = l.get_work_item(s.work_item_id)?;
            (l.get_project(wi.project_id)?, wi)
        };
        self.check_remote_control(&project)?;
        let name = self.remote_name(&project, &wi, None);
        // Assina a saída antes de escrever: a confirmação e o link vêm depois.
        self.watch_remote_control(&s.id, &pty, false);
        pty.write(format!("/remote-control {name}").as_bytes())?;
        std::thread::sleep(TYPE_PAUSE);
        pty.write(b"\r")?;
        self.rc_event(
            &s,
            "remote_control_requested",
            json!({ "name": name, "via": "command" }),
        )?;
        Ok(RemoteControlEnable {
            requested: true,
            rc_session_id: None,
        })
    }

    /// Acompanha a saída do PTY por [`WATCH_WINDOW`]: responde à confirmação
    /// uma vez e grava o id do link se o hook ainda não o trouxe. Com
    /// `snapshot`, olha também o que já estava no scrollback (logo depois do
    /// spawn).
    pub(crate) fn watch_remote_control(&self, session_id: &str, pty: &PtySession, snapshot: bool) {
        let (snap, rx) = pty.attach();
        let (this, sid, pty) = (self.clone(), session_id.to_string(), pty.clone());
        std::thread::spawn(move || {
            let deadline = Instant::now() + WATCH_WINDOW;
            let mut raw = if snapshot { snap } else { Vec::new() };
            let mut confirmed = false;
            loop {
                let text = strip_escapes(&String::from_utf8_lossy(&raw));
                if !confirmed && text.contains(CONFIRM_PROMPT) {
                    confirmed = true;
                    if pty.write(CONFIRM_ANSWER).is_ok() {
                        this.rc_confirmed(&sid);
                    }
                }
                if let Some(id) = rc_id_in(&text) {
                    this.rc_id_from_pty(&sid, &id);
                    return;
                }
                let left = deadline.saturating_duration_since(Instant::now());
                match rx.recv_timeout(left) {
                    Ok(bytes) => raw.extend_from_slice(&bytes),
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return,
                }
                // Só o fim interessa: a pergunta e o link cabem folgados.
                const KEEP: usize = 16 * 1024;
                if raw.len() > 2 * KEEP {
                    raw.drain(..raw.len() - KEEP);
                }
            }
        });
    }

    /// Id do Remote Control relatado pelo hook (`CLAUDE_CODE_BRIDGE_SESSION_ID`):
    /// grava quando mudou.
    pub(crate) fn rc_id_from_hook(&self, s: &Session, rc_session_id: &str) -> Result<()> {
        self.record_rc_id(s, rc_session_id, "hook")
    }

    /// Id visto no link do PTY: só vale se o hook ainda não gravou um.
    fn rc_id_from_pty(&self, session_id: &str, rc_session_id: &str) {
        let Ok(s) = self.ledger().get_session(session_id) else {
            return;
        };
        if s.rc_session_id.is_some() {
            return;
        }
        if let Err(e) = self.record_rc_id(&s, rc_session_id, "pty") {
            tracing::warn!("remote control: falha ao gravar o id da sessão {session_id}: {e}");
        }
    }

    fn record_rc_id(&self, s: &Session, rc_session_id: &str, source: &str) -> Result<()> {
        let id = rc_session_id.trim();
        if id.is_empty() || !self.ledger().set_session_rc_id(&s.id, id)? {
            return Ok(());
        }
        self.rc_event(
            s,
            "remote_control_connected",
            json!({ "rc_session_id": id, "source": source }),
        )?;
        self.notify(
            "session_remote_control",
            json!({ "session_id": s.id, "work_item_id": s.work_item_id, "rc_session_id": id }),
        );
        Ok(())
    }

    fn rc_confirmed(&self, session_id: &str) {
        let Ok(s) = self.ledger().get_session(session_id) else {
            return;
        };
        if let Err(e) = self.rc_event(&s, "remote_control_confirmed", json!({})) {
            tracing::warn!(
                "remote control: falha ao registrar a confirmação da sessão {session_id}: {e}"
            );
        }
    }

    fn rc_event(&self, s: &Session, kind: &str, payload: serde_json::Value) -> Result<()> {
        let l = self.ledger();
        let wi = l.get_work_item(s.work_item_id)?;
        l.append_event(NewEvent {
            project_id: wi.project_id,
            work_item_id: Some(wi.id),
            session_id: Some(&s.id),
            kind,
            payload,
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use crate::AgentOptions;
    use ct_core::{SessionState, WorkItemKind};
    use serde_json::Value;

    fn var(name: &str, value: &str, origin: &str) -> EnvVar {
        EnvVar {
            name: name.into(),
            value: value.into(),
            origin: origin.into(),
        }
    }

    #[test]
    fn nome_por_tipo_de_sessao_sem_aspas_e_truncado() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let (p, issue, plan) = {
            let l = env.sessions.ledger();
            let p = l.get_project(env.project_id).unwrap();
            let issue = l.ensure_work_item(p.id, 42, WorkItemKind::Issue).unwrap();
            let plan = l.ensure_work_item(p.id, 0, WorkItemKind::Issue).unwrap();
            (p, issue, plan)
        };
        assert_eq!(
            session_name(&p, &issue, Some("Login \"web\"\ne 'cookie'")),
            "demo#42 Login web e cookie"
        );
        assert_eq!(session_name(&p, &issue, None), "demo#42");
        assert_eq!(
            session_name(&p, &plan, Some("ignorado")),
            "demo planejamento"
        );
        let long = session_name(&p, &issue, Some(&"á".repeat(100)));
        assert_eq!(long.chars().count(), NAME_MAX);
        assert!(long.ends_with('…'), "{long}");
        let mut g = p.clone();
        g.kind = ProjectKind::Gestor;
        assert_eq!(session_name(&g, &plan, None), "gestor");
        // Projeto Jira (#353): a chave da issue no lugar de `<repo>#<n>`.
        let mut j = p.clone();
        j.tracker = ct_core::TrackerConfig::Jira(ct_core::JiraConfig {
            project_key: "PROJ".into(),
            ..Default::default()
        });
        assert_eq!(session_name(&j, &issue, Some("Login")), "PROJ-42 Login");
        assert_eq!(session_name(&j, &plan, None), "demo planejamento");
    }

    #[test]
    fn recusa_cita_nome_e_origem_sem_valor() {
        let d = "ambiente do ctowerd";
        assert_eq!(blocker(&[]), None);
        assert_eq!(blocker(&[var("ANTHROPIC_API_KEY", "  ", d)]), None);
        let r = blocker(&[var(
            "ANTHROPIC_API_KEY",
            "sk-ant-x",
            "secret do projeto o/r",
        )])
        .unwrap();
        assert!(
            r.contains("ANTHROPIC_API_KEY") && r.contains("secret do projeto o/r"),
            "{r}"
        );
        assert!(!r.contains("sk-ant-x"), "{r}");
        for ok in [
            "https://api.anthropic.com",
            "https://API.anthropic.com:443/v1/",
            "api.anthropic.com",
        ] {
            assert_eq!(blocker(&[var("ANTHROPIC_BASE_URL", ok, d)]), None, "{ok}");
        }
        for bad in [
            "https://proxy.local:8080",
            "http://user@outro.com/x",
            "https://api.anthropic.com.evil.io",
        ] {
            let r = blocker(&[var("ANTHROPIC_BASE_URL", bad, d)]).unwrap();
            assert!(r.contains("ANTHROPIC_BASE_URL") && !r.contains(bad), "{r}");
        }
        for n in [
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
            "DISABLE_GROWTHBOOK",
        ] {
            assert!(blocker(&[var(n, "1", d)]).unwrap().contains(n));
        }
        // O secret sobrepõe o processo: vale o último.
        assert_eq!(
            blocker(&[
                var("ANTHROPIC_API_KEY", "x", d),
                var("ANTHROPIC_API_KEY", "", "secret global do hub")
            ]),
            None
        );
    }

    #[test]
    fn id_do_link_na_saida_com_escapes() {
        assert_eq!(
            rc_id_in("Continue at https://claude.ai/code/session_01WfAb9 agora"),
            Some("session_01WfAb9".into())
        );
        assert_eq!(
            rc_id_in("https://claude.ai/code/outra https://claude.ai/code/session_"),
            None
        );
        let osc = "\u{1b}]8;;https://claude.ai/code/session_X1\u{1b}\\link\u{1b}]8;;\u{1b}\\";
        assert_eq!(rc_id_in(&strip_escapes(osc)), Some("session_X1".into()));
        let csi = "\u{1b}[1mEnable\u{1b}[0m Remote Control?\u{1b}[2K";
        assert_eq!(strip_escapes(csi), "Enable Remote Control?");
    }

    fn outbox_has_rc(env: &TestEnv, id: &str) -> bool {
        env.sessions
            .ledger()
            .outbox_pending(10_000)
            .unwrap()
            .iter()
            .any(|e| matches!(&e.change, ct_ledger::SyncChange::Session(c) if c.rc_session_id.as_deref() == Some(id)))
    }

    fn rc_request(env: &TestEnv, n: i64, title: &str) -> crate::StartRequest {
        let mut req = env.request(n, title);
        req.agent = AgentOptions {
            remote_control: true,
            ..AgentOptions::default()
        };
        req
    }

    fn wait_rc(env: &TestEnv, id: &str) -> Session {
        let deadline = Instant::now() + T;
        loop {
            let s = env.sessions.ledger().get_session(id).unwrap();
            if s.rc_session_id.is_some() {
                return s;
            }
            assert!(Instant::now() < deadline, "rc_session_id não chegou");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn kinds(env: &TestEnv, s: &Session) -> Vec<(String, Value)> {
        env.sessions
            .ledger()
            .list_events(s.work_item_id, 200)
            .unwrap()
            .into_iter()
            .map(|e| (e.kind, e.payload))
            .collect()
    }

    /// Agente falso que pergunta "Enable Remote Control?", grava a resposta
    /// e imprime o link: a flag e o nome vão no argv, a pergunta é
    /// respondida uma vez e o id do link é gravado (#197).
    #[test]
    fn inicio_com_remote_control_poe_a_flag_responde_a_confirmacao_e_grava_o_id() {
        let env = TestEnv::new(FakeClaude::script(
            "printf 'Enable Remote Control? (y/n) '; read ans; printf '%s' \"$ans\" > \"$out.ans\"; \
             echo 'Continue at https://claude.ai/code/session_01Pty'; sleep 30",
        ));
        let s = env
            .sessions
            .start(rc_request(&env, 12, "Login \"web\""))
            .unwrap();
        let out = env.wait_fake_output(&s.id);
        let dir = env.cfg.dirs.session_dir(&s.id);
        let tail = [
            "--settings".to_string(),
            dir.join("settings.json").to_string_lossy().into(),
            "--remote-control".into(),
            "demo#12 Login web".into(),
            "prompt inicial".into(),
        ];
        assert!(out.contains(&tail.join("\n")), "{out}");
        assert!(s.launch_cmd.ends_with(&tail), "{:?}", s.launch_cmd);

        let got = wait_rc(&env, &s.id);
        assert_eq!(got.rc_session_id.as_deref(), Some("session_01Pty"));
        let ans = env.fake_output_path(&s.id).with_extension("txt.ans");
        assert_eq!(std::fs::read_to_string(ans).unwrap(), "y");
        let evs = kinds(&env, &s);
        assert_eq!(
            evs.iter()
                .filter(|(k, _)| k == "remote_control_confirmed")
                .count(),
            1
        );
        assert!(evs
            .iter()
            .any(|(k, p)| k == "remote_control_connected" && p["source"] == "pty"));
        assert!(env
            .notices()
            .iter()
            .any(|(n, d)| n == "session_remote_control" && d["rc_session_id"] == "session_01Pty"));
        assert!(outbox_has_rc(&env, "session_01Pty"));
        env.sessions.kill(&s.id).ok();
    }

    #[test]
    fn sem_a_opcao_nao_ha_flag() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let s = env.sessions.start(env.request(13, "Sem")).unwrap();
        assert!(!s.launch_cmd.iter().any(|a| a == "--remote-control"));
        let p = env
            .sessions
            .start_project(
                env.project_id,
                "planejar",
                None,
                &AgentOptions {
                    remote_control: true,
                    ..AgentOptions::default()
                },
            )
            .unwrap();
        let i = p
            .launch_cmd
            .iter()
            .position(|a| a == "--remote-control")
            .unwrap();
        assert_eq!(p.launch_cmd[i + 1], "demo planejamento");
    }

    /// `session.remote_control.enable`: `/remote-control <nome>` chega ao
    /// PTY; com o id já gravado, nada é escrito.
    #[test]
    fn ligar_em_andamento_escreve_o_comando_no_pty() {
        let env = TestEnv::new(FakeClaude::script(
            "read cmd; printf '%s' \"$cmd\" > \"$out.cmd\"; \
             echo '/remote-control is active · https://claude.ai/code/session_01Cmd'; sleep 30",
        ));
        let s = env.sessions.start(env.request(14, "Em andamento")).unwrap();
        env.wait_fake_output(&s.id);
        let r = env.sessions.enable_remote_control(&s.id).unwrap();
        assert_eq!(
            r,
            RemoteControlEnable {
                requested: true,
                rc_session_id: None
            }
        );
        let cmd = env.fake_output_path(&s.id).with_extension("txt.cmd");
        let deadline = Instant::now() + T;
        // O `>` cria o arquivo antes do `printf` escrever: espera o conteúdo.
        while std::fs::read_to_string(&cmd).map_or(true, |c| c.is_empty()) {
            assert!(Instant::now() < deadline, "comando não chegou ao PTY");
            std::thread::sleep(Duration::from_millis(20));
        }
        // Issue fora do espelho do GitHub: o nome fica sem o título.
        assert_eq!(
            std::fs::read_to_string(&cmd).unwrap(),
            "/remote-control demo#14"
        );
        assert_eq!(
            wait_rc(&env, &s.id).rc_session_id.as_deref(),
            Some("session_01Cmd")
        );
        let r = env.sessions.enable_remote_control(&s.id).unwrap();
        assert_eq!(
            r,
            RemoteControlEnable {
                requested: false,
                rc_session_id: Some("session_01Cmd".into())
            }
        );
        env.sessions.kill(&s.id).ok();
        env.wait_state(&s.id, SessionState::Exited);
        assert!(matches!(
            env.sessions.enable_remote_control(&s.id),
            Err(SessionsError::Invalid(_))
        ));
    }

    /// `ANTHROPIC_API_KEY` num secret do projeto recusa o início com o
    /// Remote Control, sem criar sessão nem worktree, e sem o valor na
    /// mensagem; a recusa não vale sem a opção nem para outro projeto.
    #[test]
    fn recusa_com_api_key_de_secret_do_projeto() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        env.write_secrets_cache(&[(
            Some("deep-ion-ai/demo"),
            "ANTHROPIC_API_KEY",
            "sk-ant-segredo",
        )]);
        // O `env.info` só olha os globais.
        assert!(env.sessions.remote_control_status().available);
        let err = env
            .sessions
            .start(rc_request(&env, 15, "Recusa"))
            .unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, SessionsError::RemoteControl(_)), "{err}");
        assert!(
            msg.contains("ANTHROPIC_API_KEY") && msg.contains("secret do projeto deep-ion-ai/demo"),
            "{msg}"
        );
        assert!(!msg.contains("sk-ant-segredo"), "{msg}");
        assert!(
            !env.cfg.worktrees_root.join("demo").exists(),
            "nenhum worktree"
        );
        assert!(env
            .sessions
            .ledger()
            .find_work_item(env.project_id, 15)
            .unwrap()
            .is_none());
        assert!(matches!(
            env.sessions.start_project(
                env.project_id,
                "x",
                None,
                &AgentOptions {
                    remote_control: true,
                    ..AgentOptions::default()
                }
            ),
            Err(SessionsError::RemoteControl(_))
        ));
        // Ligar depois numa sessão do mesmo projeto também é recusado.
        let s = env.sessions.start(env.request(16, "Sem RC")).unwrap();
        assert!(matches!(
            env.sessions.enable_remote_control(&s.id),
            Err(SessionsError::RemoteControl(_))
        ));
        env.sessions.kill(&s.id).ok();
    }

    #[test]
    fn disponibilidade_no_env_info_com_processo_e_globais() {
        let mut env = TestEnv::new(FakeClaude::exit(0));
        assert_eq!(
            env.sessions.remote_control_status(),
            RemoteControlStatus {
                available: true,
                reason: None
            }
        );
        env.write_secrets_cache_kinds(&[(None, "DISABLE_GROWTHBOOK", Kind::Variable, "1")]);
        let st = env.sessions.remote_control_status();
        assert!(!st.available);
        assert!(st
            .reason
            .unwrap()
            .contains("DISABLE_GROWTHBOOK definida (variável global do hub)"));

        env.cfg.process_env = Some(vec![(
            "ANTHROPIC_BASE_URL".into(),
            "https://gw.local".into(),
        )]);
        let bin = env.cfg.claude_bin.clone();
        env.set_claude_bin(&bin);
        let st = env.sessions.remote_control_status();
        assert!(st
            .reason
            .unwrap()
            .contains("ANTHROPIC_BASE_URL aponta para outro host (ambiente do ctowerd)"));
    }
}
