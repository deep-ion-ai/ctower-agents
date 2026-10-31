//! Log da própria UI no app desktop (#65): o JavaScript guarda um buffer em
//! anel e manda as linhas (JSON, já mascaradas) para cá, que grava em
//! `<app_log_dir>/ui.jsonl` com rotação por tamanho. Na abertura, a UI lê o
//! arquivo para mostrar também a execução anterior. Só `std`: o formato das
//! linhas é o do log dos daemons (`ct_http::logs::LogLine`).

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Tamanho do arquivo antes de rotacionar, e quantos ficam (o atual e `.1`).
pub const MAX_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_FILES: u32 = 2;
/// Linhas devolvidas por `ui_log_read` (as mais novas).
pub const READ_MAX: usize = 2000;

pub struct UiLog {
    path: PathBuf,
    max_bytes: u64,
    lock: Mutex<()>,
}

impl UiLog {
    pub fn new(dir: &Path) -> UiLog {
        UiLog::with_limit(dir, MAX_BYTES)
    }

    pub fn with_limit(dir: &Path, max_bytes: u64) -> UiLog {
        UiLog {
            path: dir.join("ui.jsonl"),
            max_bytes,
            lock: Mutex::new(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn rotated(&self) -> PathBuf {
        let mut s = self.path.as_os_str().to_owned();
        s.push(".1");
        PathBuf::from(s)
    }

    /// Acrescenta as linhas (uma por linha do arquivo; quebras viram espaço).
    pub fn append(&self, lines: &[String]) -> io::Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut buf = String::new();
        for l in lines {
            buf.push_str(&l.replace(['\n', '\r'], " "));
            buf.push('\n');
        }
        let size = fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        if size > 0 && size + buf.len() as u64 > self.max_bytes && MAX_FILES > 1 {
            fs::rename(&self.path, self.rotated())?;
        }
        let mut f = open_private(&self.path)?;
        f.write_all(buf.as_bytes())
    }

    /// As últimas `limit` linhas gravadas (do `.1` e do atual), em ordem.
    pub fn read(&self, limit: usize) -> Vec<String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<String> = Vec::new();
        for p in [self.rotated(), self.path.clone()] {
            if let Ok(b) = fs::read(&p) {
                out.extend(
                    String::from_utf8_lossy(&b)
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(str::to_string),
                );
            }
        }
        let skip = out.len().saturating_sub(limit);
        out.split_off(skip)
    }
}

#[cfg(unix)]
fn open_private(p: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(p)
}

#[cfg(not(unix))]
fn open_private(p: &Path) -> io::Result<fs::File> {
    OpenOptions::new().create(true).append(true).open(p)
}

/// Nome de arquivo seguro: sem pastas nem caracteres reservados do Windows.
pub fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let s: String = base
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() {
        "ctower.log".into()
    } else {
        s
    }
}

/// Grava `content` em `dir/<name>` sem sobrescrever (`nome (2).log`, …).
pub fn save_into(dir: &Path, name: &str, content: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let name = safe_name(name);
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    for n in 1..1000 {
        let file = if n == 1 {
            name.clone()
        } else {
            format!("{stem} ({n}){ext}")
        };
        let p = dir.join(file);
        match OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(mut f) => {
                f.write_all(content.as_bytes())?;
                return Ok(p);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "nome de arquivo indisponível",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grava_rotaciona_e_le_as_ultimas() {
        let dir = tempfile::tempdir().unwrap();
        let log = UiLog::with_limit(dir.path(), 100);
        for i in 0..20 {
            log.append(&[format!("{{\"n\":{i}}}")]).unwrap();
        }
        assert!(dir.path().join("ui.jsonl.1").exists());
        let last = log.read(3);
        assert_eq!(last, ["{\"n\":17}", "{\"n\":18}", "{\"n\":19}"]);
        log.append(&["a\nb".into()]).unwrap();
        assert_eq!(log.read(1), ["a b"]);
    }

    #[test]
    fn exporta_sem_sobrescrever_nem_sair_da_pasta() {
        let dir = tempfile::tempdir().unwrap();
        let a = save_into(dir.path(), "../../x.log", "1").unwrap();
        let b = save_into(dir.path(), "x.log", "2").unwrap();
        assert_eq!(a, dir.path().join("x.log"));
        assert_eq!(b, dir.path().join("x (2).log"));
        assert_eq!(safe_name("a:b?.jsonl"), "a_b_.jsonl");
        assert_eq!(safe_name(".."), "ctower.log");
    }
}
