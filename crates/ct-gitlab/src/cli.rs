//! Chamadas ao glab (spec §7.2, §7.3), conferidas com as flags do glab
//! 1.124.0 (`glab mr list --help`, `glab api --help`).

use ct_proc::process::{cite, describe_status, run, RunError};
use ct_proc::Secret;
use serde_json::Value;
use std::process::Command;
use std::time::Duration;

/// Prazo de cada chamada ao glab.
pub const DEADLINE: Duration = Duration::from_secs(30);
/// MRs por página do `mr list` (um por branch costuma bastar).
pub const PAGE_SIZE: usize = 100;

/// Variáveis do ambiente do daemon que mudariam o servidor ou o token do
/// glab: saem antes de entrar as do cadastro.
const GLAB_ENV: [&str; 7] = [
    "GITLAB_TOKEN",
    "GITLAB_ACCESS_TOKEN",
    "OAUTH_TOKEN",
    "GITLAB_HOST",
    "GL_HOST",
    "GITLAB_URI",
    "GITLAB_API_HOST",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitlabError {
    #[error("glab não encontrado (glab_bin = \"{0}\")")]
    NotFound(String),
    #[error("o GitLab recusou o token (401)")]
    Unauthorized,
    #[error("glab não respondeu em {0} s")]
    Timeout(u64),
    #[error("GitLab fora do ar: {0}")]
    Unavailable(String),
    /// 404: o recurso (branch, MR) não existe. Quem chama decide se é erro.
    #[error("não encontrado no GitLab: {0}")]
    Missing(String),
    #[error("glab falhou: {0}")]
    Failed(String),
    #[error("resposta inesperada do glab: {0}")]
    Parse(String),
}

/// O `glab` do ambiente para um host do GitLab. O token vai só no ambiente
/// do processo filho (`GITLAB_TOKEN`), nunca no argv; o host vai em
/// `GITLAB_HOST` e, no `glab api`, também em `--hostname` (fora de um clone,
/// o `glab api` usaria o `gitlab.com`). A configuração do glab
/// (`~/.config/glab-cli`) não é lida para o token nem alterada.
#[derive(Debug, Clone)]
pub struct Glab {
    bin: String,
    host: String,
    deadline: Duration,
}

impl Glab {
    pub fn new(bin: &str, host: &str) -> Self {
        Glab {
            bin: bin.to_string(),
            host: host.trim().to_string(),
            deadline: DEADLINE,
        }
    }

    pub fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    pub fn bin(&self) -> &str {
        &self.bin
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    fn run(&self, token: Option<&Secret>, args: &[&str]) -> Result<String, GitlabError> {
        let mut c = Command::new(&self.bin);
        c.args(args)
            // Fora de qualquer clone: o glab não deduz host nem projeto do
            // diretório do daemon.
            .current_dir("/")
            .env("NO_COLOR", "1")
            .env("NO_PROMPT", "1")
            .env("GLAB_CHECK_UPDATE", "false");
        for v in GLAB_ENV {
            c.env_remove(v);
        }
        if !self.host.is_empty() {
            c.env("GITLAB_HOST", &self.host);
        }
        if let Some(t) = token {
            c.env("GITLAB_TOKEN", t.expose());
        }
        let out = match run(c, self.deadline) {
            Ok(o) => o,
            Err(RunError::NotFound) => return Err(GitlabError::NotFound(self.bin.clone())),
            Err(RunError::Timeout(d)) => return Err(GitlabError::Timeout(d.as_secs().max(1))),
            Err(RunError::Io(e)) => return Err(GitlabError::Failed(e.to_string())),
        };
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        if out.status.success() {
            return Ok(stdout);
        }
        let raw = format!("{} {stdout}", String::from_utf8_lossy(&out.stderr));
        Err(classify(&cite(&raw, token), &describe_status(&out.status)))
    }

    /// `glab api <endpoint> --hostname <host> [args]`.
    fn api(&self, token: &Secret, endpoint: &str, extra: &[&str]) -> Result<String, GitlabError> {
        let mut args = vec!["api", endpoint];
        if !self.host.is_empty() {
            args.extend(["--hostname", self.host.as_str()]);
        }
        args.extend(extra);
        self.run(Some(token), &args)
    }

    /// Versão do glab (`glab version`), sem o token.
    pub fn version(&self) -> Result<String, GitlabError> {
        let out = self.run(None, &["version"])?;
        parse_cli_version(&out).ok_or_else(|| GitlabError::Parse(cite(&out, None)))
    }

    /// `glab api version`: a chamada leve que confere host e token (a
    /// versão do GitLab).
    pub fn api_version(&self, token: &Secret) -> Result<String, GitlabError> {
        let v = parse_json(&self.api(token, "version", &[])?)?;
        v.get("version")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| GitlabError::Parse("version sem \"version\"".into()))
    }

    /// MRs da branch, abertos, mesclados e fechados: `glab mr list -R
    /// <path> --all -s <branch> -F json -P 100` (os objetos da API v4).
    pub fn merge_requests(
        &self,
        token: &Secret,
        path: &str,
        branch: &str,
    ) -> Result<Vec<Value>, GitlabError> {
        let per_page = PAGE_SIZE.to_string();
        let out = self.run(
            Some(token),
            &[
                "mr", "list", "-R", path, "--all", "-s", branch, "-F", "json", "-P", &per_page,
            ],
        )?;
        let t = out.trim();
        if t.is_empty() {
            return Ok(Vec::new());
        }
        match parse_json(t)? {
            Value::Array(a) => Ok(a),
            Value::Null => Ok(Vec::new()),
            _ => Err(GitlabError::Parse("mr list não é uma lista".into())),
        }
    }

    /// `projects/<path>/repository/branches/<branch>`, codificados.
    fn branch_endpoint(path: &str, branch: &str) -> String {
        format!(
            "projects/{}/repository/branches/{}",
            encode(path),
            encode(branch)
        )
    }

    /// Ponta da branch no GitLab agora (`commit.id`); `None` = ela não
    /// existe.
    pub fn branch_sha(
        &self,
        token: &Secret,
        path: &str,
        branch: &str,
    ) -> Result<Option<String>, GitlabError> {
        let out = match self.api(token, &Self::branch_endpoint(path, branch), &[]) {
            Ok(o) => o,
            Err(GitlabError::Missing(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let v = parse_json(&out)?;
        v.pointer("/commit/id")
            .and_then(Value::as_str)
            .map(|s| Some(s.to_string()))
            .ok_or_else(|| GitlabError::Parse(format!("branch {branch} sem \"commit.id\"")))
    }

    /// Apaga a branch no GitLab (`glab api -X DELETE`); `false` = já não
    /// existia.
    pub fn delete_branch(
        &self,
        token: &Secret,
        path: &str,
        branch: &str,
    ) -> Result<bool, GitlabError> {
        match self.api(
            token,
            &Self::branch_endpoint(path, branch),
            &["-X", "DELETE"],
        ) {
            Ok(_) => Ok(true),
            Err(GitlabError::Missing(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// `sha` do MR `iid` (para o MR que entrou no espelho sem ele); `None`
    /// = o MR não existe.
    pub fn mr_head_sha(
        &self,
        token: &Secret,
        path: &str,
        iid: i64,
    ) -> Result<Option<String>, GitlabError> {
        let endpoint = format!("projects/{}/merge_requests/{iid}", encode(path));
        let out = match self.api(token, &endpoint, &[]) {
            Ok(o) => o,
            Err(GitlabError::Missing(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        Ok(parse_json(&out)?
            .get("sha")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from))
    }
}

fn parse_json(out: &str) -> Result<Value, GitlabError> {
    serde_json::from_str(out.trim()).map_err(|e| GitlabError::Parse(e.to_string()))
}

/// Classifica a saída de erro do glab (já citável).
fn classify(msg: &str, status: &str) -> GitlabError {
    let lower = msg.to_lowercase();
    if lower.contains("401") || lower.contains("unauthorized") {
        return GitlabError::Unauthorized;
    }
    let msg = if msg.is_empty() {
        status.to_string()
    } else {
        msg.to_string()
    };
    if lower.contains("404") {
        return GitlabError::Missing(msg);
    }
    let down = [
        "dial tcp",
        "connection refused",
        "no such host",
        "i/o timeout",
        "timeout",
        "502 ",
        "503 ",
        "504 ",
        "tls:",
        "x509",
    ];
    if down.iter().any(|d| lower.contains(d)) || msg.contains("EOF") {
        GitlabError::Unavailable(msg)
    } else {
        GitlabError::Failed(msg)
    }
}

/// `glab 1.124.0 (d43ccacec)` (e o antigo `glab version 1.22.0 (...)`) →
/// `1.124.0`.
pub fn parse_cli_version(out: &str) -> Option<String> {
    out.split_whitespace()
        .map(|w| w.trim_start_matches('v'))
        .find(|w| w.starts_with(|c: char| c.is_ascii_digit()) && w.contains('.'))
        .map(String::from)
}

/// Percent-encoding de um segmento da API (o caminho do projeto vira
/// `grupo%2Fprojeto%2Fapp`): tudo fora de `A-Za-z0-9-._~`.
pub fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{api_mr, FakeGlab};

    const HOST: &str = "git.exemplo.test";
    const PATH: &str = "grupo/projeto/app";

    #[test]
    fn versoes_e_codificacao() {
        assert_eq!(
            parse_cli_version("glab 1.124.0 (d43ccacec)\n").as_deref(),
            Some("1.124.0")
        );
        assert_eq!(
            parse_cli_version("glab version 1.22.0 (2022-01-01)").as_deref(),
            Some("1.22.0")
        );
        assert_eq!(parse_cli_version("nada"), None);
        assert_eq!(encode(PATH), "grupo%2Fprojeto%2Fapp");
        assert_eq!(encode("feat/9-x"), "feat%2F9-x");
    }

    #[test]
    fn erros_classificados() {
        assert_eq!(
            classify("glab: 401 Unauthorized (HTTP 401)", ""),
            GitlabError::Unauthorized
        );
        assert!(matches!(
            classify("glab: 404 Branch Not Found (HTTP 404)", ""),
            GitlabError::Missing(_)
        ));
        assert!(matches!(
            classify("dial tcp: lookup git.x: no such host", ""),
            GitlabError::Unavailable(_)
        ));
        assert!(matches!(classify("", "saída 1"), GitlabError::Failed(m) if m == "saída 1"));
    }

    #[test]
    fn token_e_host_so_no_ambiente_do_processo() {
        let d = tempfile::tempdir().unwrap();
        let f = FakeGlab::new(d.path(), HOST, "token-de-teste");
        let g = Glab::new(f.bin().to_str().unwrap(), HOST);
        let t = Secret::new("token-de-teste");
        assert_eq!(g.version().unwrap(), "1.124.0");
        assert_eq!(g.api_version(&t).unwrap(), "17.5.0");
        let calls = f.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].contains("token=ausente"), "version sem o token");
        assert!(calls[1].contains("token=ok") && calls[1].contains(&format!("host={HOST}")));
        assert!(calls[1].contains(&format!("\tapi\tversion\t--hostname\t{HOST}\t")));
        assert!(calls.iter().all(|c| !c.contains("token-de-teste")));

        let e = g.api_version(&Secret::new("errado")).unwrap_err();
        assert_eq!(e, GitlabError::Unauthorized);
        assert!(!e.to_string().contains("errado"));
    }

    #[test]
    fn token_ecoado_no_erro_e_redigido() {
        let d = tempfile::tempdir().unwrap();
        let f = FakeGlab::new(d.path(), HOST, "token-de-teste");
        f.echo_token_on_error(true);
        let g = Glab::new(f.bin().to_str().unwrap(), HOST);
        let e = g
            .merge_requests(&Secret::new("token-de-teste"), PATH, "feat/9-x")
            .unwrap_err();
        let m = e.to_string();
        assert!(m.contains("***") && !m.contains("token-de-teste"), "{m}");
    }

    #[test]
    fn mrs_da_branch_e_branch_remota() {
        let d = tempfile::tempdir().unwrap();
        let f = FakeGlab::new(d.path(), HOST, "tok");
        let g = Glab::new(f.bin().to_str().unwrap(), HOST);
        let t = Secret::new("tok");
        assert!(g.merge_requests(&t, PATH, "feat/9-x").unwrap().is_empty());
        f.set_mrs(
            "feat/9-x",
            &[api_mr(HOST, PATH, 3, "feat/9-x", "merged", "aaa")],
        );
        let mrs = g.merge_requests(&t, PATH, "feat/9-x").unwrap();
        assert_eq!(mrs.len(), 1);
        assert_eq!(mrs[0]["iid"], 3);
        assert!(f.calls().last().unwrap().contains(&format!(
            "\tmr\tlist\t-R\t{PATH}\t--all\t-s\tfeat/9-x\t-F\tjson\t-P\t100\t"
        )));

        assert_eq!(g.branch_sha(&t, PATH, "feat/9-x").unwrap(), None);
        f.set_branch("feat/9-x", "aaa");
        assert_eq!(
            g.branch_sha(&t, PATH, "feat/9-x").unwrap().as_deref(),
            Some("aaa")
        );
        assert!(g.delete_branch(&t, PATH, "feat/9-x").unwrap());
        assert!(!g.delete_branch(&t, PATH, "feat/9-x").unwrap());
        assert_eq!(
            f.deletes(),
            vec!["projects/grupo%2Fprojeto%2Fapp/repository/branches/feat%2F9-x"]
        );

        f.set_mr(&api_mr(HOST, PATH, 3, "feat/9-x", "merged", "bbb"));
        assert_eq!(g.mr_head_sha(&t, PATH, 3).unwrap().as_deref(), Some("bbb"));
        assert_eq!(g.mr_head_sha(&t, PATH, 4).unwrap(), None);
    }

    #[test]
    fn binario_ausente_e_prazo() {
        let g = Glab::new("/nao/existe/glab", HOST);
        assert_eq!(
            g.version().unwrap_err().to_string(),
            "glab não encontrado (glab_bin = \"/nao/existe/glab\")"
        );
        let d = tempfile::tempdir().unwrap();
        let f = FakeGlab::new(d.path(), HOST, "s");
        f.set_sleep(Some(5));
        let g =
            Glab::new(f.bin().to_str().unwrap(), HOST).with_deadline(Duration::from_millis(300));
        assert!(matches!(
            g.api_version(&Secret::new("s")),
            Err(GitlabError::Timeout(_))
        ));
    }
}
