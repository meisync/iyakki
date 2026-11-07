//! Settings -> Cloud access: the roles manager of a cloud tenant (docs/development/CLOUD-MULTITENANT-PLAN.md, section 3a).
//! It shows the signed-in account's tenant role, the tenant members, the namespaces this account may see (with their members where it
//! is one of them) and the tail of the audit trail, all from the lists on this machine (`iyakki_agent::cloudroles`). Every change goes
//! through the `Roles` change functions, which decide who may do what; their refusals are shown as they are. The page is for
//! administrators of the machine. Removing a person asks first. The page does not rotate keys: it says they are to be rotated.

use super::*;
use iyakki_agent::cloudroles::{self, Change, NsRole, Roles, TenantRole};
use std::time::{Duration, Instant};

const FIELD_ID: usize = 932;
const ROW_H: f32 = 40.0;
const HEAD_H: f32 = 24.0;
const REREAD: Duration = Duration::from_millis(1500);

/// What a typed name is for.
enum Ask {
    TenantMember,
    Namespace,
    NsMember(String),
}

enum Sure {
    RemoveTenant(String),
    RemoveNs(String, String),
    DeleteNs(String),
}

/// One selectable row of the page.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Item {
    Tenant(String, TenantRole),
    /// A namespace this account sees, with its role there (`None`: the name only, not joined).
    Ns(String, Option<NsRole>),
    NsMember(String, String, NsRole),
}

type Loaded = Result<Option<Roles>, String>;

#[derive(Default)]
pub(super) struct RolesUi {
    ask: Option<(Ask, TextField)>,
    sure: Option<Sure>,
    cache: Option<(Instant, Loaded)>,
}

fn word(r: TenantRole) -> &'static str {
    match r {
        TenantRole::Owner => "owner",
        TenantRole::Admin => "admin",
        TenantRole::Member => "member",
    }
}

fn ns_word(r: NsRole) -> &'static str {
    match r {
        NsRole::Admin => "admin",
        NsRole::Editor => "editor",
        NsRole::Viewer => "viewer",
    }
}

/// The rows an account is shown. Tenant members are listed to owners and administrators; namespace members only of a namespace the
/// account is in (the others show their name, so people can be added, and nothing more).
pub(super) fn items(r: &Roles, me: &str) -> Vec<Item> {
    let mut v = Vec::new();
    let Some(mine) = r.tenant_role(me) else { return v };
    if mine >= TenantRole::Admin {
        let mut people: Vec<_> = r.tenant.iter().collect();
        people.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        v.extend(people.into_iter().map(|(n, role)| Item::Tenant(n.clone(), *role)));
    }
    for (ns, role) in r.visible_namespaces(me) {
        v.push(Item::Ns(ns.clone(), role));
        if role.is_some() {
            let mut people: Vec<_> = r.namespaces.get(&ns).into_iter().flatten().collect();
            people.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            v.extend(people.into_iter().map(|(n, role)| Item::NsMember(ns.clone(), n.clone(), *role)));
        }
    }
    v
}

fn section(i: &Item) -> u8 {
    matches!(i, Item::Tenant(..)) as u8 ^ 1
}

impl SettingsApp {
    fn roles_file(cx: &AppCx) -> std::path::PathBuf {
        cloudroles::path(Self::svc(cx).dir())
    }

    fn roles_fresh(&mut self, cx: &AppCx) -> Loaded {
        let l = cloudroles::load(&Self::roles_file(cx));
        self.roles.cache = Some((Instant::now(), l.clone()));
        l
    }

    fn roles_cached(&mut self, cx: &AppCx) -> Loaded {
        match &self.roles.cache {
            Some((at, l)) if at.elapsed() < REREAD => l.clone(),
            _ => self.roles_fresh(cx),
        }
    }

    pub(super) fn roles_len(&mut self, cx: &AppCx) -> usize {
        match self.roles_cached(cx) {
            Ok(Some(r)) => items(&r, &cx.session.user).len(),
            _ => 0,
        }
    }

    pub(super) fn roles_open(&self) -> bool {
        self.tab() == Tab::CloudRoles && (self.roles.ask.is_some() || self.roles.sure.is_some())
    }

    pub(super) fn roles_buttons(&self) -> Vec<Btn> {
        if self.roles.sure.is_some() {
            return vec![("Yes, remove", ERR, 'y'), ("Cancel", MUTED, '\u{1b}')];
        }
        if self.roles.ask.is_some() {
            return vec![("Add", ACCENT, '\n'), ("Cancel", MUTED, '\u{1b}')];
        }
        match &self.roles.cache {
            Some((_, Ok(None))) => vec![("Make me the owner", ACCENT, 'o')],
            _ => vec![("Add person", ACCENT, 'm'), ("New namespace", TEXT, 'n'), ("Add to namespace", TEXT, 'a'), ("Join", TEXT, 'j'), ("Remove person", ERR, 'x'), ("Delete namespace", ERR, 'd')],
        }
    }

    /// Keys and paste while a name is being typed or a removal waits for its answer. True when the event was taken.
    pub(super) fn roles_event(&mut self, ev: &Event, cx: &AppCx) -> bool {
        if !self.roles_open() {
            return false;
        }
        // the buttons send their key as a character
        let ev = &match ev {
            Event::Key(Key::Char('\n')) => Event::Key(Key::Enter),
            Event::Key(Key::Char('\u{1b}')) => Event::Key(Key::Esc),
            e => e.clone(),
        };
        if self.roles.sure.is_some() {
            match ev {
                Event::Key(Key::Char('y')) | Event::Key(Key::Char('Y')) => {
                    if let Some(s) = self.roles.sure.take() {
                        self.roles_confirmed(s, cx);
                    }
                }
                Event::Key(_) => {
                    self.roles.sure = None;
                    self.ok("Nothing was removed.");
                }
                _ => return false,
            }
            return true;
        }
        match ev {
            Event::Key(Key::Esc) => {
                self.roles.ask = None;
            }
            Event::Key(Key::Enter) => {
                if let Some((ask, f)) = self.roles.ask.take() {
                    self.roles_submit(ask, f.value.trim().to_string(), cx);
                }
            }
            Event::Key(k) => {
                if let Some((_, f)) = self.roles.ask.as_mut() {
                    f.handle(k);
                }
            }
            Event::Paste(t) => {
                if let Some((_, f)) = self.roles.ask.as_mut() {
                    f.paste(t);
                }
            }
            Event::FocusAt(id, x) if *id == FIELD_ID => {
                if let Some((_, f)) = self.roles.ask.as_mut() {
                    f.click(*x);
                }
            }
            _ => return false,
        }
        true
    }

    /// Run a change as the signed-in account: the model checks who may, the lists are saved, the trail and the app audit log get a line.
    fn roles_apply(&mut self, cx: &AppCx, f: impl FnOnce(&mut Roles, &str) -> Result<Change, String>) {
        let me = cx.session.user.clone();
        let mut r = match self.roles_fresh(cx) {
            Ok(Some(r)) => r,
            Ok(None) => return self.fail("This machine has no roles yet: choose Make me the owner first."),
            Err(e) => return self.fail(e),
        };
        match f(&mut r, &me) {
            Err(e) => {
                cx.audit.record(&me, "settings.cloud_roles.refused", &e);
                self.fail(format!("Refused: {e}."));
            }
            Ok(c) => {
                let file = Self::roles_file(cx);
                let now = iyakki_agent::clock::now_unix().max(0) as u64;
                if let Err(e) = cloudroles::save(&file, &r) {
                    return self.fail(e);
                }
                let trail = cloudroles::append_audit(&cloudroles::audit_path(Self::svc(cx).dir()), now, &c);
                cx.audit.record(&me, "settings.cloud_roles", &c.0);
                self.roles.cache = Some((Instant::now(), Ok(Some(r))));
                match trail {
                    Ok(()) => self.ok(c.0),
                    Err(e) => self.fail(format!("Saved, but the audit trail could not be written: {e}")),
                }
            }
        }
    }

    fn roles_submit(&mut self, ask: Ask, text: String, cx: &AppCx) {
        let mut words = text.split_whitespace();
        let name = words.next().unwrap_or("").to_string();
        let role_word = words.next().map(|w| w.to_lowercase());
        if name.is_empty() || words.next().is_some() {
            self.roles.ask = Some((ask, TextField::new("", false)));
            if let Some((_, f)) = self.roles.ask.as_mut() {
                f.set(&text);
            }
            return self.fail("Type one name, and for a person optionally a role after it.");
        }
        match ask {
            Ask::Namespace => self.roles_apply(cx, |r, me| r.create_namespace(me, &name)),
            Ask::TenantMember => {
                let role = match role_word.as_deref() {
                    None | Some("member") => TenantRole::Member,
                    Some("admin") => TenantRole::Admin,
                    Some("owner") => TenantRole::Owner,
                    Some(w) => return self.fail(format!("\"{w}\" is not a role: owner, admin or member.")),
                };
                self.roles_apply(cx, |r, me| r.add_tenant_member(me, &name, role));
            }
            Ask::NsMember(ns) => {
                let role = match role_word.as_deref() {
                    None | Some("editor") => NsRole::Editor,
                    Some("admin") => NsRole::Admin,
                    Some("viewer") => NsRole::Viewer,
                    Some(w) => return self.fail(format!("\"{w}\" is not a role: admin, editor or viewer.")),
                };
                self.roles_apply(cx, |r, me| r.add_namespace_member(me, &ns, &name, role));
            }
        }
    }

    fn roles_confirmed(&mut self, s: Sure, cx: &AppCx) {
        match s {
            Sure::RemoveTenant(who) => self.roles_apply(cx, |r, me| r.remove_tenant_member(me, &who)),
            Sure::RemoveNs(ns, who) => self.roles_apply(cx, |r, me| r.remove_namespace_member(me, &ns, &who)),
            Sure::DeleteNs(ns) => self.roles_apply(cx, |r, me| r.delete_namespace(me, &ns)),
        }
        let len = self.roles_len(cx);
        let i = &mut self.sel[self.tab];
        *i = (*i).min(len.saturating_sub(1));
    }

    pub(super) fn handle_cloudroles(&mut self, k: &Key, cx: &AppCx) {
        let len = self.roles_len(cx);
        match k {
            Key::Up => return self.move_sel(-1, len),
            Key::Down => return self.move_sel(1, len),
            Key::Home => return self.move_sel(-(len as i32), len),
            Key::End => return self.move_sel(len as i32, len),
            Key::Char('m' | 'n' | 'a' | 'j' | 'x' | 'd' | 'o') => {}
            _ => return,
        }
        if !self.admin(cx) {
            return;
        }
        let me = cx.session.user.clone();
        let loaded = self.roles_fresh(cx);
        let Ok(roles) = loaded else { return self.fail("The roles file cannot be read.") };
        let Some(roles) = roles else {
            if matches!(k, Key::Char('o')) {
                // the first owner of this machine's tenant: once, while there is no file
                match Roles::new(&me) {
                    Ok(r) => {
                        let c = Change(format!("tenant: {me} made themselves the first owner of this machine's tenant"));
                        let file = Self::roles_file(cx);
                        if let Err(e) = cloudroles::save(&file, &r) {
                            return self.fail(e);
                        }
                        let now = iyakki_agent::clock::now_unix().max(0) as u64;
                        let _ = cloudroles::append_audit(&cloudroles::audit_path(Self::svc(cx).dir()), now, &c);
                        cx.audit.record(&me, "settings.cloud_roles", &c.0);
                        self.roles.cache = Some((Instant::now(), Ok(Some(r))));
                        self.ok(format!("{me} is the owner of this machine's tenant."));
                    }
                    Err(e) => self.fail(e),
                }
            } else {
                self.fail("This machine has no roles yet: choose Make me the owner first.");
            }
            return;
        };
        if matches!(k, Key::Char('o')) {
            return self.fail("The roles exist already; the owner cannot be made again.");
        }
        let sel = items(&roles, &me).get(self.sel()).cloned();
        let ns_of = |s: &Option<Item>| match s {
            Some(Item::Ns(n, _)) | Some(Item::NsMember(n, ..)) => Some(n.clone()),
            _ => None,
        };
        match k {
            Key::Char('m') => self.roles.ask = Some((Ask::TenantMember, TextField::new("name [owner|admin|member]", false))),
            Key::Char('n') => self.roles.ask = Some((Ask::Namespace, TextField::new("namespace name", false))),
            Key::Char('a') => match ns_of(&sel) {
                Some(ns) => self.roles.ask = Some((Ask::NsMember(ns), TextField::new("name [admin|editor|viewer]", false))),
                None => self.fail("Choose a namespace first."),
            },
            Key::Char('j') => match ns_of(&sel) {
                Some(ns) => self.roles_apply(cx, |r, me| r.add_namespace_member(me, &ns, me, NsRole::Editor)),
                None => self.fail("Choose a namespace first."),
            },
            Key::Char('x') => match sel {
                Some(Item::Tenant(who, _)) => self.roles.sure = Some(Sure::RemoveTenant(who)),
                Some(Item::NsMember(ns, who, _)) => self.roles.sure = Some(Sure::RemoveNs(ns, who)),
                _ => self.fail("Choose a person to remove."),
            },
            Key::Char('d') => match sel {
                Some(Item::Ns(ns, _)) => self.roles.sure = Some(Sure::DeleteNs(ns)),
                _ => self.fail("Choose a namespace to delete."),
            },
            _ => {}
        }
    }

    /// The question before a removal. It does not claim that keys were rotated: this page only changes the lists.
    fn roles_question(s: &Sure) -> String {
        match s {
            Sure::RemoveTenant(who) => format!("Remove {who} from the tenant? They leave every namespace too. Their keys are to be rotated and their sessions ended; this page only changes the lists, the vault service does the rotation."),
            Sure::RemoveNs(ns, who) => format!("Remove {who} from namespace {ns}? The namespace's key is to be rotated and their sessions there ended; this page only changes the lists, the vault service does the rotation."),
            Sure::DeleteNs(ns) => format!("Delete namespace {ns}? Its member list goes with it."),
        }
    }

    pub(super) fn render_cloudroles(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let me = cx.session.user.clone();
        let loaded = self.roles_cached(cx);
        let mut y = a.y;
        let line = |g: &mut Gfx, y: &mut f32, size: f32, col: [u8; 4], face: Face, t: &str| {
            let t = g.fit(size * s, t, face, a.w);
            g.text(a.x, *y, size * s, col, &t, face);
            *y += (size + 6.0) * s;
        };
        let roles = match loaded {
            Err(e) => return line(g, &mut y, 14.0, ERR, Face::Sans, &e),
            Ok(None) => {
                line(g, &mut y, 15.0, TEXT, Face::Bold, "No roles on this machine yet.");
                line(g, &mut y, 13.0, MUTED, Face::Sans, &format!("Make me the owner makes {me} the first owner of this machine's tenant (once)."));
                line(g, &mut y, 13.0, MUTED, Face::Sans, "Owners then add people and namespaces here; roles are checked by the server, not by this page.");
                return;
            }
            Ok(Some(r)) => r,
        };
        let Some(mine) = roles.tenant_role(&me) else {
            line(g, &mut y, 15.0, TEXT, Face::Bold, &format!("{me} is not a member of this tenant."));
            line(g, &mut y, 13.0, MUTED, Face::Sans, "No access: the lists are not shown to someone outside the tenant. An owner or administrator adds you.");
            return;
        };
        line(g, &mut y, 15.0, TEXT, Face::Bold, &format!("{me}: {} of this tenant", word(mine)));
        let help = if mine >= TenantRole::Admin { "Owners and administrators add people and namespaces; only a namespace's administrators add people to it." } else { "You see the namespaces you are in. Only owners and administrators see the member list." };
        line(g, &mut y, 13.0, MUTED, Face::Sans, help);
        // which cryptography keeps the shared vault, and that it is not externally reviewed (F-CLOUD-15)
        line(g, &mut y, 12.0, MUTED, Face::Sans, &iyakki_agent::sharedvault::page_line(Self::svc(cx).dir()));
        y += 4.0 * s;

        // the question or the name being typed
        if let Some(q) = &self.roles.sure {
            let text = Self::roles_question(q);
            let lines = g.wrap(13.0 * s, &text, Face::Sans, a.w - 24.0 * s);
            let n = lines.len().min(5);
            let h = n as f32 * 18.0 * s + 16.0 * s;
            g.rrect(a.x, y, a.w, h, 8.0 * s, crate::fx::WELL);
            g.rrect_outline(a.x, y, a.w, h, 8.0 * s, 1.5 * s, ERR);
            for (i, l) in lines.iter().take(n).enumerate() {
                let l = g.fit(13.0 * s, l, Face::Sans, a.w - 24.0 * s);
                g.text(a.x + 12.0 * s, y + 8.0 * s + i as f32 * 18.0 * s, 13.0 * s, TEXT, &l, Face::Sans);
            }
            y += h + 8.0 * s;
        } else if let Some((ask, f)) = &self.roles.ask {
            let what = match ask {
                Ask::TenantMember => "Add to the tenant (name, then a role):".to_string(),
                Ask::Namespace => "New namespace (a name):".to_string(),
                Ask::NsMember(ns) => format!("Add to namespace {ns} (name, then a role):"),
            };
            line(g, &mut y, 13.0, MUTED, Face::Sans, &what);
            f.render(g, R::new(a.x, y, a.w.min(520.0 * s), 34.0 * s), true, s, blink(self.ticks), Face::Sans, FIELD_ID);
            y += 42.0 * s;
        }

        // the audit trail tail takes what is left at the bottom, up to five lines
        let trail = std::fs::read_to_string(cloudroles::audit_path(Self::svc(cx).dir())).unwrap_or_default();
        let trail: Vec<&str> = trail.lines().collect();
        let free = a.y + a.h - y;
        let n_trail = (((free - 150.0 * s) / (17.0 * s)).max(0.0) as usize).min(5).min(trail.len());
        let trail_h = if n_trail > 0 { 22.0 * s + n_trail as f32 * 17.0 * s } else { 0.0 };

        let list = items(&roles, &me);
        let sel = self.sel().min(list.len().saturating_sub(1));
        let avail = (a.y + a.h - y - trail_h).max(0.0);
        // the first row so that the chosen one is inside
        let rows_h = |from: usize, to: usize| -> f32 {
            (from..=to).map(|i| ROW_H * s + if i == from || section(&list[i]) != section(&list[i - 1]) { HEAD_H * s } else { 0.0 }).sum()
        };
        let mut start = 0;
        while !list.is_empty() && start < sel && rows_h(start, sel) > avail {
            start += 1;
        }
        let mut prev = None;
        if list.is_empty() {
            line(g, &mut y, 13.0, MUTED, Face::Sans, if mine >= TenantRole::Admin { "Nothing here yet: add a person or a namespace." } else { "You are in no namespace yet." });
        }
        for (i, it) in list.iter().enumerate().skip(start) {
            let head = prev != Some(section(it));
            let need = ROW_H * s + if head { HEAD_H * s } else { 0.0 };
            if y + need > a.y + a.h - trail_h {
                break;
            }
            if head {
                let t = if matches!(it, Item::Tenant(..)) { "Tenant members" } else { "Namespaces" };
                g.text(a.x, y + 2.0 * s, 13.0 * s, MUTED, t, Face::Bold);
                y += HEAD_H * s;
            }
            prev = Some(section(it));
            let (title, sub) = match it {
                Item::Tenant(n, r) => (n.clone(), word(*r).to_string()),
                Item::Ns(n, Some(r)) => (n.clone(), format!("namespace, you are {}", ns_word(*r))),
                Item::Ns(n, None) => (n.clone(), "namespace you are not in: the name only (Join adds you, and is recorded)".into()),
                Item::NsMember(_, n, r) => (format!("    {n}"), format!("    {}", ns_word(*r))),
            };
            let rh = ROW_H * s - 4.0 * s;
            let hot = g.hovered(R::new(a.x, y, a.w, rh));
            crate::fx::row_at(g, a.x, y, a.w, rh, 10.0 * s, i == sel, hot, ACCENT);
            g.hit(R::new(a.x, y, a.w, rh), Event::Row(i), &title);
            let t = g.fit(14.0 * s, &title, Face::Bold, a.w - 20.0 * s);
            g.text(a.x + 10.0 * s, y + 3.0 * s, 14.0 * s, TEXT, &t, Face::Bold);
            let t = g.fit(12.0 * s, &sub, Face::Sans, a.w - 20.0 * s);
            g.text(a.x + 10.0 * s, y + 20.0 * s, 12.0 * s, MUTED, &t, Face::Sans);
            y += ROW_H * s;
        }
        if n_trail > 0 {
            let mut ty = a.y + a.h - trail_h + 4.0 * s;
            g.text(a.x, ty, 13.0 * s, MUTED, "Audit trail (latest last)", Face::Bold);
            ty += 22.0 * s;
            for l in &trail[trail.len() - n_trail..] {
                let t = g.fit(12.0 * s, &l.replace('\t', "  "), Face::Mono, a.w);
                g.text(a.x, ty, 12.0 * s, DIM, &t, Face::Mono);
                ty += 17.0 * s;
            }
        }
    }
}
