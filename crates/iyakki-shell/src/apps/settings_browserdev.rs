//! Settings -> Browser, "Developer tools for the chat agent": the per-person switch (default off, key d) that lets the chat agent
//! read the failed requests, load timings and console messages of the pages open in the BROWSER tile. The choice is kept by
//! `iyakki_agent::devtools::DevTools`; every change is audited by the key handler in `settings.rs`.

use super::*;

impl SettingsApp {
    /// The section; returns the height it used.
    pub(super) fn render_browser_devtools(&mut self, g: &mut Gfx, a: R, y0: f32, s: f32, cx: &AppCx) -> f32 {
        let dt = &cx.services.devtools;
        let on = dt.enabled(&cx.session.user);
        let sum = dt.summary(&cx.session.user);
        let help = format!(
            "Lets the chat assistant list failed requests, how long the page took to load and the console errors of the pages you have open here, so it can tell you why a page is broken or slow. Addresses are kept without query strings or tokens; no page content, requests or answers are stored, and turning this off forgets what was collected.{}",
            if on { format!(" Now: {} failed request(s), {} console error(s) on the current page.", sum.failed, sum.console_errors) } else { String::new() }
        );
        let rh = 86.0 * s;
        // the page does not scroll: with no room, say where the key is instead of drawing outside the window
        if y0 + rh > a.y + a.h - 4.0 * s {
            if y0 + 22.0 * s < a.y + a.h {
                g.text(a.x + 4.0 * s, y0, 13.0 * s, DIM, &format!("Developer tools for the chat agent: {}. Press d to change.", if on { "on" } else { "off" }), Face::Sans);
            }
            return 22.0 * s;
        }
        controls::switch_row(g, R::new(a.x, y0, a.w, rh), s, "Developer tools for the chat agent", &help, on, true, Event::Key(Key::Char('d')), ACCENT);
        rh + 10.0 * s
    }
}
