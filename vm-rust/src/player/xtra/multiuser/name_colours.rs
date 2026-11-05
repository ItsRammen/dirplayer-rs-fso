//! Name colours the movie cannot draw (ROTL's VIP and Immortal names).
//!
//! The movie colours a player's name label from the alignment tag the server sends and knows only three colours (white, orange,
//! red). The server also sends a `NameColours` message, `name=RRGGBB;name=RRGGBB` (the whole list each time, only the names that
//! have a colour of their own), which this Xtra keeps instead of passing on, and the label's colour setter asks `for_label`
//! whether the name in the label has one. A movie or server without the message behaves exactly as before.
use std::cell::RefCell;
use std::collections::HashMap;

use crate::player::{cast_member::{CastMember, CastMemberType}, sprite::ColorRef};

pub const SUBJECT: &str = "NameColours";

thread_local! {
    static COLOURS: RefCell<HashMap<String, (u8, u8, u8)>> = RefCell::new(HashMap::new());
}

fn parse(list: &str) -> HashMap<String, (u8, u8, u8)> {
    let mut colours = HashMap::new();
    for entry in list.split(';') {
        let Some((name, hex)) = entry.rsplit_once('=') else { continue };
        let hex = hex.trim().trim_start_matches('#');
        if hex.len() != 6 {
            continue;
        }
        let Ok(rgb) = u32::from_str_radix(hex, 16) else { continue };
        let name = name.trim().to_lowercase();
        if !name.is_empty() {
            colours.insert(name, ((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8));
        }
    }
    colours
}

/// Replaces the list with the one the server just sent.
pub fn replace(list: &str) {
    let parsed = parse(list);
    COLOURS.with(|c| *c.borrow_mut() = parsed);
}

/// A name label is the field `Sprite<channel>Name` (the player's own is `Sprite20Name`) holding the player's name.
fn is_name_label(member_name: &str) -> bool {
    let lower = member_name.to_ascii_lowercase();
    lower
        .strip_prefix("sprite")
        .and_then(|rest| rest.strip_suffix("name"))
        .map_or(false, |digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// The colour to draw this label in, if it is a name label and the name in it has a colour of its own.
pub fn for_label(member_name: &str, text: &str) -> Option<(u8, u8, u8)> {
    if !is_name_label(member_name) {
        return None;
    }
    let name = text.trim().to_lowercase();
    COLOURS.with(|c| c.borrow().get(&name).copied())
}

const WHITE: (u8, u8, u8) = (255, 255, 255);

/// Called after the movie sets the colour of a field or text member. If it is a name label the colour is the name's own when it has one;
/// and a label that still carries a colour from this module (the movie only ever uses palette indexes) for a name that has none any more
/// (the label was reused for another player, or the colour was taken away) goes back to the movie's ordinary colour, white.
pub fn restyle(member: &mut CastMember) {
    if !is_name_label(&member.name) {
        return;
    }
    let text = match &member.member_type {
        CastMemberType::Field(field) => field.text.clone(),
        CastMemberType::Text(text) => text.text.clone(),
        _ => return,
    };
    let own = for_label(&member.name, &text);
    match &mut member.member_type {
        CastMemberType::Field(field) => {
            if let Some((r, g, b)) = own {
                field.fore_color = Some(ColorRef::Rgb(r, g, b));
                field.set_all_run_colors((r, g, b));
            } else if matches!(field.fore_color, Some(ColorRef::Rgb(..))) {
                field.fore_color = Some(ColorRef::PaletteIndex(0));
                field.set_all_run_colors(WHITE);
            }
        }
        CastMemberType::Text(text) => {
            let reset = own.is_none() && matches!(member.color, ColorRef::Rgb(..));
            if let Some((r, g, b)) = own {
                member.color = ColorRef::Rgb(r, g, b);
            } else if reset {
                member.color = ColorRef::PaletteIndex(0);
            }
            if own.is_some() || reset {
                for span in &mut text.html_styled_spans {
                    span.style.color = None;
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listed_name_gets_its_colour_in_its_label_only() {
        replace("Ann=00DFE7;bob=#31E721;broken;=FFFFFF;cy=12345");
        assert_eq!(for_label("Sprite20Name", "Ann"), Some((0x00, 0xDF, 0xE7)));
        assert_eq!(for_label("sprite24name", " BOB "), Some((0x31, 0xE7, 0x21)));
        assert_eq!(for_label("Sprite20Name", "cy"), None, "a colour that is not six digits is dropped");
        assert_eq!(for_label("Sprite20Name", "Dee"), None);
        assert_eq!(for_label("chat", "Ann"), None, "only name labels");
        assert_eq!(for_label("SpriteName", "Ann"), None);
        assert_eq!(for_label("Sprite2xName", "Ann"), None);
    }

    #[test]
    fn each_list_replaces_the_last() {
        replace("ann=FFFFFF");
        replace("");
        assert_eq!(for_label("Sprite20Name", "ann"), None);
    }
}
