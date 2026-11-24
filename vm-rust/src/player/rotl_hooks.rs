//! Lingo "after" hooks (ROTL).
//!
//! The page can register a few lines of Lingo to run each time one of the movie's handlers returns, for example after a handler
//! that works out what a character looks like:
//!
//! ```text
//! rotl_after_handler("My Script:WorkOutLook", "if Worn contains \"Hat\" then HeadPicture = \"Hat \" & MyHead")
//! ```
//!
//! What a hook is:
//! - **Keyed by script and handler**: `"<script member name>:<handler>"`, or `"<cast name>#<member number>:<handler>"` for a script with no
//!   name. Names compare without regard to case, as Lingo's do. `mouseUp` and `exitFrame` exist in hundreds of scripts; a key names one.
//! - **Parsed once**, when registered. A line the evaluator cannot read is refused then, with the reason, and nothing is registered.
//! - **Run after the handler's frame is torn down**, before its caller resumes, so it sees the globals the handler left. The movie's own
//!   bytecode always runs first and is never changed: a hook can only add to what the movie did.
//! - **Given the handler's arguments by their own names** (`x` in `on Draw x`): the lines run in a scope of their own,
//!   with the handler's arguments and no locals, so every other bare name is a global. Assignments go to globals.
//! - One statement a line, through the same evaluator as `do "..."` and the console (`eval.rs`): assignments, `put`, `set ... to`,
//!   handler calls, the one-line `if ... then ...`. No `repeat`, no multi-line `if`.
//!
//! Rules:
//! - While a hook runs, no other hook runs (a hook that calls a hooked handler does not loop).
//! - A line that fails stops that hook; the error is reported to the page (`window.__rotlHookError(key, message)`) and kept for
//!   `rotl_hooks_info`; the movie carries on as if there were no hook.
//! - With no hook registered the cost is one thread-local read per handler return; with hooks, one hash lookup by the handler's
//!   lower-case name (no allocation), and the script's name only when that handler has a hook.
//!
//! What a game lets its hooks do is the game's own rule (they change this client only; the server still decides).
//!
//! Tested on a real movie from the game's repository, through `rotl-lingo run --hook` (src/tools/rotl_lingo.rs): the fork has no
//! movie and names no handler of one. Being replaced by compiled handler patches (`lingo_patch.rs`), which do the same in bytecode.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::player::cast_lib::CastMemberRef;
use crate::player::eval::{eval_lingo_expr_ast_runtime, parse_lingo_command_ast, LingoExpr};
use crate::player::symbols::symbol::Symbol;
use crate::player::{reserve_player_mut, DatumRef};

/// The page's way to tell which hook API a player has. Bumped when the API changes.
pub const VERSION: u32 = 1;
/// A hook is a few lines, not a program.
pub const MAX_LINES: usize = 400;
pub const MAX_HOOKS: usize = 64;
const MAX_ERRORS_KEPT: usize = 20;

struct Hook {
    key: String,
    /// The script's name, lower case (or `cast#number`).
    script: String,
    lines: Rc<Vec<LingoExpr>>,
}

thread_local! {
    /// Handler name (lower case) -> the hooks on it, one per script.
    static AFTER: RefCell<HashMap<String, Vec<Hook>>> = RefCell::new(HashMap::new());
    static ANY: Cell<bool> = Cell::new(false);
    static RUNNING: Cell<bool> = Cell::new(false);
    static RUNS: Cell<u64> = Cell::new(0);
    static ERRORS: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
    static ERROR_COUNT: Cell<u64> = Cell::new(0);
}

/// `"Script:Handler"` as (script, handler), both trimmed and lower case. The handler is after the LAST colon (script names may have one).
pub fn split_key(key: &str) -> Result<(String, String), String> {
    let (script, handler) = key.rsplit_once(':').ok_or_else(|| format!("hook key \"{}\" is not \"Script:handler\"", key))?;
    let (script, handler) = (script.trim(), handler.trim());
    if script.is_empty() || handler.is_empty() {
        return Err(format!("hook key \"{}\" is not \"Script:handler\"", key));
    }
    if !handler.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("\"{}\" is not a handler name", handler));
    }
    Ok((script.to_ascii_lowercase(), handler.to_ascii_lowercase()))
}

/// The lines of a hook, parsed. Empty lines and `--` comments are skipped.
pub fn parse_lines(lingo: &str) -> Result<Vec<LingoExpr>, String> {
    let lines: Vec<&str> = lingo
        .split(|c| c == '\r' || c == '\n')
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with("--"))
        .collect();
    if lines.len() > MAX_LINES {
        return Err(format!("{} lines; a hook has at most {}", lines.len(), MAX_LINES));
    }
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| parse_lingo_command_ast(line).map_err(|e| format!("line {} `{}`: {}", i + 1, line, e.message)))
        .collect()
}

/// Register the Lingo to run after the handler named by `key` (`"Script:handler"`) returns, replacing any hook on that key; empty `lingo`
/// removes it. Returns the number of lines registered (0 when removed).
pub fn set_after(key: &str, lingo: &str) -> Result<usize, String> {
    let (script, handler) = split_key(key)?;
    let lines = if lingo.trim().is_empty() { Vec::new() } else { parse_lines(lingo)? };
    let count = lines.len();
    AFTER.with(|map| {
        let mut map = map.borrow_mut();
        let total: usize = map.values().map(Vec::len).sum();
        let list = map.entry(handler.clone()).or_default();
        let at = list.iter().position(|h| h.script == script);
        if count == 0 {
            if let Some(at) = at {
                list.remove(at);
            }
        } else {
            let hook = Hook { key: key.trim().to_string(), script, lines: Rc::new(lines) };
            match at {
                Some(at) => list[at] = hook,
                None if total >= MAX_HOOKS => return Err(format!("at most {} hooks", MAX_HOOKS)),
                None => list.push(hook),
            }
        }
        if list.is_empty() {
            map.remove(&handler);
        }
        ANY.with(|any| any.set(!map.is_empty()));
        Ok(count)
    })
}

pub fn clear() {
    AFTER.with(|map| map.borrow_mut().clear());
    ANY.with(|any| any.set(false));
}

/// The keys registered, in no particular order.
pub fn keys() -> Vec<String> {
    AFTER.with(|map| map.borrow().values().flat_map(|l| l.iter().map(|h| h.key.clone())).collect())
}

/// How many times hooks have run, and the last errors (key, message), oldest first.
pub fn runs() -> u64 {
    RUNS.with(Cell::get)
}
pub fn error_count() -> u64 {
    ERROR_COUNT.with(Cell::get)
}
pub fn recent_errors() -> Vec<(String, String)> {
    ERRORS.with(|e| e.borrow().clone())
}

/// Cheap check at every handler return: is there any hook on a handler of this name (and none running)?
#[inline]
pub fn wants(handler: &Symbol) -> bool {
    ANY.with(Cell::get) && !RUNNING.with(Cell::get) && AFTER.with(|map| map.borrow().contains_key(handler.as_lower_str()))
}

/// What a hook needs from the handler's frame, taken before the frame is torn down.
pub struct Returning {
    pub script: CastMemberRef,
    pub handler_name_id: u16,
    pub args: Vec<DatumRef>,
}

fn script_names(script: &CastMemberRef) -> (String, String) {
    reserve_player_mut(|player| {
        let name = player.movie.cast_manager.find_member_by_ref(script).map(|m| m.name.to_ascii_lowercase()).unwrap_or_default();
        let cast = player
            .movie
            .cast_manager
            .get_cast_or_null(script.cast_lib.max(0) as u32)
            .map(|c| c.name.to_ascii_lowercase())
            .unwrap_or_default();
        (name, format!("{}#{}", cast, script.cast_member))
    })
}

fn find(handler: &Symbol, script: &CastMemberRef) -> Option<(String, Rc<Vec<LingoExpr>>)> {
    let (name, numbered) = script_names(script);
    AFTER.with(|map| {
        map.borrow()
            .get(handler.as_lower_str())?
            .iter()
            .find(|h| (!name.is_empty() && h.script == name) || h.script == numbered)
            .map(|h| (h.key.clone(), h.lines.clone()))
    })
}

fn report(key: &str, message: &str) {
    log::warn!("rotl after-hook {} failed: {}", key, message);
    ERROR_COUNT.with(|c| c.set(c.get() + 1));
    ERRORS.with(|e| {
        let mut e = e.borrow_mut();
        if e.len() >= MAX_ERRORS_KEPT {
            e.remove(0);
        }
        e.push((key.to_string(), message.to_string()));
    });
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::{JsCast, JsValue};
        let Some(window) = web_sys::window() else { return };
        let Ok(function) = js_sys::Reflect::get(&window, &JsValue::from_str("__rotlHookError")) else { return };
        let Some(function) = function.dyn_ref::<js_sys::Function>() else { return };
        let _ = function.call2(&JsValue::NULL, &JsValue::from_str(key), &JsValue::from_str(message));
    }
}

/// Run the after hook for this handler of this script, if one is registered. Errors are reported, never returned.
pub async fn run_after(handler: &Symbol, returning: Returning) {
    let Some((key, lines)) = find(handler, &returning.script) else { return };
    RUNNING.with(|r| r.set(true));
    RUNS.with(|r| r.set(r.get() + 1));
    // A scope of its own: the handler's arguments by name, no locals, no receiver, so other names are globals.
    let (scope_ref, depth) = reserve_player_mut(|player| {
        let scope_ref = player.push_scope();
        let scope = player.scopes.get_mut(scope_ref).unwrap();
        scope.script_ref = returning.script.clone();
        scope.handler_name_id = returning.handler_name_id;
        scope.args = returning.args;
        scope.receiver = None;
        (scope_ref, player.scope_count)
    });
    let mut failed = None;
    for (i, line) in lines.iter().enumerate() {
        if let Err(err) = Box::pin(eval_lingo_expr_ast_runtime(line)).await {
            failed = Some(format!("line {}: {}", i + 1, err.message));
            break;
        }
    }
    reserve_player_mut(|player| {
        // Our scope is the top one again once every handler the hook called has returned.
        if player.scope_count == depth && player.current_scope_ref() == scope_ref {
            player.pop_scope();
        } else {
            log::warn!("rotl after-hook {}: scope stack moved ({} -> {}); not popping", key, depth, player.scope_count);
        }
    });
    RUNNING.with(|r| r.set(false));
    if let Some(message) = failed {
        report(&key, &message);
    }
}
