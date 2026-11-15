//! Settings -> Browser -> "Sites you allowed or blocked": the sites the person said Always allow or Block to for the camera, the
//! microphone or the screen share, and a way to take one back (key `F`, or the button): the browser asks again the next time, and the
//! device chosen for that site is forgotten. Nothing here can allow anything. The same list and the same forgetting are chat tools
//! (`browser_media_sites`, `browser_media_site_forget`, which asks first).

use super::*;

const MAX_ROWS: usize = 8;

impl SettingsApp {
    /// One row about the audio and video add-on (H.264, AAC): what it is on this computer, and where to install it. Read only here.
    pub(super) fn render_browser_addon(&mut self, g: &mut Gfx, a: R, y0: f32, s: f32, cx: &AppCx) -> f32 {
        let (title, help) = iyakki_agent::avaddon::settings_text(&iyakki_agent::avaddon::detect(cx.services.dir()));
        let mut y = y0;
        y += controls::section_title(g, a.x, y, s, "Audio and video add-on");
        controls::info_row(g, R::new(a.x, y, a.w, 56.0 * s), s, &title, &help);
        y + 62.0 * s - y0
    }

    /// The list of remembered sites; nothing at all (no title, no height) when none is remembered.
    pub(super) fn render_browser_sites(&mut self, g: &mut Gfx, a: R, y0: f32, s: f32, cx: &AppCx) -> f32 {
        let perms = crate::apps::browser_media::load(cx);
        let sites = perms.list();
        if sites.is_empty() || y0 + 90.0 * s > a.y + a.h {
            return 0.0;
        }
        let mut y = y0;
        y += controls::section_title(g, a.x, y, s, "Sites you allowed or blocked for camera, microphone and screen");
        let bw = g.text_width(15.0 * s, "Forget a site (F)", Face::Bold) + 15.0 * s * 1.7;
        for (i, (host, what)) in sites.iter().take(MAX_ROWS).enumerate() {
            let w = if i == 0 { a.w - bw - 10.0 * s } else { a.w };
            let note = perms.device_note(host);
            controls::info_row(g, R::new(a.x, y, w, 38.0 * s), s, host, &if note.is_empty() { what.clone() } else { format!("{what} · {note}") });
            if i == 0 {
                g.button(a.x + a.w - bw, y + 2.0 * s, 15.0 * s, "Forget a site (F)", ACCENT, Event::Key(Key::Char('F')));
            }
            y += 44.0 * s;
        }
        if sites.len() > MAX_ROWS {
            controls::info_row(g, R::new(a.x, y, a.w, 30.0 * s), s, &format!("and {} more", sites.len() - MAX_ROWS), "F forgets one by its name; the assistant lists all of them.");
            y += 36.0 * s;
        }
        y - y0 + 6.0 * s
    }

    /// `F`: the form to forget one site. True when the key was used.
    pub(super) fn handle_browser_sites(&mut self, k: &Key, cx: &AppCx) -> bool {
        if !matches!(k, Key::Char('F')) {
            return false;
        }
        let sites = crate::apps::browser_media::load(cx).list();
        let Some((first, _)) = sites.first() else {
            self.ok("No site is remembered: every request for the camera, the microphone or the screen asks.".to_string());
            return true;
        };
        let pre = if sites.len() == 1 { first.as_str() } else { "" };
        self.open_form(FormKind::BrowserSite, Form::new("Forget a site").text("Site (as listed)", pre, "for example meet.example.org"));
        true
    }

    pub(super) fn site_forget_submit(&mut self, cx: &AppCx, form: &Form) -> Result<String, String> {
        let host = form.value(0).trim().to_ascii_lowercase();
        let mut m = crate::apps::browser_media::load(cx);
        if !m.list().iter().any(|(h, _)| *h == host) {
            return Err(format!("{host} is not in the list."));
        }
        m.revoke(&host);
        crate::apps::browser_media::save(cx, &m);
        let what = format!("The browser will ask again before {host} uses the camera, the microphone or the screen.");
        cx.audit.record(&cx.session.user, "settings.browser", &format!("{host} forgotten for camera, microphone and screen"));
        Ok(what)
    }
}
