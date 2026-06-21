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
let lastAllUsage = null; // 最後に描画した payload (トグル反映の再描画に使う)
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
  const snapshot = result.snapshot || {};
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

function setTextScale(scale) {
  renderTextScale(scale);
  try {
    localStorage.setItem(TEXT_SCALE_KEY, textScale.toFixed(2));
  } catch {
    // localStorage が使えない環境では現在のセッションだけ反映する。
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

function saveIntervalMin(min) {
  try {
    localStorage.setItem(INTERVAL_MIN_KEY, String(min));
  } catch {
    // ignore
  }
}

function saveSonnetVisible(visible) {
  try {
    localStorage.setItem(SONNET_VISIBLE_KEY, visible ? "1" : "0");
  } catch {
    // ignore
  }
}

function saveBarVisible(visible) {
  try {
    localStorage.setItem(BAR_VISIBLE_KEY, visible ? "1" : "0");
  } catch {
    // ignore
  }
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

function saveBool(key, value) {
  try {
    localStorage.setItem(key, value ? "1" : "0");
  } catch {
    // ignore
  }
}

async function applyInterval(min) {
  currentIntervalMin = clampIntervalMin(min);
  const input = $("#interval-input");
  if (input && document.activeElement !== input) {
    input.value = currentIntervalMin;
  }
  saveIntervalMin(currentIntervalMin);
  try {
    await invoke("set_poll_interval", { secs: currentIntervalMin * 60 });
  } catch (err) {
    console.error(err);
  }
}

function applySonnetVisible(visible) {
  showSonnet = !!visible;
  $("#sonnet-toggle").checked = showSonnet;
  saveSonnetVisible(showSonnet);
  rerender();
}

function applyBarVisible(visible) {
  showBar = !!visible;
  $("#bar-toggle").checked = showBar;
  document.body.dataset.showBar = String(showBar);
  saveBarVisible(showBar);
}

function applyClaudeVisible(visible) {
  showClaude = !!visible;
  $("#claude-toggle").checked = showClaude;
  document.body.dataset.showClaude = String(showClaude);
  saveBool(CLAUDE_VISIBLE_KEY, showClaude);
}

function applyCodexVisible(visible) {
  showCodex = !!visible;
  $("#codex-toggle").checked = showCodex;
  document.body.dataset.showCodex = String(showCodex);
  saveBool(CODEX_VISIBLE_KEY, showCodex);
}

function applyWeeklyVisible(visible) {
  showWeekly = !!visible;
  $("#weekly-toggle").checked = showWeekly;
  document.body.dataset.showWeekly = String(showWeekly);
  saveBool(WEEKLY_VISIBLE_KEY, showWeekly);
}

function applyResetsVisible(visible) {
  showResets = !!visible;
  $("#resets-toggle").checked = showResets;
  document.body.dataset.showResets = String(showResets);
  saveBool(RESETS_VISIBLE_KEY, showResets);
}

async function applyTrayMetric(metric) {
  trayMetric = metric;
  const sel = $("#tray-metric-select");
  if (sel) sel.value = trayMetric;
  try { localStorage.setItem(TRAY_METRIC_KEY, trayMetric); } catch {}
  try { await invoke("set_tray_metric", { metric: trayMetric }); } catch (e) { console.error(e); }
}

function applyMiniMetric(metric) {
  miniMetric = metric;
  const sel = $("#mini-metric-select");
  if (sel) sel.value = miniMetric;
  document.body.dataset.miniMetric = miniMetric;
  try { localStorage.setItem(MINI_METRIC_KEY, miniMetric); } catch {}
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
    // バックエンドのポーラを叩き起こす。フェッチ完了時に
    // usage-updated が emit され、popover / tray / cache が同時に更新される。
    await invoke("refresh_now");
  } catch (err) {
    console.error(err);
  } finally {
    setTimeout(() => {
      if (btn) btn.disabled = false;
    }, 1500);
  }
}

function toggleSettings() {
  const panel = $("#settings-panel");
  const btn = $("#settings");
  const preview = $("#preview-label");
  if (!panel || !btn) return;
  const open = panel.hidden;
  panel.hidden = !open;
  if (preview) preview.hidden = !open;
  btn.setAttribute("aria-pressed", String(open));
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

  $("#sonnet-toggle").checked = showSonnet;
  $("#bar-toggle").checked = showBar;
  $("#claude-toggle").checked = showClaude;
  $("#codex-toggle").checked = showCodex;
  $("#weekly-toggle").checked = showWeekly;
  $("#resets-toggle").checked = showResets;

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
  $("#interval-input").value = intervalMin;

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
$("#settings").addEventListener("click", toggleSettings);
$("#pin").addEventListener("click", () => {
  setPinned(!isPinned);
});
$("#font-smaller").addEventListener("click", () => {
  setTextScale(textScale - TEXT_SCALE_STEP);
});
$("#font-larger").addEventListener("click", () => {
  setTextScale(textScale + TEXT_SCALE_STEP);
});
$("#width-narrower").addEventListener("click", () => {
  adjustPopoverWidth(-POPOVER_WIDTH_STEP);
});
$("#width-wider").addEventListener("click", () => {
  adjustPopoverWidth(POPOVER_WIDTH_STEP);
});
$("#interval-input").addEventListener("change", (e) => {
  applyInterval(e.target.value);
});
$("#scale-input").addEventListener("change", (e) => {
  setTextScale(parseFloat(e.target.value));
});
$("#sonnet-toggle").addEventListener("change", (e) => {
  applySonnetVisible(e.target.checked);
});
$("#bar-toggle").addEventListener("change", (e) => {
  applyBarVisible(e.target.checked);
});
$("#claude-toggle").addEventListener("change", (e) => {
  applyClaudeVisible(e.target.checked);
});
$("#codex-toggle").addEventListener("change", (e) => {
  applyCodexVisible(e.target.checked);
});
$("#weekly-toggle").addEventListener("change", (e) => {
  applyWeeklyVisible(e.target.checked);
});
$("#resets-toggle").addEventListener("change", (e) => {
  applyResetsVisible(e.target.checked);
});
$("#tray-metric-select").addEventListener("change", (e) => {
  applyTrayMetric(e.target.value);
});
$("#mini-metric-select").addEventListener("change", (e) => {
  applyMiniMetric(e.target.value);
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

function showUpdateBanner(info, force = false) {
  if (!info || !info.available) return;
  // 一度「閉じる」した版は再表示しない (新しい版が出れば再び出る)。
  // ただし手動チェック (force) では明示要求なので無視して表示する。
  if (!force && info.latest === dismissedUpdateVersion()) return;
  const banner = $("#update-banner");
  const versionEl = $("#update-version");
  if (!banner || !versionEl) return;
  versionEl.textContent = "v" + info.latest;
  banner.hidden = false;
}

function dismissUpdateBanner() {
  const banner = $("#update-banner");
  const version = $("#update-version");
  if (banner) banner.hidden = true;
  try {
    if (version && version.textContent) {
      localStorage.setItem(
        UPDATE_DISMISSED_KEY,
        version.textContent.replace(/^v/, "")
      );
    }
  } catch {
    // ignore
  }
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

async function applyUpdateInterval(hours) {
  updateIntervalHours = clampUpdateIntervalHours(hours);
  const input = $("#update-interval-input");
  if (input && document.activeElement !== input) {
    input.value = updateIntervalHours;
  }
  try {
    localStorage.setItem(UPDATE_INTERVAL_HOURS_KEY, String(updateIntervalHours));
  } catch {
    // ignore
  }
  try {
    await invoke("set_update_check_interval", {
      secs: updateIntervalHours * 3600,
    });
  } catch (err) {
    console.error(err);
  }
}

function setUpdateStatus(text) {
  const el = $("#update-check-status");
  if (el) el.textContent = text;
}

async function checkUpdateNow() {
  const btn = $("#update-check-now");
  if (btn) btn.disabled = true;
  setUpdateStatus("確認中…");
  try {
    const info = await invoke("check_update_now");
    if (info && info.available) {
      setUpdateStatus("新しい版があります");
      showUpdateBanner(info, true);
    } else if (info) {
      setUpdateStatus("最新版です (v" + info.current + ")");
    } else {
      setUpdateStatus("確認できませんでした");
    }
  } catch (err) {
    console.error(err);
    setUpdateStatus("確認に失敗しました");
  } finally {
    if (btn) btn.disabled = false;
    // しばらくしたら通常ラベルに戻す
    setTimeout(() => setUpdateStatus("アプリの更新"), 6000);
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
$("#update-dismiss").addEventListener("click", dismissUpdateBanner);
$("#update-interval-input").addEventListener("change", (e) => {
  applyUpdateInterval(e.target.value);
});
$("#update-check-now").addEventListener("click", checkUpdateNow);

listen("update-available", (event) => {
  showUpdateBanner(event.payload);
});

listen("usage-updated", (event) => {
  render(event.payload);
});

// 初期ロード時に API を叩かない（backend のポーラから event が来るのを待つ）。
updateDensity();
initTextScale();
initPinned();
initSettings();
initUpdateCheck();
$("#updated-at").textContent = "waiting for data…";
