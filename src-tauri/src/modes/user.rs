use crate::config::AppConfig;
use crate::log_to_window; // Добавили импорт для логов
use crate::sync::perform_sync;
use std::path::Path;
use tauri::Runtime;
use tokio::time::{sleep, Duration};

pub async fn run_user_loop<R: Runtime>(handle: tauri::AppHandle<R>, db_path: &Path) {
    loop {
        // Ожидание перед следующей проверкой (15 минут)
        sleep(Duration::from_secs(900)).await;

        let mut cfg_loop: AppConfig = confy::load("item-storage-manager", None).unwrap_or_default();

        log_to_window(
            &handle,
            "[INFO] Плановая проверка обновлений базы...".to_string(),
        );

        // Выполняем синхронизацию
        match perform_sync(&handle, &mut cfg_loop, db_path, false).await {
            Ok(_) => {
                // Если файлы идентичны, perform_sync промолчит (так как is_initial = false),
                // поэтому добавим маленькое подтверждение здесь.
                log_to_window(&handle, "[OK] Проверка завершена.".to_string());
            }
            Err(e) => {
                log_to_window(
                    &handle,
                    format!("[ERROR] Ошибка при автоматической проверке: {}", e),
                );
            }
        }
    }
}
