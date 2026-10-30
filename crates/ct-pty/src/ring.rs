//! Buffer circular de scrollback: guarda apenas os bytes mais recentes.

use std::collections::VecDeque;

pub struct RingBuffer {
    buf: VecDeque<u8>,
    cap: usize,
}

impl RingBuffer {
    pub fn new(cap: usize) -> Self {
        RingBuffer {
            buf: VecDeque::with_capacity(cap.min(64 * 1024)),
            cap,
        }
    }

    /// Acrescenta `bytes`, descartando os mais antigos ao exceder a capacidade.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.cap == 0 {
            return;
        }
        let bytes = &bytes[bytes.len().saturating_sub(self.cap)..];
        let overflow = (self.buf.len() + bytes.len()).saturating_sub(self.cap);
        self.buf.drain(..overflow);
        self.buf.extend(bytes);
    }

    pub fn snapshot(&self) -> Vec<u8> {
        self.buf.iter().copied().collect()
    }

    /// Os últimos `n` bytes (ou todos, se houver menos).
    pub fn tail(&self, n: usize) -> Vec<u8> {
        let skip = self.buf.len().saturating_sub(n);
        self.buf.iter().skip(skip).copied().collect()
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vazio() {
        let r = RingBuffer::new(8);
        assert!(r.is_empty());
        assert_eq!(r.snapshot(), b"");
    }

    #[test]
    fn abaixo_da_capacidade_guarda_tudo() {
        let mut r = RingBuffer::new(8);
        r.push(b"abc");
        r.push(b"de");
        assert_eq!(r.snapshot(), b"abcde");
    }

    #[test]
    fn acima_da_capacidade_mantem_os_mais_recentes() {
        let mut r = RingBuffer::new(5);
        r.push(b"abcd");
        r.push(b"efg");
        assert_eq!(r.snapshot(), b"cdefg");
        assert_eq!(r.len(), 5);
    }

    #[test]
    fn push_unico_maior_que_a_capacidade() {
        let mut r = RingBuffer::new(4);
        r.push(b"xy");
        r.push(b"0123456789");
        assert_eq!(r.snapshot(), b"6789");
    }

    #[test]
    fn tail_devolve_os_mais_recentes() {
        let mut r = RingBuffer::new(8);
        r.push(b"abcdef");
        assert_eq!(r.tail(3), b"def");
        assert_eq!(r.tail(100), b"abcdef");
        assert_eq!(r.tail(0), b"");
    }

    #[test]
    fn capacidade_zero_nao_guarda_nada() {
        let mut r = RingBuffer::new(0);
        r.push(b"abc");
        assert!(r.is_empty());
    }
}
