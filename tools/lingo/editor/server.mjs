#!/usr/bin/env node
// Lingo patch editor: a local web front end to the `rotl-lingo` command-line tool.
//
// A developer tool. It listens on 127.0.0.1 only, never runs git, never deploys, and the only files it
// writes are `*.ls` patch files inside the patch directory it was started with.
//
//   node tools/lingo/editor/server.mjs --tool <rotl-lingo> --movie <movie.dcr> --patches <dir>
//                                      [--port 19575] [--rig http://127.0.0.1:19572]
//
// Each option can also come from the environment: ROTL_LINGO_TOOL, ROTL_LINGO_MOVIE, ROTL_LINGO_PATCHES,
// ROTL_LINGO_EDITOR_PORT, ROTL_LINGO_RIG, and ROTL_LINGO_CONFIG for --config (the game's tool config).

import { execFile } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PUBLIC_DIR = path.join(HERE, 'public');

const DEFAULT_PORT = 19575;
const MAX_BODY = 1024 * 1024; // request bodies above 1 MB are refused
const TOOL_TIMEOUT_MS = 30_000;
const RUN_TIMEOUT_MS = 120_000;
const MAX_TOOL_OUTPUT = 64 * 1024 * 1024;
const MAX_PATCH_DEPTH = 4;

// The only static files there are; a fixed table, so no request path ever reaches the file system.
const STATIC_FILES = {
  '/': ['index.html', 'text/html; charset=utf-8'],
  '/index.html': ['index.html', 'text/html; charset=utf-8'],
  '/app.js': ['app.js', 'text/javascript; charset=utf-8'],
  '/app.css': ['app.css', 'text/css; charset=utf-8'],
};

/** An error that becomes an HTTP response with this status. */
class HttpError extends Error {
  constructor(status, message, extra) {
    super(message);
    this.status = status;
    this.extra = extra;
  }
}

// ---------------------------------------------------------------------------------------------------
// Configuration

/** Reads the options from the command line and the environment. Throws with a clear message. */
export function readConfig(argv, env) {
  const config = {
    tool: env.ROTL_LINGO_TOOL || '',
    movie: env.ROTL_LINGO_MOVIE || '',
    patches: env.ROTL_LINGO_PATCHES || '',
    port: env.ROTL_LINGO_EDITOR_PORT || String(DEFAULT_PORT),
    rig: env.ROTL_LINGO_RIG || '',
    toolConfig: env.ROTL_LINGO_CONFIG || '',
  };
  for (let i = 0; i < argv.length; i++) {
    const name = argv[i];
    if (!['--tool', '--movie', '--patches', '--port', '--rig', '--config'].includes(name)) {
      throw new Error(`unknown option ${name}`);
    }
    if (i + 1 >= argv.length) throw new Error(`${name} needs a value`);
    config[name === '--config' ? 'toolConfig' : name.slice(2)] = argv[++i];
  }
  return config;
}

/** Checks the configuration and resolves its paths. Throws with a clear message when something is missing. */
export function checkConfig(config) {
  const out = {};
  if (!config.tool) throw new Error('no tool: pass --tool <path to rotl-lingo> or set ROTL_LINGO_TOOL');
  if (!config.movie) throw new Error('no movie: pass --movie <path> or set ROTL_LINGO_MOVIE');
  if (!config.patches) throw new Error('no patch directory: pass --patches <dir> or set ROTL_LINGO_PATCHES');
  out.tool = path.resolve(config.tool);
  if (!isFile(out.tool)) throw new Error(`the tool ${out.tool} does not exist (build rotl-lingo first)`);
  out.movie = path.resolve(config.movie);
  if (!isFile(out.movie)) throw new Error(`the movie ${out.movie} does not exist`);
  const patches = path.resolve(config.patches);
  if (!isDirectory(patches)) throw new Error(`the patch directory ${patches} does not exist`);
  // The game's list of connection-code names. Without it the tool would have nothing to hide.
  if (!config.toolConfig) throw new Error('no tool config: pass --config <file> or set ROTL_LINGO_CONFIG');
  out.toolConfig = path.resolve(config.toolConfig);
  if (!isFile(out.toolConfig)) throw new Error(`the tool config ${out.toolConfig} does not exist`);
  // The real path, so that every later "is it inside?" test compares like with like.
  out.patches = fs.realpathSync(patches);
  out.port = Number(config.port ?? DEFAULT_PORT);
  if (!Number.isInteger(out.port) || out.port < 1 || out.port > 65535) throw new Error(`bad port ${config.port}`);
  out.rig = '';
  if (config.rig) {
    let url;
    try {
      url = new URL(config.rig);
    } catch {
      throw new Error(`the rig URL ${config.rig} is not a URL`);
    }
    if (url.protocol !== 'http:' && url.protocol !== 'https:') throw new Error('the rig URL must be http or https');
    out.rig = url.origin; // only the origin is ever used: it is what goes in the CORS header
  }
  return out;
}

function isFile(p) {
  try {
    return fs.statSync(p).isFile();
  } catch {
    return false;
  }
}

function isDirectory(p) {
  try {
    return fs.statSync(p).isDirectory();
  } catch {
    return false;
  }
}

// ---------------------------------------------------------------------------------------------------
// Patch file paths

/** Makes a cast, script or handler name safe as part of a file name. */
export function safeName(text) {
  const cleaned = String(text).replace(/[^A-Za-z0-9._-]+/g, '_').replace(/^[._-]+/, '').replace(/_+$/, '');
  return cleaned.slice(0, 60);
}

/** `<Cast>.<member>-<Script name>/<handler>.ls`, the layout of the game repo's patch directory. */
export function patchFileFor(cast, member, scriptName, handler) {
  const name = safeName(scriptName);
  const dir = `${safeName(cast) || 'cast'}.${member}${name ? '-' + name : ''}`;
  return `${dir}/${safeName(handler) || 'handler'}.ls`;
}

/**
 * Turns a relative patch file name from a request into an absolute path, or throws.
 * Nothing outside the patch directory, nothing but `.ls`, no symbolic link in the way.
 */
export function resolvePatchPath(patchesDir, relative) {
  if (typeof relative !== 'string' || relative === '') throw new HttpError(400, 'no patch file named');
  if (relative.length > 300) throw new HttpError(400, 'the patch file name is too long');
  if (relative.startsWith('/') || relative.includes('\\') || path.isAbsolute(relative)) {
    throw new HttpError(403, 'the patch file must be a relative path inside the patch directory');
  }
  const parts = relative.split('/');
  if (parts.length > MAX_PATCH_DEPTH) throw new HttpError(403, 'the patch file is nested too deep');
  for (const part of parts) {
    if (part === '' || part === '.' || part === '..') throw new HttpError(403, 'the patch file path may not contain empty, `.` or `..` parts');
    if (!/^[A-Za-z0-9 ._-]+$/.test(part)) throw new HttpError(403, 'patch file names may contain only letters, digits, space, `.`, `_` and `-`');
    if (part.startsWith('.') || part.startsWith('-') || part.startsWith(' ') || part.endsWith(' ')) {
      throw new HttpError(403, 'a patch file name may not start with `.`, `-` or a space, or end with a space');
    }
  }
  if (!parts[parts.length - 1].endsWith('.ls') || parts[parts.length - 1].length < 4) {
    throw new HttpError(403, 'only `.ls` patch files can be read or written');
  }
  const root = fs.realpathSync(patchesDir);
  const full = path.join(root, ...parts);

  // Walk down from the root: every part that exists must be a real directory (or, last, a real file).
  // A symbolic link anywhere is refused outright, which rules out every escape through one.
  let current = root;
  for (let i = 0; i < parts.length; i++) {
    current = path.join(current, parts[i]);
    let stat;
    try {
      stat = fs.lstatSync(current);
    } catch (e) {
      if (e.code === 'ENOENT') break; // the rest does not exist yet
      throw new HttpError(403, `cannot look at ${parts.slice(0, i + 1).join('/')}: ${e.code}`);
    }
    const last = i === parts.length - 1;
    if (stat.isSymbolicLink()) throw new HttpError(403, 'the patch file path goes through a symbolic link');
    if (last ? !stat.isFile() : !stat.isDirectory()) throw new HttpError(403, 'the patch file path is not a plain file in plain directories');
  }
  // Belt and braces: the real path of the deepest part that exists must still be under the root.
  let existing = full;
  while (!fs.existsSync(existing)) existing = path.dirname(existing);
  const real = fs.realpathSync(existing);
  if (real !== root && !real.startsWith(root + path.sep)) throw new HttpError(403, 'the patch file is outside the patch directory');
  return full;
}

/** Writes a patch file. The path has been through `resolvePatchPath`. */
function writePatchFile(full, text) {
  fs.mkdirSync(path.dirname(full), { recursive: true });
  // O_NOFOLLOW: if a link appeared here since the check, fail rather than write through it.
  const flags = fs.constants.O_WRONLY | fs.constants.O_CREAT | fs.constants.O_TRUNC | fs.constants.O_NOFOLLOW;
  const fd = fs.openSync(full, flags, 0o644);
  try {
    fs.writeFileSync(fd, text);
  } finally {
    fs.closeSync(fd);
  }
}

function shellQuote(text) {
  return /^[A-Za-z0-9_@%+=:,./-]+$/.test(text) ? text : `'${String(text).replace(/'/g, `'\\''`)}'`;
}

// ---------------------------------------------------------------------------------------------------
// The app

/**
 * Builds the request handler without listening. `config` is what `checkConfig` returns.
 * Returns `{ handler, close, config }`; `close()` removes the temp directory.
 */
export function createApp(rawConfig) {
  const config = checkConfig(rawConfig);
  const ownOrigins = new Set([`http://127.0.0.1:${config.port}`, `http://localhost:${config.port}`]);
  const ownHosts = new Set([`127.0.0.1:${config.port}`, `localhost:${config.port}`]);

  // Editor text is handed to the tool as a file. The files live in a directory only this user can read
  // (the text may be something the checks are about to refuse) and are removed after each call.
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'rotl-lingo-editor-'));
  let tempCount = 0;
  let closed = false;

  function close() {
    if (closed) return;
    closed = true;
    fs.rmSync(tempDir, { recursive: true, force: true });
  }

  async function withTempFile(text, use) {
    const file = path.join(tempDir, `patch-${process.pid}-${++tempCount}.ls`);
    fs.writeFileSync(file, text, { mode: 0o600, flag: 'wx' });
    try {
      return await use(file);
    } finally {
      fs.rmSync(file, { force: true });
    }
  }

  // One tool process at a time: each loads the whole movie, and the machine is short on memory.
  let queue = Promise.resolve();
  function runTool(args, { timeout = TOOL_TIMEOUT_MS } = {}) {
    const job = queue.then(
      () =>
        new Promise((resolve) => {
          // An argument array and no shell: nothing in a request is ever interpreted as a command.
          execFile(config.tool, [...args, '--movie', config.movie], { timeout, killSignal: 'SIGKILL', maxBuffer: MAX_TOOL_OUTPUT, windowsHide: true, env: { ...process.env, ...(config.toolConfig ? { ROTL_LINGO_CONFIG: config.toolConfig } : {}) } }, (error, stdout, stderr) => {
            const timedOut = Boolean(error && error.killed);
            const code = error ? (typeof error.code === 'number' ? error.code : -1) : 0;
            resolve({ code, stdout: String(stdout), stderr: String(stderr), timedOut, spawnError: error && typeof error.code === 'string' ? error.code : '' });
          });
        }),
    );
    queue = job.catch(() => {});
    return job;
  }

  /** Runs the tool and reads the JSON it prints. A non-zero exit with JSON is still a result. */
  async function toolJson(args, options) {
    const result = await runTool(args, options);
    if (result.timedOut) throw new HttpError(504, `rotl-lingo ${args[0]} took too long and was stopped`);
    const value = lastJson(result.stdout);
    if (value === undefined) throw new HttpError(502, toolFailure(args[0], result));
    return value;
  }

  function toolFailure(command, result) {
    if (result.spawnError) return `rotl-lingo could not be run (${result.spawnError})`;
    // The tool prints a failing file's problems on stdout and its last word on stderr; show both.
    const said = scrub(`${result.stdout.trim()}\n${result.stderr.trim()}`).split('\n').filter((l) => l.trim() !== '' && !/^\[(WARN|test)\]/.test(l)).slice(-8).join('\n');
    return `rotl-lingo ${command} failed (exit ${result.code})${said ? ': ' + said : ''}`;
  }

  /** Temp file paths mean nothing to the developer; show them as "the editor text". */
  function scrub(text) {
    return String(text).split(tempDir + path.sep).join('').replace(/patch-\d+-\d+\.ls/g, '(editor text)');
  }

  function scrubReport(report, file) {
    if (!report || typeof report !== 'object') return report;
    report.file = file || '';
    for (const key of ['problems', 'warnings']) {
      if (Array.isArray(report[key])) report[key] = report[key].map((p) => scrub(p));
    }
    return report;
  }

  // The movie does not change while the editor runs, so its script list is read once.
  let listPromise = null;
  function scriptList() {
    if (!listPromise) {
      listPromise = toolJson(['list', '--json']).catch((e) => {
        listPromise = null; // a failure is not cached
        throw e;
      });
    }
    return listPromise;
  }

  // The tool has no "end of options" marker, so a positional that looks like an option would be read as one.
  function positional(value, what) {
    if (typeof value !== 'string' || value.trim() === '') throw new HttpError(400, `no ${what} given`);
    if (value.length > 400) throw new HttpError(400, `the ${what} is too long`);
    // The tool reads `--anything`, `-o` and `-h` as options wherever they stand.
    if (value.startsWith('--') || value === '-o' || value === '-h') throw new HttpError(400, `the ${what} may not start with "--" or be "-o" or "-h" (the tool would read it as an option)`);
    if (/[\0\r\n]/.test(value)) throw new HttpError(400, `the ${what} may not contain line breaks`);
    return value;
  }

  function handlerRef(value) {
    const ref = positional(value, 'handler reference');
    if (ref.split('/').length !== 3) throw new HttpError(400, 'a handler reference is <cast>/<member>/<handler>');
    return ref;
  }

  function patchText(value) {
    if (typeof value !== 'string') throw new HttpError(400, 'no patch text given');
    return value;
  }

  function relativeOf(full) {
    return path.relative(config.patches, full).split(path.sep).join('/');
  }

  // --- the API ---------------------------------------------------------------------------------------

  const getRoutes = {
    '/api/config': async () => ({
      movie: config.movie,
      patches: config.patches,
      rig: config.rig,
      origin: `http://127.0.0.1:${config.port}`,
    }),

    '/api/scripts': async () => scriptList(),

    '/api/handler': async (query) => toolJson(['show', handlerRef(query.get('ref')), '--json', '--bytecode']),

    '/api/search': async (query) => toolJson(['search', positional(query.get('q'), 'search text'), '--json']),

    // Every patch file in the directory, with what the build's check says about it.
    '/api/patches': async () => {
      const report = await toolJson(['check', config.patches, '--json']);
      const reports = (report.reports || []).map((r) => {
        const full = path.resolve(String(r.file));
        return { ...r, file: relativeOf(full) };
      });
      return { ok: report.ok, files: report.files, failed: report.failed, reports };
    },

    '/api/patch': async (query) => {
      const file = query.get('file');
      const full = resolvePatchPath(config.patches, file);
      if (!isFile(full)) throw new HttpError(404, `${file} does not exist`);
      return { file, path: full, text: fs.readFileSync(full, 'utf8') };
    },
  };

  const postRoutes = {
    // Start a patch: the tool writes the header and the handler's own source. It refuses connection code.
    '/api/new': async (body) => {
      const ref = handlerRef(body.ref);
      const why = typeof body.why === 'string' ? body.why.replace(/\s+/g, ' ').trim() : '';
      if (!why) throw new HttpError(400, 'say why: what this patch changes and what for');
      if (why.length > 500) throw new HttpError(400, 'the "why" is too long (500 characters at most)');
      const [cast, member, handler] = ref.split('/');
      const list = await scriptList();
      const script = (list.scripts || []).find((s) => s.cast === cast && String(s.member) === member);
      if (!script) throw new HttpError(404, `no script ${cast}/${member} in the movie`);
      const known = script.handlers.find((h) => h.name.toLowerCase() === handler.toLowerCase());
      if (!known) throw new HttpError(404, `no handler ${handler} in ${cast}/${member}`);
      if (known.sensitive) throw new HttpError(403, 'connection code: strings hidden, cannot be patched');
      const file = patchFileFor(script.cast, script.member, script.name, known.name);
      const full = resolvePatchPath(config.patches, file);
      if (fs.existsSync(full)) throw new HttpError(409, `${file} already exists; open it from the patch list`, { file });
      // The directory is made here, after the path check, so the tool only ever writes the one file.
      fs.mkdirSync(path.dirname(full), { recursive: true });
      const result = await runTool(['new', `${script.cast}/${script.member}/${known.name}`, '-o', full, '--why', why, '--json']);
      if (result.timedOut) throw new HttpError(504, 'rotl-lingo new took too long and was stopped');
      if (result.code !== 0 || !isFile(full)) throw new HttpError(422, toolFailure('new', result));
      return { file, path: full, text: fs.readFileSync(full, 'utf8') };
    },

    // Compile the editor text and compare it with the movie's handler. Nothing is saved.
    '/api/compile': async (body) => {
      const text = patchText(body.text);
      const report = await withTempFile(text, (temp) => toolJson(['diff', temp, '--json']));
      return scrubReport(report, typeof body.file === 'string' ? body.file : '');
    },

    // Load the movie in the native player, apply the editor text as a patch, run some Lingo.
    '/api/run': async (body) => {
      const evals = stringList(body.evals, 'eval', 40, 4000);
      const globals = stringList(body.globals, 'global name', 40, 100);
      for (const name of globals) {
        if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) throw new HttpError(400, `"${name}" is not a global's name`);
      }
      if (evals.length === 0 && globals.length === 0) throw new HttpError(400, 'nothing to run: give some Lingo or a global to read');
      const run = async (temp) => {
        const args = ['run'];
        if (temp) args.push('--patch', temp);
        if (body.offline) args.push('--offline');
        for (const code of evals) args.push('--eval', code);
        for (const name of globals) args.push('--global', name);
        args.push('--json');
        const result = await runTool(args, { timeout: RUN_TIMEOUT_MS });
        if (result.timedOut) throw new HttpError(504, 'the run took longer than 120 seconds and was stopped');
        const value = lastJson(result.stdout);
        // Without JSON the tool refused before running (a patch with problems) or the player crashed.
        if (value === undefined) return { ok: false, notRun: true, error: toolFailure('run', result), results: [] };
        return value;
      };
      return body.usePatch === false ? run(null) : withTempFile(patchText(body.text), run);
    },

    // Save the editor text to its patch file. The only write this server makes besides `new`.
    '/api/save': async (body) => {
      const text = patchText(body.text);
      const full = resolvePatchPath(config.patches, body.file);
      // The rule that keeps connection code out of patch files. Nothing is written when it objects.
      const problems = await withTempFile(text, async (temp) => {
        const found = [];
        const fixed = await toolJson(['check', '--static', '--json', temp]);
        for (const r of fixed.reports || []) found.push(...(r.problems || []));
        if (!fixed.ok && found.length === 0) found.push('the static check failed');
        if (found.length > 0) return found;
        // The static check knows names only. The check against the movie also knows which handlers and
        // strings belong to the connection; those findings block a save too. Compile errors do not:
        // work in progress may be saved.
        const deep = await toolJson(['check', '--json', temp]);
        // The tool marks such a report `secret`; the wording test is for a tool older than that flag.
        for (const r of deep.reports || []) {
          if (r.secret === true) found.push(...(r.problems || []).filter((p) => /connection/i.test(p)), ...((r.problems || []).some((p) => /connection/i.test(p)) ? [] : ['the file must not be saved: it touches the connection code']));
          else if (r.secret === undefined) found.push(...(r.problems || []).filter((p) => /connection/i.test(p)));
        }
        return found;
      });
      if (problems.length > 0) {
        throw new HttpError(422, 'the check refuses this text; nothing was written', { problems: problems.map((p) => scrub(p)) });
      }
      writePatchFile(full, text);
      const file = relativeOf(full);
      const id = (/^--\s*handler:\s*(\S+)/im.exec(text) || [])[1] || path.basename(file, '.ls');
      return {
        ok: true,
        file,
        path: full,
        bytes: Buffer.byteLength(text),
        // Shown to the developer to run by hand. This server never runs git.
        git: [`git -C ${shellQuote(config.patches)} add -- ${shellQuote(file)}`, `git -C ${shellQuote(config.patches)} commit -m ${shellQuote(`Lingo patch: ${id}`)} -- ${shellQuote(file)}`],
      };
    },
  };

  // The manifest of one saved patch file, for a rig page to fetch from its browser console.
  async function rigManifest(query) {
    const full = resolvePatchPath(config.patches, query.get('file'));
    if (!isFile(full)) throw new HttpError(404, `${query.get('file')} does not exist (save it first)`);
    const result = await runTool(['manifest', full, '--include-disabled']);
    if (result.timedOut) throw new HttpError(504, 'rotl-lingo manifest took too long and was stopped');
    const manifest = lastJson(result.stdout);
    if (result.code !== 0 || !manifest || !Array.isArray(manifest.patches)) throw new HttpError(422, toolFailure('manifest', result));
    return manifest;
  }

  // --- HTTP ------------------------------------------------------------------------------------------

  function send(res, status, value, headers = {}) {
    const body = Buffer.from(JSON.stringify(value));
    res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8', 'Content-Length': body.length, 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff', ...headers });
    res.end(body);
  }

  function sendError(res, error, headers) {
    if (error instanceof HttpError) return send(res, error.status, { error: error.message, ...(error.extra || {}) }, headers);
    console.error(error);
    return send(res, 500, { error: `internal error: ${error && error.message}` }, headers);
  }

  function readBody(req) {
    return new Promise((resolve, reject) => {
      const declared = Number(req.headers['content-length'] || 0);
      if (declared > MAX_BODY) return reject(new HttpError(413, 'the request is larger than 1 MB'));
      const chunks = [];
      let size = 0;
      let done = false;
      req.on('data', (chunk) => {
        if (done) return;
        size += chunk.length;
        if (size > MAX_BODY) {
          done = true;
          chunks.length = 0;
          reject(new HttpError(413, 'the request is larger than 1 MB'));
          return;
        }
        chunks.push(chunk);
      });
      req.on('end', () => {
        if (done) return;
        done = true;
        resolve(Buffer.concat(chunks).toString('utf8'));
      });
      req.on('error', (e) => {
        if (done) return;
        done = true;
        reject(new HttpError(400, `the request could not be read: ${e.message}`));
      });
    });
  }

  async function handler(req, res) {
    // A page on another site that points its own domain at 127.0.0.1 still sends its own name as Host.
    const host = String(req.headers.host || '').toLowerCase();
    if (!ownHosts.has(host)) return send(res, 403, { error: 'this server answers only as 127.0.0.1 or localhost on its own port' });

    let url;
    try {
      url = new URL(req.url, `http://${host}`);
    } catch {
      return send(res, 400, { error: 'bad request URL' });
    }
    const origin = req.headers.origin;

    try {
      if (url.pathname === '/rig/manifest.json') {
        if (!config.rig) throw new HttpError(404, 'no rig is configured');
        if (req.method !== 'GET') throw new HttpError(405, 'GET only');
        // Exactly the configured rig may read this from a page; never a wildcard.
        const cors = { 'Access-Control-Allow-Origin': config.rig, Vary: 'Origin' };
        if (origin !== undefined && origin !== config.rig && !ownOrigins.has(origin)) {
          return send(res, 403, { error: 'this origin is not the configured rig' });
        }
        try {
          return send(res, 200, await rigManifest(url.searchParams), cors);
        } catch (e) {
          return sendError(res, e, cors);
        }
      }

      if (req.method === 'GET' || req.method === 'HEAD') {
        const asset = STATIC_FILES[url.pathname];
        if (asset) {
          const body = fs.readFileSync(path.join(PUBLIC_DIR, asset[0]));
          res.writeHead(200, {
            'Content-Type': asset[1],
            'Content-Length': body.length,
            'Cache-Control': 'no-store',
            'X-Content-Type-Options': 'nosniff',
            'X-Frame-Options': 'DENY',
            // The page loads nothing from anywhere else and talks to nothing else.
            'Content-Security-Policy': "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
            'Referrer-Policy': 'no-referrer',
          });
          return res.end(req.method === 'HEAD' ? undefined : body);
        }
        const route = getRoutes[url.pathname];
        if (!route) throw new HttpError(404, 'not found');
        // Another site may not even start a tool run with a cross-site GET.
        if (origin !== undefined && !ownOrigins.has(origin)) throw new HttpError(403, 'requests from other origins are refused');
        return send(res, 200, await route(url.searchParams));
      }

      if (req.method === 'POST') {
        // Every request that changes or runs something must come from this app's own page, as JSON.
        // A form on another site can send neither this Origin nor this Content-Type.
        if (origin === undefined || !ownOrigins.has(origin)) throw new HttpError(403, 'POST needs the Origin of this app');
        const type = String(req.headers['content-type'] || '').split(';')[0].trim().toLowerCase();
        if (type !== 'application/json') throw new HttpError(403, 'POST needs Content-Type: application/json');
        const route = postRoutes[url.pathname];
        if (!route) throw new HttpError(404, 'not found');
        const raw = await readBody(req);
        let body;
        try {
          body = JSON.parse(raw);
        } catch {
          throw new HttpError(400, 'the request body is not JSON');
        }
        if (!body || typeof body !== 'object' || Array.isArray(body)) throw new HttpError(400, 'the request body must be a JSON object');
        return send(res, 200, await route(body));
      }

      throw new HttpError(405, 'method not allowed');
    } catch (error) {
      // After refusing a body the rest of it is not read; close the connection rather than leave it half-read.
      const headers = error instanceof HttpError && error.status === 413 ? { Connection: 'close' } : {};
      return sendError(res, error, headers);
    }
  }

  return { handler, close, config, tempDir };
}

function stringList(value, what, maxItems, maxLength) {
  if (value === undefined || value === null) return [];
  if (!Array.isArray(value)) throw new HttpError(400, `the ${what} list must be an array`);
  if (value.length > maxItems) throw new HttpError(400, `too many ${what}s (${maxItems} at most)`);
  return value.map((item) => {
    if (typeof item !== 'string' || item.length > maxLength || item.includes('\0')) throw new HttpError(400, `bad ${what}`);
    return item;
  }).filter((item) => item.trim() !== '');
}

/** The tool may print `[WARN]` lines before its JSON: the last line of output that parses is the answer. */
export function lastJson(stdout) {
  const lines = String(stdout).split('\n');
  for (let i = lines.length - 1; i >= 0; i--) {
    const line = lines[i].trim();
    if (!line.startsWith('{') && !line.startsWith('[')) continue;
    try {
      return JSON.parse(line);
    } catch {
      // not this line
    }
  }
  return undefined;
}

// ---------------------------------------------------------------------------------------------------
// Started from the command line

function main() {
  let app;
  try {
    app = createApp(readConfig(process.argv.slice(2), process.env));
  } catch (e) {
    console.error(`lingo editor: ${e.message}`);
    console.error('usage: node tools/lingo/editor/server.mjs --tool <rotl-lingo> --movie <movie> --patches <dir> [--port 19575] [--rig http://127.0.0.1:19572]');
    process.exit(2);
  }
  const server = http.createServer((req, res) => {
    app.handler(req, res).catch((e) => {
      console.error(e);
      if (!res.headersSent) res.writeHead(500);
      res.end();
    });
  });
  const stop = () => {
    app.close();
    process.exit(0);
  };
  process.on('exit', () => app.close());
  process.on('SIGINT', stop);
  process.on('SIGTERM', stop);
  process.on('SIGHUP', stop);
  server.on('error', (e) => {
    console.error(`lingo editor: cannot listen on 127.0.0.1:${app.config.port}: ${e.message}`);
    app.close();
    process.exit(1);
  });
  // The loopback address only: nothing else on the network can reach this.
  server.listen(app.config.port, '127.0.0.1', () => {
    console.log(`Lingo patch editor: http://127.0.0.1:${app.config.port}/`);
    console.log(`  movie   ${app.config.movie}`);
    console.log(`  patches ${app.config.patches}`);
    console.log(`  rig     ${app.config.rig || '(none)'}`);
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(fs.realpathSync(process.argv[1])).href) {
  main();
}
