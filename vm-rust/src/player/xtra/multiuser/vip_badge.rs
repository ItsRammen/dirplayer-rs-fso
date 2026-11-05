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

pub const SUBJECT: &str = "Broadcast";
const TAG: &str = "[VIP]";
const STAR: &str = "\u{2605}";

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
