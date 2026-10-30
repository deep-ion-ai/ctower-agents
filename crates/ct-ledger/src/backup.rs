//! Cópia de segurança de um banco (ledger.db ou hub.db) antes de uma
//! atualização (issue #90): `VACUUM INTO` e `PRAGMA user_version`.

use crate::Result;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

/// `PRAGMA user_version` (versão do esquema) de um banco em disco.
pub fn db_user_version(path: &Path) -> Result<i64> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(super::BUSY_TIMEOUT)?;
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// Grava em `dest` (substituindo) uma cópia consistente de `src`, mesmo com
/// o daemon escrevendo nele. Devolve o `user_version` copiado.
pub fn snapshot_db(src: &Path, dest: &Path) -> Result<i64> {
    match std::fs::remove_file(dest) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(crate::LedgerError::Conflict(e.to_string())),
    }
    let conn = Connection::open_with_flags(src, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(super::BUSY_TIMEOUT)?;
    conn.execute("VACUUM INTO ?1", [dest.to_string_lossy().as_ref()])?;
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_copia_dados_e_versao() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("ledger.db");
        let l = crate::Ledger::open(&db, "env").unwrap();
        l.add_project(&crate::NewProject {
            name: "demo".into(),
            gh_owner: "o".into(),
            gh_repo: "r".into(),
            repo_path: "/x".into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
        })
        .unwrap();
        let dest = d.path().join("bk/ledger.db");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let v = snapshot_db(&db, &dest).unwrap();
        assert_eq!(v, crate::schema::MIGRATIONS.len() as i64);
        assert_eq!(db_user_version(&dest).unwrap(), v);
        // Regravar substitui o anterior.
        assert_eq!(snapshot_db(&db, &dest).unwrap(), v);
        let copy = crate::Ledger::open(&dest, "env").unwrap();
        assert_eq!(copy.list_projects().unwrap().len(), 1);
        assert!(db_user_version(&d.path().join("nao-existe.db")).is_err());
    }
}
