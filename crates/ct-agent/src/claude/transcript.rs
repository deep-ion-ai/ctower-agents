//! Transcripts do Claude Code (#241): o formato JSONL das linhas (falas e
//! uso de contexto, #328) e o layout de `~/.claude/projects` (uma pasta por
//! pasta de trabalho, #285).

use crate::{ContextUsage, TranscriptEntry, TranscriptReader};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Corte do texto de cada entrada.
const TEXT_MAX: usize = 2000;

/// Nome da pasta do Claude Code para `path` em `~/.claude/projects`: cada
/// caractere fora de `[A-Za-z0-9]` vira `-` (`/home/u/wt/a.b` →
/// `-home-u-wt-a-b`). O caminho do transcript registrado pelos hooks também
/// é usado ([`transcript_dirs_for`]), então uma mudança de regra do Claude
/// Code não faz apagar a pasta errada: no máximo deixa de achar uma.
pub fn project_slug(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Pastas de transcript de `cwd` em `projects`: a do slug do caminho e as
/// que contêm os transcripts registrados (`transcripts`), só filhas diretas
/// de `projects`. Não olha o disco.
pub fn transcript_dirs_for(projects: &Path, cwd: &str, transcripts: &[&Path]) -> Vec<PathBuf> {
    let mut dirs = vec![projects.join(project_slug(cwd))];
    for parent in transcripts.iter().filter_map(|t| t.parent()) {
        if parent.parent() == Some(projects) && !dirs.iter().any(|c| c == parent) {
            dirs.push(parent.to_path_buf());
        }
    }
    dirs
}

/// Sessão de uma entrada da pasta de transcripts: `<id>.jsonl` (o
/// transcript) ou a pasta `<id>/` (dados do agente da sessão).
pub fn transcript_entry_id(name: &str, is_file: bool) -> Option<&str> {
    if is_file {
        name.strip_suffix(".jsonl").filter(|id| !id.is_empty())
    } else {
        Some(name)
    }
}

/// Leitor das linhas JSONL do transcript do Claude Code.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeTranscripts;

impl TranscriptReader for ClaudeTranscripts {
    fn entries(&self, data: &[u8]) -> Vec<TranscriptEntry> {
        lines(data).iter().flat_map(entries_of).collect()
    }

    fn last_usage(&self, data: &[u8]) -> Option<ContextUsage> {
        lines(data).iter().rev().find_map(usage_of)
    }
}

/// Linhas JSON válidas; as quebradas ficam de fora.
fn lines(data: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(data)
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

fn cut(s: &str) -> String {
    let s = s.trim();
    match s.char_indices().nth(TEXT_MAX) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// Falas de uma linha do transcript: só `user`/`assistant` da conversa
/// principal; resultados de ferramenta e blocos de raciocínio ficam de fora.
fn entries_of(v: &Value) -> Vec<TranscriptEntry> {
    let kind = v.get("type").and_then(Value::as_str).unwrap_or_default();
    if !matches!(kind, "user" | "assistant") || v.get("isSidechain") == Some(&Value::Bool(true)) {
        return Vec::new();
    }
    let ts = v.get("timestamp").and_then(Value::as_str).map(String::from);
    let entry = |role: &str, text: String| TranscriptEntry {
        role: role.into(),
        text,
        ts: ts.clone(),
    };
    match v.pointer("/message/content") {
        Some(Value::String(t)) if !t.trim().is_empty() => vec![entry(kind, cut(t))],
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| match b.get("type").and_then(Value::as_str) {
                Some("text") => b
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|t| !t.trim().is_empty())
                    .map(|t| entry(kind, cut(t))),
                Some("tool_use") => {
                    let name = b.get("name").and_then(Value::as_str).unwrap_or("?");
                    let arg = ["command", "file_path", "pattern", "url", "description"]
                        .iter()
                        .find_map(|k| b.pointer(&format!("/input/{k}")).and_then(Value::as_str));
                    Some(entry(
                        "tool",
                        match arg {
                            Some(a) => cut(&format!("{name}: {a}")),
                            None => name.to_string(),
                        },
                    ))
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Uso de contexto de uma resposta da conversa principal.
fn usage_of(v: &Value) -> Option<ContextUsage> {
    if v.get("type").and_then(Value::as_str) != Some("assistant")
        || v.get("isSidechain") == Some(&Value::Bool(true))
    {
        return None;
    }
    let u = v.pointer("/message/usage")?;
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    Some(ContextUsage {
        tokens: Some(
            n("input_tokens")
                + n("cache_creation_input_tokens")
                + n("cache_read_input_tokens")
                + n("output_tokens"),
        ),
        model: v
            .pointer("/message/model")
            .and_then(Value::as_str)
            .map(String::from),
        ts: v.get("timestamp").and_then(Value::as_str).map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corta_texto_longo() {
        let s = "é".repeat(TEXT_MAX + 5);
        let c = cut(&s);
        assert_eq!(c.chars().count(), TEXT_MAX + 1);
        assert!(c.ends_with('…'));
    }

    #[test]
    fn slug_do_claude_code() {
        assert_eq!(
            project_slug("/Users/u/worktrees/control-tower/feat__285-x.y"),
            "-Users-u-worktrees-control-tower-feat--285-x-y"
        );
        assert_eq!(project_slug("/home/joão/wt"), "-home-jo-o-wt");
    }

    #[test]
    fn pastas_do_slug_e_dos_transcripts_registrados() {
        let p = Path::new("/h/.claude/projects");
        let dirs = transcript_dirs_for(
            p,
            "/wt/a.b",
            &[
                Path::new("/h/.claude/projects/-wt-a-b/u-1.jsonl"),
                Path::new("/h/.claude/projects/-antigo/u-2.jsonl"),
                Path::new("/h/.claude/projects/-antigo/u-3.jsonl"),
                Path::new("/h/.claude/projects/u-4.jsonl"),
                Path::new("/outro/-x/u-5.jsonl"),
            ],
        );
        assert_eq!(
            dirs,
            [p.join("-wt-a-b"), p.join("-antigo")],
            "só filhas diretas de projects, sem repetir"
        );
    }

    #[test]
    fn entradas_da_pasta_de_transcripts() {
        assert_eq!(transcript_entry_id("u-1.jsonl", true), Some("u-1"));
        assert_eq!(transcript_entry_id("u-1", false), Some("u-1"));
        assert_eq!(transcript_entry_id(".jsonl", true), None);
        assert_eq!(transcript_entry_id("notas.txt", true), None);
    }

    #[test]
    fn falas_e_uso_ignoram_linha_quebrada_e_subagente() {
        let data = concat!(
            r#"{"type":"user","timestamp":"t1","message":{"content":"oi"}}"#,
            "\nquebrada {\n",
            r#"{"type":"assistant","message":{"model":"m","content":[{"type":"text","text":"olá"}],"usage":{"input_tokens":1,"output_tokens":2}}}"#,
            "\n",
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"sub"}],"usage":{"input_tokens":9}}}"#,
            "\n",
        );
        let r = ClaudeTranscripts;
        let e = r.entries(data.as_bytes());
        let got: Vec<_> = e
            .iter()
            .map(|e| (e.role.as_str(), e.text.as_str()))
            .collect();
        assert_eq!(got, [("user", "oi"), ("assistant", "olá")]);
        let u = r.last_usage(data.as_bytes()).unwrap();
        assert_eq!((u.tokens, u.model.as_deref()), (Some(3), Some("m")));
        assert!(r.last_usage(b"").is_none());
    }
}
