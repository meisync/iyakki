

/// How far the highlight of read-aloud words is moved against the heard position of the sound, ms (Settings -> Sound "Highlight
/// timing offset", -500..500, default 0). Positive lights the words later, negative earlier. The mixer reports where the sound is
/// in the speaker by its own model of the pipe, the card cannot say how much it holds, so the person tunes what is left over.
pub fn highlight_offset_ms() -> i32 {
    HIGHLIGHT_OFFSET_MS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Set by the agent when the sound settings change (see `audioout::apply_prefs`).
pub fn set_highlight_offset_ms(ms: i32) {
    HIGHLIGHT_OFFSET_MS.store(ms.clamp(-500, 500), std::sync::atomic::Ordering::Relaxed);
}

pub static HIGHLIGHT_OFFSET_MS: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
