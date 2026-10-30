//! Boot do daemon, tela de recuperação, retomada e arquivamento (spec §6, §7.4).

use crate::launch::{AgentOptions, LaunchMode, LaunchSpec, PtySize};
use crate::transcripts::{integrity, TranscriptIntegrity};
use crate::{Result, Sessions, SessionsError};
use ct_core::{now_ms, SessionState};
use ct_ledger::{Event, Ledger, NewEvent, Session, WorkItem};
use serde::Serialize;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::process::Command;

/// Prompt sugerido ao retomar (editável pela UI).
pub const RESUME_PROMPT: &str = "A sessão foi interrompida antes de terminar. \
Verifique o estado do worktree e dos commits antes de continuar.";

/// Linhas de `git status` mostradas na recuperação.
const STATUS_LINES: usize = 20;

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RecoveryItem {
    pub session: Session,
    pub work_item: WorkItem,
    pub project_name: String,
    pub last_event: Option<Event>,
    /// Há quanto tempo (ms) chegou o último sinal da sessão.
    pub stopped_for_ms: i64,
    pub transcript: TranscriptIntegrity,
    /// `git status --short --branch` resumido; `None` se o worktree não existe.
    pub worktree_status: Option<String>,
}

/// Sessão descartada da Recuperação (`recovery.archived`).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ArchivedItem {
    pub session: Session,
    pub work_item: WorkItem,
    pub project_name: String,
    pub archived_at: i64,
    pub transcript: TranscriptIntegrity,
}

/// Sessões oferecidas na Recuperação: as `interrupted`, menos as de work
/// item concluído ([`crate::cleanup::completion`]), que não há o que retomar
/// (#285). As linhas ficam no ledger: a etapa "sessões" da limpeza as
/// arquiva e a poda do #179 as apaga depois da retenção. Lê só o ledger e o
/// espelho do GitHub.
pub fn recoverable_sessions(l: &Ledger) -> Result<Vec<Session>> {
    let mut done: HashMap<i64, bool> = HashMap::new();
    let mut out = Vec::new();
    for s in l.sessions_in_state(SessionState::Interrupted)? {
        let concluded = match done.get(&s.work_item_id) {
            Some(c) => *c,
            None => {
                let c = work_item_concluded(l, s.work_item_id)?;
                done.insert(s.work_item_id, c);
                c
            }
        };
        if !concluded {
            out.push(s);
        }
    }
    Ok(out)
}

/// O work item já concluiu ([`crate::cleanup::completion`])? É o que tira a
/// sessão `interrupted` da Recuperação, e o que a aba do terminal consulta
/// (`session.get`) para não oferecê-la (#297).
pub fn work_item_concluded(l: &Ledger, work_item_id: i64) -> Result<bool> {
    let wi = l.get_work_item(work_item_id)?;
    let project = l.get_project(wi.project_id)?;
    Ok(crate::cleanup::completion(l, &project, &wi)?.is_some())
}

impl Sessions {
    /// Boot do daemon: sessões vivas sem PTY neste daemon → `interrupted`;
    /// depois aplica o spool. Retorna os ids interrompidos.
    ///
    /// O PTY só existe na memória do daemon que lançou a sessão: depois de um
    /// restart, ela é inalcançável mesmo que o processo ainda exista (morrendo
    /// pelo SIGHUP do PTY fechado, #283). O `SessionEnd` tardio desse processo
    /// é ignorado (`interrupted` não vai para `exited`).
    pub fn boot(&self) -> Result<Vec<String>> {
        let owned: HashSet<i64> = self
            .inner
            .live
            .lock()
            .unwrap()
            .values()
            .filter_map(|p| p.pid().map(i64::from))
            .collect();
        let ids = self
            .ledger()
            .mark_interrupted(&|pid| owned.contains(&pid))?;
        self.drain_spool()?;
        if !ids.is_empty() {
            self.notify("sessions_interrupted", json!({ "session_ids": ids }));
        }
        Ok(ids)
    }

    pub fn recovery_list(&self) -> Result<Vec<RecoveryItem>> {
        let sessions = recoverable_sessions(&self.ledger())?;
        let now = now_ms();
        let mut items = Vec::with_capacity(sessions.len());
        for s in sessions {
            let (work_item, project_name, last_event) = {
                let l = self.ledger();
                let wi = l.get_work_item(s.work_item_id)?;
                let name = l.get_project(wi.project_id)?.name;
                let last = l.list_events(wi.id, 1)?.into_iter().next();
                (wi, name, last)
            };
            items.push(RecoveryItem {
                stopped_for_ms: now - s.last_event_at,
                transcript: integrity(&s),
                worktree_status: worktree_status(Path::new(&s.cwd)),
                session: s,
                work_item,
                project_name,
                last_event,
            });
        }
        Ok(items)
    }

    /// Sessões descartadas da recuperação nos últimos `since_ms` (as
    /// retomadas ficam de fora), da mais recente para a mais antiga (#328).
    pub fn recovery_archived(&self, since_ms: i64, limit: u32) -> Result<Vec<ArchivedItem>> {
        let l = self.ledger();
        let mut items = Vec::new();
        for s in l.discarded_sessions_since(since_ms, limit)? {
            let wi = l.get_work_item(s.work_item_id)?;
            items.push(ArchivedItem {
                archived_at: s.last_event_at,
                transcript: integrity(&s),
                project_name: l.get_project(wi.project_id)?.name,
                work_item: wi,
                session: s,
            });
        }
        Ok(items)
    }

    /// Retoma uma sessão `interrupted`/`exited`: restaura o transcript do
    /// backup se preciso, lança `claude --resume` no mesmo cwd e arquiva a antiga.
    ///
    /// Remote Control (#197): a sessão nova herda o `rc_session_id` da antiga
    /// (o `--resume` reconecta sozinho na mesma conversa remota). Sem o id,
    /// `remote_control` acrescenta `--remote-control "<nome>"`.
    pub fn resume(
        &self,
        session_id: &str,
        prompt: Option<&str>,
        remote_control: bool,
    ) -> Result<Session> {
        let old = self.ledger().get_session(session_id)?;
        if !matches!(old.state, SessionState::Interrupted | SessionState::Exited) {
            return Err(SessionsError::Invalid(format!(
                "sessão {session_id} está em {}; só interrupted/exited podem ser retomadas",
                old.state
            )));
        }
        let Some(tool_id) = old.tool_session_id.clone() else {
            return Err(SessionsError::Invalid(format!(
                "sessão {session_id} não tem id da ferramenta para --resume"
            )));
        };
        if !Path::new(&old.cwd).is_dir() {
            return Err(SessionsError::Invalid(format!(
                "worktree {} não existe mais",
                old.cwd
            )));
        }
        let (project, wi) = {
            let l = self.ledger();
            let wi = l.get_work_item(old.work_item_id)?;
            (l.get_project(wi.project_id)?, wi)
        };
        let remote_control = remote_control && old.rc_session_id.is_none();
        if remote_control {
            self.check_remote_control(&project)?;
        }
        match integrity(&old) {
            TranscriptIntegrity::Original => {}
            TranscriptIntegrity::RestorableFromBackup => {
                let (Some(dst), Some(src)) = (&old.transcript_path, &old.backup_path) else {
                    unreachable!("integridade garante os dois caminhos");
                };
                if let Some(parent) = Path::new(dst).parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(src, dst)?;
                self.ledger().append_event(NewEvent {
                    project_id: project.id,
                    work_item_id: Some(wi.id),
                    session_id: Some(&old.id),
                    kind: "transcript_restored",
                    payload: json!({ "from": src, "to": dst }),
                })?;
            }
            TranscriptIntegrity::Lost => {
                return Err(SessionsError::Invalid(format!(
                    "transcript da sessão {session_id} foi perdido e não há backup; \
                     inicie uma nova sessão com um prompt de contexto"
                )));
            }
        }
        let remote_name = remote_control.then(|| self.remote_name(&project, &wi, None));
        let new = self.launch(
            &project,
            &wi,
            LaunchSpec {
                cwd: &old.cwd,
                mode: LaunchMode::Resume(&tool_id),
                prompt: prompt.unwrap_or(RESUME_PROMPT),
                size: PtySize::DEFAULT,
                // A retomada segue com o modelo e o modo da sessão original.
                agent: &AgentOptions {
                    model: old.model.clone(),
                    mode: old.mode,
                    remote_control,
                },
                remote_name,
                title: "",
            },
        )?;
        let new = match &old.rc_session_id {
            Some(rc) => {
                let l = self.ledger();
                l.set_session_rc_id(&new.id, rc)?;
                l.get_session(&new.id)?
            }
            None => new,
        };
        self.ledger().archive_session(&old.id, Some(&new.id))?;
        // A nova sessão tem o próprio conjunto (valores atuais do cache).
        self.forget_mask(&old.id);
        self.notify(
            "session_resumed",
            json!({ "old_session_id": old.id, "new_session_id": new.id }),
        );
        Ok(new)
    }

    /// Descarta uma sessão `interrupted`/`exited` da recuperação.
    pub fn archive(&self, session_id: &str) -> Result<()> {
        self.ledger().archive_session(session_id, None)?;
        self.forget_mask(session_id);
        self.notify("session_archived", json!({ "session_id": session_id }));
        Ok(())
    }
}

fn worktree_status(dir: &Path) -> Option<String> {
    if !dir.is_dir() {
        return None;
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["status", "--short", "--branch"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().collect();
    let mut s = lines
        .iter()
        .take(STATUS_LINES)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    if lines.len() > STATUS_LINES {
        s.push_str(&format!("\n… +{} linhas", lines.len() - STATUS_LINES));
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;

    /// Inicia e simula a queda: processo encerrado e sessão `interrupted`.
    fn interrupted(env: &TestEnv, transcript: Option<&Path>) -> Session {
        interrupted_with(env, transcript, AgentOptions::default())
    }

    fn interrupted_with(env: &TestEnv, transcript: Option<&Path>, agent: AgentOptions) -> Session {
        let mut req = env.request(1, "A");
        req.agent = agent;
        let s = env.sessions.start(req).unwrap();
        env.wait_fake_output(&s.id);
        if let Some(t) = transcript {
            env.sessions
                .ledger()
                .set_session_tool_info(
                    &s.id,
                    s.tool_session_id.as_deref().unwrap(),
                    &t.to_string_lossy(),
                )
                .unwrap();
            env.sessions.backup_session(&s.id).unwrap();
        }
        // O on_exit do processo falso marcaria `exited`; a queda é simulada
        // marcando `interrupted` antes, como faria o boot.
        env.sessions
            .ledger()
            .transition_session(&s.id, SessionState::Interrupted, None)
            .unwrap();
        env.sessions.ledger().get_session(&s.id).unwrap()
    }

    #[test]
    fn boot_interrompe_sem_processo_e_e_idempotente() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let viva = env.sessions.start(env.request(1, "A")).unwrap();
        let morta = env.sessions.start(env.request(2, "B")).unwrap();
        env.sessions
            .ledger()
            .set_session_pid(&morta.id, i64::from(i32::MAX))
            .unwrap();
        let ids = env.sessions.boot().unwrap();
        assert_eq!(ids, vec![morta.id.clone()]);
        assert!(env.sessions.boot().unwrap().is_empty());
        assert_eq!(
            env.sessions.ledger().get_session(&viva.id).unwrap().state,
            SessionState::Starting
        );
        assert!(env
            .notices()
            .iter()
            .any(|(n, _)| n == "sessions_interrupted"));
        env.sessions.kill(&viva.id).unwrap();
        env.sessions.kill(&morta.id).unwrap();
    }

    #[test]
    fn boot_interrompe_orfa_com_processo_vivo_e_ignora_session_end_tardio() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let orfa = env.sessions.start(env.request(1, "A")).unwrap();
        // Processo vivo, mas sem PTY neste daemon (o daemon anterior caiu, #283).
        env.sessions
            .ledger()
            .set_session_pid(&orfa.id, i64::from(std::process::id()))
            .unwrap();
        assert_eq!(env.sessions.boot().unwrap(), vec![orfa.id.clone()]);
        let fim = crate::hooks::HookEnvelope {
            session: orfa.id.clone(),
            event: "SessionEnd".into(),
            payload: json!({ "reason": "other" }),
            ts: 0,
            rc_session_id: None,
        };
        env.sessions.apply_hook(&fim).unwrap();
        assert_eq!(
            env.sessions.ledger().get_session(&orfa.id).unwrap().state,
            SessionState::Interrupted
        );
        assert_eq!(env.sessions.recovery_list().unwrap().len(), 1);
        env.sessions.kill(&orfa.id).unwrap();
    }

    #[test]
    fn recovery_list_traz_contexto() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let s = interrupted(&env, None);
        std::fs::write(Path::new(&s.cwd).join("novo.txt"), "x").unwrap();
        let items = env.sessions.recovery_list().unwrap();
        assert_eq!(items.len(), 1);
        let it = &items[0];
        assert_eq!(it.session.id, s.id);
        assert_eq!(it.project_name, "demo");
        assert_eq!(it.transcript, TranscriptIntegrity::Lost);
        let st = it.worktree_status.as_deref().unwrap();
        assert!(
            st.contains("## feat/1-a") && st.contains("?? novo.txt"),
            "{st}"
        );
        assert!(it.last_event.is_some());
        assert!(it.stopped_for_ms >= 0);
        env.sessions.kill(&s.id).ok();
    }

    #[test]
    fn retomar_com_transcript_original() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let t = env.tmp.path().join("t.jsonl");
        std::fs::write(&t, "conversa\n").unwrap();
        let old = interrupted(&env, Some(&t));
        let new = env.sessions.resume(&old.id, None, false).unwrap();
        let out = env.wait_fake_output(&new.id);
        let tool_id = old.tool_session_id.clone().unwrap();
        assert!(out.starts_with(&format!("--resume\n{tool_id}\n")), "{out}");
        assert!(out.contains(RESUME_PROMPT));
        assert_eq!(new.cwd, old.cwd);
        assert_eq!(new.tool_session_id.as_deref(), Some(tool_id.as_str()));
        let l = env.sessions.ledger();
        assert_eq!(
            l.get_session(&old.id).unwrap().state,
            SessionState::Archived
        );
        let evs = l.list_events(old.work_item_id, 50).unwrap();
        assert!(evs
            .iter()
            .any(|e| e.kind == "resumed_as" && e.payload["new_session_id"] == new.id.as_str()));
        drop(l);
        env.sessions.kill(&new.id).unwrap();
        env.sessions.kill(&old.id).ok();
    }

    /// O resume lê o cache na hora (#166): o secret gravado depois da
    /// sessão original chega à retomada; a original não mudou.
    #[test]
    fn retomar_injeta_os_secrets_do_cache_atual() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let t = env.tmp.path().join("t.jsonl");
        std::fs::write(&t, "conversa\n").unwrap();
        env.write_secrets_cache(&[(None, "T_TOKEN", "valor-antigo")]);
        let old = interrupted(&env, Some(&t));
        let antes = env.wait_fake_output(&old.id);
        assert!(antes.contains("T_TOKEN=valor-antigo\n"), "{antes}");
        env.write_secrets_cache(&[
            (None, "T_TOKEN", "valor-novo"),
            (Some("deep-ion-ai/demo"), "T_PROJETO", "do-projeto"),
            (None, "CTOWER_SESSION", "sessao-falsa"),
        ]);
        let new = env.sessions.resume(&old.id, None, false).unwrap();
        let out = env.wait_fake_output(&new.id);
        assert!(out.contains("T_TOKEN=valor-novo\n"), "{out}");
        assert!(out.contains("T_PROJETO=do-projeto\n"), "{out}");
        assert!(
            out.contains(&format!("CTOWER_SESSION={}\n", new.id)),
            "{out}"
        );
        assert!(!out.contains("sessao-falsa"), "{out}");
        assert_eq!(env.wait_fake_output(&old.id), antes);
        assert!(!new.launch_cmd.iter().any(|a| a.contains("valor-novo")));
        env.sessions.kill(&new.id).unwrap();
        env.sessions.kill(&old.id).ok();
    }

    #[test]
    fn retomar_o_gestor_reaplica_os_limites_na_mesma_pasta() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let old = env
            .sessions
            .start_gestor(
                &crate::GestorScope::Portfolio,
                "oi",
                None,
                &AgentOptions::default(),
            )
            .unwrap();
        env.wait_fake_output(&old.id);
        let t = env.tmp.path().join("g.jsonl");
        std::fs::write(&t, "conversa\n").unwrap();
        env.sessions
            .ledger()
            .set_session_tool_info(
                &old.id,
                old.tool_session_id.as_deref().unwrap(),
                &t.to_string_lossy(),
            )
            .unwrap();
        env.sessions.backup_session(&old.id).unwrap();
        env.sessions
            .ledger()
            .transition_session(&old.id, SessionState::Interrupted, None)
            .unwrap();
        let new = env.sessions.resume(&old.id, None, false).unwrap();
        assert_eq!(new.cwd, old.cwd);
        let out = env.wait_fake_output(&new.id);
        assert!(out.contains("--resume"), "{out}");
        assert!(out.contains(crate::GESTOR_LIMITS), "{out}");
        assert!(out.contains("CTOWER_SESSION_KIND=gestor"), "{out}");
        // A retomada reusa o id da conversa: o escopo continua ligado.
        let m = {
            let l = env.sessions.ledger();
            crate::gestor_scopes(&l, new.work_item_id, &l.live_sessions().unwrap()).unwrap()
        };
        assert_eq!(m.get(&new.id), Some(&crate::GestorScope::Portfolio));
        env.sessions.kill(&new.id).unwrap();
        env.sessions.kill(&old.id).ok();
    }

    /// A retomada recalcula a pasta de docs (#441): a sessão que subiu com
    /// a montagem caída recebe `CT_DOCS_DIR` e o `--add-dir` ao retomar com
    /// ela de volta.
    #[test]
    fn retomar_recalcula_a_pasta_de_docs() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        env.assign_config(1, |p| p.docs.enabled = true);
        let t = env.tmp.path().join("t.jsonl");
        std::fs::write(&t, "conversa\n").unwrap();
        let old = interrupted(&env, Some(&t));
        let antes = env.wait_fake_output(&old.id);
        assert!(!antes.contains("CT_DOCS_DIR"), "{antes}");
        let root = env.docs_root_up();
        let new = env.sessions.resume(&old.id, None, false).unwrap();
        let out = env.wait_fake_output(&new.id);
        let proj = root.join("deep-ion-ai__demo").display().to_string();
        assert!(out.contains(&format!("CT_DOCS_DIR={proj}\n")), "{out}");
        assert!(out.contains(&format!("--add-dir\n{proj}\n")), "{out}");
        env.sessions.kill(&new.id).unwrap();
        env.sessions.kill(&old.id).ok();
    }

    /// A retomada passa pelo mesmo `launch` e relê o cache: recebe a versão
    /// das regras vigente agora, não a do início (#357, spec §10.1).
    #[test]
    fn retomar_recebe_a_versao_atual_das_regras() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let regras = |version: i64, c: &str| ct_ledger::ConfigRules {
            project: Some(ct_ledger::RuleText {
                version,
                content: c.into(),
            }),
            epics: vec![],
        };
        let gh = (ct_core::TrackerConfig::Github, ct_core::ForgeConfig::Github);
        env.assign_with_rules(gh.0.clone(), gh.1.clone(), false, 1, regras(1, "antiga"));
        let t = env.tmp.path().join("t.jsonl");
        std::fs::write(&t, "conversa\n").unwrap();
        let old = interrupted_with(&env, Some(&t), AgentOptions::default());
        assert!(old
            .launch_cmd
            .contains(&"Regras do projeto demo (versão 1, Control Tower):\nantiga".to_string()));
        env.assign_with_rules(gh.0, gh.1, false, 2, regras(2, "nova"));
        let new = env.sessions.resume(&old.id, None, false).unwrap();
        let out = env.wait_fake_output(&new.id);
        let want =
            "--append-system-prompt\nRegras do projeto demo (versão 2, Control Tower):\nnova\n";
        assert!(out.contains(want), "{out}");
        assert!(!out.contains("antiga"), "{out}");
        env.sessions.kill(&new.id).unwrap();
        env.sessions.kill(&old.id).ok();
    }

    /// Remote Control na retomada (#197): sem id, a flag entra a pedido; com
    /// id, não entra e a sessão nova herda o id.
    #[test]
    fn retomar_com_remote_control() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let t = env.tmp.path().join("t.jsonl");
        std::fs::write(&t, "conversa\n").unwrap();
        let old = interrupted(&env, Some(&t));
        let new = env.sessions.resume(&old.id, None, true).unwrap();
        let i = new
            .launch_cmd
            .iter()
            .position(|a| a == "--remote-control")
            .unwrap();
        assert_eq!(new.launch_cmd[i + 1], "demo#1");
        assert_eq!(new.launch_cmd[1], "--resume");
        env.sessions.kill(&new.id).unwrap();
        env.wait_state(&new.id, SessionState::Exited);

        {
            let l = env.sessions.ledger();
            let tool = new.tool_session_id.clone().unwrap();
            l.set_session_tool_info(&new.id, &tool, &t.to_string_lossy())
                .unwrap();
            l.set_session_rc_id(&new.id, "session_01Old").unwrap();
        }
        let again = env.sessions.resume(&new.id, None, true).unwrap();
        assert!(!again.launch_cmd.iter().any(|a| a == "--remote-control"));
        assert_eq!(again.rc_session_id.as_deref(), Some("session_01Old"));
        env.sessions.kill(&again.id).unwrap();
    }

    #[test]
    fn retomar_mantem_modelo_e_modo() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let t = env.tmp.path().join("t.jsonl");
        std::fs::write(&t, "conversa\n").unwrap();
        let agent = AgentOptions {
            model: Some("sonnet".into()),
            mode: ct_core::SessionMode::Subagents,
            ..AgentOptions::default()
        };
        let old = interrupted_with(&env, Some(&t), agent.clone());
        let new = env.sessions.resume(&old.id, None, false).unwrap();
        let out = env.wait_fake_output(&new.id);
        assert!(
            out.contains("\n--model\nsonnet\n--append-system-prompt\n"),
            "{out}"
        );
        assert_eq!((new.model, new.mode), (agent.model, agent.mode));
        env.sessions.kill(&new.id).unwrap();
        env.sessions.kill(&old.id).ok();
    }

    #[test]
    fn retomar_restaura_transcript_do_backup() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let t = env.tmp.path().join("proj/t.jsonl");
        std::fs::create_dir_all(t.parent().unwrap()).unwrap();
        std::fs::write(&t, "conversa\n").unwrap();
        let old = interrupted(&env, Some(&t));
        std::fs::remove_dir_all(t.parent().unwrap()).unwrap();
        let new = env
            .sessions
            .resume(&old.id, Some("continue"), false)
            .unwrap();
        assert_eq!(std::fs::read_to_string(&t).unwrap(), "conversa\n");
        let out = env.wait_fake_output(&new.id);
        assert!(out.contains("\ncontinue\n"), "{out}");
        env.sessions.kill(&new.id).unwrap();
        env.sessions.kill(&old.id).ok();
    }

    #[test]
    fn retomar_sem_transcript_e_erro_sem_efeitos() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let old = interrupted(&env, None);
        let err = env.sessions.resume(&old.id, None, false).unwrap_err();
        assert!(err.to_string().contains("perdido"), "{err}");
        let l = env.sessions.ledger();
        assert_eq!(
            l.get_session(&old.id).unwrap().state,
            SessionState::Interrupted
        );
        assert_eq!(l.list_sessions(old.work_item_id).unwrap().len(), 1);
        drop(l);
        env.sessions.kill(&old.id).ok();
    }

    #[test]
    fn retomar_sessao_viva_e_recusado_e_arquivar_funciona() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let viva = env.sessions.start(env.request(3, "C")).unwrap();
        assert!(env.sessions.resume(&viva.id, None, false).is_err());
        env.sessions.kill(&viva.id).unwrap();
        env.wait_state(&viva.id, SessionState::Exited);
        env.sessions.archive(&viva.id).unwrap();
        assert_eq!(
            env.sessions.ledger().get_session(&viva.id).unwrap().state,
            SessionState::Archived
        );
    }

    #[test]
    fn descartadas_recentes_sem_as_retomadas() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let t = env.tmp.path().join("t.jsonl");
        std::fs::write(&t, "conversa\n").unwrap();
        let retomada = interrupted(&env, Some(&t));
        let nova = env.sessions.resume(&retomada.id, None, false).unwrap();
        let antes = now_ms();
        let descartada = interrupted(&env, None);
        env.sessions.archive(&descartada.id).unwrap();
        let items = env.sessions.recovery_archived(antes - 1, 10).unwrap();
        let ids: Vec<&str> = items.iter().map(|i| i.session.id.as_str()).collect();
        assert_eq!(ids, vec![descartada.id.as_str()]);
        assert_eq!(items[0].project_name, "demo");
        assert_eq!(items[0].transcript, TranscriptIntegrity::Lost);
        assert!(items[0].archived_at >= antes);
        assert!(env
            .sessions
            .recovery_archived(now_ms() + 1000, 10)
            .unwrap()
            .is_empty());
        assert!(env.sessions.recovery_archived(0, 0).unwrap().is_empty());
        env.sessions.kill(&nova.id).ok();
        env.sessions.kill(&descartada.id).ok();
    }
}
