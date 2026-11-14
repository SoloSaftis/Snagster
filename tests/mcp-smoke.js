// Checks a built Snagster's MCP server without a screen (runs on Windows, Linux and macOS CI):
// stdio: initialize, tools/list (new tools, output schemas, hints), prompts, resources and
// templates, completions, and tools that need no display (languages, app info, open a picture,
// add/update marks, crop, save with a resource link, undo/redo, a guide with progress
// notifications, a cancelled capture delay, live-stream tools that refuse bad input without going live);
// HTTP: the same handshake over Streamable HTTP, progress on an event stream, and a refused
// foreign host. It changes no settings and nothing in the library.
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
    const progress = [];
    child.stdout.on('data', (d) => {
      buf += d.toString();
      let i;
      while ((i = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, i).trim();
        buf = buf.slice(i + 1);
        if (!line) continue;
        const msg = JSON.parse(line);
        if (msg.method === 'notifications/progress') {
          progress.push(msg);
          continue;
        }
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
    const tool = async (name, args, meta) => {
      const params = { name, arguments: args || {} };
      if (meta) params._meta = meta;
      const m = await call('tools/call', params);
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
        const names = new Set(tools.map((t) => t.name));
        const added = ['update_mark', 'run_preset', 'simplify', 'quick_save', 'capture_last_area', 'studio_add_scene', 'studio_remove_scene', 'studio_set_current', 'studio_update_source', 'library_delete', 'library_tag'];
        const missing = added.filter((n) => !names.has(n));
        check(missing.length === 0, 'tools/list has the newer tools', missing.join(', '));
        const byName = (n) => tools.find((t) => t.name === n) || {};
        check(byName('list_languages').outputSchema && byName('list_languages').outputSchema.type === 'object', 'list tools have an output schema');
        check(byName('save').annotations.destructiveHint === true && byName('set_setting').annotations.destructiveHint === true, 'save and set_setting are marked destructive');
        const rv = byName('record_video').inputSchema.properties;
        check(rv.audio && rv.microphone && rv.format.enum[0] === 'mp4', 'record_video takes audio and microphone');
        check(byName('read_text').description.includes('Windows only'), 'text recognition says Windows only');
        const prompts = await call('prompts/list', {});
        check(prompts.result && prompts.result.prompts.length >= 5, 'prompts/list');
        const res = await call('resources/list', {});
        check(res.result && res.result.resources.length >= 2, 'resources/list');
        const templates = await call('resources/templates/list', {});
        check(templates.result && templates.result.resourceTemplates.length === 3, 'resources/templates/list');
        const fps = await call('resources/read', { uri: 'snagster://settings/video_fps' });
        check(fps.result && /^\d+$/.test(fps.result.contents[0].text), 'resources/read a setting by template');
        const codes = await call('completion/complete', { ref: { type: 'ref/prompt', name: 'change_language' }, argument: { name: 'code', value: 'pt' } });
        check(codes.result && codes.result.completion.values.includes('pt-BR'), 'completion of language codes');
        const ping = await call('ping', {});
        check(ping.result !== undefined, 'ping');
        const langs = await tool('list_languages');
        check(!langs.isError && JSON.parse(langs.content[0].text).length === 86, 'list_languages has 86');
        check(langs.structuredContent && langs.structuredContent.items.length === 86, 'list_languages structured as {items}');
        const info = await tool('app_info');
        check(!info.isError && info.structuredContent && info.structuredContent.version && info.structuredContent.features, 'app_info');
        const open = await tool('open_image', { path: picture });
        check(!open.isError && open.content.some((c) => c.type === 'image'), 'open_image', open.content && open.content[0].text);
        const mark = await tool('add_mark', { kind: 'arrow', x: 10, y: 10, x2: 120, y2: 120, color: 'red', heads: 2 });
        check(!mark.isError, 'add_mark');
        const text = await tool('add_mark', { kind: 'text', x: 20, y: 200, text: 'Hello', bold: true, align: 'center' });
        check(!text.isError, 'add_mark text');
        const pen = await tool('add_mark', { kind: 'pen', points: [[5, 5], [40, 60], [80, 20]] });
        check(!pen.isError, 'add_mark pen', pen.content && pen.content[0].text);
        const upd = await tool('update_mark', { index: 1, dx: 10, text: 'Hello again', color: 'blue' });
        check(!upd.isError && upd.content[0].text.startsWith('Mark 1'), 'update_mark', upd.content && upd.content[0].text);
        const badUpd = await tool('update_mark', { index: 0, text: 'x' });
        check(badUpd.isError === true, 'update_mark refuses text on an arrow');
        const marks = await tool('list_marks');
        const items = marks.structuredContent && marks.structuredContent.items;
        check(items && items.length === 3 && items[1].kind === 'text' && items[2].kind === 'pen', 'list_marks structured');
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
        const saveLink = save.content && save.content.find((c) => c.type === 'resource_link');
        check(saveLink && saveLink.uri.startsWith('file:///') && saveLink.mimeType === 'image/png', 'save gives a resource link');
        const pdf = path.join(tmp, 'guide.pdf');
        const before = progress.length;
        const guide = await tool('make_guide', { title: 'Smoke', steps: [{ path: out, caption: 'One' }, { path: out }], path: pdf }, { progressToken: 'smoke-guide' });
        check(!guide.isError && fs.existsSync(pdf), 'make_guide PDF', guide.content && guide.content[0].text);
        const notes = progress.slice(before).filter((p) => p.params.progressToken === 'smoke-guide');
        check(notes.length === 3 && notes.every((n, i) => i === 0 || n.params.progress > notes[i - 1].params.progress) && notes[2].params.total === 3, 'make_guide sends progress notifications', JSON.stringify(notes));
        // A capture waiting out its delay stops when its own request is cancelled.
        const waitId = id + 1000;
        const waited = new Promise((res) => waiting.set(waitId, res));
        child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: waitId, method: 'tools/call', params: { name: 'capture_screen', arguments: { delay_s: 30, library: false } } }) + '\n');
        await new Promise((r) => setTimeout(r, 300));
        child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/cancelled', params: { requestId: waitId + 1 } }) + '\n');
        child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/cancelled', params: { requestId: waitId } }) + '\n');
        const t0 = Date.now();
        const cancelled = await Promise.race([waited, new Promise((r) => setTimeout(() => r(null), 20000))]);
        check(cancelled && cancelled.result && cancelled.result.isError && cancelled.result.content[0].text === 'cancelled' && Date.now() - t0 < 15000, 'a cancelled capture delay stops', JSON.stringify(cancelled));
        // Live streaming: schemas and answers only; nothing goes live.
        const ss = byName('stream_start');
        check(ss.inputSchema && ss.inputSchema.properties.source.enum.join() === 'screen,window,area,studio' && ss.inputSchema.properties.key && ss.annotations.openWorldHint === true && ss.annotations.readOnlyHint === false, 'stream_start schema and hints');
        check(byName('stream_status').annotations.readOnlyHint === true && byName('stream_stop').annotations.idempotentHint === true, 'stream_status / stream_stop hints');
        const services = await tool('stream_services');
        const svc = services.structuredContent && services.structuredContent.items;
        check(!services.isError && svc && svc.length === 6 && svc.some((s) => s.service === 'youtube' && s.url.startsWith('rtmps://')), 'stream_services lists the presets');
        const idle = await tool('stream_status');
        check(!idle.isError && idle.structuredContent && idle.structuredContent.phase === 'idle' && idle.structuredContent.live === false, 'stream_status says nothing is live');
        const none = await tool('stream_stop');
        check(!none.isError && none.content[0].text === 'Nothing is being streamed.', 'stream_stop with nothing live');
        const badStream = await tool('stream_start', { service: 'custom', url: 'ftp://example.invalid/live', key: 'sk_smoke_secret_42' });
        check(badStream.isError === true && !JSON.stringify(badStream).includes('sk_smoke_secret_42'), 'stream_start refuses a bad address without echoing the key', JSON.stringify(badStream));
        const badSource = await tool('stream_start', { source: 'window', key: 'sk_smoke_secret_42' });
        check(badSource.isError === true && badSource.content[0].text.includes('title'), 'stream_start needs a window title');
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
    const html = path.join(tmp, 'http-guide.html');
    const guide = await post(port, { jsonrpc: '2.0', id: 4, method: 'tools/call', params: { name: 'make_guide', arguments: { path: html, steps: [{ path: picture }, { path: picture }] }, _meta: { progressToken: 'http-tok' } } }, { 'Mcp-Session-Id': sid });
    const events = guide.body.split('\n').filter((l) => l.startsWith('data: ')).map((l) => JSON.parse(l.slice(6)));
    const last = events[events.length - 1] || {};
    check(
      guide.status === 200 && String(guide.headers['content-type']).startsWith('text/event-stream') && events.length >= 3 && last.id === 4 && !last.result.isError && events.slice(0, -1).every((e) => e.method === 'notifications/progress' && e.params.progressToken === 'http-tok'),
      'HTTP progress arrives on an event stream before the answer',
      guide.body.slice(0, 300)
    );
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
