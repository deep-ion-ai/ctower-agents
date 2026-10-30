//! Janela principal (#290): criada no `setup` (o `tauri.conf.json` só a
//! descreve, com `create: false`) para já abrir com a barra de título e o
//! fundo do tema do ui.json, sem o flash escuro no tema claro. O tema também
//! vai para a página (`window.__CT_THEME__`, lido pelo public/theme-boot.js).

use crate::envs::ThemePref;
use tauri::window::Color;
use tauri::{App, Theme, WebviewWindow, WebviewWindowBuilder, WindowEvent};

pub const MAIN: &str = "main";

/// `--bg` de theme.css em cada tema.
const DARK_BG: Color = Color(0x0f, 0x11, 0x15, 0xff);
const LIGHT_BG: Color = Color(0xf6, 0xf7, 0xf9, 0xff);

/// Tema da janela nativa; `None` segue o sistema.
fn native(pref: ThemePref) -> Option<Theme> {
    match pref {
        ThemePref::System => None,
        ThemePref::Light => Some(Theme::Light),
        ThemePref::Dark => Some(Theme::Dark),
    }
}

fn background(theme: Theme) -> Color {
    match theme {
        Theme::Light => LIGHT_BG,
        _ => DARK_BG,
    }
}

fn boot_script(pref: ThemePref) -> String {
    format!("window.__CT_THEME__ = \"{}\";", pref.as_str())
}

pub fn create(app: &App, pref: ThemePref) -> tauri::Result<WebviewWindow> {
    let cfg = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == MAIN)
        .cloned()
        .expect("janela main no tauri.conf.json");
    let mut builder = WebviewWindowBuilder::from_config(app.handle(), &cfg)?
        .theme(native(pref))
        .initialization_script(boot_script(pref));
    if let Some(t) = native(pref) {
        builder = builder.background_color(background(t));
    }
    let win = builder.build()?;
    sync_background(&win);
    // Seguindo o sistema, o fundo acompanha a troca de aparência.
    let w = win.clone();
    win.on_window_event(move |e| {
        if let WindowEvent::ThemeChanged(t) = e {
            let _ = w.set_background_color(Some(background(*t)));
        }
    });
    Ok(win)
}

/// Tema escolhido nas Configurações, aplicado na hora.
pub fn apply(win: &WebviewWindow, pref: ThemePref) {
    let _ = win.set_theme(native(pref));
    match native(pref) {
        Some(t) => {
            let _ = win.set_background_color(Some(background(t)));
        }
        // A troca para o tema do sistema chega por ThemeChanged.
        None => sync_background(win),
    }
}

fn sync_background(win: &WebviewWindow) {
    if let Ok(t) = win.theme() {
        let _ = win.set_background_color(Some(background(t)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tema_nativo_e_fundo() {
        assert_eq!(native(ThemePref::System), None);
        assert_eq!(native(ThemePref::Light), Some(Theme::Light));
        assert_eq!(background(Theme::Light), LIGHT_BG);
        assert_eq!(background(Theme::Dark), DARK_BG);
        assert_eq!(
            boot_script(ThemePref::Light),
            r#"window.__CT_THEME__ = "light";"#
        );
    }

    #[test]
    fn fundos_iguais_ao_bg_de_theme_css() {
        let css = include_str!("../../src/theme.css");
        let hex = |c: Color| format!("--bg: #{:02x}{:02x}{:02x};", c.0, c.1, c.2);
        let light = css
            .find(":root[data-theme=\"light\"]")
            .expect("bloco do tema claro");
        assert!(css[..light].contains(&hex(DARK_BG)));
        assert!(css[light..].contains(&hex(LIGHT_BG)));
    }
}
