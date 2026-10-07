/**
 * 会话转换视图：把一条会话转成另一个 harness 的原生会话。
 *
 * 开放 Claude Code、Codex、OpenCode 三家互转（6 个方向都在沙盒里用真实 CLI 验证过，
 * 目标工具确实把历史送进了模型请求）；其余 harness 的按钮不出现，避免宣称成功却缺内容。
 * 来源会话永不被修改。
 */

import { $, toast } from "./dom.js";
import { escapeHtml, formatBytes } from "./format.js";
import { t, formatRelative } from "../i18n.js";
import { harnessOf, HARNESS } from "./harness.js";
import { invokeTauri, loadNativeSessions, loadNativeStorage } from "./bridge.js";
import { state } from "./state.js";
import { sessionKey, sessionTitle, localized } from "./session.js";
import { renderAnnunciator, renderNavFilters, renderSessions } from "./sessions.js";
import { closeDetail, resumeSession } from "./detail.js";
import { switchView } from "../app.js";

const TRANSFER_HARNESSES = ["cc", "codex", "opencode"];
/** 与后端不传 target 时的默认搭档一致 */
const DEFAULT_TARGET = { cc: "codex", codex: "cc", opencode: "cc" };

export function canTransfer(s) {
  return TRANSFER_HARNESSES.includes(s.harness) && s.kind !== "subagent";
}

function targetsFor(harness) {
  return TRANSFER_HARNESSES.filter((h) => h !== harness);
}

function transferSource() {
  const eligible = state.sessions.filter(canTransfer);
  const selected = eligible.find((s) => sessionKey(s) === state.transfer.sourceKey);
  return selected || eligible[0] || null;
}

export function openTransfer(s) {
  state.transfer.sourceKey = sessionKey(s);
  state.transfer.target = null;
  state.transfer.result = null;
  state.transfer.error = null;
  closeDetail();
  switchView("transfer");
}

/**
 * 后端只返回错误码，界面按类别给一句话说明：
 * 来源读不到 / 内容不可移植（媒体、未知结构）/ 别的（缺 CLI、目录没了、并发占用…）
 */
export function transferErrorLabel(error) {
  const code = String(error || "").split(":", 1)[0];
  const source = new Set(["session_not_found", "source_read_failed", "source_json_invalid", "source_cwd_missing", "source_conversation_empty", "invalid_session_id", "ambiguous_session_id"]);
  const media = new Set(["unsupported_source_media", "unsupported_content", "unsupported_response_item", "unsupported_image", "unsupported_tool_output", "unsupported_tool_output_media", "unsupported_assistant_content", "multiple_project_dirs_unsupported"]);
  if (source.has(code)) return t("transfer.error.source");
  if (media.has(code)) return t("transfer.error.media");
  if (code === "cwd_missing") return t("transfer.error.cwd");
  if (code === "codex_cli_missing") return t("transfer.error.cli", { name: "Codex" });
  if (code === "opencode_cli_missing") return t("transfer.error.cli", { name: "OpenCode" });
  if (code === "opencode_model_unknown") return t("transfer.error.opencodeModel");
  if (code === "opencode_import_failed" || code === "opencode_import_incomplete") return t("transfer.error.import", { name: "OpenCode" });
  if (code === "source_changed_during_import") return t("transfer.error.changed");
  if (code === "transfer_busy") return t("transfer.error.busy");
  return t("transfer.error.other", { code });
}

export function renderTransfer() {
  const root = $("#transfer-content");
  const sources = state.sessions.filter(canTransfer);
  const source = transferSource();
  if (!source) {
    root.innerHTML = `<div class="transfer-empty">${escapeHtml(t("transfer.empty"))}</div>`;
    return;
  }
  state.transfer.sourceKey = sessionKey(source);
  const target = targetsFor(source.harness).includes(state.transfer.target) ? state.transfer.target : DEFAULT_TARGET[source.harness];
  state.transfer.target = target;
  const sourceName = harnessOf(source.harness).name;
  const targetName = harnessOf(target).name;
  const result = state.transfer.result;
  const excerpt = localized(source.excerpt);
  const options = sources.map((s) => `<option value="${escapeHtml(sessionKey(s))}"${sessionKey(s) === state.transfer.sourceKey ? " selected" : ""}>${escapeHtml(harnessOf(s.harness).name)} · ${escapeHtml(sessionTitle(s))}</option>`).join("");
  root.innerHTML = `
    <label class="transfer-picker-label" for="transfer-source">${escapeHtml(t("transfer.chooseSource"))}</label>
    <select class="transfer-picker" id="transfer-source">${options}</select>
    <div class="transfer-board">
      <section class="transfer-column">
        <p class="transfer-overline">SOURCE · ${escapeHtml(t("transfer.source"))}</p>
        <h2>${escapeHtml(sessionTitle(source))}</h2>
        <p class="transfer-sub">${escapeHtml(sourceName)} · ${escapeHtml(source.project || "—")}</p>
        <div class="transfer-facts">
          <div><span>${escapeHtml(t("transfer.sessionId"))}</span><code>${escapeHtml(source.id)}</code></div>
          <div><span>${escapeHtml(t("transfer.updated"))}</span><strong>${escapeHtml(formatRelative(source.updatedMs))}</strong></div>
          <div><span>${escapeHtml(t("transfer.size"))}</span><strong>${escapeHtml(formatBytes(source.sizeBytes))}</strong></div>
        </div>
        ${excerpt ? `<div class="transfer-excerpt"><span>${escapeHtml(t("transfer.excerpt"))}</span><p>${escapeHtml(excerpt)}</p></div>` : ""}
        <p class="transfer-preserved">${escapeHtml(t("transfer.sourceKept", { name: sourceName }))}</p>
      </section>
      <div class="transfer-arrow" aria-hidden="true">→</div>
      <section class="transfer-column target">
        <p class="transfer-overline">DESTINATION · ${escapeHtml(t("transfer.target"))}</p>
        <h2>${escapeHtml(t("transfer.continueIn", { name: targetName }))}</h2>
        <p class="transfer-sub">${escapeHtml(t("transfer.newNative"))}</p>
        <div class="seg transfer-target-picker" role="radiogroup" aria-label="${escapeHtml(t("transfer.chooseTarget"))}">
          ${targetsFor(source.harness).map((h) => `<button type="button" role="radio" class="seg-opt${h === target ? " active" : ""}" aria-checked="${h === target}" data-target="${h}"${state.transfer.working ? " disabled" : ""}>${escapeHtml(harnessOf(h).name)}</button>`).join("")}
        </div>
        <div class="transfer-facts">
          <div><span>${escapeHtml(t("transfer.project"))}</span><strong>${escapeHtml(source.project || "—")}</strong></div>
          <div><span>${escapeHtml(t("transfer.history"))}</span><strong>${escapeHtml(t("transfer.fullCopy"))}</strong></div>
          <div><span>${escapeHtml(t("transfer.resume"))}</span><strong>${escapeHtml(t("transfer.nativeHistory", { name: targetName }))}</strong></div>
        </div>
        <p class="transfer-check">✓ ${escapeHtml(t("transfer.checkAtRun"))}</p>
      </section>
    </div>
    <div class="transfer-note">${escapeHtml(t(state.runtime === "tauri" ? "transfer.note" : "transfer.previewNote"))}</div>
    ${state.transfer.error ? `<div class="transfer-feedback error" role="alert">${escapeHtml(transferErrorLabel(state.transfer.error))}</div>` : ""}
    ${result ? `<div class="transfer-feedback success" role="status"><strong>${escapeHtml(t(result.existing ? "transfer.existing" : "transfer.done", { name: targetName }))}</strong><code>${escapeHtml(result.id)}</code></div>` : ""}
    <div class="transfer-actions">
      <button type="button" class="btn" id="transfer-back">${escapeHtml(t("transfer.back"))}</button>
      ${result ? `<button type="button" class="btn primary" id="transfer-resume">${escapeHtml(t("transfer.resumeTarget", { name: targetName }))}</button>` : `<button type="button" class="btn primary" id="transfer-submit"${state.transfer.working ? " disabled" : ""}>${escapeHtml(t(state.transfer.working ? "transfer.working" : "transfer.submit", { name: targetName }))}</button>`}
    </div>`;
  root.querySelectorAll("[data-target]").forEach((b) => b.addEventListener("click", () => {
    state.transfer.target = b.dataset.target;
    state.transfer.result = null;
    state.transfer.error = null;
    renderTransfer();
  }));
  root.querySelector("#transfer-source").addEventListener("change", (e) => {
    state.transfer.sourceKey = e.target.value;
    state.transfer.target = null;
    state.transfer.result = null;
    state.transfer.error = null;
    renderTransfer();
  });
  root.querySelector("#transfer-back").addEventListener("click", () => switchView("sessions"));
  root.querySelector("#transfer-submit")?.addEventListener("click", () => submitTransfer(source));
  root.querySelector("#transfer-resume")?.addEventListener("click", () => {
    const converted = state.sessions.find((s) => s.harness === result.harness && s.id === result.id);
    resumeSession(converted || { harness: result.harness, id: result.id, project: source.project });
  });
}

async function submitTransfer(source) {
  if (state.transfer.working) return;
  state.transfer.working = true;
  state.transfer.error = null;
  renderTransfer();
  try {
    const result = state.runtime === "tauri"
      ? await invokeTauri("convert_session", { harness: source.harness, id: source.id, target: state.transfer.target })
      : { harness: state.transfer.target, id: t("transfer.previewId"), existing: false };
    state.transfer.result = result;
    if (state.runtime === "tauri") {
      // 转换产生了新会话：重新扫一遍列表，让目标出现在会话树里
      await Promise.all([loadNativeSessions(), loadNativeStorage()]);
      renderSessions();
      renderAnnunciator();
      renderNavFilters();
    }
  } catch (error) {
    state.transfer.error = String(error);
  } finally {
    state.transfer.working = false;
    renderTransfer();
  }
}
