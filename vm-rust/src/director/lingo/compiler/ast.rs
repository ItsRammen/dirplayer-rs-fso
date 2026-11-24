// The compiler's syntax tree: what the parser hands to the code generator.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Mul,
    Add,
    Sub,
    Div,
    Mod,
    JoinStr,
    JoinPadStr,
    Lt,
    LtEq,
    NtEq,
    Eq,
    Gt,
    GtEq,
    And,
    Or,
    Contains,
    Starts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChunkKind {
    Char = 1,
    Word = 2,
    Item = 3,
    Line = 4,
}

#[derive(Clone, Debug)]
pub struct ChunkRange {
    pub kind: ChunkKind,
    pub first: Expr,
    pub last: Option<Expr>,
}

/// What a Director 4 style property (`the P of ...`) belongs to.
#[derive(Clone, Debug)]
pub enum TheTarget {
    Sprite(Box<Expr>),
    /// `member`, `field` or `script`, an optional castLib, and the chunk ranges when the property is of a chunk.
    Member { kind: String, id: Box<Expr>, cast: Option<Box<Expr>>, ranges: Vec<ChunkRange> },
    Sound(Box<Expr>),
    Menu(Box<Expr>),
    MenuItem { item: Box<Expr>, menu: Box<Expr> },
    CastLib(Box<Expr>),
}

#[derive(Clone, Debug)]
pub enum Expr {
    Int(i32),
    Float(f64),
    Str(String),
    Sym(String),
    List(Vec<Expr>),
    PropList(Vec<(Expr, Expr)>),
    Var(String),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Call { name: String, args: Vec<Expr> },
    ObjCall { obj: Box<Expr>, name: String, args: Vec<Expr> },
    ObjProp { obj: Box<Expr>, prop: String },
    Index { obj: Box<Expr>, index: Box<Expr> },
    PropIndex { obj: Box<Expr>, prop: String, first: Box<Expr>, last: Option<Box<Expr>> },
    /// `the mouseH`, `the long time`
    The(String),
    /// `the locH of sprite 5`, `the foreColor of field "x"`
    TheOf { prop: String, target: TheTarget },
    /// `char 1 to 3 of word 2 of x`: the ranges that share one instruction, smallest chunk first.
    Chunk { ranges: Vec<ChunkRange>, of: Box<Expr> },
    LastChunk { kind: ChunkKind, of: Box<Expr> },
    ChunkCount { kind: ChunkKind, of: Box<Expr> },
    Field { id: Box<Expr>, cast: Option<Box<Expr>> },
    Intersects { first: Box<Expr>, second: Box<Expr>, within: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PutKind {
    Into = 1,
    After = 2,
    Before = 3,
}

#[derive(Clone, Debug)]
pub struct CaseBranch {
    pub labels: Vec<Expr>,
    pub body: Vec<Stmt>,
}

#[derive(Clone, Debug)]
pub struct Stmt {
    pub line: u32,
    pub kind: StmtKind,
}

#[derive(Clone, Debug)]
pub enum StmtKind {
    Global(Vec<String>),
    Assign { target: Expr, value: Expr },
    /// A call whose result is thrown away: `foo(1)`, `foo 1`, `obj.method()`.
    Call(Expr),
    Put { value: Expr, kind: PutKind, target: Expr },
    /// `delete word 1 of x` (a chunk) or `delete x.word[1]` (a chunk reference).
    Delete(Expr),
    Hilite(Expr),
    If { cond: Expr, then: Vec<Stmt>, otherwise: Option<Vec<Stmt>> },
    RepeatWhile { cond: Expr, body: Vec<Stmt> },
    RepeatWith { var: String, start: Expr, end: Expr, down: bool, body: Vec<Stmt> },
    RepeatIn { var: String, list: Expr, body: Vec<Stmt> },
    Case { value: Expr, branches: Vec<CaseBranch>, otherwise: Option<Vec<Stmt>> },
    Tell { target: Expr, body: Vec<Stmt> },
    Exit,
    ExitRepeat,
    NextRepeat,
    Return(Option<Expr>),
}

#[derive(Clone, Debug)]
pub struct HandlerAst {
    /// From the `on name a, b` line, when the source has one.
    pub name: Option<String>,
    pub args: Vec<String>,
    pub body: Vec<Stmt>,
}
