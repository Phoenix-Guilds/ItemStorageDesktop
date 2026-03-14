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
