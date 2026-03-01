#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod scanner;
mod github;

use std::io::{self, Write};
use std::time::Duration;
use tokio::time::sleep;
use crate::config::{AppConfig, is_valid_wow_path, is_admin_mode};
use crate::scanner::run_admin_scan;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("--- ItemStorageBrowser Manager (Dev Version) ---");

    let mut cfg: AppConfig = confy::load("item-storage-manager", None)?;
    
    // Переменные для хранения состояния (чтобы не спамить)
    let mut last_processed_char = String::new();
    let mut last_mode_admin = !is_admin_mode(&cfg.game_path); // Чтобы триггернуть сообщение при старте

    // 1. Валидация пути
    while !is_valid_wow_path(&cfg.game_path) {
        println!("[!] Путь к игре некорректен: {}", cfg.game_path);
        print!("Введите полный путь к WoW: ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        cfg.game_path = input.trim().replace('"', "");
        confy::store("item-storage-manager", None, &cfg)?;
    }

    // 2. Проверка токена
    if is_admin_mode(&cfg.game_path) && cfg.github_token.is_empty() {
        println!("[!] Для админского режима необходим GitHub Token.");
        print!("Введите токен: ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        cfg.github_token = input.trim().to_string();
        confy::store("item-storage-manager", None, &cfg)?;
    }

    loop {
        let current_is_admin = is_admin_mode(&cfg.game_path);

        if current_is_admin {
            // Выводим смену режима только один раз
            if !last_mode_admin {
                println!("\n[MODE] Режим переключен на: АДМИН. Сканирование запущено.");
                last_mode_admin = true;
            }

            if let Some(c) = run_admin_scan(&cfg.game_path) {
                let char_status = format!("{} | {}", c.name_realm, c.last_logout);
                
                // Выводим результат только если он изменился
                if char_status != last_processed_char {
                    println!("[RESULT] Последний активный: {}", char_status);
                    last_processed_char = char_status;
                    // ТУТ БУДЕТ ВЫЗОВ UPLOAD НА GITHUB
                }
            }
            sleep(Duration::from_secs(30)).await;
        } else {
            if last_mode_admin {
                println!("\n[MODE] Режим переключен на: ПОЛЬЗОВАТЕЛЬ.");
                last_mode_admin = false;
            }
            // Здесь будет логика пользователя (run_user_logic)
            sleep(Duration::from_secs(15 * 60)).await;
        }
    }
}