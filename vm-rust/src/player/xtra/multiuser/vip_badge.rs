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
pub const GOLD: (u8, u8, u8) = (0xF5, 0xC5, 0x42);
/// The colour of the time in front of a chat line.
pub const DIM: (u8, u8, u8) = (0x8F, 0x8F, 0x8F);

/// The length in bytes of a time like `[18:39]` or `[18:39:04]` at the very start of `line`, if the line starts with one.
pub fn time_len(line: &str) -> Option<usize> {
    let rest = line.strip_prefix('[')?;
    let end = rest.find(']')?;
    let time = &rest[..end];
    let parts: Vec<&str> = time.split(':').collect();
    let ok = (parts.len() == 2 || parts.len() == 3) && parts.iter().all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_digit()));
    ok.then_some(end + 2)
}

/// The character the badge travels as; the text renderer draws a picture where it stands alone as a word.
pub const BADGE_CHAR: char = '\u{2605}';

/// A word that is just the badge character.
pub fn is_badge(token: &str) -> bool {
    let mut chars = token.chars();
    chars.next() == Some(BADGE_CHAR) && chars.next().is_none()
}

fn badge_font(size_px: f64) -> String {
    format!("bold {}px Arial, sans-serif", (size_px * 0.72).round().max(7.0))
}

/// How wide the badge picture is: its letters and a little room round them.
pub fn badge_width(ctx: &web_sys::CanvasRenderingContext2d, size_px: f64) -> f64 {
    ctx.set_font(&badge_font(size_px));
    let letters = ctx.measure_text("\u{2605} VIP").map(|m| m.width()).unwrap_or(size_px * 2.4);
    (letters + 6.0).ceil()
}

/// Draws the badge on the coverage canvas (white on black; the renderer colours it gold): a pill with the letters cut out of it, so
/// they show whatever is behind the text. `x, y` is the top-left of the glyph cell, `width` what `badge_width` gave.
pub fn draw_badge(ctx: &web_sys::CanvasRenderingContext2d, x: f64, y: f64, width: f64, size_px: f64) {
    let (left, top) = (x + 1.0, y + 1.0);
    let (w, h) = (width - 3.0, (size_px - 2.0).max(8.0));
    let r = (h / 3.0).floor().max(2.0);
    ctx.set_fill_style_str("rgb(255,255,255)");
    ctx.begin_path();
    ctx.move_to(left + r, top);
    ctx.line_to(left + w - r, top);
    ctx.line_to(left + w, top + r);
    ctx.line_to(left + w, top + h - r);
    ctx.line_to(left + w - r, top + h);
    ctx.line_to(left + r, top + h);
    ctx.line_to(left, top + h - r);
    ctx.line_to(left, top + r);
    ctx.close_path();
    ctx.fill();
    ctx.set_font(&badge_font(size_px));
    ctx.set_text_baseline("middle");
    ctx.set_text_align("center");
    ctx.set_fill_style_str("rgb(0,0,0)");
    let _ = ctx.fill_text("\u{2605} VIP", left + w / 2.0, top + h / 2.0 + 0.5);
    ctx.set_text_baseline("top");
    ctx.set_text_align("start");
}

/// After a line of a field was given its colour: the badge star in that range is made gold, so it reads as a badge and not as a letter of
/// the line. `byte_start..byte_end` is the range just coloured.
pub fn gold_stars(field: &mut FieldMember, byte_start: u32, byte_end: u32) {
    let end = (byte_end as usize).min(field.text.len());
    let start = (byte_start as usize).min(end);
    // The time at the start of a chat line is dimmed, so the words stand out from it (see `time_len`).
    if let Some(len) = time_len(&field.text[start..end]) {
        field.apply_color_to_byte_range(start as u32, (start + len) as u32, DIM);
    }
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
        assert_eq!(runs.iter().find(|(at, _)| *at == 0).unwrap().1, (widen(DIM.0), widen(DIM.1), widen(DIM.2)), "the time is dim");
        assert_eq!(runs.iter().find(|(at, _)| *at == star_at).unwrap().1, gold, "the star is gold");
        assert_eq!(runs.iter().find(|(at, _)| *at == star_at + 3).unwrap().1, line, "the name after it keeps the line's colour");
    }

    #[test]
    fn the_time_in_front_of_a_chat_line_is_found_and_dimmed() {
        assert_eq!(time_len("[18:39] \u{2605} Ann says"), Some(7));
        assert_eq!(time_len("[18:39:04] Ann says"), Some(10));
        assert_eq!(time_len("[VIP] Ann"), None);
        assert_eq!(time_len("[1:3] x"), None);
        assert_eq!(time_len("* Welcome"), None);
        assert_eq!(time_len("[18:39"), None);
        crate::player::symbols::symbol_table::init_symbol_table();
        let mut field = FieldMember::new();
        field.text = "[18:39] \u{2605} Ann says \"hi\"".to_string();
        let len = field.text.len() as u32;
        field.apply_color_to_byte_range(0, len, (211, 143, 211));
        gold_stars(&mut field, 0, len);
        let widen = |c: u8| ((c as u16) << 8) | c as u16;
        let runs = run_colours(&mut field);
        assert_eq!(runs.iter().find(|(at, _)| *at == 0).unwrap().1, (widen(DIM.0), widen(DIM.1), widen(DIM.2)), "the time is dim");
        assert_eq!(runs.iter().find(|(at, _)| *at == 7).unwrap().1, (widen(211), widen(143), widen(211)), "the words keep the line's colour");
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
        assert!(runs.iter().all(|(at, _)| *at == 0 || *at == 10 || *at == first_line), "only the time is split off");
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
