// Syntax tree to Director bytecode.
//
// Director's own compiler does no optimising: each statement becomes the obvious instructions, in order.
// The rules here were read off the decompiler in ../decompiler (each instruction's reverse) and then
// checked against a whole movie: a handler's decompiled source must compile back to its own bytes.

use super::ast::*;
use super::tables;
use super::{CompileError, Literal, ScriptEnv};

// Instruction numbers (see ../opcode.rs). Single-byte ones are complete; the others take an operand.
const RET: u8 = 0x01;
const PUSHZERO: u8 = 0x03;
const MUL: u8 = 0x04;
const ADD: u8 = 0x05;
const SUB: u8 = 0x06;
const DIV: u8 = 0x07;
const MOD: u8 = 0x08;
const INV: u8 = 0x09;
const JOINSTR: u8 = 0x0a;
const JOINPADSTR: u8 = 0x0b;
const LT: u8 = 0x0c;
const LTEQ: u8 = 0x0d;
const NTEQ: u8 = 0x0e;
const EQ: u8 = 0x0f;
const GT: u8 = 0x10;
const GTEQ: u8 = 0x11;
const AND: u8 = 0x12;
const OR: u8 = 0x13;
const NOT: u8 = 0x14;
const CONTAINSSTR: u8 = 0x15;
const CONTAINS0STR: u8 = 0x16;
const GETCHUNK: u8 = 0x17;
const HILITECHUNK: u8 = 0x18;
const ONTOSPR: u8 = 0x19;
const INTOSPR: u8 = 0x1a;
const GETFIELD: u8 = 0x1b;
const STARTTELL: u8 = 0x1c;
const ENDTELL: u8 = 0x1d;
const PUSHLIST: u8 = 0x1e;
const PUSHPROPLIST: u8 = 0x1f;
const SWAP: u8 = 0x21;
const PUSHINT8: u8 = 0x41;
const PUSHARGLISTNORET: u8 = 0x42;
const PUSHARGLIST: u8 = 0x43;
const PUSHCONS: u8 = 0x44;
const PUSHSYMB: u8 = 0x45;
const PUSHVARREF: u8 = 0x46;
const GETGLOBAL: u8 = 0x49;
const GETPROP: u8 = 0x4a;
const GETPARAM: u8 = 0x4b;
const GETLOCAL: u8 = 0x4c;
const SETGLOBAL: u8 = 0x4f;
const SETPROP: u8 = 0x50;
const SETPARAM: u8 = 0x51;
const SETLOCAL: u8 = 0x52;
const JMP: u8 = 0x53;
const ENDREPEAT: u8 = 0x54;
const JMPIFZ: u8 = 0x55;
const LOCALCALL: u8 = 0x56;
const EXTCALL: u8 = 0x57;
const PUT: u8 = 0x59;
const PUTCHUNK: u8 = 0x5a;
const DELETECHUNK: u8 = 0x5b;
const GET: u8 = 0x5c;
const SET: u8 = 0x5d;
const GETMOVIEPROP: u8 = 0x5f;
const SETMOVIEPROP: u8 = 0x60;
const GETOBJPROP: u8 = 0x61;
const SETOBJPROP: u8 = 0x62;
const TELLCALL: u8 = 0x63;
const PEEK: u8 = 0x64;
const POP: u8 = 0x65;
const THEBUILTIN: u8 = 0x66;
const OBJCALL: u8 = 0x67;
const PUSHCHUNKVARREF: u8 = 0x6d;
const PUSHINT16: u8 = 0x6e;
const PUSHINT32: u8 = 0x6f;
const GETCHAINEDPROP: u8 = 0x70;
const PUSHFLOAT32: u8 = 0x71;
const GETTOPLEVELPROP: u8 = 0x72;

/// `the X` names that compile to a call-like instruction (`thebuiltin`) instead of a movie property.
/// Every name here was seen compiled that way by Director 12 in the ROTL movie.
const THE_BUILTINS: &[&str] = &[
    "clickLoc", "clickOn", "date", "frame", "key", "maxinteger", "mouseDown", "mouseH", "mouseLine", "mouseV", "mouseWord", "optionDown", "rightMouseDown", "shiftDown", "ticks", "time",
];

/// `the X` names seen compiled as a movie property (`getmovieprop`/`setmovieprop`) in the ROTL movie.
const THE_MOVIE_PROPS_SEEN: &[&str] = &[
    "actorList", "applicationPath", "currentSpriteNum", "itemDelimiter", "keyPressed", "lastChannel", "mouseLoc", "moviePath", "platform", "runMode", "stage", "emulateMultibuttonMouse", "updateLock",
];

/// `the X` names seen compiled as an old animation property (`get 7`/`set 7`) in the ROTL movie.
const THE_ANIM_PROPS_SEEN: &[&str] = &["timer", "colorDepth"];

/// Top-level objects (`_movie.x`); each has its own instruction.
const TOP_LEVEL_OBJECTS: &[&str] = &["_global", "_key", "_mouse", "_movie", "_net", "_player", "_sound", "_system"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum VarKind {
    Global,
    Param,
    Local,
    Prop,
}

struct Loop {
    /// Positions of the 16-bit operands of `jmp`s that leave the loop / go to its next round.
    exits: Vec<usize>,
    nexts: Vec<usize>,
    /// How many values the surrounding `case`s and `repeat with ... in`s keep on the stack at the loop's own level.
    stack_extra: u32,
}

pub struct Codegen<'a, 'b> {
    env: &'a mut ScriptEnv<'b>,
    pub code: Vec<u8>,
    pub args: Vec<String>,
    pub locals: Vec<String>,
    pub globals: Vec<String>,
    pub warnings: Vec<String>,
    loops: Vec<Loop>,
    /// Values that `case` and `repeat with ... in` keep on the stack; `exit` and `return` drop them first.
    stack_extra: u32,
    tell_depth: u32,
    line: u32,
}

type Res<T> = Result<T, CompileError>;

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl<'a, 'b> Codegen<'a, 'b> {
    pub fn new(env: &'a mut ScriptEnv<'b>, args: Vec<String>) -> Self {
        Codegen { env, code: Vec::new(), args, locals: Vec::new(), globals: Vec::new(), warnings: Vec::new(), loops: Vec::new(), stack_extra: 0, tell_depth: 0, line: 0 }
    }

    fn err<T>(&self, message: &str) -> Res<T> {
        Err(CompileError::at(self.line, 0, message))
    }

    // ---- emitting ----

    fn op(&mut self, op: u8) {
        self.code.push(op);
    }

    /// An instruction with an unsigned operand, in the shortest form that holds it.
    fn op_arg(&mut self, op: u8, arg: u32) {
        if arg < 0x100 {
            self.code.push(op);
            self.code.push(arg as u8);
        } else if arg < 0x10000 {
            self.code.push(op + 0x40);
            self.code.extend_from_slice(&(arg as u16).to_be_bytes());
        } else {
            self.code.push(op + 0x80);
            self.code.extend_from_slice(&arg.to_be_bytes());
        }
    }

    fn push_int(&mut self, v: i32) {
        if v == 0 {
            self.op(PUSHZERO);
        } else if (-128..=127).contains(&v) {
            self.code.push(PUSHINT8);
            self.code.push(v as i8 as u8);
        } else if (-32768..=32767).contains(&v) {
            self.code.push(PUSHINT16 + 0x40);
            self.code.extend_from_slice(&(v as i16).to_be_bytes());
        } else {
            self.code.push(PUSHINT32 + 0x80);
            self.code.extend_from_slice(&v.to_be_bytes());
        }
    }

    /// A forward jump with a 16-bit distance, filled in by `land`. Returns where the instruction starts.
    fn jump(&mut self, op: u8) -> usize {
        let at = self.code.len();
        self.code.push(op + 0x40);
        self.code.extend_from_slice(&[0, 0]);
        at
    }

    fn land(&mut self, jump_at: usize) -> Res<()> {
        let distance = self.code.len() - jump_at;
        if distance > 0xffff {
            return self.err("this handler is too long: a jump would be more than 65,535 bytes");
        }
        self.code[jump_at + 1..jump_at + 3].copy_from_slice(&(distance as u16).to_be_bytes());
        Ok(())
    }

    fn end_repeat(&mut self, start: usize) {
        let distance = (self.code.len() - start) as u32;
        self.op_arg(ENDREPEAT, distance);
    }

    fn name(&mut self, name: &str) -> u32 {
        self.env.names.intern(name) as u32
    }

    fn literal(&mut self, literal: Literal) -> u32 {
        if let Some(i) = self.env.literals.iter().position(|l| l.same_as(&literal)) {
            return i as u32;
        }
        self.env.literals.push(literal);
        (self.env.literals.len() - 1) as u32
    }

    // ---- variables ----

    fn lookup(&self, name: &str) -> Option<(VarKind, u32)> {
        // A name declared `global` wins over an argument of the same name.
        if self.globals.iter().any(|g| same(g, name)) || self.env.script_globals.iter().any(|g| same(g, name)) {
            return Some((VarKind::Global, 0));
        }
        if let Some(i) = self.args.iter().position(|a| same(a, name)) {
            return Some((VarKind::Param, i as u32));
        }
        if let Some(i) = self.locals.iter().position(|l| same(l, name)) {
            return Some((VarKind::Local, i as u32));
        }
        if self.env.properties.iter().any(|p| same(p, name)) {
            return Some((VarKind::Prop, 0));
        }
        None
    }

    /// The variable for a name; a name not seen before becomes a new local.
    fn variable(&mut self, name: &str) -> (VarKind, u32) {
        match self.lookup(name) {
            Some((VarKind::Global, _)) | Some((VarKind::Prop, _)) => {
                let kind = self.lookup(name).unwrap().0;
                (kind, self.name(name))
            }
            Some(found) => found,
            None => {
                self.locals.push(name.to_string());
                // The name table must hold every local's name.
                self.name(name);
                (VarKind::Local, (self.locals.len() - 1) as u32)
            }
        }
    }

    fn load(&mut self, name: &str) {
        let (kind, index) = self.variable(name);
        let op = match kind {
            VarKind::Global => GETGLOBAL,
            VarKind::Param => GETPARAM,
            VarKind::Local => GETLOCAL,
            VarKind::Prop => GETPROP,
        };
        self.op_arg(op, index);
    }

    fn store(&mut self, name: &str) {
        let (kind, index) = self.variable(name);
        let op = match kind {
            VarKind::Global => SETGLOBAL,
            VarKind::Param => SETPARAM,
            VarKind::Local => SETLOCAL,
            VarKind::Prop => SETPROP,
        };
        self.op_arg(op, index);
    }

    /// Pushes a reference to a variable or field for `put`, `delete` and friends; returns the reference type.
    fn container(&mut self, target: &Expr) -> Res<u32> {
        match target {
            Expr::Field { id, cast } => {
                self.expr(id)?;
                match cast {
                    Some(cast) => self.expr(cast)?,
                    None => self.op(PUSHZERO),
                }
                Ok(6)
            }
            Expr::Var(name) => {
                let (kind, index) = self.variable(name);
                Ok(match kind {
                    VarKind::Local => {
                        self.push_int(index as i32);
                        5
                    }
                    VarKind::Param => {
                        self.push_int(index as i32);
                        4
                    }
                    VarKind::Global => {
                        self.op_arg(PUSHVARREF, index);
                        2
                    }
                    VarKind::Prop => {
                        self.op_arg(PUSHVARREF, index);
                        3
                    }
                })
            }
            _ => self.err("expected a variable or a field here"),
        }
    }

    // ---- expressions ----

    /// A chunk's first or last position. A negative number here is stored as it is (Director writes
    /// -30001 for "the last one"); everywhere else `-5` is 5 followed by a negate instruction.
    fn chunk_bound(&mut self, bound: &Expr) -> Res<()> {
        if let Expr::Neg(inner) = bound {
            if let Expr::Int(v) = inner.as_ref() {
                self.push_int(-*v);
                return Ok(());
            }
        }
        self.expr(bound)
    }

    /// The eight values a chunk instruction takes: first and last of char, word, item, line (0 0 when absent).
    fn chunk_ranges(&mut self, ranges: &[ChunkRange]) -> Res<()> {
        for kind in [ChunkKind::Char, ChunkKind::Word, ChunkKind::Item, ChunkKind::Line] {
            let mut found = ranges.iter().filter(|r| r.kind == kind);
            match found.next() {
                Some(range) => {
                    if found.next().is_some() {
                        return self.err("the same kind of chunk appears twice in one chunk expression");
                    }
                    self.chunk_bound(&range.first)?;
                    match &range.last {
                        Some(last) => self.chunk_bound(last)?,
                        None => self.op(PUSHZERO),
                    }
                }
                None => {
                    self.op(PUSHZERO);
                    self.op(PUSHZERO);
                }
            }
        }
        Ok(())
    }

    fn call(&mut self, name: &str, arg_count: u32, statement: bool) {
        self.op_arg(if statement { PUSHARGLISTNORET } else { PUSHARGLIST }, arg_count);
        if let Some(i) = self.env.handlers.iter().position(|h| same(h, name)) {
            self.op_arg(LOCALCALL, i as u32);
        } else {
            let id = self.name(name);
            self.op_arg(if self.tell_depth > 0 { TELLCALL } else { EXTCALL }, id);
        }
    }

    fn the(&mut self, prop: &str, target: Option<&Expr>) -> Res<()> {
        let _ = target;
        if let Some(id) = tables::id_of(tables::MOVIE_PROPS, prop) {
            if prop.contains(' ') {
                self.push_int(id as i32);
                self.op_arg(GET, 0x00);
                return Ok(());
            }
        }
        if THE_BUILTINS.iter().any(|b| same(b, prop)) {
            self.op_arg(PUSHARGLIST, 0);
            let id = self.name(prop);
            self.op_arg(THEBUILTIN, id);
            return Ok(());
        }
        if let Some(id) = tables::id_of(tables::ANIM2_PROPS, prop) {
            if id == 0x02 {
                self.op(PUSHZERO);
            }
            self.push_int(id as i32);
            self.op_arg(GET, 0x08);
            return Ok(());
        }
        if !THE_MOVIE_PROPS_SEEN.iter().any(|p| same(p, prop)) {
            if let Some(id) = tables::id_of(tables::ANIM_PROPS, prop) {
                if !THE_ANIM_PROPS_SEEN.iter().any(|p| same(p, prop)) {
                    self.unverified(prop, "an old animation property");
                }
                self.push_int(id as i32);
                self.op_arg(GET, 0x07);
                return Ok(());
            }
            if let Some(id) = tables::id_of(tables::MOVIE_PROPS, prop) {
                self.unverified(prop, "an old movie property");
                self.push_int(id as i32);
                self.op_arg(GET, 0x00);
                return Ok(());
            }
            self.unverified(prop, "a movie property");
        }
        let id = self.name(prop);
        self.op_arg(GETMOVIEPROP, id);
        Ok(())
    }

    fn unverified(&mut self, prop: &str, how: &str) {
        let message = format!("line {}: `the {}` is compiled as {}; the movie has no example of it to confirm that choice", self.line, prop, how);
        if !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    /// Pushes what a Director 4 style property instruction needs and returns (property number, type).
    /// With `value`, the value to store is pushed at the point `set` expects it.
    fn the_of(&mut self, prop: &str, target: &TheTarget, value: Option<&Expr>) -> Res<(u8, u8)> {
        match target {
            TheTarget::Sprite(id) => {
                let Some(prop_id) = tables::id_of(tables::SPRITE_PROPS, prop) else {
                    return self.err(&format!("`the {} of sprite` is not one of the old sprite properties; write `sprite(n).{}` instead", prop, prop));
                };
                self.expr(id)?;
                if let Some(value) = value {
                    self.expr(value)?;
                }
                Ok((prop_id, 0x06))
            }
            TheTarget::Sound(id) => {
                let Some(prop_id) = tables::id_of(tables::SOUND_PROPS, prop) else {
                    return self.err(&format!("`the {} of sound` is not an old sound property; write `sound(n).{}` instead", prop, prop));
                };
                self.expr(id)?;
                if let Some(value) = value {
                    self.expr(value)?;
                }
                Ok((prop_id, 0x04))
            }
            TheTarget::Menu(id) => {
                let Some(prop_id) = tables::id_of(tables::MENU_PROPS, prop) else {
                    return self.err(&format!("`the {} of menu` is not a menu property", prop));
                };
                self.expr(id)?;
                if let Some(value) = value {
                    self.expr(value)?;
                }
                Ok((prop_id, 0x02))
            }
            TheTarget::MenuItem { item, menu } => {
                let Some(prop_id) = tables::id_of(tables::MENU_ITEM_PROPS, prop) else {
                    return self.err(&format!("`the {} of menuItem` is not a menu item property", prop));
                };
                self.expr(item)?;
                self.expr(menu)?;
                if let Some(value) = value {
                    self.expr(value)?;
                }
                Ok((prop_id, 0x03))
            }
            TheTarget::CastLib(cast) => {
                let Some(prop_id) = tables::id_of(tables::ANIM2_PROPS, prop) else {
                    return self.err(&format!("`the {} of castLib` is not supported; write `castLib(n).{}` instead", prop, prop));
                };
                self.expr(cast)?;
                if let Some(value) = value {
                    self.expr(value)?;
                }
                Ok((prop_id, 0x08))
            }
            TheTarget::Member { kind, id, cast, ranges } => {
                let Some(prop_id) = tables::id_of(tables::MEMBER_PROPS, prop) else {
                    return self.err(&format!("`the {} of {}` is not one of the old member properties; write `{}(...).{}` instead", prop, kind, kind, prop));
                };
                let has_chunk = !ranges.is_empty();
                let kind_type = match (kind.as_str(), has_chunk) {
                    ("field", false) => tables::MEMBER_TYPE_FIELD,
                    ("field", true) => tables::MEMBER_TYPE_FIELD_CHUNK,
                    ("script", false) => tables::MEMBER_TYPE_SCRIPT_TEXT,
                    ("script", true) => tables::MEMBER_TYPE_SCRIPT_TEXT_CHUNK,
                    (_, false) => tables::MEMBER_TYPE_GENERIC,
                    (_, true) => tables::MEMBER_TYPE_CHUNK,
                };
                if has_chunk {
                    self.chunk_ranges(ranges)?;
                }
                self.expr(id)?;
                match cast {
                    Some(cast) => self.expr(cast)?,
                    None => self.op(PUSHZERO),
                }
                if let Some(value) = value {
                    self.expr(value)?;
                }
                Ok((prop_id, kind_type))
            }
        }
    }

    /// `obj.prop` where more follows (`obj.prop.more`, `obj.prop.method()`).
    fn chain_object(&mut self, obj: &Expr) -> Res<()> {
        match obj {
            Expr::ObjProp { obj: inner, prop } => {
                self.chain_object(inner)?;
                let id = self.name(prop);
                self.op_arg(GETCHAINEDPROP, id);
                Ok(())
            }
            Expr::PropIndex { obj: inner, prop, first, last } => self.prop_index(inner, prop, first, last.as_deref(), true),
            _ => self.expr(obj),
        }
    }

    /// What `put ... into`, `delete` and `hilite` act on when it is not an old-style chunk:
    /// `x.line[2].word[1]` as a reference into the variable `x`, so that the variable itself changes.
    fn reference(&mut self, target: &Expr) -> Res<()> {
        match target {
            Expr::Var(name) if !TOP_LEVEL_OBJECTS.iter().any(|t| same(t, name)) || self.lookup(name).is_some() => {
                let kind = self.container(target)?;
                self.op_arg(PUSHCHUNKVARREF, kind);
                Ok(())
            }
            Expr::PropIndex { obj, prop, first, last } => {
                self.reference(obj)?;
                let id = self.name(prop);
                self.op_arg(PUSHSYMB, id);
                self.expr(first)?;
                let mut count = 3;
                if let Some(last) = last {
                    self.expr(last)?;
                    count = 4;
                }
                self.op_arg(PUSHARGLIST, count);
                let method = self.name("getPropRef");
                self.op_arg(OBJCALL, method);
                Ok(())
            }
            _ => self.chain_object(target),
        }
    }

    fn prop_index(&mut self, obj: &Expr, prop: &str, first: &Expr, last: Option<&Expr>, more_follows: bool) -> Res<()> {
        self.chain_object(obj)?;
        let id = self.name(prop);
        self.op_arg(PUSHSYMB, id);
        self.expr(first)?;
        let mut count = 3;
        if let Some(last) = last {
            self.expr(last)?;
            count = 4;
        }
        self.op_arg(PUSHARGLIST, count);
        let method = self.name(if more_follows { "getPropRef" } else { "getProp" });
        self.op_arg(OBJCALL, method);
        Ok(())
    }

    pub fn expr(&mut self, e: &Expr) -> Res<()> {
        match e {
            Expr::Int(v) => self.push_int(*v),
            Expr::Float(v) => {
                let narrow = *v as f32;
                if narrow as f64 == *v {
                    self.code.push(PUSHFLOAT32 + 0x80);
                    self.code.extend_from_slice(&narrow.to_bits().to_be_bytes());
                } else {
                    let id = self.literal(Literal::Float(*v));
                    self.op_arg(PUSHCONS, id);
                }
            }
            Expr::Str(s) => {
                let id = self.literal(Literal::Str(s.clone()));
                self.op_arg(PUSHCONS, id);
            }
            Expr::Sym(s) => {
                let id = self.name(s);
                self.op_arg(PUSHSYMB, id);
            }
            Expr::List(items) => {
                for item in items {
                    self.expr(item)?;
                }
                self.op_arg(PUSHARGLIST, items.len() as u32);
                self.op(PUSHLIST);
            }
            Expr::PropList(pairs) => {
                for (key, value) in pairs {
                    self.expr(key)?;
                    self.expr(value)?;
                }
                self.op_arg(PUSHARGLIST, pairs.len() as u32 * 2);
                self.op(PUSHPROPLIST);
            }
            Expr::Var(name) => {
                if TOP_LEVEL_OBJECTS.iter().any(|t| same(t, name)) && self.lookup(name).is_none() {
                    let id = self.name(name);
                    self.op_arg(GETTOPLEVELPROP, id);
                } else {
                    self.load(name);
                }
            }
            Expr::Binary(op, left, right) => {
                self.expr(left)?;
                self.expr(right)?;
                self.op(match op {
                    BinOp::Mul => MUL,
                    BinOp::Add => ADD,
                    BinOp::Sub => SUB,
                    BinOp::Div => DIV,
                    BinOp::Mod => MOD,
                    BinOp::JoinStr => JOINSTR,
                    BinOp::JoinPadStr => JOINPADSTR,
                    BinOp::Lt => LT,
                    BinOp::LtEq => LTEQ,
                    BinOp::NtEq => NTEQ,
                    BinOp::Eq => EQ,
                    BinOp::Gt => GT,
                    BinOp::GtEq => GTEQ,
                    BinOp::And => AND,
                    BinOp::Or => OR,
                    BinOp::Contains => CONTAINSSTR,
                    BinOp::Starts => CONTAINS0STR,
                });
            }
            Expr::Neg(value) => {
                self.expr(value)?;
                self.op(INV);
            }
            Expr::Not(value) => {
                self.expr(value)?;
                self.op(NOT);
            }
            Expr::Call { name, args } => {
                for arg in args {
                    self.expr(arg)?;
                }
                self.call(name, args.len() as u32, false);
            }
            Expr::ObjCall { obj, name, args } => {
                self.chain_object(obj)?;
                for arg in args {
                    self.expr(arg)?;
                }
                self.op_arg(PUSHARGLIST, args.len() as u32 + 1);
                let id = self.name(name);
                self.op_arg(OBJCALL, id);
            }
            Expr::ObjProp { obj, prop } => {
                self.chain_object(obj)?;
                let id = self.name(prop);
                self.op_arg(GETOBJPROP, id);
            }
            Expr::Index { obj, index } => {
                self.chain_object(obj)?;
                self.expr(index)?;
                self.op_arg(PUSHARGLIST, 2);
                let id = self.name("getAt");
                self.op_arg(OBJCALL, id);
            }
            Expr::PropIndex { obj, prop, first, last } => self.prop_index(obj, prop, first, last.as_deref(), false)?,
            Expr::The(prop) => self.the(prop, None)?,
            Expr::TheOf { prop, target } => {
                let (prop_id, kind) = self.the_of(prop, target, None)?;
                self.push_int(prop_id as i32);
                self.op_arg(GET, kind as u32);
            }
            Expr::Chunk { ranges, of } => {
                self.chunk_ranges(ranges)?;
                self.expr(of)?;
                self.op(GETCHUNK);
            }
            Expr::LastChunk { kind, of } => {
                self.expr(of)?;
                self.push_int(0x0b + *kind as i32);
                self.op_arg(GET, 0x00);
            }
            Expr::ChunkCount { kind, of } => {
                self.expr(of)?;
                self.push_int(*kind as i32);
                self.op_arg(GET, 0x01);
            }
            Expr::Field { id, cast } => {
                self.expr(id)?;
                match cast {
                    Some(cast) => self.expr(cast)?,
                    None => self.op(PUSHZERO),
                }
                self.op(GETFIELD);
            }
            Expr::Intersects { first, second, within } => {
                self.expr(first)?;
                self.expr(second)?;
                self.op(if *within { INTOSPR } else { ONTOSPR });
            }
        }
        Ok(())
    }

    // ---- statements ----

    pub fn block(&mut self, body: &[Stmt]) -> Res<()> {
        for stmt in body {
            self.statement(stmt)?;
        }
        Ok(())
    }

    fn drop_stack_extra(&mut self, down_to: u32) {
        if self.stack_extra > down_to {
            self.op_arg(POP, self.stack_extra - down_to);
        }
    }

    fn statement(&mut self, stmt: &Stmt) -> Res<()> {
        self.line = stmt.line;
        match &stmt.kind {
            StmtKind::Global(names) => {
                for name in names {
                    if !self.globals.iter().any(|g| same(g, name)) {
                        if self.locals.iter().any(|l| same(l, name)) {
                            return self.err(&format!("`{}` is already used as a local variable above this `global` line", name));
                        }
                        self.globals.push(name.clone());
                        self.name(name);
                    }
                }
            }
            StmtKind::Assign { target, value } => self.assign(target, value)?,
            StmtKind::Call(call) => match call {
                Expr::Call { name, args } => {
                    for arg in args {
                        self.expr(arg)?;
                    }
                    self.call(name, args.len() as u32, true);
                }
                Expr::ObjCall { obj, name, args } => {
                    self.chain_object(obj)?;
                    for arg in args {
                        self.expr(arg)?;
                    }
                    self.op_arg(PUSHARGLISTNORET, args.len() as u32 + 1);
                    let id = self.name(name);
                    self.op_arg(OBJCALL, id);
                }
                _ => return self.err("this line is not a call"),
            },
            StmtKind::Put { value, kind, target } => self.put(value, *kind, target)?,
            StmtKind::Delete(target) => match target {
                Expr::Chunk { ranges, of } => {
                    self.chunk_ranges(ranges)?;
                    let kind = self.container(of)?;
                    self.op_arg(DELETECHUNK, kind);
                }
                // delete x.word[1] -> x.word[1].delete(), on a reference to x
                other => {
                    self.reference(other)?;
                    self.op_arg(PUSHARGLISTNORET, 1);
                    let id = self.name("delete");
                    self.op_arg(OBJCALL, id);
                }
            },
            StmtKind::Hilite(target) => match target {
                Expr::Chunk { ranges, of } => {
                    self.chunk_ranges(ranges)?;
                    match of.as_ref() {
                        Expr::Field { .. } => {
                            self.container(of)?;
                            self.op(HILITECHUNK);
                        }
                        _ => return self.err("`hilite` works on a chunk of a field: `hilite line 2 of field \"x\"`"),
                    }
                }
                other => {
                    self.reference(other)?;
                    self.op_arg(PUSHARGLISTNORET, 1);
                    let id = self.name("hilite");
                    self.op_arg(OBJCALL, id);
                }
            },
            StmtKind::If { cond, then, otherwise } => {
                self.expr(cond)?;
                let to_else = self.jump(JMPIFZ);
                self.block(then)?;
                match otherwise {
                    Some(otherwise) => {
                        let to_end = self.jump(JMP);
                        self.land(to_else)?;
                        self.block(otherwise)?;
                        self.land(to_end)?;
                    }
                    None => self.land(to_else)?,
                }
            }
            StmtKind::RepeatWhile { cond, body } => {
                let start = self.code.len();
                self.expr(cond)?;
                let to_end = self.jump(JMPIFZ);
                self.loops.push(Loop { exits: Vec::new(), nexts: Vec::new(), stack_extra: self.stack_extra });
                self.block(body)?;
                let finished = self.loops.pop().unwrap();
                for at in finished.nexts {
                    self.land(at)?;
                }
                self.end_repeat(start);
                self.land(to_end)?;
                for at in finished.exits {
                    self.land(at)?;
                }
            }
            StmtKind::RepeatWith { var, start, end, down, body } => {
                self.expr(start)?;
                self.store(var);
                let top = self.code.len();
                self.load(var);
                self.expr(end)?;
                self.op(if *down { GTEQ } else { LTEQ });
                let to_end = self.jump(JMPIFZ);
                self.loops.push(Loop { exits: Vec::new(), nexts: Vec::new(), stack_extra: self.stack_extra });
                self.block(body)?;
                let finished = self.loops.pop().unwrap();
                for at in finished.nexts {
                    self.land(at)?;
                }
                self.push_int(if *down { -1 } else { 1 });
                self.load(var);
                self.op(ADD);
                self.store(var);
                self.end_repeat(top);
                self.land(to_end)?;
                for at in finished.exits {
                    self.land(at)?;
                }
            }
            StmtKind::RepeatIn { var, list, body } => {
                // On the stack while the loop runs: the list, its count, the index.
                self.expr(list)?;
                self.op_arg(PEEK, 0);
                self.op_arg(PUSHARGLIST, 1);
                let count = self.name("count");
                self.op_arg(EXTCALL, count);
                self.push_int(1);
                let top = self.code.len();
                self.op_arg(PEEK, 0);
                self.op_arg(PEEK, 2);
                self.op(LTEQ);
                let to_end = self.jump(JMPIFZ);
                self.op_arg(PEEK, 2);
                self.op_arg(PEEK, 1);
                self.op_arg(PUSHARGLIST, 2);
                let get_at = self.name("getAt");
                self.op_arg(EXTCALL, get_at);
                self.store(var);
                self.stack_extra += 3;
                self.loops.push(Loop { exits: Vec::new(), nexts: Vec::new(), stack_extra: self.stack_extra });
                self.block(body)?;
                let finished = self.loops.pop().unwrap();
                self.stack_extra -= 3;
                for at in finished.nexts {
                    self.land(at)?;
                }
                self.push_int(1);
                self.op(ADD);
                self.end_repeat(top);
                self.land(to_end)?;
                for at in finished.exits {
                    self.land(at)?;
                }
                self.op_arg(POP, 3);
            }
            StmtKind::Case { value, branches, otherwise } => self.case(value, branches, otherwise.as_deref())?,
            StmtKind::Tell { target, body } => {
                self.expr(target)?;
                self.op(STARTTELL);
                self.tell_depth += 1;
                self.block(body)?;
                self.tell_depth -= 1;
                self.op(ENDTELL);
            }
            StmtKind::Exit => {
                // Unlike `return`, a bare `exit` inside a `case` leaves the case's value where it is.
                self.op(RET);
            }
            StmtKind::ExitRepeat => {
                let Some(target) = self.loops.last().map(|l| l.stack_extra) else {
                    return self.err("`exit repeat` is not inside a repeat loop");
                };
                self.drop_stack_extra(target);
                let at = self.jump(JMP);
                self.loops.last_mut().unwrap().exits.push(at);
            }
            StmtKind::NextRepeat => {
                let Some(target) = self.loops.last().map(|l| l.stack_extra) else {
                    return self.err("`next repeat` is not inside a repeat loop");
                };
                self.drop_stack_extra(target);
                let at = self.jump(JMP);
                self.loops.last_mut().unwrap().nexts.push(at);
            }
            StmtKind::Return(value) => {
                self.drop_stack_extra(0);
                let mut count = 0;
                if let Some(value) = value {
                    self.expr(value)?;
                    count = 1;
                }
                self.call("return", count, true);
            }
        }
        Ok(())
    }

    fn case(&mut self, value: &Expr, branches: &[CaseBranch], otherwise: Option<&[Stmt]>) -> Res<()> {
        self.expr(value)?;
        self.stack_extra += 1;
        let mut to_end = Vec::new();
        for (i, branch) in branches.iter().enumerate() {
            let is_last = i + 1 == branches.len();
            // label, label, label: ... -- all but the last label jump into the body when they match
            let mut to_body = Vec::new();
            let mut to_next = 0;
            for (j, label) in branch.labels.iter().enumerate() {
                self.op_arg(PEEK, 0);
                self.expr(label)?;
                if j + 1 < branch.labels.len() {
                    self.op(NTEQ);
                    to_body.push(self.jump(JMPIFZ));
                } else {
                    self.op(EQ);
                    to_next = self.jump(JMPIFZ);
                }
            }
            for at in to_body {
                self.land(at)?;
            }
            self.block(&branch.body)?;
            if !is_last || otherwise.is_some() {
                to_end.push(self.jump(JMP));
            }
            self.land(to_next)?;
        }
        if let Some(otherwise) = otherwise {
            if branches.is_empty() {
                // A case that starts with `otherwise` jumps over nothing to its end.
            }
            self.block(otherwise)?;
        }
        for at in to_end {
            self.land(at)?;
        }
        self.stack_extra -= 1;
        self.op_arg(POP, 1);
        Ok(())
    }

    fn put(&mut self, value: &Expr, kind: PutKind, target: &Expr) -> Res<()> {
        let code = (kind as u32) << 4;
        match target {
            Expr::Chunk { ranges, of } => {
                self.expr(value)?;
                self.chunk_ranges(ranges)?;
                let container = self.container(of)?;
                self.op_arg(PUTCHUNK, code | container);
            }
            Expr::Field { .. } => {
                self.expr(value)?;
                let container = self.container(target)?;
                self.op_arg(PUT, code | container);
            }
            Expr::Var(name) => {
                if kind == PutKind::Into {
                    // `put x into v` is an assignment.
                    self.expr(value)?;
                    self.store(name);
                } else {
                    self.expr(value)?;
                    let container = self.container(target)?;
                    self.op_arg(PUT, code | container);
                }
            }
            // put x into member("y") -> member("y").setContents(x), and so on for any other object
            other => {
                // The value is worked out first, as written; a swap then puts the object under it.
                self.expr(value)?;
                self.reference(other)?;
                self.op(SWAP);
                self.op_arg(PUSHARGLISTNORET, 2);
                let method = self.name(match kind {
                    PutKind::Into => "setContents",
                    PutKind::After => "setContentsAfter",
                    PutKind::Before => "setContentsBefore",
                });
                self.op_arg(OBJCALL, method);
            }
        }
        Ok(())
    }

    fn assign(&mut self, target: &Expr, value: &Expr) -> Res<()> {
        match target {
            Expr::Var(name) => {
                if TOP_LEVEL_OBJECTS.iter().any(|t| same(t, name)) && self.lookup(name).is_none() {
                    return self.err(&format!("`{}` cannot be assigned to", name));
                }
                self.expr(value)?;
                self.store(name);
            }
            Expr::The(prop) => {
                let seen_movie_prop = THE_MOVIE_PROPS_SEEN.iter().any(|p| same(p, prop));
                if !seen_movie_prop {
                    if let Some(id) = tables::id_of(tables::ANIM_PROPS, prop) {
                        if !THE_ANIM_PROPS_SEEN.iter().any(|p| same(p, prop)) {
                            self.unverified(prop, "an old animation property");
                        }
                        self.expr(value)?;
                        self.push_int(id as i32);
                        self.op_arg(SET, 0x07);
                        return Ok(());
                    }
                    if let Some(id) = tables::id_of(tables::MOVIE_PROPS, prop) {
                        self.unverified(prop, "an old movie property");
                        self.expr(value)?;
                        self.push_int(id as i32);
                        self.op_arg(SET, 0x00);
                        return Ok(());
                    }
                    if THE_BUILTINS.iter().any(|b| same(b, prop)) {
                        return self.err(&format!("`the {}` cannot be set", prop));
                    }
                    self.unverified(prop, "a movie property");
                }
                self.expr(value)?;
                let id = self.name(prop);
                self.op_arg(SETMOVIEPROP, id);
            }
            Expr::TheOf { prop, target } => {
                let (prop_id, kind) = self.the_of(prop, target, Some(value))?;
                self.push_int(prop_id as i32);
                self.op_arg(SET, kind as u32);
            }
            Expr::ObjProp { obj, prop } => {
                self.chain_object(obj)?;
                self.expr(value)?;
                let id = self.name(prop);
                self.op_arg(SETOBJPROP, id);
            }
            Expr::Index { obj, index } => {
                self.chain_object(obj)?;
                self.expr(index)?;
                self.expr(value)?;
                self.op_arg(PUSHARGLISTNORET, 3);
                let id = self.name("setAt");
                self.op_arg(OBJCALL, id);
            }
            Expr::PropIndex { obj, prop, first, last } => {
                self.chain_object(obj)?;
                let id = self.name(prop);
                self.op_arg(PUSHSYMB, id);
                self.expr(first)?;
                let mut count = 4;
                if let Some(last) = last {
                    self.expr(last)?;
                    count = 5;
                }
                self.expr(value)?;
                self.op_arg(PUSHARGLISTNORET, count);
                let method = self.name("setProp");
                self.op_arg(OBJCALL, method);
            }
            Expr::Field { .. } | Expr::Chunk { .. } => self.put(value, PutKind::Into, target)?,
            _ => return self.err("this cannot be assigned to"),
        }
        Ok(())
    }

    pub fn finish(&mut self) {
        self.op(RET);
        let _ = (SWAP, PUSHCHUNKVARREF);
    }
}
