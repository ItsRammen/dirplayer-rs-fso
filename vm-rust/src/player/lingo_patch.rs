// Handlers replaced at load: the page supplies Lingo source for some of the movie's handlers, and the
// player compiles each one and puts it in place of the movie's own before any script runs.
//
// The compiler and the checks are in director::lingo::compiler (see patch.rs there). This file is the
// part that knows about a running player: where the manifest comes from, when it is applied, and which
// of the player's own structures have to follow a changed script.
//
// Safety, in the order a patch meets it:
//   1. the handler it names must exist (or the patch must say it adds one);
//   2. the gate: that handler's decompiled source must compile back to the movie's exact bytes;
//   3. the fingerprint the patch carries must be that handler's (so a patch for another build of the
//      movie is skipped, not misapplied);
//   4. the patch's source must compile.
// A patch that fails any of these changes nothing: the movie's handler stays. Each refusal is kept for
// the page to read (`rotl_lingo_status`), written to the console, and announced with a
// `rotl:lingoPatches` event on the window.
//
// A player without this file ignores the manifest and runs the stock movie, as does any player
// when the page supplies none.

use std::cell::RefCell;
use std::rc::Rc;

use serde::{Deserialize, Serialize};

use crate::director::chunks::handler::HandlerDef;
use crate::director::file::get_variable_multiplier;
use crate::director::lingo::compiler::patch::{self, PatchRequest, Refusal};
use crate::player::cast_member::CastMemberType;
use crate::player::script::Script;
use crate::player::symbols::symbol::Symbol;
use crate::player::DirPlayer;

/// Bumped when the manifest format or the exports change in a way a page has to know about.
pub const API_VERSION: u32 = 1;
const MAX_PATCHES: usize = 500;
const MAX_SOURCE_BYTES: usize = 512 * 1024;

/// Behaviour scripts are kept in a cast's script table under their script number plus this (see cast_lib.rs).
const BEHAVIOR_KEY_OFFSET: u32 = 1_000_000;

#[derive(Deserialize, Clone, Debug)]
pub struct Manifest {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub patches: Vec<PatchEntry>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct PatchEntry {
    /// A label for reports; `cast/member/handler` when absent.
    #[serde(default)]
    pub id: Option<String>,
    /// The cast library's name.
    pub cast: String,
    /// The number of the cast member that holds the script.
    pub member: u32,
    pub handler: String,
    pub source: String,
    /// The fingerprint of the handler this patch replaces (the build writes it).
    #[serde(default)]
    pub stock: Option<String>,
    /// The patch adds a handler instead of replacing one.
    #[serde(default, rename = "new")]
    pub new_handler: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct PatchResult {
    pub id: String,
    pub cast: String,
    pub member: u32,
    pub handler: String,
    pub ok: bool,
    /// Why it was refused: `not-found`, `gate`, `movie-changed`, `compile` or `invalid`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Applied after the movie had started, so the stock handler may already have run.
    pub late: bool,
    pub bytes: usize,
    #[serde(rename = "stockBytes")]
    pub stock_bytes: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Status {
    pub api: u32,
    /// `none`: no manifest was given. `pending`: one is waiting for the movie. `applied`: it has been through the movie.
    pub state: &'static str,
    pub applied: usize,
    pub refused: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub results: Vec<PatchResult>,
}

struct Replaced {
    cast_index: usize,
    script_id: u32,
    handler_index: usize,
    /// The movie's own handler; `None` when the patch added a handler.
    original: Option<HandlerDef>,
}

#[derive(Default)]
struct State {
    manifest: Option<String>,
    applied_once: bool,
    error: Option<String>,
    results: Vec<PatchResult>,
    replaced: Vec<Replaced>,
    /// Scripts the patched ones took the place of. A handler that was running when its script was
    /// replaced still points into the old one, so these are kept for as long as the player lives.
    retired: Vec<Rc<Script>>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn status_of(state: &State) -> Status {
    let applied = state.results.iter().filter(|r| r.ok).count();
    Status {
        api: API_VERSION,
        state: if state.manifest.is_none() && !state.applied_once { "none" } else if state.applied_once { "applied" } else { "pending" },
        applied,
        refused: state.results.len() - applied,
        error: state.error.clone(),
        results: state.results.clone(),
    }
}

pub fn status() -> Status {
    STATE.with(|s| status_of(&s.borrow()))
}

pub fn status_json() -> String {
    serde_json::to_string(&status()).unwrap_or_else(|_| "{}".to_string())
}

fn movie_is_loaded(player: &DirPlayer) -> bool {
    player.movie.cast_manager.casts.iter().any(|c| c.lctx.is_some())
}

fn script_id_of_member(cast: &crate::player::cast_lib::CastLib, member: u32) -> Option<u32> {
    let m = cast.members.get(&member)?;
    match &m.member_type {
        CastMemberType::Script(s) => Some(s.script_id),
        _ => m.get_script_id(),
    }
}

/// Which entries of a cast's script table were built from script `script_id`.
fn script_table_keys(cast: &crate::player::cast_lib::CastLib, script_id: u32) -> Vec<u32> {
    cast.scripts
        .keys()
        .copied()
        .filter(|key| if *key >= BEHAVIOR_KEY_OFFSET { *key - BEHAVIOR_KEY_OFFSET == script_id } else { script_id_of_member(cast, *key) == Some(script_id) })
        .collect()
}

/// Rebuilds the player's `Script` objects for one script after its chunk changed.
fn refresh_scripts(player: &mut DirPlayer, cast_index: usize, script_id: u32, retired: &mut Vec<Rc<Script>>) {
    let cast = &mut player.movie.cast_manager.casts[cast_index];
    let Some(lctx) = cast.lctx.as_ref() else { return };
    let Some(chunk) = lctx.scripts.get(&script_id) else { return };
    let names = &lctx.names;
    let mut rebuilt = Vec::new();
    for key in script_table_keys(cast, script_id) {
        let old = cast.scripts[&key].clone();
        let mut script: Script = (*old).clone();
        script.chunk = chunk.clone();
        script.handlers.clear();
        script.handler_names.clear();
        script.handler_names_raw.clear();
        for (index, handler) in chunk.handlers.iter().enumerate() {
            let name = match names.get(handler.name_id as usize) {
                Some(n) => n.clone(),
                None => format!("__anon_handler_{}", index),
            };
            let symbol = Symbol::from_str(&name);
            script.handlers.insert(symbol, Rc::new(handler.clone()));
            script.handler_names.push(symbol);
            script.handler_names_raw.push(name);
        }
        rebuilt.push((key, Rc::new(script)));
        retired.push(old);
    }
    for (key, script) in rebuilt {
        cast.scripts.insert(key, script);
    }
    player.movie.cast_manager.clear_movie_script_cache();
}

fn apply_entry(player: &mut DirPlayer, entry: &PatchEntry, state: &mut State) -> Result<(usize, usize, Vec<String>), Refusal> {
    if entry.source.len() > MAX_SOURCE_BYTES {
        return Err(Refusal::Invalid("the patch's source is too large".to_string()));
    }
    let casts = &mut player.movie.cast_manager.casts;
    let cast_index = casts.iter().position(|c| c.name.eq_ignore_ascii_case(&entry.cast)).ok_or_else(|| Refusal::NotFound(format!("the movie has no cast library \"{}\"", entry.cast)))?;
    let cast = &mut casts[cast_index];
    let script_id = script_id_of_member(cast, entry.member).ok_or_else(|| Refusal::NotFound(format!("member {} of \"{}\" has no script", entry.member, entry.cast)))?;
    let version = cast.dir_version;
    let multiplier = get_variable_multiplier(cast.capital_x, cast.dir_version);
    let lctx = cast.lctx.as_mut().ok_or_else(|| Refusal::NotFound(format!("cast library \"{}\" has no scripts", entry.cast)))?;

    let request = PatchRequest { handler: &entry.handler, source: &entry.source, stock: entry.stock.as_deref(), new_handler: entry.new_handler };
    let prepared = patch::prepare(lctx, script_id, &request, version, multiplier)?;
    let original = patch::commit(lctx, &prepared);
    for name in &prepared.names[prepared.names_before..] {
        cast.name_symbols.push(Symbol::from_str(name));
    }
    state.replaced.push(Replaced { cast_index, script_id, handler_index: prepared.handler_index, original });
    let mut retired = std::mem::take(&mut state.retired);
    refresh_scripts(player, cast_index, script_id, &mut retired);
    state.retired = retired;
    Ok((prepared.compiled_len, prepared.stock_len, prepared.warnings))
}

fn apply_manifest(player: &mut DirPlayer, state: &mut State, late: bool) {
    state.results.clear();
    state.error = None;
    let Some(text) = state.manifest.clone() else { return };
    let manifest: Manifest = match serde_json::from_str(&text) {
        Ok(m) => m,
        Err(e) => {
            state.error = Some(format!("the Lingo patch manifest is not valid JSON: {}", e));
            return;
        }
    };
    if manifest.version != API_VERSION {
        state.error = Some(format!("the Lingo patch manifest is version {}; this player reads version {}", manifest.version, API_VERSION));
        return;
    }
    if manifest.patches.len() > MAX_PATCHES {
        state.error = Some(format!("the Lingo patch manifest has {} patches; the limit is {}", manifest.patches.len(), MAX_PATCHES));
        return;
    }
    for entry in &manifest.patches {
        let id = entry.id.clone().unwrap_or_else(|| format!("{}/{}/{}", entry.cast, entry.member, entry.handler));
        let mut result = PatchResult { id, cast: entry.cast.clone(), member: entry.member, handler: entry.handler.clone(), ok: false, refused: None, error: None, late, bytes: 0, stock_bytes: 0, warnings: Vec::new() };
        match apply_entry(player, entry, state) {
            Ok((bytes, stock_bytes, warnings)) => {
                result.ok = true;
                result.bytes = bytes;
                result.stock_bytes = stock_bytes;
                result.warnings = warnings;
            }
            Err(refusal) => {
                result.refused = Some(refusal.kind().to_string());
                result.error = Some(refusal.to_string());
            }
        }
        state.results.push(result);
    }
    state.applied_once = true;
}

/// Puts every replaced handler back to the movie's own. Names and literals the patches added stay; nothing uses them.
fn revert(player: &mut DirPlayer, state: &mut State) {
    let replaced = std::mem::take(&mut state.replaced);
    let mut retired = std::mem::take(&mut state.retired);
    for item in replaced.into_iter().rev() {
        let Some(cast) = player.movie.cast_manager.casts.get_mut(item.cast_index) else { continue };
        let Some(chunk) = cast.lctx.as_mut().and_then(|l| l.scripts.get_mut(&item.script_id)) else { continue };
        match item.original {
            Some(original) => {
                if item.handler_index < chunk.handlers.len() {
                    chunk.handlers[item.handler_index] = original;
                }
            }
            None => {
                if item.handler_index + 1 == chunk.handlers.len() {
                    chunk.handlers.pop();
                }
            }
        }
        refresh_scripts(player, item.cast_index, item.script_id, &mut retired);
    }
    state.retired = retired;
    state.results.clear();
}

#[cfg(target_arch = "wasm32")]
fn manifest_from_page() -> Option<String> {
    // A page that wants its patches in before the movie starts sets this global before the player
    // script loads, so there is nothing to race.
    let value = js_sys::Reflect::get(&js_sys::global(), &wasm_bindgen::JsValue::from_str("__rotlLingoPatches")).ok()?;
    if let Some(text) = value.as_string() {
        return Some(text);
    }
    if value.is_object() {
        return js_sys::JSON::stringify(&value).ok().and_then(|s| s.as_string());
    }
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn manifest_from_page() -> Option<String> {
    None
}

#[cfg(target_arch = "wasm32")]
fn announce(status: &Status) {
    use wasm_bindgen::JsValue;
    for result in status.results.iter().filter(|r| !r.ok) {
        web_sys::console::warn_1(&JsValue::from_str(&format!("Lingo patch {} refused ({}): {}", result.id, result.refused.as_deref().unwrap_or("?"), result.error.as_deref().unwrap_or(""))));
    }
    if let Some(error) = &status.error {
        web_sys::console::warn_1(&JsValue::from_str(error));
    }
    let Some(window) = web_sys::window() else { return };
    let init = web_sys::CustomEventInit::new();
    init.set_detail(&JsValue::from_str(&serde_json::to_string(status).unwrap_or_default()));
    if let Ok(event) = web_sys::CustomEvent::new_with_event_init_dict("rotl:lingoPatches", &init) {
        let _ = window.dispatch_event(&event);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn announce(_status: &Status) {}

/// Called when a movie's casts have just been loaded, before any of its scripts runs.
pub fn apply_at_load(player: &mut DirPlayer) {
    let status = STATE.with(|s| {
        let mut state = s.borrow_mut();
        // A new movie: whatever was replaced belonged to the casts that are gone.
        state.replaced.clear();
        if state.manifest.is_none() {
            state.manifest = manifest_from_page();
        }
        if state.manifest.is_none() {
            return None;
        }
        apply_manifest(player, &mut state, false);
        Some(status_of(&state))
    });
    if let Some(status) = status {
        announce(&status);
    }
}

/// The page hands over a manifest. Before the movie is loaded it waits for the movie; after, it is
/// applied at once (and marked late), replacing whatever an earlier manifest had put in.
pub fn set_patches(player: &mut DirPlayer, manifest_json: &str) -> Status {
    let (status, applied_now) = STATE.with(|s| {
        let mut state = s.borrow_mut();
        state.manifest = Some(manifest_json.to_string());
        if !movie_is_loaded(player) {
            state.applied_once = false;
            state.results.clear();
            state.error = None;
            return (status_of(&state), false);
        }
        revert(player, &mut state);
        apply_manifest(player, &mut state, true);
        (status_of(&state), true)
    });
    if applied_now {
        announce(&status);
    }
    status
}

/// Back to the stock movie, and forget the manifest.
pub fn clear_patches(player: &mut DirPlayer) -> Status {
    STATE.with(|s| {
        let mut state = s.borrow_mut();
        revert(player, &mut state);
        state.manifest = None;
        state.applied_once = false;
        state.error = None;
        status_of(&state)
    })
}
