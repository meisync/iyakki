//! Core compatibility, step 4 of the charter (docs/development/CORE-COMPATIBILITY-DESIGN.md sections 5, 6 and 8 item 8; F-CORE-19): the
//! `deprecations` list that both `hello` answers carry, the side that must update when two versions cannot talk, and the compat report
//! (versions of the core and of every interface, the versioned surfaces in use, connected peers, deprecated names seen). Pure data and
//! text: no IO. The table of deprecated names is `docs/development/deprecations.txt`, the one the gate reads.

use serde::{Deserialize, Serialize};

/// The deprecation table, as the gate reads it (tab separated: name, kind, since, not_before, alias_test, note).
pub const DEPRECATION_TABLE: &str = include_str!("../../../docs/development/deprecations.txt");
/// The inventory of versioned surfaces (`compat-surfaces.txt`): kind, id, file, value, class, note.
pub const SURFACE_TABLE: &str = include_str!("../../../docs/development/compat-surfaces.txt");

/// One deprecated name, as `hello` announces it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deprecation {
    /// The deprecated name as written in the code or the file.
    pub name: String,
    /// `capability`, `call`, `field`, `message`, `file` or `other`.
    pub kind: String,
    /// The release that announced it.
    pub since: String,
    /// The first day it may be removed (`YYYY-MM-DD`).
    pub not_before: String,
    /// What replaces it.
    pub replacement: String,
}

/// Which side must be updated when two versions cannot talk (the charter: the refusal names it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateSide {
    /// The client (or app, or helper) is too old for this machine.
    Client,
    /// The machine is too old for this client.
    Machine,
}

impl UpdateSide {
    pub fn word(self) -> &'static str {
        match self {
            UpdateSide::Client => "client",
            UpdateSide::Machine => "machine",
        }
    }
}

/// The deprecated names of a table text (comment lines and blank lines skipped, a malformed row skipped: the gate refuses it before release).
pub fn parse_deprecations(table: &str) -> Vec<Deprecation> {
    table
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f.len() >= 5).then(|| Deprecation { name: f[0].into(), kind: f[1].into(), since: f[2].into(), not_before: f[3].into(), replacement: f.get(5).copied().unwrap_or("").into() })
        })
        .collect()
}

/// What this build announces in `hello`: the rows of the committed table.
pub fn deprecations() -> Vec<Deprecation> {
    parse_deprecations(DEPRECATION_TABLE)
}

/// One versioned surface of the inventory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Surface {
    /// `spec` (a store file's schema number), `const` or `manual`.
    pub kind: String,
    pub id: String,
    pub value: String,
    /// The charter's surface letter (A Rust interface, C wire protocol, D data on disk, E packages, F the system, G clients).
    pub letter: String,
}

/// The versioned surfaces that are promises to other releases (class `surface`), from a table text.
pub fn parse_surfaces(table: &str) -> Vec<Surface> {
    table
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 6 || f[4] != "surface" {
                return None;
            }
            let letter: String = f[5].chars().next().filter(|c| c.is_ascii_uppercase()).map(String::from).unwrap_or_default();
            Some(Surface { kind: f[0].into(), id: f[1].into(), value: f[3].trim_matches('"').into(), letter })
        })
        .collect()
}

/// A connected peer: another computer or client talking to this core.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    /// `computer` (a paired developer computer), `client` (a client session), `helper` (a process of this machine).
    pub kind: String,
    pub name: String,
    /// What it said about itself (a protocol or build version), empty when it did not say.
    pub version: String,
    /// Seconds since it was last heard, `None` when unknown.
    pub idle_secs: Option<u64>,
}

/// The compat report: the one place that answers "which versions met".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// The release of this core (`0.1.0-alpha.3`).
    pub core: String,
    /// The typed contract: version and the oldest minor still served.
    pub contract: String,
    /// The client session protocol: oldest and newest version served.
    pub client_protocol: String,
    pub surfaces: Vec<Surface>,
    pub peers: Vec<Peer>,
    pub deprecations: Vec<Deprecation>,
    /// Deprecated names that were used since start, with how many times.
    pub deprecated_seen: Vec<(String, u64)>,
}

impl Report {
    /// Build the report from what the caller knows. `surfaces` and `deprecations` come from the committed tables unless given.
    pub fn new(core: &str, peers: Vec<Peer>, deprecated_seen: Vec<(String, u64)>) -> Report {
        let c = crate::contract::CONTRACT_VERSION;
        Report {
            core: core.to_string(),
            contract: format!("{}.{} (serves from {}.{})", c.major, c.minor, c.major, crate::contract::catalog::OLDEST_MINOR),
            client_protocol: format!("{} to {}", crate::client::MIN_PROTOCOL_VERSION, crate::client::PROTOCOL_VERSION),
            surfaces: parse_surfaces(SURFACE_TABLE),
            peers,
            deprecations: deprecations(),
            deprecated_seen,
        }
    }

    /// Plain lines for `dev info`, Settings and the support bundle. `peers` are shown only when `admin` (the charter proposes: only to
    /// administrators); nothing here is a secret: versions, names of computers, counts.
    pub fn lines(&self, admin: bool) -> Vec<String> {
        let mut v = vec![format!("core {} · contract {} · client protocol {}", self.core, self.contract, self.client_protocol)];
        let by = |letter: &str| -> Vec<String> { self.surfaces.iter().filter(|s| s.letter == letter).map(|s| format!("{} {}", s.id, s.value)).collect() };
        for (letter, title) in [("A", "Rust interface"), ("B", "C and tile interface"), ("C", "wire protocols"), ("D", "data on disk"), ("E", "packages and manifests"), ("F", "the system"), ("G", "clients")] {
            let items = by(letter);
            if !items.is_empty() {
                v.push(format!("{title}: {} versioned ({})", items.len(), items.iter().take(6).cloned().collect::<Vec<_>>().join(", ") + if items.len() > 6 { ", ..." } else { "" }));
            }
        }
        if admin {
            if self.peers.is_empty() {
                v.push("peers: none connected".to_string());
            }
            for p in &self.peers {
                v.push(format!("peer {} {}{}{}", p.kind, p.name, if p.version.is_empty() { String::new() } else { format!(" · {}", p.version) }, p.idle_secs.map_or(String::new(), |s| format!(" · heard {s} s ago"))));
            }
        } else {
            v.push(format!("peers: {} (names are for administrators)", self.peers.len()));
        }
        if self.deprecations.is_empty() {
            v.push("deprecated names: none".to_string());
        }
        for d in &self.deprecations {
            let n = self.deprecated_seen.iter().find(|(x, _)| *x == d.name).map_or(0, |x| x.1);
            v.push(format!("deprecated {} {} since {}, removable from {}{} · used {n} time(s)", d.kind, d.name, d.since, d.not_before, if d.replacement.is_empty() { String::new() } else { format!(" · use {}", d.replacement) }));
        }
        for (name, n) in &self.deprecated_seen {
            if !self.deprecations.iter().any(|d| &d.name == name) {
                v.push(format!("deprecated name seen but not in the table: {name} ({n})"));
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_rows_become_deprecations_and_comments_and_bad_rows_are_skipped() {
        let t = "# comment\n\nold_call\tcall\t0.1.0-alpha.3\t2027-02-01\tcrate::t::alias\tuse new_call\nbroken row\nold_cap\tcapability\t0.1.0-alpha.3\t2027-03-01\tcrate::t::alias2\n";
        let d = parse_deprecations(t);
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].name.as_str(), d[0].kind.as_str(), d[0].not_before.as_str(), d[0].replacement.as_str()), ("old_call", "call", "2027-02-01", "use new_call"));
        assert_eq!(d[1].replacement, "");
    }

    #[test]
    fn the_surfaces_are_the_promises_with_their_charter_letter() {
        let t = "const\tCONTRACT_VERSION\tf.rs\tContractVersion { major: 0, minor: 1 }\tsurface\tC typed contract hello\nconst\tDM_VERSION\tf.rs\t[4, 0, 0]\tnot-ours\tkernel number\nspec\tsettings\tf.rs\t3\tsurface\tD settings file\n";
        let s = parse_surfaces(t);
        assert_eq!(s.iter().map(|x| (x.id.as_str(), x.letter.as_str())).collect::<Vec<_>>(), vec![("CONTRACT_VERSION", "C"), ("settings", "D")]);
        // the committed inventory has surfaces of the main kinds
        let real = parse_surfaces(SURFACE_TABLE);
        assert!(real.len() > 30 && real.iter().any(|x| x.letter == "C") && real.iter().any(|x| x.letter == "D"));
    }

    #[test]
    fn the_report_says_versions_peers_for_administrators_only_and_every_deprecated_name_with_its_use() {
        let peers = vec![Peer { kind: "computer".into(), name: "dev-laptop".into(), version: "devmode".into(), idle_secs: Some(12) }];
        let mut r = Report::new("0.1.0-alpha.3", peers, vec![("old_call".into(), 4), ("stray".into(), 1)]);
        r.deprecations = parse_deprecations("old_call\tcall\t0.1.0-alpha.3\t2027-02-01\tcrate::t::a\tnew_call\n");
        let admin = r.lines(true).join("\n");
        assert!(admin.contains("core 0.1.0-alpha.3") && admin.contains("contract 0.3") && admin.contains("client protocol 1 to 1"), "{admin}");
        assert!(admin.contains("peer computer dev-laptop · devmode · heard 12 s ago"), "{admin}");
        assert!(admin.contains("deprecated call old_call since 0.1.0-alpha.3, removable from 2027-02-01 · use new_call · used 4 time(s)"), "{admin}");
        assert!(admin.contains("deprecated name seen but not in the table: stray (1)"), "{admin}");
        let plain = r.lines(false).join("\n");
        assert!(!plain.contains("dev-laptop") && plain.contains("peers: 1 (names are for administrators)"), "{plain}");
        // nothing deprecated: said plainly
        let mut r2 = Report::new("x", vec![], vec![]);
        assert!(r2.deprecations.iter().any(|d| d.name == "stream.chunk.data_as_numbers"), "the committed table is in every report");
        r2.deprecations.clear();
        assert!(r2.lines(true).iter().any(|l| l == "peers: none connected") && r2.lines(true).iter().any(|l| l == "deprecated names: none"));
    }
}
