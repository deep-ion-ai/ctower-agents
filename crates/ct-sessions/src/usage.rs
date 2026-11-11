//! Uso por sessão pelo statusline do Claude Code (issue #106).
//!
//! O provisionamento registra `ctower statusline --v <n>` como `statusLine`
//! no `settings.json` da pasta de configuração do Claude Code do ambiente.
//! A cada atualização de qualquer sessão interativa (do ctower ou aberta
//! direto no terminal), o Claude Code chama o comando com um JSON no stdin;
//! o comando grava `<data>/usage/<session_id>.json` (0600, escrita atômica)
//! só com os campos de uso. O daemon lê esses arquivos para `session.usage`
//! e para a leitura mais recente do plano da conta.

use crate::{Dirs, Result, Sessions, SessionsError};
use ct_core::plan::{
    merge_readings, PlanAccount, PlanReading, StatuslineHealth, STATUSLINE_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Campos do JSON do statusline guardados no arquivo de uso. Fora: `cwd`,
/// `transcript_path`, `workspace` e o que mais identifique o repositório.
const KEPT: &[&str] = &[
    "session_id",
    "model",
    "effort",
    "cost",
    "context_window",
    "rate_limits",
    "prompt_cache",
];

/// Arquivos de uso sem atualização há mais que isso são apagados na
/// varredura do plano.
pub const USAGE_MAX_AGE_MS: i64 = 30 * 24 * 3_600_000;

/// Subcomando registrado no `statusLine` (o binário vem antes).
const SUBCOMMAND: &str = "statusline --v ";

/// Arquivo com o `statusLine` de terceiros que o nosso executa dentro dele.
pub fn chain_file(dirs: &Dirs) -> PathBuf {
    dirs.config.join("statusline-chain.json")
}

/// Pasta dos arquivos de uso.
pub fn usage_dir(dirs: &Dirs) -> PathBuf {
    dirs.data.join("usage")
}

/// `session_id` seguro como nome de arquivo (o uuid do Claude Code).
fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Grava o arquivo de uso da sessão a partir do JSON do statusline. `None`
/// sem `session_id` válido (nada gravado).
pub fn write_usage(dir: &Path, input: &Value, now: i64) -> io::Result<Option<PathBuf>> {
    let Some(id) = input.get("session_id").and_then(Value::as_str) else {
        return Ok(None);
    };
    if !valid_session_id(id) {
        return Ok(None);
    }
    let mut out = Map::new();
    for k in KEPT {
        if let Some(v) = input.get(*k) {
            out.insert((*k).to_string(), v.clone());
        }
    }
    out.insert("written_at".into(), json!(now));
    ct_core::files::private_dir(dir)?;
    let path = dir.join(format!("{id}.json"));
    ct_core::files::write_private_atomic(&path, Value::Object(out).to_string().as_bytes())?;
    Ok(Some(path))
}

/// Modelo da sessão.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct UsageModel {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
}

/// Custo **estimado pelo cliente** (não é cobrança).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct UsageCost {
    #[serde(default)]
    pub total_cost_usd: Option<f64>,
    /// Duração da sessão (ms).
    #[serde(default)]
    pub total_duration_ms: Option<f64>,
    #[serde(default)]
    pub total_api_duration_ms: Option<f64>,
}

/// Janela de contexto real da sessão.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct UsageContext {
    /// Tamanho real da janela (tokens).
    #[serde(default)]
    pub context_window_size: Option<u64>,
    #[serde(default)]
    pub used_percentage: Option<f64>,
    /// Tokens no contexto agora: entrada, cache criado e cache lido da
    /// última resposta.
    #[serde(default)]
    pub used_tokens: Option<u64>,
}

/// Cache de prompt da conversa principal.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct UsagePromptCache {
    #[serde(default)]
    pub warm: Option<bool>,
    /// `5m` ou `1h`.
    #[serde(default)]
    pub ttl: Option<String>,
    /// Quando o cache esfria (ms).
    #[serde(default)]
    pub expires_at: Option<i64>,
    /// 0 a 1.
    #[serde(default)]
    pub hit_ratio: Option<f64>,
}

/// Última leitura de uso de uma sessão (`session.usage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SessionUsage {
    /// Id da sessão no Claude Code.
    pub tool_session_id: String,
    /// Quando o statusline gravou (ms).
    pub written_at: i64,
    pub model: Option<UsageModel>,
    /// Nível de esforço (`low`, `medium`, `high`…).
    pub effort: Option<String>,
    pub cost: Option<UsageCost>,
    pub context_window: Option<UsageContext>,
    pub prompt_cache: Option<UsagePromptCache>,
    /// Janelas do plano **da conta** nesta leitura, sem a regra do reset
    /// (a UI aplica com o relógio dela, ou usa `account.get`).
    pub plan: Option<PlanReading>,
}

fn sub<T: for<'de> Deserialize<'de>>(v: &Value, k: &str) -> Option<T> {
    v.get(k)
        .filter(|x| x.is_object())
        .and_then(|x| serde_json::from_value(x.clone()).ok())
}

impl SessionUsage {
    /// Leitura do arquivo de uso; `None` com JSON fora do formato.
    pub fn from_file_json(v: &Value) -> Option<Self> {
        let tool_session_id = v.get("session_id")?.as_str()?.to_string();
        let written_at = v.get("written_at")?.as_i64()?;
        let context_window = v.get("context_window").filter(|c| c.is_object()).map(|c| {
            let mut ctx: UsageContext = serde_json::from_value(c.clone()).unwrap_or_default();
            ctx.used_tokens = c.get("current_usage").and_then(|u| {
                let n = |k: &str| u.get(k).and_then(Value::as_u64);
                let parts = [
                    n("input_tokens"),
                    n("cache_creation_input_tokens"),
                    n("cache_read_input_tokens"),
                ];
                parts
                    .iter()
                    .any(Option::is_some)
                    .then(|| parts.iter().flatten().sum())
            });
            ctx
        });
        let prompt_cache =
            v.get("prompt_cache")
                .filter(|c| c.is_object())
                .map(|c| UsagePromptCache {
                    warm: c.get("warm").and_then(Value::as_bool),
                    ttl: c.get("ttl").and_then(Value::as_str).map(String::from),
                    expires_at: c
                        .get("expires_at")
                        .and_then(Value::as_i64)
                        .and_then(|s| s.checked_mul(1000)),
                    hit_ratio: c.get("hit_ratio").and_then(Value::as_f64),
                });
        Some(SessionUsage {
            tool_session_id,
            written_at,
            model: sub(v, "model"),
            effort: v
                .get("effort")
                .and_then(|e| e.get("level").or(Some(e)))
                .and_then(Value::as_str)
                .map(String::from),
            cost: sub(v, "cost"),
            context_window,
            prompt_cache,
            plan: v
                .get("rate_limits")
                .and_then(|r| PlanReading::from_rate_limits(r, written_at)),
        })
    }
}

/// Lê o arquivo de uso de uma sessão do Claude Code. `None` sem arquivo
/// (ambiente sem o statusline, ou sessão anterior à instalação).
pub fn read_usage(dir: &Path, tool_session_id: &str) -> Option<SessionUsage> {
    if !valid_session_id(tool_session_id) {
        return None;
    }
    let raw = fs::read(dir.join(format!("{tool_session_id}.json"))).ok()?;
    let v: Value = serde_json::from_slice(&raw).ok()?;
    SessionUsage::from_file_json(&v)
}

/// Leitura mais recente do plano entre todas as sessões do ambiente (por
/// janela). Apaga os arquivos sem atualização há mais de
/// [`USAGE_MAX_AGE_MS`].
pub fn latest_plan(dir: &Path, now: i64) -> Option<PlanReading> {
    let Ok(entries) = fs::read_dir(dir) else {
        return None;
    };
    let mut readings = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(u) = fs::read(&path)
            .ok()
            .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
            .and_then(|v| SessionUsage::from_file_json(&v))
        else {
            continue;
        };
        if now - u.written_at > USAGE_MAX_AGE_MS {
            let _ = fs::remove_file(&path);
            continue;
        }
        readings.extend(u.plan);
    }
    merge_readings(&readings)
}

impl Sessions {
    /// Última leitura de uso da sessão `session_id` (do ledger). `None` sem
    /// arquivo de uso.
    pub fn session_usage(&self, session_id: &str) -> Result<Option<SessionUsage>> {
        let s = self.ledger().get_session(session_id)?;
        let Some(tool_id) = s.tool_session_id.as_deref() else {
            return Ok(None);
        };
        Ok(read_usage(&usage_dir(&self.config().dirs), tool_id))
    }
}

// ---------- o comando do statusline ----------

/// Linha curta do nosso statusline (sem um de terceiros encadeado).
fn own_line(u: &SessionUsage) -> String {
    let mut parts = Vec::new();
    if let Some(m) = u
        .model
        .as_ref()
        .and_then(|m| m.display_name.clone().or(m.id.clone()))
    {
        parts.push(m);
    }
    if let Some(p) = u.context_window.as_ref().and_then(|c| c.used_percentage) {
        parts.push(format!("ctx {p:.0}%"));
    }
    if let Some(w) = u.plan.and_then(|p| p.five_hour) {
        parts.push(format!("5h {:.0}%", w.used_percentage));
    }
    if let Some(w) = u.plan.and_then(|p| p.seven_day) {
        parts.push(format!("sem. {:.0}%", w.used_percentage));
    }
    parts.join(" · ")
}

/// Executa o `statusLine` de terceiros com o mesmo stdin e devolve a saída.
fn run_chained(chain: &Value, input: &[u8]) -> Option<String> {
    let cmd = chain.get("command")?.as_str()?;
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input);
    }
    let out = wait_output(child, Duration::from_secs(5))?;
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// Espera o processo até `deadline`; mata e devolve `None` se passar.
fn wait_output(mut child: std::process::Child, deadline: Duration) -> Option<Vec<u8>> {
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok()
}

/// Corpo do `ctower statusline`: grava o uso e devolve o texto a mostrar
/// (a saída do statusline encadeado, se houver). Nunca falha: o statusline
/// não pode atrapalhar a sessão.
pub fn run_statusline(dirs: &Dirs, input: &[u8], now: i64) -> String {
    let parsed: Option<Value> = serde_json::from_slice(input).ok();
    let mut own = String::new();
    if let Some(v) = &parsed {
        if let Ok(Some(path)) = write_usage(&usage_dir(dirs), v, now) {
            own = fs::read(&path)
                .ok()
                .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
                .and_then(|v| SessionUsage::from_file_json(&v))
                .map(|u| own_line(&u))
                .unwrap_or_default();
        }
    }
    let chain = fs::read(chain_file(dirs))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok());
    match chain {
        Some(c) => run_chained(&c, input).unwrap_or_default(),
        None => own,
    }
}

// ---------- instalação no ambiente ----------

/// Pasta de configuração do Claude Code do ambiente: `CLAUDE_CONFIG_DIR`,
/// senão `~/.claude`.
pub fn claude_config_dir() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    var("CLAUDE_CONFIG_DIR").or_else(|| var("HOME").map(|h| h.join(".claude")))
}

/// Comando registrado no `statusLine`.
pub fn statusline_command(ctower_bin: &str) -> String {
    format!(
        "{} {SUBCOMMAND}{STATUSLINE_VERSION}",
        shell_quote(ctower_bin)
    )
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-+:@".contains(&b))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Versão do nosso statusline num comando registrado; `None` = não é o
/// nosso.
fn our_version(command: &str) -> Option<u32> {
    let i = command.find(SUBCOMMAND)?;
    command[i + SUBCOMMAND.len()..]
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn read_settings(path: &Path) -> io::Result<Map<String, Value>> {
    match fs::read(path) {
        Ok(raw) if raw.iter().all(u8::is_ascii_whitespace) => Ok(Map::new()),
        Ok(raw) => match serde_json::from_slice::<Value>(&raw) {
            Ok(Value::Object(m)) => Ok(m),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} não é um objeto JSON válido", path.display()),
            )),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(e),
    }
}

/// Estado do statusline no ambiente.
pub fn statusline_health(claude_dir: Option<&Path>, dirs: &Dirs) -> StatuslineHealth {
    let mut h = StatuslineHealth {
        current: STATUSLINE_VERSION,
        ..Default::default()
    };
    let Some(dir) = claude_dir else {
        h.error = Some("pasta de configuração do Claude Code desconhecida".into());
        return h;
    };
    match read_settings(&dir.join("settings.json")) {
        Ok(m) => {
            h.version = m
                .get("statusLine")
                .and_then(|s| s.get("command"))
                .and_then(Value::as_str)
                .and_then(our_version);
            h.installed = h.version.is_some();
            h.chained = h.installed && chain_file(dirs).is_file();
        }
        Err(e) => h.error = Some(e.to_string()),
    }
    h
}

/// O que a instalação fez.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum StatuslineInstall {
    /// Já estava na versão atual: nada mudou.
    Unchanged,
    /// Não havia `statusLine`.
    Installed,
    /// Havia o nosso numa versão anterior.
    Updated,
    /// Havia um de terceiros: agora roda dentro do nosso, com a saída
    /// preservada.
    Chained,
}

/// Registra o nosso statusline no `settings.json` de `claude_dir`.
/// Idempotente; um `statusLine` de terceiros vai para o arquivo de
/// encadeamento e continua aparecendo.
pub fn install_statusline(
    claude_dir: &Path,
    dirs: &Dirs,
    ctower_bin: &str,
) -> Result<StatuslineInstall> {
    let path = claude_dir.join("settings.json");
    let mut settings = read_settings(&path).map_err(|e| SessionsError::Invalid(e.to_string()))?;
    let command = statusline_command(ctower_bin);
    let existing = settings.get("statusLine").cloned();
    let existing_cmd = existing
        .as_ref()
        .and_then(|s| s.get("command"))
        .and_then(Value::as_str);
    let outcome = match existing_cmd.map(|c| (c, our_version(c))) {
        Some((c, Some(_))) if c == command => return Ok(StatuslineInstall::Unchanged),
        Some((_, Some(_))) => StatuslineInstall::Updated,
        Some((_, None)) => StatuslineInstall::Chained,
        // `statusLine` sem comando (ou ausente): nada a preservar.
        None => StatuslineInstall::Installed,
    };
    if outcome == StatuslineInstall::Chained {
        let original = existing.clone().unwrap_or(Value::Null);
        ct_core::files::private_dir(&dirs.config)?;
        ct_core::files::write_private_atomic(&chain_file(dirs), original.to_string().as_bytes())?;
    }
    let mut entry = Map::new();
    entry.insert("type".into(), json!("command"));
    entry.insert("command".into(), json!(command));
    // A aparência do statusline encadeado continua a mesma.
    if let Some(Value::Object(old)) = &existing {
        for k in ["padding", "refreshInterval", "hideVimModeIndicator"] {
            if let Some(v) = old.get(k) {
                entry.insert(k.into(), v.clone());
            }
        }
    }
    settings.insert("statusLine".into(), Value::Object(entry));
    fs::create_dir_all(claude_dir)?;
    let mode = fs::metadata(&path).ok().map(|m| {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode() & 0o777
    });
    let body =
        serde_json::to_string_pretty(&Value::Object(settings)).expect("JSON serializa") + "\n";
    ct_core::files::write_private_atomic(&path, body.as_bytes())?;
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
    }
    Ok(outcome)
}

// ---------- conta do ambiente ----------

/// Conta do ambiente por `claude auth status`. `None` sem login, com falha
/// ou fora do prazo ("conta desconhecida"). Com login por token, sem a conta
/// na saída, vale a declarada em `CTOWER_CLAUDE_ORG_ID` e
/// `CTOWER_CLAUDE_EMAIL` (#179). Nada da saída vai para logs.
pub fn read_account(claude_bin: &str) -> Option<PlanAccount> {
    read_account_or(claude_bin, PlanAccount::declared_from_env().as_ref())
}

/// [`read_account`] com a conta declarada explícita.
pub fn read_account_or(claude_bin: &str, declared: Option<&PlanAccount>) -> Option<PlanAccount> {
    let child = Command::new(claude_bin)
        .args(["auth", "status"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let out = wait_output(child, Duration::from_secs(15))?;
    let v: Value = serde_json::from_slice(&out).ok()?;
    PlanAccount::from_auth_status_or(&v, declared)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn dirs(root: &Path) -> Dirs {
        Dirs {
            config: root.join("c"),
            data: root.join("d"),
        }
    }

    fn input(id: &str) -> Value {
        json!({
            "session_id": id,
            "cwd": "/home/fulano/acme-web",
            "transcript_path": "/home/fulano/.claude/projects/x.jsonl",
            "workspace": { "repo": { "owner": "acme", "name": "acme-web" } },
            "model": { "id": "claude-opus-5-5", "display_name": "Opus" },
            "effort": { "level": "high" },
            "cost": { "total_cost_usd": 1.25, "total_duration_ms": 60000 },
            "context_window": {
                "context_window_size": 1000000, "used_percentage": 12,
                "current_usage": { "input_tokens": 100, "output_tokens": 50,
                    "cache_creation_input_tokens": 1000, "cache_read_input_tokens": 118900 }
            },
            "prompt_cache": { "warm": true, "ttl": "1h", "expires_at": 2000, "hit_ratio": 0.9 },
            "rate_limits": {
                "five_hour": { "used_percentage": 72, "resets_at": 5000 },
                "seven_day": { "used_percentage": 64, "resets_at": 9000 }
            }
        })
    }

    #[test]
    fn grava_so_os_campos_de_uso_com_600() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("usage");
        let path = write_usage(&dir, &input("s-1"), 1234).unwrap().unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let v: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "context_window",
                "cost",
                "effort",
                "model",
                "prompt_cache",
                "rate_limits",
                "session_id",
                "written_at"
            ]
        );
        assert!(!v.to_string().contains("acme-web"));
    }

    #[test]
    fn session_id_invalido_nao_grava() {
        let d = tempfile::tempdir().unwrap();
        for id in ["", "../x", "a/b"] {
            assert_eq!(
                write_usage(d.path(), &json!({ "session_id": id }), 1).unwrap(),
                None
            );
        }
        assert_eq!(write_usage(d.path(), &json!({}), 1).unwrap(), None);
        assert_eq!(read_usage(d.path(), "../x"), None);
    }

    #[test]
    fn le_o_uso_com_a_janela_real() {
        let d = tempfile::tempdir().unwrap();
        write_usage(d.path(), &input("s-1"), 1234).unwrap();
        let u = read_usage(d.path(), "s-1").unwrap();
        assert_eq!(u.written_at, 1234);
        assert_eq!(u.effort.as_deref(), Some("high"));
        let ctx = u.context_window.unwrap();
        assert_eq!(ctx.context_window_size, Some(1_000_000));
        assert_eq!(ctx.used_tokens, Some(120_000));
        assert_eq!(u.cost.unwrap().total_cost_usd, Some(1.25));
        assert_eq!(u.prompt_cache.unwrap().expires_at, Some(2_000_000));
        let plan = u.plan.unwrap();
        assert_eq!(plan.read_at, 1234);
        assert_eq!(plan.five_hour.unwrap().resets_at, 5_000_000);
        assert_eq!(read_usage(d.path(), "s-2"), None);
    }

    #[test]
    fn plano_mais_recente_do_ambiente_e_poda() {
        let d = tempfile::tempdir().unwrap();
        let now = USAGE_MAX_AGE_MS + 10_000;
        let mut a = input("s-1");
        a["rate_limits"]["five_hour"]["used_percentage"] = json!(10);
        write_usage(d.path(), &a, now - 5_000).unwrap();
        write_usage(d.path(), &input("s-2"), now - 1_000).unwrap();
        let mut old = input("s-3");
        old["rate_limits"]["five_hour"]["used_percentage"] = json!(99);
        write_usage(d.path(), &old, 1).unwrap();
        let p = latest_plan(d.path(), now).unwrap();
        assert_eq!(p.five_hour.unwrap().used_percentage, 72.0);
        assert!(!d.path().join("s-3.json").exists());
        assert_eq!(latest_plan(&d.path().join("nada"), now), None);
    }

    #[test]
    fn statusline_grava_e_mostra_a_linha_propria() {
        let d = tempfile::tempdir().unwrap();
        let dirs = dirs(d.path());
        let out = run_statusline(&dirs, input("s-1").to_string().as_bytes(), 10);
        assert_eq!(out, "Opus · ctx 12% · 5h 72% · sem. 64%");
        assert!(usage_dir(&dirs).join("s-1.json").is_file());
        // Lixo no stdin não quebra.
        assert_eq!(run_statusline(&dirs, b"nao e json", 10), "");
    }

    #[test]
    fn instalacao_idempotente() {
        let d = tempfile::tempdir().unwrap();
        let dirs = dirs(d.path());
        let claude = d.path().join("claude");
        assert!(!statusline_health(Some(&claude), &dirs).installed);
        assert_eq!(
            install_statusline(&claude, &dirs, "/usr/local/bin/ctower").unwrap(),
            StatuslineInstall::Installed
        );
        let first = fs::read(claude.join("settings.json")).unwrap();
        assert_eq!(
            install_statusline(&claude, &dirs, "/usr/local/bin/ctower").unwrap(),
            StatuslineInstall::Unchanged
        );
        assert_eq!(fs::read(claude.join("settings.json")).unwrap(), first);
        let h = statusline_health(Some(&claude), &dirs);
        assert!(h.installed && !h.outdated() && !h.chained);
        assert_eq!(h.version, Some(STATUSLINE_VERSION));
    }

    #[test]
    fn instalacao_preserva_o_resto_e_atualiza_versao_antiga() {
        let d = tempfile::tempdir().unwrap();
        let dirs = dirs(d.path());
        let claude = d.path().join("claude");
        fs::create_dir_all(&claude).unwrap();
        fs::write(
            claude.join("settings.json"),
            json!({ "theme": "dark", "statusLine": { "type": "command", "command": "ctower statusline --v 0" } }).to_string(),
        )
        .unwrap();
        assert!(statusline_health(Some(&claude), &dirs).outdated());
        assert_eq!(
            install_statusline(&claude, &dirs, "ctower").unwrap(),
            StatuslineInstall::Updated
        );
        let s: Value =
            serde_json::from_slice(&fs::read(claude.join("settings.json")).unwrap()).unwrap();
        assert_eq!(s["theme"], "dark");
        assert_eq!(s["statusLine"]["command"], statusline_command("ctower"));
        assert!(!chain_file(&dirs).exists());
    }

    #[test]
    fn statusline_de_terceiros_e_encadeado() {
        let d = tempfile::tempdir().unwrap();
        let dirs = dirs(d.path());
        let claude = d.path().join("claude");
        fs::create_dir_all(&claude).unwrap();
        fs::write(
            claude.join("settings.json"),
            json!({ "statusLine": { "type": "command", "command": "cat >/dev/null; echo terceiro", "padding": 2 } })
                .to_string(),
        )
        .unwrap();
        assert_eq!(
            install_statusline(&claude, &dirs, "ctower").unwrap(),
            StatuslineInstall::Chained
        );
        let s: Value =
            serde_json::from_slice(&fs::read(claude.join("settings.json")).unwrap()).unwrap();
        assert_eq!(s["statusLine"]["padding"], 2);
        assert!(statusline_health(Some(&claude), &dirs).chained);
        // A saída do de terceiros continua aparecendo, e o uso é gravado.
        let out = run_statusline(&dirs, input("s-1").to_string().as_bytes(), 10);
        assert_eq!(out.trim(), "terceiro");
        assert!(usage_dir(&dirs).join("s-1.json").is_file());
        // Rodar de novo não troca o encadeado pelo nosso.
        assert_eq!(
            install_statusline(&claude, &dirs, "ctower").unwrap(),
            StatuslineInstall::Unchanged
        );
        let chain: Value = serde_json::from_slice(&fs::read(chain_file(&dirs)).unwrap()).unwrap();
        assert_eq!(chain["command"], "cat >/dev/null; echo terceiro");
    }

    #[test]
    fn settings_invalido_nao_e_tocado() {
        let d = tempfile::tempdir().unwrap();
        let dirs = dirs(d.path());
        let claude = d.path().join("claude");
        fs::create_dir_all(&claude).unwrap();
        fs::write(claude.join("settings.json"), "{ quebrado").unwrap();
        assert!(install_statusline(&claude, &dirs, "ctower").is_err());
        assert_eq!(
            fs::read_to_string(claude.join("settings.json")).unwrap(),
            "{ quebrado"
        );
        assert!(statusline_health(Some(&claude), &dirs).error.is_some());
    }

    #[test]
    fn caminho_com_espaco_vai_entre_aspas() {
        assert_eq!(
            statusline_command("/opt/ct bin/ctower"),
            "'/opt/ct bin/ctower' statusline --v 1"
        );
        assert_eq!(
            our_version("'/opt/ct bin/ctower' statusline --v 1"),
            Some(1)
        );
        assert_eq!(our_version("jq -r .model"), None);
    }

    fn fake_claude(dir: &Path, out: &str) -> String {
        let bin = dir.join("claude");
        fs::write(&bin, format!("#!/bin/sh\ncat <<'EOF'\n{out}\nEOF\n")).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        bin.to_string_lossy().into_owned()
    }

    #[test]
    fn conta_por_token_cai_na_declarada() {
        let d = tempfile::tempdir().unwrap();
        let bin = fake_claude(
            d.path(),
            r#"{"loggedIn": true, "authMethod": "oauth_token", "apiProvider": "firstParty"}"#,
        );
        assert_eq!(read_account_or(&bin, None), None);
        let declared = PlanAccount::declared(Some("conta-a"), Some("fulano@example.com"));
        let a = read_account_or(&bin, declared.as_ref()).unwrap();
        assert_eq!(a.key, "conta-a/fulano@example.com");
    }

    #[test]
    fn conta_do_login_vence_a_declarada() {
        let d = tempfile::tempdir().unwrap();
        let bin = fake_claude(
            d.path(),
            r#"{"loggedIn": true, "orgId": "conta-b", "email": "beltrano@example.com"}"#,
        );
        let declared = PlanAccount::declared(Some("conta-a"), None);
        assert_eq!(
            read_account_or(&bin, declared.as_ref()).unwrap().key,
            "conta-b/beltrano@example.com"
        );
        assert_eq!(
            read_account_or("/nao/existe/claude", declared.as_ref()),
            None
        );
    }
}
