mod api;
mod keychain;
#[cfg(target_os = "macos")]
mod macos_panel;
mod tray;

use api::UsageSnapshot;
use serde::Serialize;
use std::sync::atomic::{AtomicI64, Ordering};
use tauri::Manager;

/// popover が show() された最後の時刻 (epoch ms)。表示直後の
/// Focused(false) によるオートクローズを抑制するための grace 用。
pub static SHOWN_AT_MS: AtomicI64 = AtomicI64::new(0);
const FOCUS_LOSS_GRACE_MS: i64 = 300;

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FetchResult {
    Ok(UsageSnapshot),
    RateLimited { retry_after_secs: Option<u64> },
    Err { message: String },
}

#[tauri::command]
async fn get_usage() -> FetchResult {
    fetch_usage_inner().await
}

pub async fn fetch_usage_inner() -> FetchResult {
    let token = match keychain::read_access_token() {
        Ok(t) => t,
        Err(e) => return FetchResult::Err { message: e.to_string() },
    };
    match api::fetch_usage(&token).await {
        Ok(snapshot) => FetchResult::Ok(snapshot),
        Err(api::ApiError::RateLimited { retry_after_secs }) => {
            FetchResult::RateLimited { retry_after_secs }
        }
        Err(e) => FetchResult::Err { message: e.to_string() },
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_positioner::init())
        .invoke_handler(tauri::generate_handler![get_usage])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
            if let Some(popover) = app.get_webview_window("popover") {
                let _ = popover.set_visible_on_all_workspaces(true);
                #[cfg(target_os = "macos")]
                {
                    // 起動時に NSWindow → NSPanel に class 書き換え + NonactivatingPanel
                    macos_panel::convert_to_nspanel(&popover);
                    macos_panel::promote_to_floating_panel(&popover);
                    // アプリ外クリックを監視して popover を hide するモニタを登録
                    macos_panel::install_outside_click_dismiss(app.handle().clone());
                }
            }
            tray::setup(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(false) = event {
                if window.label() == "popover" {
                    // show 直後の数百ms はフルスクリーン下での race を避けるため無視
                    let shown_at = SHOWN_AT_MS.load(Ordering::SeqCst);
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    if now - shown_at < FOCUS_LOSS_GRACE_MS {
                        return;
                    }
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
