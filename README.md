# claude_token_display

Claude Max プランと Codex (ChatGPT) プランの使用枠を、macOS のメニューバー / Windows のシステムトレイに常時表示する常駐アプリ。Claude は 5h セッションと週間、Codex は週間使用量を 1 つのポップオーバーで確認できます。

<!-- アイコンを差し替えたらここにスクショを貼る -->

## ダウンロード

[Releases](https://github.com/maedayoshiaki/claude_token_display/releases) から OS 別バイナリを取得してください。

| OS                    | ファイル                                          |
| --------------------- | ------------------------------------------------- |
| macOS (Apple Silicon) | `*_aarch64.dmg`                                   |
| macOS (Intel)         | `*_x64.dmg`                                       |
| Windows               | `*_x64-setup.exe` (NSIS) または `*_x64_en-US.msi` |

## 前提

- 表示したいプロバイダの CLI でログイン済みであること (両方でも片方でも可)
  - **Claude**: `claude` CLI でログイン → Claude Code の OAuth トークンを読み取ります
  - **Codex**: `codex` CLI でログイン → `~/.codex/auth.json` の OAuth トークンを読み取ります
- 両方ログインしていればポップオーバーに Claude / Codex セクションが両方常時表示されます
- 片方しかログインしていない場合、未ログイン側のセクションには赤字でエラーが出ます

## 初回起動時の注意

**macOS**: 「開発元を確認できないため開けません」と出たら、Finder でアプリを右クリック → 開く → 「開く」。または `xattr -d com.apple.quarantine "/Applications/token_display.app"` を一度実行。

初回はメニューバー上で **「`/usr/bin/security` が "Claude Code-credentials" にアクセスしようとしています」** というダイアログが出ます。**「常に許可」** をクリックしてください（「許可」だと毎回出ます）。

**Windows**: SmartScreen が出たら「詳細情報」→「実行」。

Windows では:
- Claude の認証情報は通常 `%USERPROFILE%\.claude\.credentials.json` にあります。`CLAUDE_CONFIG_DIR` を設定している場合はそのディレクトリ配下の `.credentials.json` を読みます。
- Codex の認証情報は `%USERPROFILE%\.codex\auth.json`。`CODEX_HOME` を設定していればそのディレクトリ配下の `auth.json` を読みます。

## 使い方

メニューバー / システムトレイに、選択中プロバイダの 5h セッション % (例: `43%`) が表示されます。クリックでポップオーバー (ウィンドウ) が開きます。

ポップオーバーには、Claude と Codex の両方が縦に並びます:

```
                                 📌 ⚙ ⟳
CLAUDE
Session (5h)
43%                              in 2h
████████░░░░░░░░░░░░░░░░░
Weekly 17% · Sonnet 5%       Mon 09:00

CODEX
Weekly
12%                              Mon 09:00
████░░░░░░░░░░░░░░░░░░░░░

                          updated 12:34:56
```

- Claude の現在のセッション (5h) は **進捗バー + %** (バー表示は ⚙ で切り替え可)
- Codex は 5h 枠を表示せず、週間使用量を **進捗バー + %** で表示
- 週間 / Sonnet 週間は 1 行のコンパクト表示 (`Weekly 17% · Sonnet 5%   Mon 09:00`)
- リセット時刻の表記:
  - `in 30m` / `in 2h` / `in 2h30m` (24h 以内)
  - `Mon 09:00` (1 日以上先)
  - `now` (リセット中)
- トレイにはアイコンと共に Claude の 5h % / Codex の週間 % を併記:
  - macOS メニューバー title: `C 43% · X 12%`
  - tooltip (両 OS): `Claude: 5h: 43% · 7d: 17% · 7d Sonnet: 5%\nCodex: 7d: 12%`
  - 片方未ログインなら `C 43% · X !`、両方ダメなら `!`

データはデフォルト 5 分おきに自動更新。手動更新は右上の `⟳` ボタン、またはトレイ右クリック → "Refresh now"。

### カスタマイズ (⚙ 設定パネル)

- **更新間隔**: 1〜60 分
- **文字サイズ**: 0.6〜2.0 倍 (A-/A+ 早押しボタンまたは数値入力)
- **バーを表示**: ON/OFF (OFF で進捗バーを隠してより省スペース表示に)
- **Sonnet 週間を表示**: ON/OFF (OFF で Claude の Sonnet 部分を隠す)
- **固定 (📌)**: ON にすると popover がフォーカスを失っても閉じない (作業中に常駐させたい時)

設定はすべて localStorage に永続化されます。

### 固定 / 移動 / リサイズ

- `固定` ボタンを押すと、フォーカスを外してもポップオーバーが閉じなくなります。固定中はヘッダ部分をドラッグして自由に移動できます。
- 右下のグリップでウィンドウサイズを変更できます (最小 180×120 px まで縮められます)。

## 開発

```bash
git clone https://github.com/maedayoshiaki/claude_token_display.git
cd claude_token_display
npm install
npm run dev         # tauri dev
```

要件: Node.js (20+) と Rust toolchain (`rustup`)。

設計の進捗と計画は [status.md](./status.md) と [plans.md](./plans.md) を参照。アーキテクチャ概要は [AGENTS.md](./AGENTS.md)。

## ライセンス

未設定 (個人プロジェクト)。
