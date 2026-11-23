//! Window-level commands.

use tauri::{Manager, State};

use crate::state::AppState;

/// Written by the installer, read once by the app.
const INSTALL_LANGUAGE_FILE: &str = "install-language";

fn parse_install_language(text: &str) -> Option<&'static str> {
    match text.trim() {
        "zh-CN" => Some("zh-CN"),
        "en" => Some("en"),
        _ => None,
    }
}

/// Opens one of Linger's own pages in the default browser. The page cannot navigate by itself.
#[tauri::command(async)]
pub fn open_link(link: String) -> Result<(), String> {
    let url = link_url(&link).ok_or("settings.openFailed")?;
    #[cfg(windows)]
    std::process::Command::new("explorer").arg(url).spawn().map_err(|_| "settings.openFailed")?;
    #[cfg(not(windows))]
    let _ = url;
    Ok(())
}

fn link_url(link: &str) -> Option<&'static str> {
    match link {
        "home" => Some("https://github.com/ewwgkki/linger"),
        "licenses" => Some("https://github.com/ewwgkki/linger/blob/main/THIRD-PARTY-NOTICES.md"),
        _ => None,
    }
}

/// The language picked in the installer, if this is the first start since then.
#[tauri::command(async)]
pub fn take_installer_language(state: State<'_, AppState>) -> Option<String> {
    let path = state.data_dir.join(INSTALL_LANGUAGE_FILE);
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    parse_install_language(&text).map(str::to_string)
}

/// Window title for an interface language.
pub fn app_title(lang: &str) -> &'static str {
    if lang.starts_with("zh") { "Linger · 余音" } else { "Linger" }
}

#[tauri::command]
pub fn set_app_title(lang: String, window: tauri::Window) {
    let _ = window.set_title(app_title(&lang));
    crate::tray::set_language(window.app_handle(), &lang);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installer_language_is_validated() {
        assert_eq!(parse_install_language("zh-CN"), Some("zh-CN"));
        assert_eq!(parse_install_language(" en\r\n"), Some("en"));
        assert_eq!(parse_install_language("fr"), None);
        assert_eq!(parse_install_language(""), None);
    }

    #[test]
    fn only_known_links_open() {
        assert!(link_url("home").is_some());
        assert!(link_url("licenses").is_some());
        assert_eq!(link_url("https://example.com"), None);
    }

    #[test]
    fn title_follows_language() {
        assert_eq!(app_title("zh-CN"), "Linger · 余音");
        assert_eq!(app_title("en"), "Linger");
    }
}
