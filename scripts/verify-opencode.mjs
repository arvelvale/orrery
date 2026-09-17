// Read-only reconciliation against a running debug Tauri window (CDP port 9223).
// Private results/screenshots stay in ignored .orrery/. Never invokes delete_sessions.
import { DatabaseSync } from 'node:sqlite';
import { mkdir, writeFile, readFile, stat } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { homedir } from 'node:os';
import { join } from 'node:path';
import assert from 'node:assert/strict';

const folder = join(process.env.XDG_DATA_HOME || join(homedir(), '.local', 'share'), 'opencode');
const dbPath = join(folder, 'opencode.db');
await mkdir('.orrery', { recursive: true });
async function fingerprint() {
  const result = {};
  for (const suffix of ['', '-wal']) {
    try {
      const path = dbPath + suffix;
      result[suffix || 'db'] = { size: (await stat(path)).size,
        sha256: createHash('sha256').update(await readFile(path)).digest('hex') };
    } catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  return result;
}
if (process.argv.includes('--snapshot')) {
  await writeFile('.orrery/opencode-before.json', JSON.stringify(await fingerprint()));
  console.log('OpenCode DB/WAL fingerprint saved locally.');
  process.exit(0);
}

// Each run gets its own baseline: a CLI invocation or live OpenCode process
// between runs can legitimately change WAL. Keep --snapshot as an optional
// historical fingerprint, never reuse it to judge a later run's writes.
const before = await fingerprint();
const db = new DatabaseSync(dbPath, { readOnly: true });
db.exec('BEGIN');
// Recursive SQL is deliberately independent of the Rust parent traversal.
const expected = db.prepare(`WITH RECURSIVE tree(root, id) AS (
  SELECT id, id FROM session WHERE parent_id IS NULL OR parent_id NOT IN (SELECT id FROM session)
  UNION ALL SELECT tree.root, session.id FROM session JOIN tree ON session.parent_id = tree.id
) SELECT root AS id, COUNT(*) - 1 AS subagents,
  SUM(COALESCE(tokens_input,0)) AS input,
  SUM(COALESCE(tokens_cache_read,0)) AS cache_read,
  SUM(COALESCE(tokens_cache_write,0)) AS cache_write,
  SUM(COALESCE(tokens_output,0) + COALESCE(tokens_reasoning,0)) AS output,
  GROUP_CONCAT(session.id) AS members
FROM tree JOIN session ON session.id = tree.id GROUP BY root`).all();
const rawCount = db.prepare('SELECT COUNT(*) AS n FROM session').get().n;
assert.equal(expected.reduce((n, r) => n + r.subagents + 1, 0), rawCount, 'all rows accounted for');
const sizes = new Map();
for (const [table, key] of [['message','session_id'], ['part','session_id'], ['event','aggregate_id']]) {
  for (const r of db.prepare(`SELECT ${key} AS id, SUM(LENGTH(CAST(data AS BLOB))) AS bytes FROM ${table} GROUP BY ${key}`).all()) {
    sizes.set(r.id, (sizes.get(r.id) || 0) + r.bytes);
  }
}
db.close();

const targets = await (await fetch('http://127.0.0.1:9223/json/list')).json();
const target = targets.find(t => t.type === 'page' && t.title.startsWith('Orrery') && t.url.startsWith('http://tauri.localhost'));
assert.ok(target, 'real Orrery window found');
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
let seq = 0;
const pending = new Map(), errors = [];
ws.onmessage = ({ data }) => {
  const r = JSON.parse(data);
  if (r.id) { const p = pending.get(r.id); pending.delete(r.id); r.error ? p.reject(r.error) : p.resolve(r.result); }
  if (r.method === 'Runtime.exceptionThrown') errors.push(r.params.exceptionDetails.text);
  if (r.method === 'Runtime.consoleAPICalled' && ['error','warning'].includes(r.params.type))
    errors.push(r.params.args.map(a => a.value || a.description || '').join(' '));
};
function cdp(method, params = {}) {
  return new Promise((resolve, reject) => { const id = ++seq; pending.set(id, { resolve, reject }); ws.send(JSON.stringify({ id, method, params })); });
}
async function evaluate(expression) {
  const r = await cdp('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  assert.ok(!r.exceptionDetails, JSON.stringify(r.exceptionDetails));
  return r.result.value;
}
await cdp('Runtime.enable');
const actualAll = await evaluate(`window.__TAURI__.core.invoke('list_sessions')`);
const actual = actualAll.filter(s => s.harness === 'opencode');
assert.equal(actual.length, expected.length);
for (const e of expected) {
  const a = actual.find(s => s.id === e.id);
  assert.ok(a, 'session present');
  for (const field of ['input','cache_read','cache_write','output']) assert.equal(a.usage[field], e[field], field);
  assert.equal(a.subagents, e.subagents);
  assert.equal(a.size_bytes, e.members.split(',').reduce((n, id) => n + (sizes.get(id) || 0), 0));
}
const storage = await evaluate(`window.__TAURI__.core.invoke('storage_stats')`);
const oc = storage.find(s => s.harness === 'opencode');
assert.equal(oc.sessions, actual.length);
assert.equal(oc.session_bytes, actual.reduce((n, s) => n + s.size_bytes, 0));
await evaluate(`[...document.querySelectorAll('#filters button')].find(b=>b.textContent==='OPENCODE').click()`);
assert.equal(await evaluate(`document.querySelectorAll('#session-list .strip').length`), actual.length);
await evaluate(`document.querySelector('#session-list .strip').click()`);
assert.equal(await evaluate(`document.querySelector('#detail-badge').textContent`), 'OPENCODE');
assert.equal(await evaluate(`document.querySelector('#detail-body').textContent.includes('0 次调用')`), false);
for (const [label, locale] of [['EN','en'], ['中','zh-CN'], ['日','ja']]) {
  await evaluate(`[...document.querySelectorAll('#lang-switch button')].find(b=>b.textContent.trim()===${JSON.stringify(label)}).click()`);
  assert.equal(await evaluate('document.documentElement.lang'), locale);
  await evaluate(`document.querySelector('[data-act="delete"]').click()`);
  for (let i = 0; i < 100; i++) {
    if (await evaluate(`!!document.querySelector('#modal-box [data-act="confirm"]')`)) break;
    await new Promise(r => setTimeout(r, 100));
  }
  assert.equal(await evaluate(`document.querySelector('#modal-box [data-act="confirm"]').disabled`), true);
  assert.ok(await evaluate(`document.querySelector('#modal-box').textContent.includes('OpenCode')`));
  await evaluate(`document.querySelector('#modal-box [data-act="cancel"]').click()`);
}
await evaluate(`[...document.querySelectorAll('#lang-switch button')].find(b=>b.textContent.trim()==='中').click()`);
await evaluate(`document.querySelector('#search').value='no-such-opencode-session';document.querySelector('#search').dispatchEvent(new Event('input',{bubbles:true}))`);
assert.equal(await evaluate(`document.querySelectorAll('#session-list .strip').length`), 0);
await evaluate(`document.querySelector('#search').value='';document.querySelector('#search').dispatchEvent(new Event('input',{bubbles:true}))`);
assert.equal(await evaluate(`document.querySelectorAll('#session-list .strip').length`), actual.length);
const shot = await cdp('Page.captureScreenshot', { format: 'png' });
await writeFile('.orrery/opencode-real.png', Buffer.from(shot.data, 'base64'));
await evaluate(`document.querySelector('[data-view="status"]').click()`);
assert.ok(await evaluate(`document.querySelector('#status-grid').textContent.includes('OpenCode')`));
assert.ok(await evaluate(`[...document.querySelectorAll('.storage-row')].every(row=>{
  const name=row.querySelector('.storage-name'),bar=row.querySelector('.storage-bar');
  return !name || !bar || name.getBoundingClientRect().right < bar.getBoundingClientRect().left;
})`), 'storage labels leave room before bars');
await writeFile('.orrery/opencode-status.png', Buffer.from((await cdp('Page.captureScreenshot', { format: 'png' })).data, 'base64'));
await evaluate(`document.querySelector('[data-view="models"]').click()`);
assert.ok(await evaluate(`document.querySelector('#routes').textContent.includes('OPENCODE')`));
await evaluate(`document.querySelector('[data-view="sessions"]').click()`);
ws.close();
assert.deepEqual(errors, [], 'no browser exceptions or i18n warnings');
const after = await fingerprint();
assert.deepEqual(after.db, before.db, 'real database bytes unchanged');
// SQLite may initialize an empty WAL even for a read-only connection. Never
// accept disappearance/change of a WAL that actually contained transactions.
if (before['-wal']?.size || after['-wal']?.size)
  assert.deepEqual(after['-wal'], before['-wal'], 'WAL transactions unchanged');
const emptyWalCreated = !before['-wal'] && after['-wal']?.size === 0;
const report = { checkedAt: new Date().toISOString(), rawCount, sessions: actual.length,
  subagents: actual.reduce((n,s)=>n+s.subagents,0), allHarnessSessions: actualAll.length,
  totals: Object.fromEntries(['input','cache_read','cache_write','output'].map(k=>[k,actual.reduce((n,s)=>n+s.usage[k],0)])),
  sessionBytes: oc.session_bytes, rootBytes: oc.root_bytes, perSessionReconciliation: 'passed',
  locales: ['en','zh-CN','ja'], readOnlyDialog: 'passed', databaseUnchanged: true,
  walTransactionsUnchanged: true, emptyWalCreated, errors };
await writeFile('.orrery/opencode-validation.json', JSON.stringify(report, null, 2));
console.log(JSON.stringify(report, null, 2));
