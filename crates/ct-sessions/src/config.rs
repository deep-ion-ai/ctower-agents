//! `config.toml` do daemon (spec §13), com padrões para tudo.

use crate::paths::Dirs;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;

pub const DEFAULT_PORT: u16 = 7878;

/// `ctowerd` com `hub = true` recusa subir: o hub virou o `cthubd` (spec do
/// hub como plano de controle §2).
pub const HUB_REMOVED: &str =
    "hub = true não existe mais; migre com \"cthubd migrate\" (docs/hub.md)";

/// Chaves do papel de hub que saíram do config do ambiente. São lidas só
/// para reconhecer o config antigo: `hub = true` recusa subir, as demais
/// são ignoradas.
const LEGACY_HUB_KEYS: [&str; 3] = [
    "hub_backup_dir",
    "hub_backup_interval_hours",
    "hub_backup_keep",
];

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DaemonConfig {
    /// Nome legível deste ambiente; vazio = nome da máquina.
    pub env_name: String,
    pub port: u16,
    /// Endereço de escuta. Fora de `127.0.0.1` só com configuração explícita
    /// (ex.: o IP Tailscale do hub).
    pub bind: String,
    /// Hub (`cthubd`) para onde este ambiente envia as mudanças (vazio =
    /// não envia).
    pub hub_url: String,
    /// O config ainda tem `hub = true` (papel de hub do #64): o `ctowerd`
    /// recusa subir com [`HUB_REMOVED`].
    #[serde(skip)]
    pub legacy_hub: bool,
    /// Intervalo do pull da configuração do hub em segundos (com o mesmo
    /// backoff do envio); `0` desliga o laço (`ctower hub sync` continua).
    pub config_pull_interval_secs: u64,
    /// Horas sem sincronizar com o hub a partir das quais o spawn avisa que
    /// os secrets vêm de um cache velho (a sessão sobe assim mesmo, issue
    /// #166); `0` desliga o aviso.
    pub secrets_max_age_hours: u64,
    /// Raiz dos clones das atribuições com `repo_path` vazio
    /// (`<repos_root>/<repo>`); `~` é expandido.
    pub repos_root: String,
    /// Pasta de docs dos projetos fora do repositório (#440): o caminho
    /// local na máquina do hub, a montagem do compartilhamento nas outras;
    /// `~` é expandido. Vazio = sem docs neste ambiente.
    pub docs_root: String,
    /// Dias que um work item inativo fica no ambiente depois do ack do hub
    /// (poda, spec do hub §6); `0` desliga a poda.
    pub retention_days: u32,
    pub claude_bin: String,
    pub ctower_bin: String,
    /// Raiz dos worktrees; `~` é expandido.
    pub worktrees_root: String,
    /// Pasta das sessões do gestor de projetos (spec do gestor §2.2); vazio =
    /// `<data_dir>/gestor`. Fixa, para a retomada achar o mesmo `cwd`.
    pub gestor_dir: String,
    pub scrollback_bytes: usize,
    /// Pré-aceita no `~/.claude.json` a confiança nas pastas das sessões (issue #16).
    pub pre_trust_worktrees: bool,
    /// Intervalo do sync com o GitHub em segundos; `0` desliga o laço
    /// (o `sync.now` da UI continua funcionando).
    pub gh_sync_interval_secs: u64,
    /// Raiz da API do GitHub.
    pub gh_api_url: String,
    /// `gh` de onde vem o token (`gh auth token`).
    pub gh_bin: String,
    /// Intervalo do sync dos projetos Jira em segundos (spec do Jira e
    /// GitLab §6.5); `0` desliga o laço (o `sync.now` continua).
    pub jira_sync_interval_secs: u64,
    /// jira-cli dos projetos Jira (nome no `PATH` ou caminho absoluto).
    pub jira_bin: String,
    /// Prazo, em segundos, de cada página do `jira issue list` (num Jira
    /// lento, uma página de 100 leva bem mais que um `issue view`); a página
    /// que estoura é pedida de novo até 2 vezes antes de o ciclo cair.
    pub jira_list_timeout_secs: u64,
    /// Prazo, em segundos, das outras chamadas ao jira-cli (`issue view`,
    /// `me`, `serverinfo`).
    pub jira_view_timeout_secs: u64,
    /// glab dos projetos GitLab (MRs e branch remota; nome no `PATH` ou
    /// caminho absoluto).
    pub glab_bin: String,
    /// Repositório de onde vêm as atualizações do ctowerd/ctower (build local).
    pub update_repo: String,
    /// Branch ou tag seguida pelas atualizações (canal).
    pub update_ref: String,
    /// Intervalo da verificação de atualização; `0` desliga (o
    /// `env.check_update` continua funcionando).
    pub update_check_interval_hours: u64,
    /// Limpeza do ambiente após a conclusão do work item (`[cleanup]`,
    /// issue #287).
    pub cleanup: CleanupConfig,
    /// Log em arquivo, que a tela Logs da UI lê (`[logs]`, #65).
    pub logs: ct_core::LogsConfig,
}

/// `[cleanup]` do `config.toml` (épico #284): o job de verificação das
/// limpezas pendentes. Padrões conservadores: verifica, mas não apaga.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CleanupConfig {
    /// Intervalo do job que monta o plano (roda também no boot); `0`
    /// desliga o job (`cleanup.plan`, `cleanup.run` e `ctower clean`
    /// continuam).
    pub check_interval_hours: u64,
    /// Depois de cada verificação, executa as etapas pendentes (nunca as
    /// bloqueadas). Desligado, o job só monta o plano.
    pub auto: bool,
    /// Horas desde a conclusão antes de limpar o work item.
    pub min_age_hours: u64,
    /// Horas desde o último evento de sessão do work item antes de limpá-lo.
    pub min_idle_hours: u64,
    /// Encerra sozinho a sessão viva de work item concluído que esteja
    /// ociosa (`idle`) há pelo menos estas horas (#285). `0` = desligado:
    /// sessão viva nunca é encerrada sem ação explícita, e a etapa
    /// "sessões" fica bloqueada até alguém encerrá-la.
    pub end_idle_sessions_hours: u64,
    /// Apaga `~/.claude/projects/<slug-do-worktree>` (transcripts do Claude
    /// Code) depois que o worktree sai, e só se houver backup em
    /// `<data>/transcripts` de todos os transcripts da pasta (#285).
    pub remove_agent_transcripts: bool,
    /// Dias sem atividade antes de apagar só o `target/` de um worktree que
    /// fica (#265): o de work item ainda em andamento e o de concluído cujo
    /// worktree não pode sair. O código fica. `0` desliga.
    pub target_idle_days: u64,
}

impl Default for CleanupConfig {
    fn default() -> Self {
        CleanupConfig {
            check_interval_hours: 6,
            auto: false,
            min_age_hours: 24,
            min_idle_hours: 24,
            end_idle_sessions_hours: 0,
            remove_agent_transcripts: true,
            target_idle_days: 14,
        }
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        DaemonConfig {
            env_name: String::new(),
            port: DEFAULT_PORT,
            bind: "127.0.0.1".into(),
            hub_url: String::new(),
            legacy_hub: false,
            config_pull_interval_secs: 60,
            secrets_max_age_hours: 24,
            repos_root: "~/projetos".into(),
            docs_root: String::new(),
            retention_days: 30,
            claude_bin: "claude".into(),
            ctower_bin: "ctower".into(),
            worktrees_root: "~/worktrees".into(),
            gestor_dir: String::new(),
            scrollback_bytes: 4 * 1024 * 1024,
            pre_trust_worktrees: true,
            gh_sync_interval_secs: 180,
            gh_api_url: "https://api.github.com".into(),
            gh_bin: "gh".into(),
            jira_sync_interval_secs: 300,
            jira_bin: "jira".into(),
            jira_list_timeout_secs: 120,
            jira_view_timeout_secs: 30,
            glab_bin: "glab".into(),
            update_repo: "https://github.com/deep-ion-ai/ctower-agents.git".into(),
            update_ref: "main".into(),
            update_check_interval_hours: 6,
            cleanup: CleanupConfig::default(),
            logs: Default::default(),
        }
    }
}

impl DaemonConfig {
    /// Lê `<config>/config.toml` (ausente = padrões). `CTOWER_PORT` sobrepõe a porta.
    pub fn load(dirs: &Dirs) -> io::Result<Self> {
        let path = dirs.config.join("config.toml");
        let invalid = |e: toml::de::Error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {e}", path.display()),
            )
        };
        let mut cfg = match std::fs::read_to_string(&path) {
            Ok(s) => {
                let mut t: toml::Table = toml::from_str(&s).map_err(invalid)?;
                let legacy_hub = t.remove("hub").is_some_and(|v| v.as_bool() == Some(true));
                for k in LEGACY_HUB_KEYS {
                    t.remove(k);
                }
                let mut cfg: DaemonConfig = toml::Value::Table(t).try_into().map_err(invalid)?;
                cfg.legacy_hub = legacy_hub;
                cfg
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => DaemonConfig::default(),
            Err(e) => return Err(e),
        };
        if let Some(p) = std::env::var("CTOWER_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
        {
            cfg.port = p;
        }
        Ok(cfg)
    }

    pub fn worktrees_root(&self) -> PathBuf {
        expand_home(&self.worktrees_root)
    }

    /// `gestor_dir` com `~` expandido, ou `<data_dir>/gestor`.
    pub fn gestor_dir(&self, dirs: &Dirs) -> PathBuf {
        match self.gestor_dir.trim() {
            "" => dirs.data.join("gestor"),
            d => expand_home(d),
        }
    }

    /// `secrets_max_age_hours` como duração; `None` = aviso desligado.
    pub fn secrets_max_age(&self) -> Option<std::time::Duration> {
        (self.secrets_max_age_hours > 0).then(|| {
            std::time::Duration::from_secs(self.secrets_max_age_hours.saturating_mul(3600))
        })
    }

    pub fn repos_root(&self) -> PathBuf {
        expand_home(&self.repos_root)
    }

    /// `docs_root` com `~` expandido; `None` = vazio (sem docs).
    pub fn docs_root(&self) -> Option<PathBuf> {
        match self.docs_root.trim() {
            "" => None,
            d => Some(expand_home(d)),
        }
    }

    /// `env_name` do config ou, se vazio, o nome da máquina.
    pub fn env_name(&self) -> String {
        if !self.env_name.trim().is_empty() {
            return self.env_name.trim().to_string();
        }
        hostname().unwrap_or_else(|| "ctower".into())
    }
}

fn hostname() -> Option<String> {
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

fn expand_home(p: &str) -> PathBuf {
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ if p == "~" => std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default(),
        _ => PathBuf::from(p),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(d: &std::path::Path) -> Dirs {
        Dirs {
            config: d.to_path_buf(),
            data: d.join("data"),
        }
    }

    #[test]
    fn sem_arquivo_usa_padroes() {
        let d = tempfile::tempdir().unwrap();
        let c = DaemonConfig::load(&dirs(d.path())).unwrap();
        assert_eq!(c.claude_bin, "claude");
        assert!(c.worktrees_root().ends_with("worktrees"));
        assert_eq!(c.gh_sync_interval_secs, 180);
        assert_eq!(c.gh_api_url, "https://api.github.com");
        assert_eq!(c.gh_bin, "gh");
        assert!(!c.env_name().is_empty());
    }

    #[test]
    fn campos_do_hub() {
        let d = tempfile::tempdir().unwrap();
        let c = DaemonConfig::load(&dirs(d.path())).unwrap();
        assert_eq!((c.bind.as_str(), c.hub_url.as_str()), ("127.0.0.1", ""));
        assert!(!c.legacy_hub);
        std::fs::write(
            d.path().join("config.toml"),
            "bind = \"100.64.0.1\"\nhub_url = \"http://mac:7879\"\n",
        )
        .unwrap();
        let c = DaemonConfig::load(&dirs(d.path())).unwrap();
        assert_eq!(c.bind, "100.64.0.1");
        assert_eq!(c.hub_url, "http://mac:7879");
        assert_eq!(c.config_pull_interval_secs, 60);
        assert!(c.repos_root().ends_with("projetos"));
        assert_eq!(c.retention_days, 30);
        assert_eq!(
            c.secrets_max_age(),
            Some(std::time::Duration::from_secs(24 * 3600))
        );
        std::fs::write(
            d.path().join("config.toml"),
            "config_pull_interval_secs = 5\nrepos_root = \"/src\"\nretention_days = 0\n\
             secrets_max_age_hours = 0\n",
        )
        .unwrap();
        let c = DaemonConfig::load(&dirs(d.path())).unwrap();
        assert_eq!(c.secrets_max_age(), None);
        assert_eq!(c.retention_days, 0);
        assert_eq!(c.config_pull_interval_secs, 5);
        assert_eq!(c.repos_root(), PathBuf::from("/src"));
    }

    #[test]
    fn docs_root_vazio_desliga() {
        let d = tempfile::tempdir().unwrap();
        let c = DaemonConfig::load(&dirs(d.path())).unwrap();
        assert_eq!(c.docs_root(), None);
        std::fs::write(d.path().join("config.toml"), "docs_root = \"  \"\n").unwrap();
        assert_eq!(
            DaemonConfig::load(&dirs(d.path())).unwrap().docs_root(),
            None
        );
        std::fs::write(
            d.path().join("config.toml"),
            "docs_root = \"/Volumes/docs\"\n",
        )
        .unwrap();
        let c = DaemonConfig::load(&dirs(d.path())).unwrap();
        assert_eq!(c.docs_root(), Some(PathBuf::from("/Volumes/docs")));
    }

    /// Config do hub do #64: `hub = true` é reconhecido (o daemon recusa
    /// subir); as chaves de backup do hub antigo são ignoradas.
    #[test]
    fn config_antigo_do_hub() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("config.toml"),
            "hub = true\nhub_backup_dir = \"/Volumes/Backup/ctower\"\nhub_backup_keep = 3\n\
             hub_backup_interval_hours = 1\n",
        )
        .unwrap();
        let c = DaemonConfig::load(&dirs(d.path())).unwrap();
        assert!(c.legacy_hub);
        assert!(HUB_REMOVED.contains("cthubd migrate") && HUB_REMOVED.contains("docs/hub.md"));
        std::fs::write(
            d.path().join("config.toml"),
            "hub = false\nhub_backup_keep = 3\n",
        )
        .unwrap();
        assert!(!DaemonConfig::load(&dirs(d.path())).unwrap().legacy_hub);
    }

    #[test]
    fn env_name_do_config() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("config.toml"), "env_name = \"wsl-pc\"\n").unwrap();
        assert_eq!(
            DaemonConfig::load(&dirs(d.path())).unwrap().env_name(),
            "wsl-pc"
        );
    }

    #[test]
    fn campos_do_github() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("config.toml"),
            "gh_sync_interval_secs = 0\ngh_api_url = \"http://127.0.0.1:9\"\ngh_bin = \"/opt/gh\"\n",
        )
        .unwrap();
        let c = DaemonConfig::load(&dirs(d.path())).unwrap();
        assert_eq!(c.gh_sync_interval_secs, 0);
        assert_eq!(c.gh_api_url, "http://127.0.0.1:9");
        assert_eq!(c.gh_bin, "/opt/gh");
    }

    #[test]
    fn campos_do_jira() {
        let d = tempfile::tempdir().unwrap();
        let c = DaemonConfig::load(&dirs(d.path())).unwrap();
        assert_eq!(
            (c.jira_bin.as_str(), c.jira_sync_interval_secs),
            ("jira", 300)
        );
        assert_eq!(c.glab_bin, "glab");
        assert_eq!(
            (c.jira_list_timeout_secs, c.jira_view_timeout_secs),
            (120, 30)
        );
        std::fs::write(
            d.path().join("config.toml"),
            "jira_sync_interval_secs = 0\njira_bin = \"/opt/homebrew/bin/jira\"\n\
             glab_bin = \"/opt/homebrew/bin/glab\"\n\
             jira_list_timeout_secs = 300\njira_view_timeout_secs = 45\n",
        )
        .unwrap();
        let c = DaemonConfig::load(&dirs(d.path())).unwrap();
        assert_eq!(
            (c.jira_list_timeout_secs, c.jira_view_timeout_secs),
            (300, 45)
        );
        assert_eq!(c.jira_sync_interval_secs, 0);
        assert_eq!(c.jira_bin, "/opt/homebrew/bin/jira");
        assert_eq!(c.glab_bin, "/opt/homebrew/bin/glab");
    }

    #[test]
    fn campos_de_atualizacao() {
        let d = tempfile::tempdir().unwrap();
        let c = DaemonConfig::load(&dirs(d.path())).unwrap();
        assert_eq!(c.update_ref, "main");
        assert_eq!(c.update_check_interval_hours, 6);
        assert!(c.update_repo.ends_with("ctower-agents.git"));
        std::fs::write(
            d.path().join("config.toml"),
            "update_ref = \"develop\"\nupdate_repo = \"/srv/ct\"\nupdate_check_interval_hours = 0\n",
        )
        .unwrap();
        let c = DaemonConfig::load(&dirs(d.path())).unwrap();
        assert_eq!(
            (c.update_ref.as_str(), c.update_repo.as_str()),
            ("develop", "/srv/ct")
        );
        assert_eq!(c.update_check_interval_hours, 0);
    }

    #[test]
    fn secao_de_limpeza() {
        let d = tempfile::tempdir().unwrap();
        let c = DaemonConfig::load(&dirs(d.path())).unwrap();
        assert_eq!(c.cleanup, CleanupConfig::default());
        assert_eq!(c.cleanup.check_interval_hours, 6);
        assert!(!c.cleanup.auto);
        assert_eq!(
            (c.cleanup.min_age_hours, c.cleanup.min_idle_hours),
            (24, 24)
        );
        std::fs::write(
            d.path().join("config.toml"),
            "port = 1\n[cleanup]\nauto = true\ncheck_interval_hours = 0\n",
        )
        .unwrap();
        let c = DaemonConfig::load(&dirs(d.path())).unwrap();
        assert!(c.cleanup.auto);
        assert_eq!(c.cleanup.check_interval_hours, 0);
        assert_eq!(c.cleanup.min_age_hours, 24);
        // Sessão viva nunca é encerrada sozinha por padrão (#285).
        assert_eq!(c.cleanup.end_idle_sessions_hours, 0);
        assert!(c.cleanup.remove_agent_transcripts);
        assert_eq!(c.cleanup.target_idle_days, 14);
        std::fs::write(
            d.path().join("config.toml"),
            "[cleanup]\nend_idle_sessions_hours = 48\nremove_agent_transcripts = false\n\
             target_idle_days = 0\n",
        )
        .unwrap();
        let c = DaemonConfig::load(&dirs(d.path())).unwrap();
        assert_eq!(c.cleanup.end_idle_sessions_hours, 48);
        assert!(!c.cleanup.remove_agent_transcripts);
        assert_eq!(c.cleanup.target_idle_days, 0);
        std::fs::write(
            d.path().join("config.toml"),
            "[cleanup]\nautomatico = true\n",
        )
        .unwrap();
        assert!(DaemonConfig::load(&dirs(d.path())).is_err());
    }

    #[test]
    fn arquivo_parcial_e_campo_desconhecido() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("config.toml"),
            "claude_bin = \"/opt/claude\"\nworktrees_root = \"/w\"\n",
        )
        .unwrap();
        let c = DaemonConfig::load(&dirs(d.path())).unwrap();
        assert_eq!(c.claude_bin, "/opt/claude");
        assert_eq!(c.worktrees_root(), PathBuf::from("/w"));
        assert_eq!(c.ctower_bin, "ctower");
        std::fs::write(d.path().join("config.toml"), "porta = 1\n").unwrap();
        assert!(DaemonConfig::load(&dirs(d.path())).is_err());
    }
}
