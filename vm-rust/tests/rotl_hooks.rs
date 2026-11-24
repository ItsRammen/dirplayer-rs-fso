//! ROTL: Lingo run after the movie's own handlers return (`player::rotl_hooks`), and solid map objects by name (`player::rotl_solid`),
//! on the real ROTL movie.
//!
//! Needs the movie: `ROTL_MOVIE=/path/to/dcrgame.dcr cargo test --test rotl_hooks -- --nocapture`. Skipped without it.

use std::rc::Rc;

use vm_rust::director::lingo::datum::Datum;
use vm_rust::player::datum_ref::DatumRef;
use vm_rust::player::rotl_hooks;
use vm_rust::player::rotl_solid;
use vm_rust::player::script_ref::ScriptInstanceRef;
use vm_rust::player::symbols::symbol::Symbol;
use vm_rust::player::testing_shared::TestHarness;
use vm_rust::player::virtual_scripts::{VirtualScriptHandler, VirtualScriptRegistry};
use vm_rust::player::{reserve_player_mut, DirPlayer, ScriptError};

/// A stand-in for the Multiuser connection: `sendNetMessage` answers 0 (sent) and goes nowhere.
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

fn global(name: &str) -> String {
    reserve_player_mut(|player| match player.globals.get(&Symbol::from_str(name)) {
        Some(r) => player.get_datum(r).string_value().unwrap_or_else(|_| "?".into()),
        None => "(no such global)".to_string(),
    })
}

fn movie() -> Option<String> {
    let path = std::env::var("ROTL_MOVIE").unwrap_or_else(|_| "/home/romney/DirectorShare/dcrgame.dcr".to_string());
    std::path::Path::new(&path).exists().then_some(path)
}

// What the page registers for a worn-look table of one head piece and one weapon (client/site/js/custom-art.js, lookHookLingo).
const TABLE: &str = "rotlWorn = \":\" & StaticInf & \":\"\n\
if rotlWorn contains \":W-Eyepatch:\" and \":Head1:Head2:\" contains \":\" & MyHead & \":\" then HeadImage = \"Eyepatch \" & MyHead\n\
if rotlWorn contains \":W-Azure Blade:\" then Weapon = \"Azure Blade\"\n\
rotlHookRan = rotlHookRan + 1";

vm_rust::native_e2e_test!(rotl_after_hooks_on_the_movie, |player| async move {
    let Some(path) = movie() else {
        eprintln!("skipped: no ROTL movie (set ROTL_MOVIE)");
        return Ok(());
    };
    player.load_movie(&path).await;

    // What LoadInf reads that the login would have set. gConnect is an offline stand-in (LoadInf ends by telling the server the new look).
    reserve_player_mut(|player| {
        let script = VirtualScriptRegistry::register(player, "RotlOfflineConnection", Rc::new(OfflineConnection));
        let (_instance, datum) = VirtualScriptRegistry::create_instance(player, &script);
        player.globals.insert(Symbol::from_str("gConnect"), datum);
    });
    for line in ["MyCharsBody = \"DefChar\"", "MyHead = \"Head1\"", "CurInvList = 1", "mname = \"Tester\"", "MyChar = \"Tester\"", "bFileDat = \"X\"", "rotlHookRan = 0"] {
        if let Err(e) = player.eval(line).await {
            eprintln!("setup `{}`: {}", line, e.message);
        }
    }
    let load = |inv: &str| format!("TheInf = \"{}\"\nLoadInf(TheInf)", inv);

    // 1. The movie as it is: it knows W-Red Shirt and W-Short Sword, not W-Eyepatch or W-Azure Blade.
    rotl_hooks::clear();
    player.eval(&load("W-Red Shirt:W-Eyepatch:W-Azure Blade:Apple")).await.map_err(|e| e.message)?;
    let (head0, weapon0, shirt0) = (global("HeadImage"), global("Weapon"), global("Shirt"));
    eprintln!("no hook:          HeadImage={} Weapon={} Shirt={}", head0, weapon0, shirt0);

    // 2. Keys: a key names a script and a handler; a line the evaluator cannot read is refused when registered, and nothing is kept.
    if rotl_hooks::set_after("LoadInf", "x = 1").is_ok() {
        return Err("a key with no script was accepted".into());
    }
    let refused = rotl_hooks::set_after("INVENTORY SORT:LoadInf", "HeadImage = \"Eyepatch\"\nHeadImage = ((");
    eprintln!("bad line refused: {:?}", refused);
    if refused.is_ok() || !rotl_hooks::keys().is_empty() {
        return Err("a hook with a line that cannot be read was registered".into());
    }

    // 3. The table, after LoadInf of the script it is in: the movie's own LoadInf still runs (the shirt), the hook adds the head and weapon.
    let lines = rotl_hooks::set_after("INVENTORY SORT:LoadInf", TABLE).map_err(|e| format!("table refused: {}", e))?;
    player.eval(&load("W-Red Shirt:W-Eyepatch:W-Azure Blade:Apple")).await.map_err(|e| e.message)?;
    let (head1, weapon1, shirt1) = (global("HeadImage"), global("Weapon"), global("Shirt"));
    eprintln!("table ({} lines):  HeadImage={} Weapon={} Shirt={} ran={}", lines, head1, weapon1, shirt1, global("rotlHookRan"));
    if head1 != "Eyepatch Head1" || weapon1 != "Azure Blade" || shirt1 != "Red Shirt" {
        return Err(format!("table: HeadImage={} Weapon={} Shirt={}", head1, weapon1, shirt1));
    }

    // 4. Not worn (in the bag, no W-), or a name that only starts the same: nothing changes.
    player.eval(&load("W-Red Shirt:Eyepatch:W-Eyepatch Deluxe:Apple")).await.map_err(|e| e.message)?;
    eprintln!("not worn:         HeadImage={} Weapon={}", global("HeadImage"), global("Weapon"));
    if global("HeadImage") != "Head1" || global("Weapon") != weapon0 {
        return Err(format!("not worn: HeadImage={} Weapon={}", global("HeadImage"), global("Weapon")));
    }

    // 5. A head with no look in the table keeps its own head.
    player.eval("MyHead = \"Head7\"").await.map_err(|e| e.message)?;
    player.eval(&load("W-Eyepatch")).await.map_err(|e| e.message)?;
    eprintln!("other head:       HeadImage={}", global("HeadImage"));
    if global("HeadImage") != "Head7" {
        return Err(format!("a head with no look: HeadImage={}", global("HeadImage")));
    }
    player.eval("MyHead = \"Head1\"").await.map_err(|e| e.message)?;

    // 6. The same handler name in another script does not run the hook; a key for a script that is not there never runs.
    let ran = global("rotlHookRan");
    rotl_hooks::set_after("NoSuchScript:LoadInfB", "rotlHookRan = 1000").map_err(|e| e)?;
    player.eval("TheInf = \"W-Eyepatch\"\nLoadInfB(TheInf)").await.map_err(|e| e.message)?;
    if global("rotlHookRan") != ran {
        return Err("a hook keyed to another script ran".into());
    }
    rotl_hooks::set_after("NoSuchScript:LoadInfB", "").map_err(|e| e)?;

    // 7. The handler's arguments, by their own names: DrawPlayersHead(x, HisHead, WhichX, WhichY, HisBody) for the own character.
    rotl_hooks::set_after("HEADS!:DrawPlayersHead", "rotlSeen = x & \"/\" & HisBody & \"/\" & HeadImage").map_err(|e| e)?;
    player.eval("bFileDat = \"\"").await.map_err(|e| e.message)?;
    player.eval("DrawPlayersHead(20, \"Head1\", 100, 100, \"SP20-F2-E\")").await.map_err(|e| e.message)?;
    eprintln!("DrawPlayersHead:  rotlSeen={}", global("rotlSeen"));
    if global("rotlSeen") != "20/SP20-F2-E/Head1" {
        return Err(format!("arguments: rotlSeen={}", global("rotlSeen")));
    }
    // ... and the arguments are gone after it: a bare `x` is the global again.
    player.eval("x = \"global x\"").await.map_err(|e| e.message)?;
    if global("x") != "global x" {
        return Err(format!("the global x: {}", global("x")));
    }

    // 8. A line that fails at run time stops that hook, is counted, and the movie goes on.
    let errors = rotl_hooks::error_count();
    rotl_hooks::set_after("HEADS!:DrawPlayersHead", "rotlBefore = 1\nrotlBad = NoSuchHandlerAtAllXyz(1)\nrotlAfter = 1").map_err(|e| e)?;
    player.eval("rotlAfter = 0").await.map_err(|e| e.message)?;
    let r = player.eval("DrawPlayersHead(20, \"Head1\", 100, 100, \"SP20-F1-S\")").await;
    eprintln!("failing line:     call={:?} errors {} -> {} recent={:?} after={}", r.as_ref().err().map(|e| e.message.clone()), errors, rotl_hooks::error_count(), rotl_hooks::recent_errors().last(), global("rotlAfter"));
    if r.is_err() {
        return Err("a failing hook line broke the handler's caller".into());
    }
    if rotl_hooks::error_count() == errors || global("rotlAfter") != "0" {
        return Err("a failing hook line was not reported, or the hook went on after it".into());
    }
    rotl_hooks::clear();

    // 9. `contains` and `starts`, as the compiled operators have them (case-insensitive).
    for (expr, want) in [("\"Red Shirt\" contains \"shirt\"", "1"), ("\"Red Shirt\" contains \"pants\"", "0"), ("\"Red Shirt\" starts \"red\"", "1"), ("\"Red Shirt\" starts \"Shirt\"", "0")] {
        player.eval(&format!("rotlTest = {}", expr)).await.map_err(|e| e.message)?;
        if global("rotlTest") != want {
            return Err(format!("{} gave {}", expr, global("rotlTest")));
        }
    }

    // 10. Solid objects: a name added to the movie's list (field item49) is found as the movie looks for it.
    let added = rotl_solid::add(&["Reunion Flag", "Hall of Legends Sign"]).map_err(|e| e)?;
    let again = rotl_solid::add(&["reunion flag"]).map_err(|e| e)?;
    player.eval("rotlSolid = member(\"item49\").text contains \"*Reunion Flag*\"").await.map_err(|e| e.message)?;
    eprintln!("solid:            added={:?} again={:?} found={}", added, again, global("rotlSolid"));
    if added.len() != 2 || !again.is_empty() || global("rotlSolid") != "1" {
        return Err("solid names were not added to item49".into());
    }
    if rotl_solid::add(&["Bad*Name"]).is_ok() {
        return Err("a name with * was accepted".into());
    }
    Ok(())
});
