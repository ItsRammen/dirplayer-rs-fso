// Replacing one handler of a loaded script with a compiled one.
//
// This works on a cast's script context (names and script chunks), which is the same structure whether
// it was just read from a file or sits inside a running player. The player layer on top
// (player::lingo_patch) keeps its own caches in step.

use fxhash::FxHashMap;

use crate::director::chunks::handler::{Bytecode, HandlerDef};
use crate::director::chunks::script::ScriptChunk;
use crate::director::lingo::datum::Datum;
use crate::director::lingo::opcode::OpCode;
use crate::director::lingo::script::ScriptContext;

use super::sha256::sha256_hex;
use super::stock;
use super::{compile_handler, Literal, NameTable, ScriptEnv};

/// Why a patch was not applied. The text of each is meant to be shown to whoever maintains the patch.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    /// The cast, script or handler the patch names is not in this movie.
    NotFound(String),
    /// The handler's own source does not compile back to its bytes, so the compiler cannot be trusted with it.
    Gate(String),
    /// The handler is not the one the patch was written against (another build of the movie).
    MovieChanged { expected: String, found: String },
    /// The patch's source does not compile.
    Compile { line: u32, col: u32, message: String },
    /// Anything else: a malformed patch, an unsupported movie version.
    Invalid(String),
}

impl Refusal {
    pub fn kind(&self) -> &'static str {
        match self {
            Refusal::NotFound(_) => "not-found",
            Refusal::Gate(_) => "gate",
            Refusal::MovieChanged { .. } => "movie-changed",
            Refusal::Compile { .. } => "compile",
            Refusal::Invalid(_) => "invalid",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NotFound(what) => write!(f, "{}", what),
            Refusal::Gate(why) => write!(f, "this handler cannot be replaced: {}", why),
            Refusal::MovieChanged { expected, found } => write!(f, "the movie's handler is not the one this patch was written for (patch says {}, movie has {})", short(expected), short(found)),
            Refusal::Compile { line, col, message } => match (line, col) {
                (0, _) => write!(f, "{}", message),
                (line, 0) => write!(f, "line {}: {}", line, message),
                (line, col) => write!(f, "line {}, column {}: {}", line, col, message),
            },
            Refusal::Invalid(why) => write!(f, "{}", why),
        }
    }
}

fn short(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
}

/// A handler's fingerprint: its decompiled source and its bytes. A patch carries the fingerprint of the
/// handler it replaces, so it is skipped, not misapplied, on any other build of the movie.
pub fn fingerprint(source: &str, bytes: &[u8]) -> String {
    let mut data = source.as_bytes().to_vec();
    data.push(0);
    data.extend_from_slice(bytes);
    sha256_hex(&data)
}

pub struct PatchRequest<'a> {
    pub handler: &'a str,
    pub source: &'a str,
    /// The fingerprint of the handler being replaced. `None` skips that check (the round-trip gate still applies).
    pub stock: Option<&'a str>,
    /// The patch adds a handler the script does not have, instead of replacing one.
    pub new_handler: bool,
}

/// A patch that passed every check, ready to be put in place. Nothing is changed until `commit`.
pub struct Prepared {
    pub script_id: u32,
    /// Position in the script's handler list; equal to the list's length for a new handler.
    pub handler_index: usize,
    pub is_new: bool,
    pub handler: HandlerDef,
    /// The whole name table after the patch; names past `names_before` are new.
    pub names: Vec<String>,
    pub names_before: usize,
    /// Literals to append to the script's table.
    pub new_literals: Vec<Literal>,
    pub stock_len: usize,
    pub compiled_len: usize,
    pub stock_fingerprint: Option<String>,
    pub warnings: Vec<String>,
}

/// Decodes bytecode bytes into the instruction list the player runs.
pub fn decode_bytecode(bytes: &[u8]) -> Result<(Vec<Bytecode>, FxHashMap<usize, usize>), String> {
    let mut array = Vec::new();
    let mut index_map = FxHashMap::default();
    let mut i = 0;
    while i < bytes.len() {
        let pos = i;
        let op = bytes[i] as u16;
        i += 1;
        let base = if op >= 0x40 { 0x40 + op % 0x40 } else { op };
        let opcode: OpCode = num::FromPrimitive::from_u16(base).ok_or_else(|| format!("unknown instruction {:#04x} at byte {}", op, pos))?;
        let mut obj: i64 = 0;
        if op >= 0xc0 {
            let b = bytes.get(i..i + 4).ok_or("the bytecode ends inside an instruction")?;
            obj = i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as i64;
            i += 4;
        } else if op >= 0x80 {
            let b = bytes.get(i..i + 2).ok_or("the bytecode ends inside an instruction")?;
            let raw = u16::from_be_bytes([b[0], b[1]]);
            obj = match opcode {
                OpCode::PushInt16 | OpCode::PushInt8 => raw as i16 as i64,
                _ => raw as i64,
            };
            i += 2;
        } else if op >= 0x40 {
            let b = *bytes.get(i).ok_or("the bytecode ends inside an instruction")?;
            obj = if opcode == OpCode::PushInt8 { b as i8 as i64 } else { b as i64 };
            i += 1;
        }
        index_map.insert(pos, array.len());
        array.push(Bytecode::new(opcode, obj, pos));
    }
    Ok((array, index_map))
}

fn find_handler(chunk: &ScriptChunk, lctx: &ScriptContext, name: &str) -> Option<usize> {
    chunk.handlers.iter().position(|h| lctx.names.get(h.name_id as usize).map(|n| n.eq_ignore_ascii_case(name)).unwrap_or(false))
}

/// Checks a patch against a script and compiles it. `version` and `multiplier` are the cast's.
pub fn prepare(lctx: &ScriptContext, script_id: u32, request: &PatchRequest, version: u16, multiplier: u32) -> Result<Prepared, Refusal> {
    if multiplier != 1 {
        return Err(Refusal::Invalid(format!("this movie's Director version ({}) is older than the compiler supports", version)));
    }
    let chunk = lctx.scripts.get(&script_id).ok_or_else(|| Refusal::NotFound(format!("the cast has no script {}", script_id)))?;
    let found = find_handler(chunk, lctx, request.handler);
    let mut stock_fingerprint = None;
    let mut stock_len = 0;
    let handler_index = match (found, request.new_handler) {
        (Some(_), true) => return Err(Refusal::Invalid(format!("the patch says `{}` is a new handler, but the script already has one of that name", request.handler))),
        (None, false) => return Err(Refusal::NotFound(format!("the script has no handler `{}`", request.handler))),
        (None, true) => chunk.handlers.len(),
        (Some(index), false) => {
            // The gate: the handler's own source must compile back to its own bytes.
            let trip = stock::round_trip(&chunk.handlers[index], chunk, lctx, version, multiplier);
            if let Some(why) = trip.refusal() {
                return Err(Refusal::Gate(why));
            }
            let found_print = fingerprint(&trip.source, &trip.stock);
            if let Some(expected) = request.stock {
                if !expected.eq_ignore_ascii_case(&found_print) {
                    return Err(Refusal::MovieChanged { expected: expected.to_string(), found: found_print });
                }
            }
            stock_len = trip.stock.len();
            stock_fingerprint = Some(found_print);
            index
        }
    };

    let mut names = NameTable::new(lctx.names.clone());
    let names_before = names.names.len();
    let mut literals = stock::literals_of(chunk);
    let literals_before = literals.len();
    let mut handlers = stock::handler_names(chunk, lctx);
    if request.new_handler {
        // A new handler may call itself.
        handlers.push(request.handler.to_string());
    }
    let properties = stock::property_names(chunk, lctx);
    let script_globals = stock::script_global_names(chunk, lctx);
    let default_args: Vec<String> = match found {
        Some(index) => chunk.handlers[index].argument_name_ids.iter().filter_map(|id| lctx.names.get(*id as usize).cloned()).collect(),
        None => Vec::new(),
    };
    let compiled = {
        let mut env = ScriptEnv { names: &mut names, literals: &mut literals, handlers: &handlers, properties: &properties, script_globals: &script_globals };
        compile_handler(request.source, &mut env, &default_args).map_err(|e| Refusal::Compile { line: e.line, col: e.col, message: e.message })?
    };
    if let Some(name) = &compiled.name {
        if !name.eq_ignore_ascii_case(request.handler) {
            return Err(Refusal::Invalid(format!("the source defines `on {}`, but the patch is for `{}`", name, request.handler)));
        }
    } else if request.new_handler {
        return Err(Refusal::Invalid("a new handler's source must start with its `on name` line".to_string()));
    }
    if names.names.len() > u16::MAX as usize {
        return Err(Refusal::Invalid("the cast's name table is full".to_string()));
    }

    let (bytecode_array, bytecode_index_map) = decode_bytecode(&compiled.bytecode).map_err(Refusal::Invalid)?;
    let name_id = names.intern(request.handler);
    let ids = |list: &[String], names: &mut NameTable| -> Vec<u16> { list.iter().map(|n| names.intern(n)).collect() };
    let argument_name_ids = ids(&compiled.args, &mut names);
    let local_name_ids = ids(&compiled.locals, &mut names);
    let global_name_ids = ids(&compiled.globals, &mut names);
    let handler = HandlerDef { name_id, bytecode_array, bytecode_index_map, argument_name_ids, local_name_ids, global_name_ids, compiled_ir: std::cell::RefCell::new(None) };

    Ok(Prepared {
        script_id,
        handler_index,
        is_new: request.new_handler,
        handler,
        names: names.names,
        names_before,
        new_literals: literals.split_off(literals_before),
        stock_len,
        compiled_len: compiled.bytecode.len(),
        stock_fingerprint,
        warnings: compiled.warnings,
    })
}

pub fn literal_datum(literal: &Literal) -> Datum {
    match literal {
        Literal::Str(s) => Datum::String(s.clone()),
        Literal::Float(f) => Datum::Float(*f),
        Literal::Int(i) => Datum::Int(*i),
        Literal::Other => Datum::Void,
    }
}

/// Puts a prepared patch in place in the script context and returns the handler it replaced (if any).
pub fn commit(lctx: &mut ScriptContext, prepared: &Prepared) -> Option<HandlerDef> {
    lctx.names = prepared.names.clone();
    let chunk = lctx.scripts.get_mut(&prepared.script_id)?;
    for literal in &prepared.new_literals {
        chunk.literals.push(literal_datum(literal));
    }
    if prepared.handler_index < chunk.handlers.len() {
        Some(std::mem::replace(&mut chunk.handlers[prepared.handler_index], prepared.handler.clone()))
    } else {
        chunk.handlers.push(prepared.handler.clone());
        None
    }
}
