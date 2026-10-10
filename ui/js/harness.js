/**
 * harness 元数据：侧栏角标、告示灯、筛选、恢复/删除的可用范围。
 *
 * 自定义来源（用户在 ~/.orrery/harnesses.json 登记的 OpenCode 系工具）不在这张表里，
 * `harnessOf` 按需补一个中性条目，避免界面直接崩掉。
 */

export const HARNESS = {
  cc:   { id: "cc",   label: "CC",   name: "Claude Code", badge: "cc" },
  kimi: { id: "kimi", label: "KIMI", name: "Kimi Code",   badge: "kimi" },
  dsh:  { id: "dsh",  label: "DSH",  name: "DSH",         badge: "dsh" },
  codex: { id: "codex", label: "CODEX", name: "Codex",     badge: "codex" },
  opencode: { id: "opencode", label: "OPENCODE", name: "OpenCode", badge: "opencode" },
  zcode: { id: "zcode", label: "ZCODE", name: "Z Code", badge: "zcode" },
  antigravity: { id: "antigravity", label: "ANTIGRAVITY", name: "Antigravity", badge: "antigravity" },
  workbuddy: { id: "workbuddy", label: "WBUDDY", name: "WorkBuddy", badge: "workbuddy" },
  stepcode: { id: "stepcode", label: "STEPCODE", name: "StepCode", badge: "stepcode" },
};

export const HARNESS_IDS = Object.keys(HARNESS);

/*
 * 能在 Orrery 里删除、也能在终端恢复的 harness。其余（Z Code、WorkBuddy、
 * 自己登记的工具）只读：不给复选框、不给删除/恢复按钮，详情里直接说去哪里管理
 * ——别让人点了才发现做不了。
 * StepCode 恢复走 jsonl 绝对路径（按 id 恢复官方不支持，见 `resume.rs`），
 * 会话文件不在 sessions 根目录里时后端返回 `session_file_missing`
 */
const MANAGEABLE = new Set(["cc", "kimi", "dsh", "codex", "opencode", "antigravity", "stepcode"]);

export const canManage = (s) => MANAGEABLE.has(s.harness);

/** 按 harness id 判断（删除计划里只有 id，没有整条会话） */
export const canManageHarness = (id) => MANAGEABLE.has(id);

/**
 * 未知 harness 的兜底：补一个中性条目并登记进 HARNESS_IDS，
 * 这样筛选器、告示灯、状态页都会把它一起列出来
 */
export function harnessOf(id) {
  if (HARNESS[id]) return HARNESS[id];
  HARNESS[id] = { id, label: String(id).toUpperCase().slice(0, 8), name: id, badge: "custom" };
  if (!HARNESS_IDS.includes(id)) HARNESS_IDS.push(id);
  return HARNESS[id];
}
