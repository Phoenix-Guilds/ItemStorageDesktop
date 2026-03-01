use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppConfig {
    pub game_path: String,
    pub github_token: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            game_path: String::from(r"C:\Games\World of Warcraft"),
            github_token: String::from(""),
        }
    }
}

pub fn is_valid_wow_path(path: &str) -> bool {
    let p = Path::new(path);
    p.exists() && p.is_dir() && p.join("WTF").exists()
}

pub fn is_admin_mode(game_path: &str) -> bool {
    Path::new(game_path).join(r"Interface\AddOns\CharacterStatusLogger").exists()
}