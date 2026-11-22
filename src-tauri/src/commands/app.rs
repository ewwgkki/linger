//! Window-level commands.

use tauri::Manager;

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
    fn title_follows_language() {
        assert_eq!(app_title("zh-CN"), "Linger · 余音");
        assert_eq!(app_title("en"), "Linger");
    }
}
