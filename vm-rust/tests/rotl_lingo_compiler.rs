//! The Lingo compiler (director/lingo/compiler) and handlers replaced at load (player/lingo_patch).
//!
//! The first half needs no movie: it compiles small handlers and checks the instructions, and checks
//! that what the compiler writes, the decompiler prints back and the compiler reads again to the same bytes.
//!
//! The second half runs on a real movie and is skipped without one:
//!   ROTL_MOVIE=/path/to/movie.dcr [ROTL_LINGO_CONFIG=/path/to/config.json] cargo test --test rotl_lingo_compiler -- --nocapture
//! It recompiles every handler of the movie and requires the movie's exact bytes, then replaces a
//! handler in a native player and runs it.

use std::collections::HashMap;

use vm_rust::director::chunks::handler::HandlerDef;
use vm_rust::director::chunks::script::ScriptChunk;
use vm_rust::director::lingo::compiler::movie::{redact_strings, set_tool_config, static_secret_problems, PatchFile, ToolConfig};
use vm_rust::director::lingo::compiler::patch::{self, PatchRequest, Refusal};
use vm_rust::director::lingo::compiler::{compile_handler, stock, CompiledHandler, Literal, NameTable, ScriptEnv};
use vm_rust::director::lingo::datum::Datum;
use vm_rust::director::lingo::script::ScriptContext;

// ---- helpers ----

struct Compiled {
    handler: CompiledHandler,
    names: Vec<String>,
    literals: Vec<Literal>,
}

fn compile_with(source: &str, properties: &[&str], handlers: &[&str]) -> Result<Compiled, String> {
    let mut names = NameTable::new(Vec::new());
    let mut literals = Vec::new();
    let handlers: Vec<String> = handlers.iter().map(|s| s.to_string()).collect();
    let properties: Vec<String> = properties.iter().map(|s| s.to_string()).collect();
    let mut env = ScriptEnv { names: &mut names, literals: &mut literals, handlers: &handlers, properties: &properties, script_globals: &[] };
    let handler = compile_handler(source, &mut env, &[]).map_err(|e| e.to_string())?;
    Ok(Compiled { handler, names: names.names, literals })
}

fn compile(source: &str) -> Compiled {
    compile_with(source, &[], &[]).unwrap_or_else(|e| panic!("does not compile: {}\n{}", e, source))
}

/// The instructions as text, without positions: `pushint8 5`, `setlocal 0`, `extcall foo`, ...
fn listing(c: &Compiled) -> Vec<String> {
    stock::disassemble(&c.handler.bytecode, &c.names)
        .iter()
        .map(|l| {
            let text = l.splitn(2, "] ").nth(1).unwrap();
            // `extcall 12 (foo)` reads as `extcall foo`: the number is only the name's place in the table.
            match (text.find(" ("), text.ends_with(')')) {
                (Some(open), true) => format!("{} {}", text.split(' ').next().unwrap(), &text[open + 2..text.len() - 1]),
                _ => text.to_string(),
            }
        })
        .collect()
}

fn ops(source: &str) -> String {
    listing(&compile(source)).join("; ")
}

/// A one-handler script made from compiled bytes, as the file reader would have produced it.
fn as_script(c: &Compiled, handler_name: &str, properties: &[&str]) -> (ScriptContext, u16, u32) {
    let mut names = NameTable::new(c.names.clone());
    let (bytecode_array, bytecode_index_map) = patch::decode_bytecode(&c.handler.bytecode).unwrap();
    let handler = HandlerDef {
        name_id: names.intern(handler_name),
        bytecode_array,
        bytecode_index_map,
        argument_name_ids: c.handler.args.iter().map(|n| names.intern(n)).collect(),
        local_name_ids: c.handler.locals.iter().map(|n| names.intern(n)).collect(),
        global_name_ids: c.handler.globals.iter().map(|n| names.intern(n)).collect(),
        compiled_ir: std::cell::RefCell::new(None),
    };
    let chunk = ScriptChunk {
        script_number: 1,
        literals: c.literals.iter().map(patch::literal_datum).collect::<Vec<Datum>>(),
        handlers: vec![handler],
        property_name_ids: properties.iter().map(|n| names.intern(n)).collect(),
        global_name_ids: vec![],
        property_defaults: HashMap::new(),
    };
    let mut scripts = HashMap::new();
    scripts.insert(1u32, chunk);
    (ScriptContext { names: names.names, scripts }, 1200, 1)
}

/// Compile, decompile, compile again: the bytes must not change, and neither must the printed source.
fn assert_stable(source: &str, properties: &[&str]) {
    let first = compile_with(source, properties, &["test"]).unwrap_or_else(|e| panic!("does not compile: {}\n{}", e, source));
    let (lctx, version, multiplier) = as_script(&first, "test", properties);
    let chunk = &lctx.scripts[&1];
    let trip = stock::round_trip(&chunk.handlers[0], chunk, &lctx, version, multiplier);
    assert!(trip.identical(), "not stable: {:?}\n-- written:\n{}\n-- printed back:\n{}\n-- first:\n{}", trip.refusal(), source, trip.source, listing(&first).join("\n"));
}

// ---- the compiler, without a movie ----

#[test]
fn numbers_and_strings() {
    assert_eq!(ops("x = 0"), "pushzero; setlocal 0; ret");
    assert_eq!(ops("x = 127"), "pushint8 127; setlocal 0; ret");
    assert_eq!(ops("x = 128"), "pushint16 128; setlocal 0; ret");
    assert_eq!(ops("x = 40000"), "pushint32 40000; setlocal 0; ret");
    // A negative number is the positive one followed by a negate.
    assert_eq!(ops("x = -5"), "pushint8 5; inv; setlocal 0; ret");
    assert_eq!(ops("x = 0.5"), "pushfloat32 0.5; setlocal 0; ret");
    // A float that does not fit 32 bits goes to the literal table.
    let c = compile("x = 0.1");
    assert_eq!(listing(&c).join("; "), "pushcons 0; setlocal 0; ret");
    assert_eq!(c.literals, vec![Literal::Float(0.1)]);
    // The same string twice is one literal.
    let c = compile("x = \"a\" & \"b\" & \"a\"");
    assert_eq!(c.literals, vec![Literal::Str("a".into()), Literal::Str("b".into())]);
    assert_eq!(ops("x = EMPTY & RETURN & QUOTE"), "pushcons 0; pushcons 1; joinstr; pushcons 2; joinstr; setlocal 0; ret");
    assert_eq!(ops("x = TRUE"), "pushint8 1; setlocal 0; ret");
    assert_eq!(ops("x = [1, #a]"), "pushint8 1; pushsymb a; pusharglist 2; pushlist; setlocal 0; ret");
    assert_eq!(ops("x = [#a: 1]"), "pushsymb a; pushint8 1; pusharglist 2; pushproplist; setlocal 0; ret");
    assert_eq!(ops("x = [:]"), "pusharglist 0; pushproplist; setlocal 0; ret");
}

#[test]
fn operator_order() {
    // * before +, + before &, & before =, = before and/or; all left to right.
    assert_eq!(ops("x = 1 + 2 * 3"), "pushint8 1; pushint8 2; pushint8 3; mul; add; setlocal 0; ret");
    assert_eq!(ops("x = 1 - 2 - 3"), "pushint8 1; pushint8 2; sub; pushint8 3; sub; setlocal 0; ret");
    assert_eq!(ops("x = a & b + 1"), "getlocal 0; getlocal 1; pushint8 1; add; joinstr; setlocal 2; ret");
    assert_eq!(ops("x = a = b & c"), "getlocal 0; getlocal 1; getlocal 2; joinstr; eq; setlocal 3; ret");
    assert_eq!(ops("x = a = 1 and b = 2"), "getlocal 0; pushint8 1; eq; getlocal 1; pushint8 2; eq; and; setlocal 2; ret");
    assert_eq!(ops("x = a or b and c"), "getlocal 0; getlocal 1; or; getlocal 2; and; setlocal 3; ret");
    assert_eq!(ops("x = not a = b"), "getlocal 0; not; getlocal 1; eq; setlocal 2; ret");
    assert_eq!(ops("x = a contains b & c"), "getlocal 0; getlocal 1; getlocal 2; joinstr; containsstr; setlocal 3; ret");
    assert_eq!(ops("x = a starts b"), "getlocal 0; getlocal 1; contains0str; setlocal 2; ret");
    assert_eq!(ops("x = 7 mod 2"), "pushint8 7; pushint8 2; mod; setlocal 0; ret");
}

#[test]
fn variables() {
    // Locals are numbered as they are first used: `b` is read before `a` is stored.
    let c = compile("on test p\n  global g\n  a = b + p + g\nend");
    assert_eq!(listing(&c).join("; "), "getlocal 0; getparam 0; add; getglobal g; add; setlocal 1; ret");
    assert_eq!(c.handler.locals, vec!["b", "a"]);
    assert_eq!(c.handler.args, vec!["p"]);
    assert_eq!(c.handler.globals, vec!["g"]);
    // A global of the same name as an argument wins.
    assert_eq!(ops("on test p\n  global p\n  p = 1\nend"), "pushint8 1; setglobal p; ret");
    // Script properties.
    let c = compile_with("on test me\n  pCount = pCount + 1\nend", &["pCount"], &[]).unwrap();
    assert_eq!(listing(&c).join("; "), "getprop pCount; pushint8 1; add; setprop pCount; ret");
    // Names are case-insensitive.
    assert_eq!(ops("Foo = 1\nx = FOO"), "pushint8 1; setlocal 0; getlocal 0; setlocal 1; ret");
    assert_eq!(ops("_movie.go(1)"), "gettoplevelprop _movie; pushint8 1; pusharglistnoret 2; objcall go; ret");
}

#[test]
fn calls() {
    assert_eq!(ops("foo(1, 2)"), "pushint8 1; pushint8 2; pusharglistnoret 2; extcall foo; ret");
    assert_eq!(ops("foo 1, 2"), "pushint8 1; pushint8 2; pusharglistnoret 2; extcall foo; ret");
    assert_eq!(ops("updateStage"), "pusharglistnoret 0; extcall updateStage; ret");
    assert_eq!(ops("x = foo()"), "pusharglist 0; extcall foo; setlocal 0; ret");
    assert_eq!(ops("x = VOID"), "pusharglist 0; extcall void; setlocal 0; ret");
    // A handler of the same script is called by its position.
    let c = compile_with("on test\n  second(1)\nend", &[], &["first", "second"]).unwrap();
    assert_eq!(listing(&c).join("; "), "pushint8 1; pusharglistnoret 1; localcall 1; ret");
    assert_eq!(ops("obj.method(1)"), "getlocal 0; pushint8 1; pusharglistnoret 2; objcall method; ret");
    assert_eq!(ops("return 5"), "pushint8 5; pusharglistnoret 1; extcall return; ret");
    assert_eq!(ops("return"), "pusharglistnoret 0; extcall return; ret");
    assert_eq!(ops("exit"), "ret; ret");
    assert_eq!(ops("put x"), "getlocal 0; pusharglistnoret 1; extcall put; ret");
}

#[test]
fn objects() {
    assert_eq!(ops("x = sprite(5).locH"), "pushint8 5; pusharglist 1; extcall sprite; getobjprop locH; setlocal 0; ret");
    assert_eq!(ops("sprite(5).locH = 3"), "pushint8 5; pusharglist 1; extcall sprite; pushint8 3; setobjprop locH; ret");
    assert_eq!(ops("x = a.b.c"), "getlocal 0; getchainedprop b; getobjprop c; setlocal 1; ret");
    assert_eq!(ops("x = a[2]"), "getlocal 0; pushint8 2; pusharglist 2; objcall getAt; setlocal 1; ret");
    assert_eq!(ops("a[2] = 9"), "getlocal 0; pushint8 2; pushint8 9; pusharglistnoret 3; objcall setAt; ret");
    assert_eq!(ops("x = a.item[2]"), "getlocal 0; pushsymb item; pushint8 2; pusharglist 3; objcall getProp; setlocal 1; ret");
    assert_eq!(ops("x = a.char[1..3]"), "getlocal 0; pushsymb char; pushint8 1; pushint8 3; pusharglist 4; objcall getProp; setlocal 1; ret");
    assert_eq!(ops("x = a.line[2].word[1]"), "getlocal 0; pushsymb line; pushint8 2; pusharglist 3; objcall getPropRef; pushsymb word; pushint8 1; pusharglist 3; objcall getProp; setlocal 1; ret");
    assert_eq!(ops("a.p[2] = 9"), "getlocal 0; pushsymb p; pushint8 2; pushint8 9; pusharglistnoret 4; objcall setProp; ret");
    assert_eq!(ops("x = member(\"a\", \"b\").name"), "pushcons 0; pushcons 1; pusharglist 2; extcall member; getobjprop name; setlocal 0; ret");
    // The old-style reference is the same call.
    assert_eq!(ops("x = member \"a\" of castLib \"b\""), "pushcons 0; pushcons 1; pusharglist 2; extcall member; setlocal 0; ret");
}

#[test]
fn the_properties() {
    assert_eq!(ops("x = the mouseH"), "pusharglist 0; thebuiltin mouseH; setlocal 0; ret");
    assert_eq!(ops("x = the itemDelimiter"), "getmovieprop itemDelimiter; setlocal 0; ret");
    assert_eq!(ops("the itemDelimiter = \":\""), "pushcons 0; setmovieprop itemDelimiter; ret");
    assert_eq!(ops("x = the timer"), "pushint8 34; get 7; setlocal 0; ret");
    assert_eq!(ops("x = the long time"), "pushint8 8; get 0; setlocal 0; ret");
    assert_eq!(ops("x = the locH of sprite 5"), "pushint8 5; pushint8 13; get 6; setlocal 0; ret");
    assert_eq!(ops("set the locV of sprite n to 7"), "getlocal 0; pushint8 7; pushint8 14; set 6; ret");
    assert_eq!(ops("x = the member of sprite 5"), "pushint8 5; pushint8 37; get 6; setlocal 0; ret");
    assert_eq!(ops("x = the number of member \"a\""), "pushcons 0; pushzero; pushint8 10; get 9; setlocal 0; ret");
    assert_eq!(ops("set the foreColor of field \"a\" to 5"), "pushcons 0; pushzero; pushint8 5; pushint8 17; set 11; ret");
    assert_eq!(ops("set the foreColor of line 2 of field \"a\" to 5"), "pushzero; pushzero; pushzero; pushzero; pushzero; pushzero; pushint8 2; pushzero; pushcons 0; pushzero; pushint8 5; pushint8 17; set 12; ret");
    assert_eq!(ops("x = the number of chars in s"), "getlocal 0; pushint8 1; get 1; setlocal 1; ret");
    assert_eq!(ops("x = the last word in s"), "getlocal 0; pushint8 13; get 0; setlocal 1; ret");
    assert_eq!(ops("x = sprite 3 intersects 4"), "pushint8 3; pushint8 4; ontospr; setlocal 0; ret");
    assert_eq!(ops("x = sprite (a + 1) within b"), "getlocal 0; pushint8 1; add; getlocal 1; intospr; setlocal 2; ret");
    // An old sprite property the table does not have is an error that says what to write instead.
    let err = compile_with("x = the zork of sprite 5", &[], &[]).err().unwrap();
    assert!(err.contains("sprite(n).zork"), "{}", err);
    // A `the` property the movie has no example of compiles, with a warning.
    let c = compile("x = the stageColor");
    assert_eq!(c.handler.warnings.len(), 1);
}

#[test]
fn chunks_and_fields() {
    assert_eq!(ops("x = item 2 of s"), "pushzero; pushzero; pushzero; pushzero; pushint8 2; pushzero; pushzero; pushzero; getlocal 0; getchunk; setlocal 1; ret");
    assert_eq!(ops("x = char 1 to 3 of word 2 of s"), "pushint8 1; pushint8 3; pushint8 2; pushzero; pushzero; pushzero; pushzero; pushzero; getlocal 0; getchunk; setlocal 1; ret");
    // A smaller chunk outside a bigger one cannot share an instruction.
    assert_eq!(ops("x = line 1 of (item 2 of s)").matches("getchunk").count(), 2);
    assert_eq!(ops("x = field \"a\""), "pushcons 0; pushzero; getfield; setlocal 0; ret");
    assert_eq!(ops("x = field(\"a\", 2)"), "pushcons 0; pushint8 2; getfield; setlocal 0; ret");
    assert_eq!(ops("put 5 into field \"a\""), "pushint8 5; pushcons 0; pushzero; put 22; ret");
    assert_eq!(ops("put 5 after x"), "pushint8 5; pushzero; put 37; ret");
    assert_eq!(ops("put 5 into x"), "pushint8 5; setlocal 0; ret");
    assert_eq!(ops("put 5 into item 2 of x"), "pushint8 5; pushzero; pushzero; pushzero; pushzero; pushint8 2; pushzero; pushzero; pushzero; pushzero; putchunk 21; ret");
    assert_eq!(ops("on test\n  global g\n  delete word 1 of g\nend"), "pushzero; pushzero; pushint8 1; pushzero; pushzero; pushzero; pushzero; pushzero; pushvarref g; deletechunk 2; ret");
    assert_eq!(ops("hilite line 2 of field \"a\""), "pushzero; pushzero; pushzero; pushzero; pushzero; pushzero; pushint8 2; pushzero; pushcons 0; pushzero; hilitechunk; ret");
    // Director 7 chunk references act on the variable itself.
    assert_eq!(ops("delete x.word[1]"), "pushzero; pushchunkvarref 5; pushsymb word; pushint8 1; pusharglist 3; objcall getPropRef; pusharglistnoret 1; objcall delete; ret");
    assert_eq!(ops("put 5 into member(\"a\")"), "pushint8 5; pushcons 0; pusharglist 1; extcall member; swap; pusharglistnoret 2; objcall setContents; ret");
}

#[test]
fn blocks() {
    assert_eq!(ops("if a then\n  b = 1\nend if"), "getlocal 0; jmpifz [9]; pushint8 1; setlocal 1; ret");
    assert_eq!(ops("if a then\n  b = 1\nelse\n  b = 2\nend if"), "getlocal 0; jmpifz [12]; pushint8 1; setlocal 1; jmp [16]; pushint8 2; setlocal 1; ret");
    // The one-line forms and `else if` give the same instructions as the long forms.
    assert_eq!(ops("if a then b = 1"), ops("if a then\n  b = 1\nend if"));
    assert_eq!(ops("if a then b = 1 else b = 2"), ops("if a then\n  b = 1\nelse\n  b = 2\nend if"));
    assert_eq!(ops("if a then\n  b = 1\nelse if c then\n  b = 2\nelse\n  b = 3\nend if"), ops("if a then\n  b = 1\nelse\n  if c then\n    b = 2\n  else\n    b = 3\n  end if\nend if"));
    assert_eq!(ops("if a then\n  b = 1\nelse if c then\n  b = 2\nend if\nd = 4"), ops("if a then\n  b = 1\nelse\n  if c then\n    b = 2\n  end if\nend if\nd = 4"));
    assert_eq!(ops("repeat while a\n  b = 1\nend repeat"), "getlocal 0; jmpifz [11]; pushint8 1; setlocal 1; endrepeat [0]; ret");
    assert_eq!(ops("repeat with i = 1 to 3\n  b = i\nend repeat"), "pushint8 1; setlocal 0; getlocal 0; pushint8 3; lteq; jmpifz [25]; getlocal 0; setlocal 1; pushint8 1; getlocal 0; add; setlocal 0; endrepeat [4]; ret");
    assert_eq!(ops("repeat with i = 3 down to 1\nend repeat"), "pushint8 3; setlocal 0; getlocal 0; pushint8 1; gteq; jmpifz [21]; pushint8 -1; getlocal 0; add; setlocal 0; endrepeat [4]; ret");
    assert_eq!(
        ops("repeat with v in l\n  b = v\nend repeat"),
        "getlocal 0; peek 0; pusharglist 1; extcall count; pushint8 1; peek 0; peek 2; lteq; jmpifz [37]; peek 2; peek 1; pusharglist 2; extcall getAt; setlocal 1; getlocal 1; setlocal 2; pushint8 1; add; endrepeat [10]; pop 3; ret"
    );
    assert_eq!(ops("repeat while a\n  if b then exit repeat\n  next repeat\nend repeat"), "getlocal 0; jmpifz [18]; getlocal 1; jmpifz [13]; jmp [18]; jmp [16]; endrepeat [0]; ret");
    assert_eq!(
        ops("case a of\n  1, 2:\n    b = 1\n  3:\n    b = 3\n  otherwise:\n    b = 0\nend case"),
        "getlocal 0; peek 0; pushint8 1; nteq; jmpifz [18]; peek 0; pushint8 2; eq; jmpifz [25]; pushint8 1; setlocal 1; jmp [43]; peek 0; pushint8 3; eq; jmpifz [40]; pushint8 3; setlocal 1; jmp [43]; pushzero; setlocal 1; pop 1; ret"
    );
    // `return` inside a case drops the case's value first; `exit` does not.
    assert_eq!(ops("case a of\n  1:\n    return 5\nend case"), "getlocal 0; peek 0; pushint8 1; eq; jmpifz [18]; pop 1; pushint8 5; pusharglistnoret 1; extcall return; pop 1; ret");
    assert_eq!(ops("case a of\n  1: exit\nend case"), "getlocal 0; peek 0; pushint8 1; eq; jmpifz [11]; ret; pop 1; ret");
    assert_eq!(ops("tell w\n  foo()\nend tell"), "getlocal 0; starttell; pusharglistnoret 0; tellcall foo; endtell; ret");
}

#[test]
fn source_forms() {
    // Comments, blank lines, continuation lines, an `end` that repeats the name.
    let a = ops("on test a\n  -- a comment\n\n  x = a + \\\n    1 -- trailing\nend test\n");
    assert_eq!(a, ops("on test a\n  x = a + 1\nend"));
    // Keywords in any case.
    assert_eq!(ops("IF a THEN\n  b = 1\nEND IF"), ops("if a then\n  b = 1\nend if"));
    assert_eq!(ops("set x to 5"), ops("x = 5"));
    assert_eq!(ops("set x = 5"), ops("x = 5"));
    // A variable called like a chunk keyword is still a variable.
    assert_eq!(ops("line = 5\nx = line + 1"), "pushint8 5; setlocal 0; getlocal 0; pushint8 1; add; setlocal 1; ret");
    assert_eq!(ops("x = sprite(5)"), "pushint8 5; pusharglist 1; extcall sprite; setlocal 0; ret");
}

#[test]
fn errors_name_the_line() {
    let err = |source: &str| compile_with(source, &[], &[]).err().unwrap_or_else(|| panic!("compiled: {}", source));
    assert!(err("x = 1\nif a then\n  b = 1\n").starts_with("line 4"), "{}", err("x = 1\nif a then\n  b = 1\n"));
    assert!(err("x = \"abc").contains("closing quote"));
    assert!(err("x = (1 + 2").contains("expected `)`"));
    assert!(err("x = 1 +").starts_with("line 1"));
    assert!(err("exit repeat").contains("not inside a repeat"));
    assert!(err("end if").contains("nothing to close"));
    assert!(err("x = 5 5").contains("end of the line"));
    assert!(err("on a\nend\non b\nend").contains("one handler"));
    assert!(err("repeat with i = 1 to 3\n  x = 1\n").contains("end repeat"));
    assert!(err("x = 1\nglobal x").contains("already used as a local"));
    assert!(err("x = y @ 2").contains("unexpected character"));
    assert!(err("5 = x").contains("neither an assignment nor a call") || err("5 = x").contains("cannot be assigned"), "{}", err("5 = x"));
}

#[test]
fn what_the_compiler_writes_it_reads_back() {
    let cases: &[&str] = &[
        "x = 1 + 2 * (3 - 4) / 5 mod 6",
        "x = (a & b) & (c && d)\ny = a & (b & c)",
        "x = (a = b) = c\ny = a = (b <> c)\nz = (a contains b) starts c",
        "x = a and (b or c)\ny = (a and b) or c\nz = not (a and b)\nw = not a",
        "x = -(a + b)\ny = -a\nz = -5\nw = a - -b",
        "x = [1, [2, 3], [#a: [:], #b: \"s\"]]",
        "x = a.b.c.d\ny = a.b(1).c[2].d\na.b.c = 5",
        "x = a.count(#b)\ny = a.b.count",
        "x = sprite(a + 1).member.name\nsprite(1).member = member(\"x\", \"y\")",
        "x = the locH of sprite (a + 1) + the locV of sprite a\nset the locH of sprite a to the mouseH - 5",
        "x = the number of member(a, \"lib\")\ny = the number of member(a)",
        "set the foreColor of field(\"a\") to 255\nset the textStyle of word 1 to 2 of line a of field(\"b\", 3) to \"bold\"",
        "x = char 1 of word (a + 1) of item 2 to 3 of line n of s\ny = line 1 of (char 2 of s)\nz = char 1 of (word 2 of s)",
        "x = the number of words in (a & b)\ny = the last item in s\nz = the number of chars in s.text",
        "x = field(\"a\") & field(a + 1, 2)\nput x into field(\"b\")\nput x after field(\"b\", 2)\nput x before line 2 of field(\"b\")",
        "put 1 into item 2 of x\nput 1 after x\ndelete char 1 to 2 of x\ndelete char (-30001) of x\nhilite word 2 of field(\"a\")",
        "put 1 into x.line[2].word[3]\ndelete x.item[1..2]\nput a after member(\"m\")\nput a into sprite(1).member",
        "x = sprite a intersects b\ny = sprite (a + 1) within 5",
        "if a then\n  b = 1\nelse\nend if\nif a then\nend if",
        "if a then\n  if b then\n    c = 1\n  else\n    c = 2\n  end if\nelse\n  if d then\n    c = 3\n  end if\nend if",
        "repeat with i = a to b + 1\n  repeat while i < 5\n    if i then\n      exit repeat\n    end if\n    next repeat\n  end repeat\n  if i = 3 then\n    next repeat\n  end if\nend repeat",
        "repeat with i = 10 down to 1\n  x = i\nend repeat",
        "repeat with v in [1, 2, 3]\n  if v then\n    exit repeat\n  end if\n  x = v\nend repeat",
        "case a of\n  1:\n    b = 1\n  2, 3, 4:\n    b = 2\nend case",
        "case a.b of\n  #x:\n  \"y\", #z:\n    b = 1\n  otherwise:\n    b = 2\nend case\nc = 3",
        "case a of\n  1:\n    case b of\n      2:\n        return 5\n      otherwise:\n        exit\n    end case\n  otherwise:\n    return\nend case",
        "tell window(\"x\")\n  foo(1)\n  x = bar()\nend tell\nfoo(2)",
        "x = the timer + the long time & the mouseLoc\nthe itemDelimiter = \",\"\nset the colorDepth to 32",
        "x = _movie.frame\n_player.quit()\ny = _key.keyCode",
        "x = VOID\ny = PI\nz = SPACE & TAB & ENTER & BACKSPACE & QUOTE & RETURN & EMPTY",
        "return a\nexit\nreturn",
        "x = 0.5\ny = 0.1\nz = 3.0\nw = 123456.789",
        "foo()\nfoo(1, [2])\nx = foo(bar(1), baz())\nme.go()\ntest(1)",
    ];
    for source in cases {
        assert_stable(source, &[]);
    }
    assert_stable("pList[1] = pCount + 1\npCount = pList.count\nme.pCount = 3", &["pCount", "pList"]);
}

#[test]
fn patch_file_header() {
    let text = "-- lingo-patch 1\n-- script: Main/5 (Game Logic)\n-- handler: DrawScene\n-- stock: abc123\n-- enabled: no\n-- why: Shows the eyepatch.\n--   It is a head piece.\non DrawScene\nend\n";
    let patch = PatchFile::parse(text).unwrap();
    assert_eq!((patch.cast.as_str(), patch.member, patch.member_name.as_str(), patch.handler.as_str()), ("Main", 5, "Game Logic", "DrawScene"));
    assert_eq!(patch.stock.as_deref(), Some("abc123"));
    assert!(!patch.enabled);
    assert_eq!(patch.why, "Shows the eyepatch. It is a head piece.");
    assert_eq!(patch.id(), "Main/5/DrawScene");
    // A cast name may contain a slash-free space; the member number is what follows the last slash.
    assert_eq!(PatchFile::parse(&text.replace("Main/5", "Hireling Draw/3")).unwrap().cast, "Hireling Draw");
    assert!(PatchFile::parse("on DrawScene\nend\n").unwrap_err().contains("lingo-patch"));
    assert!(PatchFile::parse(&text.replace("-- why: Shows the eyepatch.\n--   It is a head piece.\n", "")).unwrap_err().contains("why"));
    assert!(PatchFile::parse(&text.replace("-- stock: abc123\n", "")).unwrap_err().contains("stock"));
    assert!(PatchFile::parse(&text.replace("-- stock: abc123\n", "-- new: yes\n")).unwrap().new_handler);
    assert!(PatchFile::parse(&text.replace("lingo-patch 1", "lingo-patch 2")).unwrap_err().contains("format"));
}

#[test]
fn patch_file_of_edits() {
    use vm_rust::director::lingo::compiler::movie::apply_edits;
    let head = "-- lingo-patch 1\n-- script: Scripts/11 (ITEM DRAW)\n-- handler: DrawItems\n-- stock: abc\n-- why: More object slots.\n";
    let patch = PatchFile::parse(&format!("{}@@ replace\n  repeat with x = 230 to 249\n@@ with\n  repeat with n = 1 to 40\n    x = ObjSprite(n)\n@@ end\n\n-- a note between edits\n@@ replace all\n  xxx = x - 229\n@@ with\n  xxx = n\n@@ end\n", head)).unwrap();
    assert_eq!(patch.edits.len(), 2);
    assert_eq!((patch.edits[0].count, patch.edits[1].count), (Some(1), None));
    let stock = "on DrawItems\n  repeat with x = 230 to 249\n    xxx = x - 229\n    foo(xxx)\n    xxx = x - 229\n  end repeat\nend\n";
    let out = apply_edits(stock, &patch.edits).unwrap();
    assert_eq!(out, "on DrawItems\n  repeat with n = 1 to 40\n    x = ObjSprite(n)\n  xxx = n\n    foo(xxx)\n  xxx = n\n  end repeat\nend\n");
    // An edit must match as often as it says: the movie's text is the reference, and a miss is an error.
    assert!(apply_edits("on DrawItems\nend\n", &patch.edits).unwrap_err().contains("matches the movie's handler 0 time(s)"));
    let twice = "on a\n  repeat with x = 230 to 249\n  end repeat\n  repeat with x = 230 to 249\n  end repeat\nend\n";
    assert!(apply_edits(twice, &patch.edits[..1]).unwrap_err().contains("2 time(s); it says 1"));
    // Several lines match as a run, whatever their indentation.
    let multi = PatchFile::parse(&format!("{}@@ replace 2\nif a then\n  b = 1\n@@ with\nif a then\n  b = 2\n@@ end\n", head)).unwrap();
    assert_eq!(apply_edits("on h\n    if a then\n        b = 1\n    end if\n  if a then\n    b = 1\n  end if\nend\n", &multi.edits).unwrap().matches("b = 2").count(), 2);
    // An empty `with` deletes the lines.
    let delete = PatchFile::parse(&format!("{}@@ replace\n  foo(xxx)\n@@ with\n@@ end\n", head)).unwrap();
    assert!(!apply_edits(stock, &delete.edits).unwrap().contains("foo"));
    // Malformed files say where.
    for (bad, want) in [("@@ replace\n  x\n", "no `@@ end`"), ("@@ replace\n@@ with\n@@ end\n", "names no lines"), ("@@ with\n", "follows the lines"), ("@@ replace\n x\n@@ with\n y\n@@ end\nstray = 1\n", "every line belongs"), ("@@ replace 0\n x\n@@ with\n@@ end\n", "a number"), ("@@ frobnicate\n", "expected")] {
        let err = PatchFile::parse(&format!("{}{}", head, bad)).unwrap_err();
        assert!(err.contains(want), "{} -> {}", bad, err);
    }
    assert!(PatchFile::parse(&format!("{}@@ replace\n x\n@@ with\n y\n@@ end\n", head.replace("-- stock: abc\n", "-- new: yes\n"))).unwrap_err().contains("new handler"));
}

#[test]
fn secrets_rule() {
    assert_eq!(redact_strings("x = foo(\"abc\", 5) & \"d\"\ny = \"e\"\n"), "x = foo(\"<redacted>\", 5) & \"<redacted>\"\ny = \"<redacted>\"\n");
    // Which names are connection code is the game's to say; with no config, none is.
    assert!(static_secret_problems("x = baDecryptText(a, b)").is_empty());
    set_tool_config(ToolConfig::parse(r#"{"connectionCode": {"nameContains": ["Crypt"], "names": ["connectToNetServer"]}}"#).unwrap());
    assert!(static_secret_problems("x = baDecryptText(a, b)").len() == 1);
    assert!(static_secret_problems("gConnect.connectToNetServer(a, b)").len() == 1);
    assert!(static_secret_problems("x = EncryptCd").len() == 1);
    // In a comment or a string the words are harmless.
    assert!(static_secret_problems("-- not baDecryptText\nx = \"connectToNetServer\"").is_empty());
    assert!(static_secret_problems("x = 1\nDrawScene(x)").is_empty());
}

#[test]
fn replacing_a_handler_in_a_script() {
    let stock_source = "on test a\n  x = a + 1\n  return x\nend";
    let first = compile(stock_source);
    let (mut lctx, version, multiplier) = as_script(&first, "test", &[]);
    let print = {
        let chunk = &lctx.scripts[&1];
        let trip = stock::round_trip(&chunk.handlers[0], chunk, &lctx, version, multiplier);
        assert!(trip.identical());
        patch::fingerprint(&trip.source, &trip.stock)
    };
    let request = |source: &'static str, stock: Option<&'static str>, new_handler: bool, handler: &'static str| PatchRequest { handler, source, stock, new_handler };

    // The wrong fingerprint: the patch was written for another handler.
    let refused = patch::prepare(&lctx, 1, &request("on test a\n  return 5\nend", Some("0000"), false, "test"), version, multiplier).err().unwrap();
    assert_eq!(refused.kind(), "movie-changed");
    // A source that does not compile.
    let refused = patch::prepare(&lctx, 1, &request("on test a\n  return (\nend", None, false, "test"), version, multiplier).err().unwrap();
    assert!(matches!(refused, Refusal::Compile { line: 2, .. }), "{:?}", refused);
    // A handler the script does not have.
    assert_eq!(patch::prepare(&lctx, 1, &request("on nope\nend", None, false, "nope"), version, multiplier).err().unwrap().kind(), "not-found");
    // The source is for another handler than the patch says.
    assert_eq!(patch::prepare(&lctx, 1, &request("on other\nend", None, false, "test"), version, multiplier).err().unwrap().kind(), "invalid");
    // An older movie format is refused outright.
    assert_eq!(patch::prepare(&lctx, 1, &request("on test\nend", None, false, "test"), 700, 8).err().unwrap().kind(), "invalid");

    // The gate: a handler whose bytes the compiler cannot reproduce must not be replaced.
    {
        let mut broken = lctx.clone();
        let chunk = broken.scripts.get_mut(&1).unwrap();
        // `x = a + 1` becomes `x = a - 1` with a stray extra instruction no source produces.
        chunk.handlers[0].bytecode_array.insert(0, vm_rust::director::chunks::handler::Bytecode::new(vm_rust::director::lingo::opcode::OpCode::Swap, 0, 0));
        for (i, bc) in chunk.handlers[0].bytecode_array.iter_mut().enumerate().skip(1) {
            bc.pos += 1;
            let _ = i;
        }
        let refused = patch::prepare(&broken, 1, &request("on test a\n  return 5\nend", None, false, "test"), version, multiplier).err().unwrap();
        assert_eq!(refused.kind(), "gate", "{}", refused);
    }

    // A good patch: new names and a new literal are added, the handler is replaced, the old one comes back out.
    let source: &'static str = "on test a\n  y = a & \"!\"\n  return y\nend";
    let leaked: &'static str = Box::leak(print.clone().into_boxed_str());
    let prepared = patch::prepare(&lctx, 1, &request(source, Some(leaked), false, "test"), version, multiplier).unwrap();
    assert_eq!(prepared.names[prepared.names_before..], ["y".to_string()]);
    assert_eq!(prepared.new_literals, vec![Literal::Str("!".into())]);
    let names_before = lctx.names.len();
    let original = patch::commit(&mut lctx, &prepared).unwrap();
    assert_eq!(stock::handler_bytes(&original), first.handler.bytecode);
    assert_eq!(lctx.names.len(), names_before + 1);
    let chunk = &lctx.scripts[&1];
    let now = stock::handler_source(&chunk.handlers[0], chunk, &lctx, version, multiplier);
    assert!(now.contains("y = a & \"!\""), "{}", now);
    // The replaced handler is itself a handler the compiler can read back.
    assert!(stock::round_trip(&chunk.handlers[0], chunk, &lctx, version, multiplier).identical());

    // A new handler, which may call the existing one by position.
    let prepared = patch::prepare(&lctx, 1, &request("on extra n\n  return test(n) & test(n)\nend", None, true, "extra"), version, multiplier).unwrap();
    assert!(prepared.is_new);
    assert!(patch::commit(&mut lctx, &prepared).is_none());
    assert_eq!(lctx.scripts[&1].handlers.len(), 2);
    // Saying `new` for a handler that exists is refused.
    assert_eq!(patch::prepare(&lctx, 1, &request("on test\nend", None, true, "test"), version, multiplier).err().unwrap().kind(), "invalid");
}

// ---- on a real movie ----

fn movie_path() -> Option<String> {
    // The game's tool config, when the caller has one (ROTL_LINGO_CONFIG): which handlers are connection code.
    if let Ok(config) = std::env::var("ROTL_LINGO_CONFIG") {
        if let Ok(text) = std::fs::read_to_string(&config) {
            set_tool_config(ToolConfig::parse(&text).unwrap());
        }
    }
    match std::env::var("ROTL_MOVIE") {
        Ok(path) if !path.is_empty() => Some(path),
        _ => {
            eprintln!("skipped: set ROTL_MOVIE to the movie file");
            None
        }
    }
}

#[test]
fn every_handler_of_the_movie_recompiles_to_its_own_bytes() {
    let Some(path) = movie_path() else { return };
    let _player = vm_rust::player::testing::TestPlayer::new();
    let bytes = std::fs::read(&path).unwrap();
    let file = vm_rust::director::file::read_director_file_bytes(&bytes, "movie.dcr", "file:///").unwrap();
    let movie = vm_rust::director::lingo::compiler::movie::Movie::new(file);
    let mut total = 0;
    let mut failures = Vec::new();
    for script in &movie.scripts {
        for (index, handler) in movie.chunk(script).handlers.iter().enumerate() {
            total += 1;
            if let Some(why) = movie.round_trip(script, handler).refusal() {
                failures.push(format!("{}/{}/{}: {}", script.cast, script.member, script.handlers[index], why));
            }
        }
    }
    eprintln!("round trip: {} of {} handlers identical", total - failures.len(), total);
    assert!(total > 0);
    assert!(failures.is_empty(), "{} handler(s) do not recompile to the movie's bytes:\n{}", failures.len(), failures.join("\n"));

    // No tool output shows the connection code's strings.
    let mut sensitive = 0;
    for script in &movie.scripts {
        for handler in &movie.chunk(script).handlers {
            if movie.handler_is_sensitive(script, handler) {
                sensitive += 1;
                let shown = movie.source(script, handler, false);
                for literal in vm_rust::director::lingo::compiler::movie::string_literals(&shown) {
                    assert_eq!(literal, "<redacted>");
                }
            }
        }
    }
    eprintln!("{} handler(s) treated as connection code (by ROTL_LINGO_CONFIG, when set)", sensitive);
}

vm_rust::native_e2e_test!(handlers_are_replaced_in_the_player, |player| async move {
    use vm_rust::player::lingo_patch;
    use vm_rust::player::reserve_player_mut;
    use vm_rust::player::symbols::symbol::Symbol;
    use vm_rust::player::testing_shared::TestHarness;

    let Some(path) = movie_path() else { return Ok(()) };
    let global = |name: &str| reserve_player_mut(|p| p.globals.get(&Symbol::from_str(name)).map(|r| p.get_datum(r).string_value().unwrap_or_default()).unwrap_or_else(|| "(none)".into()));

    // Any movie will do: take the first movie script, and its first handler that is not connection code.
    // Nothing here names a handler of one game; a game's own repository tests its own patches.
    let (cast, member, handler, stock_source, print) = {
        let bytes = std::fs::read(&path).unwrap();
        let file = vm_rust::director::file::read_director_file_bytes(&bytes, "movie.dcr", "file:///").unwrap();
        let movie = vm_rust::director::lingo::compiler::movie::Movie::new(file);
        let mut found = None;
        for script in movie.scripts.iter().filter(|s| s.kind == "movie" && s.member > 0) {
            for (index, h) in movie.chunk(script).handlers.iter().enumerate() {
                if !movie.handler_is_sensitive(script, h) && movie.round_trip(script, h).identical() {
                    found = Some((script.cast.clone(), script.member, script.handlers[index].clone(), movie.source(script, h, true), movie.fingerprint(script, h)));
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        found.expect("the movie has a movie script with a handler")
    };
    let added = "on LingoPatchTestAdded a\n  return a & \"!\"\nend";
    let manifest = serde_json::json!({ "version": 1, "patches": [
        { "cast": cast, "member": member, "handler": handler, "stock": print, "source": stock_source },
        { "cast": cast, "member": member, "handler": handler, "stock": "00ff", "source": stock_source },
        { "cast": cast, "member": member, "handler": "NoSuchHandlerHere", "source": "on NoSuchHandlerHere\nend" },
        { "cast": "No Such Cast", "member": 1, "handler": "x", "source": "on x\nend" },
        { "cast": cast, "member": member, "handler": handler, "source": format!("on {}\n  x = (\nend", handler) },
        { "cast": cast, "member": member, "handler": "LingoPatchTestAdded", "new": true, "source": added }
    ] }).to_string();

    // Given before the movie is loaded, the manifest waits for it.
    let waiting = reserve_player_mut(|p| lingo_patch::set_patches(p, &manifest));
    assert_eq!(waiting.state, "pending");
    player.load_movie(&path).await;
    let status = lingo_patch::status();
    assert_eq!(status.state, "applied");
    let r = &status.results;
    assert!(r[0].ok && !r[0].late, "{:?}", r[0]);
    assert_eq!(r[0].bytes, r[0].stock_bytes, "the stock source compiles to the stock size");
    assert_eq!(r[1].refused.as_deref(), Some("movie-changed"));
    assert_eq!(r[2].refused.as_deref(), Some("not-found"));
    assert_eq!(r[3].refused.as_deref(), Some("not-found"));
    assert_eq!(r[4].refused.as_deref(), Some("compile"));
    assert!(r[4].error.clone().unwrap().contains("line 2"));
    assert!(r[5].ok);
    assert_eq!((status.applied, status.refused), (2, 4));

    // The added handler is callable like one of the movie's own.
    player.eval("LingoPatchTestOut = LingoPatchTestAdded(\"hi\")").await.map_err(|e| e.message)?;
    assert_eq!(global("LingoPatchTestOut"), "hi!");

    // Clearing takes it away again.
    let cleared = reserve_player_mut(|p| lingo_patch::clear_patches(p));
    assert_eq!(cleared.state, "none");
    assert!(player.eval("LingoPatchTestOut = LingoPatchTestAdded(\"hi\")").await.is_err(), "the added handler is gone");

    // Given after the movie is loaded, a manifest is applied at once and says so.
    let late = reserve_player_mut(|p| lingo_patch::set_patches(p, &manifest));
    assert!(late.results[0].ok && late.results[0].late);
    player.eval("LingoPatchTestOut = LingoPatchTestAdded(\"again\")").await.map_err(|e| e.message)?;
    assert_eq!(global("LingoPatchTestOut"), "again!");

    // A manifest this player does not understand changes nothing and says why.
    let bad = reserve_player_mut(|p| lingo_patch::set_patches(p, "{\"version\": 99, \"patches\": []}"));
    assert!(bad.error.unwrap().contains("version 99"));
    let bad = reserve_player_mut(|p| lingo_patch::set_patches(p, "not json"));
    assert!(bad.error.unwrap().contains("not valid JSON"));
    assert!(player.eval("LingoPatchTestOut = LingoPatchTestAdded(\"x\")").await.is_err(), "a bad manifest leaves the stock movie");
    reserve_player_mut(|p| lingo_patch::clear_patches(p));
    Ok(())
});
