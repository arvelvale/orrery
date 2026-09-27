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
  codebuddy: { id: "codebuddy", label: "CBUDDY", name: "CodeBuddy Code", badge: "codebuddy" },
};

export const HARNESS_IDS = Object.keys(HARNESS);

/*
 * 能在 Orrery 里删除会话的 harness。CodeBuddy Code 的 session 文件由它自己追加写，
 * Orrery 没在沙盒里验证过删掉后它的 --resume 列表会怎样，所以只读、不进这里
 */
const MANAGEABLE = new Set(["cc", "kimi", "dsh", "codex", "opencode", "antigravity"]);

export const canManage = (s) => MANAGEABLE.has(s.harness);

/** 按 harness id 判断（删除计划里只有 id，没有整条会话） */
export const canManageHarness = (id) => MANAGEABLE.has(id);

/*
 * 能在终端里恢复的 harness：有自己的 CLI 且 resume 参数已确认。
 * 和 canManage 分开——CodeBuddy 能恢复但不能删，Z Code 两个都不能
 */
const RESUMABLE = new Set(["cc", "kimi", "dsh", "codex", "opencode", "antigravity", "codebuddy"]);

export const canResume = (s) => RESUMABLE.has(s.harness);

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
