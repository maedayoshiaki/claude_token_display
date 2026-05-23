//! メニューバー / システムトレイ。
//!
//! 表示: 現在のセッション (5h) の utilization % のみ。詳細はポップオーバーで。
//! 5分おきに自動更新。429 を受けたら Retry-After を尊重。

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, Runtime,
};
use tauri_plugin_positioner::{Position, WindowExt};

use crate::{api::UsageSnapshot, fetch_usage_inner, FetchResult};

const POLL_INTERVAL_SECS: u64 = 300;
const MIN_SLEEP_SECS: u64 = 60;
const INITIAL_DELAY_SECS: u64 = 2;

/// クリック時に記録するトレイアイコンの screen 矩形 (physical pixel)。
/// move_window が NSPanel 化後に効きにくいので自前で popover 位置を計算するための材料。
static TRAY_X: AtomicI32 = AtomicI32::new(0);
static TRAY_Y: AtomicI32 = AtomicI32::new(0);
static TRAY_W: AtomicI32 = AtomicI32::new(0);
static TRAY_H: AtomicI32 = AtomicI32::new(0);
static TRAY_RECT_SET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// popover y のトレイ底からのオフセット (physical px)。重なる/離れすぎを微調整するならここ。
const POPOVER_Y_OFFSET: i32 = 0;

/// 最後に成功 / 失敗した取得結果のキャッシュ。クリックや popover オープン時はこれを返す。
type Cache = Arc<Mutex<FetchResult>>;

pub fn setup<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let refresh_item = MenuItem::with_id(app, "refresh", "Refresh now", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&refresh_item, &quit_item])?;

    let cache: Cache = Arc::new(Mutex::new(FetchResult::Err {
        message: "Loading…".into(),
    }));

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
                    let result = fetch_usage_inner().await;
                    update_cache_and_emit(&h, &c, result);
                });
            }
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

    // ポーラ: 起動 2 秒後に初回取得 → 以降は POLL_INTERVAL_SECS or Retry-After で間隔調整
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(INITIAL_DELAY_SECS)).await;
        loop {
            let result = fetch_usage_inner().await;
            let sleep_secs = decide_sleep(&result);
            update_tray(&tray_clone, &result);
            *cache_clone.lock().unwrap() = result.clone();
            let _ = handle.emit("usage-updated", &result);
            tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
        }
    });

    Ok(())
}

fn decide_sleep(r: &FetchResult) -> u64 {
    match r {
        FetchResult::RateLimited { retry_after_secs } => {
            // Retry-After + 余裕 5s。最低 60s は空ける。
            retry_after_secs
                .map(|s| s + 5)
                .unwrap_or(POLL_INTERVAL_SECS)
                .max(MIN_SLEEP_SECS)
        }
        FetchResult::Err { .. } => MIN_SLEEP_SECS, // エラー時も叩きすぎないように 60s 待つ
        FetchResult::Ok(_) => POLL_INTERVAL_SECS,
    }
}

fn update_cache_and_emit<R: Runtime>(
    handle: &AppHandle<R>,
    cache: &Cache,
    result: FetchResult,
) {
    if let Some(tray) = handle.tray_by_id("main") {
        update_tray(&tray, &result);
    }
    *cache.lock().unwrap() = result.clone();
    let _ = handle.emit("usage-updated", &result);
}

fn update_tray<R: Runtime>(tray: &tauri::tray::TrayIcon<R>, result: &FetchResult) {
    let (title, tooltip) = match result {
        FetchResult::Ok(s) => (format_title(s), format_tooltip(s)),
        FetchResult::RateLimited { retry_after_secs } => {
            let s = retry_after_secs.unwrap_or(0);
            ("…".to_string(), format!("Rate limited, retry in {}s", s))
        }
        FetchResult::Err { message } => ("!".to_string(), message.clone()),
    };
    let _ = tray.set_title(Some(title));
    let _ = tray.set_tooltip(Some(tooltip));
}

fn format_title(s: &UsageSnapshot) -> String {
    match s.five_hour.as_ref() {
        Some(b) => format!("{}%", pct(b.utilization)),
        None => "—".to_string(),
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
        let _ = window.hide();
        return;
    }
    // macOS: フラグ事前適用
    #[cfg(target_os = "macos")]
    crate::macos_panel::promote_to_floating_panel(&window);

    let _ = window.show();

    // NonactivatingPanel 構成では set_focus すると key 取得失敗 → 副作用が出るため
    // 代わりに orderFrontRegardless で「フォーカス奪わず前面」を実現
    #[cfg(target_os = "macos")]
    crate::macos_panel::order_front_regardless(&window);
    #[cfg(not(target_os = "macos"))]
    let _ = window.set_focus();

    // 自前で位置計算: 記録したトレイ矩形 + window outer_size から popover の (x,y) を決める
    if TRAY_RECT_SET.load(Ordering::SeqCst) {
        let tray_x = TRAY_X.load(Ordering::SeqCst);
        let tray_y = TRAY_Y.load(Ordering::SeqCst);
        let tray_w = TRAY_W.load(Ordering::SeqCst);
        let tray_h = TRAY_H.load(Ordering::SeqCst);
        if let Ok(win_size) = window.outer_size() {
            let win_w = win_size.width as i32;
            let x = tray_x + tray_w / 2 - win_w / 2;
            let y = tray_y + tray_h + POPOVER_Y_OFFSET;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Bucket;
    use chrono::Utc;

    fn b(u: f64) -> Bucket {
        Bucket {
            utilization: u,
            resets_at: Utc::now(),
        }
    }

    #[test]
    fn title_shows_only_five_hour() {
        let s = UsageSnapshot {
            five_hour: Some(b(0.43)),
            seven_day: Some(b(0.17)),
            seven_day_sonnet: None,
            fetched_at: Utc::now(),
        };
        assert_eq!(format_title(&s), "43%");
    }

    #[test]
    fn title_empty() {
        let s = UsageSnapshot::default();
        assert_eq!(format_title(&s), "—");
    }

    #[test]
    fn decide_sleep_uses_retry_after_for_429() {
        let r = FetchResult::RateLimited {
            retry_after_secs: Some(120),
        };
        assert_eq!(decide_sleep(&r), 125);
    }

    #[test]
    fn decide_sleep_enforces_min() {
        let r = FetchResult::RateLimited {
            retry_after_secs: Some(5),
        };
        assert_eq!(decide_sleep(&r), 60);
    }
}
