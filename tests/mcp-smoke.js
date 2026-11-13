// Checks a built Snagster's MCP server without a screen (runs on Windows, Linux and macOS CI):
// stdio: initialize, tools/list, prompts, resources, and tools that need no display
// (languages, app info, open a picture, add marks, crop, save, undo/redo);
// HTTP: the same handshake over Streamable HTTP and a refused foreign host.
// Usage: node tests/mcp-smoke.js path/to/snagster
const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');
const http = require('http');

const exe = process.argv[2];
if (!exe) {
  console.error('usage: node tests/mcp-smoke.js path/to/snagster');
  process.exit(2);
}
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'snagster-smoke-'));
const picture = path.join(__dirname, '..', 'assets', 'snagster-256.png');
let failures = 0;
const check = (ok, what, detail) => {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}${!ok && detail ? ': ' + detail : ''}`);
  if (!ok) failures++;
};

function stdio() {
  return new Promise((resolve) => {
    const child = spawn(exe, ['--mcp'], { stdio: ['pipe', 'pipe', 'inherit'] });
    let buf = '';
    const waiting = new Map();
    child.stdout.on('data', (d) => {
      buf += d.toString();
      let i;
      while ((i = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, i).trim();
        buf = buf.slice(i + 1);
        if (!line) continue;
        const msg = JSON.parse(line);
        const w = waiting.get(msg.id);
        if (w) {
          waiting.delete(msg.id);
          w(msg);
        }
      }
    });
    let id = 0;
    const call = (method, params) =>
      new Promise((res, rej) => {
        const my = ++id;
        const timer = setTimeout(() => rej(new Error(`${method} timed out`)), 60000);
        waiting.set(my, (m) => {
          clearTimeout(timer);
          res(m);
        });
        child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: my, method, params }) + '\n');
      });
    const tool = async (name, args) => {
      const m = await call('tools/call', { name, arguments: args || {} });
      return m.result || { isError: true, content: [{ text: JSON.stringify(m.error) }] };
    };
    (async () => {
      try {
        const init = await call('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'smoke', version: '1' } });
        check(init.result && init.result.protocolVersion === '2025-06-18', 'initialize');
        child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
        const list = await call('tools/list', {});
        const tools = (list.result && list.result.tools) || [];
        check(tools.length >= 45, `tools/list (${tools.length} tools)`);
        check(tools.every((t) => t.annotations && typeof t.annotations.readOnlyHint === 'boolean' && t.inputSchema), 'every tool has a schema and hints');
        const prompts = await call('prompts/list', {});
        check(prompts.result && prompts.result.prompts.length >= 4, 'prompts/list');
        const res = await call('resources/list', {});
        check(res.result && res.result.resources.length >= 2, 'resources/list');
        const ping = await call('ping', {});
        check(ping.result !== undefined, 'ping');
        const langs = await tool('list_languages');
        check(!langs.isError && JSON.parse(langs.content[0].text).length === 86, 'list_languages has 86');
        const info = await tool('app_info');
        check(!info.isError && info.structuredContent && info.structuredContent.version, 'app_info');
        const open = await tool('open_image', { path: picture });
        check(!open.isError && open.content.some((c) => c.type === 'image'), 'open_image', open.content && open.content[0].text);
        const mark = await tool('add_mark', { kind: 'arrow', x: 10, y: 10, x2: 120, y2: 120, color: 'red' });
        check(!mark.isError, 'add_mark');
        const text = await tool('add_mark', { kind: 'text', x: 20, y: 200, text: 'Hello' });
        check(!text.isError, 'add_mark text');
        const crop = await tool('crop', { x: 0, y: 0, width: 200, height: 220 });
        check(!crop.isError, 'crop');
        const undo = await tool('undo');
        const redo = await tool('redo');
        check(!undo.isError && !redo.isError, 'undo and redo');
        const effect = await tool('apply_effect', { effect: 'DropShadow' });
        check(!effect.isError, 'apply_effect');
        const out = path.join(tmp, 'smoke.png');
        const save = await tool('save', { path: out });
        check(!save.isError && fs.existsSync(out) && fs.statSync(out).size > 1000, 'save PNG', save.content && save.content[0].text);
        const pdf = path.join(tmp, 'guide.pdf');
        const guide = await tool('make_guide', { title: 'Smoke', steps: [{ path: out, caption: 'One' }], path: pdf });
        check(!guide.isError && fs.existsSync(pdf), 'make_guide PDF', guide.content && guide.content[0].text);
        const bad = await tool('no_such_tool');
        check(bad.isError === true, 'unknown tool is an error, not a crash');
        const read = await call('resources/read', { uri: 'snagster://picture/current' });
        check(read.result && read.result.contents[0].blob, 'resources/read current picture');
      } catch (e) {
        check(false, 'stdio session', e.message);
      }
      child.stdin.end();
      child.on('exit', () => resolve());
      setTimeout(() => {
        child.kill();
        resolve();
      }, 15000);
    })();
  });
}

function post(port, body, headers) {
  return new Promise((resolve, reject) => {
    const data = JSON.stringify(body);
    const req = http.request(
      { host: '127.0.0.1', port, path: '/mcp', method: 'POST', headers: Object.assign({ 'Content-Type': 'application/json', Accept: 'application/json, text/event-stream', 'Content-Length': Buffer.byteLength(data) }, headers || {}) },
      (res) => {
        let s = '';
        res.on('data', (d) => (s += d));
        res.on('end', () => resolve({ status: res.statusCode, headers: res.headers, body: s }));
      }
    );
    req.on('error', reject);
    req.end(data);
  });
}

async function httpTest() {
  const port = 45000 + Math.floor(Math.random() * 1000);
  const child = spawn(exe, ['mcp-http', '--port', String(port)], { stdio: ['ignore', 'ignore', 'pipe'] });
  try {
    let up = false;
    for (let i = 0; i < 100 && !up; i++) {
      await new Promise((r) => setTimeout(r, 200));
      up = await post(port, { jsonrpc: '2.0', id: 0, method: 'ping' }).then(() => true, () => false);
    }
    check(up, 'HTTP server starts');
    const init = await post(port, { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'smoke', version: '1' } } });
    const sid = init.headers['mcp-session-id'];
    check(init.status === 200 && sid, 'HTTP initialize gives a session');
    const list = await post(port, { jsonrpc: '2.0', id: 2, method: 'tools/list' }, { 'Mcp-Session-Id': sid });
    check(list.status === 200 && JSON.parse(list.body).result.tools.length >= 45, 'HTTP tools/list');
    const note = await post(port, { jsonrpc: '2.0', method: 'notifications/initialized' }, { 'Mcp-Session-Id': sid });
    check(note.status === 202, 'HTTP notification is accepted');
    const evil = await post(port, { jsonrpc: '2.0', id: 3, method: 'ping' }, { Host: 'evil.example' });
    check(evil.status === 403, 'HTTP refuses a foreign host name');
  } catch (e) {
    check(false, 'HTTP session', e.message);
  }
  child.kill();
}

(async () => {
  await stdio();
  await httpTest();
  fs.rmSync(tmp, { recursive: true, force: true });
  console.log(failures ? `${failures} failed` : 'MCP smoke test: all passed');
  process.exit(failures ? 1 : 0);
})();
