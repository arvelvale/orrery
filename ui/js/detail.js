/**
 * 会话详情面板：字段展示、打开/关闭、以及那排动作按钮的实际行为。
 * 删除和转换各自由 delete.js / transfer.js 负责，这里只做跳转。
 */

import { $, copyText, toast } from "./dom.js";
import { escapeHtml, formatBytes, formatTok, statusLabel, usageSplitHtml } from "./format.js";
import { t, formatRelative } from "../i18n.js";
import { canManage, canResume, HARNESS, harnessOf } from "./harness.js";
import { invokeTauri } from "./bridge.js";
import { state } from "./state.js";
import { findSession, localized, sessionTitle } from "./session.js";
import { renderSessions } from "./sessions.js";
import { openDeleteDialog } from "./delete.js";
import { canTransfer, openTransfer } from "./transfer.js";

export function sessionDetailHtml(s) {
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
    ${canManage(s) ? "" : `<p class="detail-note">${escapeHtml(t(canResume(s) ? "detail.noDelete" : "detail.readOnly", { name: h.name }))}</p>`}
    <div class="btn-row">
      ${canResume(s) ? `<button type="button" class="btn primary" data-act="resume">${escapeHtml(t("detail.resume"))}</button>` : ""}
      ${canTransfer(s) ? `<button type="button" class="btn" data-act="transfer">${escapeHtml(t("detail.transfer"))}</button>` : ""}
      <button type="button" class="btn" data-act="open-folder">${escapeHtml(t("detail.openFolder"))}</button>
      <button type="button" class="btn" data-act="copy-path">${escapeHtml(t("detail.copyPath"))}</button>
      <button type="button" class="btn" data-act="copy-session-id">${escapeHtml(t("detail.copySessionId"))}</button>
      ${canManage(s) ? `<button type="button" class="btn danger" data-act="delete">${escapeHtml(t("detail.delete"))}</button>` : ""}
    </div>`;
}

export function openSession(id) {
  state.selectedId = id;
  const s = findSession(state.sessions, id);
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

export function closeDetail() {
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
export async function resumeSession(s) {
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
      } else if (act === "copy-session-id") {
        await copyText(s.id);
        toast(t("toast.sessionIdCopied"));
      } else if (act === "delete") {
        openDeleteDialog([s]);
      } else if (act === "transfer") {
        openTransfer(s);
      }
    });
  });
}
