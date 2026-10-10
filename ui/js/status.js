/**
 * 状态页：磁盘占用总览 + 每个 harness / 代理一张卡片。
 * 扫描路径只列我们真正读的那几家；Z Code 与自定义工具同样在列，用同一张卡展示。
 */

import { $ } from "./dom.js";
import { escapeHtml, formatBytes } from "./format.js";
import { t } from "../i18n.js";
import { HARNESS, HARNESS_IDS } from "./harness.js";
import { mockProxy, proxyState, storageRows } from "./bridge.js";
import { state } from "./state.js";

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

export function renderStatus() {
  const grid = $("#status-grid");
  const storageByHarness = Object.fromEntries(storageRows().map((r) => [r.harness, r]));
  const paths = {
    cc: "~/.claude/projects/",
    kimi: "~/.kimi-code/sessions/",
    dsh: "~/.dsh/sessions/",
    codex: "~/.codex/sessions/",
    opencode: "~/.local/share/opencode/",
    zcode: "~/.zcode/cli/",
    antigravity: "~/.gemini/antigravity-cli/",
    workbuddy: "~/.workbuddy/projects/",
    stepcode: "~/.stepcode/agent/sessions/",
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
          [t("status.scanPath"), paths[hid] || "—"],
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
