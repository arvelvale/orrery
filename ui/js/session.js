/**
 * 会话级的小工具：state 里的记录怎么读、id 怎么拼 key、标题怎么兜底。
 * 只依赖 i18n，不依赖任何渲染模块。
 */

import { t, getLocale } from "../i18n.js";

/** 不同 harness 的会话 id 可能重名，选择集合用 harness:id */
export function sessionKey(s) {
  return `${s.harness}:${s.id}`;
}

/** mock 数据的标题/摘要按语言存；真实数据是用户原文字符串 */
export function localized(v) {
  if (v && typeof v === "object") return v[getLocale()] ?? v.en ?? "";
  return v ?? "";
}

export function sessionTitle(s) {
  const title = localized(s.title);
  if (title) return title;
  // DSH / Kimi 的 id 带 `session-` / `session_` 前缀，截短前先去掉
  const short = String(s.id).replace(/^session[-_]/, "").slice(0, 8);
  // 父会话不在本机的子 agent（Codex guardian 自动审查）：标题是发给它的系统指令，不展示
  return t(s.kind === "subagent" ? "session.subagent" : "session.untitled", { id: short });
}

export function findSession(sessions, id) {
  return sessions.find((s) => s.id === id);
}
