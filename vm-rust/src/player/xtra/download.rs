//! Handing a file a movie "saved to disk" to the player.
//!
//! A browser has no disk for a movie to write to. Movies that save a log, a
//! screenshot or a report (ROTL's Edit menu: Log Chat, Log Combat Chat, Take
//! Screenshot) expect the file to appear next to the projector; the web
//! equivalent is a download. These helpers build one from bytes or a data URL.

#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;

#[cfg(target_arch = "wasm32")]
use fxhash::FxHashMap;

/// The file name without any folder: Lingo builds paths with `the applicationPath`
/// and either separator, and a download takes a bare name.
pub fn base_name(path: &str) -> String {
    path.rsplit(|c| c == '/' || c == '\\').next().unwrap_or(path).trim().to_string()
}

/// Files a movie writes that are worth offering: documents and logs a player
/// would want to keep. Anything else a movie writes (settings, caches, scratch
/// data) stays in FileIO's virtual filesystem and is never downloaded.
pub fn is_document(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    ["htm", "html", "txt", "log", "csv"].iter().any(|ext| lower.ends_with(&format!(".{}", ext)))
}

pub fn mime_for(file_name: &str) -> &'static str {
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(".htm") || lower.ends_with(".html") {
        "text/html;charset=utf-8"
    } else if lower.ends_with(".csv") {
        "text/csv;charset=utf-8"
    } else if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else {
        "text/plain;charset=utf-8"
    }
}

/// Starts a download of `url` (a data: or blob: URL) under `file_name`.
#[cfg(target_arch = "wasm32")]
pub fn download_url(file_name: &str, url: &str) -> bool {
    use wasm_bindgen::JsCast;
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return false;
    };
    let (Ok(anchor), Some(body)) = (document.create_element("a"), document.body()) else {
        return false;
    };
    let _ = anchor.set_attribute("href", url);
    let _ = anchor.set_attribute("download", file_name);
    let _ = anchor.set_attribute("style", "display:none");
    if body.append_child(&anchor).is_err() {
        return false;
    }
    if let Some(element) = anchor.dyn_ref::<web_sys::HtmlElement>() {
        element.click();
    }
    anchor.remove();
    true
}

#[cfg(not(target_arch = "wasm32"))]
pub fn download_url(_file_name: &str, _url: &str) -> bool {
    false
}

/// Starts a download of `bytes` as `file_name`.
#[cfg(target_arch = "wasm32")]
pub fn download_bytes(file_name: &str, bytes: &[u8], mime: &str) -> bool {
    use wasm_bindgen::{closure::Closure, JsCast};
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&array);
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(mime);
    let Ok(blob) = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options) else {
        return false;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return false;
    };
    let started = download_url(file_name, &url);
    // The browser has started reading the blob by now; free it a little later.
    if let Some(window) = web_sys::window() {
        let to_revoke = url.clone();
        let callback = Closure::once(move || {
            let _ = web_sys::Url::revoke_object_url(&to_revoke);
        });
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(callback.as_ref().unchecked_ref(), 60_000);
        callback.forget();
    }
    started
}

#[cfg(not(target_arch = "wasm32"))]
pub fn download_bytes(_file_name: &str, _bytes: &[u8], _mime: &str) -> bool {
    false
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    /// file name -> (latest write counter, latest contents)
    static PENDING: RefCell<FxHashMap<String, (u32, Vec<u8>)>> = RefCell::new(FxHashMap::default());
}

/// A movie wrote to a file through FileIO. When it is a document or log, offer it
/// as a download, once the writing has stopped: a script may call writeString
/// several times to build one file, and a download for each would be a mess.
#[cfg(target_arch = "wasm32")]
pub fn offer_written_file(file_name: &str, data: &[u8]) {
    use wasm_bindgen::{closure::Closure, JsCast};
    let name = base_name(file_name);
    if name.is_empty() || !is_document(&name) {
        return;
    }
    let generation = PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        let next = pending.get(&name).map(|entry| entry.0 + 1).unwrap_or(1);
        pending.insert(name.clone(), (next, data.to_vec()));
        next
    });
    let Some(window) = web_sys::window() else {
        return;
    };
    let callback = Closure::once(move || {
        let entry = PENDING.with(|pending| {
            let mut pending = pending.borrow_mut();
            match pending.get(&name) {
                Some((current, _)) if *current == generation => pending.remove(&name),
                _ => None,
            }
        });
        if let Some((_, bytes)) = entry {
            download_bytes(&name, &bytes, mime_for(&name));
        }
    });
    let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(callback.as_ref().unchecked_ref(), 300);
    callback.forget();
}

#[cfg(not(target_arch = "wasm32"))]
pub fn offer_written_file(_file_name: &str, _data: &[u8]) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_lose_their_folders() {
        assert_eq!(base_name("C:\\Games\\ROTL\\LogChat123.htm"), "LogChat123.htm");
        assert_eq!(base_name("http://host/movie/LogChat123.htm"), "LogChat123.htm");
        assert_eq!(base_name("LogChat123.htm"), "LogChat123.htm");
    }

    #[test]
    fn only_documents_and_logs_are_offered() {
        for name in ["LogChat1.htm", "x.HTML", "notes.txt", "run.log", "scores.csv"] {
            assert!(is_document(name), "{}", name);
        }
        for name in ["settings.dat", "prefs", "cache.json", "image.jpg", ""] {
            assert!(!is_document(name), "{}", name);
        }
    }

    #[test]
    fn a_log_is_served_with_its_own_type() {
        assert_eq!(mime_for("LogChat1.htm"), "text/html;charset=utf-8");
        assert_eq!(mime_for("run.log"), "text/plain;charset=utf-8");
        assert_eq!(mime_for("shot.png"), "image/png");
        assert_eq!(mime_for("shot.JPG"), "image/jpeg");
    }
}
