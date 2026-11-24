#!/usr/bin/env node
// An MCP server for the movie's Lingo: look up a handler, search the decompiled source, write a patch
// file, compile and check it, and run it in a native player.
//
// It is a thin layer over the `rotl-lingo` command (vm-rust/src/tools/rotl_lingo.rs), so an agent gets the
// same checks the build runs. What it can and cannot do:
//   - the movie is read-only: nothing here writes to it or to a server;
//   - the only files it writes are `*.ls` patch files inside the patch directory it was started with;
//   - handlers that deal with the connection come back with their strings replaced, and a patch that
//     touches them is refused (that rule is in the tool; this server has no way around it);
//   - it cannot enable a patch for release by accident: `lingo_new_patch` writes `enabled: no`, and
//     shipping is a build step in the game repo (`npm run lingo:build`), reviewed in a PR.
//
// Configuration, by environment:
//   ROTL_LINGO_TOOL     the rotl-lingo binary
//   ROTL_LINGO_MOVIE    the movie file (a copy of dcrgame.dcr)
//   ROTL_LINGO_PATCHES  the patch directory (client/lingo/patches in NodeJS-ROTL)
//
// Register with Claude Code (see docs/lingo-compiler.md in NodeJS-ROTL):
//   claude mcp add rotl-lingo -e ROTL_LINGO_TOOL=... -e ROTL_LINGO_MOVIE=... -e ROTL_LINGO_PATCHES=... \
//     -- node /path/to/dirplayer-rs-fso/tools/lingo/mcp/server.mjs
//
// The protocol is JSON-RPC 2.0 over stdin/stdout, one message a line. No dependencies.

import { execFile } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import readline from 'node:readline';
import { fileURLToPath } from 'node:url';

const PROTOCOL_VERSION = '2024-11-05';
const MAX_TEXT = 512 * 1024;
const MAX_OUTPUT = 60_000;

export function loadConfig(env = process.env) {
  const config = { tool: env.ROTL_LINGO_TOOL || '', movie: env.ROTL_LINGO_MOVIE || '', patches: env.ROTL_LINGO_PATCHES || '' };
  const problems = [];
  if (!config.tool || !fs.existsSync(config.tool)) problems.push('ROTL_LINGO_TOOL is not set to the rotl-lingo binary');
  if (!config.movie || !fs.existsSync(config.movie)) problems.push('ROTL_LINGO_MOVIE is not set to the movie file');
  if (!config.patches || !fs.existsSync(config.patches) || !fs.statSync(config.patches).isDirectory()) problems.push('ROTL_LINGO_PATCHES is not set to the patch directory');
  return { config, problems };
}

// ---- the tool ----

// One tool process at a time: each loads the 25 MB movie.
let queue = Promise.resolve();

function runTool(config, args, { timeout = 60_000 } = {}) {
  const job = queue.then(() => new Promise((resolve) => {
    execFile(config.tool, args, { timeout, maxBuffer: 64 * 1024 * 1024, env: { ...process.env, ROTL_LINGO_MOVIE: config.movie } }, (error, stdout, stderr) => {
      resolve({ code: error ? (typeof error.code === 'number' ? error.code : 1) : 0, stdout: String(stdout), stderr: String(stderr), killed: !!(error && error.killed) });
    });
  }));
  queue = job.catch(() => {});
  return job;
}

/** The last line of the output that is JSON: `run` prints the player's own log lines first. */
export function lastJson(stdout) {
  const lines = stdout.split('\n').map((l) => l.trim()).filter(Boolean);
  for (let i = lines.length - 1; i >= 0; i--) {
    if (lines[i].startsWith('{') || lines[i].startsWith('[')) {
      try { return JSON.parse(lines[i]); } catch { /* keep looking */ }
    }
  }
  return null;
}

async function toolJson(config, args, options) {
  const result = await runTool(config, [...args, '--json'], options);
  const json = lastJson(result.stdout);
  if (json === null) {
    const why = result.killed ? 'the tool took too long' : (result.stderr.trim().split('\n').pop() || `the tool exited with code ${result.code}`);
    throw new Error(why);
  }
  return json;
}

// ---- patch files: the only thing this server writes ----

const SAFE_NAME = /^[A-Za-z0-9 ._-]+$/;

/** A patch file's path inside the patch directory, or an error. `relative` is like `Scripts.27-INVENTORY SORT/LoadInf.ls`. */
export function patchPath(config, relative, { mustExist = false } = {}) {
  if (typeof relative !== 'string' || !relative) throw new Error('file: give the patch file\'s path inside the patch directory');
  if (path.isAbsolute(relative) || relative.includes('\\')) throw new Error('file: a path inside the patch directory, not an absolute one');
  const parts = relative.split('/');
  if (parts.length > 3 || parts.some((p) => !p || p === '.' || p === '..' || !SAFE_NAME.test(p))) throw new Error('file: only letters, digits, spaces, dots, dashes and underscores, at most two folders deep');
  if (!relative.endsWith('.ls')) throw new Error('file: a patch file ends in .ls');
  const root = fs.realpathSync(config.patches);
  const full = path.join(root, ...parts);
  // Whatever already exists on the way must really be inside the patch directory (no symlinks out).
  let probe = full;
  while (!fs.existsSync(probe)) probe = path.dirname(probe);
  const real = fs.realpathSync(probe);
  if (real !== root && !real.startsWith(root + path.sep)) throw new Error('file: that path leaves the patch directory');
  if (fs.existsSync(full) && fs.lstatSync(full).isSymbolicLink()) throw new Error('file: that is a link, not a patch file');
  if (mustExist && !fs.existsSync(full)) throw new Error(`there is no patch file ${relative}`);
  return full;
}

/** `Scripts`, 27, `INVENTORY SORT`, `LoadInf` -> `Scripts.27-INVENTORY SORT/LoadInf.ls` */
export function defaultPatchFile(cast, member, scriptName, handler) {
  const clean = (s) => String(s).replace(/[^A-Za-z0-9 ._-]+/g, '_').trim();
  const folder = scriptName ? `${clean(cast)}.${member}-${clean(scriptName)}` : `${clean(cast)}.${member}`;
  return `${folder}/${clean(handler)}.ls`;
}

function listPatchFiles(config) {
  const root = fs.realpathSync(config.patches);
  const out = [];
  const walk = (dir, depth) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory() && depth < 2) walk(full, depth + 1);
      else if (entry.isFile() && entry.name.endsWith('.ls')) out.push(path.relative(root, full).split(path.sep).join('/'));
    }
  };
  walk(root, 0);
  return out;
}

async function withTempPatch(text, fn) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'rotl-lingo-mcp-'));
  const file = path.join(dir, 'patch.ls');
  try {
    fs.writeFileSync(file, text);
    return await fn(file);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

// ---- tools ----

const str = (description) => ({ type: 'string', description });

export const TOOLS = [
  {
    name: 'lingo_list_scripts',
    description: 'List the movie\'s scripts and their handlers. Each script has a ref like "Scripts/27" (cast library / member number); a handler\'s ref adds its name: "Scripts/27/LoadInf". Give `cast` or `filter` to narrow the list; without them only a count per cast comes back, since the movie has about 750 scripts.',
    inputSchema: { type: 'object', properties: { cast: str('Only this cast library, e.g. "Scripts"'), filter: str('Only scripts whose name or a handler name contains this text') } },
  },
  {
    name: 'lingo_get_handler',
    description: 'A handler\'s decompiled Lingo source, as the compiler reads it back, with its fingerprint (`stock`), whether it can be patched (`editable`), and the script\'s globals and properties. `bytecode: true` adds the instruction listing. Handlers that deal with the server connection come back with every string replaced by "<redacted>" and cannot be patched.',
    inputSchema: { type: 'object', properties: { ref: str('"<cast>/<member>/<handler>", e.g. "Scripts/27/LoadInf". <member> is the member number, or the script\'s name when unique in its cast.'), bytecode: { type: 'boolean' } }, required: ['ref'] },
  },
  {
    name: 'lingo_search',
    description: 'Find text in the decompiled source of every handler (case-insensitive, plain text). Returns ref, line number and the line. Good for finding where a global, a sprite number or a handler is used.',
    inputSchema: { type: 'object', properties: { query: str('The text to look for'), limit: { type: 'integer', description: 'How many hits to return (default 60, at most 400)' } }, required: ['query'] },
  },
  {
    name: 'lingo_round_trip',
    description: 'Recompile every handler of the movie from its own decompiled source and compare with the movie\'s bytes. This is the compiler\'s self-test and the gate for patching: a handler that fails here cannot be patched.',
    inputSchema: { type: 'object', properties: {} },
  },
  {
    name: 'lingo_list_patches',
    description: 'The patch files in the patch directory, each checked against the movie: ok or its problems, the handler it replaces, and whether it is enabled for release.',
    inputSchema: { type: 'object', properties: {} },
  },
  {
    name: 'lingo_read_patch',
    description: 'The text of a patch file.',
    inputSchema: { type: 'object', properties: { file: str('The patch file\'s path inside the patch directory, as lingo_list_patches gives it') }, required: ['file'] },
  },
  {
    name: 'lingo_new_patch',
    description: 'Start a patch file for a handler: a header (script, handler, the stock fingerprint, `enabled: no`, your `why`) followed by the handler\'s own source, ready to edit. Fails if the file exists or the handler cannot be patched. Returns the file\'s path and text.',
    inputSchema: { type: 'object', properties: { ref: str('"<cast>/<member>/<handler>"'), why: str('What the patch changes and why, in a sentence or two. It goes in the file\'s header.') }, required: ['ref', 'why'] },
  },
  {
    name: 'lingo_write_patch',
    description: 'Write the full text of a patch file (header and handler). The text is checked first; a text that uses the connection code is refused and nothing is written. Writing does not enable or ship anything: release is `npm run lingo:build` in the game repo, after review. Returns the check of the saved file.',
    inputSchema: { type: 'object', properties: { file: str('The patch file\'s path inside the patch directory'), text: str('The whole file') }, required: ['file', 'text'] },
  },
  {
    name: 'lingo_check_patch',
    description: 'Compile a patch and check it against the movie: the gate (the stock handler must recompile to its exact bytes), the fingerprint, compile errors with line and column, warnings, and a diff of source and bytecode against the stock handler. Give `file` for a saved patch or `text` to check without saving.',
    inputSchema: { type: 'object', properties: { file: str('A saved patch file'), text: str('Or the text of a patch, not saved') } },
  },
  {
    name: 'lingo_run',
    description: 'Load the movie in a native player (no browser, no server), apply patches, run Lingo, and read globals. `evals` are run in order; each is one or more lines of Lingo (a call, an assignment). `globals` are read after the evals. `offline: true` gives the movie a stand-in for its server connection so handlers that send a message do not fail. Use it to compare a patched handler with the stock one: run once with `patches`, once without.',
    inputSchema: {
      type: 'object',
      properties: {
        patches: { type: 'array', items: { type: 'string' }, description: 'Saved patch files to apply (enabled or not)' },
        text: str('Or the text of one unsaved patch to apply'),
        evals: { type: 'array', items: { type: 'string' }, description: 'Lingo to run, in order' },
        globals: { type: 'array', items: { type: 'string' }, description: 'Names of globals to read afterwards' },
        offline: { type: 'boolean' },
      },
    },
  },
];

function splitRef(ref) {
  const parts = String(ref || '').split('/');
  if (parts.length !== 3 || parts.some((p) => !p)) throw new Error('ref: "<cast>/<member>/<handler>", e.g. "Scripts/27/LoadInf"');
  return parts;
}

let scriptsCache = null;
async function scripts(config) {
  if (!scriptsCache) scriptsCache = await toolJson(config, ['list']);
  return scriptsCache;
}

export async function callTool(config, name, args = {}) {
  switch (name) {
    case 'lingo_list_scripts': {
      const all = await scripts(config);
      if (!args.cast && !args.filter) {
        const casts = {};
        for (const s of all.scripts) casts[s.cast] = (casts[s.cast] || 0) + 1;
        return { scriptCount: all.scriptCount, handlerCount: all.handlerCount, scriptsPerCast: casts, hint: 'give `cast` or `filter` to list scripts' };
      }
      const needle = String(args.filter || '').toLowerCase();
      const found = all.scripts.filter((s) => (!args.cast || s.cast.toLowerCase() === String(args.cast).toLowerCase())
        && (!needle || s.name.toLowerCase().includes(needle) || s.handlers.some((h) => h.name.toLowerCase().includes(needle))));
      return { count: found.length, scripts: found.map((s) => ({ ref: s.ref, name: s.name, kind: s.kind, handlers: s.handlers.map((h) => (h.sensitive ? `${h.name} (connection code)` : h.name)) })) };
    }
    case 'lingo_get_handler': {
      splitRef(args.ref);
      return toolJson(config, ['show', args.ref, ...(args.bytecode ? ['--bytecode'] : [])]);
    }
    case 'lingo_search': {
      if (typeof args.query !== 'string' || !args.query.trim()) throw new Error('query: the text to look for');
      const limit = Math.max(1, Math.min(400, Number(args.limit) || 60));
      const found = await toolJson(config, ['search', args.query, '--all']);
      return { query: found.query, total: found.total, shown: Math.min(limit, found.hits.length), hits: found.hits.slice(0, limit) };
    }
    case 'lingo_round_trip':
      return toolJson(config, ['roundtrip'], { timeout: 120_000 });
    case 'lingo_list_patches': {
      const files = listPatchFiles(config);
      if (!files.length) return { files: 0, reports: [] };
      const checked = await toolJson(config, ['check', config.patches]);
      const root = fs.realpathSync(config.patches);
      return { ...checked, reports: checked.reports.map((r) => ({ ...r, file: path.relative(root, fs.realpathSync(r.file)).split(path.sep).join('/') })) };
    }
    case 'lingo_read_patch':
      return { file: args.file, text: fs.readFileSync(patchPath(config, args.file, { mustExist: true }), 'utf8') };
    case 'lingo_new_patch': {
      const [cast, member] = splitRef(args.ref);
      if (typeof args.why !== 'string' || args.why.trim().length < 10) throw new Error('why: say what the patch changes and why');
      const shown = await toolJson(config, ['show', args.ref]);
      if (shown.sensitive) throw new Error('that handler deals with the connection; it cannot be patched');
      if (!shown.editable) throw new Error(`that handler cannot be patched: ${shown.refusal || 'it does not pass the round trip'}`);
      const relative = defaultPatchFile(shown.cast || cast, shown.member || member, shown.script, shown.handler);
      const full = patchPath(config, relative);
      if (fs.existsSync(full)) throw new Error(`${relative} already exists; read it and write a new version instead`);
      const made = await runTool(config, ['new', args.ref, '-o', full, '--why', args.why.replace(/\s+/g, ' ').trim(), '--json']);
      if (made.code !== 0) throw new Error(made.stderr.trim().split('\n').pop() || 'the tool could not start the patch');
      return { file: relative, text: fs.readFileSync(full, 'utf8'), note: 'enabled: no. Edit the handler, then lingo_write_patch and lingo_check_patch.' };
    }
    case 'lingo_write_patch': {
      if (typeof args.text !== 'string' || !args.text.trim()) throw new Error('text: the whole patch file');
      if (Buffer.byteLength(args.text) > MAX_TEXT) throw new Error('text: too large for a patch file');
      const full = patchPath(config, args.file);
      // The part of the secrets rule that needs no movie, before anything touches the disk.
      const pre = await withTempPatch(args.text, (tmp) => toolJson(config, ['check', '--static', tmp]));
      const problems = (pre.reports || []).flatMap((r) => r.problems || []);
      if (problems.length) return { saved: false, problems };
      const checked = await withTempPatch(args.text, (tmp) => toolJson(config, ['diff', tmp]));
      if ((checked.problems || []).some((p) => /connection/.test(p))) return { saved: false, problems: checked.problems };
      fs.mkdirSync(path.dirname(full), { recursive: true });
      fs.writeFileSync(full, args.text.endsWith('\n') ? args.text : `${args.text}\n`);
      return { saved: true, file: args.file, check: { ...checked, file: args.file } };
    }
    case 'lingo_check_patch': {
      if (args.file) return { ...(await toolJson(config, ['diff', patchPath(config, args.file, { mustExist: true })])), file: args.file };
      if (typeof args.text !== 'string' || !args.text.trim()) throw new Error('give `file` or `text`');
      if (Buffer.byteLength(args.text) > MAX_TEXT) throw new Error('text: too large for a patch file');
      return withTempPatch(args.text, async (tmp) => ({ ...(await toolJson(config, ['diff', tmp])), file: '(unsaved text)' }));
    }
    case 'lingo_run': {
      const list = (value, what) => {
        if (value === undefined) return [];
        if (!Array.isArray(value) || value.some((v) => typeof v !== 'string')) throw new Error(`${what}: a list of strings`);
        if (value.length > 50) throw new Error(`${what}: at most 50`);
        return value;
      };
      const evals = list(args.evals, 'evals');
      const globals = list(args.globals, 'globals');
      if (globals.some((g) => !/^[A-Za-z_][A-Za-z0-9_]*$/.test(g))) throw new Error('globals: plain names');
      const files = list(args.patches, 'patches').map((f) => patchPath(config, f, { mustExist: true }));
      const run = (extra) => {
        const argv = ['run', ...files.flatMap((f) => ['--patch', f]), ...extra, ...(args.offline ? ['--offline'] : [])];
        for (const code of evals) argv.push('--eval', code.replace(/\r?\n/g, '\\n'));
        for (const g of globals) argv.push('--global', g);
        return toolJson(config, argv, { timeout: 120_000 });
      };
      if (typeof args.text === 'string' && args.text.trim()) return withTempPatch(args.text, (tmp) => run(['--patch', tmp]));
      return run([]);
    }
    default:
      throw new Error(`unknown tool ${name}`);
  }
}

// ---- the protocol ----

function clip(text) {
  return text.length > MAX_OUTPUT ? `${text.slice(0, MAX_OUTPUT)}\n... (${text.length - MAX_OUTPUT} more characters; narrow the request)` : text;
}

export async function handleMessage(config, problems, message) {
  const { id, method, params } = message;
  const reply = (result) => ({ jsonrpc: '2.0', id, result });
  const fail = (code, text) => ({ jsonrpc: '2.0', id, error: { code, message: text } });
  if (method === 'initialize') {
    return reply({
      protocolVersion: PROTOCOL_VERSION,
      capabilities: { tools: {} },
      serverInfo: { name: 'rotl-lingo', version: '1.0.0' },
      instructions: 'Read and patch the Lingo of the ROTL movie. The movie is read-only; the only files written are .ls patch files in the patch directory. A patch replaces one handler: lingo_get_handler, lingo_new_patch, edit, lingo_write_patch, lingo_check_patch, lingo_run. Patches are not shipped from here.',
    });
  }
  if (method === 'ping') return reply({});
  if (method === 'tools/list') return reply({ tools: TOOLS });
  if (method === 'tools/call') {
    const text = (value, isError = false) => reply({ content: [{ type: 'text', text: clip(typeof value === 'string' ? value : JSON.stringify(value, null, 1)) }], isError });
    if (problems.length) return text(`rotl-lingo is not set up: ${problems.join('; ')}`, true);
    if (!TOOLS.some((t) => t.name === params?.name)) return fail(-32602, `unknown tool ${params?.name}`);
    try {
      return text(await callTool(config, params.name, params.arguments || {}));
    } catch (error) {
      return text(String(error && error.message ? error.message : error), true);
    }
  }
  // Notifications (no id) need no answer; anything else with an id is not something this server does.
  if (id === undefined || id === null) return null;
  return fail(-32601, `method not found: ${method}`);
}

function main() {
  const { config, problems } = loadConfig();
  for (const problem of problems) process.stderr.write(`rotl-lingo mcp: ${problem}\n`);
  const input = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  let pending = Promise.resolve();
  input.on('line', (line) => {
    if (!line.trim()) return;
    let message;
    try { message = JSON.parse(line); } catch { process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', id: null, error: { code: -32700, message: 'parse error' } })}\n`); return; }
    // Answers go out in the order the requests came in.
    pending = pending.then(async () => {
      const answer = await handleMessage(config, problems, message);
      if (answer) process.stdout.write(`${JSON.stringify(answer)}\n`);
    });
  });
  input.on('close', () => pending.then(() => process.exit(0)));
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
