// The movie's own handlers, as the compiler sees them: their bytes, their source, and the round trip
// between the two.

use crate::director::chunks::handler::{Bytecode, HandlerDef};
use crate::director::chunks::script::ScriptChunk;
use crate::director::lingo::datum::Datum;
use crate::director::lingo::decompiler::decompile_handler;
use crate::director::lingo::script::ScriptContext;

use super::{compile_handler, CompiledHandler, Literal, NameTable, ScriptEnv};

/// A handler's instructions as the bytes they were read from.
///
/// The player keeps decoded instructions, not the bytes. Each instruction's width is the distance to
/// the next one, so the encoding below gives back the file's bytes exactly.
pub fn handler_bytes(handler: &HandlerDef) -> Vec<u8> {
    let code = &handler.bytecode_array;
    let mut out = Vec::new();
    for (i, bc) in code.iter().enumerate() {
        let op = num::ToPrimitive::to_u8(&bc.opcode).unwrap_or(0);
        let width = match code.get(i + 1) {
            Some(next) => next.pos.saturating_sub(bc.pos),
            None => natural_width(bc, op),
        };
        match width {
            2 => {
                out.push(op);
                out.push(bc.obj as u8);
            }
            3 => {
                out.push(op + 0x40);
                out.extend_from_slice(&(bc.obj as u16).to_be_bytes());
            }
            5 => {
                out.push(op + 0x80);
                out.extend_from_slice(&(bc.obj as u32).to_be_bytes());
            }
            _ => out.push(op),
        }
    }
    out
}

fn natural_width(bc: &Bytecode, op: u8) -> usize {
    if op < 0x40 {
        1
    } else if bc.obj < 0x100 {
        2
    } else if bc.obj < 0x10000 {
        3
    } else {
        5
    }
}

pub fn literals_of(chunk: &ScriptChunk) -> Vec<Literal> {
    chunk
        .literals
        .iter()
        .map(|l| match l {
            Datum::String(s) => Literal::Str(s.clone()),
            Datum::Float(f) => Literal::Float(*f as f64),
            Datum::Int(i) => Literal::Int(*i),
            _ => Literal::Other,
        })
        .collect()
}

fn name_of(lctx: &ScriptContext, id: u16) -> String {
    lctx.names.get(id as usize).cloned().unwrap_or_else(|| format!("UNKNOWN_NAME_{}", id))
}

pub fn handler_names(chunk: &ScriptChunk, lctx: &ScriptContext) -> Vec<String> {
    chunk.handlers.iter().map(|h| name_of(lctx, h.name_id)).collect()
}

pub fn script_global_names(chunk: &ScriptChunk, lctx: &ScriptContext) -> Vec<String> {
    chunk.global_name_ids.iter().map(|id| name_of(lctx, *id)).collect()
}

pub fn property_names(chunk: &ScriptChunk, lctx: &ScriptContext) -> Vec<String> {
    chunk.property_name_ids.iter().map(|id| name_of(lctx, *id)).collect()
}

/// The source of a handler as the compiler reads it back: `on name args`, its globals, the body, `end`.
pub fn handler_source(handler: &HandlerDef, chunk: &ScriptChunk, lctx: &ScriptContext, version: u16, multiplier: u32) -> String {
    let decompiled = decompile_handler(handler, chunk, lctx, version, multiplier);
    let mut out = String::new();
    out.push_str("on ");
    out.push_str(&decompiled.name);
    if !decompiled.arguments.is_empty() {
        out.push(' ');
        out.push_str(&decompiled.arguments.join(", "));
    }
    out.push('\n');
    if !handler.global_name_ids.is_empty() {
        out.push_str("  global ");
        // A few movies list a name number that the name table does not have; those are skipped.
        let globals: Vec<String> = handler.global_name_ids.iter().filter(|id| (**id as usize) < lctx.names.len()).map(|id| name_of(lctx, *id)).collect();
        out.push_str(&globals.join(", "));
        out.push('\n');
    }
    for line in &decompiled.lines {
        for _ in 0..=line.indent {
            out.push_str("  ");
        }
        out.push_str(&line.text);
        out.push('\n');
    }
    out.push_str("end\n");
    out
}

#[derive(Clone, Debug)]
pub struct RoundTrip {
    pub source: String,
    pub stock: Vec<u8>,
    /// `Ok` when the source compiles; the bytes may still differ.
    pub compiled: Result<CompiledHandler, String>,
    /// Empty when the recompiled handler is identical to the movie's.
    pub differences: Vec<String>,
}

impl RoundTrip {
    pub fn identical(&self) -> bool {
        self.compiled.is_ok() && self.differences.is_empty()
    }

    /// One line saying why the handler is not open for editing, or `None` when it is.
    pub fn refusal(&self) -> Option<String> {
        match &self.compiled {
            Err(e) => Some(format!("its own source does not compile ({})", e)),
            Ok(_) if !self.differences.is_empty() => Some(format!("its own source does not compile back to the movie's bytes ({})", self.differences.join("; "))),
            Ok(_) => None,
        }
    }
}

/// Decompiles a handler, compiles the text again, and compares the result with the handler's own
/// bytes, argument names and local names.
///
/// This is the gate for changing a handler: only one that survives this unchanged may be replaced, since
/// only then is the compiler known to read every construct in it the way Director wrote it.
pub fn round_trip(handler: &HandlerDef, chunk: &ScriptChunk, lctx: &ScriptContext, version: u16, multiplier: u32) -> RoundTrip {
    let source = handler_source(handler, chunk, lctx, version, multiplier);
    let stock = handler_bytes(handler);
    if multiplier != 1 {
        return RoundTrip { source, stock, compiled: Err(format!("this movie's Director version ({}) is older than the compiler supports", version)), differences: Vec::new() };
    }
    let mut names = NameTable::new(lctx.names.clone());
    let mut literals = literals_of(chunk);
    let handlers = handler_names(chunk, lctx);
    let properties = property_names(chunk, lctx);
    let script_globals = script_global_names(chunk, lctx);
    let names_before = names.names.len();
    let literals_before = literals.len();
    let mut env = ScriptEnv { names: &mut names, literals: &mut literals, handlers: &handlers, properties: &properties, script_globals: &script_globals };
    let compiled = match compile_handler(&source, &mut env, &[]) {
        Ok(c) => c,
        Err(e) => return RoundTrip { source, stock, compiled: Err(e.to_string()), differences: Vec::new() },
    };
    let mut differences = Vec::new();
    if compiled.bytecode != stock {
        let at = compiled.bytecode.iter().zip(stock.iter()).position(|(a, b)| a != b).unwrap_or(compiled.bytecode.len().min(stock.len()));
        differences.push(format!("bytecode differs at byte {} ({} bytes compiled, {} in the movie)", at, compiled.bytecode.len(), stock.len()));
    }
    let stock_args: Vec<String> = handler.argument_name_ids.iter().map(|id| name_of(lctx, *id)).collect();
    if !same_names(&compiled.args, &stock_args) {
        differences.push(format!("arguments differ: {:?} compiled, {:?} in the movie", compiled.args, stock_args));
    }
    let stock_locals: Vec<String> = handler.local_name_ids.iter().map(|id| name_of(lctx, *id)).collect();
    if !same_names(&compiled.locals, &stock_locals) {
        differences.push(format!("local variables differ: {:?} compiled, {:?} in the movie", compiled.locals, stock_locals));
    }
    if names.names.len() != names_before {
        differences.push(format!("the unchanged source needed new names: {:?}", &names.names[names_before..]));
    }
    if literals.len() != literals_before {
        differences.push(format!("the unchanged source needed {} new literals", literals.len() - literals_before));
    }
    RoundTrip { source, stock, compiled: Ok(compiled), differences }
}

fn same_names(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

/// A listing of bytecode bytes, one instruction a line, for showing where two versions differ.
pub fn disassemble(bytes: &[u8], names: &[String]) -> Vec<String> {
    use crate::director::chunks::handler::get_opcode_name;
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let pos = i;
        let op = bytes[i];
        i += 1;
        let base = if op >= 0x40 { 0x40 + op % 0x40 } else { op };
        let mut arg: Option<i64> = None;
        if op >= 0xc0 && i + 4 <= bytes.len() {
            arg = Some(i32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as i64);
            i += 4;
        } else if op >= 0x80 && i + 2 <= bytes.len() {
            let raw = u16::from_be_bytes([bytes[i], bytes[i + 1]]);
            arg = Some(if base == 0x6e || base == 0x41 { raw as i16 as i64 } else { raw as i64 });
            i += 2;
        } else if op >= 0x40 && i < bytes.len() {
            arg = Some(if base == 0x41 { bytes[i] as i8 as i64 } else { bytes[i] as i64 });
            i += 1;
        }
        let known = num::FromPrimitive::from_u16(base as u16).map(|_: crate::director::lingo::opcode::OpCode| ()).is_some();
        let name = if known { get_opcode_name(base as u16).to_string() } else { format!("unknown_{:02x}", base) };
        let mut line = format!("[{:4}] {}", pos, name);
        if let Some(arg) = arg {
            match base {
                0x53 | 0x55 => line.push_str(&format!(" [{}]", pos as i64 + arg)),
                0x54 => line.push_str(&format!(" [{}]", pos as i64 - arg)),
                0x71 => line.push_str(&format!(" {}", f32::from_bits(arg as u32))),
                0x45 | 0x46 | 0x49 | 0x4a | 0x4f | 0x50 | 0x57 | 0x5f | 0x60 | 0x61 | 0x62 | 0x63 | 0x66 | 0x67 | 0x70 | 0x72 | 0x73 => {
                    line.push_str(&format!(" {} ({})", arg, names.get(arg as usize).map(|s| s.as_str()).unwrap_or("?")));
                }
                _ => line.push_str(&format!(" {}", arg)),
            }
        }
        out.push(line);
    }
    out
}
