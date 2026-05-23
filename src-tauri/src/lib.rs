mod api;
mod keychain;
mod tray;

use api::UsageSnapshot;
use serde::Serialize;

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
            tray::setup(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(false) = event {
                if window.label() == "popover" {
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
