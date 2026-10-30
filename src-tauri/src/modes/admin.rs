use crate::config::AppConfig;
use crate::log_to_window;
use crate::scanner::{self, run_admin_scan};
use crate::sync::{perform_admin_sync, perform_sync, CloudRetryStatus};
use chrono::{TimeZone, Utc};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::time::SystemTime;
use tauri::Runtime;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::task;
use tokio::time::{interval, sleep, Duration, MissedTickBehavior};

async fn spawn_saved_variables_watcher(
    game_path: String,
    sender: Sender<PathBuf>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    task::spawn_blocking(move || -> Result<(), Box<dyn Error + Send + Sync>> {
        let (raw_tx, raw_rx) = std_mpsc::channel();
        let mut watcher: RecommendedWatcher = RecommendedWatcher::new(
            move |res| {
                let _ = raw_tx.send(res);
            },
            Config::default(),
        )?;

        let accounts = ["HKFIRST01", "HKFIRST02", "HKFIRST03", "HKFIRST04"];
        for account in accounts {
            // Watch the whole account directory recursively so we catch per-realm/per-character SavedVariables
            let watch_path = Path::new(&game_path)
                .join(format!(r"WTF\Account\{}", account));
            if watch_path.exists() {
                watcher.watch(&watch_path, RecursiveMode::Recursive)?;
            }
        }

        loop {
            let res = match raw_rx.recv_timeout(Duration::from_secs(30)) {
                Ok(result) => result,
                Err(std_mpsc::RecvTimeoutError::Timeout) => return Ok(()),
                Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("watcher event channel closed".into());
                }
            };

            let event = match res {
                Ok(event) => event,
                Err(error) => return Err(Box::new(error)),
            };

            if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                for path in event.paths {
                    if path.file_name().and_then(|name| name.to_str())
                        == Some("ItemStorageBrowser.lua")
                    {
                        let _ = sender.blocking_send(path);
                    }
                }
            }
        }
    })
    .await??;

    Ok(())
}

pub async fn run_admin_loop<R: Runtime>(handle: tauri::AppHandle<R>, db_path: &Path) {
    let mut is_first_run = true;
    let mut pending_updates: HashMap<String, crate::scanner::CharacterExportData> = HashMap::new();
    let mut cloud_status = CloudRetryStatus::default();
    let (watch_tx, mut watch_rx): (Sender<PathBuf>, Receiver<PathBuf>) = channel(32);
    let mut heartbeat = interval(Duration::from_secs(15));
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let mut last_heartbeat_at = SystemTime::now();
    let mut watcher_started = false;
    let mut watcher_game_path = String::new();

    loop {
        let mut cfg_loop: AppConfig = confy::load("item-storage-manager", None).unwrap_or_default();

        if !watcher_started || watcher_game_path != cfg_loop.game_path {
            watcher_game_path = cfg_loop.game_path.clone();
            watcher_started = true;
            let watch_path = watcher_game_path.clone();
            let tx = watch_tx.clone();
            let handle_clone = handle.clone();

            tokio::spawn(async move {
                loop {
                    if let Err(e) = spawn_saved_variables_watcher(watch_path.clone(), tx.clone()).await {
                        log_to_window(&handle_clone, format!("[WARN] watcher restarting: {}", e));
                    }
                    sleep(Duration::from_secs(2)).await;
                }
            });
        }

        if cfg_loop.last_char_name.is_empty() || cfg_loop.last_char_logout.is_empty() {
            if let Some(c) = run_admin_scan(&cfg_loop.game_path) {
                cfg_loop.last_char_name = c.name_realm.clone();
                cfg_loop.last_char_logout = c.last_logout.to_string();
                let _ = confy::store("item-storage-manager", None, &cfg_loop);
            }
        }

        if !pending_updates.is_empty() {
            match perform_admin_sync(&handle, &mut cfg_loop, db_path, &mut pending_updates).await {
                Ok(_) => {
                    cloud_status.record_success(&handle);
                    log_to_window(&handle, "[OK] Данные успешно обработаны.".to_string())
                }
                Err(e) => {
                    cloud_status.record_failure(&handle, e.as_ref());
                }
            }
        }

        tokio::select! {
            biased;
            maybe_path = watch_rx.recv() => {
                if let Some(path) = maybe_path {
                    if !path.starts_with(Path::new(&cfg_loop.game_path)) {
                        continue;
                    }

                    if let Ok(export_data) = scanner::read_last_character_export_data_from_file_path(&path) {
                        if let Ok(metadata) = fs::metadata(&path) {
                            if let Ok(modified) = metadata.modified() {
                                if let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) {
                                    let last_logout = Utc
                                        .timestamp_opt(duration.as_secs() as i64, 0)
                                        .single()
                                        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap());

                                    let last_logout_normalized = last_logout.naive_utc().to_string();
                                    let previous_logout_normalized = cfg_loop
                                        .last_char_logout
                                        .trim_end_matches(" UTC");
                                    let has_changes = export_data.character != cfg_loop.last_char_name
                                        || last_logout_normalized != previous_logout_normalized;

                                    if has_changes {
                                        if !is_first_run {
                                            log_to_window(&handle, format!("[EVENT] Активность: {}", export_data.character));
                                        }

                                        cfg_loop.last_char_name = export_data.character.clone();
                                        cfg_loop.last_char_logout = last_logout.to_string();
                                        let _ = confy::store("item-storage-manager", None, &cfg_loop);

                                        pending_updates.insert(export_data.character.clone(), export_data);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ = heartbeat.tick() => {
                let now = SystemTime::now();
                if now.duration_since(last_heartbeat_at).unwrap_or_default() > Duration::from_secs(30) {
                    log_to_window(
                        &handle,
                        "[RESUME] Возобновление после паузы; перезапускаю проверку файлов и GitHub...".to_string(),
                    );
                }
                last_heartbeat_at = now;

                if let Some((_path, export_data)) = scanner::scan_latest_admin_export(&cfg_loop.game_path) {
                    let last_logout = export_data.last_updated.to_string();
                    let last_logout_normalized = export_data.last_updated.naive_utc().to_string();
                    let previous_logout_normalized = cfg_loop
                        .last_char_logout
                        .trim_end_matches(" UTC");
                    let has_changes = export_data.character != cfg_loop.last_char_name
                        || last_logout_normalized != previous_logout_normalized;

                    if has_changes {
                        if !is_first_run {
                            log_to_window(&handle, format!("[EVENT] Активность: {}", export_data.character));
                        }

                        cfg_loop.last_char_name = export_data.character.clone();
                        cfg_loop.last_char_logout = last_logout;
                        let _ = confy::store("item-storage-manager", None, &cfg_loop);
                        pending_updates.insert(export_data.character.clone(), export_data);
                    }

                }

                match perform_sync(&handle, &mut cfg_loop, db_path, false).await {
                    Ok(()) => {
                        cloud_status.record_success(&handle);
                    }
                    Err(e) => cloud_status.record_failure(&handle, e.as_ref()),
                }
            }
        }

        is_first_run = false;
    }
}
