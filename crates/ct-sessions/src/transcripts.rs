//! Backup de transcripts e integridade para a recuperação (spec §7.4).

use crate::{Result, Sessions, SessionsError};
use ct_core::SessionState;
use ct_ledger::{NewEvent, Session};
use ct_pty::MaskSet;
use serde::Serialize;
use serde_json::json;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum TranscriptIntegrity {
    #[serde(rename = "original")]
    Original,
    #[serde(rename = "restauravel_do_backup")]
    RestorableFromBackup,
    #[serde(rename = "perdido")]
    Lost,
}

/// Copia `src` para `<dst_dir>/<tool_session_id>.jsonl` de forma atômica
/// (temporário + fsync + rename), com os valores de `mask` trocados por
/// `***NOME***` na cópia (#167; `src` não é tocado). Se `src` não existe, o
/// backup anterior fica intacto.
pub fn backup_transcript(
    src: &Path,
    dst_dir: &Path,
    tool_session_id: &str,
    mask: &MaskSet,
) -> io::Result<PathBuf> {
    if tool_session_id.is_empty() || tool_session_id.contains(['/', '\0']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("id de sessão inválido: {tool_session_id:?}"),
        ));
    }
    let data = mask.apply(&fs::read(src)?);
    fs::create_dir_all(dst_dir)?;
    let dst = dst_dir.join(format!("{tool_session_id}.jsonl"));
    let tmp = dst_dir.join(format!(".{tool_session_id}.jsonl.tmp"));
    let res = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&data)?;
        f.sync_all()?;
        fs::rename(&tmp, &dst)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res.map(|()| dst)
}

pub fn integrity(s: &Session) -> TranscriptIntegrity {
    let exists = |p: &Option<String>| p.as_deref().is_some_and(|p| Path::new(p).is_file());
    if exists(&s.transcript_path) {
        TranscriptIntegrity::Original
    } else if exists(&s.backup_path) {
        TranscriptIntegrity::RestorableFromBackup
    } else {
        TranscriptIntegrity::Lost
    }
}

/// Transcript em local que some no reboot (`/tmp`, `/run`, tmpfs).
pub fn is_volatile(path: &Path) -> bool {
    const PREFIXES: &[&str] = &[
        "/tmp",
        "/run",
        "/dev/shm",
        "/var/run",
        "/private/tmp",
        "/private/var/run",
    ];
    if PREFIXES.iter().any(|p| path.starts_with(p)) {
        return true;
    }
    let Ok(mounts) = fs::read_to_string("/proc/mounts") else {
        return false;
    };
    mounts
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let (_, mnt, fstype) = (it.next()?, it.next()?, it.next()?);
            path.starts_with(mnt).then_some((mnt.len(), fstype))
        })
        .max_by_key(|(len, _)| *len)
        .is_some_and(|(_, fs)| matches!(fs, "tmpfs" | "ramfs"))
}

impl Sessions {
    /// Faz backup do transcript da sessão e grava o caminho no ledger.
    pub fn backup_session(&self, session_id: &str) -> Result<PathBuf> {
        let s = self.ledger().get_session(session_id)?;
        let (Some(src), Some(tool_id)) =
            (s.transcript_path.as_deref(), s.tool_session_id.as_deref())
        else {
            return Err(SessionsError::Invalid(format!(
                "sessão {session_id} ainda sem transcript registrado"
            )));
        };
        let mask = self.transcript_mask(&s);
        let dst = backup_transcript(
            Path::new(src),
            &self.config().dirs.transcripts(),
            tool_id,
            &mask,
        )?;
        self.ledger()
            .set_session_backup_path(session_id, &dst.to_string_lossy())?;
        Ok(dst)
    }

    /// Backup periódico (a cada 5 min pelo daemon) das sessões `running`.
    /// Falhas viram evento `transcript_backup_failed`; retorna quantos backups deram certo.
    pub fn backup_running(&self) -> Result<usize> {
        let running = self.ledger().sessions_in_state(SessionState::Running)?;
        let mut ok = 0;
        for s in running.iter().filter(|s| s.transcript_path.is_some()) {
            match self.backup_session(&s.id) {
                Ok(_) => ok += 1,
                Err(e) => self.backup_failed(s, &e)?,
            }
        }
        Ok(ok)
    }

    pub(crate) fn backup_failed(&self, s: &Session, e: &SessionsError) -> Result<()> {
        let l = self.ledger();
        let wi = l.get_work_item(s.work_item_id)?;
        l.append_event(NewEvent {
            project_id: wi.project_id,
            work_item_id: Some(wi.id),
            session_id: Some(&s.id),
            kind: "transcript_backup_failed",
            payload: json!({ "error": e.to_string() }),
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use ct_core::Tool;

    #[test]
    fn copia_atomica_sem_temporario_residual() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("t.jsonl");
        fs::write(&src, "{\"a\":1}\n").unwrap();
        let dst =
            backup_transcript(&src, &d.path().join("bk"), "abc", &MaskSet::default()).unwrap();
        assert_eq!(dst, d.path().join("bk/abc.jsonl"));
        assert_eq!(fs::read(&dst).unwrap(), fs::read(&src).unwrap());
        let names: Vec<_> = fs::read_dir(d.path().join("bk"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec!["abc.jsonl"]);
    }

    #[test]
    fn origem_ausente_preserva_backup_anterior() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("t.jsonl");
        fs::write(&src, "v1").unwrap();
        let bk = d.path().join("bk");
        let dst = backup_transcript(&src, &bk, "abc", &MaskSet::default()).unwrap();
        fs::remove_file(&src).unwrap();
        assert!(backup_transcript(&src, &bk, "abc", &MaskSet::default()).is_err());
        assert_eq!(fs::read_to_string(dst).unwrap(), "v1");
    }

    #[test]
    fn id_com_barra_e_recusado() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("t");
        fs::write(&src, "x").unwrap();
        assert!(backup_transcript(&src, d.path(), "../fora", &MaskSet::default()).is_err());
    }

    fn session_with(transcript: Option<String>, backup: Option<String>) -> Session {
        Session {
            id: "s".into(),
            work_item_id: 1,
            tool: Tool::Claude,
            tool_session_id: Some("u".into()),
            transcript_path: transcript,
            backup_path: backup,
            cwd: "/w".into(),
            launch_cmd: vec![],
            model: None,
            mode: ct_core::SessionMode::Native,
            rc_session_id: None,
            state: SessionState::Interrupted,
            exit_code: None,
            pid: None,
            started_at: 0,
            last_event_at: 0,
            ended_at: None,
        }
    }

    #[test]
    fn integridade_nos_tres_casos() {
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join("t.jsonl");
        let b = d.path().join("b.jsonl");
        fs::write(&t, "x").unwrap();
        fs::write(&b, "x").unwrap();
        let p = |p: &Path| Some(p.to_string_lossy().to_string());
        assert_eq!(
            integrity(&session_with(p(&t), p(&b))),
            TranscriptIntegrity::Original
        );
        fs::remove_file(&t).unwrap();
        assert_eq!(
            integrity(&session_with(p(&t), p(&b))),
            TranscriptIntegrity::RestorableFromBackup
        );
        fs::remove_file(&b).unwrap();
        assert_eq!(
            integrity(&session_with(p(&t), p(&b))),
            TranscriptIntegrity::Lost
        );
        assert_eq!(
            integrity(&session_with(None, None)),
            TranscriptIntegrity::Lost
        );
    }

    #[test]
    fn caminhos_volateis() {
        assert!(is_volatile(Path::new("/tmp/x.jsonl")));
        assert!(is_volatile(Path::new("/run/user/1000/x")));
        assert!(!is_volatile(Path::new("/home/u/.claude/projects/x.jsonl")));
    }

    /// Backup do transcript sem os valores dos secrets (#167), inclusive a
    /// forma escapada em JSON; o original fica intacto. Sem o conjunto em
    /// memória (daemon reiniciado), o backup recalcula do cache.
    #[test]
    fn backup_do_transcript_nao_contem_o_valor() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        env.write_secrets_cache_kinds(&[
            (
                None,
                "T_TOKEN",
                ct_secrets::Kind::Secret,
                "token-muito-secreto",
            ),
            (
                None,
                "T_ASPAS",
                ct_secrets::Kind::Secret,
                "tem\"aspas\\barra",
            ),
            (
                None,
                "T_VAR",
                ct_secrets::Kind::Variable,
                "variavel-visivel",
            ),
        ]);
        let s = env.sessions.start(env.request(1, "A")).unwrap();
        let t = env.tmp.path().join("transcript.jsonl");
        let original = concat!(
            "{\"out\":\"T_TOKEN=token-muito-secreto\\nT_VAR=variavel-visivel\"}\n",
            "{\"out\":\"tem\\\"aspas\\\\barra token-muito-secreto\"}\n",
        );
        fs::write(&t, original).unwrap();
        env.sessions
            .ledger()
            .set_session_tool_info(&s.id, "uuid-m", &t.to_string_lossy())
            .unwrap();
        let want = concat!(
            "{\"out\":\"T_TOKEN=***T_TOKEN***\\nT_VAR=variavel-visivel\"}\n",
            "{\"out\":\"***T_ASPAS*** ***T_TOKEN***\"}\n",
        );
        let bk = env.sessions.backup_session(&s.id).unwrap();
        assert_eq!(fs::read_to_string(&bk).unwrap(), want);
        assert_eq!(fs::read_to_string(&t).unwrap(), original);

        env.sessions.forget_mask(&s.id);
        fs::remove_file(&bk).unwrap();
        let bk = env.sessions.backup_session(&s.id).unwrap();
        let got = fs::read_to_string(&bk).unwrap();
        assert_eq!(got, want);
        assert!(!got.contains("token-muito-secreto"));
        env.sessions.kill(&s.id).unwrap();
    }

    #[test]
    fn backup_session_grava_caminho_e_backup_running_conta() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let s = env.sessions.start(env.request(1, "A")).unwrap();
        let t = env.tmp.path().join("transcript.jsonl");
        fs::write(&t, "linha\n").unwrap();
        {
            let l = env.sessions.ledger();
            l.set_session_tool_info(&s.id, "uuid-1", &t.to_string_lossy())
                .unwrap();
            l.transition_session(&s.id, SessionState::Running, None)
                .unwrap();
        }
        assert_eq!(env.sessions.backup_running().unwrap(), 1);
        let got = env.sessions.ledger().get_session(&s.id).unwrap();
        let bk = env.cfg.dirs.transcripts().join("uuid-1.jsonl");
        assert_eq!(got.backup_path.as_deref(), Some(bk.to_str().unwrap()));
        assert_eq!(fs::read_to_string(bk).unwrap(), "linha\n");

        fs::remove_file(&t).unwrap();
        assert_eq!(env.sessions.backup_running().unwrap(), 0);
        let evs = env
            .sessions
            .ledger()
            .list_events(s.work_item_id, 100)
            .unwrap();
        assert!(evs.iter().any(|e| e.kind == "transcript_backup_failed"));
        env.sessions.kill(&s.id).unwrap();
    }
}
