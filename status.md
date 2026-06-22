# status.md

このリポジトリの **今の状況** を一覧する。計画は [plans.md](./plans.md) を参照。

最終更新: 2026-06-22

## いま動いていること

- **Claude / Codex 両プロバイダの使用枠を 1 つのポップオーバーで同時表示**
  - Claude: 5h セッション (バー) + 週間 + Sonnet 週間 (1 行に統合表示、Sonnet は ON/OFF 切替可)
  - Codex: 5h セッション (バー) + 週間 (`wham/usage` の `primary_window` / `secondary_window` を正規化)
- 5h バーは ON/OFF 切替可 (OFF で「数字 + reset 時刻」のみのテキスト表示)
- macOS メニューバー / Windows システムトレイに **両プロバイダの 5h % を併記** (`C 43% · X 30%`)
  - 片方失敗時は `C 43% · X !`、両方失敗時は `!`
  - tooltip は Claude / Codex を改行で並べた詳細
- 5 分おき自動ポーリング (1〜60 分で変更可能、設定変更は即時反映)
- `⟳` または設定変更時は通常経路 (tray 更新 + cache 書き換え + emit) で全体に即時反映
- pin (📌) でフォーカスロスト時の自動非表示を抑制、固定中はカード全体をドラッグで移動可
- リサイズハンドル (右下) は非表示だがドラッグ操作は可能 (cursor だけで示唆)
- 文字サイズ調整 (0.6〜2.0×, A-/A+ 早押しボタン + 数値入力)
- Windows は `%USERPROFILE%\.claude\.credentials.json` / `%USERPROFILE%\.codex\auth.json`、macOS は `security` CLI 経由で Keychain 読み取り
- **Claude トークンは CLI → Claude Desktop の順でフォールバック**: CLI 未ログインでも Claude Desktop にログイン済みなら使用枠を表示 (`claude_desktop.rs`)。Desktop の `config.json` の `oauth:tokenCacheV2`/`oauth:tokenCache` を os_crypt 復号 (Win: DPAPI+AES-256-GCM / mac: Keychain `Claude Safe Storage`+PBKDF2+AES-128-CBC)。使用枠は全サーフェス共有プールなので表示値は CLI と同一。
  - **Win の保存先はインストール形態で可変**: 旧 .exe 版は `%APPDATA%\Claude`、**Microsoft Store / MSIX 版**は `%LOCALAPPDATA%\Packages\Claude_<hash>\LocalCache\Roaming\Claude` (リダイレクト)。両方を候補化して `config.json` 実在のものを選ぶ。Store 版 v1.14271 で実機 e2e 確認済み (Windows)。macOS の復号コードは v0.6.0 の macOS CI でコンパイル確認済み・実機ランタイム e2e は未確認。
- `/api/oauth/usage` には `User-Agent: claude-code/<ver>` を付与 (無いと積極的に 429 になるため)
- **403 (credential restricted) を明示処理**: usage API が資格情報を弾く場合は生の `HTTP 403` でなく理由を表示し、ポーラは 30 分以上にバックオフ (恒久ブロックを 60s で叩き続けない)
- 表記は短い英語 (`in 2h30m` / `Mon 09:00` / `now`)
- **更新通知** (notify only): 起動 30s 後 + 設定間隔 (既定 6h、1〜168h で変更可) ごとに GitHub Releases (`releases/latest`) を確認し、新バージョンがあればポップオーバー上部にバナー表示 → 「開く」で OS ブラウザでリリースページを開く。「×」で閉じた版は再表示しない (新版が出れば再表示)
  - 設定パネルに「更新確認 (n 時間ごと)」と「今すぐ確認」ボタン
  - **テスト方法**: dev ビルドで環境変数 `TOKEN_DISPLAY_FAKE_VERSION=0.0.1` を立てて起動すると、既存の公開リリース (v0.4.2 等) が「更新あり」として扱われ、通知バナー〜リリースページを開くまでの実経路を確認できる (release ビルドでは無視)
- ナロー幅 (180 px〜) でも weekly 行は 1 行、設定パネル各行も label と入力欄が自動で折り返し

## 進行中

(なし — v0.2.1 リリース時点で plans.md のフェーズはすべて完了済み)

## 既知の課題 / TODO

- [ ] Codex API レスポンスで未知の `plan_type` (例: `prolite`) が来た場合の挙動を実機で再確認 (デコードは寛容に書いてあるが要観測)
- [ ] Codex の `reset_at` (epoch seconds) → ISO8601 への変換が tz の影響を受けないか実機で確認
- [ ] アイコン差し替え (現在はオレンジの丸+リング)
- [ ] 署名・公証は未対応 (Gatekeeper / SmartScreen は手動回避)
- [ ] Linux サポート (Secret Service バックエンド利用) は未着手
- [ ] 公開 API 上 raw token / message / session count は取得不可能 (% のみ)。表示するならローカル JSONL 集計が必要 — 今回は見送り

## リリース状況

- 現在のバージョン: **v0.6.0** (2026-06-22, Claude Desktop ログイン対応 [Microsoft Store / MSIX 版含む] + 403 ハンドリング)
- ビルド対象: macOS aarch64 / x86_64, Windows x64
- 配布: GitHub Releases。**`v*` tag を push すると `release.yml` が自動発火** → Mac/Win バイナリをビルドして自動 publish (draft なし)。`workflow_dispatch` でも手動起動可。

## 運用メモ

- 「やることを増やす」とき: [plans.md](./plans.md) に Phase を追加し、サブタスクを並べる
- 「終わった」とき: そのサブタスクを `[x]` にして、status.md の「進行中」表を更新
- Phase 全体が終わったら、plans.md からは消して status.md の「いま動いていること」に 1 行で吸収する
