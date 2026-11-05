//! The VIP badge in chat (ROTL).
//!
//! The server marks a VIP's chat with the plain tag `[VIP]`, because the connection to the movie carries one byte per character and cannot
//! carry a star. The movie only prints the line it is given, so this Xtra swaps the tag for a real star (U+2605, which the player's fonts
//! draw) on its way in. Only the two places the server puts the tag are touched, so a player typing `[VIP]` into their own message is
//! not turned into a badge:
//!
//!   map chat       `255<[12:00:00] [VIP] Ann says "hi"`    ->  `255<[12:00:00] ★ Ann says "hi"`
//!   a broadcast    `255<Ann [VIP] Broadcasts: "hi"`        ->  `255<Ann ★ Broadcasts: "hi"`
//!
//! A player that does not have this module shows the tag as it is.

use crate::player::cast_member::FieldMember;

pub const SUBJECT: &str = "Broadcast";
const TAG: &str = "[VIP]";
const STAR: &str = "\u{2605}";
/// The badge's colour: gold, whatever the colour of the line it is in.
const GOLD: (u8, u8, u8) = (0xF5, 0xC5, 0x42);

/// After a line of a field was given its colour: the badge star in that range is made gold, so it reads as a badge and not as a letter of
/// the line. `byte_start..byte_end` is the range just coloured.
pub fn gold_stars(field: &mut FieldMember, byte_start: u32, byte_end: u32) {
    let end = (byte_end as usize).min(field.text.len());
    let start = (byte_start as usize).min(end);
    let stars: Vec<usize> = field.text[start..end].match_indices(STAR).map(|(at, _)| start + at).collect();
    for at in stars {
        field.apply_color_to_byte_range(at as u32, (at + STAR.len()) as u32, GOLD);
    }
}

/// The line with the server's tag replaced by the star, or `None` when the line carries no tag in either of the server's places.
pub fn swap_tag(line: &str) -> Option<String> {
    // The movie colour prefix: `<number>` then `<`.
    let (prefix, rest) = line.split_once('<')?;
    if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Map chat: `[time] [VIP] name says "..."`.
    if let Some(after_time) = rest.strip_prefix('[').and_then(|r| r.split_once("] ")).map(|(_, tail)| tail) {
        if let Some(tail) = after_time.strip_prefix(TAG) {
            if tail.starts_with(' ') {
                let time_len = rest.len() - after_time.len();
                return Some(format!("{prefix}<{}{STAR}{tail}", &rest[..time_len]));
            }
        }
        return None;
    }
    // A broadcast: `name [VIP] Broadcasts: "..."`.
    let (name, tail) = rest.split_once(' ')?;
    let tail = tail.strip_prefix(TAG)?.strip_prefix(' ')?;
    if tail.starts_with("Broadcasts:") && !name.is_empty() {
        return Some(format!("{prefix}<{name} {STAR} {tail}"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_colours(field: &mut FieldMember) -> Vec<(u32, (u16, u16, u16))> {
        field.sync_runs();
        field.formatting_runs.iter().map(|r| (r.start_position, (r.color_r, r.color_g, r.color_b))).collect()
    }

    #[test]
    fn the_star_is_gold_and_the_rest_of_the_line_keeps_its_colour() {
        crate::player::symbols::symbol_table::init_symbol_table();
        let mut field = FieldMember::new();
        field.text = "[12:00:00] \u{2605} Ann says \"hi\"".to_string();
        let len = field.text.len() as u32;
        field.apply_color_to_byte_range(0, len, (211, 143, 211));
        gold_stars(&mut field, 0, len);
        let widen = |c: u8| ((c as u16) << 8) | c as u16;
        let line = (widen(211), widen(143), widen(211));
        let gold = (widen(0xF5), widen(0xC5), widen(0x42));
        let star_at = field.text.find('\u{2605}').unwrap() as u32;
        let runs = run_colours(&mut field);
        assert_eq!(runs.iter().find(|(at, _)| *at == 0).unwrap().1, line, "the time keeps the line's colour");
        assert_eq!(runs.iter().find(|(at, _)| *at == star_at).unwrap().1, gold, "the star is gold");
        assert_eq!(runs.iter().find(|(at, _)| *at == star_at + 3).unwrap().1, line, "the name after it keeps the line's colour");
    }

    #[test]
    fn a_line_without_a_star_is_left_alone_and_only_the_given_range_is_looked_at() {
        crate::player::symbols::symbol_table::init_symbol_table();
        let mut field = FieldMember::new();
        field.text = "[12:00:00] Rival says hi\r[12:00:01] \u{2605} Tester says hi".to_string();
        let first_line = field.text.find('\r').unwrap() as u32;
        field.apply_color_to_byte_range(0, first_line, (255, 255, 255));
        gold_stars(&mut field, 0, first_line);
        let runs = run_colours(&mut field);
        let gold = ((0xF5u16 << 8) | 0xF5, (0xC5u16 << 8) | 0xC5, (0x42u16 << 8) | 0x42);
        assert!(runs.iter().all(|(_, c)| *c != gold), "no star in that line: nothing is gold");
        assert!(runs.iter().all(|(at, _)| *at == 0 || *at == first_line), "nothing split inside the line");
    }

    #[test]
    fn map_chat_gets_the_star_in_front_of_the_name() {
        assert_eq!(swap_tag("255<[12:00:00] [VIP] Ann says \"hi\"").as_deref(), Some("255<[12:00:00] \u{2605} Ann says \"hi\""));
        assert_eq!(swap_tag("50<[01:02:03] [VIP] Bob says \"[VIP] me too\"").as_deref(), Some("50<[01:02:03] \u{2605} Bob says \"[VIP] me too\""), "only the tag before the name");
    }

    #[test]
    fn a_broadcast_gets_the_star_after_the_name() {
        assert_eq!(swap_tag("255<Ann [VIP] Broadcasts: \"hi\"").as_deref(), Some("255<Ann \u{2605} Broadcasts: \"hi\""));
    }

    #[test]
    fn what_a_player_types_is_never_turned_into_a_badge() {
        assert_eq!(swap_tag("255<[12:00:00] Ann says \"[VIP] hello\""), None, "inside the words");
        assert_eq!(swap_tag("255<Ann Broadcasts: \"[VIP] sale\""), None);
        assert_eq!(swap_tag("255<[12:00:00] Ann [VIP] says \"hi\""), None, "not where the server puts it");
        assert_eq!(swap_tag("255<Ann [VIP] shouts: \"hi\""), None);
        assert_eq!(swap_tag("Ann [VIP] Broadcasts: \"hi\""), None, "no colour prefix: not a server line");
        assert_eq!(swap_tag("x<Ann [VIP] Broadcasts: \"hi\""), None);
        assert_eq!(swap_tag("[VIP]"), None);
        assert_eq!(swap_tag(""), None);
    }

    #[test]
    fn lines_without_a_tag_are_left_alone() {
        assert_eq!(swap_tag("255<[12:00:00] Ann says \"hi\""), None);
        assert_eq!(swap_tag("255<Ann Broadcasts: \"hi\""), None);
    }
}
