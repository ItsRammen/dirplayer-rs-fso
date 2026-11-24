// node --test tools/lingo/mcp/
// The path and protocol tests need nothing. The rest runs the real tool on the real movie and is skipped
// unless ROTL_LINGO_TOOL and ROTL_LINGO_MOVIE are set.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { callTool, defaultPatchFile, handleMessage, lastJson, patchPath, TOOLS } from './server.mjs';

const scratch = () => fs.mkdtempSync(path.join(os.tmpdir(), 'rotl-lingo-mcp-test-'));

test('a patch file path cannot leave the patch directory', () => {
  const dir = scratch();
  const config = { patches: dir };
  assert.equal(patchPath(config, 'Scripts.27-INVENTORY SORT/LoadInf.ls'), path.join(fs.realpathSync(dir), 'Scripts.27-INVENTORY SORT', 'LoadInf.ls'));
  for (const bad of ['../x.ls', '/etc/x.ls', 'a/../../x.ls', 'a/b/c/d.ls', 'x.txt', 'a\\b.ls', 'a/./b.ls', 'a/b$.ls', '', 'x.ls/'])
    assert.throws(() => patchPath(config, bad), /file:/, bad);
  // A folder that is a link to somewhere else is not inside.
  const outside = scratch();
  fs.symlinkSync(outside, path.join(dir, 'link'));
  assert.throws(() => patchPath(config, 'link/x.ls'), /leaves the patch directory/);
  fs.writeFileSync(path.join(outside, 'real.ls'), '');
  fs.symlinkSync(path.join(outside, 'real.ls'), path.join(dir, 'alias.ls'));
  assert.throws(() => patchPath(config, 'alias.ls'), /leaves the patch directory|link/);
  assert.throws(() => patchPath(config, 'missing.ls', { mustExist: true }), /no patch file/);
});

test('the default file name for a patch', () => {
  assert.equal(defaultPatchFile('Scripts', 27, 'INVENTORY SORT', 'LoadInf'), 'Scripts.27-INVENTORY SORT/LoadInf.ls');
  assert.equal(defaultPatchFile('Internal', 16, '', 'exitFrame'), 'Internal.16/exitFrame.ls');
  assert.equal(defaultPatchFile('A/B', 1, 'x:y?', 'h'), 'A_B.1-x_y_/h.ls');
});

test('the last JSON line of mixed output', () => {
  assert.deepEqual(lastJson('[WARN] x\n[test] Load\n{"ok":true}\n'), { ok: true });
  assert.equal(lastJson('nothing here'), null);
});

test('the protocol: initialize, list, unknown method, not set up', async () => {
  const init = await handleMessage({}, [], { jsonrpc: '2.0', id: 1, method: 'initialize', params: {} });
  assert.equal(init.result.serverInfo.name, 'rotl-lingo');
  const list = await handleMessage({}, [], { jsonrpc: '2.0', id: 2, method: 'tools/list' });
  assert.deepEqual(list.result.tools.map((t) => t.name), TOOLS.map((t) => t.name));
  // Nothing here can delete, enable for release, push or reveal.
  assert.ok(!TOOLS.some((t) => /delete|deploy|push|reveal|enable/i.test(t.name)));
  assert.equal(await handleMessage({}, [], { jsonrpc: '2.0', method: 'notifications/initialized' }), null);
  assert.equal((await handleMessage({}, [], { jsonrpc: '2.0', id: 3, method: 'nope' })).error.code, -32601);
  const unset = await handleMessage({}, ['ROTL_LINGO_TOOL is not set'], { jsonrpc: '2.0', id: 4, method: 'tools/call', params: { name: 'lingo_search', arguments: { query: 'x' } } });
  assert.equal(unset.result.isError, true);
});

const real = process.env.ROTL_LINGO_TOOL && process.env.ROTL_LINGO_MOVIE && fs.existsSync(process.env.ROTL_LINGO_TOOL) && fs.existsSync(process.env.ROTL_LINGO_MOVIE);

test('on the real movie: find, start, edit, check and run a patch', { skip: !real && 'set ROTL_LINGO_TOOL and ROTL_LINGO_MOVIE' }, async () => {
  const config = { tool: process.env.ROTL_LINGO_TOOL, movie: process.env.ROTL_LINGO_MOVIE, patches: scratch() };

  const found = await callTool(config, 'lingo_list_scripts', { filter: 'LoadInf' });
  assert.ok(found.scripts.some((s) => s.ref === 'Scripts/27'));
  const hits = await callTool(config, 'lingo_search', { query: 'LoadInf(', limit: 5 });
  assert.ok(hits.total >= 1 && hits.hits.length <= 5);

  // Connection code: no strings, no patch.
  const scripts = (await callTool(config, 'lingo_list_scripts', { cast: 'Internal' })).scripts;
  const locked = scripts.flatMap((s) => s.handlers.filter((h) => h.endsWith('(connection code)')).map((h) => `${s.ref}/${h.split(' ')[0]}`))[0];
  assert.ok(locked, 'the movie has connection code');
  const hidden = await callTool(config, 'lingo_get_handler', { ref: locked });
  assert.equal(hidden.sensitive, true);
  assert.equal(hidden.editable, false);
  for (const literal of hidden.source.match(/"[^"\n]*"/g) || []) assert.equal(literal, '"<redacted>"');
  await assert.rejects(callTool(config, 'lingo_new_patch', { ref: locked, why: 'This must be refused by the tool.' }), /connection/);

  const made = await callTool(config, 'lingo_new_patch', { ref: 'Scripts/27/LoadInf', why: 'Test: wearing an eyepatch changes the head picture.' });
  assert.equal(made.file, 'Scripts.27-INVENTORY SORT/LoadInf.ls');
  assert.match(made.text, /^-- lingo-patch 1\n-- script: Scripts\/27 \(INVENTORY SORT\)\n-- handler: LoadInf\n-- stock: [0-9a-f]{64}\n-- enabled: no\n/);
  await assert.rejects(callTool(config, 'lingo_new_patch', { ref: 'Scripts/27/LoadInf', why: 'A second one for the same handler.' }), /already exists/);

  const unchanged = await callTool(config, 'lingo_check_patch', { file: made.file });
  assert.equal(unchanged.ok, true);
  assert.equal(unchanged.unchanged, true);

  const end = made.text.lastIndexOf('end\n');
  const edited = `${made.text.slice(0, end)}  if StaticInf contains "W-Eyepatch" then\n    HeadImage = "Eyepatch"\n  end if\n${made.text.slice(end)}`;
  const broken = await callTool(config, 'lingo_check_patch', { text: edited.replace('HeadImage = "Eyepatch"', 'HeadImage = (') });
  assert.equal(broken.ok, false);
  assert.equal(broken.refused, 'compile');
  assert.ok(broken.line > 1);

  const refused = await callTool(config, 'lingo_write_patch', { file: made.file, text: edited.replace('HeadImage = "Eyepatch"', 'HeadImage = baDecryptText(1, 2)') });
  assert.equal(refused.saved, false);
  assert.equal(fs.readFileSync(path.join(config.patches, made.file), 'utf8'), made.text, 'a refused write changes nothing');

  const saved = await callTool(config, 'lingo_write_patch', { file: made.file, text: edited });
  assert.equal(saved.saved, true);
  assert.equal(saved.check.ok, true);
  assert.equal(saved.check.unchanged, false);
  assert.ok(saved.check.sourceDiff.some((l) => l.startsWith('+ ') && l.includes('Eyepatch')));
  assert.ok(saved.check.bytecodeDiff.some((l) => l.startsWith('+ ')));

  const listed = await callTool(config, 'lingo_list_patches', {});
  assert.deepEqual(listed.reports.map((r) => [r.file, r.ok, r.enabled]), [[made.file, true, false]]);

  const setup = 'MyCharsBody = "DefChar"\nMyHead = "Head1"\nCurInvList = 1\nmname = "Tester"\nMyChar = "Tester"\nbFileDat = "X"';
  const evals = [setup, 'TheInf = "W-Red Shirt:W-Eyepatch:Apple"\nLoadInf(TheInf)'];
  const patched = await callTool(config, 'lingo_run', { patches: [made.file], evals, globals: ['HeadImage'], offline: true });
  assert.equal(patched.patches.applied, 1);
  assert.equal(patched.results.at(-1).value, '"Eyepatch"');
  const stock = await callTool(config, 'lingo_run', { evals, globals: ['HeadImage'], offline: true });
  assert.equal(stock.results.at(-1).value, '"Head1"');
});
