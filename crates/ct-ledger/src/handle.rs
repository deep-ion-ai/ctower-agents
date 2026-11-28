//! Ledger compartilhado entre o ciclo de vida das sessões e o sync (#242):
//! uma conexão de escrita atrás de um lock e um pool pequeno de conexões
//! somente leitura (#244), com clone barato.
//!
//! No WAL, uma leitura não espera a escrita: as consultas da UI e do MCP vão
//! pelo pool ([`LedgerHandle::read`]) e não ficam na fila de uma escrita
//! longa (sync, limpeza). O preço é ler o último commit, sem o que a escrita
//! em andamento ainda não gravou, o que a UI aceita porque é orientada a
//! push.

use crate::Ledger;
use ct_core::Lock;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

/// Conexões somente leitura do pool do ambiente: 2 a 4 bastam para a UI, o
/// MCP e as varreduras da limpeza, sem multiplicar descritores.
pub const READ_POOL_SIZE: usize = 3;

/// Acesso compartilhado ao ledger. Clonar compartilha as mesmas conexões.
#[derive(Clone)]
pub struct LedgerHandle(Arc<Shared>);

struct Shared {
    writer: Lock<Ledger>,
    /// `None` no ledger em memória (sem arquivo para outra conexão abrir) ou
    /// com o pool desligado: [`LedgerHandle::read`] usa o escritor.
    readers: Option<Pool>,
}

struct Pool {
    path: PathBuf,
    env_id: String,
    /// Conexões mantidas abertas. Com todas emprestadas, a próxima leitura
    /// abre uma extra, fechada na devolução: ninguém espera o pool, e uma
    /// leitura aninhada na mesma thread não trava.
    size: usize,
    state: Mutex<PoolState>,
}

#[derive(Default)]
struct PoolState {
    idle: Vec<Ledger>,
    /// Conexões abertas, ociosas ou emprestadas (inclusive as extras).
    open: usize,
}

impl Pool {
    fn state(&self) -> MutexGuard<'_, PoolState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Uma conexão ociosa ou uma nova; `None` se o pool não consegue abrir.
    fn acquire(&self) -> Option<Ledger> {
        {
            let mut st = self.state();
            if let Some(l) = st.idle.pop() {
                return Some(l);
            }
            st.open += 1;
        }
        match Ledger::open_reader(&self.path, &self.env_id) {
            Ok(l) => Some(l),
            Err(e) => {
                // Falha passageira (limite de arquivos, lock momentâneo): só
                // esta leitura vai pelo escritor; a próxima tenta de novo.
                self.state().open -= 1;
                tracing::warn!("ledger: conexão de leitura não abriu, usando o escritor: {e}");
                None
            }
        }
    }

    /// Devolve a conexão; a extra, acima de `size`, é fechada.
    fn release(&self, l: Ledger) {
        let mut st = self.state();
        if st.open > self.size {
            st.open -= 1;
            drop(st);
            drop(l);
        } else {
            st.idle.push(l);
        }
    }
}

impl LedgerHandle {
    /// Handle com o pool de leitura de [`READ_POOL_SIZE`] conexões quando o
    /// ledger tem arquivo ([`Ledger::open`]).
    pub fn new(ledger: Ledger) -> Self {
        Self::with_read_pool(ledger, READ_POOL_SIZE)
    }

    /// Como [`LedgerHandle::new`], com `size` conexões de leitura; `0`
    /// desliga o pool.
    pub fn with_read_pool(ledger: Ledger, size: usize) -> Self {
        let readers = match ledger.path() {
            Some(path) if size > 0 => Some(Pool {
                path: path.to_path_buf(),
                env_id: ledger.env_id().to_string(),
                size,
                state: Mutex::default(),
            }),
            _ => None,
        };
        LedgerHandle(Arc::new(Shared {
            writer: Lock::new(ledger),
            readers,
        }))
    }

    /// Trava a conexão de escrita. Uma thread que entrou em pânico com ela
    /// travada não a deixa inutilizável: o lock envenenado é recuperado.
    /// Use para escrever e para ler o que a mesma operação vai escrever
    /// (ler, decidir e gravar sob o mesmo lock).
    pub fn lock(&self) -> MutexGuard<'_, Ledger> {
        self.0.writer.lock()
    }

    /// Conexão de leitura do pool, para consultas que não escrevem: não
    /// espera a conexão de escrita. Escrever por ela falha
    /// (`SQLITE_READONLY`). Sem pool (ledger em memória, ou abrir a conexão
    /// falhou nesta chamada), usa o escritor: não chame com [`LedgerHandle::lock`] na mão
    /// na mesma thread.
    pub fn read(&self) -> LedgerRead<'_> {
        if let Some(pool) = &self.0.readers {
            if let Some(l) = pool.acquire() {
                return LedgerRead(ReadInner::Pooled(Some(l), pool));
            }
        }
        LedgerRead(ReadInner::Writer(self.lock()))
    }
}

/// Ledger emprestado por [`LedgerHandle::read`]; volta ao pool no drop.
pub struct LedgerRead<'a>(ReadInner<'a>);

enum ReadInner<'a> {
    Pooled(Option<Ledger>, &'a Pool),
    Writer(MutexGuard<'a, Ledger>),
}

impl Deref for LedgerRead<'_> {
    type Target = Ledger;

    fn deref(&self) -> &Ledger {
        match &self.0 {
            ReadInner::Pooled(l, _) => l.as_ref().expect("conexão devolvida antes do drop"),
            ReadInner::Writer(g) => g,
        }
    }
}

impl Drop for LedgerRead<'_> {
    fn drop(&mut self) {
        if let ReadInner::Pooled(l, pool) = &mut self.0 {
            if let Some(l) = l.take() {
                pool.release(l);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NewProject;
    use std::time::{Duration, Instant};

    fn novo(name: &str) -> NewProject {
        NewProject {
            name: name.into(),
            gh_owner: "acme".into(),
            gh_repo: name.into(),
            repo_path: format!("/tmp/{name}"),
            base_branch: "develop".into(),
            protected_branches: vec!["develop".into()],
        }
    }

    fn handle(dir: &tempfile::TempDir, size: usize) -> LedgerHandle {
        let l = Ledger::open(&dir.path().join("ledger.db"), "env").unwrap();
        LedgerHandle::with_read_pool(l, size)
    }

    /// Critério de aceite do #244: com uma escrita longa segurando a conexão
    /// de escrita (transação aberta), a consulta pelo pool responde na hora,
    /// com o último commit.
    #[test]
    fn leitura_nao_espera_escrita_longa() {
        let dir = tempfile::tempdir().unwrap();
        let h = handle(&dir, READ_POOL_SIZE);
        h.lock().add_project(&novo("acme-web")).unwrap();

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let writer = {
            let h = h.clone();
            std::thread::spawn(move || {
                let l = h.lock();
                l.conn().execute_batch("BEGIN IMMEDIATE").unwrap();
                l.add_project(&novo("acme-api")).unwrap();
                started_tx.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(800));
                l.conn().execute_batch("COMMIT").unwrap();
            })
        };
        started_rx.recv().unwrap();

        let t = Instant::now();
        let names: Vec<_> = h
            .read()
            .list_projects()
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        let took = t.elapsed();
        // O pool lê o último commit, sem a escrita em andamento.
        assert_eq!(names, vec!["acme-web"]);
        assert!(
            took < Duration::from_millis(400),
            "leitura esperou {took:?}"
        );

        writer.join().unwrap();
        assert_eq!(h.read().list_projects().unwrap().len(), 2);
    }

    #[test]
    fn conexao_de_leitura_nao_escreve() {
        let dir = tempfile::tempdir().unwrap();
        let h = handle(&dir, 1);
        let err = h.read().add_project(&novo("acme-web")).unwrap_err();
        assert!(err.to_string().contains("readonly"), "{err}");
        assert!(h.lock().list_projects().unwrap().is_empty());
    }

    #[test]
    fn pool_reusa_as_conexoes_e_fecha_as_extras() {
        let dir = tempfile::tempdir().unwrap();
        let h = handle(&dir, 2);
        let pool = h.0.readers.as_ref().unwrap();
        {
            // Leituras aninhadas além do tamanho do pool não esperam.
            let (a, b, c) = (h.read(), h.read(), h.read());
            assert_eq!(pool.state().open, 3);
            assert_eq!(a.list_projects().unwrap().len(), 0);
            drop((b, c));
        }
        let st = pool.state();
        assert_eq!((st.open, st.idle.len()), (2, 2));
        drop(st);
        let _again = h.read();
        assert_eq!(pool.state().open, 2);
    }

    #[test]
    fn em_memoria_le_pelo_escritor() {
        let h = LedgerHandle::new(Ledger::open_in_memory().unwrap());
        assert!(h.0.readers.is_none());
        h.read().add_project(&novo("acme-web")).unwrap();
        assert_eq!(h.read().list_projects().unwrap().len(), 1);
    }

    #[test]
    fn panico_com_o_escritor_nao_derruba_o_handle() {
        let dir = tempfile::tempdir().unwrap();
        let h = handle(&dir, 1);
        let h2 = h.clone();
        let r = std::thread::spawn(move || {
            let _l = h2.lock();
            let _r = h2.read();
            panic!("pânico com o ledger");
        })
        .join();
        assert!(r.is_err());
        h.lock().add_project(&novo("acme-web")).unwrap();
        assert_eq!(h.read().list_projects().unwrap().len(), 1);
    }
}
