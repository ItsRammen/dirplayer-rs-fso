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

/// An item's picture: the bitmap cast member the movie itself draws for it (the movie finds it with `member(<item name>)`, so this does too).
pub struct Icon {
    pub width: usize,
    pub height: usize,
    /// True when the bitmap carries its own alpha; otherwise the page keys out the background colour, as the movie's ink 36 does.
    pub has_alpha: bool,
    pub rgba: Vec<u8>,
}

pub fn icon(name: &str) -> Option<Icon> {
    use crate::player::cast_member::CastMemberType;
    crate::player::reserve_player_ref(|player| {
        let member_ref = player.movie.cast_manager.find_member_ref_by_name(name)?;
        let member = player.movie.cast_manager.find_member_by_ref(&member_ref)?;
        let CastMemberType::Bitmap(_) = &member.member_type else { return None };
        let found = bitmap_of(player, &member_ref)?;
        // Only item-sized pictures: a name that happens to belong to a stage graphic is not an icon.
        if found.width > 128 || found.height > 128 {
            return None;
        }
        Some(found)
    })
}

/// A piece of the game's own interface art (a panel, an arrow, a button) by cast library and member number, at any size. The Modern UI draws the Original
/// interface's extra controls from these, so they look like the rest of the frame.
pub fn member_image(cast_lib: i32, cast_member: i32) -> Option<Icon> {
    crate::player::reserve_player_ref(|player| {
        let member_ref = crate::player::cast_lib::CastMemberRef { cast_lib, cast_member };
        bitmap_of(player, &member_ref)
    })
}

fn bitmap_of(player: &crate::player::DirPlayer, member_ref: &crate::player::cast_lib::CastMemberRef) -> Option<Icon> {
    use crate::player::cast_member::CastMemberType;
    let member = player.movie.cast_manager.find_member_by_ref(member_ref)?;
    let CastMemberType::Bitmap(bitmap_member) = &member.member_type else { return None };
    let bitmap = player.bitmap_manager.get_bitmap(bitmap_member.image_ref)?;
    let palettes = player.movie.cast_manager.palettes();
    let rgba = crate::rendering_gpu::webgl2::WebGL2Renderer::bitmap_to_rgba(bitmap, &palettes, 0, None, None, None, (0, 0), false);
    let (width, height) = (bitmap.width as usize, bitmap.height as usize);
    if width == 0 || height == 0 || rgba.len() != width * height * 4 {
        return None;
    }
    Some(Icon { width, height, has_alpha: bitmap.original_bit_depth == 32 && bitmap.use_alpha, rgba })
}
