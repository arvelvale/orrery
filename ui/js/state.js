/**
 * 全局 state：一份可变的会话/代理视图。渲染模块读它，事件处理写它。
 *
 * 分层规则：只有 app.js 的 refreshAll() 会从后端刷新它，其余模块不要在渲染过程中改来源。
 */

import { DEFAULT_ROUTES, SEED_SESSIONS } from "./data.js";
import { formatTok, usageTotal } from "./format.js";

function seedSessions() {
  const now = Date.now();
  return SEED_SESSIONS.map(({ ago, ...s }) => ({
    ...s,
    updatedMs: now - ago,
    tokens: formatTok(usageTotal(s.usage)),
    path: s.project,
  }));
}

export const state = {
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
  transfer: { sourceKey: null, result: null, error: null, working: false },
};
