//! Atualização do próprio app (issue #90) pelo `tauri-plugin-updater`: o
//! `latest.json` e o instalador assinado ficam na última release do GitHub.
//! O repositório é privado, então a busca e o download levam o token do
//! GitHub lido da distro (nunca do JavaScript), pela API de assets.

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Runtime};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Repositório das releases do app.
pub const REPO: &str = "deep-ion-ai/ctower-agents";

/// Resultado de uma verificação (vai para o JavaScript).
#[derive(Debug, Clone, Serialize)]
pub struct UiUpdate {
    pub current: String,
    /// Versão nova; `None` = em dia.
    pub available: Option<UiRelease>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UiRelease {
    pub version: String,
    pub notes: Option<String>,
    /// RFC 3339.
    pub date: Option<String>,
}

/// URL de API do asset `latest.json` na resposta de `releases/latest`.
pub fn latest_json_asset(release: &Value) -> Option<String> {
    release["assets"]
        .as_array()?
        .iter()
        .find(|a| a["name"] == "latest.json")?
        .get("url")?
        .as_str()
        .map(String::from)
}

fn api_get(url: &str, token: Option<&str>) -> reqwest::RequestBuilder {
    let r = reqwest::Client::new()
        .get(url)
        .header("User-Agent", "control-tower-ui")
        .header("Accept", "application/vnd.github+json");
    match token {
        Some(t) => r.bearer_auth(t),
        None => r,
    }
}

/// Descobre o `latest.json` da última release.
async fn manifest_url(token: Option<&str>) -> Result<String, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = api_get(&url, token)
        .send()
        .await
        .map_err(|e| format!("GitHub inacessível: {e}"))?;
    match resp.status().as_u16() {
        200 => {}
        404 if token.is_none() => {
            return Err("sem token do GitHub para ler as releases (repositório privado)".into())
        }
        404 => return Err("nenhuma release publicada".into()),
        401 | 403 => return Err("token do GitHub recusado".into()),
        s => return Err(format!("GitHub respondeu {s}")),
    }
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    latest_json_asset(&v).ok_or_else(|| {
        format!(
            "a release {} não tem latest.json",
            v["tag_name"].as_str().unwrap_or("?")
        )
    })
}

async fn find<R: Runtime>(
    app: &AppHandle<R>,
    token: Option<&str>,
) -> Result<Option<Update>, String> {
    let endpoint = manifest_url(token).await?;
    let mut b = app
        .updater_builder()
        .endpoints(vec![endpoint.parse().map_err(|e| format!("{e}"))?])
        .map_err(|e| e.to_string())?
        // API de assets: o conteúdo cru vem com octet-stream (latest.json e .exe).
        .header("Accept", "application/octet-stream")
        .map_err(|e| e.to_string())?
        .header("User-Agent", "control-tower-ui")
        .map_err(|e| e.to_string())?;
    if let Some(t) = token {
        b = b
            .header("Authorization", format!("Bearer {t}"))
            .map_err(|e| e.to_string())?;
    }
    b.build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())
}

pub async fn check<R: Runtime>(app: &AppHandle<R>, token: Option<String>) -> UiUpdate {
    let current = app.package_info().version.to_string();
    match find(app, token.as_deref()).await {
        Ok(u) => UiUpdate {
            current,
            available: u.map(|u| UiRelease {
                version: u.version.clone(),
                notes: u.body.clone(),
                date: u.date.and_then(|d| {
                    d.format(&time::format_description::well_known::Rfc3339)
                        .ok()
                }),
            }),
            error: None,
        },
        Err(e) => UiUpdate {
            current,
            available: None,
            error: Some(e),
        },
    }
}

/// Baixa, confere a assinatura, instala e reinicia o app. `progress`
/// recebe (baixado, total).
pub async fn install<R: Runtime>(
    app: &AppHandle<R>,
    token: Option<String>,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<(), String> {
    let update = find(app, token.as_deref())
        .await?
        .ok_or_else(|| "o app já está na versão mais nova".to_string())?;
    let mut done = 0u64;
    update
        .download_and_install(
            |chunk, total| {
                done += chunk as u64;
                progress(done, total);
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn acha_o_latest_json_entre_os_assets() {
        let v = json!({ "tag_name": "v0.2.0", "assets": [
            { "name": "Control.Tower_0.2.0_x64-setup.exe", "url": "https://api.github.com/x/1" },
            { "name": "latest.json", "url": "https://api.github.com/x/2" },
        ]});
        assert_eq!(
            latest_json_asset(&v).as_deref(),
            Some("https://api.github.com/x/2")
        );
        assert_eq!(latest_json_asset(&json!({ "assets": [] })), None);
        assert_eq!(latest_json_asset(&json!({})), None);
    }
}
