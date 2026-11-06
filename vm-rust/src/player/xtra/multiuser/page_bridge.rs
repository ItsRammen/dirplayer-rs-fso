//! The Modern UI's feed (ROTL).
//!
//! The web page draws the game's sidebar itself in Modern mode. It needs what the server tells the movie about the party, the currencies, the
//! date and so on, as it arrives, and for a few replies it needs to be the only one to see them, so the movie does not open its own window.
//! Each message on the list below is offered to the page's `window.__rotlMsg(subject, content)` before the movie gets it. The page answers
//! true for a message it has asked for and will draw (the movie then does not see it), and false or nothing otherwise, so a page without the
//! Modern UI, or Original mode, leaves the game exactly as it was. The subjects are the movie's own: nothing is renamed or changed.

/// What the page may be offered. Chat, mail and anything else a player typed never goes to the page.
const SUBJECTS: &[&str] = &[
    "Hirelings",
    "UpdateSpellStones",
    "BANKED",
    "MyPotions",
    "Abilities",
    "DateSorting",
    "TimeSorting",
    "TeleportEnabled",
    "TeleportMenuAppear",
    "TeleportPadMenuAppear",
    "ItemInfoReturn",
    "HereAmInv",
    "HereAmCond",
];

/// True when the page took the message and the movie should not see it.
pub fn offer(subject: &str, content: &str) -> bool {
    if !SUBJECTS.contains(&subject) {
        return false;
    }
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::{JsCast, JsValue};
        let Some(window) = web_sys::window() else { return false };
        let Ok(function) = js_sys::Reflect::get(&window, &JsValue::from_str("__rotlMsg")) else { return false };
        let Some(function) = function.dyn_ref::<js_sys::Function>() else { return false };
        return function
            .call2(&JsValue::NULL, &JsValue::from_str(subject), &JsValue::from_str(content))
            .map(|answer| answer.is_truthy())
            .unwrap_or(false);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = content;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_listed_subjects_are_offered() {
        // Off the web there is no page, so nothing is ever taken; the list is what matters.
        assert!(!offer("Hirelings", "Ranger:Ann:1"));
        assert!(!offer("TypChatMsg", "hello"));
        assert!(SUBJECTS.contains(&"Hirelings") && !SUBJECTS.contains(&"chatMsg") && !SUBJECTS.contains(&"MailUpdate"));
    }
}

/// The last `count` lines of a field member with each line's colour: `(text, Some((r, g, b)))`, or `None` for a line in the field's own colour. The page's
/// chat reads the movie's own chat and combat log this way, so lines the movie writes itself ("You cast ...") are there too, with the colour the movie gave
/// them (a line's colour is the run its first character is in; see `FieldMember::apply_color_to_byte_range`).
pub fn field_lines(name: &str, count: usize) -> Option<Vec<(String, Option<(u8, u8, u8)>)>> {
    use crate::player::cast_member::CastMemberType;
    crate::player::reserve_player_ref(|player| {
        let member_ref = player.movie.cast_manager.find_member_ref_by_name(name)?;
        let member = player.movie.cast_manager.find_member_by_ref(&member_ref)?;
        let CastMemberType::Field(field) = &member.member_type else { return None };
        let runs = field.current_runs();
        let mut lines = Vec::new();
        let mut start = 0usize;
        for piece in field.text.split(|c| c == '\r' || c == '\n').map(str::to_owned).collect::<Vec<_>>() {
            let at = start;
            start += piece.len() + 1;
            if piece.trim().is_empty() {
                continue;
            }
            let run = runs.iter().rev().find(|r| (r.start_position as usize) <= at);
            let colour = run.and_then(|r| {
                let (cr, cg, cb) = (r.color_r, r.color_g, r.color_b);
                if cr == 0 && cg == 0 && cb == 0 {
                    None // "inherit"
                } else if cr == 0 && cg == 0 && cb == 0x0100 {
                    Some((0, 0, 0)) // explicit black (see `apply_color_to_byte_range`)
                } else {
                    Some(((cr >> 8) as u8, (cg >> 8) as u8, (cb >> 8) as u8))
                }
            });
            lines.push((piece, colour));
        }
        let skip = lines.len().saturating_sub(count);
        Some(lines.split_off(skip))
    })
}
