//! Shell livre no PTY do daemon (spec da segunda entrega do console §4,
//! issue #428): o shell de login do usuário no worktree do work item, ou no
//! clone principal (`repo_path`) no work item `#0` da sessão do projeto.
//!
//! Não é sessão: não passa pelo ledger, não tem hooks, MCP nem decisões, e
//! fica só na memória (morre no restart do daemon). Usa o mesmo caminho de
//! terminal das sessões ([`Sessions::terminal`]): `session.attach|input|
//! resize` e os frames binários aceitam o id do shell. A limpeza do
//! worktree (#287) encerra antes os shells daquela pasta
//! ([`Sessions::close_shells_in`]).

use crate::launch::{PtySize, PROJECT_WORK_ITEM};
use crate::{Result, Sessions, SessionsError};
use ct_core::ProjectKind;
use ct_pty::{MaskSet, PtySession, SpawnSpec};
use serde::Serialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Shells vivos por ambiente.
pub const SHELL_LIMIT: usize = 8;

/// Prefixo das variáveis do Control Tower: nenhuma herdada do daemon chega
/// ao shell (o agente recebe as suas explicitamente, no spawn).
const CT_ENV_PREFIX: &str = "CTOWER_";

/// Variáveis dos docs do agente (#441): também não chegam ao shell, mesmo
/// herdadas do ambiente do daemon (ex.: daemon de teste aberto de dentro de
/// uma sessão do CT). O shell não recebe as variáveis do agente.
const AGENT_DOCS_ENV: [&str; 2] = [crate::docs::ENV_DOCS_DIR, crate::docs::ENV_EPIC_DOCS_DIR];

/// Das variáveis herdadas do daemon, as que o shell não recebe.
fn ct_vars(vars: impl Iterator<Item = String>) -> Vec<String> {
    vars.filter(|k| k.starts_with(CT_ENV_PREFIX) || AGENT_DOCS_ENV.contains(&k.as_str()))
        .collect()
}

/// Quanto a limpeza espera cada shell sair depois do `SIGHUP`.
const CLOSE_WAIT: Duration = Duration::from_secs(5);

/// Shell de `shell.open` e `shell.list`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Shell {
    /// UUID; é também o id do terminal (attach, input, resize e frames).
    pub id: String,
    pub work_item_id: i64,
    pub cwd: String,
    /// Programa rodando (o shell de login do usuário do daemon).
    pub program: String,
    pub started_at: i64,
    /// Há um comando em primeiro plano por cima do shell (o grupo de
    /// processos do terminal não é o do shell).
    pub busy: bool,
}

pub(crate) struct LiveShell {
    pub(crate) info: Shell,
    pub(crate) pty: PtySession,
}

impl LiveShell {
    fn snapshot(&self) -> Shell {
        let busy = match (self.pty.foreground_pgid(), self.pty.pid()) {
            (Some(fg), Some(pid)) => fg != pid,
            _ => false,
        };
        Shell {
            busy,
            ..self.info.clone()
        }
    }
}

/// O shell de login do usuário do daemon: `$SHELL`, ou o do `passwd`, ou
/// `/bin/sh`.
fn login_shell() -> String {
    if let Some(s) = std::env::var("SHELL").ok().filter(|s| !s.trim().is_empty()) {
        return s;
    }
    passwd_shell().unwrap_or_else(|| "/bin/sh".to_string())
}

fn passwd_shell() -> Option<String> {
    let mut buf = vec![0 as libc::c_char; 16 * 1024];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: `pwd` e `buf` vivem até o fim; `out` só é lido se não nulo.
    let rc = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            &mut pwd,
            buf.as_mut_ptr(),
            buf.len(),
            &mut out,
        )
    };
    if rc != 0 || out.is_null() || pwd.pw_shell.is_null() {
        return None;
    }
    // SAFETY: `pw_shell` aponta para uma string C dentro de `buf`.
    let s = unsafe { std::ffi::CStr::from_ptr(pwd.pw_shell) }
        .to_string_lossy()
        .into_owned();
    (!s.trim().is_empty()).then_some(s)
}

/// `dir` é `base` ou fica abaixo dele.
fn inside(dir: &str, base: &str) -> bool {
    let base = base.trim_end_matches('/');
    dir == base || dir.starts_with(&format!("{base}/"))
}

impl Sessions {
    /// Abre um shell no worktree do work item (no `repo_path` no `#0`).
    /// Recusa o gestor (não há pasta de item), o work item sem worktree no
    /// disco e o ambiente com [`SHELL_LIMIT`] shells vivos.
    pub fn open_shell(&self, work_item_id: i64, size: Option<PtySize>) -> Result<Shell> {
        let (project, wi) = {
            let l = self.ledger();
            let wi = l.get_work_item(work_item_id)?;
            (l.get_project(wi.project_id)?, wi)
        };
        if project.kind == ProjectKind::Gestor {
            return Err(SessionsError::Invalid(
                "o gestor não tem pasta de item para abrir um shell".into(),
            ));
        }
        let cwd = if wi.gh_number == PROJECT_WORK_ITEM {
            if !Path::new(&project.repo_path).is_dir() {
                return Err(SessionsError::Invalid(format!(
                    "clone do projeto não encontrado em {}",
                    project.repo_path
                )));
            }
            project.repo_path.clone()
        } else {
            let Some(wt) = wi.worktree_path.clone().filter(|p| !p.trim().is_empty()) else {
                return Err(SessionsError::Invalid(format!(
                    "o work item #{} não tem worktree; inicie uma sessão antes",
                    wi.gh_number
                )));
            };
            if !Path::new(&wt).is_dir() {
                return Err(SessionsError::Invalid(format!(
                    "o worktree {wt} não existe mais no disco"
                )));
            }
            wt
        };

        let cfg = self.config();
        let program = cfg.shell.clone().unwrap_or_else(login_shell);
        let size = size.unwrap_or(PtySize::DEFAULT);
        let env_remove = ct_vars(std::env::vars_os().filter_map(|(k, _)| k.into_string().ok()));
        let info = Shell {
            id: uuid::Uuid::new_v4().to_string(),
            work_item_id: wi.id,
            cwd,
            program,
            started_at: ct_core::now_ms(),
            busy: false,
        };
        // Conferir o limite e inserir sob a mesma trava: dois `shell.open`
        // juntos não passam do limite.
        let pty = {
            let mut shells = self.inner.shells.lock().unwrap();
            if shells.len() >= SHELL_LIMIT {
                return Err(SessionsError::Invalid(format!(
                    "limite de {SHELL_LIMIT} shells abertos neste ambiente; feche um antes"
                )));
            }
            let pty = PtySession::spawn(
                SpawnSpec {
                    program: info.program.clone(),
                    args: vec!["-l".to_string()],
                    cwd: PathBuf::from(&info.cwd),
                    env: vec![
                        ("TERM".to_string(), "xterm-256color".to_string()),
                        ("COLORTERM".to_string(), "truecolor".to_string()),
                    ],
                    env_remove,
                    cols: size.cols,
                    rows: size.rows,
                    mask: MaskSet::default(),
                },
                cfg.scrollback,
            )?;
            shells.insert(
                info.id.clone(),
                LiveShell {
                    info: info.clone(),
                    pty: pty.clone(),
                },
            );
            pty
        };
        // Fora da trava: se o processo já saiu, o aviso roda aqui mesmo.
        let this = self.clone();
        let sid = info.id.clone();
        pty.on_exit(move |code| {
            this.inner.shells.lock().unwrap().remove(&sid);
            this.notify("shell_exited", json!({ "shell_id": sid, "code": code }));
        });
        Ok(info)
    }

    /// Shells vivos, do mais antigo ao mais novo.
    pub fn list_shells(&self) -> Vec<Shell> {
        let mut v: Vec<Shell> = self
            .inner
            .shells
            .lock()
            .unwrap()
            .values()
            .map(LiveShell::snapshot)
            .collect();
        v.sort_by(|a, b| (a.started_at, &a.id).cmp(&(b.started_at, &b.id)));
        v
    }

    /// PTY de um shell vivo.
    pub fn shell_pty(&self, shell_id: &str) -> Option<PtySession> {
        self.inner
            .shells
            .lock()
            .unwrap()
            .get(shell_id)
            .map(|s| s.pty.clone())
    }

    /// Encerra o shell (`SIGHUP`, como fechar a janela do terminal); o push
    /// `shell_exited` vem quando o processo sai.
    pub fn close_shell(&self, shell_id: &str) -> Result<()> {
        match self.shell_pty(shell_id) {
            Some(p) => Ok(p.kill()?),
            None => Err(SessionsError::Invalid(format!(
                "shell {shell_id} não está aberto neste daemon"
            ))),
        }
    }

    /// Encerra os shells com `cwd` em `dir` (ou abaixo) e espera cada um
    /// sair (até [`CLOSE_WAIT`]); devolve quantos. A limpeza do worktree
    /// (#287) chama antes de remover a pasta.
    pub fn close_shells_in(&self, dir: &str) -> usize {
        let ptys: Vec<PtySession> = self
            .inner
            .shells
            .lock()
            .unwrap()
            .values()
            .filter(|s| inside(&s.info.cwd, dir))
            .map(|s| s.pty.clone())
            .collect();
        for p in &ptys {
            let _ = p.kill();
        }
        for p in &ptys {
            if p.wait_timeout(CLOSE_WAIT).is_none() {
                tracing::warn!("shell (pid {:?}) não saiu após o SIGHUP", p.pid());
            }
        }
        ptys.len()
    }

    /// PTY de uma sessão viva ou de um shell: o caminho de terminal comum
    /// (`session.attach|input|resize` e os frames binários).
    pub fn terminal(&self, id: &str) -> Option<PtySession> {
        self.pty(id).or_else(|| self.shell_pty(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use ct_core::WorkItemKind;
    use std::sync::mpsc::Receiver;
    use std::time::Instant;

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

    /// Work item #n com worktree no disco (por uma sessão que já saiu).
    fn com_worktree(env: &TestEnv, n: i64) -> ct_ledger::WorkItem {
        let s = env.sessions.start(env.request(n, "item")).unwrap();
        env.wait_state(&s.id, ct_core::SessionState::Exited);
        env.sessions.ledger().get_work_item(s.work_item_id).unwrap()
    }

    fn wait_busy(env: &TestEnv, id: &str, busy: bool) {
        let deadline = Instant::now() + T;
        while env
            .sessions
            .list_shells()
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.busy)
            != Some(busy)
        {
            assert!(Instant::now() < deadline, "busy não virou {busy}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn variaveis_do_ct_e_dos_docs_do_agente_saem_do_shell() {
        let vars = [
            "PATH",
            "CTOWER_TOKEN",
            "CT_DOCS_DIR",
            "CT_EPIC_DOCS_DIR",
            "CT_OUTRA",
            "HOME",
        ];
        assert_eq!(
            ct_vars(vars.iter().map(|s| s.to_string())),
            vec!["CTOWER_TOKEN", "CT_DOCS_DIR", "CT_EPIC_DOCS_DIR"]
        );
    }

    #[test]
    fn abre_no_worktree_com_shell_de_login_e_sem_variaveis_do_ct() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let wi = com_worktree(&env, 3);
        let wt = wi.worktree_path.clone().unwrap();
        let sh = env
            .sessions
            .open_shell(wi.id, Some(PtySize { cols: 90, rows: 33 }))
            .unwrap();
        assert_eq!((sh.work_item_id, sh.cwd.as_str()), (wi.id, wt.as_str()));
        assert_eq!(sh.program, env.cfg.shell.clone().unwrap());
        assert!(!sh.busy);
        let out = env.wait_shell_output(&sh.id);
        assert!(out.contains("ARG=-l"), "{out}");
        let real = std::fs::canonicalize(&wt).unwrap();
        assert!(out.contains(&format!("PWD={}", real.display())), "{out}");
        assert!(out.contains("SIZE=33 90"), "{out}");
        assert!(out.contains("TERM=xterm-256color"), "{out}");
        assert!(out.contains("COLORTERM=truecolor"), "{out}");
        // Nenhuma `CTOWER_*` (o teste pode rodar dentro de uma sessão do CT).
        assert!(!out.contains("CTOWER_"), "{out}");
        assert!(!out.contains("DOCS_DIR="), "{out}");
        // Não é sessão: nada no ledger.
        assert_eq!(env.sessions.ledger().live_sessions().unwrap().len(), 0);
        assert!(env.sessions.pty(&sh.id).is_none());
        assert!(env.sessions.terminal(&sh.id).is_some());
        assert_eq!(env.sessions.list_shells(), vec![sh.clone()]);
        env.sessions.close_shell(&sh.id).unwrap();
        env.wait_notice(|n, d| n == "shell_exited" && d["shell_id"] == sh.id.as_str());
        assert!(env.sessions.list_shells().is_empty());
        assert!(env.sessions.close_shell(&sh.id).is_err());
    }

    #[test]
    fn sessao_do_projeto_abre_no_clone_principal() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let wi = env
            .sessions
            .ledger()
            .ensure_work_item(env.project_id, PROJECT_WORK_ITEM, WorkItemKind::Issue)
            .unwrap();
        let sh = env.sessions.open_shell(wi.id, None).unwrap();
        assert_eq!(sh.cwd, env.repo.to_string_lossy());
        env.sessions.close_shell(&sh.id).unwrap();
    }

    #[test]
    fn recusas_sem_worktree_removido_gestor_e_limite() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let err = |r: Result<Shell>| match r {
            Err(SessionsError::Invalid(m)) => m,
            other => panic!("esperava recusa: {other:?}"),
        };
        let sem = env
            .sessions
            .ledger()
            .ensure_work_item(env.project_id, 4, WorkItemKind::Issue)
            .unwrap();
        assert!(err(env.sessions.open_shell(sem.id, None)).contains("não tem worktree"));

        let wi = com_worktree(&env, 5);
        let wt = wi.worktree_path.clone().unwrap();
        let moved = env.tmp.path().join("fora");
        std::fs::rename(&wt, &moved).unwrap();
        assert!(err(env.sessions.open_shell(wi.id, None)).contains("não existe mais"));
        std::fs::rename(&moved, &wt).unwrap();

        let dir = env.cfg.gestor_dir.to_string_lossy().to_string();
        let (_, gestor) = env.sessions.ledger().ensure_gestor_project(&dir).unwrap();
        assert!(err(env.sessions.open_shell(gestor.id, None)).contains("gestor"));
        assert!(env.sessions.open_shell(9999, None).is_err());

        let ids: Vec<String> = (0..SHELL_LIMIT)
            .map(|_| env.sessions.open_shell(wi.id, None).unwrap().id)
            .collect();
        assert!(err(env.sessions.open_shell(wi.id, None)).contains("limite de 8"));
        // Fechar um libera a vaga.
        env.sessions.close_shell(&ids[0]).unwrap();
        env.wait_notice(|n, d| n == "shell_exited" && d["shell_id"] == ids[0].as_str());
        let novo = env.sessions.open_shell(wi.id, None).unwrap();
        assert_eq!(env.sessions.list_shells().len(), SHELL_LIMIT);
        assert_eq!(env.sessions.close_shells_in(&wt), SHELL_LIMIT);
        // `close_shells_in` espera os processos saírem; quem tira o shell da
        // lista é a thread que emite `shell_exited`, logo depois.
        for id in ids[1..].iter().chain([&novo.id]) {
            env.wait_notice(|n, d| n == "shell_exited" && d["shell_id"] == id.as_str());
        }
        assert!(env.sessions.list_shells().is_empty(), "{novo:?}");
    }

    #[test]
    fn input_busy_e_exit_com_codigo() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let wi = com_worktree(&env, 6);
        let sh = env.sessions.open_shell(wi.id, None).unwrap();
        let pty = env.sessions.terminal(&sh.id).unwrap();
        let (snap, rx) = pty.attach();
        pty.write(b"echo soma-$((20+22))\n").unwrap();
        read_until(&rx, snap, "soma-42");
        wait_busy(&env, &sh.id, false);
        pty.write(b"sleep 30\n").unwrap();
        wait_busy(&env, &sh.id, true);
        pty.write(b"\x03").unwrap();
        wait_busy(&env, &sh.id, false);
        pty.write(b"exit 7\n").unwrap();
        let n = env.wait_notice(|n, d| n == "shell_exited" && d["shell_id"] == sh.id.as_str());
        let (_, d) = n.iter().find(|(n, _)| n == "shell_exited").unwrap();
        assert_eq!(d["code"], 7, "{d}");
        assert!(env.sessions.terminal(&sh.id).is_none());
    }

    #[test]
    fn close_shells_in_so_encerra_os_da_pasta() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let a = com_worktree(&env, 10);
        let b = com_worktree(&env, 11);
        let sa = env.sessions.open_shell(a.id, None).unwrap();
        let sb = env.sessions.open_shell(b.id, None).unwrap();
        let wt = a.worktree_path.unwrap();
        assert_eq!(env.sessions.close_shells_in(&format!("{wt}/")), 1);
        env.wait_notice(|n, d| n == "shell_exited" && d["shell_id"] == sa.id.as_str());
        let vivos: Vec<String> = env
            .sessions
            .list_shells()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(vivos, vec![sb.id.clone()]);
        env.sessions.close_shell(&sb.id).unwrap();
    }
}
