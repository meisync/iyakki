//! A person's own way of arranging the store: tags, groups, and a display title for any item. It lives in one small file per
//! account (`<store>/org-<user>.json`, written atomically), is never sent anywhere, and only labels things: what an item IS
//! (its real name, id and publisher) always stays visible in the detail panel, the lists and the install review, so a display title
//! can never disguise what is installed or who made it. Catalog categories are the publisher's and are not touched here.
//!
//! Items are named by `kind:name` (`skill:meeting-notes`, `cli:kubectl`, `app:chat`), the same in Installed and Discover.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const MAX_GROUPS: usize = 32;
pub const MAX_TAGS_PER_ITEM: usize = 8;
pub const MAX_TAGS: usize = 200;
pub const TAG_LEN: usize = 24;
pub const NAME_LEN: usize = 24;
pub const ALIAS_LEN: usize = 40;
/// The largest organisation file that is read or imported.
pub const MAX_FILE: usize = 256 * 1024;

/// The fixed set of group colours: name and RGB. (Emoji are not offered: the system's fonts are English-first.)
pub const COLOURS: [(&str, [u8; 3]); 8] = [("blue", [90, 150, 255]), ("green", [90, 205, 130]), ("amber", [240, 180, 70]), ("red", [240, 100, 100]), ("violet", [170, 120, 255]), ("teal", [70, 200, 200]), ("pink", [240, 130, 190]), ("grey", [150, 160, 180])];

/// The key under which ungrouped items keep their order.
const UNGROUPED: &str = "-";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub id: String,
    pub name: String,
    /// Index into [`COLOURS`].
    pub colour: u8,
    pub collapsed: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Org {
    /// Groups in the order the person arranged them.
    pub groups: Vec<Group>,
    /// item -> its tags (lower case).
    pub tags: BTreeMap<String, Vec<String>>,
    /// item -> display title.
    pub alias: BTreeMap<String, String>,
    /// item -> group id (an item without an entry is Ungrouped).
    pub assign: BTreeMap<String, String>,
    /// group id (or `-` for Ungrouped) -> the items in the order the person arranged them.
    pub order: BTreeMap<String, Vec<String>>,
    /// How the lists are grouped: `flat`, `groups`, `category`, `kind` or `source` (empty: flat).
    pub view: String,
    #[serde(skip)]
    path: PathBuf,
}

pub fn item_key(kind: &str, name: &str) -> String {
    format!("{kind}:{name}")
}

fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

/// A tag as stored: lower case, one word (inner spaces become `-`), no commas, at most [`TAG_LEN`] characters.
pub fn clean_tag(t: &str) -> String {
    clean(&t.replace(',', " "), TAG_LEN).to_lowercase().replace(' ', "-")
}

impl Org {
    pub fn path_of(dir: &Path, user: &str) -> PathBuf {
        let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
        dir.join(format!("org-{safe}.json"))
    }

    /// The account's arrangement (empty when there is none or the file is unreadable).
    pub fn load(dir: &Path, user: &str) -> Org {
        let path = Org::path_of(dir, user);
        let mut o: Org = std::fs::read(&path).ok().filter(|b| b.len() <= MAX_FILE).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        o.path = path;
        o.sanitize();
        o
    }

    pub fn save(&self) -> Result<(), String> {
        super::install::write_private(&self.path, &serde_json::to_vec_pretty(self).unwrap_or_default())
    }

    /// Drop what a hand-edited or damaged file could not have made.
    fn sanitize(&mut self) {
        self.groups.truncate(MAX_GROUPS);
        let mut seen = HashSet::new();
        self.groups.retain(|g| !g.id.is_empty() && g.id != UNGROUPED && seen.insert(g.id.clone()));
        for g in &mut self.groups {
            g.name = clean(&g.name, NAME_LEN);
            g.colour = g.colour.min(COLOURS.len() as u8 - 1);
        }
        let ids: HashSet<String> = self.groups.iter().map(|g| g.id.clone()).collect();
        self.assign.retain(|_, g| ids.contains(g));
        self.order.retain(|g, _| g == UNGROUPED || ids.contains(g));
        for t in self.tags.values_mut() {
            *t = t.iter().map(|x| clean_tag(x)).filter(|x| !x.is_empty()).collect::<BTreeSet<_>>().into_iter().take(MAX_TAGS_PER_ITEM).collect();
        }
        self.tags.retain(|_, t| !t.is_empty());
        if !["", "flat", "groups", "category", "kind", "source"].contains(&self.view.as_str()) {
            self.view.clear();
        }
        self.alias = std::mem::take(&mut self.alias).into_iter().map(|(k, v)| (k, clean(&v, ALIAS_LEN))).filter(|(_, v)| !v.is_empty()).collect();
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty() && self.tags.is_empty() && self.alias.is_empty()
    }

    // ------------------------------------------------------------------------------------------ titles

    /// The title to show: the person's own, or the real one.
    pub fn display_title<'a>(&'a self, key: &str, real: &'a str) -> &'a str {
        self.alias.get(key).map_or(real, String::as_str)
    }

    pub fn alias_of(&self, key: &str) -> Option<&str> {
        self.alias.get(key).map(String::as_str)
    }

    /// `None` or an empty text clears it.
    pub fn set_alias(&mut self, key: &str, title: Option<&str>) -> Result<(), String> {
        let t = clean(title.unwrap_or(""), ALIAS_LEN);
        if t.is_empty() {
            self.alias.remove(key);
        } else {
            if title.is_some_and(|x| x.chars().filter(|c| !c.is_control()).count() > ALIAS_LEN) {
                return Err(format!("A title is at most {ALIAS_LEN} characters."));
            }
            self.alias.insert(key.to_string(), t);
        }
        Ok(())
    }

    // ------------------------------------------------------------------------------------------ tags

    pub fn tags_of(&self, key: &str) -> &[String] {
        self.tags.get(key).map_or(&[], Vec::as_slice)
    }

    /// Replace the item's tags (duplicates collapse, empties go). At most [`MAX_TAGS_PER_ITEM`] per item and [`MAX_TAGS`] distinct.
    pub fn set_tags(&mut self, key: &str, tags: &[String]) -> Result<(), String> {
        let new: Vec<String> = tags.iter().flat_map(|t| t.split(',')).map(clean_tag).filter(|t| !t.is_empty()).collect::<BTreeSet<_>>().into_iter().collect();
        if new.len() > MAX_TAGS_PER_ITEM {
            return Err(format!("An item can have at most {MAX_TAGS_PER_ITEM} tags."));
        }
        let mut all: BTreeSet<&String> = self.tags.iter().filter(|(k, _)| k.as_str() != key).flat_map(|(_, v)| v.iter()).collect();
        all.extend(new.iter());
        if all.len() > MAX_TAGS {
            return Err(format!("There can be at most {MAX_TAGS} different tags."));
        }
        if new.is_empty() {
            self.tags.remove(key);
        } else {
            self.tags.insert(key.to_string(), new);
        }
        Ok(())
    }

    /// Every tag with the number of items that carry it, most used first.
    pub fn all_tags(&self) -> Vec<(String, usize)> {
        let mut m = BTreeMap::<&String, usize>::new();
        for t in self.tags.values().flatten() {
            *m.entry(t).or_default() += 1;
        }
        let mut v: Vec<(String, usize)> = m.into_iter().map(|(t, n)| (t.clone(), n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }

    /// Existing tags that start with what is typed (for autocomplete), most used first.
    pub fn suggest(&self, typed: &str) -> Vec<String> {
        let t = clean_tag(typed);
        self.all_tags().into_iter().map(|x| x.0).filter(|x| x.starts_with(&t) && *x != t).collect()
    }

    /// Rename a tag everywhere; if `new` already exists the two are merged. Returns how many items changed.
    pub fn rename_tag(&mut self, old: &str, new: &str) -> Result<usize, String> {
        let (old, new) = (clean_tag(old), clean_tag(new));
        if new.is_empty() {
            return Err("A tag needs a name.".into());
        }
        let mut n = 0;
        for t in self.tags.values_mut() {
            if t.contains(&old) {
                t.retain(|x| *x != old);
                if !t.contains(&new) {
                    t.push(new.clone());
                }
                t.sort();
                n += 1;
            }
        }
        if n == 0 {
            return Err(format!("No item has the tag “{old}”."));
        }
        Ok(n)
    }

    pub fn delete_tag(&mut self, tag: &str) -> usize {
        let tag = clean_tag(tag);
        let mut n = 0;
        for t in self.tags.values_mut() {
            if t.contains(&tag) {
                t.retain(|x| *x != tag);
                n += 1;
            }
        }
        self.tags.retain(|_, t| !t.is_empty());
        n
    }

    // ------------------------------------------------------------------------------------------ groups

    pub fn group(&self, id: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn group_of(&self, key: &str) -> Option<&Group> {
        self.assign.get(key).and_then(|id| self.group(id))
    }

    /// Make a group and return its id.
    pub fn create_group(&mut self, name: &str, colour: u8) -> Result<String, String> {
        let name = clean(name, NAME_LEN);
        if name.is_empty() {
            return Err("A group needs a name.".into());
        }
        if self.groups.len() >= MAX_GROUPS {
            return Err(format!("There can be at most {MAX_GROUPS} groups."));
        }
        if self.groups.iter().any(|g| g.name.eq_ignore_ascii_case(&name)) || name.eq_ignore_ascii_case("ungrouped") {
            return Err(format!("There is already a group called “{name}”."));
        }
        let n = self.groups.iter().filter_map(|g| g.id.strip_prefix('g').and_then(|x| x.parse::<u32>().ok())).max().unwrap_or(0) + 1;
        let id = format!("g{n}");
        self.groups.push(Group { id: id.clone(), name, colour: colour.min(COLOURS.len() as u8 - 1), collapsed: false });
        Ok(id)
    }

    pub fn rename_group(&mut self, id: &str, name: &str) -> Result<(), String> {
        let name = clean(name, NAME_LEN);
        if name.is_empty() {
            return Err("A group needs a name.".into());
        }
        if self.groups.iter().any(|g| g.id != id && g.name.eq_ignore_ascii_case(&name)) || name.eq_ignore_ascii_case("ungrouped") {
            return Err(format!("There is already a group called “{name}”."));
        }
        self.groups.iter_mut().find(|g| g.id == id).ok_or("There is no such group.")?.name = name;
        Ok(())
    }

    pub fn next_colour(&mut self, id: &str) {
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == id) {
            g.colour = (g.colour + 1) % COLOURS.len() as u8;
        }
    }

    pub fn toggle_collapsed(&mut self, id: &str) {
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == id) {
            g.collapsed = !g.collapsed;
        }
    }

    /// Delete a group; its items become Ungrouped (they are not touched otherwise).
    pub fn delete_group(&mut self, id: &str) -> Result<usize, String> {
        let at = self.groups.iter().position(|g| g.id == id).ok_or("There is no such group.")?;
        self.groups.remove(at);
        let moved: Vec<String> = self.order.remove(id).unwrap_or_default();
        let n = self.assign.values().filter(|g| *g == id).count();
        self.assign.retain(|_, g| g != id);
        self.order.entry(UNGROUPED.into()).or_default().extend(moved);
        Ok(n)
    }

    /// Put group `id` in position `to` (0 is first).
    pub fn move_group(&mut self, id: &str, to: usize) -> Result<(), String> {
        let at = self.groups.iter().position(|g| g.id == id).ok_or("There is no such group.")?;
        let g = self.groups.remove(at);
        self.groups.insert(to.min(self.groups.len()), g);
        Ok(())
    }

    /// Move group `id` one place up or down.
    pub fn nudge_group(&mut self, id: &str, delta: i32) -> Result<(), String> {
        let at = self.groups.iter().position(|g| g.id == id).ok_or("There is no such group.")?;
        let to = (at as i32 + delta).clamp(0, self.groups.len() as i32 - 1) as usize;
        self.move_group(id, to)
    }

    /// Move an item into `group` (None: Ungrouped), placed before `before` (None: at the end).
    pub fn move_item(&mut self, key: &str, group: Option<&str>, before: Option<&str>) -> Result<(), String> {
        if let Some(g) = group {
            if self.group(g).is_none() {
                return Err("There is no such group.".into());
            }
        }
        for v in self.order.values_mut() {
            v.retain(|k| k != key);
        }
        let gk = group.unwrap_or(UNGROUPED).to_string();
        match group {
            Some(g) => {
                self.assign.insert(key.to_string(), g.to_string());
            }
            None => {
                self.assign.remove(key);
            }
        }
        let list = self.order.entry(gk).or_default();
        match before.and_then(|b| list.iter().position(|k| k == b)) {
            Some(i) => list.insert(i, key.to_string()),
            None => list.push(key.to_string()),
        }
        Ok(())
    }

    /// The group an item is in (`None`: Ungrouped) as an id, for callers that compare.
    pub fn group_id_of(&self, key: &str) -> Option<&str> {
        self.assign.get(key).map(String::as_str).filter(|g| self.group(g).is_some())
    }

    /// Sort `keys` (all in one group) by the person's order; items without a place keep their given order after those with one.
    pub fn arrange(&self, group: Option<&str>, keys: Vec<String>) -> Vec<String> {
        let saved = self.order.get(group.unwrap_or(UNGROUPED)).cloned().unwrap_or_default();
        let mut placed: Vec<String> = saved.into_iter().filter(|k| keys.contains(k)).collect();
        for k in keys {
            if !placed.contains(&k) {
                placed.push(k);
            }
        }
        placed
    }

    /// Move an item one place within its group: `delta` -1 up, +1 down. `visible` is the group's items as listed.
    pub fn nudge_item(&mut self, key: &str, visible: &[String], delta: i32) -> Result<(), String> {
        let at = visible.iter().position(|k| k == key).ok_or("That item is not in the list.")?;
        let to = at as i32 + delta;
        if to < 0 || to >= visible.len() as i32 {
            return Ok(());
        }
        let mut v: Vec<String> = visible.to_vec();
        v.swap(at, to as usize);
        let g = self.group_id_of(key).map(str::to_string);
        self.order.insert(g.unwrap_or_else(|| UNGROUPED.into()), v);
        Ok(())
    }

    // ------------------------------------------------------------------------------------------ searching

    /// Words that find an item by its tags, group and display title (lower case).
    pub fn hay(&self, key: &str) -> String {
        format!("{} {} {}", self.alias.get(key).cloned().unwrap_or_default(), self.tags_of(key).join(" "), self.group_of(key).map(|g| g.name.clone()).unwrap_or_default()).to_lowercase()
    }

    // ------------------------------------------------------------------------------------------ upkeep and moving between machines

    /// Forget entries for items that are not installed or listed any more. Returns how many entries went.
    pub fn prune(&mut self, existing: &HashSet<String>) -> usize {
        let before = self.tags.len() + self.alias.len() + self.assign.len() + self.order.values().map(Vec::len).sum::<usize>();
        self.tags.retain(|k, _| existing.contains(k));
        self.alias.retain(|k, _| existing.contains(k));
        self.assign.retain(|k, _| existing.contains(k));
        for v in self.order.values_mut() {
            v.retain(|k| existing.contains(k));
        }
        self.order.retain(|_, v| !v.is_empty());
        before - (self.tags.len() + self.alias.len() + self.assign.len() + self.order.values().map(Vec::len).sum::<usize>())
    }

    /// The arrangement as a small file to move to another machine.
    pub fn export(&self) -> String {
        let mut j = serde_json::to_value(self).unwrap_or_default();
        j["iyakki_organisation"] = 1.into();
        serde_json::to_string_pretty(&j).unwrap_or_default()
    }

    /// Read an exported file. With `replace` the file becomes the arrangement; otherwise it is merged into the present one (groups
    /// of the same name join, tags are added, the file's titles and placements win). Returns a sentence about what came in.
    pub fn import(&mut self, text: &str, replace: bool) -> Result<String, String> {
        if text.len() > MAX_FILE {
            return Err("That file is too large to be an organisation file.".into());
        }
        let j: serde_json::Value = serde_json::from_str(text).map_err(|_| "That is not an organisation file.".to_string())?;
        if j.get("iyakki_organisation").and_then(|v| v.as_u64()) != Some(1) {
            return Err("That is not an organisation file from this version of IYAKKI.".into());
        }
        let mut theirs: Org = serde_json::from_value(j).map_err(|e| format!("That file is damaged: {e}"))?;
        theirs.sanitize();
        let path = self.path.clone();
        if replace {
            *self = Org { path, ..theirs };
            let n = self.groups.len();
            return Ok(format!("Replaced your arrangement: {n} group(s), {} tagged item(s), {} title(s).", self.tags.len(), self.alias.len()));
        }
        // merge: map their group ids to ours by name
        let mut idmap = BTreeMap::new();
        for g in &theirs.groups {
            let mine = match self.groups.iter().find(|m| m.name.eq_ignore_ascii_case(&g.name)) {
                Some(m) => m.id.clone(),
                None => self.create_group(&g.name, g.colour)?,
            };
            idmap.insert(g.id.clone(), mine);
        }
        for (k, g) in &theirs.assign {
            if let Some(id) = idmap.get(g) {
                self.assign.insert(k.clone(), id.clone());
            }
        }
        for (g, items) in &theirs.order {
            let id = if g == UNGROUPED { UNGROUPED.to_string() } else { idmap.get(g).cloned().unwrap_or_default() };
            if id.is_empty() {
                continue;
            }
            let list = self.order.entry(id).or_default();
            for k in items {
                if !list.contains(k) {
                    list.push(k.clone());
                }
            }
        }
        for (k, t) in &theirs.tags {
            let mut all: Vec<String> = self.tags_of(k).to_vec();
            all.extend(t.iter().cloned());
            self.set_tags(k, &all)?;
        }
        for (k, a) in theirs.alias {
            self.alias.insert(k, a);
        }
        self.sanitize();
        Ok(format!("Merged: {} group(s), {} tagged item(s), {} title(s) came in.", theirs.groups.len(), theirs.tags.len(), self.alias.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-org-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn tags_are_cleaned_limited_suggested_renamed_merged_and_deleted() {
        let mut o = Org::default();
        o.set_tags("skill:a", &["Work".into(), " work ".into(), "Kids, Games".into(), "".into()]).unwrap();
        assert_eq!(o.tags_of("skill:a"), ["games", "kids", "work"].map(String::from), "one lower-case tag each, commas split words");
        o.set_tags("skill:b", &["work".into(), "dev".into()]).unwrap();
        assert_eq!(o.all_tags()[0], ("work".to_string(), 2));
        assert_eq!(o.suggest("w"), vec!["work".to_string()]);
        assert_eq!(o.suggest("work"), Vec::<String>::new(), "a finished word is not suggested again");
        assert!(o.set_tags("skill:c", &(0..9).map(|i| format!("t{i}")).collect::<Vec<_>>()).unwrap_err().contains("at most 8"));
        // rename, and a rename onto an existing tag merges
        assert_eq!(o.rename_tag("dev", "tools").unwrap(), 1);
        assert_eq!(o.rename_tag("kids", "work").unwrap(), 1);
        assert_eq!(o.tags_of("skill:a"), ["games", "work"].map(String::from), "merged without a duplicate");
        assert!(o.rename_tag("nope", "x").is_err() && o.rename_tag("work", " ").is_err());
        assert_eq!(o.delete_tag("work"), 2);
        assert_eq!(o.tags_of("skill:a"), ["games".to_string()]);
        o.set_tags("skill:a", &[]).unwrap();
        assert!(!o.tags.contains_key("skill:a"));
        let many: Vec<String> = (0..201).map(|i| format!("tag{i}")).collect();
        let mut o = Org::default();
        for (i, c) in many.chunks(8).enumerate() {
            let r = o.set_tags(&format!("skill:{i}"), c);
            if i < 25 { assert!(r.is_ok()) } else { assert!(r.unwrap_err().contains("at most 200")) }
        }
    }

    #[test]
    fn groups_are_created_renamed_coloured_ordered_collapsed_and_deleted_without_losing_their_items() {
        let mut o = Org::default();
        let work = o.create_group(" Work ", 1).unwrap();
        let kids = o.create_group("Kids", 99).unwrap();
        assert_eq!((work.as_str(), kids.as_str(), o.group(&kids).unwrap().colour), ("g1", "g2", 7), "the colour is one of the fixed set");
        assert!(o.create_group("work", 0).unwrap_err().contains("already") && o.create_group("Ungrouped", 0).is_err() && o.create_group("  ", 0).is_err());
        o.rename_group(&kids, "Children").unwrap();
        assert!(o.rename_group(&kids, "WORK").is_err() && o.rename_group("g9", "x").is_err());
        o.next_colour(&kids);
        assert_eq!(o.group(&kids).unwrap().colour, 0);
        o.toggle_collapsed(&work);
        assert!(o.group(&work).unwrap().collapsed);
        o.move_group(&kids, 0).unwrap();
        assert_eq!(o.groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["Children", "Work"]);
        o.nudge_group(&kids, 1).unwrap();
        assert_eq!(o.groups[1].name, "Children");
        o.nudge_group(&kids, 5).unwrap();
        assert_eq!(o.groups[1].name, "Children", "the end is the end");
        // items: move, place before another, between groups, and back out
        for k in ["skill:a", "skill:b", "skill:c"] {
            o.move_item(k, Some(&work), None).unwrap();
        }
        o.move_item("skill:c", Some(&work), Some("skill:a")).unwrap();
        assert_eq!(o.arrange(Some(&work), vec!["skill:a".into(), "skill:b".into(), "skill:c".into(), "skill:z".into()]), ["skill:c", "skill:a", "skill:b", "skill:z"].map(String::from), "the new item without a place goes last");
        o.move_item("skill:a", Some(&kids), None).unwrap();
        assert_eq!(o.group_of("skill:a").unwrap().name, "Children");
        assert_eq!(o.order[&work], ["skill:c", "skill:b"].map(String::from));
        assert!(o.move_item("skill:a", Some("g9"), None).is_err());
        o.nudge_item("skill:b", &["skill:c".into(), "skill:b".into()], -1).unwrap();
        assert_eq!(o.order[&work], ["skill:b", "skill:c"].map(String::from));
        o.nudge_item("skill:b", &["skill:b".into(), "skill:c".into()], -1).unwrap();
        // deleting a group keeps its items, ungrouped
        assert_eq!(o.delete_group(&work).unwrap(), 2);
        assert!(o.group_of("skill:b").is_none() && o.group_id_of("skill:c").is_none());
        assert_eq!(o.arrange(None, vec!["skill:c".into(), "skill:b".into()]), ["skill:b", "skill:c"].map(String::from));
        assert!(o.delete_group(&work).is_err());
        for i in 0..40 {
            let _ = o.create_group(&format!("g-{i}"), 0);
        }
        assert_eq!(o.groups.len(), MAX_GROUPS);
        assert!(o.create_group("one more", 0).unwrap_err().contains("at most 32"));
    }

    #[test]
    fn titles_are_cleaned_capped_and_can_be_cleared_and_the_search_text_has_them_all() {
        let mut o = Org::default();
        o.set_alias("skill:a", Some("  My\u{7} notes   helper ")).unwrap();
        assert_eq!(o.display_title("skill:a", "meeting-notes"), "My notes helper");
        assert_eq!(o.display_title("skill:b", "other"), "other");
        assert!(o.set_alias("skill:a", Some(&"x".repeat(50))).unwrap_err().contains("at most 40"));
        let g = o.create_group("Work", 0).unwrap();
        o.move_item("skill:a", Some(&g), None).unwrap();
        o.set_tags("skill:a", &["Daily".into()]).unwrap();
        assert_eq!(o.hay("skill:a"), "my notes helper daily work");
        o.set_alias("skill:a", None).unwrap();
        assert_eq!(o.display_title("skill:a", "meeting-notes"), "meeting-notes");
        o.set_alias("skill:a", Some("")).unwrap();
        assert!(o.alias.is_empty());
    }

    #[test]
    fn each_account_has_its_own_file_written_atomically_and_a_damaged_file_is_just_empty() {
        let d = tmp("persist");
        let mut a = Org::load(&d, "alice");
        let g = a.create_group("Work", 2).unwrap();
        a.move_item("skill:x", Some(&g), None).unwrap();
        a.set_tags("skill:x", &["daily".into()]).unwrap();
        a.set_alias("skill:x", Some("X")).unwrap();
        a.save().unwrap();
        assert!(!d.join("org-alice.tmp").exists());
        let again = Org::load(&d, "alice");
        assert_eq!((again.groups.len(), again.tags_of("skill:x").len(), again.alias_of("skill:x")), (1, 1, Some("X")));
        assert!(Org::load(&d, "bob").is_empty(), "another account sees nothing of it");
        std::fs::write(d.join("org-carol.json"), "{ not json").unwrap();
        assert!(Org::load(&d, "carol").is_empty());
        // a hand-edited file cannot smuggle in a group id of "-", missing groups or control characters
        std::fs::write(d.join("org-dave.json"), r#"{"groups":[{"id":"-","name":"x","colour":1,"collapsed":false},{"id":"g1","name":"Ok\u0007","colour":200,"collapsed":false}],"assign":{"skill:a":"g1","skill:b":"g7"},"alias":{"skill:a":"  "}}"#).unwrap();
        let o = Org::load(&d, "dave");
        assert_eq!((o.groups.len(), o.groups[0].name.as_str(), o.groups[0].colour, o.assign.len(), o.alias.len()), (1, "Ok", 7, 1, 0));
        // an oversized file is ignored
        std::fs::write(d.join("org-erin.json"), vec![b' '; MAX_FILE + 1]).unwrap();
        assert!(Org::load(&d, "erin").is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn what_is_no_longer_there_is_forgotten() {
        let mut o = Org::default();
        let g = o.create_group("Work", 0).unwrap();
        for k in ["skill:a", "skill:gone"] {
            o.move_item(k, Some(&g), None).unwrap();
            o.set_tags(k, &["t".into()]).unwrap();
            o.set_alias(k, Some("A")).unwrap();
        }
        let existing: HashSet<String> = ["skill:a".to_string()].into();
        assert_eq!(o.prune(&existing), 4);
        assert!(!o.tags.contains_key("skill:gone") && !o.alias.contains_key("skill:gone") && !o.assign.contains_key("skill:gone") && o.order[&g] == ["skill:a".to_string()]);
        assert_eq!(o.prune(&existing), 0);
        assert_eq!(o.groups.len(), 1, "an empty group stays");
    }

    #[test]
    fn an_exported_file_comes_back_the_same_merges_by_group_name_and_refuses_strangers() {
        let d = tmp("export");
        let mut a = Org::load(&d, "alice");
        let w = a.create_group("Work", 3).unwrap();
        let k = a.create_group("Kids", 1).unwrap();
        a.move_item("skill:a", Some(&w), None).unwrap();
        a.move_item("skill:b", Some(&w), None).unwrap();
        a.move_item("skill:c", Some(&k), None).unwrap();
        a.set_tags("skill:a", &["daily".into(), "dev".into()]).unwrap();
        a.set_alias("skill:a", Some("Alpha")).unwrap();
        let text = a.export();
        // a straight copy to another machine
        let mut b = Org::load(&d, "bob");
        b.import(&text, true).unwrap();
        assert_eq!((b.groups.clone(), b.tags.clone(), b.alias.clone(), b.assign.clone(), b.order.clone()), (a.groups.clone(), a.tags.clone(), a.alias.clone(), a.assign.clone(), a.order.clone()));
        b.save().unwrap();
        assert!(d.join("org-bob.json").exists());
        // a merge into an arrangement that already has a group of the same name and its own tags
        let mut c = Org::load(&d, "carol");
        let cw = c.create_group("work", 5).unwrap();
        c.move_item("skill:z", Some(&cw), None).unwrap();
        c.set_tags("skill:a", &["mine".into()]).unwrap();
        let said = c.import(&text, false).unwrap();
        assert!(said.starts_with("Merged"), "{said}");
        assert_eq!(c.groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["work", "Kids"], "Work joined the existing group of that name");
        assert_eq!(c.tags_of("skill:a"), ["daily", "dev", "mine"].map(String::from));
        assert_eq!(c.order[&cw], ["skill:z", "skill:a", "skill:b"].map(String::from));
        assert_eq!(c.alias_of("skill:a"), Some("Alpha"));
        // strangers
        assert!(c.import("nonsense", false).unwrap_err().contains("not an organisation file"));
        assert!(c.import("{\"groups\":[]}", false).unwrap_err().contains("not an organisation file"));
        assert!(c.import(&"x".repeat(MAX_FILE + 1), false).unwrap_err().contains("too large"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
