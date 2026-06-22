# plans.md

このリポジトリの **やること** を Phase ごとに記録する。完了状況は [status.md](./status.md) を参照。
新しい作業を始めるときは「Phase N: 概要」を 1 つ追加し、サブタスクを箇条書きで並べる。完了したら status.md に移し、ここからは削除して構わない。

## テンプレート

```markdown
## Phase N: <タイトル>

- **目的**: なぜやるか (1 行)
- **完了条件**: 何ができたら完了か (1〜3 行)
- **やること**:
  - [ ] サブタスク1
  - [ ] サブタスク2
- **メモ / リスク**: 既知の不確実性、外部依存、参考リンクなど
```

---

## いま進行中

## Phase 6: Claude Desktop ログインでも使えるようにする (非CLIユーザー対応)

- **目的**: Claude Code CLI にログインしていなくても、Claude Desktop でログイン済みなら使用枠を表示できるようにする (使用枠は全サーフェス共有プールなので、表示値は CLI と同一)。
- **完了条件**: CLI 認証が無く Claude Desktop だけがある環境で、Claude バケットが表示される。CLI があればこれまで通り CLI を優先。
- **やること**:
  - [x] `claude_desktop.rs`: `%APPDATA%\Claude\config.json` (Win) / `~/Library/Application Support/Claude/config.json` (mac) の `oauth:tokenCacheV2`(無ければ `oauth:tokenCache`) を os_crypt 復号して accessToken を取得
    - Win: `Local State` の `os_crypt.encrypted_key` を DPAPI 復号 → AES-256-GCM (`v10` + 12B nonce + ct + 16B tag) — **実機 Windows でコンパイル＆ユニットテスト確認済み**
    - mac: Keychain `"Claude Safe Storage"` のパスフレーズ → PBKDF2-HMAC-SHA1(saltysalt,1003,16) → AES-128-CBC (IV=0x20×16, PKCS7) — **コードレビュー済み / v0.6.0 の macOS CI でコンパイル確認済み (実機ランタイム e2e は未確認)**
  - [x] `lib.rs` `fetch_claude`: CLI keystore → 失敗時 Desktop の順でトークン取得 (フォールバック連鎖)
  - [x] `api.rs`: `/api/oauth/usage` に `User-Agent: claude-code/2.1.37` を付与 (無いと 429 の積極バケットに入る)
  - [x] 復号後 JSON 形の寛容パーサ + テスト。**`oauth:tokenCacheV2` の実スキーマを実機 Windows で確認**: エントリのマップで、キーが `client:org:audience:scopes`、アクセストークンは値の `token` フィールド (`accessToken` ではない)。`api.anthropic.com` + `user:profile` の非期限切れエントリを選ぶよう実装。
  - [x] **実機 Windows で end-to-end 確認済み**: `read_access_token()` が有効トークン (len=108, sk-ant- 始まり) を返す。Windows の DPAPI + AES-256-GCM 復号は正しく動作。
  - [x] **Microsoft Store / MSIX 版 Claude Desktop 対応** (`desktop_data_dir` / `windows_candidate_dirs`): Store 版は `%APPDATA%` がパッケージ専用フォルダにリダイレクトされ、`%APPDATA%\Claude\config.json` には存在しない (これが「Desktop ログインが見つからない」の原因だった)。`%LOCALAPPDATA%\Packages\Claude_<hash>\LocalCache\Roaming\Claude` を候補に追加し、`config.json` 実在の最初の候補を選ぶ。`Local State` も同じ dir から読む。**実機 (Store 版 v1.14271.0.0) で確認: トークン取得 → `/api/oauth/usage` が `five_hour=44% / seven_day=30% / sonnet=0%` を返す**。パス構築は純粋関数化してユニットテスト 3 本追加。`Local State` の鍵は `DPAPI` プレフィックス = App-Bound Encryption ではないことも実機確認。
  - [x] **一過性エラーのリトライ** (`read_with_retry` in lib.rs): Claude Desktop はトークン更新時に config.json を削除→再作成で書き換えるため、その瞬間に読むと NotFound / 部分書き込みで失敗し、それが 5 分キャッシュされて「ログインが見つからない」が出続ける事象を確認。NotFound / Decode / Decrypt を transient とみなし 50ms×3 リトライ。CLI 側 (`.credentials.json` も Claude Code が更新時に書き換える) にも同リトライを適用。
  - [x] **堅牢化** (調査ワークフローの指摘を反映): ① **403 (credential restricted) を明示処理** — `api.rs` で `ApiError::CredentialRestricted` (ボディから理由抽出) → `FetchResult::CredentialRestricted` を新設、`tray.rs` で 30 分以上にバックオフ (通常エラーの 60s 高速リトライを回避)、frontend は err 同様に message 表示。② **`best_v2_token` の選択順を是正** — `(audience, 未期限切れ, user:profile, 新しさ)` の順にし、期限切れ profile 付きより未期限切れを優先 (無駄な 401/403 回避)。③ `AGENTS.md` の ToS 記述を精緻化 (グレーではなく「規約違反だが現状未摘発」、タイムライン明記、最大リスクは BAN でなく機能停止)。
  - [x] **v0.6.0 (2026-06-22) でリリース** — Mac/Win バイナリ公開。macOS の Desktop 復号コードも v0.6.0 の macOS CI でコンパイル確認済み。
  - **残**: ① macOS 実機での end-to-end 確認 (CI コンパイルは済) ② UA バージョン文字列 (`claude-code/2.1.37`) の定期更新
- **メモ / リスク**:
  - 復号方式は Chrome Cookie と同一 os_crypt。`oauth:tokenCacheV2` の平文スキーマは未実機検証 → `claudeAiOauth.accessToken` / 直下 `accessToken` / 再帰探索 / 素のトークン文字列を順に試す。
  - **規約**: 消費者 OAuth トークンの第三者ツール利用は建前上グレー (credential 基準)。ただし摘発は推論アービトラージ (opencode/OpenClaw 等) が対象で、read-only 使用量モニタの BAN 事例は無し。Desktop トークンは現状の CLI トークンと同クラス・同エンドポイントなので追加リスクはほぼ無い。
  - claude.ai Cookie 経路 (案B) は scraping 条項 + Cloudflare + 30日失効で A より重く、表示値も同じため**見送り**。
  - リフレッシュは自前実装しない: 毎ポーリングでディスクから読み直すため、CLI/Desktop が自前で更新したトークンを自動的に拾える。

## 次にやりたいこと (バックログ)

- [ ] アイコン差し替え (現在はオレンジの丸+リング)
- [ ] 署名・公証の検討 (mac codesign / Windows code signing)
- [ ] Linux サポート (Secret Service)
- [ ] Codex の `plan_type=prolite` 等、未知 plan の網羅検証
- [ ] 起動時の自動起動 (LaunchAgent / Windows スタートアップ) のオプション化
