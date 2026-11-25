//! Settings -> Browser -> "Known-bad sites" (F-BROW-98): the person's own list of dangerous sites the browser refuses to open, on the
//! page and not only through the assistant. Key I switches the list on or off, J puts a site on it, O takes a site off (or empties the
//! list with `all`). Every change goes through the same functions as the chat tools (`browser_badsites.rs`), is applied at once to the
//! guard the browser's network consults, and is audited with the site name only. Nothing is shipped and nothing is downloaded here: a
//! downloaded list is still the chat tool `browser_badsites_download`, which the person approves first.

use crate::apps::browser_badsites as bs;
use super::*;
use super::browser_port::BrowserSettingsPort;

impl SettingsApp {
    pub(super) fn render_browser_bad(&mut self, g: &mut Gfx, a: R, y0: f32, s: f32, p: &dyn BrowserSettingsPort) -> f32 {
        let on = bs::enabled(p);
        let n = bs::load_list(p).len();
        let state = format!("{} · {} site{}", if on { "on" } else { "off" }, n, if n == 1 { "" } else { "s" });
        let rh = 86.0 * s;
        if y0 + rh > a.y + a.h - 4.0 * s {
            if y0 + 22.0 * s < a.y + a.h {
                g.text(a.x + 4.0 * s, y0, 13.0 * s, DIM, &format!("Known-bad sites: {state}. I switches it, J adds a site, O removes one."), Face::Sans);
            }
            return 22.0 * s;
        }
        let help = format!("Sites on this list are never opened (and every site under them); the page says why instead. Now: {state}. J adds a site, O removes one (or all). Nothing is shipped: the list is yours.");
        controls::switch_row(g, R::new(a.x, y0, a.w, rh), s, "Known-bad sites", &help, on, true, Event::Key(Key::Char('I')), ACCENT);
        rh + 10.0 * s
    }

    /// I, J and O of the Browser tab.
    pub(super) fn handle_browser_bad(&mut self, k: &Key, p: &dyn BrowserSettingsPort) -> bool {
        match k {
            Key::Char('I') => {
                let on = !bs::enabled(p);
                bs::set_enabled(p, on);
                p.audit("settings.browser", &format!("Known-bad site list: {}.", if on { "on" } else { "off" }));
                self.ok(format!("The known-bad site list is {}.", if on { "on" } else { "off" }));
                true
            }
            Key::Char('J') => {
                self.open_form(FormKind::BrowserBadAdd, Form::new("Never open a site").text("Site", "", "for example example.org: every site under it is covered too").text("Why (optional)", "", "shown on the page that explains the refusal"));
                true
            }
            Key::Char('O') => {
                self.open_form(FormKind::BrowserBadRemove, Form::new("Take a site off the list").text("Site", "", "as listed, or all to empty the list"));
                true
            }
            _ => false,
        }
    }

    pub(super) fn browser_bad_add_submit(&mut self, p: &dyn BrowserSettingsPort, form: &Form) -> Result<String, String> {
        let site = form.value(0);
        if site.trim().is_empty() {
            return Err("Write the site's name.".into());
        }
        let h = bs::add(p, &site, form.value(1).trim())?;
        p.audit("settings.browser", &format!("Known-bad site list: added {h}"));
        Ok(format!("{h} is on the list; it and the sites under it will not open."))
    }

    pub(super) fn browser_bad_remove_submit(&mut self, p: &dyn BrowserSettingsPort, form: &Form) -> Result<String, String> {
        let site = form.value(0);
        let site = site.trim();
        if site.is_empty() {
            return Err("Write the site's name, or all.".into());
        }
        if site.eq_ignore_ascii_case("all") {
            let n = bs::clear(p);
            p.audit("settings.browser", "Known-bad site list: emptied");
            return Ok(format!("The list is empty ({n} removed)."));
        }
        if bs::remove(p, site) {
            p.audit("settings.browser", &format!("Known-bad site list: removed {site}"));
            Ok(format!("{site} is off the list."))
        } else {
            Err(format!("{site} is not on the list (a site covered only through a parent site is removed by naming the parent)."))
        }
    }
}
