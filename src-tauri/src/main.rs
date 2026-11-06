#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod github;
mod modes;
mod scanner;
mod sync;

use crate::config::{AppConfig, WindowGeometry};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tokio::sync::Notify;

use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, Runtime, State,
};

struct AppState {
    updated: Arc<Notify>,
    unread: AtomicBool,
    tray_icon: Mutex<Option<TrayIcon>>,
    icons: Mutex<Option<(Image<'static>, Image<'static>)>>,
    taskbar_overlay: Image<'static>,
    window_geometry: Arc<Mutex<WindowGeometry>>,
    save_window_geometry: Sender<()>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsView {
    game_path: String,
    github_token: String,
    force_user_mode: bool,
    autostart_enabled: bool,
    close_to_tray: bool,
}

#[tauri::command(rename_all = "camelCase")]
async fn update_settings(
    path: String,
    token: String,
    user_mode: bool,
    close_to_tray: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut cfg: AppConfig = confy::load("item-storage-manager", None).unwrap_or_default();
    cfg.close_to_tray = close_to_tray;

    if !path.is_empty() {
        let cleaned = path.trim().replace('"', "");

        if !config::is_valid_wow_path(&cleaned) {
            return Err("INVALID_PATH".into());
        }

        cfg.game_path = cleaned;
    }

    if user_mode {
        cfg.force_user_mode = true;
        cfg.github_token.clear();
    } else {
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

#[tauri::command]
fn get_autostart_preference() -> bool {
    confy::load::<AppConfig>("item-storage-manager", None)
        .unwrap_or_default()
        .autostart_enabled
}

#[tauri::command(rename_all = "camelCase")]
fn get_settings() -> SettingsView {
    let cfg: AppConfig = confy::load("item-storage-manager", None).unwrap_or_default();
    SettingsView {
        game_path: cfg.game_path,
        github_token: cfg.github_token,
        force_user_mode: cfg.force_user_mode,
        autostart_enabled: cfg.autostart_enabled,
        close_to_tray: cfg.close_to_tray,
    }
}

#[tauri::command]
fn set_autostart_preference(enabled: bool) -> Result<(), String> {
    let mut cfg: AppConfig = confy::load("item-storage-manager", None).unwrap_or_default();
    cfg.autostart_enabled = enabled;
    confy::store("item-storage-manager", None, &cfg).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
fn log_chat_event(handle: tauri::AppHandle, message: String) {
    log_to_window(&handle, format!("[CHAT] {}", message));
}

#[tauri::command(rename_all = "camelCase")]
fn set_chat_unread(
    unread: bool,
    at_latest: bool,
    window: tauri::WebviewWindow,
    handle: tauri::AppHandle,
) {
    let actively_reading =
        at_latest && window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false);
    if unread == actively_reading {
        return;
    }

    let state = handle.state::<AppState>();
    if state.unread.swap(unread, Ordering::Relaxed) == unread {
        return;
    }

    let icon = state
        .icons
        .lock()
        .unwrap()
        .as_ref()
        .map(|(normal, badged)| {
            if unread {
                badged.clone()
            } else {
                normal.clone()
            }
        });
    let tray = state.tray_icon.lock().unwrap().clone();
    if let (Some(tray), Some(icon)) = (tray, icon) {
        let _ = tray.set_icon(Some(icon));
    }

    #[cfg(target_os = "windows")]
    if let Some(window) = handle.get_webview_window("main") {
        let overlay = if unread {
            Some(state.taskbar_overlay.clone())
        } else {
            None
        };
        let _ = window.set_overlay_icon(overlay);
    }
}

#[tauri::command]
fn reset_window_geometry(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), String> {
    window
        .set_size(tauri::PhysicalSize::new(800, 600))
        .map_err(|error| error.to_string())?;
    window.center().map_err(|error| error.to_string())?;

    let position = window.outer_position().map_err(|error| error.to_string())?;
    let size = window.inner_size().map_err(|error| error.to_string())?;
    let geometry = WindowGeometry {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    };

    let mut config: AppConfig = confy::load("item-storage-manager", None).unwrap_or_default();
    config.window_geometry = Some(geometry);
    confy::store("item-storage-manager", None, &config).map_err(|error| error.to_string())?;
    *state
        .window_geometry
        .lock()
        .map_err(|error| error.to_string())? = geometry;
    let _ = state.save_window_geometry.send(());
    Ok(())
}

fn window_geometry_is_visible(
    window: &tauri::WebviewWindow,
    geometry: WindowGeometry,
) -> bool {
    let Ok(monitors) = window.available_monitors() else {
        return true;
    };

    monitors.iter().any(|monitor| {
        let position = monitor.position();
        let size = monitor.size();
        let left = i64::from(geometry.x);
        let top = i64::from(geometry.y);
        let right = left + i64::from(geometry.width);
        let bottom = top + i64::from(geometry.height);
        let monitor_left = i64::from(position.x);
        let monitor_top = i64::from(position.y);
        let monitor_right = monitor_left + i64::from(size.width);
        let monitor_bottom = monitor_top + i64::from(size.height);

        right > monitor_left
            && left < monitor_right
            && bottom > monitor_top
            && top < monitor_bottom
    })
}

fn start_window_geometry_saver(
    receiver: Receiver<()>,
    geometry: Arc<Mutex<WindowGeometry>>,
    handle: tauri::AppHandle,
) {
    thread::spawn(move || {
        while receiver.recv().is_ok() {
            while receiver.recv_timeout(Duration::from_millis(300)).is_ok() {}

            let Ok(geometry) = geometry.lock().map(|geometry| *geometry) else {
                continue;
            };
            let mut config: AppConfig = confy::load("item-storage-manager", None).unwrap_or_default();
            config.window_geometry = Some(geometry);
            if let Err(error) = confy::store("item-storage-manager", None, &config) {
                log_to_window(&handle, format!("[WARN] Не удалось сохранить размеры окна: {error}"));
            }
        }
    });
}

fn with_unread_dot(image: &Image<'_>) -> Image<'static> {
    let width = image.width();
    let height = image.height();
    let mut rgba = image.rgba().to_vec();
    let radius = (width.min(height) as f32 / 6.0).clamp(2.0, 14.0);
    let center_x = width as f32 - radius - 1.0;
    let center_y = radius + 1.0;
    let outer_radius = radius + 1.5;

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - center_x;
            let dy = y as f32 - center_y;
            let distance = (dx * dx + dy * dy).sqrt();
            if distance <= outer_radius {
                let offset = ((y * width + x) * 4) as usize;
                let color = if distance <= radius {
                    [220, 38, 38, 255]
                } else {
                    [255, 255, 255, 255]
                };
                rgba[offset..offset + 4].copy_from_slice(&color);
            }
        }
    }

    Image::new_owned(rgba, width, height)
}

fn taskbar_overlay_icon() -> Image<'static> {
    let blank = Image::new_owned(vec![0; 16 * 16 * 4], 16, 16);
    with_unread_dot(&blank)
}

pub fn log_to_window<R: Runtime>(handle: &tauri::AppHandle<R>, msg: String) {
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let _ = handle.emit("log-event", format!("[{}] {}", timestamp, msg));
}

fn main() {
    let window_geometry = Arc::new(Mutex::new(WindowGeometry::default()));
    let (save_window_geometry, window_geometry_events) = mpsc::channel();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .manage(AppState {
            updated: Arc::new(Notify::new()),
            unread: AtomicBool::new(false),
            tray_icon: Mutex::new(None),
            icons: Mutex::new(None),
            taskbar_overlay: taskbar_overlay_icon(),
            window_geometry: window_geometry.clone(),
            save_window_geometry: save_window_geometry.clone(),
        })
        .invoke_handler(tauri::generate_handler![
            update_settings,
            get_settings,
            get_autostart_preference,
            set_autostart_preference,
            get_app_version,
            log_chat_event,
            set_chat_unread,
            reset_window_geometry
        ])
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            let handle = app.handle();
            if let Some(window) = handle.get_webview_window("main") {
                window.set_title(&format!(
                    "ItemStorageManager {}",
                    env!("CARGO_PKG_VERSION")
                ))?;

                let config: AppConfig = confy::load("item-storage-manager", None).unwrap_or_default();
                if let Some(geometry) = config.window_geometry {
                    if geometry.width >= 400 && geometry.height >= 300 {
                        window.set_size(tauri::PhysicalSize::new(geometry.width, geometry.height))?;
                        if window_geometry_is_visible(&window, geometry) {
                            window.set_position(tauri::PhysicalPosition::new(geometry.x, geometry.y))?;
                        } else {
                            window.center()?;
                        }
                    }
                }

                if let (Ok(position), Ok(size)) = (window.outer_position(), window.inner_size()) {
                    *window_geometry.lock().unwrap() = WindowGeometry {
                        x: position.x,
                        y: position.y,
                        width: size.width,
                        height: size.height,
                    };
                }

                start_window_geometry_saver(
                    window_geometry_events,
                    window_geometry.clone(),
                    handle.clone(),
                );

                let app_handle = handle.clone();
                let event_window = window.clone();
                let geometry = window_geometry.clone();
                let save_geometry = save_window_geometry.clone();
                window.on_window_event(move |event| {
                    match event {
                        tauri::WindowEvent::CloseRequested { api, .. } => {
                            let cfg: AppConfig =
                                confy::load("item-storage-manager", None).unwrap_or_default();
                            if cfg.close_to_tray {
                                api.prevent_close();
                                if let Some(window) = app_handle.get_webview_window("main") {
                                    let _ = window.hide();
                                }
                            }
                        }
                        tauri::WindowEvent::Moved(position)
                            if !event_window.is_maximized().unwrap_or(false)
                                && !event_window.is_minimized().unwrap_or(false) =>
                        {
                            if let Ok(mut geometry) = geometry.lock() {
                                geometry.x = position.x;
                                geometry.y = position.y;
                                let _ = save_geometry.send(());
                            }
                        }
                        tauri::WindowEvent::Resized(size)
                            if !event_window.is_maximized().unwrap_or(false)
                                && !event_window.is_minimized().unwrap_or(false) =>
                        {
                            if let Ok(mut geometry) = geometry.lock() {
                                geometry.width = size.width;
                                geometry.height = size.height;
                                let _ = save_geometry.send(());
                            }
                        }
                        _ => {}
                    }
                });
            }
            let state = handle.state::<AppState>();
            let notifier = state.updated.clone();

            let quit_i = MenuItem::with_id(handle, "quit", "Выход", true, None::<&str>)?;
            let show_i = MenuItem::with_id(handle, "show", "Показать/Скрыть", true, None::<&str>)?;
            let settings_i =
                MenuItem::with_id(handle, "settings", "Настройки", true, None::<&str>)?;

            let menu = Menu::with_items(handle, &[&show_i, &settings_i, &quit_i])?;

            let default_icon = app.default_window_icon().unwrap();
            let normal_icon = Image::new_owned(
                default_icon.rgba().to_vec(),
                default_icon.width(),
                default_icon.height(),
            );
            let badged_icon = with_unread_dot(&normal_icon);

            let tray = TrayIconBuilder::new()
                .icon(normal_icon.clone())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |h, event| match event.id.as_ref() {
                    "quit" => std::process::exit(0),

                    "settings" => {
                        if let Some(window) = h.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                            let _ = h.emit("open-settings", ());
                        }
                    }

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

            *state.tray_icon.lock().unwrap() = Some(tray);
            *state.icons.lock().unwrap() = Some((normal_icon, badged_icon));

            sync::start_sync_loop(handle.clone(), notifier);

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
