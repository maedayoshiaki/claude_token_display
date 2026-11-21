//! research の token-usage が書く `summary.json` から、今のアカウントの今週の内訳を取り出す。
//!
//! 設定で `summary.json` の場所を指定したときだけ使う (既定では何もしない)。
//! ファイルが古い (2 時間より前に作られた) か、組織 ID が合うアカウントがなければ何も出さない。

use serde::Serialize;

/// ウィジェットに出す内訳の 1 行。
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct BreakdownItem {
    pub label: String,
    pub usd: f64,
    /// 週間枠の % 相当。換算の比率がまだ出せない週は None。
    pub pct: Option<f64>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct UsageBreakdown {
    pub weekly_pct: Option<f64>,
    /// 額の多い順に上位 3 つ。
    pub items: Vec<BreakdownItem>,
    /// この PC 以外で使った週間枠の %。
    pub other_pct: Option<f64>,
}

/// これより古い summary.json は出さない (集計の定期タスクが止まっているとみなす)。
const MAX_AGE_MS: i64 = 2 * 3600 * 1000;

pub fn pick_breakdown(summary_json: &str, org: &str, now_ms: i64) -> Option<UsageBreakdown> {
    let v: serde_json::Value = serde_json::from_str(summary_json).ok()?;
    let generated = chrono::DateTime::parse_from_rfc3339(v.get("generated_at")?.as_str()?).ok()?;
    if now_ms - generated.timestamp_millis() > MAX_AGE_MS {
        return None;
    }
    let account = v
        .get("accounts")?
        .as_array()?
        .iter()
        .find(|a| a.get("organization_uuid").and_then(|o| o.as_str()) == Some(org))?;
    let week = account.get("week")?;
    let items = week
        .get("items")?
        .as_array()?
        .iter()
        .take(3)
        .filter_map(|i| {
            Some(BreakdownItem {
                label: i.get("label")?.as_str()?.to_string(),
                usd: i.get("usd")?.as_f64()?,
                pct: i.get("pct").and_then(|p| p.as_f64()),
            })
        })
        .collect();
    Some(UsageBreakdown {
        weekly_pct: week.get("weekly_pct").and_then(|p| p.as_f64()),
        items,
        other_pct: week.get("other_pct").and_then(|p| p.as_f64()),
    })
}

/// summary.json を読み、`org` のアカウントの内訳を返す。読めなければ None。
pub fn read_breakdown(path: &str, org: &str, now_ms: i64) -> Option<UsageBreakdown> {
    let text = std::fs::read_to_string(path).ok()?;
    pick_breakdown(&text, org, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORG: &str = "902a0f01-afeb-4697-9fb2-9a0d87da18e4";
    // 2026-11-21T13:00:00Z
    const GENERATED_MS: i64 = 1_795_266_000_000;

    fn summary(pct: &str) -> String {
        format!(
            r#"{{"version": 1, "generated_at": "2026-11-21T13:00:00Z", "unknown_models": 0,
              "accounts": [
                {{"organization_uuid": "other-org", "name": "ai-club",
                  "week": {{"weekly_pct": 66.0, "rate": null, "other_pct": null, "items": [{{"label": "x", "usd": 1.0, "pct": null}}]}},
                  "today": {{"items": []}}}},
                {{"organization_uuid": "{ORG}", "name": "private",
                  "week": {{"weekly_pct": 41.0, "rate": 0.11, "other_pct": 8.0, "items": [
                    {{"label": "hibi", "usd": 95.0, "pct": {pct}}},
                    {{"label": "hires-hsi", "usd": 70.0, "pct": 7.7}},
                    {{"label": "neru", "usd": 48.0, "pct": 5.3}},
                    {{"label": "その他", "usd": 3.0, "pct": 0.3}}]}},
                  "today": {{"items": []}}}}]}}"#
        )
    }

    #[test]
    fn picks_matching_account_top_three() {
        let b = pick_breakdown(&summary("10.5"), ORG, GENERATED_MS + 60_000).unwrap();
        assert_eq!(b.weekly_pct, Some(41.0));
        assert_eq!(b.other_pct, Some(8.0));
        assert_eq!(
            b.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(),
            vec!["hibi", "hires-hsi", "neru"]
        );
        assert_eq!(b.items[0].pct, Some(10.5));
    }

    #[test]
    fn old_file_gives_none() {
        let two_hours_later = GENERATED_MS + 2 * 3600 * 1000 + 1;
        assert_eq!(pick_breakdown(&summary("10.5"), ORG, two_hours_later), None);
    }

    #[test]
    fn other_org_gives_none() {
        assert_eq!(
            pick_breakdown(&summary("10.5"), "no-such-org", GENERATED_MS),
            None
        );
    }

    #[test]
    fn null_pct_keeps_usd_only() {
        let b = pick_breakdown(&summary("null"), ORG, GENERATED_MS).unwrap();
        assert_eq!(b.items[0].pct, None);
        assert_eq!(b.items[0].usd, 95.0);
    }

    #[test]
    fn broken_json_gives_none() {
        assert_eq!(pick_breakdown("{not json", ORG, GENERATED_MS), None);
    }
}
