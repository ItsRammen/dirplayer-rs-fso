// Tokens to syntax tree. One handler at a time.
//
// Operator levels, lowest first (as in Director; ScummVM's grammar agrees):
//   and or  |  = <> < <= > >= contains starts  |  & &&  |  + -  |  * / mod  |  not, unary -
// All are left-associative.

use super::ast::*;
use super::lexer::{Tok, Token};
use super::tables;
use super::CompileError;

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
}

type Res<T> = Result<T, CompileError>;

/// Words that end an operand, so `line` or `item` before them is a variable and not a chunk keyword.
const OPERATOR_WORDS: &[&str] = &["and", "or", "mod", "contains", "starts", "then", "to", "of", "into", "after", "before", "in", "down", "else", "intersects", "within"];

fn chunk_kind(word: &str) -> Option<ChunkKind> {
    match word.to_ascii_lowercase().as_str() {
        "char" => Some(ChunkKind::Char),
        "word" => Some(ChunkKind::Word),
        "item" => Some(ChunkKind::Item),
        "line" => Some(ChunkKind::Line),
        _ => None,
    }
}

fn chunk_kind_plural(word: &str) -> Option<ChunkKind> {
    let lower = word.to_ascii_lowercase();
    lower.strip_suffix('s').and_then(chunk_kind)
}

impl Parser {
    pub fn new(toks: Vec<Token>) -> Self {
        Parser { toks, pos: 0 }
    }

    // ---- token helpers ----

    fn peek(&self) -> &Tok {
        &self.toks[self.pos.min(self.toks.len() - 1)].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].tok
    }

    fn line(&self) -> u32 {
        self.toks[self.pos.min(self.toks.len() - 1)].line
    }

    fn err<T>(&self, message: &str) -> Res<T> {
        let t = &self.toks[self.pos.min(self.toks.len() - 1)];
        Err(CompileError::at(t.line, t.col, message))
    }

    fn describe(&self) -> String {
        match self.peek() {
            Tok::Int(v) => format!("`{}`", v),
            Tok::Float(v) => format!("`{}`", v),
            Tok::Str(s) => format!("\"{}\"", s),
            Tok::Sym(s) => format!("`#{}`", s),
            Tok::Ident(s) => format!("`{}`", s),
            Tok::Op(s) => format!("`{}`", s),
            Tok::Eol | Tok::Eof => "the end of the line".to_string(),
        }
    }

    fn next(&mut self) -> Tok {
        let t = self.peek().clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn at_word(&self, word: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s.eq_ignore_ascii_case(word))
    }

    fn word_at(&self, n: usize, word: &str) -> bool {
        matches!(self.peek_at(n), Tok::Ident(s) if s.eq_ignore_ascii_case(word))
    }

    fn at_op(&self, op: &str) -> bool {
        matches!(self.peek(), Tok::Op(s) if *s == op)
    }

    fn accept_word(&mut self, word: &str) -> bool {
        if self.at_word(word) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn accept_op(&mut self, op: &str) -> bool {
        if self.at_op(op) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_word(&mut self, word: &str) -> Res<()> {
        if self.accept_word(word) {
            Ok(())
        } else {
            self.err(&format!("expected `{}` but found {}", word, self.describe()))
        }
    }

    fn expect_op(&mut self, op: &str) -> Res<()> {
        if self.accept_op(op) {
            Ok(())
        } else {
            self.err(&format!("expected `{}` but found {}", op, self.describe()))
        }
    }

    fn at_eol(&self) -> bool {
        matches!(self.peek(), Tok::Eol | Tok::Eof)
    }

    fn expect_eol(&mut self) -> Res<()> {
        match self.peek() {
            Tok::Eol => {
                self.pos += 1;
                Ok(())
            }
            Tok::Eof => Ok(()),
            _ => self.err(&format!("expected the end of the line but found {}", self.describe())),
        }
    }

    fn skip_blank_lines(&mut self) {
        while matches!(self.peek(), Tok::Eol) {
            self.pos += 1;
        }
    }

    fn ident(&mut self, what: &str) -> Res<String> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.pos += 1;
                Ok(s)
            }
            _ => self.err(&format!("expected {} but found {}", what, self.describe())),
        }
    }

    // ---- handler ----

    pub fn parse_handler(&mut self) -> Res<HandlerAst> {
        self.skip_blank_lines();
        let mut name = None;
        let mut args = Vec::new();
        let has_header = self.at_word("on") && matches!(self.peek_at(1), Tok::Ident(_));
        if has_header {
            self.pos += 1;
            name = Some(self.ident("a handler name")?);
            let parens = self.accept_op("(");
            while let Tok::Ident(arg) = self.peek().clone() {
                self.pos += 1;
                args.push(arg);
                if !self.accept_op(",") {
                    break;
                }
            }
            if parens {
                self.expect_op(")")?;
            }
            self.expect_eol()?;
        }
        let body = self.block(&["end"])?;
        if has_header {
            if !self.accept_word("end") {
                return self.err("the handler has no `end` line");
            }
            // `end` may repeat the handler's name.
            if let Tok::Ident(_) = self.peek() {
                self.pos += 1;
            }
            self.expect_eol()?;
        } else if self.at_word("end") {
            self.pos += 1;
            if let Tok::Ident(_) = self.peek() {
                self.pos += 1;
            }
            self.expect_eol()?;
        }
        self.skip_blank_lines();
        if !matches!(self.peek(), Tok::Eof) {
            if self.at_word("on") {
                return self.err("only one handler can be compiled at a time");
            }
            return self.err(&format!("unexpected {} after the end of the handler", self.describe()));
        }
        Ok(HandlerAst { name, args, body })
    }

    /// Statements up to (not including) a line that starts with one of `enders`.
    /// An ender of two words is written with a space: "end if".
    fn block(&mut self, enders: &[&str]) -> Res<Vec<Stmt>> {
        let mut out = Vec::new();
        loop {
            self.skip_blank_lines();
            if matches!(self.peek(), Tok::Eof) {
                return Ok(out);
            }
            if self.at_block_end(enders) {
                return Ok(out);
            }
            self.statement(&mut out)?;
        }
    }

    fn at_block_end(&self, enders: &[&str]) -> bool {
        for ender in enders {
            let mut words = ender.split(' ');
            let first = words.next().unwrap();
            if !self.at_word(first) {
                continue;
            }
            match words.next() {
                Some(second) => {
                    if self.word_at(1, second) {
                        return true;
                    }
                }
                None => {
                    // A bare `end` closes the handler; `end if` and friends belong to inner blocks.
                    if first == "end" {
                        let inner = ["if", "repeat", "case", "tell"].iter().any(|w| self.word_at(1, w));
                        if !inner {
                            return true;
                        }
                    } else if first == "otherwise" || first == "else" {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn expect_end(&mut self, what: &str, opened_at: u32) -> Res<()> {
        if self.at_word("end") && self.word_at(1, what) {
            self.pos += 2;
            return self.expect_eol();
        }
        Err(CompileError::at(self.line(), 1, &format!("expected `end {}` for the `{}` on line {} but found {}", what, what, opened_at, self.describe())))
    }

    fn statement(&mut self, out: &mut Vec<Stmt>) -> Res<()> {
        let line = self.line();
        let kind = self.statement_kind()?;
        if let Some(kind) = kind {
            out.push(Stmt { line, kind });
        }
        Ok(())
    }

    /// One statement that must sit on the rest of the current line (after `then`, `else` or a case label).
    fn inline_statement(&mut self) -> Res<Vec<Stmt>> {
        let mut out = Vec::new();
        let line = self.line();
        if let Some(kind) = self.simple_statement()? {
            out.push(Stmt { line, kind });
        }
        Ok(out)
    }

    fn statement_kind(&mut self) -> Res<Option<StmtKind>> {
        let line = self.line();
        if self.at_word("if") {
            self.pos += 1;
            return self.if_statement(line).map(Some);
        }
        if self.at_word("repeat") {
            self.pos += 1;
            return self.repeat_statement(line).map(Some);
        }
        if self.at_word("case") && !matches!(self.peek_at(1), Tok::Op("=") | Tok::Op(".") | Tok::Op("[")) {
            self.pos += 1;
            return self.case_statement(line).map(Some);
        }
        if self.at_word("tell") && !matches!(self.peek_at(1), Tok::Op("=") | Tok::Op(".") | Tok::Op("[") | Tok::Op("(")) {
            self.pos += 1;
            let target = self.expr()?;
            if self.accept_word("to") {
                let body = self.inline_statement()?;
                self.expect_eol()?;
                return Ok(Some(StmtKind::Tell { target, body }));
            }
            self.expect_eol()?;
            let body = self.block(&["end tell"])?;
            self.expect_end("tell", line)?;
            return Ok(Some(StmtKind::Tell { target, body }));
        }
        if self.at_word("on") && matches!(self.peek_at(1), Tok::Ident(_)) {
            return self.err("a handler cannot start inside another handler (is an `end` missing?)");
        }
        if self.at_word("end") {
            return self.err(&format!("this `end {}` has nothing to close", match self.peek_at(1) {
                Tok::Ident(s) => s.clone(),
                _ => String::new(),
            }));
        }
        if self.at_word("else") || self.at_word("otherwise") {
            return self.err(&format!("{} is not inside an `if` or a `case`", self.describe()));
        }
        let kind = self.simple_statement()?;
        self.expect_eol()?;
        Ok(kind)
    }

    fn if_statement(&mut self, line: u32) -> Res<StmtKind> {
        let cond = self.expr()?;
        self.expect_word("then")?;
        if !self.at_eol() {
            // if c then statement [else statement]
            let then = self.inline_statement()?;
            let mut otherwise = None;
            if self.accept_word("else") {
                otherwise = Some(if self.at_word("if") {
                    self.pos += 1;
                    let inner_line = self.line();
                    vec![Stmt { line: inner_line, kind: self.if_statement(inner_line)? }]
                } else {
                    self.inline_statement()?
                });
            } else if self.at_eol() && self.next_line_starts_with_else() {
                // if c then statement
                // else statement
                self.skip_blank_lines();
                self.pos += 1;
                otherwise = Some(if self.at_word("if") {
                    self.pos += 1;
                    let inner_line = self.line();
                    vec![Stmt { line: inner_line, kind: self.if_statement(inner_line)? }]
                } else if self.at_eol() {
                    self.expect_eol()?;
                    let body = self.block(&["end if"])?;
                    self.expect_end("if", line)?;
                    return Ok(StmtKind::If { cond, then, otherwise: Some(body) });
                } else {
                    self.inline_statement()?
                });
            }
            // The nested one-line `if` forms have already consumed the end of the line.
            if !matches!(self.toks[self.pos - 1].tok, Tok::Eol) {
                self.expect_eol()?;
            }
            return Ok(StmtKind::If { cond, then, otherwise });
        }
        self.expect_eol()?;
        let then = self.block(&["else", "end if"])?;
        if self.accept_word("else") {
            if self.at_word("if") {
                // else if c then ... : the inner `if` shares this one's `end if`.
                self.pos += 1;
                let inner_line = self.line();
                let inner = self.if_statement_chained(inner_line, line)?;
                return Ok(StmtKind::If { cond, then, otherwise: Some(vec![Stmt { line: inner_line, kind: inner }]) });
            }
            if !self.at_eol() {
                let otherwise = self.inline_statement()?;
                self.expect_eol()?;
                self.skip_blank_lines();
                if self.at_word("end") && self.word_at(1, "if") {
                    self.pos += 2;
                    self.expect_eol()?;
                }
                return Ok(StmtKind::If { cond, then, otherwise: Some(otherwise) });
            }
            self.expect_eol()?;
            let otherwise = self.block(&["end if"])?;
            self.expect_end("if", line)?;
            return Ok(StmtKind::If { cond, then, otherwise: Some(otherwise) });
        }
        self.expect_end("if", line)?;
        Ok(StmtKind::If { cond, then, otherwise: None })
    }

    /// The `if` of an `else if`: same as an `if`, except that its `end if` is the outer one's.
    fn if_statement_chained(&mut self, line: u32, outer_line: u32) -> Res<StmtKind> {
        let cond = self.expr()?;
        self.expect_word("then")?;
        if !self.at_eol() {
            let then = self.inline_statement()?;
            let mut otherwise = None;
            if self.accept_word("else") {
                otherwise = Some(self.inline_statement()?);
            }
            self.expect_eol()?;
            self.skip_blank_lines();
            if self.at_word("end") && self.word_at(1, "if") {
                self.pos += 2;
                self.expect_eol()?;
            }
            return Ok(StmtKind::If { cond, then, otherwise });
        }
        self.expect_eol()?;
        let then = self.block(&["else", "end if"])?;
        if self.accept_word("else") {
            if self.at_word("if") {
                self.pos += 1;
                let inner_line = self.line();
                let inner = self.if_statement_chained(inner_line, outer_line)?;
                return Ok(StmtKind::If { cond, then, otherwise: Some(vec![Stmt { line: inner_line, kind: inner }]) });
            }
            self.expect_eol()?;
            let otherwise = self.block(&["end if"])?;
            self.expect_end("if", outer_line)?;
            return Ok(StmtKind::If { cond, then, otherwise: Some(otherwise) });
        }
        self.expect_end("if", line)?;
        Ok(StmtKind::If { cond, then, otherwise: None })
    }

    fn next_line_starts_with_else(&self) -> bool {
        let mut i = self.pos;
        while matches!(self.toks[i.min(self.toks.len() - 1)].tok, Tok::Eol) && i < self.toks.len() - 1 {
            i += 1;
        }
        matches!(&self.toks[i].tok, Tok::Ident(s) if s.eq_ignore_ascii_case("else"))
    }

    fn repeat_statement(&mut self, line: u32) -> Res<StmtKind> {
        if self.accept_word("while") {
            let cond = self.expr()?;
            self.expect_eol()?;
            let body = self.block(&["end repeat"])?;
            self.expect_end("repeat", line)?;
            return Ok(StmtKind::RepeatWhile { cond, body });
        }
        if !self.accept_word("with") {
            return self.err("expected `repeat while ...` or `repeat with ...`");
        }
        let var = self.ident("a variable name")?;
        if self.accept_word("in") {
            let list = self.expr()?;
            self.expect_eol()?;
            let body = self.block(&["end repeat"])?;
            self.expect_end("repeat", line)?;
            return Ok(StmtKind::RepeatIn { var, list, body });
        }
        self.expect_op("=")?;
        let start = self.expr()?;
        let down = self.accept_word("down");
        self.expect_word("to")?;
        let end = self.expr()?;
        self.expect_eol()?;
        let body = self.block(&["end repeat"])?;
        self.expect_end("repeat", line)?;
        Ok(StmtKind::RepeatWith { var, start, end, down, body })
    }

    fn case_statement(&mut self, line: u32) -> Res<StmtKind> {
        let value = self.expr()?;
        self.expect_word("of")?;
        self.expect_eol()?;
        let mut branches = Vec::new();
        let mut otherwise = None;
        loop {
            self.skip_blank_lines();
            if self.at_word("end") && self.word_at(1, "case") {
                self.pos += 2;
                self.expect_eol()?;
                break;
            }
            if matches!(self.peek(), Tok::Eof) {
                return Err(CompileError::at(self.line(), 1, &format!("expected `end case` for the `case` on line {}", line)));
            }
            if self.at_word("otherwise") {
                self.pos += 1;
                self.accept_op(":");
                let mut body = Vec::new();
                if !self.at_eol() {
                    body = self.inline_statement()?;
                }
                self.expect_eol()?;
                body.extend(self.block(&["end case"])?);
                self.expect_end("case", line)?;
                otherwise = Some(body);
                break;
            }
            let mut labels = vec![self.expr()?];
            while self.accept_op(",") {
                // labels may continue on the next line after a comma
                self.skip_blank_lines();
                labels.push(self.expr()?);
            }
            self.expect_op(":")?;
            let mut body = Vec::new();
            if !self.at_eol() {
                body = self.inline_statement()?;
            }
            self.expect_eol()?;
            body.extend(self.case_body()?);
            branches.push(CaseBranch { labels, body });
        }
        Ok(StmtKind::Case { value, branches, otherwise })
    }

    /// Statements of one case branch: up to the next label, `otherwise` or `end case`.
    fn case_body(&mut self) -> Res<Vec<Stmt>> {
        let mut out = Vec::new();
        loop {
            self.skip_blank_lines();
            if matches!(self.peek(), Tok::Eof) || self.at_word("otherwise") || (self.at_word("end") && self.word_at(1, "case")) {
                return Ok(out);
            }
            if self.line_is_case_label() {
                return Ok(out);
            }
            self.statement(&mut out)?;
        }
    }

    /// A line is a label when a `:` appears outside brackets before the line ends.
    fn line_is_case_label(&self) -> bool {
        let mut depth = 0i32;
        let mut i = self.pos;
        // Statements that can never be labels.
        if let Tok::Ident(s) = &self.toks[i].tok {
            let lower = s.to_ascii_lowercase();
            if ["if", "repeat", "put", "set", "return", "exit", "global", "case", "tell", "delete", "hilite", "next"].contains(&lower.as_str()) {
                return false;
            }
        }
        loop {
            match &self.toks[i].tok {
                Tok::Eol | Tok::Eof => return false,
                Tok::Op("[") | Tok::Op("(") => depth += 1,
                Tok::Op("]") | Tok::Op(")") => depth -= 1,
                Tok::Op(":") if depth == 0 => return true,
                Tok::Op("=") if depth == 0 => return false,
                _ => {}
            }
            i += 1;
        }
    }

    /// A statement without a block: assignment, call, put, set, exit and so on. Leaves the end of line alone.
    fn simple_statement(&mut self) -> Res<Option<StmtKind>> {
        if self.at_word("global") {
            self.pos += 1;
            let mut names = vec![self.ident("a global's name")?];
            while self.accept_op(",") {
                names.push(self.ident("a global's name")?);
            }
            return Ok(Some(StmtKind::Global(names)));
        }
        if self.at_word("property") && matches!(self.peek_at(1), Tok::Ident(_)) {
            return self.err("`property` lines belong to the script, not to a handler; a patch cannot add properties");
        }
        if self.at_word("exit") {
            self.pos += 1;
            if self.accept_word("repeat") {
                return Ok(Some(StmtKind::ExitRepeat));
            }
            return Ok(Some(StmtKind::Exit));
        }
        if self.at_word("next") && self.word_at(1, "repeat") {
            self.pos += 2;
            return Ok(Some(StmtKind::NextRepeat));
        }
        if self.at_word("return") && !matches!(self.peek_at(1), Tok::Op("=")) {
            self.pos += 1;
            if self.at_eol() || self.at_word("else") {
                return Ok(Some(StmtKind::Return(None)));
            }
            return Ok(Some(StmtKind::Return(Some(self.expr()?))));
        }
        if self.at_word("put") && !matches!(self.peek_at(1), Tok::Op("=") | Tok::Op(".")) {
            self.pos += 1;
            let value = self.expr()?;
            let kind = if self.accept_word("into") {
                PutKind::Into
            } else if self.accept_word("after") {
                PutKind::After
            } else if self.accept_word("before") {
                PutKind::Before
            } else {
                let mut args = vec![value];
                while self.accept_op(",") {
                    args.push(self.expr()?);
                }
                return Ok(Some(StmtKind::Call(Expr::Call { name: "put".to_string(), args })));
            };
            let target = self.unary()?;
            return Ok(Some(StmtKind::Put { value, kind, target }));
        }
        if self.at_word("set") && !matches!(self.peek_at(1), Tok::Op("=") | Tok::Op(".") | Tok::Op("(")) {
            self.pos += 1;
            let target = self.postfix()?;
            if !self.accept_word("to") && !self.accept_op("=") {
                return self.err(&format!("expected `to` in this `set` but found {}", self.describe()));
            }
            let value = self.expr()?;
            return Ok(Some(StmtKind::Assign { target, value }));
        }
        if (self.at_word("delete") || self.at_word("hilite")) && matches!(self.peek_at(1), Tok::Ident(_)) && !matches!(self.peek_at(2), Tok::Op("=")) {
            let is_delete = self.at_word("delete");
            self.pos += 1;
            let target = self.unary()?;
            return Ok(Some(if is_delete { StmtKind::Delete(target) } else { StmtKind::Hilite(target) }));
        }
        if self.at_eol() {
            return Ok(None);
        }
        // An assignment or a call.
        let start = self.pos;
        let target = self.postfix()?;
        if self.accept_op("=") {
            let value = self.expr()?;
            return Ok(Some(StmtKind::Assign { target, value }));
        }
        if self.at_eol() || self.at_word("else") {
            return match target {
                Expr::Call { .. } | Expr::ObjCall { .. } => Ok(Some(StmtKind::Call(target))),
                // A bare name is a call without arguments: `updateStage`
                Expr::Var(name) => Ok(Some(StmtKind::Call(Expr::Call { name, args: Vec::new() }))),
                _ => {
                    self.pos = start;
                    self.err("this line is neither an assignment nor a call")
                }
            };
        }
        // `name arg, arg`: a command called without parentheses.
        if let Expr::Var(name) = target {
            let mut args = vec![self.expr()?];
            while self.accept_op(",") {
                args.push(self.expr()?);
            }
            return Ok(Some(StmtKind::Call(Expr::Call { name, args })));
        }
        self.err(&format!("unexpected {} in this statement", self.describe()))
    }

    // ---- expressions ----

    pub fn expr(&mut self) -> Res<Expr> {
        self.binary(1)
    }

    fn binary_op(&self) -> Option<(u8, BinOp)> {
        match self.peek() {
            Tok::Op(op) => match *op {
                "=" => Some((2, BinOp::Eq)),
                "<>" => Some((2, BinOp::NtEq)),
                "<" => Some((2, BinOp::Lt)),
                "<=" => Some((2, BinOp::LtEq)),
                ">" => Some((2, BinOp::Gt)),
                ">=" => Some((2, BinOp::GtEq)),
                "&" => Some((3, BinOp::JoinStr)),
                "&&" => Some((3, BinOp::JoinPadStr)),
                "+" => Some((4, BinOp::Add)),
                "-" => Some((4, BinOp::Sub)),
                "*" => Some((5, BinOp::Mul)),
                "/" => Some((5, BinOp::Div)),
                _ => None,
            },
            Tok::Ident(word) => match word.to_ascii_lowercase().as_str() {
                "and" => Some((1, BinOp::And)),
                "or" => Some((1, BinOp::Or)),
                "contains" => Some((2, BinOp::Contains)),
                "starts" => Some((2, BinOp::Starts)),
                "mod" => Some((5, BinOp::Mod)),
                _ => None,
            },
            _ => None,
        }
    }

    fn binary(&mut self, min_level: u8) -> Res<Expr> {
        let mut left = self.unary()?;
        while let Some((level, op)) = self.binary_op() {
            if level < min_level {
                break;
            }
            self.pos += 1;
            let right = self.binary(level + 1)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// One operand: what `-`, `not`, `the x of sprite` and `char 1 of` take.
    fn unary(&mut self) -> Res<Expr> {
        if self.accept_op("-") {
            let value = self.unary()?;
            return Ok(Expr::Neg(Box::new(value)));
        }
        if self.at_word("not") && !matches!(self.peek_at(1), Tok::Op("=") | Tok::Op(".") | Tok::Op(")") | Tok::Op(",") | Tok::Eol | Tok::Eof) {
            self.pos += 1;
            let value = self.unary()?;
            return Ok(Expr::Not(Box::new(value)));
        }
        self.postfix()
    }

    fn args(&mut self, close: &str) -> Res<Vec<Expr>> {
        let mut out = Vec::new();
        if self.accept_op(close) {
            return Ok(out);
        }
        loop {
            out.push(self.expr()?);
            if self.accept_op(",") {
                continue;
            }
            self.expect_op(close)?;
            return Ok(out);
        }
    }

    fn postfix(&mut self) -> Res<Expr> {
        let mut node = self.primary()?;
        loop {
            if self.at_op(".") {
                self.pos += 1;
                let name = self.ident("a property or method name after `.`")?;
                if self.accept_op("(") {
                    let args = self.args(")")?;
                    node = Expr::ObjCall { obj: Box::new(node), name, args };
                } else if self.at_op("[") {
                    self.pos += 1;
                    let first = self.expr()?;
                    let last = if self.accept_op("..") { Some(Box::new(self.expr()?)) } else { None };
                    self.expect_op("]")?;
                    node = Expr::PropIndex { obj: Box::new(node), prop: name, first: Box::new(first), last };
                } else {
                    node = Expr::ObjProp { obj: Box::new(node), prop: name };
                }
            } else if self.at_op("[") {
                self.pos += 1;
                let index = self.expr()?;
                self.expect_op("]")?;
                node = Expr::Index { obj: Box::new(node), index: Box::new(index) };
            } else {
                return Ok(node);
            }
        }
    }

    fn starts_operand(&self, n: usize) -> bool {
        match self.peek_at(n) {
            Tok::Int(_) | Tok::Float(_) | Tok::Str(_) | Tok::Sym(_) => true,
            Tok::Op(op) => *op == "(",
            Tok::Ident(word) => !OPERATOR_WORDS.iter().any(|w| word.eq_ignore_ascii_case(w)),
            _ => false,
        }
    }

    fn primary(&mut self) -> Res<Expr> {
        match self.peek().clone() {
            Tok::Int(v) => {
                self.pos += 1;
                Ok(Expr::Int(v))
            }
            Tok::Float(v) => {
                self.pos += 1;
                Ok(Expr::Float(v))
            }
            Tok::Str(s) => {
                self.pos += 1;
                Ok(Expr::Str(s))
            }
            Tok::Sym(s) => {
                self.pos += 1;
                Ok(Expr::Sym(s))
            }
            Tok::Op("(") => {
                self.pos += 1;
                let inner = self.expr()?;
                self.expect_op(")")?;
                Ok(inner)
            }
            Tok::Op("[") => {
                self.pos += 1;
                self.list()
            }
            Tok::Ident(word) => self.word(word),
            _ => self.err(&format!("expected a value but found {}", self.describe())),
        }
    }

    fn list(&mut self) -> Res<Expr> {
        if self.accept_op(":") {
            self.expect_op("]")?;
            return Ok(Expr::PropList(Vec::new()));
        }
        if self.accept_op("]") {
            return Ok(Expr::List(Vec::new()));
        }
        let first = self.expr()?;
        if self.accept_op(":") {
            let mut pairs = vec![(first, self.expr()?)];
            while self.accept_op(",") {
                let key = self.expr()?;
                self.expect_op(":")?;
                pairs.push((key, self.expr()?));
            }
            self.expect_op("]")?;
            return Ok(Expr::PropList(pairs));
        }
        let mut items = vec![first];
        while self.accept_op(",") {
            items.push(self.expr()?);
        }
        self.expect_op("]")?;
        Ok(Expr::List(items))
    }

    fn word(&mut self, word: String) -> Res<Expr> {
        let lower = word.to_ascii_lowercase();
        let followed_by_paren = matches!(self.peek_at(1), Tok::Op("("));
        match lower.as_str() {
            "the" => {
                self.pos += 1;
                return self.the_expr();
            }
            "true" if !followed_by_paren => {
                self.pos += 1;
                return Ok(Expr::Int(1));
            }
            "false" if !followed_by_paren => {
                self.pos += 1;
                return Ok(Expr::Int(0));
            }
            "empty" if !followed_by_paren => {
                self.pos += 1;
                return Ok(Expr::Str(String::new()));
            }
            "return" if !followed_by_paren => {
                self.pos += 1;
                return Ok(Expr::Str("\r".to_string()));
            }
            "quote" if !followed_by_paren => {
                self.pos += 1;
                return Ok(Expr::Str("\"".to_string()));
            }
            "tab" if !followed_by_paren => {
                self.pos += 1;
                return Ok(Expr::Str("\t".to_string()));
            }
            "backspace" if !followed_by_paren => {
                self.pos += 1;
                return Ok(Expr::Str("\x08".to_string()));
            }
            "enter" if !followed_by_paren => {
                self.pos += 1;
                return Ok(Expr::Str("\x03".to_string()));
            }
            "void" | "pi" | "space" if !followed_by_paren => {
                self.pos += 1;
                return Ok(Expr::Call { name: if lower == "void" { "void".to_string() } else { lower.to_ascii_uppercase() }, args: Vec::new() });
            }
            "field" => {
                self.pos += 1;
                let (id, cast) = self.member_ref()?;
                return Ok(Expr::Field { id: Box::new(id), cast: cast.map(Box::new) });
            }
            "sprite" => {
                self.pos += 1;
                if self.at_op("(") {
                    let save = self.pos;
                    self.pos += 1;
                    let mut args = self.args(")")?;
                    if self.at_word("intersects") || self.at_word("within") {
                        if args.len() != 1 {
                            self.pos = save;
                            return self.err("`sprite ... intersects` takes one sprite number on each side");
                        }
                        let within = self.at_word("within");
                        self.pos += 1;
                        let second = self.unary()?;
                        return Ok(Expr::Intersects { first: Box::new(args.remove(0)), second: Box::new(second), within });
                    }
                    return Ok(Expr::Call { name: word, args });
                }
                if !self.starts_operand(0) {
                    return Ok(Expr::Var(word));
                }
                let first = self.unary()?;
                if self.at_word("intersects") || self.at_word("within") {
                    let within = self.at_word("within");
                    self.pos += 1;
                    let second = self.unary()?;
                    return Ok(Expr::Intersects { first: Box::new(first), second: Box::new(second), within });
                }
                return Ok(Expr::Call { name: word, args: vec![first] });
            }
            // `member "x" of castLib 2`, `castLib "x"`, `script "x"`, `window "x"`: old-style references are calls.
            "member" | "castlib" | "script" | "window" | "cast" | "sound" | "menu" if !followed_by_paren && self.starts_operand(1) && !matches!(self.peek_at(1), Tok::Op(_)) => {
                self.pos += 1;
                let id = self.unary()?;
                let mut args = vec![id];
                if (lower == "member" || lower == "cast" || lower == "script") && self.at_word("of") && self.word_at(1, "castLib") {
                    self.pos += 2;
                    args.push(self.unary()?);
                }
                return Ok(Expr::Call { name: word, args });
            }
            _ => {}
        }
        if let Some(kind) = chunk_kind(&lower) {
            if !followed_by_paren && self.starts_operand(1) && !matches!(self.peek_at(1), Tok::Op(_)) || (followed_by_paren && self.paren_then_chunk_word()) {
                self.pos += 1;
                return self.chunk_expr(kind);
            }
        }
        self.pos += 1;
        if self.accept_op("(") {
            let args = self.args(")")?;
            return Ok(Expr::Call { name: word, args });
        }
        Ok(Expr::Var(word))
    }

    /// `char (i + 1) of x`: after the closing parenthesis comes `to` or `of`.
    fn paren_then_chunk_word(&self) -> bool {
        let mut depth = 0i32;
        let mut i = self.pos + 1;
        loop {
            match &self.toks[i.min(self.toks.len() - 1)].tok {
                Tok::Eol | Tok::Eof => return false,
                Tok::Op("(") | Tok::Op("[") => depth += 1,
                Tok::Op(")") | Tok::Op("]") => {
                    depth -= 1;
                    if depth == 0 {
                        return matches!(&self.toks[(i + 1).min(self.toks.len() - 1)].tok, Tok::Ident(s) if s.eq_ignore_ascii_case("to") || s.eq_ignore_ascii_case("of"));
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    /// After `field`, `member` and the like: `(id)`, `(id, cast)`, `id` or `id of castLib cast`.
    fn member_ref(&mut self) -> Res<(Expr, Option<Expr>)> {
        if self.at_op("(") {
            self.pos += 1;
            let mut args = self.args(")")?;
            return match args.len() {
                1 => Ok((args.remove(0), None)),
                2 => {
                    let cast = args.remove(1);
                    Ok((args.remove(0), Some(cast)))
                }
                _ => self.err("expected one member, or a member and a castLib"),
            };
        }
        let id = self.unary()?;
        if self.at_word("of") && self.word_at(1, "castLib") {
            self.pos += 2;
            let cast = self.unary()?;
            return Ok((id, Some(cast)));
        }
        Ok((id, None))
    }

    /// `char A [to B] of [word C of ...] S`, with the keyword already consumed.
    fn chunk_expr(&mut self, kind: ChunkKind) -> Res<Expr> {
        let mut ranges = Vec::new();
        let mut kind = kind;
        loop {
            let first = self.unary()?;
            let last = if self.accept_word("to") { Some(self.unary()?) } else { None };
            self.expect_word("of")?;
            ranges.push(ChunkRange { kind, first, last });
            // A bigger chunk right after `of` shares the instruction: `char 1 of word 2 of x`.
            if let Tok::Ident(next) = self.peek().clone() {
                if let Some(next_kind) = chunk_kind(&next) {
                    if next_kind > kind && self.starts_operand(1) {
                        self.pos += 1;
                        kind = next_kind;
                        continue;
                    }
                }
            }
            break;
        }
        let of = self.unary()?;
        Ok(Expr::Chunk { ranges, of: Box::new(of) })
    }

    fn the_expr(&mut self) -> Res<Expr> {
        let mut prop = self.ident("a property name after `the`")?;
        let lower = prop.to_ascii_lowercase();
        // the last char in x
        if lower == "last" {
            if let Tok::Ident(next) = self.peek().clone() {
                if let Some(kind) = chunk_kind(&next) {
                    self.pos += 1;
                    if !self.accept_word("in") && !self.accept_word("of") {
                        return self.err("expected `in` after `the last ...`");
                    }
                    let of = self.unary()?;
                    return Ok(Expr::LastChunk { kind, of: Box::new(of) });
                }
            }
        }
        if lower == "number" && self.at_word("of") {
            if let Tok::Ident(next) = self.peek_at(1).clone() {
                // the number of chars in x
                if let Some(kind) = chunk_kind_plural(&next) {
                    self.pos += 2;
                    if !self.accept_word("in") && !self.accept_word("of") {
                        return self.err("expected `in` after `the number of ...`");
                    }
                    let of = self.unary()?;
                    return Ok(Expr::ChunkCount { kind, of: Box::new(of) });
                }
                // the number of castMembers [of castLib x], the number of menus, ...
                let joined = format!("number of {}", next);
                if tables::id_of(tables::ANIM2_PROPS, &joined).is_some() {
                    self.pos += 2;
                    if self.at_word("of") && self.word_at(1, "castLib") {
                        self.pos += 2;
                        let cast = self.unary()?;
                        return Ok(Expr::TheOf { prop: joined, target: TheTarget::CastLib(Box::new(cast)) });
                    }
                    return Ok(Expr::The(joined));
                }
                if next.eq_ignore_ascii_case("menuItems") {
                    self.pos += 2;
                    prop = "number of menuItems".to_string();
                }
            }
        }
        // the long time, the short date, ...
        if ["long", "short", "abbr", "abbrev", "abbreviated"].contains(&lower.as_str()) {
            if self.at_word("time") || self.at_word("date") {
                let what = self.ident("")?.to_ascii_lowercase();
                let size = if lower.starts_with("abbr") { "abbr" } else { lower.as_str() };
                return Ok(Expr::The(format!("{} {}", size, what)));
            }
        }
        if !self.at_word("of") {
            return Ok(Expr::The(prop));
        }
        self.pos += 1;
        let target = if self.accept_word("sprite") {
            TheTarget::Sprite(Box::new(self.unary()?))
        } else if self.accept_word("sound") {
            TheTarget::Sound(Box::new(self.unary()?))
        } else if self.accept_word("menuItem") {
            let item = self.unary()?;
            self.expect_word("of")?;
            self.expect_word("menu")?;
            TheTarget::MenuItem { item: Box::new(item), menu: Box::new(self.unary()?) }
        } else if self.accept_word("menu") {
            TheTarget::Menu(Box::new(self.unary()?))
        } else {
            // the P of [chunk ranges of] member|field|script X
            let mut ranges = Vec::new();
            let save = self.pos;
            loop {
                let kind = match self.peek() {
                    Tok::Ident(w) => chunk_kind(w),
                    _ => None,
                };
                let Some(kind) = kind else { break };
                if !self.starts_operand(1) {
                    break;
                }
                self.pos += 1;
                let first = self.unary()?;
                let last = if self.accept_word("to") { Some(self.unary()?) } else { None };
                self.expect_word("of")?;
                ranges.push(ChunkRange { kind, first, last });
            }
            let kind = match self.peek() {
                Tok::Ident(w) if ["member", "field", "script", "cast"].iter().any(|k| w.eq_ignore_ascii_case(k)) => Some(w.to_ascii_lowercase()),
                _ => None,
            };
            match kind {
                Some(kind) => {
                    self.pos += 1;
                    let (id, cast) = self.member_ref()?;
                    TheTarget::Member { kind, id: Box::new(id), cast: cast.map(Box::new), ranges }
                }
                None => {
                    // `the text of x` on anything else is the same as `x.text`.
                    self.pos = save;
                    let obj = self.unary()?;
                    return Ok(Expr::ObjProp { obj: Box::new(obj), prop });
                }
            }
        };
        Ok(Expr::TheOf { prop, target })
    }
}
