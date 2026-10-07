// Images saved as files during a transfer must be openable by the target tool.
// A loopback fake model asks the tool to read the saved image path that the
// transferred history mentions; the test passes when the tool's next request
// carries the image back. No real session, account or API key is involved.
//
//   ORRERY_TRANSFER_RESULT=<sandbox>/opencode-result.json  (from sandbox_opencode_both_ways)
//   ORRERY_OPENCODE_BIN / ORRERY_CLAUDE_BIN = the real executables
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

const fixture = JSON.parse(await readFile(process.env.ORRERY_TRANSFER_RESULT, 'utf8'));
const bins = { opencode: process.env.ORRERY_OPENCODE_BIN, claude: process.env.ORRERY_CLAUDE_BIN };
if (Object.values(bins).some(b => !b)) throw new Error('Set ORRERY_OPENCODE_BIN and ORRERY_CLAUDE_BIN');
const PNG_PREFIX = 'iVBORw0KGgo';

/** The first saved-image path mentioned in a request body */
function savedPath(raw) {
  const m = raw.match(/Image saved to (.+?\.png) - open it/);
  // the body is JSON, so backslashes arrive doubled
  return m ? m[1].replace(/\\\\/g, '\\') : null;
}

async function fakeProvider(respond) {
  const requests = [];
  const server = createServer((req, res) => {
    let raw = '';
    req.on('data', c => raw += c);
    req.on('end', () => { requests.push({ url: req.url, raw }); respond(req, raw, res, requests); });
  });
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  return { port: server.address().port, requests, close: () => server.close() };
}

function run(bin, args, opts) {
  return new Promise(resolve => {
    const child = spawn(bin, args, { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], ...opts });
    let stdout = '', stderr = '';
    child.stdout.on('data', c => stdout += c);
    child.stderr.on('data', c => stderr += c);
    const timer = setTimeout(() => child.kill(), 120000);
    child.on('exit', code => { clearTimeout(timer); resolve({ code, stdout, stderr }); });
  });
}

async function claudeOpensImage(sessionId) {
  let asked = null;
  const provider = await fakeProvider((req, raw, res, requests) => {
    if (req.url.includes('count_tokens')) { res.writeHead(200, { 'content-type': 'application/json' }); res.end('{"input_tokens":10}'); return; }
    if (!req.url.includes('messages')) { res.writeHead(404); res.end(); return; }
    const turn = requests.filter(r => r.url.includes('messages') && !r.url.includes('count_tokens')).length;
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    const event = (name, data) => res.write(`event: ${name}\ndata: ${JSON.stringify(data)}\n\n`);
    event('message_start', { type: 'message_start', message: { id: `msg_${turn}`, type: 'message', role: 'assistant', model: 'claude-sonnet-4-6', content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: 10, output_tokens: 0 } } });
    asked ??= turn === 1 ? savedPath(raw) : null;
    if (turn === 1 && asked) {
      event('content_block_start', { type: 'content_block_start', index: 0, content_block: { type: 'tool_use', id: 'toolu_open', name: 'Read', input: {} } });
      event('content_block_delta', { type: 'content_block_delta', index: 0, delta: { type: 'input_json_delta', partial_json: JSON.stringify({ file_path: asked }) } });
      event('content_block_stop', { type: 'content_block_stop', index: 0 });
      event('message_delta', { type: 'message_delta', delta: { stop_reason: 'tool_use', stop_sequence: null }, usage: { output_tokens: 5 } });
    } else {
      event('content_block_start', { type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } });
      event('content_block_delta', { type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: 'seen' } });
      event('content_block_stop', { type: 'content_block_stop', index: 0 });
      event('message_delta', { type: 'message_delta', delta: { stop_reason: 'end_turn', stop_sequence: null }, usage: { output_tokens: 1 } });
    }
    event('message_stop', { type: 'message_stop' });
    res.end();
  });
  const args = ['--bare', '--print', 'Look at the saved screenshot.', '--output-format', 'json', '--max-turns', '3', '--model', 'claude-sonnet-4-6', '--allowedTools', 'Read', '--resume', sessionId];
  const out = await run(bins.claude, args, {
    cwd: fixture.project,
    env: { ...process.env, CLAUDE_CONFIG_DIR: join(fixture.home, '.claude'), ANTHROPIC_API_KEY: 'synthetic-test-key', ANTHROPIC_BASE_URL: `http://127.0.0.1:${provider.port}`, CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: '1', DISABLE_TELEMETRY: '1' },
  });
  provider.close();
  const calls = provider.requests.filter(r => r.url.includes('messages') && !r.url.includes('count_tokens'));
  if (!asked) throw new Error('Claude history carried no saved-image reference');
  const followUp = calls[1]?.raw || '';
  if (!followUp.includes(PNG_PREFIX)) {
    throw new Error(`Claude did not return the image after Read: exit=${out.code} calls=${calls.length} follow-up=${followUp.slice(-400)}`);
  }
  console.log(`Claude Code: opened ${asked} and sent the image back to the model`);
}

async function opencodeOpensImage(sessionId) {
  let asked = null;
  let toolSchema = null;
  const provider = await fakeProvider((req, raw, res, requests) => {
    if (!req.url.includes('chat/completions')) { res.writeHead(404); res.end(); return; }
    const turn = requests.filter(r => r.url.includes('chat/completions')).length;
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    const chunk = (delta, finish = null) => `data: ${JSON.stringify({ id: `c${turn}`, object: 'chat.completion.chunk', created: 1, model: 'probe', choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`;
    if (turn === 1) {
      asked = savedPath(raw);
      const body = JSON.parse(raw);
      toolSchema = (body.tools || []).find(t => t.function?.name === 'read')?.function?.parameters;
    }
    // the parameter name comes from the tool schema OpenCode itself declared
    const key = toolSchema?.properties?.path ? 'path' : 'filePath';
    if (turn === 1 && asked && toolSchema) {
      res.write(chunk({ role: 'assistant', tool_calls: [{ index: 0, id: 'call_open', type: 'function', function: { name: 'read', arguments: JSON.stringify({ [key]: asked }) } }] }));
      res.write(chunk({}, 'tool_calls'));
    } else {
      res.write(chunk({ role: 'assistant', content: 'seen' }));
      res.write(chunk({}, 'stop'));
    }
    res.end('data: [DONE]\n\n');
  });
  const cfg = join(fixture.home, 'cfg', 'opencode');
  await mkdir(cfg, { recursive: true });
  await writeFile(join(cfg, 'opencode.json'), JSON.stringify({
    provider: { fake: { npm: '@ai-sdk/openai-compatible', name: 'Fake', options: { baseURL: `http://127.0.0.1:${provider.port}/v1`, apiKey: 'synthetic-test-key' }, models: { probe: { name: 'probe', tool_call: true, attachment: true, modalities: { input: ['text', 'image'], output: ['text'] } } } } },
  }));
  const env = { ...process.env, XDG_DATA_HOME: join(fixture.home, '.local', 'share'), XDG_CONFIG_HOME: join(fixture.home, 'cfg'), XDG_STATE_HOME: join(fixture.home, 'state'), XDG_CACHE_HOME: join(fixture.home, 'cache') };
  const out = await run(bins.opencode, ['run', '--session', sessionId, 'Look at the saved screenshot.'], { cwd: fixture.project, env });
  provider.close();
  const calls = provider.requests.filter(r => r.url.includes('chat/completions'));
  if (!asked) throw new Error('OpenCode history carried no saved-image reference');
  if (!toolSchema) throw new Error('OpenCode declared no read tool');
  const followUp = calls[1]?.raw || '';
  if (!followUp.includes(PNG_PREFIX)) {
    throw new Error(`OpenCode did not return the image after read: exit=${out.code} calls=${calls.length} stderr=${out.stderr.slice(0, 300)} follow-up=${followUp.slice(-500)}`);
  }
  console.log(`OpenCode: opened ${asked} and sent the image back to the model`);
}

await claudeOpensImage(fixture.claude_from_opencode);
await opencodeOpensImage(fixture.opencode_from_claude);
