//! Sessões de sub-issue da execução de épico (#298, épico #295): o hub as
//! inicia com `session.start { …, epic_run: { base } }` a partir da `wip`
//! do épico ([`Sessions::start_epic_run`]), marcadas com o evento
//! [`EPIC_RUN_SESSION`] no work item (a sessão aceita texto no
//! `session.continue`). Depois do merge da sub-issue na `wip`, o hub pede a
//! remoção do worktree (`ct_cleanup::EpicWorktree::remove_epic_worktree`),
//! com as mesmas proteções da limpeza.

use crate::{Result, Sessions, SessionsError};
use ct_core::branch::branch_from_pattern;
use ct_core::WorkItemKind;
use ct_git::integration::{publish_branch, Published};
use ct_ledger::Session;
use serde::Serialize;
use std::path::Path;

/// Evento que marca o work item da sub-issue iniciada pela execução de
/// épico (payload: `base`).
pub const EPIC_RUN_SESSION: &str = "epic_run_session";
/// Entrada humana (teclado da UI) numa sessão da execução de épico (#301):
/// o hub marca a sub-issue como assumida. O texto do motor
/// (`session.continue`) não gera o evento.
pub const HUMAN_INPUT: &str = "human_input";
/// Intervalo mínimo entre dois `human_input` da mesma sessão.
pub const HUMAN_INPUT_DEBOUNCE_MS: i64 = 60_000;

/// `data` é só resposta automática do terminal (xterm), sem tecla nenhuma
/// (#341): o que ele responde às consultas do agente (atributos `CSI … c`,
/// posição do cursor `CSI n;m R`, estado `CSI n n`, modos `CSI ? … $ y`,
/// teclado `CSI ? n u`, janela `CSI … t`), os eventos de foco (`CSI I`,
/// `CSI O`) e as respostas `OSC` (cores) e `DCS` (versão, `DECRQSS`). O
/// mouse sem botão também não conta: o Claude Code liga o rastreamento de
/// qualquer movimento (modo 1003, SGR), e passar o cursor sobre o terminal
/// (`CSI < 35;x;y M`) ou rolar a roda (`CSI < 64;x;y M`) para ler a sessão
/// não é assumi-la. Teclas (setas, `CSI 1;5A`, `CSI 15~`, `CSI 97;5u`),
/// clique, arrasto com botão e texto contam como gente. Vazio não é
/// resposta.
pub fn is_terminal_reply(data: &[u8]) -> bool {
    let mut i = 0;
    let mut any = false;
    while i < data.len() {
        let Some(n) = reply_len(&data[i..]) else {
            return false;
        };
        i += n;
        any = true;
    }
    any
}

/// O formato da sequência de controle no início de `data`, para o log do
/// diagnóstico (#341): só `ESC`, o introdutor e os bytes de parâmetro,
/// intermediários e final de um `CSI` (dígitos e símbolos) ou o número de
/// um `OSC`/`DCS`. Nunca texto: a colagem sai como `ESC[200~`. `None` =
/// não começa com `ESC`.
pub fn control_shape(data: &[u8]) -> Option<String> {
    if data.first() != Some(&0x1b) {
        return None;
    }
    let mut out = String::from("ESC");
    match data.get(1) {
        Some(b'[') => {
            out.push('[');
            for &b in data.iter().skip(2).take(24) {
                if (0x20..=0x3f).contains(&b) {
                    out.push(b as char);
                } else if (0x40..=0x7e).contains(&b) {
                    out.push(b as char);
                    break;
                } else {
                    break;
                }
            }
        }
        Some(&c @ (b']' | b'P')) => {
            out.push(c as char);
            for &b in data.iter().skip(2).take(8) {
                if b.is_ascii_digit() || matches!(b, b'>' | b'+' | b'$' | b'|') {
                    out.push(b as char);
                } else {
                    break;
                }
            }
            out.push('…');
        }
        Some(&b) if (0x20..=0x7e).contains(&b) && !b.is_ascii_alphanumeric() => out.push(b as char),
        Some(_) => out.push('…'),
        None => {}
    }
    Some(out)
}

/// Tamanho da resposta de terminal no início de `d`; `None` = não é uma.
fn reply_len(d: &[u8]) -> Option<usize> {
    const ESC: u8 = 0x1b;
    if d.len() < 3 || d[0] != ESC {
        return None;
    }
    match d[1] {
        b'[' => {
            // CSI: parâmetros (0x30–0x3F), intermediários (0x20–0x2F), final.
            let mut j = 2;
            while j < d.len() && (0x30..=0x3f).contains(&d[j]) {
                j += 1;
            }
            let params = &d[2..j];
            let mid_start = j;
            while j < d.len() && (0x20..=0x2f).contains(&d[j]) {
                j += 1;
            }
            let mid = &d[mid_start..j];
            let fin = *d.get(j)?;
            let private = params.first() == Some(&b'?');
            let reply = match fin {
                // Mouse (SGR) sem botão: movimento (35) e roda (64–67), com ou
                // sem Shift/Alt/Ctrl.
                b'M' | b'm' if params.first() == Some(&b'<') => {
                    std::str::from_utf8(&params[1..])
                        .ok()
                        .and_then(|p| p.split(';').next())
                        .and_then(|b| b.parse::<u32>().ok())
                        .is_some_and(|b| {
                            let b = b & !0b1_1100;
                            b == 35 || (64..=67).contains(&b)
                        })
                        && mid.is_empty()
                }
                // Atributos do terminal (DA1, DA2, DA3).
                b'c' => true,
                // Posição do cursor e estado do terminal.
                b'R' | b'n' => !params.is_empty() && mid.is_empty(),
                // Modo (DECRPM).
                b'y' => mid == b"$",
                // Flags do teclado (kitty): só com `?`; sem, é tecla.
                b'u' => private,
                // Relatórios da janela.
                b't' => !params.is_empty() && mid.is_empty(),
                // Foco.
                b'I' | b'O' => params.is_empty() && mid.is_empty(),
                _ => false,
            };
            reply.then_some(j + 1)
        }
        // OSC e DCS, até BEL ou ST (`ESC \`).
        b']' | b'P' => {
            let mut j = 2;
            while j < d.len() {
                if d[j] == 0x07 {
                    return Some(j + 1);
                }
                if d[j] == ESC && d.get(j + 1) == Some(&b'\\') {
                    return Some(j + 2);
                }
                j += 1;
            }
            None
        }
        _ => None,
    }
}

/// Resultado de `epic_run.prepare` (#341).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EpicBranch {
    /// A branch de integração do épico, já no `origin`.
    pub branch: String,
    /// `existing` (já estava no remoto), `local` (subiu a do clone) ou
    /// `base` (criada a partir da base do projeto).
    pub published: String,
}

impl Sessions {
    /// Prepara a branch de integração do épico para a execução (#341), sem
    /// sessão nem worktree: usa a branch registrada no work item do épico
    /// ou calcula o nome pelo padrão do projeto (prefixo `wip`), garante a
    /// branch no `origin` ([`publish_branch`]: a do clone sobe como está; sem
    /// nenhuma, nasce da base) e a registra no work item, que o hub recebe
    /// pelo sync.
    pub fn prepare_epic_branch(
        &self,
        project_id: i64,
        epic: i64,
        title: &str,
    ) -> Result<EpicBranch> {
        if epic <= 0 {
            return Err(SessionsError::Invalid(format!(
                "número de épico inválido: {epic}"
            )));
        }
        let project = self.ledger().get_project(project_id)?;
        let wi = self
            .ledger()
            .ensure_work_item(project_id, epic, WorkItemKind::Epic)?;
        // A branch registrada vale para sempre, como no início de sessão.
        let branch = match wi.branch.clone() {
            Some(b) => b,
            None => branch_from_pattern(
                project.effective_branch_pattern(),
                WorkItemKind::Epic,
                epic,
                &ct_core::item_key(&project.tracker_ref(), epic),
                title,
                &[],
            )
            .map_err(|e| SessionsError::Invalid(e.to_string()))?,
        };
        let published =
            publish_branch(Path::new(&project.repo_path), &branch, &project.base_branch)?;
        if published != Published::Existing {
            tracing::info!(
                "execução de épico: {branch} publicada em {}/{} ({published:?})",
                project.gh_owner,
                project.gh_repo
            );
        }
        if wi.branch.is_none() {
            self.ledger().set_branch_if_missing(wi.id, &branch)?;
        }
        Ok(EpicBranch {
            branch,
            published: match published {
                Published::Existing => "existing",
                Published::FromLocal => "local",
                Published::FromBase => "base",
            }
            .into(),
        })
    }

    /// A sessão é de uma sub-issue iniciada pela execução de épico.
    pub fn is_epic_run_session(&self, s: &Session) -> Result<bool> {
        self.is_epic_run_item(s.work_item_id)
    }

    /// O work item é de uma sub-issue iniciada pela execução de épico
    /// (marcado com [`EPIC_RUN_SESSION`]).
    pub fn is_epic_run_item(&self, work_item_id: i64) -> Result<bool> {
        Ok(!self
            .ledger()
            .list_events_of_kind(work_item_id, EPIC_RUN_SESSION)?
            .is_empty())
    }

    /// Teclado humano na sessão (`session.input`, canal de entrada do
    /// `/ws`): numa sessão da execução de épico, grava [`HUMAN_INPUT`] no
    /// máximo uma vez a cada [`HUMAN_INPUT_DEBOUNCE_MS`]. `data` só com
    /// respostas automáticas do terminal ([`is_terminal_reply`]) não conta.
    /// Devolve se gravou. Erros não impedem a escrita no PTY (quem chama
    /// ignora).
    pub fn note_human_input(&self, session_id: &str, data: &[u8], now: i64) -> Result<bool> {
        // A resposta automática do terminal não é gente digitando (#341).
        if is_terminal_reply(data) {
            return Ok(false);
        }
        // Reserva a marca na mesma trava da checagem: entradas simultâneas
        // não passam todas pelo intervalo (#341).
        {
            let mut seen = self.inner.human_input.lock();
            if seen
                .get(session_id)
                .is_some_and(|t| now - t < HUMAN_INPUT_DEBOUNCE_MS)
            {
                return Ok(false);
            }
            seen.insert(session_id.to_string(), now);
        }
        let release = || {
            self.inner.human_input.lock().remove(session_id);
        };
        let recorded = (|| -> Result<bool> {
            let s = self.ledger().get_session(session_id)?;
            if !self.is_epic_run_session(&s)? {
                return Ok(false);
            }
            let project_id = self.ledger().get_work_item(s.work_item_id)?.project_id;
            self.ledger().append_event(ct_ledger::NewEvent {
                project_id,
                work_item_id: Some(s.work_item_id),
                session_id: Some(&s.id),
                kind: HUMAN_INPUT,
                payload: serde_json::json!({ "session_id": s.id }),
            })?;
            Ok(true)
        })();
        if matches!(recorded, Ok(true)) {
            if let Some(shape) = control_shape(data) {
                tracing::info!(
                    "execução de épico: sessão {session_id} assumida por entrada que começa com \
                     {shape}"
                );
            }
        }
        // Sem gravar (erro), a próxima entrada tenta de novo. Sessão fora da
        // execução de épico fica marcada: não volta ao ledger a cada tecla.
        if recorded.is_err() {
            release();
        }
        recorded
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::{git_in, FakeClaude, TestEnv};
    use crate::SessionsError;
    use std::fs;

    /// `TestEnv` com `origin` e a `wip/10-x` (um commit além da `develop`)
    /// enviada.
    fn env() -> TestEnv {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let origin = env.tmp.path().join("origin.git");
        let o = origin.to_string_lossy().to_string();
        git_in(env.tmp.path(), &["init", "-q", "--bare", &o]);
        git_in(&env.repo, &["remote", "add", "origin", &o]);
        git_in(&env.repo, &["push", "-q", "-u", "origin", "develop"]);
        git_in(&env.repo, &["checkout", "-q", "-b", "wip/10-x"]);
        fs::write(env.repo.join("wip.txt"), "wip\n").unwrap();
        git_in(&env.repo, &["add", "wip.txt"]);
        git_in(&env.repo, &["commit", "-q", "-m", "wip"]);
        git_in(&env.repo, &["push", "-q", "-u", "origin", "wip/10-x"]);
        git_in(&env.repo, &["checkout", "-q", "develop"]);
        env
    }

    /// #341: o que o xterm responde sozinho não é gente digitando.
    #[test]
    fn respostas_do_terminal_nao_contam_como_teclado() {
        use super::is_terminal_reply as r;
        for reply in [
            &b"\x1b[?1;2c"[..],
            b"\x1b[>0;276;0c",
            b"\x1b[24;80R",
            b"\x1b[0n",
            b"\x1b[?2026;2$y",
            b"\x1b[?0u",
            b"\x1b[8;30;100t",
            b"\x1b[I",
            b"\x1b[O",
            b"\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\",
            b"\x1b]10;rgb:ffff/ffff/ffff\x07",
            b"\x1bP>|xterm.js(5.5.0)\x1b\\",
            // Cursor passando sobre o terminal (movimento sem botão).
            b"\x1b[<35;40;12M",
            b"\x1b[<35;40;12M\x1b[<35;41;12M\x1b[<35;42;13M",
            b"\x1b[<39;40;12M",
            // Roda do mouse, também com Ctrl (64 + 16).
            b"\x1b[<64;10;5M",
            b"\x1b[<65;10;5M\x1b[<65;10;5M",
            b"\x1b[<80;3;3M",
            // A rajada de quatro respostas que marcou a sessão.
            b"\x1b[?1;2c\x1b]11;rgb:0/0/0\x07\x1b[?0u\x1b[I",
        ] {
            assert!(r(reply), "{:?}", String::from_utf8_lossy(reply));
        }
        for key in [
            &b""[..],
            b"x",
            b"\r",
            b"\x1b",
            b"\x1b[A",
            b"\x1b[1;5A",
            b"\x1b[15~",
            b"\x1b[Z",
            b"\x1b[97;5u",
            b"\x1b[<0;10;5M",
            b"\x1b[<0;10;5m",
            b"\x1b[<2;10;5M",
            // Arrasto com o botão esquerdo (32 + 0).
            b"\x1b[<32;10;5M",
            b"\x1b[200~colado\x1b[201~",
            b"\x1b[?1;2cx",
            b"\x1b]11;sem fim",
        ] {
            assert!(!r(key), "{:?}", String::from_utf8_lossy(key));
        }
    }

    /// O formato do log de diagnóstico nunca leva texto.
    #[test]
    fn formato_da_sequencia_sem_texto() {
        use super::control_shape as c;
        assert_eq!(c(b"x"), None);
        assert_eq!(c(b"\x1b[?1;2c").as_deref(), Some("ESC[?1;2c"));
        assert_eq!(c(b"\x1b[200~segredo\x1b[201~").as_deref(), Some("ESC[200~"));
        assert_eq!(c(b"\x1b]11;rgb:0/0/0\x07").as_deref(), Some("ESC]11…"));
        assert_eq!(c(b"\x1bOA").as_deref(), Some("ESC…"));
        assert_eq!(c(b"\x1b").as_deref(), Some("ESC"));
    }

    /// #341: entradas simultâneas não passam todas pelo intervalo.
    #[test]
    fn rajada_simultanea_grava_uma_vez() {
        let env = env();
        let s = env
            .sessions
            .start_epic_run(env.request(11, "Tela"), "wip/10-x")
            .unwrap();
        let gravou: Vec<bool> = std::thread::scope(|t| {
            let hs: Vec<_> = (0..8)
                .map(|_| t.spawn(|| env.sessions.note_human_input(&s.id, b"x", 1_000).unwrap()))
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(gravou.iter().filter(|g| **g).count(), 1, "{gravou:?}");
        let n = env
            .sessions
            .ledger()
            .list_events_of_kind(s.work_item_id, super::HUMAN_INPUT)
            .unwrap()
            .len();
        assert_eq!(n, 1);
        // Resposta do terminal não grava nem ocupa o intervalo.
        assert!(!env
            .sessions
            .note_human_input(&s.id, b"\x1b[?1;2c", 70_000)
            .unwrap());
        assert!(env.sessions.note_human_input(&s.id, b"y", 70_000).unwrap());
        env.sessions.kill(&s.id).unwrap();
    }

    #[test]
    fn teclado_humano_marca_a_sessao_da_execucao_e_o_texto_do_motor_nao() {
        let env = env();
        let s = env
            .sessions
            .start_epic_run(env.request(11, "Tela"), "wip/10-x")
            .unwrap();
        env.wait_fake_output(&s.id);
        let count = || {
            env.sessions
                .ledger()
                .list_events_of_kind(s.work_item_id, super::HUMAN_INPUT)
                .unwrap()
                .len()
        };
        // O texto do motor não marca.
        env.sessions
            .continue_session(&s.id, Some("CI verde: mescle"))
            .unwrap();
        assert_eq!(count(), 0);
        assert!(env.sessions.note_human_input(&s.id, b"x", 1_000).unwrap());
        // Debounce de 60 s.
        assert!(!env.sessions.note_human_input(&s.id, b"x", 30_000).unwrap());
        assert!(env.sessions.note_human_input(&s.id, b"x", 61_000).unwrap());
        assert_eq!(count(), 2);
        // Sessão comum: nada.
        let outra = env.sessions.start(env.request(12, "Outra")).unwrap();
        assert!(!env
            .sessions
            .note_human_input(&outra.id, b"x", 1_000)
            .unwrap());
        for id in [&s.id, &outra.id] {
            env.sessions.kill(id).ok();
        }
    }

    fn remote_sha(env: &TestEnv, branch: &str) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&env.repo)
            .args(["ls-remote", "origin", &format!("refs/heads/{branch}")])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// #341: a sub-issue sai da `wip` do `origin` como está agora (o
    /// bloqueante mesclado por outro ambiente), não da cópia velha do clone.
    #[test]
    fn sub_issue_sai_da_wip_atualizada_do_origin() {
        let env = env();
        let origin = env.tmp.path().join("origin.git");
        let outro = env.tmp.path().join("outro");
        git_in(
            env.tmp.path(),
            &[
                "clone",
                "-q",
                "-b",
                "wip/10-x",
                origin.to_str().unwrap(),
                outro.to_str().unwrap(),
            ],
        );
        fs::write(outro.join("bloqueante.txt"), "mesclado\n").unwrap();
        git_in(&outro, &["add", "bloqueante.txt"]);
        git_in(
            &outro,
            &[
                "-c",
                "user.name=Teste",
                "-c",
                "user.email=teste@example.com",
                "commit",
                "-q",
                "-m",
                "bloqueante",
            ],
        );
        git_in(&outro, &["push", "-q", "origin", "wip/10-x"]);
        let s = env
            .sessions
            .start_epic_run(env.request(12, "Depende"), "wip/10-x")
            .unwrap();
        let wt = std::path::PathBuf::from(&s.cwd);
        assert!(wt.join("bloqueante.txt").is_file(), "saiu da wip velha");
        env.sessions.kill(&s.id).unwrap();
    }

    /// #341: o motor prepara a `wip` sem sessão do épico.
    #[test]
    fn prepara_a_wip_do_epico_sem_sessao_nem_worktree() {
        let env = env();
        let l = || env.sessions.ledger();
        // Épico sem branch: o nome sai do padrão e a branch nasce da base.
        let b = env
            .sessions
            .prepare_epic_branch(env.project_id, 20, "Login novo")
            .unwrap();
        assert_eq!(b.branch, "wip/20-login-novo");
        assert_eq!(b.published, "base");
        assert_eq!(remote_sha(&env, &b.branch), remote_sha(&env, "develop"));
        let wi = l().find_work_item(env.project_id, 20).unwrap().unwrap();
        assert_eq!(wi.kind, ct_core::WorkItemKind::Epic);
        assert_eq!(wi.branch.as_deref(), Some("wip/20-login-novo"));
        assert_eq!(wi.worktree_path, None);
        // De novo: o mesmo nome, mesmo com outro título.
        let b = env
            .sessions
            .prepare_epic_branch(env.project_id, 20, "Outro título")
            .unwrap();
        assert_eq!(
            (b.branch.as_str(), b.published.as_str()),
            ("wip/20-login-novo", "existing")
        );

        // Branch registrada por uma sessão antiga, só no clone: sobe a local.
        git_in(&env.repo, &["branch", "wip/30-antigo", "develop"]);
        git_in(&env.repo, &["checkout", "-q", "wip/30-antigo"]);
        fs::write(env.repo.join("antigo.txt"), "x\n").unwrap();
        git_in(&env.repo, &["add", "antigo.txt"]);
        git_in(&env.repo, &["commit", "-q", "-m", "antigo"]);
        git_in(&env.repo, &["checkout", "-q", "develop"]);
        let wi = l()
            .ensure_work_item(env.project_id, 30, ct_core::WorkItemKind::Epic)
            .unwrap();
        l().set_work_item_branch(wi.id, "wip/30-antigo", "/tmp/acme-wt-30")
            .unwrap();
        let b = env
            .sessions
            .prepare_epic_branch(env.project_id, 30, "Título novo")
            .unwrap();
        assert_eq!(
            (b.branch.as_str(), b.published.as_str()),
            ("wip/30-antigo", "local")
        );
        let local = std::process::Command::new("git")
            .arg("-C")
            .arg(&env.repo)
            .args(["rev-parse", "wip/30-antigo"])
            .output()
            .unwrap();
        let local = String::from_utf8_lossy(&local.stdout).trim().to_string();
        assert_eq!(remote_sha(&env, "wip/30-antigo"), local);

        assert!(matches!(
            env.sessions.prepare_epic_branch(env.project_id, 0, "x"),
            Err(SessionsError::Invalid(_))
        ));
    }
}
