//! Users (admin only): create and remove accounts, assign roles, switch apps on or off per user,
//! set resource limits and network/MCP scopes, and review the audit log. Every change is made
//! through `UserStore`, which re-checks the actor's permission, and is written to the audit log.

use crate::safeclamp::SafeClamp;
use iyakki_sec::policy::{App as PApp, PolicyOverride, Role};

use super::widgets::{render_confirm, Form, FormEvent};
use super::users_port::{CxPort, UsersPort};
use super::{blink, App, AppCx, Flow};
use crate::accounts::AccountError;
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, DIM, ERR, MUTED, R, TEXT};

const ACCENT: [u8; 4] = rgb(255, 107, 154);
const FUEL: [u64; 4] = [20_000_000, 200_000_000, 1_000_000_000, 10_000_000_000];
const MEM: [u32; 4] = [16, 64, 256, 1024];
const DISK: [u32; 4] = [4, 64, 256, 1024];
const TOGGLE: [(char, PApp); 4] = [('1', PApp::Chat), ('2', PApp::Rag), ('3', PApp::Mcp), ('4', PApp::Cli)];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormKind {
    NewUser,
    Password(String),
    Hosts(String),
    Servers(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    List,
    Audit,
    Form(FormKind),
    ConfirmDelete(String),
}

pub struct UsersApp {
    sel: usize,
    mode: Mode,
    form: Option<Form>,
    status: Option<(String, bool)>,
}

impl Default for UsersApp {
    fn default() -> Self {
        Self::new()
    }
}

fn next_preset<T: Copy + PartialOrd>(list: &[T], cur: T) -> T {
    list.iter().copied().find(|v| *v > cur).unwrap_or(list[0])
}

impl UsersApp {
    pub fn new() -> UsersApp {
        UsersApp { sel: 0, mode: Mode::List, form: None, status: None }
    }

    pub fn mode(&self) -> &Mode {
        &self.mode
    }
    pub fn status(&self) -> Option<&(String, bool)> {
        self.status.as_ref()
    }

    fn selected(&self, port: &dyn UsersPort) -> Option<crate::accounts::UserRecord> {
        port.users().get(self.sel).cloned()
    }

    fn report(&mut self, port: &dyn UsersPort, event: &str, detail: &str, r: Result<(), AccountError>) {
        match r {
            Ok(()) => {
                port.audit(event, detail);
                self.status = Some((format!("{event}: {detail}"), true));
            }
            Err(e) => {
                port.audit(&format!("{event}.failed"), &format!("{detail}: {e}"));
                self.status = Some((e.to_string(), false));
            }
        }
    }

    fn edit_overrides(&mut self, port: &dyn UsersPort, event: &str, f: impl FnOnce(&mut PolicyOverride, &iyakki_sec::policy::Policy)) {
        let Some(u) = self.selected(port) else { return };
        let mut ov = u.overrides.clone();
        f(&mut ov, &u.policy());
        let r = port.set_overrides(&u.name, ov);
        self.report(port, event, &u.name, r);
    }

    fn open_form(&mut self, kind: FormKind, form: Form) {
        self.form = Some(form);
        self.mode = Mode::Form(kind);
    }

    fn submit_form(&mut self, port: &dyn UsersPort, kind: FormKind) {
        let Some(form) = self.form.clone() else { return };
        match kind {
            FormKind::NewUser => {
                let role = Role::ALL[form.choice_idx(2)];
                let name = form.value(0);
                let r = port.create_user(&name, &form.raw(1), role);
                let ok = r.is_ok();
                self.report(port, "user.create", &format!("{name} ({})", role.name()), r);
                if !ok {
                    if let (Some(f), Some((m, _))) = (self.form.as_mut(), self.status.clone()) {
                        f.error = Some(m);
                    }
                    return;
                }
            }
            FormKind::Password(name) => {
                if name == port.me() {
                    self.status = Some(("Use Settings → Account to change your own password.".into(), false));
                    self.mode = Mode::List;
                    return;
                }
                let r = port.set_password(&name, &form.raw(0));
                let ok = r.is_ok();
                self.report(port, "user.password", &name, r);
                if !ok {
                    if let (Some(f), Some((m, _))) = (self.form.as_mut(), self.status.clone()) {
                        f.error = Some(m);
                    }
                    return;
                }
            }
            FormKind::Hosts(_) => {
                let list: Vec<String> = form.value(0).split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
                self.edit_overrides(port, "user.net_hosts", |ov, _| ov.net_hosts = Some(list));
            }
            FormKind::Servers(_) => {
                let list: Vec<String> = form.value(0).split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
                self.edit_overrides(port, "user.mcp_servers", |ov, _| ov.mcp_servers = Some(list));
            }
        }
        self.form = None;
        self.mode = Mode::List;
    }

    fn handle_list(&mut self, k: &Key, port: &dyn UsersPort) -> Flow {
        let n = port.users().len();
        match k {
            Key::Esc => return Flow::Back,
            Key::Up => self.sel = self.sel.saturating_sub(1),
            Key::Down => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            Key::Char('n') => {
                let f = Form::new("New user").text("Username", "", "lowercase letters, digits, - and _").secret("Password (min 8)", "").choice("Role", &["admin", "operator", "user", "guest"], 2);
                self.open_form(FormKind::NewUser, f);
            }
            Key::Char('l') => self.mode = Mode::Audit,
            Key::Char('d') => {
                if let Some(u) = self.selected(port) {
                    self.mode = Mode::ConfirmDelete(u.name);
                }
            }
            Key::Char('p') => {
                if let Some(u) = self.selected(port) {
                    let f = Form::new(&format!("Reset password: {}", u.name)).secret("New password (min 8)", "their personal vault will be reset");
                    self.open_form(FormKind::Password(u.name), f);
                }
            }
            Key::Char('r') => {
                if let Some(u) = self.selected(port) {
                    let role = u.role.cycle();
                    let r = port.set_role(&u.name, role);
                    self.report(port, "user.role", &format!("{} -> {}", u.name, role.name()), r);
                }
            }
            Key::Char('h') => {
                if let Some(u) = self.selected(port) {
                    let cur = u.policy().net_hosts.join(", ");
                    let f = Form::new(&format!("Network access: {}", u.name)).text("Hosts, comma separated (* = all)", &cur, "api.openai.com, localhost");
                    self.open_form(FormKind::Hosts(u.name), f);
                }
            }
            Key::Char('s') => {
                if let Some(u) = self.selected(port) {
                    let cur = u.policy().mcp_servers.join(", ");
                    let f = Form::new(&format!("MCP servers: {}", u.name)).text("Server names, comma separated (* = all)", &cur, "files, search");
                    self.open_form(FormKind::Servers(u.name), f);
                }
            }
            Key::Char(c) if TOGGLE.iter().any(|(t, _)| t == c) => {
                let app = TOGGLE.iter().find(|(t, _)| t == c).map(|(_, a)| *a).unwrap();
                self.edit_overrides(port, &format!("user.app.{}", app.id()), |ov, eff| {
                    if eff.allows_app(app) {
                        ov.allow_apps.retain(|a| *a != app);
                        if !ov.deny_apps.contains(&app) {
                            ov.deny_apps.push(app);
                        }
                    } else {
                        ov.deny_apps.retain(|a| *a != app);
                        if !ov.allow_apps.contains(&app) {
                            ov.allow_apps.push(app);
                        }
                    }
                });
            }
            Key::Char('v') => self.edit_overrides(port, "user.voice", |ov, eff| ov.voice = Some(!eff.voice)),
            Key::Char('b') => self.edit_overrides(port, "user.budget", |ov, eff| ov.budget = Some(eff.budget.next_preset())),
            Key::Char('k') => self.edit_overrides(port, "user.skills", |ov, eff| ov.skills = Some(!eff.skills)),
            Key::Char('a') => self.edit_overrides(port, "user.agent", |ov, eff| ov.agent = Some(!eff.agent)),
            Key::Char('w') => self.edit_overrides(port, "user.browser_agent", |ov, eff| ov.browser_agent = Some(eff.browser_agent.next())),
            Key::Char('f') => self.edit_overrides(port, "user.fuel", |ov, eff| ov.max_fuel = Some(next_preset(&FUEL, eff.max_fuel))),
            Key::Char('m') => self.edit_overrides(port, "user.memory", |ov, eff| ov.max_memory_mb = Some(next_preset(&MEM, eff.max_memory_mb))),
            Key::Char('q') => self.edit_overrides(port, "user.disk", |ov, eff| ov.vfs_quota_mb = Some(next_preset(&DISK, eff.vfs_quota_mb))),
            _ => {}
        }
        Flow::Stay
    }

    fn handle_with(&mut self, ev: &Event, port: &dyn UsersPort) -> Flow {
        match self.mode.clone() {
            Mode::List => match ev {
                Event::Key(k) => return self.handle_list(k, port),
                Event::Row(i) => {
                    let n = port.users().len();
                    self.sel = (*i).min(n.saturating_sub(1));
                }
                Event::Scroll(d) => {
                    let n = port.users().len();
                    self.sel = (self.sel as i32 - d).clamp_safe(0, n.saturating_sub(1) as i32) as usize;
                }
                _ => {}
            },
            Mode::Audit => {
                // N (capital, on purpose) starts a new log when the chain reads BROKEN; the old file is kept aside, never deleted
                if matches!(ev, Event::Key(Key::Char('N'))) && port.can_manage_users() && port.audit_verify().is_err() {
                    self.status = Some(match port.audit_start_new_log() {
                        Ok(kept) => {
                            (format!("A new audit log was started; the broken one is kept as {}", kept.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()), true)
                        }
                        Err(e) => (e, false),
                    });
                }
                if matches!(ev, Event::Key(_)) {
                    self.mode = Mode::List;
                }
            }
            Mode::ConfirmDelete(name) => {
                if let Event::Key(k) = ev {
                    if *k == Key::Char('y') {
                        let r = port.delete_user(&name);
                        self.report(port, "user.delete", &name, r);
                        self.sel = self.sel.saturating_sub(1);
                    }
                    self.mode = Mode::List;
                }
            }
            Mode::Form(kind) => {
                let out = match self.form.as_mut() {
                    Some(f) => f.handle_event(ev),
                    None => FormEvent::Cancel,
                };
                match out {
                    FormEvent::Cancel => {
                        self.form = None;
                        self.mode = Mode::List;
                    }
                    FormEvent::Submit => self.submit_form(port, kind),
                    _ => {}
                }
            }
        }
        Flow::Stay
    }

    fn render_with(&mut self, g: &mut Gfx, area: R, s: f32, port: &dyn UsersPort, tick: u64) {
        let x = area.x + 24.0 * s;
        let users: Vec<_> = port.users();
        self.sel = self.sel.min(users.len().saturating_sub(1));
        if self.mode == Mode::Audit {
            g.hit(area, Event::Key(Key::Esc), "audit");
            crate::fx::card(g, R::new(x - 12.0 * s, area.y + 2.0 * s, area.w - 24.0 * s, area.h - 8.0 * s), s, ACCENT, 0.0);
            g.text(x, area.y + 12.0 * s, 16.0 * s, MUTED, &format!("Audit log — chain {}", match port.audit_verify() { Ok(n) => format!("intact ({n} entries)"), Err(i) => format!("BROKEN at entry {i}") }), Face::Sans);
            for (i, l) in port.audit_recent(((area.h - 60.0 * s) / (20.0 * s)) as usize).iter().enumerate() {
                let l = g.fit(14.0 * s, l, Face::Mono, area.w - 48.0 * s);
                g.text(x, area.y + 44.0 * s + i as f32 * 20.0 * s, 14.0 * s, TEXT, &l, Face::Mono);
            }
            return;
        }
        // a narrow window stacks the list above the details instead of side by side
        let narrow = area.w < 640.0 * s;
        let list_w = if narrow { area.w - 48.0 * s } else { (area.w * 0.36).min(380.0 * s) };
        let mut y = area.y + 8.0 * s;
        // the action buttons wrap onto more lines when they do not fit in one: lay them out first, so the body keeps clear of them
        let actions = [("New user", ACCENT, 'n'), ("Change role", TEXT, 'r'), ("Reset password", TEXT, 'p'), ("Delete", ERR, 'd'), ("Audit log", MUTED, 'l')];
        let mut action_rows = 1usize;
        {
            let mut bx = 0.0f32;
            for (label, _, _) in actions {
                let w = g.text_width(14.0 * s, label, Face::Sans) + 14.0 * s * 1.7;
                if bx > 0.0 && bx + w > area.w - 48.0 * s {
                    action_rows += 1;
                    bx = 0.0;
                }
                bx += w + 10.0 * s;
            }
        }
        let body_bottom = area.y + area.h - 96.0 * s - (action_rows - 1) as f32 * 40.0 * s;
        let (row_h, row_step) = if narrow { (38.0 * s, 44.0 * s) } else { (54.0 * s, 62.0 * s) };
        // stacked, the list shows three people at most (the one chosen always among them) so the details keep their room
        let first = if narrow { self.sel.saturating_sub(2) } else { 0 };
        for (i, u) in users.iter().enumerate().skip(first).take(if narrow { 3 } else { usize::MAX }) {
            if y > body_bottom - row_h {
                break;
            }
            let r = R::new(x, y, list_w, row_h);
            crate::fx::row_at(g, x, y, list_w, row_h, 10.0 * s, i == self.sel, g.hovered(r), ACCENT);
            g.hit(r, Event::Row(i), &u.name);
            if narrow {
                // name and role on one line
                let role = u.role.name();
                let rw = g.text_width(13.0 * s, role, Face::Sans);
                let name = g.fit(16.0 * s, &u.name, Face::Bold, (list_w - 32.0 * s - rw - 12.0 * s).max(30.0 * s));
                g.text(x + 16.0 * s, y + 9.0 * s, 16.0 * s, TEXT, &name, Face::Bold);
                g.text(x + list_w - 16.0 * s - rw, y + 11.0 * s, 13.0 * s, MUTED, role, Face::Sans);
            } else {
                g.text(x + 16.0 * s, y + 7.0 * s, 18.0 * s, TEXT, &u.name, Face::Bold);
                g.text(x + 16.0 * s, y + 31.0 * s, 13.0 * s, MUTED, u.role.name(), Face::Sans);
            }
            y += row_step;
        }
        // details of the selected user
        if let Some(u) = users.get(self.sel) {
            let p = u.policy();
            let (dx, mut dy) = if narrow { (x, y + 6.0 * s) } else { (x + list_w + 28.0 * s, area.y + 8.0 * s) };
            let dw = area.x + area.w - dx - 24.0 * s;
            crate::fx::card(g, R::new(dx - 18.0 * s, dy - 4.0 * s, dw + 36.0 * s, (body_bottom - dy).max(120.0 * s)), s, ACCENT, 0.0);
            dy += 10.0 * s;
            let head = g.fit(22.0 * s, &format!("{} · {}", u.name, u.role.name()), Face::Bold, dw);
            g.text(dx, dy, 22.0 * s, TEXT, &head, Face::Bold);
            dy += 40.0 * s;
            let apps_note = g.fit(14.0 * s, "Apps (click to switch on/off)", Face::Sans, dw);
            if dy + 20.0 * s < body_bottom {
                g.text(dx, dy, 14.0 * s, MUTED, &apps_note, Face::Sans);
            }
            dy += 24.0 * s;
            let mut ax = dx;
            let mut chips: Vec<(bool, String, char)> = TOGGLE.iter().map(|(key, app)| (p.allows_app(*app), format!("{key} {}", app.label()), *key)).collect();
            chips.extend([(p.agent, "a AGENT".to_string(), 'a'), (p.voice, "v VOICE".to_string(), 'v'), (p.skills, "k SKILLS".to_string(), 'k')]);
            for (on, label, key) in chips {
                // a chip that does not fit on the line goes to the next one
                let cw = g.text_width(14.0 * s, &label, Face::Sans) + 14.0 * s * 2.0;
                if ax > dx && ax + cw > dx + dw {
                    ax = dx;
                    dy += 42.0 * s;
                }
                if dy + 34.0 * s > body_bottom {
                    break;
                }
                let w = crate::fx::toggle_chip(g, ax, dy, s, &label, on, ACCENT);
                g.hit(R::new(ax, dy, w, 34.0 * s), Event::Key(Key::Char(key)), &label);
                ax += w + 10.0 * s;
            }
            dy += 54.0 * s;
            if dy + 20.0 * s < body_bottom {
                g.text(dx, dy - 8.0 * s, 13.0 * s, DIM, "click a line to change it", Face::Sans);
            }
            dy += 14.0 * s;
            let facts: [(String, Option<char>); 9] = [
                (format!("compute   {} instructions per run", p.max_fuel), Some('f')),
                (format!("memory    {} MiB per task", p.max_memory_mb), Some('m')),
                (format!("storage   {} MiB", p.vfs_quota_mb), Some('q')),
                (format!("budget    {}: {}", p.budget.preset_name(), p.budget.describe()), Some('b')),
                (format!("network   {}", if p.net_hosts.is_empty() { "none".into() } else { p.net_hosts.join(", ") }), Some('h')),
                (format!("browser   assistant: {}   ({})", p.browser_agent.name().replace('_', "-"), p.browser_agent.describe()), Some('w')),
                (format!("MCP       {}", if p.mcp_servers.is_empty() { "none".into() } else { p.mcp_servers.join(", ") }), Some('s')),
                (format!("files     read {}   write {}", p.read_scopes(&u.name).join(" "), p.write_scopes(&u.name).join(" ")), None),
                (format!("terminal  {:?}   power {}   swarm {}   voice {}   agent {}", p.cli, if p.power { "yes" } else { "no" }, if p.swarm { "yes" } else { "no" }, if p.voice { "yes" } else { "no" }, if p.agent { "yes" } else { "no" }), None),
            ];
            // a row is as tall as its text at this text size
            let fact_h = (g.line_height(15.0 * s, Face::Mono) + 8.0 * s).max(26.0 * s);
            for (f, key) in facts {
                // the lines that would run into the buttons are left out (the Settings pages have them all)
                if dy + fact_h > body_bottom {
                    break;
                }
                let r = R::new(dx - 6.0 * s, dy - 3.0 * s, dw + 12.0 * s, fact_h - 2.0 * s);
                if let Some(k) = key {
                    let hot = g.hovered(r);
                    if hot {
                        crate::fx::row_at(g, r.x, r.y, r.w, r.h, 8.0 * s, false, true, ACCENT);
                    }
                    g.hit(r, Event::Key(Key::Char(k)), "setting");
                }
                let f = g.fit(15.0 * s, &f, Face::Mono, dw - 16.0 * s);
                g.text(dx, dy, 15.0 * s, if key.is_some() { TEXT } else { MUTED }, &f, Face::Mono);
                dy += fact_h;
            }
        }
        // action buttons
        let mut by = area.y + area.h - 78.0 * s - (action_rows - 1) as f32 * 40.0 * s;
        let mut bx = x;
        for (label, col, key) in actions {
            let w = g.text_width(14.0 * s, label, Face::Sans) + 14.0 * s * 1.7;
            if bx > x && bx + w > x + area.w - 48.0 * s {
                bx = x;
                by += 40.0 * s;
            }
            bx += g.button(bx, by, 14.0 * s, label, col, Event::Key(Key::Char(key))) + 10.0 * s;
        }
        if let Some((m, ok)) = &self.status {
            crate::fx::status_line(g, x, area.y + area.h - 30.0 * s, area.w - 48.0 * s, s, m, *ok);
        }
        match &self.mode {
            Mode::Form(_) => {
                if let Some(f) = &self.form {
                    f.render(g, area, s, ACCENT, blink(tick));
                }
            }
            Mode::ConfirmDelete(n) => render_confirm(g, area, s, &format!("Delete user “{n}”?"), "Delete"),
            _ => {}
        }
    }
}

impl App for UsersApp {
    fn title(&self) -> &'static str {
        "Users"
    }
    fn icon(&self) -> Icon {
        Icon::Users
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        match &self.mode {
            Mode::List => "click a user, then a button · or n r d p 1-4 a v k b w f m q h s l".into(),
            Mode::Audit => "click or press any key to return · N starts a new log when the chain is BROKEN (the old file is kept)".into(),
            Mode::Form(_) => "Save · Cancel · Tab next field · Ctrl+V paste".into(),
            Mode::ConfirmDelete(_) => "y confirm · Esc cancel".into(),
        }
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        self.handle_with(ev, &CxPort(cx))
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.render_with(g, area, s, &CxPort(cx), tick)
    }
}

#[cfg(test)]
#[path = "users_fake_tests.rs"]
mod fake_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;

    fn keys(app: &mut UsersApp, cx: &AppCx, s: &str) {
        for c in s.chars() {
            app.handle(&Event::Key(Key::Char(c)), cx);
        }
    }

    #[test]
    fn create_user_via_form_and_audit() {
        let cx = cx_with_admin_store("users1");
        let mut app = UsersApp::new();
        app.handle(&Event::Key(Key::Char('n')), &cx);
        keys(&mut app, &cx, "carol");
        app.handle(&Event::Key(Key::Tab), &cx);
        keys(&mut app, &cx, "carolpass1");
        app.handle(&Event::Key(Key::Tab), &cx); // role field
        app.handle(&Event::Key(Key::Right), &cx); // user -> guest
        app.handle(&Event::Key(Key::Enter), &cx);
        assert_eq!(app.mode(), &Mode::List);
        let rec = cx.store.lock().unwrap().get("carol").cloned().unwrap();
        assert_eq!(rec.role, Role::Guest);
        assert!(cx.audit.recent(5).iter().any(|l| l.contains("user.create") && l.contains("carol")));
        // duplicate is refused and the form stays open with the reason shown
        app.handle(&Event::Key(Key::Char('n')), &cx);
        keys(&mut app, &cx, "carol");
        app.handle(&Event::Key(Key::Tab), &cx);
        keys(&mut app, &cx, "otherpass1");
        app.handle(&Event::Key(Key::Tab), &cx);
        app.handle(&Event::Key(Key::Enter), &cx);
        assert!(matches!(app.mode(), Mode::Form(_)));
        assert!(!app.status().unwrap().1);
        assert_eq!(app.form.as_ref().unwrap().error.as_deref(), Some("user already exists"));
        assert_eq!(cx.store.lock().unwrap().get("carol").unwrap().role, Role::Guest);
    }

    #[test]
    fn password_can_be_pasted_into_the_form() {
        let cx = cx_with_admin_store("users1b");
        let mut app = UsersApp::new();
        app.handle(&Event::Key(Key::Char('n')), &cx);
        app.handle(&Event::Paste("dave".into()), &cx);
        app.handle(&Event::Key(Key::Tab), &cx);
        app.handle(&Event::Paste("pasted-pass-1\n".into()), &cx);
        app.handle(&Event::Key(Key::Tab), &cx);
        app.handle(&Event::Key(Key::Enter), &cx);
        assert_eq!(app.mode(), &Mode::List);
        assert!(cx.store.lock().unwrap().authenticate("dave", "pasted-pass-1").is_ok());
    }

    #[test]
    fn toggles_limits_scopes_and_roles() {
        let cx = cx_with_admin_store("users2");
        {
            let admin = cx.session.clone();
            cx.store.lock().unwrap().create_user(&admin, "dave", "davepass12", Role::User).unwrap();
        }
        let mut app = UsersApp::new();
        app.handle(&Event::Row(1), &cx); // click dave
        // user role has MCP; toggle 3 turns it off, again turns it back on
        app.handle(&Event::Key(Key::Char('3')), &cx);
        let dave = cx.store.lock().unwrap().get("dave").cloned().unwrap();
        assert!(!dave.policy().allows_app(PApp::Mcp));
        app.handle(&Event::Key(Key::Char('3')), &cx);
        assert!(cx.store.lock().unwrap().get("dave").unwrap().policy().allows_app(PApp::Mcp));
        // memory cycles through presets (user default 64 -> 256 -> 1024 -> 16)
        app.handle(&Event::Key(Key::Char('m')), &cx);
        assert_eq!(cx.store.lock().unwrap().get("dave").unwrap().policy().max_memory_mb, 256);
        app.handle(&Event::Key(Key::Char('m')), &cx);
        app.handle(&Event::Key(Key::Char('m')), &cx);
        assert_eq!(cx.store.lock().unwrap().get("dave").unwrap().policy().max_memory_mb, 16);
        // network hosts form: select-all then type replaces the text
        app.handle(&Event::Key(Key::Char('h')), &cx);
        app.handle(&Event::Key(Key::Ctrl('a')), &cx);
        keys(&mut app, &cx, "api.example.com, b.org");
        app.handle(&Event::Key(Key::Enter), &cx);
        assert_eq!(cx.store.lock().unwrap().get("dave").unwrap().policy().net_hosts, vec!["api.example.com", "b.org"]);
        // role cycling
        app.handle(&Event::Key(Key::Char('r')), &cx);
        assert_eq!(cx.store.lock().unwrap().get("dave").unwrap().role, Role::Guest);
        let mut g = Gfx::new(1100, 650);
        g.clear(rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 80.0, 1100.0, 570.0), 1.0, &cx, 0);
    }

    #[test]
    fn delete_needs_confirmation_and_last_admin_survives() {
        let cx = cx_with_admin_store("users3");
        let mut app = UsersApp::new();
        app.handle(&Event::Key(Key::Char('d')), &cx);
        assert!(matches!(app.mode(), Mode::ConfirmDelete(_)));
        app.handle(&Event::Key(Key::Char('x')), &cx); // cancel
        assert_eq!(cx.store.lock().unwrap().list().len(), 1);
        app.handle(&Event::Key(Key::Char('d')), &cx);
        app.handle(&Event::Key(Key::Char('y')), &cx);
        assert_eq!(cx.store.lock().unwrap().list().len(), 1, "the last administrator cannot be deleted");
        assert!(app.status().unwrap().0.contains("last administrator"));
        assert!(cx.audit.recent(3).iter().any(|l| l.contains("user.delete.failed")));
    }

    #[test]
    fn non_admin_actor_is_refused_by_the_store_even_if_the_app_is_reached() {
        let mut cx = cx_with_admin_store("users4");
        cx.session.policy = iyakki_sec::policy::Role::User.template();
        let mut app = UsersApp::new();
        app.handle(&Event::Key(Key::Char('n')), &cx);
        keys(&mut app, &cx, "eve");
        app.handle(&Event::Key(Key::Tab), &cx);
        keys(&mut app, &cx, "evepass123");
        app.handle(&Event::Key(Key::Tab), &cx);
        app.handle(&Event::Key(Key::Enter), &cx);
        assert!(cx.store.lock().unwrap().get("eve").is_none());
        assert_eq!(app.status().unwrap().0, "not permitted");
    }

    #[test]
    fn n_in_the_audit_view_starts_a_new_log_only_when_the_chain_is_broken_and_keeps_the_old_file() {
        let cx = cx_with_admin_store("users5n");
        cx.audit.record("root", "login", "ok");
        let mut app = UsersApp::new();
        app.handle(&Event::Key(Key::Char('l')), &cx);
        app.handle(&Event::Key(Key::Char('N')), &cx);
        assert_eq!(app.mode(), &Mode::List);
        assert!(cx.audit.verify().is_ok() && !cx.audit.recent(5).iter().any(|l| l.contains("audit.restart")), "an intact chain is left alone");
        cx.audit.record("root", "login", "ok");
        let text = std::fs::read_to_string(cx.audit.path()).unwrap();
        std::fs::write(cx.audit.path(), text.replacen("\"login\"", "\"logout\"", 1)).unwrap();
        assert!(cx.audit.verify().is_err());
        app.handle(&Event::Key(Key::Char('l')), &cx);
        app.handle(&Event::Key(Key::Char('N')), &cx);
        assert!(cx.audit.verify().is_ok());
        assert!(app.status.as_ref().is_some_and(|(m, ok)| *ok && m.contains("audit.log.broken-")), "{:?}", app.status);
    }

    #[test]
    fn audit_view_renders_and_closes_on_click() {
        let cx = cx_with_admin_store("users5");
        cx.audit.record("root", "login", "ok");
        let mut app = UsersApp::new();
        app.handle(&Event::Key(Key::Char('l')), &cx);
        assert_eq!(app.mode(), &Mode::Audit);
        let mut g = Gfx::new(900, 600);
        g.clear(rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 80.0, 900.0, 520.0), 1.0, &cx, 0);
        assert!(g.hits.iter().any(|h| h.label == "audit" && h.ev == Event::Key(Key::Esc)));
        app.handle(&Event::Key(Key::Esc), &cx);
        assert_eq!(app.mode(), &Mode::List);
    }

    #[test]
    fn every_action_has_a_clickable_control() {
        let cx = cx_with_admin_store("users6");
        let mut app = UsersApp::new();
        let mut g = Gfx::new(1280, 650);
        g.clear(rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 72.0, 1280.0, 578.0), 1.0, &cx, 0);
        let have = |label: &str| g.hits.iter().any(|h| h.label == label);
        for want in ["New user", "Change role", "Reset password", "Delete", "Audit log", "1 CHAT", "2 RAG", "3 MCP", "4 CLI", "a AGENT", "v VOICE", "root", "setting"] {
            assert!(have(want), "missing clickable `{want}`");
        }
        // keys behind the controls
        for (label, key) in [("New user", 'n'), ("Delete", 'd'), ("Audit log", 'l'), ("3 MCP", '3')] {
            let h = g.hits.iter().find(|h| h.label == label).unwrap();
            assert_eq!(h.ev, Event::Key(Key::Char(key)), "{label}");
        }
    }

    #[test]
    fn admin_can_switch_a_users_skills_on_and_off() {
        let cx = cx_with_admin_store("users-skills");
        cx.store.lock().unwrap().create_user(&cx.session, "erin", "erinpass-12", Role::User).unwrap();
        let mut app = UsersApp::new();
        app.sel = cx.store.lock().unwrap().list().iter().position(|u| u.name == "erin").unwrap();
        assert!(cx.store.lock().unwrap().get("erin").unwrap().policy().skills);
        app.handle(&Event::Key(Key::Char('k')), &cx);
        assert!(!cx.store.lock().unwrap().get("erin").unwrap().policy().skills);
        assert!(cx.audit.recent(5).iter().any(|l| l.contains("user.skills") && l.contains("erin")));
        app.handle(&Event::Key(Key::Char('k')), &cx);
        assert!(cx.store.lock().unwrap().get("erin").unwrap().policy().skills);
    }

    #[test]
    fn admin_cycles_a_users_agent_budget() {
        let cx = cx_with_admin_store("users-budget");
        cx.store.lock().unwrap().create_user(&cx.session, "fay", "faypass-123", Role::User).unwrap();
        let mut app = UsersApp::new();
        app.sel = cx.store.lock().unwrap().list().iter().position(|u| u.name == "fay").unwrap();
        let budget = |cx: &AppCx| cx.store.lock().unwrap().get("fay").unwrap().policy().budget;
        assert_eq!(budget(&cx).preset_name(), "standard");
        app.handle(&Event::Key(Key::Char('b')), &cx);
        assert_eq!(budget(&cx).preset_name(), "large");
        app.handle(&Event::Key(Key::Char('b')), &cx);
        assert_eq!(budget(&cx).preset_name(), "unlimited");
        app.handle(&Event::Key(Key::Char('b')), &cx);
        assert_eq!(budget(&cx).preset_name(), "tiny", "wraps around");
        assert!(cx.audit.recent(5).iter().any(|l| l.contains("user.budget") && l.contains("fay")));
    }

    #[test]
    fn admin_sets_how_far_the_assistant_may_go_in_a_users_browser() {
        use iyakki_sec::policy::BrowserAgent;
        let cx = cx_with_admin_store("users-browser");
        cx.store.lock().unwrap().create_user(&cx.session, "gus", "guspass-123", Role::User).unwrap();
        let mut app = UsersApp::new();
        app.sel = cx.store.lock().unwrap().list().iter().position(|u| u.name == "gus").unwrap();
        let level = |cx: &AppCx| cx.store.lock().unwrap().get("gus").unwrap().policy().browser_agent;
        assert_eq!(level(&cx), BrowserAgent::ReadOnly, "read-only is the default");
        app.handle(&Event::Key(Key::Char('w')), &cx);
        assert_eq!(level(&cx), BrowserAgent::Interactive);
        app.handle(&Event::Key(Key::Char('w')), &cx);
        assert_eq!(level(&cx), BrowserAgent::Off);
        app.handle(&Event::Key(Key::Char('w')), &cx);
        assert_eq!(level(&cx), BrowserAgent::ReadOnly);
        assert!(cx.audit.recent(6).iter().any(|l| l.contains("user.browser_agent") && l.contains("gus")));
        // the detail pane says it
        let mut g = crate::gfx::Gfx::new(1280, 800);
        let _ = &mut g;
        let guest = cx.store.lock().unwrap().create_user(&cx.session, "hal", "halpass-123", Role::Guest);
        assert!(guest.is_ok());
        assert_eq!(cx.store.lock().unwrap().get("hal").unwrap().policy().browser_agent, BrowserAgent::Off, "off for guests");
    }
}
