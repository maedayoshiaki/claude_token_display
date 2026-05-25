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

### Phase 0: 設計と骨組み ✅
- [x] 技術選定 (Tauri 2.x)
- [x] git init + ローカル user.name/user.email を設定
- [x] CLAUDE.md / README.md / .gitignore / package.json / Cargo.toml / tauri.conf.json

### Phase 0.5: 要件再確認とピボット ✅
- [x] 初期はローカル JSONL 集計で実装 → 要件は「プラン使用枠の % と リセット時刻」と判明
- [x] 参考実装の解析: 「Keychain → /api/oauth/usage」が現実的だと確認
- [x] バックエンドを **OAuth API クライアント方式**に書き換え

### Phase 1: バックエンド ✅
- [x] `keychain.rs`: `keyring` crate で `service="Claude Code-credentials" / account=$USER` を読み、JSON から `claudeAiOauth.accessToken` を取り出し
- [x] `api.rs`: `reqwest` で `GET /api/oauth/usage` (Bearer + `anthropic-beta: oauth-2025-04-20`)、レスポンスを `Bucket { utilization, resets_at }` × 3 に整形
- [x] `lib.rs`: `get_usage` コマンドを公開、エラーは `FetchResult::Err { message }` で返す
- [x] `tray.rs`: 5分ごとにポーリング、メニューバー title を「43% · 17%」形式に更新

### Phase 2: フロントエンド ✅
- [x] 3バケットそれぞれを「% 使用済み + リセット時刻 + 横バー」で表示
- [x] 使用率に応じてバー色を変える (緑/黄/赤)
- [x] エラー時はエラーメッセージを表示
- [x] `usage-updated` イベントを listen して 5分おきに自動更新

### Phase 3: ユーザー作業フェーズ（**ユーザー担当**）👈 いまここ
- [x] Rust toolchain インストール
- [ ] `npm install`
- [ ] `npm run tauri dev` で起動確認、初回 keychain アクセス許可ダイアログを「常に許可」で承認
- [ ] メニューバーに「43% · 17%」のような表示が出ることを確認
- [ ] アイコンクリックで詳細ポップオーバーが開くことを確認
- [ ] 初回コミット & GitHub push

### Phase 4: Windows 対応の検証
- [ ] Windows 機での `cargo build` 確認
- [ ] Claude Code on Windows が Windows Credential Manager に同じ service 名で保存しているか確認
- [ ] `keyring` crate が Win Credential Manager を正しく叩けるか動作確認
- [ ] トレイの title 表示は Windows では tooltip にフォールバック（既に対応済みのつもりだが要確認）

### Phase 5: 配布 ✅(枠組み)
- [x] `assets/icon-source.png` (1024x1024) を作成
- [x] `npm run icon` で .icns / .ico / 各サイズ PNG を生成
- [x] `.github/workflows/release.yml` で tag push → Mac (Apple Silicon + Intel) + Windows をビルドして Release Draft 作成
- [x] README をエンドユーザー向けに整備
- [ ] アイコン差し替え（現在はオレンジの丸+リング、要差し替え）
- [ ] 初回 tag (`v0.1.0`) を切って Actions 動作確認
- [ ] 署名・公証は未対応（無署名配布、ユーザー側で Gatekeeper / SmartScreen 回避してもらう）

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

### 既知の制約 / リスク

1. **API は非公開仕様**。Anthropic が変更すれば壊れる。
2. **Keychain ダイアログ**: 初回起動時に macOS が「token_display が "Claude Code-credentials" にアクセスしようとしています」とダイアログを出す。「常に許可」しないと毎回出る。
3. **トークン有効期限**: access_token が切れたら 401 が返る。本実装は refresh_token を使った更新を行っていない（Claude Code 本体に再ログインしてもらう想定）。
4. **rate limit**: 同 API への過剰アクセスは 429 になる可能性。我々は 5分ポーリングなので問題ない想定。

## よく使うコマンド

```bash
# 依存インストール
npm install

# 開発起動
npm run tauri dev

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
