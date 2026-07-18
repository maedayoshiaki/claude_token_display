//! Claude Code が保存している OAuth access_token を OS の keystore から取り出す。
//!
//! - macOS: `/usr/bin/security find-generic-password -a <user> -s "Claude Code-credentials" -w`
//!          を spawn する。直接 Security framework を叩くと ACL ダイアログが出ない
//!          ケースがあるため CLI 経由のほうが安定 (参考実装 token-checker と同じ方針)。
//! - Windows: Claude Code 公式の保存先 `%USERPROFILE%\.claude\.credentials.json`
//!            から読み取り。`CLAUDE_CONFIG_DIR` があればその配下を優先する。
//!
//! 値は JSON 文字列で、`claudeAiOauth.accessToken` を取り出す。

use serde::Deserialize;

const SERVICE_NAME: &str = "Claude Code-credentials";

#[derive(thiserror::Error, Debug)]
pub enum KeychainError {
    #[error("OS keystore access failed: {0}")]
    Access(String),

    #[error("Claude Code credentials not found. Have you logged in via the `claude` CLI?")]
    NotFound,

    #[error("Access to the Claude Code keychain item was denied. Re-run and choose \"Always Allow\" in the dialog.")]
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    AccessDenied,

    #[error("Keychain payload is not valid JSON: {0}")]
    Decode(String),

    #[error("Keychain payload has no claudeAiOauth.accessToken")]
    EmptyToken,
}

#[derive(Deserialize)]
struct Payload {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<OAuth>,
    #[serde(rename = "organizationUuid")]
    organization_uuid: Option<String>,
}

#[derive(Deserialize)]
struct OAuth {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<i64>,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
    #[serde(rename = "rateLimitTier")]
    rate_limit_tier: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ClaudeCodeCredential {
    pub access_token: String,
    /// アクセストークンの失効時刻 (epoch ms)。`.credentials.json` の
    /// `claudeAiOauth.expiresAt`。無ければ判定不能。
    pub expires_at: Option<i64>,
    pub organization_uuid: Option<String>,
    pub subscription_type: Option<String>,
    pub rate_limit_tier: Option<String>,
}

impl ClaudeCodeCredential {
    /// このトークンが (指定時刻 `now_ms` 時点で) 期限切れかどうか。
    ///
    /// Claude Code CLI を起動していない間 (= Claude Desktop のみ起動している間) は
    /// `.credentials.json` のトークンが更新されず、期限切れのまま残る。そのトークンで
    /// usage API を叩いても 401 (無効トークンの連続で 429 にもなりうる) になるだけなので、
    /// ローカルで期限切れと分かるなら Desktop トークンへ先にフォールバックするために使う。
    ///
    /// `margin_ms` はフェッチ往復中に失効する寸前のトークンを避けるための前倒し余裕。
    /// `expires_at` が無い (旧フォーマット等) 場合は判定不能として `false` を返す。
    pub fn is_expired_at(&self, now_ms: i64, margin_ms: i64) -> bool {
        match self.expires_at {
            Some(exp) => exp <= now_ms.saturating_add(margin_ms),
            None => false,
        }
    }
}

#[allow(dead_code)]
pub fn read_access_token() -> Result<String, KeychainError> {
    read_credentials().map(|c| c.access_token)
}

pub fn read_credentials() -> Result<ClaudeCodeCredential, KeychainError> {
    // Claude Code がトークン更新時に `.credentials.json` を書き換える瞬間に当たると
    // NotFound / パース失敗になりうるので transient リトライする。
    crate::read_with_retry(read_credentials_once, is_transient)
}

fn read_credentials_once() -> Result<ClaudeCodeCredential, KeychainError> {
    let raw = read_raw()?;
    parse_credential(&raw)
}

/// `.credentials.json` (`{ "claudeAiOauth": { "accessToken", "expiresAt", ... } }`) を
/// `ClaudeCodeCredential` に変換する純粋関数。
fn parse_credential(raw: &str) -> Result<ClaudeCodeCredential, KeychainError> {
    let payload: Payload =
        serde_json::from_str(raw).map_err(|e| KeychainError::Decode(e.to_string()))?;
    let oauth = payload.claude_ai_oauth.ok_or(KeychainError::EmptyToken)?;
    let access_token = oauth
        .access_token
        .filter(|t| !t.is_empty())
        .ok_or(KeychainError::EmptyToken)?;
    Ok(ClaudeCodeCredential {
        access_token,
        expires_at: oauth.expires_at,
        organization_uuid: payload.organization_uuid.filter(|s| !s.is_empty()),
        subscription_type: oauth.subscription_type.filter(|s| !s.is_empty()),
        rate_limit_tier: oauth.rate_limit_tier.filter(|s| !s.is_empty()),
    })
}

fn is_transient(e: &KeychainError) -> bool {
    matches!(e, KeychainError::NotFound | KeychainError::Decode(_))
}

#[cfg(target_os = "macos")]
fn read_raw() -> Result<String, KeychainError> {
    use std::process::Command;

    let user = whoami::username();
    let output = Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-a",
            &user,
            "-s",
            SERVICE_NAME,
            "-w",
        ])
        .output()
        .map_err(|e| KeychainError::Access(e.to_string()))?;

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    // 終了コードのざっくり分類
    // 44 = errSecItemNotFound, 51 = errSecAuthFailed (ACL拒否)
    match output.status.code() {
        Some(44) => Err(KeychainError::NotFound),
        Some(51) => Err(KeychainError::AccessDenied),
        Some(45) => Err(KeychainError::AccessDenied), // errSecInteractionNotAllowed
        _ => Err(KeychainError::Access(stderr.trim().to_string())),
    }
}

#[cfg(target_os = "windows")]
fn read_raw() -> Result<String, KeychainError> {
    // Claude Code 2.x on Windows stores OAuth credentials in this JSON file, not in
    // Windows Credential Manager. Keep the old Credential Manager lookup as a
    // fallback in case earlier installs used it.
    match read_raw_from_credentials_file() {
        Ok(s) => return Ok(s),
        Err(KeychainError::NotFound) => {}
        Err(e) => return Err(e),
    }

    read_raw_from_credential_manager()
}

#[cfg(target_os = "windows")]
fn read_raw_from_credentials_file() -> Result<String, KeychainError> {
    let path = claude_credentials_path()?;
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(KeychainError::NotFound),
        Err(e) => Err(KeychainError::Access(format!(
            "failed to read {}: {}",
            path.display(),
            e
        ))),
    }
}

#[cfg(target_os = "windows")]
fn claude_credentials_path() -> Result<std::path::PathBuf, KeychainError> {
    if let Some(config_dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return Ok(std::path::PathBuf::from(config_dir).join(".credentials.json"));
    }

    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| {
            KeychainError::Access("could not determine Claude Code config directory".into())
        })?;
    Ok(std::path::PathBuf::from(home)
        .join(".claude")
        .join(".credentials.json"))
}

#[cfg(target_os = "windows")]
fn read_raw_from_credential_manager() -> Result<String, KeychainError> {
    let user = whoami::username();
    let entry = keyring::Entry::new(SERVICE_NAME, &user)
        .map_err(|e| KeychainError::Access(e.to_string()))?;
    match entry.get_password() {
        Ok(s) => Ok(s),
        Err(keyring::Error::NoEntry) => Err(KeychainError::NotFound),
        Err(e) => Err(KeychainError::Access(e.to_string())),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn read_raw() -> Result<String, KeychainError> {
    Err(KeychainError::Access(
        "unsupported platform — only macOS / Windows supported".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_expires_at() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-x","expiresAt":1784120107123,"subscriptionType":"max"}}"#;
        let cred = parse_credential(raw).unwrap();
        assert_eq!(cred.access_token, "sk-ant-oat01-x");
        assert_eq!(cred.expires_at, Some(1784120107123));
        assert_eq!(cred.subscription_type.as_deref(), Some("max"));
    }

    #[test]
    fn missing_expires_at_is_none() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-x"}}"#;
        let cred = parse_credential(raw).unwrap();
        assert_eq!(cred.expires_at, None);
    }

    #[test]
    fn is_expired_at_true_when_past() {
        let cred = ClaudeCodeCredential {
            access_token: "t".into(),
            expires_at: Some(1_000),
            organization_uuid: None,
            subscription_type: None,
            rate_limit_tier: None,
        };
        assert!(cred.is_expired_at(2_000, 0));
    }

    #[test]
    fn is_expired_at_false_when_future() {
        let cred = ClaudeCodeCredential {
            access_token: "t".into(),
            expires_at: Some(1_000_000),
            organization_uuid: None,
            subscription_type: None,
            rate_limit_tier: None,
        };
        assert!(!cred.is_expired_at(2_000, 1_000));
    }

    #[test]
    fn is_expired_at_uses_margin() {
        // now=2000, margin=60000 → 62000 以内に切れるトークンは期限切れ扱い。
        let cred = ClaudeCodeCredential {
            access_token: "t".into(),
            expires_at: Some(50_000),
            organization_uuid: None,
            subscription_type: None,
            rate_limit_tier: None,
        };
        assert!(
            cred.is_expired_at(2_000, 60_000),
            "expires within the margin window"
        );
        assert!(
            !cred.is_expired_at(2_000, 1_000),
            "expires well beyond the margin"
        );
    }

    #[test]
    fn is_expired_at_unknown_expiry_is_not_expired() {
        let cred = ClaudeCodeCredential {
            access_token: "t".into(),
            expires_at: None,
            organization_uuid: None,
            subscription_type: None,
            rate_limit_tier: None,
        };
        assert!(!cred.is_expired_at(i64::MAX, 60_000));
    }
}
