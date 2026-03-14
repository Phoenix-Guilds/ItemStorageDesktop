#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod github;
mod modes;
mod scanner;
mod sync; // Добавили эту строку

use crate::config::AppConfig;
use std::sync::Arc;
use tokio::sync::Notify;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, Runtime, State,
};

struct AppState {
    updated: Arc<Notify>,
}

#[tauri::command(rename_all = "camelCase")]
async fn update_settings(
    path: String,
    token: String,
    user_mode: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut cfg: AppConfig = confy::load("item-storage-manager", None).unwrap_or_default();

    // -------- проверка пути --------

    if !path.is_empty() {
        let cleaned = path.trim().replace('"', "");

        if !config::is_valid_wow_path(&cleaned) {
            return Err("INVALID_PATH".into());
        }

        cfg.game_path = cleaned;
    }

    // -------- пользовательский режим --------

    if user_mode {
        cfg.force_user_mode = true;
        cfg.github_token.clear();
    } else {
        // токен проверяем ТОЛЬКО если он введён

        if !token.trim().is_empty() {
            if !github::validate_token(&token).await {
                return Err("INVALID_TOKEN".into());
            }

            cfg.github_token = token.trim().to_string();
            cfg.force_user_mode = false;
        }
    }

    confy::store("item-storage-manager", None, &cfg).map_err(|e| e.to_string())?;

    state.updated.notify_one();

    Ok(())
}

pub fn log_to_window<R: Runtime>(handle: &tauri::AppHandle<R>, msg: String) {
    let _ = handle.emit("log-event", msg);
}

fn main() {
    tauri::Builder::default()
        .manage(AppState {
            updated: Arc::new(Notify::new()),
        })
        .invoke_handler(tauri::generate_handler![update_settings])
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle();
            let state = handle.state::<AppState>();
            let notifier = state.updated.clone();

            let quit_i = MenuItem::with_id(handle, "quit", "Выход", true, None::<&str>)?;
            let show_i = MenuItem::with_id(handle, "show", "Показать/Скрыть", true, None::<&str>)?;
            let menu = Menu::with_items(handle, &[&show_i, &quit_i])?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .on_menu_event(move |h, event| match event.id.as_ref() {
                    "quit" => std::process::exit(0),
                    "show" => {
                        if let Some(w) = h.get_webview_window("main") {
                            if w.is_visible().unwrap_or(false) {
                                let _ = w.hide();
                            } else {
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let h = tray.app_handle();
                        if let Some(w) = h.get_webview_window("main") {
                            if w.is_visible().unwrap_or(false) {
                                let _ = w.hide();
                            } else {
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            sync::start_sync_loop(handle.clone(), notifier);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
