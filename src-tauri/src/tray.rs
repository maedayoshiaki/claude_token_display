//! メニューバー / システムトレイ。
//!
//! 表示: 現在のセッション (5h) の utilization % のみ。詳細はポップオーバーで。
//! 5 分おき (ユーザ設定で変更可) に自動更新。429 を受けたら Retry-After を尊重。

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, Runtime,
};
use tauri_plugin_positioner::{Position, WindowExt};

use crate::{
    api::UsageSnapshot, current_poll_interval_secs, current_tray_metric, fetch_all_usage,
    poll_wake, resize_popover_width, AllUsage, FetchResult, Provider, TrayMetric,
    POPOVER_DEFAULT_WIDTH, POPOVER_WIDTH_STEP, MIN_POLL_INTERVAL_SECS,
};

const INITIAL_DELAY_SECS: u64 = 2;

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
    // 設定変更 (provider / interval) があれば notify_one() で即起き → キャッシュからトレイを即時更新 → 再取得。
    let wake = poll_wake();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(INITIAL_DELAY_SECS)).await;
        loop {
            let all = fetch_all_usage().await;
            let sleep_secs = decide_sleep_all(&all, current_poll_interval_secs());
            update_tray(&tray_clone, &all);
            *cache_clone.lock().unwrap() = all.clone();
            let _ = handle.emit("usage-updated", &all);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(sleep_secs)) => {}
                _ = wake.notified() => {
                    // 設定変更で起こされたケース。新フェッチを待たずに
                    // キャッシュ + 新しい provider 設定でトレイを即時更新する。
                    let cached = cache_clone.lock().unwrap().clone();
                    update_tray(&tray_clone, &cached);
                }
            }
        }
    });

    Ok(())
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

fn decide_sleep(r: &FetchResult, configured_interval: u64) -> u64 {
    let normal = configured_interval.max(MIN_POLL_INTERVAL_SECS);
    match r {
        FetchResult::RateLimited {
            retry_after_secs, ..
        } => retry_after_secs
            .map(|s| s + 5)
            .unwrap_or(normal)
            .max(MIN_POLL_INTERVAL_SECS),
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
    let title = format_dual_title(all);
    let tooltip = format_dual_tooltip(all);
    let _ = tray.set_title(Some(title));
    let _ = tray.set_tooltip(Some(tooltip));
}

/// トレイ title は両プロバイダのメトリクスを併記: `C 43% · X 30%`。
/// 片方しかログインしていない場合は失敗側を `!` に。両方失敗で `!`。
fn format_dual_title(all: &AllUsage) -> String {
    let metric = current_tray_metric();
    let c = short_status(&all.claude, metric);
    let x = short_status(&all.codex, metric);
    if c == "!" && x == "!" {
        return "!".to_string();
    }
    format!("C {} · X {}", c, x)
}

fn short_status(r: &FetchResult, metric: TrayMetric) -> String {
    match r {
        FetchResult::Ok { snapshot, .. } => {
            let bucket = match metric {
                TrayMetric::FiveHour => snapshot.five_hour.as_ref(),
                TrayMetric::Weekly => snapshot.seven_day.as_ref(),
            };
            match bucket {
                Some(b) => format!("{}%", pct(b.utilization)),
                None => "—".to_string(),
            }
        }
        FetchResult::RateLimited { .. } => "…".to_string(),
        FetchResult::Err { .. } => "!".to_string(),
    }
}

fn format_dual_tooltip(all: &AllUsage) -> String {
    let claude = provider_tooltip_line(Provider::Claude, &all.claude);
    let codex = provider_tooltip_line(Provider::Codex, &all.codex);
    format!("{}\n{}", claude, codex)
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
        FetchResult::Ok { snapshot, .. } => format!("{}: {}", name, format_tooltip(snapshot)),
        FetchResult::RateLimited {
            retry_after_secs, ..
        } => {
            let s = retry_after_secs.unwrap_or(0);
            format!("{}: rate limited (retry {}s)", name, s)
        }
        FetchResult::Err { message, .. } => format!("{}: {}", name, message),
    }
}

fn format_tooltip(s: &UsageSnapshot) -> String {
    let mut parts = Vec::new();
    if let Some(b) = &s.five_hour {
        parts.push(format!("5h: {}%", pct(b.utilization)));
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
                },
            },
            codex: FetchResult::Ok {
                provider: Provider::Codex,
                snapshot: UsageSnapshot {
                    five_hour: Some(b(0.30)),
                    seven_day: Some(b(0.12)),
                    seven_day_sonnet: None,
                    fetched_at: Utc::now(),
                },
            },
        };
        assert_eq!(format_dual_title(&all), "C 43% · X 30%");
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
        assert_eq!(format_dual_title(&all), "C 43% · X !");
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
        assert_eq!(format_dual_title(&all), "!");
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
