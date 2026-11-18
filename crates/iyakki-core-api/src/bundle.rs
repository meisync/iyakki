//! The bundle part of an app manifest: `manifest_version`, `requires`, the `contents` list (docs/development/CORE-ACCESS-MODEL.md section
//! 6d) and the carrier for `wizards` (section 6e, see [`crate::wizard`]).
//!
//! Data and validation only, no IO and nothing beyond serde, so the Store, the broker and a WASM client read the same types.
//! The rules follow the compatibility charter (docs/development/CORE-COMPATIBILITY-DESIGN.md, sections 4 and 7): unknown fields are kept and
//! written back, unknown member kinds are data (shown as not supported, never an error that fails the other members), an old manifest without
//! any of these fields means the same as before, and `requires` is checked against the running core with a message that says which side
//! to update.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::appcaps::{self, CapabilityRequest};
use crate::wizard::{self, WizardSpec};

/// The manifest format this IYAKKI reads in full. A manifest without `manifest_version` is version 1 (what every older manifest is).
pub const MANIFEST_VERSION: u32 = 1;
/// The version of the core interface apps are written against, `major.minor`. Owner file for this number (charter, section 3).
pub const CORE_INTERFACE: &str = "0.1";

pub const MAX_MEMBERS: usize = 16;
pub const MAX_REQUIRES_NAMES: usize = 24;
pub const MAX_ID: usize = 48;
pub const MAX_FILE_PATH: usize = 100;
pub const MAX_TITLE: usize = 60;

// ------------------------------------------------------------------------------------------------ versions

pub type Triple = (u32, u32, u32);

/// `1`, `1.2` or `1.2.3`; missing parts are zero.
pub fn parse_version(s: &str) -> Option<Triple> {
    let parts: Vec<&str> = s.trim().split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut v = [0u32; 3];
    for (i, p) in parts.iter().enumerate() {
        if p.is_empty() || p.len() > 6 || !p.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        v[i] = p.parse().ok()?;
    }
    Some((v[0], v[1], v[2]))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Ge,
    Gt,
    Le,
    Lt,
    Eq,
}

/// A version range in words: comparators separated by spaces, all of which must hold (`>=0.3 <1`). A bare version means exactly that version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionRange {
    pub parts: Vec<(Op, Triple)>,
}

impl VersionRange {
    pub fn parse(s: &str) -> Result<VersionRange, String> {
        let mut parts = Vec::new();
        for tok in s.split_whitespace() {
            let (op, rest) = if let Some(r) = tok.strip_prefix(">=") {
                (Op::Ge, r)
            } else if let Some(r) = tok.strip_prefix("<=") {
                (Op::Le, r)
            } else if let Some(r) = tok.strip_prefix('>') {
                (Op::Gt, r)
            } else if let Some(r) = tok.strip_prefix('<') {
                (Op::Lt, r)
            } else if let Some(r) = tok.strip_prefix('=') {
                (Op::Eq, r)
            } else {
                (Op::Eq, tok)
            };
            let v = parse_version(rest).ok_or_else(|| format!("{tok:?} is not a version comparison like >=0.3 or <1"))?;
            parts.push((op, v));
        }
        if parts.is_empty() || parts.len() > 4 {
            return Err("a version range has one to four comparisons, like \">=0.3 <1\"".into());
        }
        Ok(VersionRange { parts })
    }

    fn holds(op: Op, v: Triple, bound: Triple) -> bool {
        match op {
            Op::Ge => v >= bound,
            Op::Gt => v > bound,
            Op::Le => v <= bound,
            Op::Lt => v < bound,
            Op::Eq => v == bound,
        }
    }

    pub fn satisfied_by(&self, v: Triple) -> bool {
        self.parts.iter().all(|(op, b)| Self::holds(*op, v, *b))
    }

    /// The first comparison that fails, if any.
    pub fn first_failure(&self, v: Triple) -> Option<(Op, Triple)> {
        self.parts.iter().find(|(op, b)| !Self::holds(*op, v, *b)).copied()
    }
}

// ------------------------------------------------------------------------------------------------ requires

/// `requires` of an app or of a bundle member.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requires {
    /// A range of the core interface version, like `>=0.3 <1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core_interface: Option<String>,
    /// Capabilities the core must have (the catalogue names); without one of them the app cannot work.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities_needed: Vec<String>,
    /// Capabilities the app uses when they exist; their absence only shrinks the app.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities_optional: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Which side has to change for the pair to work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateSide {
    /// This IYAKKI is too old for the app: "needs a newer IYAKKI".
    Core,
    /// The app was written for an interface this IYAKKI no longer has.
    App,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequiresRefusal {
    pub update: UpdateSide,
    /// Names the versions and the side to update; plain words for the card.
    pub message: String,
}

impl std::fmt::Display for RequiresRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

fn plain_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

/// Whether the `requires` is well formed (a range that reads, names that look like names, lists of a sensible size). Unknown capability
/// names are NOT an error here: they are what [`check_requires`] reports against the running core.
pub fn validate_requires(r: &Requires) -> Result<(), String> {
    if let Some(range) = &r.core_interface {
        VersionRange::parse(range).map_err(|e| format!("requires.core_interface: {e}"))?;
    }
    for (label, list) in [("capabilities_needed", &r.capabilities_needed), ("capabilities_optional", &r.capabilities_optional)] {
        if list.len() > MAX_REQUIRES_NAMES {
            return Err(format!("requires.{label}: at most {MAX_REQUIRES_NAMES} names"));
        }
        for (i, n) in list.iter().enumerate() {
            if !plain_name(n) {
                return Err(format!("requires.{label}: {n:?} is not a capability name"));
            }
            if list[..i].contains(n) {
                return Err(format!("requires.{label}: {n:?} is listed twice"));
            }
        }
    }
    Ok(())
}

/// Check `requires` against the running core: its interface version (`core`, like `"0.1"`) and the capability catalogue it has.
pub fn check_requires(r: &Requires, core: &str) -> Result<(), RequiresRefusal> {
    let cv = parse_version(core).unwrap_or((0, 0, 0));
    if let Some(range_text) = &r.core_interface {
        let Ok(range) = VersionRange::parse(range_text) else {
            return Err(RequiresRefusal { update: UpdateSide::App, message: format!("The app's core_interface range {range_text:?} cannot be read. Ask its publisher for a fixed version.") });
        };
        if let Some((op, _)) = range.first_failure(cv) {
            return Err(match op {
                Op::Ge | Op::Gt => RequiresRefusal { update: UpdateSide::Core, message: format!("Needs a newer IYAKKI: the app needs core interface {range_text}, this IYAKKI has {core}. Update IYAKKI, then install it.") },
                Op::Le | Op::Lt => RequiresRefusal { update: UpdateSide::App, message: format!("The app was made for core interface {range_text}, but this IYAKKI has {core}. Ask the publisher for a newer version of the app.") },
                Op::Eq => RequiresRefusal { update: if cv < range.parts.first().map_or((0, 0, 0), |p| p.1) { UpdateSide::Core } else { UpdateSide::App }, message: format!("The app needs core interface {range_text}, this IYAKKI has {core}. {}", "Update whichever side is older.") },
            });
        }
    }
    let missing: Vec<&str> = r.capabilities_needed.iter().filter(|n| appcaps::lookup(n).is_none()).map(String::as_str).collect();
    if !missing.is_empty() {
        return Err(RequiresRefusal { update: UpdateSide::Core, message: format!("Needs a newer IYAKKI: the app needs {} which this IYAKKI (core interface {core}) does not have. Update IYAKKI, then install it.", missing.join(", ")) });
    }
    Ok(())
}

/// Optional capabilities this IYAKKI lacks (shown as "works without").
pub fn missing_optional(r: &Requires) -> Vec<String> {
    r.capabilities_optional.iter().filter(|n| appcaps::lookup(n).is_none()).cloned().collect()
}

// ------------------------------------------------------------------------------------------------ members

/// The kind of a bundle member. Open: a kind this IYAKKI does not know is `Unknown` and stays in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemberKind {
    Skill,
    Flow,
    Tool,
    Mcp,
    Agent,
    Template,
    Theme,
    Extension,
    Tile,
    Unknown(String),
}

impl MemberKind {
    pub fn of(s: &str) -> MemberKind {
        match s {
            "skill" => MemberKind::Skill,
            "flow" => MemberKind::Flow,
            "tool" => MemberKind::Tool,
            "mcp" => MemberKind::Mcp,
            "agent" => MemberKind::Agent,
            "template" => MemberKind::Template,
            "theme" => MemberKind::Theme,
            "extension" => MemberKind::Extension,
            "tile" => MemberKind::Tile,
            other => MemberKind::Unknown(other.to_string()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            MemberKind::Skill => "skill",
            MemberKind::Flow => "flow",
            MemberKind::Tool => "tool",
            MemberKind::Mcp => "mcp",
            MemberKind::Agent => "agent",
            MemberKind::Template => "template",
            MemberKind::Theme => "theme",
            MemberKind::Extension => "extension",
            MemberKind::Tile => "tile",
            MemberKind::Unknown(s) => s,
        }
    }

    pub fn is_known(&self) -> bool {
        !matches!(self, MemberKind::Unknown(_))
    }
}

/// One entry of `contents`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    /// Names the member inside the bundle (`<bundle>/<member>` in the approval book).
    pub id: String,
    pub kind: String,
    /// A file inside the app's own files.
    pub file: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// An optional member that this IYAKKI cannot install is left out and the rest installs.
    #[serde(default, skip_serializing_if = "is_false")]
    pub optional: bool,
    /// The member's own capabilities, approved on their own; it never uses the app's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<CapabilityRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<Requires>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wizards: Vec<WizardSpec>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Member {
    pub fn kind(&self) -> MemberKind {
        MemberKind::of(&self.kind)
    }
}

/// A safe relative path inside an app: letters, digits and `. _ - /`, no `..`, no hidden parts, not absolute.
pub fn rel_path_ok(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= MAX_FILE_PATH
        && !p.starts_with('/')
        && p.split('/').all(|s| !s.is_empty() && s != "." && s != ".." && !s.starts_with('.'))
        && p.chars().all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c))
}

/// Lowercase letters, digits and dashes, at most 48 characters, not starting with a dash.
pub fn id_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_ID && !s.starts_with('-') && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Shape of the member list: how many, ids, paths, kinds that can be named, capabilities, requires and wizards. Unknown KINDS are fine.
pub fn validate_contents(members: &[Member]) -> Result<(), String> {
    if members.len() > MAX_MEMBERS {
        return Err(format!("contents: {} members, at most {MAX_MEMBERS}", members.len()));
    }
    for (i, m) in members.iter().enumerate() {
        let at = |why: &str| format!("contents[{i}] {:?}: {why}", m.id);
        if !id_ok(&m.id) {
            return Err(at("id is lowercase letters, digits and dashes, at most 48 characters"));
        }
        if members[..i].iter().any(|o| o.id == m.id) {
            return Err(at("the id is used twice"));
        }
        if !plain_name(&m.kind) {
            return Err(at("kind is a short plain word"));
        }
        if !rel_path_ok(&m.file) {
            return Err(at("file is a path inside the app without .., a leading / or hidden parts, letters digits . _ - / only, at most 100 characters"));
        }
        if members[..i].iter().any(|o| o.file == m.file) {
            return Err(at("the file is used by another member"));
        }
        if m.title.chars().count() > MAX_TITLE || m.title.chars().any(|c| c.is_control()) {
            return Err(at(&format!("title is at most {MAX_TITLE} characters")));
        }
        appcaps::validate_requests(&m.capabilities).map_err(|e| at(&e.to_string()))?;
        if let Some(r) = &m.requires {
            validate_requires(r).map_err(|e| at(&e))?;
        }
        let declared: Vec<&str> = m.capabilities.iter().map(|c| c.name.as_str()).collect();
        wizard::validate_wizards(&m.wizards, &declared).map_err(|e| at(&e))?;
    }
    Ok(())
}

/// Each member's file must exist and be at most `max_bytes`. `size_of` answers with the size of a file of the app, `None` when it is not there.
pub fn validate_member_files(members: &[Member], size_of: &dyn Fn(&str) -> Option<usize>, max_bytes: usize) -> Result<(), String> {
    for m in members {
        match size_of(&m.file) {
            None => return Err(format!("contents {:?}: the file {} is not in the app", m.id, m.file)),
            Some(n) if n > max_bytes => return Err(format!("contents {:?}: {} is larger than {} KB", m.id, m.file, max_bytes / 1024)),
            Some(_) => {}
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------ the manifest extension

/// The fields of `app.json` that this module owns, plus every field it does not know (kept, written back).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManifestExt {
    /// `None` when the manifest does not say (an older one): it is read as version 1.
    pub manifest_version: Option<u32>,
    pub requires: Option<Requires>,
    pub contents: Vec<Member>,
    pub wizards: Vec<WizardSpec>,
    /// Fields nobody here knows. A manifest of a newer version keeps `contents` and `wizards` here too, unread.
    pub extra: BTreeMap<String, Value>,
}

const OWN: [&str; 4] = ["manifest_version", "requires", "contents", "wizards"];

impl ManifestExt {
    /// Read the fields from the manifest object. `known` are the fields the caller reads itself (`id`, `name`, ...). Nothing is validated beyond what
    /// the types need; call [`ManifestExt::validate`].
    pub fn from_object(obj: &serde_json::Map<String, Value>, known: &[&str]) -> Result<ManifestExt, String> {
        let mut e = ManifestExt::default();
        e.manifest_version = match obj.get("manifest_version") {
            None => None,
            Some(v) => Some(v.as_u64().filter(|n| *n >= 1 && *n <= u64::from(u32::MAX)).ok_or("manifest_version is a whole number from 1")? as u32),
        };
        let newer = e.effective_version() > MANIFEST_VERSION;
        for (k, v) in obj {
            if known.contains(&k.as_str()) {
                continue;
            }
            match k.as_str() {
                "manifest_version" => {}
                // a manifest of a newer version may mean something else by these: keep them unread, the Store refuses the install and says why
                "requires" | "contents" | "wizards" if newer => {
                    e.extra.insert(k.clone(), v.clone());
                }
                "requires" => e.requires = Some(serde_json::from_value(v.clone()).map_err(|er| format!("requires: {er}"))?),
                "contents" => e.contents = serde_json::from_value(v.clone()).map_err(|er| format!("contents: {er}"))?,
                "wizards" => e.wizards = serde_json::from_value(v.clone()).map_err(|er| format!("wizards: {er}"))?,
                _ => {
                    e.extra.insert(k.clone(), v.clone());
                }
            }
        }
        Ok(e)
    }

    pub fn effective_version(&self) -> u32 {
        self.manifest_version.unwrap_or(1)
    }

    /// The manifest is from a format this IYAKKI cannot read in full.
    pub fn is_newer(&self) -> bool {
        self.effective_version() > MANIFEST_VERSION
    }

    /// Validate what this IYAKKI understands. `declared` are the capabilities the app itself declared (wizard checks may only use those).
    /// A newer manifest is not validated (its rules are not ours); [`ManifestExt::check_version`] refuses it.
    pub fn validate(&self, declared: &[CapabilityRequest]) -> Result<(), String> {
        if self.is_newer() {
            return Ok(());
        }
        if let Some(r) = &self.requires {
            validate_requires(r)?;
        }
        validate_contents(&self.contents)?;
        let names: Vec<&str> = declared.iter().map(|c| c.name.as_str()).collect();
        wizard::validate_wizards(&self.wizards, &names)
    }

    /// The manifest-version part of the Store's check: refuses a manifest of a newer format and says which side to update.
    pub fn check_version(&self) -> Result<(), RequiresRefusal> {
        if self.is_newer() {
            return Err(RequiresRefusal { update: UpdateSide::Core, message: format!("Needs a newer IYAKKI: the app's manifest is version {}, this IYAKKI reads up to version {MANIFEST_VERSION}. Update IYAKKI, then install it.", self.effective_version()) });
        }
        Ok(())
    }

    /// The whole `requires` check for the app: manifest version, then the interface range and capabilities.
    pub fn check_core(&self, core: &str) -> Result<(), RequiresRefusal> {
        self.check_version()?;
        match &self.requires {
            Some(r) => check_requires(r, core),
            None => Ok(()),
        }
    }

    /// Write the fields back into the manifest object (only those that are present, so an old manifest stays as it was).
    pub fn write_into(&self, out: &mut serde_json::Map<String, Value>) {
        if let Some(v) = self.manifest_version {
            out.insert("manifest_version".into(), Value::from(v));
        }
        if let Some(r) = &self.requires {
            out.insert("requires".into(), serde_json::to_value(r).unwrap_or(Value::Null));
        }
        if !self.contents.is_empty() {
            out.insert("contents".into(), serde_json::to_value(&self.contents).unwrap_or(Value::Null));
        }
        if !self.wizards.is_empty() {
            out.insert("wizards".into(), serde_json::to_value(&self.wizards).unwrap_or(Value::Null));
        }
        for (k, v) in &self.extra {
            if !OWN.contains(&k.as_str()) || self.is_newer() {
                out.insert(k.clone(), v.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests;
