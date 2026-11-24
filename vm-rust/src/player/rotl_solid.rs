//! Solid map objects (ROTL).
//!
//! The movie decides which map objects a character cannot walk through by name: the field member `item49` is a list of `*Name*`, and a
//! map object whose picture is `Name-...` (or `Name`) is solid when `*Name*` is in it (`DrawItems`, `ReCheckBlockTiles`,
//! `RemakeCollisionMap`). The list is fixed in the movie, so a new decoration could only be made solid with a second, invisible object
//! on its tile (a Hidden Block), which uses a second of the map's 20 object slots.
//!
//! `add` puts more names on that list, at runtime, for this client only. `DrawItems` and `ReCheckBlockTiles` read the field each time
//! they run; `RemakeCollisionMap` keeps its own copy in the global `CanwalkThroughList` once it has read it, so that copy is updated too.
//! Nothing else changes: the server's own idea of which tiles are blocked is its own (`blocks`), as with a Hidden Block.

use crate::director::lingo::datum::Datum;
use crate::player::cast_member::CastMemberType;
use crate::player::reserve_player_mut;
use crate::player::symbols::symbol::Symbol;

pub const FIELD: &str = "item49";
const GLOBAL: &str = "CanwalkThroughList";
pub const MAX_NAMES: usize = 200;

/// Whether `name` may go on the list: 2 to 40 letters, digits, single spaces, `'` and `+` (the names `art:build` allows), so it can
/// neither break the `*Name*` list nor end a Lingo string. No `-`: the movie cuts an object's picture name at the first one.
pub fn name_ok(name: &str) -> bool {
    let bytes = name.as_bytes();
    (2..=40).contains(&bytes.len())
        && bytes.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'\'' | b'+'))
        && bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1] != b' '
        && !name.contains("  ")
}

/// The list with `names` added (each once, compared without regard to case, as the movie's `contains` does); and the names added.
pub fn with_names(list: &str, names: &[&str]) -> (String, Vec<String>) {
    let mut out = list.to_string();
    let mut added = Vec::new();
    for name in names {
        let entry = format!("*{}*", name);
        if out.to_ascii_lowercase().contains(&entry.to_ascii_lowercase()) {
            continue;
        }
        out.push_str(&entry);
        added.push(name.to_string());
    }
    (out, added)
}

/// Make map objects of these names solid. Returns the names newly added (a name already on the list is not added again), or why
/// nothing was changed: a bad name, or no `item49` field (the movie is not loaded yet).
pub fn add(names: &[&str]) -> Result<Vec<String>, String> {
    if names.len() > MAX_NAMES {
        return Err(format!("{} names; at most {}", names.len(), MAX_NAMES));
    }
    if let Some(bad) = names.iter().find(|n| !name_ok(n)) {
        return Err(format!("\"{}\" is not a name that can be made solid", bad.chars().take(60).collect::<String>()));
    }
    reserve_player_mut(|player| {
        let member_ref = player.movie.cast_manager.find_member_ref_by_name(FIELD).ok_or_else(|| "the movie has no item49 field (not loaded yet?)".to_string())?;
        let member = player.movie.cast_manager.find_member_by_ref_mut(&member_ref).ok_or_else(|| "item49 is missing".to_string())?;
        let (text, added) = match &mut member.member_type {
            CastMemberType::Field(field) => {
                let (text, added) = with_names(&field.text, names);
                if !added.is_empty() {
                    field.set_text_preserving_caret(text.clone());
                }
                (text, added)
            }
            CastMemberType::Text(member) => {
                let (text, added) = with_names(&member.text, names);
                if !added.is_empty() {
                    member.set_text_preserving_caret(text.clone());
                }
                (text, added)
            }
            _ => return Err("item49 is not a field".to_string()),
        };
        // RemakeCollisionMap's copy, once it has one (it reads the field only while the global is VOID).
        if !added.is_empty() {
            let symbol = Symbol::from_str(GLOBAL);
            let has_copy = player.globals.get(&symbol).map(|r| player.get_datum(r).is_string()).unwrap_or(false);
            if has_copy {
                let datum = player.alloc_datum(Datum::String(text));
                player.globals.insert(symbol, datum);
            }
        }
        Ok(added)
    })
}

/// The `*Name*` list as the movie has it now, or None before the movie is loaded.
pub fn list() -> Option<String> {
    reserve_player_mut(|player| {
        let member_ref = player.movie.cast_manager.find_member_ref_by_name(FIELD)?;
        match &player.movie.cast_manager.find_member_by_ref(&member_ref)?.member_type {
            CastMemberType::Field(field) => Some(field.text.clone()),
            CastMemberType::Text(member) => Some(member.text.clone()),
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_could_break_the_list_are_refused() {
        assert!(name_ok("Reunion Flag") && name_ok("Hall of Legends Sign") && name_ok("Bob's Crate+"));
        for bad in ["", "A", "Flag*", "*", "Reunion-Flag", "a\"b", " Lead", "Trail ", "two  spaces", &"x".repeat(41)] {
            assert!(!name_ok(bad), "{:?}", bad);
        }
    }

    #[test]
    fn a_name_is_added_once_whatever_its_case() {
        let (list, added) = with_names("*Tree1*Rock*", &["Reunion Flag", "rock", "Reunion Flag"]);
        assert_eq!(list, "*Tree1*Rock**Reunion Flag*");
        assert_eq!(added, vec!["Reunion Flag".to_string()]);
    }
}
