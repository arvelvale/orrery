// Verify the OpenCode fixtures from transfer::opencode::tests::sandbox_opencode_both_ways:
// each target tool must actually load the transferred history into a model request.
// All provider traffic goes to loopback fakes; no real session, account or API key is used.
//
//   ORRERY_TRANSFER_RESULT=<sandbox>/opencode-result.json
//   ORRERY_OPENCODE_BIN / ORRERY_CODEX_BIN / ORRERY_CLAUDE_BIN = the real executables
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

const resultFile = process.env.ORRERY_TRANSFER_RESULT;
const bins = { opencode: process.env.ORRERY_OPENCODE_BIN, codex: process.env.ORRERY_CODEX_BIN, claude: process.env.ORRERY_CLAUDE_BIN };
if (!resultFile || Object.values(bins).some(b => !b)) {
  throw new Error('Set ORRERY_TRANSFER_RESULT, ORRERY_OPENCODE_BIN, ORRERY_CODEX_BIN and ORRERY_CLAUDE_BIN');
}
const fixture = JSON.parse(await readFile(resultFile, 'utf8'));
const PNG_PREFIX = 'iVBORw0KGgo';

/** Loopback server that records request bodies and answers with `respond` */
async function fakeProvider(respond) {
  const bodies = [];
  const server = createServer((req, res) => {
    let raw = '';
    req.on('data', chunk => raw += chunk);
    req.on('end', () => { bodies.push({ url: req.url, raw }); respond(req, raw, res); });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  return { port: server.address().port, bodies, close: () => server.close() };
}

function run(bin, args, opts) {
  return new Promise((resolve) => {
    const child = spawn(bin, args, { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], ...opts });
    let stdout = '', stderr = '';
    child.stdout.on('data', c => stdout += c);
    child.stderr.on('data', c => stderr += c);
    const timer = setTimeout(() => child.kill(), 120000);
    child.on('exit', code => { clearTimeout(timer); resolve({ code, stdout, stderr }); });
  });
}

/** OpenCode, resumed without -m: it must use the model recorded on the messages.
 *  The fake model declares image input; otherwise OpenCode itself swaps images for a
 *  "model does not support image input" note before sending (measured). */
async function resumeOpenCode(sessionId, markers, { image = false } = {}) {
  const provider = await fakeProvider((req, raw, res) => {
    if (!req.url.includes('chat/completions')) { res.writeHead(404); res.end(); return; }
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    const chunk = (delta, finish = null) => `data: ${JSON.stringify({ id: 'c1', object: 'chat.completion.chunk', created: 1, model: 'probe', choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`;
    res.write(chunk({ role: 'assistant', content: 'sandbox reply' }));
    res.write(chunk({}, 'stop'));
    res.end('data: [DONE]\n\n');
  });
  const cfg = join(fixture.home, 'cfg', 'opencode');
  await mkdir(cfg, { recursive: true });
  await writeFile(join(cfg, 'opencode.json'), JSON.stringify({
    provider: { fake: { npm: '@ai-sdk/openai-compatible', name: 'Fake', options: { baseURL: `http://127.0.0.1:${provider.port}/v1`, apiKey: 'synthetic-test-key' }, models: { probe: { name: 'probe', attachment: true, modalities: { input: ['text', 'image'], output: ['text'] } } } } },
  }));
  const env = {
    ...process.env,
    XDG_DATA_HOME: join(fixture.home, '.local', 'share'),
    XDG_CONFIG_HOME: join(fixture.home, 'cfg'),
    XDG_STATE_HOME: join(fixture.home, 'state'),
    XDG_CACHE_HOME: join(fixture.home, 'cache'),
  };
  const out = await run(bins.opencode, ['run', '--session', sessionId, 'What marker was established?'], { cwd: fixture.project, env });
  provider.close();
  const sent = provider.bodies.filter(b => b.url.includes('chat/completions')).map(b => b.raw).join('\n');
  if (out.code !== 0 || !sent) throw new Error(`OpenCode resume failed: exit=${out.code} requests=${provider.bodies.length} stderr=${out.stderr.slice(0, 300)}`);
  for (const m of markers) if (!sent.includes(m)) throw new Error(`OpenCode request lacks ${m}`);
  if (image && !sent.includes(PNG_PREFIX)) throw new Error('OpenCode request lost the transferred image');
  console.log(`OpenCode run --session: ${markers.join(', ')}${image ? ' and the image' : ''} reached the provider`);
}

async function resumeClaude(sessionId, markers) {
  const provider = await fakeProvider((req, raw, res) => {
    if (req.url.includes('count_tokens')) { res.writeHead(200, { 'content-type': 'application/json' }); res.end('{"input_tokens":10}'); return; }
    if (!req.url.includes('messages')) { res.writeHead(404); res.end(); return; }
    const model = (() => { try { return JSON.parse(raw).model; } catch { return 'claude-sonnet-4-6'; } })();
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
  const args = ['--bare', '--print', 'What marker was established?', '--output-format', 'json', '--max-turns', '1', '--model', 'claude-sonnet-4-6', '--resume', sessionId];
  const out = await run(bins.claude, args, {
    cwd: fixture.project,
    env: { ...process.env, CLAUDE_CONFIG_DIR: join(fixture.home, '.claude'), ANTHROPIC_API_KEY: 'synthetic-test-key', ANTHROPIC_BASE_URL: `http://127.0.0.1:${provider.port}`, CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: '1', DISABLE_TELEMETRY: '1' },
  });
  provider.close();
  const sent = provider.bodies.filter(b => b.url.includes('messages') && !b.url.includes('count_tokens')).map(b => b.raw).join('\n');
  if (out.code !== 0) throw new Error(`Claude resume failed: exit=${out.code} stderr=${out.stderr.slice(0, 300)}`);
  for (const m of markers) if (!sent.includes(m)) throw new Error(`Claude request lacks ${m}`);
  if (JSON.parse(out.stdout).session_id !== sessionId) throw new Error('Claude resumed a different session');
  console.log(`Claude --resume: ${markers.join(', ')} reached the provider`);
}

async function readCodexThread(threadId, marker) {
  const child = spawn(bins.codex, ['app-server', '--stdio'], {
    cwd: fixture.project, windowsHide: true, stdio: ['pipe', 'pipe', 'ignore'],
    env: { ...process.env, CODEX_HOME: join(fixture.home, '.codex'), HOME: fixture.home, USERPROFILE: fixture.home },
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
    const read = await call(2, 'thread/read', { threadId, includeTurns: true });
    if (read.error) throw new Error('Codex thread/read rejected the imported session');
    if (!JSON.stringify(read.result).includes(marker)) throw new Error(`Codex thread lacks ${marker}`);
    console.log(`Codex thread/read: ${marker} visible`);
  } finally { child.kill(); }
}

await resumeOpenCode(fixture.opencode_from_claude, ['saffron-lake', 'amber-trail'], { image: true });
await resumeOpenCode(fixture.opencode_from_codex, ['violet-coral', 'jade-river']);
await resumeClaude(fixture.claude_from_opencode, ['violet-coral', 'jade-river']);
await readCodexThread(fixture.codex_from_opencode, 'violet-coral');
