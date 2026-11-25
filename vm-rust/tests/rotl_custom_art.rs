//! ROTL custom art (player/rotl_art.rs) on the real movie: every picture of a custom-art manifest added to a native player, the way
//! the page adds them, and the same members as adding them with the name table rebuilt after each add (as the player did before).
//! Prints where the time goes. Skipped without a movie:
//!   ROTL_MOVIE=/path/to/dcrgame.dcr ROTL_ART_MANIFEST=/path/to/manifest.json ROTL_ART_BUNDLE=/path/to/bundle.bin \
//!     cargo test --release --test rotl_custom_art -- --nocapture

use std::time::{Duration, Instant};

use vm_rust::player::cast_lib::CastMemberRef;
use vm_rust::player::reserve_player_mut;
use vm_rust::player::rotl_art::{self, ArtAdd, ArtPixels};
use vm_rust::player::testing::{run_test, TestPlayer};
use vm_rust::player::testing_shared::TestHarness;

struct Entry {
    cast: String,
    name: String,
    png: Vec<u8>,
    reg_x: i32,
    reg_y: i32,
    replace: bool,
}

fn entries() -> Option<(String, Vec<Entry>)> {
    let movie = std::env::var("ROTL_MOVIE").ok()?;
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(std::env::var("ROTL_ART_MANIFEST").ok()?).ok()?).ok()?;
    let bundle = std::fs::read(std::env::var("ROTL_ART_BUNDLE").ok()?).ok()?;
    let list = manifest["members"]
        .as_array()?
        .iter()
        .filter_map(|m| {
            let at = m["at"].as_array()?;
            let (start, len) = (at[0].as_u64()? as usize, at[1].as_u64()? as usize);
            Some(Entry {
                cast: m["cast"].as_str()?.to_string(),
                name: m["name"].as_str()?.to_string(),
                png: bundle.get(start..start + len)?.to_vec(),
                reg_x: m["regX"].as_i64().unwrap_or(0) as i32,
                reg_y: m["regY"].as_i64().unwrap_or(0) as i32,
                replace: m["replace"].as_bool() == Some(true),
            })
        })
        .collect();
    Some((movie, list))
}

#[derive(Default, Debug)]
struct Times {
    decode: Duration,
    name_check: Duration,
    install: Duration,
}

/// Adds every entry; `rebuild` drops the name table before each add, as an add did before.
fn add_all(movie: &str, list: &[Entry], rebuild: bool) -> (Vec<Result<(CastMemberRef, u16, u16), String>>, Times, usize) {
    let mut out = Vec::new();
    let mut times = Times::default();
    let mut members = 0;
    run_test(async {
        let mut player = TestPlayer::new();
        player.load_movie(movie).await;
        members = reserve_player_mut(|p| p.movie.cast_manager.casts.iter().map(|c| c.members.len()).sum());
        for e in list {
            let t = Instant::now();
            let prepared = rotl_art::prepare(ArtAdd {
                cast: &e.cast,
                name: &e.name,
                pixels: ArtPixels::Encoded(&e.png),
                reg_x: e.reg_x,
                reg_y: e.reg_y,
                replace: e.replace,
            });
            times.decode += t.elapsed();
            let result = prepared.and_then(|p| {
                reserve_player_mut(|player| {
                    if rebuild {
                        player.movie.cast_manager.invalidate_member_name_cache();
                    }
                    // the name check alone (install repeats it, from the table this builds)
                    let t = Instant::now();
                    let _ = player.movie.cast_manager.find_member_ref_by_name(&e.name);
                    times.name_check += t.elapsed();
                    let t = Instant::now();
                    let r = rotl_art::install(player, p);
                    times.install += t.elapsed();
                    r
                })
            });
            out.push(result.map(|a| (a.member, a.width, a.height)));
        }
        // every added picture is found by name, as the movie looks for it
        reserve_player_mut(|player| {
            for (e, r) in list.iter().zip(&out) {
                if let Ok((member, _, _)) = r {
                    assert_eq!(player.movie.cast_manager.find_member_ref_by_name(&e.name).as_ref(), Some(member), "{}", e.name);
                }
            }
        });
    });
    (out, times, members)
}

#[test]
fn custom_art_adds_match_the_old_way_and_are_fast() {
    let Some((movie, list)) = entries() else {
        eprintln!("skipped: set ROTL_MOVIE, ROTL_ART_MANIFEST and ROTL_ART_BUNDLE");
        return;
    };
    let (old, old_t, members) = add_all(&movie, &list, true);
    let (new, new_t, _) = add_all(&movie, &list, false);
    assert_eq!(old, new, "the same members, numbers and sizes");
    let ok = new.iter().filter(|r| r.is_ok()).count();
    eprintln!("movie members before: {}; pictures: {}, added: {}", members, list.len(), ok);
    for (e, r) in list.iter().zip(&new) {
        if let Err(err) = r {
            eprintln!("  refused {}: {}", e.name, err);
        }
    }
    let per = |d: Duration| d.as_secs_f64() * 1000.0 / list.len() as f64;
    eprintln!("rebuild each add: decode {:.3} ms, name check {:.3} ms, install {:.3} ms (per picture)", per(old_t.decode), per(old_t.name_check), per(old_t.install));
    eprintln!("kept table:       decode {:.3} ms, name check {:.3} ms, install {:.3} ms (per picture)", per(new_t.decode), per(new_t.name_check), per(new_t.install));
}
