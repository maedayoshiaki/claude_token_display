//! メニューバー / システムトレイ。
//!
//! 表示: Claude は現在のセッション (5h)、Codex は週間使用量の utilization %。
//! 5 分おき (ユーザ設定で変更可) に自動更新。429 を受けたら Retry-After を尊重。

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, Runtime,
};
use tauri_plugin_positioner::{Position, WindowExt};

use crate::{
    api::UsageSnapshot, current_poll_interval_secs, current_tray_metric, fetch_all_usage,
    poll_wake, request_poll_fetch, resize_popover_width, take_poll_fetch_request, AllUsage,
    FetchResult, Provider, TrayMetric, POPOVER_DEFAULT_WIDTH, POPOVER_WIDTH_STEP,
    MIN_POLL_INTERVAL_SECS,
};

const INITIAL_DELAY_SECS: u64 = 2;

/// 手動リフレッシュ (refresh / reload) を連打しても、直前のフェッチからこの時間未満なら
/// 実 API を叩かず直近結果を描画し直すだけにする。usage エンドポイントは Claude Code /
/// Desktop と同じアカウント単位の狭い枠を共有するので、連続ヒットは 429 を誘発する。
const MIN_MANUAL_FETCH_SPACING_MS: i64 = 5_000;

/// クリック時に記録するトレイアイコンの screen 矩形 (physical pixel)。
/// move_window が NSPanel 化後に効きにくいので自前で popover 位置を計算するための材料。
static TRAY_X: AtomicI32 = AtomicI32::new(0);
static TRAY_Y: AtomicI32 = AtomicI32::new(0);
static TRAY_W: AtomicI32 = AtomicI32::new(0);
static TRAY_H: AtomicI32 = AtomicI32::new(0);
static TRAY_RECT_SET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// popover とタスクバー / メニューバーの間隔 (physical px)。
const POPOVER_GAP: i32 = 8;
/// 画面端にぴったり付けず、わずかに内側へ収める余白 (physical px)。
const SCREEN_MARGIN: i32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScreenRect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WindowSize {
    w: i32,
    h: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScreenInfo {
    full: ScreenRect,
    work: ScreenRect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScreenEdge {
    Top,
    Bottom,
    Left,
    Right,
}

impl ScreenRect {
    fn right(self) -> i32 {
        self.x + self.w
    }

    fn bottom(self) -> i32 {
        self.y + self.h
    }

    fn center_x(self) -> i32 {
        self.x + self.w / 2
    }

    fn center_y(self) -> i32 {
        self.y + self.h / 2
    }

    fn contains_point(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    fn inset(self, margin: i32) -> Self {
        if self.w <= margin * 2 || self.h <= margin * 2 {
            return self;
        }
        Self {
            x: self.x + margin,
            y: self.y + margin,
            w: self.w - margin * 2,
            h: self.h - margin * 2,
        }
    }
}

/// 最後に成功 / 失敗した取得結果のキャッシュ。クリックや popover オープン時はこれを返す。
type Cache = Arc<Mutex<AllUsage>>;
static USAGE_CACHE: OnceLock<Cache> = OnceLock::new();

fn loading_all_usage() -> AllUsage {
    AllUsage {
        claude: FetchResult::Err {
            provider: Provider::Claude,
            message: "Loading…".into(),
        },
        codex: FetchResult::Err {
            provider: Provider::Codex,
            message: "Loading…".into(),
        },
    }
}

pub fn setup<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let refresh_item = MenuItem::with_id(app, "refresh", "Refresh now", true, None::<&str>)?;
    let wider_item = MenuItem::with_id(app, "wider_popover", "Wider popover", true, None::<&str>)?;
    let reset_width_item = MenuItem::with_id(
        app,
        "reset_popover_width",
        "Reset popover width",
        true,
        None::<&str>,
    )?;
    let menu = Menu::with_items(
        app,
        &[&refresh_item, &wider_item, &reset_width_item, &quit_item],
    )?;

    let cache: Cache = Arc::new(Mutex::new(loading_all_usage()));
    let _ = USAGE_CACHE.set(cache.clone());

    let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))
        .expect("embedded tray icon should decode");

    let menu_handle = app.clone();
    let menu_cache = cache.clone();

    let click_cache = cache.clone();
    let tray = TrayIconBuilder::with_id("main")
        .icon(tray_icon)
        .icon_as_template(true)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .title("…")
        .on_menu_event(move |_app, event| match event.id.as_ref() {
            "quit" => menu_handle.exit(0),
            "refresh" => {
                let h = menu_handle.clone();
                let c = menu_cache.clone();
                update_cache_and_emit(&h, &c, loading_all_usage());
                tauri::async_runtime::spawn(async move {
                    let result = fetch_all_usage().await;
                    update_cache_and_emit(&h, &c, result);
                });
            }
            "wider_popover" => adjust_popover_width(&menu_handle, POPOVER_WIDTH_STEP),
            "reset_popover_width" => set_popover_width(&menu_handle, POPOVER_DEFAULT_WIDTH),
            _ => {}
        })
        .on_tray_icon_event(move |tray, event| {
            tauri_plugin_positioner::on_tray_event(tray.app_handle(), &event);

            // どんなイベントでも rect 情報があればトレイ位置を最新化
            if let Some((pos, size)) = tray_rect(&event) {
                TRAY_X.store(pos.0, Ordering::SeqCst);
                TRAY_Y.store(pos.1, Ordering::SeqCst);
                TRAY_W.store(size.0, Ordering::SeqCst);
                TRAY_H.store(size.1, Ordering::SeqCst);
                TRAY_RECT_SET.store(true, Ordering::SeqCst);
            }

            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = &event
            {
                toggle_popover(tray.app_handle(), &click_cache);
            }
        })
        .build(app)?;

    let tray = Arc::new(tray);
    let handle = app.clone();
    let tray_clone = tray.clone();
    let cache_clone = cache.clone();

    // ポーラ: 起動 INITIAL_DELAY_SECS 秒後に初回取得 → 以降は設定値 or Retry-After で間隔調整。
    // wake で起こされたときは 2 種類ある:
    //   - 表示 / 間隔だけの変更 (tray_metric / provider / poll_interval): フェッチせず
    //     キャッシュ再描画 + sleep 再計算のみ (take_poll_fetch_request() == false)。
    //   - 手動リフレッシュ (refresh / reload): 実フェッチ。ただし直前フェッチから
    //     MIN_MANUAL_FETCH_SPACING_MS 未満なら連打とみなしスキップ (429 誘発防止)。
    let wake = poll_wake();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(INITIAL_DELAY_SECS)).await;
        let mut last = fetch_all_usage().await;
        let mut last_fetch_ms = now_ms();
        loop {
            // 直近の取得結果でトレイ / キャッシュ / popover を更新。
            update_tray(&tray_clone, &last);
            *cache_clone.lock().unwrap() = last.clone();
            let _ = handle.emit("usage-updated", &last);

            let sleep_secs = decide_sleep_all(&last, current_poll_interval_secs());
            let woke = tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(sleep_secs)) => false,
                _ = wake.notified() => true,
            };

            if woke {
                // フェッチ要求のない wake (表示 / 間隔変更) はループ先頭に戻って
                // キャッシュ再描画 + sleep 再計算だけ行う。API は叩かない。
                if !take_poll_fetch_request() {
                    continue;
                }
                // 「レート制限を無視して更新」時は最小間隔チェックを飛ばして必ず取得する。
                let force_immediate = crate::take_poll_force_immediate();
                // 通常の手動リフレッシュは直前フェッチから間隔が短すぎるとネットワークを
                // 叩かず直近結果を描画し直す。reload_now が置いた "Loading…" もここで実データに戻る。
                if !force_immediate && now_ms() - last_fetch_ms < MIN_MANUAL_FETCH_SPACING_MS {
                    continue;
                }
            }

            last = fetch_all_usage().await;
            last_fetch_ms = now_ms();
        }
    });

    Ok(())
}

pub fn clear_cached_usage_and_reload<R: Runtime>(handle: &AppHandle<R>) {
    if let Some(cache) = USAGE_CACHE.get() {
        update_cache_and_emit(handle, cache, loading_all_usage());
    }
    // 手動リフレッシュなので実フェッチを要求する (スペーシングが効けばスキップされ、
    // その場合は直近の実データが再描画されて "Loading…" は消える)。
    request_poll_fetch();
}

fn current_popover_width<R: Runtime>(window: &tauri::WebviewWindow<R>) -> Option<f64> {
    let scale = window.scale_factor().ok()?;
    let size = window.inner_size().ok()?;
    Some(size.to_logical::<f64>(scale).width)
}

fn adjust_popover_width<R: Runtime>(app: &AppHandle<R>, delta: f64) {
    let Some(window) = app.get_webview_window("popover") else {
        return;
    };
    let width = current_popover_width(&window)
        .map(|width| width + delta)
        .unwrap_or(POPOVER_DEFAULT_WIDTH);
    let _ = resize_popover_width(&window, width);
}

fn set_popover_width<R: Runtime>(app: &AppHandle<R>, width: f64) {
    let Some(window) = app.get_webview_window("popover") else {
        return;
    };
    let _ = resize_popover_width(&window, width);
}

/// 403 (credential restricted) は待っても直らない恒久ブロックなので、通常エラーの
/// 高速リトライ (MIN_POLL_INTERVAL_SECS) ではなく長めにバックオフする。再ログインや
/// Anthropic 側の方針変更で解消する可能性は残すため、停止ではなく低頻度ポーリング。
const CREDENTIAL_RESTRICTED_BACKOFF_SECS: u64 = 30 * 60;

fn decide_sleep(r: &FetchResult, configured_interval: u64) -> u64 {
    let normal = configured_interval.max(MIN_POLL_INTERVAL_SECS);
    match r {
        FetchResult::RateLimited {
            retry_after_secs, ..
        } => retry_after_secs
            .map(|s| s + 5)
            .unwrap_or(normal)
            .max(MIN_POLL_INTERVAL_SECS),
        FetchResult::CredentialRestricted { .. } => {
            configured_interval.max(CREDENTIAL_RESTRICTED_BACKOFF_SECS)
        }
        FetchResult::Err { .. } => MIN_POLL_INTERVAL_SECS,
        FetchResult::Ok { .. } => normal,
    }
}

/// 両プロバイダの結果を見て、より長めの待ちを採用する (どちらかが 429 ならそれを尊重)。
fn decide_sleep_all(all: &AllUsage, configured_interval: u64) -> u64 {
    decide_sleep(&all.claude, configured_interval)
        .max(decide_sleep(&all.codex, configured_interval))
}

fn update_cache_and_emit<R: Runtime>(handle: &AppHandle<R>, cache: &Cache, result: AllUsage) {
    if let Some(tray) = handle.tray_by_id("main") {
        update_tray(&tray, &result);
    }
    *cache.lock().unwrap() = result.clone();
    let _ = handle.emit("usage-updated", &result);
}

fn update_tray<R: Runtime>(tray: &tauri::tray::TrayIcon<R>, all: &AllUsage) {
    let show_claude = crate::tray_shows_claude();
    let show_codex = crate::tray_shows_codex();
    let title = format_dual_title(all, show_claude, show_codex);
    let tooltip = format_dual_tooltip(all, show_claude, show_codex);
    let _ = tray.set_title(Some(title));
    let _ = tray.set_tooltip(Some(tooltip));
}

/// トレイ title は表示中のプロバイダのメトリクスを併記: `C 43% · X 30%`。
/// 片方しかログインしていない場合は失敗側を `!` に。両方失敗で `!`。
/// 片方だけ表示 (トレイ設定で非表示) のときは接頭辞なしの数字のみ (`43%`)。
/// 両方非表示なら空文字 (アイコンのみ)。
fn format_dual_title(all: &AllUsage, show_claude: bool, show_codex: bool) -> String {
    let metric = current_tray_metric();
    match (show_claude, show_codex) {
        (true, true) => {
            let c = short_status(Provider::Claude, &all.claude, metric);
            let x = short_status(Provider::Codex, &all.codex, metric);
            if c == "!" && x == "!" {
                "!".to_string()
            } else {
                format!("C {} · X {}", c, x)
            }
        }
        (true, false) => short_status(Provider::Claude, &all.claude, metric),
        (false, true) => short_status(Provider::Codex, &all.codex, metric),
        (false, false) => String::new(),
    }
}

fn short_status(provider: Provider, r: &FetchResult, metric: TrayMetric) -> String {
    match r {
        FetchResult::Ok { snapshot, .. } => {
            let bucket = match provider {
                // Codex の primary_window (旧5h) は使用せず、設定値に関係なく週次を表示。
                Provider::Codex => snapshot.seven_day.as_ref(),
                Provider::Claude => match metric {
                    TrayMetric::FiveHour => snapshot.five_hour.as_ref(),
                    TrayMetric::Weekly => snapshot.seven_day.as_ref(),
                },
            };
            match bucket {
                Some(b) => format!("{}%", pct(b.utilization)),
                None => "—".to_string(),
            }
        }
        FetchResult::RateLimited { .. } => "…".to_string(),
        FetchResult::CredentialRestricted { .. } | FetchResult::Err { .. } => "!".to_string(),
    }
}

fn format_dual_tooltip(all: &AllUsage, show_claude: bool, show_codex: bool) -> String {
    let mut lines: Vec<String> = Vec::new();
    if show_claude {
        lines.push(provider_tooltip_line(Provider::Claude, &all.claude));
    }
    if show_codex {
        lines.push(provider_tooltip_line(Provider::Codex, &all.codex));
    }
    if lines.is_empty() {
        "token_display".to_string()
    } else {
        lines.join("\n")
    }
}

fn provider_label(p: Provider) -> &'static str {
    match p {
        Provider::Claude => "Claude",
        Provider::Codex => "Codex",
    }
}

fn provider_tooltip_line(provider: Provider, r: &FetchResult) -> String {
    let name = provider_label(provider);
    match r {
        FetchResult::Ok { snapshot, .. } => {
            format!("{}: {}", name, format_tooltip(provider, snapshot))
        }
        FetchResult::RateLimited {
            retry_after_secs, ..
        } => {
            let s = retry_after_secs.unwrap_or(0);
            format!("{}: rate limited (retry {}s)", name, s)
        }
        FetchResult::CredentialRestricted { message, .. } | FetchResult::Err { message, .. } => {
            format!("{}: {}", name, message)
        }
    }
}

fn format_tooltip(provider: Provider, s: &UsageSnapshot) -> String {
    let mut parts = Vec::new();
    if provider == Provider::Claude {
        if let Some(b) = &s.five_hour {
            parts.push(format!("5h: {}%", pct(b.utilization)));
        }
    }
    if let Some(b) = &s.seven_day {
        parts.push(format!("7d: {}%", pct(b.utilization)));
    }
    if let Some(b) = &s.seven_day_sonnet {
        parts.push(format!("7d Sonnet: {}%", pct(b.utilization)));
    }
    if parts.is_empty() {
        "no data".to_string()
    } else {
        parts.join(" · ")
    }
}

fn pct(u: f64) -> u32 {
    (u * 100.0).round() as u32
}

fn toggle_popover<R: Runtime>(app: &AppHandle<R>, cache: &Cache) {
    let Some(window) = app.get_webview_window("popover") else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        if crate::is_popover_pinned() {
            let _ = window.show();
            #[cfg(target_os = "macos")]
            crate::macos_panel::order_front_regardless(&window);
            #[cfg(not(target_os = "macos"))]
            let _ = window.set_focus();
            return;
        }
        let _ = window.hide();
        return;
    }
    // macOS: フラグ事前適用
    #[cfg(target_os = "macos")]
    crate::macos_panel::promote_to_floating_panel(&window);

    let _ = window.set_shadow(false);
    let _ = window.show();

    // NonactivatingPanel 構成では set_focus すると key 取得失敗 → 副作用が出るため
    // 代わりに orderFrontRegardless で「フォーカス奪わず前面」を実現
    #[cfg(target_os = "macos")]
    crate::macos_panel::order_front_regardless(&window);
    #[cfg(not(target_os = "macos"))]
    let _ = window.set_focus();

    // 自前で位置計算: 記録したトレイ矩形 + window outer_size から popover の (x,y) を決める
    if TRAY_RECT_SET.load(Ordering::SeqCst) {
        let tray_rect = ScreenRect {
            x: TRAY_X.load(Ordering::SeqCst),
            y: TRAY_Y.load(Ordering::SeqCst),
            w: TRAY_W.load(Ordering::SeqCst),
            h: TRAY_H.load(Ordering::SeqCst),
        };
        if let Ok(win_size) = window.outer_size() {
            let win_size = WindowSize {
                w: win_size.width as i32,
                h: win_size.height as i32,
            };
            let (x, y) = match screen_info_for_tray(&window, tray_rect) {
                Some(screen) => popover_position(tray_rect, win_size, screen),
                None => fallback_popover_position(tray_rect, win_size),
            };
            let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
        }
    } else {
        // 初回でトレイ rect 未取得時のフォールバック
        let _ = window.move_window(Position::TrayCenter);
    }

    // show 直後の Focused(false) によるオートクローズ抑止
    crate::SHOWN_AT_MS.store(now_ms(), Ordering::SeqCst);

    // キャッシュ値だけを popover に流す（API は叩かない）
    if let Ok(guard) = cache.lock() {
        let _ = app.emit("usage-updated", &*guard);
    }

    // 起動直後に取りこぼした「更新あり」を、開いたタイミングで再通知する
    // (initUpdateCheck はバックエンド初回チェック前に走り空振りするため)。
    crate::update::reemit_cached_update(app);
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// TrayIconEvent の各バリアントから rect を取り出す。
/// 返り値: ((x, y), (w, h)) physical pixel。
fn tray_rect(event: &TrayIconEvent) -> Option<((i32, i32), (i32, i32))> {
    let rect = match event {
        TrayIconEvent::Click { rect, .. }
        | TrayIconEvent::DoubleClick { rect, .. }
        | TrayIconEvent::Enter { rect, .. }
        | TrayIconEvent::Move { rect, .. }
        | TrayIconEvent::Leave { rect, .. } => rect,
        _ => return None,
    };
    let pos = rect.position.to_physical::<i32>(1.0);
    let size = rect.size.to_physical::<i32>(1.0);
    Some(((pos.x, pos.y), (size.width, size.height)))
}

fn screen_info_for_tray<R: Runtime>(
    window: &tauri::WebviewWindow<R>,
    tray: ScreenRect,
) -> Option<ScreenInfo> {
    let monitors = window.available_monitors().ok()?;
    let center_x = tray.center_x();
    let center_y = tray.center_y();

    monitors
        .iter()
        .find(|monitor| {
            screen_info_from_monitor(monitor)
                .full
                .contains_point(center_x, center_y)
        })
        .or_else(|| monitors.first())
        .map(screen_info_from_monitor)
}

fn screen_info_from_monitor(monitor: &tauri::Monitor) -> ScreenInfo {
    let pos = monitor.position();
    let size = monitor.size();
    let work = monitor.work_area();
    ScreenInfo {
        full: ScreenRect {
            x: pos.x,
            y: pos.y,
            w: size.width as i32,
            h: size.height as i32,
        },
        work: ScreenRect {
            x: work.position.x,
            y: work.position.y,
            w: work.size.width as i32,
            h: work.size.height as i32,
        },
    }
}

fn popover_position(tray: ScreenRect, win: WindowSize, screen: ScreenInfo) -> (i32, i32) {
    let (x, y) = match tray_edge(tray, screen) {
        ScreenEdge::Bottom => (tray.center_x() - win.w / 2, tray.y - POPOVER_GAP - win.h),
        ScreenEdge::Top => (tray.center_x() - win.w / 2, tray.bottom() + POPOVER_GAP),
        ScreenEdge::Right => (tray.x - POPOVER_GAP - win.w, tray.center_y() - win.h / 2),
        ScreenEdge::Left => (tray.right() + POPOVER_GAP, tray.center_y() - win.h / 2),
    };
    clamp_to_rect(x, y, win, screen.work.inset(SCREEN_MARGIN))
}

fn fallback_popover_position(tray: ScreenRect, win: WindowSize) -> (i32, i32) {
    let y = if cfg!(target_os = "windows") {
        tray.y - POPOVER_GAP - win.h
    } else {
        tray.bottom() + POPOVER_GAP
    };
    (tray.center_x() - win.w / 2, y)
}

fn tray_edge(tray: ScreenRect, screen: ScreenInfo) -> ScreenEdge {
    let work = screen.work;
    let full = screen.full;
    let center_x = tray.center_x();
    let center_y = tray.center_y();

    if work.bottom() < full.bottom() && center_y >= work.bottom() {
        return ScreenEdge::Bottom;
    }
    if work.y > full.y && center_y <= work.y {
        return ScreenEdge::Top;
    }
    if work.right() < full.right() && center_x >= work.right() {
        return ScreenEdge::Right;
    }
    if work.x > full.x && center_x <= work.x {
        return ScreenEdge::Left;
    }

    let bottom_distance = (full.bottom() - tray.bottom()).abs();
    let top_distance = (tray.y - full.y).abs();
    let edge_threshold = tray.h.max(32);
    if bottom_distance <= top_distance && bottom_distance <= edge_threshold {
        ScreenEdge::Bottom
    } else if top_distance < bottom_distance && top_distance <= edge_threshold {
        ScreenEdge::Top
    } else {
        let right_distance = (full.right() - tray.right()).abs();
        let left_distance = (tray.x - full.x).abs();
        if right_distance <= left_distance {
            ScreenEdge::Right
        } else {
            ScreenEdge::Left
        }
    }
}

fn clamp_to_rect(x: i32, y: i32, win: WindowSize, bounds: ScreenRect) -> (i32, i32) {
    (
        clamp_axis(x, bounds.x, bounds.right() - win.w),
        clamp_axis(y, bounds.y, bounds.bottom() - win.h),
    )
}

fn clamp_axis(value: i32, min: i32, max: i32) -> i32 {
    if max < min {
        min
    } else {
        value.clamp(min, max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Bucket;
    use crate::Provider;
    use chrono::Utc;

    fn b(u: f64) -> Bucket {
        Bucket {
            utilization: u,
            resets_at: Some(Utc::now()),
        }
    }

    #[test]
    fn dual_title_combines_both_providers() {
        let all = AllUsage {
            claude: FetchResult::Ok {
                provider: Provider::Claude,
                snapshot: UsageSnapshot {
                    five_hour: Some(b(0.43)),
                    seven_day: Some(b(0.17)),
                    seven_day_sonnet: None,
                    fetched_at: Utc::now(),
                    ..UsageSnapshot::default()
                },
            },
            codex: FetchResult::Ok {
                provider: Provider::Codex,
                snapshot: UsageSnapshot {
                    five_hour: Some(b(0.30)),
                    seven_day: Some(b(0.12)),
                    seven_day_sonnet: None,
                    fetched_at: Utc::now(),
                    ..UsageSnapshot::default()
                },
            },
        };
        assert_eq!(format_dual_title(&all, true, true), "C 43% · X 12%");
    }

    #[test]
    fn dual_title_marks_errors_with_bang() {
        let all = AllUsage {
            claude: FetchResult::Ok {
                provider: Provider::Claude,
                snapshot: UsageSnapshot {
                    five_hour: Some(b(0.43)),
                    ..UsageSnapshot::default()
                },
            },
            codex: FetchResult::Err {
                provider: Provider::Codex,
                message: "not logged in".into(),
            },
        };
        assert_eq!(format_dual_title(&all, true, true), "C 43% · X !");
    }

    #[test]
    fn dual_title_collapses_when_both_fail() {
        let all = AllUsage {
            claude: FetchResult::Err {
                provider: Provider::Claude,
                message: "x".into(),
            },
            codex: FetchResult::Err {
                provider: Provider::Codex,
                message: "y".into(),
            },
        };
        assert_eq!(format_dual_title(&all, true, true), "!");
    }

    #[test]
    fn title_hides_codex_when_tray_codex_off() {
        let all = AllUsage {
            claude: FetchResult::Ok {
                provider: Provider::Claude,
                snapshot: UsageSnapshot {
                    five_hour: Some(b(0.43)),
                    ..UsageSnapshot::default()
                },
            },
            codex: FetchResult::Ok {
                provider: Provider::Codex,
                snapshot: UsageSnapshot {
                    seven_day: Some(b(0.30)),
                    ..UsageSnapshot::default()
                },
            },
        };
        // Codex 非表示: 接頭辞なしで Claude の数字のみ。
        assert_eq!(format_dual_title(&all, true, false), "43%");
        // Claude 非表示: Codex の数字のみ。
        assert_eq!(format_dual_title(&all, false, true), "30%");
        // 両方非表示: 空 (アイコンのみ)。
        assert_eq!(format_dual_title(&all, false, false), "");
    }

    #[test]
    fn tooltip_hides_lines_for_hidden_providers() {
        let all = AllUsage {
            claude: FetchResult::Ok {
                provider: Provider::Claude,
                snapshot: UsageSnapshot {
                    five_hour: Some(b(0.43)),
                    ..UsageSnapshot::default()
                },
            },
            codex: FetchResult::Ok {
                provider: Provider::Codex,
                snapshot: UsageSnapshot {
                    five_hour: Some(b(0.30)),
                    ..UsageSnapshot::default()
                },
            },
        };
        let tip = format_dual_tooltip(&all, true, false);
        assert!(tip.contains("Claude"));
        assert!(!tip.contains("Codex"));
        assert_eq!(format_dual_tooltip(&all, false, false), "token_display");
    }

    #[test]
    fn codex_tooltip_uses_weekly_only() {
        let snapshot = UsageSnapshot {
            five_hour: Some(b(0.30)),
            seven_day: Some(b(0.12)),
            ..UsageSnapshot::default()
        };
        assert_eq!(format_tooltip(Provider::Codex, &snapshot), "7d: 12%");
    }

    #[test]
    fn decide_sleep_backs_off_on_credential_restricted() {
        let r = FetchResult::CredentialRestricted {
            provider: Provider::Claude,
            message: "blocked".into(),
        };
        // 設定 5 分でも credential 制限時は最低 30 分にバックオフ (高速リトライしない)
        assert_eq!(decide_sleep(&r, 300), CREDENTIAL_RESTRICTED_BACKOFF_SECS);
        // 設定がバックオフより長ければ設定値を尊重
        assert_eq!(decide_sleep(&r, 60 * 60), 60 * 60);
    }

    #[test]
    fn credential_restricted_marks_title_with_bang() {
        let all = AllUsage {
            claude: FetchResult::CredentialRestricted {
                provider: Provider::Claude,
                message: "blocked".into(),
            },
            codex: FetchResult::Ok {
                provider: Provider::Codex,
                snapshot: UsageSnapshot {
                    five_hour: Some(b(0.30)),
                    seven_day: Some(b(0.30)),
                    ..UsageSnapshot::default()
                },
            },
        };
        assert_eq!(format_dual_title(&all, true, true), "C ! · X 30%");
    }

    #[test]
    fn decide_sleep_uses_retry_after_for_429() {
        let r = FetchResult::RateLimited {
            provider: Provider::Claude,
            retry_after_secs: Some(120),
        };
        assert_eq!(decide_sleep(&r, 300), 125);
    }

    #[test]
    fn decide_sleep_enforces_min() {
        let r = FetchResult::RateLimited {
            provider: Provider::Claude,
            retry_after_secs: Some(5),
        };
        assert_eq!(decide_sleep(&r, 300), 60);
    }

    #[test]
    fn decide_sleep_honors_configured_interval_on_ok() {
        let r = FetchResult::Ok {
            provider: Provider::Claude,
            snapshot: UsageSnapshot::default(),
        };
        assert_eq!(decide_sleep(&r, 900), 900);
    }

    #[test]
    fn decide_sleep_all_takes_longest_wait() {
        let all = AllUsage {
            claude: FetchResult::Ok {
                provider: Provider::Claude,
                snapshot: UsageSnapshot::default(),
            },
            codex: FetchResult::RateLimited {
                provider: Provider::Codex,
                retry_after_secs: Some(200),
            },
        };
        // Claude: configured 300; Codex: 205 (200+5). 結果は 300。
        assert_eq!(decide_sleep_all(&all, 300), 300);
    }

    #[test]
    fn decide_sleep_all_respects_long_retry_after() {
        let all = AllUsage {
            claude: FetchResult::Ok {
                provider: Provider::Claude,
                snapshot: UsageSnapshot::default(),
            },
            codex: FetchResult::RateLimited {
                provider: Provider::Codex,
                retry_after_secs: Some(900),
            },
        };
        assert_eq!(decide_sleep_all(&all, 300), 905);
    }

    fn rect(x: i32, y: i32, w: i32, h: i32) -> ScreenRect {
        ScreenRect { x, y, w, h }
    }

    fn screen(full: ScreenRect, work: ScreenRect) -> ScreenInfo {
        ScreenInfo { full, work }
    }

    #[test]
    fn popover_is_above_bottom_taskbar() {
        let screen = screen(rect(0, 0, 1920, 1080), rect(0, 0, 1920, 1040));
        let tray = rect(1800, 1040, 32, 40);
        let win = WindowSize { w: 340, h: 420 };

        let (x, y) = popover_position(tray, win, screen);

        assert!(y < tray.y);
        assert!(y + win.h <= screen.work.bottom() - SCREEN_MARGIN);
        assert!(x + win.w <= screen.work.right() - SCREEN_MARGIN);
    }

    #[test]
    fn popover_is_below_top_taskbar() {
        let screen = screen(rect(0, 0, 1920, 1080), rect(0, 40, 1920, 1040));
        let tray = rect(1800, 0, 32, 40);
        let win = WindowSize { w: 340, h: 420 };

        let (_, y) = popover_position(tray, win, screen);

        assert!(y > tray.bottom());
        assert!(y >= screen.work.y + SCREEN_MARGIN);
    }

    #[test]
    fn popover_is_left_of_right_taskbar() {
        let screen = screen(rect(0, 0, 1920, 1080), rect(0, 0, 1880, 1080));
        let tray = rect(1880, 500, 40, 32);
        let win = WindowSize { w: 340, h: 420 };

        let (x, y) = popover_position(tray, win, screen);

        assert!(x < tray.x);
        assert!(x + win.w <= screen.work.right() - SCREEN_MARGIN);
        assert!(y >= screen.work.y + SCREEN_MARGIN);
        assert!(y + win.h <= screen.work.bottom() - SCREEN_MARGIN);
    }
}
