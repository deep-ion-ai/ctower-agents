//! Mutex tolerante a poison (#244).
//!
//! Com `std::sync::Mutex`, uma thread que entra em pânico com o lock na mão
//! o envenena, e todo `lock().unwrap()` seguinte entra em pânico também: um
//! erro numa sessão derruba as operações das outras. O estado guardado aqui
//! (mapas de sessões vivas, máscaras, tokens) continua válido depois de um
//! pânico no meio de uma operação, então o lock envenenado é recuperado.

use std::sync::{Mutex, MutexGuard};

/// `Mutex` cujo [`Lock::lock`] nunca falha por poison.
#[derive(Debug, Default)]
pub struct Lock<T>(Mutex<T>);

impl<T> Lock<T> {
    pub const fn new(value: T) -> Self {
        Lock(Mutex::new(value))
    }

    /// Trava. Um lock envenenado por pânico em outra thread é recuperado.
    pub fn lock(&self) -> MutexGuard<'_, T> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Tenta travar sem esperar; `None` se outra thread está com o lock.
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        match self.0.try_lock() {
            Ok(g) => Some(g),
            Err(std::sync::TryLockError::Poisoned(e)) => Some(e.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }

    pub fn into_inner(self) -> T {
        self.0.into_inner().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn panico_com_o_lock_nao_derruba_os_proximos() {
        let l = Arc::new(Lock::new(vec![1]));
        let l2 = Arc::clone(&l);
        let r = std::thread::spawn(move || {
            let mut g = l2.lock();
            g.push(2);
            panic!("pânico com o lock");
        })
        .join();
        assert!(r.is_err());
        l.lock().push(3);
        assert_eq!(*l.lock(), vec![1, 2, 3]);
        assert!(l.try_lock().is_some());
    }
}
