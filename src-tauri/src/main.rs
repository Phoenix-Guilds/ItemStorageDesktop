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

// Функция подключения консоли для Windows
#[cfg(target_os = "windows")]
fn attach_console() {
    use windows::Win32::System::Console::{AllocConsole, AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        // Пробуем прикрепиться к консоли родителя или создаем новую
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            let _ = AllocConsole();
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    attach_console();

    println!("--- ItemStorageBrowser Manager (Phoenix Nest Edition) ---");
    println!("[INFO] Инициализация системы...");

    let mut cfg: AppConfig = confy::load("item-storage-manager", None)?;

    let mut last_processed_char = String::new();
    let mut last_mode_admin = !is_admin_mode(&cfg.game_path);

    // 1. Валидация пути к игре
    while !is_valid_wow_path(&cfg.game_path) {
        println!("\n[!] Путь к игре некорректен: {}", cfg.game_path);
        print!("Введите полный путь к WoW (например, C:\\Games\\World of Warcraft): ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        cfg.game_path = input.trim().replace('"', "");
        confy::store("item-storage-manager", None, &cfg)?;
    }

    // 2. Проверка токена (только для Админов)
    if is_admin_mode(&cfg.game_path) && cfg.github_token.is_empty() {
        println!("\n[!] ОБНАРУЖЕН РЕЖИМ АДМИНА");
        println!("[!] Для синхронизации данных необходим GitHub Token.");
        print!("Введите токен: ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        cfg.github_token = input.trim().to_string();
        confy::store("item-storage-manager", None, &cfg)?;
    }

    println!("\n[OK] Настройка завершена. Программа запущена.");

    loop {
        let current_is_admin = is_admin_mode(&cfg.game_path);
        let db_relative_path = r"Interface\AddOns\ItemStorageBrowser\ItemStorageDB.lua";
        let db_path = Path::new(&cfg.game_path).join(db_relative_path);

        if current_is_admin {
            if !last_mode_admin {
                println!("\n[MODE] АКТИВИРОВАН РЕЖИМ: АДМИН");
                last_mode_admin = true;
            }

            if let Some(c) = run_admin_scan(&cfg.game_path) {
                let char_status = format!("{} | {}", c.name_realm, c.last_logout);

                if char_status != last_processed_char {
                    if db_path.exists() {
                        if let Ok(content) = fs::read_to_string(&db_path) {
                            println!(
                                "[LOG] Активность: {}. Проверка синхронизации...",
                                char_status
                            );
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
                                    println!(
                                        "[OK] Данные в облаке обновлены (если были изменения)."
                                    );
                                }
                                Err(e) => println!("[ERROR] Ошибка синхронизации: {}", e),
                            }
                        }
                    }
                }
            }
            sleep(Duration::from_secs(30)).await;
        } else {
            if last_mode_admin {
                println!("\n[MODE] АКТИВИРОВАН РЕЖИМ: ПОЛЬЗОВАТЕЛЬ");
                last_mode_admin = false;
            }

            println!("[USER] Проверка обновлений в GitHub...");
            match github::download_from_github(&cfg.github_token, "ItemStorageDB.lua").await {
                Ok(remote_content) => {
                    let mut need_update = true;
                    if db_path.exists() {
                        if let Ok(local_content) = fs::read_to_string(&db_path) {
                            // Сравниваем хеши локального и удаленного файла
                            if github::calculate_github_sha(&local_content)
                                == github::calculate_github_sha(&remote_content)
                            {
                                println!("[OK] Ваша база данных актуальна.");
                                need_update = false;
                            }
                        }
                    }

                    if need_update {
                        println!("[UPDATE] Найдена новая версия! Заменяю локальный файл...");
                        if let Err(e) = fs::write(&db_path, remote_content) {
                            println!("[ERROR] Не удалось записать файл: {}", e);
                        } else {
                            println!("[SUCCESS] База данных успешно обновлена.");
                        }
                    }
                }
                Err(e) => println!("[ERROR] Ошибка при загрузке данных: {}", e),
            }

            println!("[IDLE] Следующая проверка через 15 минут...");
            sleep(Duration::from_secs(15 * 60)).await;
        }
    }
}
