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
    // The Modern UI's friends window: the server's whole friends list with who is online (`name:1|name:0`). The movie has no such message and does not
    // see it (the page takes it); `YourFriends`, the online list, still goes to the movie as before.
    "FriendsAll",
    // The Modern UI's battle dock: the battle's enemies as JSON, sent by the server after every change (the movie only learns an enemy by a mouse-over). The movie never sees it.
    "FoeList",
    // The Modern UI's Guild window: the whole guild (name, crest, message, members with rank and who is online, stats) as JSON, or `NONE`, sent by the server at login and
    // whenever the roster, the crest or who is online changes. The movie never sees it.
    "GuildState",
    // The Modern UI's guild invitation popup: who invited, how long it lasts and the guild as JSON, sent to the invitee along with the classic chat line. The movie never sees it.
    "GuildInviteOffer",
    // The Modern UI's profile window: a character's profile (`uBio1`, with the kills that follow it as `MonsterKills` and `MonsterKillsTOTAL`), however it was asked for
    // (right-click on a player, a friend's View Profile, a guild member). In Original mode the page answers false and the movie shows its own profile as before.
    "uBio1",
    "MonsterKills",
    "MonsterKillsTOTAL",
    // The Modern UI's battle: the game's own enemy panel that opens when the mouse rests on an enemy (the server's reply to `CkOTleTSpt`, drawn by `DrawZeHirelingBattleViewfinder`).
    // The page shows its own card for it, so in a Modern-mode battle it answers true and the movie draws nothing; otherwise (Original mode, or out of a battle) false and the movie draws its panel.
    "HirBatViewGoGoGo",
    // The Modern UI's Who's Online (Game menu): the server's reply to `system.group.getusers` (`["AllUsers", n, ["name", ...]]`, names only), which the movie draws as its own window. The page
    // answers true in Modern mode (outside a battle) and shows its own; otherwise false and the movie draws its window as before.
    "UserList",
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
        assert!(SUBJECTS.contains(&"FriendsAll") && !offer("FriendsAll", "Ann:1"));
        assert!(SUBJECTS.contains(&"FoeList") && !offer("FoeList", "[]"));
        assert!(SUBJECTS.contains(&"GuildState") && !offer("GuildState", "NONE"));
        assert!(SUBJECTS.contains(&"GuildInviteOffer") && !offer("GuildInviteOffer", "{}"));
        assert!(SUBJECTS.contains(&"UserList") && !offer("UserList", "[\"AllUsers\", 1, [\"Ann\"]]") && !SUBJECTS.contains(&"UserListPluskick"));
        assert!(SUBJECTS.contains(&"HirBatViewGoGoGo") && !offer("HirBatViewGoGoGo", "Rat:Mozart:1")); assert!(SUBJECTS.contains(&"uBio1") && !offer("uBio1", "Ann:20:Body1-Head1") && SUBJECTS.contains(&"MonsterKills") && SUBJECTS.contains(&"MonsterKillsTOTAL") && !SUBJECTS.contains(&"NPCBio"));
    }
}

/// The colour a formatting run gives text: `None` for "inherit", `Some((0, 0, 0))` for explicit black (see `FieldMember::apply_color_to_byte_range`), else the run's colour.
fn run_colour(run: &crate::director::chunks::text::StxtFormattingRun) -> Option<(u8, u8, u8)> {
    let (cr, cg, cb) = (run.color_r, run.color_g, run.color_b);
    if cr == 0 && cg == 0 && cb == 0 {
        None
    } else if cr == 0 && cg == 0 && cb == 0x0100 {
        Some((0, 0, 0))
    } else {
        Some(((cr >> 8) as u8, (cg >> 8) as u8, (cb >> 8) as u8))
    }
}

/// One line of a field, cut at every place its colour changes: `piece` is the line's text, `at` its byte offset in the field, `runs` the runs' `(start byte, colour)` in order.
/// Each part is `(text, colour)`. A run that starts inside the line starts a new part; the part before it keeps the colour of the run it was in.
pub fn cut_runs(piece: &str, at: usize, runs: &[(usize, Option<(u8, u8, u8)>)]) -> Vec<(String, Option<(u8, u8, u8)>)> {
    let end = at + piece.len();
    let colour_at = |pos: usize| runs.iter().rev().find(|(start, _)| *start <= pos).and_then(|(_, c)| *c);
    let mut cuts: Vec<usize> = vec![at];
    for (start, _) in runs {
        if *start > at && *start < end && piece.is_char_boundary(*start - at) {
            cuts.push(*start);
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    cuts.push(end);
    let mut out: Vec<(String, Option<(u8, u8, u8)>)> = Vec::new();
    for pair in cuts.windows(2) {
        let Some(text) = piece.get(pair[0] - at..pair[1] - at) else { continue };
        if text.is_empty() {
            continue;
        }
        let colour = colour_at(pair[0]);
        match out.last_mut() {
            Some((last, last_colour)) if *last_colour == colour => last.push_str(text), // a colour that did not change is one part
            _ => out.push((text.to_string(), colour)),
        }
    }
    out
}

/// Like `field_lines`, but every line comes as its coloured parts (see `cut_runs`): the time, the name and the speech of a chat line each in the colour the movie gave it.
pub fn field_runs(name: &str, count: usize) -> Option<Vec<Vec<(String, Option<(u8, u8, u8)>)>>> {
    use crate::player::cast_member::CastMemberType;
    crate::player::reserve_player_ref(|player| {
        let member_ref = player.movie.cast_manager.find_member_ref_by_name(name)?;
        let member = player.movie.cast_manager.find_member_by_ref(&member_ref)?;
        let CastMemberType::Field(field) = &member.member_type else { return None };
        let runs: Vec<(usize, Option<(u8, u8, u8)>)> = field.current_runs().iter().map(|r| (r.start_position as usize, run_colour(r))).collect();
        let mut lines = Vec::new();
        let mut start = 0usize;
        for piece in field.text.split(|c| c == '\r' || c == '\n').map(str::to_owned).collect::<Vec<_>>() {
            let at = start;
            start += piece.len() + 1;
            if piece.trim().is_empty() {
                continue;
            }
            lines.push(cut_runs(&piece, at, &runs));
        }
        let skip = lines.len().saturating_sub(count);
        Some(lines.split_off(skip))
    })
}

#[cfg(test)]
mod run_tests {
    use super::cut_runs;

    #[test]
    fn a_chat_line_is_cut_where_its_colour_changes() {
        let grey = Some((143, 143, 143));
        let blue = Some((90, 160, 255));
        // "[17:43] Tester says hi" with the time grey, the name blue and the rest inherited
        let parts = cut_runs("[17:43] Tester says hi", 100, &[(0, None), (100, grey), (108, blue), (115, None)]);
        assert_eq!(parts, vec![("[17:43] ".to_string(), grey), ("Tester ".to_string(), blue), ("says hi".to_string(), None)]);
    }

    #[test]
    fn a_line_in_one_colour_is_one_part_and_runs_outside_it_are_ignored() {
        let white = Some((255, 255, 255));
        assert_eq!(cut_runs("hello", 10, &[(0, white), (40, None)]), vec![("hello".to_string(), white)]);
        // two runs of the same colour in a row are still one part
        assert_eq!(cut_runs("hello", 0, &[(0, white), (2, white)]), vec![("hello".to_string(), white)]);
    }

    #[test]
    fn a_cut_never_lands_inside_a_character() {
        let a = Some((1, 2, 3));
        // the diamond is three bytes; a run that "starts" inside it is not a cut
        let parts = cut_runs("a\u{25C6}b", 0, &[(0, a), (2, None)]);
        assert_eq!(parts, vec![("a\u{25C6}b".to_string(), a)]);
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
