//! Sites installed as app tiles (F-BROW-100) and the plain message for protected video (F-BROW-101). The record, the manifest reader, the
//! origin rules, the profile naming and the protected-video message live in the `iyakki-site-apps` crate; this file is the shell's side:
//! where the person's list is kept (`load`, `save`) and the re-exports the chat tools and the browser use.

use std::path::PathBuf;

pub use iyakki_site_apps::*;

use super::AppCx;

fn store_path(cx: &AppCx) -> PathBuf {
    cx.data_dir().join(format!("site-apps-{}.json", super::browser_config::who(cx)))
}

pub fn load(cx: &AppCx) -> Store {
    iyakki_schema::read_plain("site apps", &store_path(cx), iyakki_schema::now_secs()).unwrap_or_default()
}

pub fn save(cx: &AppCx, s: &Store) {
    if let Ok(t) = serde_json::to_string(s) {
        super::browser_config::write_checked("site apps", &store_path(cx), &t);
    }
}
