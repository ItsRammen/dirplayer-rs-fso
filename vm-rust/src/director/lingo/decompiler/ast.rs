// Lingo decompiler AST nodes
// Ported from ProjectorRays

use std::rc::Rc;
use std::cell::{Cell, RefCell};
use crate::director::lingo::opcode::OpCode;
use super::enums::{ChunkExprType, PutType, DatumType, CaseExpect};
use super::code_writer::CodeWriter;

/// Maximum recursion depth for write_script to prevent stack overflow
const MAX_WRITE_DEPTH: usize = 100;

/// Datum represents values in the decompiler
#[derive(Clone, Debug)]
pub struct Datum {
    pub datum_type: DatumType,
    pub int_value: i32,
    pub float_value: f64,
    pub string_value: String,
    pub list_value: Vec<Rc<AstNode>>,
}

impl Datum {
    pub fn void() -> Self {
        Self {
            datum_type: DatumType::Void,
            int_value: 0,
            float_value: 0.0,
            string_value: String::new(),
            list_value: Vec::new(),
        }
    }

    pub fn int(val: i32) -> Self {
        Self {
            datum_type: DatumType::Int,
            int_value: val,
            float_value: 0.0,
            string_value: String::new(),
            list_value: Vec::new(),
        }
    }

    pub fn float(val: f64) -> Self {
        Self {
            datum_type: DatumType::Float,
            int_value: 0,
            float_value: val,
            string_value: String::new(),
            list_value: Vec::new(),
        }
    }

    pub fn string(val: String) -> Self {
        Self {
            datum_type: DatumType::String,
            int_value: 0,
            float_value: 0.0,
            string_value: val,
            list_value: Vec::new(),
        }
    }

    pub fn symbol(val: String) -> Self {
        Self {
            datum_type: DatumType::Symbol,
            int_value: 0,
            float_value: 0.0,
            string_value: val,
            list_value: Vec::new(),
        }
    }

    pub fn var_ref(val: String) -> Self {
        Self {
            datum_type: DatumType::VarRef,
            int_value: 0,
            float_value: 0.0,
            string_value: val,
            list_value: Vec::new(),
        }
    }

    pub fn list(items: Vec<Rc<AstNode>>) -> Self {
        Self {
            datum_type: DatumType::List,
            int_value: 0,
            float_value: 0.0,
            string_value: String::new(),
            list_value: items,
        }
    }

    pub fn arg_list(items: Vec<Rc<AstNode>>) -> Self {
        Self {
            datum_type: DatumType::ArgList,
            int_value: 0,
            float_value: 0.0,
            string_value: String::new(),
            list_value: items,
        }
    }

    pub fn arg_list_no_ret(items: Vec<Rc<AstNode>>) -> Self {
        Self {
            datum_type: DatumType::ArgListNoRet,
            int_value: 0,
            float_value: 0.0,
            string_value: String::new(),
            list_value: items,
        }
    }

    pub fn prop_list(items: Vec<Rc<AstNode>>) -> Self {
        Self {
            datum_type: DatumType::PropList,
            int_value: 0,
            float_value: 0.0,
            string_value: String::new(),
            list_value: items,
        }
    }

    pub fn to_int(&self) -> i32 {
        match self.datum_type {
            DatumType::Int => self.int_value,
            DatumType::Float => self.float_value as i32,
            _ => 0,
        }
    }

    pub fn write_script(&self, code: &mut CodeWriter, dot: bool, sum: bool) {
        self.write_script_with_depth(code, dot, sum, 0);
    }

    fn write_script_with_depth(&self, code: &mut CodeWriter, _dot: bool, _sum: bool, depth: usize) {
        if depth > MAX_WRITE_DEPTH {
            code.write("/* MAX DEPTH */");
            return;
        }
        let mut text = String::new();
        super::printer::write_datum(&mut text, self, depth);
        code.write(&text);
    }
}

/// A child statement within a block, pairing the AST node with its bytecode indices
#[derive(Clone, Debug)]
pub struct BlockChild {
    pub node: Rc<AstNode>,
    pub bytecode_indices: Vec<usize>,
}

/// Block node for containing statements
#[derive(Clone, Debug)]
pub struct BlockNode {
    pub children: Vec<BlockChild>,
    pub end_pos: u32,
    pub current_case_label: Option<Rc<RefCell<CaseLabelNode>>>,
}

impl BlockNode {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            end_pos: u32::MAX,
            current_case_label: None,
        }
    }

    pub fn add_child(&mut self, child: Rc<AstNode>, bytecode_indices: Vec<usize>) {
        self.children.push(BlockChild { node: child, bytecode_indices });
    }

    pub fn write_script(&self, code: &mut CodeWriter, dot: bool, sum: bool) {
        self.write_script_with_depth(code, dot, sum, 0);
    }

    fn write_script_with_depth(&self, code: &mut CodeWriter, dot: bool, sum: bool, depth: usize) {
        if depth > MAX_WRITE_DEPTH {
            code.write("-- MAX DEPTH EXCEEDED");
            code.end_line();
            return;
        }
        for child in &self.children {
            child.node.write_script_with_depth(code, dot, sum, depth + 1);
            code.end_line();
        }
    }
}

impl Default for BlockNode {
    fn default() -> Self {
        Self::new()
    }
}

/// Main AST node enum
#[derive(Clone, Debug)]
pub enum AstNode {
    Error,
    Comment(String),
    Literal(Datum),
    Block(BlockNode),
    Var(String),
    /// An expression that keeps its parentheses in print. Used where two different instruction
    /// sequences would otherwise print the same: `char 1 of (word 2 of x)` takes the word first and
    /// then the char of that, while `char 1 of word 2 of x` is one instruction.
    Paren(Rc<AstNode>),
    Assignment { variable: Rc<AstNode>, value: Rc<AstNode>, force_verbose: bool },
    BinaryOp { opcode: OpCode, left: Rc<AstNode>, right: Rc<AstNode> },
    InverseOp(Rc<AstNode>),
    NotOp(Rc<AstNode>),
    ChunkExpr { chunk_type: ChunkExprType, first: Rc<AstNode>, last: Rc<AstNode>, string: Rc<AstNode> },
    ChunkHilite(Rc<AstNode>),
    ChunkDelete(Rc<AstNode>),
    SpriteIntersects { first: Rc<AstNode>, second: Rc<AstNode> },
    SpriteWithin { first: Rc<AstNode>, second: Rc<AstNode> },
    Member { member_type: String, member_id: Rc<AstNode>, cast_id: Option<Rc<AstNode>> },
    The(String),
    TheProp { obj: Rc<AstNode>, prop: String },
    ObjProp { obj: Rc<AstNode>, prop: String },
    ObjBracket { obj: Rc<AstNode>, prop: Rc<AstNode> },
    ObjPropIndex { obj: Rc<AstNode>, prop: String, index: Rc<AstNode>, index2: Option<Rc<AstNode>> },
    LastStringChunk { chunk_type: ChunkExprType, obj: Rc<AstNode> },
    StringChunkCount { chunk_type: ChunkExprType, obj: Rc<AstNode> },
    MenuProp { menu_id: Rc<AstNode>, prop: u32 },
    MenuItemProp { menu_id: Rc<AstNode>, item_id: Rc<AstNode>, prop: u32 },
    SoundProp { sound_id: Rc<AstNode>, prop: u32 },
    SpriteProp { sprite_id: Rc<AstNode>, prop: u32 },
    Call { name: String, args: Rc<AstNode> },
    ObjCall { name: String, args: Rc<AstNode> },
    ObjCallV4 { obj: Rc<AstNode>, args: Rc<AstNode> },
    Exit,
    ExitRepeat,
    NextRepeat,
    Put { put_type: PutType, variable: Rc<AstNode>, value: Rc<AstNode> },
    If { condition: Rc<AstNode>, block1: Rc<RefCell<BlockNode>>, block2: Rc<RefCell<BlockNode>>, has_else: Cell<bool> },
    RepeatWhile { condition: Rc<AstNode>, block: Rc<RefCell<BlockNode>>, start_index: u32 },
    RepeatWithIn { var_name: String, list: Rc<AstNode>, block: Rc<RefCell<BlockNode>>, start_index: u32 },
    RepeatWithTo { var_name: String, start: Rc<AstNode>, end: Rc<AstNode>, up: bool, block: Rc<RefCell<BlockNode>>, start_index: u32 },
    Tell { window: Rc<AstNode>, block: Rc<RefCell<BlockNode>> },
    Case { value: Rc<AstNode>, first_label: RefCell<Option<Rc<RefCell<CaseLabelNode>>>>, otherwise: RefCell<Option<Rc<RefCell<OtherwiseNode>>>>, end_pos: Cell<i32>, potential_otherwise_pos: Cell<i32> },
    NewObj { obj_type: String, args: Rc<AstNode> },
    When { event: i32, script: String },
    SoundCmd { cmd: String, args: Rc<AstNode> },
    PlayCmd { args: Rc<AstNode> },
}

impl AstNode {
    pub fn is_expression(&self) -> bool {
        match self {
            AstNode::Literal(_) |
            AstNode::Var(_) |
            AstNode::Paren(_) |
            AstNode::BinaryOp { .. } |
            AstNode::InverseOp(_) |
            AstNode::NotOp(_) |
            AstNode::ChunkExpr { .. } |
            AstNode::Member { .. } |
            AstNode::The(_) |
            AstNode::TheProp { .. } |
            AstNode::ObjProp { .. } |
            AstNode::ObjBracket { .. } |
            AstNode::ObjPropIndex { .. } |
            AstNode::LastStringChunk { .. } |
            AstNode::StringChunkCount { .. } |
            AstNode::MenuProp { .. } |
            AstNode::MenuItemProp { .. } |
            AstNode::SoundProp { .. } |
            AstNode::SpriteProp { .. } |
            AstNode::SpriteIntersects { .. } |
            AstNode::SpriteWithin { .. } |
            AstNode::NewObj { .. } => true,

            // Call/ObjCall/ObjCallV4 are expressions if arg list is NOT ArgListNoRet
            AstNode::Call { args, .. } |
            AstNode::ObjCall { args, .. } |
            AstNode::ObjCallV4 { args, .. } => {
                if let AstNode::Literal(datum) = args.as_ref() {
                    datum.datum_type != DatumType::ArgListNoRet
                } else {
                    true
                }
            }

            _ => false,
        }
    }

    pub fn is_statement(&self) -> bool {
        match self {
            AstNode::Assignment { .. } |
            AstNode::Exit |
            AstNode::ExitRepeat |
            AstNode::NextRepeat |
            AstNode::Put { .. } |
            AstNode::If { .. } |
            AstNode::RepeatWhile { .. } |
            AstNode::RepeatWithIn { .. } |
            AstNode::RepeatWithTo { .. } |
            AstNode::Tell { .. } |
            AstNode::Case { .. } |
            AstNode::ChunkHilite(_) |
            AstNode::ChunkDelete(_) |
            AstNode::When { .. } |
            AstNode::SoundCmd { .. } |
            AstNode::PlayCmd { .. } => true,

            // Call/ObjCall/ObjCallV4 are statements if arg list IS ArgListNoRet
            AstNode::Call { args, .. } |
            AstNode::ObjCall { args, .. } |
            AstNode::ObjCallV4 { args, .. } => {
                if let AstNode::Literal(datum) = args.as_ref() {
                    datum.datum_type == DatumType::ArgListNoRet
                } else {
                    false
                }
            }

            _ => false,
        }
    }

    pub fn get_value(&self) -> Option<&Datum> {
        match self {
            AstNode::Literal(d) => Some(d),
            _ => None,
        }
    }

    pub fn write_script(&self, code: &mut CodeWriter, dot: bool, sum: bool) {
        self.write_script_with_depth(code, dot, sum, 0);
    }

    fn write_script_with_depth(&self, code: &mut CodeWriter, dot: bool, sum: bool, depth: usize) {
        if depth > MAX_WRITE_DEPTH {
            code.write("/* MAX DEPTH */");
            return;
        }
        // One-line statements and expressions: the printer the compiler can read back (see printer.rs).
        let mut text = String::new();
        if super::printer::write_node(&mut text, self, depth) {
            code.write(&text);
            return;
        }
        // What is left are the block statements, laid out over several lines.
        match self {
            AstNode::Block(block) => block.write_script_with_depth(code, dot, sum, depth),
            AstNode::If { condition, block1, block2, has_else } => {
                code.write("if ");
                condition.write_script_with_depth(code, dot, sum, depth + 1);
                code.write(" then");
                code.end_line();
                code.indent();
                block1.borrow().write_script_with_depth(code, dot, sum, depth + 1);
                code.unindent();
                if has_else.get() {
                    code.write("else");
                    code.end_line();
                    code.indent();
                    block2.borrow().write_script_with_depth(code, dot, sum, depth + 1);
                    code.unindent();
                }
                code.write("end if");
            }
            AstNode::RepeatWhile { condition, block, .. } => {
                code.write("repeat while ");
                condition.write_script_with_depth(code, dot, sum, depth + 1);
                code.end_line();
                code.indent();
                block.borrow().write_script_with_depth(code, dot, sum, depth + 1);
                code.unindent();
                code.write("end repeat");
            }
            AstNode::RepeatWithIn { var_name, list, block, .. } => {
                code.write("repeat with ");
                code.write(var_name);
                code.write(" in ");
                list.write_script_with_depth(code, dot, sum, depth + 1);
                code.end_line();
                code.indent();
                block.borrow().write_script_with_depth(code, dot, sum, depth + 1);
                code.unindent();
                code.write("end repeat");
            }
            AstNode::RepeatWithTo { var_name, start, end, up, block, .. } => {
                code.write("repeat with ");
                code.write(var_name);
                code.write(" = ");
                start.write_script_with_depth(code, dot, sum, depth + 1);
                if *up {
                    code.write(" to ");
                } else {
                    code.write(" down to ");
                }
                end.write_script_with_depth(code, dot, sum, depth + 1);
                code.end_line();
                code.indent();
                block.borrow().write_script_with_depth(code, dot, sum, depth + 1);
                code.unindent();
                code.write("end repeat");
            }
            AstNode::Tell { window, block } => {
                code.write("tell ");
                window.write_script_with_depth(code, dot, sum, depth + 1);
                code.end_line();
                code.indent();
                block.borrow().write_script_with_depth(code, dot, sum, depth + 1);
                code.unindent();
                code.write("end tell");
            }
            AstNode::Case { value, first_label, otherwise, .. } => {
                code.write("case ");
                value.write_script_with_depth(code, dot, sum, depth + 1);
                code.write(" of");
                code.end_line();
                code.indent();

                // Write case labels
                let mut current_label = first_label.borrow().clone();
                while let Some(label) = current_label {
                    let label_ref = label.borrow();
                    label_ref.write_script_with_depth(code, dot, sum, depth + 1);
                    current_label = label_ref.next_label.clone();
                }

                // Write otherwise
                if let Some(ow) = &*otherwise.borrow() {
                    ow.borrow().write_script_with_depth(code, dot, sum, depth + 1);
                }

                code.unindent();
                code.write("end case");
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug)]
pub struct CaseLabelNode {
    pub value: Rc<AstNode>,
    pub expect: CaseExpect,
    pub next_or: Option<Rc<RefCell<CaseLabelNode>>>,
    pub next_label: Option<Rc<RefCell<CaseLabelNode>>>,
    pub block: Rc<RefCell<BlockNode>>,
}

impl CaseLabelNode {
    pub fn new(value: Rc<AstNode>, expect: CaseExpect) -> Self {
        Self {
            value,
            expect,
            next_or: None,
            next_label: None,
            block: Rc::new(RefCell::new(BlockNode::new())),
        }
    }

    pub fn write_script(&self, code: &mut CodeWriter, dot: bool, sum: bool) {
        self.write_script_with_depth(code, dot, sum, 0);
    }

    fn write_script_with_depth(&self, code: &mut CodeWriter, dot: bool, sum: bool, depth: usize) {
        if depth > MAX_WRITE_DEPTH {
            code.write("-- MAX DEPTH EXCEEDED");
            code.end_line();
            return;
        }
        // Write value(s)
        self.value.write_script_with_depth(code, dot, sum, depth + 1);

        // Write chained "or" values
        let mut current_or = self.next_or.clone();
        while let Some(or_label) = current_or {
            code.write(", ");
            or_label.borrow().value.write_script_with_depth(code, dot, sum, depth + 1);
            current_or = or_label.borrow().next_or.clone();
        }

        code.write(":");
        code.end_line();
        code.indent();
        self.block.borrow().write_script_with_depth(code, dot, sum, depth + 1);
        code.unindent();
    }
}

#[derive(Clone, Debug)]
pub struct OtherwiseNode {
    pub block: Rc<RefCell<BlockNode>>,
}

impl OtherwiseNode {
    pub fn new() -> Self {
        Self {
            block: Rc::new(RefCell::new(BlockNode::new())),
        }
    }

    pub fn write_script(&self, code: &mut CodeWriter, dot: bool, sum: bool) {
        self.write_script_with_depth(code, dot, sum, 0);
    }

    fn write_script_with_depth(&self, code: &mut CodeWriter, dot: bool, sum: bool, depth: usize) {
        if depth > MAX_WRITE_DEPTH {
            code.write("-- MAX DEPTH EXCEEDED");
            code.end_line();
            return;
        }
        code.write("otherwise:");
        code.end_line();
        code.indent();
        self.block.borrow().write_script_with_depth(code, dot, sum, depth + 1);
        code.unindent();
    }
}

impl Default for OtherwiseNode {
    fn default() -> Self {
        Self::new()
    }
}
