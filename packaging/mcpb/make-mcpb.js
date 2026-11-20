// Snagster as an MCP Bundle (.mcpb) for each system, and the server.json the MCP Registry
// (registry.modelcontextprotocol.io) publishes, with each bundle's address and SHA-256.
//
// An .mcpb is a zip with manifest.json at its root (https://github.com/modelcontextprotocol/mcpb).
// This script writes the zip itself, so the release needs nothing beyond Node.
//
//   node packaging/mcpb/make-mcpb.js bundle --exe <snagster binary> --os windows|linux|macos --out <file.mcpb>
//   node packaging/mcpb/make-mcpb.js server-json --dir <folder with the .mcpb files> --tag vX.Y.Z --out <server.json>
//   node packaging/mcpb/make-mcpb.js self-test
//
// bundle asks the binary itself for its tools (snagster --mcp, tools/list), so the manifest lists
// exactly what that build serves. server-json fills in the repository's server.json.
'use strict';
const fs = require('fs');
const os = require('os');
const path = require('path');
const zlib = require('zlib');
const crypto = require('crypto');
const { spawn } = require('child_process');

const ROOT = path.join(__dirname, '..', '..');
const REPO = 'https://github.com/SoloSaftis/Snagster';

// What each system's bundle is called and holds. The registry only takes an MCPB whose file
// name (or repository name) contains "mcp".
const SYSTEMS = {
  windows: { file: 'Snagster-mcp-windows-x64.mcpb', platform: 'win32', exe: 'Snagster.exe' },
  linux: { file: 'Snagster-mcp-linux-x86_64.mcpb', platform: 'linux', exe: 'snagster' },
  macos: { file: 'Snagster-mcp-macos-arm64.mcpb', platform: 'darwin', exe: 'snagster', note: ' This bundle is for Macs with Apple silicon (M1 or newer).' },
};

function cargoVersion() {
  const m = fs.readFileSync(path.join(ROOT, 'Cargo.toml'), 'utf8').match(/^version\s*=\s*"([^"]+)"/m);
  if (!m) throw new Error('no version in Cargo.toml');
  return m[1];
}

// ---- zip (deflate, Unix permissions kept so the program stays runnable) ----

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

/** entries: [{name, data: Buffer, mode}] -> a zip as one Buffer. */
function zip(entries) {
  const parts = [];
  const central = [];
  let offset = 0;
  // 1980-01-01 00:00 for every file, so the same input always gives the same zip.
  const time = 0;
  const date = (0 << 9) | (1 << 5) | 1;
  for (const e of entries) {
    const name = Buffer.from(e.name, 'utf8');
    const packed = zlib.deflateRawSync(e.data, { level: 9 });
    const crc = crc32(e.data);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4); // version needed
    local.writeUInt16LE(0x0800, 6); // UTF-8 names
    local.writeUInt16LE(8, 8); // deflate
    local.writeUInt16LE(time, 10);
    local.writeUInt16LE(date, 12);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(packed.length, 18);
    local.writeUInt32LE(e.data.length, 22);
    local.writeUInt16LE(name.length, 26);
    local.writeUInt16LE(0, 28);
    parts.push(local, name, packed);
    const c = Buffer.alloc(46);
    c.writeUInt32LE(0x02014b50, 0);
    c.writeUInt16LE((3 << 8) | 20, 4); // made by Unix, so the permissions below count
    c.writeUInt16LE(20, 6);
    c.writeUInt16LE(0x0800, 8);
    c.writeUInt16LE(8, 10);
    c.writeUInt16LE(time, 12);
    c.writeUInt16LE(date, 14);
    c.writeUInt32LE(crc, 16);
    c.writeUInt32LE(packed.length, 20);
    c.writeUInt32LE(e.data.length, 24);
    c.writeUInt16LE(name.length, 28);
    c.writeUInt32LE((((e.mode || 0o644) | 0o100000) << 16) >>> 0, 38); // a regular file
    c.writeUInt32LE(offset, 42);
    central.push(c, name);
    offset += local.length + name.length + packed.length;
  }
  const dir = Buffer.concat(central);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(dir.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...parts, dir, end]);
}

/** Read a zip back: {name: {data, mode}}; checks every file's CRC. */
function unzip(buf) {
  const endAt = buf.lastIndexOf(Buffer.from([0x50, 0x4b, 0x05, 0x06]));
  if (endAt < 0) throw new Error('not a zip');
  const count = buf.readUInt16LE(endAt + 10);
  let p = buf.readUInt32LE(endAt + 16);
  const out = {};
  for (let i = 0; i < count; i++) {
    if (buf.readUInt32LE(p) !== 0x02014b50) throw new Error('broken central directory');
    const method = buf.readUInt16LE(p + 10);
    const crc = buf.readUInt32LE(p + 16);
    const size = buf.readUInt32LE(p + 20);
    const nameLen = buf.readUInt16LE(p + 28);
    const extraLen = buf.readUInt16LE(p + 30);
    const commentLen = buf.readUInt16LE(p + 32);
    const mode = buf.readUInt32LE(p + 38) >>> 16;
    const at = buf.readUInt32LE(p + 42);
    const name = buf.toString('utf8', p + 46, p + 46 + nameLen);
    const dataAt = at + 30 + buf.readUInt16LE(at + 26) + buf.readUInt16LE(at + 28);
    const raw = buf.subarray(dataAt, dataAt + size);
    const data = method === 8 ? zlib.inflateRawSync(raw) : raw;
    if (crc32(data) !== crc) throw new Error(`${name}: CRC differs`);
    out[name] = { data, mode: mode & 0o777 };
    p += 46 + nameLen + extraLen + commentLen;
  }
  return out;
}

// ---- the bundle ----

/** Ask a Snagster binary for its tools (name and description), over stdio. */
function toolsOf(exe) {
  return new Promise((resolve, reject) => {
    const child = spawn(exe, ['--mcp'], { stdio: ['pipe', 'pipe', 'ignore'] });
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error(`${exe} --mcp did not answer tools/list within 60 s`));
    }, 60000);
    let buf = '';
    child.on('error', (e) => {
      clearTimeout(timer);
      reject(e);
    });
    child.stdout.on('data', (d) => {
      buf += d.toString();
      let i;
      while ((i = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, i).trim();
        buf = buf.slice(i + 1);
        if (!line) continue;
        const msg = JSON.parse(line);
        if (msg.id === 2) {
          clearTimeout(timer);
          child.stdin.end();
          child.kill();
          resolve(msg.result.tools.map((t) => ({ name: t.name, description: t.description })));
        }
      }
    });
    const send = (m) => child.stdin.write(JSON.stringify(m) + '\n');
    send({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'make-mcpb', version: '1' } } });
    send({ jsonrpc: '2.0', method: 'notifications/initialized' });
    send({ jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} });
  });
}

/** The MCPB manifest for one system. */
function manifest(system, version, tools) {
  const s = SYSTEMS[system];
  if (!s) throw new Error(`--os is windows, linux or macos, not ${system}`);
  const server = readServerJson();
  return {
    manifest_version: '0.3',
    name: 'snagster',
    display_name: 'Snagster',
    version,
    description: server.description,
    long_description:
      'Snagster captures the screen, a window, an area or a scrolling page; marks pictures up with arrows, boxes, text and steps; ' +
      'hides private details; reads text; records videos and GIFs; makes step-by-step guides; runs Studio scenes, a replay buffer ' +
      'and live streams. Free and open source (MIT or Apache-2.0); everything runs on this computer.' +
      (s.note || ''),
    author: { name: 'SoloSaftis', url: 'https://github.com/SoloSaftis' },
    repository: { type: 'git', url: REPO },
    homepage: REPO,
    documentation: `${REPO}/blob/main/skills/snagster/SKILL.md`,
    support: `${REPO}/issues`,
    icon: 'icon.png',
    license: 'MIT OR Apache-2.0',
    keywords: ['screenshot', 'screen capture', 'screen recording', 'annotation', 'ocr', 'redaction', 'streaming', 'gif'],
    server: {
      type: 'binary',
      entry_point: `server/${s.exe}`,
      mcp_config: { command: `\${__dirname}/server/${s.exe}`, args: ['--mcp'], env: {} },
    },
    tools,
    tools_generated: false,
    prompts_generated: true,
    compatibility: { platforms: [s.platform] },
  };
}

async function bundle(exe, system, out, tools) {
  const version = cargoVersion();
  const list = tools || (await toolsOf(exe));
  if (list.length < 40) throw new Error(`${exe} lists only ${list.length} tools`);
  const m = manifest(system, version, list);
  const entries = [
    { name: 'manifest.json', data: Buffer.from(JSON.stringify(m, null, 2) + '\n'), mode: 0o644 },
    { name: m.server.entry_point, data: fs.readFileSync(exe), mode: 0o755 },
    { name: 'icon.png', data: fs.readFileSync(path.join(ROOT, 'assets', 'snagster-256.png')), mode: 0o644 },
    { name: 'LICENSE-MIT', data: fs.readFileSync(path.join(ROOT, 'LICENSE-MIT')), mode: 0o644 },
    { name: 'LICENSE-APACHE', data: fs.readFileSync(path.join(ROOT, 'LICENSE-APACHE')), mode: 0o644 },
  ];
  fs.mkdirSync(path.dirname(path.resolve(out)), { recursive: true });
  fs.writeFileSync(out, zip(entries));
  console.log(`${out}: Snagster ${version} for ${system}, ${list.length} tools, sha256 ${sha256(fs.readFileSync(out))}`);
}

// ---- server.json ----

function readServerJson() {
  return JSON.parse(fs.readFileSync(path.join(ROOT, 'server.json'), 'utf8'));
}

function sha256(buf) {
  return crypto.createHash('sha256').update(buf).digest('hex');
}

/** Problems with a server.json, by the registry's rules (empty when it is fine). */
function problems(s) {
  const p = [];
  if (!/^[a-zA-Z0-9.-]+\/[a-zA-Z0-9._-]+$/.test(s.name || '') || s.name.length > 200) p.push('name');
  if (!s.description || s.description.length > 100) p.push('description is 1-100 characters');
  if (s.title && s.title.length > 100) p.push('title');
  if (!s.version || s.version.length > 255 || /[\^~<>=*x ]/.test(s.version)) p.push('version is one exact version');
  for (const k of (s.packages || []).keys()) {
    const pk = s.packages[k];
    if (pk.registryType !== 'mcpb') p.push(`packages[${k}] is not mcpb`);
    if (!/^https:\/\/github\.com\/[^/]+\/[^/]+\/releases\/download\/[^/]+\/[^/]*mcp[^/]*\.mcpb$/i.test(pk.identifier || '')) p.push(`packages[${k}].identifier is a GitHub release download whose file name has "mcp"`);
    if (!/^[a-f0-9]{64}$/.test(pk.fileSha256 || '')) p.push(`packages[${k}].fileSha256`);
    if (!pk.transport || pk.transport.type !== 'stdio') p.push(`packages[${k}].transport is stdio`);
  }
  return p;
}

/** server.json for a release: the repository's one with this tag's bundles and their SHA-256. */
function serverJsonFor(dir, tag) {
  const s = readServerJson();
  const version = tag.replace(/^v/, '');
  s.version = version;
  const files = new Set(Object.values(SYSTEMS).map((x) => x.file));
  s.packages = s.packages.map((pk) => {
    const file = pk.identifier.split('/').pop();
    if (!files.has(file)) throw new Error(`server.json names ${file}, which no system makes`);
    const at = path.join(dir, file);
    if (!fs.existsSync(at)) throw new Error(`${at} is missing`);
    return { ...pk, version, identifier: `${REPO}/releases/download/${tag}/${file}`, fileSha256: sha256(fs.readFileSync(at)) };
  });
  const p = problems(s);
  if (p.length) throw new Error('server.json: ' + p.join('; '));
  return s;
}

/** Write a release's server.json to `out`, making its folder when it isn't there yet. */
function writeServerJson(dir, tag, out) {
  const s = serverJsonFor(dir, tag);
  fs.mkdirSync(path.dirname(path.resolve(out)), { recursive: true });
  fs.writeFileSync(out, JSON.stringify(s, null, 2) + '\n');
}

// ---- self-test (no Snagster binary needed) ----

async function selfTest() {
  const assert = (ok, what) => {
    if (!ok) throw new Error('self-test: ' + what);
  };
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'mcpb-test-'));
  try {
    // The zip round-trips, keeps permissions, and is the same every time.
    const entries = [
      { name: 'a.txt', data: Buffer.from('hello '.repeat(1000)), mode: 0o644 },
      { name: 'server/run', data: crypto.randomBytes(5000), mode: 0o755 },
      { name: 'empty', data: Buffer.alloc(0), mode: 0o644 },
    ];
    const z = zip(entries);
    assert(z.equals(zip(entries)), 'the same files give the same zip');
    const back = unzip(z);
    for (const e of entries) assert(back[e.name] && back[e.name].data.equals(e.data) && back[e.name].mode === e.mode, `${e.name} round-trips`);
    let refused = false;
    try {
      const bad = Buffer.from(z);
      bad[40] ^= 0xff; // inside a.txt's packed data
      unzip(bad);
    } catch {
      refused = true;
    }
    assert(refused, 'a damaged zip is refused');

    // A bundle per system, with the program runnable and the manifest complete.
    const fake = path.join(tmp, 'snagster');
    fs.writeFileSync(fake, 'not really a program');
    const tools = Array.from({ length: 50 }, (_, i) => ({ name: `tool_${i}`, description: `Tool ${i}.` }));
    for (const [system, s] of Object.entries(SYSTEMS)) {
      const out = path.join(tmp, s.file);
      await bundle(fake, system, out, tools);
      const files = unzip(fs.readFileSync(out));
      const m = JSON.parse(files['manifest.json'].data.toString());
      assert(m.version === cargoVersion() && m.name === 'snagster' && m.server.type === 'binary', `${system} manifest basics`);
      for (const k of ['manifest_version', 'name', 'version', 'description', 'author', 'server']) assert(m[k], `${system} manifest has ${k}`);
      assert(files[m.server.entry_point] && files[m.server.entry_point].mode === 0o755, `${system} program is runnable`);
      assert(m.server.mcp_config.command === '${__dirname}/' + m.server.entry_point && m.server.mcp_config.args.join() === '--mcp', `${system} starts as snagster --mcp`);
      assert(m.compatibility.platforms.join() === s.platform, `${system} says its platform`);
      assert(files['icon.png'] && files['LICENSE-MIT'] && files['LICENSE-APACHE'], `${system} has the icon and licenses`);
      assert(m.tools.length === 50, `${system} lists the tools`);
      assert(s.file.toLowerCase().includes('mcp'), `${system} file name has mcp, as the registry wants`);
      assert(system !== 'macos' || m.long_description.includes('Apple silicon'), 'the Mac bundle says it needs Apple silicon');
    }
    let wrongOs = false;
    try {
      await bundle(fake, 'beos', path.join(tmp, 'x.mcpb'), tools);
    } catch {
      wrongOs = true;
    }
    assert(wrongOs, 'an unknown system is refused');

    // server.json for a release: every package points at its bundle with the right hash.
    const s = serverJsonFor(tmp, 'v9.8.7');
    assert(s.version === '9.8.7', 'server.json version');
    for (const pk of s.packages) {
      const file = pk.identifier.split('/').pop();
      assert(pk.identifier === `${REPO}/releases/download/v9.8.7/${file}`, 'identifier is the release download');
      assert(pk.fileSha256 === sha256(fs.readFileSync(path.join(tmp, file))), 'fileSha256 is the bundle\'s');
    }
    assert(s.packages.length === Object.keys(SYSTEMS).length, 'one package per system');
    // As the release job does it: into a folder that isn't there yet.
    const written = path.join(tmp, 'server-json', 'server.json');
    writeServerJson(tmp, 'v9.8.7', written);
    assert(JSON.stringify(JSON.parse(fs.readFileSync(written, 'utf8'))) === JSON.stringify(s), 'server.json is written into a new folder');
    fs.unlinkSync(path.join(tmp, SYSTEMS.macos.file));
    let missing = false;
    try {
      serverJsonFor(tmp, 'v9.8.7');
    } catch (e) {
      missing = /missing/.test(e.message);
    }
    assert(missing, 'a missing bundle is an error');
    assert(problems({ ...s, description: 'x'.repeat(101) }).length === 1, 'a long description is caught');
    assert(problems({ ...s, packages: [{ ...s.packages[0], identifier: 'https://example.com/a.mcpb' }] }).length === 1, 'a foreign address is caught');

    // The repository's server.json itself.
    const repo = readServerJson();
    assert(repo.version === cargoVersion(), `server.json version ${repo.version} is Cargo.toml's ${cargoVersion()}`);
    assert(problems(repo).length === 0, 'server.json in the repository: ' + problems(repo).join('; '));
    console.log('make-mcpb self-test: all passed');
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
}

// ---- command line ----

function flag(args, name) {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
}

async function main() {
  const [cmd, ...args] = process.argv.slice(2);
  if (cmd === 'bundle') {
    const exe = flag(args, '--exe');
    const system = flag(args, '--os');
    const out = flag(args, '--out') || (SYSTEMS[system] && path.join('dist', SYSTEMS[system].file));
    if (!exe || !system || !out) throw new Error('bundle needs --exe <binary> --os windows|linux|macos [--out file.mcpb]');
    await bundle(exe, system, out);
  } else if (cmd === 'server-json') {
    const dir = flag(args, '--dir');
    const tag = flag(args, '--tag');
    const out = flag(args, '--out') || 'server.json';
    if (!dir || !/^v\d+\.\d+\.\d+/.test(tag || '')) throw new Error('server-json needs --dir <folder> --tag vX.Y.Z [--out server.json]');
    writeServerJson(dir, tag, out);
    console.log(fs.readFileSync(out, 'utf8'));
  } else if (cmd === 'self-test') {
    await selfTest();
  } else {
    throw new Error('usage: make-mcpb.js bundle | server-json | self-test (see the top of the file)');
  }
}

if (require.main === module) {
  main().catch((e) => {
    console.error(e.message);
    process.exit(1);
  });
}

module.exports = { zip, unzip, manifest, serverJsonFor, problems, SYSTEMS };
