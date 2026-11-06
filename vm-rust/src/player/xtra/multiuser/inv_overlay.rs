//! The inventory pages overlay (ROTL, prototype).
//!
//! The movie's inventory shows 30 slots. The server keeps more in extra pages and answers the page overlay's requests with an `InvAll`
//! message (the whole inventory as JSON). The movie knows nothing of it, so this Xtra keeps the message off the movie and hands it to the web
//! page's `window.__rotlInvAll(json)`, if the page defines one. A page without the overlay, or a server without the message, behaves as before.

pub const SUBJECT: &str = "InvAll";

/// Passes the server's `InvAll` content to the page. Without a page function the message is dropped.
pub fn deliver(content: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::{JsCast, JsValue};
        let Some(window) = web_sys::window() else { return };
        let Ok(function) = js_sys::Reflect::get(&window, &JsValue::from_str("__rotlInvAll")) else { return };
        if let Some(function) = function.dyn_ref::<js_sys::Function>() {
            let _ = function.call1(&JsValue::NULL, &JsValue::from_str(content));
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = content;
}
