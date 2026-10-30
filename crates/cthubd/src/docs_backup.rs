//! Pacote dos docs dos projetos (#442, épico #445): a pasta `docs_dir` do
//! hub é a única cópia dos docs (decisão 269), então um pacote completo
//! `docs-<AAAAMMDD-HHMM>.tar.zst` vai para o `backup_dir` e, cifrado, para o
//! mesmo destino remoto do `hub.db` (veja [`crate::backup`]). Só crates Rust
//! (`tar` + `zstd`): nada de restic, rclone ou outra ferramenta (decisão
//! 261).
//!
//! A restauração ([`extract`]) sempre vai para uma pasta nova ou vazia,
//! nunca para o `docs_dir`, e recusa entradas com `..` ou caminho absoluto.

use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

pub const PREFIX: &str = "docs-";
pub const EXT: &str = ".tar.zst";
/// Nível do zstd: texto comprime bem já no padrão, sem pesar na CPU do hub.
const LEVEL: i32 = 3;

/// `docs-<AAAAMMDD-HHMM>.tar.zst` para o instante `ms` (UTC).
pub fn package_name(ms: i64) -> String {
    format!("{PREFIX}{}{EXT}", ct_core::utc_stamp(ms))
}

/// Nome de um pacote local (`docs-AAAAMMDD-HHMM.tar.zst`).
pub fn is_package_name(n: &str) -> bool {
    let Some(stamp) = n.strip_prefix(PREFIX).and_then(|r| r.strip_suffix(EXT)) else {
        return false;
    };
    let b = stamp.as_bytes();
    b.len() == 13
        && b[8] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 8 || c.is_ascii_digit())
}

/// Pacotes em `dir`, do mais novo para o mais antigo (o carimbo no nome
/// ordena cronologicamente).
pub fn list_packages(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(is_package_name)
            })
            .collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    out.sort();
    out.reverse();
    Ok(out)
}

/// Apaga os pacotes além dos `keep` mais novos. Devolve os apagados.
pub fn rotate(dir: &Path, keep: usize) -> io::Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    for p in list_packages(dir)?.into_iter().skip(keep.max(1)) {
        std::fs::remove_file(&p)?;
        removed.push(p);
    }
    Ok(removed)
}

/// O que há em `docs_dir`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tree {
    /// A pasta não existe.
    Missing,
    /// Nenhum arquivo (só pastas vazias, ou nada).
    Empty,
    /// `bytes` = soma dos arquivos regulares.
    Files { files: usize, bytes: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Dir,
    File,
    Symlink,
}

/// Entradas de `root` (relativas, ordenadas, pastas antes do conteúdo),
/// sem seguir links simbólicos. `skip` (o `backup_dir`, se estiver dentro
/// dos docs) fica de fora; arquivos especiais (socket, fifo) também. Uma
/// entrada que some no meio da leitura é ignorada.
fn walk(root: &Path, skip: Option<&Path>) -> io::Result<Vec<(PathBuf, u64, Kind)>> {
    fn rec(
        root: &Path,
        rel: &Path,
        skip: Option<&Path>,
        out: &mut Vec<(PathBuf, u64, Kind)>,
    ) -> io::Result<()> {
        let dir = root.join(rel);
        let mut names: Vec<_> = match std::fs::read_dir(&dir) {
            Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.file_name())).collect(),
            Err(e) if e.kind() == io::ErrorKind::NotFound && !rel.as_os_str().is_empty() => {
                return Ok(())
            }
            Err(e) => return Err(e),
        };
        names.sort();
        for n in names {
            let r = rel.join(&n);
            let full = root.join(&r);
            if skip.is_some_and(|s| s == full) {
                continue;
            }
            let m = match std::fs::symlink_metadata(&full) {
                Ok(m) => m,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            let t = m.file_type();
            if t.is_symlink() {
                out.push((r, 0, Kind::Symlink));
            } else if t.is_dir() {
                out.push((r.clone(), 0, Kind::Dir));
                rec(root, &r, skip, out)?;
            } else if t.is_file() {
                out.push((r, m.len(), Kind::File));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    rec(root, Path::new(""), skip, &mut out)?;
    Ok(out)
}

/// `skip` só vale se estiver dentro de `root` (o `backup_dir` dentro dos
/// docs não entra no pacote, senão cada pacote levaria os anteriores).
fn skip_inside(root: &Path, skip: &Path) -> Option<PathBuf> {
    let (r, s) = (root.canonicalize().ok()?, skip.canonicalize().ok()?);
    s.starts_with(&r)
        .then(|| root.join(s.strip_prefix(&r).unwrap_or(Path::new(""))))
}

/// Mede `docs_dir` sem ler o conteúdo dos arquivos.
pub fn measure(root: &Path, skip: &Path) -> io::Result<Tree> {
    if !root.is_dir() {
        return Ok(Tree::Missing);
    }
    let skip = skip_inside(root, skip);
    let entries = walk(root, skip.as_deref())?;
    let files = entries.iter().filter(|e| e.2 != Kind::Dir).count();
    if files == 0 {
        return Ok(Tree::Empty);
    }
    Ok(Tree::Files {
        files,
        bytes: entries.iter().map(|e| e.1).sum(),
    })
}

/// Gera o pacote de `root` inteiro em `out` (0600, via arquivo temporário
/// e `rename`). Devolve o tamanho do pacote.
pub fn pack(root: &Path, skip: &Path, out: &Path) -> Result<u64, String> {
    let tmp = PathBuf::from(format!("{}.tmp", out.display()));
    let r = (|| -> io::Result<u64> {
        let skip = skip_inside(root, skip);
        let f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        let enc = zstd::Encoder::new(f, LEVEL)?;
        let mut b = tar::Builder::new(enc);
        b.follow_symlinks(false);
        b.mode(tar::HeaderMode::Complete);
        for (rel, _, kind) in walk(root, skip.as_deref())? {
            let full = root.join(&rel);
            let r = match kind {
                Kind::Dir => b.append_dir(&rel, &full),
                Kind::File | Kind::Symlink => b.append_path_with_name(&full, &rel),
            };
            match r {
                Ok(()) => {}
                // Apagado entre a listagem e a leitura: fica de fora.
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(io::Error::new(e.kind(), format!("{}: {e}", rel.display()))),
            }
        }
        let mut f = b.into_inner()?.finish()?;
        f.flush()?;
        f.sync_all()?;
        let size = f.metadata()?.len();
        std::fs::rename(&tmp, out)?;
        Ok(size)
    })();
    r.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("pacote dos docs: {e}")
    })
}

/// Caminho de uma entrada do pacote: só componentes normais (nada de
/// `..`, raiz ou prefixo).
fn safe_entry_path(p: &Path) -> Result<(), String> {
    for c in p.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => {
                return Err(format!(
                    "pacote recusado: entrada fora da pasta de destino ({})",
                    p.display()
                ))
            }
        }
    }
    Ok(())
}

/// Extrai o pacote (`.tar.zst` já decifrado) em `to`, que precisa existir.
/// Recusa entradas com `..` ou absolutas e tipos além de pasta, arquivo e
/// link simbólico; o `unpack_in` do `tar` ainda confere que nada sai de
/// `to` por um link. Devolve quantos arquivos e links foram extraídos.
pub fn extract(data: impl Read, to: &Path) -> Result<usize, String> {
    let err = |e: io::Error| format!("pacote dos docs ilegível: {e}");
    let dec = zstd::Decoder::new(data).map_err(err)?;
    let mut a = tar::Archive::new(dec);
    a.set_preserve_permissions(false);
    a.set_overwrite(false);
    let mut n = 0;
    for e in a.entries().map_err(err)? {
        let mut e = e.map_err(err)?;
        let path = e.path().map_err(err)?.into_owned();
        safe_entry_path(&path)?;
        let t = e.header().entry_type();
        if t.is_pax_global_extensions() || t.is_pax_local_extensions() {
            continue;
        }
        if !(t.is_dir() || t.is_file() || t.is_symlink()) {
            return Err(format!(
                "pacote recusado: tipo de entrada não suportado em {}",
                path.display()
            ));
        }
        if !e.unpack_in(to).map_err(err)? {
            return Err(format!(
                "pacote recusado: entrada fora da pasta de destino ({})",
                path.display()
            ));
        }
        if !t.is_dir() {
            n += 1;
        }
    }
    Ok(n)
}

/// Caminho absoluto de `p`, sem `.`/`..` (resolvidos no texto), com os
/// links resolvidos no ancestral mais próximo que existe (o resto pode
/// ainda não existir).
fn resolve(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    let mut p = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::ParentDir => {
                p.pop();
            }
            Component::CurDir => {}
            c => p.push(c),
        }
    }
    let mut rest = Vec::new();
    let mut cur = p.as_path();
    loop {
        if let Ok(c) = cur.canonicalize() {
            return rest.iter().rev().fold(c, |acc, n| acc.join(n));
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                cur = parent;
            }
            _ => return p,
        }
    }
}

/// Confere e prepara a pasta de destino da restauração: nova ou vazia, e
/// nunca o `docs_dir` nem algo dentro dele. Devolve se a pasta foi criada
/// aqui (para limpar numa falha).
pub fn prepare_target(to: &Path, docs_dir: Option<&Path>) -> Result<bool, String> {
    let abs = resolve(to);
    if let Some(d) = docs_dir {
        let d = resolve(d);
        if abs.starts_with(&d) {
            return Err(format!(
                "{} fica no docs_dir ({}): restaure em outra pasta e copie de lá \
                 só o que precisar",
                to.display(),
                d.display()
            ));
        }
    }
    match std::fs::read_dir(to) {
        Ok(mut rd) => {
            if rd.next().is_some() {
                return Err(format!(
                    "{} não está vazia: restaure numa pasta nova ou vazia",
                    to.display()
                ));
            }
            Ok(false)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            ct_core::files::private_dir(to).map_err(|e| format!("{}: {e}", to.display()))?;
            Ok(true)
        }
        Err(e) => Err(format!("{}: {e}", to.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arvore(root: &Path) {
        std::fs::create_dir_all(root.join("acme__app/_projeto/specs")).unwrap();
        std::fs::create_dir_all(root.join("acme__app/epicos/12-busca")).unwrap();
        std::fs::create_dir_all(root.join("acme__app/vazia")).unwrap();
        std::fs::write(root.join("acme__app/_projeto/specs/spec.md"), "# Spec\n").unwrap();
        std::fs::write(root.join("acme__app/epicos/12-busca/README.md"), "épico 12").unwrap();
        std::os::unix::fs::symlink("_projeto", root.join("acme__app/atalho")).unwrap();
    }

    #[test]
    fn nomes_lista_e_rotacao() {
        assert_eq!(package_name(0), "docs-19700101-0000.tar.zst");
        assert!(is_package_name("docs-20261029-1430.tar.zst"));
        assert!(!is_package_name("docs-20261029-1430.tar.zst.tmp"));
        assert!(!is_package_name("hub-20261029-1430.db"));
        let d = tempfile::tempdir().unwrap();
        for i in 0..9 {
            std::fs::write(d.path().join(format!("docs-2026102{i}-0000.tar.zst")), "x").unwrap();
        }
        std::fs::write(d.path().join("hub-20261020-0000.db"), "x").unwrap();
        let removed = rotate(d.path(), 7).unwrap();
        assert_eq!(removed.len(), 2);
        let left = list_packages(d.path()).unwrap();
        assert_eq!(left.len(), 7);
        assert!(left[0].ends_with("docs-20261028-0000.tar.zst"));
        // O snapshot do hub.db não é da conta desta rotação.
        assert!(d.path().join("hub-20261020-0000.db").exists());
    }

    #[test]
    fn pacote_com_a_arvore_inteira_e_extracao() {
        let d = tempfile::tempdir().unwrap();
        let docs = d.path().join("docs");
        arvore(&docs);
        // backup_dir dentro dos docs: fica de fora do pacote.
        let bk = docs.join("backups");
        std::fs::create_dir_all(&bk).unwrap();
        std::fs::write(bk.join("docs-20261001-0000.tar.zst"), "antigo").unwrap();
        assert_eq!(
            measure(&docs, &bk).unwrap(),
            Tree::Files {
                files: 3,
                bytes: 7 + "épico 12".len() as u64
            }
        );
        let out = bk.join(package_name(1_700_000_000_000));
        let size = pack(&docs, &bk, &out).unwrap();
        assert!(size > 0 && out.exists());
        assert!(!PathBuf::from(format!("{}.tmp", out.display())).exists());

        let to = d.path().join("restaurado");
        assert!(prepare_target(&to, Some(&docs)).unwrap());
        let n = extract(std::fs::File::open(&out).unwrap(), &to).unwrap();
        assert_eq!(n, 3);
        assert_eq!(
            std::fs::read_to_string(to.join("acme__app/_projeto/specs/spec.md")).unwrap(),
            "# Spec\n"
        );
        assert!(to.join("acme__app/vazia").is_dir());
        assert_eq!(
            std::fs::read_link(to.join("acme__app/atalho")).unwrap(),
            PathBuf::from("_projeto")
        );
        assert!(!to.join("backups").exists());
    }

    #[test]
    fn vazio_e_inexistente() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            measure(&d.path().join("nada"), d.path()).unwrap(),
            Tree::Missing
        );
        std::fs::create_dir_all(d.path().join("docs/so-pastas")).unwrap();
        assert_eq!(
            measure(&d.path().join("docs"), d.path()).unwrap(),
            Tree::Empty
        );
    }

    #[test]
    fn destino_nunca_e_o_docs_dir_nem_pasta_com_conteudo() {
        let d = tempfile::tempdir().unwrap();
        let docs = d.path().join("docs");
        arvore(&docs);
        assert!(prepare_target(&docs, Some(&docs)).is_err());
        assert!(prepare_target(&docs.join("acme__app/novo"), Some(&docs)).is_err());
        // Por um caminho com `..` também.
        let torto = d.path().join("x/../docs/novo");
        assert!(prepare_target(&torto, Some(&docs)).is_err());
        let cheia = d.path().join("cheia");
        std::fs::create_dir_all(&cheia).unwrap();
        std::fs::write(cheia.join("a"), "a").unwrap();
        assert!(prepare_target(&cheia, Some(&docs)).is_err());
        let vazia = d.path().join("vazia");
        std::fs::create_dir_all(&vazia).unwrap();
        assert!(!prepare_target(&vazia, Some(&docs)).unwrap());
    }

    /// Pacote montado à mão com `../` no caminho (o `tar::Builder` não
    /// deixa): a extração recusa e nada é escrito fora do destino.
    #[test]
    fn extracao_recusa_travessia_de_caminho() {
        fn com_caminho(nome: &[u8]) -> Vec<u8> {
            let mut h = tar::Header::new_gnu();
            h.as_old_mut().name[..nome.len()].copy_from_slice(nome);
            h.set_size(4);
            h.set_mode(0o644);
            h.set_entry_type(tar::EntryType::Regular);
            h.set_cksum();
            let mut raw = Vec::new();
            {
                let mut b = tar::Builder::new(&mut raw);
                b.append(&h, &b"mal!"[..]).unwrap();
                b.finish().unwrap();
            }
            zstd::encode_all(&raw[..], 3).unwrap()
        }
        let d = tempfile::tempdir().unwrap();
        let to = d.path().join("to");
        std::fs::create_dir_all(&to).unwrap();
        for nome in [&b"../fora.txt"[..], b"a/../../fora.txt", b"/tmp/abs.txt"] {
            let e = extract(&com_caminho(nome)[..], &to).unwrap_err();
            assert!(e.contains("fora da pasta"), "{e}");
        }
        assert!(!d.path().join("fora.txt").exists());
    }
}
