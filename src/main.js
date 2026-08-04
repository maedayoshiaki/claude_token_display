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
const TRAY_CLAUDE_KEY = "token_display_tray_claude";
const TRAY_CODEX_KEY = "token_display_tray_codex";
const UPDATE_DISMISSED_KEY = "token_display_update_dismissed";
const UPDATE_INTERVAL_HOURS_KEY = "token_display_update_interval_hours";
const THEME_KEY = "token_display_theme";
const UPDATE_NOTIFY_KEY = "token_display_update_notify";

const TEXT_SCALE_MIN = 0.6;
const TEXT_SCALE_MAX = 2.0;
const TEXT_SCALE_STEP = 0.05;
// compact = 「中モード」(バー/フッタは省くが %・週間・リセット時刻は見える)。
// full に切り替わる上限 (COMPACT_*_MAX) を広めに、condensed に落ちる下限
// (CONDENSED_*_MAX) を低めに取り、compact が効く範囲を広げている。
const COMPACT_WIDTH_MAX = 280;
const COMPACT_HEIGHT_MAX = 210;
const CONDENSED_WIDTH_MAX = 155;
const CONDENSED_HEIGHT_MAX = 84;
const MINIMAL_WIDTH_MAX = 118;
const MINIMAL_HEIGHT_MAX = 54;
const CONDENSED_RESETS_HEIGHT_MIN = 88;
// 極小モードでリセット時刻 (%の下) を出す最小の高さ。プロバイダを 1 つだけ表示している
// ときは行が半分で済むので、より低い高さから時刻を出せる。
const MINIMAL_RESETS_HEIGHT_MIN = 58;
const MINIMAL_RESETS_HEIGHT_MIN_SOLO = 34;
// 小モードで weekly 併記時、この幅以下なら見出しを縦積みにして横幅を節約する
// (これ以上あれば見出しは横に置いて縦を詰める)。
const MIN_NARROW_WIDTH_MAX = 144;
// 横並び (1 行 2 列) に切り替える条件。ウィンドウが「横長で、縦に 2 つ積むには低い」
// ときに Claude / Codex を左右に並べる。両方表示中のときだけ有効。
const TWO_COL_MIN_WIDTH = 170;
const TWO_COL_MAX_HEIGHT = 200;
// 横並びでは weekly を hero (%+時間) の右に横並びにする。列がこの幅以上あるときだけ
// weekly を出す (幅で判定)。狭いときは hero だけにして % と時間を優先表示する。
const TWO_COL_WEEKLY_MIN_WIDTH = 330;
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
let currentLayout = "";
let currentMinNarrow = "";
let currentColsWeekly = "";

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
  // Codex は現行仕様で週次使用量のみ。共通の hero DOM を使うが、値は weekly を渡す。
  renderHeroBucket(
    heroSection,
    providerKey === "codex" ? snapshot.seven_day : snapshot.five_hour
  );

  if (providerKey === "claude") {
    const weeklySection = section.querySelector(
      '[data-bucket="weekly-combined"]'
    );
    renderClaudeWeekly(weeklySection, snapshot.seven_day, snapshot.seven_day_sonnet);
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
  // 最小(最コンパクト)と中間を「交換」してある: 一番小さいレンジは inline で最コンパクトな
  // condensed 表示、その一段上は % + 時間が見える minimal 表示。これで小さくなるほど素朴に
  // なり (フォントも小さく)、中間モードで時間が見える。
  if (width <= MINIMAL_WIDTH_MAX || height <= MINIMAL_HEIGHT_MAX) {
    return "condensed";
  }
  if (width <= CONDENSED_WIDTH_MAX || height <= CONDENSED_HEIGHT_MAX) {
    return "minimal";
  }
  if (width <= COMPACT_WIDTH_MAX || height <= COMPACT_HEIGHT_MAX) {
    return "compact";
  }
  return "full";
}

// ちょうど 1 つのプロバイダだけ表示しているか (もう片方は非表示)。
function isSoloProvider() {
  return showClaude !== showCodex;
}

function densityResetsForSize(density, height) {
  if (density === "minimal") {
    const min = isSoloProvider()
      ? MINIMAL_RESETS_HEIGHT_MIN_SOLO
      : MINIMAL_RESETS_HEIGHT_MIN;
    return height >= min;
  }
  if (density === "condensed") {
    return height >= CONDENSED_RESETS_HEIGHT_MIN;
  }
  return false;
}

// 小モードで実際に body に反映する mini-metric。既定 (5h のみ) でも weekly を併記する
// "both" 表示にする (片方でも両方でも weekly を出す)。見出しは狭いとき縦積みになるので
// 両方表示でも収まる。ユーザーが明示的に weekly / both を選んでいる場合はその設定を尊重する。
function effectiveMiniMetric() {
  if (miniMetric === "five_hour") return "both";
  return miniMetric;
}

function applyMiniMetric() {
  document.body.dataset.miniMetric = effectiveMiniMetric();
}

// 横並び (cols) か縦積み (rows) か。両方のプロバイダを表示していて、かつウィンドウが
// 横長で低い (縦に積むとき窮屈になる) ときだけ左右 2 列にする。1 つしか表示していない
// ときは 2 列にする意味がないので rows のまま。各列の中身の密度は従来どおり
// densityForSize が面積 (全高) から決めるので、狭く / 低くなれば列ごとに condensed →
// minimal と自然に縮む。
function layoutForSize(width, height) {
  if (
    showClaude &&
    showCodex &&
    width >= TWO_COL_MIN_WIDTH &&
    height <= TWO_COL_MAX_HEIGHT
  ) {
    return "cols";
  }
  return "rows";
}

function updateDensity() {
  const w = window.innerWidth;
  const h = window.innerHeight;
  const layout = layoutForSize(w, h);
  let density = densityForSize(w, h);
  // 横並びは横方向レイアウトなので、高さが低くても condensed/minimal に落とさず compact
  // 固定にする。これで condensed の「リセット時刻を隠す」が効かず、時間が確実に出る。
  if (layout === "cols") density = "compact";
  // 片方だけ表示 (solo) のときは 1 プロバイダで情報も少ないので、最小レンジ (condensed)
  // だけ中間モード (minimal = %+週間+時刻のコンパクト表示) に上げる。compact はそのまま
  // compact を出す。こうしないと幅を少し狭めた (compact 入りした) だけで一気に最小表示へ
  // 落ちてしまう。full 相当まで広げれば full を出す。
  if (isSoloProvider() && density === "condensed") {
    density = "minimal";
  }
  const densityResets = String(densityResetsForSize(density, h));
  // 横並びでは weekly を hero の右に横並びにするので、幅で出し分ける (それ以外は "true")。
  const colsWeekly =
    layout === "cols" ? String(w >= TWO_COL_WEEKLY_MIN_WIDTH) : "true";
  // 小モード (中間=minimal / 最小=condensed) で幅が狭いか。weekly 併記時の見出し縦積み判定用。
  const minNarrow = String(
    (density === "minimal" || density === "condensed") &&
      w <= MIN_NARROW_WIDTH_MAX
  );
  if (density !== currentDensity) {
    currentDensity = density;
    document.body.dataset.density = density;
  }
  if (minNarrow !== currentMinNarrow) {
    currentMinNarrow = minNarrow;
    document.body.dataset.minNarrow = minNarrow;
  }
  if (densityResets !== currentDensityResets) {
    currentDensityResets = densityResets;
    document.body.dataset.densityResets = densityResets;
  }
  if (layout !== currentLayout) {
    currentLayout = layout;
    document.body.dataset.layout = layout;
  }
  if (colsWeekly !== currentColsWeekly) {
    currentColsWeekly = colsWeekly;
    document.body.dataset.colsWeekly = colsWeekly;
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
  // 小さい2モード (最小=condensed / 中間=minimal) から幅リセットで抜けられるように。
  if (
    (currentDensity !== "minimal" && currentDensity !== "condensed") ||
    isInteractiveTarget(event.target)
  ) {
    return;
  }
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
  // Claude / Codex の表示切り替えは横並び成立条件 (両方表示) と極小の mini-metric
  // (1つ表示なら weekly 併記) に影響するので再判定する。
  updateDensity();
  applyMiniMetric();
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
  // 読み込んだ表示状態で横並び / 縦積みを再判定 (初期の updateDensity は既定値で走るため)。
  updateDensity();

  // 小さいモード設定
  try { trayMetric = localStorage.getItem(TRAY_METRIC_KEY) || backendSettings?.tray_metric || "five_hour"; } catch {}
  try { miniMetric = localStorage.getItem(MINI_METRIC_KEY) || "five_hour"; } catch {}
  const traySelEl = $("#tray-metric-select");
  if (traySelEl) traySelEl.value = trayMetric;
  const miniSelEl = $("#mini-metric-select");
  if (miniSelEl) miniSelEl.value = miniMetric;
  applyMiniMetric();
  // バックエンドに tray metric を反映
  if (!backendSettings || backendSettings.tray_metric !== trayMetric) {
    invoke("set_tray_metric", { metric: trayMetric }).catch(() => {});
  }

  // トレイのプロバイダ表示 (localStorage) を起動時にバックエンドへ反映する。設定ウィンドウは
  // 起動時に開かないので、常に読み込まれる popover 側でここで初期同期する。
  const trayShowClaude = loadStoredBool(TRAY_CLAUDE_KEY);
  const trayShowCodex = loadStoredBool(TRAY_CODEX_KEY);
  if (
    !backendSettings ||
    backendSettings.tray_show_claude !== trayShowClaude ||
    backendSettings.tray_show_codex !== trayShowCodex
  ) {
    invoke("set_tray_providers", {
      claude: trayShowClaude,
      codex: trayShowCodex,
    }).catch(() => {});
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
    if (mark) mark.title = `新しいバージョン ${label} が利用可能です（クリックで更新）`;
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

// ワンクリック更新: バックエンドが署名検証付きで DL → インストール → 再起動する。
// 成功時はアプリが再起動するのでこの関数からは戻らない。失敗時 (latest.json 未公開の
// 旧リリース・署名不一致・ネットワーク不通など) はバナーにエラーを出し、手動フォール
// バック (「開く」= リリースページ) ボタンを見せる。
let updateInstalling = false;
async function installUpdate(opts = {}) {
  if (updateInstalling) return;
  updateInstalling = true;
  const textEl = $("#update-banner")?.querySelector(".update-banner__text");
  const installBtn = $("#update-install");
  const openBtn = $("#update-open");
  const dismissBtn = $("#update-dismiss");
  if (installBtn) {
    installBtn.disabled = true;
    installBtn.textContent = "更新中…";
  }
  if (dismissBtn) dismissBtn.disabled = true;
  if (openBtn) openBtn.hidden = true;
  if (textEl) textEl.textContent = "更新を準備中…";
  try {
    await invoke("install_update");
    // 通常はここに到達しない (再起動するため)。
  } catch (err) {
    console.error(err);
    updateInstalling = false;
    if (textEl) textEl.textContent = "更新に失敗しました。手動で更新してください";
    if (installBtn) {
      installBtn.disabled = false;
      installBtn.textContent = "再試行";
    }
    if (dismissBtn) dismissBtn.disabled = false;
    if (openBtn) openBtn.hidden = false; // 手動 DL フォールバックを見せる
    // 最小/極小モードから呼ばれた場合はバナー自体が隠れていてエラーが見えないので、
    // リリースページを開いて手動更新へ誘導する。
    if (opts.fallbackToPage) openReleasePage();
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

$("#update-install").addEventListener("click", () => installUpdate());
$("#update-open").addEventListener("click", openReleasePage);
// 最小/極小モードの更新マークもワンクリック更新にする (バナーと挙動を統一)。
// バナーが隠れている小モードなので、失敗時はリリースページを開くフォールバックに倒す。
$("#update-mark").addEventListener("click", () => installUpdate({ fallbackToPage: true }));
$("#update-dismiss").addEventListener("click", dismissUpdateBanner);

listen("update-available", (event) => {
  showUpdateBanner(event.payload);
});

// install_update 実行中の DL 進捗をバナーに反映する。
listen("update-download-progress", (event) => {
  const textEl = $("#update-banner")?.querySelector(".update-banner__text");
  if (!textEl) return;
  const p = event.payload || {};
  const done = Number(p.downloaded);
  const total = Number(p.total);
  if (Number.isFinite(total) && total > 0 && Number.isFinite(done)) {
    const pct = Math.min(100, Math.floor((done / total) * 100));
    textEl.textContent = `ダウンロード中… ${pct}%`;
  } else if (Number.isFinite(done) && done > 0) {
    textEl.textContent = `ダウンロード中… ${Math.floor(done / 1024)} KB`;
  }
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
