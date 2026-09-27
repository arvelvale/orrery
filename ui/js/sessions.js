/**
 * 会话视图：告示灯、筛选、列表、勾选条。
 *
 * 列表只读 state.sessions，删改一律通过事件 -> app.js 的 refreshAll；
 * 勾选集合 state.selected 在这里维护，删除成功后由 delete.js 清空。
 */

import { $, toast } from "./dom.js";
import { escapeHtml, formatBytes, statusLabel } from "./format.js";
import { t, formatRelative } from "../i18n.js";
import { canManage, HARNESS, HARNESS_IDS, harnessOf } from "./harness.js";
import { loadNativeSessions, loadNativeStorage, proxyState, storageRows } from "./bridge.js";
import { sessionKey, sessionTitle, localized } from "./session.js";
import { state } from "./state.js";
import { openSession } from "./detail.js";
import { openDeleteDialog } from "./delete.js";
import { switchView } from "../app.js";

/** 一个 harness 的灯：有报错 → 琥珀；有在跑 → 绿；只有会话 → 蓝；没有会话 → 灰 */
function harnessLampState(hid) {
  const list = state.sessions.filter((s) => s.harness === hid);
  if (!list.length) return "idle";
  if (list.some((s) => s.status === "error")) return "warn";
  if (list.some((s) => s.status === "running")) return "run";
  return "idle";
}

export function renderAnnunciator(boot = false) {
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

export function renderFilters() {
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

export function renderNavFilters() {
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

export function filteredSessions() {
  const q = state.query.trim().toLowerCase();
  const items = state.sessions.filter((s) => {
    if (state.filter !== "all" && s.harness !== state.filter) return false;
    if (!q) return true;
    return [sessionTitle(s), s.project, s.model, localized(s.excerpt), s.id].join(" ").toLowerCase().includes(q);
  });
  if (state.sort === "size") items.sort((a, b) => (b.sizeBytes || 0) - (a.sizeBytes || 0));
  return items;
}

export function renderSessions() {
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
      ${canManage(s) ? `<label class="strip-check" title="${escapeHtml(t("select.toggle"))}">
        <input type="checkbox" ${checked ? "checked" : ""} aria-label="${escapeHtml(t("select.toggle"))}" />
      </label>` : `<span class="strip-check" aria-hidden="true"></span>`}
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
    const input = btn.querySelector(".strip-check input");
    if (input) {
      btn.querySelector(".strip-check").addEventListener("click", (e) => e.stopPropagation());
      input.addEventListener("change", (e) => {
        if (e.target.checked) state.selected.add(key);
        else state.selected.delete(key);
        btn.classList.toggle("checked", e.target.checked);
        renderSelectionBar();
      });
    }
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

/* ── 选择与删除入口 ── */

export function selectedSessions() {
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
    filteredSessions().filter(canManage).forEach((s) => state.selected.add(sessionKey(s)));
    renderSessions();
  });
  bar.querySelector('[data-sel="clear"]').addEventListener("click", () => {
    state.selected.clear();
    renderSessions();
  });
  bar.querySelector('[data-sel="delete"]').addEventListener("click", () => openDeleteDialog(picked));
}
