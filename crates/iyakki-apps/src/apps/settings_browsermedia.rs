//! Settings -> Browser -> "Video and sound" (F-BROX-18, F-BROX-20): the tallest video the full engine is asked for (key q cycles
//! Automatic, 1080, 720, 480, 360 pixels) and whether it may use the audio and video add-on's decoders when one is installed (key y).
//! The choices are the person's own, kept with the other browser choices, and told to the engine's helper before the next page it loads
//! (`ToHelper::MediaPolicy`). Every change is audited without more than the words of the choice.

use super::*;
use crate::apps::browser_config::{self as bc, cap_words, VIDEO_CAPS};

impl SettingsApp {
    pub(super) fn render_browser_media(&mut self, g: &mut Gfx, a: R, y0: f32, s: f32, cx: &AppCx) -> f32 {
        let c = bc::load(cx);
        let need = 2.0 * 66.0 * s;
        if y0 + need > a.y + a.h - 4.0 * s {
            if y0 + 22.0 * s < a.y + a.h {
                g.text(a.x + 4.0 * s, y0, 13.0 * s, DIM, &format!("Video quality: {}. q changes it; y turns the add-on's decoders on or off ({}).", cap_words(c.video_max_height), if c.codecs { "on" } else { "off" }), Face::Sans);
            }
            return 22.0 * s;
        }
        let size = 15.0 * s;
        let btn_w = ["Change (q)", "On or off (y)"].iter().map(|l| g.text_width(size, l, Face::Bold) + size * 1.7).fold(0.0, f32::max);
        let info_w = (a.w - btn_w - 10.0 * s).max(120.0 * s);
        let mut y = y0;
        controls::info_row(g, R::new(a.x, y, info_w, 58.0 * s), s, &format!("Video quality: {}", cap_words(c.video_max_height)), "The tallest video the web engine asks a page for. A lower number keeps a slow computer smooth; Automatic lets the page choose. Applies to the next page.");
        g.button(a.x + a.w - btn_w, y + 8.0 * s, size, "Change (q)", ACCENT, Event::Key(Key::Char('q')));
        y += 66.0 * s;
        controls::info_row(g, R::new(a.x, y, info_w, 58.0 * s), s, &format!("Audio and video add-on decoders: {}", if c.codecs { "allowed" } else { "off" }), "When the administrator installed the add-on, pages may use it for H.264 video and AAC sound. Off: pages that need it are told the engine cannot play them. Applies to the next page.");
        g.button(a.x + a.w - btn_w, y + 8.0 * s, size, "On or off (y)", ACCENT, Event::Key(Key::Char('y')));
        y += 66.0 * s;
        y - y0
    }

    /// q and y of the Browser tab.
    pub(super) fn handle_browser_media(&mut self, k: &Key, cx: &AppCx) -> bool {
        match k {
            Key::Char('q') => {
                let mut c = bc::load(cx);
                let at = VIDEO_CAPS.iter().position(|h| *h == c.video_max_height).unwrap_or(0);
                c.video_max_height = VIDEO_CAPS[(at + 1) % VIDEO_CAPS.len()];
                bc::save(cx, &c);
                let what = format!("Video quality: {}.", cap_words(c.video_max_height));
                cx.audit.record(&cx.session.user, "settings.browser", &what);
                self.ok(what);
                true
            }
            Key::Char('y') => {
                let mut c = bc::load(cx);
                c.codecs = !c.codecs;
                bc::save(cx, &c);
                let what = format!("Audio and video add-on decoders: {}.", if c.codecs { "allowed" } else { "off" });
                cx.audit.record(&cx.session.user, "settings.browser", &what);
                self.ok(what);
                true
            }
            _ => false,
        }
    }
}
