//! Ledger compartilhado entre o ciclo de vida das sessões e o sync (#242):
//! o mesmo `Ledger` atrás de um `Mutex`, com clone barato.

use crate::Ledger;
use std::sync::{Arc, Mutex, MutexGuard};

/// Acesso compartilhado ao ledger. Clonar compartilha a mesma conexão.
#[derive(Clone)]
pub struct LedgerHandle(Arc<Mutex<Ledger>>);

impl LedgerHandle {
    pub fn new(ledger: Ledger) -> Self {
        LedgerHandle(Arc::new(Mutex::new(ledger)))
    }

    /// Trava o ledger. Uma thread que entrou em pânico com ele travado não
    /// o deixa inutilizável: o lock envenenado é recuperado.
    pub fn lock(&self) -> MutexGuard<'_, Ledger> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}
