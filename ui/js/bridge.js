/**
 * Tauri 桥：所有 invoke 收口在这里。
 *
 * 浏览器预览没有后端，每个 `invokeTauri` 返回 null；调用方据此走 mock 分支。
 * 取不到 `window.__TAURI__` 时不抛错——预览模式是常态，不是异常。
 */

import { t } from "../i18n.js";
import { state } from "./state.js";
import { HARNESS_IDS } from "./harness.js";
import { DEFAULT_ROUTES } from "./data.js";

export const LS_KEY = "orrery.v1";

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

/** 返回 null = 不在 Tauri 里；抛错 = 后端真的失败了，两者要分开处理 */
export async function invokeTauri(cmd, args = {}) {
  const tauri = await getTauri();
  if (!tauri) return null;
  return tauri.invoke(cmd, args);
}

export async function detectRuntime() {
  const tauri = await getTauri();
  state.runtime = tauri ? "tauri" : "browser";
  renderRuntimeBadge();
  return tauri;
}

export function renderRuntimeBadge() {
  const badge = document.getElementById("build-badge");
  if (badge) badge.textContent = t(state.runtime === "tauri" ? "runtime.tauri" : "runtime.browser");
}

export async function loadNativeSessions() {
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

export async function loadNativeStorage() {
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
export function storageRows() {
  if (state.storage) return state.storage;
  return HARNESS_IDS.map((hid) => {
    const list = state.sessions.filter((s) => s.harness === hid);
    const bytes = list.reduce((n, s) => n + (s.sizeBytes || 0), 0);
    return { harness: hid, connected: list.length > 0, sessions: list.length, sessionBytes: bytes, rootBytes: bytes, root: "" };
  });
}

export async function loadNativeProxy() {
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
export function mockProxy() {
  const providers = [
    { name: "anthropic", base_url: "https://api.anthropic.com/v1", wire: "anthropic", key_env: "ANTHROPIC_API_KEY", key_present: false, key_source: "none", model_prefixes: ["claude"] },
    { name: "openai", base_url: "https://api.openai.com/v1", wire: "openai", key_env: "OPENAI_API_KEY", key_present: false, key_source: "none", model_prefixes: ["gpt"] },
    { name: "moonshot", base_url: "https://moonshot.cn/v1", wire: "openai", key_env: "MOONSHOT_API_KEY", key_present: false, key_source: "none", model_prefixes: ["kimi"] },
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

/** 编辑表单优先读 get_proxy_config（含明文密钥），退回读状态里的 providers */
export function editProviders() {
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

export function editModels() {
  const edit = state.proxyEdit;
  if (edit?.models) {
    return edit.models.map((m) => ({ id: m.id, provider: m.provider }));
  }
  return (state.proxy || mockProxy()).models || [];
}

/** 三态：本进程在跑 / 端口被别的程序占着 / 未启用 */
export function proxyState() {
  const p = state.proxy;
  if (!p) return "off";
  if (p.running) return "run";
  return p.online ? "warn" : "off";
}

/* ── Persistence ── */

export function loadLocal() {
  try {
    const raw = localStorage.getItem(LS_KEY);
    if (!raw) return;
    const data = JSON.parse(raw);
    if (data.routes) state.routes = { ...DEFAULT_ROUTES, ...data.routes };
  } catch (_) {}
}

export function saveLocal() {
  try {
    localStorage.setItem(LS_KEY, JSON.stringify({ routes: state.routes }));
  } catch (_) {}
}
