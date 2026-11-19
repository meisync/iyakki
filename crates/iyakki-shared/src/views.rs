//! The person's desktop views: several named views (virtual desktops) on one screen, each with its own apps and window
//! arrangement. Kept per person in `views-<user>.json` next to the other interface choices. This file is the pure model
//! (names, order, lock flag, templates, key set, the active view); the shell keeps the live apps of each view and does the
//! switching (`crates/iyakki-shell/src/shell_views.rs`). A view never widens what a person may open: the role policy still
//! decides which apps run, in every view.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Most views one person can have (the jump keys reach 1 to 9).
pub const MAX_VIEWS: usize = 9;
/// Longest view name, in characters.
pub const MAX_NAME: usize = 16;
/// The name of the view every person starts with.
pub const DEFAULT_NAME: &str = "Main";

/// Which keys switch views. The jump keys (Ctrl+Alt+Super+1..9) are the same in every set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeySet {
    /// Previous and next view on Ctrl+Alt+PageUp and Ctrl+Alt+PageDown (as tabs in a browser).
    Browser,
    /// Previous and next view on Control+Command+Left and Right (as the spaces of a Mac).
    Mac,
    /// Both sets at once (no key is used twice).
    #[default]
    Both,
}

impl KeySet {
    pub fn browser(self) -> bool {
        matches!(self, KeySet::Browser | KeySet::Both)
    }
    pub fn mac(self) -> bool {
        matches!(self, KeySet::Mac | KeySet::Both)
    }
    pub fn label(self) -> &'static str {
        match self {
            KeySet::Browser => "browser-style (Ctrl+Alt+PageUp / PageDown)",
            KeySet::Mac => "Mac-style (Control+Command+Left / Right)",
            KeySet::Both => "both sets",
        }
    }
    /// The next choice when the person presses the button.
    pub fn next(self) -> KeySet {
        match self {
            KeySet::Both => KeySet::Browser,
            KeySet::Browser => KeySet::Mac,
            KeySet::Mac => KeySet::Both,
        }
    }
}

/// A ready-made starting point for a view: which apps open in it (by app id), in pane order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Template {
    pub id: &'static str,
    pub name: &'static str,
    pub tiles: &'static [&'static str],
}

pub const TEMPLATES: &[Template] = &[
    Template { id: "work", name: "Work", tiles: &["edit", "cli"] },
    Template { id: "media", name: "Media", tiles: &["media", "radio"] },
    Template { id: "monitoring", name: "Monitoring", tiles: &["system", "usage", "mesh"] },
];

pub fn template(id: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.id == id)
}

/// Most panes a saved layout may name (the shell's largest pane layout has fewer).
pub const MAX_PANES: usize = 9;
/// Most apps the one-window rule may list.
pub const MAX_SINGLE: usize = 40;

/// The window arrangement of a view, kept when the person leaves the view and when they sign out, and put back the next time the view is shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedLayout {
    /// The shell's pane layout id (`single`, `cols-50-50`, `grid`, ...). The shell falls back to one window when it does not know it or it does not fit.
    pub layout: String,
    /// The app in each pane, in pane order, by app id; an empty string is a pane showing the launcher.
    pub apps: Vec<String>,
    /// The pane that had the keyboard.
    #[serde(default)]
    pub focus: usize,
    /// Fields a newer release wrote: kept, and written back (charter rule 2).
    #[serde(flatten)]
    pub extra: iyakki_schema::Extra,
}

fn plain_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 24 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

impl SavedLayout {
    /// Whether the record is usable: a plain layout id, 1 to [`MAX_PANES`] panes each holding a plain app id or nothing, the focus inside them.
    pub fn is_valid(&self) -> bool {
        plain_id(&self.layout) && (1..=MAX_PANES).contains(&self.apps.len()) && self.apps.iter().all(|a| a.is_empty() || plain_id(a)) && self.focus < self.apps.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// Never reused inside one person's file.
    pub id: u32,
    pub name: String,
    /// Switching to this view asks for the person's password again.
    #[serde(default)]
    pub locked: bool,
    /// The apps that open the first time the view is shown (app ids), from its template.
    #[serde(default)]
    pub tiles: Vec<String>,
    /// The window arrangement this view had when it was last left (see [`Views::remember_layout`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<SavedLayout>,
    /// Fields a newer release wrote: kept, and written back (charter rule 2).
    #[serde(flatten)]
    pub extra: iyakki_schema::Extra,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Views {
    pub views: Vec<View>,
    /// The id of the view that was showing (kept across sign-out).
    pub active: u32,
    pub key_set: KeySet,
    /// After the screen lock, return to the first view (default: stay on the active one).
    pub reset_on_lock: bool,
    /// Keep each view's window arrangement across switching and sign-out (default: on). Off forgets the saved ones.
    pub remember_layout: bool,
    /// Apps that keep one window: opening one that is already open in a pane or another view goes to it instead of opening a second. App ids, or `*` for every app. Empty: every app may open more than once (as before).
    pub single_apps: Vec<String>,
    next_id: u32,
    /// Fields a newer release wrote: kept, and written back (charter rule 2).
    #[serde(flatten)]
    pub extra: iyakki_schema::Extra,
}

impl Default for Views {
    fn default() -> Views {
        Views { views: vec![View { id: 1, name: DEFAULT_NAME.into(), locked: false, tiles: Vec::new(), layout: None, extra: Default::default() }], active: 1, key_set: KeySet::Both, reset_on_lock: false, remember_layout: true, single_apps: Vec::new(), next_id: 2, extra: Default::default() }
    }
}

/// The file's schema: 1 is the first stamped layout (the same fields as the unstamped alpha.2 file).
pub const SPEC: iyakki_schema::Spec = iyakki_schema::Spec { name: "views", current: 1, migrations: &[iyakki_schema::Migration { to: 1, what: "stamp the schema (layout unchanged)", run: |_| {} }] };

fn path(dir: &Path, user: &str) -> PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    dir.join(format!("views-{who}.json"))
}

fn clean_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("Give the view a name".into());
    }
    if n.chars().count() > MAX_NAME {
        return Err(format!("A view name has at most {MAX_NAME} characters"));
    }
    if n.chars().any(|c| c.is_control()) {
        return Err("A view name has letters, digits and spaces only".into());
    }
    Ok(n.to_string())
}

impl Views {
    /// The saved views; a missing or unreadable file gives the single view "Main". A file edited by hand is put right:
    /// at least one view, at most [`MAX_VIEWS`], ids unique, names usable, the active view one that exists.
    pub fn load(dir: &Path, user: &str) -> Views {
        // a damaged file is moved aside (`views-<user>.json.corrupt-<time>`), an older one is lifted with a `.pre-v1` copy, a newer
        // one is read and left alone
        let (_, mut v, _) = iyakki_schema::load_typed::<Views>(&SPEC, &path(dir, user), iyakki_schema::now_secs());
        v.extra.remove(iyakki_schema::KEY);
        v.repaired()
    }

    pub fn save(&self, dir: &Path, user: &str) {
        let p = path(dir, user);
        if let Err(e) = iyakki_schema::save_typed(&SPEC, &p, self, 0o600) {
            iyakki_schema::report(format!("views {}: not saved ({e})", p.display()));
        }
    }

    fn repaired(mut self) -> Views {
        let mut seen: Vec<u32> = Vec::new();
        let mut kept: Vec<View> = Vec::new();
        for mut v in std::mem::take(&mut self.views) {
            if seen.contains(&v.id) || kept.len() >= MAX_VIEWS {
                continue;
            }
            match clean_name(&v.name) {
                Ok(n) => v.name = n,
                Err(_) => v.name = format!("View {}", kept.len() + 1),
            }
            v.tiles.truncate(4);
            if !self.remember_layout || v.layout.as_ref().is_some_and(|l| !l.is_valid()) {
                v.layout = None;
            }
            seen.push(v.id);
            kept.push(v);
        }
        if kept.is_empty() {
            return Views { key_set: self.key_set, reset_on_lock: self.reset_on_lock, remember_layout: self.remember_layout, single_apps: self.single_apps, extra: self.extra, ..Views::default() };
        }
        self.next_id = self.next_id.max(kept.iter().map(|v| v.id).max().unwrap_or(0) + 1);
        if !kept.iter().any(|v| v.id == self.active) {
            self.active = kept[0].id;
        }
        self.views = kept;
        self.single_apps.retain(|a| a == "*" || plain_id(a));
        self.single_apps.dedup();
        self.single_apps.truncate(MAX_SINGLE);
        self
    }

    /// Keep the window arrangement a view had (the shell calls it when the person leaves the view and at sign-out). Does nothing, and says so, when
    /// remembering is off or the record is not usable.
    pub fn remember(&mut self, id: u32, layout: SavedLayout) -> bool {
        if !self.remember_layout || !layout.is_valid() {
            return false;
        }
        match self.views.iter_mut().find(|v| v.id == id) {
            Some(v) => {
                v.layout = Some(layout);
                true
            }
            None => false,
        }
    }

    /// Switch remembering on or off. Off forgets every saved arrangement.
    pub fn set_remember_layout(&mut self, on: bool) {
        self.remember_layout = on;
        if !on {
            self.views.iter_mut().for_each(|v| v.layout = None);
        }
    }

    /// Does this app keep one window (the person's one-window rule)?
    pub fn is_single(&self, app: &str) -> bool {
        self.single_apps.iter().any(|a| a == "*" || a == app)
    }

    /// Set the one-window rule: app ids, or `*` for every app; an empty list gives the old behaviour (more than one window of an app).
    pub fn set_single(&mut self, apps: &[String]) -> Result<(), String> {
        let mut v: Vec<String> = Vec::new();
        for a in apps {
            let a = a.trim().to_ascii_lowercase();
            if a != "*" && !plain_id(&a) {
                return Err(format!("\"{a}\" is not an app id (letters, digits, - and _), or * for every app"));
            }
            if !v.contains(&a) {
                v.push(a);
            }
        }
        if v.len() > MAX_SINGLE {
            return Err(format!("At most {MAX_SINGLE} apps"));
        }
        self.single_apps = v;
        Ok(())
    }

    pub fn index_of(&self, id: u32) -> Option<usize> {
        self.views.iter().position(|v| v.id == id)
    }

    pub fn active_index(&self) -> usize {
        self.index_of(self.active).unwrap_or(0)
    }

    pub fn get(&self, id: u32) -> Option<&View> {
        self.views.iter().find(|v| v.id == id)
    }

    fn name_taken(&self, name: &str, except: Option<u32>) -> bool {
        self.views.iter().any(|v| Some(v.id) != except && v.name.eq_ignore_ascii_case(name))
    }

    /// Add a view, optionally from a template (`work`, `media`, `monitoring`). Returns its id.
    pub fn add(&mut self, name: &str, template_id: Option<&str>) -> Result<u32, String> {
        if self.views.len() >= MAX_VIEWS {
            return Err(format!("You can have {MAX_VIEWS} views at most; delete one first"));
        }
        let tpl = match template_id {
            Some(t) => Some(template(t).ok_or_else(|| "No such view template".to_string())?),
            None => None,
        };
        let name = clean_name(name)?;
        if self.name_taken(&name, None) {
            return Err(format!("A view called {name} exists already"));
        }
        let id = self.next_id;
        self.next_id += 1;
        self.views.push(View { id, name, locked: false, tiles: tpl.map(|t| t.tiles.iter().map(|s| s.to_string()).collect()).unwrap_or_default(), layout: None, extra: Default::default() });
        Ok(id)
    }

    /// Add a view from a template, named after it (a number is added when the name is taken). The Settings page and the assistant both use this.
    pub fn add_from_template(&mut self, template_id: &str) -> Result<(u32, String), String> {
        let t = template(template_id).ok_or_else(|| "No such view template".to_string())?;
        let mut name = t.name.to_string();
        let mut n = 2;
        while self.name_taken(&name, None) {
            name = format!("{} {n}", t.name);
            n += 1;
        }
        self.add(&name, Some(t.id)).map(|id| (id, name))
    }

    /// Find a view by its number (1 to 9, as the jump keys count) or by its name (not case sensitive).
    pub fn find(&self, which: &str) -> Result<u32, String> {
        let w = which.trim();
        if let Ok(n) = w.parse::<u8>() {
            return self.nth(n).ok_or_else(|| format!("There is no view number {n}"));
        }
        self.views.iter().find(|v| v.name.eq_ignore_ascii_case(w)).map(|v| v.id).ok_or_else(|| format!("There is no view called {w}"))
    }

    pub fn rename(&mut self, id: u32, name: &str) -> Result<(), String> {
        let name = clean_name(name)?;
        if self.name_taken(&name, Some(id)) {
            return Err(format!("A view called {name} exists already"));
        }
        self.views.iter_mut().find(|v| v.id == id).map(|v| v.name = name).ok_or_else(|| "No such view".to_string())
    }

    /// Delete a view. The last one stays. When the active view goes, the first one becomes active.
    pub fn delete(&mut self, id: u32) -> Result<(), String> {
        let i = self.index_of(id).ok_or_else(|| "No such view".to_string())?;
        if self.views.len() == 1 {
            return Err("The last view cannot be deleted".into());
        }
        self.views.remove(i);
        if self.active == id {
            self.active = self.views[0].id;
        }
        Ok(())
    }

    /// Move a view one place earlier or later. False at the end of the list.
    pub fn shift(&mut self, id: u32, earlier: bool) -> bool {
        let Some(i) = self.index_of(id) else { return false };
        let j = if earlier { i.checked_sub(1) } else { (i + 1 < self.views.len()).then_some(i + 1) };
        match j {
            Some(j) => {
                self.views.swap(i, j);
                true
            }
            None => false,
        }
    }

    pub fn set_locked(&mut self, id: u32, locked: bool) -> bool {
        self.views.iter_mut().find(|v| v.id == id).map(|v| v.locked = locked).is_some()
    }

    /// The id of view number `n` (1 to 9) in the person's order.
    pub fn nth(&self, n: u8) -> Option<u32> {
        (n as usize).checked_sub(1).and_then(|i| self.views.get(i)).map(|v| v.id)
    }

    /// The id of the view after (or before) the active one, wrapping around. None with a single view.
    pub fn step(&self, back: bool) -> Option<u32> {
        let n = self.views.len();
        if n < 2 {
            return None;
        }
        let i = self.active_index();
        Some(self.views[if back { (i + n - 1) % n } else { (i + 1) % n }].id)
    }
}

fn switch_path(dir: &Path, user: &str) -> PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    dir.join(format!("views-switch-{who}"))
}

/// Ask the shell showing `user` to go to view `id` (the assistant's `views_switch`). A locked view still asks for the password.
pub fn request_switch(dir: &Path, user: &str, id: u32) {
    let _ = std::fs::write(switch_path(dir, user), format!("{id}\n"));
}

/// The shell of `user` takes its request: the id of the view to go to. Cheap: one `stat` while nothing is asked.
pub fn take_switch(dir: &Path, user: &str) -> Option<u32> {
    let p = switch_path(dir, user);
    let text = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    text.trim().parse().ok()
}

// ---------------------------------------------------------------------------------------------------------- the person's own templates

/// Most templates one person can keep.
pub const MAX_USER_TEMPLATES: usize = 12;
/// Most apps in one template.
pub const MAX_TEMPLATE_TILES: usize = 6;

/// A template the person made (F-VIEW-01): kept as data in `view-templates-<user>.json`, next to `views-<user>.json`. The built-in ones
/// stay in [`TEMPLATES`]; an id never clashes with a built-in one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserTemplate {
    pub id: String,
    pub name: String,
    /// App ids in pane order. The role policy still decides which of them open.
    pub tiles: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserTemplates {
    pub list: Vec<UserTemplate>,
}

fn tpl_path(dir: &Path, user: &str) -> PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    dir.join(format!("view-templates-{who}.json"))
}

fn slug(name: &str) -> String {
    let s: String = name.trim().to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

impl UserTemplates {
    /// The saved templates; a missing or damaged file gives none, and entries that do not pass the rules are dropped.
    pub fn load(dir: &Path, user: &str) -> UserTemplates {
        let mut t: UserTemplates = std::fs::read_to_string(tpl_path(dir, user)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        t.list.retain(|x| !x.id.is_empty() && template(&x.id).is_none() && clean_name(&x.name).is_ok() && check_tiles(&x.tiles).is_ok());
        t.list.truncate(MAX_USER_TEMPLATES);
        t
    }

    /// Write the file (atomically). False when it could not be written.
    pub fn save(&self, dir: &Path, user: &str) -> bool {
        let p = tpl_path(dir, user);
        let tmp = p.with_extension("json.part");
        serde_json::to_string_pretty(self).ok().is_some_and(|s| std::fs::write(&tmp, s).and_then(|_| std::fs::rename(&tmp, &p)).is_ok())
    }

    pub fn get(&self, id: &str) -> Option<&UserTemplate> {
        self.list.iter().find(|t| t.id == id.trim().to_lowercase() || t.name.eq_ignore_ascii_case(id.trim()))
    }

    /// Make a template (or replace the one with the same name). Returns its id.
    pub fn save_template(&mut self, name: &str, tiles: &[String]) -> Result<String, String> {
        let name = clean_name(name)?;
        check_tiles(tiles)?;
        let id = slug(&name);
        if id.is_empty() {
            return Err("Give the template a name with letters or digits".into());
        }
        if template(&id).is_some() {
            return Err(format!("{name} is the name of a built-in template; choose another"));
        }
        let t = UserTemplate { id: id.clone(), name, tiles: tiles.to_vec() };
        if let Some(i) = self.list.iter().position(|x| x.id == id) {
            self.list[i] = t;
        } else if self.list.len() >= MAX_USER_TEMPLATES {
            return Err(format!("You can keep {MAX_USER_TEMPLATES} templates at most; delete one first"));
        } else {
            self.list.push(t);
        }
        Ok(id)
    }

    pub fn remove(&mut self, which: &str) -> Result<(), String> {
        let id = self.get(which).map(|t| t.id.clone()).ok_or_else(|| format!("There is no template called {which}"))?;
        self.list.retain(|t| t.id != id);
        Ok(())
    }
}

fn check_tiles(tiles: &[String]) -> Result<(), String> {
    if tiles.is_empty() || tiles.len() > MAX_TEMPLATE_TILES {
        return Err(format!("A template has 1 to {MAX_TEMPLATE_TILES} apps"));
    }
    if tiles.iter().any(|t| t.is_empty() || t.len() > 24 || !t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
        return Err("App ids are letters, digits, - and _ (at most 24 characters)".into());
    }
    Ok(())
}

impl Views {
    /// Add a view from one of the person's own templates, named after it (a number is added when the name is taken).
    pub fn add_from_user_template(&mut self, t: &UserTemplate) -> Result<(u32, String), String> {
        let mut name = t.name.clone();
        let mut n = 2;
        while self.name_taken(&name, None) {
            name = format!("{} {n}", t.name);
            n += 1;
        }
        let id = self.add(&name, None)?;
        if let Some(v) = self.views.iter_mut().find(|v| v.id == id) {
            v.tiles = t.tiles.clone();
        }
        Ok((id, name))
    }
}

#[cfg(test)]
mod user_template_tests {
    use super::*;

    fn tiles(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_person_keeps_own_templates_as_data_and_a_view_can_be_made_from_one() {
        let d = std::env::temp_dir().join(format!("iyakki-usertpl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let mut t = UserTemplates::load(&d, "ann");
        assert!(t.list.is_empty());
        assert_eq!(t.save_template("Study room", &tiles(&["learn", "notes"])).unwrap(), "study-room");
        assert!(t.save(&d, "ann"));
        assert!(UserTemplates::load(&d, "bob").list.is_empty(), "per person");
        let again = UserTemplates::load(&d, "ann");
        assert_eq!(again, t);
        let mut v = Views::default();
        let (id, name) = v.add_from_user_template(again.get("study room").unwrap()).unwrap();
        assert_eq!(name, "Study room");
        assert_eq!(v.get(id).unwrap().tiles, tiles(&["learn", "notes"]));
        assert_eq!(v.add_from_user_template(again.get("study-room").unwrap()).unwrap().1, "Study room 2");
        // the same name replaces
        t.save_template("study ROOM", &tiles(&["edit"])).unwrap();
        assert_eq!(t.list.len(), 1);
        t.remove("Study room").unwrap();
        assert!(t.remove("Study room").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bad_names_apps_and_built_in_clashes_are_refused_and_a_hand_edited_file_is_repaired() {
        let mut t = UserTemplates::default();
        assert!(t.save_template("Work", &tiles(&["edit"])).unwrap_err().contains("built-in"));
        assert!(t.save_template("", &tiles(&["edit"])).is_err());
        assert!(t.save_template("x", &[]).is_err());
        assert!(t.save_template("x", &tiles(&["a", "b", "c", "d", "e", "f", "g"])).is_err());
        assert!(t.save_template("x", &tiles(&["../etc"])).is_err());
        for i in 0..MAX_USER_TEMPLATES {
            t.save_template(&format!("t{i}"), &tiles(&["edit"])).unwrap();
        }
        assert!(t.save_template("one more", &tiles(&["edit"])).unwrap_err().contains("at most"));
        let d = std::env::temp_dir().join(format!("iyakki-usertpl2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(tpl_path(&d, "ann"), r#"{"list":[{"id":"work","name":"Work","tiles":["edit"]},{"id":"ok","name":"Ok","tiles":["edit"]},{"id":"bad","name":"Bad","tiles":[]}]}"#).unwrap();
        let l = UserTemplates::load(&d, "ann");
        assert_eq!(l.list.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), vec!["ok"]);
        std::fs::write(tpl_path(&d, "ann"), "not json").unwrap();
        assert!(UserTemplates::load(&d, "ann").list.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-views-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn starts_with_main() {
        let v = Views::default();
        assert_eq!(v.views.len(), 1);
        assert_eq!(v.views[0].name, "Main");
        assert_eq!(v.active, v.views[0].id);
        assert_eq!(v.key_set, KeySet::Both);
        assert!(!v.reset_on_lock);
    }

    #[test]
    fn add_rename_delete_shift() {
        let mut v = Views::default();
        let a = v.add("Chat", None).unwrap();
        let b = v.add("  Docs ", None).unwrap();
        assert_eq!(v.get(b).unwrap().name, "Docs");
        assert!(v.add("chat", None).is_err(), "names differ by more than case");
        assert!(v.add("", None).is_err());
        assert!(v.add(&"x".repeat(17), None).is_err());
        assert!(v.add("bad\u{7}", None).is_err());
        v.rename(a, "Talk").unwrap();
        assert!(v.rename(a, "docs").is_err());
        assert!(v.shift(b, true));
        assert_eq!(v.views.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["Main", "Docs", "Talk"]);
        assert!(!v.shift(v.views[0].id, true), "already first");
        assert!(!v.shift(a, false), "already last");
        v.active = a;
        v.delete(a).unwrap();
        assert_eq!(v.active, v.views[0].id, "deleting the active view returns to the first");
        v.delete(b).unwrap();
        assert!(v.delete(v.views[0].id).is_err(), "the last view stays");
    }

    #[test]
    fn ids_are_not_reused_and_count_is_bounded() {
        let mut v = Views::default();
        let a = v.add("A", None).unwrap();
        v.delete(a).unwrap();
        let b = v.add("B", None).unwrap();
        assert!(b > a);
        for i in 0..8 {
            let r = v.add(&format!("V{i}"), None);
            if v.views.len() <= MAX_VIEWS {
                assert!(r.is_ok() || v.views.len() == MAX_VIEWS, "{r:?}");
            }
        }
        assert_eq!(v.views.len(), MAX_VIEWS);
        assert!(v.add("One more", None).unwrap_err().contains("at most"));
    }

    #[test]
    fn templates_give_tiles() {
        let mut v = Views::default();
        let id = v.add("Desk", Some("work")).unwrap();
        assert_eq!(v.get(id).unwrap().tiles, ["edit", "cli"]);
        assert!(v.add("Odd", Some("nope")).is_err());
        assert!(TEMPLATES.len() >= 3 && template("monitoring").is_some());
    }

    #[test]
    fn step_wraps_and_nth_is_in_order() {
        let mut v = Views::default();
        let a = v.add("A", None).unwrap();
        let b = v.add("B", None).unwrap();
        assert_eq!(v.step(false), Some(a));
        v.active = b;
        assert_eq!(v.step(false), Some(1), "wraps to the first");
        v.active = 1;
        assert_eq!(v.step(true), Some(b), "wraps to the last");
        assert_eq!(v.nth(2), Some(a));
        assert_eq!(v.nth(9), None);
        assert_eq!(v.nth(0), None);
        assert_eq!(Views::default().step(false), None);
    }

    #[test]
    fn round_trip_and_repair() {
        let d = tmp("rt");
        let mut v = Views::default();
        let a = v.add("Media", Some("media")).unwrap();
        v.set_locked(a, true);
        v.active = a;
        v.key_set = KeySet::Mac;
        v.reset_on_lock = true;
        v.save(&d, "ada");
        assert_eq!(Views::load(&d, "ada"), v);
        assert_eq!(Views::load(&d, "bob"), Views::default(), "another person has their own");
        std::fs::write(path(&d, "ada"), r#"{"views":[{"id":3,"name":""},{"id":3,"name":"dup"}],"active":99}"#).unwrap();
        let r = Views::load(&d, "ada");
        assert_eq!(r.views.len(), 1);
        assert_eq!(r.active, 3);
        assert!(!r.views[0].name.is_empty());
        std::fs::write(path(&d, "ada"), "not json").unwrap();
        assert_eq!(Views::load(&d, "ada"), Views::default());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn key_sets() {
        assert!(KeySet::Both.browser() && KeySet::Both.mac());
        assert!(KeySet::Browser.browser() && !KeySet::Browser.mac());
        assert!(!KeySet::Mac.browser() && KeySet::Mac.mac());
        assert_eq!(KeySet::Both.next().next().next(), KeySet::Both);
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    fn lay(layout: &str, apps: &[&str], focus: usize) -> SavedLayout {
        SavedLayout { layout: layout.into(), apps: apps.iter().map(|s| s.to_string()).collect(), focus, extra: Default::default() }
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-vl-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_view_keeps_its_window_layout_across_a_save_and_a_load_and_bad_records_are_refused() {
        let d = tmp("keep");
        let mut v = Views::default();
        let id = v.add("Work", None).unwrap();
        assert!(v.remember(id, lay("cols-50-50", &["edit", ""], 1)));
        assert!(!v.remember(id, lay("../x", &["edit"], 0)), "an id with a slash is refused");
        assert!(!v.remember(id, lay("grid", &["edit", "chat"], 5)), "the focus must be inside the panes");
        assert!(!v.remember(id, lay("grid", &[], 0)), "no panes");
        assert!(!v.remember(99, lay("single", &["edit"], 0)), "no such view");
        v.save(&d, "ann");
        let back = Views::load(&d, "ann");
        let l = back.get(id).unwrap().layout.clone().unwrap();
        assert_eq!((l.layout.as_str(), l.apps, l.focus), ("cols-50-50", vec!["edit".to_string(), String::new()], 1));
        assert!(back.views[0].layout.is_none(), "the other view has none");
    }

    #[test]
    fn switching_remembering_off_forgets_the_layouts_and_a_hand_edited_bad_record_is_dropped_on_load() {
        let d = tmp("off");
        let mut v = Views::default();
        let main = v.views[0].id;
        v.remember(main, lay("single", &["chat"], 0));
        v.set_remember_layout(false);
        assert!(v.views[0].layout.is_none() && !v.remember(main, lay("single", &["chat"], 0)), "nothing is kept while it is off");
        v.set_remember_layout(true);
        v.save(&d, "bo");
        let p = path(&d, "bo");
        let mut doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        doc["views"][0]["layout"] = serde_json::json!({"layout": "grid", "apps": ["a/b"], "focus": 0});
        let text = doc.to_string();
        assert!(text.contains("a/b"), "{text}");
        std::fs::write(&p, text).unwrap();
        assert!(Views::load(&d, "bo").views[0].layout.is_none(), "a record with a bad app id is dropped");
        // an older file without the new fields loads with the defaults
        std::fs::write(&p, r#"{"views":[{"id":1,"name":"Main"}],"active":1}"#).unwrap();
        let old = Views::load(&d, "bo");
        assert!(old.remember_layout && old.single_apps.is_empty() && old.views[0].layout.is_none());
    }

    #[test]
    fn the_one_window_rule_takes_app_ids_or_star_refuses_bad_ones_and_empty_turns_it_off() {
        let mut v = Views::default();
        assert!(!v.is_single("browser"));
        v.set_single(&["Browser".to_string(), "browser".to_string(), "edit".to_string()]).unwrap();
        assert_eq!(v.single_apps, ["browser", "edit"]);
        assert!(v.is_single("browser") && v.is_single("edit") && !v.is_single("chat"));
        v.set_single(&["*".to_string()]).unwrap();
        assert!(v.is_single("anything"));
        assert!(v.set_single(&["../etc".to_string()]).is_err() && v.is_single("anything"), "a bad id changes nothing");
        assert!(v.set_single(&(0..41).map(|i| format!("a{i}")).collect::<Vec<_>>()).is_err());
        v.set_single(&[]).unwrap();
        assert!(!v.is_single("browser"));
    }
}
