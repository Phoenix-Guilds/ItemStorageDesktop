#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod github;
mod scanner;

use crate::config::{is_admin_mode, is_valid_wow_path, AppConfig};
use crate::scanner::run_admin_scan;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("--- ItemStorageBrowser Manager (Dev Version) ---");

    let mut cfg: AppConfig = confy::load("item-storage-manager", None)?;

    let mut last_processed_char = String::new();
    let mut last_mode_admin = !is_admin_mode(&cfg.game_path);

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

    // 2. Проверка токена (для админа или на случай лимитов API)
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
        let db_relative_path = r"Interface\AddOns\ItemStorageBrowser\ItemStorageDB.lua";
        let db_path = Path::new(&cfg.game_path).join(db_relative_path);

        if current_is_admin {
            if !last_mode_admin {
                println!("\n[MODE] Режим: АДМИН.");
                last_mode_admin = true;
            }

            if let Some(c) = run_admin_scan(&cfg.game_path) {
                let char_status = format!("{} | {}", c.name_realm, c.last_logout);

                if char_status != last_processed_char {
                    println!("[RESULT] Последний активный: {}", char_status);

                    if db_path.exists() {
                        if let Ok(content) = fs::read_to_string(&db_path) {
                            println!("[GITHUB] Проверка синхронизации...");
                            let msg = format!(
                                "Update DB: {}",
                                chrono::Local::now().format("%Y-%m-%d %H:%M")
                            );

                            match github::upload_to_github(
                                &cfg.github_token,
                                &content,
                                "ItemStorageDB.lua",
                                &msg,
                            )
                            .await
                            {
                                Ok(_) => {
                                    last_processed_char = char_status;
                                }
                                Err(e) => {
                                    println!("[ERROR] Синхронизация не удалась: {}", e);
                                    if e.to_string().contains("403")
                                        || e.to_string().contains("401")
                                    {
                                        print!("Ошибка доступа. Введите новый токен: ");
                                        io::stdout().flush()?;
                                        let mut nt = String::new();
                                        io::stdin().read_line(&mut nt)?;
                                        if !nt.trim().is_empty() {
                                            cfg.github_token = nt.trim().to_string();
                                            let _ =
                                                confy::store("item-storage-manager", None, &cfg);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            sleep(Duration::from_secs(30)).await;
        } else {
            if last_mode_admin {
                println!("\n[MODE] Режим: ПОЛЬЗОВАТЕЛЬ.");
                last_mode_admin = false;
            }

            println!("[USER] Проверка обновлений базы данных...");
            match github::download_from_github(&cfg.github_token, "ItemStorageDB.lua").await {
                Ok(remote_content) => {
                    let mut need_update = true;
                    if db_path.exists() {
                        if let Ok(local_content) = fs::read_to_string(&db_path) {
                            if github::calculate_github_sha(&local_content)
                                == github::calculate_github_sha(&remote_content)
                            {
                                println!("[OK] У вас установлена актуальная база.");
                                need_update = false;
                            }
                        }
                    }

                    if need_update {
                        println!("[UPDATE] Найдена новая версия! Заменяю локальный файл...");
                        if let Err(e) = fs::write(&db_path, remote_content) {
                            println!("[ERROR] Не удалось сохранить файл: {}", e);
                        } else {
                            println!("[SUCCESS] База данных успешно обновлена.");
                        }
                    }
                }
                Err(e) => println!("[ERROR] Не удалось проверить обновления: {}", e),
            }

            sleep(Duration::from_secs(15 * 60)).await;
        }
    }
}
