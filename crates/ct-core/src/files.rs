//! Arquivos privados dos daemons (`ctowerd` e `cthubd`): diretórios 0700,
//! tokens, identidade e store de secrets 0600. É a única parte deste crate
//! que toca o disco.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub fn private_dir(path: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(path)
}

/// Escreve `contents` em `path` com permissão 0600 (sobrescreve).
pub fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(contents)?;
    // `mode` só vale na criação: garante 0600 também ao sobrescrever.
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

/// Escreve `contents` em `path` de forma atômica, com 0600: grava um
/// temporário no mesmo diretório (criado já com 0600), faz `fsync`, troca
/// pelo `rename` e faz `fsync` do diretório. Uma falha no meio deixa o
/// arquivo anterior intacto (e o temporário é apagado).
pub fn write_private_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_private_atomic_with(path, contents, |_| Ok(()))
}

/// [`write_private_atomic`] com um gancho chamado com o temporário já
/// gravado e sincronizado, logo antes do `rename`: um erro dele aborta a
/// troca. Serve para os testes simularem uma falha no meio da escrita.
pub fn write_private_atomic_with(
    path: &Path,
    contents: &[u8],
    before_rename: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "caminho sem nome de arquivo")
    })?;
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let tmp = dir.join(format!(
        ".{}.tmp-{}-{}",
        name.to_string_lossy(),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let res = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
        drop(f);
        before_rename(&tmp)?;
        fs::rename(&tmp, path)?;
        fs::File::open(dir)?.sync_all()
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

/// Lê o token de `path` ou, se não existir, gera um novo (32 bytes, hex) com 0600.
pub fn ensure_token(path: &Path) -> io::Result<String> {
    if let Ok(t) = fs::read_to_string(path) {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return Ok(t);
        }
    }
    let mut bytes = [0u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    if let Some(dir) = path.parent() {
        private_dir(dir)?;
    }
    write_private(path, token.as_bytes())?;
    Ok(token)
}

/// Lê o `env_id` deste ambiente de `path` ou, no primeiro boot, gera um
/// UUID novo e o grava com 0600 (spec do ledger centralizado §3).
pub fn ensure_env_id(path: &Path) -> io::Result<String> {
    if let Ok(s) = fs::read_to_string(path) {
        let s = s.trim();
        if uuid::Uuid::parse_str(s).is_ok() {
            return Ok(s.to_string());
        }
        if !s.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: env_id inválido", path.display()),
            ));
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    if let Some(dir) = path.parent() {
        private_dir(dir)?;
    }
    write_private(path, id.as_bytes())?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn token_e_criado_uma_vez_com_0600() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("cfg/ui.token");
        let t1 = ensure_token(&p).unwrap();
        assert_eq!(t1.len(), 64);
        assert!(t1.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(mode(&p), 0o600);
        assert_eq!(ensure_token(&p).unwrap(), t1);
        let t2 = ensure_token(&d.path().join("cfg/hook.token")).unwrap();
        assert_ne!(t1, t2);
    }

    #[test]
    fn env_id_e_gerado_uma_vez() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("cfg/env_id");
        let a = ensure_env_id(&p).unwrap();
        assert!(uuid::Uuid::parse_str(&a).is_ok());
        assert_eq!(mode(&p), 0o600);
        assert_eq!(ensure_env_id(&p).unwrap(), a);
        fs::write(&p, "lixo").unwrap();
        assert!(ensure_env_id(&p).is_err());
    }

    #[test]
    fn escrita_atomica_preserva_o_anterior_em_falha() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("store");
        write_private_atomic(&p, b"v1").unwrap();
        assert_eq!(mode(&p), 0o600);
        let e = write_private_atomic_with(&p, b"v2", |tmp| {
            assert_eq!(fs::read(tmp).unwrap(), b"v2");
            assert_eq!(mode(tmp), 0o600);
            Err(io::Error::other("falha simulada"))
        })
        .unwrap_err();
        assert_eq!(e.to_string(), "falha simulada");
        assert_eq!(fs::read(&p).unwrap(), b"v1");
        // Só o arquivo final: o temporário foi apagado.
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1);
        write_private_atomic(&p, b"v3").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"v3");
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1);
    }

    #[test]
    fn write_private_corrige_permissao_ao_sobrescrever() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x");
        fs::write(&p, "a").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&p, b"b").unwrap();
        assert_eq!(mode(&p), 0o600);
        assert_eq!(fs::read_to_string(&p).unwrap(), "b");
    }
}
