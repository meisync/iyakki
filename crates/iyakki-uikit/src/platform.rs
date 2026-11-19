//! Words that differ by platform. On a Mac the desktop window pastes with Cmd+V (the system sends the paste; Ctrl+V does nothing there), so a
//! hint that says "Ctrl+V pastes" would be wrong. The names of the keys are decided here, once.

/// The key that pastes on this platform, for a hint.
pub fn paste_key() -> &'static str {
    paste_key_for(cfg!(target_os = "macos"))
}

pub fn paste_key_for(mac: bool) -> &'static str {
    if mac {
        "Cmd+V"
    } else {
        "Ctrl+V"
    }
}

/// A hint written with "Ctrl+V" says the key of this platform instead.
pub fn keys(text: &str) -> String {
    keys_for(text, cfg!(target_os = "macos"))
}

pub fn keys_for(text: &str, mac: bool) -> String {
    if mac {
        text.replace("Ctrl+V", paste_key_for(true))
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mac_is_told_cmd_v_and_everything_else_ctrl_v() {
        assert_eq!(paste_key_for(true), "Cmd+V");
        assert_eq!(paste_key_for(false), "Ctrl+V");
        assert_eq!(keys_for("Click a field or press Tab · Ctrl+V pastes", true), "Click a field or press Tab · Cmd+V pastes");
        assert_eq!(keys_for("Click a field or press Tab · Ctrl+V pastes", false), "Click a field or press Tab · Ctrl+V pastes");
        assert_eq!(keys_for("Tab/click to move · Ctrl+V paste · Ctrl+Shift+V history", true), "Tab/click to move · Cmd+V paste · Ctrl+Shift+V history", "other shortcuts are left alone");
        assert_eq!(paste_key(), if cfg!(target_os = "macos") { "Cmd+V" } else { "Ctrl+V" });
    }
}
