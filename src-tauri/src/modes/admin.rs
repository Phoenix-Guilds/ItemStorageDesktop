use crate::config::AppConfig;
use crate::log_to_window;
use crate::scanner::run_admin_scan;
use crate::sync::perform_sync;
use std::path::Path;
use tauri::Runtime;
use tokio::time::{sleep, Duration};

pub async fn run_admin_loop<R: Runtime>(handle: tauri::AppHandle<R>, db_path: &Path) {
    let mut is_first_run = true;

    loop {
        let mut cfg_loop: AppConfig = confy::load("item-storage-manager", None).unwrap_or_default();

        if let Some(c) = run_admin_scan(&cfg_loop.game_path) {
            let has_changes = c.name_realm != cfg_loop.last_char_name
                || c.last_logout.to_string() != cfg_loop.last_char_logout;

            if has_changes {
                if !is_first_run {
                    log_to_window(&handle, format!("[EVENT] Активность: {}.", c.name_realm));
                }

                cfg_loop.last_char_name = c.name_realm;
                cfg_loop.last_char_logout = c.last_logout.to_string();
                let _ = confy::store("item-storage-manager", None, &cfg_loop);

                if !is_first_run {
                    // Пытаемся синхронизировать и выводим статус
                    match perform_sync(&handle, &mut cfg_loop, db_path, false).await {
                        Ok(_) => {
                            log_to_window(&handle, "[OK] Данные успешно обработаны.".to_string())
                        }
                        Err(e) => {
                            log_to_window(&handle, format!("[ERROR] Ошибка синхронизации: {}", e))
                        }
                    }
                }
            } else {
                // Обычная фоновая проверка
                let _ = perform_sync(&handle, &mut cfg_loop, db_path, false).await;
            }
        }

        is_first_run = false;
        sleep(Duration::from_secs(30)).await;
    }
}
