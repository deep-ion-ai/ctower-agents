//! O atualizador em si, genérico no serviço: busca o ref, compila num
//! diretório à parte, guarda os binários e um snapshot consistente dos
//! bancos (`VACUUM INTO`), instala, reinicia e confere a saúde. Se o serviço
//! novo não ficar saudável, volta os binários (e os bancos que migraram) e o
//! reinicia. Usado pelo `ctower update --run` (ctowerd + ctower) e pelo
//! `cthubd update` (cthubd).

use crate::{
    git, log_file, tail, workspace_version, write_state, Build, Phase, Source, UpdateState,
};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Linhas do `update.log` levadas ao estado quando falha.
const LOG_TAIL: usize = 40;

/// O que o gerenciador de serviços deve fazer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Restart,
    Stop,
    Start,
}

/// Comando do gerenciador de serviços para uma ação.
pub type ServiceCtl = Box<dyn Fn(Action) -> Command>;

/// Sonda de saúde: o build do serviço no ar, ou o motivo de não estar
/// saudável (fora do ar, resposta inválida...).
pub type Probe = Box<dyn Fn() -> std::result::Result<Build, String>>;

/// O serviço a atualizar.
pub struct Target {
    /// Nome do serviço nas mensagens (ex.: `ctowerd`, `cthubd`).
    pub service: String,
    /// Crates do workspace a compilar (`crates/<b>`), cada um com um binário
    /// de mesmo nome instalado em `bin_dir`.
    pub bins: Vec<String>,
    /// Bancos com snapshot antes do restart (os ausentes são pulados).
    pub dbs: Vec<PathBuf>,
    pub bin_dir: PathBuf,
    /// `cargo` usado no build.
    pub cargo: String,
    /// Prefixo dos assets da release (`<prefixo>-vX.Y.Z-<arch>-<os>.tar.gz`
    /// e `.sha256`) com os binários prontos. Se o commit alvo é o da tag da
    /// versão e o asset existe, instala dele em vez de compilar (#279);
    /// qualquer falha cai para o build.
    pub release_prefix: Option<String>,
    /// Quanto esperar o serviço novo ficar saudável.
    pub verify_timeout: Duration,
    pub ctl: ServiceCtl,
    pub probe: Probe,
    /// Roda em cada banco logo depois de ele voltar do snapshot (ex.: o hub
    /// sorteia outra `config_epoch`). Devolve uma observação para o log.
    pub after_db_restore: Option<DbHook>,
    /// No rollback manual, voltar um banco do snapshot exige `force`: os
    /// dados que entraram depois do snapshot saem do banco em uso.
    pub force_db_rollback: bool,
    /// Observação no log quando um banco volta do snapshot (ex.: o que
    /// fazer nos ambientes).
    pub db_rollback_note: Option<String>,
    /// Passos antes do `cargo install` (ex.: o bundle web do cthubd, #199).
    pub pre_build: Option<PreBuild>,
}

/// Conferência das ferramentas do pré-build.
pub type PreBuildCheck = Box<dyn Fn() -> std::result::Result<(), String>>;
/// Comandos do pré-build, a partir do diretório do fonte.
pub type PreBuildCommands = Box<dyn Fn(&Path) -> Vec<Command>>;

/// Passos de build fora do `cargo` (ex.: `npm ci` e `npm run build:web`).
pub struct PreBuild {
    /// Confere as ferramentas antes de tudo (antes do fetch, de qualquer
    /// backup e da troca do binário). O erro é a mensagem final.
    pub check: PreBuildCheck,
    /// Comandos rodados no fonte, na ordem, com a saída no `update.log`.
    pub commands: PreBuildCommands,
    /// Variáveis a mais do `cargo install` (ex.: `CTHUB_WEB_REQUIRED=1`).
    pub cargo_env: Vec<(String, String)>,
}

/// Gancho chamado com o caminho do banco restaurado.
pub type DbHook = Box<dyn Fn(&Path) -> std::result::Result<String, String>>;

/// O que a atualização guardou para voltar (`<update>/prev/prev.json`).
#[derive(serde::Serialize, serde::Deserialize)]
struct Prev {
    build: Build,
    /// `user_version` de cada banco no momento do backup (nome do arquivo).
    dbs: Vec<(String, i64)>,
    /// Quando os snapshots foram tirados (ms; 0 = desconhecido).
    #[serde(default)]
    taken_at: i64,
}

/// `<arquivo>.pre-rollback-<stamp>` livre ao lado de `db` (nem ele nem o
/// `-wal`/`-shm` dele existem): tenta `-2`, `-3`... Nunca sobrescreve.
pub fn free_pre_rollback_name(db: &Path, stamp: &str) -> PathBuf {
    let taken = |p: &Path| {
        ["", "-wal", "-shm"]
            .iter()
            .any(|ext| Path::new(&format!("{}{ext}", p.display())).exists())
    };
    let base = PathBuf::from(format!("{}.pre-rollback-{stamp}", db.display()));
    if !taken(&base) {
        return base;
    }
    (2u32..)
        .map(|n| PathBuf::from(format!("{}-{n}", base.display())))
        .find(|p| !taken(p))
        .expect("algum sufixo livre")
}

/// Linhas por tabela de um banco (só leitura).
fn table_counts(path: &Path) -> std::result::Result<Vec<(String, i64)>, String> {
    use rusqlite::{Connection, OpenFlags};
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table'
             AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .map_err(|e| e.to_string())?;
    let names: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .and_then(|rows| rows.collect())
        .map_err(|e| e.to_string())?;
    names
        .into_iter()
        .map(|t| {
            let q = format!("SELECT count(*) FROM \"{}\"", t.replace('"', "\"\""));
            conn.query_row(&q, [], |r| r.get(0))
                .map(|n| (t, n))
                .map_err(|e| e.to_string())
        })
        .collect()
}

/// O que se perde ao voltar `cur` para `snap`: as tabelas com mais linhas
/// no banco em uso (ou que só existem nele), como `tabela +N`.
pub fn lost_rows(cur: &Path, snap: &Path) -> std::result::Result<Vec<String>, String> {
    let old: std::collections::HashMap<_, _> = table_counts(snap)?.into_iter().collect();
    Ok(table_counts(cur)?
        .into_iter()
        .filter_map(|(t, n)| {
            let before = old.get(&t).copied().unwrap_or(0);
            (n > before).then(|| format!("{t} +{}", n - before))
        })
        .collect())
}

/// `<update>/prev/prev.json`, gravado pela última atualização.
fn read_prev(update_dir: &Path) -> Result<Prev, String> {
    std::fs::read(update_dir.join("prev").join("prev.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or_else(|| "nenhuma versão anterior guardada".to_string())
}

/// Build para o qual um rollback voltaria; `None` = nada guardado.
pub fn previous(update_dir: &Path) -> Option<Build> {
    read_prev(update_dir).ok().map(|p| p.build)
}

/// Bancos que voltariam do snapshot: os com esquema maior que o do
/// backup (`(nome, caminho, esquema atual, esquema do snapshot)`).
fn dbs_to_restore(dbs: &[PathBuf], p: &Prev) -> Vec<(String, PathBuf, i64, i64)> {
    p.dbs
        .iter()
        .filter_map(|(name, v)| {
            let path = dbs.iter().find(|d| db_name(d) == *name)?;
            let cur = ct_ledger::db_user_version(path).unwrap_or(*v);
            (cur > *v).then(|| (name.clone(), path.clone(), cur, *v))
        })
        .collect()
}

/// O que um rollback manual tiraria dos bancos em uso: `None` se nenhum
/// banco volta do snapshot; senão a explicação (quando foi o snapshot, os
/// esquemas e as linhas que saem), para pedir confirmação (`--force` no
/// terminal, `force` no `hub.update.rollback`). Erro sem versão guardada.
pub fn db_rollback_loss(update_dir: &Path, dbs: &[PathBuf]) -> Result<Option<String>, String> {
    let p = read_prev(update_dir)?;
    let plan = dbs_to_restore(dbs, &p);
    if plan.is_empty() {
        return Ok(None);
    }
    let when = if p.taken_at > 0 {
        format!("{} UTC", ct_core::utc_stamp(p.taken_at))
    } else {
        "data desconhecida".into()
    };
    let mut parts = Vec::new();
    for (name, path, cur, v) in &plan {
        let snap = update_dir.join("db").join(name);
        let lost = lost_rows(path, &snap).unwrap_or_else(|e| vec![format!("? ({e})")]);
        let lost = if lost.is_empty() {
            "nenhuma linha a mais".into()
        } else {
            lost.join(", ")
        };
        parts.push(format!(
            "{name} volta ao snapshot de {when} (esquema {cur} → {v}); sai do banco em uso: {lost}"
        ));
    }
    Ok(Some(format!(
        "o rollback troca o banco: {}. O banco atual fica numa cópia .pre-rollback-*",
        parts.join("; ")
    )))
}

/// Rótulo de uma fase para o terminal.
pub fn label(p: Phase, service: &str) -> String {
    match p {
        Phase::Starting => "iniciando".into(),
        Phase::Fetch => "buscando o ref".into(),
        Phase::Build => "compilando".into(),
        Phase::Backup => "guardando binários e bancos".into(),
        Phase::Install => "instalando".into(),
        Phase::Restart => format!("reiniciando o {service}"),
        Phase::Verify => "conferindo a versão no ar".into(),
        Phase::Done => "concluído".into(),
        Phase::Failed => "falhou".into(),
        Phase::RolledBack => "revertido".into(),
    }
}

pub fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

/// Copia `src` para `<dir>/.<nome>.new` e o renomeia por cima de `dest`
/// (atômico no mesmo diretório; o executável em uso segue intacto).
pub fn replace_file(src: &Path, dest: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let name = dest.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dest.with_file_name(format!(".{name}.new"));
    std::fs::copy(src, &tmp)?;
    let mode = std::fs::metadata(src)?.permissions().mode();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    std::fs::rename(&tmp, dest)
}

pub struct Runner {
    source: Source,
    target: Target,
    state: UpdateState,
    log: std::fs::File,
}

fn db_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

impl Runner {
    /// `kind` é `update` ou `rollback`; `from` é o build no ar agora.
    pub fn new(kind: &str, source: Source, target: Target, from: Build) -> crate::Result<Self> {
        ct_core::files::private_dir(&source.dir)?;
        let log = std::fs::File::create(log_file(&source.dir))?;
        let now = ct_core::now_ms();
        Ok(Runner {
            state: UpdateState {
                kind: kind.into(),
                phase: Phase::Starting,
                git_ref: source.git_ref.clone(),
                from,
                to: None,
                started_at: now,
                updated_at: now,
                finished_at: None,
                message: None,
                error: None,
                log_tail: Vec::new(),
            },
            source,
            target,
            log,
        })
    }

    pub fn state(&self) -> &UpdateState {
        &self.state
    }

    fn dir(&self) -> &Path {
        &self.source.dir
    }

    fn note(&mut self, msg: &str) {
        let _ = writeln!(self.log, "{msg}");
        println!("{msg}");
    }

    fn phase(&mut self, p: Phase) {
        self.state.phase = p;
        self.state.updated_at = ct_core::now_ms();
        if p.is_terminal() {
            self.state.finished_at = Some(self.state.updated_at);
            if p != Phase::Done {
                self.state.log_tail = tail(&log_file(self.dir()), LOG_TAIL);
            }
        }
        let _ = write_state(self.dir(), &self.state);
        let l = label(p, &self.target.service);
        self.note(&format!("== {l}"));
    }

    /// Roda `c` com stdout/stderr no `update.log`.
    fn sh(&mut self, c: &mut Command) -> Result<(), String> {
        let line = format!(
            "$ {} {}",
            c.get_program().to_string_lossy(),
            c.get_args()
                .map(|a| a.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        );
        self.note(&line);
        let out = self.log.try_clone().map_err(|e| e.to_string())?;
        let err = self.log.try_clone().map_err(|e| e.to_string())?;
        let st = c
            .stdout(out)
            .stderr(err)
            .status()
            .map_err(|e| format!("{line}: {e}"))?;
        if st.success() {
            Ok(())
        } else {
            Err(format!("{line} falhou ({st})"))
        }
    }

    fn ctl(&mut self, a: Action) -> Result<(), String> {
        let mut c = (self.target.ctl)(a);
        self.sh(&mut c)
    }

    fn fail(&mut self, phase: Phase, err: String) -> i32 {
        self.note(&format!("erro: {err}"));
        self.state.error = Some(err);
        self.phase(phase);
        1
    }

    /// Espera a sonda responder saudável com `want`.
    fn verify(&mut self, want: &Build) -> Result<(), String> {
        let timeout = self.target.verify_timeout;
        let deadline = Instant::now() + timeout;
        let mut last = None;
        let mut last_err = None;
        while Instant::now() < deadline {
            match (self.target.probe)() {
                Ok(b) => {
                    let ok = if want.commit.is_empty() {
                        b.version == want.version
                    } else {
                        b.commit == want.commit
                    };
                    if ok {
                        return Ok(());
                    }
                    last = Some(b);
                    last_err = None;
                }
                Err(e) => last_err = Some(e),
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let service = &self.target.service;
        Err(match (last, last_err) {
            (_, Some(e)) => format!(
                "o {service} não ficou saudável em {}s depois do restart: {e}",
                timeout.as_secs()
            ),
            (Some(b), None) => format!(
                "o daemon respondeu com {} ({}), não com {} ({})",
                b.version,
                short(&b.commit),
                want.version,
                short(&want.commit)
            ),
            (None, None) => format!(
                "o {service} não respondeu em {}s depois do restart",
                timeout.as_secs()
            ),
        })
    }

    /// Atualiza para o commit do ref. Devolve o código de saída.
    pub fn update(&mut self) -> i32 {
        // ferramentas do pré-build: falha cedo, sem fetch nem build
        if let Some(pre) = &self.target.pre_build {
            if let Err(e) = (pre.check)() {
                return self.fail(Phase::Failed, e);
            }
        }

        // fetch
        self.phase(Phase::Fetch);
        let commit = match self.source.fetch() {
            Ok(c) => c,
            Err(e) => return self.fail(Phase::Failed, e.to_string()),
        };
        let src = self.source.src();
        if let Err(e) =
            self.sh(git(&src).args(["checkout", "--quiet", "--detach", "--force", &commit]))
        {
            return self.fail(Phase::Failed, e);
        }
        let version = std::fs::read_to_string(src.join("Cargo.toml"))
            .ok()
            .and_then(|m| workspace_version(&m))
            .unwrap_or_default();
        let to = Build { version, commit };
        self.state.to = Some(to.clone());
        if !self.state.from.commit.is_empty() && self.state.from.commit == to.commit {
            self.state.message = Some("já está em dia".into());
            self.phase(Phase::Done);
            return 0;
        }

        // build (falha aqui não toca em nada)
        self.phase(Phase::Build);
        let stage = self.dir().join("stage");
        let _ = std::fs::remove_dir_all(&stage);
        let from_release = self.stage_from_release(&to, &stage);
        let (pre_cmds, cargo_env) = match (&self.target.pre_build, from_release) {
            (Some(pre), false) => ((pre.commands)(&src), pre.cargo_env.clone()),
            _ => (Vec::new(), Vec::new()),
        };
        for mut c in pre_cmds {
            if let Err(e) = self.sh(&mut c) {
                return self.fail(Phase::Failed, e);
            }
        }
        for b in self.target.bins.clone() {
            if from_release {
                break;
            }
            let r = self.sh(Command::new(&self.target.cargo)
                .args(["install", "--locked", "--force", "--path"])
                .arg(src.join("crates").join(&b))
                .arg("--root")
                .arg(&stage)
                .env("CARGO_TARGET_DIR", self.dir().join("target"))
                .env("CTOWER_COMMIT", &to.commit)
                .envs(cargo_env.iter().map(|(k, v)| (k, v))));
            if let Err(e) = r {
                return self.fail(Phase::Failed, e);
            }
        }
        for b in self.target.bins.clone() {
            if !stage.join("bin").join(&b).is_file() {
                return self.fail(Phase::Failed, format!("o build não gerou {b}"));
            }
        }

        // backup dos binários e dos bancos
        self.phase(Phase::Backup);
        if let Err(e) = self.backup() {
            return self.fail(Phase::Failed, e);
        }

        // install
        self.phase(Phase::Install);
        if let Err(e) = self.install_from(&stage.join("bin")) {
            let msg = format!("instalação falhou: {e}");
            return self.restore_after(msg);
        }

        // restart + verify
        self.phase(Phase::Restart);
        if let Err(e) = self.ctl(Action::Restart) {
            return self.restore_after(e);
        }
        self.phase(Phase::Verify);
        match self.verify(&to) {
            Ok(()) => {
                self.note(&format!(
                    "{} {} ({}) no ar",
                    self.target.service,
                    to.version,
                    short(&to.commit)
                ));
                self.phase(Phase::Done);
                0
            }
            Err(e) => self.restore_after(e),
        }
    }

    /// Tenta preencher `<stage>/bin` com o asset da release da versão `to`
    /// (sha256 conferido). Só vale se a tag da versão aponta para o commit
    /// alvo; devolve false (e o build do fonte segue) em qualquer falha.
    fn stage_from_release(&mut self, to: &Build, stage: &Path) -> bool {
        let Some(prefix) = self.target.release_prefix.clone() else {
            return false;
        };
        if to.version.is_empty() {
            return false;
        }
        let tag = format!("v{}", to.version);
        let src = self.source.src();
        let _ = self.sh(git(&src).args(["fetch", "--quiet", "--force", "origin", "tag", &tag]));
        let tag_commit = crate::run(git(&src).args(["rev-parse", &format!("{tag}^{{commit}}")]));
        if tag_commit.as_deref().ok() != Some(to.commit.as_str()) {
            self.note(&format!("{tag} não é o commit alvo; compilando do fonte"));
            return false;
        }
        let arch = match std::env::consts::ARCH {
            "arm64" => "aarch64",
            a => a,
        };
        let os = if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        };
        let name = format!("{prefix}-{tag}-{arch}-{os}");
        let repo = self.source.repo.trim_end_matches(".git");
        let repo = repo
            .split_once("github.com")
            .map(|(_, r)| r.trim_start_matches([':', '/']))
            .unwrap_or(repo)
            .to_string();
        let tmp = self.dir().join("release");
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::create_dir_all(&tmp);
        let ok = self
            .sh(Command::new("gh")
                .args(["release", "download", &tag, "--repo", &repo, "--dir"])
                .arg(&tmp)
                .args(["--pattern", &format!("{name}.tar.gz")])
                .args(["--pattern", &format!("{name}.tar.gz.sha256")]))
            .is_ok()
            && self
                .sh(Command::new("sh").current_dir(&tmp).args([
                    "-c",
                    &format!(
                        "if command -v sha256sum >/dev/null; then sha256sum -c {name}.tar.gz.sha256; else shasum -a 256 -c {name}.tar.gz.sha256; fi"
                    ),
                ]))
                .is_ok()
            && self
                .sh(Command::new("tar").arg("-xzf").arg(tmp.join(format!("{name}.tar.gz"))).arg("-C").arg(&tmp))
                .is_ok();
        let bin = stage.join("bin");
        let done = ok
            && std::fs::create_dir_all(&bin).is_ok()
            && self
                .target
                .bins
                .iter()
                .all(|b| std::fs::copy(tmp.join(&name).join(b), bin.join(b)).is_ok());
        let _ = std::fs::remove_dir_all(&tmp);
        if done {
            self.note(&format!("binários de {tag} baixados da release"));
        } else {
            let _ = std::fs::remove_dir_all(stage);
            self.note("sem asset utilizável na release; compilando do fonte");
        }
        done
    }

    fn prev_dir(&self) -> PathBuf {
        self.dir().join("prev")
    }

    fn backup(&mut self) -> Result<(), String> {
        let prev = self.prev_dir();
        let _ = std::fs::remove_dir_all(&prev);
        std::fs::create_dir_all(&prev).map_err(|e| e.to_string())?;
        for b in &self.target.bins {
            let cur = self.target.bin_dir.join(b);
            if cur.is_file() {
                std::fs::copy(&cur, prev.join(b)).map_err(|e| format!("{}: {e}", cur.display()))?;
            }
        }
        let db_dir = self.dir().join("db");
        std::fs::create_dir_all(&db_dir).map_err(|e| e.to_string())?;
        let mut dbs = Vec::new();
        for path in self.target.dbs.clone() {
            if !path.exists() {
                continue;
            }
            let name = db_name(&path);
            let v = ct_ledger::snapshot_db(&path, &db_dir.join(&name))
                .map_err(|e| format!("snapshot de {name}: {e}"))?;
            self.note(&format!("snapshot de {name} (esquema {v})"));
            dbs.push((name, v));
        }
        let p = Prev {
            build: self.state.from.clone(),
            dbs,
            taken_at: ct_core::now_ms(),
        };
        std::fs::write(
            prev.join("prev.json"),
            serde_json::to_vec_pretty(&p).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
    }

    fn install_from(&mut self, dir: &Path) -> Result<(), String> {
        for b in self.target.bins.clone() {
            let src = dir.join(&b);
            if !src.is_file() {
                continue;
            }
            let dest = self.target.bin_dir.join(&b);
            replace_file(&src, &dest).map_err(|e| format!("{b}: {e}"))?;
            self.note(&format!("instalado {}", dest.display()));
        }
        Ok(())
    }

    fn read_prev(&self) -> Result<Prev, String> {
        read_prev(self.dir())
    }

    fn dbs_to_restore(&self, p: &Prev) -> Vec<(String, PathBuf, i64, i64)> {
        dbs_to_restore(&self.target.dbs, p)
    }

    /// Volta os binários (e os bancos que migraram) de `prev/`, com o
    /// serviço parado, e o sobe de novo. Antes de trocar um banco, guarda o
    /// atual (cópia consistente, com o WAL) em `<banco>.pre-rollback-<stamp>`.
    fn restore(&mut self) -> Result<Build, String> {
        let prev = self.prev_dir();
        let p = self.read_prev()?;
        let _ = self.ctl(Action::Stop);
        let plan = self.dbs_to_restore(&p);
        let stamp = ct_core::utc_stamp(ct_core::now_ms());
        let mut kept = Vec::new();
        for (name, path, _, _) in &plan {
            let pre = free_pre_rollback_name(path, &stamp);
            if let Err(e) = ct_ledger::snapshot_db(path, &pre) {
                // Sem a cópia, não troca nada: volta o serviço como estava.
                let _ = self.ctl(Action::Start);
                return Err(format!("cópia de {name} antes do rollback: {e}"));
            }
            self.note(&format!("{name} atual guardado em {}", pre.display()));
            kept.push(pre);
        }
        self.install_from(&prev)?;
        for (i, (name, path, cur, v)) in plan.iter().enumerate() {
            let snap = self.dir().join("db").join(name);
            replace_file(&snap, path).map_err(|e| format!("{name}: {e}"))?;
            for ext in ["-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
            }
            self.note(&format!(
                "{name} restaurado do snapshot (esquema {cur} → {v}); o que entrou depois \
                 do snapshot ficou em {}",
                kept[i].display()
            ));
            if let Some(hook) = &self.target.after_db_restore {
                match hook(path) {
                    Ok(msg) if !msg.is_empty() => self.note(&msg),
                    Ok(_) => {}
                    Err(e) => self.note(&format!("aviso: {name}: {e}")),
                }
            }
        }
        if !plan.is_empty() {
            if let Some(n) = self.target.db_rollback_note.clone() {
                self.note(&n);
            }
        }
        self.ctl(Action::Start)?;
        Ok(p.build)
    }

    /// Falha depois de mexer nos binários: rollback automático.
    fn restore_after(&mut self, err: String) -> i32 {
        self.note(&format!("erro: {err}; voltando à versão anterior"));
        self.state.error = Some(err);
        match self.restore() {
            Ok(b) => {
                if let Err(e) = self.verify(&b) {
                    self.note(&format!("aviso: {e}"));
                }
                self.phase(Phase::RolledBack);
            }
            Err(e) => {
                let first = self.state.error.take().unwrap_or_default();
                self.state.error = Some(format!("{first}; rollback falhou: {e}"));
                self.phase(Phase::Failed);
            }
        }
        1
    }

    /// Sem `force` e com `force_db_rollback`, recusa voltar bancos do
    /// snapshot e diz o que se perderia.
    fn confirm_db_rollback(&self, force: bool) -> Result<(), String> {
        if force || !self.target.force_db_rollback {
            return Ok(());
        }
        match db_rollback_loss(self.dir(), &self.target.dbs)? {
            None => Ok(()),
            Some(loss) => Err(format!("{loss}. Para seguir, rode de novo com --force")),
        }
    }

    /// Rollback manual: volta para os binários de `prev/`. Com bancos que
    /// migraram e `force_db_rollback`, exige `force`.
    pub fn rollback(&mut self, force: bool) -> i32 {
        if let Err(e) = self.confirm_db_rollback(force) {
            return self.fail(Phase::Failed, e);
        }
        self.phase(Phase::Install);
        match self.restore() {
            Ok(b) => {
                self.state.to = Some(b.clone());
                self.phase(Phase::Verify);
                match self.verify(&b) {
                    Ok(()) => {
                        self.phase(Phase::Done);
                        0
                    }
                    Err(e) => self.fail(Phase::Failed, e),
                }
            }
            Err(e) => self.fail(Phase::Failed, e),
        }
    }
}

/// Resumo final de uma atualização no terminal; devolve o código de saída.
pub fn report(s: &UpdateState, service: &str) -> i32 {
    let to =
        s.to.as_ref()
            .map(|b| format!("{} ({})", b.version, short(&b.commit)))
            .unwrap_or_default();
    match s.phase {
        Phase::Done => {
            match &s.message {
                Some(m) => println!("Concluído: {m}."),
                None => println!("Concluído: {service} {to} no ar."),
            }
            0
        }
        _ => {
            let verb = if s.phase == Phase::RolledBack {
                "Falhou e voltou à versão anterior"
            } else {
                "Falhou; a versão anterior segue no ar"
            };
            eprintln!("{verb}: {}", s.error.as_deref().unwrap_or("?"));
            for l in &s.log_tail {
                eprintln!("  | {l}");
            }
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nome_pre_rollback_nunca_sobrescreve() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("hub.db");
        let base = d.path().join("hub.db.pre-rollback-20261018-1200");
        assert_eq!(free_pre_rollback_name(&db, "20261018-1200"), base);
        std::fs::write(format!("{}-wal", base.display()), "x").unwrap();
        assert_eq!(
            free_pre_rollback_name(&db, "20261018-1200"),
            d.path().join("hub.db.pre-rollback-20261018-1200-2")
        );
    }

    #[test]
    fn linhas_perdidas_por_tabela() {
        let d = tempfile::tempdir().unwrap();
        let (snap, cur) = (d.path().join("snap.db"), d.path().join("cur.db"));
        let c = rusqlite::Connection::open(&snap).unwrap();
        c.execute_batch("CREATE TABLE a (x); INSERT INTO a VALUES (1);")
            .unwrap();
        drop(c);
        std::fs::copy(&snap, &cur).unwrap();
        let c = rusqlite::Connection::open(&cur).unwrap();
        c.execute_batch(
            "INSERT INTO a VALUES (2); INSERT INTO a VALUES (3);
             CREATE TABLE \"b c\" (y); INSERT INTO \"b c\" VALUES (1);",
        )
        .unwrap();
        drop(c);
        assert_eq!(lost_rows(&cur, &snap).unwrap(), vec!["a +2", "b c +1"]);
        assert!(lost_rows(&snap, &snap).unwrap().is_empty());
    }
}
