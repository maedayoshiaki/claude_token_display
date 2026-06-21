mod api;
mod claude_desktop;
mod codex;
mod keychain;
#[cfg(target_os = "macos")]
mod macos_panel;
mod tray;
mod update;

use api::UsageSnapshot;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};
#[cfg(not(target_os = "windows"))]
use tauri::LogicalSize;
use tauri::{LogicalUnit, Manager, PixelUnit, WindowSizeConstraints};
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
pub(crate) const POPOVER_MIN_WIDTH: f64 = 1.0;
const POPOVER_MIN_HEIGHT: f64 = 40.0;
pub(crate) const POPOVER_DEFAULT_WIDTH: f64 = 340.0;
pub(crate) const POPOVER_WIDTH_STEP: f64 = 24.0;
static POLL_INTERVAL_SECS: AtomicU64 = AtomicU64::new(DEFAULT_POLL_INTERVAL_SECS);

/// 更新チェック間隔 (秒)。usage ポーリングとは別系統。短すぎると GitHub の
/// 未認証レート制限 (IP あたり 60 req/h) に触れるので最小 1 時間。
pub const MIN_UPDATE_CHECK_INTERVAL_SECS: u64 = 3_600;
pub const MAX_UPDATE_CHECK_INTERVAL_SECS: u64 = 7 * 24 * 3_600;
pub const DEFAULT_UPDATE_CHECK_INTERVAL_SECS: u64 = 6 * 3_600;
static UPDATE_CHECK_INTERVAL_SECS: AtomicU64 =
    AtomicU64::new(DEFAULT_UPDATE_CHECK_INTERVAL_SECS);

/// 更新チェック間隔の変更時にチェッカーを起こす notifier。
static UPDATE_WAKE: OnceLock<Arc<Notify>> = OnceLock::new();

fn update_wake_internal() -> &'static Arc<Notify> {
    UPDATE_WAKE.get_or_init(|| Arc::new(Notify::new()))
}

pub fn update_wake() -> Arc<Notify> {
    update_wake_internal().clone()
}

pub fn current_update_check_interval_secs() -> u64 {
    UPDATE_CHECK_INTERVAL_SECS.load(Ordering::SeqCst)
}

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
    pub update_check_interval_secs: u64,
}

/// 全プロバイダの取得結果をまとめたもの。ポップオーバーはこの構造をそのまま受け取って描画する。
#[derive(Serialize, Clone, Debug)]
pub struct AllUsage {
    pub claude: FetchResult,
    pub codex: FetchResult,
}

#[derive(Serialize, Clone, Debug)]
pub struct PopoverSizeReport {
    pub requested_width: f64,
    pub inner_width: u32,
    pub outer_width: u32,
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
fn set_popover_width(
    window: tauri::WebviewWindow,
    width: f64,
) -> Result<PopoverSizeReport, String> {
    resize_popover_width(&window, width)
}

pub(crate) fn resize_popover_width<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
    width: f64,
) -> Result<PopoverSizeReport, String> {
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let current = window.inner_size().map_err(|e| e.to_string())?;
    let logical = current.to_logical::<f64>(scale);
    let width = width.clamp(POPOVER_MIN_WIDTH, 640.0);
    set_popover_width_inner(&window, width, logical.height)?;
    let inner = window.inner_size().map_err(|e| e.to_string())?;
    let outer = window.outer_size().map_err(|e| e.to_string())?;
    Ok(PopoverSizeReport {
        requested_width: width,
        inner_width: inner.width,
        outer_width: outer.width,
    })
}

#[cfg(not(target_os = "windows"))]
fn set_popover_width_inner<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
    width: f64,
    height: f64,
) -> Result<(), String> {
    window
        .set_size(LogicalSize::new(width, height))
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "windows")]
fn set_popover_width_inner<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
    width: f64,
    height: f64,
) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
    };

    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let hwnd = window.hwnd().map_err(|e| e.to_string())?;
    let hwnd = hwnd.0 as HWND;
    let width = (width * scale).round().max(1.0) as i32;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };

    let ok = unsafe { GetWindowRect(hwnd, &mut rect) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }

    let fallback_height = (height * scale).round().max(1.0) as i32;
    let current_height = (rect.bottom - rect.top).max(fallback_height).max(1);
    let ok = unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            width,
            current_height,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

fn popover_size_constraints() -> WindowSizeConstraints {
    WindowSizeConstraints {
        min_width: Some(PixelUnit::Logical(LogicalUnit::new(POPOVER_MIN_WIDTH))),
        min_height: Some(PixelUnit::Logical(LogicalUnit::new(POPOVER_MIN_HEIGHT))),
        max_width: None,
        max_height: None,
    }
}

#[tauri::command]
fn get_settings() -> Settings {
    Settings {
        provider: current_provider(),
        poll_interval_secs: POLL_INTERVAL_SECS.load(Ordering::SeqCst),
        tray_metric: current_tray_metric(),
        update_check_interval_secs: current_update_check_interval_secs(),
    }
}

#[tauri::command]
fn set_update_check_interval(secs: u64) -> Settings {
    let clamped = secs.clamp(
        MIN_UPDATE_CHECK_INTERVAL_SECS,
        MAX_UPDATE_CHECK_INTERVAL_SECS,
    );
    UPDATE_CHECK_INTERVAL_SECS.store(clamped, Ordering::SeqCst);
    update_wake_internal().notify_one();
    get_settings()
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

/// Claude のトークンを取得する。Claude Code CLI の保存先を優先し、無ければ
/// Claude Desktop の保存先にフォールバックする (どちらも同種の subscription
/// OAuth トークンで、使用枠は共有プールなので同じ数字が得られる)。
fn read_claude_token() -> Result<String, String> {
    let cli_err = match keychain::read_access_token() {
        Ok(token) => return Ok(token),
        Err(e) => e,
    };
    match claude_desktop::read_access_token() {
        Ok(token) => Ok(token),
        Err(desktop_err) => Err(combine_claude_token_errors(&cli_err, &desktop_err)),
    }
}

/// クレデンシャルファイル (Claude Code の `.credentials.json` / Claude Desktop の
/// `config.json`) は各アプリがトークン更新時に書き換える。その「削除→再作成」や
/// 部分書き込みの一瞬に読みに行くと NotFound / パース失敗になりうるので、それらは
/// transient とみなして短い間隔で数回だけリトライする。恒久的な失敗 (未ログイン等)
/// は `is_transient` が false を返すので即座に返る。
pub(crate) const CREDENTIAL_READ_RETRIES: usize = 3;
pub(crate) const CREDENTIAL_READ_RETRY_DELAY_MS: u64 = 50;

pub(crate) fn read_with_retry<T, E>(
    mut read: impl FnMut() -> Result<T, E>,
    is_transient: impl Fn(&E) -> bool,
) -> Result<T, E> {
    let mut attempt = 0;
    loop {
        match read() {
            Ok(value) => return Ok(value),
            Err(e) if attempt < CREDENTIAL_READ_RETRIES && is_transient(&e) => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(
                    CREDENTIAL_READ_RETRY_DELAY_MS,
                ));
            }
            Err(e) => return Err(e),
        }
    }
}

/// 両方の取得元が失敗したときのメッセージ。どちらも「未ログイン」なら 1 行で案内し、
/// それ以外 (アクセス拒否・復号失敗等) は診断用に両方を出す。
fn combine_claude_token_errors(
    cli: &keychain::KeychainError,
    desktop: &claude_desktop::DesktopError,
) -> String {
    let cli_missing = matches!(cli, keychain::KeychainError::NotFound);
    let desktop_missing = matches!(desktop, claude_desktop::DesktopError::NotFound);
    if cli_missing && desktop_missing {
        return "No Claude login found. Log in via the `claude` CLI or Claude Desktop.".to_string();
    }
    format!("Claude Code: {cli} / Claude Desktop: {desktop}")
}

async fn fetch_claude() -> FetchResult {
    let token = match read_claude_token() {
        Ok(t) => t,
        Err(message) => {
            return FetchResult::Err {
                provider: Provider::Claude,
                message,
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
            set_popover_width,
            get_settings,
            set_provider,
            set_poll_interval,
            set_tray_metric,
            set_update_check_interval,
            update::get_update_info,
            update::open_release_page,
            update::check_update_now,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
            if let Some(popover) = app.get_webview_window("popover") {
                let _ = popover.set_size_constraints(popover_size_constraints());
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
            // アプリ自身の更新チェック (起動少し後 + 6時間ごと)。
            update::spawn_checker(app.handle().clone());
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
    fn retry_succeeds_after_transient_failures() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let result: Result<u8, u8> = read_with_retry(
            || {
                let n = calls.get();
                calls.set(n + 1);
                if n < 2 {
                    Err(1)
                } else {
                    Ok(7)
                }
            },
            |_| true,
        );
        assert_eq!(result, Ok(7));
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn retry_returns_immediately_on_non_transient() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let result: Result<u8, u8> = read_with_retry(
            || {
                calls.set(calls.get() + 1);
                Err(9)
            },
            |_| false,
        );
        assert_eq!(result, Err(9));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn retry_gives_up_after_max_attempts() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let result: Result<u8, u8> = read_with_retry(
            || {
                calls.set(calls.get() + 1);
                Err(5)
            },
            |_| true,
        );
        assert_eq!(result, Err(5));
        assert_eq!(calls.get(), CREDENTIAL_READ_RETRIES + 1);
    }

    #[test]
    fn both_sources_missing_gives_single_line_hint() {
        let msg = combine_claude_token_errors(
            &keychain::KeychainError::NotFound,
            &claude_desktop::DesktopError::NotFound,
        );
        assert!(msg.contains("claude") || msg.contains("Claude"));
        assert!(!msg.contains("Claude Code:"), "should be the friendly hint, not the diagnostic form");
    }

    #[test]
    fn non_missing_error_surfaces_both_sources() {
        let msg = combine_claude_token_errors(
            &keychain::KeychainError::Decode("bad json".into()),
            &claude_desktop::DesktopError::NotFound,
        );
        assert!(msg.contains("Claude Code:"));
        assert!(msg.contains("Claude Desktop:"));
    }

    #[test]
    fn provider_roundtrips_via_u8() {
        assert_eq!(Provider::from_u8(Provider::Claude.as_u8()), Provider::Claude);
        assert_eq!(Provider::from_u8(Provider::Codex.as_u8()), Provider::Codex);
        // unknown values fall back to Claude
        assert_eq!(Provider::from_u8(42), Provider::Claude);
    }
}
