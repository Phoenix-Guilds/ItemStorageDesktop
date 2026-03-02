#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod github;
mod scanner;

use crate::config::{is_admin_mode, is_valid_wow_path, AppConfig};
use crate::scanner::run_admin_scan;
use semver::Version;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, SystemTime};
use tokio::time::sleep;

#[cfg(target_os = "windows")]
fn attach_console() {
    use windows::Win32::System::Console::{AllocConsole, AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            let _ = AllocConsole();
        }
    }
}

fn get_local_file_time(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| {
            t.duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        })
        .unwrap_or(0)
}

async fn perform_sync(
    cfg: &mut AppConfig,
    db_path: &Path,
    is_initial: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if is_initial {
        println!("[SYNC] Сверка базы данных с облаком...");
    }

    if db_path.exists() {
        if let Ok(local_content) = fs::read_to_string(db_path) {
            let local_sha = github::calculate_github_sha(&local_content);

            let client = reqwest::Client::new();
            let url = "https://api.github.com/repos/Phoenix-Guilds/ItemStorageBrowser/contents/ItemStorageDB.lua?ref=data";
            let resp = client
                .get(url)
                .headers(github::get_gh_headers(&cfg.github_token))
                .send()
                .await?;

            if resp.status().is_success() {
                let json: serde_json::Value = resp.json().await?;
                let remote_sha = json["sha"].as_str().unwrap_or("");

                if local_sha == remote_sha {
                    if is_initial {
                        println!("[OK] Файлы идентичны по хешу.");
                    }
                    return Ok(());
                }
            }
        }
    }

    // Если это первый запуск и SHA не совпал (или файла нет) - ВСЕГДА скачиваем из облака
    if is_initial && !cfg.first_run_sync_done {
        println!("[SYNC] Первая синхронизация: приоритет облачной версии.");
        let content = github::download_from_github(&cfg.github_token, "ItemStorageDB.lua").await?;
        fs::write(db_path, content)?;
        cfg.first_run_sync_done = true;
        let _ = confy::store("item-storage-manager", None, &cfg);
        println!("[SUCCESS] База синхронизирована с облаком.");
        return Ok(());
    }

    // Обычная логика сравнения времени
    let remote_time = github::get_remote_file_time(&cfg.github_token, "ItemStorageDB.lua")
        .await
        .unwrap_or(0);
    let local_time = get_local_file_time(db_path);

    let time_diff = if local_time > remote_time {
        local_time - remote_time
    } else {
        remote_time - local_time
    };

    if time_diff > 5 {
        if remote_time > local_time {
            println!("[SYNC] В облаке версия новее. Загрузка...");
            let content =
                github::download_from_github(&cfg.github_token, "ItemStorageDB.lua").await?;
            fs::write(db_path, content)?;
            println!("[SUCCESS] Локальная база обновлена.");
        } else if local_time > remote_time && is_admin_mode(&cfg.game_path) {
            println!("[SYNC] Локальная база новее. Отправка в облако...");
            let content = fs::read_to_string(db_path)?;
            let msg = format!(
                "Auto-update: {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M")
            );
            github::upload_to_github(&cfg.github_token, &content, "ItemStorageDB.lua", &msg)
                .await?;
            println!("[SUCCESS] Облако обновлено.");
        }
    } else if is_initial {
        println!("[OK] Разница во времени незначительна.");
    }

    Ok(())
}

async fn check_for_updates() {
    let current_v_str = env!("CARGO_PKG_VERSION");
    println!(
        "[INFO] Ваша версия: v{}. Проверка обновлений...",
        current_v_str
    );

    match github::get_latest_release_version().await {
        Ok((latest_tag, release_url)) => {
            let latest_v_clean = latest_tag.trim_start_matches('v');
            if let (Ok(latest_v), Ok(current_v)) = (
                Version::parse(latest_v_clean),
                Version::parse(current_v_str),
            ) {
                if latest_v > current_v {
                    println!("\n==========================================");
                    println!("[UPDATE] ДОСТУПНА НОВАЯ ВЕРСИЯ ПРИЛОЖЕНИЯ: v{}", latest_v);
                    println!("[UPDATE] Ссылка: {}", release_url);
                    println!("[UPDATE] Открыть страницу загрузки в браузере? (y/n)");
                    println!("==========================================\n");
                    io::stdout().flush().unwrap();
                    let mut input = String::new();
                    if io::stdin().read_line(&mut input).is_ok()
                        && input.trim().to_lowercase() == "y"
                    {
                        let _ = open::that(release_url);
                    }
                } else {
                    println!("[INFO] У вас установлена актуальная версия приложения.");
                }
            }
        }
        Err(e) => println!("[WARN] Не удалось проверить обновления приложения: {}", e),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    attach_console();

    println!("--- ItemStorageBrowser Manager (Phoenix Nest Edition) ---");
    check_for_updates().await;

    let mut cfg: AppConfig = match confy::load("item-storage-manager", None) {
        Ok(config) => config,
        Err(_) => AppConfig::default(),
    };

    while !is_valid_wow_path(&cfg.game_path) {
        println!("\n[!] Путь к игре некорректен.");
        print!("Введите полный путь к WoW: ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        cfg.game_path = input.trim().replace('"', "");
        let _ = confy::store("item-storage-manager", None, &cfg);
    }

    if is_admin_mode(&cfg.game_path) && cfg.github_token.is_empty() {
        print!("\n[!] Режим админа. Введите GitHub Token: ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        cfg.github_token = input.trim().to_string();
        let _ = confy::store("item-storage-manager", None, &cfg);
    }

    let db_path =
        Path::new(&cfg.game_path).join(r"Interface\AddOns\ItemStorageBrowser\ItemStorageDB.lua");

    if db_path.exists() {
        let _ = perform_sync(&mut cfg, &db_path, true).await;

        if is_admin_mode(&cfg.game_path) {
            if let Some(c) = run_admin_scan(&cfg.game_path) {
                cfg.last_char_name = c.name_realm;
                cfg.last_char_logout = c.last_logout.to_string();
                let _ = confy::store("item-storage-manager", None, &cfg);
                println!("[INFO] Кэш активности персонажей синхронизирован.");
            }
        }
    }

    println!("\n[OK] Система запущена. Ожидание активности...");

    loop {
        if is_admin_mode(&cfg.game_path) {
            if let Some(c) = run_admin_scan(&cfg.game_path) {
                let current_logout_str = c.last_logout.to_string();
                if c.name_realm != cfg.last_char_name || current_logout_str != cfg.last_char_logout
                {
                    println!("\n[EVENT] Обнаружена активность: {}.", c.name_realm);
                    cfg.last_char_name = c.name_realm;
                    cfg.last_char_logout = current_logout_str;
                    let _ = confy::store("item-storage-manager", None, &cfg);

                    println!("[SYNC] Обнаружены изменения. Сверка с облаком...");
                    let _ = perform_sync(&mut cfg, &db_path, false).await;
                } else {
                    let _ = perform_sync(&mut cfg, &db_path, false).await;
                }
            } else {
                let _ = perform_sync(&mut cfg, &db_path, false).await;
            }

            sleep(Duration::from_secs(30)).await;
        } else {
            println!("[IDLE] Плановая проверка базы...");
            let _ = perform_sync(&mut cfg, &db_path, false).await;
            sleep(Duration::from_secs(15 * 60)).await;
        }
    }
}
