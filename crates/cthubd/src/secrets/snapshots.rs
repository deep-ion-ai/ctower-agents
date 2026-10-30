//! Snapshots cifrados do store de secrets (issue #168, épico #171).
//!
//! O store mestre (`secrets.age`) vive na máquina do hub; perder a máquina
//! leva o store junto. Como o arquivo já é cifrado para a chave do hub e,
//! com a recuperação configurada (#169), para a chave de recuperação, uma
//! cópia byte a byte dele pode ir para uma pasta sincronizada (OneDrive,
//! iCloud) sem expor nada.
//!
//! - **Quando:** logo depois de cada `set`/`delete` bem-sucedido, ainda com
//!   o lock do store (fora do lock do hub), e na conferência periódica
//!   ([`CHECK_INTERVAL`], a primeira no start) se o mais recente não bate
//!   com o store. Uma falha nunca desfaz nem falha a alteração local: vira
//!   aviso em `hub.status`.
//! - **Onde:** `secrets_snapshot_dir` do config (vazio = o `backup_dir` em
//!   vigor dos snapshots do `hub.db`, inclusive o que a UI muda: um destino
//!   só para apontar à pasta sincronizada), como `secrets-<AAAAMMDDTHHMMSSZ>.age` (UTC; `-2`, `-3`
//!   em colisão), 0600, escrita atômica.
//! - **Rotação:** os [`KEEP_RECENT`] mais recentes mais o mais recente de
//!   cada dia (UTC) dos últimos [`KEEP_DAYS`] dias. Só mexe em
//!   `secrets-*.age`; a rotação do `hub.db` é outra.
//!
//! O estado (último sucesso, último erro, "o mais recente não bate") fica
//! em memória, atrás de um lock folha: `hub.status` lê os avisos segurando
//! o lock do hub, sem tocar em disco.

use crate::backup::Warning;
use ct_secrets::{Identity, Recipient};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// Quantos snapshots mais recentes ficam, sempre.
pub const KEEP_RECENT: usize = 20;
/// Além deles, o mais recente de cada dia destes últimos dias (UTC).
pub const KEEP_DAYS: i64 = 30;
/// Intervalo da conferência periódica (destino acessível e mais recente
/// igual ao store).
pub const CHECK_INTERVAL: Duration = Duration::from_secs(3600);

const PREFIX: &str = "secrets-";
const SUFFIX: &str = ".age";
const DAY_MS: i64 = 86_400_000;

/// Um snapshot em disco, com o carimbo e a sequência do nome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub(super) path: PathBuf,
    /// Carimbo do nome (ms, UTC, resolução de segundo).
    pub(super) at_ms: i64,
    /// `1` sem sufixo; `n` para `-n`.
    seq: u32,
}

/// `AAAAMMDDTHHMMSSZ` em UTC.
fn stamp(ms: i64) -> String {
    let (y, m, d) = ct_core::civil_from_days(ms.div_euclid(DAY_MS));
    let s = ms.rem_euclid(DAY_MS) / 1000;
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

/// Dias desde 1970-01-01 de uma data gregoriana (inverso de
/// [`ct_core::civil_from_days`], Howard Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `(ms, seq)` de um nome `secrets-<carimbo>[-n].age`; `None` se não é
/// snapshot de secrets.
fn parse_name(name: &str) -> Option<(i64, u32)> {
    let rest = name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    let (st, seq) = match rest.split_once('-') {
        Some((st, n)) => {
            let n: u32 = n.parse().ok().filter(|n| *n >= 2)?;
            (st, n)
        }
        None => (rest, 1),
    };
    let b = st.as_bytes();
    let digits = |r: std::ops::Range<usize>| -> Option<i64> {
        let s = std::str::from_utf8(b.get(r)?).ok()?;
        s.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| s.parse().ok())?
    };
    if b.len() != 16 || b[8] != b'T' || b[15] != b'Z' {
        return None;
    }
    let (y, mo, d) = (digits(0..4)?, digits(4..6)?, digits(6..8)?);
    let (h, mi, s) = (digits(9..11)?, digits(11..13)?, digits(13..15)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 59 {
        return None;
    }
    let ms = days_from_civil(y, mo, d) * DAY_MS + (h * 3600 + mi * 60 + s) * 1000;
    Some((ms, seq))
}

/// Snapshots de secrets em `dir`, do mais recente para o mais antigo.
/// Diretório ausente = nenhum.
pub(super) fn list(dir: &Path) -> io::Result<Vec<Snapshot>> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out: Vec<Snapshot> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter_map(|path| {
            let (at_ms, seq) = parse_name(path.file_name()?.to_str()?)?;
            path.is_file().then_some(Snapshot { path, at_ms, seq })
        })
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse((s.at_ms, s.seq)));
    Ok(out)
}

/// Os que a rotação apaga de `snaps` (mais recente primeiro): fica cada um
/// dos [`KEEP_RECENT`] primeiros e o primeiro (mais recente) de cada dia
/// UTC de `now` até [`KEEP_DAYS`] − 1 dias antes.
fn rotation_victims(snaps: &[Snapshot], now_ms: i64) -> Vec<&Snapshot> {
    let first_day = now_ms.div_euclid(DAY_MS) - (KEEP_DAYS - 1);
    let mut days = HashSet::new();
    snaps
        .iter()
        .enumerate()
        .filter(|(i, s)| {
            let day = s.at_ms.div_euclid(DAY_MS);
            let day_keeper = day >= first_day && days.insert(day);
            *i >= KEEP_RECENT && !day_keeper
        })
        .map(|(_, s)| s)
        .collect()
}

/// Aplica a rotação em `dir`. Devolve os apagados.
pub(super) fn rotate(dir: &Path, now_ms: i64) -> io::Result<Vec<PathBuf>> {
    let snaps = list(dir)?;
    let mut removed = Vec::new();
    for s in rotation_victims(&snaps, now_ms) {
        std::fs::remove_file(&s.path)?;
        removed.push(s.path.clone());
    }
    Ok(removed)
}

/// Resultado de [`rekey`].
#[derive(Debug, Default)]
pub(super) struct Rekeyed {
    /// Recifrados para os destinatários novos.
    pub(super) reencrypted: usize,
    /// Apagados: nenhuma das chaves abre ou não deu para recifrar.
    pub(super) removed: Vec<PathBuf>,
}

/// Rotação das chaves (`cthubd secrets rotate-keys`, #170): recifra cada
/// snapshot de `dir` para `recipients`, abrindo com a primeira de `keys`
/// que servir, no lugar e com o mesmo mtime (a rotação continua igual). Os
/// que nenhuma chave abre e os que não dá para recifrar são **apagados**:
/// depois da rotação, nada na pasta fica preso a uma chave que deixou de
/// valer. `Err` só se não dá para listar o destino ou apagar um deles.
pub(super) fn rekey(
    dir: &Path,
    keys: &[&Identity],
    recipients: &[Recipient],
) -> io::Result<Rekeyed> {
    let mut out = Rekeyed::default();
    for s in list(dir)? {
        let reencrypt = || -> Result<(), String> {
            let bytes = std::fs::read(&s.path).map_err(|e| e.to_string())?;
            let plain = keys
                .iter()
                .find_map(|k| ct_secrets::decrypt(&bytes, k).ok())
                .ok_or("nenhuma chave conhecida abre")?;
            let sealed = ct_secrets::encrypt(&plain, recipients).map_err(|e| e.to_string())?;
            let mtime = std::fs::metadata(&s.path)
                .and_then(|m| m.modified())
                .map_err(|e| e.to_string())?;
            ct_core::files::write_private_atomic(&s.path, &sealed).map_err(|e| e.to_string())?;
            std::fs::File::options()
                .write(true)
                .open(&s.path)
                .and_then(|f| f.set_modified(mtime))
                .map_err(|e| e.to_string())
        };
        match reencrypt() {
            Ok(()) => out.reencrypted += 1,
            Err(e) => {
                tracing::warn!("{}: {e}; snapshot removido", s.path.display());
                std::fs::remove_file(&s.path)?;
                out.removed.push(s.path);
            }
        }
    }
    Ok(out)
}

/// Primeiro nome livre para o carimbo de `now_ms`: sem sufixo, depois
/// `-2`, `-3`...
fn free_name(dir: &Path, now_ms: i64) -> PathBuf {
    let st = stamp(now_ms);
    let mut p = dir.join(format!("{PREFIX}{st}{SUFFIX}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{PREFIX}{st}-{n}{SUFFIX}"));
        n += 1;
    }
    p
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Cria `dir` (0700) se preciso e confere que dá para gravar nele.
fn probe(dir: &Path) -> io::Result<()> {
    ct_core::files::private_dir(dir)?;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    // SAFETY: `c` é uma string C válida durante a chamada.
    if unsafe { libc::access(c.as_ptr(), libc::W_OK | libc::X_OK) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[derive(Debug, Default)]
struct State {
    last_ok_at: Option<i64>,
    /// Última cópia ou conferência que falhou (destino inacessível).
    error: Option<String>,
    /// O snapshot mais recente não bate com o store (ou não existe com o
    /// store gravado).
    stale: bool,
}

#[derive(Debug, Clone, Default)]
struct Target {
    dir: Option<PathBuf>,
    same_device: bool,
}

/// Snapshots de secrets de um hub. Desligado (sem destino) nos testes que
/// não tratam deles.
pub(super) struct Snapshots {
    /// Muda em execução quando o destino segue o `backup_dir` da UI.
    target: Mutex<Target>,
    state: Mutex<State>,
}

impl Snapshots {
    pub(super) fn disabled() -> Self {
        Snapshots {
            target: Mutex::new(Target::default()),
            state: Mutex::new(State::default()),
        }
    }

    /// `store` = caminho do `secrets.age`, para o aviso de mesmo disco.
    pub(super) fn new(dir: PathBuf, store: &Path) -> Self {
        Snapshots {
            target: Mutex::new(Target {
                same_device: crate::backup::same_device(store, &dir),
                dir: Some(dir),
            }),
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn target(&self) -> Target {
        self.target
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(super) fn dir(&self) -> Option<PathBuf> {
        self.target().dir
    }

    /// Troca o destino (ligado) por `dir` e zera o estado dos avisos; a
    /// próxima conferência tira o snapshot no destino novo. Chame com o
    /// lock do store. `false` = desligado ou já era `dir`.
    pub(super) fn retarget(&self, dir: &Path, store: &Path) -> bool {
        let mut t = self.target.lock().unwrap_or_else(|e| e.into_inner());
        if t.dir.as_deref().is_none_or(|d| d == dir) {
            return false;
        }
        *t = Target {
            same_device: crate::backup::same_device(store, dir),
            dir: Some(dir.to_path_buf()),
        };
        drop(t);
        *self.state() = State::default();
        true
    }

    /// O snapshot mais recente, se houver (erro de leitura do destino =
    /// nenhum).
    pub(super) fn newest(&self) -> Option<PathBuf> {
        list(&self.dir()?).ok()?.into_iter().next().map(|s| s.path)
    }

    /// O mais recente tem o mesmo conteúdo que `store`? Store ausente
    /// (nunca gravado) conta como "bate".
    fn matches(dir: &Path, store: &Path) -> io::Result<bool> {
        let bytes = match std::fs::read(store) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(true),
            Err(e) => return Err(e),
        };
        let Some(newest) = list(dir)?.into_iter().next() else {
            return Ok(false);
        };
        Ok(sha256(&std::fs::read(&newest.path)?) == sha256(&bytes))
    }

    /// Copia `store` para um snapshot novo, se o mais recente não for igual
    /// a ele, e roda a rotação. Chame com o lock do store. `Ok(None)` =
    /// desligado, store ausente ou já igual. O resultado vai para o estado
    /// dos avisos.
    pub(super) fn take(&self, store: &Path) -> Result<Option<PathBuf>, String> {
        let Some(dir) = self.dir() else {
            return Ok(None);
        };
        let dir = dir.as_path();
        let r = (|| -> io::Result<Option<PathBuf>> {
            let bytes = match std::fs::read(store) {
                Ok(b) => b,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e),
            };
            probe(dir)?;
            if let Some(newest) = list(dir)?.first() {
                if sha256(&std::fs::read(&newest.path)?) == sha256(&bytes) {
                    return Ok(None);
                }
            }
            let now = ct_core::now_ms();
            let path = free_name(dir, now);
            ct_core::files::write_private_atomic(&path, &bytes)?;
            let removed = rotate(dir, now)?;
            if !removed.is_empty() {
                tracing::debug!("{} snapshot(s) de secrets antigos removidos", removed.len());
            }
            Ok(Some(path))
        })();
        let mut st = self.state();
        match &r {
            Ok(_) => {
                st.last_ok_at = Some(ct_core::now_ms());
                st.error = None;
                st.stale = false;
            }
            Err(e) => {
                st.error = Some(format!("{}: {e}", dir.display()));
                st.stale = true;
            }
        }
        r.map_err(|e| format!("{}: {e}", dir.display()))
    }

    /// Conferência periódica: destino acessível e mais recente igual ao
    /// store. Se o destino responde e o mais recente não bate, tira um
    /// snapshot na hora (é o que normaliza depois de uma falha). Chame com
    /// o lock do store.
    pub(super) fn check(&self, store: &Path) {
        let Some(dir) = self.dir() else {
            return;
        };
        let dir = dir.as_path();
        if let Err(e) = probe(dir) {
            tracing::warn!(
                "snapshots de secrets: destino inacessível: {}: {e}",
                dir.display()
            );
            let stale = !Self::matches(dir, store).unwrap_or(false);
            let mut st = self.state();
            st.error = Some(format!("{}: {e}", dir.display()));
            st.stale = stale;
            return;
        }
        match Self::matches(dir, store) {
            Ok(true) => {
                let mut st = self.state();
                st.error = None;
                st.stale = false;
            }
            Ok(false) => match self.take(store) {
                Ok(Some(p)) => tracing::info!("snapshot de secrets em {}", p.display()),
                Ok(None) => {}
                Err(e) => tracing::warn!("snapshot de secrets falhou: {e}"),
            },
            Err(e) => {
                let mut st = self.state();
                st.error = Some(format!("{}: {e}", dir.display()));
                st.stale = true;
            }
        }
    }

    /// Recifra cada snapshot para `recipients` (hub + recuperação), com a
    /// chave do hub. Os iguais a `store` (já cifrados para os destinatários
    /// atuais) e os que a chave do hub não abre ficam como estão. Mantém
    /// nome e mtime. Devolve quantos recifrou.
    pub(super) fn reencrypt(
        &self,
        hub: &Identity,
        recipients: &[Recipient],
        store: &Path,
    ) -> Result<usize, String> {
        let Some(dir) = self.dir() else {
            return Ok(0);
        };
        let dir = dir.as_path();
        let r = (|| -> Result<usize, String> {
            let err = |p: &Path, e: &dyn std::fmt::Display| format!("{}: {e}", p.display());
            let current = std::fs::read(store).ok().map(|b| sha256(&b));
            let mut n = 0;
            for s in list(dir).map_err(|e| err(dir, &e))? {
                let bytes = std::fs::read(&s.path).map_err(|e| err(&s.path, &e))?;
                if Some(sha256(&bytes)) == current {
                    continue;
                }
                let plain = match ct_secrets::decrypt(&bytes, hub) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("{}: não abre com a chave do hub ({e})", s.path.display());
                        continue;
                    }
                };
                let sealed =
                    ct_secrets::encrypt(&plain, recipients).map_err(|e| err(&s.path, &e))?;
                let mtime = std::fs::metadata(&s.path)
                    .and_then(|m| m.modified())
                    .map_err(|e| err(&s.path, &e))?;
                ct_core::files::write_private_atomic(&s.path, &sealed)
                    .map_err(|e| err(&s.path, &e))?;
                std::fs::File::options()
                    .write(true)
                    .open(&s.path)
                    .and_then(|f| f.set_modified(mtime))
                    .map_err(|e| err(&s.path, &e))?;
                n += 1;
            }
            Ok(n)
        })();
        if let Err(e) = &r {
            self.state().error = Some(e.clone());
        }
        r
    }

    /// Avisos de `hub.status`: destino no mesmo disco do store, destino
    /// inacessível (ou última cópia falhou) e mais recente desatualizado.
    /// Só pega o lock do estado.
    pub(super) fn warnings(&self) -> Vec<Warning> {
        let Target {
            dir: Some(dir),
            same_device,
        } = self.target()
        else {
            return Vec::new();
        };
        let st = self.state();
        let mut out = Vec::new();
        if same_device {
            out.push(Warning {
                kind: "secrets_snapshot_same_device",
                message: format!(
                    "snapshots de secrets no mesmo disco do store: {} (aponte \
                     secrets_snapshot_dir ou backup_dir para uma pasta sincronizada \
                     ou configure um destino remoto)",
                    dir.display()
                ),
            });
        }
        if let Some(e) = &st.error {
            out.push(Warning {
                kind: "secrets_snapshot_unreachable",
                message: format!(
                    "destino dos snapshots de secrets inacessível: {e}. As alterações \
                     continuam gravadas no hub; o snapshot sai quando o destino voltar"
                ),
            });
        }
        if st.stale {
            out.push(Warning {
                kind: "secrets_snapshot_stale",
                message: "o snapshot mais recente dos secrets não corresponde ao store: a \
                          última alteração ainda não tem cópia fora da máquina"
                    .into(),
            });
        }
        out
    }

    #[cfg(test)]
    pub(super) fn last_ok_at(&self) -> Option<i64> {
        self.state().last_ok_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nomes_e_carimbos() {
        // 2026-10-19T15:30:12Z
        let ms = 1_792_423_812_000;
        assert_eq!(stamp(ms), "20261019T153012Z");
        assert_eq!(parse_name("secrets-20261019T153012Z.age"), Some((ms, 1)));
        assert_eq!(parse_name("secrets-20261019T153012Z-3.age"), Some((ms, 3)));
        for ruim in [
            "secrets.age",
            "secrets-20261019T153012Z-1.age",
            "secrets-20261019T153012.age",
            "secrets-20261319T153012Z.age",
            "hub-20261019-1530.db",
            "secrets-20261019T153012Z.age.tmp",
        ] {
            assert_eq!(parse_name(ruim), None, "{ruim}");
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        for z in [-800_000, -1, 0, 59, 20_000, 2_000_000] {
            let (y, m, d) = ct_core::civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
    }

    #[test]
    fn colisao_no_mesmo_segundo_ganha_sufixo_e_ordena() {
        let d = tempfile::tempdir().unwrap();
        let ms = 1_792_423_812_000;
        let mut nomes = Vec::new();
        for _ in 0..11 {
            let p = free_name(d.path(), ms);
            std::fs::write(&p, "x").unwrap();
            nomes.push(p);
        }
        assert!(nomes[1].ends_with("secrets-20261019T153012Z-2.age"));
        // `-10` é mais recente que `-9` (ordem numérica, não de texto).
        let l = list(d.path()).unwrap();
        assert_eq!(l[0].path, nomes[10]);
        assert_eq!(l[1].path, nomes[9]);
        assert_eq!(l[10].path, nomes[0]);
    }

    #[test]
    fn rotacao_20_recentes_e_um_por_dia_em_30_dias() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        // Agora: 2026-10-19T15:30:12Z.
        let now = 1_792_423_812_000;
        let put = |ms: i64| {
            let p = free_name(dir, ms);
            std::fs::write(&p, "x").unwrap();
            p
        };
        // 25 de hoje, a cada minuto: os 20 mais recentes ficam; dos 5
        // restantes, nenhum é o mais recente do dia.
        let hoje: Vec<PathBuf> = (0..25).map(|i| put(now - i * 60_000)).collect();
        // Dois por dia em cada um dos 40 dias anteriores (10h e 22h... do
        // dia), espalhados.
        let mut por_dia = Vec::new();
        for dia in 1..=40 {
            let base = (now.div_euclid(DAY_MS) - dia) * DAY_MS;
            let cedo = put(base + 10 * 3_600_000);
            let tarde = put(base + 22 * 3_600_000);
            por_dia.push((dia, cedo, tarde));
        }
        // Outros arquivos não são tocados.
        for outro in ["secrets.age", "hub-20261019-1530.db", "notas.txt"] {
            std::fs::write(dir.join(outro), "x").unwrap();
        }

        let removed = rotate(dir, now).unwrap();
        let sobra: HashSet<PathBuf> = list(dir).unwrap().into_iter().map(|s| s.path).collect();
        for (i, p) in hoje.iter().enumerate() {
            assert_eq!(sobra.contains(p), i < 20, "hoje #{i}");
        }
        for (dia, cedo, tarde) in &por_dia {
            // Dias 1..=29 estão na janela de 30 dias (com hoje).
            assert_eq!(sobra.contains(tarde), *dia <= 29, "dia -{dia} 22h");
            assert!(!sobra.contains(cedo), "dia -{dia} 10h");
        }
        assert_eq!(sobra.len(), 20 + 29);
        assert_eq!(removed.len(), 25 + 80 - sobra.len());
        for outro in ["secrets.age", "hub-20261019-1530.db", "notas.txt"] {
            assert!(dir.join(outro).exists(), "{outro}");
        }
        // Rodar de novo não apaga mais nada.
        assert!(rotate(dir, now).unwrap().is_empty());

        // Poucos arquivos antigos: os 20 mais recentes ficam mesmo fora da
        // janela.
        let d2 = tempfile::tempdir().unwrap();
        for dia in 0..15 {
            std::fs::write(free_name(d2.path(), now - (100 + dia) * DAY_MS), "x").unwrap();
        }
        assert!(rotate(d2.path(), now).unwrap().is_empty());
    }
}
