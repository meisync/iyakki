//! What a check of an installed item found out (data only). The check itself stays in `iyakki_agent::store::lifecycle`.

/// What a check found out about one installed thing.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Check {
    UpToDate,
    Update,
    /// The store it came from is gone, removed or switched off, and no other store offers it from the same publisher.
    SourceGone(String),
    /// Withdrawn by the revocation list.
    Revoked,
    /// The private store it came from withdrew (yanked) this very version; the copy still works until it is removed.
    Withdrawn(String),
    /// The store could not be reached (or has not been fetched yet), so nothing newer can be known.
    Offline(String),
    /// No way to tell: installed from a file, written by hand, or the vendor publishes no version number.
    NoWay(String),
    /// Part of IYAKKI: changed only by updating the system.
    BuiltIn,
    /// A command-line tool whose newest version has not been asked yet.
    NotChecked,
}

impl Check {
    pub fn label(&self) -> String {
        match self {
            Check::UpToDate => "Up to date".into(),
            Check::Update => "Update available".into(),
            Check::SourceGone(_) => "Source gone".into(),
            Check::Revoked => "Revoked".into(),
            Check::Withdrawn(_) => "Withdrawn".into(),
            Check::Offline(_) => "Can't check offline".into(),
            Check::NoWay(_) => "No way to check".into(),
            Check::BuiltIn => "Built in".into(),
            Check::NotChecked => "Not checked yet".into(),
        }
    }

    pub fn note(&self) -> String {
        match self {
            Check::SourceGone(n) | Check::Withdrawn(n) | Check::Offline(n) | Check::NoWay(n) => n.clone(),
            _ => String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct RemoveOpts {
    /// Delete its data folder and vault entries as well (default: keep).
    pub delete_data: bool,
    /// For deleting data: the item's name, typed by the person.
    pub typed: String,
    /// The person read the dependents warning.
    pub ack_dependents: bool,
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use crate::store_install::InstallError;
use crate::store_install::Perm;
use crate::store_install::Review;
use crate::store_installed::Entry as IEntry;
use crate::store_installed::Status;
use crate::store_installed::Type;

/// One installed thing with everything the lifecycle knows about it.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Life {
    pub entry: IEntry,
    /// What a person types: `skill:meeting-notes`, `cli:kubectl`.
    pub id: String,
    pub source: String,
    pub source_name: String,
    pub latest: Option<String>,
    pub check: Check,
    pub changelog: String,
    /// Permissions the newer version lists that the installed one does not (from the catalog; the download is checked again).
    pub added_permissions: Vec<String>,
    pub pinned: Option<Pin>,
    pub previous: Option<String>,
    pub history: Vec<Hist>,
    pub builtin: bool,
    /// The update comes from a store other than the one that installed it (same publisher key).
    pub from_other: Option<String>,
    pub cat_source: String,
}

impl Life {
    pub fn key(&self) -> &str {
        &self.entry.key
    }

    pub fn has_update(&self) -> bool {
        self.check == Check::Update
    }

    pub fn can_update(&self) -> bool {
        self.has_update() && self.pinned.is_none() && !matches!(self.entry.status, Status::Revoked)
    }

    pub fn can_rollback(&self) -> bool {
        self.previous.is_some()
    }

    /// Pinning makes sense for what can be updated at all.
    pub fn can_pin(&self) -> bool {
        !self.builtin && !matches!(self.check, Check::NoWay(_)) && self.entry.ty != Type::App
    }

    /// One line for lists: `1.2.0 -> 1.3.0`, `up to date`, ...
    pub fn status_line(&self) -> String {
        let pin = self.pinned.as_ref().map(|p| format!(" (pinned at {})", p.version)).unwrap_or_default();
        match (&self.check, &self.latest) {
            (Check::Update, Some(l)) => format!("{} -> {l}{pin}", self.entry.version),
            (c, _) => format!("{}{pin}", c.label().to_lowercase()),
        }
    }
}

/// What the caller knows about the last look at the network.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Ctx {
    /// `(store name, why)` for every store the last refresh could not read.
    pub refresh_errors: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LifeError {
    /// Needs a yes first: the text says what to confirm.
    NeedsConfirm(String),
    Refused(String),
    Failed(String),
}

impl std::fmt::Display for LifeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LifeError::NeedsConfirm(m) | LifeError::Refused(m) | LifeError::Failed(m) => write!(f, "{m}"),
        }
    }
}

impl From<InstallError> for LifeError {
    fn from(e: InstallError) -> LifeError {
        match e {
            InstallError::NeedsConfirmation(m) => LifeError::NeedsConfirm(m),
            InstallError::Refused(m) | InstallError::Changed(m) => LifeError::Refused(m),
            InstallError::Failed(m) => LifeError::Failed(m),
        }
    }
}

/// What an update would do, worked out from the real package (downloaded and reviewed) before anything changes.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct UpdatePlan {
    pub key: String,
    pub title: String,
    pub ty: Type,
    pub from: String,
    pub to: String,
    pub review: Option<Review>,
    /// Permissions the new version asks for that the installed one did not.
    pub added: Vec<Perm>,
    pub removed: Vec<String>,
    pub warnings: Vec<String>,
    pub changelog: String,
    pub from_other: Option<String>,
    pub cli: bool,
}

impl UpdatePlan {
    /// Whether the person must confirm something before it is applied.
    pub fn needs_confirm(&self) -> bool {
        !self.added.is_empty() || self.review.as_ref().is_some_and(Review::needs_warning_ack) || self.from_other.is_some()
    }

    pub fn summary(&self) -> String {
        let mut s = format!("UPDATE {} from {} to {}\n", self.title, self.from, self.to);
        if !self.changelog.is_empty() {
            s.push_str(&format!("what is new: {}\n", self.changelog));
        }
        if let Some(r) = &self.review {
            s.push_str(&format!("{}: {}\n", r.verdict.level.label(), r.verdict.label));
        }
        if self.added.is_empty() {
            s.push_str("permissions: no new ones\n");
        } else {
            s.push_str(&format!("NEW PERMISSIONS (it did not have these before): {}\n", self.added.iter().map(|p| p.words.clone()).collect::<Vec<_>>().join("; ")));
        }
        if !self.removed.is_empty() {
            s.push_str(&format!("no longer asks for: {}\n", self.removed.join("; ")));
        }
        if let Some(o) = &self.from_other {
            s.push_str(&format!("NOTE: this version comes from {o}, not the store that installed it (same publisher key).\n"));
        }
        for w in &self.warnings {
            s.push_str(&format!("warning: {w}\n"));
        }
        if self.cli {
            s.push_str("It downloads the vendor's release, checks it against the vendor's published checksum where there is one, and only replaces the old version if the new one starts. The old version is kept for one rollback.\n");
        } else {
            s.push_str("The old version is kept for one rollback. If the new one fails its check after installing, the old one is put back.\n");
        }
        s
    }
}

/// What a removal would do.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RemovePlan {
    pub key: String,
    pub title: String,
    pub ty: Type,
    /// Files and registrations that go.
    pub removes: Vec<String>,
    /// Data folders (name, bytes) that exist and can be deleted if the person chooses.
    pub data: Vec<(String, u64)>,
    /// Vault entries named after it.
    pub vault: Vec<String>,
    pub vault_note: String,
    /// Installed things that use it.
    pub dependents: Vec<String>,
    pub cached: u64,
}

impl RemovePlan {
    pub fn has_data(&self) -> bool {
        !self.data.is_empty() || !self.vault.is_empty()
    }

    pub fn summary(&self) -> String {
        let mut s = format!("REMOVE {}\n", self.title);
        for r in &self.removes {
            s.push_str(&format!("- {r}\n"));
        }
        if self.cached > 0 {
            s.push_str(&format!("- the kept copies of its package ({} KB)\n", self.cached.div_ceil(1024)));
        }
        if !self.dependents.is_empty() {
            s.push_str(&format!("WARNING: these use it and may stop working: {}\n", self.dependents.join(", ")));
        }
        if self.has_data() {
            s.push_str("Its data is KEPT unless you choose to delete it: ");
            s.push_str(&self.data.iter().map(|(n, b)| format!("{n} ({} KB)", b.div_ceil(1024))).chain(self.vault.iter().map(|v| format!("vault entry {v}"))).collect::<Vec<_>>().join(", "));
            s.push('\n');
        } else {
            s.push_str("It keeps no data of its own, so there is nothing to keep or delete.\n");
        }
        if !self.vault_note.is_empty() {
            s.push_str(&format!("{}\n", self.vault_note));
        }
        s
    }
}

/// Find one item by `kind:name`, a bare name, or the tile's key. A user's own item wins over a shared one of the same name. (Pure: the agent's
/// `Store::find_life` and the UI's remote store handle both use it, so the lookup needs no call over the socket.)
pub fn find_life<'a>(lives: &'a [Life], id: &str) -> Result<&'a Life, String> {
    let id = id.trim();
    let hits: Vec<&Life> = lives.iter().filter(|l| l.entry.key == id || l.id == id || l.entry.name == id).collect();
    match hits.as_slice() {
        [] => Err(format!("Nothing installed is called \"{id}\". `store list` shows what is.")),
        [one] => Ok(one),
        many => {
            let mine = many.iter().filter(|l| matches!(l.entry.scope, crate::skills::Scope::User(_))).collect::<Vec<_>>();
            if mine.len() == 1 {
                return Ok(mine[0]);
            }
            Err(format!("\"{id}\" is ambiguous: {}. Use kind:name.", many.iter().map(|l| l.id.clone()).collect::<Vec<_>>().join(", ")))
        }
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Report {
    pub items: Vec<(String, Done)>,
}

impl Report {
    pub fn summary(&self) -> String {
        let n = |f: fn(&Done) -> bool| self.items.iter().filter(|(_, d)| f(d)).count();
        let mut s = format!("{} updated, {} skipped, {} failed.\n", n(|d| matches!(d, Done::Updated(_))), n(|d| matches!(d, Done::Skipped(_))), n(|d| matches!(d, Done::Failed(_))));
        for (t, d) in &self.items {
            s.push_str(&match d {
                Done::Updated(m) => format!("updated  {t}: {m}\n"),
                Done::Skipped(m) => format!("skipped  {t}: {m}\n"),
                Done::Failed(m) => format!("FAILED   {t}: {m}\n"),
            });
        }
        s
    }
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use serde::Deserialize;
use serde::Serialize;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Pin {
    pub version: String,
    pub at: i64,
    /// `pinned` (the person chose) or `rolled back` (pinned by going back, so the update does not come straight back).
    pub why: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Hist {
    pub version: String,
    pub action: String,
    pub at: i64,
}

/// One item's result in an Update all run.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Done {
    Updated(String),
    /// Skipped, with the reason (needs confirmation, pinned, source gone, ...).
    Skipped(String),
    Failed(String),
}
