/**
 * 纯格式化工具：不依赖 state、不碰 DOM。数字与 token 的规则必须和后端一致，
 * 否则界面上的总数会和 Rust 报的不一样（formatTok 与 Rust format_tokens 同规则）。
 */

import { t } from "../i18n.js";

/** HTML 转义。所有插进 innerHTML 的用户内容都必须过这里 */
export function escapeHtml(str) {
  return String(str)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

export function formatBytes(n) {
  if (!Number.isFinite(n) || n <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return `${i === 0 ? n : n.toFixed(n >= 100 ? 0 : 1)} ${units[i]}`;
}

export function formatTok(n) {
  if (!n) return "0";
  // 与 Rust format_tokens 同规则：一位小数，去掉多余的 .0
  const short = (v, unit) => `${v.toFixed(1).replace(/\.0$/, "")}${unit}`;
  if (n >= 1e6) return short(n / 1e6, "M");
  if (n >= 1e3) return short(n / 1e3, "k");
  return String(n);
}

export function formatUptime(ms) {
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`;
  return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
}

/** 后端只给英文状态码，界面按 DESIGN.md 转成大写标签 */
export function statusLabel(st) {
  return ({ running: "RUNNING", idle: "IDLE", done: "DONE", error: "ERROR" })[st] || st;
}

/** 主 agent + 子 agent 累计，按 API 调用去重；列表里的总数 = 四项之和 */
export function usageTotal(u) {
  return u ? u.input + u.cache_write + u.cache_read + u.output + (u.unsplit || 0) : 0;
}

/** 四项分开展示，避免直接相加后看不出缓存命中的占比 */
export function usageSplitHtml(u) {
  return [
    ["usage.input", u.input],
    ["usage.cacheWrite", u.cache_write],
    ["usage.cacheRead", u.cache_read],
    ["usage.output", u.output],
    // 旧版 Codex 只记总数、没有分项，单列出来而不是猜测拆分
    ...(u.unsplit ? [["usage.unsplit", u.unsplit]] : []),
  ].map(([k, v]) => `<span>${escapeHtml(t(k))} ${formatTok(v)}</span>`).join("");
}
