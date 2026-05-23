// バンドラを使わない構成のため Tauri グローバル経由
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (sel) => document.querySelector(sel);

function levelOf(util) {
  if (util < 0.5) return "low";
  if (util < 0.85) return "mid";
  return "high";
}

function formatResetIn(isoString) {
  if (!isoString) return "—";
  const resets = new Date(isoString);
  const now = new Date();
  const diffMs = resets - now;
  if (diffMs <= 0) {
    return "リセット中";
  }
  const mins = Math.floor(diffMs / 60000);
  const hours = Math.floor(mins / 60);
  const remainMins = mins % 60;

  // 24h 以内は「X時間Y分後」、それ以上は曜日と時刻
  if (mins < 60 * 24) {
    if (hours === 0) return `${remainMins}分後にリセット`;
    return `${hours}時間${remainMins}分後にリセット`;
  }
  const wday = ["日", "月", "火", "水", "木", "金", "土"][resets.getDay()];
  const hh = String(resets.getHours()).padStart(2, "0");
  const mm = String(resets.getMinutes()).padStart(2, "0");
  return `${hh}:${mm} (${wday})にリセット`;
}

function renderBucket(selector, bucket) {
  const section = $(selector);
  if (!section) return;
  if (!bucket) {
    section.hidden = true;
    return;
  }
  section.hidden = false;
  const util = bucket.utilization ?? 0;
  const pct = Math.round(util * 100);
  section.querySelector("[data-pct]").textContent = `${pct}% 使用済み`;
  section.querySelector("[data-resets]").textContent = formatResetIn(bucket.resets_at);
  const fill = section.querySelector("[data-fill]");
  fill.style.width = `${Math.min(100, pct)}%`;
  fill.dataset.level = levelOf(util);
}

function showError(message) {
  const el = $("#error");
  el.hidden = false;
  el.textContent = message;
  // バケットセクションは隠す
  for (const sel of ["#bucket-5h", "#bucket-7d", "#bucket-7d-sonnet"]) {
    const s = $(sel);
    if (s) s.hidden = true;
  }
}

function clearError() {
  const el = $("#error");
  el.hidden = true;
  el.textContent = "";
}

function render(result) {
  if (!result) return;
  if (result.kind === "err") {
    showError(result.message || "unknown error");
    return;
  }
  if (result.kind === "rate_limited") {
    const s = result.retry_after_secs;
    showError(
      `Rate limited by Anthropic API. ${s ? `Retrying in ${s}s.` : "Retrying shortly."}`
    );
    return;
  }
  clearError();
  renderBucket("#bucket-5h", result.five_hour);
  renderBucket("#bucket-7d", result.seven_day);
  renderBucket("#bucket-7d-sonnet", result.seven_day_sonnet);

  const fetchedAt = result.fetched_at ? new Date(result.fetched_at) : new Date();
  $("#updated-at").textContent = "updated " + fetchedAt.toLocaleTimeString();
}

async function refresh() {
  try {
    const result = await invoke("get_usage");
    render(result);
  } catch (err) {
    showError(String(err));
  }
}

$("#refresh").addEventListener("click", refresh);

listen("usage-updated", (event) => {
  render(event.payload);
});

// 初期ロード時に API を叩かない（backend のポーラから event が来るのを待つ）。
// プレースホルダだけ出す。
$("#updated-at").textContent = "waiting for data…";
