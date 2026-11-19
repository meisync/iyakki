//! The approval record of an installation: who approved which core capabilities, when, for which app version
//! (docs/development/CORE-ACCESS-MODEL.md section 5). Kept in `<store>/approvals.json`, one entry per installation
//! (`tile/user/alice/notes`), private to the machine. The pure rules live in `iyakki_core_api::appcaps`.

use std::collections::BTreeMap;

use iyakki_core_api::appcaps::{Approval, ApprovalBook};
use iyakki_schema::{Migration, Outcome, Spec, Value};

use super::Store;

/// The file: schema 1 is `{"schema":1,"approvals":{<installation>: Approval}}`; a file without a schema is the alpha.2 layout, the bare
/// map, and is lifted (the old bytes kept as `approvals.json.pre-v1`). Fields of the root this release does not know are kept.
pub const SPEC: Spec = Spec {
    name: "approvals",
    current: 1,
    migrations: &[Migration {
        to: 1,
        what: "wrap the bare map as {schema, approvals}",
        run: |v| {
            if let Value::Object(o) = v {
                if !(o.contains_key("approvals") && o.get("approvals").is_some_and(Value::is_object)) {
                    let all = std::mem::take(o);
                    o.insert("approvals".into(), Value::Object(all));
                }
            }
        },
    }],
};

pub use iyakki_shared::store_approvals::{map_of, read_map};

/// The store's file as an [`ApprovalBook`].
pub struct FileBook<'a>(pub &'a Store);

/// What a change starts from: the whole root (to keep unknown root fields), the approvals, and whether a write is allowed.
struct Loaded {
    root: Value,
    map: BTreeMap<String, Approval>,
    writable: Result<(), String>,
}

impl FileBook<'_> {
    fn path(&self) -> std::path::PathBuf {
        self.0.dir().join("approvals.json")
    }

    fn all(&self) -> BTreeMap<String, Approval> {
        read_map(&self.path())
    }

    /// Open for a change. A damaged file is moved aside as `approvals.json.corrupt-<time>` (its bytes are kept, a sentence is queued
    /// for the audit log); when that is not possible, or the file is from a newer release, the change is refused.
    fn load(&self) -> Loaded {
        let path = self.path();
        let out = iyakki_schema::open(&SPEC, &path, iyakki_schema::now_secs());
        if let Some(n) = out.note(&SPEC, &path) {
            iyakki_schema::report(n);
        }
        let fresh = || Value::Object(Default::default());
        match out {
            Outcome::Missing => Loaded { root: fresh(), map: BTreeMap::new(), writable: Ok(()) },
            Outcome::Newer { value, found } => {
                let map = map_of(&value).unwrap_or_default();
                Loaded { root: value, map, writable: Err(iyakki_schema::newer_message(&SPEC, found)) }
            }
            Outcome::Corrupt { moved_to, reason } => {
                let writable = if moved_to.is_some() { Ok(()) } else { Err(format!("The approvals file is damaged ({reason}) and could not be moved aside; it is left as it is: {}", path.display())) };
                Loaded { root: fresh(), map: BTreeMap::new(), writable }
            }
            Outcome::Current(value) | Outcome::Migrated { value, .. } => match map_of(&value) {
                Some(map) => Loaded { root: value, map, writable: Ok(()) },
                None => {
                    // an object that does not fit: damaged, never read as empty and overwritten
                    let moved = iyakki_schema::move_aside(&path, iyakki_schema::now_secs());
                    iyakki_schema::report(format!("approvals {}: the entries do not fit this release; {}", path.display(), moved.as_ref().map(|m| format!("moved to {}", m.display())).unwrap_or_else(|| "could not be moved aside, so nothing will be saved over it".into())));
                    let writable = if moved.is_some() { Ok(()) } else { Err(format!("The approvals file cannot be read and could not be moved aside; it is left as it is: {}", path.display())) };
                    Loaded { root: fresh(), map: BTreeMap::new(), writable }
                }
            },
        }
    }

    fn save(&self, mut l: Loaded) {
        if let Err(e) = &l.writable {
            iyakki_schema::report(format!("approvals {}: change refused, nothing written. {e}", self.path().display()));
            return;
        }
        if let (Value::Object(o), Ok(m)) = (&mut l.root, serde_json::to_value(&l.map)) {
            o.insert("approvals".into(), m);
        }
        if let Err(e) = iyakki_schema::save(&SPEC, &self.path(), &l.root, 0o600) {
            iyakki_schema::report(format!("approvals {}: not saved ({e})", self.path().display()));
        }
    }
}

impl ApprovalBook for FileBook<'_> {
    fn get(&self, installation: &str) -> Option<Approval> {
        self.all().get(installation).cloned()
    }

    fn put(&mut self, installation: &str, a: Approval) {
        let mut l = self.load();
        l.map.insert(installation.into(), a);
        self.save(l);
    }

    fn remove(&mut self, installation: &str) {
        let mut l = self.load();
        if l.map.remove(installation).is_some() {
            self.save(l);
        }
    }
}

impl FileBook<'_> {
    /// Drop the installation's own approval and those of its bundle members (`<installation>/<member>`).
    pub fn remove_with_members(&mut self, installation: &str) {
        let mut l = self.load();
        let prefix = format!("{installation}/");
        let before = l.map.len();
        l.map.retain(|k, _| k != installation && !k.starts_with(&prefix));
        if l.map.len() != before {
            self.save(l);
        }
    }
}

impl Store {
    /// What the person approved for this installation, if anything.
    pub fn approval(&self, installation: &str) -> Option<Approval> {
        FileBook(self).get(installation)
    }
}
