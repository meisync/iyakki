//! Settings -> Browser -> "Cameras and microphones you marked don't use": a camera or microphone hidden with Delete in the question of the
//! Browser is not offered to any page. Here it comes back: the keys 1 to 9 press **Use again** on the row. Nothing is opened or tested here.

use super::*;
use super::browser_port::BrowserSettingsPort;
use iyakki_shared::browserdev::{self, DevicePrefs};

const MAX_ROWS: usize = 9;

impl SettingsApp {
    fn hidden_device_rows(p: &dyn BrowserSettingsPort) -> (DevicePrefs, Vec<(&'static str, String, String)>) {
        let prefs = DevicePrefs::load(&crate::apps::browser_media::devprefs_path(p));
        let cams = browserdev::list_cameras(&p.agent().browserdev_sys_root());
        let mics = p.agent().voicedesk_list_mics(&p.agent().voicedesk_proc_root());
        let rows = prefs.hidden_rows(&cams, &mics);
        (prefs, rows)
    }

    /// The list of hidden devices; nothing at all (no title, no height) when none is hidden.
    pub(super) fn render_browser_devices(&mut self, g: &mut Gfx, a: R, y0: f32, s: f32, p: &dyn BrowserSettingsPort) -> f32 {
        let (_, rows) = Self::hidden_device_rows(p);
        if rows.is_empty() || y0 + 90.0 * s > a.y + a.h {
            return 0.0;
        }
        let mut y = y0;
        y += controls::section_title(g, a.x, y, s, "Cameras and microphones you marked \"don't use\"");
        for (i, (kind, _, label)) in rows.iter().take(MAX_ROWS).enumerate() {
            if y + 44.0 * s > a.y + a.h {
                break;
            }
            let bw = g.text_width(15.0 * s, "Use again (1)", Face::Bold) + 15.0 * s * 1.7;
            controls::info_row(g, R::new(a.x, y, a.w - bw - 10.0 * s, 38.0 * s), s, label, &format!("{kind}: no page can see or use it"));
            g.button(a.x + a.w - bw, y + 2.0 * s, 15.0 * s, &format!("Use again ({})", i + 1), ACCENT, Event::Key(Key::Char(char::from(b'1' + i as u8))));
            y += 44.0 * s;
        }
        y - y0 + 6.0 * s
    }

    /// The keys 1 to 9 of the Browser tab: use the n-th hidden device again.
    pub(super) fn handle_browser_devices(&mut self, k: &Key, p: &dyn BrowserSettingsPort) -> bool {
        let Key::Char(c @ '1'..='9') = k else { return false };
        let n = (*c as u8 - b'1') as usize;
        let (mut prefs, rows) = Self::hidden_device_rows(p);
        let Some((kind, id, label)) = rows.get(n).cloned() else { return false };
        if kind == "camera" {
            prefs.show_cam(&id);
        } else {
            prefs.show_mic(&id);
        }
        if let Err(e) = prefs.save(&crate::apps::browser_media::devprefs_path(p)) {
            self.fail(format!("Could not save the choice: {e}"));
            return true;
        }
        browserdev::share_all(&p.agent().voicedesk_mic_share_dir(), &prefs, &prefs.cam, &p.voice_mic());
        let what = format!("Pages can use the {kind} \"{label}\" again.");
        p.audit("settings.browser", &format!("{kind} used again"));
        self.ok(what);
        true
    }
}
