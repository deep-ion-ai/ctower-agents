//! Processos em PTY com scrollback e fan-out da saída para vários assinantes.

mod mask;
mod ring;

pub use mask::{MaskSet, Masker};
pub use ring::RingBuffer;
pub use zeroize::Zeroizing;

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Scrollback padrão por sessão (spec §9).
pub const DEFAULT_SCROLLBACK: usize = 4 * 1024 * 1024;

/// Depois que o processo sai, quanto esperar o leitor drenar a saída restante.
const DRAIN_GRACE: Duration = Duration::from_secs(1);

/// Quanto o mascaramento segura uma cauda que pode ser começo de um valor
/// à espera da próxima leitura. Um valor impresso de uma vez chega em
/// leituras seguidas (microssegundos); passado o prazo, a cauda sai como
/// está, para não congelar a tela (ex.: o eco de uma tecla que por acaso é
/// a primeira letra de um secret).
pub const MASK_HOLD: Duration = Duration::from_millis(50);

pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    /// Variáveis herdadas do processo que o filho não recebe (aplicado
    /// antes de `env`).
    pub env_remove: Vec<String>,
    pub cols: u16,
    pub rows: u16,
    /// Valores substituídos por `***NOME***` na saída, antes do scrollback
    /// e dos assinantes (issue #167). Vazio = saída intacta.
    pub mask: MaskSet,
}

type ExitCallback = Box<dyn FnOnce(Option<i32>) + Send>;

struct Output {
    ring: RingBuffer,
    subs: Vec<Sender<Vec<u8>>>,
    masker: Masker,
    /// Muda a cada leitura que deixa cauda pendente: o prazo de
    /// [`MASK_HOLD`] conta da última.
    pending_gen: u64,
    /// O leitor chegou ao fim (a cauda já foi descarregada).
    closed: bool,
}

impl Output {
    fn emit(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.ring.push(bytes);
        self.subs.retain(|s| s.send(bytes.to_vec()).is_ok());
    }
}

#[derive(Default)]
struct ExitState {
    /// `Some(código)` após a saída; código `None` = morto por sinal.
    status: Option<Option<i32>>,
    callbacks: Vec<ExitCallback>,
}

struct Inner {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pid: Option<u32>,
    out: Mutex<Output>,
    /// Avisa o descarregador da cauda do mascaramento (par de `out`).
    pending: Condvar,
    exit: Mutex<ExitState>,
    exited: Condvar,
}

/// Processo rodando num PTY. Clonar compartilha o mesmo processo.
#[derive(Clone)]
pub struct PtySession {
    inner: Arc<Inner>,
}

impl PtySession {
    pub fn spawn(spec: SpawnSpec, scrollback: usize) -> io::Result<Self> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: spec.rows,
                cols: spec.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(io::Error::other)?;
        let mut cmd = CommandBuilder::new(&spec.program);
        cmd.args(&spec.args);
        cmd.cwd(&spec.cwd);
        for k in &spec.env_remove {
            cmd.env_remove(k);
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        let mut child = pair.slave.spawn_command(cmd).map_err(io::Error::other)?;
        // Sem fechar o lado escravo no pai, o leitor nunca vê EOF.
        drop(pair.slave);

        let reader = pair.master.try_clone_reader().map_err(io::Error::other)?;
        let writer = pair.master.take_writer().map_err(io::Error::other)?;
        let masked = !spec.mask.is_empty();
        let inner = Arc::new(Inner {
            pid: child.process_id(),
            killer: Mutex::new(child.clone_killer()),
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            out: Mutex::new(Output {
                ring: RingBuffer::new(scrollback),
                subs: Vec::new(),
                masker: spec.mask.masker(),
                pending_gen: 0,
                closed: false,
            }),
            pending: Condvar::new(),
            exit: Mutex::new(ExitState::default()),
            exited: Condvar::new(),
        });

        let (drained_tx, drained_rx) = mpsc::channel::<()>();
        let reading = Arc::clone(&inner);
        std::thread::spawn(move || {
            read_loop(reader, &reading);
            let _ = drained_tx.send(());
        });
        if masked {
            let flushing = Arc::clone(&inner);
            std::thread::spawn(move || flush_loop(&flushing));
        }

        let waiting = Arc::clone(&inner);
        std::thread::spawn(move || {
            let code = match child.wait() {
                Ok(st) if st.signal().is_some() => None,
                Ok(st) => Some(st.exit_code() as i32),
                Err(_) => None,
            };
            let _ = drained_rx.recv_timeout(DRAIN_GRACE);
            let callbacks = {
                let mut ex = waiting.exit.lock().unwrap();
                ex.status = Some(code);
                std::mem::take(&mut ex.callbacks)
            };
            waiting.exited.notify_all();
            // Fecha os assinantes: quem lê vê o fim do fluxo.
            waiting.out.lock().unwrap().subs.clear();
            for cb in callbacks {
                cb(code);
            }
        });

        Ok(PtySession { inner })
    }

    pub fn pid(&self) -> Option<u32> {
        self.inner.pid
    }

    /// Grupo de processos em primeiro plano no terminal (`tcgetpgrp`);
    /// `None` se não der para saber (ex.: o processo já saiu). Diferente do
    /// [`PtySession::pid`] = há um comando rodando por cima do shell.
    pub fn foreground_pgid(&self) -> Option<u32> {
        self.inner
            .master
            .lock()
            .unwrap()
            .process_group_leader()
            .and_then(|p| u32::try_from(p).ok())
    }

    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let mut w = self.inner.writer.lock().unwrap();
        w.write_all(bytes)?;
        w.flush()
    }

    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        self.inner
            .master
            .lock()
            .unwrap()
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(io::Error::other)
    }

    pub fn kill(&self) -> io::Result<()> {
        if self.exit_status().is_some() {
            return Ok(());
        }
        self.inner.killer.lock().unwrap().kill()
    }

    /// Snapshot do scrollback e receptor da saída ao vivo, obtidos sob a mesma
    /// trava: nenhum byte é perdido nem duplicado entre os dois.
    pub fn attach(&self) -> (Vec<u8>, Receiver<Vec<u8>>) {
        let (tx, rx) = mpsc::channel();
        let mut out = self.inner.out.lock().unwrap();
        let snap = out.ring.snapshot();
        if self.exit_status().is_none() {
            out.subs.push(tx);
        }
        (snap, rx)
    }

    pub fn snapshot(&self) -> Vec<u8> {
        self.inner.out.lock().unwrap().ring.snapshot()
    }

    /// Os últimos `n` bytes do scrollback, sem anexar.
    pub fn tail(&self, n: usize) -> Vec<u8> {
        self.inner.out.lock().unwrap().ring.tail(n)
    }

    /// `None` enquanto roda; `Some(código)` depois (código `None` = sinal).
    pub fn exit_status(&self) -> Option<Option<i32>> {
        self.inner.exit.lock().unwrap().status
    }

    /// Bloqueia até o processo sair.
    pub fn wait(&self) -> Option<i32> {
        let mut ex = self.inner.exit.lock().unwrap();
        loop {
            if let Some(code) = ex.status {
                return code;
            }
            ex = self.inner.exited.wait(ex).unwrap();
        }
    }

    /// Bloqueia até sair ou até `timeout`; `None` se ainda roda.
    pub fn wait_timeout(&self, timeout: Duration) -> Option<Option<i32>> {
        let ex = self.inner.exit.lock().unwrap();
        let (ex, _) = self
            .inner
            .exited
            .wait_timeout_while(ex, timeout, |e| e.status.is_none())
            .unwrap();
        ex.status
    }

    /// Executa `f` quando o processo sair (imediatamente, se já saiu).
    pub fn on_exit(&self, f: impl FnOnce(Option<i32>) + Send + 'static) {
        let mut ex = self.inner.exit.lock().unwrap();
        match ex.status {
            Some(code) => {
                drop(ex);
                f(code);
            }
            None => ex.callbacks.push(Box::new(f)),
        }
    }
}

fn read_loop(mut reader: Box<dyn Read + Send>, inner: &Inner) {
    let mut buf = [0u8; 16 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut out = inner.out.lock().unwrap();
                let masked = out.masker.feed(&buf[..n]);
                out.emit(&masked);
                if out.masker.has_pending() {
                    out.pending_gen += 1;
                    inner.pending.notify_all();
                }
            }
        }
    }
    let mut out = inner.out.lock().unwrap();
    let rest = out.masker.finish();
    out.emit(&rest);
    out.closed = true;
    inner.pending.notify_all();
}

/// Libera a cauda pendente do mascaramento que passou de [`MASK_HOLD`] sem
/// nova leitura. Só roda em sessões com valores a mascarar.
fn flush_loop(inner: &Inner) {
    let mut out = inner.out.lock().unwrap();
    loop {
        if out.closed {
            return;
        }
        if !out.masker.has_pending() {
            out = inner.pending.wait(out).unwrap();
            continue;
        }
        let gen = out.pending_gen;
        let (o, timeout) = inner
            .pending
            .wait_timeout_while(out, MASK_HOLD, |o| {
                !o.closed && o.pending_gen == gen && o.masker.has_pending()
            })
            .unwrap();
        out = o;
        if timeout.timed_out() && !out.closed && out.masker.has_pending() {
            let rest = out.masker.finish();
            out.emit(&rest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const T: Duration = Duration::from_secs(10);

    fn spec(program: &str, args: &[&str]) -> SpawnSpec {
        SpawnSpec {
            program: program.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: std::env::temp_dir(),
            env: vec![],
            env_remove: vec![],
            cols: 80,
            rows: 24,
            mask: MaskSet::default(),
        }
    }

    fn sh(script: &str) -> SpawnSpec {
        spec("bash", &["-c", script])
    }

    /// Lê do receptor até encontrar `needle` ou estourar o prazo.
    fn read_until(rx: &Receiver<Vec<u8>>, mut acc: Vec<u8>, needle: &str) -> String {
        let deadline = Instant::now() + T;
        loop {
            let s = String::from_utf8_lossy(&acc).to_string();
            if s.contains(needle) {
                return s;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(chunk) => acc.extend(chunk),
                Err(_) => panic!("não encontrou {needle:?} em {s:?}"),
            }
        }
    }

    #[test]
    fn captura_saida_e_codigo() {
        let p = PtySession::spawn(sh("echo ola; exit 3"), DEFAULT_SCROLLBACK).unwrap();
        assert!(p.pid().is_some());
        assert_eq!(p.wait(), Some(3));
        assert!(String::from_utf8_lossy(&p.snapshot()).contains("ola"));
    }

    #[test]
    fn env_e_cwd_sao_aplicados() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = sh("echo \"[$FOO]\"; pwd");
        s.cwd = dir.path().to_path_buf();
        s.env.push(("FOO".into(), "barra".into()));
        let p = PtySession::spawn(s, DEFAULT_SCROLLBACK).unwrap();
        assert_eq!(p.wait(), Some(0));
        let out = String::from_utf8_lossy(&p.snapshot()).to_string();
        assert!(out.contains("[barra]"), "{out}");
        let real = dir.path().canonicalize().unwrap();
        assert!(out.contains(real.to_str().unwrap()), "{out}");
    }

    #[test]
    fn env_remove_tira_a_variavel_herdada() {
        // `HOME` sempre existe no processo de teste.
        let mut s = sh("echo \"[${HOME:-vazio}]\"");
        s.env_remove.push("HOME".into());
        let p = PtySession::spawn(s, DEFAULT_SCROLLBACK).unwrap();
        assert_eq!(p.wait(), Some(0));
        let out = String::from_utf8_lossy(&p.snapshot()).to_string();
        assert!(out.contains("[vazio]"), "{out}");
    }

    #[test]
    fn primeiro_plano_muda_com_o_comando_do_shell() {
        let p = PtySession::spawn(spec("bash", &["--norc", "--noprofile", "-i"]), 4096).unwrap();
        let (snap, rx) = p.attach();
        p.write(b"echo pronto-$((1+1))\n").unwrap();
        read_until(&rx, snap, "pronto-2");
        let pid = p.pid();
        let deadline = Instant::now() + T;
        while p.foreground_pgid() != pid {
            assert!(
                Instant::now() < deadline,
                "shell não voltou ao primeiro plano"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        p.write(b"sleep 30\n").unwrap();
        while p.foreground_pgid() == pid {
            assert!(Instant::now() < deadline, "sleep não foi ao primeiro plano");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(p.foreground_pgid().is_some());
        p.kill().unwrap();
        p.wait_timeout(T).expect("deveria ter saído");
    }

    #[test]
    fn dois_assinantes_recebem_a_mesma_saida() {
        let p = PtySession::spawn(sh("read x; echo \"got-$x\"; sleep 0.2"), 1024).unwrap();
        let (s1, rx1) = p.attach();
        let (s2, rx2) = p.attach();
        p.write(b"zz\n").unwrap();
        assert!(read_until(&rx1, s1, "got-zz").contains("got-zz"));
        assert!(read_until(&rx2, s2, "got-zz").contains("got-zz"));
        assert_eq!(p.wait(), Some(0));
    }

    #[test]
    fn input_chega_ao_bash_interativo() {
        let p = PtySession::spawn(spec("bash", &["--norc", "-i"]), 1024).unwrap();
        let (snap, rx) = p.attach();
        p.write(b"echo $((40+2))\n").unwrap();
        read_until(&rx, snap, "42");
        p.write(b"exit 0\n").unwrap();
        assert_eq!(p.wait_timeout(T), Some(Some(0)));
    }

    #[test]
    fn kill_encerra_o_processo() {
        let p = PtySession::spawn(sh("sleep 30"), 1024).unwrap();
        p.kill().unwrap();
        let st = p.wait_timeout(T).expect("deveria ter saído");
        assert_ne!(st, Some(0));
        // kill depois de sair é inofensivo
        p.kill().unwrap();
    }

    #[test]
    fn on_exit_roda_antes_e_depois_da_saida() {
        let p = PtySession::spawn(sh("exit 7"), 1024).unwrap();
        let (tx, rx) = mpsc::channel();
        let tx2 = tx.clone();
        p.on_exit(move |c| tx.send(c).unwrap());
        assert_eq!(rx.recv_timeout(T).unwrap(), Some(7));
        p.on_exit(move |c| tx2.send(c).unwrap());
        assert_eq!(rx.recv_timeout(T).unwrap(), Some(7));
    }

    #[test]
    fn scrollback_respeita_a_capacidade() {
        let p =
            PtySession::spawn(sh("head -c 5000 /dev/zero | tr '\\0' a; echo FIM"), 100).unwrap();
        p.wait();
        let snap = p.snapshot();
        assert!(snap.len() <= 100);
        assert!(String::from_utf8_lossy(&snap).contains("FIM"));
        let tail = p.tail(5);
        assert_eq!(tail, snap[snap.len() - 5..]);
    }

    fn masked(script: &str) -> SpawnSpec {
        let mut s = sh(script);
        s.mask = MaskSet::new([(
            "API_TOKEN".to_string(),
            Zeroizing::new(b"sk-abc123456".to_vec()),
        )]);
        s
    }

    #[test]
    fn mascara_no_scrollback_e_no_assinante_mesmo_partido() {
        // O valor sai em duas escritas: chega em leituras separadas.
        let p = PtySession::spawn(
            masked("read x; printf 'a=sk-abc'; sleep 0.01; printf '123456 b=sk-abc123456\\n'"),
            DEFAULT_SCROLLBACK,
        )
        .unwrap();
        let (snap, rx) = p.attach();
        p.write(b"\n").unwrap();
        let live = read_until(&rx, snap, "b=***API_TOKEN***");
        assert!(
            live.contains("a=***API_TOKEN*** b=***API_TOKEN***"),
            "{live}"
        );
        assert!(!live.contains("123456"), "{live}");
        assert_eq!(p.wait(), Some(0));
        let out = String::from_utf8_lossy(&p.snapshot()).to_string();
        assert!(out.contains("a=***API_TOKEN*** b=***API_TOKEN***"), "{out}");
        assert!(!out.contains("sk-abc"), "{out}");
    }

    #[test]
    fn cauda_pendente_sai_depois_do_prazo() {
        // `sk-` pode ser começo do valor, mas nada mais chega: sai sozinho.
        let p = PtySession::spawn(masked("read x; printf 'x=sk-'; sleep 30"), 1024).unwrap();
        let (snap, rx) = p.attach();
        p.write(b"\n").unwrap();
        let t0 = Instant::now();
        read_until(&rx, snap, "x=sk-");
        assert!(t0.elapsed() < Duration::from_secs(5));
        p.kill().unwrap();
        p.wait_timeout(T).expect("deveria ter saído");
    }

    #[test]
    fn prefixo_no_fim_do_processo_e_descarregado() {
        let p = PtySession::spawn(masked("printf 'fim=sk-abc'"), 1024).unwrap();
        assert_eq!(p.wait(), Some(0));
        assert!(String::from_utf8_lossy(&p.snapshot()).contains("fim=sk-abc"));
    }

    #[test]
    fn assinante_ve_fim_do_fluxo() {
        let p = PtySession::spawn(sh("echo x"), 1024).unwrap();
        let (_, rx) = p.attach();
        p.wait();
        let deadline = Instant::now() + T;
        while rx.recv_timeout(deadline - Instant::now()).is_ok() {}
        // recv_timeout retornou Err: desconectado (ou prazo) — não trava.
    }
}
