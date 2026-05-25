# status.md

このリポジトリの **今の状況** を一覧する。計画は [plans.md](./plans.md) を参照。

最終更新: 2026-05-25

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
- 表記は短い英語 (`in 2h30m` / `Mon 09:00` / `now`)
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

- 現在のバージョン: **v0.2.1** (UI 整理 + バー表示トグル + 両プロバイダ同時 tray 表示)
- ビルド対象: macOS aarch64 / x86_64, Windows x64
- 配布: GitHub Releases (`v*` tag push → 手動 `gh workflow run release.yml --ref vX.Y.Z` で発火 → Mac/Win バイナリ自動添付)

## 運用メモ

- 「やることを増やす」とき: [plans.md](./plans.md) に Phase を追加し、サブタスクを並べる
- 「終わった」とき: そのサブタスクを `[x]` にして、status.md の「進行中」表を更新
- Phase 全体が終わったら、plans.md からは消して status.md の「いま動いていること」に 1 行で吸収する
