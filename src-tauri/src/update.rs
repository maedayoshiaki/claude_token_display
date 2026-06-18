//! アプリ自身の更新チェック。
//!
//! GitHub Releases の最新版 (`GET /repos/{repo}/releases/latest`) を参照し、
//! 現バージョン (`CARGO_PKG_VERSION`) より新しければ「更新あり」を通知する。
//! 自動インストールはせず、ユーザにリリースページを案内するだけ (notify only)。
//!
//! チェック頻度は控えめ: 起動少し後に 1 回 + 以降 `CHECK_INTERVAL` ごと。
//! GitHub の未認証 API はIPあたり 60 req/h なので、この頻度なら余裕で収まる。

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};

/// 監視対象リポジトリ (owner/repo)。
const REPO: &str = "maedayoshiaki/claude_token_display";
/// 起動後、最初のチェックまでの待ち (usage の初回取得や起動処理を邪魔しないため)。
const INITIAL_DELAY: Duration = Duration::from_secs(30);

/// frontend に渡す更新情報。
#[derive(Serialize, Clone, Debug)]
pub struct UpdateInfo {
    /// 現在インストールされているバージョン (例: "0.4.2")。
    pub current: String,
    /// GitHub 上の最新リリースのバージョン (例: "0.4.3")。
    pub latest: String,
    /// latest が current より新しいか。
    pub available: bool,
    /// リリースページの URL (ユーザに開かせる)。
    pub url: String,
}

/// 直近のチェック結果のキャッシュ。popover が後から開いたときに
/// `get_update_info` で取り出して即バナー表示できるようにする。
fn cache() -> &'static Mutex<Option<UpdateInfo>> {
    static CACHE: OnceLock<Mutex<Option<UpdateInfo>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// 現在のアプリバージョン (ビルド時に Cargo が埋め込む)。
///
/// テスト用: dev ビルド (`tauri dev`) でのみ環境変数 `TOKEN_DISPLAY_FAKE_VERSION`
/// で現バージョンを偽装できる。例えば `0.0.1` にすれば、既存の公開リリース
/// (v0.4.2 等) が「更新あり」として扱われ、リリースを増やさずに通知〜
/// リリースページを開くまでの実経路をまるごと確認できる。
/// release ビルドでは無視される (誤動作防止)。
fn current_version() -> String {
    #[cfg(debug_assertions)]
    {
        if let Ok(v) = std::env::var("TOKEN_DISPLAY_FAKE_VERSION") {
            let v = v.trim();
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    env!("CARGO_PKG_VERSION").to_string()
}

/// "v1.2.3" / "1.2.3-beta" 等から数値の (major, minor, patch) を取り出す。
/// pre-release / build メタデータは無視する (notify 用途には十分)。
fn parse_version(raw: &str) -> Option<(u64, u64, u64)> {
    let trimmed = raw.trim().trim_start_matches(['v', 'V']);
    // "1.2.3-beta+build" → core "1.2.3" だけ見る
    let core = trimmed
        .split(['-', '+'])
        .next()
        .unwrap_or(trimmed);
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let major = parts.next().flatten()?;
    let minor = parts.next().flatten().unwrap_or(0);
    let patch = parts.next().flatten().unwrap_or(0);
    Some((major, minor, patch))
}

/// `latest` が `current` より新しいバージョンか。
/// どちらかがパースできなければ「更新なし」(false) に倒す (誤通知を避ける)。
fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

/// GitHub から最新リリースを取得して UpdateInfo を組み立てる。
async fn fetch_latest() -> Result<UpdateInfo, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .get(&url)
        // GitHub API は User-Agent 必須。
        .header("User-Agent", "token_display-update-check")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = resp.status().as_u16();
    // 404 = まだ公開リリースが無い。エラー扱いせず「更新なし」を返す。
    if status == 404 {
        let current = current_version();
        return Ok(UpdateInfo {
            latest: current.clone(),
            current,
            available: false,
            url: format!("https://github.com/{REPO}/releases"),
        });
    }
    if status != 200 {
        return Err(format!("GitHub API HTTP {status}"));
    }

    let release: GithubRelease = resp.json().await.map_err(|e| e.to_string())?;
    let current = current_version().to_string();
    let latest = release.tag_name.trim_start_matches(['v', 'V']).to_string();
    Ok(UpdateInfo {
        available: is_newer(&latest, &current),
        current,
        latest,
        url: release.html_url,
    })
}

/// 1 回チェックしてキャッシュ更新。更新ありなら `update-available` を emit し、
/// 取得した UpdateInfo を返す。
async fn run_check<R: Runtime>(app: &AppHandle<R>) -> Result<UpdateInfo, String> {
    let info = fetch_latest().await?;
    *cache().lock().unwrap() = Some(info.clone());
    if info.available {
        let _ = app.emit("update-available", &info);
    }
    Ok(info)
}

/// 起動時に 1 回 + 以降ユーザ設定の間隔ごとにチェックするタスクを spawn する。
/// 間隔は `crate::current_update_check_interval_secs()` を毎回読み、
/// 設定変更時は `crate::update_wake()` の notify で待ち直す。
pub fn spawn_checker<R: Runtime>(app: AppHandle<R>) {
    let wake = crate::update_wake();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(INITIAL_DELAY).await;
        loop {
            if let Err(e) = run_check(&app).await {
                // ネットワーク不通などは黙ってスキップ (次回チェックで再試行)。
                eprintln!("update check failed: {e}");
            }
            // 次のチェックまで待つ。間隔変更で起こされたら新しい間隔で待ち直す
            // (即チェックはせず、GitHub への余計なアクセスを避ける)。
            loop {
                let secs = crate::current_update_check_interval_secs();
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(secs)) => break,
                    _ = wake.notified() => continue,
                }
            }
        }
    });
}

/// 設定パネルの「今すぐ確認」ボタンから呼ぶ即時チェック。
/// 結果 (最新版か更新ありか) を frontend に返す。
#[tauri::command]
pub async fn check_update_now(app: tauri::AppHandle) -> Result<UpdateInfo, String> {
    run_check(&app).await
}

/// frontend が popover ロード時に直近のチェック結果を取り出すためのコマンド。
#[tauri::command]
pub fn get_update_info() -> Option<UpdateInfo> {
    cache().lock().unwrap().clone()
}

/// キャッシュ済みリリース URL を OS の既定ブラウザで開く。
/// 任意 URL は受けず、我々がチェックで得た URL のみを開く (安全側)。
#[tauri::command]
pub fn open_release_page() -> Result<(), String> {
    let url = cache()
        .lock()
        .unwrap()
        .as_ref()
        .map(|i| i.url.clone())
        .unwrap_or_else(|| format!("https://github.com/{REPO}/releases"));
    open_url(&url)
}

#[cfg(target_os = "windows")]
fn open_url(url: &str) -> Result<(), String> {
    use std::process::Command;
    // cmd の start は第1引数をウィンドウタイトルと解釈するので空文字を渡す。
    Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
fn open_url(url: &str) -> Result<(), String> {
    use std::process::Command;
    Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn open_url(url: &str) -> Result<(), String> {
    use std::process::Command;
    Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_v_prefixed() {
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("v0.4.2"), Some((0, 4, 2)));
        assert_eq!(parse_version("V2.0"), Some((2, 0, 0)));
    }

    #[test]
    fn ignores_prerelease_and_build_metadata() {
        assert_eq!(parse_version("1.2.3-beta.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2.3+build9"), Some((1, 2, 3)));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_version("not-a-version"), None);
    }

    #[test]
    fn newer_detects_patch_minor_major() {
        assert!(is_newer("0.4.3", "0.4.2"));
        assert!(is_newer("0.5.0", "0.4.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
    }

    #[test]
    fn not_newer_when_equal_or_older() {
        assert!(!is_newer("0.4.2", "0.4.2"));
        assert!(!is_newer("0.4.1", "0.4.2"));
    }

    #[test]
    fn unparsable_versions_are_not_newer() {
        assert!(!is_newer("garbage", "0.4.2"));
        assert!(!is_newer("0.4.3", "garbage"));
    }
}
