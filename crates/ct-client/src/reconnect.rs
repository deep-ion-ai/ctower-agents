//! Espera entre tentativas de conexão (spec §12): começa em `min` e dobra a
//! cada falha seguida até `max`.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    pub min: Duration,
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff {
            min: Duration::from_secs(1),
            max: Duration::from_secs(30),
        }
    }
}

impl Backoff {
    /// Espera antes da próxima tentativa, após `failures` falhas seguidas
    /// (0 = a conexão acabou de cair depois de funcionar).
    pub fn delay(&self, failures: u32) -> Duration {
        let factor = 1u32.checked_shl(failures).unwrap_or(u32::MAX);
        self.min.saturating_mul(factor).min(self.max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dobra_ate_o_teto() {
        let b = Backoff::default();
        let s = |n| Duration::from_secs(n);
        let got: Vec<_> = (0..8).map(|f| b.delay(f)).collect();
        assert_eq!(got, [s(1), s(2), s(4), s(8), s(16), s(30), s(30), s(30)]);
        assert_eq!(b.delay(u32::MAX), s(30));
    }
}
