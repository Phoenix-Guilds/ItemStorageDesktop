use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppConfig {
    pub game_path: String,
    pub github_token: String,
    pub force_user_mode: bool,

    // Кэш последнего обработанного персонажа
    pub last_char_name: String,
    pub last_char_logout: String,

    #[serde(default)]
    pub first_run_sync_done: bool,

    #[serde(default = "default_autostart")]
    pub autostart_enabled: bool,

    #[serde(default = "default_close_to_tray")]
    pub close_to_tray: bool,

    #[serde(default)]
    pub window_geometry: Option<WindowGeometry>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Default for WindowGeometry {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        }
    }
}

fn default_autostart() -> bool {
    true
}

fn default_close_to_tray() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            game_path: "".into(),
            github_token: "".into(),
            force_user_mode: false,
            last_char_name: String::from(""),
            last_char_logout: String::from(""),
            first_run_sync_done: false,
            autostart_enabled: true,
            close_to_tray: true,
            window_geometry: None,
        }
    }
}

pub fn is_valid_wow_path(path: &str) -> bool {
    let p = Path::new(path);
    p.join("WTF").exists()
}

pub fn is_admin_mode(game_path: &str) -> bool {
    Path::new(game_path)
        .join(r"Interface\AddOns\CharacterStatusLogger")
        .exists()
}

#[cfg(test)]
mod tests {
    use super::AppConfig;

    #[test]
    fn close_to_tray_defaults_to_enabled_for_existing_configs() {
        let config: AppConfig = serde_json::from_str(
            r#"{"game_path":"","github_token":"","force_user_mode":false,"last_char_name":"","last_char_logout":"","first_run_sync_done":false,"autostart_enabled":true}"#,
        )
        .unwrap();

        assert!(config.close_to_tray);
    }
}
