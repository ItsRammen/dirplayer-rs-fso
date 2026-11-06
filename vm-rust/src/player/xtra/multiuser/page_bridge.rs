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
