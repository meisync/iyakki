//! The engine's tuning words an administrator may switch on, here the one for hardware video decoding (F-BROX-21): the `vaapi` word of
//! `<data>/web-profile/web-tuning`, which `iyakki_agent::webengine::tuning_env` turns into `IYAKKI_WEBVIEW_VAAPI=1` for the helper, which then
//! raises GStreamer's VA decoders above the software ones when a render node and the VA plug-in are there (and logs why not otherwise).
//! The file holds other words too (`cpu`, `lp:4`, `fps:20`, ...): only the `vaapi` word is read and written here, the others are kept as they
//! are. Applies the next time the engine starts.

use super::chat_port::UserData;

/// The word that turns it on.
pub const WORD: &str = "vaapi";

/// Whether the file's text has the bare `vaapi` word (`vaapi:2` or `vaapi:off` is not it: the engine ignores those too).
pub fn is_on(text: &str) -> bool {
    text.split_whitespace().take(8).any(|w| w == WORD)
}

/// The file's text with the word added or taken away; every other word stays, in its place.
pub fn with_word(text: &str, on: bool) -> String {
    let mut words: Vec<&str> = text.split_whitespace().filter(|w| *w != WORD).collect();
    if on {
        words.push(WORD);
    }
    if words.is_empty() {
        String::new()
    } else {
        format!("{}\n", words.join(" "))
    }
}

fn path(cx: &dyn UserData) -> std::path::PathBuf {
    cx.data_root().join("web-profile").join("web-tuning")
}

pub fn load(cx: &dyn UserData) -> bool {
    std::fs::read_to_string(path(cx)).map(|t| is_on(&t)).unwrap_or(false)
}

/// Switch it; only an administrator may (it is a device setting for everybody's engine). Audited.
pub fn set(cx: &dyn UserData, on: bool) -> Result<(), String> {
    if !cx.acting_policy().manage_settings {
        return Err("An administrator changes hardware video decoding: it is a setting of this computer.".into());
    }
    let p = path(cx);
    let old = std::fs::read_to_string(&p).unwrap_or_default();
    let new = with_word(&old, on);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    if new.is_empty() {
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    } else {
        std::fs::write(&p, new).map_err(|e| e.to_string())?;
    }
    cx.record_audit("settings.browser", &format!("Hardware video decoding: {}.", if on { "on" } else { "off" }));
    Ok(())
}

pub fn words(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}
