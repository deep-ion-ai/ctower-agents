//! "Abrir pasta de docs" (#444): onde fica, nesta máquina, uma pasta do
//! ambiente. WSL: pela distro (`\\wsl.localhost\<distro>\...` no Windows).
//! Ambiente "remoto" com URL de loopback (o `ctowerd` local do macOS, que
//! o app registra como remoto): o próprio caminho. Remoto de verdade: a
//! pasta fica em outra máquina, e não abre. Só `std`, testável à parte.

/// Tipo do ambiente para a decisão (espelha `envs::EnvKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Wsl,
    Remote,
}

/// A URL do ambiente aponta para esta máquina: `localhost`, `127.0.0.0/8`
/// ou `::1` (`ws://`, `wss://`, `http://` ou `https://`).
pub fn is_loopback(url: &str) -> bool {
    let url = url.trim().to_ascii_lowercase();
    let Some(rest) = ["ws://", "wss://", "http://", "https://"]
        .iter()
        .find_map(|s| url.strip_prefix(s))
    else {
        return false;
    };
    let auth = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let auth = auth.rsplit('@').next().unwrap_or_default();
    let host = match auth.strip_prefix('[') {
        Some(h) => h.split(']').next().unwrap_or_default(),
        None => auth.split(':').next().unwrap_or_default(),
    };
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Caminho absoluto, sem `..`, barra invertida nem caracteres de controle.
fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.chars().any(|c| c.is_control() || c == '\\')
        && !path.split('/').any(|seg| seg == "..")
}

/// Caminho, nesta máquina, de uma pasta da distro: no Windows,
/// `\\wsl.localhost\<distro>\...`; fora dele (app na própria máquina do
/// ambiente), o próprio caminho.
pub fn host_path(distro: &str, path: &str, windows: bool) -> Result<String, String> {
    let distro_ok = !distro.is_empty()
        && distro
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c));
    if !distro_ok || !valid_path(path) {
        return Err("distro ou caminho com caracteres não suportados".into());
    }
    if !windows {
        return Ok(path.to_string());
    }
    let rest = path.trim_start_matches('/').replace('/', "\\");
    Ok(format!("\\\\wsl.localhost\\{distro}\\{rest}"))
}

/// Onde abrir `path` do ambiente nesta máquina, ou por que não dá.
pub fn local_folder(
    kind: Kind,
    distro: Option<&str>,
    url: &str,
    path: &str,
    windows: bool,
) -> Result<String, String> {
    match (kind, distro) {
        (Kind::Wsl, Some(d)) => host_path(d, path, windows),
        (Kind::Remote, _) if is_loopback(url) => {
            if valid_path(path) {
                Ok(path.to_string())
            } else {
                Err("caminho com caracteres não suportados".into())
            }
        }
        _ => Err(
            "abrir a pasta só está disponível para ambientes desta máquina (WSL ou local)".into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_e_so_esta_maquina() {
        for u in [
            "ws://127.0.0.1:7878/ws",
            "ws://127.1.2.3:7878/ws",
            "WS://LOCALHOST:7878/ws",
            "ws://localhost/ws",
            "wss://[::1]:7878/ws",
            "ws://tok@127.0.0.1:7878/ws",
        ] {
            assert!(is_loopback(u), "{u}");
        }
        for u in [
            "ws://192.168.0.10:7878/ws",
            "ws://mac-mini:7878/ws",
            "ws://localhost.example.com/ws",
            "ws://127.0.0.1.example.com/ws",
            "ws://[::2]:7878/ws",
            "file:///tmp",
            "",
        ] {
            assert!(!is_loopback(u), "{u}");
        }
    }

    #[test]
    fn caminho_da_pasta_na_maquina() {
        assert_eq!(
            host_path("ct", "/mnt/ct-docs/o__r/epicos/7-x", true).unwrap(),
            r"\\wsl.localhost\ct\mnt\ct-docs\o__r\epicos\7-x"
        );
        assert_eq!(
            host_path("ct", "/mnt/ct-docs/o__r", false).unwrap(),
            "/mnt/ct-docs/o__r"
        );
        for (d, p) in [
            ("ct", "relativo/x"),
            ("ct", "/a/../b"),
            ("ct", "/a\\b"),
            ("ct", "/a\nb"),
            ("c t", "/a"),
            ("", "/a"),
        ] {
            assert!(host_path(d, p, true).is_err(), "{d} {p}");
        }
    }

    #[test]
    fn decisao_local_ou_remoto() {
        let mac = "ws://127.0.0.1:7878/ws";
        // ctowerd local do macOS (registrado como remoto): caminho direto,
        // nunca convertido para \\wsl.localhost, mesmo se `windows`.
        assert_eq!(
            local_folder(Kind::Remote, None, mac, "/Volumes/ctdocs/o__r", true).unwrap(),
            "/Volumes/ctdocs/o__r"
        );
        assert!(local_folder(Kind::Remote, None, mac, "/Volumes/../etc", false).is_err());
        assert!(local_folder(Kind::Remote, None, "ws://mac-mini:7878/ws", "/x", false).is_err());
        assert_eq!(
            local_folder(Kind::Wsl, Some("ct"), mac, "/mnt/d", true).unwrap(),
            r"\\wsl.localhost\ct\mnt\d"
        );
        assert!(local_folder(Kind::Wsl, None, mac, "/mnt/d", false).is_err());
    }
}
