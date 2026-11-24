// Lingo source text to tokens.

use super::CompileError;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Int(i32),
    Float(f64),
    Str(String),
    Sym(String),
    Ident(String),
    Op(&'static str),
    Eol,
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
    pub col: u32,
}

const OPS: &[&str] = &["<>", "<=", ">=", "&&", "..", "-", "+", "*", "/", "&", "=", "<", ">", "(", ")", "[", "]", ",", ":", "."];

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn tokenize(source: &str) -> Result<Vec<Token>, CompileError> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1u32;
    let mut line_start = 0usize;
    macro_rules! push {
        ($tok:expr, $at:expr) => {
            out.push(Token { tok: $tok, line, col: ($at - line_start) as u32 + 1 })
        };
    }
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' => i += 1,
            '\r' | '\n' => {
                push!(Tok::Eol, i);
                if c == '\r' && chars.get(i + 1) == Some(&'\n') {
                    i += 1;
                }
                i += 1;
                line += 1;
                line_start = i;
            }
            // A line that continues on the next one: `\` (Director 7 and later) or `¬` (older).
            '\\' | '¬' => {
                let mut j = i + 1;
                while j < chars.len() && (chars[j] == ' ' || chars[j] == '\t') {
                    j += 1;
                }
                if j < chars.len() && chars[j] != '\r' && chars[j] != '\n' {
                    return Err(CompileError::at(line, (i - line_start) as u32 + 1, "a line continuation must be the last thing on its line"));
                }
                if chars.get(j) == Some(&'\r') && chars.get(j + 1) == Some(&'\n') {
                    j += 1;
                }
                i = (j + 1).min(chars.len());
                line += 1;
                line_start = i;
            }
            '-' if chars.get(i + 1) == Some(&'-') => {
                while i < chars.len() && chars[i] != '\r' && chars[i] != '\n' {
                    i += 1;
                }
            }
            '"' => {
                let start = i;
                i += 1;
                let mut text = String::new();
                loop {
                    match chars.get(i) {
                        Some('"') => break,
                        Some('\r') | Some('\n') | None => return Err(CompileError::at(line, (start - line_start) as u32 + 1, "this string has no closing quote")),
                        Some(ch) => text.push(*ch),
                    }
                    i += 1;
                }
                i += 1;
                push!(Tok::Str(text), start);
            }
            '#' => {
                let start = i;
                i += 1;
                let mut text = String::new();
                while i < chars.len() && is_ident_char(chars[i]) {
                    text.push(chars[i]);
                    i += 1;
                }
                if text.is_empty() {
                    return Err(CompileError::at(line, (start - line_start) as u32 + 1, "a symbol needs a name after #"));
                }
                push!(Tok::Sym(text), start);
            }
            _ if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).map(|d| d.is_ascii_digit()).unwrap_or(false) && !matches!(out.last().map(|t: &Token| &t.tok), Some(Tok::Ident(_)) | Some(Tok::Op(")")) | Some(Tok::Op("]")) | Some(Tok::Op(".")) | Some(Tok::Int(_)))) => {
                let start = i;
                let mut text = String::new();
                while i < chars.len() && chars[i].is_ascii_digit() {
                    text.push(chars[i]);
                    i += 1;
                }
                let mut is_float = false;
                if chars.get(i) == Some(&'.') && chars.get(i + 1).map(|d| d.is_ascii_digit()).unwrap_or(false) {
                    is_float = true;
                    text.push('.');
                    i += 1;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        text.push(chars[i]);
                        i += 1;
                    }
                }
                if (chars.get(i) == Some(&'e') || chars.get(i) == Some(&'E')) && is_float {
                    let mut j = i + 1;
                    if chars.get(j) == Some(&'-') || chars.get(j) == Some(&'+') {
                        j += 1;
                    }
                    if chars.get(j).map(|d| d.is_ascii_digit()).unwrap_or(false) {
                        while i < j {
                            text.push(chars[i]);
                            i += 1;
                        }
                        while i < chars.len() && chars[i].is_ascii_digit() {
                            text.push(chars[i]);
                            i += 1;
                        }
                    }
                }
                if i < chars.len() && is_ident_start(chars[i]) {
                    return Err(CompileError::at(line, (start - line_start) as u32 + 1, "a name cannot start with a digit"));
                }
                if !is_float {
                    if let Ok(v) = text.parse::<i32>() {
                        push!(Tok::Int(v), start);
                        continue;
                    }
                }
                match text.parse::<f64>() {
                    Ok(v) => push!(Tok::Float(v), start),
                    Err(_) => return Err(CompileError::at(line, (start - line_start) as u32 + 1, "this number cannot be read")),
                }
            }
            _ if is_ident_start(c) => {
                let start = i;
                let mut text = String::new();
                while i < chars.len() && is_ident_char(chars[i]) {
                    text.push(chars[i]);
                    i += 1;
                }
                push!(Tok::Ident(text), start);
            }
            _ => {
                let rest: String = chars[i..(i + 2).min(chars.len())].iter().collect();
                match OPS.iter().find(|op| rest.starts_with(**op)) {
                    Some(op) => {
                        push!(Tok::Op(op), i);
                        i += op.chars().count();
                    }
                    None => return Err(CompileError::at(line, (i - line_start) as u32 + 1, &format!("unexpected character `{}`", c))),
                }
            }
        }
    }
    push!(Tok::Eol, i);
    push!(Tok::Eof, i);
    Ok(out)
}
