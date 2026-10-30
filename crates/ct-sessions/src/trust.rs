//! Pré-aceite da confiança na pasta do Claude Code (issue #16).
//!
//! O Claude Code pergunta "Is this a project you trust?" (com "No, exit"
//! pré-selecionado) em toda pasta nova, e nenhum hook dispara até alguém
//! responder. O daemon marca como confiáveis só as pastas que ele mesmo usa:
//! a raiz do repositório (chave que o Claude usa para worktrees) e o worktree.
//!
//! Formato (Claude Code 2.1.301): `projects["<pasta>"].hasTrustDialogAccepted`
//! em `~/.claude.json` (ou `$CLAUDE_CONFIG_DIR/.claude.json`).

use serde_json::{json, Map, Value};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Caminho do `.claude.json` do ambiente.
pub fn claude_json_path() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    var("CLAUDE_CONFIG_DIR")
        .or_else(|| var("HOME"))
        .map(|d| d.join(".claude.json"))
}

/// Pasta dos transcripts do Claude Code por pasta de trabalho:
/// `$CLAUDE_CONFIG_DIR/projects` ou `~/.claude/projects` (#285).
pub fn claude_projects_dir() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    var("CLAUDE_CONFIG_DIR")
        .or_else(|| var("HOME").map(|h| h.join(".claude")))
        .map(|d| d.join("projects"))
}

/// Marca `dirs` como confiáveis em `claude_json`. Retorna as pastas que
/// mudaram (vazio = nada a fazer, arquivo intocado).
///
/// Arquivo ausente ou JSON inválido → erro, sem alterar nada. Antes de
/// alterar, copia o original para `<arquivo>.ctower.bak`; a escrita é
/// atômica (temporário + rename) e mantém a permissão do original.
pub fn ensure_trusted(claude_json: &Path, dirs: &[&str]) -> io::Result<Vec<String>> {
    let raw = fs::read_to_string(claude_json)?;
    let mut doc: Value = serde_json::from_str(&raw).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{}: JSON inválido ({e}); nada foi alterado",
                claude_json.display()
            ),
        )
    })?;
    let Some(root) = doc.as_object_mut() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{}: esperado um objeto JSON; nada foi alterado",
                claude_json.display()
            ),
        ));
    };
    let projects = root
        .entry("projects")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(projects) = projects.as_object_mut() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "campo `projects` não é um objeto; nada foi alterado",
        ));
    };
    let mut changed = Vec::new();
    for dir in dirs {
        let entry = projects
            .entry((*dir).to_string())
            .or_insert_with(|| json!({}));
        let Some(entry) = entry.as_object_mut() else {
            continue;
        };
        if entry.get("hasTrustDialogAccepted") != Some(&Value::Bool(true)) {
            entry.insert("hasTrustDialogAccepted".into(), Value::Bool(true));
            changed.push((*dir).to_string());
        }
    }
    if changed.is_empty() {
        return Ok(changed);
    }

    let mode = fs::metadata(claude_json)?.permissions().mode() & 0o777;
    let backup = sibling(claude_json, ".ctower.bak");
    fs::write(&backup, &raw)?;
    fs::set_permissions(&backup, fs::Permissions::from_mode(0o600))?;

    let tmp = sibling(claude_json, &format!(".ctower.{}.tmp", std::process::id()));
    let res = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)?;
        let mut body = serde_json::to_string_pretty(&doc).expect("Value serializa");
        body.push('\n');
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, claude_json)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res.map(|()| changed)
}

fn sibling(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(p: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn marca_pastas_e_preserva_o_resto() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join(".claude.json");
        let original = json!({
            "numStartups": 7,
            "projects": { "/repo": { "allowedTools": ["Bash"], "hasTrustDialogAccepted": false } }
        });
        fs::write(&f, original.to_string()).unwrap();
        fs::set_permissions(&f, fs::Permissions::from_mode(0o600)).unwrap();

        let changed = ensure_trusted(&f, &["/repo", "/wt/feat__1-x"]).unwrap();
        assert_eq!(changed, ["/repo", "/wt/feat__1-x"]);
        let v = read(&f);
        assert_eq!(v["numStartups"], 7);
        assert_eq!(v["projects"]["/repo"]["allowedTools"], json!(["Bash"]));
        assert_eq!(v["projects"]["/repo"]["hasTrustDialogAccepted"], true);
        assert_eq!(
            v["projects"]["/wt/feat__1-x"]["hasTrustDialogAccepted"],
            true
        );
        assert_eq!(
            fs::metadata(&f).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // Backup é o original.
        assert_eq!(read(&d.path().join(".claude.json.ctower.bak")), original);
        // Sem temporários residuais.
        let names: Vec<_> = fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
    }

    #[test]
    fn ja_confiavel_nao_reescreve() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join(".claude.json");
        let body = r#"{"projects":{"/repo":{"hasTrustDialogAccepted":true}}}"#;
        fs::write(&f, body).unwrap();
        assert!(ensure_trusted(&f, &["/repo"]).unwrap().is_empty());
        assert_eq!(fs::read_to_string(&f).unwrap(), body);
        assert!(!d.path().join(".claude.json.ctower.bak").exists());
    }

    #[test]
    fn sem_projects_cria_o_mapa() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join(".claude.json");
        fs::write(&f, r#"{"a":1}"#).unwrap();
        ensure_trusted(&f, &["/r"]).unwrap();
        assert_eq!(read(&f)["projects"]["/r"]["hasTrustDialogAccepted"], true);
    }

    #[test]
    fn ausente_ou_invalido_nao_altera() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join(".claude.json");
        assert!(ensure_trusted(&f, &["/r"]).is_err());
        assert!(!f.exists());
        for body in ["{ quebrado", "[1,2]", r#"{"projects":[]}"#] {
            fs::write(&f, body).unwrap();
            assert!(ensure_trusted(&f, &["/r"]).is_err(), "{body}");
            assert_eq!(fs::read_to_string(&f).unwrap(), body);
        }
    }
}
