//! No secret and no vault value leaves in a notification body. The credential detector of the chat (`credguard`) does the finding; this
//! adds the vault references and control characters, and bounds the size.

use crate::credguard;

pub const MAX_TITLE: usize = 160;
pub const MAX_BODY: usize = 2000;

fn strip_control(s: &str, keep_newlines: bool) -> String {
    s.chars().map(|c| if c.is_control() && !(keep_newlines && c == '\n') { ' ' } else { c }).collect()
}

/// The text with credentials, vault references and URL passwords replaced, control characters removed, cut to `max` characters.
pub fn redact(text: &str, max: usize, keep_newlines: bool) -> String {
    let mut t = strip_control(text, keep_newlines);
    for _ in 0..4 {
        match credguard::detect(&t) {
            Some(f) => t = f.redacted,
            None => break,
        }
    }
    // a vault reference names where a secret lives; it is not for outsiders either
    let words: Vec<String> = t
        .split(' ')
        .map(|w| {
            let bare = w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != ':' && c != '/');
            if bare.starts_with("vault:") {
                "[vault reference removed]".to_string()
            } else if let Some(i) = w.find("://") {
                // user:password@host
                match (w[i + 3..].find('@'), w[i + 3..].find('/')) {
                    (Some(a), s) if s.is_none_or(|s| a < s) && w[i + 3..i + 3 + a].contains(':') => format!("{}[credential removed]@{}", &w[..i + 3], &w[i + 4 + a..]),
                    _ => w.to_string(),
                }
            } else {
                w.to_string()
            }
        })
        .collect();
    let t = words.join(" ");
    let t = t.trim();
    if t.chars().count() > max {
        let head: String = t.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    } else {
        t.to_string()
    }
}
