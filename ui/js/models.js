/**
 * 模型页：本地代理的状态与启停、供应商（base URL + 密钥）、模型登记、每个 harness 的默认模型。
 *
 * 密钥规则（2026-09 产品决策）：用户可以在应用内填明文密钥，和填环境变量名二选一；
 * 界面只显示"是否已设置 / 来源"，任何地方都不回显密钥内容。
 */

import { $, toast } from "./dom.js";
import { escapeHtml, formatUptime } from "./format.js";
import { t } from "../i18n.js";
import { HARNESS, HARNESS_IDS } from "./harness.js";
import { editModels, editProviders, invokeTauri, loadNativeProxy, mockProxy, proxyState, saveLocal } from "./bridge.js";
import { state } from "./state.js";
import { DEFAULT_ROUTES } from "./data.js";
import { renderStatus } from "./status.js";
import { renderAnnunciator } from "./sessions.js";

export function modelOptionsHtml(selected) {
  const models = editModels();
  const ids = models.map((m) => m.id);
  if (selected && !ids.includes(selected)) ids.push(selected);
  if (!ids.length) ids.push("");
  return ids
    .map((id) => `<option value="${escapeHtml(id)}" ${id === selected ? "selected" : ""}>${escapeHtml(id || "—")}</option>`)
    .join("");
}

async function reloadProxyUi() {
  await loadNativeProxy();
  state.routes = { ...DEFAULT_ROUTES, ...(state.proxy?.routes || {}) };
  renderModels();
  renderStatus();
  renderAnnunciator();
}

export function renderModels() {
  renderProxyPanel();
  renderProviderEditor();
  renderModelEditor();
  renderRoutesEditor();
  updateProxyRow();
}

function renderRoutesEditor() {
  const routes = $("#routes");
  if (!routes) return;
  routes.innerHTML = "";
  HARNESS_IDS.forEach((hid) => {
    const h = HARNESS[hid];
    const current = state.routes[hid] || "";
    const row = document.createElement("div");
    row.className = "route";
    row.innerHTML = `
      <div class="route-harness">
        <div class="name">${escapeHtml(h.name)}</div>
        <div class="sub">${escapeHtml(h.label)} · ${escapeHtml(t("models.routesDefault"))}</div>
      </div>
      <div class="route-arrow" aria-hidden="true">→</div>
      <div class="route-model">
        <label class="sr-only" for="route-${hid}">${escapeHtml(t("models.defaultFor", { name: h.name }))}</label>
        <select id="route-${hid}">${modelOptionsHtml(current)}</select>
      </div>`;
    routes.appendChild(row);
    row.querySelector("select").addEventListener("change", async (e) => {
      state.routes[hid] = e.target.value;
      saveLocal();
      await invokeTauri("save_route", { harness: hid, model: e.target.value }).catch((err) => console.warn(err));
      toast(`${h.name} → ${e.target.value || "—"}`);
      if (state.proxy) state.proxy.routes = { ...state.routes };
    });
  });
}

function renderProviderEditor() {
  const list = $("#provider-list");
  const form = $("#provider-form");
  if (!list || !form) return;
  const providers = editProviders();

  list.innerHTML = providers
    .map((pv) => {
      const hasKey = !!(state.proxyEdit?.providers?.[pv.name]?.api_key) || false;
      const st = (state.proxy?.providers || []).find((x) => x.name === pv.name);
      const present = st ? st.key_present : hasKey;
      const source = st ? st.key_source : hasKey ? "config" : "none";
      const label = present
        ? source === "config"
          ? t("proxy.keyFromConfig")
          : t("proxy.keyFromEnv")
        : t("proxy.keyMissing");
      const pillCls = present ? (source === "config" ? "src-config" : "ok") : "none";
      return `
        <div class="edit-item" data-provider="${escapeHtml(pv.name)}">
          <div class="edit-item-head">
            <span class="name">${escapeHtml(pv.name)}</span>
            <span class="pill ${pillCls}">${escapeHtml(label)}</span>
            <button type="button" class="btn small" data-act="edit-provider">${escapeHtml(t("models.edit"))}</button>
            <button type="button" class="btn small danger" data-act="rm-provider">${escapeHtml(t("models.remove"))}</button>
          </div>
          <div class="meta">${escapeHtml(pv.wire)} · ${escapeHtml(pv.base_url)}${
            pv.api_key_env ? ` · env:${escapeHtml(pv.api_key_env)}` : ""
          }${pv.model_prefixes?.length ? ` · ${escapeHtml(pv.model_prefixes.join(", "))}` : ""}</div>
        </div>`;
    })
    .join("");

  form.innerHTML = providerFormHtml(null);
  wireProviderForm(form, null);

  list.querySelectorAll("[data-act='edit-provider']").forEach((btn) => {
    btn.addEventListener("click", () => {
      const name = btn.closest("[data-provider]")?.dataset.provider;
      const pv = providers.find((p) => p.name === name);
      form.innerHTML = providerFormHtml(pv);
      wireProviderForm(form, pv?.name || null);
    });
  });
  list.querySelectorAll("[data-act='rm-provider']").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const name = btn.closest("[data-provider]")?.dataset.provider;
      if (!name) return;
      if (!confirm(t("models.confirmRemoveProvider", { name }))) return;
      try {
        await invokeTauri("remove_provider", { name });
        toast(t("models.removed", { name }));
        await reloadProxyUi();
      } catch (e) {
        toast(String(e));
      }
    });
  });
}

function providerFormHtml(pv) {
  const prefixes = (pv?.model_prefixes || []).join(", ");
  return `
    <h3>${pv ? escapeHtml(t("models.editProvider", { name: pv.name })) : escapeHtml(t("models.addProvider"))}</h3>
    <div class="field">
      <label for="pf-name">${escapeHtml(t("models.fieldName"))}</label>
      <input id="pf-name" value="${escapeHtml(pv?.name || "")}" placeholder="anthropic / my-proxy" ${pv ? "readonly" : ""} />
    </div>
    <div class="field">
      <label for="pf-base">${escapeHtml(t("models.fieldBaseUrl"))}</label>
      <input id="pf-base" value="${escapeHtml(pv?.base_url || "")}" placeholder="https://api.anthropic.com/v1" />
    </div>
    <div class="field-row">
      <div class="field">
        <label for="pf-wire">${escapeHtml(t("models.fieldWire"))}</label>
        <select id="pf-wire">
          <option value="openai" ${pv?.wire === "openai" || !pv ? "selected" : ""}>OpenAI-compatible</option>
          <option value="anthropic" ${pv?.wire === "anthropic" ? "selected" : ""}>Anthropic Messages</option>
        </select>
      </div>
      <div class="field">
        <label for="pf-env">${escapeHtml(t("models.fieldKeyEnv"))}</label>
        <input id="pf-env" value="${escapeHtml(pv?.api_key_env || "")}" placeholder="ANTHROPIC_API_KEY" />
      </div>
    </div>
    <div class="field">
      <label for="pf-key">${escapeHtml(t("models.fieldApiKey"))}</label>
      <div class="key-row">
        <input id="pf-key" type="password" value="${escapeHtml(pv?.api_key || "")}" placeholder="${escapeHtml(t("models.keyPlaceholder"))}" autocomplete="off" />
        <button type="button" class="btn small" id="pf-key-toggle">${escapeHtml(t("models.showKey"))}</button>
      </div>
    </div>
    <div class="field">
      <label for="pf-prefix">${escapeHtml(t("models.fieldPrefixes"))}</label>
      <input id="pf-prefix" value="${escapeHtml(prefixes)}" placeholder="claude, my-llm" />
    </div>
    <div class="btn-row">
      <button type="button" class="btn primary" id="pf-save">${escapeHtml(t("models.save"))}</button>
    </div>`;
}

function wireProviderForm(form, existingName) {
  const keyInput = form.querySelector("#pf-key");
  const toggle = form.querySelector("#pf-key-toggle");
  if (toggle && keyInput) {
    toggle.addEventListener("click", () => {
      const show = keyInput.type === "password";
      keyInput.type = show ? "text" : "password";
      toggle.textContent = show ? t("models.hideKey") : t("models.showKey");
    });
  }
  form.querySelector("#pf-save")?.addEventListener("click", async () => {
    const name = form.querySelector("#pf-name").value.trim() || existingName;
    const base_url = form.querySelector("#pf-base").value.trim();
    const wire = form.querySelector("#pf-wire").value;
    const api_key_env = form.querySelector("#pf-env").value.trim();
    const api_key = form.querySelector("#pf-key").value;
    const model_prefixes = form
      .querySelector("#pf-prefix")
      .value.split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    if (!name) {
      toast(t("models.fieldName"));
      return;
    }
    try {
      await invokeTauri("save_provider", {
        name,
        baseUrl: base_url,
        wire,
        apiKey: api_key,
        apiKeyEnv: api_key_env,
        modelPrefixes: model_prefixes,
      });
      toast(t("models.saved", { name }));
      await reloadProxyUi();
    } catch (e) {
      toast(String(e));
    }
  });
}

function renderModelEditor() {
  const list = $("#model-list");
  const form = $("#model-form");
  if (!list || !form) return;
  const models = editModels();
  const providers = editProviders();

  list.innerHTML = models
    .map(
      (m) => `
      <div class="edit-item" data-model="${escapeHtml(m.id)}">
        <div class="edit-item-head">
          <code class="name">${escapeHtml(m.id)}</code>
          <span class="tag">${escapeHtml(m.provider)}</span>
          <button type="button" class="btn small danger" data-act="rm-model">${escapeHtml(t("models.remove"))}</button>
        </div>
      </div>`
    )
    .join("");

  form.innerHTML = `
    <h3>${escapeHtml(t("models.addModel"))}</h3>
    <div class="field-row">
      <div class="field">
        <label for="mf-id">${escapeHtml(t("models.fieldModelId"))}</label>
        <input id="mf-id" placeholder="claude-sonnet-5.5" />
      </div>
      <div class="field">
        <label for="mf-provider">${escapeHtml(t("models.fieldProvider"))}</label>
        <select id="mf-provider">${providers
          .map((p) => `<option value="${escapeHtml(p.name)}">${escapeHtml(p.name)}</option>`)
          .join("")}</select>
      </div>
    </div>
    <div class="btn-row">
      <button type="button" class="btn primary" id="mf-save">${escapeHtml(t("models.save"))}</button>
    </div>`;

  form.querySelector("#mf-save")?.addEventListener("click", async () => {
    const id = form.querySelector("#mf-id").value.trim();
    const provider = form.querySelector("#mf-provider").value;
    if (!id) {
      toast(t("models.fieldModelId"));
      return;
    }
    try {
      await invokeTauri("save_model", { id, provider });
      toast(t("models.saved", { name: id }));
      await reloadProxyUi();
    } catch (e) {
      toast(String(e));
    }
  });

  list.querySelectorAll("[data-act='rm-model']").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const id = btn.closest("[data-model]")?.dataset.model;
      if (!id) return;
      try {
        await invokeTauri("remove_model", { id });
        toast(t("models.removed", { name: id }));
        await reloadProxyUi();
      } catch (e) {
        toast(String(e));
      }
    });
  });
}

export function updateProxyRow() {
  const p = state.proxy || mockProxy();
  const mode = proxyState();
  const dot = $("#proxy-dot");
  const tag = $("#proxy-tag");
  const chip = $("#proxy-chip");
  chip.classList.toggle("off", mode !== "run");
  chip.classList.toggle("busy", mode === "warn");
  $("#proxy-chip-text").textContent = mode === "run" ? p.listen : t(mode === "warn" ? "proxy.busyChip" : "proxy.stoppedChip");
  chip.title = t(mode === "run" ? "lamp.proxyOn" : mode === "warn" ? "lamp.proxyBusy" : "lamp.proxyOff");
  if (!dot || !tag) return;
  dot.classList.toggle("off", mode !== "run");
  tag.textContent = { run: "RUNNING", warn: "PORT IN USE", off: "STOPPED" }[mode];
  tag.style.color = { run: "var(--run)", warn: "var(--warn)", off: "var(--muted)" }[mode];
}

/** 端点卡片：只显示运行信息；供应商/模型在下方可编辑卡片 */
export function renderProxyPanel() {
  const p = state.proxy || mockProxy();
  const mode = proxyState();
  const rows = [];
  if (mode === "run") {
    rows.push([t("proxy.uptime"), formatUptime(p.uptime_ms)]);
    rows.push([t("proxy.requests"), `${p.requests}${p.failures ? " · " + t("proxy.failures", { n: p.failures }) : ""}`]);
    if (p.last_request) {
      rows.push([t("proxy.lastRequest"), `${p.last_request.provider} · ${p.last_request.model} · HTTP ${p.last_request.status}`]);
    }
  }
  if (p.last_error) rows.push([t("proxy.lastError"), p.last_error]);
  rows.push([t("proxy.config"), p.config_path]);

  $("#proxy-info").innerHTML = `<dl class="kv">${rows
    .map(([k, v]) => `<dt>${escapeHtml(k)}</dt><dd>${escapeHtml(v)}</dd>`)
    .join("")}</dl>`;

  const toggle = $("#btn-proxy-toggle");
  toggle.textContent = t(mode === "run" ? "proxy.stop" : "proxy.start");
  toggle.classList.toggle("primary", mode !== "run");
  toggle.disabled = mode === "warn";
  $("#proxy-autostart").checked = !!p.auto_start;
  $("#proxy-endpoint").textContent = p.endpoint;
}
