# status.md

このリポジトリの **今の状況** を一覧する。計画は [plans.md](./plans.md) を参照。

最終更新: 2026-05-25

## いま動いていること

- **Claude / Codex 両プロバイダの使用枠を 1 つのポップオーバーで同時表示**
  - Claude: 5h セッション (バー) + 週間 + Sonnet 週間 (1 行に統合表示、Sonnet は ON/OFF 切替可)
  - Codex: 5h セッション (バー) + 週間 (`wham/usage` の `primary_window` / `secondary_window` を正規化)
- macOS メニューバー / Windows システムトレイにいずれか片方の 5h % を常時表示 (設定で選択)
- トレイ tooltip には両プロバイダのスナップショットを並べて表示 (▶ がプライマリ)
- 5 分おき自動ポーリング (1〜60 分で変更可能、設定変更は即時反映)
- `⟳` または設定変更時は通常経路 (tray 更新 + cache 書き換え + emit) で全体に即時反映
- pin / リサイズ / 文字サイズ調整 (0.6〜2.0×) に対応
- Windows は `%USERPROFILE%\.claude\.credentials.json` / `%USERPROFILE%\.codex\auth.json`、macOS は `security` CLI 経由で Keychain 読み取り
- 表記は短い英語 (`in 2h30m` / `Mon 09:00` / `now`)
- ナロー幅 (180 px〜) でも weekly 行は 1 行のまま、ヒーローの数字だけ縮小

## 進行中

(なし — v0.2.0 リリース時点で plans.md の全フェーズが完了済み)

## 既知の課題 / TODO

- [ ] Codex API レスポンスで未知の `plan_type` (例: `prolite`) が来た場合の挙動を実機で再確認 (デコードは寛容に書いてあるが要観測)
- [ ] Codex の `reset_at` (epoch seconds) → ISO8601 への変換が tz の影響を受けないか実機で確認
- [ ] アイコン差し替え (現在はオレンジの丸+リング)
- [ ] 署名・公証は未対応 (Gatekeeper / SmartScreen は手動回避)
- [ ] Linux サポート (Secret Service バックエンド利用) は未着手

## リリース状況

- 現在のバージョン: **v0.2.0** (Codex サポート + dual provider UI)
- ビルド対象: macOS aarch64 / x86_64, Windows x64
- 配布: GitHub Releases (`v*` tag push で GitHub Actions が走り、Draft Release に Mac/Win バイナリを添付)

## 運用メモ

- 「やることを増やす」とき: [plans.md](./plans.md) に Phase を追加し、サブタスクを並べる
- 「終わった」とき: そのサブタスクを `[x]` にして、status.md の「進行中」表を更新
- Phase 全体が終わったら、plans.md からは消して status.md の「いま動いていること」に 1 行で吸収する
