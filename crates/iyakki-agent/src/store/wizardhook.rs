//! The install hook of the wizard engine (F-CORE-24): when an app with a `wizards` list in its manifest is installed or updated, the wizards
//! that apply (`on_install`, or `on_update` whose `from` range matches the version being replaced) are put on the person's PENDING list
//! (`wizards/<person>.json` in the data folder). Nothing runs by itself: the person starts a pending wizard from the chat (`wizard_start`) and
//! the steps that change something ask first. `reconfigure` and `on_remove` wizards are not queued; the person starts them by name.

use std::path::{Path, PathBuf};

use iyakki_core_api::wizard::{TriggerKind, WizardSpec};
use serde::{Deserialize, Serialize};

use super::Store;
use super::tile::Bundle;

const MAX_PENDING: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub app: String,
    pub wizard: String,
    pub title: String,
    pub trigger: String,
    #[serde(default)]
    pub from_version: Option<String>,
    pub queued_at: i64,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    schema: u32,
    #[serde(default)]
    pending: Vec<Pending>,
    #[serde(flatten)]
    extra: std::collections::BTreeMap<String, serde_json::Value>,
}

fn path(data: &Path, user: &str) -> Option<PathBuf> {
    iyakki_sec::policy::valid_username(user).then(|| data.join("wizards").join(format!("{user}.json")))
}

fn read(data: &Path, user: &str) -> File {
    path(data, user).and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(File { schema: 1, ..Default::default() })
}

fn write(data: &Path, user: &str, f: &File) -> Result<(), String> {
    let p = path(data, user).ok_or("not a valid account name")?;
    iyakki_schema::write_atomic(&p, &serde_json::to_vec_pretty(f).map_err(|e| e.to_string())?, 0o600)
}

/// The wizards of every tile app this person has installed: `(app, version, spec)`.
pub fn installed(store: &Store, user: &str) -> Vec<(String, String, WizardSpec)> {
    let mut out = vec![];
    for c in store.content("tile", user) {
        if let Ok(b) = Bundle::from_payload(&c.payload) {
            out.extend(b.manifest.ext.wizards.into_iter().map(|w| (c.name.clone(), c.version.clone(), w)));
        }
    }
    out
}

/// Put the wizards that apply to this install or update on the pending list. `old_version` is the version being replaced (None: a fresh install).
/// Returns how many were queued.
pub fn after_install(store: &Store, data: &Path, user: &str, app: &str, old_version: Option<&str>, now: i64) -> usize {
    let mut f = read(data, user);
    let mut n = 0;
    for (a, _, w) in installed(store, user).into_iter().filter(|(a, _, _)| a == app) {
        let applies = match (w.trigger.kind(), old_version) {
            (TriggerKind::OnInstall, None) => true,
            (TriggerKind::OnUpdate, Some(old)) => w.trigger.matches_update_from(old),
            _ => false,
        };
        if !applies || f.pending.iter().any(|p| p.app == a && p.wizard == w.id && p.trigger == w.trigger.on) || f.pending.len() >= MAX_PENDING {
            continue;
        }
        f.pending.push(Pending { app: a, wizard: w.id.clone(), title: w.title.clone(), trigger: w.trigger.on.clone(), from_version: old_version.map(str::to_string), queued_at: now });
        n += 1;
    }
    if n > 0 {
        let _ = write(data, user, &f);
    }
    n
}

/// What a wizard will ask and run, in one plain line (the same facts as the card of `wizard_continue`, shown before anything starts).
pub fn outline(w: &WizardSpec) -> String {
    use iyakki_core_api::wizard::Step;
    let (mut forms, mut fields, mut checks, mut moves) = (0, 0, vec![], vec![]);
    for s in &w.steps {
        match s {
            Step::Form(f) => {
                forms += 1;
                fields += f.fields.len();
            }
            Step::Check(c) => checks.push(format!("{} {}", c.check, c.param.clone().unwrap_or_default()).trim().to_string()),
            Step::Migration(m) => moves.push(m.migration.clone()),
            _ => {}
        }
    }
    let mut parts = vec![];
    if forms > 0 {
        parts.push(format!("asks {fields} question{}", if fields == 1 { "" } else { "s" }));
    }
    if !checks.is_empty() {
        parts.push(format!("checks: {}", checks.join(", ")));
    }
    if !moves.is_empty() {
        parts.push(format!("moves data: {}", moves.join(", ")));
    }
    if parts.is_empty() { "has nothing to ask or run".into() } else { parts.join(" · ") }
}

pub fn pending(data: &Path, user: &str) -> Vec<Pending> {
    read(data, user).pending
}

/// Take one entry off the list (the wizard was started or the person dismissed it).
pub fn clear(data: &Path, user: &str, app: &str, wizard: &str) -> bool {
    let mut f = read(data, user);
    let before = f.pending.len();
    f.pending.retain(|p| !(p.app == app && p.wizard == wizard));
    f.pending.len() != before && write(data, user, &f).is_ok()
}

/// When an app is removed its queued wizards go with it.
pub fn forget_app(data: &Path, user: &str, app: &str) {
    let mut f = read(data, user);
    let before = f.pending.len();
    f.pending.retain(|p| p.app != app);
    if f.pending.len() != before {
        let _ = write(data, user, &f);
    }
}
