//! Claude Desktop (Electron アプリ) がローカルに保存している OAuth access_token を読む。
//!
//! Claude Code CLI 未ログインでも Claude Desktop にログイン済みなら使用枠を出せるようにするための
//! フォールバック源。使用枠は全サーフェス共有プールなので、ここで取れる accessToken を
//! 既存の `api::fetch_usage` に渡せば CLI と同じ数字が得られる。
//!
//! ## 保存場所と暗号化方式 (Chromium os_crypt = Chrome の Cookie と同一)
//!
//! - 設定ファイル:
//!   - Windows (旧 .exe 版): `%APPDATA%\Claude\config.json`
//!   - Windows (Microsoft Store / MSIX 版): `%APPDATA%` がパッケージ専用フォルダに
//!     リダイレクトされ、実体は
//!     `%LOCALAPPDATA%\Packages\<PackageFamilyName>\LocalCache\Roaming\Claude\config.json`。
//!     PackageFamilyName は `Claude_<publisherハッシュ>` (例: `Claude_pzs8sxrjxfjjc`)。
//!   - macOS:   `~/Library/Application Support/Claude/config.json`
//! - トークンは `oauth:tokenCacheV2` (無ければ `oauth:tokenCache`) に base64 文字列で入る。
//!   先頭3バイトは `v10` (Electron safeStorage / os_crypt の版マーカー)。
//! - 復号:
//!   - Windows: AES-256-GCM。鍵は `%APPDATA%\Claude\Local State` の
//!     `os_crypt.encrypted_key` (base64, 先頭 "DPAPI") を DPAPI (`CryptUnprotectData`) で
//!     アンラップした 32 バイト。暗号文 = `v10`(3) + nonce(12) + ciphertext + tag(16)。
//!   - macOS: AES-128-CBC。鍵は Keychain の generic password `"Claude Safe Storage"` を
//!     PBKDF2-HMAC-SHA1(salt="saltysalt", iterations=1003, dkLen=16) したもの。
//!     IV = 0x20 を 16 個、PKCS#7 パディング。暗号文 = `v10`(3) + ciphertext。
//!
//! 復号後の平文は JSON。形は Claude Code の `.credentials.json` と同じ
//! `{ "claudeAiOauth": { "accessToken": ... } }` の想定だが、`oauth:tokenCacheV2` の
//! 平文スキーマは実機で未確認のため、寛容に複数の形を試す ([`parse_access_token`])。

#[derive(thiserror::Error, Debug)]
pub enum DesktopError {
    #[error("Claude Desktop login not found. Install and log in to Claude Desktop, or use the `claude` CLI.")]
    NotFound,

    #[error("Failed to read Claude Desktop config: {0}")]
    Access(String),

    #[error("Claude Desktop config is not valid JSON: {0}")]
    Decode(String),

    #[error("Claude Desktop config has no oauth token cache")]
    NoTokenCache,

    #[error("Failed to decrypt Claude Desktop token: {0}")]
    Decrypt(String),

    #[error("Decrypted Claude Desktop token has no accessToken")]
    EmptyToken,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[allow(dead_code)]
pub fn read_access_token() -> Result<String, DesktopError> {
    read_credentials().map(|c| c.access_token)
}

#[derive(Clone, Debug)]
pub struct DesktopCredential {
    pub access_token: String,
    pub organization_uuid: Option<String>,
    pub subscription_type: Option<String>,
    pub rate_limit_tier: Option<String>,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub fn read_credentials() -> Result<DesktopCredential, DesktopError> {
    // Claude Desktop はトークン更新時に config.json を削除→再作成で書き換える。
    // その一瞬に読みに行くと NotFound / 部分書き込みによるパース・復号失敗になりうるので
    // transient とみなしてリトライする。
    crate::read_with_retry(read_credentials_once, is_transient)
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn read_credentials_once() -> Result<DesktopCredential, DesktopError> {
    let config = read_config_json()?;
    let cipher_b64 = extract_token_cache(&config)?;
    let plaintext = decrypt_os_crypt(&cipher_b64)?;
    parse_credential(&plaintext)
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn is_transient(e: &DesktopError) -> bool {
    // 書き換え途中のファイルに当たったときに出るもの。NoTokenCache / EmptyToken /
    // Access は「ファイルはあるが中身が無い/読めない」= 恒久的なのでリトライしない。
    matches!(
        e,
        DesktopError::NotFound | DesktopError::Decode(_) | DesktopError::Decrypt(_)
    )
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
#[allow(dead_code)]
pub fn read_access_token() -> Result<String, DesktopError> {
    Err(DesktopError::NotFound)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn read_credentials() -> Result<DesktopCredential, DesktopError> {
    Err(DesktopError::NotFound)
}

// ---- config.json / Local State の読み取り (パスはプラットフォーム依存) ----

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn read_config_json() -> Result<String, DesktopError> {
    let path = config_path()?;
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(DesktopError::NotFound),
        Err(e) => Err(DesktopError::Access(format!("{}: {}", path.display(), e))),
    }
}

#[cfg(target_os = "windows")]
fn config_path() -> Result<std::path::PathBuf, DesktopError> {
    Ok(desktop_data_dir()?.join("config.json"))
}

/// Claude Desktop の data ディレクトリ (中に `config.json` / `Local State` がある) を解決する。
/// インストール形態で場所が変わる:
///  - 旧 .exe (NSIS/Squirrel) 版: `%APPDATA%\Claude`
///  - Microsoft Store / MSIX 版: `%APPDATA%` がパッケージ専用フォルダにリダイレクトされ、
///    実体は `%LOCALAPPDATA%\Packages\<PackageFamilyName>\LocalCache\Roaming\Claude`。
///
/// `config.json` が実在する最初の候補を返す。`Local State` も同じディレクトリから読むことで、
/// os_crypt 鍵と暗号文の組が必ず一致するようにする。
#[cfg(target_os = "windows")]
fn desktop_data_dir() -> Result<std::path::PathBuf, DesktopError> {
    let appdata = std::env::var_os("APPDATA")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from);
    let local_appdata = std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from);
    let list_packages = |packages: &std::path::Path| -> Vec<String> {
        match std::fs::read_dir(packages) {
            Ok(rd) => rd
                .flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .collect(),
            Err(_) => Vec::new(),
        }
    };
    windows_candidate_dirs(appdata.as_deref(), local_appdata.as_deref(), list_packages)
        .into_iter()
        .find(|dir| dir.join("config.json").exists())
        .ok_or(DesktopError::NotFound)
}

/// data ディレクトリの候補を優先順 (旧 .exe 版 → MSIX 版) に並べる純粋関数。
/// `%LOCALAPPDATA%\Packages` 配下の列挙はテスト用にクロージャで注入する。
#[cfg(target_os = "windows")]
fn windows_candidate_dirs(
    appdata: Option<&std::path::Path>,
    local_appdata: Option<&std::path::Path>,
    list_packages: impl Fn(&std::path::Path) -> Vec<String>,
) -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(appdata) = appdata {
        dirs.push(appdata.join("Claude"));
    }
    if let Some(local) = local_appdata {
        let packages = local.join("Packages");
        for name in list_packages(&packages) {
            // PackageFamilyName が "Claude" で始まるもの (`Claude_<hash>`) だけを候補にする。
            if name.starts_with("Claude") {
                dirs.push(
                    packages
                        .join(name)
                        .join("LocalCache")
                        .join("Roaming")
                        .join("Claude"),
                );
            }
        }
    }
    dirs
}

#[cfg(target_os = "macos")]
fn config_path() -> Result<std::path::PathBuf, DesktopError> {
    let home = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .ok_or_else(|| DesktopError::Access("HOME is not set".into()))?;
    Ok(std::path::PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("Claude")
        .join("config.json"))
}

// ---- 共通 (純粋関数) ----

/// `oauth:tokenCacheV2` を優先し、無ければ `oauth:tokenCache` を返す。
fn extract_token_cache(config: &str) -> Result<String, DesktopError> {
    let v: serde_json::Value =
        serde_json::from_str(config).map_err(|e| DesktopError::Decode(e.to_string()))?;
    for key in ["oauth:tokenCacheV2", "oauth:tokenCache"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            if !s.is_empty() {
                return Ok(s.to_string());
            }
        }
    }
    Err(DesktopError::NoTokenCache)
}

/// os_crypt の版プレフィックス (`v10` / `v11`) を確認して剥がす。
fn strip_version_prefix(blob: &[u8]) -> Result<&[u8], DesktopError> {
    if blob.len() >= 3 && (&blob[..3] == b"v10" || &blob[..3] == b"v11") {
        Ok(&blob[3..])
    } else {
        Err(DesktopError::Decrypt(format!(
            "unexpected os_crypt prefix: {:?}",
            &blob[..blob.len().min(3)]
        )))
    }
}

/// 復号後の平文から accessToken を取り出す。取りうる形:
///
/// - Claude Code `.credentials.json` / 旧 `oauth:tokenCache`:
///   `{ "claudeAiOauth": { "accessToken": "..." } }` または直下 `accessToken`
/// - Claude Desktop `oauth:tokenCacheV2`: **エントリのマップ**。キーは
///   `"<clientId>:<orgId>:<audience>:<scopes>"`、値は
///   `{ "token": "sk-ant-...", "refreshToken": ..., "expiresAt": <ms>, "subscriptionType": ..., "rateLimitTier": ... }`。
///   アクセストークンのフィールド名は `accessToken` ではなく **`token`**。
///   `/api/oauth/usage` は `user:profile` スコープ + audience `api.anthropic.com` が要るので、
///   その条件に合い・期限が新しいエントリを優先して選ぶ。
#[allow(dead_code)]
fn parse_access_token(plaintext: &[u8]) -> Result<String, DesktopError> {
    parse_credential(plaintext).map(|c| c.access_token)
}

fn parse_credential(plaintext: &[u8]) -> Result<DesktopCredential, DesktopError> {
    let s = std::str::from_utf8(plaintext)
        .map_err(|e| DesktopError::Decode(e.to_string()))?
        .trim();

    if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
        if let Some(tok) = v
            .get("claudeAiOauth")
            .and_then(|o| o.get("accessToken"))
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
        {
            return Ok(DesktopCredential {
                access_token: tok.to_string(),
                organization_uuid: None,
                subscription_type: None,
                rate_limit_tier: None,
            });
        }
        if let Some(tok) = v
            .get("accessToken")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
        {
            return Ok(DesktopCredential {
                access_token: tok.to_string(),
                organization_uuid: None,
                subscription_type: None,
                rate_limit_tier: None,
            });
        }
        if let Some(cred) = best_v2_credential(&v, now_epoch_ms()) {
            return Ok(cred);
        }
        if let Some(tok) = find_oauth_token(&v) {
            return Ok(DesktopCredential {
                access_token: tok,
                organization_uuid: None,
                subscription_type: None,
                rate_limit_tier: None,
            });
        }
    }

    // JSON でない / 見つからない場合: 素のトークン文字列か?
    if s.starts_with("sk-ant-") && !s.chars().any(|c| c.is_whitespace()) {
        return Ok(DesktopCredential {
            access_token: s.to_string(),
            organization_uuid: None,
            subscription_type: None,
            rate_limit_tier: None,
        });
    }

    Err(DesktopError::EmptyToken)
}

/// `oauth:tokenCacheV2` のマップから最良のエントリの `token` を選ぶ。
/// 優先度 (高い順):
///   1. audience が `api.anthropic.com` — usage API が受理する前提条件 (他 audience は弾かれる)
///   2. 期限切れでない (`expiresAt > now`) — 期限切れの `user:profile` 付きより未期限切れを優先し、
///      確実に失敗する 401/403 の無駄打ちを避ける (`user:profile` を最上位重みにしない)
///   3. スコープに `user:profile` を含む — 同条件内のタイブレーク
///   4. `expiresAt` が新しい
#[allow(dead_code)]
fn best_v2_token(v: &serde_json::Value, now_ms: i64) -> Option<String> {
    best_v2_credential(v, now_ms).map(|c| c.access_token)
}

fn best_v2_credential(v: &serde_json::Value, now_ms: i64) -> Option<DesktopCredential> {
    let obj = v.as_object()?;
    let mut best: Option<((i32, i32, i32, i64), DesktopCredential)> = None;
    for (key, entry) in obj {
        let entry = match entry.as_object() {
            Some(e) => e,
            None => continue,
        };
        let token = match entry
            .get("token")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
        {
            Some(t) => t.to_string(),
            None => continue,
        };
        let expires = entry.get("expiresAt").and_then(|e| e.as_i64()).unwrap_or(0);
        let audience = key.contains("api.anthropic.com");
        let unexpired = expires > now_ms;
        let profile = key.contains("user:profile");
        let score = (audience as i32, unexpired as i32, profile as i32, expires);
        let credential = DesktopCredential {
            access_token: token,
            organization_uuid: organization_uuid_from_v2_key(key),
            subscription_type: entry
                .get("subscriptionType")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            rate_limit_tier: entry
                .get("rateLimitTier")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        };
        let better = match &best {
            None => true,
            Some((best_score, _)) => score > *best_score,
        };
        if better {
            best = Some((score, credential));
        }
    }
    best.map(|(_, c)| c)
}

fn organization_uuid_from_v2_key(key: &str) -> Option<String> {
    key.split(':')
        .nth(1)
        .filter(|s| !s.is_empty() && !s.contains("anthropic.com"))
        .map(str::to_string)
}

/// 任意の深さで最初に見つかった非空の `accessToken`、または `sk-ant-` で始まる
/// `token` 文字列を返す (スキーマが将来変わっても拾えるようにする保険)。
fn find_oauth_token(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(s) = map
                .get("accessToken")
                .and_then(|t| t.as_str())
                .filter(|t| !t.is_empty())
            {
                return Some(s.to_string());
            }
            if let Some(s) = map
                .get("token")
                .and_then(|t| t.as_str())
                .filter(|t| t.starts_with("sk-ant-"))
            {
                return Some(s.to_string());
            }
            for child in map.values() {
                if let Some(found) = find_oauth_token(child) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(arr) => arr.iter().find_map(find_oauth_token),
        _ => None,
    }
}

fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---- Windows: DPAPI + AES-256-GCM ----

#[cfg(target_os = "windows")]
fn decrypt_os_crypt(cipher_b64: &str) -> Result<Vec<u8>, DesktopError> {
    use base64::Engine as _;

    let blob = base64::engine::general_purpose::STANDARD
        .decode(cipher_b64.trim())
        .map_err(|e| DesktopError::Decrypt(format!("token base64: {e}")))?;
    let body = strip_version_prefix(&blob)?;
    if body.len() < 12 + 16 {
        return Err(DesktopError::Decrypt("ciphertext too short".into()));
    }
    let (nonce, ct_and_tag) = body.split_at(12);

    let key = windows_os_crypt_key()?;

    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|_| DesktopError::Decrypt("invalid AES key length".into()))?;
    cipher
        .decrypt(Nonce::from_slice(nonce), ct_and_tag)
        .map_err(|_| DesktopError::Decrypt("AES-256-GCM decrypt failed".into()))
}

#[cfg(target_os = "windows")]
fn windows_os_crypt_key() -> Result<Vec<u8>, DesktopError> {
    use base64::Engine as _;

    let path = desktop_data_dir()?.join("Local State");
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(DesktopError::NotFound),
        Err(e) => return Err(DesktopError::Access(format!("{}: {}", path.display(), e))),
    };
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| DesktopError::Decode(e.to_string()))?;
    let enc_key_b64 = v
        .get("os_crypt")
        .and_then(|o| o.get("encrypted_key"))
        .and_then(|k| k.as_str())
        .ok_or_else(|| DesktopError::Decrypt("Local State has no os_crypt.encrypted_key".into()))?;
    let enc_key = base64::engine::general_purpose::STANDARD
        .decode(enc_key_b64)
        .map_err(|e| DesktopError::Decrypt(format!("key base64: {e}")))?;
    if enc_key.len() < 5 || &enc_key[..5] != b"DPAPI" {
        return Err(DesktopError::Decrypt(
            "Local State key missing DPAPI prefix".into(),
        ));
    }
    dpapi_unprotect(&enc_key[5..])
}

#[cfg(target_os = "windows")]
fn dpapi_unprotect(data: &[u8]) -> Result<Vec<u8>, DesktopError> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    let ok = unsafe {
        CryptUnprotectData(
            &in_blob,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            &mut out_blob,
        )
    };
    if ok == 0 {
        return Err(DesktopError::Decrypt(format!(
            "CryptUnprotectData failed: {}",
            std::io::Error::last_os_error()
        )));
    }

    let out =
        unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) }.to_vec();
    let _ = unsafe { LocalFree(out_blob.pbData as *mut core::ffi::c_void) };
    Ok(out)
}

// ---- macOS: Keychain passphrase + PBKDF2 + AES-128-CBC ----

#[cfg(target_os = "macos")]
fn decrypt_os_crypt(cipher_b64: &str) -> Result<Vec<u8>, DesktopError> {
    use base64::Engine as _;

    let blob = base64::engine::general_purpose::STANDARD
        .decode(cipher_b64.trim())
        .map_err(|e| DesktopError::Decrypt(format!("token base64: {e}")))?;
    let body = strip_version_prefix(&blob)?;

    let key = macos_os_crypt_key()?;
    let iv = [0x20u8; 16];

    use aes::Aes128;
    use cbc::cipher::block_padding::Pkcs7;
    use cbc::cipher::{BlockDecryptMut, KeyIvInit};
    let dec = cbc::Decryptor::<Aes128>::new_from_slices(&key, &iv)
        .map_err(|e| DesktopError::Decrypt(format!("key/iv: {e}")))?;
    dec.decrypt_padded_vec_mut::<Pkcs7>(body)
        .map_err(|_| DesktopError::Decrypt("AES-128-CBC decrypt failed".into()))
}

#[cfg(target_os = "macos")]
fn macos_os_crypt_key() -> Result<Vec<u8>, DesktopError> {
    use std::process::Command;

    // keychain.rs と同じく `security` CLI 経由 (security-framework を増やさない)。
    let out = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-w", "-s", "Claude Safe Storage"])
        .output()
        .map_err(|e| DesktopError::Access(e.to_string()))?;
    if !out.status.success() {
        return Err(match out.status.code() {
            Some(44) => DesktopError::NotFound, // errSecItemNotFound
            _ => DesktopError::Decrypt(format!(
                "keychain read failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )),
        });
    }
    let passphrase = String::from_utf8_lossy(&out.stdout);
    let passphrase = passphrase.trim_end_matches(['\n', '\r']);

    let mut key = vec![0u8; 16];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(passphrase.as_bytes(), b"saltysalt", 1003, &mut key);
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 一時診断: 実機の Desktop トークン取得が何を返すかを確認する (秘密値は出さない)。
    // 実行: cargo test desktop_read_smoke -- --ignored --nocapture
    #[test]
    #[ignore]
    fn desktop_read_smoke() {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        if let Ok(p) = config_path() {
            eprintln!("DESKTOP_READ: config={} exists={}", p.display(), p.exists());
        }
        match read_access_token() {
            Ok(t) => eprintln!(
                "DESKTOP_READ: OK len={} sk-ant-prefix={}",
                t.len(),
                t.starts_with("sk-ant-")
            ),
            Err(e) => eprintln!("DESKTOP_READ: ERR kind={e:?}"),
        }
    }

    #[test]
    fn token_cache_prefers_v2() {
        let cfg = r#"{"oauth:tokenCache":"OLD","oauth:tokenCacheV2":"NEW"}"#;
        assert_eq!(extract_token_cache(cfg).unwrap(), "NEW");
    }

    #[test]
    fn token_cache_falls_back_to_v1() {
        let cfg = r#"{"locale":"ja","oauth:tokenCache":"ONLYV1"}"#;
        assert_eq!(extract_token_cache(cfg).unwrap(), "ONLYV1");
    }

    #[test]
    fn token_cache_absent_errors() {
        let cfg = r#"{"locale":"ja"}"#;
        assert!(matches!(
            extract_token_cache(cfg),
            Err(DesktopError::NoTokenCache)
        ));
    }

    #[test]
    fn token_cache_empty_string_is_ignored() {
        let cfg = r#"{"oauth:tokenCacheV2":"","oauth:tokenCache":"V1"}"#;
        assert_eq!(extract_token_cache(cfg).unwrap(), "V1");
    }

    #[test]
    fn parse_credentials_json_shape() {
        let pt = br#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-abc","refreshToken":"x"}}"#;
        assert_eq!(parse_access_token(pt).unwrap(), "sk-ant-oat01-abc");
    }

    #[test]
    fn parse_top_level_access_token() {
        let pt = br#"{"accessToken":"sk-ant-oat01-top","expiresAt":123}"#;
        assert_eq!(parse_access_token(pt).unwrap(), "sk-ant-oat01-top");
    }

    #[test]
    fn parse_nested_access_token() {
        let pt = br#"{"wrapper":{"inner":{"accessToken":"sk-ant-oat01-deep"}}}"#;
        assert_eq!(parse_access_token(pt).unwrap(), "sk-ant-oat01-deep");
    }

    #[test]
    fn parse_desktop_v2_token_cache_map() {
        // 実機 oauth:tokenCacheV2 の形: キーが client:org:audience:scopes、
        // 値の token フィールドがアクセストークン。
        let future = now_epoch_ms() + 1_000_000;
        let pt = format!(
            r#"{{"9d1c:org-123:https://api.anthropic.com:user:inference user:profile":{{"token":"sk-ant-oat01-v2","refreshToken":"r","expiresAt":{future},"subscriptionType":"max","rateLimitTier":"tier_1"}}}}"#
        );
        assert_eq!(parse_access_token(pt.as_bytes()).unwrap(), "sk-ant-oat01-v2");
        let credential = parse_credential(pt.as_bytes()).unwrap();
        assert_eq!(credential.organization_uuid.as_deref(), Some("org-123"));
        assert_eq!(credential.subscription_type.as_deref(), Some("max"));
        assert_eq!(credential.rate_limit_tier.as_deref(), Some("tier_1"));
    }

    #[test]
    fn v2_prefers_unexpired_api_profile_entry() {
        let now = 1_000_000_000_000i64;
        let v: serde_json::Value = serde_json::from_str(
            r#"{
              "x:org:https://api.anthropic.com:user:inference":{"token":"sk-ant-no-profile","expiresAt":9999999999999},
              "y:org:https://api.anthropic.com:user:inference user:profile":{"token":"sk-ant-expired","expiresAt":1},
              "z:org:https://api.anthropic.com:user:inference user:profile":{"token":"sk-ant-good","expiresAt":9999999999999}
            }"#,
        )
        .unwrap();
        assert_eq!(best_v2_token(&v, now).unwrap(), "sk-ant-good");
    }

    #[test]
    fn v2_prefers_unexpired_over_expired_with_weaker_scope() {
        // 同じ api.anthropic.com audience。期限切れの user:profile 付きより、
        // 未期限切れ (profile 無し) を優先する (期限切れトークンの無駄打ち回避)。
        let now = 1_000_000_000_000i64;
        let v: serde_json::Value = serde_json::from_str(
            r#"{
              "a:org:https://api.anthropic.com:user:inference user:profile":{"token":"sk-ant-expired-profile","expiresAt":1},
              "b:org:https://api.anthropic.com:user:inference":{"token":"sk-ant-fresh-noprofile","expiresAt":9999999999999}
            }"#,
        )
        .unwrap();
        assert_eq!(best_v2_token(&v, now).unwrap(), "sk-ant-fresh-noprofile");
    }

    #[test]
    fn v2_requires_api_audience_over_unexpired_non_api() {
        // audience が api.anthropic.com でないトークンは usage API で弾かれるので、
        // たとえ未期限切れでも api.anthropic.com (期限切れ) を優先する。
        let now = 1_000_000_000_000i64;
        let v: serde_json::Value = serde_json::from_str(
            r#"{
              "a:org:https://api.anthropic.com:user:inference user:profile":{"token":"sk-ant-api-expired","expiresAt":1},
              "b:org:https://console.anthropic.com:user:inference user:profile":{"token":"sk-ant-nonapi-fresh","expiresAt":9999999999999}
            }"#,
        )
        .unwrap();
        assert_eq!(best_v2_token(&v, now).unwrap(), "sk-ant-api-expired");
    }

    #[test]
    fn v2_falls_back_to_expired_when_only_option() {
        let now = 9_000_000_000_000i64;
        let v: serde_json::Value = serde_json::from_str(
            r#"{"y:org:https://api.anthropic.com:user:inference user:profile":{"token":"sk-ant-old","expiresAt":1}}"#,
        )
        .unwrap();
        assert_eq!(best_v2_token(&v, now).unwrap(), "sk-ant-old");
    }

    #[test]
    fn parse_bare_token_string() {
        let pt = b"sk-ant-oat01-bare-token";
        assert_eq!(parse_access_token(pt).unwrap(), "sk-ant-oat01-bare-token");
    }

    #[test]
    fn parse_empty_access_token_is_rejected() {
        let pt = br#"{"claudeAiOauth":{"accessToken":""}}"#;
        assert!(matches!(parse_access_token(pt), Err(DesktopError::EmptyToken)));
    }

    #[test]
    fn parse_no_token_errors() {
        let pt = br#"{"something":"else"}"#;
        assert!(matches!(parse_access_token(pt), Err(DesktopError::EmptyToken)));
    }

    #[test]
    fn strip_v10_prefix() {
        let mut blob = b"v10".to_vec();
        blob.extend_from_slice(&[1, 2, 3, 4]);
        assert_eq!(strip_version_prefix(&blob).unwrap(), &[1, 2, 3, 4]);
    }

    #[test]
    fn strip_v11_prefix() {
        let mut blob = b"v11".to_vec();
        blob.extend_from_slice(&[9, 9]);
        assert_eq!(strip_version_prefix(&blob).unwrap(), &[9, 9]);
    }

    #[test]
    fn strip_bad_prefix_errors() {
        let blob = b"xxxdata";
        assert!(matches!(
            strip_version_prefix(blob),
            Err(DesktopError::Decrypt(_))
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_candidates_try_appdata_then_msix() {
        use std::path::{Path, PathBuf};
        let appdata = PathBuf::from(r"C:\Users\me\AppData\Roaming");
        let local = PathBuf::from(r"C:\Users\me\AppData\Local");
        // %LOCALAPPDATA%\Packages の中身を模擬。Claude 以外は無視されるべき。
        let list = |_p: &Path| {
            vec![
                "Microsoft.WindowsStore_8wekyb3d8bbwe".to_string(),
                "Claude_pzs8sxrjxfjjc".to_string(),
            ]
        };
        let dirs = windows_candidate_dirs(Some(&appdata), Some(&local), list);
        assert_eq!(dirs.len(), 2, "appdata + 1 MSIX package");
        assert_eq!(dirs[0], appdata.join("Claude"), "legacy .exe path first");
        assert_eq!(
            dirs[1],
            local
                .join("Packages")
                .join("Claude_pzs8sxrjxfjjc")
                .join("LocalCache")
                .join("Roaming")
                .join("Claude"),
            "MSIX redirected path second"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_candidates_skip_non_claude_packages() {
        use std::path::{Path, PathBuf};
        let local = PathBuf::from(r"C:\L");
        let list = |_p: &Path| vec!["NotClaudeApp_x".to_string(), "Foo".to_string()];
        let dirs = windows_candidate_dirs(None, Some(&local), list);
        assert!(dirs.is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_candidates_without_appdata_still_finds_msix() {
        use std::path::{Path, PathBuf};
        let local = PathBuf::from(r"C:\L");
        let list = |_p: &Path| vec!["Claude_abcde".to_string()];
        let dirs = windows_candidate_dirs(None, Some(&local), list);
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].ends_with(r"Claude_abcde\LocalCache\Roaming\Claude"));
    }
}
