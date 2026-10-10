// Read-only comparison against StepCode's local session files.
// Prints only check names and counts: session titles and project paths stay local.
//
// What this cross-checks, per session:
//   - Orrery's four token buckets against an independent re-sum of the JSONL
//   - the API call count
//   - the folded subagent count and the session's on-disk size
//   - that every .jsonl under the sessions root is accounted for exactly once
//     (a subagent counted twice, or dropped, would show up as a total mismatch)
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repo = fileURLToPath(new URL("..", import.meta.url));

// Same three-level resolution the Rust adapter uses, in the same order.
function sessionsRoot() {
  const direct = process.env.STEP_CODING_AGENT_SESSION_DIR;
  if (direct && fs.existsSync(direct)) return direct;
  const base = process.env.STEP_CODING_AGENT_DIR ||
    path.join(process.env.USERPROFILE || os.homedir(), ".stepcode", "agent");
  const root = path.join(base, "sessions");
  return fs.existsSync(root) ? root : null;
}

const root = sessionsRoot();
let failures = 0;
function check(label, valid) {
  console.log(`${valid ? "PASS" : "FAIL"} ${label}`);
  if (!valid) failures++;
}
if (!root) {
  console.log("SKIP no StepCode sessions directory on this machine");
  process.exit(0);
}

const buckets = (u) => ({
  input: u.input ?? 0,
  output: u.output ?? 0,
  cacheRead: u.cacheRead ?? 0,
  cacheWrite: u.cacheWrite ?? 0,
  totalTokens: u.totalTokens ?? 0,
});

// Independent re-sum of one .jsonl: assistant entries, plus compaction and
// branch_summary. toolResult details carry a subagent's usage — those files are
// folded in separately, so counting them here would double the total.
function sumFile(file) {
  let input = 0, output = 0, cacheRead = 0, cacheWrite = 0, unsplit = 0, calls = 0;
  let first = 0, last = 0;
  for (const line of fs.readFileSync(file, "utf8").split(/\r?\n/)) {
    if (!line.trim()) continue;
    let v;
    try { v = JSON.parse(line); } catch { continue; }
    const ms = v.timestamp ? Date.parse(v.timestamp) : (v.message?.timestamp ?? 0);
    if (ms) { if (!first) first = ms; last = Math.max(last, ms); }
    const type = v.type;
    let usage = null;
    if (type === "message" && v.message?.role === "assistant") usage = v.message.usage;
    else if (type === "compaction" || type === "branch_summary") usage = v.usage;
    if (!usage) continue;
    const b = buckets(usage);
    input += b.input;
    output += b.output;
    cacheRead += b.cacheRead;
    cacheWrite += b.cacheWrite;
    unsplit += Math.max(0, b.totalTokens - b.input - b.output - b.cacheRead - b.cacheWrite);
    calls += 1;
  }
  return { input, output, cacheRead, cacheWrite, unsplit, calls, first, last };
}

const dirs = fs.readdirSync(root, { withFileTypes: true })
  .filter((d) => d.isDirectory())
  .map((d) => path.join(root, d.name));
const all = [];
for (const dir of dirs) {
  for (const name of fs.readdirSync(dir)) {
    if (name.endsWith(".jsonl")) all.push({ dir, file: path.join(dir, name), name });
  }
}
const headerOf = (file) => {
  const line = fs.readFileSync(file, "utf8").split(/\r?\n/).find((l) => l.trim());
  try { return JSON.parse(line); } catch { return {}; }
};
const isSub = (f) => f.name.replace(/\.jsonl$/, "").split("_").slice(1).join("_").startsWith("subagent-");
const mains = all.filter((f) => !isSub(f));
const subs = all.filter((f) => isSub(f));
console.log(`${all.length} session file(s): ${mains.length} main, ${subs.length} subagent`);

// Exercise the same public Rust scan entry used by Tauri, without exposing
// session text in the verifier output or changing any StepCode files.
const examples = path.join(repo, "src-tauri", "examples");
const example = path.join(examples, "verify-stepcode.rs");
fs.mkdirSync(examples, { recursive: true });
fs.writeFileSync(example, `use orrery_lib::adapters as a;
fn main() {
    let rows = a::list_all_sessions().unwrap();
    for row in rows.iter().filter(|s| s.harness == "stepcode") {
        println!("ROW_JSON={}", serde_json::to_string(row).unwrap());
    }
    let storage = a::storage_stats().into_iter().find(|s| s.harness == "stepcode").unwrap();
    println!("STORAGE_JSON={}", serde_json::to_string(&storage).unwrap());
}
`);
let result;
try {
  result = spawnSync(process.env.CARGO || "cargo",
    ["run", "--manifest-path", "src-tauri/Cargo.toml", "--example", "verify-stepcode", "--quiet"],
    { cwd: repo, encoding: "utf8" });
} finally {
  fs.rmSync(example, { force: true });
  try { fs.rmdirSync(examples); } catch {}
}
if (result.error || result.status !== 0) {
  throw new Error(`Rust scan failed: ${result.error?.message || result.stderr?.slice(0, 300)}`);
}
const lines = result.stdout.split(/\r?\n/);
const rows = lines.filter((l) => l.startsWith("ROW_JSON=")).map((l) => JSON.parse(l.slice("ROW_JSON=".length)));
const storage = JSON.parse(lines.find((l) => l.startsWith("STORAGE_JSON="))?.slice("STORAGE_JSON=".length) || "null");

// Fold subagents into their parent by time containment, the same rule the
// adapter uses (the format records no parent id).
const spanOf = (f) => { const s = sumFile(f.file); return { first: s.first, last: s.last }; };
const groups = new Map();
for (const f of all) {
  const key = f.dir;
  if (!groups.has(key)) groups.set(key, { mains: [], subs: [] });
  groups.get(key)[isSub(f) ? "subs" : "mains"].push(f);
}
const folded = new Map();
for (const { mains: ms, subs: ss } of groups.values()) {
  const spans = ms.map((m) => ({ m, ...spanOf(m) }));
  for (const sub of ss) {
    const { first, last } = spanOf(sub);
    let best = null;
    for (const s of spans) {
      if (first >= s.first && last <= s.last && (!best || s.last - s.first < best.last - best.first)) best = s;
    }
    if (best) {
      if (!folded.has(best.m.name)) folded.set(best.m.name, []);
      folded.get(best.m.name).push(sub);
    }
  }
}

check("every main session is listed", rows.filter((r) => r.kind !== "subagent").length === mains.length);
check("storage session count matches main sessions", storage?.sessions === mains.length);
check("storage bytes match every file on disk", storage?.session_bytes ===
  all.reduce((sum, f) => sum + fs.statSync(f.file).size, 0));

for (const main of mains) {
  const id = headerOf(main.file).id;
  const row = rows.find((r) => r.id === id);
  check("main session indexed by native ID", !!row);
  if (!row) continue;
  const want = sumFile(main.file);
  const children = folded.get(main.name) || [];
  for (const child of children) {
    const s = sumFile(child.file);
    for (const k of ["input", "output", "cacheRead", "cacheWrite", "unsplit", "calls"]) want[k] += s[k];
  }
  const size = fs.statSync(main.file).size + children.reduce((n, c) => n + fs.statSync(c.file).size, 0);
  check("token buckets match independent re-sum",
    row.usage.input === want.input && row.usage.output === want.output &&
    row.usage.cache_read === want.cacheRead && row.usage.cache_write === want.cacheWrite &&
    row.usage.unsplit === want.unsplit);
  check("API call count matches assistant entries", row.usage.calls === want.calls);
  check("folded subagent count matches time containment", row.subagents === children.length);
  check("session size includes folded subagent files", row.size_bytes === size);
}

// A subagent counted twice, or dropped entirely, breaks the grand total.
const grand = all.reduce((acc, f) => {
  const s = sumFile(f.file);
  for (const k of ["input", "output", "cacheRead", "cacheWrite", "unsplit", "calls"]) acc[k] += s[k];
  return acc;
}, { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, unsplit: 0, calls: 0 });
const rowsTotal = rows.reduce((acc, r) => {
  for (const k of ["input", "output", "cache_read", "cache_write", "unsplit", "calls"]) {
    acc[k.replace("cache_read", "cacheRead").replace("cache_write", "cacheWrite")] += r.usage[k];
  }
  return acc;
}, { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, unsplit: 0, calls: 0 });
check("grand totals count every file exactly once",
  ["input", "output", "cacheRead", "cacheWrite", "unsplit", "calls"]
    .every((k) => rowsTotal[k] === grand[k]));
// Subagents whose parent is not on this machine are listed on their own rather
// than folded; either way their tokens must not vanish.
const listed = rows.length;
const orphaned = subs.filter((s) => !folded.has(s.name) &&
  ![...folded.values()].some((list) => list.some((c) => c.name === s.name)));
check("row count covers main sessions plus unfolded subagents",
  listed === mains.length + orphaned.length);

const fmt = (n) => n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${(n / 1e3).toFixed(1)}K` : String(n);
const foldedCount = [...folded.values()].reduce((n, list) => n + list.length, 0);
console.log(`Checked ${mains.length} main session(s) + ${subs.length} subagent file(s); ${failures} failure(s).`);
console.log(`Subagents: ${foldedCount} folded into a parent, ${orphaned.length} listed on their own.`);
console.log(`Totals: input ${fmt(grand.input)}, cache read ${fmt(grand.cacheRead)}, ` +
  `cache write ${fmt(grand.cacheWrite)}, output ${fmt(grand.output)}, ` +
  `unsplit ${grand.unsplit}, calls ${grand.calls}.`);
if (failures) process.exit(1);
