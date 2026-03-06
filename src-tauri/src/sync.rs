use crate::config::{is_admin_mode, is_valid_wow_path, AppConfig};
use crate::github;
use crate::log_to_window;
use crate::modes::{admin, user}; // Импорт режимов
use semver::Version;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tauri::{Manager, Runtime};
use tokio::sync::Notify;
use tokio::time::sleep;

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

pub async fn check_for_updates<R: Runtime>(handle: tauri::AppHandle<R>) {
    let current_v_str = env!("CARGO_PKG_VERSION");
    log_to_window(
        &handle,
        format!(
            "[INFO] Ваша версия: v{}. Проверка обновлений...",
            current_v_str
        ),
    );

    match github::get_latest_release_version().await {
        Ok((latest_tag, release_url)) => {
            let latest_v_clean = latest_tag.trim_start_matches('v');
            if let (Ok(latest_v), Ok(current_v)) = (
                Version::parse(latest_v_clean),
                Version::parse(current_v_str),
            ) {
                if latest_v > current_v {
                    log_to_window(
                        &handle,
                        "[UPDATE] ==========================================".to_string(),
                    );
                    log_to_window(
                        &handle,
                        format!("[UPDATE] ДОСТУПНА НОВАЯ ВЕРСИЯ: v{}", latest_v),
                    );
                    log_to_window(&handle, format!("[UPDATE] Ссылка: {}", release_url));
                    log_to_window(
                        &handle,
                        "[UPDATE] ==========================================".to_string(),
                    );
                } else {
                    log_to_window(
                        &handle,
                        "[INFO] У вас установлена актуальная версия.".to_string(),
                    );
                }
            }
        }
        Err(e) => log_to_window(&handle, format!("[WARN] Ошибка проверки обновлений: {}", e)),
    }
}

pub async fn perform_sync<R: Runtime>(
    handle: &tauri::AppHandle<R>,
    cfg: &mut AppConfig,
    db_path: &Path,
    is_initial: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if is_initial {
        log_to_window(handle, "[SYNC] Сверка базы данных с облаком...".to_string());
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
                        log_to_window(handle, "[OK] Файлы идентичны.".to_string());
                    }
                    return Ok(());
                }
            }
        }
    }

    if is_initial && !cfg.first_run_sync_done {
        log_to_window(handle, "[SYNC] Первая синхронизация...".to_string());
        let content = github::download_from_github(&cfg.github_token, "ItemStorageDB.lua").await?;
        fs::write(db_path, content)?;
        cfg.first_run_sync_done = true;
        let _ = confy::store("item-storage-manager", None, &cfg);
        log_to_window(
            handle,
            "[SUCCESS] База синхронизирована с облаком.".to_string(),
        );
        return Ok(());
    }

    let remote_time = github::get_remote_file_time(&cfg.github_token, "ItemStorageDB.lua")
        .await
        .unwrap_or(0);
    let local_time = get_local_file_time(db_path);

    let time_diff = (local_time as i64 - remote_time as i64).abs();

    if time_diff > 5 {
        if remote_time > local_time {
            log_to_window(
                handle,
                "[SYNC] В облаке версия новее. Загрузка...".to_string(),
            );
            let content =
                github::download_from_github(&cfg.github_token, "ItemStorageDB.lua").await?;
            fs::write(db_path, content)?;
            log_to_window(
                handle,
                "[SUCCESS] Локальная база успешно обновлена.".to_string(),
            );
        } else if local_time > remote_time && is_admin_mode(&cfg.game_path) {
            log_to_window(
                handle,
                "[SYNC] Локальная база новее. Отправка...".to_string(),
            );
            let content = fs::read_to_string(db_path)?;
            let msg = format!(
                "Auto-update: {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M")
            );
            github::upload_to_github(&cfg.github_token, &content, "ItemStorageDB.lua", &msg)
                .await?;
            log_to_window(handle, "[SUCCESS] Облако успешно обновлено.".to_string());
        }
    }
    Ok(())
}

pub fn start_sync_loop(handle: tauri::AppHandle, state_notifier: Arc<Notify>) {
    tauri::async_runtime::spawn(async move {
        sleep(Duration::from_secs(2)).await;
        check_for_updates(handle.clone()).await;

        loop {
            let mut cfg: AppConfig = confy::load("item-storage-manager", None).unwrap_or_default();

            if !is_valid_wow_path(&cfg.game_path) {
                log_to_window(
                    &handle,
                    "[!] [NEED_SETUP] Путь к WoW не настроен.".to_string(),
                );
                let _ = handle.get_webview_window("main").map(|w| w.show());
                state_notifier.notified().await;
                continue;
            }

            if is_admin_mode(&cfg.game_path) && cfg.github_token.is_empty() {
                log_to_window(
                    &handle,
                    "[!] [NEED_SETUP_TOKEN] Режим админа требует GitHub Token.".to_string(),
                );
                let _ = handle.get_webview_window("main").map(|w| w.show());
                state_notifier.notified().await;
                continue;
            }

            let db_path = PathBuf::from(&cfg.game_path)
                .join(r"Interface\AddOns\ItemStorageBrowser\ItemStorageDB.lua");
            let _ = perform_sync(&handle, &mut cfg, &db_path, true).await;

            if is_admin_mode(&cfg.game_path) {
                log_to_window(
                    &handle,
                    "[INFO] Запущена АДМИНСКАЯ сессия (активный мониторинг).".to_string(),
                );
            } else {
                log_to_window(
                    &handle,
                    "[INFO] Запущена ПОЛЬЗОВАТЕЛЬСКАЯ сессия (только чтение).".to_string(),
                );
            }

            log_to_window(&handle, "[OK] Система запущена. Мониторинг...".to_string());

            if is_admin_mode(&cfg.game_path) {
                admin::run_admin_loop(handle.clone(), &db_path).await;
            } else {
                user::run_user_loop(handle.clone(), &db_path).await;
            }
        }
    });
}
