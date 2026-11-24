// The text form of decompiled Lingo.
//
// This replaces the simplified printer the decompiler port came with. It follows ProjectorRays' output
// (lingodec/ast.cpp, MPL-2.0) with one difference in aim: every construct prints in exactly one way, and
// two different instruction sequences never print the same. That is what lets the compiler in
// ../compiler turn a decompiled handler back into the bytes it came from.
//
// Where it departs from ProjectorRays:
// - `&`, `&&`, `contains` and `starts` get parentheses like the other operators, so `a & (b & c)` and
//   `(a & b) & c` differ.
// - `obj.prop` always prints with a dot. `the prop of obj` is kept for the Director 4 style
//   property instructions only (`the locH of sprite 5`, `the foreColor of field "x"`).
// - `field`, `member`, `script` and `castLib` references print as `field("x")` or `member(x, lib)`.
// - an `if` with an empty `else` keeps its `else` line (the jump is in the bytecode).

use std::rc::Rc;

use crate::director::lingo::compiler::tables;
use crate::director::lingo::opcode::OpCode;

use super::ast::{AstNode, Datum};
use super::enums::DatumType;

const MAX_DEPTH: usize = 200;

pub fn binary_op_text(opcode: OpCode) -> &'static str {
    match opcode {
        OpCode::Mul => "*",
        OpCode::Add => "+",
        OpCode::Sub => "-",
        OpCode::Div => "/",
        OpCode::Mod => "mod",
        OpCode::JoinStr => "&",
        OpCode::JoinPadStr => "&&",
        OpCode::Lt => "<",
        OpCode::LtEq => "<=",
        OpCode::NtEq => "<>",
        OpCode::Eq => "=",
        OpCode::Gt => ">",
        OpCode::GtEq => ">=",
        OpCode::And => "and",
        OpCode::Or => "or",
        OpCode::ContainsStr => "contains",
        OpCode::Contains0Str => "starts",
        _ => "???",
    }
}

/// Only used to decide where parentheses go. A left operand of the same level prints bare; every
/// other operand that is itself an operator gets parentheses.
fn print_level(opcode: OpCode) -> u32 {
    match opcode {
        OpCode::Mul | OpCode::Div | OpCode::Mod => 1,
        OpCode::Add | OpCode::Sub => 2,
        OpCode::JoinStr | OpCode::JoinPadStr => 3,
        OpCode::Lt | OpCode::LtEq | OpCode::NtEq | OpCode::Eq | OpCode::Gt | OpCode::GtEq | OpCode::ContainsStr | OpCode::Contains0Str => 4,
        OpCode::And => 5,
        OpCode::Or => 6,
        _ => 0,
    }
}

/// A float as ProjectorRays prints it: fixed notation, 17 decimals, trailing zeros removed.
pub fn float_text(f: f64) -> String {
    let mut s = format!("{:.17}", f);
    while s.ends_with('0') && !s.ends_with(".0") {
        s.pop();
    }
    s
}

pub fn string_text(s: &str) -> String {
    if s.is_empty() {
        return "EMPTY".to_string();
    }
    let mut chars = s.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        match c {
            '\x03' => return "ENTER".to_string(),
            '\x08' => return "BACKSPACE".to_string(),
            '\t' => return "TAB".to_string(),
            '\r' => return "RETURN".to_string(),
            '"' => return "QUOTE".to_string(),
            _ => {}
        }
    }
    format!("\"{}\"", s)
}

/// Whether a node can stand as an operand without parentheses: after `-` or `not`, before `.prop`,
/// as a chunk bound, a sprite number, or the string a chunk is taken from.
fn is_atomic(node: &AstNode) -> bool {
    match node {
        AstNode::Literal(d) => match d.datum_type {
            DatumType::Int => d.int_value >= 0,
            DatumType::Float => d.float_value >= 0.0,
            DatumType::ArgList | DatumType::ArgListNoRet => false,
            _ => true,
        },
        AstNode::Var(_) | AstNode::Member { .. } | AstNode::ObjProp { .. } | AstNode::ObjBracket { .. } | AstNode::ObjPropIndex { .. } | AstNode::ObjCall { .. } | AstNode::Error => true,
        AstNode::Call { name, args } => !call_without_parens(name, args),
        _ => false,
    }
}

fn arg_list(args: &AstNode) -> Option<&Datum> {
    match args {
        AstNode::Literal(d) => Some(d),
        _ => None,
    }
}

fn call_without_parens(name: &str, args: &AstNode) -> bool {
    let is_statement = arg_list(args).map(|d| d.datum_type == DatumType::ArgListNoRet).unwrap_or(false);
    is_statement && (name == "put" || name == "return")
}

fn operand(code: &mut String, node: &AstNode, depth: usize) {
    if is_atomic(node) {
        write_node(code, node, depth + 1);
    } else {
        code.push('(');
        write_node(code, node, depth + 1);
        code.push(')');
    }
}

fn list_items(code: &mut String, items: &[Rc<AstNode>], depth: usize) {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            code.push_str(", ");
        }
        write_node(code, item, depth + 1);
    }
}

pub fn write_datum(code: &mut String, datum: &Datum, depth: usize) {
    match datum.datum_type {
        DatumType::Void => code.push_str("VOID"),
        DatumType::Int => code.push_str(&datum.int_value.to_string()),
        DatumType::Float => code.push_str(&float_text(datum.float_value)),
        DatumType::String => code.push_str(&string_text(&datum.string_value)),
        DatumType::Symbol => {
            code.push('#');
            code.push_str(&datum.string_value);
        }
        DatumType::VarRef => code.push_str(&datum.string_value),
        DatumType::List => {
            code.push('[');
            list_items(code, &datum.list_value, depth);
            code.push(']');
        }
        DatumType::ArgList | DatumType::ArgListNoRet => list_items(code, &datum.list_value, depth),
        DatumType::PropList => {
            code.push('[');
            if datum.list_value.is_empty() {
                code.push(':');
            }
            let mut i = 0;
            while i < datum.list_value.len() {
                if i > 0 {
                    code.push_str(", ");
                }
                write_node(code, &datum.list_value[i], depth + 1);
                code.push_str(": ");
                match datum.list_value.get(i + 1) {
                    Some(value) => {
                        write_node(code, value, depth + 1);
                    }
                    None => code.push_str("ERROR"),
                }
                i += 2;
            }
            code.push(']');
        }
    }
}

fn is_int(node: &AstNode, value: i32) -> bool {
    matches!(node, AstNode::Literal(d) if d.datum_type == DatumType::Int && d.int_value == value)
}

/// Writes a statement or an expression that fits on one line. Returns false for the block statements
/// (`if`, `repeat`, `case`, `tell`), which the callers lay out themselves.
pub fn write_node(code: &mut String, node: &AstNode, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        code.push_str("ERROR");
        return true;
    }
    match node {
        AstNode::Error => code.push_str("ERROR"),
        AstNode::Comment(text) => {
            code.push_str("-- ");
            code.push_str(text);
        }
        AstNode::Literal(datum) => write_datum(code, datum, depth),
        AstNode::Var(name) => code.push_str(name),
        AstNode::Assignment { variable, value, force_verbose } => {
            if *force_verbose {
                code.push_str("set ");
                write_node(code, variable, depth + 1);
                code.push_str(" to ");
            } else {
                write_node(code, variable, depth + 1);
                code.push_str(" = ");
            }
            write_node(code, value, depth + 1);
        }
        AstNode::BinaryOp { opcode, left, right } => {
            let level = print_level(*opcode);
            let paren_left = match left.as_ref() {
                AstNode::BinaryOp { opcode: inner, .. } => print_level(*inner) != level,
                other => !is_atomic(other) && !starts_with_the(other),
            };
            let paren_right = match right.as_ref() {
                AstNode::BinaryOp { .. } => true,
                other => !is_atomic(other) && !starts_with_the(other),
            };
            for (operand, paren, last) in [(left, paren_left, false), (right, paren_right, true)] {
                if paren {
                    code.push('(');
                }
                write_node(code, operand, depth + 1);
                if paren {
                    code.push(')');
                }
                if !last {
                    code.push(' ');
                    code.push_str(binary_op_text(*opcode));
                    code.push(' ');
                }
            }
        }
        AstNode::InverseOp(value) => {
            code.push('-');
            operand(code, value, depth);
        }
        AstNode::NotOp(value) => {
            code.push_str("not ");
            operand(code, value, depth);
        }
        AstNode::ChunkExpr { chunk_type, first, last, string } => {
            code.push_str(chunk_type.name());
            code.push(' ');
            operand(code, first, depth);
            if !is_int(last, 0) {
                code.push_str(" to ");
                operand(code, last, depth);
            }
            code.push_str(" of ");
            let bigger_chunk = matches!(string.as_ref(), AstNode::ChunkExpr { chunk_type: inner, .. } if (*inner as u8) > (*chunk_type as u8));
            if bigger_chunk {
                write_node(code, string, depth + 1);
            } else {
                operand(code, string, depth);
            }
        }
        AstNode::ChunkHilite(chunk) => {
            code.push_str("hilite ");
            write_node(code, chunk, depth + 1);
        }
        AstNode::ChunkDelete(chunk) => {
            code.push_str("delete ");
            write_node(code, chunk, depth + 1);
        }
        AstNode::SpriteIntersects { first, second } | AstNode::SpriteWithin { first, second } => {
            code.push_str("sprite ");
            operand(code, first, depth);
            code.push_str(if matches!(node, AstNode::SpriteIntersects { .. }) { " intersects " } else { " within " });
            operand(code, second, depth);
        }
        AstNode::Member { member_type, member_id, cast_id } => {
            code.push_str(member_type);
            code.push('(');
            write_node(code, member_id, depth + 1);
            if let Some(cast) = cast_id {
                if !is_int(cast, 0) {
                    code.push_str(", ");
                    write_node(code, cast, depth + 1);
                }
            }
            code.push(')');
        }
        AstNode::The(prop) => {
            code.push_str("the ");
            code.push_str(prop);
        }
        AstNode::TheProp { obj, prop } => {
            code.push_str("the ");
            code.push_str(prop);
            code.push_str(" of ");
            match obj.as_ref() {
                AstNode::ChunkExpr { .. } => {
                    write_node(code, obj, depth + 1);
                }
                other => operand(code, other, depth),
            }
        }
        AstNode::ObjProp { obj, prop } => {
            operand(code, obj, depth);
            code.push('.');
            code.push_str(prop);
        }
        AstNode::ObjBracket { obj, prop } => {
            operand(code, obj, depth);
            code.push('[');
            write_node(code, prop, depth + 1);
            code.push(']');
        }
        AstNode::ObjPropIndex { obj, prop, index, index2 } => {
            operand(code, obj, depth);
            code.push('.');
            code.push_str(prop);
            code.push('[');
            write_node(code, index, depth + 1);
            if let Some(index2) = index2 {
                code.push_str("..");
                write_node(code, index2, depth + 1);
            }
            code.push(']');
        }
        AstNode::LastStringChunk { chunk_type, obj } => {
            code.push_str("the last ");
            code.push_str(chunk_type.name());
            code.push_str(" in ");
            operand(code, obj, depth);
        }
        AstNode::StringChunkCount { chunk_type, obj } => {
            code.push_str("the number of ");
            code.push_str(chunk_type.name());
            code.push_str("s in ");
            operand(code, obj, depth);
        }
        AstNode::MenuProp { menu_id, prop } => {
            code.push_str("the ");
            code.push_str(tables::name_of(tables::MENU_PROPS, *prop as i64).unwrap_or("ERROR"));
            code.push_str(" of menu ");
            operand(code, menu_id, depth);
        }
        AstNode::MenuItemProp { menu_id, item_id, prop } => {
            code.push_str("the ");
            code.push_str(tables::name_of(tables::MENU_ITEM_PROPS, *prop as i64).unwrap_or("ERROR"));
            code.push_str(" of menuItem ");
            operand(code, item_id, depth);
            code.push_str(" of menu ");
            operand(code, menu_id, depth);
        }
        AstNode::SoundProp { sound_id, prop } => {
            code.push_str("the ");
            code.push_str(tables::name_of(tables::SOUND_PROPS, *prop as i64).unwrap_or("ERROR"));
            code.push_str(" of sound ");
            operand(code, sound_id, depth);
        }
        AstNode::SpriteProp { sprite_id, prop } => {
            code.push_str("the ");
            code.push_str(tables::name_of(tables::SPRITE_PROPS, *prop as i64).unwrap_or("ERROR"));
            code.push_str(" of sprite ");
            operand(code, sprite_id, depth);
        }
        AstNode::Call { name, args } => {
            let list = arg_list(args);
            let count = list.map(|d| d.list_value.len()).unwrap_or(0);
            let is_statement = list.map(|d| d.datum_type == DatumType::ArgListNoRet).unwrap_or(false);
            if !is_statement && count == 0 {
                match name.as_str() {
                    "pi" => {
                        code.push_str("PI");
                        return true;
                    }
                    "space" => {
                        code.push_str("SPACE");
                        return true;
                    }
                    "void" => {
                        code.push_str("VOID");
                        return true;
                    }
                    _ => {}
                }
            }
            code.push_str(name);
            if call_without_parens(name, args) {
                if count > 0 {
                    code.push(' ');
                    write_node(code, args, depth + 1);
                }
            } else {
                code.push('(');
                write_node(code, args, depth + 1);
                code.push(')');
            }
        }
        AstNode::ObjCall { name, args } => {
            let empty: Vec<Rc<AstNode>> = Vec::new();
            let items = arg_list(args).map(|d| &d.list_value).unwrap_or(&empty);
            match items.first() {
                Some(obj) => operand(code, obj, depth),
                None => code.push_str("ERROR"),
            }
            code.push('.');
            code.push_str(name);
            code.push('(');
            if items.len() > 1 {
                list_items(code, &items[1..], depth);
            }
            code.push(')');
        }
        AstNode::ObjCallV4 { obj, args } => {
            write_node(code, obj, depth + 1);
            code.push('(');
            write_node(code, args, depth + 1);
            code.push(')');
        }
        AstNode::Exit => code.push_str("exit"),
        AstNode::ExitRepeat => code.push_str("exit repeat"),
        AstNode::NextRepeat => code.push_str("next repeat"),
        AstNode::Put { put_type, variable, value } => {
            code.push_str("put ");
            write_node(code, value, depth + 1);
            code.push(' ');
            code.push_str(put_type.name());
            code.push(' ');
            write_node(code, variable, depth + 1);
        }
        AstNode::NewObj { obj_type, args } => {
            code.push_str("new ");
            code.push_str(obj_type);
            code.push('(');
            write_node(code, args, depth + 1);
            code.push(')');
        }
        AstNode::When { .. } | AstNode::SoundCmd { .. } | AstNode::PlayCmd { .. } => code.push_str("ERROR"),
        AstNode::Block(_) | AstNode::If { .. } | AstNode::RepeatWhile { .. } | AstNode::RepeatWithIn { .. } | AstNode::RepeatWithTo { .. } | AstNode::Tell { .. } | AstNode::Case { .. } => return false,
    }
    true
}

/// `the mouseH`, `the locH of sprite 5` and the like read naturally as operands (`the mouseH > 5`), and
/// the compiler reads them as one operand, so they need no parentheses inside an operator.
fn starts_with_the(node: &AstNode) -> bool {
    matches!(
        node,
        AstNode::The(_) | AstNode::TheProp { .. } | AstNode::SpriteProp { .. } | AstNode::SoundProp { .. } | AstNode::MenuProp { .. } | AstNode::MenuItemProp { .. } | AstNode::LastStringChunk { .. } | AstNode::StringChunkCount { .. } | AstNode::ChunkExpr { .. }
    )
}

pub fn node_text(node: &AstNode) -> String {
    let mut code = String::new();
    write_node(&mut code, node, 0);
    code
}
