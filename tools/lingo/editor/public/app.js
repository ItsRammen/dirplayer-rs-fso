// Lingo patch editor: the page. Plain DOM, no libraries. Everything it knows comes from the local server
// (which runs the rotl-lingo tool); all text from the tool is put in the page as text, never as HTML.
'use strict';

const $ = (id) => document.getElementById(id);

const SENSITIVE_NOTE = 'connection code: strings hidden, cannot be patched';

const state = {
  config: null,
  scripts: [], // from the tool's `list`
  pickedScript: '', // "Cast/27"
  handler: null, // what `show` said about the handler in the middle pane
  patches: [], // reports from `check` on the patch directory
  file: '', // the patch file open in the editor, relative to the patch directory
  savedText: '', // the editor text as last loaded or saved
  report: null, // the last compile report for the editor text
  compileSeq: 0,
  compileTimer: 0,
  busy: 0,
};

// ---------------------------------------------------------------------------------------------------
// Small helpers

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function clear(node) {
  while (node.firstChild) node.removeChild(node.firstChild);
  return node;
}

function showPageError(message) {
  $('page-error-text').textContent = message;
  $('page-error').hidden = false;
}

/** Calls the server. Resolves with the JSON; rejects with an Error whose `data` is the server's answer. */
async function api(path, body) {
  state.busy++;
  $('busy').hidden = false;
  try {
    const options = body === undefined ? {} : { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) };
    let response;
    try {
      response = await fetch(path, options);
    } catch (e) {
      throw new Error(`the editor's server did not answer (${e.message}); is it still running?`);
    }
    let data = null;
    try {
      data = await response.json();
    } catch {
      // not JSON: reported below
    }
    if (!response.ok) {
      const error = new Error((data && data.error) || `the server answered ${response.status}`);
      error.data = data || {};
      error.status = response.status;
      throw error;
    }
    return data;
  } finally {
    state.busy--;
    $('busy').hidden = state.busy === 0;
  }
}

function selectTab(attribute, name) {
  for (const button of document.querySelectorAll(`button[${attribute}]`)) {
    const on = button.getAttribute(attribute) === name;
    button.setAttribute('aria-selected', on ? 'true' : 'false');
    const prefix = attribute === 'data-side' ? 'side-' : 'out-';
    $(prefix + button.getAttribute(attribute)).hidden = !on;
  }
}

function isDirty() {
  return state.file !== '' && $('editor').value !== state.savedText;
}

function okToLeaveEditor() {
  return !isDirty() || window.confirm(`${state.file} has changes that are not saved. Discard them?`);
}

// ---------------------------------------------------------------------------------------------------
// Browse: scripts grouped by cast, then handlers

function renderScripts() {
  const filter = $('script-filter').value.trim().toLowerCase();
  const list = clear($('script-list'));
  let shown = 0;
  let lastCast = null;
  for (const script of state.scripts) {
    const nameHit = !filter || script.name.toLowerCase().includes(filter) || script.ref.toLowerCase().includes(filter) || String(script.member) === filter;
    const handlerHits = filter ? script.handlers.filter((h) => h.name.toLowerCase().includes(filter)) : [];
    if (!nameHit && handlerHits.length === 0) continue;
    shown++;
    if (script.cast !== lastCast) {
      lastCast = script.cast;
      list.appendChild(el('div', 'cast-head', `Cast ${script.cast}`));
    }
    const picked = script.ref === state.pickedScript;
    const row = el('button', 'item' + (picked ? ' picked' : ''));
    row.type = 'button';
    row.title = `${script.ref} (${script.kind} script, ${script.handlers.length} handler${script.handlers.length === 1 ? '' : 's'})`;
    row.appendChild(el('span', 'num', String(script.member)));
    row.appendChild(document.createTextNode(` ${script.name || '(unnamed)'} `));
    row.appendChild(el('span', 'kind', `${script.kind}, ${script.handlers.length}`));
    row.addEventListener('click', () => {
      state.pickedScript = picked ? '' : script.ref;
      renderScripts();
    });
    list.appendChild(row);
    // A picked script shows all its handlers; a filter shows the handlers it matched.
    const handlers = picked ? script.handlers : handlerHits;
    for (const handler of handlers) {
      const ref = `${script.ref}/${handler.name}`;
      const item = el('button', 'item handler' + (state.handler && state.handler.ref === ref ? ' picked' : ''));
      item.type = 'button';
      item.appendChild(document.createTextNode(`${handler.name}(${handler.args.join(', ')}) `));
      item.appendChild(el('span', 'small', `${handler.bytes} b `));
      if (handler.sensitive) {
        item.appendChild(el('span', 'tag lock', 'locked'));
        item.title = SENSITIVE_NOTE;
      }
      item.addEventListener('click', () => openHandler(ref));
      list.appendChild(item);
    }
  }
  if (shown === 0) list.appendChild(el('div', 'empty', 'No script or handler matches the filter.'));
  $('script-summary').textContent = `${shown} of ${state.scripts.length} scripts shown`;
}

// ---------------------------------------------------------------------------------------------------
// Search

async function runSearch(event) {
  event.preventDefault();
  const text = $('search-text').value;
  const results = clear($('search-results'));
  if (!text.trim()) {
    $('search-summary').textContent = 'Type some text to look for.';
    return;
  }
  $('search-summary').textContent = 'Searching...';
  try {
    const found = await api(`/api/search?q=${encodeURIComponent(text)}`);
    $('search-summary').textContent = found.total === 0 ? 'No line contains that text.' : `${found.shown} of ${found.total} lines shown`;
    for (const hit of found.hits) {
      const row = el('button', 'item hit');
      row.type = 'button';
      row.appendChild(el('span', 'small', `${hit.ref}:${hit.line}  ${hit.script}`));
      row.appendChild(el('code', '', hit.text));
      row.addEventListener('click', () => openHandler(hit.ref));
      results.appendChild(row);
    }
  } catch (e) {
    $('search-summary').textContent = '';
    results.appendChild(el('div', 'msg error', e.message));
  }
}

// ---------------------------------------------------------------------------------------------------
// The movie's handler (read-only)

async function openHandler(ref) {
  $('stock-title').textContent = ref;
  $('stock-meta').textContent = 'Loading...';
  $('why-form').hidden = true;
  let handler;
  try {
    handler = await api(`/api/handler?ref=${encodeURIComponent(ref)}`);
  } catch (e) {
    $('stock-meta').textContent = '';
    clear($('stock-flags')).appendChild(el('div', 'msg error', e.message));
    return;
  }
  state.handler = handler;
  state.pickedScript = `${handler.cast}/${handler.member}`;
  renderScripts();
  renderHandler();
}

function patchesFor(ref) {
  return state.patches.filter((p) => p.id && p.id.toLowerCase() === ref.toLowerCase());
}

function renderHandler() {
  const h = state.handler;
  if (!h) return;
  $('stock-title').textContent = h.ref;
  $('stock-meta').textContent = `${h.kind} script "${h.script || '(unnamed)'}", ${h.bytes} bytes of bytecode`;
  const flags = clear($('stock-flags'));
  if (h.sensitive) {
    const note = el('div', 'msg warn');
    note.appendChild(el('span', 'tag lock', 'locked'));
    note.appendChild(document.createTextNode(' ' + SENSITIVE_NOTE));
    flags.appendChild(note);
  } else if (!h.editable) {
    flags.appendChild(el('div', 'msg warn', `Cannot be patched: ${h.refusal || 'the tool cannot recompile this handler to the same bytes'}`));
  }
  const existing = patchesFor(h.ref);
  $('stock-actions').hidden = false;
  $('start-patch').disabled = !h.editable;
  $('start-patch').title = h.editable ? '' : 'This handler cannot be patched';
  $('open-existing').hidden = existing.length === 0;
  $('open-existing').textContent = existing.length > 1 ? `Open its patch file (${existing.length} exist, see Patch files)` : 'Open its patch file';
  $('stock-names').hidden = false;
  $('stock-globals').textContent = h.scriptGlobals && h.scriptGlobals.length ? h.scriptGlobals.join(', ') : '(none)';
  $('stock-properties').textContent = h.properties && h.properties.length ? h.properties.join(', ') : '(none)';
  $('stock-source').textContent = h.source;
  $('stock-bytecode').textContent = (h.bytecode || []).join('\n');
  $('stock-bytecode-wrap').hidden = !$('show-bytecode').checked;
}

// ---------------------------------------------------------------------------------------------------
// Patch files

async function loadPatches() {
  $('patches-summary').textContent = 'Checking...';
  try {
    const result = await api('/api/patches');
    state.patches = result.reports;
    $('patches-summary').textContent = result.files === 0 ? 'No patch files yet.' : `${result.files} file${result.files === 1 ? '' : 's'}, ${result.failed} with problems`;
    $('patch-count').textContent = `(${result.files})`;
  } catch (e) {
    state.patches = [];
    $('patches-summary').textContent = '';
    clear($('patch-list')).appendChild(el('div', 'msg error', e.message));
    return;
  }
  renderPatches();
  renderHandler();
}

function renderPatches() {
  const list = clear($('patch-list'));
  if (state.patches.length === 0) {
    list.appendChild(el('div', 'empty', 'No patch files. Pick a handler and choose "Start a patch".'));
    return;
  }
  for (const patch of state.patches) {
    const row = el('button', 'item patch' + (patch.file === state.file ? ' picked' : ''));
    row.type = 'button';
    row.appendChild(el('span', patch.ok ? 'tag ok' : 'tag bad', patch.ok ? 'ok' : 'FAIL'));
    row.appendChild(document.createTextNode(' '));
    row.appendChild(el('span', patch.enabled ? 'tag ok' : 'tag off', patch.enabled ? 'enabled' : 'not enabled'));
    row.appendChild(document.createTextNode(` ${patch.id || '(no header)'}`));
    row.appendChild(el('div', 'small', patch.file));
    if (patch.why) row.appendChild(el('div', 'small', patch.why));
    for (const problem of patch.problems || []) row.appendChild(el('div', 'small error', problem));
    row.addEventListener('click', () => openPatch(patch.file));
    list.appendChild(row);
  }
}

async function openPatch(file) {
  if (file !== state.file && !okToLeaveEditor()) return;
  try {
    const patch = await api(`/api/patch?file=${encodeURIComponent(file)}`);
    showPatch(patch.file, patch.text);
  } catch (e) {
    showPageError(`Cannot open ${file}: ${e.message}`);
  }
}

function showPatch(file, text) {
  state.file = file;
  state.savedText = text;
  state.report = null;
  $('editor').value = text;
  $('patch-title').textContent = file;
  $('patch-meta').textContent = `${state.config.patches}/${file}`;
  $('patch-body').hidden = false;
  $('editor').scrollTop = 0;
  renderGutter();
  renderDirty();
  renderRig();
  renderPatches();
  scheduleCompile(0);
}

function suggestedFile(h) {
  // Only a preview; the server decides the name with the same rule.
  const safe = (text) => String(text).replace(/[^A-Za-z0-9._-]+/g, '_').replace(/^[._-]+/, '').replace(/_+$/, '').slice(0, 60);
  const name = safe(h.script);
  return `${safe(h.cast) || 'cast'}.${h.member}${name ? '-' + name : ''}/${safe(h.handler) || 'handler'}.ls`;
}

function startPatch() {
  const h = state.handler;
  if (!h || !h.editable) return;
  $('why-form').hidden = false;
  $('why-error').hidden = true;
  $('why-file').textContent = `will create ${suggestedFile(h)} (enabled: no)`;
  $('why-text').focus();
}

async function createPatch(event) {
  event.preventDefault();
  const h = state.handler;
  if (!h) return;
  if (!okToLeaveEditor()) return;
  $('why-error').hidden = true;
  try {
    const made = await api('/api/new', { ref: h.ref, why: $('why-text').value });
    $('why-form').hidden = true;
    $('why-text').value = '';
    showPatch(made.file, made.text);
    await loadPatches();
  } catch (e) {
    $('why-error').textContent = e.message;
    $('why-error').hidden = false;
  }
}

// ---------------------------------------------------------------------------------------------------
// The editor: a textarea with a gutter of line numbers

function renderGutter() {
  const editor = $('editor');
  const gutter = $('gutter');
  const lines = editor.value.split('\n').length;
  const badLine = state.report && !state.report.ok && Number.isInteger(state.report.line) ? state.report.line : 0;
  if (gutter.childElementCount !== lines) {
    clear(gutter);
    const fragment = document.createDocumentFragment();
    for (let i = 1; i <= lines; i++) fragment.appendChild(el('div', '', String(i)));
    gutter.appendChild(fragment);
  }
  for (const marked of gutter.querySelectorAll('.bad-line')) {
    marked.className = '';
    marked.title = '';
  }
  if (badLine >= 1 && badLine <= lines) {
    gutter.children[badLine - 1].className = 'bad-line';
  }
  gutter.scrollTop = editor.scrollTop;
}

function renderDirty() {
  $('dirty').textContent = isDirty() ? 'Changes not saved' : 'Saved text';
}

function onEditorKey(event) {
  // Tab types two spaces instead of leaving the editor. Escape then Tab still moves the focus on.
  if (event.key !== 'Tab' || event.shiftKey || event.ctrlKey || event.altKey || event.metaKey) return;
  event.preventDefault();
  const editor = $('editor');
  editor.setRangeText('  ', editor.selectionStart, editor.selectionEnd, 'end');
  onEditorInput();
}

function onEditorInput() {
  renderGutter();
  renderDirty();
  scheduleCompile(500);
}

// ---------------------------------------------------------------------------------------------------
// Compile and compare while typing

function scheduleCompile(delay) {
  clearTimeout(state.compileTimer);
  $('compile-state').className = 'state wait';
  $('compile-state').textContent = 'checking...';
  state.compileTimer = setTimeout(compileNow, delay);
}

async function compileNow() {
  if (!state.file) return;
  const seq = ++state.compileSeq;
  const file = state.file;
  let report;
  try {
    report = await api('/api/compile', { text: $('editor').value, file });
  } catch (e) {
    report = { ok: false, problems: [e.message], warnings: [], serverError: true };
  }
  // A newer edit, or another file, has overtaken this answer.
  if (seq !== state.compileSeq || file !== state.file) return;
  state.report = report;
  renderReport();
  renderGutter();
}

function renderDiff(target, lines, emptyText) {
  clear(target);
  if (!lines || lines.length === 0) {
    target.appendChild(el('span', 'same', emptyText));
    return;
  }
  for (const line of lines) {
    const kind = line.startsWith('+ ') ? 'add' : line.startsWith('- ') ? 'del' : 'same';
    target.appendChild(el('span', kind, line));
  }
}

function renderReport() {
  const r = state.report;
  const out = clear($('out-check'));
  const badge = $('compile-state');
  if (!r) return;
  badge.className = r.ok ? 'state ok' : 'state bad';
  badge.textContent = r.ok ? 'compiles' : `${(r.problems || []).length || 1} problem${(r.problems || []).length === 1 ? '' : 's'}`;
  document.querySelector('button[data-out="check"]').classList.toggle('has-problem', !r.ok);

  if (r.ok) {
    const size = `${r.stockBytes} -> ${r.bytes} bytes`;
    const same = r.unchanged ? '; identical to the movie\'s handler' : '';
    out.appendChild(el('div', 'msg ok', `OK: ${r.id} compiles (${size}${same}).`));
  }
  for (const problem of r.problems || []) {
    out.appendChild(el('div', 'msg error', `Problem: ${problem}`)); // the tool's text already names the line
  }
  if (!r.ok && Number.isInteger(r.line)) {
    const jump = el('button', '', `Go to line ${r.line}`);
    jump.type = 'button';
    jump.addEventListener('click', () => goToLine(r.line));
    out.appendChild(jump);
  }
  for (const warning of r.warnings || []) out.appendChild(el('div', 'msg warn', `Warning: ${warning}`));

  if (r.id) {
    const facts = el('dl');
    const add = (name, value) => {
      facts.appendChild(el('dt', '', name));
      facts.appendChild(el('dd', '', value));
    };
    add('Handler', r.id);
    add('Enabled', r.enabled ? 'yes' : 'no (the build skips it; a rig and Run still apply it)');
    if (r.new) add('New handler', 'yes');
    add('Why', r.why || '(none)');
    if (r.newNames) add('New names', r.newNames.length ? r.newNames.join(', ') : '(none)');
    if (r.newLiterals !== undefined) add('New literals', String(r.newLiterals));
    out.appendChild(facts);
  }

  renderDiff($('out-sdiff'), r.sourceDiff, r.serverError ? 'No diff: the check did not run.' : 'No difference from the movie\'s source.');
  renderDiff($('out-bdiff'), r.bytecodeDiff, r.ok ? 'No difference from the movie\'s bytecode.' : 'No bytecode: the patch does not compile.');
}

function goToLine(line) {
  const editor = $('editor');
  const lines = editor.value.split('\n');
  let start = 0;
  for (let i = 0; i < line - 1 && i < lines.length; i++) start += lines[i].length + 1;
  editor.focus();
  editor.setSelectionRange(start, start + (lines[line - 1] || '').length);
  const lineHeight = parseFloat(getComputedStyle(editor).lineHeight) || 18;
  editor.scrollTop = Math.max(0, (line - 4) * lineHeight);
}

// ---------------------------------------------------------------------------------------------------
// Run

async function runIt() {
  const evals = $('run-evals').value.split(/\n[ \t]*\n/).map((block) => block.replace(/^\n+|\s+$/g, '')).filter((block) => block.trim() !== '');
  const globals = $('run-globals').value.split(/[\s,]+/).filter((name) => name !== '');
  const results = clear($('run-results'));
  $('run-go').disabled = true;
  $('run-state').textContent = 'Running: the movie is loaded in a native player, this takes a few seconds...';
  try {
    const run = await api('/api/run', { text: $('editor').value, usePatch: $('run-patch').checked, offline: $('run-offline').checked, evals, globals });
    $('run-state').textContent = '';
    if (run.notRun) {
      results.appendChild(el('div', 'msg error', `Not run. ${run.error}`));
      return;
    }
    results.appendChild(el('div', run.ok ? 'msg ok' : 'msg error', run.ok ? 'Run finished: everything succeeded.' : 'Run finished with failures.'));
    const patches = run.patches || {};
    for (const p of patches.results || []) {
      const text = p.ok ? `Patch applied: ${p.id} (${p.stockBytes} -> ${p.bytes} bytes)` : `Patch REFUSED by the player: ${p.id}: ${p.error || ''}`;
      results.appendChild(el('div', p.ok ? 'msg info' : 'msg error', text));
    }
    if ((patches.results || []).length === 0) results.appendChild(el('div', 'msg info', 'No patch applied: the movie ran as it is.'));
    for (const r of run.results || []) {
      const box = el('div', r.ok ? 'result' : 'result bad');
      box.appendChild(el('div', 'what', r.global !== undefined ? `global ${r.global}` : r.eval));
      box.appendChild(el('div', 'value', r.ok ? `=> ${r.value}` : `error: ${r.error}`));
      results.appendChild(box);
    }
  } catch (e) {
    $('run-state').textContent = '';
    results.appendChild(el('div', 'msg error', e.message));
  } finally {
    $('run-go').disabled = false;
  }
}

// ---------------------------------------------------------------------------------------------------
// Rig snippets

function renderRig() {
  const rig = state.config && state.config.rig;
  $('rig-tab').hidden = !rig;
  if (!rig || !state.file) return;
  $('rig-origin').textContent = rig;
  const url = `${state.config.origin}/rig/manifest.json?file=${encodeURIComponent(state.file)}`;
  $('rig-set').value = [
    `fetch(${JSON.stringify(url)}, { cache: "no-store" })`,
    '  .then(async (r) => { const text = await r.text(); if (!r.ok) throw new Error(text); return text; })',
    '  .then((text) => console.log(JSON.parse(__vm.rotl_lingo_set_patches(text))))',
    '  .catch((e) => console.error("lingo patch not applied:", e));',
  ].join('\n');
  $('rig-clear').value = 'console.log(JSON.parse(__vm.rotl_lingo_clear_patches()));';
}

async function copyFrom(id, button) {
  const box = $(id);
  try {
    await navigator.clipboard.writeText(box.value);
    button.textContent = 'Copied';
  } catch {
    // No clipboard permission: select the text so Ctrl+C does it.
    box.focus();
    box.select();
    button.textContent = 'Selected: press Ctrl+C';
  }
  setTimeout(() => (button.textContent = 'Copy'), 2000);
}

// ---------------------------------------------------------------------------------------------------
// Save

async function save() {
  if (!state.file) return;
  const text = $('editor').value;
  const out = clear($('out-saved'));
  selectTab('data-out', 'saved');
  $('save').disabled = true;
  out.appendChild(el('div', 'msg info', 'Checking, then saving...'));
  try {
    const saved = await api('/api/save', { file: state.file, text });
    state.savedText = text;
    renderDirty();
    clear(out);
    out.appendChild(el('div', 'msg ok', `Saved ${saved.bytes} bytes to ${saved.path}`));
    if (state.report && !state.report.ok) out.appendChild(el('div', 'msg warn', 'Saved, but it does not compile yet: the build\'s check will fail until it does.'));
    out.appendChild(el('h3', 'sub', 'To commit it, run by hand (this app never runs git, pushes or deploys):'));
    const commands = el('pre', 'code wrap', saved.git.join('\n'));
    commands.tabIndex = 0;
    out.appendChild(commands);
    loadPatches(); // refreshes the list in its own time; Save does not wait for it
  } catch (e) {
    clear(out);
    out.appendChild(el('div', 'msg error', `Not saved: ${e.message}`));
    for (const problem of (e.data && e.data.problems) || []) out.appendChild(el('div', 'msg error', problem));
  } finally {
    $('save').disabled = false;
  }
}

// ---------------------------------------------------------------------------------------------------
// Start

async function start() {
  $('page-error-close').addEventListener('click', () => ($('page-error').hidden = true));
  for (const button of document.querySelectorAll('button[data-side]')) button.addEventListener('click', () => selectTab('data-side', button.getAttribute('data-side')));
  for (const button of document.querySelectorAll('button[data-out]')) button.addEventListener('click', () => selectTab('data-out', button.getAttribute('data-out')));
  for (const button of document.querySelectorAll('button[data-copy]')) button.addEventListener('click', () => copyFrom(button.getAttribute('data-copy'), button));
  $('script-filter').addEventListener('input', renderScripts);
  $('search-form').addEventListener('submit', runSearch);
  $('patches-refresh').addEventListener('click', loadPatches);
  $('show-bytecode').addEventListener('change', renderHandler);
  $('start-patch').addEventListener('click', startPatch);
  $('why-form').addEventListener('submit', createPatch);
  $('why-cancel').addEventListener('click', () => ($('why-form').hidden = true));
  $('open-existing').addEventListener('click', () => {
    const existing = state.handler ? patchesFor(state.handler.ref) : [];
    if (existing.length === 1) openPatch(existing[0].file);
    else selectTab('data-side', 'patches');
  });
  $('editor').addEventListener('input', onEditorInput);
  $('editor').addEventListener('keydown', onEditorKey);
  $('editor').addEventListener('scroll', () => ($('gutter').scrollTop = $('editor').scrollTop));
  $('save').addEventListener('click', save);
  $('jump-stock').addEventListener('click', () => {
    if (state.report && state.report.id) openHandler(state.report.id);
  });
  $('run-go').addEventListener('click', runIt);
  document.addEventListener('keydown', (event) => {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') {
      event.preventDefault();
      save();
    }
  });
  window.addEventListener('beforeunload', (event) => {
    if (isDirty()) event.preventDefault();
  });

  try {
    state.config = await api('/api/config');
    $('cfg-movie').textContent = state.config.movie;
    $('cfg-patches').textContent = state.config.patches;
    renderRig();
  } catch (e) {
    showPageError(`Cannot read the configuration: ${e.message}`);
    return;
  }
  // The two first reads queue up on the server; neither waits for the other here.
  const scripts = api('/api/scripts').then(
    (list) => {
      state.scripts = list.scripts || [];
      renderScripts();
    },
    (e) => {
      clear($('script-list')).appendChild(el('div', 'msg error', `Cannot list the movie's scripts: ${e.message}`));
    },
  );
  await Promise.all([scripts, loadPatches()]);
}

start();
