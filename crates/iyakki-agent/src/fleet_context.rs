//! The project Context shared across fleet members (F-FLEET-04; docs/development/FLEET-AND-MODEL-ROUTER-PLAN.md section 3.4; the Context is
//! `notes.rs`, F-IDE-13). Sharing is off until the person turns it on for a fleet and sending asks the core every time.
//!
//! A member sends the notes that changed since the last time it sent (compared by a hash of each note's content against a base kept
//! outside the workspace, so the committed Context files do not change just because of syncing). A receiver treats each incoming note like
//! this:
//!
//! * not here yet: it is added;
//! * here and the same: nothing;
//! * here, changed only on the sender's side since the base: it is replaced (a fast-forward);
//! * here, changed here and also there (or no common base): **both versions are kept**. The local note stays as it is and the incoming one is
//!   added beside it as a copy with `(from NAME)` in the title; the pair is listed as a conflict until a person resolves it. Nothing is
//!   overwritten silently, and a conflict copy is never sent on, so two members cannot bounce one forever.
//!
//! What arrives is data, not an order: an incoming note is added as an unreviewed, untrusted note by an agent (the person's own trust marks on
//! a note already here are kept), a note whose text looks like a secret is refused, and sizes are clipped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::cloudkeys::hex;
use crate::notes::{self, Kind, Note, Status};

/// A note in two versions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    /// The note as this computer had it (it stays).
    pub mine: String,
    /// The copy of the other member's version.
    pub theirs: String,
    pub from: String,
    pub title: String,
}

pub struct ContextSync {
    root: PathBuf,
    dir: PathBuf,
}

fn hash_of(n: &Note) -> String {
    hex(&Sha256::digest(format!("{}\n{}\n{}\n{}\n{}", n.title, n.text, n.kind.key(), n.pinned, n.status.key()).as_bytes()))
}

fn status_of(s: &str) -> Status {
    match s {
        "archived" => Status::Archived,
        "superseded" => Status::Superseded,
        _ => Status::Active,
    }
}

fn clip(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control() || *c == '\n').take(max).collect::<String>().trim().to_string()
}

impl ContextSync {
    /// `root` is the workspace, `dir` a folder outside it for the sync base and the conflict list.
    pub fn new(root: &Path, dir: &Path) -> ContextSync {
        ContextSync { root: root.to_path_buf(), dir: dir.to_path_buf() }
    }

    fn file(&self, fleet: &str, what: &str) -> PathBuf {
        self.dir.join(format!("{}.{what}.json", fleet.chars().filter(char::is_ascii_hexdigit).take(64).collect::<String>()))
    }

    fn base(&self, fleet: &str) -> BTreeMap<String, String> {
        std::fs::read_to_string(self.file(fleet, "base")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save<T: Serialize>(&self, fleet: &str, what: &str, v: &T) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        std::fs::write(self.file(fleet, what), serde_json::to_string(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }

    pub fn conflicts(&self, fleet: &str) -> Vec<Conflict> {
        std::fs::read_to_string(self.file(fleet, "conflicts")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    /// The message to send: the notes whose content differs from what was last sent or received, never a conflict copy. Also the
    /// (id, hash) pairs to hand to [`ContextSync::sent`] when every member took it.
    pub fn outgoing(&self, fleet: &str) -> (Value, Vec<(String, String)>) {
        let base = self.base(fleet);
        let copies: Vec<String> = self.conflicts(fleet).into_iter().map(|c| c.theirs).collect();
        let (all, _) = notes::load(&self.root);
        let mut out = vec![];
        let mut pairs = vec![];
        for n in all.notes.iter().filter(|n| !copies.contains(&n.id)) {
            let h = hash_of(n);
            if base.get(&n.id) != Some(&h) {
                out.push(json!({"id": n.id, "title": n.title, "text": n.text, "kind": n.kind.key(), "pinned": n.pinned, "status": n.status.key(), "author": n.author, "updated": n.updated, "base": base.get(&n.id).cloned().unwrap_or_default(), "hash": h}));
                pairs.push((n.id.clone(), h));
            }
        }
        (json!({"notes": out}), pairs)
    }

    /// The notes sent are now the base of the next comparison.
    pub fn sent(&self, fleet: &str, pairs: &[(String, String)]) -> Result<(), String> {
        let mut base = self.base(fleet);
        for (id, h) in pairs {
            base.insert(id.clone(), h.clone());
        }
        self.save(fleet, "base", &base)
    }

    /// Take what a member sent. Returns one plain line (counts).
    pub fn incoming(&self, fleet: &str, from_name: &str, msg: &Value) -> Result<String, String> {
        let list = msg.get("notes").and_then(Value::as_array).ok_or("the message holds no notes")?;
        if list.len() > notes::MAX_NOTES {
            return Err("too many notes in one message".into());
        }
        let mut base = self.base(fleet);
        let mut conflicts = self.conflicts(fleet);
        let who = clip(from_name, 30);
        let (mut added, mut updated, mut kept_both, mut refused) = (0, 0, 0, 0);
        notes::edit(&self.root, |all| {
            for v in list {
                let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                let (id, title, text) = (clip(&s("id"), 12), clip(&s("title"), notes::MAX_TITLE), clip(&s("text"), notes::MAX_TEXT_PERSON));
                if id.is_empty() || title.is_empty() || notes::refuse_secret(&format!("{title}\n{text}")).is_err() {
                    refused += 1;
                    continue;
                }
                let kind = Kind::parse(&s("kind")).unwrap_or(Kind::Reference);
                let incoming = Note {
                    id: id.clone(),
                    title: title.clone(),
                    text: text.clone(),
                    kind,
                    pinned: v.get("pinned").and_then(Value::as_bool).unwrap_or(false),
                    author: format!("{who} (fleet)"),
                    by_agent: true,
                    created: v.get("updated").and_then(Value::as_i64).unwrap_or(0),
                    updated: v.get("updated").and_then(Value::as_i64).unwrap_or(0),
                    source: format!("fleet:{who}"),
                    status: status_of(&s("status")),
                    reviewed: false,
                    trusted: false,
                    pin_requested: false,
                    superseded_by: String::new(),
                };
                let their_hash = hash_of(&incoming);
                match all.notes.iter().position(|n| n.id == id) {
                    None => {
                        if all.notes.len() >= notes::MAX_NOTES {
                            refused += 1;
                            continue;
                        }
                        all.notes.push(incoming);
                        base.insert(id, their_hash);
                        added += 1;
                    }
                    Some(at) => {
                        let mine = hash_of(&all.notes[at]);
                        if mine == their_hash {
                            base.insert(id, their_hash);
                            continue;
                        }
                        let their_base = s("base");
                        let my_base = base.get(&id).cloned().unwrap_or_default();
                        // they did not change it since we last agreed: ours is newer
                        if !their_base.is_empty() && their_base == their_hash {
                            continue;
                        }
                        if !my_base.is_empty() && mine == my_base {
                            // only the sender changed it: take it, keeping the trust the person here gave the note
                            let keep = &all.notes[at];
                            let n = Note { by_agent: keep.by_agent, reviewed: keep.reviewed, trusted: keep.trusted, author: keep.author.clone(), created: keep.created, ..incoming };
                            all.notes[at] = n;
                            base.insert(id, their_hash);
                            updated += 1;
                        } else {
                            if all.notes.len() >= notes::MAX_NOTES {
                                refused += 1;
                                continue;
                            }
                            let copy_id = format!("N-f{}", &hex(&Sha256::digest(format!("{fleet}{id}{their_hash}").as_bytes()))[..8]);
                            if all.notes.iter().any(|n| n.id == copy_id) {
                                continue;
                            }
                            all.notes.push(Note { id: copy_id.clone(), title: clip(&format!("{title} (from {who})"), notes::MAX_TITLE), ..incoming });
                            // the copy is never sent on, and the local note is not marked as agreed
                            conflicts.push(Conflict { mine: id.clone(), theirs: copy_id, from: who.clone(), title });
                            kept_both += 1;
                        }
                    }
                }
            }
            Ok(())
        })?;
        self.save(fleet, "base", &base)?;
        self.save(fleet, "conflicts", &conflicts)?;
        Ok(format!("{added} added, {updated} updated, {kept_both} kept in both versions, {refused} refused"))
    }

    /// A person settles a conflict: `keep` is `mine` (the copy is archived), `theirs` (the local note takes the other text and the copy is
    /// archived) `both` (nothing changes; the entry is only cleared from the list) or `merge` (the two texts joined line by line, see [`merge_lines`]).
    pub fn resolve(&self, fleet: &str, copy_id: &str, keep: &str) -> Result<String, String> {
        let mut conflicts = self.conflicts(fleet);
        let at = conflicts.iter().position(|c| c.theirs == copy_id || c.mine == copy_id).ok_or("no such conflict")?;
        let c = conflicts.remove(at);
        notes::edit(&self.root, |all| {
            let copy = all.notes.iter().find(|n| n.id == c.theirs).cloned();
            match keep {
                "mine" => {
                    if let Some(n) = all.notes.iter_mut().find(|n| n.id == c.theirs) {
                        n.status = Status::Archived;
                    }
                }
                "theirs" => {
                    let copy = copy.ok_or("the copy is gone")?;
                    if let Some(n) = all.notes.iter_mut().find(|n| n.id == c.mine) {
                        n.text = copy.text.clone();
                        n.kind = copy.kind;
                        n.pinned = copy.pinned;
                        n.updated = copy.updated.max(n.updated);
                    }
                    if let Some(n) = all.notes.iter_mut().find(|n| n.id == c.theirs) {
                        n.status = Status::Archived;
                    }
                }
                "both" => {}
                "merge" => {
                    let copy = copy.ok_or("the copy is gone")?;
                    let mine = all.notes.iter().find(|n| n.id == c.mine).map(|n| n.text.clone()).ok_or("the note is gone")?;
                    let merged = clip(&merge_lines(&mine, &copy.text), notes::MAX_TEXT_PERSON);
                    notes::refuse_secret(&merged)?;
                    if let Some(n) = all.notes.iter_mut().find(|n| n.id == c.mine) {
                        n.text = merged;
                        n.updated = copy.updated.max(n.updated);
                    }
                    if let Some(n) = all.notes.iter_mut().find(|n| n.id == c.theirs) {
                        n.status = Status::Archived;
                    }
                }
                _ => return Err("keep is one of mine, theirs, both, merge".into()),
            }
            Ok(())
        })?;
        self.save(fleet, "conflicts", &conflicts)?;
        Ok(format!("settled \"{}\" ({keep})", c.title))
    }
}

/// Two versions of one note's text joined line by line: the lines both have appear once, in order; between them the lines only this computer
/// has come first and then the lines only the other member has (longest common subsequence of the lines). Nothing is lost from either side.
pub fn merge_lines(mine: &str, theirs: &str) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (mine.lines().collect(), theirs.lines().collect());
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out: Vec<&str> = vec![];
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            out.push(a[i]);
            i += 1;
            j += 1;
        } else if i < n && (j >= m || lcs[i + 1][j] >= lcs[i][j + 1]) {
            out.push(a[i]);
            i += 1;
        } else {
            out.push(b[j]);
            j += 1;
        }
    }
    out.join("\n")
}

#[cfg(test)]
#[path = "fleet_context_tests.rs"]
mod tests;
