mod api;
mod codex;
mod keychain;
#[cfg(target_os = "macos")]
mod macos_panel;
mod tray;

use api::UsageSnapshot;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};
use tauri::Manager;
use tokio::sync::Notify;

/// popover が show() された最後の時刻 (epoch ms)。表示直後の
/// Focused(false) によるオートクローズを抑制するための grace 用。
pub static SHOWN_AT_MS: AtomicI64 = AtomicI64::new(0);
const FOCUS_LOSS_GRACE_MS: i64 = 300;
const RESIZE_AUTO_HIDE_SUPPRESSION_MS: i64 = 4_000;

static POPOVER_PINNED: AtomicBool = AtomicBool::new(false);
static POPOVER_AUTO_HIDE_SUPPRESSED_UNTIL_MS: AtomicI64 = AtomicI64::new(0);

/// 現在のプロバイダ。`Provider::as_u8` で AtomicU8 に格納。
static CURRENT_PROVIDER: AtomicU8 = AtomicU8::new(Provider::CLAUDE_U8);

/// ポーラの待ち秒数 (ユーザ設定)。`tray.rs` のループが各イテレーションでこの値を読む。
pub const MIN_POLL_INTERVAL_SECS: u64 = 60;
pub const MAX_POLL_INTERVAL_SECS: u64 = 3_600;
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 300;
static POLL_INTERVAL_SECS: AtomicU64 = AtomicU64::new(DEFAULT_POLL_INTERVAL_SECS);

/// トレイに表示するメトリクス。
static TRAY_METRIC: AtomicU8 = AtomicU8::new(0); // FiveHour

/// poll 間隔変更 / プロバイダ変更時にポーラを起こすための notifier。
static POLL_WAKE: OnceLock<Arc<Notify>> = OnceLock::new();

fn poll_wake_internal() -> &'static Arc<Notify> {
    POLL_WAKE.get_or_init(|| Arc::new(Notify::new()))
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrayMetric {
    FiveHour,
    Weekly,
}

impl TrayMetric {
    fn as_u8(self) -> u8 {
        match self {
            TrayMetric::FiveHour => 0,
            TrayMetric::Weekly => 1,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => TrayMetric::Weekly,
            _ => TrayMetric::FiveHour,
        }
    }
}

impl Provider {
    const CLAUDE_U8: u8 = 0;
    const CODEX_U8: u8 = 1;

    fn as_u8(self) -> u8 {
        match self {
            Provider::Claude => Self::CLAUDE_U8,
            Provider::Codex => Self::CODEX_U8,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            Self::CODEX_U8 => Provider::Codex,
            _ => Provider::Claude,
        }
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FetchResult {
    Ok {
        provider: Provider,
        snapshot: UsageSnapshot,
    },
    RateLimited {
        provider: Provider,
        retry_after_secs: Option<u64>,
    },
    Err {
        provider: Provider,
        message: String,
    },
}

impl FetchResult {
    pub fn provider(&self) -> Provider {
        match self {
            FetchResult::Ok { provider, .. }
            | FetchResult::RateLimited { provider, .. }
            | FetchResult::Err { provider, .. } => *provider,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Settings {
    /// トレイのタイトルに表示するプロバイダ。ポップオーバーは常に両方を表示する。
    pub provider: Provider,
    pub poll_interval_secs: u64,
    pub tray_metric: TrayMetric,
}

/// 全プロバイダの取得結果をまとめたもの。ポップオーバーはこの構造をそのまま受け取って描画する。
#[derive(Serialize, Clone, Debug)]
pub struct AllUsage {
    pub claude: FetchResult,
    pub codex: FetchResult,
}

#[tauri::command]
async fn get_usage() -> AllUsage {
    fetch_all_usage().await
}

/// 即時リフレッシュ要求。ポーラを叩き起こしてフェッチをトリガし、
/// 通常経路 (tray 更新 + cache 書き換え + usage-updated emit) で全体に反映する。
#[tauri::command]
fn refresh_now() {
    poll_wake_internal().notify_one();
}

pub async fn fetch_all_usage() -> AllUsage {
    let (claude, codex) = tokio::join!(
        fetch_usage_inner(Provider::Claude),
        fetch_usage_inner(Provider::Codex)
    );
    AllUsage { claude, codex }
}

#[tauri::command]
fn get_popover_pinned() -> bool {
    is_popover_pinned()
}

#[tauri::command]
fn set_popover_pinned(pinned: bool) -> bool {
    POPOVER_PINNED.store(pinned, Ordering::SeqCst);
    pinned
}

#[tauri::command]
fn suppress_popover_auto_hide() {
    suppress_popover_auto_hide_for(RESIZE_AUTO_HIDE_SUPPRESSION_MS);
}

#[tauri::command]
fn get_settings() -> Settings {
    Settings {
        provider: current_provider(),
        poll_interval_secs: POLL_INTERVAL_SECS.load(Ordering::SeqCst),
        tray_metric: current_tray_metric(),
    }
}

#[tauri::command]
fn set_tray_metric(metric: TrayMetric) -> Settings {
    TRAY_METRIC.store(metric.as_u8(), Ordering::SeqCst);
    poll_wake_internal().notify_one();
    get_settings()
}

#[tauri::command]
fn set_provider(provider: Provider) -> Settings {
    CURRENT_PROVIDER.store(provider.as_u8(), Ordering::SeqCst);
    poll_wake_internal().notify_one();
    get_settings()
}

#[tauri::command]
fn set_poll_interval(secs: u64) -> Settings {
    let clamped = secs.clamp(MIN_POLL_INTERVAL_SECS, MAX_POLL_INTERVAL_SECS);
    POLL_INTERVAL_SECS.store(clamped, Ordering::SeqCst);
    poll_wake_internal().notify_one();
    get_settings()
}

pub fn is_popover_pinned() -> bool {
    POPOVER_PINNED.load(Ordering::SeqCst)
}

pub fn is_popover_auto_hide_suppressed() -> bool {
    now_ms() < POPOVER_AUTO_HIDE_SUPPRESSED_UNTIL_MS.load(Ordering::SeqCst)
}

pub fn current_provider() -> Provider {
    Provider::from_u8(CURRENT_PROVIDER.load(Ordering::SeqCst))
}

pub fn current_poll_interval_secs() -> u64 {
    POLL_INTERVAL_SECS.load(Ordering::SeqCst)
}

pub fn current_tray_metric() -> TrayMetric {
    TrayMetric::from_u8(TRAY_METRIC.load(Ordering::SeqCst))
}

pub fn poll_wake() -> Arc<Notify> {
    poll_wake_internal().clone()
}

fn suppress_popover_auto_hide_for(duration_ms: i64) {
    POPOVER_AUTO_HIDE_SUPPRESSED_UNTIL_MS.store(now_ms() + duration_ms, Ordering::SeqCst);
}

fn focus_loss_should_be_ignored(
    pinned: bool,
    shown_at: i64,
    suppressed_until: i64,
    now: i64,
) -> bool {
    pinned || now < suppressed_until || now - shown_at < FOCUS_LOSS_GRACE_MS
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub async fn fetch_usage_inner(provider: Provider) -> FetchResult {
    match provider {
        Provider::Claude => fetch_claude().await,
        Provider::Codex => fetch_codex().await,
    }
}

async fn fetch_claude() -> FetchResult {
    let token = match keychain::read_access_token() {
        Ok(t) => t,
        Err(e) => {
            return FetchResult::Err {
                provider: Provider::Claude,
                message: e.to_string(),
            }
        }
    };
    match api::fetch_usage(&token).await {
        Ok(snapshot) => FetchResult::Ok {
            provider: Provider::Claude,
            snapshot,
        },
        Err(api::ApiError::RateLimited { retry_after_secs }) => FetchResult::RateLimited {
            provider: Provider::Claude,
            retry_after_secs,
        },
        Err(e) => FetchResult::Err {
            provider: Provider::Claude,
            message: e.to_string(),
        },
    }
}

async fn fetch_codex() -> FetchResult {
    let creds = match codex::read_credentials() {
        Ok(c) => c,
        Err(e) => {
            return FetchResult::Err {
                provider: Provider::Codex,
                message: e.to_string(),
            }
        }
    };
    match codex::fetch_usage(&creds).await {
        Ok(snapshot) => FetchResult::Ok {
            provider: Provider::Codex,
            snapshot,
        },
        Err(api::ApiError::RateLimited { retry_after_secs }) => FetchResult::RateLimited {
            provider: Provider::Codex,
            retry_after_secs,
        },
        Err(e) => FetchResult::Err {
            provider: Provider::Codex,
            message: e.to_string(),
        },
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_positioner::init())
        .invoke_handler(tauri::generate_handler![
            get_usage,
            refresh_now,
            get_popover_pinned,
            set_popover_pinned,
            suppress_popover_auto_hide,
            get_settings,
            set_provider,
            set_poll_interval,
            set_tray_metric,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
            if let Some(popover) = app.get_webview_window("popover") {
                let _ = popover.set_visible_on_all_workspaces(true);
                let _ = popover.set_shadow(false);
                #[cfg(target_os = "macos")]
                {
                    // 起動時に NSWindow → NSPanel に class 書き換え + NonactivatingPanel
                    macos_panel::convert_to_nspanel(&popover);
                    let _ = popover.set_shadow(false);
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
                    let shown_at = SHOWN_AT_MS.load(Ordering::SeqCst);
                    let suppressed_until =
                        POPOVER_AUTO_HIDE_SUPPRESSED_UNTIL_MS.load(Ordering::SeqCst);
                    let now = now_ms();
                    if focus_loss_should_be_ignored(
                        is_popover_pinned(),
                        shown_at,
                        suppressed_until,
                        now,
                    ) {
                        return;
                    }
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_loss_is_ignored_while_pinned() {
        assert!(focus_loss_should_be_ignored(true, 0, 0, 1_000));
    }

    #[test]
    fn focus_loss_is_ignored_during_show_grace() {
        assert!(focus_loss_should_be_ignored(false, 1_000, 0, 1_100));
    }

    #[test]
    fn focus_loss_is_ignored_during_resize_suppression() {
        assert!(focus_loss_should_be_ignored(false, 0, 2_000, 1_000));
    }

    #[test]
    fn focus_loss_is_not_ignored_after_grace_and_suppression() {
        assert!(!focus_loss_should_be_ignored(false, 1_000, 1_500, 2_000));
    }

    #[test]
    fn provider_roundtrips_via_u8() {
        assert_eq!(Provider::from_u8(Provider::Claude.as_u8()), Provider::Claude);
        assert_eq!(Provider::from_u8(Provider::Codex.as_u8()), Provider::Codex);
        // unknown values fall back to Claude
        assert_eq!(Provider::from_u8(42), Provider::Claude);
    }
}
