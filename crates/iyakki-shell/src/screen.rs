//! The screen facts the shell reads (connectors and modes, the firmware framebuffer's signature, which graphics card to drive): the core's
//! [`ScreenFacts`] (F-ARCH-06), never `/sys` itself. The shell gives the handle it got from the core (`Shell::screen_facts`) once when it starts
//! driving a screen ([`set_facts`]); until then, and in the splash that runs before a shell exists, the machine's own facts answer.

use std::sync::{Arc, RwLock};

use iyakki_core_api::screenfacts::ScreenFacts;

static FACTS: RwLock<Option<Arc<dyn ScreenFacts>>> = RwLock::new(None);

/// The facts every screen read of the shell goes through.
pub fn facts() -> Arc<dyn ScreenFacts> {
    match FACTS.read().unwrap_or_else(|p| p.into_inner()).clone() {
        Some(f) => f,
        None => Arc::new(iyakki_core_local::LocalScreenFacts::system()),
    }
}

/// The shell's own handle (the guarded one of `cx.os`) replaces the machine's default.
pub fn set_facts(f: Arc<dyn ScreenFacts>) {
    *FACTS.write().unwrap_or_else(|p| p.into_inner()) = Some(f);
}

/// The signature of the screen behind `fb_path`: the firmware framebuffer line and the display connectors. `None` without the framebuffer's name
/// (the caller then asks the device).
pub fn signature(facts: &dyn ScreenFacts, fb_path: &str) -> Option<String> {
    facts.fb_signature(fb_path).map(|s| format!("{s} | {}", facts.signature()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_fake::FakeScreenFacts;

    #[test]
    fn the_signature_joins_the_framebuffer_line_and_the_connectors_and_is_none_without_a_name() {
        let f = FakeScreenFacts::default();
        assert_eq!(signature(&f, "/dev/fb0"), None);
        f.set_fb_signature(Some("simpledrmdrmfb 2880,1800 stride 11520 32 bpp"));
        f.connector("/dev/dri/card0", "eDP-1", true, &[(2560, 1600)]);
        assert_eq!(signature(&f, "/dev/fb0").unwrap(), "simpledrmdrmfb 2880,1800 stride 11520 32 bpp | eDP-1:2560x1600");
        f.connector("/dev/dri/card0", "DP-1", true, &[(1920, 1080)]);
        assert_ne!(signature(&f, "/dev/fb0").unwrap(), "simpledrmdrmfb 2880,1800 stride 11520 32 bpp | eDP-1:2560x1600", "a plugged-in display changes it");
    }

    #[test]
    fn the_default_facts_are_the_machines_and_a_given_handle_replaces_them() {
        // this process-wide handle is set by this one test only
        assert!(facts().fb_signature("/dev/no-such-fb").is_none());
        let fake = Arc::new(FakeScreenFacts::default());
        fake.set_fb_signature(Some("made-up"));
        set_facts(fake);
        assert_eq!(facts().fb_signature("/dev/fb0").as_deref(), Some("made-up"));
        *FACTS.write().unwrap() = None;
    }
}
