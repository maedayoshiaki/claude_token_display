// 設定ウィンドウ。値は localStorage に永続化し、変更はバックエンドコマンド呼び出しと
// "settings-changed" イベントで popover に反映する (popover はイベント payload を見て即適用)。
const { invoke } = window.__TAURI__.core;
const { emit } = window.__TAURI__.event;

const $ = (sel) => document.querySelector(sel);

// localStorage キー (main.js と一致させること)
const TEXT_SCALE_KEY = "token_display_text_scale";
const INTERVAL_MIN_KEY = "token_display_interval_min";
const SONNET_VISIBLE_KEY = "token_display_sonnet_visible";
const BAR_VISIBLE_KEY = "token_display_bar_visible";
const CLAUDE_VISIBLE_KEY = "token_display_claude_visible";
const CODEX_VISIBLE_KEY = "token_display_codex_visible";
const WEEKLY_VISIBLE_KEY = "token_display_weekly_visible";
const RESETS_VISIBLE_KEY = "token_display_resets_visible";
const TRAY_METRIC_KEY = "token_display_tray_metric";
const MINI_METRIC_KEY = "token_display_mini_metric";
const UPDATE_INTERVAL_HOURS_KEY = "token_display_update_interval_hours";
const THEME_KEY = "token_display_theme";
const UPDATE_NOTIFY_KEY = "token_display_update_notify";

const TEXT_SCALE_MIN = 0.6;
const TEXT_SCALE_MAX = 2.0;
const TEXT_SCALE_STEP = 0.05;
const INTERVAL_MIN_MIN = 1;
const INTERVAL_MIN_MAX = 60;
const DEFAULT_INTERVAL_MIN = 5;
const UPDATE_INTERVAL_HOURS_MIN = 1;
const UPDATE_INTERVAL_HOURS_MAX = 168;
const DEFAULT_UPDATE_INTERVAL_HOURS = 6;
const POPOVER_WIDTH_MIN = 1;
const POPOVER_WIDTH_MAX = 640;
const POPOVER_WIDTH_STEP = 24;
const POPOVER_WIDTH_DEFAULT = 340;

// 現在値 (localStorage / バックエンドから初期化)
let textScale = 1;
let intervalMin = DEFAULT_INTERVAL_MIN;
let updateIntervalHours = DEFAULT_UPDATE_INTERVAL_HOURS;
let theme = "auto";
let updateNotify = true;
let showSonnet = true;
let showBar = true;
let showClaude = true;
let showCodex = true;
let showWeekly = true;
let showResets = true;
let trayMetric = "five_hour";
let miniMetric = "five_hour";
let widthValue = POPOVER_WIDTH_DEFAULT;

const clamp = (n, lo, hi) => Math.min(hi, Math.max(lo, n));

function getBool(key, dflt) {
  try {
    const v = localStorage.getItem(key);
    if (v === "0" || v === "false") return false;
    if (v === "1" || v === "true") return true;
  } catch {
    // ignore
  }
  return dflt;
}
function setBool(key, value) {
  try {
    localStorage.setItem(key, value ? "1" : "0");
  } catch {
    // ignore
  }
}
function setStr(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // ignore
  }
}

function clampScale(s) {
  const n = Number(s);
  return Number.isFinite(n) ? clamp(n, TEXT_SCALE_MIN, TEXT_SCALE_MAX) : 1;
}
function clampIntervalMin(v) {
  const n = Number(v);
  if (!Number.isFinite(n)) return DEFAULT_INTERVAL_MIN;
  return clamp(Math.round(n), INTERVAL_MIN_MIN, INTERVAL_MIN_MAX);
}
function clampUpdateHours(v) {
  const n = Number(v);
  if (!Number.isFinite(n)) return DEFAULT_UPDATE_INTERVAL_HOURS;
  return clamp(Math.round(n), UPDATE_INTERVAL_HOURS_MIN, UPDATE_INTERVAL_HOURS_MAX);
}
function clampWidth(v) {
  const n = Number(v);
  if (!Number.isFinite(n)) return POPOVER_WIDTH_DEFAULT;
  return clamp(Math.round(n), POPOVER_WIDTH_MIN, POPOVER_WIDTH_MAX);
}

// popover に送る表示系設定のスナップショット。
function viewSnapshot() {
  return {
    showClaude,
    showCodex,
    showBar,
    showWeekly,
    showResets,
    showSonnet,
    textScale,
    miniMetric,
    theme,
    updateNotify,
  };
}

function broadcast() {
  emit("settings-changed", viewSnapshot()).catch((e) => console.error(e));
}

function applyThemeLocally() {
  document.documentElement.dataset.theme = theme;
}

async function init() {
  let backend = null;
  try {
    backend = await invoke("get_settings");
  } catch {
    // バックエンドが古い場合は localStorage だけで初期化
  }

  textScale = clampScale(localStorage.getItem(TEXT_SCALE_KEY) || "1");
  showSonnet = getBool(SONNET_VISIBLE_KEY, true);
  showBar = getBool(BAR_VISIBLE_KEY, true);
  showClaude = getBool(CLAUDE_VISIBLE_KEY, true);
  showCodex = getBool(CODEX_VISIBLE_KEY, true);
  showWeekly = getBool(WEEKLY_VISIBLE_KEY, true);
  showResets = getBool(RESETS_VISIBLE_KEY, true);
  updateNotify = getBool(UPDATE_NOTIFY_KEY, true);
  try {
    theme = localStorage.getItem(THEME_KEY) || "auto";
  } catch {
    theme = "auto";
  }
  try {
    trayMetric = localStorage.getItem(TRAY_METRIC_KEY) || backend?.tray_metric || "five_hour";
  } catch {
    trayMetric = "five_hour";
  }
  try {
    miniMetric = localStorage.getItem(MINI_METRIC_KEY) || "five_hour";
  } catch {
    miniMetric = "five_hour";
  }

  const storedInterval = localStorage.getItem(INTERVAL_MIN_KEY);
  intervalMin =
    storedInterval !== null
      ? clampIntervalMin(storedInterval)
      : backend?.poll_interval_secs
        ? clampIntervalMin(backend.poll_interval_secs / 60)
        : DEFAULT_INTERVAL_MIN;

  const storedHours = localStorage.getItem(UPDATE_INTERVAL_HOURS_KEY);
  updateIntervalHours =
    storedHours !== null
      ? clampUpdateHours(storedHours)
      : backend?.update_check_interval_secs
        ? clampUpdateHours(backend.update_check_interval_secs / 3600)
        : DEFAULT_UPDATE_INTERVAL_HOURS;

  applyThemeLocally();
  renderControls();
}

function renderControls() {
  $("#claude-toggle").checked = showClaude;
  $("#codex-toggle").checked = showCodex;
  $("#bar-toggle").checked = showBar;
  $("#weekly-toggle").checked = showWeekly;
  $("#sonnet-toggle").checked = showSonnet;
  $("#resets-toggle").checked = showResets;
  $("#theme-select").value = theme;
  $("#scale-input").value = textScale.toFixed(2);
  $("#width-input").value = widthValue;
  $("#interval-input").value = intervalMin;
  $("#update-interval-input").value = updateIntervalHours;
  $("#update-notify-toggle").checked = updateNotify;
  $("#tray-metric-select").value = trayMetric;
  $("#mini-metric-select").value = miniMetric;
  refreshScaleButtons();
}

function refreshScaleButtons() {
  $("#font-smaller").disabled = textScale <= TEXT_SCALE_MIN + 1e-6;
  $("#font-larger").disabled = textScale >= TEXT_SCALE_MAX - 1e-6;
}

// ───── 表示トグル ─────
function bindToggle(sel, key, setter) {
  $(sel).addEventListener("change", (e) => {
    const v = e.target.checked;
    setter(v);
    setBool(key, v);
    broadcast();
  });
}
bindToggle("#claude-toggle", CLAUDE_VISIBLE_KEY, (v) => (showClaude = v));
bindToggle("#codex-toggle", CODEX_VISIBLE_KEY, (v) => (showCodex = v));
bindToggle("#bar-toggle", BAR_VISIBLE_KEY, (v) => (showBar = v));
bindToggle("#weekly-toggle", WEEKLY_VISIBLE_KEY, (v) => (showWeekly = v));
bindToggle("#sonnet-toggle", SONNET_VISIBLE_KEY, (v) => (showSonnet = v));
bindToggle("#resets-toggle", RESETS_VISIBLE_KEY, (v) => (showResets = v));

// ───── テーマ ─────
$("#theme-select").addEventListener("change", (e) => {
  theme = e.target.value;
  setStr(THEME_KEY, theme);
  applyThemeLocally();
  broadcast();
});

// ───── 文字サイズ ─────
function setScale(s) {
  textScale = clampScale(s);
  setStr(TEXT_SCALE_KEY, textScale.toFixed(2));
  $("#scale-input").value = textScale.toFixed(2);
  refreshScaleButtons();
  broadcast();
}
$("#font-smaller").addEventListener("click", () => setScale(textScale - TEXT_SCALE_STEP));
$("#font-larger").addEventListener("click", () => setScale(textScale + TEXT_SCALE_STEP));
$("#scale-input").addEventListener("change", (e) => setScale(parseFloat(e.target.value)));

// ───── 横幅 (popover を直接リサイズ) ─────
async function applyWidth(w) {
  widthValue = clampWidth(w);
  $("#width-input").value = widthValue;
  try {
    await invoke("set_popover_width", { width: widthValue });
  } catch (e) {
    console.error(e);
  }
}
$("#width-narrower").addEventListener("click", () => applyWidth(widthValue - POPOVER_WIDTH_STEP));
$("#width-wider").addEventListener("click", () => applyWidth(widthValue + POPOVER_WIDTH_STEP));
$("#width-input").addEventListener("change", (e) => applyWidth(e.target.value));
$("#reset-size").addEventListener("click", async () => {
  widthValue = POPOVER_WIDTH_DEFAULT;
  $("#width-input").value = widthValue;
  try {
    await invoke("reset_popover_size");
  } catch (e) {
    console.error(e);
  }
});

// ───── データ取得間隔 ─────
$("#interval-input").addEventListener("change", async (e) => {
  intervalMin = clampIntervalMin(e.target.value);
  e.target.value = intervalMin;
  setStr(INTERVAL_MIN_KEY, String(intervalMin));
  try {
    await invoke("set_poll_interval", { secs: intervalMin * 60 });
  } catch (err) {
    console.error(err);
  }
});

// ───── アプリ更新確認間隔 ─────
$("#update-interval-input").addEventListener("change", async (e) => {
  updateIntervalHours = clampUpdateHours(e.target.value);
  e.target.value = updateIntervalHours;
  setStr(UPDATE_INTERVAL_HOURS_KEY, String(updateIntervalHours));
  try {
    await invoke("set_update_check_interval", { secs: updateIntervalHours * 3600 });
  } catch (err) {
    console.error(err);
  }
});

// ───── アプリ更新通知 ON/OFF ─────
$("#update-notify-toggle").addEventListener("change", (e) => {
  updateNotify = e.target.checked;
  setBool(UPDATE_NOTIFY_KEY, updateNotify);
  broadcast();
});

// ───── 今すぐ確認 ─────
function setUpdateStatus(text) {
  const el = $("#update-check-status");
  if (el) el.textContent = text;
}
$("#update-check-now").addEventListener("click", async () => {
  const btn = $("#update-check-now");
  const link = $("#update-open-link");
  btn.disabled = true;
  setUpdateStatus("確認中…");
  if (link) link.hidden = true;
  try {
    const info = await invoke("check_update_now");
    if (info && info.available) {
      setUpdateStatus("新しい版があります (v" + info.latest + ")");
      if (link) link.hidden = false;
    } else if (info) {
      setUpdateStatus("最新版です (v" + info.current + ")");
    } else {
      setUpdateStatus("確認できませんでした");
    }
  } catch (err) {
    console.error(err);
    setUpdateStatus("確認に失敗しました");
  } finally {
    btn.disabled = false;
  }
});

async function openReleasePage() {
  try {
    await invoke("open_release_page");
  } catch (err) {
    console.error(err);
  }
}
$("#update-open-link").addEventListener("click", openReleasePage);
$("#update-open-link").addEventListener("keydown", (e) => {
  if (e.key === "Enter" || e.key === " ") {
    e.preventDefault();
    openReleasePage();
  }
});

// ───── トレイ表示 / 極小ウィンドウ ─────
$("#tray-metric-select").addEventListener("change", async (e) => {
  trayMetric = e.target.value;
  setStr(TRAY_METRIC_KEY, trayMetric);
  try {
    await invoke("set_tray_metric", { metric: trayMetric });
  } catch (err) {
    console.error(err);
  }
});
$("#mini-metric-select").addEventListener("change", (e) => {
  miniMetric = e.target.value;
  setStr(MINI_METRIC_KEY, miniMetric);
  broadcast();
});

init();
