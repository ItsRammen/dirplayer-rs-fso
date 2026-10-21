//! ShockFiler Xtra — the file saving half that Return of the Legends uses.
//!
//! ROTL registers it at startup (`sf_Register([2071, 4, 7084])`) and its Edit
//! menu's "Take Screenshot" saves the stage with
//!
//! ```lingo
//! sf_Send("TODISK", the applicationPath, "", "", "StageHolder", "",
//!         fileName, #jpeg, [#quality: 75], 0, 1, "")
//! ```
//!
//! A browser has no disk to save to, so "to disk" means the player's download
//! folder: the named bitmap member is encoded as JPEG or PNG and downloaded as
//! `fileName`. Anything else ShockFiler can do (open and save dialogs, reading
//! files) has no browser meaning and answers 0, "not done".

use crate::{
    director::lingo::datum::Datum,
    player::{cast_member::CastMemberType, reserve_player_mut, reserve_player_ref, DatumRef, ScriptError},
};

use super::download;

pub struct ShockFilerXtra;

impl ShockFilerXtra {
    pub fn has_handler(name: &str) -> bool {
        name.to_ascii_lowercase().starts_with("sf_")
    }

    pub fn call_handler(name: &str, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            // 1 = registered. The movie only checks the result in a commented-out alert.
            "sf_register" => ok_int(1),
            "sf_send" => sf_send(args),
            _ => ok_int(0),
        }
    }
}

fn ok_int(n: i32) -> Result<DatumRef, ScriptError> {
    reserve_player_mut(|player| Ok(player.alloc_datum(Datum::Int(n))))
}

fn string_arg(args: &Vec<DatumRef>, idx: usize) -> String {
    reserve_player_ref(|player| match args.get(idx) {
        Some(a) => Ok::<String, ScriptError>(player.get_datum(a).string_value().unwrap_or_default()),
        None => Ok(String::new()),
    })
    .unwrap_or_default()
}

/// The image format asked for: `#jpeg` / `#jpg` or `#png` (anything else saves as JPEG, as the
/// movie only ever asks for one of those).
pub fn format_from(name: &str) -> &'static str {
    match name.trim().to_ascii_lowercase().as_str() {
        "png" => "png",
        _ => "jpeg",
    }
}

/// `[#quality: 75]` as a 0..1 canvas quality; JPEG at 75 when it isn't given or isn't usable.
pub fn quality_from(percent: Option<i32>) -> f64 {
    match percent {
        Some(q) if (1..=100).contains(&q) => q as f64 / 100.0,
        _ => 0.75,
    }
}

/// The file name to download: the one asked for, as a bare name, with an extension that
/// matches the format.
pub fn download_name(requested: &str, format: &str) -> String {
    let mut name = download::base_name(requested);
    if name.is_empty() {
        name = "screenshot".to_string();
    }
    let ext = if format == "png" { "png" } else { "jpg" };
    let lower = name.to_ascii_lowercase();
    let has_ext = if format == "png" {
        lower.ends_with(".png")
    } else {
        lower.ends_with(".jpg") || lower.ends_with(".jpeg")
    };
    if has_ext {
        name
    } else {
        format!("{}.{}", name, ext)
    }
}

/// Opaque RGB over white for JPEG (which has no alpha), straight RGBA for PNG.
pub fn finish_pixels(rgba: &mut [u8], keep_alpha: bool) {
    if keep_alpha {
        return;
    }
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a != 255 {
            for c in 0..3 {
                px[c] = ((px[c] as u32 * a + 255 * (255 - a)) / 255) as u8;
            }
            px[3] = 255;
        }
    }
}

fn sf_send(args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
    // Only "send to disk" has a browser meaning.
    if !string_arg(args, 0).eq_ignore_ascii_case("TODISK") {
        return ok_int(0);
    }
    let format = format_from(&string_arg(args, 7));
    let file_name = download_name(&string_arg(args, 6), format);

    // The picture: the member named (or numbered) in the fifth argument.
    let picture = reserve_player_ref(|player| {
        let Some(arg) = args.get(4) else { return Ok(None) };
        let member_ref = match player.get_datum(arg) {
            Datum::CastMember(member_ref) => Some(member_ref.clone()),
            other => player.movie.cast_manager.find_member_ref_by_name(&other.string_value().unwrap_or_default()),
        };
        let Some(member_ref) = member_ref else { return Ok(None) };
        let Some(member) = player.movie.cast_manager.find_member_by_ref(&member_ref) else { return Ok(None) };
        let CastMemberType::Bitmap(bitmap_member) = &member.member_type else { return Ok(None) };
        let Some(bitmap) = player.bitmap_manager.get_bitmap(bitmap_member.image_ref) else { return Ok(None) };
        let palettes = player.movie.cast_manager.palettes();
        let (w, h) = (bitmap.width as usize, bitmap.height as usize);
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let (r, g, b, a) = bitmap.get_pixel_color_with_alpha(&palettes, x as u16, y as u16);
                let i = (y * w + x) * 4;
                rgba[i] = r;
                rgba[i + 1] = g;
                rgba[i + 2] = b;
                rgba[i + 3] = a;
            }
        }
        Ok(Some((w as u32, h as u32, rgba)))
    })?;
    let Some((width, height, mut rgba)) = picture else {
        return ok_int(0);
    };
    if width == 0 || height == 0 {
        return ok_int(0);
    }
    finish_pixels(&mut rgba, format == "png");

    let quality = quality_from(quality_percent(args.get(8)));
    ok_int(if save_picture(&file_name, format, quality, width, height, &rgba) { 1 } else { 0 })
}

/// `quality` from a property list argument, as a percentage.
fn quality_percent(arg: Option<&DatumRef>) -> Option<i32> {
    let arg = arg?;
    reserve_player_ref(|player| {
        let Datum::PropList(pairs, _) = player.get_datum(arg) else { return Ok::<Option<i32>, ScriptError>(None) };
        for (key, value) in pairs.iter() {
            if player.get_datum(key).string_value().map(|k| k.eq_ignore_ascii_case("quality")).unwrap_or(false) {
                return Ok(player.get_datum(value).int_value().ok());
            }
        }
        Ok(None)
    })
    .unwrap_or(None)
}

#[cfg(target_arch = "wasm32")]
fn save_picture(file_name: &str, format: &str, quality: f64, width: u32, height: u32, rgba: &[u8]) -> bool {
    use wasm_bindgen::{Clamped, JsCast, JsValue};
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return false;
    };
    let Some(canvas) = document.create_element("canvas").ok().and_then(|c| c.dyn_into::<web_sys::HtmlCanvasElement>().ok()) else {
        return false;
    };
    canvas.set_width(width);
    canvas.set_height(height);
    let Some(ctx) = canvas.get_context("2d").ok().flatten().and_then(|c| c.dyn_into::<web_sys::CanvasRenderingContext2d>().ok()) else {
        return false;
    };
    let Ok(image_data) = web_sys::ImageData::new_with_u8_clamped_array_and_sh(Clamped(rgba), width, height) else {
        return false;
    };
    if ctx.put_image_data(&image_data, 0.0, 0.0).is_err() {
        return false;
    }
    let mime = if format == "png" { "image/png" } else { "image/jpeg" };
    let Ok(url) = canvas.to_data_url_with_type_and_encoder_options(mime, &JsValue::from_f64(quality)) else {
        return false;
    };
    download::download_url(file_name, &url)
}

#[cfg(not(target_arch = "wasm32"))]
fn save_picture(_file_name: &str, _format: &str, _quality: f64, _width: u32, _height: u32, _rgba: &[u8]) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_png_is_png() {
        assert_eq!(format_from("png"), "png");
        assert_eq!(format_from("PNG"), "png");
        assert_eq!(format_from("jpeg"), "jpeg");
        assert_eq!(format_from("jpg"), "jpeg");
        assert_eq!(format_from(""), "jpeg");
    }

    #[test]
    fn quality_is_a_percentage_with_a_sane_default() {
        assert_eq!(quality_from(Some(75)), 0.75);
        assert_eq!(quality_from(Some(100)), 1.0);
        assert_eq!(quality_from(None), 0.75);
        assert_eq!(quality_from(Some(0)), 0.75);
        assert_eq!(quality_from(Some(400)), 0.75);
    }

    #[test]
    fn the_download_name_is_bare_and_matches_the_format() {
        assert_eq!(download_name("Screenshot123.jpg", "jpeg"), "Screenshot123.jpg");
        assert_eq!(download_name("C:\\ROTL\\Screenshot123.jpg", "jpeg"), "Screenshot123.jpg");
        assert_eq!(download_name("shot", "png"), "shot.png");
        assert_eq!(download_name("shot.png", "jpeg"), "shot.png.jpg", "never mislabel a PNG as a JPEG");
        assert_eq!(download_name("", "jpeg"), "screenshot.jpg");
    }

    #[test]
    fn jpeg_pixels_are_flattened_onto_white() {
        let mut px = vec![0, 0, 0, 0, 10, 20, 30, 255, 100, 100, 100, 128];
        finish_pixels(&mut px, false);
        assert_eq!(&px[0..4], &[255, 255, 255, 255], "fully transparent becomes white");
        assert_eq!(&px[4..8], &[10, 20, 30, 255], "opaque is unchanged");
        assert_eq!(px[11], 255);
        assert!(px[8] > 100 && px[8] < 255, "half transparent is lightened");
    }

    #[test]
    fn png_pixels_keep_their_alpha() {
        let mut px = vec![1, 2, 3, 0];
        finish_pixels(&mut px, true);
        assert_eq!(px, vec![1, 2, 3, 0]);
    }
}
