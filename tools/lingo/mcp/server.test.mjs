// node --test tools/lingo/mcp/
// These need no tool and no movie. The run against a real movie is the game repository's test, since it
// names that game's handlers.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { defaultPatchFile, handleMessage, lastJson, patchPath, TOOLS } from './server.mjs';

const scratch = () => fs.mkdtempSync(path.join(os.tmpdir(), 'rotl-lingo-mcp-test-'));

test('a patch file path cannot leave the patch directory', () => {
  const dir = scratch();
  const config = { patches: dir };
  assert.equal(patchPath(config, 'Scripts.27-Game Logic/DrawScene.ls'), path.join(fs.realpathSync(dir), 'Scripts.27-Game Logic', 'DrawScene.ls'));
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
  assert.equal(defaultPatchFile('Scripts', 27, 'Game Logic', 'DrawScene'), 'Scripts.27-Game Logic/DrawScene.ls');
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
