//! What the STORE shows for an installed item (data only): the entry, its type and status, the sort order, the counts for the chips and the filter.
//! Reading what is installed stays in `iyakki_agent::store::installed`.

use crate::skills::Scope;
use crate::store_trust::Level;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    Mcp,
    Cli,
    Skill,
    App,
    Extension,
    /// Driver packs (firmware, module options, kernel modules for this machine): `extension` packages of ext_kind `driver`.
    Driver,
}

impl Type {
    pub const ALL: [Type; 6] = [Type::Mcp, Type::Cli, Type::Skill, Type::App, Type::Extension, Type::Driver];

    /// The filter chip.
    pub fn chip(self) -> &'static str {
        match self {
            Type::Mcp => "MCP",
            Type::Cli => "CLI",
            Type::Skill => "Skills",
            Type::App => "Apps",
            Type::Extension => "Extensions",
            Type::Driver => "Drivers",
        }
    }

    /// The type chip on a row.
    pub fn one(self) -> &'static str {
        match self {
            Type::Mcp => "MCP server",
            Type::Cli => "CLI tool",
            Type::Skill => "Skill",
            Type::App => "System app",
            Type::Extension => "Extension",
            Type::Driver => "Driver",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    On,
    Off,
    Update(String),
    Revoked,
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::On => "On".into(),
            Status::Off => "Turned off".into(),
            Status::Update(v) => format!("Update {v}"),
            Status::Revoked => "Revoked".into(),
        }
    }
}

/// Where "Open" goes: a tile, and what in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open {
    pub tile: String,
    pub item: String,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub key: String,
    pub ty: Type,
    pub kind: String,
    pub name: String,
    pub title: String,
    pub version: String,
    pub level: Level,
    pub trust_label: String,
    pub status: Status,
    /// Bytes, 0 when not known.
    pub size: u64,
    /// Unix seconds, 0 when the system put it there.
    pub installed_at: i64,
    pub scope: Scope,
    pub publisher: String,
    pub license: String,
    pub source: String,
    pub summary: String,
    /// For extensions: the tile it is for and what it is.
    pub target: Option<String>,
    pub ext_kind: Option<String>,
    pub open: Option<Open>,
    pub permissions: Vec<String>,
    pub files: Vec<String>,
    /// The publisher's categories (empty when the item did not come with any).
    pub categories: Vec<String>,
    /// Can be removed here (a system app cannot).
    pub removable: bool,
    /// Can be turned off and on (only what was installed from a package).
    pub can_toggle: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Name,
    Recent,
    Size,
    Trust,
    /// Things with an update first.
    Updates,
}

impl Sort {
    pub fn label(self) -> &'static str {
        match self {
            Sort::Name => "name",
            Sort::Recent => "recently installed or updated",
            Sort::Size => "size",
            Sort::Trust => "trust",
            Sort::Updates => "updates first",
        }
    }

    pub fn next(self) -> Sort {
        match self {
            Sort::Name => Sort::Recent,
            Sort::Recent => Sort::Size,
            Sort::Size => Sort::Trust,
            Sort::Trust => Sort::Updates,
            Sort::Updates => Sort::Name,
        }
    }
}

/// Counts for the chips: all, then each type in [`Type::ALL`] order.
pub fn counts(entries: &[Entry]) -> [usize; 7] {
    let mut c = [entries.len(), 0, 0, 0, 0, 0, 0];
    for e in entries {
        c[1 + Type::ALL.iter().position(|t| *t == e.ty).unwrap_or(0)] += 1;
    }
    c
}

/// The words a search looks through for an entry (lower case).
pub fn hay(e: &Entry) -> String {
    format!("{} {} {} {} {} {}", e.title, e.name, e.summary, e.publisher, e.ty.one(), e.target.as_deref().unwrap_or("")).to_lowercase()
}

/// The entries of a type (all when `None`) that match the words, in the chosen order.
pub fn filter<'a>(entries: &'a [Entry], ty: Option<Type>, query: &str, sort: Sort) -> Vec<&'a Entry> {
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_string).collect();
    let mut v: Vec<&Entry> = entries
        .iter()
        .filter(|e| ty.is_none_or(|t| e.ty == t))
        .filter(|e| {
            let hay = hay(e);
            words.iter().all(|w| hay.contains(w))
        })
        .collect();
    v.sort_by(|a, b| {
        // the tiles of the system always come last: what a person added is what they came to see
        let apps = (a.ty == Type::App).cmp(&(b.ty == Type::App));
        let by_name = a.title.to_lowercase().cmp(&b.title.to_lowercase());
        match sort {
            Sort::Name => apps.then(by_name),
            Sort::Recent => apps.then(b.installed_at.cmp(&a.installed_at)).then(by_name),
            Sort::Size => apps.then(b.size.cmp(&a.size)).then(by_name),
            Sort::Trust => apps.then(a.level.cmp(&b.level)).then(by_name),
            Sort::Updates => apps.then((!matches!(a.status, Status::Update(_))).cmp(&!matches!(b.status, Status::Update(_)))).then(by_name),
        }
    });
    v
}
