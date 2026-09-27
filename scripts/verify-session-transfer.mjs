// Verify a synthetic transfer fixture from transfer::tests::sandbox_bidirectional.
// All provider traffic is served by a loopback fake; no real session or API key is used.
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';

const resultFile = process.env.ORRERY_TRANSFER_RESULT;
const codexBin = process.env.ORRERY_CODEX_BIN;
const claudeBin = process.env.ORRERY_CLAUDE_BIN;
if (!resultFile || !codexBin || !claudeBin) throw new Error('Set ORRERY_TRANSFER_RESULT, ORRERY_CODEX_BIN, and ORRERY_CLAUDE_BIN');
const fixture = JSON.parse(await readFile(resultFile, 'utf8'));
const marker = 'saffron-lake';

async function readCodexThread() {
  const child = spawn(codexBin, ['app-server', '--stdio'], {
    cwd: fixture.project, windowsHide: true,
    env: { ...process.env, CODEX_HOME: join(fixture.home, '.codex'), HOME: fixture.home, USERPROFILE: fixture.home },
    stdio: ['pipe', 'pipe', 'ignore'],
  });
  const pending = new Map();
  let buffer = '';
  child.stdout.on('data', chunk => {
    buffer += String(chunk);
    let end;
    while ((end = buffer.indexOf('\n')) >= 0) {
      const line = buffer.slice(0, end); buffer = buffer.slice(end + 1);
      let msg; try { msg = JSON.parse(line); } catch { continue; }
      if (pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); }
    }
  });
  const call = (id, method, params) => new Promise((resolve, reject) => {
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timed out`)); }, 30000);
    pending.set(id, msg => { clearTimeout(timer); resolve(msg); });
    child.stdin.write(JSON.stringify({ id, method, params }) + '\n');
  });
  try {
    const init = await call(1, 'initialize', { clientInfo: { name: 'orrery-transfer-verifier', version: '0.1' }, capabilities: { experimentalApi: true } });
    if (init.error) throw new Error('Codex initialize failed');
    child.stdin.write(JSON.stringify({ method: 'initialized' }) + '\n');
    const read = await call(2, 'thread/read', { threadId: fixture.codex_id, includeTurns: true });
    if (read.error) throw new Error('Codex thread/read rejected imported session');
    if (!JSON.stringify(read.result).includes(marker)) throw new Error('Codex native thread lacks source marker');
    console.log('Codex thread/read: imported history visible');
    const histories = await call(3, 'externalAgentConfig/import/readHistories');
    if (histories.error) throw new Error('Codex import histories unavailable');
    const imported = histories.result?.data?.flatMap(x => x.successes || []) || [];
    if (!imported.some(x => x.target === fixture.codex_id && x.source)) throw new Error('Codex import ledger lacks the target session');
    console.log('Codex import ledger: source and target recorded');
  } finally { child.kill(); }
}

async function resumeClaude(sessionId, expectedMarker) {
  let sawHistory = false;
  let sawImage = false;
  let requests = 0;
  const server = createServer((req, res) => {
    let raw = '';
    req.on('data', chunk => raw += chunk);
    req.on('end', () => {
      if (req.url?.includes('count_tokens')) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ input_tokens: 10 }));
        return;
      }
      if (!req.url?.includes('messages')) { res.writeHead(404); res.end(); return; }
      requests++;
      let body; try { body = JSON.parse(raw); } catch { body = {}; }
      const history = JSON.stringify(body.messages || []);
      sawHistory ||= history.includes(expectedMarker);
      sawImage ||= history.includes('"type":"image"');
      const model = body.model || 'claude-sonnet-4-6';
      if (!body.stream) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ id: 'msg_fake', type: 'message', role: 'assistant', model, content: [{ type: 'text', text: 'sandbox reply' }], stop_reason: 'end_turn', usage: { input_tokens: 10, output_tokens: 2 } }));
        return;
      }
      res.writeHead(200, { 'content-type': 'text/event-stream' });
      const event = (name, data) => res.write(`event: ${name}\ndata: ${JSON.stringify(data)}\n\n`);
      event('message_start', { type: 'message_start', message: { id: 'msg_fake', type: 'message', role: 'assistant', model, content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: 10, output_tokens: 0 } } });
      event('content_block_start', { type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } });
      event('content_block_delta', { type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: 'sandbox reply' } });
      event('content_block_stop', { type: 'content_block_stop', index: 0 });
      event('message_delta', { type: 'message_delta', delta: { stop_reason: 'end_turn', stop_sequence: null }, usage: { output_tokens: 2 } });
      event('message_stop', { type: 'message_stop' });
      res.end();
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = server.address().port;
  const claudeArgs = ['--bare', '--print', 'What marker was established?', '--output-format', 'json', '--max-turns', '1', '--model', 'claude-sonnet-4-6', '--resume', sessionId];
  const child = spawn(claudeBin.endsWith('.js') ? process.execPath : claudeBin, claudeBin.endsWith('.js') ? [claudeBin, ...claudeArgs] : claudeArgs, {
    cwd: fixture.project, windowsHide: true,
    env: { ...process.env, CLAUDE_CONFIG_DIR: join(fixture.home, '.claude'), ANTHROPIC_API_KEY: 'synthetic-test-key', ANTHROPIC_BASE_URL: `http://127.0.0.1:${port}`, CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: '1', DISABLE_TELEMETRY: '1' },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let stdout = '', stderr = '';
  child.stdout.on('data', chunk => stdout += chunk);
  child.stderr.on('data', chunk => stderr += chunk);
  const timeout = setTimeout(() => child.kill(), 30000);
  const exit = await new Promise(resolve => child.on('exit', resolve));
  clearTimeout(timeout);
  server.close();
  if (exit !== 0 || !sawHistory || requests !== 1) throw new Error(`Claude resume failed: exit=${exit} requests=${requests} history=${sawHistory} stderr=${stderr.slice(0, 200)}`);
  if (expectedMarker === 'violet-coral' && !sawImage) throw new Error('Claude resume lost the imported user image');
  if (JSON.parse(stdout).session_id !== sessionId) throw new Error('Claude resumed a different session');
  console.log(`Claude --resume: ${expectedMarker} reached the provider request`);
}

await readCodexThread();
await resumeClaude(fixture.claude_id, marker);
await resumeClaude(fixture.claude_tool_id, 'violet-coral');
