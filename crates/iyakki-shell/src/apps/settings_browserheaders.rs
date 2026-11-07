//! Settings -> Browser -> Custom headers (key H on the Browser page, Esc comes back). A list of rules, each a header name, a value,
//! a domain filter and an optional URL prefix, that can be added, edited, enabled, disabled and deleted (after asking). The value is
//! secret-like: it is typed in a hidden field, kept in the person's vault and shown only as asterisks. The rules and their refusals
//! are in `iyakki_agent::browserheaders`; where they are kept is `browser_headers.rs`.

use iyakki_agent::browserheaders::{self as bh, HeaderRule};

use super::super::browser_headers as store_of;
use super::*;

/// What the sub-page keeps between frames.
#[derive(Default)]
pub(super) struct BhPanel {
    /// The list is showing instead of the Browser page.
    pub(super) open: bool,
}

/// The forms of this page: a new rule, or the rule with this id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum BhForm {
    Edit(Option<String>),
}

const ACK: [&str; 2] = ["No", "Yes, I know"];

impl SettingsApp {
    pub(super) fn bh_open(&self) -> bool {
        self.tab() == Tab::Browser && self.bh.open
    }

    pub(super) fn bh_len(&self, cx: &AppCx) -> usize {
        store_of::store(cx).list().len()
    }

    pub(super) fn bh_buttons(&self) -> Vec<Btn> {
        vec![("Add", ACCENT, 'a'), ("Edit", TEXT, 'e'), ("Enable / Disable", TEXT, 'x'), ("Delete", ERR, 'd'), ("Back", TEXT, 'b')]
    }

    fn bh_rule(&self, cx: &AppCx) -> Option<HeaderRule> {
        let rules = store_of::store(cx).list();
        let sel = self.sel[self.tab].min(rules.len().saturating_sub(1));
        rules.into_iter().nth(sel)
    }

    /// Keys of the sub-page (and `H` on the Browser page). True when the key was used.
    pub(super) fn handle_browser_headers(&mut self, k: &Key, cx: &AppCx) -> bool {
        if !self.bh.open {
            if matches!(k, Key::Char('H')) {
                self.bh.open = true;
                self.sel[self.tab] = 0;
                return true;
            }
            return false;
        }
        let store = store_of::store(cx);
        let len = store.list().len();
        let row = self.bh_rule(cx);
        match k {
            Key::Esc | Key::Char('b') => self.bh.open = false,
            Key::Up => self.move_sel(-1, len),
            Key::Down => self.move_sel(1, len),
            Key::Char('a') => self.bh_open_form(None),
            Key::Char('e') => match row {
                Some(r) => self.bh_open_form(Some(r)),
                None => self.fail("Choose a rule first (Up/Down)."),
            },
            Key::Char('x') | Key::Enter => match row {
                Some(r) => {
                    let on = !r.enabled;
                    match store.set_enabled(&r.id, on) {
                        Ok(r) => {
                            store_of::refresh(cx);
                            let what = format!("Custom header rule \u{201C}{}\u{201D}: {}.", r.name, if on { "on" } else { "off" });
                            cx.audit.record(&cx.session.user, "settings.browser.headers", &what);
                            self.ok(if on { format!("{} is on: {} is sent to the sites in its filter.", r.name, r.header) } else { format!("{} is off: kept, but nothing is sent.", r.name) });
                        }
                        Err(e) => self.fail(e),
                    }
                }
                None => self.fail("Choose a rule first (Up/Down)."),
            },
            Key::Char('d') | Key::Delete => match row {
                Some(r) => {
                    let text = format!("Delete the rule \u{201C}{}\u{201D}? Its value is removed from your vault too. This cannot be undone.", r.name);
                    self.modal = Some(Modal::Confirm(ConfirmKind::BrowserHeaderDelete(r.id), text));
                }
                None => self.fail("Choose a rule first (Up/Down)."),
            },
            _ => {}
        }
        true
    }

    fn bh_open_form(&mut self, edit: Option<HeaderRule>) {
        let r = edit.clone().unwrap_or_default();
        let f = Form::new(if edit.is_some() { "Edit custom header" } else { "Add a custom header" })
            .text("Rule name", &r.name, "for example: Staging token")
            .text("Header name", &r.header, "X-Api-Key")
            .secret(if edit.is_some() { "Header value (empty keeps the saved one)" } else { "Header value" }, "kept in your vault, never shown")
            .text("Sites (domain filter)", &r.domains, "api.example.org, *.corp.example (a name matches only itself; *. matches subdomains)")
            .text("Only below this address (optional)", &r.prefix, "https://example.org/api")
            .text("Only when the path matches (regular expression, optional)", &r.path_regex, "^/api/v[0-9]+/   (the path and ?query are tested)")
            .choice("Cookie or Authorization: I know this sign-in goes to those sites", &ACK, usize::from(r.ack));
        self.open_form(FormKind::BrowserHeader(BhForm::Edit(edit.map(|r| r.id))), f);
    }

    pub(super) fn bh_submit(&mut self, cx: &AppCx, kind: &BhForm, form: &mut Form) -> Result<String, String> {
        let store = store_of::store(cx);
        let BhForm::Edit(id) = kind;
        let old = id.as_ref().and_then(|i| store.list().into_iter().find(|r| r.id == *i));
        if id.is_some() && old.is_none() {
            return Err("That rule is gone.".into());
        }
        let rule = HeaderRule {
            id: String::new(),
            name: form.value(0).trim().to_string(),
            header: form.value(1).trim().to_string(),
            domains: form.value(3).trim().to_string(),
            prefix: form.value(4).trim().to_string(),
            path_regex: form.value(5).trim().to_string(),
            enabled: old.as_ref().map_or(true, |o| o.enabled),
            ack: form.choice_idx(6) == 1,
        };
        let value = form.value(2);
        let saved = match (&old, value.is_empty()) {
            (Some(o), true) => store.update(&o.id, rule, None)?,
            (Some(o), false) => store.update(&o.id, rule, Some(&value))?,
            (None, _) => store.add(rule, &value)?,
        };
        store_of::refresh(cx);
        // names only: never the value
        cx.audit.record(&cx.session.user, "settings.browser.headers", &format!("Custom header rule \u{201C}{}\u{201D} saved ({})", saved.name, saved.header));
        Ok(format!("{} saved. {} is sent over https to the sites in its filter.", saved.name, saved.header))
    }

    pub(super) fn bh_delete(&mut self, cx: &AppCx, id: &str) {
        match store_of::store(cx).delete(id) {
            Ok(r) => {
                store_of::refresh(cx);
                cx.audit.record(&cx.session.user, "settings.browser.headers", &format!("Custom header rule \u{201C}{}\u{201D} deleted", r.name));
                self.ok(format!("Deleted {}.", r.name));
                let len = self.bh_len(cx);
                self.sel[self.tab] = self.sel[self.tab].min(len.saturating_sub(1));
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn render_browser_headers(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let rules = store_of::store(cx).list();
        let mut y = a.y;
        super::sshtun_page::intro(g, a, s, &mut y, &["Send a header with requests to chosen sites: a token for an internal page, a test header for a staging site. It goes only to addresses that match the filter, over https (plain http only to this computer or a private address), and never follows a redirect to another site.", "The value is kept in your vault and shown as asterisks. The full web engine does not send these headers yet; the reader and the built-in engine do."], "Send a header to chosen sites, over https. The value stays in your vault.", if rules.is_empty() { 40.0 * s } else { 74.0 * s + 60.0 * s + 16.0 * s });
        if rules.is_empty() {
            y += 6.0 * s;
            for w in g.wrap(14.0 * s, "No rules yet. Press a to add one.", Face::Sans, a.w) {
                g.text(a.x, y, 14.0 * s, DIM, &w, Face::Sans);
                y += 20.0 * s;
            }
            return;
        }
        let sel = self.sel[self.tab].min(rules.len() - 1);
        let detail_h = 74.0 * s;
        let rh = 60.0 * s;
        let room = (a.y + a.h - detail_h - y - 8.0 * s).max(rh);
        let heights = vec![rh; rules.len()];
        let first = self.win[self.tab].layout(sel, &heights, room);
        let fit = self.win[self.tab].fit();
        for (i, r) in rules.iter().enumerate().skip(first).take(fit) {
            let chips: Vec<(&str, [u8; 4])> = vec![if r.enabled { ("\u{25CF} on", OK) } else { ("\u{25CB} off", DIM) }];
            let mut sites = if r.prefix.is_empty() { r.domains.clone() } else if r.domains.is_empty() { r.prefix.clone() } else { format!("{} below {}", r.domains, r.prefix) };
            if !r.path_regex.is_empty() {
                sites = format!("{sites}, path {}", r.path_regex);
            }
            let sub = format!("{}: {} -> {}", r.header, bh::MASK, sites);
            row(g, &mut self.tip, i, a.x, y, a.w, rh - 6.0 * s, s, i == sel, &r.name, &sub, &chips);
            y += rh;
        }
        if rules.len() > fit {
            g.text(a.x + a.w - 120.0 * s, a.y + a.h - detail_h - 6.0 * s, 11.0 * s, DIM, &format!("{}-{} of {}", first + 1, (first + fit).min(rules.len()), rules.len()), Face::Sans);
        }
        let r = &rules[sel];
        let top = a.y + a.h - detail_h;
        g.rect(a.x, top - 4.0 * s, a.w, 1.0, [255, 255, 255, 24]);
        let state = if r.enabled { "On" } else { "Off (kept, nothing is sent)" };
        let lines = [
            (format!("{state}. {} goes to: {}{}.", r.header, if r.domains.is_empty() { "any site below the address" } else { r.domains.as_str() }, if r.prefix.is_empty() { String::new() } else { format!(", only below {}", r.prefix) }) + &if r.path_regex.is_empty() { String::new() } else { format!(", when the path matches {}", r.path_regex) }, TEXT),
            ("Edit with e: leave the value empty to keep the saved one.".to_string(), DIM),
        ];
        let mut ty = top + 4.0 * s;
        'draw: for (l, col) in lines {
            for w in g.wrap(12.0 * s, &l, Face::Sans, a.w) {
                if ty + 16.0 * s > a.y + a.h {
                    break 'draw;
                }
                g.text(a.x, ty, 12.0 * s, col, &w, Face::Sans);
                ty += 16.0 * s;
            }
        }
    }
}
