//! Credentials in a text replaced by a placeholder (moved out of `iyakki_agent::bugs` for F-SPLIT-03; the agent re-exports it).

/// Credentials in the text replaced by a placeholder (keys, tokens, `password: …`, private keys, `user:pw@host`).
pub fn redact(text: &str) -> String {
    let mut t = text.to_string();
    // a few passes: the detector reports every credential of the text, but a replaced text can reveal another pattern
    for _ in 0..3 {
        match crate::detect(&t) {
            Some(f) => t = f.redacted,
            None => break,
        }
    }
    // `DB_PASSWORD=value` and `api_token: value` anywhere in a line (names the detector does not know): the value goes
    const KEYS: [&str; 8] = ["password", "passwd", "secret", "token", "apikey", "api_key", "credential", "private_key"];
    t.lines()
        .map(|l| {
            let lower = l.to_lowercase();
            for (i, c) in l.char_indices() {
                if c != '=' && c != ':' {
                    continue;
                }
                let before = &lower[..i];
                let word = before.rsplit(|ch: char| !(ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.'))).next().unwrap_or("");
                let rest = l[i + 1..].trim();
                if KEYS.iter().any(|k| word.contains(k)) && !rest.is_empty() && !rest.starts_with("[redacted]") && !rest.starts_with("[credential") {
                    return format!("{} [redacted]", &l[..=i]);
                }
            }
            l.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The text cut to `max` bytes: the head and the tail are kept, with a note of how much was cut from the middle.
pub fn cap(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let half = max / 2;
    let mut head_end = half;
    while !text.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = text.len() - half;
    while !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    format!("{}\n[... {} bytes cut from the middle ...]\n{}", &text[..head_end], tail_start - head_end, &text[tail_start..])
}

