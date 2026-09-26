use std::collections::HashMap;

use super::bitmap::Bitmap;

pub type BitmapRef = u32;
pub const INVALID_BITMAP_REF: BitmapRef = 0;

struct ManagedBitmap {
    bitmap: Bitmap,
    datum_refs: u32,
    anchored: bool,
    retain_until_movie_clear: bool,
}

pub struct BitmapManager {
    bitmaps: HashMap<BitmapRef, ManagedBitmap>,
    ref_counter: BitmapRef,
}

impl BitmapManager {
    pub fn new() -> Self {
        Self { bitmaps: HashMap::new(), ref_counter: 0 }
    }

    /// Clear movie-owned pixels without reusing IDs still held by old datums.
    pub fn clear_movie_bitmaps(&mut self) {
        self.bitmaps.clear();
    }

    /// Register pixels owned by a cast member or another long-lived holder.
    pub fn add_bitmap(&mut self, bitmap: Bitmap) -> BitmapRef {
        self.ref_counter += 1;
        self.bitmaps.insert(self.ref_counter, ManagedBitmap {
            bitmap, datum_refs: 0, anchored: true, retain_until_movie_clear: false,
        });
        self.ref_counter
    }

    /// Register resources also retained outside the cast, such as GIF animation frames.
    /// A member only borrows these pixels; removing it must not invalidate the holder.
    pub fn add_movie_bitmap(&mut self, bitmap: Bitmap) -> BitmapRef {
        let bitmap_ref = self.add_bitmap(bitmap);
        self.bitmaps.get_mut(&bitmap_ref).unwrap().retain_until_movie_clear = true;
        bitmap_ref
    }

    /// Register an image whose lifetime will be held by Lingo BitmapRef datums.
    pub fn add_ephemeral_bitmap(&mut self, bitmap: Bitmap) -> BitmapRef {
        let bitmap_ref = self.add_bitmap(bitmap);
        self.bitmaps.get_mut(&bitmap_ref).unwrap().anchored = false;
        bitmap_ref
    }

    /// Release a removed/replaced member's ownership, preserving saved Lingo images.
    pub fn release_anchor(&mut self, bitmap_ref: BitmapRef) {
        let should_free = self.bitmaps.get_mut(&bitmap_ref).map(|entry| {
            entry.anchored = false;
            !entry.retain_until_movie_clear && entry.datum_refs == 0
        }).unwrap_or(false);
        if should_free {
            self.bitmaps.remove(&bitmap_ref);
        }
    }

    pub fn replace_bitmap(&mut self, bitmap_ref: BitmapRef, mut bitmap: Bitmap) {
        if let Some(entry) = self.bitmaps.get_mut(&bitmap_ref) {
            bitmap.version = entry.bitmap.version.wrapping_add(1);
            entry.bitmap = bitmap;
        } else {
            self.bitmaps.insert(bitmap_ref, ManagedBitmap {
                bitmap, datum_refs: 0, anchored: true, retain_until_movie_clear: false,
            });
        }
    }

    pub fn get_bitmap(&self, bitmap_ref: BitmapRef) -> Option<&Bitmap> {
        self.bitmaps.get(&bitmap_ref).map(|entry| &entry.bitmap)
    }

    pub fn get_bitmap_mut(&mut self, bitmap_ref: BitmapRef) -> Option<&mut Bitmap> {
        self.bitmaps.get_mut(&bitmap_ref).map(|entry| {
            entry.bitmap.version = entry.bitmap.version.wrapping_add(1);
            &mut entry.bitmap
        })
    }

    /// Track every BitmapRef datum, including views saved before a member is erased.
    pub fn incref_bitmap(&mut self, bitmap_ref: BitmapRef) {
        if let Some(entry) = self.bitmaps.get_mut(&bitmap_ref) {
            entry.datum_refs += 1;
        }
    }

    pub fn decref_bitmap(&mut self, bitmap_ref: BitmapRef) {
        let should_free = self.bitmaps.get_mut(&bitmap_ref).map(|entry| {
            entry.datum_refs = entry.datum_refs.saturating_sub(1);
            !entry.anchored && !entry.retain_until_movie_clear && entry.datum_refs == 0
        }).unwrap_or(false);
        if should_free {
            self.bitmaps.remove(&bitmap_ref);
        }
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use crate::player::bitmap::bitmap::{BuiltInPalette, PaletteRef};

    fn image() -> Bitmap {
        Bitmap::new(8, 8, 32, 32, 8, PaletteRef::BuiltIn(BuiltInPalette::GrayScale))
    }

    #[test]
    fn animation_owned_frame_survives_member_removal_until_movie_clear() {
        let mut manager = BitmapManager::new();
        let id = manager.add_movie_bitmap(image());
        manager.incref_bitmap(id);
        manager.release_anchor(id);
        manager.decref_bitmap(id);
        assert!(manager.get_bitmap(id).is_some());
        manager.clear_movie_bitmaps();
        assert!(manager.get_bitmap(id).is_none());
    }

    #[test]
    fn saved_member_image_outlives_its_anchor_until_the_last_datum_drops() {
        let mut manager = BitmapManager::new();
        let id = manager.add_bitmap(image());
        manager.incref_bitmap(id);
        manager.incref_bitmap(id);
        manager.release_anchor(id);
        manager.decref_bitmap(id);
        assert!(manager.get_bitmap(id).is_some());
        manager.decref_bitmap(id);
        assert!(manager.get_bitmap(id).is_none());
    }

    #[test]
    fn dropping_a_view_keeps_the_member_alive_and_unviewed_release_frees_it() {
        let mut manager = BitmapManager::new();
        let id = manager.add_bitmap(image());
        manager.incref_bitmap(id);
        manager.decref_bitmap(id);
        assert!(manager.get_bitmap(id).is_some());
        manager.release_anchor(id);
        assert!(manager.get_bitmap(id).is_none());
    }

    #[test]
    fn replacing_pixels_preserves_the_saved_image_lifetime_and_version() {
        let mut manager = BitmapManager::new();
        let id = manager.add_bitmap(image());
        manager.incref_bitmap(id);
        let version = manager.get_bitmap(id).unwrap().version;
        manager.replace_bitmap(id, image());
        assert_eq!(manager.get_bitmap(id).unwrap().version, version + 1);
        manager.release_anchor(id);
        assert!(manager.get_bitmap(id).is_some());
        manager.decref_bitmap(id);
        assert!(manager.get_bitmap(id).is_none());
    }

    #[test]
    fn ephemeral_images_still_free_on_last_reference_and_old_ids_are_not_reused() {
        let mut manager = BitmapManager::new();
        let old = manager.add_ephemeral_bitmap(image());
        manager.incref_bitmap(old);
        manager.decref_bitmap(old);
        assert!(manager.get_bitmap(old).is_none());
        let other = manager.add_bitmap(image());
        manager.clear_movie_bitmaps();
        let new = manager.add_bitmap(image());
        manager.decref_bitmap(other);
        manager.release_anchor(old);
        assert!(new > other && other > old);
        assert!(manager.get_bitmap(new).is_some());
    }
}
