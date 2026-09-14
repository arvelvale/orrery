/**
 * Openplane UI — WebView 前身
 * 在浏览器中用 mock；在 Tauri 里通过 invoke 调 Rust。
 * 所有面向用户的文案走 i18n.js 的 t()，这里不写死任何自然语言。
 */

import { LOCALES, applyStatic, detectLocale, formatRelative, getLocale, missingKeys, setLocale, t } from "./i18n.js";

const HARNESS = {
  cc:   { id: "cc",   label: "CC",   name: "Claude Code", badge: "cc" },
  kimi: { id: "kimi", label: "KIMI", name: "Kimi Code",   badge: "kimi" },
  dsh:  { id: "dsh",  label: "DSH",  name: "DSH",         badge: "dsh" },
  codex: { id: "codex", label: "CODEX", name: "Codex",     badge: "codex" },
};

const HARNESS_IDS = Object.keys(HARNESS);

const MODELS = [
  { id: "claude-opus-5", vendor: "anthropic", note: "models.note.flagship" },
  { id: "claude-sonnet-5", vendor: "anthropic", note: "models.note.balanced" },
  { id: "kimi-k2.5", vendor: "moonshot", note: "models.note.kimi" },
  { id: "gpt-5.2", vendor: "openai", note: "models.note.openai" },
  { id: "deepseek-v3.2", vendor: "deepseek", note: "models.note.value" },
  { id: "gpt-5.3-codex", vendor: "openai", note: "models.note.codex" },
];

/*
 * 浏览器预览用的 mock 会话：全部是虚构的示例项目，不要换成真实项目名或路径
 * （README 截图就截这份数据，仓库是公开的）。
 * 标题/摘要是"用户输入的内容"，按语言各写一份，让三语截图各自自然。
 */
const MIN = 60_000;
const SEED_SESSIONS = [
  {
    id: "7f3a9c2e",
    harness: "cc",
    title: {
      en: "acme-web · Migrate login to OAuth 2.1",
      "zh-CN": "acme-web · 登录迁移到 OAuth 2.1",
      ja: "acme-web · ログインを OAuth 2.1 に移行",
    },
    excerpt: {
      en: "Swap the session cookie flow for PKCE and keep existing users signed in.",
      "zh-CN": "把会话 Cookie 流程换成 PKCE，同时保持老用户登录态。",
      ja: "セッション Cookie のフローを PKCE に置き換え、既存ユーザーのログイン状態を維持する。",
    },
    project: "~/code/acme-web",
    model: "claude-sonnet-5",
    status: "running",
    ago: 2 * MIN,
    usage: { input: 3200, cache_write: 5100, cache_read: 31400, output: 2400, calls: 18 },
    sizeBytes: 3_984_589,
    subagents: 2,
    log: [
      ["hi", "read src/auth/session.ts"],
      ["ok", "PKCE verifier + challenge added"],
      ["ok", "refresh token rotation covered by tests"],
      ["", "updating callback route…"],
    ],
  },
  {
    id: "b81d04f7",
    harness: "kimi",
    title: {
      en: "weather-cli · Add hourly forecast command",
      "zh-CN": "weather-cli · 新增逐小时预报命令",
      ja: "weather-cli · 1 時間ごとの予報コマンドを追加",
    },
    excerpt: {
      en: "Waiting for confirmation on the output table format.",
      "zh-CN": "等待确认输出表格的格式。",
      ja: "出力テーブルの形式について確認待ち。",
    },
    project: "~/code/weather-cli",
    model: "kimi-k2.5",
    status: "idle",
    ago: 18 * MIN,
    usage: { input: 9800, cache_write: 12000, cache_read: 101500, output: 4700, calls: 41 },
    sizeBytes: 22_439_526,
    subagents: 5,
    log: [
      ["ok", "hourly subcommand wired"],
      ["warn", "API rate limit hit twice"],
    ],
  },
  {
    id: "c29e6b10",
    harness: "dsh",
    title: {
      en: "pixel-notes · Fix flaky sync test",
      "zh-CN": "pixel-notes · 修复不稳定的同步测试",
      ja: "pixel-notes · 不安定な同期テストを修正",
    },
    excerpt: {
      en: "Same failure showed up twice. Back to the stack trace before touching code.",
      "zh-CN": "同一个报错第二次出现，先回到错误栈，再动代码。",
      ja: "同じエラーが 2 回目。コードを触る前にスタックトレースに戻る。",
    },
    project: "~/code/pixel-notes",
    model: "deepseek-v3.2",
    status: "error",
    ago: 60 * MIN,
    usage: { input: 1200, cache_write: 1800, cache_read: 5600, output: 800, calls: 6 },
    sizeBytes: 629_146,
    subagents: 0,
    log: [
      ["warn", "sync.spec.ts timed out after 5000ms"],
      ["warn", "retry with fake timers → still fails"],
      ["", "root cause not isolated yet"],
    ],
  },
  {
    id: "d4c71a58",
    harness: "codex",
    title: {
      en: "todo-api · Write OpenAPI docs",
      "zh-CN": "todo-api · 编写 OpenAPI 文档",
      ja: "todo-api · OpenAPI ドキュメントを作成",
    },
    excerpt: {
      en: "All 14 endpoints documented, examples generated from tests.",
      "zh-CN": "14 个接口全部写完，示例由测试用例生成。",
      ja: "14 個のエンドポイントをすべて記述し、例はテストから生成。",
    },
    project: "~/code/todo-api",
    model: "gpt-5.3-codex",
    status: "done",
    ago: 26 * 60 * MIN,
    usage: { input: 2600, cache_write: 3900, cache_read: 23300, output: 1900, calls: 14 },
    sizeBytes: 5_138_022,
    subagents: 1,
    log: [
      ["ok", "openapi.yaml validated"],
      ["ok", "docs site preview built"],
    ],
  },
  {
    id: "e6f2b390",
    harness: "cc",
    title: {
      en: "blog-engine · Speed up static build",
      "zh-CN": "blog-engine · 加速静态站点构建",
      ja: "blog-engine · 静的ビルドを高速化",
    },
    excerpt: {
      en: "Profile first: markdown parsing is 70% of build time.",
      "zh-CN": "先做性能分析：Markdown 解析占了构建时间的 70%。",
      ja: "まず計測：Markdown の解析がビルド時間の 70% を占める。",
    },
    project: "~/code/blog-engine",
    model: "claude-opus-5",
    status: "idle",
    ago: 3 * 60 * MIN,
    usage: { input: 4100, cache_write: 6300, cache_read: 42900, output: 2900, calls: 22 },
    sizeBytes: 8_703_181,
    subagents: 3,
    log: [
      ["hi", "profiled 412 posts"],
      ["", "awaiting cache strategy decision"],
    ],
  },
  {
    id: "f90a3d21",
    harness: "codex",
    title: {
      en: "chess-bot · Tune search depth",
      "zh-CN": "chess-bot · 调整搜索深度",
      ja: "chess-bot · 探索深度を調整",
    },
    excerpt: {
      en: "Benchmarking depth 6 vs 7 against the opening book.",
      "zh-CN": "用开局库对比搜索深度 6 和 7 的表现。",
      ja: "定跡データで探索深度 6 と 7 を比較中。",
    },
    project: "~/code/chess-bot",
    model: "gpt-5.3-codex",
    status: "running",
    ago: 20_000,
    usage: { input: 900, cache_write: 1400, cache_read: 5300, output: 600, calls: 5 },
    sizeBytes: 419_430,
    subagents: 0,
    log: [
      ["hi", "running 200-game match"],
      ["ok", "depth 7: +38 Elo, 2.1× slower"],
    ],
  },
];

const DEFAULT_ROUTES = {
  cc: "claude-sonnet-5",
  kimi: "kimi-k2.5",
  dsh: "deepseek-v3.2",
  codex: "gpt-5.3-codex",
};

function seedSessions() {
  const now = Date.now();
  return SEED_SESSIONS.map(({ ago, ...s }) => ({
    ...s,
    updatedMs: now - ago,
    tokens: formatTok(usageTotal(s.usage)),
    path: s.project,
  }));
}

const state = {
  sessions: seedSessions(),
  filter: "all",
  query: "",
  routes: { ...DEFAULT_ROUTES },
  selectedId: null,
  view: "sessions",
  proxyOnline: true,
  runtime: "browser", // browser | tauri
  source: "mock",
  storage: null, // Tauri: storage_stats 结果；浏览器预览按 mock 会话汇总
};

/* ── 本地化取值 ── */

/** mock 数据的标题/摘要按语言存；真实数据是用户原文字符串 */
function localized(v) {
  if (v && typeof v === "object") return v[getLocale()] ?? v.en ?? "";
  return v ?? "";
}

function sessionTitle(s) {
  return localized(s.title) || t("session.untitled", { id: String(s.id).slice(0, 8) });
}

function usageTotal(u) {
  return u ? u.input + u.cache_write + u.cache_read + u.output + (u.unsplit || 0) : 0;
}

/* ── Tauri bridge ── */
async function getTauri() {
  // Tauri 2 withGlobalTauri: window.__TAURI__.core.invoke
  if (window.__TAURI__?.core?.invoke) {
    return { invoke: window.__TAURI__.core.invoke };
  }
  if (window.__TAURI_INTERNALS__) {
    try {
      const core = await import("@tauri-apps/api/core");
      if (core?.invoke) return { invoke: core.invoke };
    } catch (_) {}
  }
  return null;
}

async function invokeTauri(cmd, args = {}) {
  const tauri = await getTauri();
  if (!tauri) return null;
  return tauri.invoke(cmd, args);
}

async function detectRuntime() {
  const tauri = await getTauri();
  state.runtime = tauri ? "tauri" : "browser";
  renderRuntimeBadge();
  return tauri;
}

function renderRuntimeBadge() {
  const badge = document.getElementById("build-badge");
  if (badge) badge.textContent = t(state.runtime === "tauri" ? "runtime.tauri" : "runtime.browser");
}

async function loadNativeSessions() {
  try {
    const rows = await invokeTauri("list_sessions");
    if (Array.isArray(rows) && rows.length) {
      state.sessions = rows.map((r) => ({
        id: r.id,
        harness: r.harness || "cc",
        title: r.title || "",
        project: r.project || "",
        model: r.model || "—",
        status: r.status || "idle",
        updatedMs: Number(r.updated_ms) || 0,
        tokens: r.tokens || "—",
        usage: r.usage || null,
        excerpt: r.excerpt || "",
        log: r.log || [],
        path: r.path || "",
        sizeBytes: Number(r.size_bytes) || 0,
        subagents: Number(r.subagents) || 0,
      }));
      state.source = "native";
      return true;
    }
    if (Array.isArray(rows)) {
      // 真实扫描结果为空
      state.sessions = [];
      state.source = "native-empty";
      return true;
    }
  } catch (e) {
    console.warn("list_sessions failed", e);
  }
  return false;
}

async function loadNativeStorage() {
  try {
    const rows = await invokeTauri("storage_stats");
    if (Array.isArray(rows)) {
      state.storage = rows.map((r) => ({
        harness: r.harness,
        connected: !!r.connected,
        sessions: Number(r.sessions) || 0,
        sessionBytes: Number(r.session_bytes) || 0,
        rootBytes: Number(r.root_bytes) || 0,
        root: r.root || "",
      }));
      return true;
    }
  } catch (e) {
    console.warn("storage_stats failed", e);
  }
  state.storage = null;
  return false;
}

/** 无原生统计时（浏览器预览）按当前会话列表汇总 */
function storageRows() {
  if (state.storage) return state.storage;
  return HARNESS_IDS.map((hid) => {
    const list = state.sessions.filter((s) => s.harness === hid);
    const bytes = list.reduce((n, s) => n + (s.sizeBytes || 0), 0);
    return { harness: hid, connected: list.length > 0, sessions: list.length, sessionBytes: bytes, rootBytes: bytes, root: "" };
  });
}

function formatBytes(n) {
  if (!Number.isFinite(n) || n <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return `${i === 0 ? n : n.toFixed(n >= 100 ? 0 : 1)} ${units[i]}`;
}

async function loadNativeProxy() {
  try {
    const st = await invokeTauri("get_proxy_status");
    if (st && typeof st.online === "boolean") {
      state.proxyOnline = st.online;
      return true;
    }
  } catch (e) {
    console.warn("get_proxy_status failed", e);
  }
  return false;
}

/* ── Persistence ── */
const LS_KEY = "openplane.v1";
function loadLocal() {
  try {
    const raw = localStorage.getItem(LS_KEY);
    if (!raw) return;
    const data = JSON.parse(raw);
    if (data.routes) state.routes = { ...DEFAULT_ROUTES, ...data.routes };
  } catch (_) {}
}
function saveLocal() {
  try {
    localStorage.setItem(LS_KEY, JSON.stringify({ routes: state.routes }));
  } catch (_) {}
}

const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];

let toastTimer;
function toast(msg) {
  const el = $("#toast");
  el.textContent = msg;
  el.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.remove("show"), 2200);
}

/* ── 语言切换 ── */

function renderLangSwitch() {
  const wrap = $("#lang-switch");
  wrap.innerHTML = "";
  LOCALES.forEach((l) => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "lang-opt" + (getLocale() === l.id ? " active" : "");
    b.textContent = l.short;
    b.title = l.name;
    b.lang = l.id;
    b.setAttribute("aria-pressed", String(getLocale() === l.id));
    b.addEventListener("click", () => {
      if (getLocale() === l.id) return;
      setLocale(l.id, { persist: true });
      rerenderAll();
    });
    wrap.appendChild(b);
  });
}

/** 切语言后：静态文案 + 所有动态区域 + 已打开的详情 */
function rerenderAll(boot = false) {
  applyStatic();
  renderLangSwitch();
  renderRuntimeBadge();
  renderAnnunciator(boot);
  renderFilters();
  renderNavFilters();
  renderSessions();
  renderModels();
  renderStatus();
  updateProxyRow();
  if (state.selectedId && findSession(state.selectedId)) openSession(state.selectedId);
  else closeDetail();
}

/* ── Annunciator / filters ── */

function harnessLampState(hid) {
  const list = state.sessions.filter((s) => s.harness === hid);
  if (!list.length) return "idle";
  if (list.some((s) => s.status === "error")) return "warn";
  if (list.some((s) => s.status === "running")) return "run";
  return "idle";
}

function renderAnnunciator(boot = false) {
  const bar = $("#annunciator");
  bar.innerHTML = "";
  const items = [
    { id: "proxy", label: "PRX", state: state.proxyOnline ? "run" : "warn", title: t(state.proxyOnline ? "lamp.proxyOn" : "lamp.proxyOff") },
    ...HARNESS_IDS.map((hid) => ({
      id: hid,
      label: HARNESS[hid].label,
      state: harnessLampState(hid),
      title: HARNESS[hid].name,
    })),
  ];
  items.forEach((item, i) => {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "lamp" + (boot ? " booting" : "");
    btn.dataset.state = item.state;
    btn.title = item.title;
    btn.setAttribute("aria-label", `${item.title}: ${item.state}`);
    if (boot) btn.style.animationDelay = `${i * 60}ms`;
    btn.innerHTML = `<span class="lamp-dot"></span><span class="lamp-label">${item.label}</span>`;
    btn.addEventListener("click", () => {
      if (item.id === "proxy") switchView("models");
      else {
        state.filter = item.id;
        switchView("sessions");
        renderFilters();
        renderNavFilters();
        renderSessions();
      }
    });
    bar.appendChild(btn);
  });
}

function renderFilters() {
  const wrap = $("#filters");
  const keys = ["all", ...HARNESS_IDS];
  wrap.innerHTML = "";
  keys.forEach((k) => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "chip" + (state.filter === k ? " active" : "");
    b.textContent = k === "all" ? t("filter.all") : HARNESS[k].label;
    b.addEventListener("click", () => {
      state.filter = k;
      renderFilters();
      renderNavFilters();
      renderSessions();
    });
    wrap.appendChild(b);
  });
}

function renderNavFilters() {
  const wrap = $("#nav-filters");
  wrap.innerHTML = "";
  HARNESS_IDS.forEach((hid) => {
    const count = state.sessions.filter((s) => s.harness === hid).length;
    const b = document.createElement("button");
    b.type = "button";
    b.className = "nav-item" + (state.filter === hid ? " active" : "");
    b.title = HARNESS[hid].name;
    b.innerHTML = `
      <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="4" y="6" width="16" height="12" rx="2"/></svg>
      <span class="label">${HARNESS[hid].label}</span>
      <span class="count">${count}</span>`;
    b.addEventListener("click", () => {
      state.filter = hid;
      switchView("sessions");
      renderFilters();
      renderNavFilters();
      renderSessions();
    });
    wrap.appendChild(b);
  });
}

function filteredSessions() {
  const q = state.query.trim().toLowerCase();
  return state.sessions.filter((s) => {
    if (state.filter !== "all" && s.harness !== state.filter) return false;
    if (!q) return true;
    return [sessionTitle(s), s.project, s.model, localized(s.excerpt), s.id].join(" ").toLowerCase().includes(q);
  });
}

function statusLabel(st) {
  return ({ running: "RUNNING", idle: "IDLE", done: "DONE", error: "ERROR" })[st] || st;
}

/* ── Sessions ── */

function renderSessions() {
  const list = $("#session-list");
  const items = filteredSessions();
  const source = t(
    state.source === "native" ? "source.native" :
    state.source === "native-empty" ? "source.nativeEmpty" :
    "source.mock"
  );
  const listedBytes = items.reduce((n, s) => n + (s.sizeBytes || 0), 0);
  $("#session-count").textContent = t("sessions.count", { n: items.length, source, size: formatBytes(listedBytes) });
  $("#nav-count-sessions").textContent = String(state.sessions.length);

  if (!items.length) {
    list.innerHTML = `
      <div class="empty">
        <strong>${escapeHtml(t("empty.title"))}</strong>
        ${escapeHtml(t(state.source.startsWith("native") ? "empty.native" : "empty.mock"))}
        <div><button type="button" class="btn" id="btn-scan">${escapeHtml(t("empty.scan"))}</button></div>
      </div>`;
    const scan = $("#btn-scan");
    if (scan) scan.addEventListener("click", async () => {
      const [ok] = await Promise.all([loadNativeSessions(), loadNativeStorage()]);
      renderSessions();
      renderAnnunciator();
      renderNavFilters();
      toast(t(ok ? "toast.rescanned" : state.runtime === "tauri" ? "toast.scanFailed" : "toast.scanNeedsDesktop"));
    });
    return;
  }

  list.innerHTML = "";
  items.forEach((s) => {
    const h = HARNESS[s.harness] || HARNESS.cc;
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "strip" + (state.selectedId === s.id ? " selected" : "");
    btn.dataset.status = s.status;
    btn.dataset.id = s.id;
    btn.innerHTML = `
      <span class="strip-bar" aria-hidden="true"></span>
      <span class="strip-body">
        <span class="strip-top">
          <span class="badge ${h.badge}">${h.label}</span>
          <span class="strip-title">${escapeHtml(sessionTitle(s))}</span>
          <span class="strip-status">${statusLabel(s.status)}</span>
        </span>
        <span class="strip-meta">
          <span>${escapeHtml(s.model)}</span>
          <span>${escapeHtml(s.tokens)} tok</span>
          <span class="size" title="${escapeHtml(t("sessions.sizeTitle"))}">${formatBytes(s.sizeBytes)}</span>
          <span class="path">${escapeHtml(s.project)}</span>
        </span>
      </span>
      <span class="strip-side">
        <span class="strip-time">${escapeHtml(formatRelative(s.updatedMs))}</span>
      </span>`;
    btn.addEventListener("click", () => openSession(s.id));
    list.appendChild(btn);
  });
}

function escapeHtml(str) {
  return String(str)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function findSession(id) {
  return state.sessions.find((s) => s.id === id);
}

function formatTok(n) {
  if (!n) return "0";
  // 与 Rust format_tokens 同规则：一位小数，去掉多余的 .0
  const short = (v, unit) => `${v.toFixed(1).replace(/\.0$/, "")}${unit}`;
  if (n >= 1e6) return short(n / 1e6, "M");
  if (n >= 1e3) return short(n / 1e3, "k");
  return String(n);
}

/** 主 agent + 子 agent 累计，按 API 调用去重；列表里的总数 = 四项之和 */
function usageSplitHtml(u) {
  return [
    ["usage.input", u.input],
    ["usage.cacheWrite", u.cache_write],
    ["usage.cacheRead", u.cache_read],
    ["usage.output", u.output],
    // 旧版 Codex 只记总数、没有分项，单列出来而不是猜测拆分
    ...(u.unsplit ? [["usage.unsplit", u.unsplit]] : []),
  ].map(([k, v]) => `<span>${escapeHtml(t(k))} ${formatTok(v)}</span>`).join("");
}

function sessionDetailHtml(s) {
  const h = HARNESS[s.harness] || HARNESS.cc;
  const log = (s.log || []).map(([cls, line]) => {
    const c = cls ? ` class="${cls}"` : "";
    return `<span${c}>${escapeHtml(line)}</span>`;
  }).join("\n");
  const calls = s.usage ? ` · ${escapeHtml(t("detail.calls", { n: s.usage.calls }))}` : "";
  const subagents = s.subagents ? ` · ${escapeHtml(t("detail.subagents", { n: s.subagents }))}` : "";
  return `
    <dl class="kv">
      <dt>HARNESS</dt><dd>${escapeHtml(h.name)}</dd>
      <dt>STATUS</dt><dd>${statusLabel(s.status)}</dd>
      <dt>MODEL</dt><dd>${escapeHtml(s.model)}</dd>
      <dt>TOKENS</dt><dd>${escapeHtml(s.tokens)}${calls}</dd>
      ${s.usage ? `<dt>USAGE</dt><dd class="usage-split">${usageSplitHtml(s.usage)}</dd>` : ""}
      <dt>SIZE</dt><dd>${formatBytes(s.sizeBytes)}${subagents}</dd>
      <dt>PROJECT</dt><dd>${escapeHtml(s.project)}</dd>
      <dt>UPDATED</dt><dd>${escapeHtml(formatRelative(s.updatedMs))}</dd>
      <dt>SESSION</dt><dd>${escapeHtml(s.id)}</dd>
    </dl>
    <p class="excerpt">${escapeHtml(localized(s.excerpt))}</p>
    ${log ? `<div class="log">${log}</div>` : ""}
    <div class="btn-row">
      <button type="button" class="btn primary" data-act="resume">${escapeHtml(t("detail.resume"))}</button>
      <button type="button" class="btn" data-act="copy-path">${escapeHtml(t("detail.copyPath"))}</button>
    </div>`;
}

function openSession(id) {
  state.selectedId = id;
  const s = findSession(id);
  if (!s) return;
  const h = HARNESS[s.harness] || HARNESS.cc;
  $("#shell").classList.add("has-detail");
  const badge = $("#detail-badge");
  badge.textContent = h.label;
  badge.className = "badge " + h.badge;
  $("#detail-title").textContent = sessionTitle(s);
  $("#detail-body").innerHTML = sessionDetailHtml(s);
  wireDetailActions($("#detail-body"), s);
  renderSessions();
}

function closeDetail() {
  state.selectedId = null;
  $("#shell").classList.remove("has-detail");
  $("#detail-title").textContent = t("detail.none");
  $("#detail-badge").textContent = "—";
  $("#detail-badge").className = "badge";
  $("#detail-body").innerHTML = `<p class="hint">${escapeHtml(t("detail.hint"))}</p>`;
  renderSessions();
}

function wireDetailActions(root, s) {
  root.querySelectorAll("[data-act]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const act = btn.getAttribute("data-act");
      if (act === "resume") {
        const path = s.path || s.project;
        const res = await invokeTauri("open_path", { path });
        if (res === true) toast(t("toast.openedExplorer"));
        else toast(state.runtime === "tauri" ? t("toast.openFailed") : t("toast.previewWouldOpen", { path }));
      } else if (act === "copy-path") {
        await copyText(s.project);
        toast(t("toast.pathCopied"));
      }
    });
  });
}

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    document.body.appendChild(ta);
    ta.select();
    document.execCommand("copy");
    ta.remove();
  }
}

/* ── Models ── */

function renderModels() {
  const routes = $("#routes");
  routes.innerHTML = "";
  HARNESS_IDS.forEach((hid) => {
    const h = HARNESS[hid];
    const row = document.createElement("div");
    row.className = "route";
    const options = MODELS.map((m) =>
      `<option value="${m.id}" ${state.routes[hid] === m.id ? "selected" : ""}>${m.id}</option>`
    ).join("");
    row.innerHTML = `
      <div class="route-harness">
        <div class="name">${escapeHtml(h.name)}</div>
        <div class="sub">${h.label} · default</div>
      </div>
      <div class="route-arrow" aria-hidden="true">→</div>
      <div class="route-model">
        <label class="sr-only" for="route-${hid}">${escapeHtml(t("models.defaultFor", { name: h.name }))}</label>
        <select id="route-${hid}">${options}</select>
      </div>`;
    routes.appendChild(row);
    row.querySelector("select").addEventListener("change", async (e) => {
      state.routes[hid] = e.target.value;
      saveLocal();
      await invokeTauri("save_route", { harness: hid, model: e.target.value });
      toast(`${h.name} → ${e.target.value}`);
      renderAnnunciator();
      renderStatus();
    });
  });

  $("#model-pool").innerHTML = MODELS.map((m) => `
    <div class="pool-item">
      <code>${escapeHtml(m.id)}</code>
      <span class="tag">${escapeHtml(m.vendor)} · ${escapeHtml(t(m.note))}</span>
    </div>`).join("");

  updateProxyRow();
}

function updateProxyRow() {
  const dot = $("#proxy-dot");
  const tag = $("#proxy-tag");
  const chip = $("#proxy-chip");
  if (state.proxyOnline) {
    dot.classList.remove("off");
    tag.textContent = "ONLINE";
    tag.style.color = "var(--run)";
    chip.classList.remove("off");
    $("#proxy-chip-text").textContent = "127.0.0.1:8787";
  } else {
    dot.classList.add("off");
    tag.textContent = "OFFLINE";
    tag.style.color = "var(--warn)";
    chip.classList.add("off");
    $("#proxy-chip-text").textContent = t("proxy.offlineChip");
  }
}

/* ── Status ── */

function storageCardHtml() {
  const rows = storageRows();
  const connected = rows.filter((r) => r.connected);
  const total = connected.reduce((n, r) => n + r.sessionBytes, 0);
  const totalSessions = connected.reduce((n, r) => n + r.sessions, 0);
  const max = Math.max(1, ...connected.map((r) => r.sessionBytes));
  const lines = rows.map((r) => {
    const h = HARNESS[r.harness];
    if (!r.connected) {
      return `
        <div class="storage-row off">
          <span class="storage-name">${h.label}</span>
          <span class="storage-bar"></span>
          <span class="storage-size">${escapeHtml(t("storage.notConnected"))}</span>
        </div>`;
    }
    const pct = Math.max(2, Math.round((r.sessionBytes / max) * 100));
    const extra = r.rootBytes > r.sessionBytes ? t("storage.rootExtra", { size: formatBytes(r.rootBytes) }) : "";
    const tip = [t("storage.sessions", { n: r.sessions }), r.root, extra].filter(Boolean).join(" · ");
    return `
      <div class="storage-row" title="${escapeHtml(tip)}">
        <span class="storage-name">${h.label}</span>
        <span class="storage-bar"><i style="width:${pct}%"></i></span>
        <span class="storage-size">${formatBytes(r.sessionBytes)}</span>
      </div>`;
  }).join("");
  const summary = t("storage.summary", {
    sessions: t("storage.sessions", { n: totalSessions }),
    n: connected.length,
  });
  return `
    <article class="status-card storage-card">
      <div class="status-card-head">
        <span class="name">${escapeHtml(t("storage.title"))}</span>
        <span class="pill">${state.storage ? "DISK" : "MOCK"}</span>
      </div>
      <div class="storage-total">
        <strong>${formatBytes(total)}</strong>
        <span>${escapeHtml(summary)}</span>
      </div>
      <div class="storage-rows">${lines}</div>
    </article>`;
}

function renderStatus() {
  const grid = $("#status-grid");
  const storageByHarness = Object.fromEntries(storageRows().map((r) => [r.harness, r]));
  const paths = {
    cc: "~/.claude/projects/",
    kimi: "~/.kimi-code/sessions/",
    dsh: "~/.dsh/sessions/",
    codex: "~/.codex/sessions/",
  };
  const cards = [
    {
      name: t("status.proxyName"),
      pill: state.proxyOnline ? ["run", "ONLINE"] : ["warn", "OFFLINE"],
      kv: [
        [t("status.endpoint"), "http://127.0.0.1:8787/v1"],
        [t("status.protocol"), "OpenAI-compatible + Anthropic"],
        [t("status.config"), "~/.openplane/proxy.json"],
        [t("status.note"), t("status.proxyNote")],
      ],
    },
    ...HARNESS_IDS.map((hid) => {
      const h = HARNESS[hid];
      const list = state.sessions.filter((s) => s.harness === hid);
      const running = list.filter((s) => s.status === "running").length;
      const errors = list.filter((s) => s.status === "error").length;
      const pill = errors ? ["warn", `${errors} ERROR`] : running ? ["run", `${running} RUN`] : ["idle", "IDLE"];
      const st = storageByHarness[hid];
      return {
        name: h.name,
        pill,
        kv: [
          [t("status.sessions"), String(list.length)],
          [t("status.disk"), st && st.connected ? formatBytes(st.sessionBytes) : t("storage.notConnected")],
          [t("status.defaultModel"), state.routes[hid] || "—"],
          [t("status.scanPath"), paths[hid]],
          [t("status.access"), t("status.readLocal")],
        ],
      };
    }),
  ];

  grid.innerHTML = storageCardHtml() + cards.map((c) => `
    <article class="status-card">
      <div class="status-card-head">
        <span class="name">${escapeHtml(c.name)}</span>
        <span class="pill ${c.pill[0]}">${c.pill[1]}</span>
      </div>
      <dl class="kv">
        ${c.kv.map(([k, v]) => `<dt>${escapeHtml(k)}</dt><dd>${escapeHtml(v)}</dd>`).join("")}
      </dl>
    </article>`).join("");
}

function switchView(name) {
  state.view = name;
  $$(".view").forEach((v) => v.classList.toggle("active", v.id === `view-${name}`));
  $$(".sidebar > .nav-item[data-view]").forEach((el) => {
    el.classList.toggle("active", el.dataset.view === name);
  });
  if (name === "models") renderModels();
  if (name === "status") renderStatus();
}

async function refreshAll(boot = false) {
  await detectRuntime();
  await loadNativeProxy();
  await Promise.all([loadNativeSessions(), loadNativeStorage()]);
  rerenderAll(boot);
}

async function init() {
  setLocale(detectLocale());
  applyStatic();
  renderLangSwitch();
  const missing = missingKeys();
  if (Object.keys(missing).length) console.warn("[i18n] missing keys", missing);

  loadLocal();
  await refreshAll(true);

  $$(".sidebar > .nav-item[data-view]").forEach((el) => {
    el.addEventListener("click", () => switchView(el.dataset.view));
  });

  $("#search").addEventListener("input", (e) => {
    state.query = e.target.value;
    renderSessions();
  });

  $("#btn-ping").addEventListener("click", async () => {
    const st = await invokeTauri("ping_proxy");
    if (st && typeof st.online === "boolean") {
      state.proxyOnline = st.online;
      toast(st.online ? t("toast.proxyReachable", { ms: st.latency_ms || 0 }) : t("toast.proxyNoResponse"));
    } else {
      state.proxyOnline = Math.random() >= 0.15;
      toast(state.proxyOnline ? t("toast.proxyReachable", { ms: 12 }) + t("toast.simulated") : t("toast.proxyNoResponse"));
    }
    updateProxyRow();
    renderAnnunciator();
    renderStatus();
  });

  $("#btn-copy-endpoint").addEventListener("click", async () => {
    await copyText("http://127.0.0.1:8787/v1");
    toast(t("toast.endpointCopied"));
  });

  $("#btn-close-detail").addEventListener("click", closeDetail);
  $("#btn-refresh").addEventListener("click", async () => {
    await refreshAll(true);
    toast(t(state.source.startsWith("native") ? "toast.refreshedNative" : "toast.refreshedMock"));
  });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDetail();
  });
}

init();
