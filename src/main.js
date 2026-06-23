// バンドラを使わない構成のため Tauri グローバル経由
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const currentWindow = window.__TAURI__.window.getCurrentWindow();

const $ = (sel) => document.querySelector(sel);
const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

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
const UPDATE_DISMISSED_KEY = "token_display_update_dismissed";
const UPDATE_INTERVAL_HOURS_KEY = "token_display_update_interval_hours";
const THEME_KEY = "token_display_theme";
const UPDATE_NOTIFY_KEY = "token_display_update_notify";

const TEXT_SCALE_MIN = 0.6;
const TEXT_SCALE_MAX = 2.0;
const TEXT_SCALE_STEP = 0.05;
const COMPACT_WIDTH_MAX = 240;
const COMPACT_HEIGHT_MAX = 160;
const CONDENSED_WIDTH_MAX = 170;
const CONDENSED_HEIGHT_MAX = 96;
const MINIMAL_WIDTH_MAX = 132;
const MINIMAL_HEIGHT_MAX = 64;
const CONDENSED_RESETS_HEIGHT_MIN = 88;
const MINIMAL_RESETS_HEIGHT_MIN = 64;
const POPOVER_WIDTH_MIN = 1;
const POPOVER_WIDTH_MAX = 640;
const POPOVER_WIDTH_STEP = 24;
const POPOVER_WIDTH_DEFAULT = 340;
const INTERVAL_MIN_MIN = 1;
const INTERVAL_MIN_MAX = 60;
const DEFAULT_INTERVAL_MIN = 5;
const UPDATE_INTERVAL_HOURS_MIN = 1;
const UPDATE_INTERVAL_HOURS_MAX = 168;
const DEFAULT_UPDATE_INTERVAL_HOURS = 6;
const PROVIDER_LABELS = { claude: "Claude", codex: "Codex" };
const WDAY_EN = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

let isPinned = false;
let textScale = 1;
let currentIntervalMin = DEFAULT_INTERVAL_MIN;
let showSonnet = true;
let showBar = true;
let showClaude = true;
let showCodex = true;
let showWeekly = true;
let showResets = true;
let trayMetric = "five_hour";
let miniMetric = "five_hour";
let updateIntervalHours = DEFAULT_UPDATE_INTERVAL_HOURS;
let theme = "auto";
let updateNotify = true; // アプリ更新通知 (バナー/マーク) の有効/無効
let lastAllUsage = null; // 最後に描画した payload (トグル反映の再描画に使う)
let latestUpdateInfo = null; // 直近の更新情報 (バナー / マーク / data-update-available の単一ソース)
let currentDensity = "";
let currentDensityResets = "";

function levelOf(util) {
  if (util < 0.5) return "low";
  if (util < 0.85) return "mid";
  return "high";
}

function pctOf(bucket) {
  if (!bucket) return null;
  const raw = Number(bucket.utilization);
  return Number.isFinite(raw) ? Math.round(raw * 100) : 0;
}

function formatResetShort(isoString) {
  if (!isoString) return "—";
  const resets = new Date(isoString);
  const now = new Date();
  const diffMs = resets - now;
  if (diffMs <= 0) return "now";
  const mins = Math.floor(diffMs / 60000);
  if (mins < 60) return `in ${mins}m`;
  const hours = Math.floor(mins / 60);
  if (mins < 60 * 24) {
    const rem = mins % 60;
    return rem === 0 ? `in ${hours}h` : `in ${hours}h${rem}m`;
  }
  const wday = WDAY_EN[resets.getDay()];
  const hh = String(resets.getHours()).padStart(2, "0");
  const mm = String(resets.getMinutes()).padStart(2, "0");
  return `${wday} ${hh}:${mm}`;
}

function renderHeroBucket(section, bucket) {
  if (!section) return;
  if (!bucket) {
    section.querySelector("[data-pct]").textContent = "—";
    section.querySelector("[data-resets]").textContent = "waiting…";
    const fill = section.querySelector("[data-fill]");
    fill.style.width = "0%";
    fill.dataset.level = "low";
    return;
  }
  const pct = pctOf(bucket);
  section.querySelector("[data-pct]").textContent = `${pct}%`;
  section.querySelector("[data-resets]").textContent = formatResetShort(
    bucket.resets_at
  );
  const fill = section.querySelector("[data-fill]");
  fill.style.width = `${Math.max(0, Math.min(100, pct))}%`;
  fill.dataset.level = levelOf(Number(bucket.utilization) || 0);
}

function renderClaudeWeekly(section, weekly, sonnet) {
  if (!section) return;
  section.hidden = false;
  const weeklyPct = section.querySelector("[data-pct-weekly]");
  const sonnetWrap = section.querySelector("[data-sonnet-wrap]");
  const sonnetPct = section.querySelector("[data-pct-sonnet]");
  const resetsEl = section.querySelector("[data-resets]");

  weeklyPct.textContent = weekly ? `${pctOf(weekly)}%` : "—%";

  if (sonnetWrap) {
    const showIt = showSonnet && !!sonnet;
    sonnetWrap.hidden = !showIt;
    if (showIt && sonnetPct) {
      sonnetPct.textContent = `${pctOf(sonnet)}%`;
    }
  }

  const resetIso =
    (weekly && weekly.resets_at) || (sonnet && sonnet.resets_at) || null;
  resetsEl.textContent = formatResetShort(resetIso);
}

function renderCodexWeekly(section, weekly) {
  if (!section) return;
  section.hidden = false;
  const weeklyPct = section.querySelector("[data-pct-weekly]");
  const resetsEl = section.querySelector("[data-resets]");
  weeklyPct.textContent = weekly ? `${pctOf(weekly)}%` : "—%";
  resetsEl.textContent = formatResetShort(weekly ? weekly.resets_at : null);
}

function renderProvider(providerKey, result) {
  const section = document.querySelector(
    `.provider[data-provider="${providerKey}"]`
  );
  if (!section) return;
  const body = section.querySelector("[data-body]");
  const errorEl = section.querySelector("[data-error]");
  const snapshot = (result && result.snapshot) || {};

  // err と credential_restricted (403) はどちらも message をそのまま表示する。
  if (!result || result.kind === "err" || result.kind === "credential_restricted") {
    body.hidden = true;
    errorEl.hidden = false;
    errorEl.textContent =
      (result && result.message) || `${PROVIDER_LABELS[providerKey]} failed`;
    errorEl.title = errorEl.textContent;
    return;
  }
  if (result.kind === "rate_limited") {
    body.hidden = true;
    errorEl.hidden = false;
    const s = result.retry_after_secs;
    errorEl.textContent = `Rate limited. ${
      s ? `Retry in ${s}s.` : "Retrying shortly."
    }`;
    errorEl.title = errorEl.textContent;
    return;
  }
  errorEl.hidden = true;
  errorEl.removeAttribute("title");
  body.hidden = false;
  const heroSection = section.querySelector('[data-bucket="five_hour"]');
  renderHeroBucket(heroSection, snapshot.five_hour);

  if (providerKey === "claude") {
    const weeklySection = section.querySelector(
      '[data-bucket="weekly-combined"]'
    );
    renderClaudeWeekly(weeklySection, snapshot.seven_day, snapshot.seven_day_sonnet);
  } else {
    const weeklySection = section.querySelector('[data-bucket="weekly"]');
    renderCodexWeekly(weeklySection, snapshot.seven_day);
  }
}

function render(all) {
  if (!all) return;
  lastAllUsage = all;
  renderProvider("claude", all.claude);
  renderProvider("codex", all.codex);

  const dates = [all.claude, all.codex]
    .map((r) => r && r.snapshot && r.snapshot.fetched_at)
    .filter(Boolean)
    .map((s) => new Date(s));
  const latest = dates.length ? new Date(Math.max(...dates)) : new Date();
  $("#updated-at").textContent = "updated " + latest.toLocaleTimeString();
}

function rerender() {
  if (lastAllUsage) render(lastAllUsage);
}

function densityForSize(width, height) {
  if (width <= MINIMAL_WIDTH_MAX || height <= MINIMAL_HEIGHT_MAX) {
    return "minimal";
  }
  if (width <= CONDENSED_WIDTH_MAX || height <= CONDENSED_HEIGHT_MAX) {
    return "condensed";
  }
  if (width <= COMPACT_WIDTH_MAX || height <= COMPACT_HEIGHT_MAX) {
    return "compact";
  }
  return "full";
}

function densityResetsForSize(density, height) {
  if (density === "minimal") {
    return height >= MINIMAL_RESETS_HEIGHT_MIN;
  }
  if (density === "condensed") {
    return height >= CONDENSED_RESETS_HEIGHT_MIN;
  }
  return false;
}

function updateDensity() {
  const density = densityForSize(window.innerWidth, window.innerHeight);
  const densityResets = String(
    densityResetsForSize(density, window.innerHeight)
  );
  if (density !== currentDensity) {
    currentDensity = density;
    document.body.dataset.density = density;
  }
  if (densityResets !== currentDensityResets) {
    currentDensityResets = densityResets;
    document.body.dataset.densityResets = densityResets;
  }
}

function renderPinned(pinned) {
  isPinned = pinned;
  document.body.dataset.pinned = String(pinned);
  currentWindow.setResizable(true).catch(() => {});
  const btn = $("#pin");
  if (btn) {
    btn.setAttribute("aria-pressed", String(pinned));
    btn.title = pinned ? "固定解除" : "固定表示";
    btn.setAttribute("aria-label", btn.title);
  }
}

function clampTextScale(scale) {
  const normalized = Number.isFinite(scale) ? scale : 1;
  return Math.min(TEXT_SCALE_MAX, Math.max(TEXT_SCALE_MIN, normalized));
}

function renderTextScale(scale) {
  textScale = clampTextScale(scale);
  document.documentElement.style.setProperty(
    "--text-scale",
    textScale.toFixed(2)
  );
  const smaller = $("#font-smaller");
  const larger = $("#font-larger");
  if (smaller) smaller.disabled = textScale <= TEXT_SCALE_MIN + 1e-6;
  if (larger) larger.disabled = textScale >= TEXT_SCALE_MAX - 1e-6;
  const input = $("#scale-input");
  if (input && document.activeElement !== input) {
    input.value = textScale.toFixed(2);
  }
}

function initTextScale() {
  try {
    renderTextScale(parseFloat(localStorage.getItem(TEXT_SCALE_KEY) || "1"));
  } catch {
    renderTextScale(1);
  }
}

function clampIntervalMin(value) {
  const n = Number(value);
  if (!Number.isFinite(n)) return DEFAULT_INTERVAL_MIN;
  return Math.min(INTERVAL_MIN_MAX, Math.max(INTERVAL_MIN_MIN, Math.round(n)));
}

function loadStoredIntervalMin() {
  try {
    const raw = localStorage.getItem(INTERVAL_MIN_KEY);
    if (raw === null) return null;
    return clampIntervalMin(raw);
  } catch {
    return null;
  }
}

function loadStoredSonnetVisible() {
  try {
    const v = localStorage.getItem(SONNET_VISIBLE_KEY);
    if (v === "0" || v === "false") return false;
    if (v === "1" || v === "true") return true;
  } catch {
    // ignore
  }
  return true; // デフォルトは表示
}

function loadStoredBarVisible() {
  try {
    const v = localStorage.getItem(BAR_VISIBLE_KEY);
    if (v === "0" || v === "false") return false;
    if (v === "1" || v === "true") return true;
  } catch {
    // ignore
  }
  return true;
}

function loadStoredBool(key) {
  try {
    const v = localStorage.getItem(key);
    if (v === "0" || v === "false") return false;
    if (v === "1" || v === "true") return true;
  } catch {
    // ignore
  }
  return true;
}

async function setPinned(pinned) {
  try {
    const current = await invoke("set_popover_pinned", { pinned });
    renderPinned(current);
  } catch (err) {
    await initPinned();
    console.error(err);
  }
}

async function initPinned() {
  try {
    renderPinned(await invoke("get_popover_pinned"));
  } catch {
    renderPinned(false);
  }
}

async function refresh() {
  const btn = $("#refresh");
  if (btn) btn.disabled = true;
  try {
    // 表示中の cache を破棄してからバックエンドのポーラを叩き起こす。フェッチ完了時に
    // usage-updated が emit され、popover / tray / cache が同時に更新される。
    await invoke("reload_now");
  } catch (err) {
    console.error(err);
  } finally {
    setTimeout(() => {
      if (btn) btn.disabled = false;
    }, 1500);
  }
}

// 設定は別ウィンドウ。⚙ ボタンでそのウィンドウを開く (バックエンドが show + focus)。
async function openSettingsWindow() {
  try {
    await invoke("open_settings_window");
  } catch (err) {
    console.error(err);
  }
}

function startPinnedDrag(event) {
  if (!isPinned || event.button !== 0) return;
  if (event.target.closest("button, a, input, textarea, select")) return;
  event.preventDefault();
  currentWindow.startDragging().catch(() => {});
}

async function startResize(event) {
  if (event.button !== 0) return;
  event.preventDefault();
  event.stopPropagation();
  try {
    await invoke("suppress_popover_auto_hide");
  } catch {
    // 古いビルドでコマンドがない場合でもリサイズ操作自体は試す。
  }
  currentWindow.startResizeDragging("SouthEast").catch(() => {});
}

async function adjustPopoverWidth(delta) {
  const width = Math.min(
    POPOVER_WIDTH_MAX,
    Math.max(POPOVER_WIDTH_MIN, Math.round(window.innerWidth + delta))
  );
  await setPopoverWidth(width);
}

async function setPopoverWidth(width) {
  try {
    const clampedWidth = Math.min(
      POPOVER_WIDTH_MAX,
      Math.max(POPOVER_WIDTH_MIN, Math.round(width))
    );
    const result = await invoke("set_popover_width", { width: clampedWidth });
    console.debug("popover width", result);
    updateDensity();
  } catch (err) {
    console.error(err);
  }
}

function isInteractiveTarget(target) {
  return !!(
    target &&
    target.closest &&
    target.closest("button, a, input, textarea, select, [contenteditable='true']")
  );
}

function handleWidthShortcut(event) {
  if (!event.altKey || event.ctrlKey || event.metaKey || isInteractiveTarget(event.target)) {
    return;
  }

  if (event.key === "ArrowLeft" || event.key === "[") {
    event.preventDefault();
    adjustPopoverWidth(-POPOVER_WIDTH_STEP);
  } else if (event.key === "ArrowRight" || event.key === "]") {
    event.preventDefault();
    adjustPopoverWidth(POPOVER_WIDTH_STEP);
  } else if (event.key === "0" || event.key === "Home") {
    event.preventDefault();
    setPopoverWidth(POPOVER_WIDTH_DEFAULT);
  }
}

function recoverWidthFromMinimal(event) {
  if (currentDensity !== "minimal" || isInteractiveTarget(event.target)) return;
  setPopoverWidth(POPOVER_WIDTH_DEFAULT);
}

function loadStoredTheme() {
  try {
    return localStorage.getItem(THEME_KEY) || "auto";
  } catch {
    return "auto";
  }
}

// "auto" | "light" | "dark" を documentElement に反映 (CSS が配色を切り替える)。
function applyTheme(value) {
  theme = value === "light" || value === "dark" ? value : "auto";
  document.documentElement.dataset.theme = theme;
}

// 設定ウィンドウから届いた表示系設定スナップショットを popover に即適用する。
function applyViewSettings(s) {
  if (!s) return;
  if (typeof s.showClaude === "boolean") {
    showClaude = s.showClaude;
    document.body.dataset.showClaude = String(showClaude);
  }
  if (typeof s.showCodex === "boolean") {
    showCodex = s.showCodex;
    document.body.dataset.showCodex = String(showCodex);
  }
  if (typeof s.showBar === "boolean") {
    showBar = s.showBar;
    document.body.dataset.showBar = String(showBar);
  }
  if (typeof s.showWeekly === "boolean") {
    showWeekly = s.showWeekly;
    document.body.dataset.showWeekly = String(showWeekly);
  }
  if (typeof s.showResets === "boolean") {
    showResets = s.showResets;
    document.body.dataset.showResets = String(showResets);
  }
  if (typeof s.showSonnet === "boolean") {
    showSonnet = s.showSonnet;
  }
  if (typeof s.miniMetric === "string") {
    miniMetric = s.miniMetric;
    document.body.dataset.miniMetric = miniMetric;
  }
  if (typeof s.theme === "string") {
    applyTheme(s.theme);
  }
  if (typeof s.updateNotify === "boolean") {
    updateNotify = s.updateNotify;
  }
  if (s.textScale != null && Number.isFinite(Number(s.textScale))) {
    renderTextScale(Number(s.textScale));
  }
  rerender();
  // 更新通知の状態変化をバナー/マークに反映する。
  if (!updateNotify) {
    const banner = $("#update-banner");
    if (banner) banner.hidden = true;
  }
  syncUpdateIndicators();
  if (updateNotify && latestUpdateInfo) showUpdateBanner(latestUpdateInfo);
}

async function initSettings() {
  let backendSettings = null;
  try {
    backendSettings = await invoke("get_settings");
  } catch {
    // backend が古い場合フォールバック
  }

  const storedIntervalMin = loadStoredIntervalMin();
  showSonnet = loadStoredSonnetVisible();
  showBar = loadStoredBarVisible();
  showClaude = loadStoredBool(CLAUDE_VISIBLE_KEY);
  showCodex = loadStoredBool(CODEX_VISIBLE_KEY);
  showWeekly = loadStoredBool(WEEKLY_VISIBLE_KEY);
  showResets = loadStoredBool(RESETS_VISIBLE_KEY);
  updateNotify = loadStoredBool(UPDATE_NOTIFY_KEY);
  applyTheme(loadStoredTheme());

  document.body.dataset.showBar = String(showBar);
  document.body.dataset.showClaude = String(showClaude);
  document.body.dataset.showCodex = String(showCodex);
  document.body.dataset.showWeekly = String(showWeekly);
  document.body.dataset.showResets = String(showResets);

  // 小さいモード設定
  try { trayMetric = localStorage.getItem(TRAY_METRIC_KEY) || backendSettings?.tray_metric || "five_hour"; } catch {}
  try { miniMetric = localStorage.getItem(MINI_METRIC_KEY) || "five_hour"; } catch {}
  const traySelEl = $("#tray-metric-select");
  if (traySelEl) traySelEl.value = trayMetric;
  const miniSelEl = $("#mini-metric-select");
  if (miniSelEl) miniSelEl.value = miniMetric;
  document.body.dataset.miniMetric = miniMetric;
  // バックエンドに tray metric を反映
  if (!backendSettings || backendSettings.tray_metric !== trayMetric) {
    invoke("set_tray_metric", { metric: trayMetric }).catch(() => {});
  }

  const intervalMin =
    storedIntervalMin ??
    (backendSettings?.poll_interval_secs
      ? clampIntervalMin(backendSettings.poll_interval_secs / 60)
      : DEFAULT_INTERVAL_MIN);

  currentIntervalMin = intervalMin;

  // 永続化されているインターバルをバックエンドにも反映
  if (
    !backendSettings ||
    Math.round((backendSettings.poll_interval_secs || 0) / 60) !== intervalMin
  ) {
    try {
      await invoke("set_poll_interval", { secs: intervalMin * 60 });
    } catch {
      // ignore — 次回反映される
    }
  }

  // 更新確認の間隔 (時間)
  const storedUpdateHours = loadStoredUpdateIntervalHours();
  updateIntervalHours =
    storedUpdateHours ??
    (backendSettings?.update_check_interval_secs
      ? clampUpdateIntervalHours(backendSettings.update_check_interval_secs / 3600)
      : DEFAULT_UPDATE_INTERVAL_HOURS);
  const updateIntervalInput = $("#update-interval-input");
  if (updateIntervalInput) updateIntervalInput.value = updateIntervalHours;

  if (
    !backendSettings ||
    Math.round((backendSettings.update_check_interval_secs || 0) / 3600) !==
      updateIntervalHours
  ) {
    try {
      await invoke("set_update_check_interval", {
        secs: updateIntervalHours * 3600,
      });
    } catch {
      // ignore — 次回反映される
    }
  }
}

$("#refresh").addEventListener("click", refresh);
$("#settings").addEventListener("click", openSettingsWindow);
$("#pin").addEventListener("click", () => {
  setPinned(!isPinned);
});
$(".card").addEventListener("mousedown", startPinnedDrag);
$(".card").addEventListener("dblclick", recoverWidthFromMinimal);
$("#resize-handle").addEventListener("mousedown", startResize);
window.addEventListener("keydown", handleWidthShortcut);
window.addEventListener("resize", updateDensity);

// ───────── 更新通知 ─────────
function dismissedUpdateVersion() {
  try {
    return localStorage.getItem(UPDATE_DISMISSED_KEY) || "";
  } catch {
    return "";
  }
}

// バナー (full/compact) と更新マーク (condensed/minimal) と body[data-update-available]
// を、latestUpdateInfo + dismiss 状態から一括同期する。マークの表示可否は CSS が
// data-update-available × data-density で決めるので、ここでは状態だけ更新する。
function syncUpdateIndicators() {
  const info = latestUpdateInfo;
  const dismissed = !!(info && info.latest === dismissedUpdateVersion());
  const active = !!(info && info.available && !dismissed && updateNotify);
  document.body.dataset.updateAvailable = String(active);
  if (info && info.available) {
    const label = "v" + info.latest;
    const versionEl = $("#update-version");
    if (versionEl) versionEl.textContent = label;
    const mark = $("#update-mark");
    if (mark) mark.title = `新しいバージョン ${label} が利用可能です（クリックで開く）`;
  }
}

function showUpdateBanner(info, force = false) {
  if (!info || !info.available) return;
  latestUpdateInfo = info;
  // マーク / データセットは dismiss を尊重して同期 (小窓の表示はこれが担当)。
  syncUpdateIndicators();
  // 更新通知が OFF ならバナーもマークも出さない。
  if (!updateNotify) return;
  // 一度「閉じる」した版は自動ではバナーを再表示しない (新しい版が出れば再び出る)。
  // 手動チェック (force) は明示要求なので dismiss を無視して表示する。
  if (!force && info.latest === dismissedUpdateVersion()) return;
  const banner = $("#update-banner");
  const versionEl = $("#update-version");
  if (!banner || !versionEl) return;
  versionEl.textContent = "v" + info.latest;
  banner.hidden = false;
}

function dismissUpdateBanner() {
  const banner = $("#update-banner");
  if (banner) banner.hidden = true;
  try {
    const version =
      latestUpdateInfo && latestUpdateInfo.available
        ? latestUpdateInfo.latest
        : ($("#update-version")?.textContent || "").replace(/^v/, "");
    if (version) {
      localStorage.setItem(UPDATE_DISMISSED_KEY, version);
    }
  } catch {
    // ignore
  }
  // dismiss を反映してマーク / データセットも消す。
  syncUpdateIndicators();
}

async function openReleasePage() {
  try {
    await invoke("open_release_page");
  } catch (err) {
    console.error(err);
  }
}

function clampUpdateIntervalHours(value) {
  const n = Number(value);
  if (!Number.isFinite(n)) return DEFAULT_UPDATE_INTERVAL_HOURS;
  return Math.min(
    UPDATE_INTERVAL_HOURS_MAX,
    Math.max(UPDATE_INTERVAL_HOURS_MIN, Math.round(n))
  );
}

function loadStoredUpdateIntervalHours() {
  try {
    const raw = localStorage.getItem(UPDATE_INTERVAL_HOURS_KEY);
    if (raw === null) return null;
    return clampUpdateIntervalHours(raw);
  } catch {
    return null;
  }
}

async function initUpdateCheck() {
  // バックエンドが起動直後に行ったチェック結果を取りに行く。
  // (定期チェックの結果は update-available イベントで届く)
  try {
    const info = await invoke("get_update_info");
    if (info) showUpdateBanner(info);
  } catch {
    // 古いバックエンドなど。イベント側で拾えれば表示される。
  }
}

$("#update-open").addEventListener("click", openReleasePage);
$("#update-mark").addEventListener("click", openReleasePage);
$("#update-dismiss").addEventListener("click", dismissUpdateBanner);

listen("update-available", (event) => {
  showUpdateBanner(event.payload);
});

listen("usage-updated", (event) => {
  render(event.payload);
});

// 設定ウィンドウでの変更を即時反映する。
listen("settings-changed", (event) => {
  applyViewSettings(event.payload);
});

// 初期ロード時に API を叩かない（backend のポーラから event が来るのを待つ）。
updateDensity();
document.body.dataset.updateAvailable = "false";
applyTheme(loadStoredTheme());
initTextScale();
initPinned();
initSettings();
initUpdateCheck();
$("#updated-at").textContent = "waiting for data…";
