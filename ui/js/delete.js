/**
 * 删除对话框：先 plan_delete 预览，确认后才 delete_sessions。
 *
 * 两个必须守住的行为：
 * 1. 执行前重新由后端规划，界面不采信早先的计划（openDeleteDialog→runDelete 之间会话可能已变）
 * 2. 删除完成后核对"列表占用下降量"与"后端报告的删除量"，不一致就会在结果里提示用户
 */

import { $, hideModal, showModal, setDeleteInProgress } from "./dom.js";
import { escapeHtml, formatBytes } from "./format.js";
import { t } from "../i18n.js";
import { canManageHarness, HARNESS } from "./harness.js";
import { invokeTauri, storageRows } from "./bridge.js";
import { state } from "./state.js";
import { sessionKey, sessionTitle } from "./session.js";
import { refreshAll, rerenderAll } from "../app.js";

const dialog = { targets: [], plans: [], mode: "trash", ack: false, phase: "plan", results: [] };

/** 浏览器预览：按 mock 会话模拟后端的计划（运行中的会话按"最近写入"拦下，便于演示保护逻辑） */
function mockPlans(targets) {
  // 与后端同口径：OpenCode 走 CLI、不动文件；只在自己库里的工具只读
  const readOnly = (h) => !canManageHarness(h);
  const noFiles = (h) => h === "opencode" || readOnly(h);
  return targets.map((s) => ({
    harness: s.harness,
    id: s.id,
    files: noFiles(s.harness) ? [] : [s.project],
    bytes: readOnly(s.harness) ? 0 : s.sizeBytes || 0,
    index_files: noFiles(s.harness) || s.harness === "cc" ? [] : ["session_index.jsonl"],
    codex_threads: s.harness === "codex" ? [s.id] : [],
    cli_sessions: s.harness === "opencode" ? [s.id] : [],
    blocked: readOnly(s.harness) ? "read_only" : s.status === "running" ? "active" : null,
    warnings: s.harness === "antigravity" ? ["agy_title_kept"] : [],
  }));
}

export async function openDeleteDialog(sessions) {
  dialog.targets = sessions;
  dialog.mode = "trash";
  dialog.ack = false;
  dialog.phase = "loading";
  dialog.results = [];
  showModal();
  renderDeleteDialog();
  const targets = sessions.map((s) => ({ harness: s.harness, id: s.id }));
  try {
    dialog.plans = state.runtime === "tauri" ? await invokeTauri("plan_delete", { targets }) : mockPlans(sessions);
  } catch (e) {
    console.warn("plan_delete failed", e);
    dialog.plans = [];
    dialog.error = String(e);
  }
  dialog.phase = "plan";
  renderDeleteDialog();
}

function findTarget(p) {
  return dialog.targets.find((s) => s.harness === p.harness && s.id === p.id);
}

function planRowHtml(p, extra = "") {
  const s = findTarget(p);
  const h = HARNESS[p.harness] || HARNESS.cc;
  return `
    <li class="del-row">
      <span class="badge ${h.badge}">${h.label}</span>
      <span class="del-title">${escapeHtml(s ? sessionTitle(s) : p.id)}</span>
      <span class="del-size${extra ? " reason" : ""}">${extra || formatBytes(p.bytes)}</span>
    </li>`;
}

function listHtml(rows, render) {
  const shown = rows.slice(0, 6).map(render).join("");
  const more = rows.length > 6 ? `<li class="del-more">${escapeHtml(t("del.more", { n: rows.length - 6 }))}</li>` : "";
  return `<ul class="del-list">${shown}${more}</ul>`;
}

function renderDeleteDialog() {
  const box = $("#modal-box");
  if (dialog.phase === "loading") {
    box.innerHTML = `<p class="hint">${escapeHtml(t("del.planning"))}</p>`;
    return;
  }
  if (dialog.phase === "done") return renderDeleteResults();

  const ok = dialog.plans.filter((p) => !p.blocked);
  const blocked = dialog.plans.filter((p) => p.blocked);
  const bytes = ok.reduce((n, p) => n + p.bytes, 0);
  const files = ok.reduce((n, p) => n + p.files.length, 0);
  const warnings = [...new Set(ok.flatMap((p) => p.warnings.map((w) => `${w}|${p.harness}`)))];
  const hasIndex = ok.some((p) => p.index_files.length);
  const hasCodex = ok.some((p) => p.harness === "codex");
  const hasOpencode = ok.some((p) => p.harness === "opencode");
  // 全是 OpenCode 时"可从回收站找回"不成立，顶部直接换成导出说明
  const onlyOpencode = hasOpencode && ok.every((p) => p.harness === "opencode");
  const permanent = dialog.mode === "permanent";
  // OpenCode 全在它的库里，没有文件可列；全是这种会话时不显示"0 个文件"
  const summary = files
    ? t("del.summary", { n: ok.length, files: t("del.files", { n: files }) })
    : t("del.summary", { n: ok.length, files: "" }).replace(/\s*·\s*$/, "");
  const working = dialog.phase === "working";
  const canConfirm = ok.length > 0 && (!permanent || dialog.ack) && !working;

  box.innerHTML = `
    <h2 id="modal-title">${escapeHtml(t("del.title", { n: dialog.targets.length }))}</h2>
    <div class="seg" role="radiogroup" aria-label="${escapeHtml(t("del.modeLabel"))}">
      ${[["trash", "del.modeTrash"], ["permanent", "del.modePermanent"]].map(([m, label]) => `
        <button type="button" role="radio" class="seg-opt${dialog.mode === m ? " active" : ""}" aria-checked="${dialog.mode === m}" data-mode="${m}" ${working ? "disabled" : ""}>${escapeHtml(t(label))}</button>`).join("")}
    </div>
    <p class="del-note${permanent ? " danger" : ""}">${escapeHtml(t(permanent ? "del.permanentNote" : onlyOpencode ? "del.opencodeTrashNote" : "del.trashNote"))}</p>
    ${ok.length ? `
      <div class="del-summary">
        <strong>${formatBytes(bytes)}</strong>
        <span>${escapeHtml(summary)}</span>
      </div>
      ${listHtml(ok, (p) => planRowHtml(p))}` : `<p class="hint">${escapeHtml(t("del.nothing"))}</p>`}
    ${blocked.length ? `
      <h3>${escapeHtml(t("del.blockedTitle", { n: blocked.length }))}</h3>
      ${listHtml(blocked, (p) => planRowHtml(p, escapeHtml(t(`del.reason.${p.blocked}`))))}` : ""}
    ${warnings.map((w) => {
      const [code, hid] = w.split("|");
      return `<p class="del-warn">${escapeHtml(t(`del.warn.${code}`, { name: (HARNESS[hid] || {}).name || hid }))}</p>`;
    }).join("")}
    ${hasIndex ? `<p class="del-fine">${escapeHtml(t("del.indexNote"))}</p>` : ""}
    ${hasCodex ? `<p class="del-fine">${escapeHtml(t("del.codexNote"))}</p>` : ""}
    ${hasOpencode ? `<p class="del-fine">${escapeHtml(t("del.opencodeNote"))}${permanent || onlyOpencode ? "" : ` ${escapeHtml(t("del.opencodeTrashNote"))}`}</p>` : ""}
    ${state.runtime !== "tauri" ? `<p class="del-fine">${escapeHtml(t("del.previewNote"))}</p>` : ""}
    ${permanent && ok.length ? `
      <label class="del-ack"><input type="checkbox" id="del-ack" ${dialog.ack ? "checked" : ""} ${working ? "disabled" : ""} /> ${escapeHtml(t("del.ack"))}</label>` : ""}
    <div class="btn-row modal-actions">
      <button type="button" class="btn" data-act="cancel" ${working ? "disabled" : ""}>${escapeHtml(t("del.cancel"))}</button>
      <button type="button" class="btn ${permanent ? "danger solid" : "danger"}" data-act="confirm" ${canConfirm ? "" : "disabled"}>
        ${escapeHtml(t(working ? "del.working" : permanent ? "del.confirmPermanent" : "del.confirmTrash"))}
      </button>
    </div>`;

  box.querySelectorAll("[data-mode]").forEach((b) => b.addEventListener("click", () => {
    dialog.mode = b.dataset.mode;
    dialog.ack = false;
    renderDeleteDialog();
  }));
  const ack = box.querySelector("#del-ack");
  if (ack) ack.addEventListener("change", (e) => {
    dialog.ack = e.target.checked;
    renderDeleteDialog();
  });
  box.querySelector('[data-act="cancel"]').addEventListener("click", hideModal);
  box.querySelector('[data-act="confirm"]').addEventListener("click", () => runDelete(ok));
}

async function runDelete(plans) {
  dialog.phase = "working";
  setDeleteInProgress(true);
  renderDeleteDialog();
  const targets = plans.map((p) => ({ harness: p.harness, id: p.id }));
  const before = storageRows().filter((r) => r.connected).reduce((n, r) => n + r.sessionBytes, 0);

  if (state.runtime === "tauri") {
    try {
      dialog.results = await invokeTauri("delete_sessions", { targets, mode: dialog.mode });
    } catch (e) {
      dialog.results = plans.map((p) => ({ ...p, ok: false, error: String(e) }));
    }
  } else {
    // 预览模式只从列表移除，不碰任何文件
    dialog.results = plans.map((p) => ({ harness: p.harness, id: p.id, ok: true, bytes: p.bytes, mode: dialog.mode }));
    const gone = new Set(plans.map((p) => `${p.harness}:${p.id}`));
    state.sessions = state.sessions.filter((s) => !gone.has(sessionKey(s)));
  }

  // 一次删除完成即结束这轮选择；被保护而跳过的会话不留在勾选里，避免混进下一次删除
  state.selected.clear();
  if (state.runtime === "tauri") await refreshAll();
  else rerenderAll();

  // 核对：列表统计的占用下降量应与后端报告的删除量一致
  const after = storageRows().filter((r) => r.connected).reduce((n, r) => n + r.sessionBytes, 0);
  dialog.verified = { reported: dialog.results.filter((r) => r.ok).reduce((n, r) => n + (r.bytes || 0), 0), dropped: before - after };
  dialog.phase = "done";
  setDeleteInProgress(false);
  renderDeleteDialog();
}

function errorLabel(err) {
  if (!err) return "";
  const code = String(err).split(":")[0];
  if (code === "blocked") return t(`del.reason.${String(err).split(":")[1]}`);
  return t(`del.error.${code}`, {}) === `del.error.${code}` ? String(err) : t(`del.error.${code}`);
}

function renderDeleteResults() {
  const box = $("#modal-box");
  const ok = dialog.results.filter((r) => r.ok);
  const failed = dialog.results.filter((r) => !r.ok);
  const bytes = ok.reduce((n, r) => n + (r.bytes || 0), 0);
  const v = dialog.verified || { reported: 0, dropped: 0 };
  const backups = [...new Set(ok.map((r) => r.backup_dir).filter(Boolean))];
  // 每条会话一个导出子目录，显示它们共同的上级（本批的时间戳目录）
  const exports = [...new Set(ok.map((r) => r.export_dir).filter(Boolean))];
  const codexFallback = ok.some((r) => r.codex_cli === "fallback" || r.codex_cli === "missing");
  box.innerHTML = `
    <h2 id="modal-title">${escapeHtml(t("del.doneTitle"))}</h2>
    <div class="del-summary">
      <strong>${formatBytes(bytes)}</strong>
      <span>${escapeHtml(t(dialog.mode === "permanent" ? "del.doneFreed" : ok.length && ok.every((r) => r.export_dir) ? "del.doneExported" : "del.doneTrashed", { n: ok.length }))}</span>
    </div>
    ${ok.length ? `<p class="del-fine">${escapeHtml(t("del.verify", { reported: formatBytes(v.reported), dropped: formatBytes(Math.max(0, v.dropped)) }))}</p>` : ""}
    ${failed.length ? `
      <h3>${escapeHtml(t("del.failedTitle", { n: failed.length }))}</h3>
      ${listHtml(failed, (r) => planRowHtml(r, escapeHtml(errorLabel(r.error))))}` : ""}
    ${codexFallback ? `<p class="del-warn">${escapeHtml(t("del.codexFallback"))}</p>` : ""}
    ${backups.length ? `<p class="del-fine">${escapeHtml(t("del.backups", { path: backups[0].replace(/[\\/][^\\/]+$/, "") }))}</p>` : ""}
    ${exports.length ? `<p class="del-fine">${escapeHtml(t("del.exports", { path: exports.length === 1 ? exports[0] : exports[0].replace(/[\\/][^\\/]+$/, "") }))}</p>` : ""}
    <div class="btn-row modal-actions">
      <button type="button" class="btn primary" data-act="close">${escapeHtml(t("del.close"))}</button>
    </div>`;
  box.querySelector('[data-act="close"]').addEventListener("click", hideModal);
}
