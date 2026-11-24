// A movie file's scripts, for tools: listing, source, the round trip, patch files and the secrets rule.
//
// Used by the `rotl-lingo` command (src/tools/rotl_lingo.rs) and by the tests. Nothing here runs in the
// player.

use crate::director::chunks::handler::HandlerDef;
use crate::director::chunks::script::ScriptChunk;
use crate::director::enums::ScriptType;
use crate::director::file::{get_variable_multiplier, DirectorFile};
use crate::director::lingo::datum::Datum;
use crate::director::lingo::opcode::OpCode;
use crate::director::lingo::script::ScriptContext;

use super::patch::{self, PatchRequest, Prepared, Refusal};
use super::stock;

/// One script of the movie: where it is and what is in it.
pub struct ScriptInfo {
    pub cast_index: usize,
    pub cast: String,
    pub script_id: u32,
    /// The cast member that holds the script; 0 when no member claims it.
    pub member: u32,
    pub member_name: String,
    /// `movie`, `parent`, `behavior`, `cast` (a script attached to a member) or `unknown`.
    pub kind: &'static str,
    pub handlers: Vec<String>,
}

pub struct Movie {
    pub file: DirectorFile,
    pub scripts: Vec<ScriptInfo>,
}

/// What a game tells the tools about its movie. Nothing about any one game is built in: the game's
/// repository keeps this as a JSON file and hands it to every tool (`--config`, `ROTL_LINGO_CONFIG`).
///
/// ```json
/// {
///   "connectionCode": { "nameContains": ["crypt"], "names": ["connectToNetServer"] },
///   "offlineConnection": { "global": "gConnect", "handlers": ["sendNetMessage"] }
/// }
/// ```
///
/// `connectionCode`: a handler that uses one of these names (calls it, reads or writes it, or is called
/// it) is sensitive. Its string literals may be keys or addresses, so no tool shows them and no tool
/// accepts a patch for it or a patch that uses those names.
///
/// `offlineConnection`: for `run --offline`, the global that holds the movie's server connection and
/// the handlers a stand-in for it answers, so that handlers which send a message run without a server.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct ToolConfig {
    #[serde(default, rename = "connectionCode")]
    pub connection_code: ConnectionCode,
    #[serde(default, rename = "offlineConnection")]
    pub offline_connection: Option<OfflineConnection>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct ConnectionCode {
    #[serde(default, rename = "nameContains")]
    pub name_contains: Vec<String>,
    #[serde(default)]
    pub names: Vec<String>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct OfflineConnection {
    pub global: String,
    #[serde(default)]
    pub handlers: Vec<String>,
}

impl ToolConfig {
    pub fn parse(text: &str) -> Result<ToolConfig, String> {
        let mut config: ToolConfig = serde_json::from_str(text).map_err(|e| format!("the tool config is not valid: {}", e))?;
        for list in [&mut config.connection_code.name_contains, &mut config.connection_code.names] {
            for name in list.iter_mut() {
                *name = name.to_ascii_lowercase();
            }
            list.retain(|n| !n.is_empty());
        }
        Ok(config)
    }
}

thread_local! {
    static CONFIG: std::cell::RefCell<ToolConfig> = std::cell::RefCell::new(ToolConfig::default());
}

/// Sets the game's config for this thread's tools. Without one, no name is sensitive.
pub fn set_tool_config(config: ToolConfig) {
    CONFIG.with(|c| *c.borrow_mut() = config);
}

pub fn tool_config() -> ToolConfig {
    CONFIG.with(|c| c.borrow().clone())
}

/// Whether a name belongs to the connection code, by the game's config.
pub fn name_is_sensitive(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    CONFIG.with(|c| {
        let c = c.borrow();
        c.connection_code.name_contains.iter().any(|part| lower.contains(part.as_str())) || c.connection_code.names.iter().any(|n| *n == lower)
    })
}

pub const REDACTED: &str = "<redacted>";

impl Movie {
    pub fn new(file: DirectorFile) -> Movie {
        let mut scripts = Vec::new();
        for (cast_index, cast) in file.casts.iter().enumerate() {
            let Some(lctx) = &cast.lctx else { continue };
            let mut ids: Vec<u32> = lctx.scripts.keys().copied().collect();
            ids.sort();
            for script_id in ids {
                let chunk = &lctx.scripts[&script_id];
                let mut member = 0;
                let mut member_name = String::new();
                let mut kind = "unknown";
                let mut owners: Vec<(&u32, &crate::director::chunks::cast_member::CastMemberDef)> =
                    cast.members.iter().filter(|(_, m)| m.chunk.member_info.as_ref().map(|i| i.header.script_id) == Some(script_id)).collect();
                owners.sort_by_key(|(number, _)| **number);
                if let Some((number, def)) = owners.first() {
                    member = **number;
                    member_name = def.chunk.member_info.as_ref().map(|i| i.name.clone()).unwrap_or_default();
                    kind = match def.chunk.specific_data.script_type() {
                        Some(ScriptType::Movie) => "movie",
                        Some(ScriptType::Parent) => "parent",
                        Some(ScriptType::Score) => "behavior",
                        Some(ScriptType::Member) => "cast",
                        Some(_) => "unknown",
                        None => "cast",
                    };
                }
                scripts.push(ScriptInfo { cast_index, cast: cast.name.clone(), script_id, member, member_name, kind, handlers: stock::handler_names(chunk, lctx) });
            }
        }
        Movie { file, scripts }
    }

    pub fn lctx(&self, script: &ScriptInfo) -> &ScriptContext {
        self.file.casts[script.cast_index].lctx.as_ref().unwrap()
    }

    pub fn chunk(&self, script: &ScriptInfo) -> &ScriptChunk {
        &self.lctx(script).scripts[&script.script_id]
    }

    pub fn version(&self, script: &ScriptInfo) -> (u16, u32) {
        let cast = &self.file.casts[script.cast_index];
        (cast.dir_version, get_variable_multiplier(cast.capital_x, cast.dir_version))
    }

    /// Finds a script by cast name and member number, or by cast name and member name.
    pub fn find_script(&self, cast: &str, member: &str) -> Result<&ScriptInfo, String> {
        let in_cast: Vec<&ScriptInfo> = self.scripts.iter().filter(|s| s.cast.eq_ignore_ascii_case(cast)).collect();
        if in_cast.is_empty() {
            return Err(format!("the movie has no cast library \"{}\" with scripts", cast));
        }
        if let Ok(number) = member.parse::<u32>() {
            return in_cast.into_iter().find(|s| s.member == number).ok_or_else(|| format!("member {} of \"{}\" has no script", number, cast));
        }
        let named: Vec<&ScriptInfo> = in_cast.into_iter().filter(|s| s.member_name.eq_ignore_ascii_case(member)).collect();
        match named.len() {
            0 => Err(format!("\"{}\" has no script member named \"{}\"", cast, member)),
            1 => Ok(named[0]),
            n => Err(format!("\"{}\" has {} script members named \"{}\"; use the member number", cast, n, member)),
        }
    }

    pub fn find_handler<'a>(&'a self, script: &'a ScriptInfo, handler: &str) -> Result<(usize, &'a HandlerDef), String> {
        let index = script.handlers.iter().position(|h| h.eq_ignore_ascii_case(handler)).ok_or_else(|| format!("script {}/{} has no handler `{}`", script.cast, script.member, handler))?;
        Ok((index, &self.chunk(script).handlers[index]))
    }

    /// Whether a handler touches the connection (see `name_is_sensitive`).
    pub fn handler_is_sensitive(&self, script: &ScriptInfo, handler: &HandlerDef) -> bool {
        let lctx = self.lctx(script);
        let chunk = self.chunk(script);
        if lctx.names.get(handler.name_id as usize).map(|n| name_is_sensitive(n)).unwrap_or(false) {
            return true;
        }
        handler.bytecode_array.iter().any(|bc| {
            let name = match bc.opcode {
                OpCode::LocalCall => chunk.handlers.get(bc.obj as usize).and_then(|h| lctx.names.get(h.name_id as usize)),
                OpCode::ExtCall | OpCode::TellCall | OpCode::ObjCall | OpCode::GetGlobal | OpCode::SetGlobal | OpCode::GetProp | OpCode::SetProp | OpCode::PushVarRef | OpCode::PushSymb | OpCode::GetObjProp | OpCode::SetObjProp | OpCode::GetChainedProp => {
                    lctx.names.get(bc.obj as usize)
                }
                _ => None,
            };
            name.map(|n| name_is_sensitive(n)).unwrap_or(false)
        })
    }

    pub fn round_trip(&self, script: &ScriptInfo, handler: &HandlerDef) -> stock::RoundTrip {
        let (version, multiplier) = self.version(script);
        stock::round_trip(handler, self.chunk(script), self.lctx(script), version, multiplier)
    }

    /// A handler's source. For a sensitive handler every string literal is replaced, unless `reveal`.
    pub fn source(&self, script: &ScriptInfo, handler: &HandlerDef, reveal: bool) -> String {
        let (version, multiplier) = self.version(script);
        let text = stock::handler_source(handler, self.chunk(script), self.lctx(script), version, multiplier);
        if !reveal && self.handler_is_sensitive(script, handler) {
            redact_strings(&text)
        } else {
            text
        }
    }

    pub fn fingerprint(&self, script: &ScriptInfo, handler: &HandlerDef) -> String {
        let (version, multiplier) = self.version(script);
        let text = stock::handler_source(handler, self.chunk(script), self.lctx(script), version, multiplier);
        patch::fingerprint(&text, &stock::handler_bytes(handler))
    }

    /// The string literals that only sensitive handlers use, for the check that keeps them out of patch
    /// files. A string that ordinary handlers use too ("OFF", an item name) is no secret.
    pub fn sensitive_literals(&self) -> Vec<String> {
        let mut sensitive: Vec<String> = Vec::new();
        let mut ordinary: std::collections::HashSet<String> = std::collections::HashSet::new();
        for script in &self.scripts {
            let chunk = self.chunk(script);
            for handler in &chunk.handlers {
                let is_sensitive = self.handler_is_sensitive(script, handler);
                for bc in &handler.bytecode_array {
                    if bc.opcode == OpCode::PushCons {
                        if let Some(Datum::String(s)) = chunk.literals.get(bc.obj as usize) {
                            if is_sensitive {
                                if !sensitive.contains(s) {
                                    sensitive.push(s.clone());
                                }
                            } else {
                                ordinary.insert(s.clone());
                            }
                        }
                    }
                }
            }
        }
        sensitive.retain(|s| !ordinary.contains(s));
        sensitive
    }

    /// The handler source a patch file stands for: the file itself, or the stock handler with the file's edits made.
    pub fn resolved_source(&self, patch_file: &PatchFile) -> Result<String, Refusal> {
        if patch_file.edits.is_empty() {
            return Ok(patch_file.source.clone());
        }
        let script = self.find_script(&patch_file.cast, &patch_file.member.to_string()).map_err(Refusal::NotFound)?;
        let (_, handler) = self.find_handler(script, &patch_file.handler).map_err(Refusal::NotFound)?;
        if self.handler_is_sensitive(script, handler) {
            return Err(Refusal::Invalid(format!("`{}` handles the connection; it cannot be patched", patch_file.handler)));
        }
        apply_edits(&self.source(script, handler, true), &patch_file.edits).map_err(Refusal::Invalid)
    }

    /// Checks a patch against the movie without changing anything. Also returns the source that was compiled.
    pub fn prepare(&self, patch_file: &PatchFile) -> Result<(Prepared, String), Refusal> {
        let script = self.find_script(&patch_file.cast, &patch_file.member.to_string()).map_err(Refusal::NotFound)?;
        let (version, multiplier) = self.version(script);
        let source = self.resolved_source(patch_file)?;
        let request = PatchRequest { handler: &patch_file.handler, source: &source, stock: patch_file.stock.as_deref(), new_handler: patch_file.new_handler };
        let prepared = patch::prepare(self.lctx(script), script.script_id, &request, version, multiplier)?;
        Ok((prepared, source))
    }
}

/// Replaces the inside of every string literal on every line.
pub fn redact_strings(text: &str) -> String {
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        let mut in_string = false;
        for ch in line.chars() {
            if ch == '"' {
                if !in_string {
                    out.push('"');
                    out.push_str(REDACTED);
                }
                in_string = !in_string;
                if !in_string {
                    out.push('"');
                }
            } else if !in_string {
                out.push(ch);
            }
        }
        if in_string {
            out.push('"');
        }
    }
    out
}

/// The string literals in a piece of Lingo source (comments skipped).
pub fn string_literals(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '-' && chars.get(i + 1) == Some(&'-') {
                break;
            }
            if chars[i] == '"' {
                let mut text = String::new();
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    text.push(chars[i]);
                    i += 1;
                }
                out.push(text);
            }
            i += 1;
        }
    }
    out
}

/// A patch file: Lingo source for one handler, under a header of comment lines.
///
/// ```text
/// -- lingo-patch 1
/// -- script: Main/5 (Game Logic)
/// -- handler: DrawScene
/// -- stock: 5f0c...            (fingerprint of the handler being replaced; `rotl-lingo new` writes it)
/// -- enabled: yes              (`no` keeps the file out of the manifest)
/// -- why: one or more lines saying what this changes and why
/// on DrawScene TheInf
///   ...
/// end
/// ```
#[derive(Clone, Debug, Default)]
pub struct PatchFile {
    pub cast: String,
    pub member: u32,
    /// The member's name as the header gives it in parentheses; informational.
    pub member_name: String,
    pub handler: String,
    pub stock: Option<String>,
    pub enabled: bool,
    pub new_handler: bool,
    pub why: String,
    /// The whole file. The header lines are comments, so line numbers in errors match the file.
    pub source: String,
    /// When the file gives changes to the stock handler instead of the whole handler (see `Edit`).
    pub edits: Vec<Edit>,
}

/// One change to the stock handler's source, for a patch file that holds changes instead of a whole handler:
///
/// ```text
/// @@ replace            (or `@@ replace all`, or `@@ replace 3` for exactly three places)
///   repeat with x = 230 to 249
/// @@ with
///   repeat with x = 230 to 269
/// @@ end
/// ```
///
/// The lines to replace must be found in the stock source as a run of whole lines (indentation aside),
/// exactly as many times as the edit says: once, unless it says otherwise. The file then stays a short,
/// reviewable difference, and the movie's own text stays out of the repository.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Edit {
    pub find: Vec<String>,
    pub with: Vec<String>,
    /// How many places must match; `None` for `all` (at least one).
    pub count: Option<usize>,
    /// The line of the file the edit starts on.
    pub line: usize,
}

fn parse_edits(text: &str) -> Result<Vec<Edit>, String> {
    let mut edits = Vec::new();
    let mut current: Option<Edit> = None;
    let mut in_with = false;
    let mut started = false;
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if !started {
            if line.is_empty() || line.starts_with("--") {
                continue;
            }
            started = true;
        }
        if let Some(rest) = line.strip_prefix("@@") {
            let mut words = rest.split_whitespace();
            match words.next() {
                Some("replace") => {
                    if current.is_some() {
                        return Err(format!("line {}: the edit before this one has no `@@ end`", index + 1));
                    }
                    let count = match words.next() {
                        None => Some(1),
                        Some("all") => None,
                        Some(n) => Some(n.parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(|| format!("line {}: `@@ replace` takes nothing, `all` or a number", index + 1))?),
                    };
                    current = Some(Edit { count, line: index + 1, ..Default::default() });
                    in_with = false;
                }
                Some("with") => {
                    if current.is_none() || in_with {
                        return Err(format!("line {}: `@@ with` follows the lines of an `@@ replace`", index + 1));
                    }
                    in_with = true;
                }
                Some("end") => {
                    let edit = current.take().filter(|_| in_with).ok_or_else(|| format!("line {}: `@@ end` closes an `@@ replace` ... `@@ with` pair", index + 1))?;
                    if edit.find.is_empty() {
                        return Err(format!("line {}: the edit names no lines to replace", edit.line));
                    }
                    edits.push(edit);
                    in_with = false;
                }
                _ => return Err(format!("line {}: expected `@@ replace`, `@@ with` or `@@ end`", index + 1)),
            }
            continue;
        }
        match current.as_mut() {
            Some(edit) => {
                if line.is_empty() {
                    continue;
                }
                if in_with {
                    edit.with.push(raw.trim_end().to_string());
                } else {
                    edit.find.push(line.to_string());
                }
            }
            None => {
                if !line.is_empty() && !line.starts_with("--") {
                    return Err(format!("line {}: in a patch file of edits, every line belongs to an `@@ replace` ... `@@ with` ... `@@ end` block", index + 1));
                }
            }
        }
    }
    if let Some(edit) = current {
        return Err(format!("line {}: this edit has no `@@ end`", edit.line));
    }
    Ok(edits)
}

/// The stock source with the edits made. An edit that does not match as often as it says is an error.
pub fn apply_edits(stock: &str, edits: &[Edit]) -> Result<String, String> {
    let mut lines: Vec<String> = stock.lines().map(|l| l.to_string()).collect();
    for edit in edits {
        let mut out: Vec<String> = Vec::with_capacity(lines.len());
        let mut found = 0;
        let mut i = 0;
        while i < lines.len() {
            let matches = i + edit.find.len() <= lines.len() && edit.find.iter().enumerate().all(|(k, want)| lines[i + k].trim() == want);
            if matches {
                found += 1;
                out.extend(edit.with.iter().cloned());
                i += edit.find.len();
            } else {
                out.push(lines[i].clone());
                i += 1;
            }
        }
        let first = edit.find.first().map(|s| s.as_str()).unwrap_or("");
        match edit.count {
            Some(n) if found != n => return Err(format!("the edit on line {} (`{}`...) matches the movie's handler {} time(s); it says {}", edit.line, first, found, n)),
            None if found == 0 => return Err(format!("the edit on line {} (`{}`...) matches nothing in the movie's handler", edit.line, first)),
            _ => {}
        }
        lines = out;
    }
    let mut text = lines.join("\n");
    text.push('\n');
    Ok(text)
}

pub const PATCH_FORMAT: u32 = 1;

impl PatchFile {
    pub fn id(&self) -> String {
        format!("{}/{}/{}", self.cast, self.member, self.handler)
    }

    pub fn parse(text: &str) -> Result<PatchFile, String> {
        let mut patch = PatchFile { enabled: true, source: text.replace("\r\n", "\n"), ..Default::default() };
        let first_code = patch.source.lines().map(|l| l.trim()).find(|l| !l.is_empty() && !l.starts_with("--")).unwrap_or("");
        if first_code.starts_with("@@") {
            patch.edits = parse_edits(&patch.source)?;
        }
        let mut saw_magic = false;
        let mut last_key = String::new();
        for (index, line) in patch.source.clone().lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some(comment) = trimmed.strip_prefix("--") else { break };
            let comment = comment.trim();
            if !saw_magic {
                let version = comment.strip_prefix("lingo-patch").map(|v| v.trim()).ok_or_else(|| format!("line {}: a patch file starts with `-- lingo-patch {}`", index + 1, PATCH_FORMAT))?;
                if version != PATCH_FORMAT.to_string() {
                    return Err(format!("line {}: this is patch format `{}`; this tool reads format {}", index + 1, version, PATCH_FORMAT));
                }
                saw_magic = true;
                continue;
            }
            let (key, value) = match comment.split_once(':') {
                Some((key, value)) if ["script", "handler", "stock", "enabled", "new", "why"].contains(&key.trim().to_ascii_lowercase().as_str()) => (key.trim().to_ascii_lowercase(), value.trim().to_string()),
                _ => {
                    // A header line without a known key continues the `why`.
                    if last_key == "why" {
                        if !patch.why.is_empty() {
                            patch.why.push(' ');
                        }
                        patch.why.push_str(comment);
                    }
                    continue;
                }
            };
            last_key = key.clone();
            match key.as_str() {
                "script" => {
                    let (path, name) = match value.split_once('(') {
                        Some((path, name)) => (path.trim().to_string(), name.trim_end_matches(')').trim().to_string()),
                        None => (value.clone(), String::new()),
                    };
                    let (cast, member) = path.rsplit_once('/').ok_or_else(|| format!("line {}: `script:` is `<cast>/<member number>`", index + 1))?;
                    patch.cast = cast.trim().to_string();
                    patch.member = member.trim().parse().map_err(|_| format!("line {}: `script:` needs the member's number after the `/`", index + 1))?;
                    patch.member_name = name;
                }
                "handler" => patch.handler = value,
                "stock" => patch.stock = if value.is_empty() || value == "none" { None } else { Some(value) },
                "enabled" => patch.enabled = yes_no(&value).ok_or_else(|| format!("line {}: `enabled:` is yes or no", index + 1))?,
                "new" => patch.new_handler = yes_no(&value).ok_or_else(|| format!("line {}: `new:` is yes or no", index + 1))?,
                "why" => patch.why = value,
                _ => {}
            }
        }
        if !saw_magic {
            return Err(format!("a patch file starts with `-- lingo-patch {}`", PATCH_FORMAT));
        }
        if patch.cast.is_empty() {
            return Err("the header has no `-- script: <cast>/<member number>` line".to_string());
        }
        if patch.handler.is_empty() {
            return Err("the header has no `-- handler: <name>` line".to_string());
        }
        if patch.why.trim().is_empty() {
            return Err("the header has no `-- why:` line; say what the patch changes and why".to_string());
        }
        if !patch.edits.is_empty() && patch.new_handler {
            return Err("a new handler has no stock source to edit; give the whole handler".to_string());
        }
        if patch.stock.is_none() && !patch.new_handler {
            return Err("the header has no `-- stock:` line (the fingerprint of the handler being replaced; `rotl-lingo new` writes it)".to_string());
        }
        Ok(patch)
    }

    /// A new patch file for a handler: the header and the handler's own source, ready to edit.
    pub fn scaffold(script: &ScriptInfo, handler: &str, fingerprint: &str, source: &str, why: &str) -> String {
        let name = if script.member_name.is_empty() { String::new() } else { format!(" ({})", script.member_name) };
        format!(
            "-- lingo-patch {}\n-- script: {}/{}{}\n-- handler: {}\n-- stock: {}\n-- enabled: no\n-- why: {}\n{}",
            PATCH_FORMAT, script.cast, script.member, name, handler, fingerprint, why, source
        )
    }
}

fn yes_no(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "yes" | "true" | "on" => Some(true),
        "no" | "false" | "off" => Some(false),
        _ => None,
    }
}

/// Why a patch file must not be committed or shipped, if any reason: it targets a sensitive handler, or it
/// carries one of the sensitive handlers' string literals.
pub fn secret_problems(movie: &Movie, patch_file: &PatchFile) -> Vec<String> {
    let mut problems = Vec::new();
    if let Ok(script) = movie.find_script(&patch_file.cast, &patch_file.member.to_string()) {
        if let Ok((_, handler)) = movie.find_handler(script, &patch_file.handler) {
            if movie.handler_is_sensitive(script, handler) {
                problems.push(format!("`{}` handles the connection (its text holds keys or addresses); it cannot be patched from a file", patch_file.handler));
            }
        }
    }
    problems.extend(static_secret_problems(&patch_file.source));
    let secrets = movie.sensitive_literals();
    for literal in string_literals(&patch_file.source) {
        // Short strings ("", "X", "OFF") are everywhere; only longer ones identify a secret.
        if literal.len() >= 8 && secrets.contains(&literal) {
            problems.push("the file contains a string that the movie's connection code uses; it must not be copied into a patch".to_string());
            break;
        }
    }
    problems
}

/// The part of the secrets check that needs no movie: names a patch has no business using.
pub fn static_secret_problems(source: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let mut code = String::new();
    for line in source.lines() {
        let mut in_string = false;
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '"' {
                in_string = !in_string;
            } else if !in_string {
                if chars[i] == '-' && chars.get(i + 1) == Some(&'-') {
                    break;
                }
                code.push(chars[i]);
            }
            i += 1;
        }
        code.push('\n');
    }
    let lower = code.to_ascii_lowercase();
    let mut word = String::new();
    let mut seen: Vec<String> = Vec::new();
    for ch in lower.chars().chain(std::iter::once(' ')) {
        if ch.is_alphanumeric() || ch == '_' {
            word.push(ch);
        } else {
            if !word.is_empty() && name_is_sensitive(&word) && !seen.contains(&word) {
                seen.push(word.clone());
            }
            word.clear();
        }
    }
    for word in seen {
        problems.push(format!("the patch uses `{}`, which belongs to the connection code; patches must stay away from it", word));
    }
    problems
}
