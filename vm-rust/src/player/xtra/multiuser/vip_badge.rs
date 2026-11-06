//! The chat badges (ROTL): VIP, MOD and ADMIN.
//!
//! The server marks a badge holder's chat with a plain tag, `[VIP]`, `[MOD]` or `[ADMIN]`, because the connection to the movie carries one byte
//! per character and cannot carry a picture. The movie only prints the line it is given, so this Xtra swaps the tag for one character on its
//! way in (see `Badge::ch`), and the text renderer draws a small picture where that character stands alone as a word. Only the two places the
//! server puts the tag are touched, so a player typing `[VIP]` into their own message is not turned into a badge:
//!
//!   map chat       `255<[12:00] [VIP] Ann says "hi"`    ->  `255<[12:00] ★ Ann says "hi"`
//!   a private one  `133<[12:00] > [VIP] Ann tells you "hi"` ->  `133<[12:00] > ★ Ann tells you "hi"`
//!   a broadcast    `255<Ann [VIP] [12:00] Broadcasts: "hi"` ->  `255<Ann ★ [12:00] Broadcasts: "hi"`
//!   guild chat     `[12:00] [VIP] Ann tells Guild "hi"` ->  `[12:00] ★ Ann tells Guild "hi"` (subject `GLDCHT`, no colour prefix)
//!
//! A renderer that does not draw the picture shows the character, coloured (`style_chat_line`); a player without this module shows the tag.

use std::cell::Cell;

use crate::player::cast_member::FieldMember;

pub const SUBJECT: &str = "Broadcast";
/// Guild chat: the line has no colour prefix, the movie puts it in the chat field as it is.
pub const GUILD_SUBJECT: &str = "GLDCHT";
/// The colour of the time in front of a chat line.
pub const DIM: (u8, u8, u8) = (0x8F, 0x8F, 0x8F);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Badge {
    Vip,
    Mod,
    Admin,
}

impl Badge {
    pub const ALL: [Badge; 3] = [Badge::Vip, Badge::Mod, Badge::Admin];

    /// What the server writes.
    fn tag(self) -> &'static str {
        match self {
            Badge::Vip => "[VIP]",
            Badge::Mod => "[MOD]",
            Badge::Admin => "[ADMIN]",
        }
    }

    /// The one character the badge travels as inside the movie.
    pub fn ch(self) -> char {
        match self {
            Badge::Vip => '\u{2605}',
            Badge::Mod => '\u{25C6}',
            Badge::Admin => '\u{25B2}',
        }
    }

    /// The words on the picture.
    fn label(self) -> &'static str {
        match self {
            Badge::Vip => "\u{2605} VIP",
            Badge::Mod => "MOD",
            Badge::Admin => "ADMIN",
        }
    }

    /// Fill, border and letters of the picture.
    fn paint(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Badge::Vip => ("#E9B93A", "#8B5A1B", "#4A2D0A"),
            Badge::Mod => ("#6DB3EE", "#1F4E7A", "#0B2540"),
            Badge::Admin => ("#E8645B", "#7A1F1A", "#3A0B08"),
        }
    }

    /// The colour of the character where no picture is drawn.
    fn fallback(self) -> (u8, u8, u8) {
        match self {
            Badge::Vip => (0xF5, 0xC5, 0x42),
            Badge::Mod => (0x6D, 0xB3, 0xEE),
            Badge::Admin => (0xE8, 0x64, 0x5B),
        }
    }
}

impl Badge {
    /// The name the server uses for it in a profile (`%VIPBADGE:vip`).
    fn from_kind(kind: &str) -> Option<Badge> {
        match kind.trim() {
            "vip" => Some(Badge::Vip),
            "mod" => Some(Badge::Mod),
            "admin" => Some(Badge::Admin),
            _ => None,
        }
    }
}

thread_local! {
    /// The badge of the profile the movie is about to show: set when its `uBio1` message is taken from the queue, used when the movie writes
    /// the name into the profile window's header field.
    static PROFILE_BADGE: Cell<Option<Badge>> = const { Cell::new(None) };
}

pub const PROFILE_SUBJECT: &str = "uBio1";
const PROFILE_MARK: &str = "%VIPBADGE:";
/// The profile window's header field (the character's name; the movie keeps the real name in a variable of its own and never reads this back).
const PROFILE_NAME_FIELD: &str = "Bio-Name";

/// Called as a message leaves the queue for the movie. A `uBio1` profile carries its badge as a last item (`...%VIPBADGE:vip`, after the ones the
/// movie reads): it is taken off, so the movie sees the profile as it always was, and remembered for the name field. Any other profile message
/// (a profile without a badge, or an NPC's) forgets it.
pub fn take_profile_badge(subject: &str, content: &mut crate::director::static_datum::StaticDatum) {
    use crate::director::static_datum::StaticDatum;
    if subject != PROFILE_SUBJECT && subject != "NPCBio" {
        return;
    }
    let mut found = None;
    if subject == PROFILE_SUBJECT {
        if let StaticDatum::String(text) = content {
            if let Some((rest, kind)) = text.rsplit_once(PROFILE_MARK) {
                if let Some(badge) = Badge::from_kind(kind) {
                    found = Some(badge);
                    *text = rest.to_string();
                }
            }
        }
    }
    PROFILE_BADGE.with(|b| b.set(found));
}

/// After the movie wrote a field's text: the profile window's name gets its badge character in front (a word of its own, so the renderer draws the
/// picture), if the profile being shown has one.
pub fn decorate_profile_name(member_name: &str, field: &mut FieldMember) {
    if !member_name.eq_ignore_ascii_case(PROFILE_NAME_FIELD) {
        return;
    }
    let Some(badge) = PROFILE_BADGE.with(|b| b.get()) else { return };
    if !field.text.is_empty() && !field.text.starts_with(badge.ch()) {
        let text = format!("{} {}", badge.ch(), field.text);
        field.set_text_preserving_caret(text);
    }
}

/// Which badge a word is, if it is just the badge's character.
pub fn badge_of(token: &str) -> Option<Badge> {
    let mut chars = token.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Badge::ALL.into_iter().find(|b| b.ch() == c)
}

/// The text with each badge character swapped for a few wide letters, to measure a line as the picture will fill it.
pub fn for_measuring(text: &str) -> String {
    Badge::ALL.iter().fold(text.to_string(), |t, b| t.replace(b.ch(), "WWW"))
}

fn badge_font(size_px: f64) -> String {
    format!("bold {}px Arial, sans-serif", (size_px * 0.72).round().max(7.0))
}

/// How wide the badge picture is: its letters and a little room round them.
pub fn badge_width(ctx: &web_sys::CanvasRenderingContext2d, size_px: f64, badge: Badge) -> f64 {
    ctx.set_font(&badge_font(size_px));
    let letters = ctx.measure_text(badge.label()).map(|m| m.width()).unwrap_or(size_px * 2.4);
    (letters + 8.0).ceil()
}

/// Draws the badge in full colour on a canvas of its own (2x, like the text canvas) and returns its pixels (RGBA) with their size.
/// `width` is what `badge_width` gave and `size_px` the line's font size; the picture is `ceil(width)` by `ceil(size_px)` pixels at 1x.
pub fn badge_pixels(document: &web_sys::Document, badge: Badge, width: f64, size_px: f64) -> Option<(Vec<u8>, u32, u32)> {
    use wasm_bindgen::JsCast;
    let (w, h) = (width.ceil().max(1.0), size_px.ceil().max(8.0));
    let canvas: web_sys::HtmlCanvasElement = document.create_element("canvas").ok()?.dyn_into().ok()?;
    let (cw, ch) = ((w * 2.0) as u32, (h * 2.0) as u32);
    canvas.set_width(cw);
    canvas.set_height(ch);
    let ctx: web_sys::CanvasRenderingContext2d = canvas.get_context("2d").ok()??.dyn_into().ok()?;
    let _ = ctx.scale(2.0, 2.0);
    let (fill, border, ink) = badge.paint();
    // A rounded pill with a 1 px border, a pixel in from the edges so the outline is whole.
    let (left, top, pw, ph) = (0.5, 1.0, w - 1.0, h - 2.0);
    let r = 3.0;
    ctx.begin_path();
    ctx.move_to(left + r, top);
    let _ = ctx.arc_to(left + pw, top, left + pw, top + ph, r);
    let _ = ctx.arc_to(left + pw, top + ph, left, top + ph, r);
    let _ = ctx.arc_to(left, top + ph, left, top, r);
    let _ = ctx.arc_to(left, top, left + pw, top, r);
    ctx.close_path();
    ctx.set_fill_style_str(fill);
    ctx.fill();
    ctx.set_line_width(1.0);
    ctx.set_stroke_style_str(border);
    ctx.stroke();
    ctx.set_font(&badge_font(size_px));
    ctx.set_text_baseline("middle");
    ctx.set_text_align("center");
    ctx.set_fill_style_str(ink);
    let _ = ctx.fill_text(badge.label(), left + pw / 2.0, top + ph / 2.0 + 0.5);
    let data = ctx.get_image_data(0.0, 0.0, cw as f64, ch as f64).ok()?.data().0;
    Some((data, cw, ch))
}

/// Puts a badge's pixels (from `badge_pixels`, 2x) on a text bitmap (RGBA, 1x) with its top-left at `(dest_x, dest_y)`: each bitmap pixel
/// takes the average of the 2x2 block above it, so the edges are smooth.
pub fn composite(bitmap: &mut [u8], bitmap_w: i32, bitmap_h: i32, dest_x: i32, dest_y: i32, pixels: &[u8], pw: u32, ph: u32) {
    let (out_w, out_h) = ((pw / 2) as i32, (ph / 2) as i32);
    for oy in 0..out_h {
        let by = dest_y + oy;
        if by < 0 || by >= bitmap_h {
            continue;
        }
        for ox in 0..out_w {
            let bx = dest_x + ox;
            if bx < 0 || bx >= bitmap_w {
                continue;
            }
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..2 {
                for sx in 0..2 {
                    let i = ((((oy * 2 + sy) as u32) * pw + (ox * 2 + sx) as u32) * 4) as usize;
                    let alpha = pixels[i + 3] as u32;
                    // The canvas hands back straight (un-premultiplied) colour: weight it by its alpha to average.
                    r += pixels[i] as u32 * alpha;
                    g += pixels[i + 1] as u32 * alpha;
                    b += pixels[i + 2] as u32 * alpha;
                    a += alpha;
                }
            }
            if a == 0 {
                continue;
            }
            let at = ((by as usize) * bitmap_w as usize + bx as usize) * 4;
            if at + 3 < bitmap.len() {
                bitmap[at] = (r / a) as u8;
                bitmap[at + 1] = (g / a) as u8;
                bitmap[at + 2] = (b / a) as u8;
                bitmap[at + 3] = (a / 4) as u8;
            }
        }
    }
}

/// The length in bytes of a time like `[18:39]` or `[18:39:04]` at the very start of `line`, if the line starts with one.
pub fn time_len(line: &str) -> Option<usize> {
    let rest = line.strip_prefix('[')?;
    let end = rest.find(']')?;
    let time = &rest[..end];
    let parts: Vec<&str> = time.split(':').collect();
    let ok = (parts.len() == 2 || parts.len() == 3) && parts.iter().all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_digit()));
    ok.then_some(end + 2)
}

/// Where the time of a chat line stands in it, as (byte offset, length): at the very start, or in a broadcast after the name and badge
/// (`Ann ★ [12:00] Broadcasts: "hi"`: the name stays the first word, so the time comes later).
pub fn time_span(line: &str) -> Option<(usize, usize)> {
    if let Some(len) = time_len(line) {
        return Some((0, len));
    }
    let head = &line[..line.find(" Broadcasts:")?];
    let at = head.rfind(" [")? + 1;
    time_len(&head[at..]).filter(|len| at + len == head.len()).map(|len| (at, len))
}

/// After a line of a field was given its colour: the time in front is dimmed, so the words stand out from it, and a badge character that no
/// picture will cover is coloured as the badge. `byte_start..byte_end` is the range just coloured.
pub fn style_chat_line(field: &mut FieldMember, byte_start: u32, byte_end: u32) {
    let end = (byte_end as usize).min(field.text.len());
    let start = (byte_start as usize).min(end);
    if let Some((at, len)) = time_span(&field.text[start..end]) {
        field.apply_color_to_byte_range((start + at) as u32, (start + at + len) as u32, DIM);
    }
    for badge in Badge::ALL {
        let ch = badge.ch();
        let at: Vec<usize> = field.text[start..end].match_indices(ch).map(|(at, _)| start + at).collect();
        for pos in at {
            field.apply_color_to_byte_range(pos as u32, (pos + ch.len_utf8()) as u32, badge.fallback());
        }
    }
}

/// The line with the server's tag replaced by the badge's character, or `None` when the line carries no tag in either of the server's places.
pub fn swap_tag(line: &str) -> Option<String> {
    // The movie colour prefix: `<number>` then `<`.
    let (prefix, rest) = line.split_once('<')?;
    if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    swap_in(rest, true).map(|swapped| format!("{prefix}<{swapped}"))
}

/// A guild chat line, `[12:00] [VIP] Ann tells Guild "hi"`: no colour prefix, and the tag stands where map chat has it.
pub fn swap_guild_tag(line: &str) -> Option<String> {
    swap_in(line, false)
}

/// `rest` is a line without its colour prefix. Map chat `[time] [VIP] name says "..."`, a private message `[time] > [VIP] name tells you "..."`
/// (and guild chat the same as map chat), or, when `broadcast` is set, `name [VIP] [time] Broadcasts: "..."`.
fn swap_in(rest: &str, broadcast: bool) -> Option<String> {
    if let Some(after_time) = rest.strip_prefix('[').and_then(|r| r.split_once("] ")).map(|(_, tail)| tail) {
        let lead = if after_time.starts_with("> ") { 2 } else { 0 };
        let body = &after_time[lead..];
        for badge in Badge::ALL {
            if let Some(tail) = body.strip_prefix(badge.tag()) {
                if tail.starts_with(' ') {
                    let kept = rest.len() - body.len();
                    return Some(format!("{}{}{tail}", &rest[..kept], badge.ch()));
                }
            }
        }
        return None;
    }
    if !broadcast {
        return None;
    }
    let (name, tail) = rest.split_once(' ')?;
    for badge in Badge::ALL {
        if let Some(tail) = tail.strip_prefix(badge.tag()).and_then(|t| t.strip_prefix(' ')) {
            // The time, when there is one, sits between the badge and the word.
            let after_time = time_len(tail).map(|len| &tail[len..]).and_then(|t| t.strip_prefix(' ')).unwrap_or(tail);
            if after_time.starts_with("Broadcasts:") && !name.is_empty() {
                return Some(format!("{name} {} {tail}", badge.ch()));
            }
        }
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
        style_chat_line(&mut field, 0, len);
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
        style_chat_line(&mut field, 0, len);
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
        style_chat_line(&mut field, 0, first_line);
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
        assert_eq!(swap_tag("255<Ann [VIP] [12:00] Broadcasts: \"hi\"").as_deref(), Some("255<Ann \u{2605} [12:00] Broadcasts: \"hi\""), "with the time");
        assert_eq!(swap_tag("252<Riley [ADMIN] [18:39] Broadcasts: \"x\"").as_deref(), Some("252<Riley \u{25B2} [18:39] Broadcasts: \"x\""));
        assert_eq!(swap_tag("255<Ann [VIP] [12:00] shouts: \"hi\""), None);
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

    #[test]
    fn mod_and_admin_tags_become_their_own_characters() {
        assert_eq!(swap_tag("255<[12:00] [MOD] Ann says \"hi\"").as_deref(), Some("255<[12:00] \u{25C6} Ann says \"hi\""));
        assert_eq!(swap_tag("255<[12:00] [ADMIN] Ann says \"hi\"").as_deref(), Some("255<[12:00] \u{25B2} Ann says \"hi\""));
        assert_eq!(swap_tag("255<Ann [MOD] Broadcasts: \"hi\"").as_deref(), Some("255<Ann \u{25C6} Broadcasts: \"hi\""));
        assert_eq!(swap_tag("255<Ann [ADMIN] Broadcasts: \"hi\"").as_deref(), Some("255<Ann \u{25B2} Broadcasts: \"hi\""));
        assert_eq!(swap_tag("255<[12:00] Ann says \"[ADMIN] hi\""), None, "typed by a player: left alone");
        assert_eq!(swap_tag("255<[12:00] [GM] Ann says \"hi\""), None, "only the tags the server writes");
    }

    #[test]
    fn a_word_is_a_badge_only_when_it_is_exactly_the_badge_character() {
        assert_eq!(badge_of("\u{2605}"), Some(Badge::Vip));
        assert_eq!(badge_of("\u{25C6}"), Some(Badge::Mod));
        assert_eq!(badge_of("\u{25B2}"), Some(Badge::Admin));
        assert_eq!(badge_of("\u{2605}x"), None);
        assert_eq!(badge_of(""), None);
        assert_eq!(badge_of("*"), None);
        assert_eq!(for_measuring("a \u{2605} b \u{25B2}"), "a WWW b WWW");
    }

    #[test]
    fn a_badge_picture_is_averaged_down_and_laid_over_the_bitmap() {
        // A 2x picture of 2x2 output pixels: the top-left output pixel is solid red, the top-right half red (two of its four), the rest clear.
        let mut pixels = vec![0u8; 4 * 4 * 4];
        let mut set = |x: usize, y: usize, rgba: [u8; 4]| pixels[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4].copy_from_slice(&rgba);
        for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1), (2, 0), (2, 1)] {
            set(x, y, [200, 0, 0, 255]);
        }
        let mut bitmap = vec![7u8; 4 * 3 * 4];
        composite(&mut bitmap, 4, 3, 1, 1, &pixels, 4, 4);
        let at = |x: usize, y: usize| &bitmap[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
        assert_eq!(at(1, 1), &[200, 0, 0, 255], "solid");
        assert_eq!(at(2, 1), &[200, 0, 0, 127], "half covered: half alpha, the colour kept");
        assert_eq!(at(1, 2), &[7, 7, 7, 7], "nothing drawn: the bitmap is left as it was");
        assert_eq!(at(0, 0), &[7, 7, 7, 7], "outside the picture");
    }

    #[test]
    fn a_profile_loses_its_badge_item_before_the_movie_sees_it_and_the_name_field_gets_the_character() {
        use crate::director::static_datum::StaticDatum;
        crate::player::symbols::symbol_table::init_symbol_table();
        let mut content = StaticDatum::String("Tester:1|inv|1|bio|NONE|%0:0|%|%%0%0:0:0%x%1-0:0:0:%Head1%VIPBADGE:mod".to_string());
        take_profile_badge("uBio1", &mut content);
        match &content {
            StaticDatum::String(t) => assert_eq!(t, "Tester:1|inv|1|bio|NONE|%0:0|%|%%0%0:0:0%x%1-0:0:0:%Head1", "the movie sees what it always saw"),
            _ => panic!("still a string"),
        }
        assert_eq!(PROFILE_BADGE.with(|b| b.get()), Some(Badge::Mod));
        let mut field = FieldMember::new();
        field.text = "Tester".to_string();
        decorate_profile_name("Bio-Name", &mut field);
        assert_eq!(field.text, "\u{25C6} Tester");
        decorate_profile_name("Bio-Name", &mut field);
        assert_eq!(field.text, "\u{25C6} Tester", "not twice");
        let mut other = FieldMember::new();
        other.text = "1 Year Old".to_string();
        decorate_profile_name("Bio-Age", &mut other);
        assert_eq!(other.text, "1 Year Old", "only the name field");
        // A profile without a badge, or an NPC's, forgets it; other messages leave it alone.
        let mut plain = StaticDatum::String("Rival:1|x".to_string());
        take_profile_badge("Broadcast", &mut plain);
        assert_eq!(PROFILE_BADGE.with(|b| b.get()), Some(Badge::Mod));
        take_profile_badge("uBio1", &mut plain);
        assert_eq!(PROFILE_BADGE.with(|b| b.get()), None);
    }

    #[test]
    fn a_private_message_gets_the_badge_after_the_arrow() {
        assert_eq!(swap_tag("133<[12:00] > [ADMIN] Admin tells you \"hi\"").as_deref(), Some("133<[12:00] > \u{25B2} Admin tells you \"hi\""));
        assert_eq!(swap_tag("133<[12:00] > [VIP] Ann tells you \"hi\"").as_deref(), Some("133<[12:00] > \u{2605} Ann tells you \"hi\""));
        assert_eq!(swap_tag("133<[12:00] > [MOD] Gwen (Admin) tells you \"hi\"").as_deref(), Some("133<[12:00] > \u{25C6} Gwen (Admin) tells you \"hi\""));
        assert_eq!(swap_tag("133<[12:00] > Ann tells you \"[VIP] hi\""), None, "typed by a player");
        assert_eq!(swap_tag("133<[12:00] * [VIP] You tell Ann \"hi\""), None, "only the arrow form");
        assert_eq!(swap_tag("133<[12:00] > x [VIP] Ann tells you"), None, "only straight after the arrow");
    }

    #[test]
    fn guild_chat_gets_the_badge_after_the_time() {
        assert_eq!(swap_guild_tag("[12:00] [VIP] Ann tells Heroes \"hi\"").as_deref(), Some("[12:00] \u{2605} Ann tells Heroes \"hi\""));
        assert_eq!(swap_guild_tag("[12:00] [MOD] Ann tells Heroes \"hi\"").as_deref(), Some("[12:00] \u{25C6} Ann tells Heroes \"hi\""));
        assert_eq!(swap_guild_tag("[12:00] [ADMIN] Ann tells Heroes \"[VIP] x\"").as_deref(), Some("[12:00] \u{25B2} Ann tells Heroes \"[VIP] x\""));
        assert_eq!(swap_guild_tag("[12:00] Ann tells Heroes \"[VIP] hi\""), None, "typed by a player");
        assert_eq!(swap_guild_tag("Ann [VIP] [12:00] Broadcasts: \"hi\""), None, "a broadcast form is not a guild line");
        assert_eq!(swap_guild_tag("[12:00] *** Ann has left Heroes."), None);
    }

    #[test]
    fn the_time_is_found_at_the_start_or_after_the_name_in_a_broadcast() {
        assert_eq!(time_span("[12:00] \u{2605} Ann says \"hi\""), Some((0, 7)));
        assert_eq!(time_span("Ann \u{2605} [12:00] Broadcasts: \"hi\""), Some(("Ann \u{2605} ".len(), 7)));
        assert_eq!(time_span("Ann [12:00] Broadcasts: \"hi\""), Some((4, 7)));
        assert_eq!(time_span("Ann Broadcasts: \"at [12:00] sharp\""), None, "inside the words");
        assert_eq!(time_span("Ann says \"hi\""), None);
    }
}
