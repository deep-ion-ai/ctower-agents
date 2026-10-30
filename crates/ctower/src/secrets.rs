//! `ctower secrets ...` no ambiente (issue #184): a chave do ambiente que
//! recebe os secrets do hub (`<config>/env.key`) e o cache cifrado deles
//! (`<data>/secrets-cache.age`).

use ct_sessions::Dirs;
use serde_json::Value;
use std::time::Duration;

const ROTATE_TIMEOUT: Duration = Duration::from_secs(2 * 60);

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
