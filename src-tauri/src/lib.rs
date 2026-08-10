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

/// トレイ (メニューバー / タスクトレイ) に各プロバイダを表示するか。ポップオーバーの
/// 表示トグルとは独立した設定。既定は両方表示。フロント (localStorage) が真の値を持ち、
/// 起動時に popover 側から set_tray_providers で反映される。
static TRAY_SHOW_CLAUDE: AtomicBool = AtomicBool::new(true);
static TRAY_SHOW_CODEX: AtomicBool = AtomicBool::new(true);

/// ポーラの待ち秒数 (ユーザ設定)。`tray.rs` のループが各イテレーションでこの値を読む。
pub const MIN_POLL_INTERVAL_SECS: u64 = 60;
pub const MAX_POLL_INTERVAL_SECS: u64 = 3_600;
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 300;
pub(crate) const POPOVER_MIN_WIDTH: f64 = 1.0;
const POPOVER_MIN_HEIGHT: f64 = 40.0;
pub(crate) const POPOVER_DEFAULT_WIDTH: f64 = 340.0;
pub(crate) const POPOVER_DEFAULT_HEIGHT: f64 = 420.0;
pub(crate) const POPOVER_WIDTH_STEP: f64 = 24.0;
static POLL_INTERVAL_SECS: AtomicU64 = AtomicU64::new(DEFAULT_POLL_INTERVAL_SECS);

// Temporary test switch: skip reading Claude Code credentials and force the
// Claude Desktop credential path. Set this back to false after Desktop testing.
const DISABLE_CLAUDE_CODE_TOKEN_READ_FOR_DESKTOP_TEST: bool = false;

/// CLI トークンを「期限切れ」とみなすときの前倒し余裕 (ms)。フェッチ往復の最中に
/// 失効する寸前のトークンを掴んで 401 を食らうのを避けるため、この分だけ早めに
/// 期限切れ扱いにする。
const CLI_TOKEN_EXPIRY_MARGIN_MS: i64 = 60_000;

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

/// 次回 wake で実 API フェッチを行うかどうか。手動リフレッシュ (refresh/reload) だけが
/// true にする。表示・間隔だけの変更 (tray_metric / provider / poll_interval) は false の
/// まま起こし、ポーラはキャッシュ再描画と sleep 再計算だけ行い **API を叩かない**。
/// これがないと設定をいじるたびに usage エンドポイントへ無駄打ちして 429 を招く。
static POLL_FORCE_FETCH: AtomicBool = AtomicBool::new(false);

fn poll_wake_internal() -> &'static Arc<Notify> {
    POLL_WAKE.get_or_init(|| Arc::new(Notify::new()))
}

/// 手動リフレッシュ用: 次回 wake で実フェッチを要求してポーラを起こす。
pub fn request_poll_fetch() {
    POLL_FORCE_FETCH.store(true, Ordering::SeqCst);
    poll_wake_internal().notify_one();
}

/// 表示・間隔だけの変更用: フェッチせずにポーラを起こし、キャッシュ再描画と
/// sleep 再計算だけさせる。
pub fn wake_poller_no_fetch() {
    poll_wake_internal().notify_one();
}

/// ポーラが wake 時に呼ぶ: フェッチ要求フラグを取り出してクリアする。
pub fn take_poll_fetch_request() -> bool {
    POLL_FORCE_FETCH.swap(false, Ordering::SeqCst)
}

/// 「レート制限を無視して即時更新」フラグ。true のとき、ポーラは最小フェッチ間隔
/// (MIN_MANUAL_FETCH_SPACING_MS) や 429/403 のバックオフ待ちを飛ばして即座に取得する。
static POLL_FORCE_IMMEDIATE: AtomicBool = AtomicBool::new(false);

/// レート制限を無視した即時更新を要求する (フェッチ要求も立ててポーラを起こす)。
pub fn request_force_immediate_fetch() {
    POLL_FORCE_IMMEDIATE.store(true, Ordering::SeqCst);
    POLL_FORCE_FETCH.store(true, Ordering::SeqCst);
    poll_wake_internal().notify_one();
}

/// ポーラが wake 時に呼ぶ: 即時更新フラグを取り出してクリアする。
pub fn take_poll_force_immediate() -> bool {
    POLL_FORCE_IMMEDIATE.swap(false, Ordering::SeqCst)
}

/// usage API への実アクセス数の記録。ユーザーが「どれくらい叩いたか」を把握できるよう
/// 設定画面に注意書きとして表示する。total は起動以降の累計、log は直近 1 時間の
/// タイムスタンプ (ms)、last は最後のアクセス時刻 (ms, 0=未取得)。
static API_ACCESS_TOTAL: AtomicU64 = AtomicU64::new(0);
static API_LAST_ACCESS_MS: AtomicI64 = AtomicI64::new(0);
static API_ACCESS_LOG: OnceLock<std::sync::Mutex<Vec<i64>>> = OnceLock::new();
const ONE_HOUR_MS: i64 = 3_600_000;

/// usage API (Claude / Codex) へ実際に HTTP リクエストを送るたびに api.rs / codex.rs から呼ぶ。
pub fn record_api_access() {
    API_ACCESS_TOTAL.fetch_add(1, Ordering::SeqCst);
    let now = now_ms();
    API_LAST_ACCESS_MS.store(now, Ordering::SeqCst);
    let log = API_ACCESS_LOG.get_or_init(|| std::sync::Mutex::new(Vec::new()));
    if let Ok(mut v) = log.lock() {
        v.push(now);
        let cutoff = now - ONE_HOUR_MS;
        v.retain(|&t| t >= cutoff);
    }
}

fn access_stats() -> AccessStats {
    let now = now_ms();
    let cutoff = now - ONE_HOUR_MS;
    let last_hour = API_ACCESS_LOG
        .get()
        .and_then(|m| m.lock().ok().map(|v| v.iter().filter(|&&t| t >= cutoff).count()))
        .unwrap_or(0);
    AccessStats {
        total: API_ACCESS_TOTAL.load(Ordering::SeqCst),
        last_hour: last_hour as u64,
        last_access_ms: API_LAST_ACCESS_MS.load(Ordering::SeqCst),
        poll_interval_secs: current_poll_interval_secs(),
    }
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
    /// 403。トークンは有効だがこの資格情報では usage API が許可されていない (恒久ブロック)。
    CredentialRestricted {
        provider: Provider,
        message: String,
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
            | FetchResult::CredentialRestricted { provider, .. }
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
    /// トレイに各プロバイダを表示するか (ポップオーバーの表示トグルとは独立)。
    pub tray_show_claude: bool,
    pub tray_show_codex: bool,
}

/// usage API への実アクセス状況。設定画面に「どれくらい叩いたか」の注意書きとして出す。
#[derive(Serialize, Clone, Debug)]
pub struct AccessStats {
    /// 起動以降の累計アクセス数 (Claude / Codex の HTTP リクエストを個別に数える)。
    pub total: u64,
    /// 直近 1 時間のアクセス数。
    pub last_hour: u64,
    /// 最後にアクセスした時刻 (epoch ms)。0 = まだ取得していない。
    pub last_access_ms: i64,
    /// 現在の自動取得間隔 (秒)。目安表示用。
    pub poll_interval_secs: u64,
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

#[derive(Serialize, Clone, Debug)]
pub struct CredentialEntry {
    pub source: String,
    pub available: bool,
    pub organization_uuid: Option<String>,
    pub subscription_type: Option<String>,
    pub rate_limit_tier: Option<String>,
    pub account_label: Option<String>,
    pub error: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct CredentialInfo {
    pub claude_code: CredentialEntry,
    pub claude_desktop: CredentialEntry,
    pub codex: CredentialEntry,
}

#[tauri::command]
async fn get_usage() -> AllUsage {
    fetch_all_usage().await
}

/// 即時リフレッシュ要求。ポーラを叩き起こしてフェッチをトリガし、
/// 通常経路 (tray 更新 + cache 書き換え + usage-updated emit) で全体に反映する。
#[tauri::command]
fn refresh_now() {
    request_poll_fetch();
}

/// 現在表示中の usage キャッシュを捨ててから即時再取得する。
#[tauri::command]
fn reload_now(app: tauri::AppHandle) {
    tray::clear_cached_usage_and_reload(&app);
}

/// レート制限保護 (最小フェッチ間隔・429/403 バックオフ待ち) を無視して即時取得する。
/// 設定画面の「レート制限を無視して更新」ボタン用。多用すると 429 を招くので注意。
#[tauri::command]
fn force_reload_now(app: tauri::AppHandle) {
    // 先に即時フラグを立ててから wake する (起こされたポーラが確実に見えるように)。
    request_force_immediate_fetch();
    tray::clear_cached_usage_and_reload(&app);
}

/// usage API のアクセス状況 (累計 / 直近1時間 / 最終取得) を返す。
#[tauri::command]
fn get_access_stats() -> AccessStats {
    access_stats()
}

#[tauri::command]
fn get_credential_info() -> CredentialInfo {
    credential_info()
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

/// popover の幅を変更する。別ウィンドウ (settings) からも呼べるよう、呼び出し元の
/// window ではなく常に "popover" ラベルのウィンドウを対象にする。
#[tauri::command]
fn set_popover_width(app: tauri::AppHandle, width: f64) -> Result<PopoverSizeReport, String> {
    let window = app
        .get_webview_window("popover")
        .ok_or_else(|| "popover window not found".to_string())?;
    resize_popover_width(&window, width)
}

/// popover を既定サイズ (幅・高さとも) へ戻す。設定ウィンドウの「サイズをリセット」から呼ぶ。
/// 常に "popover" ラベルのウィンドウを対象にする。
#[tauri::command]
fn reset_popover_size(app: tauri::AppHandle) -> Result<PopoverSizeReport, String> {
    let window = app
        .get_webview_window("popover")
        .ok_or_else(|| "popover window not found".to_string())?;
    window
        .set_size(tauri::LogicalSize::new(
            POPOVER_DEFAULT_WIDTH,
            POPOVER_DEFAULT_HEIGHT,
        ))
        .map_err(|e| e.to_string())?;
    let inner = window.inner_size().map_err(|e| e.to_string())?;
    let outer = window.outer_size().map_err(|e| e.to_string())?;
    Ok(PopoverSizeReport {
        requested_width: POPOVER_DEFAULT_WIDTH,
        inner_width: inner.width,
        outer_width: outer.width,
    })
}

/// 設定ウィンドウ (label="settings") を表示してフォーカスする。
/// ウィンドウは静的定義 (visible:false) で常駐し、閉じる操作では破棄せず hide する。
#[tauri::command]
fn open_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("settings")
        .ok_or_else(|| "settings window not found".to_string())?;
    let _ = win.unminimize();
    win.show().map_err(|e| e.to_string())?;
    win.set_focus().map_err(|e| e.to_string())?;
    Ok(())
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
        tray_show_claude: tray_shows_claude(),
        tray_show_codex: tray_shows_codex(),
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
    // 表示メトリクスの切替はキャッシュ済みスナップショットに全バケットが入っているので
    // 再フェッチ不要。ポーラを起こしてトレイを再描画させるだけ。
    wake_poller_no_fetch();
    get_settings()
}

#[tauri::command]
fn set_provider(provider: Provider) -> Settings {
    CURRENT_PROVIDER.store(provider.as_u8(), Ordering::SeqCst);
    // トレイは両プロバイダをキャッシュから描画するので再フェッチ不要。再描画のみ。
    wake_poller_no_fetch();
    get_settings()
}

#[tauri::command]
fn set_tray_providers(claude: bool, codex: bool) -> Settings {
    TRAY_SHOW_CLAUDE.store(claude, Ordering::SeqCst);
    TRAY_SHOW_CODEX.store(codex, Ordering::SeqCst);
    // 表示切替はキャッシュ済みスナップショットから再描画するだけ。API は叩かない。
    wake_poller_no_fetch();
    get_settings()
}

#[tauri::command]
fn set_poll_interval(secs: u64) -> Settings {
    let clamped = secs.clamp(MIN_POLL_INTERVAL_SECS, MAX_POLL_INTERVAL_SECS);
    POLL_INTERVAL_SECS.store(clamped, Ordering::SeqCst);
    // 間隔変更は次の sleep 計算にだけ効けばよい。フェッチせずに起こして sleep を再計算させる。
    wake_poller_no_fetch();
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

pub fn tray_shows_claude() -> bool {
    TRAY_SHOW_CLAUDE.load(Ordering::SeqCst)
}

pub fn tray_shows_codex() -> bool {
    TRAY_SHOW_CODEX.load(Ordering::SeqCst)
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

fn should_try_desktop_after_cli_api_error(e: &api::ApiError) -> bool {
    matches!(
        e,
        api::ApiError::Unauthorized | api::ApiError::CredentialRestricted { .. }
    )
}

fn claude_api_error_to_result(e: api::ApiError) -> FetchResult {
    match e {
        api::ApiError::RateLimited { retry_after_secs } => FetchResult::RateLimited {
            provider: Provider::Claude,
            retry_after_secs,
        },
        api::ApiError::CredentialRestricted { message } => FetchResult::CredentialRestricted {
            provider: Provider::Claude,
            message,
        },
        e => FetchResult::Err {
            provider: Provider::Claude,
            message: e.to_string(),
        },
    }
}

fn short_id(raw: &str) -> String {
    let s = raw.trim();
    if s.len() <= 12 {
        return s.to_string();
    }
    let tail: String = s
        .chars()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("...{tail}")
}

fn claude_account_label(
    organization_uuid: Option<&str>,
    subscription_type: Option<&str>,
    rate_limit_tier: Option<&str>,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(subscription) = subscription_type.filter(|s| !s.is_empty()) {
        parts.push(subscription.to_string());
    }
    if let Some(tier) = rate_limit_tier.filter(|s| !s.is_empty()) {
        if !parts.iter().any(|p| p == tier) {
            parts.push(tier.to_string());
        }
    }
    if let Some(org) = organization_uuid.filter(|s| !s.is_empty()) {
        parts.push(format!("org {}", short_id(org)));
    }
    (!parts.is_empty()).then(|| parts.join(" / "))
}

fn cli_account_label(c: &keychain::ClaudeCodeCredential) -> Option<String> {
    claude_account_label(
        c.organization_uuid.as_deref(),
        c.subscription_type.as_deref(),
        c.rate_limit_tier.as_deref(),
    )
}

fn desktop_account_label(c: &claude_desktop::DesktopCredential) -> Option<String> {
    claude_account_label(
        c.organization_uuid.as_deref(),
        c.subscription_type.as_deref(),
        c.rate_limit_tier.as_deref(),
    )
}

fn credential_info() -> CredentialInfo {
    let claude_code = match keychain::read_credentials() {
        Ok(c) => CredentialEntry {
            source: "Claude Code".to_string(),
            available: true,
            organization_uuid: c.organization_uuid.clone(),
            subscription_type: c.subscription_type.clone(),
            rate_limit_tier: c.rate_limit_tier.clone(),
            account_label: cli_account_label(&c),
            error: None,
        },
        Err(e) => CredentialEntry {
            source: "Claude Code".to_string(),
            available: false,
            organization_uuid: None,
            subscription_type: None,
            rate_limit_tier: None,
            account_label: None,
            error: Some(e.to_string()),
        },
    };

    let claude_desktop = match claude_desktop::read_credentials() {
        Ok(c) => CredentialEntry {
            source: "Claude Desktop".to_string(),
            available: true,
            organization_uuid: c.organization_uuid.clone(),
            subscription_type: c.subscription_type.clone(),
            rate_limit_tier: c.rate_limit_tier.clone(),
            account_label: desktop_account_label(&c),
            error: None,
        },
        Err(e) => CredentialEntry {
            source: "Claude Desktop".to_string(),
            available: false,
            organization_uuid: None,
            subscription_type: None,
            rate_limit_tier: None,
            account_label: None,
            error: Some(e.to_string()),
        },
    };

    let codex = match codex::read_credentials() {
        Ok(c) => CredentialEntry {
            source: "Codex CLI".to_string(),
            available: true,
            organization_uuid: None,
            subscription_type: None,
            rate_limit_tier: None,
            account_label: c.account_id.as_deref().map(short_id),
            error: None,
        },
        Err(e) => CredentialEntry {
            source: "Codex CLI".to_string(),
            available: false,
            organization_uuid: None,
            subscription_type: None,
            rate_limit_tier: None,
            account_label: None,
            error: Some(e.to_string()),
        },
    };

    CredentialInfo {
        claude_code,
        claude_desktop,
        codex,
    }
}

async fn fetch_claude_with_token(token: &str) -> FetchResult {
    match api::fetch_usage(token).await {
        Ok(snapshot) => FetchResult::Ok {
            provider: Provider::Claude,
            snapshot,
        },
        Err(e) => claude_api_error_to_result(e),
    }
}

async fn fetch_claude() -> FetchResult {
    if DISABLE_CLAUDE_CODE_TOKEN_READ_FOR_DESKTOP_TEST {
        return match claude_desktop::read_credentials() {
            Ok(credential) => fetch_claude_with_token(&credential.access_token).await,
            Err(e) => FetchResult::Err {
                provider: Provider::Claude,
                message: format!("Claude Code credential read is disabled for Desktop test / Claude Desktop: {e}"),
            },
        };
    }

    let cli_credential = match keychain::read_credentials() {
        Ok(credential) => Some(credential),
        Err(cli_err) => match claude_desktop::read_credentials() {
            Ok(credential) => return fetch_claude_with_token(&credential.access_token).await,
            Err(desktop_err) => {
                let message = combine_claude_token_errors(&cli_err, &desktop_err);
                return FetchResult::Err {
                    provider: Provider::Claude,
                    message,
                };
            }
        },
    };

    let cli_credential = cli_credential.expect("Some after successful keychain read");

    // Claude Code CLI を起動していない間 (= Claude Desktop のみ起動) は
    // `.credentials.json` のトークンが更新されず、期限切れのまま残る。その死んだトークンで
    // usage API を叩くと 401 (無効トークンの連続アクセスで 429 にもなりうる) が返るだけで、
    // 429 は Desktop フォールバックの対象外なので「使用量が取れない」状態に陥る。
    // ローカルで期限切れと分かるなら API を無駄打ちせず、先に Desktop トークンを使う。
    if cli_credential.is_expired_at(now_ms(), CLI_TOKEN_EXPIRY_MARGIN_MS) {
        if let Ok(desktop_credential) = claude_desktop::read_credentials() {
            if desktop_credential.access_token != cli_credential.access_token {
                return fetch_claude_with_token(&desktop_credential.access_token).await;
            }
        }
        // Desktop が使えない / CLI と同一トークンなら、期限切れでも CLI を試す (最後の手段)。
    }

    match api::fetch_usage(&cli_credential.access_token).await {
        Ok(snapshot) => FetchResult::Ok {
            provider: Provider::Claude,
            snapshot,
        },
        Err(cli_api_err) if should_try_desktop_after_cli_api_error(&cli_api_err) => {
            let cli_message = cli_api_err.to_string();
            match claude_desktop::read_credentials() {
                Ok(desktop_credential)
                    if desktop_credential.access_token != cli_credential.access_token =>
                {
                    fetch_claude_with_token(&desktop_credential.access_token).await
                }
                Ok(_) => claude_api_error_to_result(cli_api_err),
                Err(desktop_err) => FetchResult::Err {
                    provider: Provider::Claude,
                    message: format!(
                        "Claude Code token was rejected: {cli_message} / Claude Desktop: {desktop_err}"
                    ),
                },
            }
        }
        Err(e) => claude_api_error_to_result(e),
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
        // 多重起動防止は最初に登録する。2 つ目のインスタンスが起動すると、この
        // コールバックが既存インスタンス側で走り、2 つ目のプロセスは即終了する。
        // 常駐トレイアプリを二重起動するとポーラも二重になり usage API を無駄打ちして
        // 429 を招くため、それを防ぐ。既存の設定ウィンドウがあれば前面に出す。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(win) = app.get_webview_window("settings") {
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
        // OSログイン時の自動起動。設定画面のトグルから登録 / 解除する。
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_positioner::init())
        // アプリ自身の自動更新。app.updater() を Rust から使うために登録する
        // (実際の check/DL/install は update::install_update コマンドで駆動)。
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            get_usage,
            refresh_now,
            reload_now,
            force_reload_now,
            get_access_stats,
            get_credential_info,
            get_popover_pinned,
            set_popover_pinned,
            suppress_popover_auto_hide,
            set_popover_width,
            reset_popover_size,
            open_settings_window,
            get_settings,
            set_provider,
            set_tray_providers,
            set_poll_interval,
            set_tray_metric,
            set_update_check_interval,
            update::get_update_info,
            update::open_release_page,
            update::check_update_now,
            update::install_update,
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
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::Focused(false) => {
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
            // 設定ウィンドウは静的定義で常駐させたいので、閉じる操作では破棄せず hide する。
            tauri::WindowEvent::CloseRequested { api, .. } => {
                if window.label() == "settings" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            _ => {}
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
    fn force_fetch_flag_is_one_shot() {
        // 手動リフレッシュ経路だけが true にし、ポーラが 1 回取り出したらクリアされる。
        // これが崩れると設定変更 (no-fetch) でも実フェッチが走り 429 を招く。
        assert!(!take_poll_fetch_request(), "初期状態はフェッチ要求なし");
        request_poll_fetch();
        assert!(take_poll_fetch_request(), "request 後は一度だけ true");
        assert!(!take_poll_fetch_request(), "取り出したらクリアされる");
    }

    #[test]
    fn provider_roundtrips_via_u8() {
        assert_eq!(Provider::from_u8(Provider::Claude.as_u8()), Provider::Claude);
        assert_eq!(Provider::from_u8(Provider::Codex.as_u8()), Provider::Codex);
        // unknown values fall back to Claude
        assert_eq!(Provider::from_u8(42), Provider::Claude);
    }

    #[test]
    fn desktop_retry_is_limited_to_cli_auth_failures() {
        assert!(should_try_desktop_after_cli_api_error(
            &api::ApiError::Unauthorized
        ));
        assert!(should_try_desktop_after_cli_api_error(
            &api::ApiError::CredentialRestricted {
                message: "blocked".into(),
            }
        ));
        assert!(!should_try_desktop_after_cli_api_error(
            &api::ApiError::RateLimited {
                retry_after_secs: Some(60),
            }
        ));
        assert!(!should_try_desktop_after_cli_api_error(
            &api::ApiError::Network("offline".into())
        ));
    }
}
