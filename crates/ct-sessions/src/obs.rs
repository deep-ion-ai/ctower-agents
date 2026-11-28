//! Spans de observabilidade (#243): `session{id, project, item}`.
//!
//! O `launch`, os hooks e o MCP abrem o mesmo span, para filtrar nos logs
//! tudo o que aconteceu com uma sessão pelo id (`session.id` no arquivo de
//! log da UI, `span.id` no JSON).

use crate::Sessions;
use tracing::Span;

/// Span `session` com os três campos. `project` é `owner/repo` e `item` o
/// número da issue (`0` na sessão do projeto).
pub fn session_span(id: &str, project: &str, item: i64) -> Span {
    tracing::info_span!("session", id, project, item)
}

impl Sessions {
    /// [`session_span`] da sessão `session_id`, com projeto e item lidos do
    /// ledger. Sessão ou work item desconhecido: o span leva só o id (os
    /// outros campos ficam vazios), para o log ainda correlacionar.
    pub fn session_span(&self, session_id: &str) -> Span {
        let l = self.ledger_read();
        let found = l.get_session(session_id).ok().and_then(|s| {
            let wi = l.get_work_item(s.work_item_id).ok()?;
            let p = l.get_project(wi.project_id).ok()?;
            Some((format!("{}/{}", p.gh_owner, p.gh_repo), wi.gh_number))
        });
        match found {
            Some((project, item)) => session_span(session_id, &project, item),
            None => tracing::info_span!(
                "session",
                id = session_id,
                project = tracing::field::Empty,
                item = tracing::field::Empty
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::{FakeClaude, TestEnv};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Texto que o `tracing` escreve para `f`.
    fn log_of(f: impl FnOnce()) -> String {
        let out = Captured::default();
        let w = out.clone();
        let sub = tracing_subscriber::fmt()
            .with_writer(move || w.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(sub, f);
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        text
    }

    #[test]
    fn span_da_sessao_carrega_projeto_e_item() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let s = env.sessions.start(env.request(1, "A")).unwrap();
        let text = log_of(|| {
            env.sessions
                .session_span(&s.id)
                .in_scope(|| tracing::info!("evento"));
        });
        assert!(
            text.contains(&format!("session{{id=\"{}\"", s.id)),
            "{text}"
        );
        assert!(
            text.contains("project=\"") && text.contains("item=1"),
            "{text}"
        );
    }

    #[test]
    fn sessao_desconhecida_ainda_leva_o_id() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let text = log_of(|| {
            env.sessions
                .session_span("nao-existe")
                .in_scope(|| tracing::info!("evento"));
        });
        assert!(text.contains("session{id=\"nao-existe\"}"), "{text}");
    }
}
