//! The person's home-screen layout: the groups they made, where they moved tiles to, and what they chose for apps that name a
//! group of their own. Kept per person in `layout-<user>.json` next to their other interface choices. Moving a tile only changes
//! where it is listed: never what the app may do (permissions come from the role, not from the layout).
//!
//! Pure data and rules, no drawing: the shell, the Settings page and the tests all use this one type.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The built-in groups: (id, label). They stay as they are: they cannot be renamed, deleted or reordered.
pub const BUILTIN: [(&str, &str); 4] = [("home", "Home"), ("web-media", "Web & Media"), ("work", "Work"), ("system", "System")];
/// Longest group name, in characters.
pub const MAX_NAME: usize = 16;
/// Most groups a person can make.
pub const MAX_CUSTOM: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomGroup {
    pub id: String,
    pub label: String,
}

/// What the person chose when an app asked for a group that did not exist: the name the app asked for, and where the app went.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppChoice {
    pub asked: String,
    pub target: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    /// The person's own groups, in the order of their chips (after the built-in ones).
    pub groups: Vec<CustomGroup>,
    /// Tile key (`sys:chat`, `app:calculator`) -> group id: the person moved it there.
    pub placed: BTreeMap<String, String>,
    /// Group id -> tile keys in the order the person wants (tiles not listed follow in the usual order).
    pub order: BTreeMap<String, Vec<String>>,
    /// App name -> the answer to "this app wants to be in the group X".
    pub app_groups: BTreeMap<String, AppChoice>,
    /// Counter for group ids, so a deleted group's id is not reused.
    next_id: u32,
}

pub fn sys_key(label: &str) -> String {
    format!("sys:{}", label.to_lowercase().replace(' ', "-"))
}

pub fn app_key(name: &str) -> String {
    format!("app:{name}")
}

pub fn is_builtin(id: &str) -> bool {
    BUILTIN.iter().any(|(i, _)| *i == id)
}

fn path(dir: &Path, user: &str) -> PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    dir.join(format!("layout-{who}.json"))
}

impl Layout {
    /// The saved layout; a missing, unreadable or older file gives the default one (nothing moved, no groups).
    pub fn load(dir: &Path, user: &str) -> Layout {
        std::fs::read_to_string(path(dir, user)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path, user: &str) {
        let p = path(dir, user);
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }

    /// Nothing moved, no groups, no remembered answers.
    pub fn is_default(&self) -> bool {
        *self == Layout::default()
    }

    pub fn exists(&self, id: &str) -> bool {
        is_builtin(id) || self.groups.iter().any(|g| g.id == id)
    }

    pub fn label_of(&self, id: &str) -> Option<String> {
        BUILTIN.iter().find(|(i, _)| *i == id).map(|(_, l)| l.to_string()).or_else(|| self.groups.iter().find(|g| g.id == id).map(|g| g.label.clone()))
    }

    /// The id of the group with this name or id (case-insensitive), built-in or the person's.
    pub fn find(&self, name: &str) -> Option<String> {
        let n = name.trim().to_lowercase();
        if n.is_empty() {
            return None;
        }
        // old section names: Assistant became Home, and the sample apps still say "My apps" (the section it was; now Work)
        let n = match n.as_str() {
            "assistant" => "home".to_string(),
            "my apps" | "my-apps" => "work".to_string(),
            _ => n,
        };
        BUILTIN.iter().find(|(i, l)| *i == n || l.to_lowercase() == n).map(|(i, _)| i.to_string()).or_else(|| self.groups.iter().find(|g| g.label.to_lowercase() == n).map(|g| g.id.clone()))
    }

    /// Is this a name the person may give a group (`except`: the group being renamed)?
    pub fn check_name(&self, name: &str, except: Option<&str>) -> Result<String, String> {
        let n = name.trim();
        if n.is_empty() {
            return Err("Give the group a name.".into());
        }
        if n.chars().count() > MAX_NAME {
            return Err(format!("Keep the name to {MAX_NAME} characters or fewer."));
        }
        if n.chars().any(|c| c.is_control()) {
            return Err("The name has a character that cannot be shown.".into());
        }
        let low = n.to_lowercase();
        let taken = BUILTIN.iter().any(|(i, l)| *i == low || l.to_lowercase() == low) || matches!(low.as_str(), "assistant" | "all" | "my apps" | "my-apps") || self.groups.iter().any(|g| g.label.to_lowercase() == low && Some(g.id.as_str()) != except);
        if taken {
            return Err(format!("There is already a group called “{n}”."));
        }
        Ok(n.to_string())
    }

    /// Make a group; returns its id.
    pub fn create_group(&mut self, name: &str) -> Result<String, String> {
        let label = self.check_name(name, None)?;
        if self.groups.len() >= MAX_CUSTOM {
            return Err(format!("You can have {MAX_CUSTOM} groups of your own. Delete one first."));
        }
        self.next_id += 1;
        let id = format!("c{}", self.next_id);
        self.groups.push(CustomGroup { id: id.clone(), label });
        Ok(id)
    }

    pub fn rename_group(&mut self, id: &str, name: &str) -> Result<(), String> {
        if is_builtin(id) {
            return Err("The built-in groups keep their names.".into());
        }
        let label = self.check_name(name, Some(id))?;
        let g = self.groups.iter_mut().find(|g| g.id == id).ok_or("No such group.")?;
        g.label = label;
        Ok(())
    }

    /// How many tiles are listed in this group because the person (or their answer about an app) put them there.
    pub fn count_in(&self, id: &str) -> usize {
        let mut keys: Vec<&String> = self.placed.iter().filter(|(_, g)| *g == id).map(|(k, _)| k).collect();
        keys.extend(self.app_groups.iter().filter(|(_, c)| c.target == id).map(|(k, _)| k).filter(|k| !self.placed.contains_key(&app_key(k))));
        keys.len()
    }

    /// Delete one of the person's groups. Its tiles go back to their own default group (apps that asked for it go to Work and
    /// are not asked again). Never uninstalls anything. Returns how many tiles went back. Built-in groups cannot be deleted.
    pub fn delete_group(&mut self, id: &str) -> Result<usize, String> {
        if is_builtin(id) {
            return Err("The built-in groups cannot be deleted.".into());
        }
        if !self.groups.iter().any(|g| g.id == id) {
            return Err("No such group.".into());
        }
        let n = self.count_in(id);
        self.groups.retain(|g| g.id != id);
        self.placed.retain(|_, g| g != id);
        self.order.remove(id);
        for c in self.app_groups.values_mut().filter(|c| c.target == id) {
            c.target = "work".into();
        }
        Ok(n)
    }

    /// Move a custom group one place left (`up`) or right.
    pub fn shift_group(&mut self, id: &str, up: bool) -> bool {
        let Some(i) = self.groups.iter().position(|g| g.id == id) else { return false };
        let j = if up { i.checked_sub(1) } else { Some(i + 1).filter(|j| *j < self.groups.len()) };
        match j {
            Some(j) => {
                self.groups.swap(i, j);
                true
            }
            None => false,
        }
    }

    /// Put a tile in a group. `default` is the group the tile is in when left alone: moving a built-in tile back there removes the
    /// override, while an app keeps the person's choice (it wins over the app's own `group` on updates).
    pub fn place(&mut self, tile: &str, group: &str, default: &str) -> Result<(), String> {
        if !self.exists(group) {
            return Err("No such group.".into());
        }
        for v in self.order.values_mut() {
            v.retain(|k| k != tile);
        }
        if group == default && tile.starts_with("sys:") {
            self.placed.remove(tile);
        } else {
            self.placed.insert(tile.to_string(), group.to_string());
        }
        Ok(())
    }

    /// Put one tile back where it came from.
    pub fn reset_tile(&mut self, tile: &str) {
        self.placed.remove(tile);
        for v in self.order.values_mut() {
            v.retain(|k| k != tile);
        }
    }

    /// Everything back in its own default group (the groups the person made stay, empty or not).
    pub fn reset(&mut self) {
        self.placed.clear();
        self.order.clear();
    }

    /// Where a tile is listed: the person's move if its group still exists, else `default`.
    pub fn group_of(&self, tile: &str, default: &str) -> String {
        match self.placed.get(tile) {
            Some(g) if self.exists(g) => g.clone(),
            _ => default.to_string(),
        }
    }

    /// The group a Store app is in when the person has not moved it: the group its app.json names (built-in or the person's,
    /// any case), else what the person answered, else Work. `None` in the second field: the app names a group that does not
    /// exist and has not been answered yet (the shell asks).
    pub fn app_default(&self, name: &str, declared: &str) -> (String, Option<String>) {
        let d = declared.trim();
        if d.is_empty() {
            return ("work".into(), None);
        }
        if let Some(id) = self.find(d) {
            return (id, None);
        }
        match self.app_groups.get(name) {
            Some(c) if c.asked.eq_ignore_ascii_case(d) && self.exists(&c.target) => (c.target.clone(), None),
            Some(c) if c.asked.eq_ignore_ascii_case(d) => ("work".into(), None),
            _ => ("work".into(), Some(d.to_string())),
        }
    }

    /// The answer to "this app wants to be in the group X. Create it?". `create` makes the group (when its name is fine and there
    /// is room), otherwise the app goes to Work; either way the answer is remembered for this app. Returns where it went.
    pub fn answer_app_group(&mut self, name: &str, asked: &str, create: bool) -> (String, Option<String>) {
        let mut note = None;
        let target = if create {
            match self.create_group(asked) {
                Ok(id) => id,
                Err(e) => {
                    note = Some(e);
                    "work".into()
                }
            }
        } else {
            "work".into()
        };
        self.app_groups.insert(name.to_string(), AppChoice { asked: asked.trim().to_string(), target: target.clone() });
        (target, note)
    }

    /// The tiles of a group in the person's order: those they ordered first, then the rest as given.
    pub fn ordered(&self, group: &str, keys: Vec<String>) -> Vec<String> {
        let Some(o) = self.order.get(group) else { return keys };
        let mut out: Vec<String> = o.iter().filter(|k| keys.contains(k)).cloned().collect();
        let rest: Vec<String> = keys.into_iter().filter(|k| !out.contains(k)).collect();
        out.extend(rest);
        out
    }

    /// Move a tile one place earlier or later within its group. `current` is the group's tiles as shown now.
    pub fn reorder(&mut self, group: &str, tile: &str, earlier: bool, current: Vec<String>) -> bool {
        let mut v = self.ordered(group, current);
        let Some(i) = v.iter().position(|k| k == tile) else { return false };
        let j = if earlier { i.checked_sub(1) } else { Some(i + 1).filter(|j| *j < v.len()) };
        let Some(j) = j else { return false };
        v.swap(i, j);
        self.order.insert(group.to_string(), v);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-layout-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_layout_round_trips_and_a_missing_or_old_file_gives_the_default() {
        let d = tmp("rt");
        assert!(Layout::load(&d, "ann").is_default());
        let mut l = Layout::default();
        let g = l.create_group("Games").unwrap();
        l.place("sys:radio", &g, "web-media").unwrap();
        l.answer_app_group("calc", "Math", false);
        l.save(&d, "ann");
        assert_eq!(Layout::load(&d, "ann"), l);
        assert!(Layout::load(&d, "bob").is_default(), "per person");
        // a file from before the layout existed, or with other keys, or damaged
        std::fs::write(d.join("layout-cy.json"), "{}").unwrap();
        assert!(Layout::load(&d, "cy").is_default());
        std::fs::write(d.join("layout-di.json"), "{\"unknown\":1,\"groups\":[{\"id\":\"c1\",\"label\":\"A\"}]}").unwrap();
        assert_eq!(Layout::load(&d, "di").groups.len(), 1);
        std::fs::write(d.join("layout-ed.json"), "not json").unwrap();
        assert!(Layout::load(&d, "ed").is_default());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn group_names_are_checked_and_built_ins_are_protected() {
        let mut l = Layout::default();
        assert!(l.create_group("").is_err() && l.create_group("   ").is_err());
        assert!(l.create_group("12345678901234567").is_err(), "17 characters");
        assert!(l.create_group("1234567890123456").is_ok(), "16 characters");
        for n in ["work", "WORK", "Home", "system", "Web & Media", "web-media", "assistant"] {
            assert!(l.create_group(n).is_err(), "{n}");
        }
        assert!(l.create_group("games").is_ok());
        assert!(l.create_group("GAMES").is_err() && l.create_group(" Games ").is_err(), "duplicates, any case");
        let id = l.find("games").unwrap();
        assert!(l.rename_group(&id, "Games").is_ok(), "the same name in another case is the same group");
        assert!(l.rename_group(&id, "Work").is_err());
        assert!(l.rename_group("work", "Jobs").is_err(), "built-ins keep their names");
        assert!(l.delete_group("work").is_err() && l.delete_group("home").is_err() && l.delete_group("system").is_err());
        assert!(BUILTIN.iter().all(|(i, _)| l.exists(i)));
        // the limit
        let mut l = Layout::default();
        for i in 0..MAX_CUSTOM {
            l.create_group(&format!("G{i}")).unwrap();
        }
        assert!(l.create_group("One more").is_err());
    }

    #[test]
    fn moving_resetting_and_deleting_a_group_send_tiles_back_without_losing_any() {
        let mut l = Layout::default();
        let g = l.create_group("Games").unwrap();
        l.place("sys:radio", &g, "web-media").unwrap();
        l.place("app:chess", &g, "work").unwrap();
        l.place("sys:chat", "work", "home").unwrap();
        assert_eq!(l.group_of("sys:radio", "web-media"), g);
        assert_eq!(l.group_of("sys:chat", "home"), "work");
        assert_eq!(l.group_of("sys:data", "work"), "work");
        // moving a built-in tile back to its own group drops the override; an app keeps the choice
        l.place("sys:chat", "home", "home").unwrap();
        assert!(!l.placed.contains_key("sys:chat"));
        l.place("app:calc", "work", "work").unwrap();
        assert!(l.placed.contains_key("app:calc"));
        assert!(l.place("sys:chat", "nope", "home").is_err());
        assert_eq!(l.delete_group(&g), Ok(2), "two tiles go back");
        assert_eq!(l.group_of("sys:radio", "web-media"), "web-media");
        assert!(l.groups.is_empty());
        let g2 = l.create_group("More").unwrap();
        assert_ne!(g, g2, "ids are not reused");
        l.place("sys:radio", &g2, "web-media").unwrap();
        l.reset_tile("sys:radio");
        assert_eq!(l.group_of("sys:radio", "web-media"), "web-media");
        l.place("sys:radio", &g2, "web-media").unwrap();
        l.reset();
        assert!(l.placed.is_empty() && l.groups.len() == 1, "the groups stay on a reset");
    }

    #[test]
    fn an_app_that_names_a_group_goes_there_or_the_person_is_asked_once() {
        let mut l = Layout::default();
        assert_eq!(l.app_default("a", ""), ("work".into(), None));
        assert_eq!(l.app_default("a", "web & MEDIA"), ("web-media".into(), None), "a built-in group, any case");
        assert_eq!(l.app_default("a", "Assistant"), ("home".into(), None));
        assert_eq!(l.app_default("a", "My apps"), ("work".into(), None), "the old name of Work: no question");
        assert_eq!(l.app_default("a", "Games"), ("work".into(), Some("Games".into())), "missing: ask");
        let (id, note) = l.answer_app_group("a", "Games", true);
        assert!(note.is_none() && l.exists(&id));
        assert_eq!(l.app_default("a", "Games"), (id.clone(), None));
        assert_eq!(l.app_default("b", "games"), (id.clone(), None), "another app finds the group by name now");
        // the other answer
        assert_eq!(l.app_default("c", "Music"), ("work".into(), Some("Music".into())));
        l.answer_app_group("c", "Music", false);
        assert_eq!(l.app_default("c", "Music"), ("work".into(), None), "remembered, not asked again");
        assert_eq!(l.app_default("c", "Radio shows"), ("work".into(), Some("Radio shows".into())), "a different group asked for later: ask again");
        // deleting the group the app was created for sends it to Work without asking again
        assert_eq!(l.delete_group(&id), Ok(1));
        assert_eq!(l.app_default("a", "Games"), ("work".into(), None));
        // the person's own move wins over the app
        l.place("app:a", "system", "work").unwrap();
        assert_eq!(l.group_of("app:a", "web"), "system");
        // a name that cannot be a group (too long) falls back to Work with a reason
        let (t, note) = l.answer_app_group("d", "A group name that is far too long", true);
        assert_eq!(t, "work");
        assert!(note.is_some());
    }

    #[test]
    fn order_within_a_group_and_the_order_of_the_groups() {
        let mut l = Layout::default();
        let keys = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(l.ordered("work", keys(&["a", "b", "c"])), keys(&["a", "b", "c"]));
        assert!(l.reorder("work", "c", true, keys(&["a", "b", "c"])));
        assert_eq!(l.ordered("work", keys(&["a", "b", "c"])), keys(&["a", "c", "b"]));
        assert_eq!(l.ordered("work", keys(&["a", "c", "b", "d"])), keys(&["a", "c", "b", "d"]), "new tiles follow");
        assert!(!l.reorder("work", "a", true, keys(&["a", "c", "b"])), "already first");
        let (a, b) = (l.create_group("A").unwrap(), l.create_group("B").unwrap());
        assert!(l.shift_group(&b, true) && !l.shift_group(&b, true));
        assert_eq!(l.groups.iter().map(|g| g.id.clone()).collect::<Vec<_>>(), vec![b, a]);
    }
}
