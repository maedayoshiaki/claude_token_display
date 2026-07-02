//! Codex (OpenAI ChatGPT plan) の OAuth トークン読み取り + Usage API クライアント。
//!
//! - 認証ファイル: `${CODEX_HOME}/auth.json` または `~/.codex/auth.json` (JSON)
//!   `{ "tokens": { "access_token": "...", "account_id": "..." (optional) } }`
//! - Usage API: `GET https://chatgpt.com/backend-api/wham/usage`
//!     Header: `Authorization: Bearer <access_token>` (+ optional `ChatGPT-Account-Id`)
//!     Response: `rate_limit.{primary_window,secondary_window}.{used_percent,reset_at,limit_window_seconds}`
//!     `reset_at` は epoch seconds (整数)。Claude の ISO8601 と違うので変換が要る。

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

use crate::api::{ApiError, Bucket, UsageSnapshot};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

#[derive(thiserror::Error, Debug)]
pub enum CodexAuthError {
    #[error("Codex credentials not found. Have you logged in via the `codex` CLI?")]
    NotFound,

    #[error("Failed to read Codex credentials file: {0}")]
    Access(String),

    #[error("Codex credentials file is not valid JSON: {0}")]
    Decode(String),

    #[error("Codex credentials are missing tokens.access_token")]
    EmptyToken,
}

#[derive(Clone, Debug, Default)]
pub struct CodexCredentials {
    pub access_token: String,
    pub account_id: Option<String>,
}

#[derive(Deserialize)]
struct AuthFile {
    tokens: Option<TokensSection>,
}

#[derive(Deserialize)]
struct TokensSection {
    access_token: Option<String>,
    account_id: Option<String>,
}

pub fn read_credentials() -> Result<CodexCredentials, CodexAuthError> {
    let path = codex_auth_path()?;
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(CodexAuthError::NotFound)
        }
        Err(e) => {
            return Err(CodexAuthError::Access(format!(
                "{}: {}",
                path.display(),
                e
            )))
        }
    };
    let parsed: AuthFile =
        serde_json::from_str(&raw).map_err(|e| CodexAuthError::Decode(e.to_string()))?;
    let tokens = parsed.tokens.ok_or(CodexAuthError::EmptyToken)?;
    let access_token = tokens
        .access_token
        .filter(|t| !t.is_empty())
        .ok_or(CodexAuthError::EmptyToken)?;
    Ok(CodexCredentials {
        access_token,
        account_id: tokens.account_id.filter(|s| !s.is_empty()),
    })
}

fn codex_auth_path() -> Result<std::path::PathBuf, CodexAuthError> {
    if let Some(home) = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
        return Ok(std::path::PathBuf::from(home).join("auth.json"));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| {
            CodexAuthError::Access("could not determine Codex config directory".into())
        })?;
    Ok(std::path::PathBuf::from(home)
        .join(".codex")
        .join("auth.json"))
}

#[derive(Deserialize, Debug)]
struct RawWindow {
    used_percent: Option<f64>,
    reset_at: Option<i64>,
    #[serde(default)]
    #[allow(dead_code)]
    limit_window_seconds: Option<i64>,
}

#[derive(Deserialize, Debug, Default)]
struct RawRateLimit {
    primary_window: Option<RawWindow>,
    secondary_window: Option<RawWindow>,
}

#[derive(Deserialize, Debug)]
struct RawUsage {
    #[serde(default)]
    rate_limit: Option<RawRateLimit>,
}

impl RawWindow {
    fn into_bucket(self) -> Option<Bucket> {
        let used = self.used_percent?;
        let resets_at = self.reset_at.and_then(epoch_seconds_to_datetime);
        Some(Bucket {
            utilization: used / 100.0,
            resets_at,
        })
    }
}

fn epoch_seconds_to_datetime(secs: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_opt(secs, 0).single()
}

pub async fn fetch_usage(creds: &CodexCredentials) -> Result<UsageSnapshot, ApiError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| ApiError::Network(e.to_string()))?;

    let mut req = client
        .get(USAGE_URL)
        .bearer_auth(&creds.access_token)
        .header("Accept", "application/json")
        .header("User-Agent", "token_display");
    if let Some(account_id) = &creds.account_id {
        req = req.header("ChatGPT-Account-Id", account_id);
    }

    // 実際に usage API を叩くのでアクセス数として記録する (設定画面の注意書き用)。
    crate::record_api_access();
    let resp = req
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;

    match resp.status().as_u16() {
        200 => {}
        401 | 403 => return Err(ApiError::Unauthorized),
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

    let rate_limit = raw.rate_limit.unwrap_or_default();
    Ok(UsageSnapshot {
        five_hour: rate_limit.primary_window.and_then(RawWindow::into_bucket),
        seven_day: rate_limit.secondary_window.and_then(RawWindow::into_bucket),
        seven_day_sonnet: None,
        fetched_at: Utc::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_with_used_percent_only_yields_bucket() {
        let bucket = RawWindow {
            used_percent: Some(42.0),
            reset_at: None,
            limit_window_seconds: None,
        }
        .into_bucket()
        .expect("utilization only window keeps bucket");
        assert!((bucket.utilization - 0.42).abs() < 1e-9);
        assert!(bucket.resets_at.is_none());
    }

    #[test]
    fn epoch_seconds_converts_to_utc_datetime() {
        let dt = epoch_seconds_to_datetime(0).expect("epoch 0 is valid");
        assert_eq!(dt.timestamp(), 0);
    }

    #[test]
    fn window_without_used_percent_is_dropped() {
        let bucket = RawWindow {
            used_percent: None,
            reset_at: Some(1_700_000_000),
            limit_window_seconds: Some(300),
        }
        .into_bucket();
        assert!(bucket.is_none());
    }
}
