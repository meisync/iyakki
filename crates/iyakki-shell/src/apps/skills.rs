//! SKILLS: what the assistant knows how to do, in a tile of its own. A skill is a short Markdown file of instructions
//! with a name, a description and trigger words; the assistant picks the ones that fit each request. Skills can be pasted
//! from the clipboard, written in a form, loaded from a file, or installed from a signed package after reading it. Mine
//! are private; shared ones reach everyone and need the permission to manage them.

use iyakki_agent::pkg::{PkgError, Review, Trust};
use iyakki_agent::skills::{Scope, Skill};

use super::widgets::{render_confirm, Form, FormEvent};
use super::{blink, App, AppCx, Flow};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{clipboard, Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

const ACCENT: [u8; 4] = rgb(255, 170, 90);
const WARN: [u8; 4] = rgb(255, 196, 94);

enum Mode {
    Browse,
    Form(Form, FormKind),
    Confirm(Scope, String),
    /// A package that has been read and checked; the person decides.
    Review(Box<Review>),
    Keys,
}

enum FormKind {
    New,
    File,
    Trust,
}

pub struct SkillsApp {
    sel: usize,
    scroll: usize,
    /// Where new skills go.
    shared: bool,
    mode: Mode,
    status: Option<(String, bool)>,
}

impl SkillsApp {
    pub fn new(_cx: &AppCx) -> SkillsApp {
        SkillsApp { sel: 0, scroll: 0, shared: false, mode: Mode::Browse, status: None }
    }

    pub fn status_text(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.0.as_str())
    }

    fn list(cx: &AppCx) -> Vec<Skill> {
        let mut v = cx.services.skills.list(&cx.session.user);
        // mine first, then the shared ones, each by name
        v.sort_by(|a, b| (matches!(a.scope, Scope::Shared), &a.name).cmp(&(matches!(b.scope, Scope::Shared), &b.name)));
        v
    }

    pub fn names(cx: &AppCx) -> Vec<String> {
        Self::list(cx).into_iter().map(|s| s.name).collect()
    }

    fn scope(&self, cx: &AppCx) -> Scope {
        if self.shared && cx.session.policy.manage_skills {
            Scope::Shared
        } else {
            Scope::User(cx.session.user.clone())
        }
    }

    fn can_own(cx: &AppCx) -> bool {
        cx.session.policy.skills
    }

    fn can_shared(cx: &AppCx) -> bool {
        cx.session.policy.manage_skills
    }

    fn fail(&mut self, m: impl Into<String>) {
        self.status = Some((m.into(), false));
    }

    fn ok(&mut self, m: impl Into<String>) {
        self.status = Some((m.into(), true));
    }

    /// Text that came from the clipboard or a file: a package gets a review, a plain skill is added.
    fn take_text(&mut self, cx: &AppCx, text: &str, scope: Scope) {
        if text.trim_start().starts_with("IYAKKI-PKG") {
            match cx.services.packages.review(text, &scope) {
                Ok(r) => self.mode = Mode::Review(Box::new(r)),
                Err(e) => self.fail(format!("That package cannot be installed: {e}")),
            }
            return;
        }
        match cx.services.skills.add(scope.clone(), text, Self::can_shared(cx), Self::can_own(cx)) {
            Ok(s) => {
                cx.audit.record(&cx.session.user, if scope == Scope::Shared { "skill.shared.add" } else { "skill.add" }, &s.name);
                self.ok(format!("Added the skill “{}”.", s.name));
                self.select_named(cx, &s.name);
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    fn select_named(&mut self, cx: &AppCx, name: &str) {
        if let Some(i) = Self::list(cx).iter().position(|s| s.name == name) {
            self.sel = i;
            self.scroll = 0;
        }
    }

    fn install(&mut self, cx: &AppCx, review: &Review) {
        match cx.services.packages.install(review, true, Self::can_shared(cx), Self::can_own(cx)) {
            Ok(s) => {
                cx.audit.record(&cx.session.user, "skill.package.install", &format!("{} {} ({})", s.name, review.package.manifest.version, review.trust.label()));
                self.ok(format!("Installed {} {}.", s.name, review.package.manifest.version));
                self.select_named(cx, &s.name);
            }
            Err(e @ PkgError::NotNewer { .. }) => self.fail(e.to_string()),
            Err(e) => self.fail(format!("Not installed: {e}")),
        }
    }

    fn new_form(&self, cx: &AppCx) -> Form {
        let f = Form::new("New skill").text("Name", "", "lowercase-with-dashes").text("Description", "", "when should the assistant use it?").text("Triggers", "", "words that suggest it, comma separated").text("Instructions", "", "what the assistant should do");
        if Self::can_shared(cx) { f.choice("For", &["Me only", "Everyone"], usize::from(self.shared)) } else { f.choice("For", &["Me only"], 0) }
    }

    fn file_form(&self, cx: &AppCx) -> Form {
        let f = Form::new("Load a skill or package from a file").text("File path", "", "/home/you/skills/notes.md  or  a signed package (.iyp)");
        if Self::can_shared(cx) { f.choice("For", &["Me only", "Everyone"], usize::from(self.shared)) } else { f.choice("For", &["Me only"], 0) }
    }
}

fn wrap_lines(g: &mut Gfx, text: &str, size: f32, face: Face, w: f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.lines() {
        if para.trim().is_empty() {
            out.push(String::new());
        } else {
            out.extend(g.wrap(size, para, face, w));
        }
    }
    out
}

impl App for SkillsApp {
    fn title(&self) -> &'static str {
        "Skills"
    }
    fn icon(&self) -> Icon {
        Icon::Skills
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        "↑↓ choose · v paste · n new · l load a file · d delete · m mine or shared · k publishers".into()
    }

    fn help(&self) -> Vec<String> {
        [
            "Up / Down · choose a skill      PgUp / PgDn · scroll its instructions",
            "v · add what you copied: a skill (Markdown with a name and description at the top) or a signed package (.iyp)",
            "n · write a new skill in a form",
            "l · load a skill or a package from a file you may read",
            "d · delete the chosen skill (asks first)",
            "m · choose where new skills go: only for you, or for everyone (if your role may)",
            "k · the publishers whose packages are trusted (administrators can add one)",
            "A package is always shown to you first, with its instructions and who signed it. One that is not from a trusted publisher is installed only if you say so.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        if let Mode::Form(form, kind) = &mut self.mode {
            match form.handle_event(ev) {
                FormEvent::Cancel => self.mode = Mode::Browse,
                FormEvent::Submit => match kind {
                    FormKind::New => {
                        let (name, desc, trig, body) = (form.value(0), form.value(1), form.value(2), form.value(3));
                        let shared = form.choice_idx(4) == 1;
                        let md = format!("---\nname: {name}\ndescription: {desc}\n{}---\n{body}\n", if trig.is_empty() { String::new() } else { format!("triggers: {trig}\n") });
                        let scope = if shared { Scope::Shared } else { Scope::User(cx.session.user.clone()) };
                        match cx.services.skills.add(scope.clone(), &md, Self::can_shared(cx), Self::can_own(cx)) {
                            Ok(s) => {
                                cx.audit.record(&cx.session.user, if shared { "skill.shared.add" } else { "skill.add" }, &s.name);
                                self.mode = Mode::Browse;
                                self.ok(format!("Added the skill “{}”.", s.name));
                                self.select_named(cx, &s.name);
                            }
                            Err(e) => form.error = Some(e.to_string()),
                        }
                    }
                    FormKind::File => {
                        let path = form.value(0);
                        let scope = if form.choice_idx(1) == 1 { Scope::Shared } else { Scope::User(cx.session.user.clone()) };
                        let p = std::path::PathBuf::from(&path);
                        let read = iyakki_agent::scope::check_read(&cx.session.policy, &cx.session.user, &p).and_then(|_| match std::fs::metadata(&p) {
                            Ok(m) if m.is_file() && m.len() <= iyakki_agent::pkg::MAX_PACKAGE_BYTES as u64 => std::fs::read_to_string(&p).map_err(|e| e.to_string()),
                            Ok(_) => Err("not a text file under 64 KiB".to_string()),
                            Err(e) => Err(e.to_string()),
                        });
                        match read {
                            Ok(text) => {
                                self.mode = Mode::Browse;
                                self.take_text(cx, &text, scope);
                            }
                            Err(e) => form.error = Some(e),
                        }
                    }
                    FormKind::Trust => {
                        if !Self::can_shared(cx) {
                            form.error = Some("Only people who manage shared skills can trust a publisher.".into());
                        } else {
                            match cx.services.packages.trust_key(&form.value(0), &form.value(1)) {
                                Ok(()) => {
                                    cx.audit.record(&cx.session.user, "skill.publisher.trust", &form.value(1));
                                    self.mode = Mode::Keys;
                                    self.ok("Publisher trusted.");
                                }
                                Err(e) => form.error = Some(e.to_string()),
                            }
                        }
                    }
                },
                _ => {}
            }
            return Flow::Stay;
        }
        let n = Self::list(cx).len();
        match ev {
            Event::Row(i) => {
                self.sel = (*i).min(n.saturating_sub(1));
                self.scroll = 0;
                return Flow::Stay;
            }
            Event::Scroll(d) => {
                self.scroll = (self.scroll as i32 - d * 3).max(0) as usize;
                return Flow::Stay;
            }
            Event::Tab(100) => {
                return self.handle(&Event::Key(Key::Char('v')), cx);
            }
            Event::Tab(101) => {
                return self.handle(&Event::Key(Key::Char('n')), cx);
            }
            Event::Tab(102) => {
                return self.handle(&Event::Key(Key::Char('l')), cx);
            }
            Event::Tab(103) => {
                return self.handle(&Event::Key(Key::Char('d')), cx);
            }
            _ => {}
        }
        let Event::Key(k) = ev else { return Flow::Stay };
        match &self.mode {
            Mode::Confirm(scope, name) => {
                let (scope, name) = (scope.clone(), name.clone());
                if *k == Key::Char('y') {
                    match cx.services.packages.remove(&scope, &name, Self::can_shared(cx), Self::can_own(cx)) {
                        Ok(()) => {
                            cx.audit.record(&cx.session.user, "skill.delete", &name);
                            self.ok(format!("Deleted “{name}”."));
                        }
                        Err(e) => self.fail(e.to_string()),
                    }
                    self.sel = self.sel.min(Self::list(cx).len().saturating_sub(1));
                }
                self.mode = Mode::Browse;
                return Flow::Stay;
            }
            Mode::Review(r) => {
                let r = r.clone();
                match k {
                    Key::Char('y') | Key::Enter => {
                        self.mode = Mode::Browse;
                        self.install(cx, &r);
                    }
                    Key::Esc | Key::Char('n') => {
                        self.mode = Mode::Browse;
                        self.ok("Nothing was installed.");
                    }
                    _ => {}
                }
                return Flow::Stay;
            }
            Mode::Keys => {
                match k {
                    Key::Esc | Key::Char('k') => self.mode = Mode::Browse,
                    Key::Char('t') => {
                        if Self::can_shared(cx) {
                            self.mode = Mode::Form(Form::new("Trust a publisher").text("Public key (64 hex characters)", "", "from the publisher").text("Name", "", "who it is"), FormKind::Trust);
                        } else {
                            self.fail("Only people who manage shared skills can trust a publisher.");
                        }
                    }
                    _ => {}
                }
                return Flow::Stay;
            }
            _ => {}
        }
        match k {
            Key::Esc => return Flow::Back,
            Key::Up => {
                self.sel = self.sel.saturating_sub(1);
                self.scroll = 0;
            }
            Key::Down => {
                self.sel = (self.sel + 1).min(n.saturating_sub(1));
                self.scroll = 0;
            }
            Key::PageUp => self.scroll = self.scroll.saturating_sub(8),
            Key::PageDown => self.scroll += 8,
            Key::Char('v') => match clipboard::latest() {
                Some(t) => {
                    let scope = self.scope(cx);
                    self.take_text(cx, &t, scope);
                }
                None => self.fail("The clipboard has no text. Copy a skill first."),
            },
            Key::Char('n') => self.mode = Mode::Form(self.new_form(cx), FormKind::New),
            Key::Char('l') => self.mode = Mode::Form(self.file_form(cx), FormKind::File),
            Key::Char('m') => {
                if Self::can_shared(cx) {
                    self.shared = !self.shared;
                    self.ok(if self.shared { "New skills go to everyone." } else { "New skills are only for you." });
                } else {
                    self.fail("Your role keeps skills for you only.");
                }
            }
            Key::Char('d') => {
                if let Some(sk) = Self::list(cx).get(self.sel) {
                    if sk.scope == Scope::Shared && !Self::can_shared(cx) {
                        self.fail("Only operators and administrators can delete shared skills.");
                    } else {
                        self.mode = Mode::Confirm(sk.scope.clone(), sk.name.clone());
                    }
                }
            }
            Key::Char('k') => self.mode = Mode::Keys,
            _ => {}
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 10.0 * s;
        // the target and the buttons
        let target = if self.shared && Self::can_shared(cx) { "new skills: for everyone" } else { "new skills: only for you" };
        g.text(x, y + 6.0 * s, 13.0 * s, DIM, target, Face::Sans);
        let mut bx = x + w - 6.0 * s;
        for (label, ev) in [("Delete (d)", 103usize), ("Load file (l)", 102), ("New (n)", 101), ("Paste (v)", 100)] {
            let bw = g.text_width(14.0 * s, label, Face::Bold) + 28.0 * s;
            bx -= bw + 8.0 * s;
            g.button(bx, y, 14.0 * s, label, if ev == 103 { ERR } else { ACCENT }, Event::Tab(ev));
        }
        y += 42.0 * s;
        if let Some((m, ok)) = &self.status {
            crate::fx::status_line(g, x, y, w, s, m, *ok);
            y += 24.0 * s;
        }
        let list = Self::list(cx);
        let list_w = (w * 0.36).min(380.0 * s);
        let rh = 54.0 * s;
        let fit = (((area.y + area.h - y - 6.0 * s) / (rh + 6.0 * s)).floor() as usize).max(1);
        if list.is_empty() {
            crate::fx::empty_state(g, R::new(x, y, w, (area.y + area.h - y).max(200.0 * s)), s, Icon::Skills, "No skills yet", "Copy a skill and press v, press n to write one, or l to load a file or a signed package.", ACCENT);
        }
        let start = self.sel.saturating_sub(fit - 1);
        let mut ry = y;
        for (i, sk) in list.iter().enumerate().skip(start).take(fit) {
            let r = R::new(x, ry, list_w, rh);
            let sel = i == self.sel;
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, sel, g.hovered(r), ACCENT);
            g.hit(r, Event::Row(i), &sk.name);
            g.rect(x, ry + 10.0 * s, 4.0 * s, rh - 20.0 * s, if sk.scope == Scope::Shared { rgb(120, 170, 255) } else { ACCENT });
            let nm = g.fit(16.0 * s, &sk.name, Face::Bold, list_w - 100.0 * s);
            g.text(x + 16.0 * s, ry + 6.0 * s, 16.0 * s, TEXT, &nm, Face::Bold);
            let ds = g.fit(12.0 * s, &sk.description, Face::Sans, list_w - 32.0 * s);
            g.text(x + 16.0 * s, ry + 30.0 * s, 12.0 * s, MUTED, &ds, Face::Sans);
            let tag = sk.scope.label();
            let tw = g.text_width(12.0 * s, tag, Face::Bold) + 17.0 * s;
            crate::fx::chip(g, r.x + r.w - tw - 10.0 * s, ry + 6.0 * s, s, tag, if sk.scope == Scope::Shared { rgb(120, 170, 255) } else { ACCENT }, false);
            ry += rh + 6.0 * s;
        }
        // the chosen skill
        if let Some(sk) = list.get(self.sel) {
            let dx = x + list_w + 16.0 * s;
            let dw = w - list_w - 16.0 * s;
            let dh = area.y + area.h - y - 6.0 * s;
            crate::fx::panel(g, R::new(dx, y, dw, dh), s, ACCENT);
            g.text(dx + 18.0 * s, y + 12.0 * s, 20.0 * s, TEXT, &sk.name, Face::Bold);
            let pkg = cx.services.packages.list(&sk.scope).into_iter().find(|(n, _)| *n == sk.name);
            let meta = format!(
                "{}{}{}",
                sk.scope.label(),
                if sk.triggers.is_empty() { String::new() } else { format!("  ·  used when you mention: {}", sk.triggers.join(", ")) },
                pkg.as_ref().map(|(_, i)| format!("  ·  package {} ({})", i.version, i.signer.as_deref().map_or("unsigned".to_string(), |k| format!("signed {}…", &k[..12.min(k.len())])))).unwrap_or_default()
            );
            let lines = g.wrap(12.0 * s, &meta, Face::Sans, dw - 36.0 * s);
            let mut ty = y + 42.0 * s;
            for l in lines.iter().take(2) {
                g.text(dx + 18.0 * s, ty, 12.0 * s, DIM, l, Face::Sans);
                ty += 16.0 * s;
            }
            ty += 8.0 * s;
            let ds = wrap_lines(g, &sk.description, 14.0 * s, Face::Bold, dw - 36.0 * s);
            for l in ds.iter().take(3) {
                g.text(dx + 18.0 * s, ty, 14.0 * s, ACCENT, l, Face::Bold);
                ty += 20.0 * s;
            }
            ty += 8.0 * s;
            let body = wrap_lines(g, &sk.body, 14.0 * s, Face::Sans, dw - 36.0 * s);
            let rows = (((y + dh - ty - 10.0 * s) / (19.0 * s)).floor() as usize).max(1);
            let first = self.scroll.min(body.len().saturating_sub(rows));
            self.scroll = first;
            for l in body.iter().skip(first).take(rows) {
                g.text(dx + 18.0 * s, ty, 14.0 * s, TEXT, l, Face::Sans);
                ty += 19.0 * s;
            }
            if body.len() > rows {
                g.text(dx + dw - 150.0 * s, y + dh - 24.0 * s, 11.0 * s, DIM, &format!("lines {}–{} of {}  (PgUp/PgDn)", first + 1, (first + rows).min(body.len()), body.len()), Face::Sans);
            }
        }
        match &self.mode {
            Mode::Form(f, _) => f.render(g, area, s, ACCENT, blink(tick)),
            Mode::Confirm(scope, name) => render_confirm(g, area, s, &format!("Delete the {} skill “{name}”?", scope.label()), "Delete"),
            Mode::Review(r) => render_review(g, area, s, r),
            Mode::Keys => render_keys(g, area, s, cx),
            Mode::Browse => {}
        }
    }
}

/// The package, as the person is asked to approve it: who signed it, what it is, and the instructions it contains.
fn render_review(g: &mut Gfx, area: R, s: f32, r: &Review) {
    let p = crate::fx::modal(g, area, s, 760.0 * s, (area.h - 40.0 * s).min(600.0 * s), ACCENT, false);
    let (x, y, w, h) = (p.x, p.y, p.w, p.h);
    g.text(x + 26.0 * s, y + 18.0 * s, 20.0 * s, TEXT, &format!("Install “{}”?", r.package.manifest.name), Face::Bold);
    let (trust, color) = match &r.trust {
        Trust::Trusted(_) => (r.trust.label(), OK),
        Trust::UnknownSigner(_) => (format!("{}: not a publisher you trust", r.trust.label()), WARN),
        Trust::Unsigned => ("not signed: anyone could have written it".to_string(), WARN),
    };
    let t = g.fit(14.0 * s, &trust, Face::Bold, w - 52.0 * s);
    g.text(x + 26.0 * s, y + 50.0 * s, 14.0 * s, color, &t, Face::Bold);
    let m = &r.package.manifest;
    g.text(x + 26.0 * s, y + 74.0 * s, 12.0 * s, DIM, &format!("version {} · licence {} · hash {}…", m.version, m.license, m.sha256.get(..12).unwrap_or("")), Face::Sans);
    let mut ly = y + 100.0 * s;
    let mut lines = vec![r.skill.description.clone()];
    if !r.skill.triggers.is_empty() {
        lines.push(format!("Used when you mention: {}", r.skill.triggers.join(", ")));
    }
    lines.push(String::new());
    lines.extend(r.skill.body.lines().map(str::to_string));
    for para in lines {
        for l in if para.is_empty() { vec![String::new()] } else { g.wrap(14.0 * s, &para, Face::Sans, w - 52.0 * s) } {
            if ly + 18.0 * s > y + h - 70.0 * s {
                break;
            }
            g.text(x + 26.0 * s, ly, 14.0 * s, TEXT, &l, Face::Sans);
            ly += 19.0 * s;
        }
    }
    g.text(x + 26.0 * s, y + h - 60.0 * s, 12.0 * s, MUTED, "A skill steers the assistant. Read it before you let it in.", Face::Sans);
    let b1 = g.button(x + 26.0 * s, y + h - 42.0 * s, 14.0 * s, if matches!(r.trust, Trust::Trusted(_)) { "Install (y)" } else { "Install anyway (y)" }, if matches!(r.trust, Trust::Trusted(_)) { OK } else { WARN }, Event::Key(Key::Char('y')));
    g.button(x + 26.0 * s + b1 + 12.0 * s, y + h - 42.0 * s, 14.0 * s, "Cancel (Esc)", MUTED, Event::Key(Key::Esc));
}

fn render_keys(g: &mut Gfx, area: R, s: f32, cx: &AppCx) {
    let p = crate::fx::modal(g, area, s, 700.0 * s, (area.h - 80.0 * s).min(420.0 * s), ACCENT, false);
    let (x, y, w, h) = (p.x, p.y, p.w, p.h);
    g.text(x + 26.0 * s, y + 18.0 * s, 20.0 * s, TEXT, "Trusted publishers", Face::Bold);
    g.text(x + 26.0 * s, y + 48.0 * s, 13.0 * s, MUTED, "Packages signed by these keys install without a warning (you still read them first).", Face::Sans);
    let keys = cx.services.packages.trusted_keys();
    let mut ly = y + 80.0 * s;
    if keys.is_empty() {
        g.text(x + 26.0 * s, ly, 14.0 * s, DIM, "None yet.", Face::Sans);
    }
    for (k, label) in keys.iter().take(8) {
        g.text(x + 26.0 * s, ly, 15.0 * s, TEXT, label, Face::Bold);
        g.text(x + 26.0 * s, ly + 20.0 * s, 11.0 * s, DIM, &format!("{}…{}", &k[..16], &k[k.len() - 8..]), Face::Mono);
        ly += 46.0 * s;
    }
    g.button(x + 26.0 * s, y + h - 46.0 * s, 14.0 * s, "Trust a publisher (t)", ACCENT, Event::Key(Key::Char('t')));
    g.button(x + w - 140.0 * s, y + h - 46.0 * s, 14.0 * s, "Close (Esc)", MUTED, Event::Key(Key::Esc));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
    use std::sync::Arc;

    fn admin_cx(tag: &str) -> AppCx {
        let mut cx = cx_with_admin_store(tag);
        let d = temp_dir(&format!("{tag}-svc"));
        cx.services = services_in(&d, Arc::new(Offline));
        cx.services.on_login("root", "rootpass12", true);
        cx
    }

    const SKILL: &str = "---\nname: weekly-report\ndescription: write the weekly report\ntriggers: report, weekly\n---\nSummarise the week in five bullets.\n";

    fn draw(a: &mut SkillsApp, cx: &AppCx) {
        let mut g = Gfx::new(1280, 720);
        g.clear(rgb(9, 11, 20));
        a.render(&mut g, R::new(0.0, 46.0, 1280.0, 674.0), 1.0, cx, 3);
    }

    #[test]
    fn a_copied_skill_is_added_listed_audited_and_deleted_after_asking() {
        let cx = admin_cx("skl1");
        let mut a = SkillsApp::new(&cx);
        draw(&mut a, &cx);
        clipboard::clear();
        a.handle(&Event::Key(Key::Char('v')), &cx);
        assert!(a.status_text().unwrap().contains("clipboard"));
        clipboard::copy(SKILL);
        a.handle(&Event::Key(Key::Char('v')), &cx);
        assert!(SkillsApp::names(&cx).contains(&"weekly-report".to_string()), "{:?}", a.status_text());
        draw(&mut a, &cx);
        // delete asks first; anything but y keeps it
        a.handle(&Event::Key(Key::Char('d')), &cx);
        draw(&mut a, &cx);
        a.handle(&Event::Key(Key::Char('x')), &cx);
        assert!(SkillsApp::names(&cx).contains(&"weekly-report".to_string()));
        a.handle(&Event::Key(Key::Char('d')), &cx);
        a.handle(&Event::Key(Key::Char('y')), &cx);
        assert!(!SkillsApp::names(&cx).contains(&"weekly-report".to_string()), "{:?}", a.status_text());
        assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
    }

    #[test]
    fn the_new_form_and_a_bad_file_report_problems_and_every_dialog_draws() {
        let cx = admin_cx("skl2");
        let mut a = SkillsApp::new(&cx);
        a.handle(&Event::Key(Key::Char('n')), &cx);
        draw(&mut a, &cx);
        a.handle(&Event::Key(Key::Esc), &cx);
        a.handle(&Event::Key(Key::Char('l')), &cx);
        for c in "/definitely/not/here.md".chars() {
            a.handle(&Event::Key(Key::Char(c)), &cx);
        }
        a.handle(&Event::Key(Key::Enter), &cx);
        draw(&mut a, &cx);
        a.handle(&Event::Key(Key::Esc), &cx);
        a.handle(&Event::Key(Key::Char('k')), &cx);
        draw(&mut a, &cx);
        a.handle(&Event::Key(Key::Esc), &cx);
        a.handle(&Event::Key(Key::Char('m')), &cx);
        assert!(a.status_text().unwrap().contains("everyone"));
        draw(&mut a, &cx);
        assert!(!a.help().is_empty());
    }
}
