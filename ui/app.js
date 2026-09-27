/**
 * Orrery UI — WebView 前身
 * 在浏览器中用 mock；在 Tauri 里通过 invoke 调 Rust。
 * 所有面向用户的文案走 i18n.js 的 t()，这里不写死任何自然语言。
 *
 * 本文件是入口：语言/视图切换、全局重渲染、事件绑定、启动。
 * 具体视图在 js/ 下的各模块，后端调用收口在 js/bridge.js。
 */

import { LOCALES, applyStatic, detectLocale, getLocale, missingKeys, setLocale, t } from "./i18n.js";
import { $$, $, copyText, hideModal, toast } from "./js/dom.js";
import { state } from "./js/state.js";
import {
  detectRuntime, invokeTauri, loadLocal, loadNativeProxy, loadNativeSessions, loadNativeStorage,
  proxyState, renderRuntimeBadge,
} from "./js/bridge.js";
import { renderAnnunciator, renderFilters, renderNavFilters, renderSessions } from "./js/sessions.js";
import { renderTransfer } from "./js/transfer.js";
import { renderModels, renderProxyPanel, updateProxyRow } from "./js/models.js";
import { renderStatus } from "./js/status.js";
import { closeDetail, openSession } from "./js/detail.js";
import { findSession } from "./js/session.js";

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
export function rerenderAll(boot = false) {
  applyStatic();
  renderLangSwitch();
  renderRuntimeBadge();
  renderAnnunciator(boot);
  renderFilters();
  renderNavFilters();
  renderSessions();
  renderTransfer();
  renderModels();
  renderStatus();
  updateProxyRow();
  renderProxyPanel();
  if (state.selectedId && findSession(state.sessions, state.selectedId)) openSession(state.selectedId);
  else closeDetail();
}

/* ── 视图切换 ── */

export function switchView(name) {
  if (name === "transfer") closeDetail();
  state.view = name;
  $$(".view").forEach((v) => v.classList.toggle("active", v.id === `view-${name}`));
  $$(".sidebar > .nav-item[data-view]").forEach((el) => {
    el.classList.toggle("active", el.dataset.view === name);
  });
  if (name === "models") renderModels();
  if (name === "status") renderStatus();
  if (name === "transfer") renderTransfer();
}

export async function refreshAll(boot = false) {
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
