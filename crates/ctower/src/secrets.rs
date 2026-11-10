//! `ctower secrets ...` no ambiente (issue #184): a chave do ambiente que
//! recebe os secrets do hub (`<config>/env.key`) e o cache cifrado deles
//! (`<data>/secrets-cache.age`).
//!
//! `ctower secret get NOME` (#165), dentro de uma sessão: o valor atual de
//! um secret ou variável para o projeto da sessão, pedido ao ctowerd com o
//! token da sessão (`CTOWER_SESSION_TOKEN`).

use ct_sessions::Dirs;
use serde_json::Value;
use std::time::Duration;

const ROTATE_TIMEOUT: Duration = Duration::from_secs(2 * 60);

/// Prazo de `ctower secret get` (o daemon decifra o cache local).
const GET_TIMEOUT: Duration = Duration::from_secs(10);

/// Código de saída de `secret get` para nome que não vale na sessão
/// (inexistente, de outro projeto, reservado ou inválido).
pub const NOT_FOUND_EXIT: i32 = 2;

/// `ctower secret get NOME`: imprime o valor atual no stdout, sem quebra de
/// linha. Erro no stderr, nunca com o valor: [`NOT_FOUND_EXIT`] para o nome,
/// `1` para o resto (fora de uma sessão, token recusado, daemon fora do ar,
/// cache ausente ou ilegível).
pub fn get(name: &str) -> i32 {
    match fetch(name) {
        Ok(value) => {
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            if out
                .write_all(value.as_bytes())
                .and_then(|()| out.flush())
                .is_err()
            {
                return 1;
            }
            0
        }
        Err((code, e)) => {
            eprintln!("ctower secret get {name}: {e}");
            code
        }
    }
}

fn fetch(name: &str) -> Result<String, (i32, String)> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let token = env(ct_sessions::SESSION_TOKEN_ENV).ok_or_else(|| {
        (
            1,
            format!(
                "{} ausente: rode dentro de uma sessão do Control Tower",
                ct_sessions::SESSION_TOKEN_ENV
            ),
        )
    })?;
    let url = env("CTOWER_URL").ok_or_else(|| (1, "CTOWER_URL ausente".to_string()))?;
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(2))
        .timeout(GET_TIMEOUT)
        .build();
    let resp = agent
        .post(&format!("{}/session/secret", url.trim_end_matches('/')))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .send_string(&serde_json::json!({ "name": name }).to_string());
    match resp {
        Ok(r) => r.into_string().map_err(|e| (1, e.to_string())),
        Err(ureq::Error::Status(code, r)) => {
            let msg = r.into_string().unwrap_or_default().trim().to_string();
            let exit = if matches!(code, 400 | 404) {
                NOT_FOUND_EXIT
            } else {
                1
            };
            Err((
                exit,
                if msg.is_empty() {
                    format!("HTTP {code}")
                } else {
                    msg
                },
            ))
        }
        Err(ureq::Error::Transport(e)) => Err((1, format!("ctowerd fora do ar: {e}"))),
    }
}

/// Pede ao ctowerd que troque a chave do ambiente e puxe a configuração (o
/// hub reentrega os secrets para a chave nova). Com o daemon fora do ar,
/// troca aqui mesmo: o cache é recifrado e a chave nova vai ao hub no
/// próximo pull do daemon.
pub fn rotate_env_key() -> i32 {
    match rotate() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("erro: {e}");
            1
        }
    }
}

fn rotate() -> Result<(), String> {
    let (url, token) = crate::status::daemon()?;
    let agent = ureq::AgentBuilder::new().timeout(ROTATE_TIMEOUT).build();
    let resp = agent
        .post(&format!("{url}/secrets/rotate-env-key"))
        .set("Authorization", &format!("Bearer {token}"))
        .call();
    let body = match resp {
        Ok(r) => r.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(_, r)) => {
            return Err(r.into_string().unwrap_or_default().trim().to_string())
        }
        Err(ureq::Error::Transport(_)) => return rotate_offline(),
    };
    let r: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    println!(
        "chave do ambiente trocada: {}",
        r["public_key"].as_str().unwrap_or("?")
    );
    if r["cache_kept"] == true {
        println!("cache de secrets recifrado para a chave nova");
    }
    match &r["pull"] {
        Value::Null => println!("ambiente não ligado a um hub: nada a reentregar"),
        c => {
            crate::hub::summarize_config(c);
            if c["ok"] != true {
                println!("o hub recebe a chave nova no próximo pull do ctowerd");
            }
        }
    }
    Ok(())
}

fn rotate_offline() -> Result<(), String> {
    let d = Dirs::from_env().map_err(|e| e.to_string())?;
    let cache = ct_secrets::EnvCache::new(d.env_key_file(), d.secrets_cache_file());
    let r = cache.rotate_key().map_err(|e| e.to_string())?;
    println!(
        "ctowerd fora do ar: chave do ambiente trocada aqui: {}",
        r.public
    );
    if r.cache_kept {
        println!("cache de secrets recifrado para a chave nova");
    }
    println!("o hub recebe a chave nova no próximo pull do ctowerd");
    Ok(())
}
