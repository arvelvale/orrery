/**
 * Orrery UI — WebView 前身
 * 在浏览器中用 mock；在 Tauri 里通过 invoke 调 Rust。
 * 所有面向用户的文案走 i18n.js 的 t()，这里不写死任何自然语言。
 */

import { LOCALES, applyStatic, detectLocale, formatRelative, getLocale, missingKeys, setLocale, t } from "./i18n.js";

const HARNESS = {
  cc:   { id: "cc",   label: "CC",   name: "Claude Code", badge: "cc" },
  kimi: { id: "kimi", label: "KIMI", name: "Kimi Code",   badge: "kimi" },
  dsh:  { id: "dsh",  label: "DSH",  name: "DSH",         badge: "dsh" },
  codex: { id: "codex", label: "CODEX", name: "Codex",     badge: "codex" },
  opencode: { id: "opencode", label: "OPENCODE", name: "OpenCode", badge: "opencode" },
  zcode: { id: "zcode", label: "ZCODE", name: "Z Code", badge: "zcode" },
  antigravity: { id: "antigravity", label: "ANTIGRAVITY", name: "Antigravity", badge: "antigravity" },
};

const HARNESS_IDS = Object.keys(HARNESS);

/*
 * 未知 harness 的兜底：用户可以在 ~/.orrery/harnesses.json 里登记自定义来源，
 * 它们的 id 不在上面那张表里，这里按需补一个中性角标，避免界面直接崩掉。
 */
function harnessOf(id) {
  if (HARNESS[id]) return HARNESS[id];
  HARNESS[id] = { id, label: String(id).toUpperCase().slice(0, 8), name: id, badge: "custom" };
  if (!HARNESS_IDS.includes(id)) HARNESS_IDS.push(id);
  return HARNESS[id];
}

const MODELS = [
  { id: "claude-opus-4.7", vendor: "anthropic", note: "models.note.flagship" },
  { id: "claude-sonnet-4.6", vendor: "anthropic", note: "models.note.balanced" },
  { id: "kimi-k3", vendor: "moonshot", note: "models.note.kimi" },
  { id: "gpt-5.2", vendor: "openai", note: "models.note.openai" },
  { id: "gpt-5.3-codex", vendor: "openai", note: "models.note.codex" },
  { id: "deepseek-v3.2", vendor: "deepseek", note: "models.note.value" },
  { id: "mimo-v2.5-pro", vendor: "xiaomi", note: "models.note.mimo" },
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
    model: "claude-sonnet-4.6",
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
    model: "kimi-k3",
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
    model: "claude-opus-4.7",
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
  {
    id: "ses_a4d0e7b1",
    harness: "opencode",
    title: {
      en: "ledger-sync · Reconcile duplicate entries",
      "zh-CN": "ledger-sync · 对账去掉重复流水",
      ja: "ledger-sync · 重複した仕訳を突き合わせる",
    },
    excerpt: {
      en: "Two imports created the same rows; dedupe by external id, not by amount.",
      "zh-CN": "两次导入生成了相同的流水，按外部 id 去重，不按金额。",
      ja: "2 回の取り込みで同じ行ができた。金額ではなく外部 ID で重複を排除する。",
    },
    project: "~/code/ledger-sync",
    model: "claude-sonnet-4.6",
    status: "idle",
    ago: 9 * 60 * MIN,
    usage: { input: 6400, cache_write: 900, cache_read: 88200, output: 5100, calls: 27 },
    sizeBytes: 14_680_064,
    subagents: 2,
    log: [
      ["ok", "found 312 duplicate rows"],
      ["", "waiting on the dedupe key decision"],
    ],
  },
  {
    id: "sess_5be02c71",
    harness: "zcode",
    title: {
      en: "trail-map · Cluster markers at low zoom",
      "zh-CN": "trail-map · 缩小时合并地图标记",
      ja: "trail-map · 縮小時にマーカーをまとめる",
    },
    excerpt: {
      en: "4,000 markers freeze the tab below zoom 9; cluster on the server side instead.",
      "zh-CN": "缩放到 9 级以下时 4000 个标记会卡死页面，改成服务端聚合。",
      ja: "ズーム 9 未満で 4,000 個のマーカーがタブを固める。サーバー側でまとめる。",
    },
    project: "~/code/trail-map",
    model: "glm-5.3-flash",
    status: "idle",
    ago: 5 * 60 * MIN,
    usage: { input: 5200, cache_write: 0, cache_read: 96400, output: 3100, calls: 31 },
    sizeBytes: 12_582_912,
    subagents: 0,
    log: [
      ["ok", "supercluster wired to /tiles"],
      ["", "benchmarking zoom 6–9"],
    ],
  },
  {
    id: "4c1e9a07-2b6d-4f3e-9a51-7d0c8e2f6b13",
    harness: "antigravity",
    title: {
      en: "recipe-box · Import recipes from a URL",
      "zh-CN": "recipe-box · 从网址导入菜谱",
      ja: "recipe-box · URL からレシピを取り込む",
    },
    excerpt: {
      en: "Most sites embed schema.org Recipe JSON-LD; fall back to readability for the rest.",
      "zh-CN": "大多数网站都内嵌了 schema.org 的 Recipe JSON-LD，其余的再用正文提取兜底。",
      ja: "多くのサイトは schema.org の Recipe JSON-LD を埋め込んでいる。残りは本文抽出で補う。",
    },
    project: "~/code/recipe-box",
    model: "gemini-3.8-flash",
    status: "idle",
    ago: 7 * 60 * MIN,
    usage: { input: 48200, cache_write: 0, cache_read: 512400, output: 6300, calls: 22 },
    sizeBytes: 1_153_434,
    subagents: 0,
    log: [
      ["ok", "JSON-LD parser handles 9 of 10 test sites"],
      ["", "adding the readability fallback"],
    ],
  },
];

const DEFAULT_ROUTES = {
  cc: "claude-sonnet-4.6",
  kimi: "kimi-k3",
  dsh: "deepseek-v3.2",
  codex: "gpt-5.3-codex",
  opencode: "claude-sonnet-4.6",
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
  proxy: null, // get_proxy_status 结果；浏览器预览用 mockProxy()
  proxyEdit: null, // get_proxy_config（含密钥，仅 Tauri）
  runtime: "browser", // browser | tauri
  source: "mock",
  storage: null, // Tauri: storage_stats 结果；浏览器预览按 mock 会话汇总
  selected: new Set(), // 勾选待删除的会话，键为 sessionKey()
  sort: "recent", // recent | size
};

/** 不同 harness 的会话 id 可能重名，选择集合用 harness:id */
function sessionKey(s) {
  return `${s.harness}:${s.id}`;
}

/* ── 本地化取值 ── */

/** mock 数据的标题/摘要按语言存；真实数据是用户原文字符串 */
function localized(v) {
  if (v && typeof v === "object") return v[getLocale()] ?? v.en ?? "";
  return v ?? "";
}

function sessionTitle(s) {
  const title = localized(s.title);
  if (title) return title;
  // DSH / Kimi 的 id 带 `session-` / `session_` 前缀，截短前先去掉
  const short = String(s.id).replace(/^session[-_]/, "").slice(0, 8);
  // 父会话不在本机的子 agent（Codex guardian 自动审查）：标题是发给它的系统指令，不展示
  return t(s.kind === "subagent" ? "session.subagent" : "session.untitled", { id: short });
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
        kind: r.kind || "",
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
    if (st && typeof st.running === "boolean") {
      state.proxy = st;
      state.routes = { ...DEFAULT_ROUTES, ...st.routes };
      await loadProxyEditConfig();
      return true;
    }
  } catch (e) {
    console.warn("get_proxy_status failed", e);
  }
  state.proxy = mockProxy();
  return false;
}

/** 编辑表单数据：含密钥明文；浏览器预览用 mock */
async function loadProxyEditConfig() {
  try {
    const cfg = await invokeTauri("get_proxy_config");
    if (cfg && cfg.providers) {
      state.proxyEdit = cfg;
      return true;
    }
  } catch (e) {
    console.warn("get_proxy_config failed", e);
  }
  state.proxyEdit = null;
  return false;
}

/** 浏览器预览没有后端，给一份"未启用"的状态，按钮点了只提示 */
function mockProxy() {
  const providers = [
    { name: "anthropic", base_url: "https://api.anthropic.com/v1", wire: "anthropic", key_env: "ANTHROPIC_API_KEY", key_present: false, key_source: "none", model_prefixes: ["claude"] },
    { name: "openai", base_url: "https://api.openai.com/v1", wire: "openai", key_env: "OPENAI_API_KEY", key_present: false, key_source: "none", model_prefixes: ["gpt"] },
    { name: "moonshot", base_url: "https://api.moonshot.cn/v1", wire: "openai", key_env: "MOONSHOT_API_KEY", key_present: false, key_source: "none", model_prefixes: ["kimi"] },
    { name: "deepseek", base_url: "https://api.deepseek.com/v1", wire: "openai", key_env: "DEEPSEEK_API_KEY", key_present: false, key_source: "none", model_prefixes: ["deepseek"] },
  ];
  const models = [
    { id: "claude-opus-4.7", provider: "anthropic" },
    { id: "claude-sonnet-4.6", provider: "anthropic" },
    { id: "kimi-k3", provider: "moonshot" },
    { id: "gpt-5.2", provider: "openai" },
    { id: "deepseek-v3.2", provider: "deepseek" },
  ];
  return {
    running: false, online: false, endpoint: "http://127.0.0.1:8787/v1", listen: "127.0.0.1:8787",
    auto_start: false, uptime_ms: 0, requests: 0, failures: 0, latency_ms: null,
    last_request: null, last_error: null, routes: { ...state.routes },
    config_path: "~/.orrery/proxy.json", message: "stopped",
    providers,
    models,
  };
}

function editProviders() {
  const edit = state.proxyEdit;
  if (edit?.providers) {
    return Object.entries(edit.providers).map(([name, p]) => ({
      name,
      base_url: p.base_url || "",
      wire: p.wire || "openai",
      api_key: p.api_key || "",
      api_key_env: p.api_key_env || "",
      model_prefixes: p.model_prefixes || [],
    }));
  }
  return (state.proxy || mockProxy()).providers.map((pv) => ({
    name: pv.name,
    base_url: pv.base_url,
    wire: pv.wire,
    api_key: "",
    api_key_env: pv.key_env || "",
    model_prefixes: pv.model_prefixes || [],
  }));
}

function editModels() {
  const edit = state.proxyEdit;
  if (edit?.models) {
    return edit.models.map((m) => ({ id: m.id, provider: m.provider }));
  }
  return (state.proxy || mockProxy()).models || [];
}

/** 三态：本进程在跑 / 端口被别的程序占着 / 未启用 */
function proxyState() {
  const p = state.proxy;
  if (!p) return "off";
  if (p.running) return "run";
  return p.online ? "warn" : "off";
}

/* ── Persistence ── */
const LS_KEY = "orrery.v1";
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
  renderProxyPanel();
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
    { id: "proxy", label: "PRX", state: proxyState(), title: t({ run: "lamp.proxyOn", warn: "lamp.proxyBusy", off: "lamp.proxyOff" }[proxyState()]) },
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

  // 排序：找占空间大户时按占用排
  const sort = document.createElement("div");
  sort.className = "sort-switch";
  sort.setAttribute("role", "group");
  sort.setAttribute("aria-label", t("sessions.sortLabel"));
  [["recent", "sessions.sortRecent"], ["size", "sessions.sortSize"]].forEach(([key, label]) => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "chip" + (state.sort === key ? " active" : "");
    b.textContent = t(label);
    b.setAttribute("aria-pressed", String(state.sort === key));
    b.addEventListener("click", () => {
      state.sort = key;
      renderFilters();
      renderSessions();
    });
    sort.appendChild(b);
  });
  wrap.appendChild(sort);
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
  const items = state.sessions.filter((s) => {
    if (state.filter !== "all" && s.harness !== state.filter) return false;
    if (!q) return true;
    return [sessionTitle(s), s.project, s.model, localized(s.excerpt), s.id].join(" ").toLowerCase().includes(q);
  });
  if (state.sort === "size") items.sort((a, b) => (b.sizeBytes || 0) - (a.sizeBytes || 0));
  return items;
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
    const h = harnessOf(s.harness);
    const key = sessionKey(s);
    const checked = state.selected.has(key);
    // 情报条内含复选框，不能用 <button> 嵌套交互元素
    const btn = document.createElement("div");
    btn.setAttribute("role", "button");
    btn.tabIndex = 0;
    btn.className = "strip" + (state.selectedId === s.id ? " selected" : "") + (checked ? " checked" : "");
    btn.dataset.status = s.status;
    btn.dataset.id = s.id;
    btn.innerHTML = `
      <span class="strip-bar" aria-hidden="true"></span>
      <label class="strip-check" title="${escapeHtml(t("select.toggle"))}">
        <input type="checkbox" ${checked ? "checked" : ""} aria-label="${escapeHtml(t("select.toggle"))}" />
      </label>
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
    const check = btn.querySelector(".strip-check");
    check.addEventListener("click", (e) => e.stopPropagation());
    check.querySelector("input").addEventListener("change", (e) => {
      if (e.target.checked) state.selected.add(key);
      else state.selected.delete(key);
      btn.classList.toggle("checked", e.target.checked);
      renderSelectionBar();
    });
    btn.addEventListener("click", () => openSession(s.id));
    btn.addEventListener("keydown", (e) => {
      if (e.target !== btn) return;
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        openSession(s.id);
      }
    });
    list.appendChild(btn);
  });
  renderSelectionBar();
}

/* ── 选择与删除 ── */

function selectedSessions() {
  return state.sessions.filter((s) => state.selected.has(sessionKey(s)));
}

function renderSelectionBar() {
  const bar = $("#selbar");
  // 已不存在的会话（被删除或重新扫描后消失）从选择里移除
  const alive = new Set(state.sessions.map(sessionKey));
  for (const k of [...state.selected]) if (!alive.has(k)) state.selected.delete(k);

  const picked = selectedSessions();
  bar.hidden = picked.length === 0;
  if (!picked.length) return;
  const size = picked.reduce((n, s) => n + (s.sizeBytes || 0), 0);
  bar.innerHTML = `
    <span class="selbar-count">${escapeHtml(t("select.count", { n: picked.length, size: formatBytes(size) }))}</span>
    <button type="button" class="btn" data-sel="all">${escapeHtml(t("select.all"))}</button>
    <button type="button" class="btn" data-sel="clear">${escapeHtml(t("select.clear"))}</button>
    <button type="button" class="btn danger" data-sel="delete">${escapeHtml(t("select.delete"))}</button>`;
  bar.querySelector('[data-sel="all"]').addEventListener("click", () => {
    filteredSessions().forEach((s) => state.selected.add(sessionKey(s)));
    renderSessions();
  });
  bar.querySelector('[data-sel="clear"]').addEventListener("click", () => {
    state.selected.clear();
    renderSessions();
  });
  bar.querySelector('[data-sel="delete"]').addEventListener("click", () => openDeleteDialog(picked));
}

const dialog = { targets: [], plans: [], mode: "trash", ack: false, phase: "plan", results: [] };

/** 浏览器预览：按 mock 会话模拟后端的计划（运行中的会话按"最近写入"拦下，便于演示保护逻辑） */
function mockPlans(targets) {
  // 与后端同口径：OpenCode 走 CLI、不动文件；只在自己库里的工具只读
  const readOnly = (h) => ["zcode", "antigravity"].includes(h);
  const noFiles = (h) => h === "opencode" || readOnly(h);
  return targets.map((s) => ({
    harness: s.harness,
    id: s.id,
    files: noFiles(s.harness) ? [] : [s.project],
    bytes: readOnly(s.harness) ? 0 : s.sizeBytes || 0,
    index_files: noFiles(s.harness) || s.harness === "cc" ? [] : ["session_index.jsonl"],
    codex_threads: s.harness === "codex" ? [s.id] : [],
    cli_sessions: s.harness === "opencode" ? [s.id] : [],
    blocked: readOnly(s.harness) ? "read_only" : s.status === "running" ? "active" : null,
    warnings: [],
  }));
}

async function openDeleteDialog(sessions) {
  dialog.targets = sessions;
  dialog.mode = "trash";
  dialog.ack = false;
  dialog.phase = "loading";
  dialog.results = [];
  showModal();
  renderDeleteDialog();
  const targets = sessions.map((s) => ({ harness: s.harness, id: s.id }));
  try {
    dialog.plans = state.runtime === "tauri" ? await invokeTauri("plan_delete", { targets }) : mockPlans(sessions);
  } catch (e) {
    console.warn("plan_delete failed", e);
    dialog.plans = [];
    dialog.error = String(e);
  }
  dialog.phase = "plan";
  renderDeleteDialog();
}

function findTarget(p) {
  return dialog.targets.find((s) => s.harness === p.harness && s.id === p.id);
}

function planRowHtml(p, extra = "") {
  const s = findTarget(p);
  const h = HARNESS[p.harness] || HARNESS.cc;
  return `
    <li class="del-row">
      <span class="badge ${h.badge}">${h.label}</span>
      <span class="del-title">${escapeHtml(s ? sessionTitle(s) : p.id)}</span>
      <span class="del-size${extra ? " reason" : ""}">${extra || formatBytes(p.bytes)}</span>
    </li>`;
}

function listHtml(rows, render) {
  const shown = rows.slice(0, 6).map(render).join("");
  const more = rows.length > 6 ? `<li class="del-more">${escapeHtml(t("del.more", { n: rows.length - 6 }))}</li>` : "";
  return `<ul class="del-list">${shown}${more}</ul>`;
}

function renderDeleteDialog() {
  const box = $("#modal-box");
  if (dialog.phase === "loading") {
    box.innerHTML = `<p class="hint">${escapeHtml(t("del.planning"))}</p>`;
    return;
  }
  if (dialog.phase === "done") return renderDeleteResults();

  const ok = dialog.plans.filter((p) => !p.blocked);
  const blocked = dialog.plans.filter((p) => p.blocked);
  const bytes = ok.reduce((n, p) => n + p.bytes, 0);
  const files = ok.reduce((n, p) => n + p.files.length, 0);
  const warnings = [...new Set(ok.flatMap((p) => p.warnings.map((w) => `${w}|${p.harness}`)))];
  const hasIndex = ok.some((p) => p.index_files.length);
  const hasCodex = ok.some((p) => p.harness === "codex");
  const hasOpencode = ok.some((p) => p.harness === "opencode");
  // 全是 OpenCode 时"可从回收站找回"不成立，顶部直接换成导出说明
  const onlyOpencode = hasOpencode && ok.every((p) => p.harness === "opencode");
  const permanent = dialog.mode === "permanent";
  // OpenCode 全在它的库里，没有文件可列；全是这种会话时不显示"0 个文件"
  const summary = files
    ? t("del.summary", { n: ok.length, files: t("del.files", { n: files }) })
    : t("del.summary", { n: ok.length, files: "" }).replace(/\s*·\s*$/, "");
  const working = dialog.phase === "working";
  const canConfirm = ok.length > 0 && (!permanent || dialog.ack) && !working;

  box.innerHTML = `
    <h2 id="modal-title">${escapeHtml(t("del.title", { n: dialog.targets.length }))}</h2>
    <div class="seg" role="radiogroup" aria-label="${escapeHtml(t("del.modeLabel"))}">
      ${[["trash", "del.modeTrash"], ["permanent", "del.modePermanent"]].map(([m, label]) => `
        <button type="button" role="radio" class="seg-opt${dialog.mode === m ? " active" : ""}" aria-checked="${dialog.mode === m}" data-mode="${m}" ${working ? "disabled" : ""}>${escapeHtml(t(label))}</button>`).join("")}
    </div>
    <p class="del-note${permanent ? " danger" : ""}">${escapeHtml(t(permanent ? "del.permanentNote" : onlyOpencode ? "del.opencodeTrashNote" : "del.trashNote"))}</p>
    ${ok.length ? `
      <div class="del-summary">
        <strong>${formatBytes(bytes)}</strong>
        <span>${escapeHtml(summary)}</span>
      </div>
      ${listHtml(ok, (p) => planRowHtml(p))}` : `<p class="hint">${escapeHtml(t("del.nothing"))}</p>`}
    ${blocked.length ? `
      <h3>${escapeHtml(t("del.blockedTitle", { n: blocked.length }))}</h3>
      ${listHtml(blocked, (p) => planRowHtml(p, escapeHtml(t(`del.reason.${p.blocked}`))))}` : ""}
    ${warnings.map((w) => {
      const [code, hid] = w.split("|");
      return `<p class="del-warn">${escapeHtml(t(`del.warn.${code}`, { name: (HARNESS[hid] || {}).name || hid }))}</p>`;
    }).join("")}
    ${hasIndex ? `<p class="del-fine">${escapeHtml(t("del.indexNote"))}</p>` : ""}
    ${hasCodex ? `<p class="del-fine">${escapeHtml(t("del.codexNote"))}</p>` : ""}
    ${hasOpencode ? `<p class="del-fine">${escapeHtml(t("del.opencodeNote"))}${permanent || onlyOpencode ? "" : ` ${escapeHtml(t("del.opencodeTrashNote"))}`}</p>` : ""}
    ${state.runtime !== "tauri" ? `<p class="del-fine">${escapeHtml(t("del.previewNote"))}</p>` : ""}
    ${permanent && ok.length ? `
      <label class="del-ack"><input type="checkbox" id="del-ack" ${dialog.ack ? "checked" : ""} ${working ? "disabled" : ""} /> ${escapeHtml(t("del.ack"))}</label>` : ""}
    <div class="btn-row modal-actions">
      <button type="button" class="btn" data-act="cancel" ${working ? "disabled" : ""}>${escapeHtml(t("del.cancel"))}</button>
      <button type="button" class="btn ${permanent ? "danger solid" : "danger"}" data-act="confirm" ${canConfirm ? "" : "disabled"}>
        ${escapeHtml(t(working ? "del.working" : permanent ? "del.confirmPermanent" : "del.confirmTrash"))}
      </button>
    </div>`;

  box.querySelectorAll("[data-mode]").forEach((b) => b.addEventListener("click", () => {
    dialog.mode = b.dataset.mode;
    dialog.ack = false;
    renderDeleteDialog();
  }));
  const ack = box.querySelector("#del-ack");
  if (ack) ack.addEventListener("change", (e) => {
    dialog.ack = e.target.checked;
    renderDeleteDialog();
  });
  box.querySelector('[data-act="cancel"]').addEventListener("click", hideModal);
  box.querySelector('[data-act="confirm"]').addEventListener("click", () => runDelete(ok));
}

async function runDelete(plans) {
  dialog.phase = "working";
  renderDeleteDialog();
  const targets = plans.map((p) => ({ harness: p.harness, id: p.id }));
  const before = storageRows().filter((r) => r.connected).reduce((n, r) => n + r.sessionBytes, 0);

  if (state.runtime === "tauri") {
    try {
      dialog.results = await invokeTauri("delete_sessions", { targets, mode: dialog.mode });
    } catch (e) {
      dialog.results = plans.map((p) => ({ ...p, ok: false, error: String(e) }));
    }
  } else {
    // 预览模式只从列表移除，不碰任何文件
    dialog.results = plans.map((p) => ({ harness: p.harness, id: p.id, ok: true, bytes: p.bytes, mode: dialog.mode }));
    const gone = new Set(plans.map((p) => `${p.harness}:${p.id}`));
    state.sessions = state.sessions.filter((s) => !gone.has(sessionKey(s)));
  }

  // 一次删除完成即结束这轮选择；被保护而跳过的会话不留在勾选里，避免混进下一次删除
  state.selected.clear();
  if (state.runtime === "tauri") await refreshAll();
  else rerenderAll();

  // 核对：列表统计的占用下降量应与后端报告的删除量一致
  const after = storageRows().filter((r) => r.connected).reduce((n, r) => n + r.sessionBytes, 0);
  dialog.verified = { reported: dialog.results.filter((r) => r.ok).reduce((n, r) => n + (r.bytes || 0), 0), dropped: before - after };
  dialog.phase = "done";
  renderDeleteDialog();
}

function errorLabel(err) {
  if (!err) return "";
  const code = String(err).split(":")[0];
  if (code === "blocked") return t(`del.reason.${String(err).split(":")[1]}`);
  return t(`del.error.${code}`, {}) === `del.error.${code}` ? String(err) : t(`del.error.${code}`);
}

function renderDeleteResults() {
  const box = $("#modal-box");
  const ok = dialog.results.filter((r) => r.ok);
  const failed = dialog.results.filter((r) => !r.ok);
  const bytes = ok.reduce((n, r) => n + (r.bytes || 0), 0);
  const v = dialog.verified || { reported: 0, dropped: 0 };
  const backups = [...new Set(ok.map((r) => r.backup_dir).filter(Boolean))];
  // 每条会话一个导出子目录，显示它们共同的上级（本批的时间戳目录）
  const exports = [...new Set(ok.map((r) => r.export_dir).filter(Boolean))];
  const codexFallback = ok.some((r) => r.codex_cli === "fallback" || r.codex_cli === "missing");
  box.innerHTML = `
    <h2 id="modal-title">${escapeHtml(t("del.doneTitle"))}</h2>
    <div class="del-summary">
      <strong>${formatBytes(bytes)}</strong>
      <span>${escapeHtml(t(dialog.mode === "permanent" ? "del.doneFreed" : ok.length && ok.every((r) => r.export_dir) ? "del.doneExported" : "del.doneTrashed", { n: ok.length }))}</span>
    </div>
    ${ok.length ? `<p class="del-fine">${escapeHtml(t("del.verify", { reported: formatBytes(v.reported), dropped: formatBytes(Math.max(0, v.dropped)) }))}</p>` : ""}
    ${failed.length ? `
      <h3>${escapeHtml(t("del.failedTitle", { n: failed.length }))}</h3>
      ${listHtml(failed, (r) => planRowHtml(r, escapeHtml(errorLabel(r.error))))}` : ""}
    ${codexFallback ? `<p class="del-warn">${escapeHtml(t("del.codexFallback"))}</p>` : ""}
    ${backups.length ? `<p class="del-fine">${escapeHtml(t("del.backups", { path: backups[0].replace(/[\\/][^\\/]+$/, "") }))}</p>` : ""}
    ${exports.length ? `<p class="del-fine">${escapeHtml(t("del.exports", { path: exports.length === 1 ? exports[0] : exports[0].replace(/[\\/][^\\/]+$/, "") }))}</p>` : ""}
    <div class="btn-row modal-actions">
      <button type="button" class="btn primary" data-act="close">${escapeHtml(t("del.close"))}</button>
    </div>`;
  box.querySelector('[data-act="close"]').addEventListener("click", hideModal);
}

let lastFocus = null;
function showModal() {
  lastFocus = document.activeElement;
  $("#modal").hidden = false;
  requestAnimationFrame(() => $("#modal-box").focus());
}
function hideModal() {
  if (dialog.phase === "working") return;
  $("#modal").hidden = true;
  if (lastFocus && document.contains(lastFocus)) lastFocus.focus();
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
  const h = harnessOf(s.harness);
  const log = (s.log || []).map(([cls, line]) => {
    const c = cls ? ` class="${cls}"` : "";
    return `<span${c}>${escapeHtml(line)}</span>`;
  }).join("\n");
  // OpenCode 的 session 汇总没有可靠的 API 调用次数，不能把未知显示成 0 次。
  const calls = s.usage && !(s.harness === "opencode" && !s.usage.calls)
    ? ` · ${escapeHtml(t("detail.calls", { n: s.usage.calls }))}` : "";
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
      <button type="button" class="btn" data-act="open-folder">${escapeHtml(t("detail.openFolder"))}</button>
      <button type="button" class="btn" data-act="copy-path">${escapeHtml(t("detail.copyPath"))}</button>
      <button type="button" class="btn danger" data-act="delete">${escapeHtml(t("detail.delete"))}</button>
    </div>`;
}

function openSession(id) {
  state.selectedId = id;
  const s = findSession(id);
  if (!s) return;
  const h = harnessOf(s.harness);
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

/*
 * 在终端里恢复会话。后端返回将要执行的命令，成功时原样提示给用户，
 * 失败时按错误码给出能直接照做的说明（缺 CLI / 目录没了 / 该 harness 不支持）。
 */
async function resumeSession(s) {
  if (state.runtime !== "tauri") {
    toast(t("toast.previewWouldResume", { harness: HARNESS[s.harness]?.name || s.harness }));
    return;
  }
  try {
    const cmd = await invokeTauri("resume_session", { harness: s.harness, id: s.id, project: s.project });
    // DSH 只装了 web profile 时打开的是网页界面，不是直接恢复这条会话，要说清楚
    if (String(cmd).startsWith("web_fallback:")) toast(t("toast.resumedWebUi", { cmd: String(cmd).slice(13) }));
    else toast(t("toast.resumed", { cmd }));
  } catch (err) {
    const code = String(err || "");
    if (code.startsWith("cli_missing:")) toast(t("toast.resumeNoCli", { cli: code.split(":")[1] }));
    else if (code === "dsh_no_terminal_profile") toast(t("toast.resumeDshProfile"));
    else if (code === "cwd_missing") toast(t("toast.resumeNoCwd", { path: s.project }));
    else if (code === "unsupported_harness") toast(t("toast.resumeUnsupported"));
    else toast(t("toast.resumeFailed", { msg: code.slice(0, 80) }));
  }
}

function wireDetailActions(root, s) {
  root.querySelectorAll("[data-act]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const act = btn.getAttribute("data-act");
      if (act === "resume") {
        await resumeSession(s);
      } else if (act === "open-folder") {
        const path = s.path || s.project;
        const res = await invokeTauri("open_path", { path });
        if (res === true) toast(t("toast.openedExplorer"));
        else toast(state.runtime === "tauri" ? t("toast.openFailed") : t("toast.previewWouldOpen", { path }));
      } else if (act === "copy-path") {
        await copyText(s.project);
        toast(t("toast.pathCopied"));
      } else if (act === "delete") {
        openDeleteDialog([s]);
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

function modelOptionsHtml(selected) {
  const models = editModels();
  const ids = models.map((m) => m.id);
  if (selected && !ids.includes(selected)) ids.push(selected);
  if (!ids.length) ids.push("");
  return ids
    .map((id) => `<option value="${escapeHtml(id)}" ${id === selected ? "selected" : ""}>${escapeHtml(id || "—")}</option>`)
    .join("");
}

async function reloadProxyUi() {
  await loadNativeProxy();
  state.routes = { ...DEFAULT_ROUTES, ...(state.proxy?.routes || {}) };
  renderModels();
  renderStatus();
  renderAnnunciator();
}

function renderModels() {
  renderProxyPanel();
  renderProviderEditor();
  renderModelEditor();
  renderRoutesEditor();
  updateProxyRow();
}

function renderRoutesEditor() {
  const routes = $("#routes");
  if (!routes) return;
  routes.innerHTML = "";
  HARNESS_IDS.forEach((hid) => {
    const h = HARNESS[hid];
    const current = state.routes[hid] || "";
    const row = document.createElement("div");
    row.className = "route";
    row.innerHTML = `
      <div class="route-harness">
        <div class="name">${escapeHtml(h.name)}</div>
        <div class="sub">${escapeHtml(h.label)} · ${escapeHtml(t("models.routesDefault"))}</div>
      </div>
      <div class="route-arrow" aria-hidden="true">→</div>
      <div class="route-model">
        <label class="sr-only" for="route-${hid}">${escapeHtml(t("models.defaultFor", { name: h.name }))}</label>
        <select id="route-${hid}">${modelOptionsHtml(current)}</select>
      </div>`;
    routes.appendChild(row);
    row.querySelector("select").addEventListener("change", async (e) => {
      state.routes[hid] = e.target.value;
      saveLocal();
      await invokeTauri("save_route", { harness: hid, model: e.target.value }).catch((err) => console.warn(err));
      toast(`${h.name} → ${e.target.value || "—"}`);
      if (state.proxy) state.proxy.routes = { ...state.routes };
    });
  });
}

function renderProviderEditor() {
  const list = $("#provider-list");
  const form = $("#provider-form");
  if (!list || !form) return;
  const providers = editProviders();

  list.innerHTML = providers
    .map((pv) => {
      const hasKey = !!(state.proxyEdit?.providers?.[pv.name]?.api_key) || false;
      const st = (state.proxy?.providers || []).find((x) => x.name === pv.name);
      const present = st ? st.key_present : hasKey;
      const source = st ? st.key_source : hasKey ? "config" : "none";
      const label = present
        ? source === "config"
          ? t("proxy.keyFromConfig")
          : t("proxy.keyFromEnv")
        : t("proxy.keyMissing");
      const pillCls = present ? (source === "config" ? "src-config" : "ok") : "none";
      return `
        <div class="edit-item" data-provider="${escapeHtml(pv.name)}">
          <div class="edit-item-head">
            <span class="name">${escapeHtml(pv.name)}</span>
            <span class="pill ${pillCls}">${escapeHtml(label)}</span>
            <button type="button" class="btn small" data-act="edit-provider">${escapeHtml(t("models.edit"))}</button>
            <button type="button" class="btn small danger" data-act="rm-provider">${escapeHtml(t("models.remove"))}</button>
          </div>
          <div class="meta">${escapeHtml(pv.wire)} · ${escapeHtml(pv.base_url)}${
            pv.api_key_env ? ` · env:${escapeHtml(pv.api_key_env)}` : ""
          }${pv.model_prefixes?.length ? ` · ${escapeHtml(pv.model_prefixes.join(", "))}` : ""}</div>
        </div>`;
    })
    .join("");

  form.innerHTML = providerFormHtml(null);
  wireProviderForm(form, null);

  list.querySelectorAll("[data-act='edit-provider']").forEach((btn) => {
    btn.addEventListener("click", () => {
      const name = btn.closest("[data-provider]")?.dataset.provider;
      const pv = providers.find((p) => p.name === name);
      form.innerHTML = providerFormHtml(pv);
      wireProviderForm(form, pv?.name || null);
    });
  });
  list.querySelectorAll("[data-act='rm-provider']").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const name = btn.closest("[data-provider]")?.dataset.provider;
      if (!name) return;
      if (!confirm(t("models.confirmRemoveProvider", { name }))) return;
      try {
        await invokeTauri("remove_provider", { name });
        toast(t("models.removed", { name }));
        await reloadProxyUi();
      } catch (e) {
        toast(String(e));
      }
    });
  });
}

function providerFormHtml(pv) {
  const prefixes = (pv?.model_prefixes || []).join(", ");
  return `
    <h3>${pv ? escapeHtml(t("models.editProvider", { name: pv.name })) : escapeHtml(t("models.addProvider"))}</h3>
    <div class="field">
      <label for="pf-name">${escapeHtml(t("models.fieldName"))}</label>
      <input id="pf-name" value="${escapeHtml(pv?.name || "")}" placeholder="anthropic / my-proxy" ${pv ? "readonly" : ""} />
    </div>
    <div class="field">
      <label for="pf-base">${escapeHtml(t("models.fieldBaseUrl"))}</label>
      <input id="pf-base" value="${escapeHtml(pv?.base_url || "")}" placeholder="https://api.anthropic.com/v1" />
    </div>
    <div class="field-row">
      <div class="field">
        <label for="pf-wire">${escapeHtml(t("models.fieldWire"))}</label>
        <select id="pf-wire">
          <option value="openai" ${pv?.wire === "openai" || !pv ? "selected" : ""}>OpenAI-compatible</option>
          <option value="anthropic" ${pv?.wire === "anthropic" ? "selected" : ""}>Anthropic Messages</option>
        </select>
      </div>
      <div class="field">
        <label for="pf-env">${escapeHtml(t("models.fieldKeyEnv"))}</label>
        <input id="pf-env" value="${escapeHtml(pv?.api_key_env || "")}" placeholder="ANTHROPIC_API_KEY" />
      </div>
    </div>
    <div class="field">
      <label for="pf-key">${escapeHtml(t("models.fieldApiKey"))}</label>
      <div class="key-row">
        <input id="pf-key" type="password" value="${escapeHtml(pv?.api_key || "")}" placeholder="${escapeHtml(t("models.keyPlaceholder"))}" autocomplete="off" />
        <button type="button" class="btn small" id="pf-key-toggle">${escapeHtml(t("models.showKey"))}</button>
      </div>
    </div>
    <div class="field">
      <label for="pf-prefix">${escapeHtml(t("models.fieldPrefixes"))}</label>
      <input id="pf-prefix" value="${escapeHtml(prefixes)}" placeholder="claude, my-llm" />
    </div>
    <div class="btn-row">
      <button type="button" class="btn primary" id="pf-save">${escapeHtml(t("models.save"))}</button>
    </div>`;
}

function wireProviderForm(form, existingName) {
  const keyInput = form.querySelector("#pf-key");
  const toggle = form.querySelector("#pf-key-toggle");
  if (toggle && keyInput) {
    toggle.addEventListener("click", () => {
      const show = keyInput.type === "password";
      keyInput.type = show ? "text" : "password";
      toggle.textContent = show ? t("models.hideKey") : t("models.showKey");
    });
  }
  form.querySelector("#pf-save")?.addEventListener("click", async () => {
    const name = form.querySelector("#pf-name").value.trim() || existingName;
    const base_url = form.querySelector("#pf-base").value.trim();
    const wire = form.querySelector("#pf-wire").value;
    const api_key_env = form.querySelector("#pf-env").value.trim();
    const api_key = form.querySelector("#pf-key").value;
    const model_prefixes = form
      .querySelector("#pf-prefix")
      .value.split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    if (!name) {
      toast(t("models.fieldName"));
      return;
    }
    try {
      await invokeTauri("save_provider", {
        name,
        baseUrl: base_url,
        wire,
        apiKey: api_key,
        apiKeyEnv: api_key_env,
        modelPrefixes: model_prefixes,
      });
      toast(t("models.saved", { name }));
      await reloadProxyUi();
    } catch (e) {
      toast(String(e));
    }
  });
}

function renderModelEditor() {
  const list = $("#model-list");
  const form = $("#model-form");
  if (!list || !form) return;
  const models = editModels();
  const providers = editProviders();

  list.innerHTML = models
    .map(
      (m) => `
      <div class="edit-item" data-model="${escapeHtml(m.id)}">
        <div class="edit-item-head">
          <code class="name">${escapeHtml(m.id)}</code>
          <span class="tag">${escapeHtml(m.provider)}</span>
          <button type="button" class="btn small danger" data-act="rm-model">${escapeHtml(t("models.remove"))}</button>
        </div>
      </div>`
    )
    .join("");

  form.innerHTML = `
    <h3>${escapeHtml(t("models.addModel"))}</h3>
    <div class="field-row">
      <div class="field">
        <label for="mf-id">${escapeHtml(t("models.fieldModelId"))}</label>
        <input id="mf-id" placeholder="claude-sonnet-4.6" />
      </div>
      <div class="field">
        <label for="mf-provider">${escapeHtml(t("models.fieldProvider"))}</label>
        <select id="mf-provider">${providers
          .map((p) => `<option value="${escapeHtml(p.name)}">${escapeHtml(p.name)}</option>`)
          .join("")}</select>
      </div>
    </div>
    <div class="btn-row">
      <button type="button" class="btn primary" id="mf-save">${escapeHtml(t("models.save"))}</button>
    </div>`;

  form.querySelector("#mf-save")?.addEventListener("click", async () => {
    const id = form.querySelector("#mf-id").value.trim();
    const provider = form.querySelector("#mf-provider").value;
    if (!id) {
      toast(t("models.fieldModelId"));
      return;
    }
    try {
      await invokeTauri("save_model", { id, provider });
      toast(t("models.saved", { name: id }));
      await reloadProxyUi();
    } catch (e) {
      toast(String(e));
    }
  });

  list.querySelectorAll("[data-act='rm-model']").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const id = btn.closest("[data-model]")?.dataset.model;
      if (!id) return;
      try {
        await invokeTauri("remove_model", { id });
        toast(t("models.removed", { name: id }));
        await reloadProxyUi();
      } catch (e) {
        toast(String(e));
      }
    });
  });
}

function updateProxyRow() {
  const p = state.proxy || mockProxy();
  const mode = proxyState();
  const dot = $("#proxy-dot");
  const tag = $("#proxy-tag");
  const chip = $("#proxy-chip");
  chip.classList.toggle("off", mode !== "run");
  chip.classList.toggle("busy", mode === "warn");
  $("#proxy-chip-text").textContent = mode === "run" ? p.listen : t(mode === "warn" ? "proxy.busyChip" : "proxy.stoppedChip");
  chip.title = t(mode === "run" ? "lamp.proxyOn" : mode === "warn" ? "lamp.proxyBusy" : "lamp.proxyOff");
  if (!dot || !tag) return;
  dot.classList.toggle("off", mode !== "run");
  tag.textContent = { run: "RUNNING", warn: "PORT IN USE", off: "STOPPED" }[mode];
  tag.style.color = { run: "var(--run)", warn: "var(--warn)", off: "var(--muted)" }[mode];
}

function formatUptime(ms) {
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`;
  return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
}

/** 端点卡片：只显示运行信息；供应商/模型在下方可编辑卡片 */
function renderProxyPanel() {
  const p = state.proxy || mockProxy();
  const mode = proxyState();
  const rows = [];
  if (mode === "run") {
    rows.push([t("proxy.uptime"), formatUptime(p.uptime_ms)]);
    rows.push([t("proxy.requests"), `${p.requests}${p.failures ? " · " + t("proxy.failures", { n: p.failures }) : ""}`]);
    if (p.last_request) {
      rows.push([t("proxy.lastRequest"), `${p.last_request.provider} · ${p.last_request.model} · HTTP ${p.last_request.status}`]);
    }
  }
  if (p.last_error) rows.push([t("proxy.lastError"), p.last_error]);
  rows.push([t("proxy.config"), p.config_path]);

  $("#proxy-info").innerHTML = `<dl class="kv">${rows
    .map(([k, v]) => `<dt>${escapeHtml(k)}</dt><dd>${escapeHtml(v)}</dd>`)
    .join("")}</dl>`;

  const toggle = $("#btn-proxy-toggle");
  toggle.textContent = t(mode === "run" ? "proxy.stop" : "proxy.start");
  toggle.classList.toggle("primary", mode !== "run");
  toggle.disabled = mode === "warn";
  $("#proxy-autostart").checked = !!p.auto_start;
  $("#proxy-endpoint").textContent = p.endpoint;
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
      pill: { run: ["run", "RUNNING"], warn: ["warn", "PORT IN USE"], off: ["idle", "STOPPED"] }[proxyState()],
      kv: [
        [t("status.endpoint"), (state.proxy || mockProxy()).endpoint],
        [t("status.protocol"), "OpenAI-compatible + Anthropic"],
        [t("proxy.requests"), String((state.proxy || mockProxy()).requests)],
        [t("status.config"), (state.proxy || mockProxy()).config_path],
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
    if (st && typeof st.running === "boolean") {
      state.proxy = st;
      toast(st.online ? t("toast.proxyReachable", { ms: st.latency_ms ?? 0 }) : t("toast.proxyNoResponse"));
    } else {
      toast(t("toast.proxyNeedsDesktop"));
    }
    updateProxyRow();
    renderProxyPanel();
    renderAnnunciator();
    renderStatus();
  });

  $("#btn-proxy-toggle").addEventListener("click", async () => {
    const running = proxyState() === "run";
    const btn = $("#btn-proxy-toggle");
    btn.disabled = true;
    try {
      const st = await invokeTauri(running ? "stop_proxy" : "start_proxy");
      if (st && typeof st.running === "boolean") {
        state.proxy = st;
        toast(t(st.running ? "toast.proxyStarted" : "toast.proxyStopped", { listen: st.listen }));
      } else {
        toast(t("toast.proxyNeedsDesktop"));
      }
    } catch (e) {
      // 端口被占、监听地址不是回环地址等都会走这里
      toast(t("toast.proxyFailed", { error: String(e) }));
    }
    btn.disabled = false;
    updateProxyRow();
    renderProxyPanel();
    renderAnnunciator();
    renderStatus();
  });

  $("#proxy-autostart").addEventListener("change", async (e) => {
    const ok = await invokeTauri("set_proxy_auto_start", { enabled: e.target.checked });
    if (ok === null) {
      toast(t("toast.proxyNeedsDesktop"));
      e.target.checked = false;
      return;
    }
    if (state.proxy) state.proxy.auto_start = e.target.checked;
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

  $("#modal").addEventListener("click", (e) => {
    if (e.target.id === "modal") hideModal();
  });
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    if (!$("#modal").hidden) hideModal();
    else closeDetail();
  });
}

init();
