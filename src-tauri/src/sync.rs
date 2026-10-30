use crate::config::{is_admin_mode, is_valid_wow_path, AppConfig};
use crate::github;
use crate::log_to_window;
use crate::modes::{admin, user}; // Импорт режимов
use crate::scanner::CharacterExportData;
use chrono::{Local, Utc};
use regex::Regex;
use semver::Version;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tauri::{Manager, Runtime};
use tokio::sync::Notify;
use tokio::time::sleep;

#[derive(Default)]
pub struct CloudRetryStatus {
    consecutive_failures: u8,
    warning_shown: bool,
}

impl CloudRetryStatus {
    pub fn record_failure<R: Runtime>(
        &mut self,
        handle: &tauri::AppHandle<R>,
        error: &dyn std::fmt::Display,
    ) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures >= 10 && !self.warning_shown {
            log_to_window(
                handle,
                format!(
                    "[WARN] GitHub недоступен после 10 попыток. Повторные проверки продолжатся автоматически. Последняя ошибка: {}",
                    error
                ),
            );
            self.warning_shown = true;
        }
    }

    pub fn record_success<R: Runtime>(&mut self, handle: &tauri::AppHandle<R>) -> bool {
        let recovered = self.warning_shown;
        if recovered {
            log_to_window(
                handle,
                "[SUCCESS] Связь с GitHub восстановлена. Синхронизация продолжается.".to_string(),
            );
        }
        self.consecutive_failures = 0;
        self.warning_shown = false;
        recovered
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

fn normalize_line_endings(content: &str) -> String {
    content.replace("\r\n", "\n")
}

fn apply_line_endings(content: &str, use_crlf: bool) -> String {
    if use_crlf {
        normalize_line_endings(content).replace("\n", "\r\n")
    } else {
        normalize_line_endings(content)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SyncLock {
    timestamp: u64,
    ttl: u64,
}

impl SyncLock {
    fn is_active(&self, now: u64) -> bool {
        self.timestamp + self.ttl > now
    }
}

async fn read_sync_lock(
    token: &str,
) -> Result<Option<SyncLock>, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(content) = github::download_raw_from_github(token, "sync.lock").await? {
        let lock: SyncLock = serde_json::from_str(&content)?;
        Ok(Some(lock))
    } else {
        Ok(None)
    }
}

async fn write_sync_lock(
    token: &str,
    lock: &SyncLock,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let content = serde_json::to_string(lock)?;
    github::upload_to_github(token, &content, "sync.lock", "Update sync lock").await
}

async fn acquire_sync_lock(token: &str, ttl_secs: u64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let now = Utc::now().timestamp() as u64;

    if let Some(existing) = read_sync_lock(token).await? {
        if existing.is_active(now) {
            return Err("sync.lock is active".into());
        }
    }

    let lock = SyncLock {
        timestamp: now,
        ttl: ttl_secs,
    };
    write_sync_lock(token, &lock).await
}

async fn release_sync_lock(token: &str, ttl_secs: u64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let now = Utc::now().timestamp() as u64;
    let lock = SyncLock {
        timestamp: now.saturating_sub(ttl_secs + 1),
        ttl: ttl_secs,
    };
    write_sync_lock(token, &lock).await
}

pub async fn perform_admin_sync<R: Runtime>(
    handle: &tauri::AppHandle<R>,
    cfg: &mut AppConfig,
    db_path: &Path,
    pending_updates: &mut HashMap<String, CharacterExportData>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if pending_updates.is_empty() {
        return Ok(());
    }

    log_to_window(handle, "[SYNC] Попытка взять блокировку sync.lock...".to_string());
    match acquire_sync_lock(&cfg.github_token, 120).await {
        Ok(_) => {
            log_to_window(handle, "[SYNC] Блокировка получена.".to_string());
        }
        Err(e) => {
            log_to_window(
                handle,
                format!(
                    "[SYNC] sync.lock активен, повторим через 10 секунд: {}",
                    e
                ),
            );
            return Err("sync.lock active".into());
        }
    }

    let sync_result = async {
        let local_uses_crlf = db_path
            .exists()
            && fs::read_to_string(db_path)
                .map(|content| content.contains("\r\n"))
                .unwrap_or(false);

        let remote_content =
            github::download_from_github(&cfg.github_token, "ItemStorageDB.lua").await?;
        let updated_content = merge_database_with_updates(
            &remote_content,
            pending_updates,
            local_uses_crlf,
        )?;
        let msg = format!(
            "Warehouse auto-sync: {}",
            Local::now().format("%Y-%m-%d %H:%M:%S")
        );

        github::upload_to_github(&cfg.github_token, &updated_content, "ItemStorageDB.lua", &msg)
            .await?;
        fs::write(db_path, &updated_content)?;
        pending_updates.clear();
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    }
    .await;

    let release_result = release_sync_lock(&cfg.github_token, 120).await;
    match sync_result {
        Ok(()) => {
            release_result?;
        }
        Err(err) => {
            let _ = release_result;
            return Err(err);
        }
    }

    log_to_window(
        handle,
        "[SUCCESS] Данные успешно отправлены и локальная база обновлена.".to_string(),
    );
    Ok(())
}

fn merge_database_with_updates(
    db_content: &str,
    updates: &HashMap<String, CharacterExportData>,
    use_crlf: bool,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mut lines: Vec<String> = db_content.lines().map(|s| s.to_string()).collect();
    let mut updated = false;
    let mut entry_ranges: Vec<(usize, usize, String)> = Vec::new();
    let mut depth = 0;
    let mut record_start: Option<usize> = None;
    let mut current_name: Option<String> = None;

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed == "ItemStorageDB = {" {
            depth = 1;
            continue;
        }
        if depth > 0 {
            depth += trimmed.matches('{').count() as i32;
            depth -= trimmed.matches('}').count() as i32;

            if record_start.is_none() && trimmed.starts_with('{') {
                record_start = Some(index);
                current_name = None;
            }

            if let Some(name) = current_name.as_ref() {
                if name.is_empty() {
                    current_name = None;
                }
            }

            if record_start.is_some() && current_name.is_none() {
                if let Some(name) = parse_record_name(trimmed) {
                    current_name = Some(name);
                }
            }

            if let Some(start) = record_start {
                if depth == 1 {
                    let end = index;
                    let name = current_name
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string());
                    entry_ranges.push((start, end, name));
                    record_start = None;
                    current_name = None;
                }
            }
        }
    }

    for (start, end, name) in entry_ranges.iter().rev() {
        if let Some(update) = updates.get(name) {
            let replacement = build_record(update);
            let existing_record = lines[*start..=*end].join("\n");
            if normalize_record_for_compare(&existing_record)
                != normalize_record_for_compare(&replacement)
            {
                lines.splice(*start..=*end, replacement.lines().map(|s| s.to_string()));
                updated = true;
            }
        }
    }

    let insert_at = lines
        .iter()
        .rposition(|line| line.trim() == "}")
        .unwrap_or(lines.len() - 1);

    for (name, update) in updates {
        if !entry_ranges.iter().any(|(_, _, existing)| existing == name) {
            let record_text = build_record(update);
            lines.insert(insert_at, record_text);
            updated = true;
        }
    }

    if updated {
        if let Some(idx) = lines
            .iter()
            .position(|line| line.trim().starts_with("ItemStorageDB_LastUpdate"))
        {
            lines[idx] = format!(
                "ItemStorageDB_LastUpdate = {}",
                Utc::now().timestamp()
            );
        }
    }

    let result = lines.join("\n");
    Ok(apply_line_endings(&result, use_crlf))
}

fn parse_record_name(line: &str) -> Option<String> {
    let re = Regex::new(r#"name\s*=\s*\"(.*?)\""#).unwrap();
    if let Some(cap) = re.captures(line) {
        return Some(cap[1].to_string());
    }
    None
}

fn normalize_record_for_compare(record: &str) -> String {
    record
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.starts_with("updated_at ="))
        .collect::<Vec<&str>>()
        .join("\n")
}

fn escape_lua_string(s: &str) -> String {
    s.replace("\\", "\\\\").replace("\"", "\\\"")
}

fn build_record(update: &CharacterExportData) -> String {
    let mut result = Vec::new();

    result.push("    {".to_string());
    result.push(format!("        name = \"{}\",", escape_lua_string(&update.character)));
    // include realm if present
    if let Some(realm) = &update.realm {
        result.push(format!("        realm = \"{}\",", escape_lua_string(realm)));
    }
    result.push(format!(
        "        updated_at = \"{}\",",
        Local::now().format("%Y-%m-%d %H:%M:%S")
    ));
    result.push(format!("        money = {},", update.money));
    result.push(format!("        empty = {},", update.free_total_slots));
    result.push(format!("        location = \"{}\",", escape_lua_string(&update.location)));

    // consolidated items for backwards compatibility and UI filters
    let mut consolidated_items: HashMap<String, (String, u64)> = HashMap::new();
    for item in update.bags.items.iter().chain(update.bank.items.iter()) {
        let entry = consolidated_items
            .entry(item.item_link.clone())
            .or_insert_with(|| (item.item_name.clone(), 0));
        entry.1 += item.item_count;
    }
    let mut consolidated_items: Vec<(&String, &(String, u64))> = consolidated_items.iter().collect();
    consolidated_items.sort_by(|a, b| a.0.cmp(b.0));

    result.push("        items = {".to_string());
    for (link, (name, count)) in consolidated_items {
        result.push("            {".to_string());
        result.push(format!("                link = \"{}\",", escape_lua_string(link)));
        result.push(format!("                name = \"{}\",", escape_lua_string(name)));
        result.push(format!("                count = {},", count));
        result.push("            },".to_string());
    }
    result.push("        },".to_string());

    // bank
    result.push("        bank = {".to_string());
    let bank_ts = update
        .bank
        .last_update
        .map(|dt| dt.timestamp() as u64)
        .unwrap_or(0);
    result.push(format!("            last_update = {},", bank_ts));
    result.push("            items = {".to_string());
    for item in &update.bank.items {
        result.push("                {".to_string());
        result.push(format!("                    link = \"{}\",", escape_lua_string(&item.item_link)));
        result.push(format!("                    name = \"{}\",", escape_lua_string(&item.item_name)));
        result.push(format!("                    count = {},", item.item_count));
        result.push("                },".to_string());
    }
    result.push("            },".to_string());
    result.push("        },".to_string());

    // bags
    result.push("        bags = {".to_string());
    let bags_ts = update
        .bags
        .last_update
        .map(|dt| dt.timestamp() as u64)
        .unwrap_or(0);
    result.push(format!("            last_update = {},", bags_ts));
    result.push("            items = {".to_string());
    for item in &update.bags.items {
        result.push("                {".to_string());
        result.push(format!("                    link = \"{}\",", escape_lua_string(&item.item_link)));
        result.push(format!("                    name = \"{}\",", escape_lua_string(&item.item_name)));
        result.push(format!("                    count = {},", item.item_count));
        result.push("                },".to_string());
    }
    result.push("            },".to_string());
    result.push("        },".to_string());

    result.push("    },".to_string());

    result.join("\n")
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
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if is_initial {
        log_to_window(handle, "[SYNC] Сверка базы данных с облаком...".to_string());
    }

    let mut local_uses_crlf = false;
    let mut local_normalized: Option<String> = None;
    let mut remote_normalized: Option<String> = None;

    if db_path.exists() {
        if let Ok(local_content) = fs::read_to_string(db_path) {
            local_uses_crlf = local_content.contains("\r\n");
            local_normalized = Some(normalize_line_endings(&local_content));

            if let Some(remote_content) =
                github::download_raw_from_github(&cfg.github_token, "ItemStorageDB.lua").await?
            {
                let remote_norm = normalize_line_endings(&remote_content);
                remote_normalized = Some(remote_norm.clone());

                if local_normalized.as_ref() == Some(&remote_norm) {
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
        let mut content = github::download_from_github(&cfg.github_token, "ItemStorageDB.lua").await?;
        if local_uses_crlf {
            content = apply_line_endings(&content, true);
        }
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
            let mut content =
                github::download_from_github(&cfg.github_token, "ItemStorageDB.lua").await?;
            let remote_norm = normalize_line_endings(&content);
            if let Some(local_norm) = local_normalized.as_ref() {
                if local_norm == &remote_norm {
                    log_to_window(handle, "[OK] Локальная и облачная базы идентичны. Обновление не требуется.".to_string());
                    return Ok(());
                }
            }
            if local_uses_crlf {
                content = apply_line_endings(&content, true);
            }
            fs::write(db_path, content)?;
            log_to_window(
                handle,
                "[SUCCESS] Локальная база успешно обновлена.".to_string(),
            );
        } else if local_time > remote_time && is_admin_mode(&cfg.game_path) && !cfg.force_user_mode
        {
            let content = fs::read_to_string(db_path)?;
            let local_norm = normalize_line_endings(&content);
            let mut same_content = false;

            if let Some(remote_norm) = remote_normalized.as_ref() {
                if remote_norm == &local_norm {
                    same_content = true;
                }
            } else if let Some(remote_content) =
                github::download_raw_from_github(&cfg.github_token, "ItemStorageDB.lua").await?
            {
                let remote_norm = normalize_line_endings(&remote_content);
                if remote_norm == local_norm {
                    same_content = true;
                }
            }

            if same_content {
                log_to_window(handle, "[OK] Локальная и облачная базы идентичны. Обновление не требуется.".to_string());
            } else {
                log_to_window(
                    handle,
                    "[SYNC] Локальная база новее. Отправка...".to_string(),
                );
                let msg = format!(
                    "Auto-update: {}",
                    chrono::Local::now().format("%Y-%m-%d %H:%M")
                );
                github::upload_to_github(&cfg.github_token, &content, "ItemStorageDB.lua", &msg)
                    .await?;
                log_to_window(handle, "[SUCCESS] Облако успешно обновлено.".to_string());
            }
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

            if is_admin_mode(&cfg.game_path) && !cfg.force_user_mode && cfg.github_token.is_empty()
            {
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

            if is_admin_mode(&cfg.game_path) && !cfg.force_user_mode {
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

            if !cfg.github_token.is_empty() {
                if !github::validate_token(&cfg.github_token).await {
                    log_to_window(
                        &handle,
                        "[!] GitHub Token больше не действителен.".to_string(),
                    );

                    cfg.github_token.clear();
                    let _ = confy::store("item-storage-manager", None, &cfg);

                    let _ = handle.get_webview_window("main").map(|w| w.show());
                    state_notifier.notified().await;
                    continue;
                }
            }

            log_to_window(&handle, "[OK] Система запущена. Мониторинг...".to_string());

            if is_admin_mode(&cfg.game_path) && !cfg.force_user_mode {
                admin::run_admin_loop(handle.clone(), &db_path).await;
            } else {
                user::run_user_loop(handle.clone(), &db_path).await;
            }
        }
    });
}
