// Read-only comparison against WorkBuddy's local session files and SQLite index.
// Prints only check names and counts: session titles and project paths stay local.
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repo = fileURLToPath(new URL("..", import.meta.url));
const root = path.join(process.env.USERPROFILE || os.homedir(), ".workbuddy");
const projects = path.join(root, "projects");
const database = path.join(root, "workbuddy.db");
let failures = 0;
function check(label, valid) {
  console.log(`${valid ? "PASS" : "FAIL"} ${label}`);
  if (!valid) failures++;
}

const files = fs.existsSync(projects)
  ? fs.readdirSync(projects, { withFileTypes: true }).flatMap((dir) =>
      dir.isDirectory()
        ? fs.readdirSync(path.join(projects, dir.name))
            .filter((name) => name.endsWith(".jsonl"))
            .map((name) => path.join(projects, dir.name, name))
        : [])
  : [];
check("local WorkBuddy sessions exist", files.length > 0);
check("WorkBuddy SQLite index exists", fs.existsSync(database));
if (failures) process.exit(1);

const indexed = JSON.parse(execFileSync("sqlite3", ["-json", database,
  "SELECT id, title, cwd, updated_at FROM sessions;"], { encoding: "utf8" }) || "[]");
const byId = new Map(indexed.map((row) => [row.id, row]));

// Exercise the same public Rust scan entry used by Tauri, without exposing
// session text in the verifier output or changing any WorkBuddy files.
const examples = path.join(repo, "src-tauri", "examples");
const example = path.join(examples, "verify-workbuddy.rs");
fs.mkdirSync(examples, { recursive: true });
fs.writeFileSync(example, `use orrery_lib::adapters as a;
fn main() {
    let rows = a::list_all_sessions().unwrap();
    for row in rows.iter().filter(|s| s.harness == "workbuddy") {
        println!("ROW_JSON={}", serde_json::to_string(row).unwrap());
    }
    let storage = a::storage_stats().into_iter().find(|s| s.harness == "workbuddy").unwrap();
    println!("STORAGE_JSON={}", serde_json::to_string(&storage).unwrap());
}
`);
let result;
try {
  result = spawnSync(process.env.CARGO || "cargo",
    ["run", "--manifest-path", "src-tauri/Cargo.toml", "--example", "verify-workbuddy", "--quiet"],
    { cwd: repo, encoding: "utf8" });
} finally {
  fs.rmSync(example, { force: true });
  try { fs.rmdirSync(examples); } catch {}
}
if (result.error || result.status !== 0) {
  throw new Error(`Rust scan failed: ${result.error?.message || result.stderr?.slice(0, 300)}`);
}
const lines = result.stdout.split(/\r?\n/);
const rows = lines.filter((line) => line.startsWith("ROW_JSON="))
  .map((line) => JSON.parse(line.slice("ROW_JSON=".length)));
const storage = JSON.parse(lines.find((line) => line.startsWith("STORAGE_JSON="))?.slice("STORAGE_JSON=".length) || "null");
check("scan count matches session files", rows.length === files.length);
check("storage count matches session files", storage?.sessions === files.length);
check("storage byte count matches files", storage?.session_bytes ===
  files.reduce((sum, file) => sum + fs.statSync(file).size, 0));

for (const file of files) {
  const id = path.basename(file, ".jsonl");
  const row = rows.find((item) => item.id === id);
  const db = byId.get(id);
  const items = fs.readFileSync(file, "utf8").split(/\r?\n/)
    .filter(Boolean).flatMap((line) => { try { return [JSON.parse(line)]; } catch { return []; } });
  const title = items.find((item) => item.type === "ai-title")?.aiTitle;
  const cwd = items.find((item) => item.cwd)?.cwd
    ?.replace(/\\/g, "/").replace(/^([a-z]):/, (_, drive) => `${drive.toUpperCase()}:`);
  check("session indexed by native ID", !!row && !!db);
  if (!row || !db) continue;
  check("title agrees with session and WorkBuddy index", row.title === title && row.title === db.title);
  check("project path comes from cwd", row.project === cwd && row.project ===
    db.cwd.replace(/\\/g, "/").replace(/^([a-z]):/, (_, drive) => `${drive.toUpperCase()}:`));
  check("session size agrees with file", row.size_bytes === fs.statSync(file).size);
  const seen = new Set();
  let input = 0, output = 0, total = 0;
  for (const item of items) {
    const usage = item.message?.usage || item.providerData?.rawUsage;
    const key = item.providerData?.messageId || item.id;
    if (!usage || !key || seen.has(key)) continue;
    seen.add(key);
    input += usage.input_tokens ?? usage.inputTokens ?? usage.prompt_tokens ?? 0;
    output += usage.output_tokens ?? usage.outputTokens ?? usage.completion_tokens ?? 0;
    total += usage.total_tokens ?? usage.totalTokens ?? 0;
  }
  check("input and output match independent call sum", row.usage.input === input && row.usage.output === output);
  check("API call count deduplicates shared message IDs", row.usage.calls === seen.size);
  check("total matches provider records", total === 0 ||
    row.usage.input + row.usage.output + row.usage.cache_write + row.usage.cache_read + row.usage.unsplit === total);
}
console.log(`Checked ${files.length} session(s); ${failures} failure(s).`);
if (failures) process.exit(1);
