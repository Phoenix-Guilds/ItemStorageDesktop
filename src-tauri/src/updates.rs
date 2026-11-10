use crate::config::AppConfig;
use crate::github;
use crate::log_to_window;
use semver::Version;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};
use tokio::time::sleep;

#[derive(Default)]
struct NotifiedVersions {
    application: Option<Version>,
    addon: Option<Version>,
}

pub fn start_version_monitor(handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        sleep(Duration::from_secs(3)).await;

        let mut notified = NotifiedVersions::default();
        check_for_new_versions(&handle, &mut notified).await;
        let mut last_check_at = SystemTime::now();
        let mut last_poll_at = last_check_at;

        loop {
            sleep(Duration::from_secs(15)).await;
            let now = SystemTime::now();
            let time_since_poll = now.duration_since(last_poll_at).unwrap_or_default();
            let time_since_check = now.duration_since(last_check_at).unwrap_or_default();
            last_poll_at = now;

            let resumed_after_sleep = time_since_poll > Duration::from_secs(30);
            let hourly_check_due = time_since_check >= Duration::from_secs(60 * 60);
            if resumed_after_sleep || hourly_check_due {
                check_for_new_versions(&handle, &mut notified).await;
                last_check_at = SystemTime::now();
                last_poll_at = last_check_at;
            }
        }
    });
}

async fn check_for_new_versions(handle: &tauri::AppHandle, notified: &mut NotifiedVersions) {
    check_application_version(handle, &mut notified.application).await;
    check_addon_version(handle, &mut notified.addon).await;
}

async fn check_application_version(handle: &tauri::AppHandle, last_notified: &mut Option<Version>) {
    let current = match Version::parse(env!("CARGO_PKG_VERSION")) {
        Ok(version) => version,
        Err(_) => return,
    };

    let Ok((tag, release_url)) = github::get_latest_release_version().await else {
        return;
    };
    let Some(latest) = parse_release_version(&tag) else {
        return;
    };

    if latest > current && last_notified.as_ref() != Some(&latest) {
        log_to_window(
            handle,
            format!(
                "[UPDATE_LINK] Для приложения доступна новая версия {}|{}",
                tag, release_url
            ),
        );
        *last_notified = Some(latest);
    }
}

async fn check_addon_version(handle: &tauri::AppHandle, last_notified: &mut Option<Version>) {
    let config: AppConfig = confy::load("item-storage-manager", None).unwrap_or_default();
    let toc_path = Path::new(&config.game_path)
        .join("Interface")
        .join("AddOns")
        .join("ItemStorageBrowser")
        .join("ItemStorageBrowser.toc");
    let Ok(toc_contents) = fs::read_to_string(toc_path) else {
        return;
    };
    let Some(current) = parse_toc_version(&toc_contents) else {
        return;
    };

    let Ok((tag, release_url)) = github::get_latest_addon_release_version().await else {
        return;
    };
    let Some(latest) = parse_release_version(&tag) else {
        return;
    };

    if latest > current && last_notified.as_ref() != Some(&latest) {
        log_to_window(
            handle,
            format!(
                "[UPDATE_LINK] Для аддона ItemStorageBrowser доступна новая версия {}|{}",
                tag, release_url
            ),
        );
        *last_notified = Some(latest);
    }
}

fn parse_toc_version(contents: &str) -> Option<Version> {
    contents.lines().find_map(|line| {
        line.trim()
            .strip_prefix("## Version:")
            .and_then(parse_release_version)
    })
}

fn parse_release_version(tag: &str) -> Option<Version> {
    Version::parse(tag.trim().trim_start_matches('v')).ok()
}
