//! ROTL custom art: pictures the page adds to the movie's casts as bitmap members (`rotl_add_bitmap`, `rotl_add_bitmaps` in lib.rs).
//!
//! The page adds every custom picture before the movie needs it, so the movie finds each one by name like one of its own. An add
//! used to drop the movie-wide member-name table, and the next add's "is this name taken?" check rebuilt it from every member of
//! every cast: about 3.7 ms an add in the browser, 2.5 to 5 s for the site's 640 to 1,427 pictures. A new name now goes into the
//! table where it stands ([`CastManager::note_member_added`]), so an add costs its picture's decode and little else, and a batch
//! crosses from the page into the player once.

use crate::director::enums::BitmapInfo;
use crate::player::bitmap::bitmap::{Bitmap, BuiltInPalette, PaletteRef};
use crate::player::cast_lib::{cast_member_ref, CastMemberRef};
use crate::player::cast_member::{BitmapMember, CastMember, CastMemberType};
use crate::player::DirPlayer;

/// A side of a picture is 1 to this many pixels.
pub const MAX_SIDE: u32 = 1024;

/// The picture of one add: a PNG (or any format the player decodes), or pixels already decoded (RGBA, 4 bytes a pixel).
pub enum ArtPixels<'a> {
    Encoded(&'a [u8]),
    Rgba { width: u32, height: u32, data: Vec<u8> },
}

/// One picture for the movie: the cast library by name (e.g. "Inventory"), the member's name and registration point, and
/// whether it may replace the bitmap member that already has the name.
pub struct ArtAdd<'a> {
    pub cast: &'a str,
    pub name: &'a str,
    pub pixels: ArtPixels<'a>,
    pub reg_x: i32,
    pub reg_y: i32,
    pub replace: bool,
}

/// What an add did: the member, the picture's size, and whether it was new (false: an existing member got the picture).
#[derive(Clone, Debug, PartialEq)]
pub struct ArtAdded {
    pub member: CastMemberRef,
    pub width: u16,
    pub height: u16,
    pub created: bool,
}

/// Checks the name and registration point and decodes the picture. Needs no player, so a batch decodes outside the player's lock.
pub fn prepare(add: ArtAdd) -> Result<Prepared, String> {
    let name = add.name;
    if name.is_empty() || name.len() > 255 || name.trim() != name {
        return Err(format!("bad member name {:?}", name));
    }
    let (w, h, data) = match add.pixels {
        ArtPixels::Encoded(bytes) => {
            let decoded = image::load_from_memory(bytes)
                .map_err(|e| format!("{}: cannot decode the picture ({} bytes): {}", name, bytes.len(), e))?
                .to_rgba8();
            let (w, h) = decoded.dimensions();
            (w, h, decoded.into_raw())
        }
        ArtPixels::Rgba { width, height, data } => {
            if (width as u64) * (height as u64) * 4 != data.len() as u64 {
                return Err(format!("{}: {} bytes of pixels for {}x{} (4 bytes a pixel)", name, data.len(), width, height));
            }
            (width, height, data)
        }
    };
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Err(format!("{}: picture is {}x{} (1 to {} pixels a side)", name, w, h, MAX_SIDE));
    }
    if add.reg_x.abs() > 0x7FFF || add.reg_y.abs() > 0x7FFF {
        return Err(format!("{}: registration point ({}, {}) out of range", name, add.reg_x, add.reg_y));
    }
    Ok(Prepared {
        cast: add.cast.to_string(),
        name: name.to_string(),
        width: w as u16,
        height: h as u16,
        rgba: data,
        reg_x: add.reg_x,
        reg_y: add.reg_y,
        replace: add.replace,
    })
}

/// A checked, decoded picture, ready for [`install`].
pub struct Prepared {
    cast: String,
    name: String,
    width: u16,
    height: u16,
    rgba: Vec<u8>,
    reg_x: i32,
    reg_y: i32,
    replace: bool,
}

/// Puts a prepared picture into the movie: a new bitmap member in the first free slot of its cast, or, with `replace`, the new
/// picture in the bitmap member that has the name. A name the movie already has (in any cast) is refused otherwise.
///
/// The member is 32-bit, painted exactly as given (no white-space trimming); the movie's ink, e.g. background transparent over
/// the key colour, applies as for its own members.
pub fn install(player: &mut DirPlayer, p: Prepared) -> Result<ArtAdded, String> {
    let (w, h) = (p.width, p.height);
    let name = p.name.as_str();
    let mut bitmap = Bitmap::new(w, h, 32, 32, 8, PaletteRef::BuiltIn(BuiltInPalette::SystemWin));
    bitmap.data = p.rgba;
    bitmap.trim_white_space = false;
    let info = BitmapInfo {
        width: w,
        height: h,
        reg_x: p.reg_x as i16,
        reg_y: p.reg_y as i16,
        bit_depth: 32,
        pitch: w.saturating_mul(4),
        trim_white_space: false,
        ..BitmapInfo::default()
    };

    let cast_index = player
        .movie
        .cast_manager
        .casts
        .iter()
        .position(|c| c.name.eq_ignore_ascii_case(&p.cast))
        .ok_or_else(|| format!("{}: the movie has no cast library {:?}", name, p.cast))?;

    if let Some(existing) = player.movie.cast_manager.find_member_ref_by_name(name) {
        if !p.replace {
            return Err(format!(
                "{}: the movie already has a member of that name (member {} of castLib {})",
                name, existing.cast_member, existing.cast_lib
            ));
        }
        let member = player
            .movie
            .cast_manager
            .find_mut_member_by_ref(&existing)
            .ok_or_else(|| format!("{}: member vanished", name))?;
        let image_ref = match &mut member.member_type {
            CastMemberType::Bitmap(b) => {
                b.info = BitmapInfo { palette_id: b.info.palette_id, clut_cast_lib: b.info.clut_cast_lib, ..info };
                b.reg_point = (p.reg_x as i16, p.reg_y as i16);
                b.image_ref
            }
            _ => return Err(format!("{}: the existing member is not a bitmap", name)),
        };
        member.reg_point = (p.reg_x, p.reg_y);
        player.bitmap_manager.replace_bitmap(image_ref, bitmap);
        return Ok(ArtAdded { member: existing, width: w, height: h, created: false });
    }

    let cast = &mut player.movie.cast_manager.casts[cast_index];
    let number = cast.first_free_member_id();
    if number == 0 {
        return Err(format!("{}: cast library {:?} is full", name, p.cast));
    }
    let image_ref = player.bitmap_manager.add_bitmap(bitmap);
    let mut member = CastMember::new(
        number,
        CastMemberType::Bitmap(BitmapMember {
            image_ref,
            reg_point: (p.reg_x as i16, p.reg_y as i16),
            script_id: 0,
            member_script_ref: None,
            info,
        }),
    );
    member.name = p.name.clone();
    member.reg_point = (p.reg_x, p.reg_y);
    let cast_number = cast.number;
    cast.insert_member(number, member, &mut player.bitmap_manager);
    // The name was free in every cast (checked above), so it goes into the movie-wide table as it stands; dropping the table
    // instead made the next lookup rebuild it from every member of every cast.
    player.movie.cast_manager.note_member_added(cast_number, number, name);
    Ok(ArtAdded { member: cast_member_ref(cast_number as i32, number as i32), width: w, height: h, created: true })
}

/// [`prepare`] then [`install`].
pub fn add(player: &mut DirPlayer, add: ArtAdd) -> Result<ArtAdded, String> {
    install(player, prepare(add)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::testing::TestPlayer;
    use crate::player::{reserve_player_mut, reserve_player_ref};

    fn png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba(rgba));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn with_casts(names: &[&str]) {
        use crate::player::cast_lib::{CastLib, CastLibState};
        use fxhash::FxHashMap;
        crate::player::symbols::symbol_table::init_symbol_table();
        reserve_player_mut(|player| {
            player.movie.cast_manager.casts.clear();
            for (i, name) in names.iter().enumerate() {
                player.movie.cast_manager.casts.push(CastLib {
                    name: name.to_string(),
                    file_name: String::new(),
                    number: (i + 1) as u32,
                    is_external: false,
                    state: CastLibState::Loaded,
                    lctx: None,
                    members: FxHashMap::default(),
                    scripts: FxHashMap::default(),
                    name_symbols: Vec::new(),
                    preload_mode: 0,
                    capital_x: false,
                    dir_version: 0,
                    palette_id_offset: 0,
                    name_index: std::cell::RefCell::new(None),
                    font_table: std::collections::HashMap::new(),
                });
            }
            player.movie.cast_manager.invalidate_member_name_cache();
        });
    }

    #[test]
    fn adds_keep_the_name_table_right_without_rebuilding_it() {
        let _player = TestPlayer::new();
        with_casts(&["Internal", "Inventory"]);
        let picture = png(3, 2, [255, 0, 0, 255]);
        let first = reserve_player_mut(|p| {
            // warm the table, then add: the name must be found without dropping it
            assert!(p.movie.cast_manager.find_member_ref_by_name("Nothing").is_none());
            add(p, ArtAdd { cast: "inventory", name: "Red Gem", pixels: ArtPixels::Encoded(&picture), reg_x: 1, reg_y: 2, replace: false })
        })
        .unwrap();
        assert_eq!(first, ArtAdded { member: cast_member_ref(2, 1), width: 3, height: 2, created: true });
        reserve_player_ref(|p| {
            assert!(p.movie.cast_manager.member_name_cache.borrow().is_some(), "the table is kept");
            assert_eq!(p.movie.cast_manager.find_member_ref_by_name("red gem"), Some(cast_member_ref(2, 1)));
            assert_eq!(p.movie.cast_manager.find_member_ref_by_number(65537), Some(cast_member_ref(2, 1)));
            let cast = p.movie.cast_manager.get_cast_by_name("Inventory").unwrap();
            assert_eq!(cast.find_member_by_name("RED GEM").map(|m| m.number), Some(1));
        });
        // the same name again is refused, in any case
        let again = reserve_player_mut(|p| add(p, ArtAdd { cast: "Internal", name: "RED GEM", pixels: ArtPixels::Encoded(&picture), reg_x: 0, reg_y: 0, replace: false }));
        assert!(again.unwrap_err().contains("already has a member of that name"));
        // a second one takes the next slot; the table built from scratch agrees with the one kept up to date
        let second = reserve_player_mut(|p| add(p, ArtAdd { cast: "Inventory", name: "Blue Gem", pixels: ArtPixels::Rgba { width: 1, height: 1, data: vec![0, 0, 255, 255] }, reg_x: 0, reg_y: 0, replace: false })).unwrap();
        assert_eq!(second.member, cast_member_ref(2, 2));
        let kept = reserve_player_ref(|p| p.movie.cast_manager.member_name_cache.borrow().clone().unwrap());
        reserve_player_mut(|p| {
            p.movie.cast_manager.invalidate_member_name_cache();
            p.movie.cast_manager.find_member_ref_by_name("x");
        });
        let rebuilt = reserve_player_ref(|p| p.movie.cast_manager.member_name_cache.borrow().clone().unwrap());
        assert_eq!(kept, rebuilt);
        // replace: same member, new picture and registration point
        let replaced = reserve_player_mut(|p| add(p, ArtAdd { cast: "Inventory", name: "Red Gem", pixels: ArtPixels::Encoded(&png(5, 4, [0, 255, 0, 255])), reg_x: 7, reg_y: 8, replace: true })).unwrap();
        assert_eq!(replaced, ArtAdded { member: cast_member_ref(2, 1), width: 5, height: 4, created: false });
        reserve_player_ref(|p| {
            let m = p.movie.cast_manager.find_member_by_ref(&cast_member_ref(2, 1)).unwrap();
            assert_eq!(m.reg_point, (7, 8));
        });
    }

    #[test]
    fn refusals() {
        let _player = TestPlayer::new();
        with_casts(&["Inventory"]);
        let r = |a: ArtAdd| reserve_player_mut(|p| add(p, a)).unwrap_err();
        let ok = png(1, 1, [0, 0, 0, 255]);
        assert!(r(ArtAdd { cast: "Nope", name: "A", pixels: ArtPixels::Encoded(&ok), reg_x: 0, reg_y: 0, replace: false }).contains("has no cast library"));
        assert!(r(ArtAdd { cast: "Inventory", name: " A", pixels: ArtPixels::Encoded(&ok), reg_x: 0, reg_y: 0, replace: false }).contains("bad member name"));
        assert!(r(ArtAdd { cast: "Inventory", name: "A", pixels: ArtPixels::Encoded(b"nope"), reg_x: 0, reg_y: 0, replace: false }).contains("cannot decode"));
        assert!(r(ArtAdd { cast: "Inventory", name: "A", pixels: ArtPixels::Rgba { width: 2, height: 2, data: vec![0; 4] }, reg_x: 0, reg_y: 0, replace: false }).contains("bytes of pixels"));
        assert!(r(ArtAdd { cast: "Inventory", name: "A", pixels: ArtPixels::Encoded(&ok), reg_x: 40000, reg_y: 0, replace: false }).contains("out of range"));
        assert!(r(ArtAdd { cast: "Inventory", name: "A", pixels: ArtPixels::Encoded(&png(1025, 1, [0; 4])), reg_x: 0, reg_y: 0, replace: false }).contains("pixels a side"));
        reserve_player_ref(|p| assert!(p.movie.cast_manager.casts[0].members.is_empty()));
    }
}
