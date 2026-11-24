// Tests for the Lingo patch editor's server. Run with: node --test tools/lingo/editor/
//
// They use a fake `rotl-lingo` (a small node script written into a temp directory) so that no movie is
// needed, and they call the request handler directly, so no port is opened.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { Readable } from 'node:stream';
import { after, before, test } from 'node:test';

import { checkConfig, createApp, lastJson, patchFileFor, readConfig, resolvePatchPath } from './server.mjs';

const PORT = 19575;
const ORIGIN = `http://127.0.0.1:${PORT}`;
const RIG = 'http://127.0.0.1:19572';

// The fake tool: canned JSON for each command. It objects to the word BADNAME in a static check and to
// CONNSTRING in a check against the "movie", and logs every call's arguments.
const FAKE_TOOL = `#!/usr/bin/env node
const fs = require('fs');
const args = process.argv.slice(2);
fs.appendFileSync(process.env.FAKE_TOOL_LOG, JSON.stringify(args) + '\\n');
// Notices when two copies run at once: the server must never let that happen.
const lock = process.env.FAKE_TOOL_LOG + '.lock';
try { fs.writeFileSync(lock, '', { flag: 'wx' }); } catch { fs.writeFileSync(process.env.FAKE_TOOL_LOG + '.overlap', 'two at once'); }
process.on('exit', () => { try { fs.unlinkSync(lock); } catch {} });
Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 15);
const command = args[0];
const has = (flag) => args.includes(flag);
const positional = [];
for (let i = 1; i < args.length; i++) {
  if (['--movie', '-o', '--why', '--patch', '--eval', '--global'].includes(args[i])) { i++; continue; }
  if (!args[i].startsWith('--')) positional.push(args[i]);
}
const say = (value) => console.log(JSON.stringify(value));
const report = (file, text) => {
  const problems = [];
  if (/BADNAME/.test(text)) problems.push('the patch uses \`badname\`, which belongs to the connection code; patches must stay away from it');
  if (!has('--static') && /CONNSTRING/.test(text)) problems.push('the file contains a string that the movie\\'s connection code uses; it must not be copied into a patch');
  if (!has('--static') && /SYNTAXERR/.test(text)) problems.push('line 3: expected end');
  return { file, ok: problems.length === 0, problems, warnings: [], id: 'Scripts/27/DrawScene', enabled: false };
};
if (command === 'list') {
  say({ scripts: [{ cast: 'Scripts', member: 27, name: 'Game Logic', kind: 'movie', ref: 'Scripts/27', handlers: [{ name: 'DrawScene', args: ['TheInf'], bytes: 10, sensitive: false }, { name: 'Secret', args: [], bytes: 5, sensitive: true }] }], scriptCount: 1, handlerCount: 2 });
} else if (command === 'show') {
  say({ ref: positional[0], source: 'on DrawScene\\nend\\n', sensitive: false, editable: true, bytecode: [] });
} else if (command === 'search') {
  say({ query: positional[0], total: 0, shown: 0, hits: [] });
} else if (command === 'check') {
  const target = positional[0];
  const files = fs.statSync(target).isDirectory() ? fs.readdirSync(target, { recursive: true }).filter((f) => f.endsWith('.ls')).map((f) => target + '/' + f) : [target];
  const reports = files.map((f) => report(f, fs.readFileSync(f, 'utf8')));
  const failed = reports.filter((r) => !r.ok).length;
  say({ ok: failed === 0, files: reports.length, failed, reports });
  process.exit(failed ? 1 : 0);
} else if (command === 'diff') {
  const r = report(positional[0], fs.readFileSync(positional[0], 'utf8'));
  console.log('[WARN] a line of noise before the JSON');
  say({ ...r, sourceDiff: ['+ added'], bytecodeDiff: [] });
  process.exit(r.ok ? 0 : 1);
} else if (command === 'manifest') {
  console.error('1 patch(es) in the manifest, 0 not enabled');
  say({ version: 1, patches: [{ id: 'Scripts/27/DrawScene', source: fs.readFileSync(positional[0], 'utf8') }] });
} else if (command === 'new') {
  const out = args[args.indexOf('-o') + 1];
  if (fs.existsSync(out)) { console.error('rotl-lingo: already exists'); process.exit(1); }
  fs.writeFileSync(out, '-- lingo-patch 1\\n-- why: ' + args[args.indexOf('--why') + 1] + '\\non DrawScene\\nend\\n');
  say({ ok: true, file: out, ref: positional[0] });
} else if (command === 'run') {
  console.log('[test] noise');
  say({ ok: true, patches: { results: [] }, results: [{ eval: 'put 1', ok: true, value: 'Void' }] });
} else {
  process.exit(2);
}
`;

let work; // everything the tests create lives here
let patches;
let outside;
let toolLog;
let app;

before(() => {
  work = fs.mkdtempSync(path.join(os.tmpdir(), 'rotl-lingo-editor-test-'));
  patches = path.join(work, 'patches');
  outside = path.join(work, 'outside');
  fs.mkdirSync(patches);
  fs.mkdirSync(outside);
  fs.writeFileSync(path.join(work, 'movie.dcr'), 'not a real movie');
  fs.writeFileSync(path.join(work, 'fake-rotl-lingo'), FAKE_TOOL, { mode: 0o755 });
  fs.writeFileSync(path.join(outside, 'victim.ls'), 'untouched');
  fs.writeFileSync(path.join(outside, 'victim.txt'), 'untouched');
  toolLog = path.join(work, 'tool.log');
  fs.writeFileSync(toolLog, '');
  process.env.FAKE_TOOL_LOG = toolLog;
  app = createApp({ tool: path.join(work, 'fake-rotl-lingo'), movie: path.join(work, 'movie.dcr'), patches, port: PORT, rig: RIG, toolConfig: path.join(work, 'movie.dcr') });
});

after(() => {
  app.close();
  fs.rmSync(work, { recursive: true, force: true });
});

/** Calls the request handler as the HTTP server would, without a socket. */
function call({ method = 'GET', url, headers = {}, body, host = `127.0.0.1:${PORT}` }) {
  const chunks = body === undefined ? [] : [Buffer.isBuffer(body) ? body : Buffer.from(typeof body === 'string' ? body : JSON.stringify(body))];
  const req = Readable.from(chunks);
  req.method = method;
  req.url = url;
  req.headers = {};
  if (host !== null) req.headers.host = host;
  for (const [name, value] of Object.entries(headers)) req.headers[name.toLowerCase()] = value;
  return new Promise((resolve, reject) => {
    const res = {
      status: 0,
      headers: {},
      headersSent: false,
      writeHead(status, responseHeaders = {}) {
        this.status = status;
        this.headersSent = true;
        for (const [name, value] of Object.entries(responseHeaders)) this.headers[name.toLowerCase()] = value;
      },
      end(data) {
        const text = data ? Buffer.from(data).toString('utf8') : '';
        let json;
        try {
          json = JSON.parse(text);
        } catch {
          json = undefined;
        }
        resolve({ status: this.status, headers: this.headers, text, json });
      },
    };
    app.handler(req, res).catch(reject);
  });
}

function post(url, body, headers = {}) {
  return call({ method: 'POST', url, body, headers: { Origin: ORIGIN, 'Content-Type': 'application/json', ...headers } });
}

function toolCalls() {
  return fs.readFileSync(toolLog, 'utf8').trim().split('\n').filter(Boolean).map((line) => JSON.parse(line));
}

const GOOD_TEXT = '-- lingo-patch 1\n-- why: test\non DrawScene\nend\n';

// ---------------------------------------------------------------------------------------------------

test('start-up refuses a missing tool, movie or patch directory with a clear message', () => {
  const good = { tool: path.join(work, 'fake-rotl-lingo'), movie: path.join(work, 'movie.dcr'), patches, port: PORT, toolConfig: path.join(work, 'movie.dcr') };
  assert.throws(() => checkConfig({ ...good, toolConfig: '' }), /no tool config/);
  assert.throws(() => checkConfig({ ...good, tool: '' }), /no tool/);
  assert.throws(() => checkConfig({ ...good, tool: path.join(work, 'nope') }), /does not exist/);
  assert.throws(() => checkConfig({ ...good, movie: path.join(work, 'nope.dcr') }), /movie .* does not exist/);
  assert.throws(() => checkConfig({ ...good, patches: path.join(work, 'nope') }), /patch directory .* does not exist/);
  assert.throws(() => checkConfig({ ...good, rig: 'ftp://example' }), /http/);
  assert.equal(checkConfig({ ...good, rig: 'http://127.0.0.1:19572/play.html' }).rig, 'http://127.0.0.1:19572');
});

test('options come from the command line over the environment', () => {
  const config = readConfig(['--port', '19576', '--tool', '/t'], { ROTL_LINGO_TOOL: '/env-tool', ROTL_LINGO_MOVIE: '/m', ROTL_LINGO_EDITOR_PORT: '19570', ROTL_LINGO_RIG: 'http://r' });
  assert.deepEqual(config, { tool: '/t', movie: '/m', patches: '', port: '19576', rig: 'http://r', toolConfig: '' });
  assert.throws(() => readConfig(['--shell', 'x'], {}), /unknown option/);
});

test('the Host header must be this server on its own port', async () => {
  for (const host of ['evil.example', 'evil.example:19575', '127.0.0.1', '127.0.0.1:19576', 'localhost:80', '127.0.0.1.evil.example:19575', null]) {
    const res = await call({ url: '/api/config', host });
    assert.equal(res.status, 403, `Host ${host}`);
  }
  assert.equal((await call({ url: '/api/config', host: `localhost:${PORT}` })).status, 200);
  assert.equal((await call({ url: '/api/config' })).status, 200);
  // Static files are behind the same check.
  assert.equal((await call({ url: '/', host: 'evil.example' })).status, 403);
  assert.equal((await call({ url: '/' })).status, 200);
});

test('a POST needs this app\'s Origin and a JSON content type', async () => {
  const body = { text: GOOD_TEXT };
  assert.equal((await call({ method: 'POST', url: '/api/compile', body, headers: { 'Content-Type': 'application/json' } })).status, 403, 'no Origin');
  assert.equal((await post('/api/compile', body, { Origin: 'http://evil.example' })).status, 403, 'foreign Origin');
  assert.equal((await post('/api/compile', body, { Origin: 'null' })).status, 403, 'null Origin');
  assert.equal((await post('/api/compile', body, { Origin: RIG })).status, 403, 'the rig may not POST');
  assert.equal((await post('/api/compile', body, { Origin: `http://127.0.0.1:19576` })).status, 403, 'another port');
  assert.equal((await post('/api/compile', body, { 'Content-Type': 'text/plain' })).status, 403, 'text/plain');
  assert.equal((await post('/api/compile', body, { 'Content-Type': 'application/x-www-form-urlencoded' })).status, 403, 'form');
  const ok = await post('/api/compile', body, { 'Content-Type': 'application/json; charset=utf-8' });
  assert.equal(ok.status, 200);
  assert.equal(ok.json.ok, true);
});

test('a GET of the API from another origin is refused', async () => {
  assert.equal((await call({ url: '/api/scripts', headers: { Origin: 'http://evil.example' } })).status, 403);
  assert.equal((await call({ url: '/api/scripts', headers: { Origin: ORIGIN } })).status, 200);
});

test('compile reads the last JSON line, hides the temp file and leaves no temp file behind', async () => {
  const res = await post('/api/compile', { text: GOOD_TEXT, file: 'a/b.ls' });
  assert.equal(res.status, 200);
  assert.deepEqual(res.json.sourceDiff, ['+ added']);
  assert.equal(res.json.file, 'a/b.ls');
  assert.deepEqual(fs.readdirSync(app.tempDir), []);
  // The tool was given an argument array ending in the movie, and a temp file inside the private directory.
  const last = toolCalls().at(-1);
  assert.equal(last[0], 'diff');
  assert.ok(last[1].startsWith(app.tempDir + path.sep));
  assert.deepEqual(last.slice(-2), ['--movie', path.join(work, 'movie.dcr')]);
  assert.equal(fs.statSync(app.tempDir).mode & 0o077, 0, 'the temp directory is private');
});

test('save: a normal save lands in the patch directory and shows git commands', async () => {
  const res = await post('/api/save', { file: 'Scripts.27-Game_Logic/DrawScene.ls', text: GOOD_TEXT });
  assert.equal(res.status, 200, res.text);
  const full = path.join(fs.realpathSync(patches), 'Scripts.27-Game_Logic', 'DrawScene.ls');
  assert.equal(res.json.path, full);
  assert.equal(fs.readFileSync(full, 'utf8'), GOOD_TEXT);
  assert.match(res.json.git[0], /^git -C \S+ add -- Scripts\.27-Game_Logic\/DrawScene\.ls$/);
  assert.match(res.json.git[1], /^git -C \S+ commit -m /);
  // Saving again overwrites the same patch file.
  const again = await post('/api/save', { file: 'Scripts.27-Game_Logic/DrawScene.ls', text: GOOD_TEXT + '-- more\n' });
  assert.equal(again.status, 200);
  assert.equal(fs.readFileSync(full, 'utf8'), GOOD_TEXT + '-- more\n');
  // The static check ran on the text before each write.
  assert.ok(toolCalls().some((c) => c[0] === 'check' && c.includes('--static')));
});

test('save: paths outside the patch directory are refused and nothing is written', async () => {
  const bad = [
    '../outside/victim.ls',
    'a/../../outside/victim.ls',
    'a/../b.ls',
    './b.ls',
    path.join(outside, 'victim.ls'), // absolute
    '/etc/passwd.ls',
    'a\\..\\b.ls',
    '..%2foutside%2fvictim.ls',
    'a//b.ls',
    'a/b.ls/',
    '.hidden.ls',
    '-o.ls',
    'a/b;c.ls',
    'a/b\u0000.ls',
    'a/b/c/d/e.ls', // too deep
    '',
  ];
  for (const file of bad) {
    const res = await post('/api/save', { file, text: GOOD_TEXT });
    assert.ok(res.status === 403 || res.status === 400, `${JSON.stringify(file)} gave ${res.status}`);
  }
  for (const file of [undefined, 5, ['a.ls'], { a: 1 }]) {
    assert.equal((await post('/api/save', { file, text: GOOD_TEXT })).status, 400);
  }
  assert.equal(fs.readFileSync(path.join(outside, 'victim.ls'), 'utf8'), 'untouched');
  assert.equal(fs.existsSync(path.join(patches, 'a')), false);
});

test('save: only .ls files', async () => {
  for (const file of ['notes.txt', 'a/patch.ls.txt', 'a/patch', 'a/patch.LS', '.ls', 'a/.ls', 'manifest.json']) {
    const res = await post('/api/save', { file, text: GOOD_TEXT });
    assert.equal(res.status, 403, file);
    assert.equal(fs.existsSync(path.join(patches, file)), false);
  }
});

test('save: symbolic links are refused, whether a file or a directory on the way', async () => {
  fs.symlinkSync(path.join(outside, 'victim.ls'), path.join(patches, 'link.ls'));
  fs.symlinkSync(path.join(outside, 'victim.txt'), path.join(patches, 'totxt.ls'));
  fs.symlinkSync(outside, path.join(patches, 'linkdir'));
  fs.symlinkSync(path.join(outside, 'not-there.ls'), path.join(patches, 'dangling.ls'));
  for (const file of ['link.ls', 'totxt.ls', 'linkdir/victim.ls', 'linkdir/new.ls', 'dangling.ls']) {
    const res = await post('/api/save', { file, text: GOOD_TEXT });
    assert.equal(res.status, 403, file);
    // Reading through a link is refused as well.
    assert.equal((await call({ url: `/api/patch?file=${encodeURIComponent(file)}` })).status, 403, `read ${file}`);
  }
  assert.equal(fs.readFileSync(path.join(outside, 'victim.ls'), 'utf8'), 'untouched');
  assert.equal(fs.readFileSync(path.join(outside, 'victim.txt'), 'utf8'), 'untouched');
  assert.deepEqual(fs.readdirSync(outside).sort(), ['victim.ls', 'victim.txt']);
  for (const name of ['link.ls', 'totxt.ls', 'linkdir', 'dangling.ls']) fs.unlinkSync(path.join(patches, name));
});

test('resolvePatchPath accepts the normal layout', () => {
  const file = patchFileFor('Scripts', 27, 'Obj Outlining + item swap', 'DrawScene');
  assert.equal(file, 'Scripts.27-Obj_Outlining_item_swap/DrawScene.ls');
  assert.equal(resolvePatchPath(patches, file), path.join(fs.realpathSync(patches), file));
  assert.equal(patchFileFor('Internal', 16, '', 'exitFrame'), 'Internal.16/exitFrame.ls');
  assert.equal(patchFileFor('../x', 1, '../../etc', '..'), 'x.1-etc/handler.ls');
});

test('save: the static check blocks connection code and nothing is written', async () => {
  const res = await post('/api/save', { file: 'blocked/one.ls', text: GOOD_TEXT + 'x = BADNAME\n' });
  assert.equal(res.status, 422);
  assert.match(res.json.error, /nothing was written/);
  assert.equal(res.json.problems.length, 1);
  assert.match(res.json.problems[0], /belongs to the connection code/);
  assert.equal(fs.existsSync(path.join(patches, 'blocked')), false);

  // An existing file is not overwritten by refused text either.
  const kept = path.join(patches, 'Scripts.27-Game_Logic', 'DrawScene.ls');
  const before = fs.readFileSync(kept, 'utf8');
  const second = await post('/api/save', { file: 'Scripts.27-Game_Logic/DrawScene.ls', text: 'BADNAME' });
  assert.equal(second.status, 422);
  assert.equal(fs.readFileSync(kept, 'utf8'), before);
  assert.deepEqual(fs.readdirSync(app.tempDir), []);
});

test('save: a connection finding from the check against the movie blocks too; a compile error does not', async () => {
  const blocked = await post('/api/save', { file: 'blocked/two.ls', text: GOOD_TEXT + 'x = "CONNSTRING"\n' });
  assert.equal(blocked.status, 422);
  assert.match(blocked.json.problems[0], /connection code uses/);
  assert.equal(fs.existsSync(path.join(patches, 'blocked')), false);

  const wip = await post('/api/save', { file: 'wip/unfinished.ls', text: GOOD_TEXT + 'SYNTAXERR\n' });
  assert.equal(wip.status, 200, wip.text);
  assert.equal(fs.existsSync(path.join(patches, 'wip', 'unfinished.ls')), true);
});

test('new: creates the file through the tool, refuses connection handlers and existing files', async () => {
  const made = await post('/api/new', { ref: 'Scripts/27/drawscene', why: 'because\n  of this' });
  assert.equal(made.status, 409, 'the earlier save made this file already');
  fs.rmSync(path.join(patches, 'Scripts.27-Game_Logic'), { recursive: true });
  const fresh = await post('/api/new', { ref: 'Scripts/27/drawscene', why: 'because\n  of this' });
  assert.equal(fresh.status, 200, fresh.text);
  assert.equal(fresh.json.file, 'Scripts.27-Game_Logic/DrawScene.ls');
  assert.match(fresh.json.text, /-- why: because of this\n/);
  const call_ = toolCalls().filter((c) => c[0] === 'new').at(-1);
  assert.equal(call_[call_.indexOf('-o') + 1], path.join(fs.realpathSync(patches), 'Scripts.27-Game_Logic', 'DrawScene.ls'));

  assert.equal((await post('/api/new', { ref: 'Scripts/27/Secret', why: 'x' })).status, 403);
  assert.equal((await post('/api/new', { ref: 'Scripts/27/Nope', why: 'x' })).status, 404);
  assert.equal((await post('/api/new', { ref: 'Scripts/27/DrawScene', why: '  ' })).status, 400);
  assert.equal((await post('/api/new', { ref: '--json/27/DrawScene', why: 'x' })).status, 400);
});

test('positional arguments that look like options never reach the tool', async () => {
  const before = toolCalls().length;
  assert.equal((await call({ url: '/api/search?q=--all' })).status, 400);
  assert.equal((await call({ url: '/api/search?q=-o' })).status, 400);
  assert.equal((await call({ url: '/api/handler?ref=--movie/x/y' })).status, 400);
  assert.equal((await call({ url: '/api/handler?ref=Scripts/27' })).status, 400);
  assert.equal(toolCalls().length, before);
  assert.equal((await call({ url: '/api/search?q=sword' })).status, 200);
  assert.equal((await call({ url: '/api/search?q=-1' })).status, 200, 'a leading minus alone is not an option');
});

test('run: passes evals and globals as separate arguments and reads the JSON after the noise', async () => {
  const res = await post('/api/run', { text: GOOD_TEXT, offline: true, evals: ['put 1', 'x = 1\nput x; rm -rf /'], globals: ['HeadImage'] });
  assert.equal(res.status, 200);
  assert.equal(res.json.ok, true);
  const args = toolCalls().at(-1);
  assert.equal(args[0], 'run');
  assert.equal(args[1], '--patch');
  assert.ok(args.includes('--offline'));
  assert.equal(args[args.indexOf('--global') + 1], 'HeadImage');
  assert.ok(args.includes('x = 1\nput x; rm -rf /'), 'the eval is one argument, untouched by any shell');
  assert.equal((await post('/api/run', { text: GOOD_TEXT, evals: [], globals: ['--movie'] })).status, 400);
  assert.equal((await post('/api/run', { text: GOOD_TEXT, evals: [], globals: [] })).status, 400);
  assert.deepEqual(fs.readdirSync(app.tempDir), []);
});

test('rig manifest: allows exactly the configured rig origin', async () => {
  const file = 'Scripts.27-Game_Logic/DrawScene.ls';
  const res = await call({ url: `/rig/manifest.json?file=${encodeURIComponent(file)}`, headers: { Origin: RIG } });
  assert.equal(res.status, 200, res.text);
  assert.equal(res.headers['access-control-allow-origin'], RIG);
  assert.notEqual(res.headers['access-control-allow-origin'], '*');
  assert.equal(res.json.version, 1);
  assert.equal(res.json.patches.length, 1);
  const call_ = toolCalls().at(-1);
  assert.deepEqual(call_.slice(0, 3), ['manifest', path.join(fs.realpathSync(patches), file), '--include-disabled']);

  // Another origin is turned away, and is never named in the header.
  const evil = await call({ url: `/rig/manifest.json?file=${encodeURIComponent(file)}`, headers: { Origin: 'http://evil.example' } });
  assert.equal(evil.status, 403);
  assert.equal(evil.headers['access-control-allow-origin'], undefined);
  // The path rules hold here too.
  assert.equal((await call({ url: '/rig/manifest.json?file=../outside/victim.ls', headers: { Origin: RIG } })).status, 403);
  assert.equal((await call({ url: '/rig/manifest.json?file=missing/x.ls', headers: { Origin: RIG } })).status, 404);
  // No other route carries a CORS header.
  assert.equal((await call({ url: '/api/config' })).headers['access-control-allow-origin'], undefined);
  assert.equal((await call({ method: 'POST', url: '/rig/manifest.json', headers: { Origin: RIG } })).status, 405);
});

test('rig manifest: absent when no rig is configured', async () => {
  const plain = createApp({ tool: path.join(work, 'fake-rotl-lingo'), movie: path.join(work, 'movie.dcr'), patches, port: PORT, toolConfig: path.join(work, 'movie.dcr') });
  try {
    const req = Readable.from([]);
    Object.assign(req, { method: 'GET', url: '/rig/manifest.json?file=a.ls', headers: { host: `127.0.0.1:${PORT}` } });
    const status = await new Promise((resolve) => {
      plain.handler(req, { writeHead(s) { this.s = s; }, end() { resolve(this.s); } });
    });
    assert.equal(status, 404);
  } finally {
    plain.close();
    assert.equal(fs.existsSync(plain.tempDir), false, 'close() removes the temp directory');
  }
});

test('request bodies over 1 MB are refused', async () => {
  const big = JSON.stringify({ file: 'big/one.ls', text: 'x'.repeat(1024 * 1024 + 10) });
  const streamed = await post('/api/save', big);
  assert.equal(streamed.status, 413);
  const declared = await post('/api/save', '{}', { 'Content-Length': String(2 * 1024 * 1024) });
  assert.equal(declared.status, 413);
  assert.equal(fs.existsSync(path.join(patches, 'big')), false);
  // Just under the limit is read (and then judged on its content).
  const under = await post('/api/compile', JSON.stringify({ text: 'y'.repeat(1024 * 1024 - 100) }));
  assert.equal(under.status, 200);
});

test('bad JSON, unknown routes and other methods', async () => {
  assert.equal((await post('/api/save', '{not json')).status, 400);
  assert.equal((await post('/api/save', '[1]')).status, 400);
  assert.equal((await post('/api/delete', {})).status, 404);
  assert.equal((await call({ url: '/api/nope' })).status, 404);
  assert.equal((await call({ url: '/../server.mjs' })).status, 404);
  assert.equal((await call({ method: 'DELETE', url: '/api/patch?file=a.ls', headers: { Origin: ORIGIN } })).status, 405);
  assert.equal((await call({ method: 'PUT', url: '/api/save', headers: { Origin: ORIGIN } })).status, 405);
});

test('one tool process at a time, and the script list is read once', async () => {
  const before = toolCalls().filter((c) => c[0] === 'list').length;
  const answers = await Promise.all([
    call({ url: '/api/scripts' }),
    call({ url: '/api/scripts' }),
    call({ url: '/api/search?q=a' }),
    call({ url: '/api/patches' }),
    post('/api/compile', { text: GOOD_TEXT }),
    post('/api/save', { file: 'queue/one.ls', text: GOOD_TEXT }),
    call({ url: '/api/handler?ref=Scripts/27/DrawScene' }),
  ]);
  for (const res of answers) assert.equal(res.status, 200, res.text);
  assert.equal(fs.existsSync(toolLog + '.overlap'), false, 'two tool processes ran at the same time');
  assert.equal(answers[0].json.scriptCount, 1);
  assert.ok(toolCalls().filter((c) => c[0] === 'list').length <= Math.max(before, 1), 'list ran at most once in the whole process');
  // Patch files are reported relative to the patch directory.
  assert.ok(answers[3].json.reports.every((r) => !path.isAbsolute(r.file)));
});

test('lastJson takes the last line that parses', () => {
  assert.deepEqual(lastJson('[WARN] x\n{"a":1}\n'), { a: 1 });
  assert.deepEqual(lastJson('{"a":1}\n{"b":2}\n[test] trailing\n'), { b: 2 });
  assert.equal(lastJson('no json here\n'), undefined);
});
