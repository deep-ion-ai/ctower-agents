//! App de Windows do Control Tower (spec §10, §13): casca fina sobre o
//! `ct-client`. Comandos para o JavaScript; eventos e canais de terminal
//! saem da [`bridge::Bridge`].

pub mod bridge;
pub mod envs;
pub mod folder;
pub mod hub;
pub mod notify;
pub mod uilog;
pub mod updater;
pub mod window;
pub mod wsl;

use bridge::{Bridge, EnvStatus, Resolver, Sink, TermSink};
use envs::{EnvEntry, EnvInput, EnvKind, EnvView, NotifyPrefs, Store, UiConfig};
use hub::{HubEntry, HubInput, HubSink, HubView, HUB_ID};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

struct AppState {
    bridge: Arc<Bridge>,
    /// Ponte só da conexão com o hub (`hub.info`, protocolo do hub).
    hub_bridge: Arc<Bridge>,
    store: Store,
    envs: Mutex<Vec<EnvEntry>>,
    hub: Mutex<Option<HubEntry>>,
    /// Log da própria UI (#65), em `<app_log_dir>/ui.jsonl`.
    ui_log: uilog::UiLog,
}

struct WindowSink(AppHandle);

impl Sink for WindowSink {
    fn emit(&self, event: &str, payload: Value) {
        let _ = self.0.emit(event, payload);
    }

    fn notify(&self, title: &str, body: &str) {
        let _ = self
            .0
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show();
    }
}

/// Token do ambiente: da distro (sobe a distro antes) ou do `envs.json`.
fn resolve_token(e: &EnvEntry) -> Result<String, String> {
    match e.kind {
        EnvKind::Wsl => {
            let distro = e.distro.as_deref().unwrap_or(envs::DEFAULT_DISTRO);
            wsl::bootstrap(distro)?;
            wsl::read_token(distro)
        }
        EnvKind::Remote => e
            .token
            .clone()
            .filter(|t| !t.is_empty())
            .ok_or_else(|| "ambiente sem token configurado".into()),
    }
}

#[tauri::command]
fn envs_list(st: State<'_, AppState>) -> Vec<EnvView> {
    st.envs.lock().unwrap().iter().map(EnvView::from).collect()
}

#[tauri::command]
async fn envs_save(st: State<'_, AppState>, envs: Vec<EnvInput>) -> Result<Vec<EnvView>, String> {
    let new = {
        let mut cur = st.envs.lock().unwrap();
        let new = envs::merge(&cur, envs)?;
        st.store.save_envs(&new)?;
        *cur = new.clone();
        new
    };
    st.bridge.reconfigure(new.clone());
    Ok(new.iter().map(EnvView::from).collect())
}

#[tauri::command]
fn env_statuses(st: State<'_, AppState>) -> Vec<EnvStatus> {
    st.bridge.statuses()
}

#[tauri::command]
async fn daemon_call(
    st: State<'_, AppState>,
    env: String,
    method: String,
    params: Value,
) -> Result<Value, String> {
    st.bridge.call(&env, &method, params).await
}

#[tauri::command]
fn hub_get(st: State<'_, AppState>) -> Option<HubView> {
    st.hub.lock().unwrap().as_ref().map(HubView::from)
}

/// `hub: null` desliga a conexão com o hub.
#[tauri::command]
async fn hub_save(
    st: State<'_, AppState>,
    hub: Option<HubInput>,
) -> Result<Option<HubView>, String> {
    let new = {
        let mut cur = st.hub.lock().unwrap();
        let new = hub::merge(cur.as_ref(), hub)?;
        st.store.save_hub(new.as_ref())?;
        *cur = new.clone();
        new
    };
    st.hub_bridge
        .reconfigure(new.iter().map(hub::bridge_entry).collect());
    Ok(new.as_ref().map(HubView::from))
}

#[tauri::command]
fn hub_status(st: State<'_, AppState>) -> Option<EnvStatus> {
    st.hub_bridge
        .statuses()
        .into_iter()
        .find(|s| s.env == HUB_ID)
}

#[tauri::command]
async fn hub_call(st: State<'_, AppState>, method: String, params: Value) -> Result<Value, String> {
    st.hub_bridge.call(HUB_ID, &method, params).await
}

#[tauri::command]
async fn term_attach(
    st: State<'_, AppState>,
    env: String,
    session: String,
    channel: Channel<InvokeResponseBody>,
) -> Result<(), String> {
    let sink: TermSink = Box::new(move |bytes| {
        let _ = channel.send(InvokeResponseBody::Raw(bytes));
    });
    st.bridge.attach(&env, &session, sink).await
}

#[tauri::command]
async fn term_detach(st: State<'_, AppState>, env: String, session: String) -> Result<(), String> {
    st.bridge.detach(&env, &session).await
}

#[tauri::command]
fn term_input(
    st: State<'_, AppState>,
    env: String,
    session: String,
    data: String,
) -> Result<(), String> {
    st.bridge.input(&env, &session, &data)
}

#[tauri::command]
async fn term_resize(
    st: State<'_, AppState>,
    env: String,
    session: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    st.bridge.resize(&env, &session, cols, rows).await
}

#[tauri::command]
fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    if !openable(&url) {
        return Err("só endereços http:// e https:// são abertos".into());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

/// Só a web abre no navegador (links do console incluem `http://localhost`).
fn openable(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

#[tauri::command]
fn open_vscode(st: State<'_, AppState>, env: String, path: String) -> Result<(), String> {
    let e = st
        .bridge
        .entry(&env)
        .ok_or_else(|| format!("ambiente desconhecido: {env}"))?;
    match (e.kind, e.distro) {
        (EnvKind::Wsl, Some(distro)) => wsl::open_vscode(&distro, &path),
        _ => Err("abrir no VS Code só está disponível para ambientes WSL".into()),
    }
}

/// "Abrir pasta de docs" (#444): abre no gerenciador de arquivos do
/// sistema a pasta que o ambiente devolveu (`docs.dir`). Só ambiente desta
/// máquina: WSL ou "remoto" de loopback (o `ctowerd` local do macOS); a
/// pasta de um remoto de verdade fica em outra máquina ([`folder`]).
#[tauri::command]
fn open_folder(
    app: AppHandle,
    st: State<'_, AppState>,
    env: String,
    path: String,
) -> Result<(), String> {
    let e = st
        .bridge
        .entry(&env)
        .ok_or_else(|| format!("ambiente desconhecido: {env}"))?;
    let kind = match e.kind {
        EnvKind::Wsl => folder::Kind::Wsl,
        EnvKind::Remote => folder::Kind::Remote,
    };
    let local = folder::local_folder(kind, e.distro.as_deref(), &e.url, &path, cfg!(windows))?;
    app.opener()
        .open_path(local, None::<&str>)
        .map_err(|e| format!("não foi possível abrir a pasta: {e}"))
}

/// Texto da área de transferência (colar no console), pelo plugin: não
/// depende do `navigator.clipboard` da WebView.
#[tauri::command]
fn clipboard_read(app: AppHandle) -> Result<String, String> {
    app.clipboard().read_text().map_err(|e| e.to_string())
}

#[tauri::command]
fn clipboard_write(app: AppHandle, text: String) -> Result<(), String> {
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

/// Token do GitHub para o updater: `CTOWER_GITHUB_TOKEN` ou o da primeira
/// distro WSL configurada.
async fn github_token(st: &AppState) -> Option<String> {
    if let Ok(t) = std::env::var("CTOWER_GITHUB_TOKEN") {
        return Some(t).filter(|t| !t.trim().is_empty());
    }
    let distro = st
        .envs
        .lock()
        .unwrap()
        .iter()
        .find(|e| e.kind == EnvKind::Wsl)
        .map(|e| {
            e.distro
                .clone()
                .unwrap_or_else(|| envs::DEFAULT_DISTRO.into())
        })?;
    tauri::async_runtime::spawn_blocking(move || wsl::read_github_token(&distro))
        .await
        .ok()
        .flatten()
}

#[tauri::command]
async fn ui_update_check(
    app: AppHandle,
    st: State<'_, AppState>,
) -> Result<updater::UiUpdate, String> {
    let token = github_token(&st).await;
    Ok(updater::check(&app, token).await)
}

/// Baixa e instala a versão nova e reinicia o app; o progresso sai em
/// `ui://update-progress` (`{downloaded, total}`).
#[tauri::command]
async fn ui_update_install(app: AppHandle, st: State<'_, AppState>) -> Result<(), String> {
    let token = github_token(&st).await;
    let emitter = app.clone();
    updater::install(&app, token, move |downloaded, total| {
        let _ = emitter.emit(
            "ui://update-progress",
            serde_json::json!({ "downloaded": downloaded, "total": total }),
        );
    })
    .await
}

#[tauri::command]
fn ui_config_get(st: State<'_, AppState>) -> UiConfig {
    st.store.ui_config()
}

#[tauri::command]
fn ui_config_save(app: AppHandle, st: State<'_, AppState>, cfg: UiConfig) -> Result<(), String> {
    st.store.save_ui_config(&cfg)?;
    if let Some(w) = app.get_webview_window(window::MAIN) {
        window::apply(&w, cfg.appearance.theme);
    }
    st.bridge.set_prefs(cfg.notify);
    Ok(())
}

/// Acrescenta linhas (JSON, já mascaradas pela UI) ao log da UI (#65).
#[tauri::command]
fn ui_log_append(st: State<'_, AppState>, lines: Vec<String>) -> Result<(), String> {
    st.ui_log.append(&lines).map_err(|e| e.to_string())
}

/// As últimas linhas do log da UI, inclusive as da execução anterior.
#[tauri::command]
fn ui_log_read(st: State<'_, AppState>) -> Vec<String> {
    st.ui_log.read(uilog::READ_MAX)
}

/// Exportar da tela Logs (#65): grava na pasta Downloads do sistema, sem
/// sobrescrever, e devolve o caminho.
#[tauri::command]
fn save_download(app: AppHandle, name: String, content: String) -> Result<String, String> {
    let dir = app.path().download_dir().map_err(|e| e.to_string())?;
    uilog::save_into(&dir, &name, &content)
        .map(|p| p.display().to_string())
        .map_err(|e| e.to_string())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let store = Store::new(&app.path().app_data_dir()?);
            window::create(app, store.ui_config().appearance.theme)?;
            let envs = store.envs();
            let resolver: Resolver = Arc::new(resolve_token);
            let bridge = Bridge::new(
                Arc::new(WindowSink(app.handle().clone())),
                resolver,
                store.ui_config().notify,
            );
            let b = bridge.clone();
            let initial = envs.clone();
            tauri::async_runtime::spawn(async move { b.start(initial) });
            let hub = store.hub();
            let silent = NotifyPrefs {
                needs_human: false,
                session_error: false,
                env_down: false,
                muted_envs: Vec::new(),
            };
            let hub_bridge = Bridge::with_handshake(
                Arc::new(HubSink(Arc::new(WindowSink(app.handle().clone())))),
                Arc::new(resolve_token),
                silent,
                ct_client::Handshake::HUB,
            );
            let hb = hub_bridge.clone();
            let initial: Vec<EnvEntry> = hub.iter().map(hub::bridge_entry).collect();
            tauri::async_runtime::spawn(async move { hb.start(initial) });
            let ui_log = uilog::UiLog::new(&app.path().app_log_dir()?);
            app.manage(AppState {
                bridge,
                hub_bridge,
                store,
                envs: Mutex::new(envs),
                hub: Mutex::new(hub),
                ui_log,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            envs_list,
            envs_save,
            env_statuses,
            daemon_call,
            hub_get,
            hub_save,
            hub_status,
            hub_call,
            term_attach,
            term_detach,
            term_input,
            term_resize,
            open_url,
            open_vscode,
            open_folder,
            clipboard_read,
            clipboard_write,
            ui_config_get,
            ui_config_save,
            ui_update_check,
            ui_update_install,
            ui_log_append,
            ui_log_read,
            save_download,
        ])
        .run(tauri::generate_context!())
        .expect("erro ao iniciar o Control Tower");
}

#[cfg(test)]
mod tests {
    use super::openable;

    #[test]
    fn so_abre_http_e_https() {
        assert!(openable("https://github.com/o/r/pull/1"));
        assert!(openable("http://localhost:5173/"));
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "vscode://x",
            "ftp://h/f",
            "",
        ] {
            assert!(!openable(url), "{url}");
        }
    }
}
