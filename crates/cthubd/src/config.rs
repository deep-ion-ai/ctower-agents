//! Config e diretórios do `cthubd` (spec do hub como plano de controle §10):
//! `~/.config/cthub/config.toml` e `~/.local/share/cthub/`, separados do
//! `ctowerd`.

use serde::Deserialize;
use std::io;
use std::path::PathBuf;

pub const DEFAULT_PORT: u16 = 7879;

#[derive(Debug, Clone)]
pub struct Dirs {
    /// `config.toml`, `ui.token`, `github.token`, `hub_id`, `vapid.key` e o store de
    /// secrets (`secrets.age`, `secrets.key`, `secrets.recovery.pub`,
    /// `secrets.recovery.pending.pub`, `secrets.recovery.verified`,
    /// `secrets.rotate-pending.json`).
    pub config: PathBuf,
    /// `hub.db` e, por padrão, `backups/`.
    pub data: PathBuf,
}

impl Dirs {
    /// `CTHUB_CONFIG_DIR`/`CTHUB_DATA_DIR`, senão `~/.config/cthub` e
    /// `~/.local/share/cthub` (respeitando `XDG_*_HOME`).
    pub fn from_env() -> io::Result<Self> {
        let var = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let home = var("HOME")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME não definido"))?;
        let config = var("CTHUB_CONFIG_DIR").unwrap_or_else(|| {
            var("XDG_CONFIG_HOME")
                .unwrap_or_else(|| home.join(".config"))
                .join("cthub")
        });
        let data = var("CTHUB_DATA_DIR").unwrap_or_else(|| {
            var("XDG_DATA_HOME")
                .unwrap_or_else(|| home.join(".local/share"))
                .join("cthub")
        });
        Ok(Dirs { config, data })
    }

    pub fn hub_db(&self) -> PathBuf {
        self.data.join("hub.db")
    }

    pub fn ui_token_file(&self) -> PathBuf {
        self.config.join("ui.token")
    }

    pub fn github_token_file(&self) -> PathBuf {
        self.config.join("github.token")
    }

    /// Identidade do hub nos eventos que ele mesmo grava (status manual,
    /// gatilhos do GitHub), no lugar do `env_id` de um ambiente.
    pub fn hub_id_file(&self) -> PathBuf {
        self.config.join("hub_id")
    }

    /// Store cifrado de secrets e variáveis (issue #164).
    pub fn secrets_file(&self) -> PathBuf {
        self.config.join("secrets.age")
    }

    /// Chave (identidade age) do hub que abre o store, gerada no `serve`.
    pub fn secrets_key_file(&self) -> PathBuf {
        self.config.join("secrets.key")
    }

    /// Chave pública de recuperação (#169), opcional: se existir, o store
    /// também é cifrado para ela.
    pub fn secrets_recovery_file(&self) -> PathBuf {
        self.config.join("secrets.recovery.pub")
    }

    /// Chave pública de recuperação gerada e ainda não verificada (#169):
    /// vira `secrets.recovery.pub` quando a verificação passa.
    pub fn secrets_recovery_pending_file(&self) -> PathBuf {
        self.config.join("secrets.recovery.pending.pub")
    }

    /// Data da última verificação bem-sucedida da chave de recuperação e a
    /// pública verificada (JSON, #169), para o lembrete periódico.
    pub fn secrets_recovery_verified_file(&self) -> PathBuf {
        self.config.join("secrets.recovery.verified")
    }

    /// Secrets a rotacionar na origem depois de uma restauração com a chave
    /// de recuperação (#170): JSON só com escopos e nomes, nunca valores.
    pub fn secrets_rotate_pending_file(&self) -> PathBuf {
        self.config.join("secrets.rotate-pending.json")
    }

    /// Backup configurado pela UI (#263): sobrepõe `backup_*` do config.toml.
    /// Sem credenciais (essas ficam no store de secrets).
    pub fn backup_settings_file(&self) -> PathBuf {
        self.config.join("backup.json")
    }

    /// Chave VAPID da Web Push (#203, PKCS#8 em base64url, 0600): fora do
    /// `hub.db` e dos snapshots.
    pub fn vapid_key_file(&self) -> PathBuf {
        self.config.join("vapid.key")
    }

    /// Clone, build, backups e estado do `cthubd update`.
    pub fn update_dir(&self) -> PathBuf {
        self.data.join("update")
    }

    /// Cria os diretórios privados (0700).
    pub fn ensure(&self) -> io::Result<()> {
        for d in [&self.config, &self.data] {
            ct_core::files::private_dir(d)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HubConfig {
    /// Nome legível do hub; vazio = nome da máquina.
    pub name: String,
    pub port: u16,
    /// Endereço de escuta além do loopback (ex.: IP Tailscale). Fora de
    /// `127.0.0.1` só com configuração explícita.
    pub bind: String,
    /// Intervalo do sync com o GitHub de todos os projetos; `0` desliga.
    pub gh_sync_interval_secs: u64,
    /// Raiz da API do GitHub.
    pub gh_api_url: String,
    /// `gh` de onde vem o token na falta de `github.token` (`gh auth token`).
    pub gh_bin: String,
    /// Snapshots do `hub.db`; vazio = `<data>/backups`. Ponha em outro disco.
    pub backup_dir: String,
    /// Intervalo entre snapshots; `0` desliga (com aviso).
    pub backup_interval_hours: u64,
    /// Quantos snapshots manter (os mais novos).
    pub backup_keep: usize,
    /// Snapshots cifrados do store de secrets (#168); vazio = `backup_dir`.
    /// Pensado para uma pasta sincronizada (OneDrive, iCloud): os arquivos
    /// são cifrados para a chave do hub e a de recuperação.
    pub secrets_snapshot_dir: String,
    /// Pasta única dos docs dos projetos fora do repositório (épico #445):
    /// os ambientes a acessam por um compartilhamento montado em
    /// `docs_root`; `~` é expandido. Vazio = docs desligados.
    pub docs_dir: String,
    /// Intervalo do pacote completo de `docs_dir` (#442); `0` desliga.
    pub docs_backup_interval_hours: u64,
    /// Quantos pacotes dos docs manter (os mais novos), no `backup_dir` e
    /// no destino remoto; separado de `backup_keep`.
    pub docs_backup_keep: usize,
    /// Acima deste tamanho (soma dos arquivos), o backup dos docs não roda
    /// e avisa (`docs_backup_too_large`).
    pub docs_backup_max_mb: u64,
    /// Repositório de onde o `cthubd update` compila a versão nova.
    pub update_repo: String,
    /// Canal do `cthubd update`: branch (ex.: `develop`) ou tag.
    pub update_ref: String,
    /// Intervalo da verificação de atualização do hub (só confere, nunca
    /// instala, #421); `0` desliga (o `hub.update.check` continua).
    pub update_check_interval_hours: u64,
    /// Limiares dos sinais de saúde (`[health]`, spec do gestor §5.2).
    pub health: ct_ledger::HealthThresholds,
    /// Dias até o lembrete para verificar de novo a chave de recuperação
    /// (#169); `0` desliga o lembrete.
    pub recovery_reverify_days: u64,
    /// Rotas da PWA (`[web]`, spec da PWA §3.4, épico #205).
    pub web: WebConfig,
    /// Log em arquivo, que a tela Logs da UI lê (`[logs]`, #65).
    pub logs: ct_core::LogsConfig,
}

/// `[web]`: a UI web servida pelo hub (#199) e a sessão web (#200, spec da
/// PWA §3.4).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebConfig {
    /// `/`, `/assets/*` e `/web/*` desligados por padrão (respondem `404`).
    pub enabled: bool,
    /// `Origin` aceitos no upgrade do `/web/ws` e nos `POST /web/*`; vazio
    /// = `http://127.0.0.1:<port>` e `http://localhost:<port>`.
    pub origins: Vec<String>,
    /// Validade absoluta da sessão web (cookie `ct_web`), em dias.
    pub session_days: u64,
    /// Dias sem uso até a sessão web vencer.
    pub idle_days: u64,
    /// Cloudflare Access na frente do hub (`[web.cloudflare]`), opcional.
    pub cloudflare: Option<CloudflareConfig>,
    /// `tailscale serve` na frente do hub (`[web.tailscale]`), opcional.
    pub tailscale: Option<TailscaleConfig>,
    /// Web Push da PWA (`[web.push]`, #203).
    pub push: PushConfig,
}

/// `[web.push]`: notificações Web Push da PWA (spec da PWA §7).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PushConfig {
    /// Liga a Web Push (com `[web] enabled`); a chave VAPID nasce no
    /// primeiro `serve` com ela ligada.
    pub enabled: bool,
    /// Minutos sem sync até o aviso `env_stale` de um ambiente; `0` desliga.
    pub env_stale_minutes: u64,
    /// `sub` do JWT VAPID (`mailto:` ou `https://`); vazio = o primeiro
    /// e-mail de `[web.cloudflare]` ou o primeiro origin `https://`.
    pub subject: String,
}

impl Default for PushConfig {
    fn default() -> Self {
        PushConfig {
            enabled: true,
            env_stale_minutes: 30,
            subject: String::new(),
        }
    }
}

impl Default for WebConfig {
    fn default() -> Self {
        WebConfig {
            enabled: false,
            origins: Vec::new(),
            session_days: 30,
            idle_days: 7,
            cloudflare: None,
            tailscale: None,
            push: PushConfig::default(),
        }
    }
}

/// `[web.cloudflare]`: o JWT `Cf-Access-Jwt-Assertion` é exigido e
/// conferido contra as chaves do time.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CloudflareConfig {
    /// Domínio do time, ex.: `minha-equipe.cloudflareaccess.com`.
    pub team_domain: String,
    /// AUD da aplicação no Access.
    pub aud: String,
    /// E-mails aceitos (sem diferença de maiúsculas).
    pub emails: Vec<String>,
}

/// `[web.tailscale]`: `Tailscale-User-Login` aceito do loopback.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TailscaleConfig {
    /// Logins aceitos (ex.: `eu@github`, sem diferença de maiúsculas).
    pub logins: Vec<String>,
}

impl WebConfig {
    /// Erros de configuração que deixariam a web aberta ou inútil: validade
    /// zero e proxy configurado pela metade.
    pub fn validate(&self) -> Result<(), String> {
        if self.session_days == 0 || self.idle_days == 0 {
            return Err("[web] session_days e idle_days precisam ser maiores que 0".into());
        }
        if let Some(c) = &self.cloudflare {
            if c.team_domain.trim().is_empty() || c.aud.trim().is_empty() {
                return Err("[web.cloudflare] precisa de team_domain e aud".into());
            }
            if c.emails.iter().all(|e| e.trim().is_empty()) {
                return Err("[web.cloudflare] precisa de ao menos um e-mail em emails".into());
            }
        }
        if let Some(t) = &self.tailscale {
            if t.logins.iter().all(|l| l.trim().is_empty()) {
                return Err("[web.tailscale] precisa de ao menos um login em logins".into());
            }
        }
        let sub = self.push.subject.trim();
        if !sub.is_empty() && !sub.starts_with("mailto:") && !sub.starts_with("https://") {
            return Err("[web.push] subject precisa começar com mailto: ou https://".into());
        }
        Ok(())
    }
}

impl Default for HubConfig {
    fn default() -> Self {
        HubConfig {
            name: String::new(),
            port: DEFAULT_PORT,
            bind: "127.0.0.1".into(),
            gh_sync_interval_secs: 180,
            gh_api_url: "https://api.github.com".into(),
            gh_bin: "gh".into(),
            backup_dir: String::new(),
            backup_interval_hours: 6,
            backup_keep: 28,
            secrets_snapshot_dir: String::new(),
            docs_dir: String::new(),
            docs_backup_interval_hours: crate::backup::DEFAULT_DOCS_INTERVAL_HOURS,
            docs_backup_keep: crate::backup::DEFAULT_DOCS_KEEP,
            docs_backup_max_mb: crate::backup::DEFAULT_DOCS_MAX_MB,
            update_repo: "https://github.com/deep-ion-ai/ctower-agents.git".into(),
            update_ref: "main".into(),
            update_check_interval_hours: 6,
            health: ct_ledger::HealthThresholds::default(),
            recovery_reverify_days: crate::secrets::DEFAULT_REVERIFY_DAYS,
            web: WebConfig::default(),
            logs: Default::default(),
        }
    }
}

impl HubConfig {
    /// Lê `<config>/config.toml` (ausente = padrões). `CTHUB_PORT` sobrepõe a porta.
    pub fn load(dirs: &Dirs) -> io::Result<Self> {
        let path = dirs.config.join("config.toml");
        let mut cfg = match std::fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {e}", path.display()),
                )
            })?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => HubConfig::default(),
            Err(e) => return Err(e),
        };
        if let Some(p) = std::env::var("CTHUB_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
        {
            cfg.port = p;
        }
        Ok(cfg)
    }

    /// Diretório dos snapshots (`~` expandido).
    pub fn backup_dir(&self, dirs: &Dirs) -> PathBuf {
        match self.backup_dir.trim() {
            "" => dirs.data.join("backups"),
            d => expand_home(d),
        }
    }

    /// Diretório dos snapshots de secrets (`~` expandido); vazio = o
    /// `backup_dir` em vigor (o do `backup.json` da UI, senão
    /// [`HubConfig::backup_dir`]), um destino só para a pasta sincronizada.
    pub fn secrets_snapshot_dir(&self, dirs: &Dirs) -> PathBuf {
        match self.secrets_snapshot_dir.trim() {
            "" => crate::backup::persisted_dir(&dirs.backup_settings_file())
                .unwrap_or_else(|| self.backup_dir(dirs)),
            d => expand_home(d),
        }
    }

    /// `docs_dir` com `~` expandido; `None` = docs desligados.
    pub fn docs_dir(&self) -> Option<PathBuf> {
        match self.docs_dir.trim() {
            "" => None,
            d => Some(expand_home(d)),
        }
    }

    /// Sem `secrets_snapshot_dir`: os snapshots de secrets acompanham o
    /// `backup_dir` também quando a UI o muda.
    pub fn secrets_snapshot_follows_backup(&self) -> bool {
        self.secrets_snapshot_dir.trim().is_empty()
    }

    /// `name` do config ou, se vazio, o nome da máquina.
    pub fn name(&self) -> String {
        if !self.name.trim().is_empty() {
            return self.name.trim().to_string();
        }
        hostname().unwrap_or_else(|| "cthub".into())
    }
}

pub(crate) fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: o buffer é válido e o tamanho passado é o dele.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]).trim().to_string();
    (!name.is_empty()).then_some(name)
}

pub(crate) fn expand_home(p: &str) -> PathBuf {
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ if p == "~" => std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(p)),
        _ => PathBuf::from(p),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(root: &std::path::Path) -> Dirs {
        Dirs {
            config: root.join("config"),
            data: root.join("data"),
        }
    }

    #[test]
    fn padroes_e_chaves_do_config() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        let c = HubConfig::load(&ds).unwrap();
        assert_eq!(c.port, DEFAULT_PORT);
        assert_eq!((c.backup_interval_hours, c.backup_keep), (6, 28));
        assert_eq!(c.backup_dir(&ds), d.path().join("data/backups"));
        assert_eq!(c.secrets_snapshot_dir(&ds), d.path().join("data/backups"));
        assert_eq!(c.docs_dir(), None);
        assert_eq!(
            (
                c.docs_backup_interval_hours,
                c.docs_backup_keep,
                c.docs_backup_max_mb
            ),
            (24, 7, 1024)
        );
        assert!(!c.name().is_empty());

        std::fs::create_dir_all(&ds.config).unwrap();
        std::fs::write(
            ds.config.join("config.toml"),
            "name = \"mac-mini\"\nbind = \"100.1.2.3\"\nbackup_dir = \"/Volumes/Backup/cthub\"\n\
             backup_interval_hours = 0\nbackup_keep = 3\ngh_sync_interval_secs = 60\n\
             docs_dir = \"/srv/docs\"\ndocs_backup_keep = 3\n[health]\nstale_days = 5\n",
        )
        .unwrap();
        let c = HubConfig::load(&ds).unwrap();
        assert_eq!(
            (c.name().as_str(), c.bind.as_str()),
            ("mac-mini", "100.1.2.3")
        );
        assert_eq!(c.backup_dir(&ds), PathBuf::from("/Volumes/Backup/cthub"));
        assert_eq!(c.docs_dir(), Some(PathBuf::from("/srv/docs")));
        assert_eq!((c.docs_backup_keep, c.docs_backup_interval_hours), (3, 24));
        // Sem secrets_snapshot_dir, os snapshots de secrets vão junto.
        assert_eq!(
            c.secrets_snapshot_dir(&ds),
            PathBuf::from("/Volumes/Backup/cthub")
        );
        assert!(c.secrets_snapshot_follows_backup());
        // O `dir` do backup.json (UI) vale sobre o backup_dir do config.
        std::fs::write(ds.backup_settings_file(), r#"{"dir": "/mnt/ntfs/cthub"}"#).unwrap();
        assert_eq!(
            c.secrets_snapshot_dir(&ds),
            PathBuf::from("/mnt/ntfs/cthub")
        );
        std::fs::write(ds.backup_settings_file(), r#"{"dir": " "}"#).unwrap();
        assert_eq!(
            c.secrets_snapshot_dir(&ds),
            PathBuf::from("/Volumes/Backup/cthub")
        );
        assert_eq!((c.backup_interval_hours, c.backup_keep), (0, 3));
        assert_eq!(c.gh_sync_interval_secs, 60);
        assert_eq!(c.update_ref, "main");
        assert_eq!(c.update_check_interval_hours, 6);
        assert_eq!(c.health.stale_days, 5);
        assert_eq!(c.health.blocked_days, 2);
        assert!(c.update_repo.ends_with("ctower-agents.git"));
        assert_eq!(c.recovery_reverify_days, 90);

        std::fs::write(
            ds.config.join("config.toml"),
            "recovery_reverify_days = 30\n\
             secrets_snapshot_dir = \"/mnt/c/Users/eu/OneDrive/cthub\"\n",
        )
        .unwrap();
        let c = HubConfig::load(&ds).unwrap();
        assert_eq!(c.recovery_reverify_days, 30);
        assert_eq!(c.web, WebConfig::default());
        assert!(!c.web.enabled);
        assert_eq!(
            c.secrets_snapshot_dir(&ds),
            PathBuf::from("/mnt/c/Users/eu/OneDrive/cthub")
        );
        assert!(!c.secrets_snapshot_follows_backup());
    }

    #[test]
    fn secao_web() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        std::fs::create_dir_all(&ds.config).unwrap();
        std::fs::write(
            ds.config.join("config.toml"),
            "[web]\nenabled = true\norigins = [\"https://hub.exemplo.ts.net\"]\n",
        )
        .unwrap();
        let c = HubConfig::load(&ds).unwrap();
        assert!(c.web.enabled);
        assert_eq!(c.web.origins, ["https://hub.exemplo.ts.net"]);
        assert_eq!((c.web.session_days, c.web.idle_days), (30, 7));
        assert!(c.web.cloudflare.is_none() && c.web.tailscale.is_none());
        assert!(c.web.validate().is_ok());
        std::fs::write(
            ds.config.join("config.toml"),
            "[web]\nenabled = true\nsession_days = 10\nidle_days = 2\n\
             [web.cloudflare]\nteam_domain = \"eq.cloudflareaccess.com\"\naud = \"abc\"\n\
             emails = [\"eu@exemplo.com\"]\n[web.tailscale]\nlogins = [\"eu@github\"]\n",
        )
        .unwrap();
        let c = HubConfig::load(&ds).unwrap();
        assert_eq!((c.web.session_days, c.web.idle_days), (10, 2));
        let cf = c.web.cloudflare.clone().unwrap();
        assert_eq!(
            (cf.team_domain.as_str(), cf.aud.as_str()),
            ("eq.cloudflareaccess.com", "abc")
        );
        assert_eq!(cf.emails, ["eu@exemplo.com"]);
        assert_eq!(c.web.tailscale.clone().unwrap().logins, ["eu@github"]);
        assert!(c.web.validate().is_ok());
        let mut w = c.web.clone();
        w.cloudflare.as_mut().unwrap().emails.clear();
        assert!(w.validate().unwrap_err().contains("emails"));
        let mut w = c.web.clone();
        w.cloudflare.as_mut().unwrap().aud = " ".into();
        assert!(w.validate().unwrap_err().contains("aud"));
        let mut w = c.web.clone();
        w.tailscale.as_mut().unwrap().logins = vec!["".into()];
        assert!(w.validate().unwrap_err().contains("logins"));
        let mut w = c.web;
        w.idle_days = 0;
        assert!(w.validate().is_err());
        assert_eq!(w.push, PushConfig::default());
        assert_eq!(w.push.env_stale_minutes, 30);
        std::fs::write(
            ds.config.join("config.toml"),
            "[web]\nenabled = true\n[web.push]\nenv_stale_minutes = 10\n\
             subject = \"mailto:eu@exemplo.com\"\n",
        )
        .unwrap();
        let c = HubConfig::load(&ds).unwrap();
        assert_eq!(c.web.push.env_stale_minutes, 10);
        assert!(c.web.push.enabled);
        assert!(c.web.validate().is_ok());
        let mut w = c.web.clone();
        w.push.subject = "eu@exemplo.com".into();
        assert!(w.validate().unwrap_err().contains("subject"));
        std::fs::write(
            ds.config.join("config.toml"),
            "[web.push]\nenabled = false\n",
        )
        .unwrap();
        assert!(!HubConfig::load(&ds).unwrap().web.push.enabled);
        std::fs::write(ds.config.join("config.toml"), "[web.push]\nintervalo = 1\n").unwrap();
        assert!(HubConfig::load(&ds).is_err());
        std::fs::write(ds.config.join("config.toml"), "[web]\nportas = 1\n").unwrap();
        assert!(HubConfig::load(&ds).is_err());
    }

    #[test]
    fn chave_desconhecida_e_erro() {
        let d = tempfile::tempdir().unwrap();
        let ds = dirs(d.path());
        std::fs::create_dir_all(&ds.config).unwrap();
        // Chave do ctowerd não vale aqui.
        std::fs::write(ds.config.join("config.toml"), "hub = true\n").unwrap();
        let e = HubConfig::load(&ds).unwrap_err();
        assert!(e.to_string().contains("config.toml"), "{e}");
    }
}
