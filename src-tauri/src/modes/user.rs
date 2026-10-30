use crate::config::AppConfig;
use crate::log_to_window; // Добавили импорт для логов
use crate::sync::{perform_sync, CloudRetryStatus};
use std::path::Path;
use std::time::{Duration as StdDuration, SystemTime};
use tauri::Runtime;
use tokio::time::{interval, sleep, Duration, MissedTickBehavior};

pub async fn run_user_loop<R: Runtime>(handle: tauri::AppHandle<R>, db_path: &Path) {
    let mut heartbeat = interval(Duration::from_secs(15));
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let mut last_sync_at = SystemTime::now();
    let mut last_heartbeat_at = last_sync_at;
    let mut cloud_status = CloudRetryStatus::default();

    loop {
        heartbeat.tick().await;

        let now = SystemTime::now();
        let time_since_heartbeat = now
            .duration_since(last_heartbeat_at)
            .unwrap_or_default();
        last_heartbeat_at = now;
        let resumed_after_sleep = time_since_heartbeat > StdDuration::from_secs(30);

        if resumed_after_sleep {
            log_to_window(
                &handle,
                "[RESUME] Возобновление после паузы; проверяю GitHub и локальную базу..."
                    .to_string(),
            );
        }

        if !resumed_after_sleep
            && now
                .duration_since(last_sync_at)
                .unwrap_or_default()
                < StdDuration::from_secs(900)
        {
            continue;
        }

        let mut retrying = false;
        loop {
            let mut cfg_loop: AppConfig =
                confy::load("item-storage-manager", None).unwrap_or_default();

            if !retrying {
                log_to_window(
                    &handle,
                    "[INFO] Плановая проверка обновлений базы...".to_string(),
                );
            }

            match perform_sync(&handle, &mut cfg_loop, db_path, false).await {
                Ok(_) => {
                    let recovered = cloud_status.record_success(&handle);
                    if !recovered {
                        log_to_window(&handle, "[OK] Проверка завершена.".to_string());
                    }
                    last_sync_at = SystemTime::now();
                    break;
                }
                Err(e) => {
                    cloud_status.record_failure(&handle, e.as_ref());
                    retrying = true;
                    sleep(Duration::from_secs(60)).await;
                }
            }
        }
    }
}
