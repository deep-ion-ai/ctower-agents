//! Download dos assets de release sem token (#232).
//!
//! Um repositório público se baixa sem nenhum cabeçalho de autenticação. O
//! `gh` autenticado só entra quando o próprio GitHub mostra que o repositório
//! exige login (ver [`needs_token`]). O agente nunca repassa `Authorization`
//! em redirect: o ajuste é explícito aqui, não o padrão do ureq.

use serde_json::Value;
use std::path::Path;
use std::time::Duration;
use ureq::RedirectAuthHeaders;

/// Site do GitHub: `<WEB>/<owner>/<repo>/releases/download/<tag>/<arquivo>`.
pub const WEB: &str = "https://github.com";
/// API do GitHub, usada só para descobrir se o repositório exige login.
pub const API: &str = "https://api.github.com";

const UA: &str = "ct-update";
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(timeout)
        .redirect_auth_headers(RedirectAuthHeaders::Never)
        .build()
}

/// `true` só quando o GitHub prova que o repositório exige autenticação: a
/// consulta anônima à API responde `401`/`404` (privado sem login) ou
/// `"private": true`. Qualquer outra resposta (inclusive falha de rede e
/// limite de requisições) conta como público, sem token: o conservador é não
/// mandar o token sem prova; se o asset não vier, o build cai para o fonte.
pub fn needs_token(api: &str, repo: &str) -> bool {
    let url = format!("{api}/repos/{repo}");
    let r = agent(PROBE_TIMEOUT)
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", UA)
        .call();
    match r {
        Ok(r) => r
            .into_json::<Value>()
            .ok()
            .and_then(|v| v["private"].as_bool())
            .unwrap_or(false),
        Err(ureq::Error::Status(code, _)) => matches!(code, 401 | 404),
        Err(_) => false,
    }
}

/// Baixa `url` para `dest`. Sem `token` não sai nenhum `Authorization`. Com
/// ele (só o caminho autenticado usaria), o cabeçalho vai à origem e é
/// descartado em redirect para outro host.
pub fn get_to_file(url: &str, dest: &Path, token: Option<&str>) -> Result<(), String> {
    let mut req = agent(DOWNLOAD_TIMEOUT).get(url).set("User-Agent", UA);
    if let Some(t) = token {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    let resp = req.call().map_err(|e| format!("{url}: {e}"))?;
    let mut out = std::fs::File::create(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    std::io::copy(&mut resp.into_reader(), &mut out).map_err(|e| format!("{url}: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    /// (caminho, se a requisição trouxe `Authorization`).
    type Seen = Arc<Mutex<Vec<(String, bool)>>>;

    /// Servidor HTTP/1.1 mínimo em `l`, para `n` conexões. `route(caminho)`
    /// devolve (status, `Location`, corpo).
    fn serve(
        l: TcpListener,
        n: usize,
        route: impl Fn(&str) -> (u16, String, String) + Send + 'static,
    ) -> Seen {
        let seen: Seen = Arc::default();
        let s2 = seen.clone();
        thread::spawn(move || {
            for _ in 0..n {
                let Ok((mut sock, _)) = l.accept() else {
                    return;
                };
                let mut rd = BufReader::new(sock.try_clone().unwrap());
                let mut line = String::new();
                rd.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                let mut auth = false;
                loop {
                    let mut h = String::new();
                    if rd.read_line(&mut h).unwrap() == 0 || h == "\r\n" {
                        break;
                    }
                    auth |= h.to_ascii_lowercase().starts_with("authorization:");
                }
                s2.lock().unwrap().push((path.clone(), auth));
                let (code, loc, body) = route(&path);
                let loc = if loc.is_empty() {
                    String::new()
                } else {
                    format!("Location: {loc}\r\n")
                };
                let resp = format!(
                    "HTTP/1.1 {code} X\r\nContent-Length: {}\r\n{loc}Connection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(resp.as_bytes()).unwrap();
            }
        });
        seen
    }

    fn local() -> (TcpListener, u16) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        (l, p)
    }

    #[test]
    fn release_publica_sai_sem_authorization() {
        let (l, p) = local();
        let seen = serve(l, 3, |path| match path {
            "/repos/o/r" => (200, String::new(), r#"{"private":false}"#.into()),
            "/o/r/releases/download/v1/a.tar.gz" => (200, String::new(), "dados".into()),
            _ => (404, String::new(), String::new()),
        });
        let api = format!("http://127.0.0.1:{p}");
        assert!(!needs_token(&api, "o/r"));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("a.tar.gz");
        let url = format!("http://127.0.0.1:{p}/o/r/releases/download/v1/a.tar.gz");
        get_to_file(&url, &dest, None).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "dados");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|(_, auth)| !auth), "{seen:?}");
    }

    #[test]
    fn redirect_para_outro_host_nao_leva_token() {
        let (la, pa) = local();
        let (lb, pb) = local();
        let seen_b = serve(lb, 1, |_| (200, String::new(), "ok".into()));
        let loc = format!("http://localhost:{pb}/final");
        let seen_a = serve(la, 1, move |_| (302, loc.clone(), String::new()));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("x");
        get_to_file(
            &format!("http://127.0.0.1:{pa}/x"),
            &dest,
            Some("tok-de-teste"),
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "ok");
        assert_eq!(*seen_a.lock().unwrap(), [("/x".to_string(), true)]);
        assert_eq!(*seen_b.lock().unwrap(), [("/final".to_string(), false)]);
    }

    #[test]
    fn visibilidade_decide_se_exige_token() {
        let (l, p) = local();
        serve(l, 3, |path| match path {
            "/repos/o/priv" => (200, String::new(), r#"{"private":true}"#.into()),
            "/repos/o/gone" => (404, String::new(), "{}".into()),
            _ => (500, String::new(), String::new()),
        });
        let api = format!("http://127.0.0.1:{p}");
        assert!(needs_token(&api, "o/priv"));
        assert!(needs_token(&api, "o/gone"));
        // Falha que não prova nada (aqui um 500) não manda o token.
        assert!(!needs_token(&api, "o/outro"));
    }
}
