// 小窓のレイアウト検査。tests/layout/audit.html をヘッドレスの Chromium 系ブラウザで開き、
// 窓の大きさ × 条件の格子で、文字列の消失・見切れ・重なりと、窓を広げたときに情報が
// 減らないかを確かめる。問題があれば終了コード 1。
//
//   npm run test:layout            # 結果の要約
//   npm run test:layout -- --grid  # 条件ごとに、各寸法で選ばれた表示の表も出す
//
// ブラウザは BROWSER_PATH で指定できる (未指定なら Edge / Chrome / Chromium を探す)。

import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const showGrid = process.argv.includes("--grid");

function findBrowser() {
  if (process.env.BROWSER_PATH) return process.env.BROWSER_PATH;
  const candidates = {
    win32: [
      "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
      "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
      "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    ],
    darwin: [
      "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
      "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
      "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ],
    linux: ["/usr/bin/chromium", "/usr/bin/chromium-browser", "/usr/bin/google-chrome"],
  }[process.platform] || [];
  const found = candidates.find((p) => existsSync(p));
  if (!found) {
    console.error("Chromium 系のブラウザが見つかりません。BROWSER_PATH で指定してください。");
    process.exit(2);
  }
  return found;
}

function runBrowser(browser) {
  const profile = mkdtempSync(join(tmpdir(), "token-display-layout-"));
  const url = pathToFileURL(join(here, "audit.html")).href;
  const args = [
    "--headless=new",
    "--disable-gpu",
    "--no-first-run",
    "--no-default-browser-check",
    `--user-data-dir=${profile}`,
    "--allow-file-access-from-files",
    "--virtual-time-budget=600000",
    "--dump-dom",
    url,
  ];
  return new Promise((resolve, reject) => {
    const child = spawn(browser, args, { stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    child.stdout.on("data", (b) => (out += b));
    child.stderr.on("data", () => {});
    const timer = setTimeout(() => child.kill(), 300000);
    child.on("error", reject);
    child.on("close", () => {
      clearTimeout(timer);
      try {
        rmSync(profile, { recursive: true, force: true });
      } catch {}
      resolve(out);
    });
  });
}

function decodeHtml(s) {
  return s
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&");
}

const DATA_ITEMS = ["5h", "5hReset", "weekly", "sonnet", "wReset", "name"];
const FORM_LETTER = { card: "C", line: "L", cols: "H", stack: "S" };

function infoSet(row) {
  return new Set(
    row.shown.filter((n) => DATA_ITEMS.some((k) => n.endsWith("." + k)))
  );
}

function check(results) {
  const problems = [];
  for (const [sc, rows] of Object.entries(results)) {
    const grid = new Map(rows.map((r) => [`${r.W}x${r.H}`, r]));
    const Ws = [...new Set(rows.map((r) => r.W))].sort((a, b) => a - b);
    const Hs = [...new Set(rows.map((r) => r.H))].sort((a, b) => a - b);
    for (const r of rows) {
      const at = `${sc} ${r.W}x${r.H} [${r.view}${r.fit < 1 ? ` fit=${r.fit}` : ""}]`;
      if (r.gone.length) problems.push(`${at} 消失: ${r.gone.join(", ")}`);
      if (r.cut.length) problems.push(`${at} 見切れ: ${r.cut.join(", ")}`);
      if (r.overlap.length) problems.push(`${at} 重なり: ${r.overlap.join(", ")}`);
      for (const p of ["claude", "codex"]) {
        const visible = r.shown.some((n) => n.startsWith(p + "."));
        const hasPct = r.shown.includes(p + ".5h") || r.shown.includes(p + ".weekly");
        const hasErr = r.shown.includes(p + ".error");
        const expected = sc !== `solo-${p === "claude" ? "codex" : "claude"}`;
        if (expected && !visible) problems.push(`${at} ${p} が出ていない`);
        else if (expected && !hasPct && !hasErr) problems.push(`${at} ${p} の % が出ていない`);
      }
    }
    // 窓を広げて情報が減らないか (幅・高さそれぞれ 1 段ずつ比べる)。
    const lost = (a, b) => [...infoSet(a)].filter((x) => !infoSet(b).has(x));
    for (const H of Hs) {
      for (let i = 1; i < Ws.length; i++) {
        const a = grid.get(`${Ws[i - 1]}x${H}`);
        const b = grid.get(`${Ws[i]}x${H}`);
        const l = lost(a, b);
        if (l.length) problems.push(`${sc} ${a.W}x${H} → ${b.W}x${H} (幅を広げる) で減る: ${l.join(", ")}`);
      }
    }
    for (const W of Ws) {
      for (let i = 1; i < Hs.length; i++) {
        const a = grid.get(`${W}x${Hs[i - 1]}`);
        const b = grid.get(`${W}x${Hs[i]}`);
        const l = lost(a, b);
        if (l.length) problems.push(`${sc} ${W}x${a.H} → ${W}x${b.H} (高さを広げる) で減る: ${l.join(", ")}`);
      }
    }
    if (showGrid) printGrid(sc, rows, Ws, Hs, grid);
  }
  return problems;
}

function printGrid(sc, rows, Ws, Hs, grid) {
  console.log(`\n=== ${sc}  (C=card L=line H=cols S=stack, 数字=表示している項目数, *=文字を縮小, !=問題あり)`);
  console.log("H/W  " + Ws.map((w) => String(w).padStart(6)).join(""));
  for (const H of Hs) {
    const cells = Ws.map((W) => {
      const r = grid.get(`${W}x${H}`);
      let s = FORM_LETTER[r.form] + infoSet(r).size;
      if (r.fit < 1) s += "*";
      if (r.gone.length || r.cut.length || r.overlap.length) s += "!";
      return s.padStart(6);
    });
    console.log(String(H).padStart(4) + " " + cells.join(""));
  }
}

const browser = findBrowser();
const dom = await runBrowser(browser);
const err = dom.match(/ERROR=([\s\S]*?)<\/pre>/);
if (err) {
  console.error("audit.html でエラー:\n" + decodeHtml(err[1]));
  process.exit(2);
}
const m = dom.match(/RESULT_JSON=([\s\S]*?)<\/pre>/);
if (!m) {
  console.error("結果を読めませんでした (ブラウザの出力に RESULT_JSON がない)。");
  process.exit(2);
}
const results = JSON.parse(decodeHtml(m[1]));
const problems = check(results);
const cells = Object.values(results).reduce((n, rows) => n + rows.length, 0);
console.log(`\n${Object.keys(results).length} 条件 × ${cells / Object.keys(results).length} 寸法 = ${cells} 通りを検査`);if (problems.length) {
  console.log(`問題 ${problems.length} 件:`);
  for (const p of problems.slice(0, 200)) console.log("  " + p);
  if (problems.length > 200) console.log(`  ... ほか ${problems.length - 200} 件`);
  process.exit(1);
}
console.log("問題なし");
