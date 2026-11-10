//! Arquivos compartilhados pelo agente na sessão (`share_file` do MCP,
//! #167). Só o registro: quem valida o caminho é `ct-sessions::files`.

use crate::{Ledger, Result};
use ct_core::now_ms;
use rusqlite::{params, Row};
use serde::Serialize;

/// Um arquivo compartilhado, como a UI lista no painel da sessão.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SharedFile {
    pub session_id: String,
    /// Caminho absoluto e canônico no ambiente.
    pub path: String,
    /// Título dado pelo agente; `None` = a UI mostra o nome do arquivo.
    pub title: Option<String>,
    /// Último compartilhamento (ms); compartilhar de novo atualiza.
    pub shared_at: i64,
}

fn row_to_shared(r: &Row) -> rusqlite::Result<SharedFile> {
    Ok(SharedFile {
        session_id: r.get(0)?,
        path: r.get(1)?,
        title: r.get(2)?,
        shared_at: r.get(3)?,
    })
}

impl Ledger {
    /// Registra (ou atualiza o título e a hora de) `path` na sessão.
    pub fn share_file(
        &self,
        session_id: &str,
        path: &str,
        title: Option<&str>,
    ) -> Result<SharedFile> {
        self.get_session(session_id)?;
        let title = title.map(str::trim).filter(|t| !t.is_empty());
        // Hora estritamente crescente na sessão: dois compartilhamentos no
        // mesmo milissegundo ainda saem na ordem em que vieram.
        let last: Option<i64> = self.conn.query_row(
            "SELECT MAX(shared_at) FROM shared_file WHERE session_id = ?1",
            [session_id],
            |r| r.get(0),
        )?;
        let at = last.map_or(now_ms(), |l| now_ms().max(l + 1));
        self.conn.execute(
            "INSERT INTO shared_file (session_id, path, title, shared_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (session_id, path) DO UPDATE SET title = excluded.title,
                                                          shared_at = excluded.shared_at",
            params![session_id, path, title, at],
        )?;
        Ok(self.conn.query_row(
            "SELECT session_id, path, title, shared_at FROM shared_file
             WHERE session_id = ?1 AND path = ?2",
            params![session_id, path],
            row_to_shared,
        )?)
    }

    /// Arquivos compartilhados na sessão, do mais recente ao mais antigo.
    pub fn list_shared_files(&self, session_id: &str) -> Result<Vec<SharedFile>> {
        let mut stmt = self.conn.prepare(
            "SELECT session_id, path, title, shared_at FROM shared_file
             WHERE session_id = ?1 ORDER BY shared_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([session_id], row_to_shared)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::ledger_with_project;
    use crate::{Ledger, NewSession};
    use ct_core::{SessionMode, Tool, WorkItemKind};

    fn sessao(l: &Ledger, project_id: i64) -> String {
        let wi = l
            .ensure_work_item(project_id, 1, WorkItemKind::Issue)
            .unwrap();
        l.create_session(&NewSession {
            work_item_id: wi.id,
            tool: Tool::Claude,
            cwd: "/w/demo".into(),
            launch_cmd: vec![],
            model: None,
            mode: SessionMode::Native,
        })
        .unwrap()
        .id
    }

    #[test]
    fn compartilhar_de_novo_atualiza_sem_duplicar() {
        let (l, p) = ledger_with_project();
        let s = sessao(&l, p.id);
        l.share_file(&s, "/w/a.md", Some("Análise")).unwrap();
        l.share_file(&s, "/w/b.md", None).unwrap();
        let again = l.share_file(&s, "/w/a.md", Some("  ")).unwrap();
        assert_eq!(again.title, None);
        let list = l.list_shared_files(&s).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].path, "/w/a.md", "o mais recente primeiro");
    }

    #[test]
    fn sessao_desconhecida_e_erro() {
        let (l, _) = ledger_with_project();
        assert!(l.share_file("nada", "/w/a.md", None).is_err());
        assert!(l.list_shared_files("nada").unwrap().is_empty());
    }
}
