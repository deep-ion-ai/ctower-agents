//! Chaves age (X25519) e a cifra/decifra de bytes, sem formato: o store usa
//! estas funções e o `ctowerd` pode usá-las no cache do ambiente.

use crate::{Result, SecretsError};
use age::secrecy::ExposeSecret;
use age::x25519::{Identity, Recipient};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use zeroize::Zeroizing;

/// Primeira linha útil de um arquivo de chave no formato do age (linhas
/// vazias e comentários `#` são ignorados).
fn key_line(text: &str) -> Option<&str> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
}

/// Lê uma identidade colada como texto (ex.: a chave de recuperação, #169):
/// aceita a linha `AGE-SECRET-KEY-1...` sozinha ou um arquivo de chave
/// inteiro, com comentários. O erro é só o motivo, nunca o texto recebido.
pub fn parse_identity(text: &str) -> std::result::Result<Identity, &'static str> {
    key_line(text).ok_or("texto vazio")?.parse::<Identity>()
}

/// Texto da identidade (`AGE-SECRET-KEY-1...`), zerado no `drop`. Para
/// mostrar uma chave que fica fora da máquina (a de recuperação, #169).
pub fn identity_text(identity: &Identity) -> Zeroizing<String> {
    Zeroizing::new(identity.to_string().expose_secret().to_string())
}

/// Lê a identidade (`AGE-SECRET-KEY-1...`) de `path`.
pub fn read_identity(path: &Path) -> Result<Identity> {
    let text = Zeroizing::new(fs::read_to_string(path).map_err(|e| SecretsError::io(path, e))?);
    let line = key_line(&text).ok_or(SecretsError::InvalidKey {
        path: path.into(),
        reason: "arquivo vazio",
    })?;
    line.parse::<Identity>()
        .map_err(|reason| SecretsError::InvalidKey {
            path: path.into(),
            reason,
        })
}

/// Lê a identidade de `path` ou, se o arquivo não existe, gera uma nova e a
/// grava com 0600 (escrita atômica). Um arquivo existente com permissão
/// mais aberta volta para 0600.
pub fn ensure_identity(path: &Path) -> Result<Identity> {
    match fs::metadata(path) {
        Ok(meta) => {
            if meta.permissions().mode() & 0o077 != 0 {
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                    .map_err(|e| SecretsError::io(path, e))?;
            }
            return read_identity(path);
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(SecretsError::io(path, e)),
    }
    let identity = Identity::generate();
    write_identity(
        path,
        &identity,
        "chave do store de secrets do Control Tower",
    )?;
    Ok(identity)
}

/// Grava `identity` em `path` no formato de arquivo de chave do age, com
/// 0600 e escrita atômica (cria o diretório, 0700, se faltar). `title` vira
/// o comentário da primeira linha.
pub fn write_identity(path: &Path, identity: &Identity, title: &str) -> Result<()> {
    let contents = Zeroizing::new(format!(
        "# {title} (age X25519)\n# pública: {}\n{}\n",
        identity.to_public(),
        identity.to_string().expose_secret()
    ));
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        ct_core::files::private_dir(dir).map_err(|e| SecretsError::io(dir, e))?;
    }
    ct_core::files::write_private_atomic(path, contents.as_bytes())
        .map_err(|e| SecretsError::io(path, e))
}

/// Lê a chave pública (`age1...`) de `path`; `None` se o arquivo não existe.
pub fn read_recipient(path: &Path) -> Result<Option<Recipient>> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(SecretsError::io(path, e)),
    };
    let line = key_line(&text).ok_or(SecretsError::InvalidKey {
        path: path.into(),
        reason: "arquivo vazio",
    })?;
    line.parse::<Recipient>()
        .map(Some)
        .map_err(|reason| SecretsError::InvalidKey {
            path: path.into(),
            reason,
        })
}

/// Cifra `plaintext` para todos os `recipients` (formato binário do age).
pub fn encrypt(plaintext: &[u8], recipients: &[Recipient]) -> Result<Vec<u8>> {
    let encryptor =
        age::Encryptor::with_recipients(recipients.iter().map(|r| r as &dyn age::Recipient))
            .map_err(|e| SecretsError::Encrypt(e.to_string()))?;
    let mut out = Vec::with_capacity(plaintext.len() + 256);
    let enc_err = |e: io::Error| SecretsError::Encrypt(e.to_string());
    let mut w = encryptor.wrap_output(&mut out).map_err(enc_err)?;
    w.write_all(plaintext).map_err(enc_err)?;
    w.finish().map_err(enc_err)?;
    Ok(out)
}

/// Decifra `ciphertext` com `identity`. O plaintext é zerado no `drop`.
pub fn decrypt(ciphertext: &[u8], identity: &Identity) -> Result<Zeroizing<Vec<u8>>> {
    let decryptor = age::Decryptor::new_buffered(ciphertext)
        .map_err(|e| SecretsError::Decrypt(e.to_string()))?;
    let mut r = decryptor
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(|e| SecretsError::Decrypt(e.to_string()))?;
    let mut out = Zeroizing::new(Vec::new());
    r.read_to_end(&mut out)
        .map_err(|e| SecretsError::Decrypt(e.to_string()))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn identidade_e_gerada_uma_vez_com_0600() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("cfg/secrets.key");
        let a = ensure_identity(&p).unwrap();
        assert_eq!(mode(&p), 0o600);
        let text = fs::read_to_string(&p).unwrap();
        assert!(text.contains("AGE-SECRET-KEY-1"), "formato padrão do age");
        let b = ensure_identity(&p).unwrap();
        assert_eq!(a.to_public(), b.to_public());

        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        ensure_identity(&p).unwrap();
        assert_eq!(mode(&p), 0o600);
    }

    #[test]
    fn chave_ruim_nao_vaza_conteudo() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("secrets.key");
        fs::write(&p, "AGE-SECRET-KEY-1NAOVALE\n").unwrap();
        let e = read_identity(&p).err().unwrap().to_string();
        assert!(e.contains("chave inválida"), "{e}");
        assert!(!e.contains("NAOVALE"), "{e}");
    }

    #[test]
    fn recipient_ausente_e_none() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("secrets.recovery.pub");
        assert!(read_recipient(&p).unwrap().is_none());
        let id = Identity::generate();
        fs::write(&p, format!("# recuperação\n{}\n", id.to_public())).unwrap();
        assert_eq!(read_recipient(&p).unwrap(), Some(id.to_public()));
        fs::write(&p, "lixo\n").unwrap();
        assert!(read_recipient(&p).is_err());
    }

    #[test]
    fn identidade_colada() {
        let id = Identity::generate();
        let chave = id.to_string();
        let chave = chave.expose_secret();
        assert_eq!(parse_identity(chave).unwrap().to_public(), id.to_public());
        let arquivo = format!(
            "# recuperação\n# pública: {}\n  {chave}  \n",
            id.to_public()
        );
        assert_eq!(
            parse_identity(&arquivo).unwrap().to_public(),
            id.to_public()
        );
        assert_eq!(identity_text(&id).as_str(), chave);
        assert!(parse_identity("").is_err());
        let e = parse_identity("AGE-SECRET-KEY-1NAOVALE").err().unwrap();
        assert!(!e.contains("NAOVALE"), "{e}");
    }

    #[test]
    fn cifra_e_decifra_bytes() {
        let a = Identity::generate();
        let b = Identity::generate();
        let c = encrypt(b"oi", &[a.to_public(), b.to_public()]).unwrap();
        assert_eq!(&decrypt(&c, &a).unwrap()[..], b"oi");
        assert_eq!(&decrypt(&c, &b).unwrap()[..], b"oi");
        assert!(decrypt(&c, &Identity::generate()).is_err());
        assert!(encrypt(b"oi", &[]).is_err());
    }
}
