/**
 * DOM 与全局交互：选择器、toast、模态框、剪贴板。
 * 模态框的焦点记录在这里，删除/转换等流程只管调用。
 */

export const $ = (sel, root = document) => root.querySelector(sel);
export const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];

let toastTimer;
export function toast(msg) {
  const el = $("#toast");
  el.textContent = msg;
  el.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.remove("show"), 2200);
}

let lastFocus = null;
export function showModal() {
  lastFocus = document.activeElement;
  $("#modal").hidden = false;
  requestAnimationFrame(() => $("#modal-box").focus());
}

/** 删除进行中不让人关掉——否则后端的删除还在跑，界面已经没有进度了 */
export function hideModal() {
  if (deleteInProgress()) return;
  $("#modal").hidden = true;
  if (lastFocus && document.contains(lastFocus)) lastFocus.focus();
}

/**
 * 删除对话框的运行态在 delete.js 里，这里只知道问一句，避免 dom ↔ delete 反向依赖
 */
let inProgress = false;
export function setDeleteInProgress(v) {
  inProgress = v;
}
function deleteInProgress() {
  return inProgress;
}

export async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    document.body.appendChild(ta);
    ta.select();
    document.execCommand("copy");
    ta.remove();
  }
}
