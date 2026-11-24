// A Lingo compiler: Lingo source for one handler in, Director bytecode out.
//
// It exists so that a handler of a movie can be changed without Director: decompile the handler
// (../decompiler), edit the text, compile it here, and put the result in place of the original
// (see player::lingo_patch). It targets what Director 12 writes and nothing older.
//
// The test of its correctness is the movie itself: `stock::round_trip` decompiles a handler, compiles
// the text again and compares with the handler's own bytes.

pub mod ast;
pub mod codegen;
pub mod lexer;
pub mod parser;
pub mod stock;
pub mod tables;

use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct CompileError {
    /// 1-based line in the source text; 0 when the error is not about a line.
    pub line: u32,
    /// 1-based column; 0 when only the line is known.
    pub col: u32,
    pub message: String,
}

impl CompileError {
    pub fn at(line: u32, col: u32, message: &str) -> Self {
        CompileError { line, col, message: message.to_string() }
    }
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.line, self.col) {
            (0, _) => write!(f, "{}", self.message),
            (line, 0) => write!(f, "line {}: {}", line, self.message),
            (line, col) => write!(f, "line {}, column {}: {}", line, col, self.message),
        }
    }
}

/// An entry of a script's literal table.
#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    Str(String),
    Float(f64),
    Int(i32),
    /// Something the compiler never produces or reuses (a JavaScript blob, for one).
    Other,
}

impl Literal {
    pub fn same_as(&self, other: &Literal) -> bool {
        match (self, other) {
            (Literal::Str(a), Literal::Str(b)) => a == b,
            (Literal::Float(a), Literal::Float(b)) => a.to_bits() == b.to_bits(),
            (Literal::Int(a), Literal::Int(b)) => a == b,
            _ => false,
        }
    }
}

/// A cast's name table (`Lnam`): handler, variable, property and symbol names, each used by number.
#[derive(Clone, Debug, Default)]
pub struct NameTable {
    pub names: Vec<String>,
    exact: HashMap<String, u16>,
    lower: HashMap<String, u16>,
}

impl NameTable {
    pub fn new(names: Vec<String>) -> Self {
        let mut table = NameTable { names: Vec::new(), exact: HashMap::new(), lower: HashMap::new() };
        for name in names {
            table.push(name);
        }
        table
    }

    fn push(&mut self, name: String) -> u16 {
        let id = self.names.len() as u16;
        self.exact.entry(name.clone()).or_insert(id);
        self.lower.entry(name.to_ascii_lowercase()).or_insert(id);
        self.names.push(name);
        id
    }

    pub fn find(&self, name: &str) -> Option<u16> {
        self.exact.get(name).or_else(|| self.lower.get(&name.to_ascii_lowercase())).copied()
    }

    /// The number of a name, adding the name when the table does not have it. Names are case-insensitive.
    pub fn intern(&mut self, name: &str) -> u16 {
        match self.find(name) {
            Some(id) => id,
            None => self.push(name.to_string()),
        }
    }

    pub fn get(&self, id: u16) -> Option<&str> {
        self.names.get(id as usize).map(|s| s.as_str())
    }
}

/// What a handler is compiled against: its cast's names and its script's literals, handlers and properties.
/// Names and literals grow when the source uses new ones.
pub struct ScriptEnv<'a> {
    pub names: &'a mut NameTable,
    pub literals: &'a mut Vec<Literal>,
    /// The script's handler names, in the script's order (a call to one of them is a local call by position).
    pub handlers: &'a [String],
    /// The script's `property` names.
    pub properties: &'a [String],
    /// Globals declared for the whole script, outside any handler. Every handler sees them.
    pub script_globals: &'a [String],
}

#[derive(Clone, Debug)]
pub struct CompiledHandler {
    pub name: Option<String>,
    pub bytecode: Vec<u8>,
    pub args: Vec<String>,
    pub locals: Vec<String>,
    pub globals: Vec<String>,
    /// Things worth a look that did not stop the compile.
    pub warnings: Vec<String>,
}

/// Compiles one handler. `source` is the whole handler (`on name a, b` ... `end`) or only its body;
/// with only a body, `default_args` names the arguments.
pub fn compile_handler(source: &str, env: &mut ScriptEnv, default_args: &[String]) -> Result<CompiledHandler, CompileError> {
    let tokens = lexer::tokenize(source)?;
    let mut parser = parser::Parser::new(tokens);
    let ast = parser.parse_handler()?;
    let args = if ast.name.is_some() { ast.args.clone() } else { default_args.to_vec() };
    for arg in &args {
        env.names.intern(arg);
    }
    let mut codegen = codegen::Codegen::new(env, args);
    codegen.block(&ast.body)?;
    codegen.finish();
    Ok(CompiledHandler { name: ast.name, bytecode: std::mem::take(&mut codegen.code), args: std::mem::take(&mut codegen.args), locals: std::mem::take(&mut codegen.locals), globals: std::mem::take(&mut codegen.globals), warnings: std::mem::take(&mut codegen.warnings) })
}
