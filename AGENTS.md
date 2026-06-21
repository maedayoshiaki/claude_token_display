# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

**token_display** — Claude Max プランの **使用枠**（5時間セッション %、週間 %、Sonnet 週間 %、各リセット時刻）を、macOS のメニューバー / Windows のシステムトレイに常時表示する常駐型アプリ。

- 表示内容: 設定ページ（claude.ai/settings/usage）に出るプラン使用率と、各バケットのリセット時刻
- データ源: **Anthropic OAuth usage API** (`GET https://api.anthropic.com/api/oauth/usage`)
- 認証: Claude Code が OS の keystore に保存している OAuth access_token を流用
- スタック: **Tauri 2.x** (Rust backend + 素の HTML/CSS/JS frontend) / 単一コードベースで Mac/Windows 両対応

### 参考にした実装

macOS 向けの Swift native 実装。Keychain サービス名・API URL・ヘッダ・レスポンス DTO はこれを参考にしている。我々の差分は **Tauri による Win 対応** と **frontend を Web 化** したこと。

## 進捗トラッカー

進行中タスクと完了履歴は [plans.md](./plans.md) と [status.md](./status.md) で管理する。新しい Phase を立てるときは plans.md にテンプレートを追加し、終わったら status.md に吸収する。

過去 Phase の要約:

- Phase 0〜2: 技術選定 (Tauri 2.x) + Keychain / OAuth Usage API クライアント + 3 バケット表示の初期実装 (完了)
- Phase 3: ユーザー作業 (npm install, 起動確認, 初回 commit/push)
- Phase 4: Windows 対応 (`.credentials.json` 読み取り、トレイ tooltip フォールバック) (完了)
- Phase 5: 配布枠組み (icon, GitHub Actions, README) — アイコン差し替えと初回タグは未完了

## リリース運用

```bash
git tag v0.1.0
git push origin v0.1.0
```

→ GitHub Actions が走り、macOS (aarch64 + x86_64) と Windows のバイナリを Draft Release に添付する。GitHub の Releases 画面でドラフトを編集 → publish で公開。

icon を変えるとき:
```bash
# assets/icon-source.png を 1024x1024 PNG に差し替え
npm run icon
git add src-tauri/icons/ assets/
git commit -m "Update app icon"
```

## アーキテクチャ

```
┌─────────────────────────────────────────────────────┐
│ Tray Icon (Tauri)                                   │ ← 例: "43% · 17%"
│   ↓ click                                           │
│ Popover Window (HTML/CSS/JS, no bundler)            │ ← 3バケットの詳細
└──────────────▲──────────────────────────────────────┘
               │ invoke("get_usage")  /  event("usage-updated")
               │
┌──────────────┴──────────────────────────────────────┐
│ Rust backend                                        │
│   ├ keychain.rs : OS keystore 読み取り → access_token │
│   ├ api.rs      : GET /api/oauth/usage              │
│   ├ tray.rs     : トレイ & 5分ポーラ                │
│   └ lib.rs      : Tauri コマンド + 起動シーケンス   │
└─────────────────────────────────────────────────────┘
                  │ Authorization: Bearer <token>
                  ▼
        https://api.anthropic.com/api/oauth/usage
```

### OAuth Usage API レスポンス

```jsonc
{
  "five_hour":         { "utilization": 43.0, "resets_at": "2026-05-23T17:00:00Z" },
  "seven_day":         { "utilization": 17.0, "resets_at": "2026-05-26T09:00:00Z" },
  "seven_day_sonnet":  { "utilization": ...,  "resets_at": "..." }
}
```

- `utilization` は **0〜100 の整数 or 浮動小数**（100超過もありうる）。Rust 側で `/100.0` して 0.0〜1.0+ に正規化。
- `resets_at` は ISO 8601。
- Headers 必須: `Authorization: Bearer <token>` と `anthropic-beta: oauth-2025-04-20`。

### Keychain / Credential Manager のレコード形式

サービス名 `Claude Code-credentials`、アカウント名は実行ユーザー名（`$USER`）。値は JSON:

```jsonc
{ "claudeAiOauth": { "accessToken": "..." /* + 他のフィールド */ } }
```

`keyring` crate v3 は macOS Keychain と Windows Credential Manager の両方を抽象化するため、**同じコードで両 OS 対応** できる見込み。Linux は今回対象外（必要なら Secret Service バックエンド利用可）。

### Claude Desktop フォールバック (`claude_desktop.rs`)

Claude Code CLI が未ログインでも、**Claude Desktop にログイン済みなら** そのトークンを使う。`fetch_claude` は CLI keystore → Desktop の順でトークンを探す。使用枠は全サーフェス共有プールなので、取れる accessToken は同種・表示値も同じ。

- 設定ファイル (Windows はインストール形態で場所が変わる。`desktop_data_dir` が `config.json` 実在の最初の候補を選ぶ):
  - Windows 旧 .exe (NSIS/Squirrel) 版: `%APPDATA%\Claude\config.json`
  - **Windows Microsoft Store / MSIX 版**: `%APPDATA%` がパッケージ専用フォルダにリダイレクトされ、実体は `%LOCALAPPDATA%\Packages\Claude_<publisherハッシュ>\LocalCache\Roaming\Claude\config.json` (例: `Claude_pzs8sxrjxfjjc`)。`%LOCALAPPDATA%\Packages` 配下を `Claude` 始まりで列挙して候補化する。**実機 (Store 版 v1.14271) で end-to-end 確認済み: 取得トークンで `/api/oauth/usage` が 200 を返し使用率を取得**。
  - macOS: `~/Library/Application Support/Claude/config.json` (Mac App Store のサンドボックス版が出たら `~/Library/Containers/.../Data/...` への同種リダイレクト対応が要る — 現状は直 DL .dmg 版のみ)
- トークンは `oauth:tokenCacheV2`（無ければ `oauth:tokenCache`）に base64。先頭 `v10` = Electron safeStorage / Chromium os_crypt（**Chrome の Cookie 暗号化と同一**。`Local State` の鍵プレフィックスは `DPAPI` で、Chrome 127+ の App-Bound Encryption ではない＝従来 DPAPI で復号可、を実機確認）。
  - Windows: `Local State` の `os_crypt.encrypted_key`（先頭 `DPAPI`）を `CryptUnprotectData` で 32B 鍵に → AES-256-GCM（`v10`+nonce12+ct+tag16）。
  - macOS: Keychain の generic password `"Claude Safe Storage"` を PBKDF2-HMAC-SHA1(salt="saltysalt", 1003, 16) → AES-128-CBC（IV=0x20×16, PKCS7）。
- 復号後の平文 JSON 形 (実機 Windows で確認済み):
  - `oauth:tokenCacheV2` は **エントリのマップ**。キーは `"<clientId>:<orgId>:<audience>:<scopes>"`、値は `{ "token": "sk-ant-...", "refreshToken": ..., "expiresAt": <ms>, "subscriptionType": ..., "rateLimitTier": ... }`。**アクセストークンのフィールド名は `accessToken` ではなく `token`**。複数クライアント (例: Claude Code の `9d1c250a-...` と Desktop 自身) のエントリが並ぶことがある。
  - 旧 `oauth:tokenCache` / `.credentials.json` は `{ "claudeAiOauth": { "accessToken": ... } }`。
  - パーサ ([`parse_access_token`]) は `claudeAiOauth.accessToken` → 直下 `accessToken` → V2 マップ (`api.anthropic.com` + `user:profile` かつ未期限切れ・期限が新しいエントリの `token`) → 再帰探索 → 素の `sk-ant-...` の順で寛容に取り出す。

### 既知の制約 / リスク

1. **API は非公開仕様**。Anthropic が変更すれば壊れる。
2. **Keychain ダイアログ**: 初回起動時に macOS が「token_display が "Claude Code-credentials" にアクセスしようとしています」とダイアログを出す。「常に許可」しないと毎回出る。Desktop フォールバック時は `"Claude Safe Storage"` でも同様のダイアログが出る。
3. **トークン有効期限**: access_token が切れたら 401 が返る。本実装は refresh_token を使った更新を行っていない（毎ポーリングでディスクから読み直すので、CLI/Desktop 本体が更新したトークンは自動で拾う）。
   - **書き換え中の窓**: Claude Code / Desktop はトークン更新時に credential ファイルを削除→再作成で書き換える。その一瞬に読みに行くと `NotFound` / 部分書き込みでのパース・復号失敗になり、失敗が次ポーリング (5分) までキャッシュされて「ログインが見つからない」が出続ける。対策として `read_with_retry` (lib.rs) で NotFound / Decode / Decrypt を transient とみなし 50ms×3 リトライする (CLI・Desktop 両経路に適用)。
4. **rate limit**: 同 API への過剰アクセスは 429 になる可能性。`/api/oauth/usage` は `User-Agent: claude-code/<ver>` が無いと積極的に 429 になるため付与している。5分ポーリングは問題ない想定。
5. **規約 (ToS) のグレー**: Anthropic は消費者 OAuth トークンの第三者ツール利用を「Claude Code / claude.ai 以外は不可」と明文化（2026/2、credential 基準・read-only 免除なし）。ただし実摘発は推論アービトラージ（opencode/OpenClaw 等）が対象で、read-only 使用量モニタの BAN 事例は確認されていない。Desktop トークンは現行 CLI トークンと同クラスのため追加リスクはほぼ無い。claude.ai Cookie 経路は scraping 条項が上乗せされ A より重いので**採用しない**。

## よく使うコマンド

```bash
# 依存インストール
npm install

# 開発起動
npm run tauri dev

# 更新通知のテスト (リリースを増やさずに「更新あり」を再現; dev ビルドのみ有効)
# 既存の公開リリース (v0.4.2 等) が新版として通知される。
$env:TOKEN_DISPLAY_FAKE_VERSION="0.0.1"; npm run tauri dev   # Windows PowerShell
# macOS/Linux: TOKEN_DISPLAY_FAKE_VERSION=0.0.1 npm run tauri dev

# 本番ビルド
npm run tauri build

# Rust 単体テスト
cd src-tauri && cargo test
```

## Git / GitHub

ローカル設定:
```
user.name  = <your name>
user.email = <your email>
```
push 先: `https://github.com/OWNER/REPO.git`

## 設計判断のメモ

- **OAuth API 方式**を選んだ理由: Webview 埋め込み・手動トークン貼り付けより自動度が高く、参考実装で実績があるため。
- **frontend をバンドラレス**にした理由: 単一 HTML/JS で十分。Vite を入れると tauri dev の起動が重くなる。`withGlobalTauri: true` で `window.__TAURI__` 経由で API を叩く。
- **ポーリング 5分**: バケットは時間単位で動くので 30秒では細かすぎる。手動 refresh ボタンも用意。
- **トレイ title 表記**: macOS は `set_title` がメニューバーに反映される。Windows のシステムトレイは title 非対応のため、同じ文字列を tooltip にもセット。
