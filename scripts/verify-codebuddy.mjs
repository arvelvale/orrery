// CodeBuddy adapter 端到端核对：用隔离 ORRERY_HOME 指向沙盒，
// 在里面造出与真实 CLI 落盘格式一致的会话文件，然后跑真实的 Rust 扫描入口。
//
// 真实数据来源（都在本机取得，不是猜的）：
//   1. `@tencent-ai/codebuddy-code` 2.1.4 产物 dist/codebuddy.js 的 SessionStoreImpl
//   2. 在隔离 HOME 里实跑 CLI 得到的一行真实 session jsonl
// 沙盒放在仓库的忽略目录 .orrery/ 下（与 verify-opencode.mjs 同一约定），不入库。
// 用法：node scripts/verify-codebuddy.mjs
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const REPO = fileURLToPath(new URL("..", import.meta.url));
const SANDBOX = path.join(REPO, ".orrery", "verify-codebuddy");
const CARGO = process.env.CARGO || "cargo";

fs.rmSync(SANDBOX, { recursive: true, force: true });
const projects = path.join(SANDBOX, ".codebuddy", "projects");

// 与真机落盘一致：目录名 = 工作目录把 / \ : 换成 -，盘符小写。
// 项目名一律用虚构示例，与 ui/ 的 mock 数据同规矩
const cases = [
  {
    dir: "d-code-acme",
    id: "b602c0ad-5818-4c08-a755-9be0b63bda5b",
    lines: [
      // 这一行是隔离 HOME 里实跑 CLI 真实写出来的内容
      '{"type":"message","role":"user","content":[{"type":"input_text","text":"say hi"}],"providerData":{"agent":"cli"},"id":"a45465263d29433c9ce2d7300ee2c506","timestamp":1790515660056}',
    ],
  },
  {
    dir: "d-code-acme",
    id: "019fdba8-940e-7f20-bfda-365ecb643e52",
    lines: [
      '{"type":"message","role":"user","content":[{"type":"input_text","text":"把登录迁到 OAuth 2.1"}],"providerData":{"agent":"cli"},"id":"1","timestamp":1788357389663}',
      '{"type":"message","role":"assistant","content":[{"type":"output_text","text":"PKCE verifier added"}],"providerData":{"agent":"cli"},"id":"2","timestamp":1788357400000,"model":"gemini-2.5-pro","message":{"usage":{"input_tokens":3200,"output_tokens":400,"total_tokens":3600}},"providerData2":0}',
      // 同一 id 重复写第二遍（模拟流式重写）：只能算一次
      '{"type":"message","role":"assistant","content":[{"type":"output_text","text":"PKCE verifier added"}],"providerData":{"agent":"cli"},"id":"2","timestamp":1788357400000,"model":"gemini-2.5-pro","message":{"usage":{"input_tokens":3200,"output_tokens":400,"total_tokens":3600}}}',
      // CLI 自己塞进去的指令，不能当标题
      '{"type":"message","role":"user","content":[{"type":"input_text","text":"<system-reminder>ignore"}],"providerData":{"agent":"cli","skipRun":true},"id":"3","timestamp":1788358355000}',
      '{"type":"message","role":"user","content":[{"type":"input_text","text":"补一下刷新令牌轮换的测试"}],"providerData":{"agent":"cli"},"id":"4","timestamp":1788358355449}',
    ],
  },
  {
    dir: "d-test",
    idb: null,
    id: "019fdba9-940e-7f20-bfda-365ecb643e53",
    lines: [
      // 只有 total 没有可信分项形状：走 unsplit，不猜比例
      '{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}],"providerData":{"usage":{"promptTokens":100,"completionTokens":20,"totalTokens":3000}},"id":"9","timestamp":1789109473979}',
    ],
  },
];

fs.mkdirSync(projects, { recursive: true });
for (const c of cases) {
  const d = path.join(projects, c.dir);
  fs.mkdirSync(d, { recursive: true });
  fs.writeFileSync(path.join(d, `${c.id}.jsonl`), c.lines.map((l) => JSON.parse(l) && l).join("\n") + "\n");
}

// 通过真实的 Tauri 命令入口读：直接调库的 list_all_sessions / storage_stats
const probe = `
use orrery_lib::adapters as a;
fn main() {
  let rows = a::list_all_sessions().unwrap_or_default();
  let cb: Vec<_> = rows.iter().filter(|s| s.harness == "codebuddy").collect();
  println!("CODEBUDDY_COUNT={}", cb.len());
  for s in &cb {
    println!("ROW|{}|{}|{}|{}|{}|{}|{}|{}", s.id, s.title, s.project, s.model, s.status, s.tokens, s.size_bytes, s.updated_ms);
    println!("USAGE|{}|{}|{}|{}|{}", s.usage.input, s.usage.cache_write, s.usage.cache_read, s.usage.output, s.usage.unsplit);
  }
  let st = a::storage_stats().iter().find(|s| s.harness == "codebuddy").cloned();
  if let Some(st) = st {
    println!("STORAGE|{}|{}|{}|{}|{}", st.connected, st.sessions, st.session_bytes, st.root_bytes, st.root);
  }
}
`;

fs.mkdirSync(path.join(REPO, "src-tauri/examples"), { recursive: true });
fs.writeFileSync(path.join(REPO, "src-tauri/examples/verify-codebuddy.rs"), probe);

let out = "";
try {
  out = execFileSync(CARGO, ["run", "--manifest-path", "src-tauri/Cargo.toml", "--example", "verify-codebuddy", "--quiet"],
    { cwd: REPO, env: { ...process.env, ORRERY_HOME: SANDBOX }, encoding: "utf8", maxBuffer: 1 << 26 });
} catch (e) {
  console.error("cargo 失败：", e.stdout, e.stderr);
  process.exit(1);
} finally {
  fs.rmSync(path.join(REPO, "src-tauri/examples/verify-codebuddy.rs"), { force: true });
  try { fs.rmdirSync(path.join(REPO, "src-tauri/examples")); } catch {}
}

console.log(out.trim());
console.log("\n=== 逐项判定 ===");
const ROW = {};
for (const l of out.split("\n").filter((l) => l.startsWith("ROW|"))) {
  const [id, title, project, model, status, tokens, size, updated] = l.split("|").slice(1);
  ROW[id] = { title, project, model, status, tokens, size, updated };
}
const USAGE = {};
out.split("\n").filter((l) => l.startsWith("USAGE|")).forEach((l) => {
  // 行的顺序与 ROW 一致（按 updated 倒序），用行序对上 id
  const ids = Object.keys(ROW);
  USAGE[ids[out.split("\n").filter((x) => x.startsWith("USAGE|")).indexOf(l)]] = l.split("|").slice(1).map(Number);
});
const storage = out.split("\n").find((l) => l.startsWith("STORAGE|"))?.split("|").slice(1);

const check = (name, ok, detail) => console.log(`  ${ok ? "✓" : "✗"} ${name}${detail ? "  " + detail : ""}`);

const real = "b602c0ad-5818-4c08-a755-9be0b63bda5b";
const multi = "019fdba8-940e-7f20-bfda-365ecb643e52";
const unsplit = "019fdba9-940e-7f20-bfda-365ecb643e53";

check("真实 CLI 行：标题 = say hi", ROW[real]?.title === "say hi", ROW[real]?.title);
check("真实 CLI 行：项目路径按目录名还原", ROW[real]?.project === "D:/code/acme", ROW[real]?.project);
check("真实 CLI 行：updated_ms 取行内 timestamp", ROW[real]?.updated === "1790515660056", ROW[real]?.updated);
check("真实 CLI 行：无 usage 时 tokens 显示占位符", ROW[real]?.tokens === "—", ROW[real]?.tokens);

check("多行会话：标题取最近一条用户消息（跳过 skipRun）", ROW[multi]?.title === "补一下刷新令牌轮换的测试", ROW[multi]?.title);
check("多行会话：updated_ms 取最后一条时间戳", ROW[multi]?.updated === "1788358355449", ROW[multi]?.updated);
check("多行会话：model 从行里读出来", ROW[multi]?.model === "gemini-2.5-pro", ROW[multi]?.model);
check("多行会话：tokens 短格式 3600 → 3.6k", ROW[multi]?.tokens === "3.6k", ROW[multi]?.tokens);
check("流式重复 id 只累加一次（output=400 而非 800）", USAGE[multi]?.[3] === 400, "output=" + UNSPLIT_OR(USAGE[multi]?.[3]));
check("total 与分项一致时 unsplit = 0", UNSPLIT_OR(USAGE[multi]?.[4]) === 0, "unsplit=" + UNSPLIT_OR(USAGE[multi]?.[4]));

check("只有 total 更大时差额进 unsplit（2880）", UNSPLIT_OR(USAGE[unsplit]?.[4]) === 2880, "unsplit=" + UNSPLIT_OR(USAGE[unsplit]?.[4]));
check("unsplit 计入 tokens 总数（3k）", ROW[unsplit]?.tokens === "3k", ROW[unsplit]?.tokens);
check("promptTokens/completionTokens 字段名兼容", UNSPLIT_OR(USAGE[unsplit]?.[0]) === 100 && UNSPLIT_OR(USAGE[unsplit]?.[3]) === 20,
  `input=${UNSPLIT_OR(USAGE[unsplit]?.[0])} output=${UNSPLIT_OR(USAGE[unsplit]?.[3])}`);

check("storage 统计到 3 条会话", storage?.[1] === "3", storage?.join(" | "));
check("storage root 路径正确", storage?.[4] === "~/.codebuddy/projects/", storage?.[4]);
check("storage connected = true", storage?.[0] === "true", storage?.[0]);
check("storage bytes 与文件实际字节一致", (() => {
  const want = cases.reduce((n, c) => n + fs.statSync(path.join(projects, c.dir, `${c.id}.jsonl`)).size, 0);
  return storage?.[2] === String(want);
})(), `声明=${storage?.[2]}`);
check("单条 size_bytes 与文件字节一致", (() => {
  const want = fs.statSync(path.join(projects, "d-code-acme", `${multi}.jsonl`)).size;
  return ROW[multi]?.size === String(want);
})(), `声明=${ROW[multi]?.size}`);

function UNSPLIT_OR(v) { return v === undefined ? "?" : v; }

console.log("\n=== 未验证项（如实说明）===");
console.log("  · 本机 CLI 账号已登出，拿不到带 usage 的真实会话文件；usage 取值规则按产物代码推演 + 单测覆盖");
console.log("  · `codebuddy --resume <id>` 的参数来自 CLI 自己的 --help，未在真机联调");
console.log("  · 删除未开放");
