//! Anthropic OAuth usage エンドポイント `GET /api/oauth/usage` のクライアント。
//!
//! Headers:
//!   Authorization: Bearer <access_token>
//!   anthropic-beta: oauth-2025-04-20

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const BETA_HEADER: &str = "oauth-2025-04-20";

/// `/api/oauth/usage` は `claude-code/<version>` を名乗る User-Agent でないと
/// 攻撃的にレート制限 (429) される積極バケットに入れられる (read-only 用途)。
/// 古いバージョンを名乗ると厳しいバケット行きになりうるため、Claude Code 本体の
/// バージョン更新に追従して適宜上げる (`claude --version` に合わせる)。
const CLAUDE_CODE_USER_AGENT: &str = "claude-code/2.1.197";

#[derive(thiserror::Error, Debug)]
pub enum ApiError {
    #[error("network error: {0}")]
    Network(String),

    #[error("unauthorized (token expired or revoked) — try `claude` login again")]
    Unauthorized,

    /// 403。トークン自体は有効だが、この資格情報では `/api/oauth/usage` が許可されていない。
    /// Anthropic が「この資格情報は Claude Code 専用」等の方針で usage 系を弾き始めた場合に出る。
    /// 待っても直らない恒久的なブロックなので、ポーラは長めにバックオフする (tray.rs)。
    #[error("{message}")]
    CredentialRestricted { message: String },

    #[error("rate limited; retry after {retry_after_secs:?}s")]
    RateLimited { retry_after_secs: Option<u64> },

    #[error("HTTP {0}")]
    Http(u16),

    #[error("decode error: {0}")]
    Decode(String),
}

/// API から受け取る生のバケット。
#[derive(Deserialize, Debug)]
struct RawBucket {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

#[derive(Deserialize, Debug)]
struct RawUsage {
    five_hour: Option<RawBucket>,
    seven_day: Option<RawBucket>,
    seven_day_sonnet: Option<RawBucket>,
}

/// frontend に渡す形式。
#[derive(Serialize, Clone, Debug)]
pub struct Bucket {
    /// 0.0 〜 1.0+（API は 0〜100 で返すので /100 する。1 超過は仕様上ありうる）。
    pub utilization: f64,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct UsageSnapshot {
    pub five_hour: Option<Bucket>,
    pub seven_day: Option<Bucket>,
    pub seven_day_sonnet: Option<Bucket>,
    pub fetched_at: DateTime<Utc>,
}

impl RawBucket {
    fn into_bucket(self) -> Option<Bucket> {
        let util = self.utilization?;
        let resets_at = self
            .resets_at
            .and_then(|resets| DateTime::parse_from_rfc3339(&resets).ok())
            .map(|resets| resets.with_timezone(&Utc));
        Some(Bucket {
            utilization: util / 100.0,
            resets_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_bucket_when_resets_at_is_missing() {
        let bucket = RawBucket {
            utilization: Some(43.0),
            resets_at: None,
        }
        .into_bucket()
        .expect("utilization-only bucket should be kept");

        assert!((bucket.utilization - 0.43).abs() < f64::EPSILON);
        assert!(bucket.resets_at.is_none());
    }

    #[test]
    fn keeps_bucket_when_resets_at_is_invalid() {
        let bucket = RawBucket {
            utilization: Some(17.0),
            resets_at: Some("not a date".to_string()),
        }
        .into_bucket()
        .expect("invalid reset timestamp should not drop utilization");

        assert!((bucket.utilization - 0.17).abs() < f64::EPSILON);
        assert!(bucket.resets_at.is_none());
    }

    #[test]
    fn extracts_nested_json_error_message() {
        let body = r#"{"error":{"type":"forbidden","message":"This credential is only authorized for use with Claude Code."}}"#;
        assert_eq!(
            extract_error_message(body).unwrap(),
            "This credential is only authorized for use with Claude Code."
        );
    }

    #[test]
    fn extracts_top_level_message() {
        let body = r#"{"message":"forbidden"}"#;
        assert_eq!(extract_error_message(body).unwrap(), "forbidden");
    }

    #[test]
    fn extract_error_message_returns_none_for_non_json() {
        assert!(extract_error_message("<html>403 Forbidden</html>").is_none());
        assert!(extract_error_message("").is_none());
    }

    #[test]
    fn credential_restricted_message_uses_detail_when_present() {
        let m = credential_restricted_message(r#"{"error":{"message":"only Claude Code"}}"#);
        assert!(m.contains("403"));
        assert!(m.contains("only Claude Code"));
    }

    #[test]
    fn credential_restricted_message_has_fallback() {
        let m = credential_restricted_message("not json");
        assert!(m.contains("403"));
        assert!(m.to_lowercase().contains("re-login") || m.to_lowercase().contains("restricted"));
    }

    #[test]
    fn drops_bucket_when_utilization_is_missing() {
        let bucket = RawBucket {
            utilization: None,
            resets_at: Some("2026-05-24T00:00:00Z".to_string()),
        }
        .into_bucket();

        assert!(bucket.is_none());
    }
}

/// 403 レスポンスボディから人間可読なメッセージを作る。生の `HTTP 403` ではなく
/// 「何が起きたか・どうすればよいか」が分かる文言を tray tooltip / popover に出すため。
fn credential_restricted_message(body: &str) -> String {
    match extract_error_message(body) {
        Some(detail) => format!("Not authorized for the usage API (403): {detail}"),
        None => "Not authorized for the usage API (403). This token may be restricted to Claude Code — re-login via `claude` / Claude Desktop, or this credential class is blocked.".to_string(),
    }
}

/// API のエラーボディ (`{"error":{"message":...}}` か `{"message":...}`) からメッセージを抜く。
/// JSON でない / 該当フィールドが無ければ `None`。長すぎる本文は 200 文字に丸める。
fn extract_error_message(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let msg = v
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .or_else(|| v.get("message").and_then(|m| m.as_str()))?
        .trim();
    if msg.is_empty() {
        return None;
    }
    Some(msg.chars().take(200).collect())
}

pub async fn fetch_usage(access_token: &str) -> Result<UsageSnapshot, ApiError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| ApiError::Network(e.to_string()))?;

    let resp = client
        .get(USAGE_URL)
        .bearer_auth(access_token)
        .header("anthropic-beta", BETA_HEADER)
        .header("user-agent", CLAUDE_CODE_USER_AGENT)
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;

    match resp.status().as_u16() {
        200 => {}
        401 => return Err(ApiError::Unauthorized),
        403 => {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiError::CredentialRestricted {
                message: credential_restricted_message(&body),
            });
        }
        429 => {
            let retry_after_secs = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());
            return Err(ApiError::RateLimited { retry_after_secs });
        }
        code => return Err(ApiError::Http(code)),
    }

    let raw: RawUsage = resp
        .json()
        .await
        .map_err(|e| ApiError::Decode(e.to_string()))?;

    Ok(UsageSnapshot {
        five_hour: raw.five_hour.and_then(RawBucket::into_bucket),
        seven_day: raw.seven_day.and_then(RawBucket::into_bucket),
        seven_day_sonnet: raw.seven_day_sonnet.and_then(RawBucket::into_bucket),
        fetched_at: Utc::now(),
    })
}
