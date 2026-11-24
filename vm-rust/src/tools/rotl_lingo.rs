//! `rotl-lingo`: read, check and compile the Lingo of a Director movie, from the command line.
//!
//! A developer tool. It never changes the movie file and never talks to a server; the only files it
//! writes are the patch file `new` creates and the manifest `manifest` builds.
//!
//!   rotl-lingo list       [--json]                         every script and its handlers
//!   rotl-lingo show       <cast>/<member>/<handler>        a handler's source (--bytecode for the listing)
//!   rotl-lingo search     <text> [--regex-free]            lines of source containing the text
//!   rotl-lingo roundtrip  [--failures]                     recompile every handler and compare with the movie
//!   rotl-lingo new        <cast>/<member>/<handler> -o f   start a patch file from the handler's own source
//!   rotl-lingo compile    <file.ls>                        check one patch file; --bytecode shows what it compiles to
//!   rotl-lingo diff       <file.ls>                        the patch against the movie's handler, source and bytecode
//!   rotl-lingo check      <file-or-dir>...                 check patch files (what the build runs); exit 1 on any problem
//!   rotl-lingo manifest   <dir> -o manifest.json           the manifest of the enabled patches, for the player
//!                         [--include-disabled]             (also the ones not enabled: for a rig, never for a release)
//!   rotl-lingo run        [--patch f.ls]... --eval "..."   load the movie in a native player, apply patches, run Lingo
//!                         [--global Name]... [--offline]   (--global prints a global; --offline stubs the connection)
//!
//! The movie comes from `--movie <path>` or `$ROTL_LINGO_MOVIE`. `--json` gives machine-readable output.
//! `<member>` is the member's number, or its name when that is unique in the cast.
//!
//! A patch file is one whole handler, or a list of edits to the stock handler (`@@ replace` / `@@ with` / `@@ end`;
//! see `Edit` in compiler/movie.rs). `new --edits` starts the second kind.
//!
//! Handlers that deal with the connection have their strings replaced in every output, and patches for them
//! are refused (see compiler/movie.rs).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use vm_rust::director::file::read_director_file_bytes;
use vm_rust::director::lingo::compiler::movie::{redact_strings, secret_problems, static_secret_problems, Movie, PatchFile, ScriptInfo};
use vm_rust::director::lingo::compiler::patch::{fingerprint, Refusal};
use vm_rust::director::lingo::compiler::stock;

struct Args {
    command: String,
    positional: Vec<String>,
    movie: Option<String>,
    json: bool,
    bytecode: bool,
    failures: bool,
    out: Option<String>,
    why: Option<String>,
    patches: Vec<String>,
    evals: Vec<String>,
    offline: bool,
    all: bool,
    static_only: bool,
    edits: bool,
}

fn usage() -> ! {
    eprintln!("usage: rotl-lingo <list|show|search|roundtrip|new|compile|diff|check|manifest|run> [args] [--movie <movie>] [--json]\nSee the top of vm-rust/src/tools/rotl_lingo.rs or docs/lingo-compiler.md in NodeJS-ROTL.");
    std::process::exit(2);
}

fn parse_args() -> Args {
    let mut raw = std::env::args().skip(1);
    let Some(command) = raw.next() else { usage() };
    let mut args = Args { command, positional: Vec::new(), movie: std::env::var("ROTL_LINGO_MOVIE").ok().filter(|s| !s.is_empty()), json: false, bytecode: false, failures: false, out: None, why: None, patches: Vec::new(), evals: Vec::new(), offline: false, all: false, static_only: false, edits: false };
    let mut options_done = false;
    while let Some(arg) = raw.next() {
        // After `--` everything is a plain argument, so a search text may start with a dash.
        if options_done {
            args.positional.push(arg);
            continue;
        }
        if arg == "--" {
            options_done = true;
            continue;
        }
        let mut value = |name: &str| raw.next().unwrap_or_else(|| fail(2, &format!("{} needs a value", name)));
        match arg.as_str() {
            "--movie" => args.movie = Some(value("--movie")),
            "--json" => args.json = true,
            "--bytecode" => args.bytecode = true,
            "--failures" => args.failures = true,
            "--all" => args.all = true,
            "--offline" => args.offline = true,
            "--static" => args.static_only = true,
            "--edits" => args.edits = true,
            // In `manifest`: include patch files that say `enabled: no` (for trying one on a rig).
            "--include-disabled" => args.all = true,
            "-o" | "--out" => args.out = Some(value("-o")),
            "--why" => args.why = Some(value("--why")),
            "--patch" => args.patches.push(value("--patch")),
            "--eval" => args.evals.push(value("--eval")),
            // In `run`, read a global after the Lingo before it has run: `--eval "Foo()" --global Result`.
            "--global" => args.evals.push(format!("\u{1}{}", value("--global"))),
            "-h" | "--help" => usage(),
            _ if arg.starts_with("--") => fail(2, &format!("unknown option {}", arg)),
            _ => args.positional.push(arg),
        }
    }
    args
}

fn fail(code: i32, message: &str) -> ! {
    eprintln!("rotl-lingo: {}", message);
    std::process::exit(code);
}

fn load_movie(args: &Args) -> Movie {
    let Some(path) = &args.movie else { fail(2, "no movie: pass --movie <path> or set ROTL_LINGO_MOVIE") };
    let bytes = std::fs::read(path).unwrap_or_else(|e| fail(2, &format!("cannot read the movie {}: {}", path, e)));
    let name = Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "movie.dcr".to_string());
    let file = read_director_file_bytes(&bytes, &name, "file:///").unwrap_or_else(|e| fail(2, &format!("cannot read {} as a Director movie: {}", path, e)));
    Movie::new(file)
}

/// `<cast>/<member>/<handler>`; the cast name may itself contain no slash.
fn split_ref(reference: &str, want_handler: bool) -> (String, String, Option<String>) {
    let parts: Vec<&str> = reference.split('/').collect();
    match (parts.len(), want_handler) {
        (3, _) => (parts[0].to_string(), parts[1].to_string(), Some(parts[2].to_string())),
        (2, false) => (parts[0].to_string(), parts[1].to_string(), None),
        _ => fail(2, &format!("`{}` is not <cast>/<member>{}", reference, if want_handler { "/<handler>" } else { "[/<handler>]" })),
    }
}

fn script_json(movie: &Movie, script: &ScriptInfo) -> Value {
    let chunk = movie.chunk(script);
    let lctx = movie.lctx(script);
    let handlers: Vec<Value> = chunk
        .handlers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let args: Vec<String> = h.argument_name_ids.iter().filter_map(|id| lctx.names.get(*id as usize).cloned()).collect();
            json!({ "name": script.handlers[i], "args": args, "bytes": stock::handler_bytes(h).len(), "sensitive": movie.handler_is_sensitive(script, h) })
        })
        .collect();
    json!({ "cast": script.cast, "member": script.member, "name": script.member_name, "kind": script.kind, "ref": format!("{}/{}", script.cast, script.member), "handlers": handlers })
}

fn cmd_list(args: &Args) {
    let movie = load_movie(args);
    if args.json {
        let scripts: Vec<Value> = movie.scripts.iter().map(|s| script_json(&movie, s)).collect();
        let handlers: usize = movie.scripts.iter().map(|s| s.handlers.len()).sum();
        println!("{}", json!({ "scripts": scripts, "scriptCount": movie.scripts.len(), "handlerCount": handlers }));
        return;
    }
    for script in &movie.scripts {
        println!("{}/{}  {}  [{}]  {}", script.cast, script.member, if script.member_name.is_empty() { "(unnamed)" } else { &script.member_name }, script.kind, script.handlers.join(", "));
    }
}

fn bytecode_lines(movie: &Movie, script: &ScriptInfo, bytes: &[u8]) -> Vec<String> {
    stock::disassemble(bytes, &movie.lctx(script).names)
}

fn cmd_show(args: &Args) {
    let movie = load_movie(args);
    let Some(reference) = args.positional.first() else { fail(2, "show needs <cast>/<member>[/<handler>]") };
    let (cast, member, handler) = split_ref(reference, false);
    let script = movie.find_script(&cast, &member).unwrap_or_else(|e| fail(1, &e));
    let chunk = movie.chunk(script);
    let lctx = movie.lctx(script);
    let wanted: Vec<usize> = match &handler {
        Some(name) => vec![movie.find_handler(script, name).unwrap_or_else(|e| fail(1, &e)).0],
        None => (0..chunk.handlers.len()).collect(),
    };
    let mut out = Vec::new();
    for index in wanted {
        let h = &chunk.handlers[index];
        let sensitive = movie.handler_is_sensitive(script, h);
        let source = movie.source(script, h, false);
        let trip = movie.round_trip(script, h);
        let mut entry = json!({
            "ref": format!("{}/{}/{}", script.cast, script.member, script.handlers[index]),
            "cast": script.cast, "member": script.member, "script": script.member_name, "kind": script.kind,
            "handler": script.handlers[index],
            "source": source,
            "sensitive": sensitive,
            "editable": trip.identical() && !sensitive,
            "stock": fingerprint(&trip.source, &trip.stock),
            "bytes": trip.stock.len(),
            "scriptGlobals": stock::script_global_names(chunk, lctx),
            "properties": stock::property_names(chunk, lctx),
        });
        if let Some(why) = trip.refusal() {
            entry["refusal"] = json!(why);
        } else if sensitive {
            entry["refusal"] = json!("it handles the connection: its strings are hidden and it cannot be patched");
        }
        if args.bytecode {
            entry["bytecode"] = json!(bytecode_lines(&movie, script, &trip.stock));
        }
        out.push(entry);
    }
    if args.json {
        println!("{}", if handler.is_some() { out.remove(0) } else { json!({ "script": script_json(&movie, script), "handlers": out }) });
        return;
    }
    for entry in out {
        println!("-- {}  ({} script \"{}\"{})", entry["ref"].as_str().unwrap(), script.kind, script.member_name, if entry["sensitive"].as_bool().unwrap() { "; connection code: strings hidden, not patchable" } else { "" });
        let globals: Vec<String> = serde_json::from_value(entry["scriptGlobals"].clone()).unwrap_or_default();
        if !globals.is_empty() {
            println!("-- script globals: {}", globals.join(", "));
        }
        let properties: Vec<String> = serde_json::from_value(entry["properties"].clone()).unwrap_or_default();
        if !properties.is_empty() {
            println!("-- script properties: {}", properties.join(", "));
        }
        print!("{}", entry["source"].as_str().unwrap());
        if args.bytecode {
            println!("-- bytecode ({} bytes)", entry["bytes"]);
            for line in entry["bytecode"].as_array().unwrap() {
                println!("{}", line.as_str().unwrap());
            }
        }
        println!();
    }
}

fn cmd_search(args: &Args) {
    let movie = load_movie(args);
    let Some(needle) = args.positional.first() else { fail(2, "search needs the text to look for") };
    let needle_lower = needle.to_lowercase();
    let limit = if args.all { usize::MAX } else { 200 };
    let mut hits = Vec::new();
    let mut total = 0usize;
    for script in &movie.scripts {
        for (index, h) in movie.chunk(script).handlers.iter().enumerate() {
            let source = movie.source(script, h, false);
            for (line_no, line) in source.lines().enumerate() {
                if line.to_lowercase().contains(&needle_lower) {
                    total += 1;
                    if hits.len() < limit {
                        hits.push(json!({ "ref": format!("{}/{}/{}", script.cast, script.member, script.handlers[index]), "script": script.member_name, "line": line_no + 1, "text": line.trim() }));
                    }
                }
            }
        }
    }
    if args.json {
        println!("{}", json!({ "query": needle, "total": total, "shown": hits.len(), "hits": hits }));
        return;
    }
    for hit in &hits {
        println!("{}:{}: {}", hit["ref"].as_str().unwrap(), hit["line"], hit["text"].as_str().unwrap());
    }
    if total > hits.len() {
        println!("... {} more (use --all)", total - hits.len());
    }
}

fn cmd_roundtrip(args: &Args) {
    let movie = load_movie(args);
    let mut total = 0;
    let mut identical = 0;
    let mut bytes_total = 0usize;
    let mut failures = Vec::new();
    for script in &movie.scripts {
        for (index, h) in movie.chunk(script).handlers.iter().enumerate() {
            total += 1;
            let trip = movie.round_trip(script, h);
            bytes_total += trip.stock.len();
            match trip.refusal() {
                None => identical += 1,
                Some(why) => failures.push(json!({ "ref": format!("{}/{}/{}", script.cast, script.member, script.handlers[index]), "why": why })),
            }
        }
    }
    if args.json {
        println!("{}", json!({ "handlers": total, "identical": identical, "bytes": bytes_total, "failures": failures }));
    } else {
        if args.failures || !failures.is_empty() {
            for f in &failures {
                println!("{}: {}", f["ref"].as_str().unwrap(), f["why"].as_str().unwrap());
            }
        }
        println!("{} of {} handlers recompile to the movie's exact bytes ({} bytes of bytecode)", identical, total, bytes_total);
    }
    if identical != total {
        std::process::exit(1);
    }
}

fn cmd_new(args: &Args) {
    let movie = load_movie(args);
    let Some(reference) = args.positional.first() else { fail(2, "new needs <cast>/<member>/<handler>") };
    let (cast, member, handler) = split_ref(reference, true);
    let handler = handler.unwrap();
    let script = movie.find_script(&cast, &member).unwrap_or_else(|e| fail(1, &e));
    let (index, h) = movie.find_handler(script, &handler).unwrap_or_else(|e| fail(1, &e));
    if movie.handler_is_sensitive(script, h) {
        fail(1, &format!("`{}` handles the connection; it cannot be patched from a file", handler));
    }
    let trip = movie.round_trip(script, h);
    if let Some(why) = trip.refusal() {
        fail(1, &format!("`{}` cannot be patched: {}", handler, why));
    }
    let why = args.why.clone().unwrap_or_else(|| "(say what this changes and why)".to_string());
    // --edits: a file of changes. It starts with one edit that changes nothing, to show the form.
    let body = if args.edits {
        let first = trip.source.lines().find(|l| !l.trim_start().starts_with("on ") && !l.trim_start().starts_with("global ") && !l.trim().is_empty()).unwrap_or("").trim().to_string();
        format!("@@ replace\n  {}\n@@ with\n  {}\n@@ end\n", first, first)
    } else {
        trip.source.clone()
    };
    let text = PatchFile::scaffold(script, &script.handlers[index], &fingerprint(&trip.source, &trip.stock), &body, &why);
    match &args.out {
        Some(path) => {
            if Path::new(path).exists() {
                fail(1, &format!("{} already exists", path));
            }
            if let Some(parent) = Path::new(path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(path, &text).unwrap_or_else(|e| fail(1, &format!("cannot write {}: {}", path, e)));
            if args.json {
                println!("{}", json!({ "ok": true, "file": path, "ref": format!("{}/{}/{}", script.cast, script.member, script.handlers[index]) }));
            } else {
                println!("wrote {} (enabled: no). Edit it, then: rotl-lingo compile {}", path, path);
            }
        }
        None => print!("{}", text),
    }
}

/// Everything `compile`, `check` and `manifest` need to know about one patch file.
fn examine(movie: Option<&Movie>, path: &Path) -> Value {
    let mut report = json!({ "file": path.to_string_lossy(), "ok": false, "problems": [], "warnings": [] });
    let mut problems: Vec<String> = Vec::new();
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            report["problems"] = json!([format!("cannot read the file: {}", e)]);
            return report;
        }
    };
    let patch = match PatchFile::parse(&text) {
        Ok(p) => p,
        Err(e) => {
            // Even a file whose header is wrong must not carry connection code.
            let mut all = vec![e];
            let secret = static_secret_problems(&text);
            report["secret"] = json!(!secret.is_empty());
            all.extend(secret);
            report["problems"] = json!(all);
            return report;
        }
    };
    report["id"] = json!(patch.id());
    report["cast"] = json!(patch.cast);
    report["member"] = json!(patch.member);
    report["handler"] = json!(patch.handler);
    report["enabled"] = json!(patch.enabled);
    report["new"] = json!(patch.new_handler);
    report["why"] = json!(patch.why);
    report["stock"] = json!(patch.stock);
    let Some(movie) = movie else {
        problems.extend(static_secret_problems(&patch.source));
        report["secret"] = json!(!problems.is_empty());
        report["ok"] = json!(problems.is_empty());
        report["problems"] = json!(problems);
        return report;
    };
    let secrets = secret_problems(movie, &patch);
    let has_secrets = !secrets.is_empty();
    // Set when the file must not be saved or committed at all, as opposed to merely not compiling yet.
    report["secret"] = json!(has_secrets);
    problems.extend(secrets);
    report["form"] = json!(if patch.edits.is_empty() { "handler" } else { "edits" });
    match movie.prepare(&patch) {
        Ok((prepared, resolved)) => {
            if !patch.edits.is_empty() {
                // What the edits make of the stock handler; compile errors give lines of this text.
                report["resolvedSource"] = json!(resolved);
            }
            report["bytes"] = json!(prepared.compiled_len);
            report["stockBytes"] = json!(prepared.stock_len);
            report["newNames"] = json!(prepared.names[prepared.names_before..].to_vec());
            report["newLiterals"] = json!(prepared.new_literals.len());
            report["warnings"] = json!(prepared.warnings);
            if let Ok(script) = movie.find_script(&patch.cast, &patch.member.to_string()) {
                if !patch.member_name.is_empty() && !patch.member_name.eq_ignore_ascii_case(&script.member_name) {
                    problems.push(format!("the header names the script \"{}\", but member {} of {} is \"{}\"", patch.member_name, patch.member, patch.cast, script.member_name));
                }
                let bytes = stock::handler_bytes(&prepared.handler);
                report["bytecode"] = json!(stock::disassemble(&bytes, &prepared.names));
                if let Ok((_, h)) = movie.find_handler(script, &patch.handler) {
                    let stock_bytes = stock::handler_bytes(h);
                    report["unchanged"] = json!(stock_bytes == bytes);
                    report["stockBytecode"] = json!(stock::disassemble(&stock_bytes, &movie.lctx(script).names));
                    report["stockSource"] = json!(movie.source(script, h, false));
                }
            }
            // The source goes into the manifest only when nothing is wrong with the file.
            if !has_secrets {
                report["source"] = json!(resolved);
            }
        }
        Err(refusal) => {
            if !patch.edits.is_empty() {
                // A compile error names a line of the edited handler, so show that text.
                if let Ok(resolved) = movie.resolved_source(&patch) {
                    report["resolvedSource"] = json!(resolved);
                }
            }
            report["refused"] = json!(refusal.kind());
            if let Refusal::Compile { line, col, .. } = &refusal {
                report["line"] = json!(line);
                report["col"] = json!(col);
            }
            problems.push(refusal.to_string());
        }
    }
    report["ok"] = json!(problems.is_empty());
    report["problems"] = json!(problems);
    report
}

fn patch_files(inputs: &[String]) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for path in paths {
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().map(|e| e == "ls").unwrap_or(false) {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    for input in inputs {
        let path = PathBuf::from(input);
        if path.is_dir() {
            walk(&path, &mut out);
        } else {
            out.push(path);
        }
    }
    out
}

fn print_report(report: &Value, show_bytecode: bool) {
    let file = report["file"].as_str().unwrap_or("");
    if report["ok"].as_bool().unwrap_or(false) {
        let state = if report["enabled"].as_bool().unwrap_or(false) { "enabled" } else { "not enabled" };
        let unchanged = if report["unchanged"].as_bool().unwrap_or(false) { ", identical to the movie's handler" } else { "" };
        println!("ok   {}  {}  {} -> {} bytes ({}{})", file, report["id"].as_str().unwrap_or(""), report["stockBytes"], report["bytes"], state, unchanged);
    } else {
        println!("FAIL {}  {}", file, report["id"].as_str().unwrap_or(""));
    }
    for problem in report["problems"].as_array().into_iter().flatten() {
        println!("     {}", problem.as_str().unwrap_or(""));
    }
    for warning in report["warnings"].as_array().into_iter().flatten() {
        println!("     warning: {}", warning.as_str().unwrap_or(""));
    }
    if show_bytecode {
        for line in report["bytecode"].as_array().into_iter().flatten() {
            println!("{}", line.as_str().unwrap_or(""));
        }
    }
}

/// Reports without the bulky fields, unless the caller asked for them.
fn slim(mut report: Value, keep_bytecode: bool, keep_source: bool) -> Value {
    if let Some(map) = report.as_object_mut() {
        if !keep_bytecode {
            map.remove("bytecode");
            map.remove("stockBytecode");
        }
        if !keep_source {
            map.remove("source");
            map.remove("stockSource");
            map.remove("resolvedSource");
        }
    }
    report
}

fn cmd_compile(args: &Args) {
    let Some(file) = args.positional.first() else { fail(2, "compile needs a patch file") };
    let movie = load_movie(args);
    let report = examine(Some(&movie), Path::new(file));
    let ok = report["ok"].as_bool().unwrap_or(false);
    if args.json {
        println!("{}", slim(report, args.bytecode, false));
    } else {
        print_report(&report, args.bytecode);
    }
    std::process::exit(if ok { 0 } else { 1 });
}

/// A plain line diff (longest common subsequence), enough for two versions of one handler.
fn line_diff(a: &[String], b: &[String]) -> Vec<String> {
    let (n, m) = (a.len(), b.len());
    let mut table = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if a[i] == b[j] { table[i + 1][j + 1] + 1 } else { table[i + 1][j].max(table[i][j + 1]) };
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(format!("  {}", a[i]));
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            out.push(format!("- {}", a[i]));
            i += 1;
        } else {
            out.push(format!("+ {}", b[j]));
            j += 1;
        }
    }
    while i < n {
        out.push(format!("- {}", a[i]));
        i += 1;
    }
    while j < m {
        out.push(format!("+ {}", b[j]));
        j += 1;
    }
    out
}

/// Only the changed lines of a diff, each with a little of what surrounds it.
fn with_context(diff: &[String], context: usize) -> Vec<String> {
    let changed: Vec<usize> = diff.iter().enumerate().filter(|(_, l)| !l.starts_with("  ")).map(|(i, _)| i).collect();
    let mut out = Vec::new();
    let mut last_shown: Option<usize> = None;
    for (i, line) in diff.iter().enumerate() {
        if changed.iter().any(|c| i + context >= *c && i <= *c + context) {
            if let Some(last) = last_shown {
                if i > last + 1 {
                    out.push("  ...".to_string());
                }
            }
            out.push(line.clone());
            last_shown = Some(i);
        }
    }
    out
}

fn body_lines(source: &str) -> Vec<String> {
    source.lines().filter(|l| !l.trim_start().starts_with("--") && !l.trim().is_empty()).map(|l| l.trim_end().to_string()).collect()
}

/// Bytecode listings without positions, so that a diff shows what changed and not what moved.
fn unpositioned(lines: &Value) -> Vec<String> {
    lines
        .as_array()
        .into_iter()
        .flatten()
        .map(|l| {
            let text = l.as_str().unwrap_or("");
            let rest = text.splitn(2, "] ").nth(1).unwrap_or(text);
            if rest.starts_with("jmp") || rest.starts_with("endrepeat") {
                rest.split(' ').next().unwrap_or(rest).to_string()
            } else {
                rest.to_string()
            }
        })
        .collect()
}

fn cmd_diff(args: &Args) {
    let Some(file) = args.positional.first() else { fail(2, "diff needs a patch file") };
    let movie = load_movie(args);
    let mut report = examine(Some(&movie), Path::new(file));
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let stock_source = report["stockSource"].as_str().unwrap_or("").to_string();
    let patched = report["resolvedSource"].as_str().map(|s| s.to_string()).unwrap_or(text);
    let source_diff = with_context(&line_diff(&body_lines(&stock_source), &body_lines(&patched)), 3);
    let bytecode_diff = with_context(&line_diff(&unpositioned(&report["stockBytecode"]), &unpositioned(&report["bytecode"])), 2);
    let ok = report["ok"].as_bool().unwrap_or(false);
    if args.json {
        let resolved = report["resolvedSource"].take();
        let mut out = slim(report, false, false);
        if !resolved.is_null() {
            out["resolvedSource"] = resolved;
        }
        out["sourceDiff"] = json!(source_diff);
        out["bytecodeDiff"] = json!(bytecode_diff);
        println!("{}", out);
    } else {
        print_report(&report, false);
        println!("--- source: movie (-) against patch (+)");
        for line in &source_diff {
            println!("{}", line);
        }
        if ok {
            println!("--- bytecode: movie (-) against patch (+)");
            for line in &bytecode_diff {
                println!("{}", line);
            }
        }
    }
    std::process::exit(if ok { 0 } else { 1 });
}

fn cmd_check(args: &Args) {
    if args.positional.is_empty() {
        fail(2, "check needs patch files or directories");
    }
    let files = patch_files(&args.positional);
    // --static checks what can be checked without the movie (the header and the secrets rule), for a commit hook.
    let movie = if args.static_only { None } else { Some(load_movie(args)) };
    let mut reports = Vec::new();
    let mut seen: Vec<(String, String)> = Vec::new();
    for file in &files {
        let mut report = examine(movie.as_ref(), file);
        if let (Some(id), true) = (report["id"].as_str().map(|s| s.to_lowercase()), report["enabled"].as_bool().unwrap_or(false)) {
            if let Some((_, other)) = seen.iter().find(|(seen_id, _)| *seen_id == id) {
                let mut problems: Vec<String> = serde_json::from_value(report["problems"].clone()).unwrap_or_default();
                problems.push(format!("{} is enabled for the same handler; only one patch per handler can be enabled", other));
                report["problems"] = json!(problems);
                report["ok"] = json!(false);
            } else {
                seen.push((id, file.to_string_lossy().to_string()));
            }
        }
        reports.push(report);
    }
    let bad = reports.iter().filter(|r| !r["ok"].as_bool().unwrap_or(false)).count();
    if args.json {
        let slimmed: Vec<Value> = reports.into_iter().map(|r| slim(r, false, false)).collect();
        println!("{}", json!({ "ok": bad == 0, "files": slimmed.len(), "failed": bad, "reports": slimmed }));
    } else {
        for report in &reports {
            print_report(report, false);
        }
        println!("{} patch file(s), {} with problems", reports.len(), bad);
    }
    std::process::exit(if bad == 0 { 0 } else { 1 });
}

fn cmd_manifest(args: &Args) {
    if args.positional.is_empty() {
        fail(2, "manifest needs the patch directory");
    }
    let movie = load_movie(args);
    let files = patch_files(&args.positional);
    let mut patches = Vec::new();
    let mut skipped = Vec::new();
    for file in &files {
        let report = examine(Some(&movie), file);
        if !report["ok"].as_bool().unwrap_or(false) {
            print_report(&report, false);
            fail(1, "the manifest is not written while a patch file has problems");
        }
        if !report["enabled"].as_bool().unwrap_or(false) && !args.all {
            skipped.push(report["file"].clone());
            continue;
        }
        let mut entry = json!({ "id": report["id"], "cast": report["cast"], "member": report["member"], "handler": report["handler"], "source": report["source"] });
        if report["new"].as_bool().unwrap_or(false) {
            entry["new"] = json!(true);
        } else {
            entry["stock"] = report["stock"].clone();
        }
        patches.push(entry);
    }
    let manifest = json!({ "version": 1, "patches": patches });
    let text = serde_json::to_string(&manifest).unwrap();
    match &args.out {
        Some(path) => std::fs::write(path, &text).unwrap_or_else(|e| fail(1, &format!("cannot write {}: {}", path, e))),
        None => println!("{}", text),
    }
    eprintln!("{} patch(es) in the manifest, {} not enabled", manifest["patches"].as_array().unwrap().len(), skipped.len());
}

fn cmd_run(args: &Args) {
    use vm_rust::director::lingo::datum::Datum;
    use vm_rust::player::datum_formatting::format_datum;
    use vm_rust::player::datum_ref::DatumRef;
    use vm_rust::player::script_ref::ScriptInstanceRef;
    use vm_rust::player::symbols::symbol::Symbol;
    use vm_rust::player::testing_shared::TestHarness;
    use vm_rust::player::virtual_scripts::{VirtualScriptHandler, VirtualScriptRegistry};
    use vm_rust::player::{reserve_player_mut, DirPlayer, ScriptError};

    /// Stands in for the Multiuser connection, so that handlers which send a message run without a server.
    struct OfflineConnection;
    impl VirtualScriptHandler for OfflineConnection {
        fn has_handler(&self, name: Symbol) -> bool {
            name.as_lower_str() == "sendnetmessage"
        }
        fn call_handler(&self, player: &mut DirPlayer, _instance: Option<&ScriptInstanceRef>, name: Symbol, _args: &Vec<DatumRef>) -> Result<Option<DatumRef>, ScriptError> {
            if name.as_lower_str() == "sendnetmessage" {
                return Ok(Some(player.alloc_datum(Datum::Int(0))));
            }
            Ok(None)
        }
    }

    let Some(movie_path) = args.movie.clone() else { fail(2, "no movie: pass --movie <path> or set ROTL_LINGO_MOVIE") };
    // The same checks as `check`, then the manifest the player itself reads.
    let mut entries = Vec::new();
    if !args.patches.is_empty() {
        let _reader = vm_rust::player::testing::TestPlayer::new();
        let movie = load_movie(args);
        for file in patch_files(&args.patches) {
            let report = examine(Some(&movie), &file);
            if !report["ok"].as_bool().unwrap_or(false) {
                if args.json {
                    println!("{}", json!({ "ok": false, "notRun": true, "error": "not run: a patch file has problems", "report": slim(report, false, false) }));
                    std::process::exit(1);
                }
                print_report(&report, false);
                fail(1, "not run: a patch file has problems");
            }
            let mut entry = json!({ "id": report["id"], "cast": report["cast"], "member": report["member"], "handler": report["handler"], "source": report["source"] });
            if report["new"].as_bool().unwrap_or(false) {
                entry["new"] = json!(true);
            } else {
                entry["stock"] = report["stock"].clone();
            }
            entries.push(entry);
        }
    }
    let manifest = json!({ "version": 1, "patches": entries }).to_string();
    let evals = args.evals.clone();
    let offline = args.offline;
    let json_out = args.json;
    let mut results = Vec::new();
    let mut status = Value::Null;
    vm_rust::player::testing::run_test(async {
        let mut player = vm_rust::player::testing::TestPlayer::new();
        reserve_player_mut(|p| {
            vm_rust::player::lingo_patch::set_patches(p, &manifest);
        });
        player.load_movie(&movie_path).await;
        status = serde_json::from_str(&vm_rust::player::lingo_patch::status_json()).unwrap_or(Value::Null);
        if offline {
            reserve_player_mut(|p| {
                let script = VirtualScriptRegistry::register(p, "RotlOfflineConnection", std::rc::Rc::new(OfflineConnection));
                let (_instance, datum) = VirtualScriptRegistry::create_instance(p, &script);
                p.globals.insert(Symbol::from_str("gConnect"), datum);
            });
        }
        for code in &evals {
            if let Some(name) = code.strip_prefix('\u{1}') {
                let text = reserve_player_mut(|p| match p.globals.get(&Symbol::from_str(name)).cloned() {
                    Some(value) => format_datum(&value, p),
                    None => "(no such global)".to_string(),
                });
                results.push(json!({ "global": name, "ok": true, "value": text }));
                continue;
            }
            let code = code.replace("\\n", "\n");
            match player.eval(&code).await {
                Ok(value) => {
                    let text = reserve_player_mut(|p| format_datum(&value, p));
                    results.push(json!({ "eval": code, "ok": true, "value": text }));
                }
                Err(e) => results.push(json!({ "eval": code, "ok": false, "error": e.message })),
            }
        }
    });
    let failed = results.iter().any(|r| !r["ok"].as_bool().unwrap_or(false)) || status["refused"].as_u64().unwrap_or(0) > 0;
    if json_out {
        println!("{}", json!({ "ok": !failed, "patches": status, "results": results }));
    } else {
        for r in status["results"].as_array().into_iter().flatten() {
            if r["ok"].as_bool().unwrap_or(false) {
                println!("patched {} ({} -> {} bytes)", r["id"].as_str().unwrap_or(""), r["stockBytes"], r["bytes"]);
            } else {
                println!("REFUSED {}: {}", r["id"].as_str().unwrap_or(""), r["error"].as_str().unwrap_or(""));
            }
        }
        for r in &results {
            if let Some(name) = r["global"].as_str() {
                println!("{} = {}", name, r["value"].as_str().unwrap_or(""));
            } else if r["ok"].as_bool().unwrap_or(false) {
                println!("{}\n  => {}", r["eval"].as_str().unwrap_or(""), r["value"].as_str().unwrap_or(""));
            } else {
                println!("{}\n  error: {}", r["eval"].as_str().unwrap_or(""), r["error"].as_str().unwrap_or(""));
            }
        }
    }
    // The player's own threads would keep the process alive.
    std::process::exit(if failed { 1 } else { 0 });
}

fn main() {
    // `rotl-lingo list | head` closes the pipe early; that is not an error worth a panic message.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if info.to_string().contains("Broken pipe") {
            std::process::exit(0);
        }
        default_hook(info);
    }));
    let args = parse_args();
    // Reading a movie parses its score, which evaluates behaviour parameters through the player.
    let needs_player = !(args.command == "run" || (args.command == "check" && args.static_only));
    let _player = if needs_player { Some(vm_rust::player::testing::TestPlayer::new()) } else { None };
    match args.command.as_str() {
        "list" => cmd_list(&args),
        "show" => cmd_show(&args),
        "search" => cmd_search(&args),
        "roundtrip" => cmd_roundtrip(&args),
        "new" => cmd_new(&args),
        "compile" => cmd_compile(&args),
        "diff" => cmd_diff(&args),
        "check" => cmd_check(&args),
        "manifest" => cmd_manifest(&args),
        "run" => {
            drop(_player);
            cmd_run(&args)
        }
        _ => usage(),
    }
    let _ = redact_strings;
}
