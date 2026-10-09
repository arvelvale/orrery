/**
 * mock 数据：只在没有 Tauri 后端的浏览器预览里用。
 * 全部是虚构的示例项目，不要换成真实项目名或路径（README 截图就截这份数据，仓库是公开的）。
 * 标题/摘要是"用户输入的内容"，按语言各写一份，让三语截图各自自然。
 */
const MODELS = [
  { id: "claude-opus-5.5", vendor: "anthropic", note: "models.note.flagship" },
  { id: "claude-sonnet-5.5", vendor: "anthropic", note: "models.note.balanced" },
  { id: "kimi-k3", vendor: "moonshot", note: "models.note.kimi" },
  { id: "gpt-6-astra", vendor: "openai", note: "models.note.openai" },
  { id: "gpt-6-sol", vendor: "openai", note: "models.note.codex" },
  { id: "deepseek-v4.1-flash", vendor: "deepseek", note: "models.note.value" },
  { id: "mimo-v2.6-pro", vendor: "xiaomi", note: "models.note.mimo" },
];

/*
 * 浏览器预览用的 mock 会话：全部是虚构的示例项目，不要换成真实项目名或路径
 * （README 截图就截这份数据，仓库是公开的）。
 * 标题/摘要是"用户输入的内容"，按语言各写一份，让三语截图各自自然。
 */
const MIN = 60_000;
const SEED_SESSIONS = [
  {
    id: "7f3a9c2e",
    harness: "cc",
    title: {
      en: "acme-web · Migrate login to OAuth 2.1",
      "zh-CN": "acme-web · 登录迁移到 OAuth 2.1",
      ja: "acme-web · ログインを OAuth 2.1 に移行",
    },
    excerpt: {
      en: "Swap the session cookie flow for PKCE and keep existing users signed in.",
      "zh-CN": "把会话 Cookie 流程换成 PKCE，同时保持老用户登录态。",
      ja: "セッション Cookie のフローを PKCE に置き換え、既存ユーザーのログイン状態を維持する。",
    },
    project: "~/code/acme-web",
    model: "claude-sonnet-5.5",
    status: "running",
    ago: 2 * MIN,
    usage: { input: 3200, cache_write: 5100, cache_read: 31400, output: 2400, calls: 18 },
    sizeBytes: 3_984_589,
    subagents: 2,
    log: [
      ["hi", "read src/auth/session.ts"],
      ["ok", "PKCE verifier + challenge added"],
      ["ok", "refresh token rotation covered by tests"],
      ["", "updating callback route…"],
    ],
  },
  {
    id: "b81d04f7",
    harness: "kimi",
    title: {
      en: "weather-cli · Add hourly forecast command",
      "zh-CN": "weather-cli · 新增逐小时预报命令",
      ja: "weather-cli · 1 時間ごとの予報コマンドを追加",
    },
    excerpt: {
      en: "Waiting for confirmation on the output table format.",
      "zh-CN": "等待确认输出表格的格式。",
      ja: "出力テーブルの形式について確認待ち。",
    },
    project: "~/code/weather-cli",
    model: "kimi-k3",
    status: "idle",
    ago: 18 * MIN,
    usage: { input: 9800, cache_write: 12000, cache_read: 101500, output: 4700, calls: 41 },
    sizeBytes: 22_439_526,
    subagents: 5,
    log: [
      ["ok", "hourly subcommand wired"],
      ["warn", "API rate limit hit twice"],
    ],
  },
  {
    id: "c29e6b10",
    harness: "dsh",
    title: {
      en: "pixel-notes · Fix flaky sync test",
      "zh-CN": "pixel-notes · 修复不稳定的同步测试",
      ja: "pixel-notes · 不安定な同期テストを修正",
    },
    excerpt: {
      en: "Same failure showed up twice. Back to the stack trace before touching code.",
      "zh-CN": "同一个报错第二次出现，先回到错误栈，再动代码。",
      ja: "同じエラーが 2 回目。コードを触る前にスタックトレースに戻る。",
    },
    project: "~/code/pixel-notes",
    model: "deepseek-v4.1-flash",
    status: "error",
    ago: 60 * MIN,
    usage: { input: 1200, cache_write: 1800, cache_read: 5600, output: 800, calls: 6 },
    sizeBytes: 629_146,
    subagents: 0,
    log: [
      ["warn", "sync.spec.ts timed out after 5000ms"],
      ["warn", "retry with fake timers → still fails"],
      ["", "root cause not isolated yet"],
    ],
  },
  {
    id: "d4c71a58",
    harness: "codex",
    title: {
      en: "todo-api · Write OpenAPI docs",
      "zh-CN": "todo-api · 编写 OpenAPI 文档",
      ja: "todo-api · OpenAPI ドキュメントを作成",
    },
    excerpt: {
      en: "All 14 endpoints documented, examples generated from tests.",
      "zh-CN": "14 个接口全部写完，示例由测试用例生成。",
      ja: "14 個のエンドポイントをすべて記述し、例はテストから生成。",
    },
    project: "~/code/todo-api",
    model: "gpt-6-sol",
    status: "done",
    ago: 26 * 60 * MIN,
    usage: { input: 2600, cache_write: 3900, cache_read: 23300, output: 1900, calls: 14 },
    sizeBytes: 5_138_022,
    subagents: 1,
    log: [
      ["ok", "openapi.yaml validated"],
      ["ok", "docs site preview built"],
    ],
  },
  {
    id: "e6f2b390",
    harness: "cc",
    title: {
      en: "blog-engine · Speed up static build",
      "zh-CN": "blog-engine · 加速静态站点构建",
      ja: "blog-engine · 静的ビルドを高速化",
    },
    excerpt: {
      en: "Profile first: markdown parsing is 70% of build time.",
      "zh-CN": "先做性能分析：Markdown 解析占了构建时间的 70%。",
      ja: "まず計測：Markdown の解析がビルド時間の 70% を占める。",
    },
    project: "~/code/blog-engine",
    model: "claude-opus-5.5",
    status: "idle",
    ago: 3 * 60 * MIN,
    usage: { input: 4100, cache_write: 6300, cache_read: 42900, output: 2900, calls: 22 },
    sizeBytes: 8_703_181,
    subagents: 3,
    log: [
      ["hi", "profiled 412 posts"],
      ["", "awaiting cache strategy decision"],
    ],
  },
  {
    id: "f90a3d21",
    harness: "codex",
    title: {
      en: "chess-bot · Tune search depth",
      "zh-CN": "chess-bot · 调整搜索深度",
      ja: "chess-bot · 探索深度を調整",
    },
    excerpt: {
      en: "Benchmarking depth 6 vs 7 against the opening book.",
      "zh-CN": "用开局库对比搜索深度 6 和 7 的表现。",
      ja: "定跡データで探索深度 6 と 7 を比較中。",
    },
    project: "~/code/chess-bot",
    model: "gpt-6-sol",
    status: "running",
    ago: 20_000,
    usage: { input: 900, cache_write: 1400, cache_read: 5300, output: 600, calls: 5 },
    sizeBytes: 419_430,
    subagents: 0,
    log: [
      ["hi", "running 200-game match"],
      ["ok", "depth 7: +38 Elo, 2.1× slower"],
    ],
  },
  {
    id: "ses_a4d0e7b1",
    harness: "opencode",
    title: {
      en: "ledger-sync · Reconcile duplicate entries",
      "zh-CN": "ledger-sync · 对账去掉重复流水",
      ja: "ledger-sync · 重複した仕訳を突き合わせる",
    },
    excerpt: {
      en: "Two imports created the same rows; dedupe by external id, not by amount.",
      "zh-CN": "两次导入生成了相同的流水，按外部 id 去重，不按金额。",
      ja: "2 回の取り込みで同じ行ができた。金額ではなく外部 ID で重複を排除する。",
    },
    project: "~/code/ledger-sync",
    model: "claude-sonnet-5.5",
    status: "idle",
    ago: 9 * 60 * MIN,
    usage: { input: 6400, cache_write: 900, cache_read: 88200, output: 5100, calls: 27 },
    sizeBytes: 14_680_064,
    subagents: 2,
    log: [
      ["ok", "found 312 duplicate rows"],
      ["", "waiting on the dedupe key decision"],
    ],
  },
  {
    id: "sess_5be02c71",
    harness: "zcode",
    title: {
      en: "trail-map · Cluster markers at low zoom",
      "zh-CN": "trail-map · 缩小时合并地图标记",
      ja: "trail-map · 縮小時にマーカーをまとめる",
    },
    excerpt: {
      en: "4,000 markers freeze the tab below zoom 9; cluster on the server side instead.",
      "zh-CN": "缩放到 9 级以下时 4000 个标记会卡死页面，改成服务端聚合。",
      ja: "ズーム 9 未満で 4,000 個のマーカーがタブを固める。サーバー側でまとめる。",
    },
    project: "~/code/trail-map",
    model: "glm-5.3-flash",
    status: "idle",
    ago: 5 * 60 * MIN,
    usage: { input: 5200, cache_write: 0, cache_read: 96400, output: 3100, calls: 31 },
    sizeBytes: 12_582_912,
    subagents: 0,
    log: [
      ["ok", "supercluster wired to /tiles"],
      ["", "benchmarking zoom 6–9"],
    ],
  },
  {
    id: "4c1e9a07-2b6d-4f3e-9a51-7d0c8e2f6b13",
    harness: "antigravity",
    title: {
      en: "recipe-box · Import recipes from a URL",
      "zh-CN": "recipe-box · 从网址导入菜谱",
      ja: "recipe-box · URL からレシピを取り込む",
    },
    excerpt: {
      en: "Most sites embed schema.org Recipe JSON-LD; fall back to readability for the rest.",
      "zh-CN": "大多数网站都内嵌了 schema.org 的 Recipe JSON-LD，其余的再用正文提取兜底。",
      ja: "多くのサイトは schema.org の Recipe JSON-LD を埋め込んでいる。残りは本文抽出で補う。",
    },
    project: "~/code/recipe-box",
    model: "gemini-3.8-flash",
    status: "idle",
    ago: 7 * 60 * MIN,
    usage: { input: 48200, cache_write: 0, cache_read: 512400, output: 6300, calls: 22 },
    sizeBytes: 1_153_434,
    subagents: 0,
    log: [
      ["ok", "JSON-LD parser handles 9 of 10 test sites"],
      ["", "adding the readability fallback"],
    ],
  },
  {
    // WorkBuddy：只读。cwd 在每行里，项目路径是精确的；没有可用的终端恢复命令
    id: "8c41a7d2",
    harness: "workbuddy",
    title: {
      en: "canvas-notes · Add illustrations to study cards",
      "zh-CN": "canvas-notes · 给学习卡片添加插图",
      ja: "canvas-notes · 学習カードにイラストを追加",
    },
    excerpt: {
      en: "48 of the 120 sample cards still need cover art.",
      "zh-CN": "120 张示例卡片中，还有 48 张缺少封面图。",
      ja: "120 枚のサンプルカードのうち、48 枚に表紙画像がありません。",
    },
    project: "~/code/canvas-notes",
    model: "deepseek-v4.1-flash",
    status: "idle",
    ago: 5 * 60 * MIN,
    usage: { input: 12_460, cache_write: 0, cache_read: 0, output: 1_340, calls: 14 },
    sizeBytes: 864_220,
    subagents: 0,
    log: [
      ["hi", "先看下项目结构"],
      ["ok", "read package.json"],
      ["ok", "generate covers in batches of 20"],
    ],
  },
];

const DEFAULT_ROUTES = {
  cc: "claude-sonnet-5.5",
  kimi: "kimi-k3",
  dsh: "deepseek-v4.1-flash",
  codex: "gpt-6-sol",
  opencode: "claude-sonnet-5.5",
};

export { MODELS, MIN, SEED_SESSIONS, DEFAULT_ROUTES };
