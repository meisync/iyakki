//! Handing a saved download to the tile that suits it (F-BROX-01), for the Downloads list of the browser and for the assistant.
//!
//! [`resolve`] is the one check both use: a plain file name in the person's Downloads folder, a kind a tile opens, the file there, and
//! the account's file rules letting them read it. [`item_for`] turns the answer into the item the tile receives (`play:<path>` for MEDIA,
//! the path for EDIT and DATA). When the assistant asks, the request waits in [`queue_chat`] until the shell takes it on its next beat
//! ([`take_chat`]); the shell then keeps the screen the person is on aside, so Back returns to it, exactly as for a click in the list.

use std::sync::Mutex;

use super::browser::{open_target, BrowserApp};
use super::AppCx;

/// The tile that opens this saved file and its full path, or the sentence that says why not.
pub fn resolve(cx: &AppCx, name: &str) -> Result<(&'static str, String), String> {
    let Some((tile, _)) = open_target(name).filter(|_| !name.is_empty() && !name.contains(['/', '\\']) && !name.starts_with('.')) else {
        return Err("That file has no tile that opens it.".into());
    };
    let dir = BrowserApp::downloads_dir(cx)?;
    let path = dir.join(name);
    if !path.is_file() {
        return Err(format!("{name} is not in Downloads any more."));
    }
    cx.core.file_scope().check_read(&cx.session.policy, &cx.session.user, &path)?;
    Ok((tile, path.to_string_lossy().into_owned()))
}

/// The item the tile gets. For DATA this also tells the table tile to start on the file, so call it on the shell's own thread.
pub fn item_for(tile: &str, path: &str) -> String {
    match tile {
        "media" => format!("play:{path}"),
        "data" => {
            super::data::request_open(path, None);
            path.to_string()
        }
        _ => path.to_string(),
    }
}

static CHAT: Mutex<Option<(String, String, String)>> = Mutex::new(None);

/// The assistant asks for `path` to open in `tile` for `user` (one request waits; a newer one replaces it).
pub fn queue_chat(user: &str, tile: &str, path: &str) {
    if let Ok(mut q) = CHAT.lock() {
        *q = Some((user.to_string(), tile.to_string(), path.to_string()));
    }
}

/// The waiting request, when it is for this person.
pub fn take_chat(user: &str) -> Option<(String, String)> {
    let mut q = CHAT.lock().ok()?;
    if q.as_ref().is_some_and(|r| r.0 == user) {
        return q.take().map(|r| (r.1, r.2));
    }
    None
}

/// The saved downloads as sentences for the assistant: name, size, and the tile that opens it (or that none does).
pub fn describe(cx: &AppCx) -> Vec<String> {
    super::browser_data::load(cx)
        .downloads
        .iter()
        .filter(|d| d.status == "saved")
        .take(50)
        .map(|d| match open_target(&d.name) {
            Some((tile, how)) => format!("{} ({} bytes, {how}; tile {tile})", d.name, d.bytes),
            None => format!("{} ({} bytes, no tile opens it)", d.name, d.bytes),
        })
        .collect()
}
