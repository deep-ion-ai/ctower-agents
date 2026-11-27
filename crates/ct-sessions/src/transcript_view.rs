//! Leitura do transcript de uma sessão para a UI (#328): o fim da conversa
//! (`transcript.tail`, também de sessão parada, sem PTY) e o uso de contexto
//! (`session.context`). Lê o transcript original ou, sem ele, o backup, só
//! o fim do arquivo, com os secrets da sessão mascarados. O formato das
//! linhas é do runtime do agente ([`ct_agent::TranscriptReader`], #241).

use crate::{Result, Sessions, SessionsError};
pub use ct_agent::{ContextUsage, TranscriptEntry};
use ct_ledger::Session;
use serde::Serialize;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Quanto do fim do arquivo é lido (linhas de tool_result podem ser grandes).
const TAIL_BYTES: u64 = 2 * 1024 * 1024;

/// Limite de `transcript.tail`.
pub const TAIL_MAX_ENTRIES: usize = 200;

/// De onde veio a leitura.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum TranscriptSource {
    Original,
    Backup,
}

/// Resposta de `transcript.tail`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TranscriptTail {
    pub source: TranscriptSource,
    /// Da mais antiga para a mais recente.
    pub entries: Vec<TranscriptEntry>,
}

impl Sessions {
    /// Fim da conversa de qualquer sessão com transcript (viva ou parada).
    pub fn transcript_tail(&self, session_id: &str, max: usize) -> Result<TranscriptTail> {
        let s = self.ledger().get_session(session_id)?;
        let reader = crate::agent_runtime(s.tool)?.transcripts();
        let (source, data) = self.read_tail(&s)?;
        let mut entries = reader.entries(&data);
        let skip = entries.len().saturating_sub(max);
        entries.drain(..skip);
        Ok(TranscriptTail { source, entries })
    }

    /// Uso de contexto da sessão, pela última resposta do agente.
    pub fn context_usage(&self, session_id: &str) -> Result<ContextUsage> {
        let s = self.ledger().get_session(session_id)?;
        let reader = crate::agent_runtime(s.tool)?.transcripts();
        let (_, data) = self.read_tail(&s)?;
        Ok(reader.last_usage(&data).unwrap_or(ContextUsage {
            tokens: None,
            model: None,
            ts: None,
        }))
    }

    /// Fim do transcript (original ou backup), mascarado.
    fn read_tail(&self, s: &Session) -> Result<(TranscriptSource, Vec<u8>)> {
        let (source, path) = match (existing(&s.transcript_path), existing(&s.backup_path)) {
            (Some(p), _) => (TranscriptSource::Original, p),
            (None, Some(p)) => (TranscriptSource::Backup, p),
            (None, None) => {
                return Err(SessionsError::Invalid(format!(
                    "sessão {} sem transcript nem backup",
                    s.id
                )))
            }
        };
        let raw = tail_bytes(Path::new(path), TAIL_BYTES)?;
        Ok((source, self.transcript_mask(s).apply(&raw)))
    }
}

fn existing(p: &Option<String>) -> Option<&str> {
    p.as_deref().filter(|p| Path::new(p).is_file())
}

/// Os últimos `n` bytes do arquivo, começando numa linha inteira.
fn tail_bytes(path: &Path, n: u64) -> std::io::Result<Vec<u8>> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(n);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    if start > 0 {
        let cut = buf
            .iter()
            .position(|&b| b == b'\n')
            .map_or(buf.len(), |i| i + 1);
        buf.drain(..cut);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use serde_json::json;
    use std::io::Write;

    fn transcript(path: &Path) {
        let lines = [
            json!({ "type": "user", "timestamp": "t1", "message": { "role": "user", "content": "faça X" } }),
            json!({ "type": "assistant", "timestamp": "t2", "message": {
                "model": "claude-opus-5-5",
                "content": [
                    { "type": "thinking", "thinking": "..." },
                    { "type": "text", "text": "Vou rodar os testes." },
                    { "type": "tool_use", "name": "Bash", "input": { "command": "cargo test" } },
                ],
                "usage": { "input_tokens": 2, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 1000, "output_tokens": 50 },
            } }),
            json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "content": "ok" }] } }),
            json!({ "type": "assistant", "isSidechain": true, "message": {
                "content": [{ "type": "text", "text": "subagente" }],
                "usage": { "input_tokens": 9 },
            } }),
            json!({ "type": "permission-mode" }),
        ];
        let mut f = File::create(path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        writeln!(f, "linha quebrada {{").unwrap();
    }

    #[test]
    fn fim_da_conversa_e_uso_de_contexto() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let s = env.sessions.start(env.request(1, "A")).unwrap();
        env.wait_fake_output(&s.id);
        assert!(env.sessions.transcript_tail(&s.id, 10).is_err());
        let t = env.tmp.path().join("t.jsonl");
        transcript(&t);
        env.sessions
            .ledger()
            .set_session_tool_info(&s.id, "u", &t.to_string_lossy())
            .unwrap();
        let tail = env.sessions.transcript_tail(&s.id, 10).unwrap();
        assert_eq!(tail.source, TranscriptSource::Original);
        let got: Vec<(&str, &str)> = tail
            .entries
            .iter()
            .map(|e| (e.role.as_str(), e.text.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("user", "faça X"),
                ("assistant", "Vou rodar os testes."),
                ("tool", "Bash: cargo test"),
            ]
        );
        assert_eq!(tail.entries[0].ts.as_deref(), Some("t1"));
        let tail = env.sessions.transcript_tail(&s.id, 1).unwrap();
        assert_eq!(tail.entries.len(), 1);
        assert_eq!(tail.entries[0].role, "tool");

        let u = env.sessions.context_usage(&s.id).unwrap();
        assert_eq!(u.tokens, Some(1152));
        assert_eq!(u.model.as_deref(), Some("claude-opus-5-5"));

        // Sem o original, lê o backup.
        env.sessions.backup_session(&s.id).unwrap();
        std::fs::remove_file(&t).unwrap();
        let tail = env.sessions.transcript_tail(&s.id, 10).unwrap();
        assert_eq!(tail.source, TranscriptSource::Backup);
        assert_eq!(tail.entries.len(), 3);
    }

    #[test]
    fn fim_do_arquivo_comeca_em_linha_inteira() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("t");
        std::fs::write(&p, "aaaa\nbb\ncc\n").unwrap();
        assert_eq!(tail_bytes(&p, 5).unwrap(), b"cc\n");
        assert_eq!(tail_bytes(&p, 100).unwrap(), b"aaaa\nbb\ncc\n");
    }
}
