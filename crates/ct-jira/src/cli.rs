//! Chamadas ao jira-cli (spec §6.1, §6.2), verificadas com o jira-cli 1.7.0.

use crate::process::{cite as clean, describe_status, run, RunError};
use crate::Secret;
use ct_core::JiraFieldInfo;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// Prazo de cada chamada ao jira-cli, fora a página da listagem.
pub const DEADLINE: Duration = Duration::from_secs(30);
/// Prazo de cada página do `issue list` (num Jira lento, uma página de 100
/// leva bem mais que um `issue view`).
pub const LIST_DEADLINE: Duration = Duration::from_secs(120);
/// Esperas antes de tentar de novo uma página da listagem que falhou por
/// prazo ou erro transitório: uma tentativa a mais por item.
pub const LIST_RETRY_WAITS: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(5)];
/// Tamanho da página do `issue list` (o máximo do jira-cli).
pub const PAGE_SIZE: usize = 100;
/// Teto de páginas por listagem (50 mil issues): uma resposta que nunca
/// encurta não vira laço infinito.
const MAX_PAGES: usize = 500;
/// Configuração do jira-cli quando o cadastro não diz outra.
pub const DEFAULT_CONFIG: &str = "~/.config/.jira/.config.yml";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JiraError {
    #[error("jira-cli não encontrado (jira_bin = \"{0}\")")]
    NotFound(String),
    #[error("configuração do jira-cli ausente em {0}: rode \"jira init\" no ambiente")]
    ConfigMissing(String),
    #[error("o Jira recusou a senha (401)")]
    Unauthorized,
    #[error("jira-cli não respondeu em {0} s")]
    Timeout(u64),
    #[error("Jira fora do ar: {0}")]
    Unavailable(String),
    #[error("jira-cli falhou: {0}")]
    Failed(String),
    #[error("resposta inesperada do jira-cli: {0}")]
    Parse(String),
}

impl JiraError {
    /// Falha que pode passar sozinha (prazo, Jira fora do ar, erro do
    /// servidor, resposta truncada); jira-cli ausente, configuração ausente
    /// e senha recusada não passam tentando de novo.
    pub fn is_transient(&self) -> bool {
        !matches!(
            self,
            JiraError::NotFound(_) | JiraError::ConfigMissing(_) | JiraError::Unauthorized
        )
    }
}

/// O que a listagem relata enquanto pagina.
#[derive(Debug)]
pub enum ListEvent<'a> {
    /// Página lida: o `offset` dela e as entradas listadas até agora.
    Page { offset: usize, listed: usize },
    /// A página do `offset` falhou e vai ser pedida de novo (`attempt` de
    /// `of`), depois de `wait`.
    Retry {
        offset: usize,
        attempt: usize,
        of: usize,
        wait: Duration,
        error: &'a JiraError,
    },
}

/// Listagem do `issue list`: projeto e filtro JQL (o jira-cli acrescenta
/// `project="<KEY>" AND` sozinho ao `-q`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListQuery {
    pub project: String,
    pub jql: String,
    /// Só as `n` mais recentes por `updated` (ordem decrescente, sem o
    /// `--reverse`); `None` = todas, em ordem crescente (`--reverse`), que
    /// não pula issue atualizada durante a paginação.
    pub limit: Option<usize>,
}

/// O `jira` do ambiente. A senha vai só no ambiente do processo filho
/// (`JIRA_API_TOKEN`), nunca no argv; `JIRA_CONFIG_FILE` vai quando o
/// cadastro aponta outra configuração.
#[derive(Debug, Clone)]
pub struct JiraCli {
    bin: String,
    config_file: Option<PathBuf>,
    deadline: Duration,
    list_deadline: Duration,
    list_retry_waits: Vec<Duration>,
}

impl JiraCli {
    /// `config_file` vazio = a configuração padrão do jira-cli.
    pub fn new(bin: &str, config_file: &str) -> Self {
        let config_file = match config_file.trim() {
            "" => None,
            f => Some(expand_home(f)),
        };
        JiraCli {
            bin: bin.to_string(),
            config_file,
            deadline: DEADLINE,
            list_deadline: LIST_DEADLINE,
            list_retry_waits: LIST_RETRY_WAITS.to_vec(),
        }
    }

    /// Prazo das chamadas fora a listagem (`issue view`, `me`, ...).
    pub fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    /// Prazo de cada página do `issue list`.
    pub fn with_list_deadline(mut self, deadline: Duration) -> Self {
        self.list_deadline = deadline;
        self
    }

    /// Esperas antes de cada nova tentativa de uma página da listagem (o
    /// tamanho é o número de tentativas a mais; vazio = sem nova tentativa).
    pub fn with_list_retry_waits(mut self, waits: Vec<Duration>) -> Self {
        self.list_retry_waits = waits;
        self
    }

    pub fn bin(&self) -> &str {
        &self.bin
    }

    /// Caminho da configuração do jira-cli que este cliente usa.
    pub fn config_path(&self) -> PathBuf {
        self.config_file
            .clone()
            .unwrap_or_else(|| expand_home(DEFAULT_CONFIG))
    }

    /// O caminho como o usuário o escreveu (`~/...` no padrão).
    fn config_display(&self) -> String {
        match &self.config_file {
            Some(p) => p.display().to_string(),
            None => DEFAULT_CONFIG.to_string(),
        }
    }

    /// O `server` do `.config.yml` (`None` = arquivo ausente ou sem a
    /// chave). Lido à mão: é uma chave de primeiro nível do YAML.
    pub fn config_server(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.config_path()).ok()?;
        text.lines().find_map(|l| {
            let v = l.strip_prefix("server:")?.trim();
            let v = v.trim_matches(|c| c == '"' || c == '\'');
            (!v.is_empty()).then(|| v.to_string())
        })
    }

    /// Catálogo de campos próprios do `.config.yml` (`issue.fields.custom`).
    /// Arquivo ausente ou sem a seção = lista vazia.
    pub fn config_custom_fields(&self) -> Vec<JiraFieldInfo> {
        std::fs::read_to_string(self.config_path())
            .map(|t| parse_custom_fields(&t))
            .unwrap_or_default()
    }

    fn run(&self, token: Option<&Secret>, args: &[&str]) -> Result<String, JiraError> {
        self.run_within(token, args, self.deadline)
    }

    fn run_within(
        &self,
        token: Option<&Secret>,
        args: &[&str],
        deadline: Duration,
    ) -> Result<String, JiraError> {
        let mut c = Command::new(&self.bin);
        c.args(args)
            .env_remove("JIRA_API_TOKEN")
            .env_remove("JIRA_CONFIG_FILE")
            .env("NO_COLOR", "1");
        if let Some(t) = token {
            c.env("JIRA_API_TOKEN", t.expose());
        }
        if let Some(f) = &self.config_file {
            c.env("JIRA_CONFIG_FILE", f);
        }
        let out = match run(c, deadline) {
            Ok(o) => o,
            Err(RunError::NotFound) => return Err(JiraError::NotFound(self.bin.clone())),
            Err(RunError::Timeout(d)) => return Err(JiraError::Timeout(d.as_secs().max(1))),
            Err(RunError::Io(e)) => return Err(JiraError::Failed(e.to_string())),
        };
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        if out.status.success() {
            return Ok(stdout);
        }
        let raw = format!("{} {stdout}", String::from_utf8_lossy(&out.stderr));
        let msg = clean(&raw, token);
        Err(classify(
            &msg,
            &describe_status(&out.status),
            &self.config_display(),
        ))
    }

    /// Versão do jira-cli (`jira version`), sem a senha.
    pub fn version(&self) -> Result<String, JiraError> {
        let out = self.run(None, &["version"])?;
        parse_cli_version(&out).ok_or_else(|| JiraError::Parse(clean(&out, None)))
    }

    /// `jira me`: o usuário da configuração. No jira-cli 1.7.0 ele não fala
    /// com o servidor; quem confere a senha é o `serverinfo`.
    pub fn me(&self, token: &Secret) -> Result<String, JiraError> {
        Ok(self.run(Some(token), &["me"])?.trim().to_string())
    }

    /// Texto do `jira serverinfo` (a versão sai com [`server_version`]).
    pub fn server_info(&self, token: &Secret) -> Result<String, JiraError> {
        self.run(Some(token), &["serverinfo"])
    }

    /// `jira issue list --raw`, paginado de 100 em 100 até vir uma página
    /// menor (ou até o `limit`), em ordem crescente de `updated` (com o
    /// `limit`, decrescente: no jira-cli 1.7.0 a ordem padrão é DESC e o
    /// `--reverse` a inverte). Cada item é a estrutura
    /// reduzida do jira-cli (chave, resumo, tipo, `updated`), não o JSON da
    /// API: o detalhe vem do [`JiraCli::view`].
    pub fn list(&self, token: &Secret, q: &ListQuery) -> Result<Vec<Value>, JiraError> {
        self.list_with(token, q, &mut |_| {})
    }

    /// [`JiraCli::list`] relatando cada página e cada nova tentativa. Cada
    /// página tem o prazo da listagem; a que falha por prazo ou erro
    /// transitório ([`JiraError::is_transient`]) é pedida de novo, com as
    /// esperas de [`JiraCli::with_list_retry_waits`], antes de a listagem
    /// falhar.
    pub fn list_with(
        &self,
        token: &Secret,
        q: &ListQuery,
        on: &mut dyn FnMut(ListEvent<'_>),
    ) -> Result<Vec<Value>, JiraError> {
        let mut all = Vec::new();
        let limit = q.limit.unwrap_or(usize::MAX);
        for page in 0..MAX_PAGES {
            let offset = page * PAGE_SIZE;
            let size = PAGE_SIZE.min(limit - all.len());
            let mut items = self.page(token, q, offset, size, on)?;
            let n = items.len();
            items.truncate(size);
            all.extend(items);
            on(ListEvent::Page {
                offset,
                listed: all.len(),
            });
            if n < size || all.len() >= limit {
                break;
            }
        }
        Ok(all)
    }

    /// Uma página da listagem, com as novas tentativas.
    fn page(
        &self,
        token: &Secret,
        q: &ListQuery,
        offset: usize,
        size: usize,
        on: &mut dyn FnMut(ListEvent<'_>),
    ) -> Result<Vec<Value>, JiraError> {
        let paginate = format!("{offset}:{size}");
        let mut args = vec!["issue", "list", "-p", q.project.as_str()];
        if !q.jql.trim().is_empty() {
            args.extend(["-q", q.jql.as_str()]);
        }
        args.extend(["--order-by", "updated"]);
        if q.limit.is_none() {
            args.push("--reverse");
        }
        args.extend(["--paginate", paginate.as_str(), "--raw"]);
        let of = self.list_retry_waits.len();
        let mut attempt = 0;
        loop {
            let res = match self.run_within(Some(token), &args, self.list_deadline) {
                Ok(out) => parse_array(&out),
                // Página vazia: o jira-cli sai com 1 e "No result found".
                Err(JiraError::Failed(m)) if m.contains("No result found") => Ok(Vec::new()),
                Err(e) => Err(e),
            };
            match res {
                Err(e) if e.is_transient() && attempt < of => {
                    let wait = self.list_retry_waits[attempt];
                    attempt += 1;
                    on(ListEvent::Retry {
                        offset,
                        attempt,
                        of,
                        wait,
                        error: &e,
                    });
                    std::thread::sleep(wait);
                }
                r => return r,
            }
        }
    }

    /// `jira issue view <KEY> --raw`: o JSON da API v2 da issue, com a
    /// categoria do status e os campos do épico.
    pub fn view(&self, token: &Secret, key: &str) -> Result<Value, JiraError> {
        let out = self.run(Some(token), &["issue", "view", key, "--raw"])?;
        let v: Value =
            serde_json::from_str(out.trim()).map_err(|e| JiraError::Parse(e.to_string()))?;
        if v.get("key").is_none() {
            return Err(JiraError::Parse(format!("issue {key} sem \"key\"")));
        }
        Ok(v)
    }
}

fn parse_array(out: &str) -> Result<Vec<Value>, JiraError> {
    let t = out.trim();
    if t.is_empty() {
        return Ok(Vec::new());
    }
    match serde_json::from_str::<Value>(t) {
        Ok(Value::Array(a)) => Ok(a),
        // Por garantia: a busca da API (`{"issues": [...]}`).
        Ok(Value::Object(mut o)) => match o.remove("issues") {
            Some(Value::Array(a)) => Ok(a),
            _ => Err(JiraError::Parse("listagem sem \"issues\"".into())),
        },
        Ok(_) => Err(JiraError::Parse("listagem não é uma lista".into())),
        Err(e) => Err(JiraError::Parse(e.to_string())),
    }
}

/// Classifica a saída de erro do jira-cli (já limpa).
fn classify(msg: &str, status: &str, config: &str) -> JiraError {
    let lower = msg.to_lowercase();
    if lower.contains("missing configuration file") {
        return JiraError::ConfigMissing(config.to_string());
    }
    if lower.contains("401") || lower.contains("unauthorized") {
        return JiraError::Unauthorized;
    }
    let down = [
        "dial tcp",
        "connection refused",
        "no such host",
        "timeout",
        "i/o timeout",
        "502 ",
        "503 ",
        "504 ",
        "tls:",
        "x509",
    ];
    let msg = if msg.is_empty() {
        status.to_string()
    } else {
        msg.to_string()
    };
    if down.iter().any(|d| lower.contains(d)) || msg.contains("EOF") {
        JiraError::Unavailable(msg)
    } else {
        JiraError::Failed(msg)
    }
}

/// `(Version="1.7.0", GitCommit=...)` → `1.7.0`.
pub fn parse_cli_version(out: &str) -> Option<String> {
    let rest = &out[out.find("Version=\"")? + "Version=\"".len()..];
    let v = &rest[..rest.find('"')?];
    (!v.is_empty()).then(|| v.to_string())
}

/// Versão do Jira no texto do `jira serverinfo` (`Version: \t 8.5.2`).
pub fn server_version(out: &str) -> Option<String> {
    out.lines().find_map(|l| {
        let v = l.trim().strip_prefix("Version:")?.trim();
        (!v.is_empty()).then(|| v.to_string())
    })
}

fn expand_home(p: &str) -> PathBuf {
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(p),
    }
}

/// Teto do catálogo de campos: um Jira com milhares de campos não incha o
/// status do rastreador.
pub const MAX_CUSTOM_FIELDS: usize = 2000;

/// Campos padrão do Jira: aparecem sempre, não entram no catálogo.
const STANDARD_FIELDS: [&str; 5] = ["status", "issuetype", "priority", "assignee", "reporter"];

/// Lê `issue.fields.custom` do `.config.yml` do jira-cli, ordenado por nome
/// (sem caixa nem acento).
/// Lido à mão como o `server` (sem crate de YAML): só o subconjunto que o
/// `jira init` grava, em blocos indentados. O que não se entende é ignorado;
/// item sem `key` fica de fora.
pub fn parse_custom_fields(text: &str) -> Vec<JiraFieldInfo> {
    const PATH: [&str; 3] = ["issue", "fields", "custom"];
    // Chaves abertas (sem valor na linha), com a indentação de cada uma.
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut out: Vec<JiraFieldInfo> = Vec::new();
    let mut cur: Option<JiraFieldInfo> = None;
    for line in text.lines() {
        let body = line.trim_start_matches(' ');
        if body.trim().is_empty() || body.starts_with('#') {
            continue;
        }
        let mut indent = line.len() - body.len();
        let mut body = body.trim_end();
        let item = body == "-" || body.starts_with("- ");
        if item {
            // O item fecha o anterior; a chave dona da lista fica.
            stack.retain(|(i, _)| *i <= indent);
            body = body[1..].trim_start();
            indent += 2;
        } else {
            stack.retain(|(i, _)| *i < indent);
        }
        let in_list = stack.len() == 3 && stack.iter().map(|(_, k)| k.as_str()).eq(PATH);
        let in_schema = stack.len() == 4
            && stack[..3].iter().map(|(_, k)| k.as_str()).eq(PATH)
            && stack[3].1 == "schema";
        if item && in_list {
            out.extend(cur.take());
            cur = Some(JiraFieldInfo::default());
        } else if !in_list && !in_schema && stack.len() < 3 {
            // Fora da lista: o item em curso terminou.
            out.extend(cur.take());
        }
        let Some((key, value)) = body.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
        if value.is_empty() {
            stack.push((indent, key.to_string()));
            continue;
        }
        let Some(f) = cur.as_mut() else { continue };
        match (in_list, in_schema, key) {
            (true, _, "key") => f.id = value.to_string(),
            (true, _, "name") => f.name = value.to_string(),
            (_, true, "datatype") => f.datatype = value.to_string(),
            _ => {}
        }
    }
    out.extend(cur);
    let mut out: Vec<JiraFieldInfo> = out
        .into_iter()
        .filter(|f| !f.id.is_empty() && !STANDARD_FIELDS.contains(&f.id.as_str()))
        .map(|mut f| {
            if f.name.is_empty() {
                f.name = f.id.clone();
            }
            f
        })
        .collect();
    out.sort_by_cached_key(|f| (sort_key(&f.name), f.name.clone(), f.id.clone()));
    out.truncate(MAX_CUSTOM_FIELDS);
    out
}

/// Chave de ordenação sem caixa nem acento ("Área" vem antes de "Sem").
fn sort_key(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeJira;

    #[test]
    fn versoes() {
        assert_eq!(
            parse_cli_version(
                "(Version=\"1.7.0\", GitCommit=\"Homebrew\", CommitDate=\"2025-08-30\")"
            )
            .as_deref(),
            Some("1.7.0")
        );
        assert_eq!(parse_cli_version("nada"), None);
        let info = "SERVER INFO\n-----------\n\nVersion: \t 8.5.2\nBuild Number: \t 805002\n";
        assert_eq!(server_version(info).as_deref(), Some("8.5.2"));
    }

    fn campo(id: &str, name: &str, datatype: &str) -> JiraFieldInfo {
        JiraFieldInfo {
            id: id.into(),
            name: name.into(),
            datatype: datatype.into(),
        }
    }

    #[test]
    fn catalogo_de_campos_do_config_yml() {
        // Como o `jira init` grava, com campos padrão e tipos variados.
        let yml = "\
installation: Local
server: https://jira.x
issue:
    types:
        - id: \"1\"
          name: Bug
    fields:
        custom:
            - name: Responsável pela implementação
              key: customfield_10099
              schema:
                datatype: user
            - name: \"Área: Negócio\"
              key: customfield_10010
              schema:
                datatype: array
                items: option
            - name: Sem tipo
              key: customfield_10500
            - name: Situação
              key: status
              schema:
                datatype: status
epic:
    name: customfield_10001
";
        assert_eq!(
            parse_custom_fields(yml),
            vec![
                campo("customfield_10010", "Área: Negócio", "array"),
                campo(
                    "customfield_10099",
                    "Responsável pela implementação",
                    "user"
                ),
                campo("customfield_10500", "Sem tipo", ""),
            ]
        );
        // Lista no mesmo nível da chave também é YAML válido.
        let compacto = "issue:\n  fields:\n    custom:\n    - name: B\n      key: cf_2\n    - name: A\n      key: cf_1\n";
        assert_eq!(
            parse_custom_fields(compacto),
            vec![campo("cf_1", "A", ""), campo("cf_2", "B", "")]
        );
    }

    #[test]
    fn catalogo_sem_a_secao_ou_parcial() {
        assert!(parse_custom_fields("server: https://jira.x\nlogin: u\n").is_empty());
        assert!(parse_custom_fields("").is_empty());
        assert!(parse_custom_fields("issue:\n    fields:\n        custom: []\n").is_empty());
        assert!(parse_custom_fields("{nao: [e, yaml de bloco").is_empty());
        // `custom` fora de `issue.fields` não conta.
        assert!(parse_custom_fields("custom:\n  - name: X\n    key: cf_1\n").is_empty());
        // Arquivo cortado no meio: fica o que deu para ler; item sem `key` sai.
        let cortado = "\
issue:
    fields:
        custom:
            - name: Inteiro
              key: customfield_1
              schema:
                datatype: string
            - name: Sem chave
              schema:
            - key: customfield_3
              sch";
        assert_eq!(
            parse_custom_fields(cortado),
            vec![
                campo("customfield_3", "customfield_3", ""),
                campo("customfield_1", "Inteiro", "string"),
            ]
        );
    }

    #[test]
    fn catalogo_tem_teto() {
        let mut yml = String::from("issue:\n  fields:\n    custom:\n");
        for i in 0..MAX_CUSTOM_FIELDS + 10 {
            yml.push_str(&format!(
                "      - name: Campo {i:05}\n        key: cf_{i}\n"
            ));
        }
        let got = parse_custom_fields(&yml);
        assert_eq!(got.len(), MAX_CUSTOM_FIELDS);
        assert_eq!(got[0].name, "Campo 00000");
    }

    #[test]
    fn catalogo_lido_do_arquivo_do_cadastro() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("c.yml");
        let cli = JiraCli::new("jira", f.to_str().unwrap());
        assert!(cli.config_custom_fields().is_empty(), "arquivo ausente");
        std::fs::write(
            &f,
            "server: https://jira.x\nissue:\n  fields:\n    custom:\n      - name: R\n        key: cf_9\n",
        )
        .unwrap();
        assert_eq!(cli.config_custom_fields(), vec![campo("cf_9", "R", "")]);
    }

    #[test]
    fn mensagens_de_erro_sao_limpas_e_classificadas() {
        let t = Secret::new("senha-x");
        let m = clean(
            "\u{1b}[0;31m✗\u{1b}[0m Missing configuration file.\nRun 'jira init'",
            Some(&t),
        );
        assert_eq!(m, "Missing configuration file. Run 'jira init'");
        assert_eq!(
            classify(&m, "saída 1", DEFAULT_CONFIG).to_string(),
            "configuração do jira-cli ausente em ~/.config/.jira/.config.yml: rode \"jira init\" \
             no ambiente"
        );
        let m = clean(
            "\njira: Received unexpected response '401 Unauthorized'.\nsenha-x",
            Some(&t),
        );
        assert!(!m.contains("senha-x"));
        assert_eq!(classify(&m, "", ""), JiraError::Unauthorized);
        let m = "Error: Get \"http://x/rest/api/2/search\": dial tcp: connection refused";
        assert!(matches!(classify(m, "", ""), JiraError::Unavailable(_)));
        assert_eq!(clean(&"x".repeat(400), None).chars().count(), 301);
    }

    #[test]
    fn senha_so_no_ambiente_do_processo() {
        let d = tempfile::tempdir().unwrap();
        let f = FakeJira::new(d.path(), "senha-de-teste");
        let cli = JiraCli::new(f.bin().to_str().unwrap(), f.config_file().to_str().unwrap());
        let t = Secret::new("senha-de-teste");
        assert_eq!(cli.me(&t).unwrap(), "usuario");
        assert_eq!(
            server_version(&cli.server_info(&t).unwrap()).as_deref(),
            Some("8.5.2")
        );
        assert_eq!(cli.version().unwrap(), "1.7.0");
        let calls = f.calls();
        assert_eq!(calls.len(), 3);
        assert!(calls[0].contains("\tme\t") && calls[0].contains("token=senha-ok"));
        assert!(calls[2].contains("token=ausente"), "version sem senha");
        assert!(calls.iter().all(|c| !c.contains("senha-de-teste")));
        assert!(calls[0].contains(&format!("config={}", f.config_file().display())));

        let e = cli.me(&Secret::new("errada")).unwrap_err();
        assert_eq!(e, JiraError::Unauthorized);
    }

    #[test]
    fn lista_pagina_ate_a_pagina_menor() {
        let d = tempfile::tempdir().unwrap();
        let f = FakeJira::new(d.path(), "senha-de-teste");
        let issues: Vec<Value> = (1..=150)
            .map(|n| crate::fake::api_issue(&format!("P-{n}"), "Tarefa", "new"))
            .collect();
        f.set_issues(&issues);
        let cli = JiraCli::new(f.bin().to_str().unwrap(), "");
        let q = ListQuery {
            project: "P".into(),
            jql: "updated >= -10m".into(),
            limit: None,
        };
        let got = cli.list(&Secret::new("senha-de-teste"), &q).unwrap();
        assert_eq!(got.len(), 150);
        let calls = f.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].contains(
            "\tissue\tlist\t-p\tP\t-q\tupdated >= -10m\t--order-by\tupdated\t--reverse\t\
             --paginate\t0:100\t--raw\t"
        ));
        assert!(calls[1].contains("--paginate\t100:100"));
        assert!(calls[0].contains("config=ausente"));

        // Página exata de 100: a seguinte vem vazia ("No result found").
        f.set_issues(&issues[..100]);
        assert_eq!(
            cli.list(&Secret::new("senha-de-teste"), &q).unwrap().len(),
            100
        );
    }

    fn flaky_setup(d: &std::path::Path) -> (FakeJira, ListQuery) {
        let f = FakeJira::new(d, "senha-x");
        let issues: Vec<Value> = (1..=250)
            .map(|n| crate::fake::api_issue(&format!("P-{n}"), "Tarefa", "new"))
            .collect();
        f.set_issues(&issues);
        let q = ListQuery {
            project: "P".into(),
            jql: String::new(),
            limit: None,
        };
        (f, q)
    }

    fn fast(f: &FakeJira) -> JiraCli {
        JiraCli::new(f.bin().to_str().unwrap(), "").with_list_retry_waits(vec![
            Duration::from_millis(
                10
            );
            2
        ])
    }

    #[test]
    fn limite_lista_as_mais_recentes_e_para() {
        let d = tempfile::tempdir().unwrap();
        let (f, q) = flaky_setup(d.path());
        let t = Secret::new("senha-x");
        let cli = fast(&f);
        let q = ListQuery {
            limit: Some(150),
            ..q
        };
        let got = cli.list(&t, &q).unwrap();
        assert_eq!(got.len(), 150);
        // Sem o `--reverse`: as mais recentes primeiro (o falso devolve a
        // ordem inversa à do `set_issues`).
        assert_eq!(got[0]["key"], "P-250");
        assert_eq!(got[149]["key"], "P-101");
        let calls = f.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|c| !c.contains("--reverse")));
        assert!(calls[0].contains("--paginate\t0:100\t"));
        assert!(calls[1].contains("--paginate\t100:50\t"));
        // Limite acima do total: para na página menor.
        f.clear_calls();
        let q = ListQuery {
            limit: Some(1000),
            ..q
        };
        assert_eq!(cli.list(&t, &q).unwrap().len(), 250);
        assert_eq!(f.calls().len(), 3);
    }

    #[test]
    fn pagina_da_listagem_e_pedida_de_novo() {
        let d = tempfile::tempdir().unwrap();
        let (f, q) = flaky_setup(d.path());
        let t = Secret::new("senha-x");

        // Prazo estourado uma vez e erro 503 duas: tenta de novo até 2 vezes.
        f.flaky_page(1, 1, "sleep:6");
        f.flaky_page(2, 2, "503");
        let cli = fast(&f).with_list_deadline(Duration::from_secs(2));
        let mut events = Vec::new();
        let got = cli
            .list_with(&t, &q, &mut |e| {
                events.push(match e {
                    ListEvent::Page { offset, listed } => format!("p{offset}:{listed}"),
                    ListEvent::Retry {
                        offset,
                        attempt,
                        of,
                        error,
                        ..
                    } => format!("r{offset}:{attempt}/{of}:{error}"),
                })
            })
            .unwrap();
        assert_eq!(got.len(), 250);
        assert_eq!(
            (f.page_calls(0), f.page_calls(100), f.page_calls(200)),
            (1, 2, 3)
        );
        assert_eq!(events.len(), 6, "{events:?}");
        assert_eq!(events[0], "p0:100");
        assert!(
            events[1].starts_with("r100:1/2:jira-cli não respondeu"),
            "{events:?}"
        );
        assert_eq!(events[2], "p100:200");
        assert!(
            events[3].starts_with("r200:1/2:Jira fora do ar"),
            "{events:?}"
        );
        assert!(events[4].starts_with("r200:2/2:"), "{events:?}");
        assert_eq!(events[5], "p200:250");

        // Três falhas seguidas na mesma página: a listagem cai.
        f.clear_calls();
        f.flaky_page(1, 3, "503");
        let e = fast(&f).list(&t, &q).unwrap_err();
        assert!(matches!(e, JiraError::Unavailable(_)), "{e}");
        assert_eq!(f.page_calls(100), 3);
        assert_eq!(f.page_calls(200), 0);

        // Senha recusada não é tentada de novo.
        f.clear_calls();
        f.flaky_page(0, 3, "401");
        assert_eq!(fast(&f).list(&t, &q).unwrap_err(), JiraError::Unauthorized);
        assert_eq!(f.page_calls(0), 1);
    }

    #[test]
    fn prazo_da_listagem_e_do_view_sao_distintos() {
        let d = tempfile::tempdir().unwrap();
        let (f, q) = flaky_setup(d.path());
        f.set_issues(&[crate::fake::api_issue("P-1", "Tarefa", "new")]);
        f.set_sleep(Some(2));
        let t = Secret::new("senha-x");
        let cli = fast(&f)
            .with_deadline(Duration::from_millis(500))
            .with_list_deadline(Duration::from_secs(10));
        assert_eq!(cli.list(&t, &q).unwrap().len(), 1);
        assert!(matches!(cli.view(&t, "P-1"), Err(JiraError::Timeout(_))));
        let cli = fast(&f)
            .with_deadline(Duration::from_secs(10))
            .with_list_deadline(Duration::from_millis(500))
            .with_list_retry_waits(Vec::new());
        assert!(matches!(cli.list(&t, &q), Err(JiraError::Timeout(_))));
        assert_eq!(cli.view(&t, "P-1").unwrap()["key"], "P-1");
    }

    #[test]
    fn binario_ausente_e_prazo() {
        let cli = JiraCli::new("/nao/existe/jira", "");
        assert_eq!(
            cli.version().unwrap_err().to_string(),
            "jira-cli não encontrado (jira_bin = \"/nao/existe/jira\")"
        );
        let d = tempfile::tempdir().unwrap();
        let f = FakeJira::new(d.path(), "s");
        f.set_sleep(Some(5));
        let cli =
            JiraCli::new(f.bin().to_str().unwrap(), "").with_deadline(Duration::from_millis(300));
        assert!(matches!(
            cli.me(&Secret::new("s")),
            Err(JiraError::Timeout(_))
        ));
    }
}
