//! `ctower hub ...` no ambiente: token de sync e resync com o hub (spec do
//! ledger centralizado §5 e §8). Acessa o ledger direto (SQLite com WAL),
//! então funciona com o daemon no ar ou fora dele. Só `sync` fala com o
//! daemon. Os tokens de cada ambiente são gerados no hub (`cthubd add-env`).

use ct_ledger::Ledger;
use ct_sessions::paths::write_private;
use ct_sessions::{ensure_env_id, Dirs};
use serde_json::Value;
use std::io::Read;
use std::time::Duration;

fn dirs() -> Result<Dirs, String> {
    Dirs::from_env().map_err(|e| e.to_string())
}

fn report(r: Result<(), String>) -> i32 {
    match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("erro: {e}");
            1
        }
    }
}

/// Lê do stdin um token sem espaços e, opcionalmente, um segundo campo (o
/// `env_id` que o `cthubd add-env` imprime na linha seguinte).
fn read_token_and_id() -> Result<(String, Option<String>), String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| e.to_string())?;
    let mut fields = input.split_whitespace();
    let Some(token) = fields.next() else {
        return Err("token vazio ou inválido no stdin".into());
    };
    let env_id = fields.next().map(str::to_string);
    if fields.next().is_some() {
        return Err("stdin com campos demais (esperado: token e, opcional, env_id)".into());
    }
    Ok((token.to_string(), env_id))
}

/// Lê do stdin só um token sem espaços.
fn read_token() -> Result<String, String> {
    match read_token_and_id()? {
        (token, None) => Ok(token),
        _ => Err("token vazio ou inválido no stdin".into()),
    }
}

/// No ambiente: grava em `<config_dir>/hub.token` (0600) o token lido do stdin
/// e, se vier (`cthubd add-env <nome> | ctower hub set-token`), o `env_id`
/// preso a ele em `<config_dir>/env_id` (#195). Um `env_id` diferente do que
/// o ambiente já usa é recusado: o ledger local já foi gravado com aquele.
pub fn set_token() -> i32 {
    report((|| {
        let d = dirs()?;
        let (token, env_id) = read_token_and_id()?;
        if token.starts_with(ct_ledger::AGENT_TOKEN_PREFIX) {
            return Err(
                "este é o token de agente (cta_...); grave-o com ctower hub \
                        set-agent-token"
                    .into(),
            );
        }
        d.ensure().map_err(|e| e.to_string())?;
        if let Some(id) = &env_id {
            if uuid::Uuid::parse_str(id).is_err() {
                return Err(format!("env_id inválido no stdin: {id}"));
            }
            let file = d.env_id_file();
            match std::fs::read_to_string(&file) {
                Ok(atual) if !atual.trim().is_empty() && atual.trim() != id => {
                    return Err(format!(
                        "este ambiente já usa o env_id {atual} ({}); o token é do env_id {id}. \
                         No hub, gere o token preso ao env_id deste ambiente: \
                         cthubd add-env --env-id {atual} <nome> (se o nome já existe preso a \
                         outro env_id, revogue-o antes com cthubd revoke-env <nome>)",
                        file.display(),
                        atual = atual.trim()
                    ));
                }
                _ => write_private(&file, id.as_bytes()).map_err(|e| e.to_string())?,
            }
        }
        write_private(&d.hub_token_file(), token.as_bytes()).map_err(|e| e.to_string())?;
        println!(
            "token gravado em {}; reinicie o ctowerd para usar",
            d.hub_token_file().display()
        );
        Ok(())
    })())
}

/// No ambiente: grava em `<config_dir>/hub-agent.token` (0600) o token de
/// agente lido do stdin (`cthubd agent-token <nome>`). O `ctowerd` lê o
/// arquivo a cada chamada, sem reiniciar.
pub fn set_agent_token() -> i32 {
    report((|| {
        let d = dirs()?;
        let token = read_token()?;
        if !token.starts_with(ct_ledger::AGENT_TOKEN_PREFIX) {
            return Err(format!(
                "token de agente começa com {}; o de sync vai em ctower hub set-token",
                ct_ledger::AGENT_TOKEN_PREFIX
            ));
        }
        d.ensure().map_err(|e| e.to_string())?;
        write_private(&d.hub_agent_token_file(), token.as_bytes()).map_err(|e| e.to_string())?;
        println!(
            "token de agente gravado em {}",
            d.hub_agent_token_file().display()
        );
        Ok(())
    })())
}

/// No ambiente: põe de novo no outbox tudo o que o ledger tem (hub perdido
/// ou trocado). O daemon envia no próximo ciclo. Só reenvia o que **não foi
/// podado** (spec do hub §6.4): avisa quantos work items já saíram do
/// ambiente e só existem no hub (ou nos snapshots dele).
pub fn resync() -> i32 {
    report((|| {
        let d = dirs()?;
        let env_id = ensure_env_id(&d.env_id_file()).map_err(|e| e.to_string())?;
        let ledger = Ledger::open(&d.ledger_db(), &env_id).map_err(|e| e.to_string())?;
        let n = ledger.enqueue_all().map_err(|e| e.to_string())?;
        println!("{n} mudança(s) no outbox para reenviar ao hub");
        let pruned = ledger.pruned_work_items().map_err(|e| e.to_string())?;
        if pruned > 0 {
            println!(
                "aviso: {pruned} work item(s) já podado(s) neste ambiente não são reenviados; \
                 o histórico deles só existe no hub e nos snapshots dele (cthubd restore)"
            );
        }
        Ok(())
    })())
}

/// Espera máxima de `ctower hub sync` (vários lotes com o hub lento).
const SYNC_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// No ambiente: pede ao ctowerd que puxe a configuração do hub e envie o
/// outbox agora, ignorando o backoff. Sai com 1 se o hub não confirmou tudo
/// ou se o pull falhou.
pub fn sync() -> i32 {
    report((|| {
        let (url, token) = crate::status::daemon()?;
        let agent = ureq::AgentBuilder::new().timeout(SYNC_TIMEOUT).build();
        let resp = agent
            .post(&format!("{url}/hub/sync"))
            .set("Authorization", &format!("Bearer {token}"))
            .call();
        let body = match resp {
            Ok(r) => r.into_string().map_err(|e| e.to_string())?,
            Err(ureq::Error::Status(_, r)) => {
                return Err(r.into_string().unwrap_or_default().trim().to_string())
            }
            Err(e) => return Err(format!("ctowerd fora do ar ({e})")),
        };
        let r: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        summarize(&r)
    })())
}

/// Linha do pull da configuração (feito antes do envio).
pub(crate) fn summarize_config(c: &Value) {
    if c.is_null() {
        return;
    }
    if c["legacy_hub"] == true {
        println!("hub sem pull da configuração (anterior à #177): projetos locais mantidos");
        return;
    }
    if let Some(e) = c["error"].as_str() {
        println!("configuração do hub não atualizada ({e}); o ambiente segue com o cache");
        return;
    }
    let n = |k: &str| c[k].as_array().map_or(0, Vec::len);
    if c["changed"] == true {
        println!(
            "configuração do hub {}: {} projeto(s) novo(s), {} alterado(s), {} desatribuído(s)",
            c["config_rev"],
            n("added"),
            n("updated"),
            n("unassigned")
        );
    } else {
        println!("configuração do hub {} já estava em dia", c["config_rev"]);
    }
    if let Some(n) = c["secrets"].as_u64() {
        println!("secrets do hub: {n} entrada(s) recebida(s) e gravada(s) no cache do ambiente");
    }
    let pending = c["pending_applies"].as_u64().unwrap_or(0);
    if pending > 0 {
        println!(
            "{pending} atribuição(ões) sendo aplicada(s) em segundo plano (clone ou validação); \
             o relato vai ao hub no próximo envio"
        );
    }
}

fn summarize(r: &Value) -> Result<(), String> {
    summarize_config(&r["config"]);
    let pending = r["pending"].as_i64().unwrap_or(0);
    let sent = r["sent"].as_i64().unwrap_or(0);
    if r["joined"] == true {
        println!("já havia um envio em andamento: resultado dele");
    }
    if let Some(e) = r["error"].as_str() {
        if sent > 0 {
            println!("{sent} mudança(s) confirmada(s) antes da falha");
        }
        return Err(format!(
            "{e}\n{pending} mudança(s) continuam no outbox; o daemon tenta de novo sozinho"
        ));
    }
    if sent == 0 {
        println!("nada a enviar: o hub já tem tudo deste ambiente");
    } else {
        println!(
            "hub confirmou {sent} mudança(s) em {} lote(s) (até seq {})",
            r["batches"], r["acked_seq"]
        );
    }
    let pruned = r["pruned"].as_u64().unwrap_or(0);
    if pruned > 0 {
        println!(
            "{pruned} work item(s) inativo(s) podado(s) do ambiente (o hub guarda o histórico)"
        );
    }
    if pending > 0 {
        return Err(format!(
            "{pending} mudança(s) chegaram ao outbox durante o envio; rode de novo"
        ));
    }
    Ok(())
}
