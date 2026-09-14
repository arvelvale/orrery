/**
 * Openplane i18n — en / zh-CN / ja
 *
 * 规则：
 * - 新增文案三种语言同时写，缺哪个 `missingKeys()` 会在控制台报出来
 * - 仪表式等宽标签（RUNNING / IDLE / HARNESS …）不翻译，属于视觉身份
 * - 值可以是字符串，或 { one, other } 按 vars.n 取复数形式
 */

export const LOCALES = [
  { id: "en", short: "EN", name: "English" },
  { id: "zh-CN", short: "中", name: "简体中文" },
  { id: "ja", short: "日", name: "日本語" },
];

const MESSAGES = {
  en: {
    "doc.title": "Openplane · Agent Cockpit",
    "lang.label": "Language",

    "header.annunciator": "System status annunciator",
    "header.proxyChip": "Local model proxy",
    "header.refresh": "Rescan",
    "proxy.offlineChip": "Proxy offline",

    "nav.aria": "Main navigation",
    "nav.section": "Navigate",
    "nav.sessions": "Sessions",
    "nav.models": "Models",
    "nav.status": "Status",
    "foot.prototype": "Prototype v0.1",
    "foot.tagline": "Tauri · local-first",
    "runtime.tauri": "Tauri desktop",
    "runtime.browser": "Browser preview",

    "search.placeholder": "Search title / path / model…",
    "filter.all": "All",
    "source.native": "local scan",
    "source.nativeEmpty": "local scan · empty",
    "source.mock": "mock data",
    "sessions.count": { one: "{n} session · {source} · {size}", other: "{n} sessions · {source} · {size}" },
    "sessions.sizeTitle": "Session file + subagent logs + tool output",
    "session.untitled": "Session {id}",

    "empty.title": "No matching sessions",
    "empty.native": "No session folders found on this machine, or the filter is too narrow.",
    "empty.mock": "Adjust the filter or search. The desktop build scans ~/.claude and other folders.",
    "empty.scan": "Scan local folders",

    "detail.aria": "Session details",
    "detail.none": "No session selected",
    "detail.close": "Close details",
    "detail.hint": "Pick a session from the list to see its path, model and recent log.",
    "detail.calls": { one: "{n} call", other: "{n} calls" },
    "detail.subagents": { one: "incl. {n} subagent", other: "incl. {n} subagents" },
    "detail.resume": "Resume in terminal",
    "detail.copyPath": "Copy path",
    "usage.input": "in",
    "usage.cacheWrite": "cache write",
    "usage.cacheRead": "cache read",
    "usage.output": "out",

    "models.endpointTitle": "Proxy endpoint",
    "models.endpointDesc": "Every harness switches models through this entry point. API keys stay in local environment variables and never touch the repo.",
    "models.ping": "Test connection",
    "models.copyEndpoint": "Copy endpoint",
    "models.routesTitle": "Route map",
    "models.routesDesc": "Pick a default model for each harness. Saved to ~/.openplane/proxy.json (browser preview: localStorage).",
    "models.poolTitle": "Model pool",
    "models.defaultFor": "{name} default model",
    "models.note.flagship": "flagship",
    "models.note.balanced": "balanced",
    "models.note.kimi": "Kimi native",
    "models.note.openai": "OpenAI",
    "models.note.value": "best value",
    "models.note.mimo": "MiMo",

    "lamp.proxyOn": "Model proxy online",
    "lamp.proxyOff": "Model proxy offline",

    "storage.title": "Local session storage",
    "storage.notConnected": "not connected",
    "storage.rootExtra": "Folder total {size} (includes non-session files such as memory)",
    "storage.sessions": { one: "{n} session", other: "{n} sessions" },
    "storage.summary": { one: "{sessions} · {n} harness connected", other: "{sessions} · {n} harnesses connected" },

    "status.proxyName": "Model proxy",
    "status.endpoint": "Endpoint",
    "status.protocol": "Protocol",
    "status.config": "Config",
    "status.note": "Note",
    "status.proxyNote": "Keys stay on this machine, never in the repo",
    "status.sessions": "Sessions",
    "status.disk": "Disk usage",
    "status.defaultModel": "Default model",
    "status.scanPath": "Scan path",
    "status.access": "Access",
    "status.readLocal": "Reads local session folders",

    "toast.rescanned": "Rescanned",
    "toast.scanFailed": "Scan failed or not connected",
    "toast.scanNeedsDesktop": "Browser preview: scanning needs the desktop build",
    "toast.openedExplorer": "Opened in file explorer",
    "toast.openFailed": "Couldn't open it",
    "toast.previewWouldOpen": "Preview: would open {path}",
    "toast.pathCopied": "Project path copied",
    "toast.proxyReachable": "Proxy reachable · {ms}ms",
    "toast.proxyNoResponse": "No response from proxy · check port 8787",
    "toast.simulated": " (simulated)",
    "toast.endpointCopied": "Proxy endpoint copied",
    "toast.refreshedNative": "Local sessions refreshed",
    "toast.refreshedMock": "Refreshed (mock data)",
  },

  "zh-CN": {
    "doc.title": "Openplane · Agent 驾驶舱",
    "lang.label": "语言",

    "header.annunciator": "系统状态告示牌",
    "header.proxyChip": "本地模型代理",
    "header.refresh": "刷新扫描",
    "proxy.offlineChip": "代理离线",

    "nav.aria": "主导航",
    "nav.section": "导航",
    "nav.sessions": "会话",
    "nav.models": "模型",
    "nav.status": "状态",
    "foot.prototype": "原型 v0.1",
    "foot.tagline": "Tauri · 本地优先",
    "runtime.tauri": "Tauri 桌面",
    "runtime.browser": "浏览器预览",

    "search.placeholder": "搜索标题 / 路径 / 模型…",
    "filter.all": "全部",
    "source.native": "本机扫描",
    "source.nativeEmpty": "本机扫描 · 空",
    "source.mock": "mock 数据",
    "sessions.count": "{n} 条 · {source} · 占用 {size}",
    "sessions.sizeTitle": "会话文件 + 子 agent 记录 + 工具输出",
    "session.untitled": "会话 {id}",

    "empty.title": "没有匹配的会话",
    "empty.native": "本机未扫到会话目录，或筛选条件过窄。",
    "empty.mock": "调整筛选或搜索词。桌面版会扫描 ~/.claude 等目录。",
    "empty.scan": "扫描本地路径",

    "detail.aria": "会话详情",
    "detail.none": "未选择会话",
    "detail.close": "关闭详情",
    "detail.hint": "从左侧列表选择一条会话，这里显示路径、模型与最近日志。",
    "detail.calls": "{n} 次调用",
    "detail.subagents": "含 {n} 个子 agent",
    "detail.resume": "在终端恢复",
    "detail.copyPath": "复制路径",
    "usage.input": "输入",
    "usage.cacheWrite": "缓存写",
    "usage.cacheRead": "缓存读",
    "usage.output": "输出",

    "models.endpointTitle": "代理端点",
    "models.endpointDesc": "所有 harness 经此入口切换模型。密钥只在本机环境变量，不写入仓库。",
    "models.ping": "检测连通",
    "models.copyEndpoint": "复制端点",
    "models.routesTitle": "路由映射",
    "models.routesDesc": "为每个 harness 指定默认模型。写入 ~/.openplane/proxy.json（浏览器预览写 localStorage）。",
    "models.poolTitle": "可用模型池",
    "models.defaultFor": "{name} 默认模型",
    "models.note.flagship": "旗舰",
    "models.note.balanced": "均衡",
    "models.note.kimi": "Kimi 原生",
    "models.note.openai": "OpenAI",
    "models.note.value": "高性价比",
    "models.note.mimo": "MiMo",

    "lamp.proxyOn": "模型代理在线",
    "lamp.proxyOff": "模型代理离线",

    "storage.title": "本地会话占用",
    "storage.notConnected": "未接入",
    "storage.rootExtra": "目录合计 {size}（含 memory 等非会话文件）",
    "storage.sessions": "{n} 个会话",
    "storage.summary": "{sessions} · {n} 个已接入 harness",

    "status.proxyName": "模型代理",
    "status.endpoint": "端点",
    "status.protocol": "协议",
    "status.config": "配置",
    "status.note": "说明",
    "status.proxyNote": "密钥只在本机，不落仓库",
    "status.sessions": "会话数",
    "status.disk": "磁盘占用",
    "status.defaultModel": "默认模型",
    "status.scanPath": "扫描路径",
    "status.access": "接入",
    "status.readLocal": "读本地会话目录",

    "toast.rescanned": "已重新扫描",
    "toast.scanFailed": "扫描失败或未接入",
    "toast.scanNeedsDesktop": "浏览器预览：扫描需要桌面版",
    "toast.openedExplorer": "已在资源管理器打开",
    "toast.openFailed": "打开失败",
    "toast.previewWouldOpen": "预览：将打开 {path}",
    "toast.pathCopied": "已复制项目路径",
    "toast.proxyReachable": "代理可达 · {ms}ms",
    "toast.proxyNoResponse": "代理无响应 · 检查 8787 端口",
    "toast.simulated": "（模拟）",
    "toast.endpointCopied": "已复制代理端点",
    "toast.refreshedNative": "已刷新本机会话",
    "toast.refreshedMock": "已刷新（mock 数据）",
  },

  ja: {
    "doc.title": "Openplane · エージェントコックピット",
    "lang.label": "言語",

    "header.annunciator": "システム状態アナンシエーター",
    "header.proxyChip": "ローカルモデルプロキシ",
    "header.refresh": "再スキャン",
    "proxy.offlineChip": "プロキシ停止中",

    "nav.aria": "メインナビゲーション",
    "nav.section": "ナビゲーション",
    "nav.sessions": "セッション",
    "nav.models": "モデル",
    "nav.status": "ステータス",
    "foot.prototype": "プロトタイプ v0.1",
    "foot.tagline": "Tauri · ローカルファースト",
    "runtime.tauri": "Tauri デスクトップ",
    "runtime.browser": "ブラウザプレビュー",

    "search.placeholder": "タイトル / パス / モデルで検索…",
    "filter.all": "すべて",
    "source.native": "ローカルスキャン",
    "source.nativeEmpty": "ローカルスキャン · 空",
    "source.mock": "モックデータ",
    "sessions.count": "{n} 件 · {source} · {size}",
    "sessions.sizeTitle": "セッションファイル + サブエージェントログ + ツール出力",
    "session.untitled": "セッション {id}",

    "empty.title": "一致するセッションはありません",
    "empty.native": "このマシンでセッションフォルダが見つからないか、絞り込み条件が狭すぎます。",
    "empty.mock": "フィルターや検索語を調整してください。デスクトップ版は ~/.claude などのフォルダをスキャンします。",
    "empty.scan": "ローカルフォルダをスキャン",

    "detail.aria": "セッション詳細",
    "detail.none": "セッション未選択",
    "detail.close": "詳細を閉じる",
    "detail.hint": "一覧からセッションを選ぶと、パス・モデル・最近のログがここに表示されます。",
    "detail.calls": "{n} 回の呼び出し",
    "detail.subagents": "サブエージェント {n} 件を含む",
    "detail.resume": "ターミナルで再開",
    "detail.copyPath": "パスをコピー",
    "usage.input": "入力",
    "usage.cacheWrite": "キャッシュ書込",
    "usage.cacheRead": "キャッシュ読込",
    "usage.output": "出力",

    "models.endpointTitle": "プロキシエンドポイント",
    "models.endpointDesc": "すべてのハーネスはこの入口を通じてモデルを切り替えます。API キーはローカルの環境変数にのみ置かれ、リポジトリには書き込まれません。",
    "models.ping": "接続テスト",
    "models.copyEndpoint": "エンドポイントをコピー",
    "models.routesTitle": "ルートマップ",
    "models.routesDesc": "ハーネスごとにデフォルトモデルを選びます。~/.openplane/proxy.json に保存されます（ブラウザプレビューでは localStorage）。",
    "models.poolTitle": "モデルプール",
    "models.defaultFor": "{name} のデフォルトモデル",
    "models.note.flagship": "フラッグシップ",
    "models.note.balanced": "バランス",
    "models.note.kimi": "Kimi ネイティブ",
    "models.note.openai": "OpenAI",
    "models.note.value": "コスパ重視",
    "models.note.mimo": "MiMo",

    "lamp.proxyOn": "モデルプロキシ稼働中",
    "lamp.proxyOff": "モデルプロキシ停止中",

    "storage.title": "ローカルセッション容量",
    "storage.notConnected": "未接続",
    "storage.rootExtra": "フォルダ合計 {size}（memory などセッション以外のファイルを含む）",
    "storage.sessions": "{n} 件のセッション",
    "storage.summary": "{sessions} · 接続済みハーネス {n}",

    "status.proxyName": "モデルプロキシ",
    "status.endpoint": "エンドポイント",
    "status.protocol": "プロトコル",
    "status.config": "設定",
    "status.note": "メモ",
    "status.proxyNote": "キーはこのマシンにのみ保存し、リポジトリには置かない",
    "status.sessions": "セッション数",
    "status.disk": "ディスク使用量",
    "status.defaultModel": "デフォルトモデル",
    "status.scanPath": "スキャンパス",
    "status.access": "接続方式",
    "status.readLocal": "ローカルのセッションフォルダを読み取り",

    "toast.rescanned": "再スキャンしました",
    "toast.scanFailed": "スキャンに失敗したか、未接続です",
    "toast.scanNeedsDesktop": "ブラウザプレビュー：スキャンにはデスクトップ版が必要です",
    "toast.openedExplorer": "エクスプローラーで開きました",
    "toast.openFailed": "開けませんでした",
    "toast.previewWouldOpen": "プレビュー：{path} を開きます",
    "toast.pathCopied": "プロジェクトパスをコピーしました",
    "toast.proxyReachable": "プロキシ到達可能 · {ms}ms",
    "toast.proxyNoResponse": "プロキシ応答なし · ポート 8787 を確認してください",
    "toast.simulated": "（シミュレーション）",
    "toast.endpointCopied": "プロキシエンドポイントをコピーしました",
    "toast.refreshedNative": "ローカルセッションを更新しました",
    "toast.refreshedMock": "更新しました（モックデータ）",
  },
};

const LS_KEY = "openplane.locale";
let current = "en";

function normalize(tag) {
  const t = String(tag || "").toLowerCase();
  if (t.startsWith("zh")) return "zh-CN";
  if (t.startsWith("ja")) return "ja";
  if (t.startsWith("en")) return "en";
  return null;
}

/** 优先级：?lang= → 上次手动选择 → 系统语言 → en */
export function detectLocale() {
  const fromUrl = normalize(new URLSearchParams(location.search).get("lang"));
  if (fromUrl) return fromUrl;
  try {
    const saved = normalize(localStorage.getItem(LS_KEY));
    if (saved) return saved;
  } catch (_) {}
  for (const tag of navigator.languages || [navigator.language]) {
    const hit = normalize(tag);
    if (hit) return hit;
  }
  return "en";
}

export function getLocale() {
  return current;
}

export function setLocale(id, { persist = false } = {}) {
  current = MESSAGES[id] ? id : "en";
  document.documentElement.lang = current;
  if (persist) {
    try { localStorage.setItem(LS_KEY, current); } catch (_) {}
  }
}

export function t(key, vars = {}) {
  let msg = MESSAGES[current][key] ?? MESSAGES.en[key];
  if (msg == null) return key;
  if (typeof msg === "object") {
    const form = new Intl.PluralRules(current).select(Number(vars.n) || 0);
    msg = msg[form] ?? msg.other;
  }
  return msg.replace(/\{(\w+)\}/g, (_, k) => (vars[k] ?? `{${k}}`));
}

/** 静态 DOM：data-i18n / data-i18n-title / data-i18n-placeholder / data-i18n-aria */
export function applyStatic(root = document) {
  root.querySelectorAll("[data-i18n]").forEach((el) => { el.textContent = t(el.dataset.i18n); });
  root.querySelectorAll("[data-i18n-title]").forEach((el) => { el.title = t(el.dataset.i18nTitle); });
  root.querySelectorAll("[data-i18n-placeholder]").forEach((el) => { el.placeholder = t(el.dataset.i18nPlaceholder); });
  root.querySelectorAll("[data-i18n-aria]").forEach((el) => { el.setAttribute("aria-label", t(el.dataset.i18nAria)); });
  document.title = t("doc.title");
}

/** 相对时间；一周以上显示日期 */
export function formatRelative(ms) {
  if (!ms) return "—";
  const diff = (ms - Date.now()) / 1000;
  const abs = Math.abs(diff);
  const rtf = new Intl.RelativeTimeFormat(current, { numeric: "auto" });
  if (abs < 60) return rtf.format(0, "second");
  if (abs < 3600) return rtf.format(Math.round(diff / 60), "minute");
  if (abs < 86400) return rtf.format(Math.round(diff / 3600), "hour");
  if (abs < 86400 * 7) return rtf.format(Math.round(diff / 86400), "day");
  return new Intl.DateTimeFormat(current, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }).format(ms);
}

/** 开发自检：列出任一语言缺失的键 */
export function missingKeys() {
  const all = new Set(Object.values(MESSAGES).flatMap((m) => Object.keys(m)));
  const out = {};
  for (const [id, m] of Object.entries(MESSAGES)) {
    const miss = [...all].filter((k) => !(k in m));
    if (miss.length) out[id] = miss;
  }
  return out;
}
