//! Ties the licence inventory `legal/downloads.txt` to the code, so it cannot drift (release gate point 7, docs/project/BACKLOG.md):
//!   * every Quick install item and every command-line tool of the CLI tile has a row, and no row is left over,
//!   * the pinned digest / version of a row is really in the source of the item,
//!   * a tool without a checksum in `clis.rs` is not described as checksum-verified,
//!   * every item shows a licence string, and a copyleft or vendor-licensed one says that it is downloaded or pulled, never shipped,
//!   * every built-in store catalogue entry shows a licence string, except the ones listed in `legal/catalogue-gaps.txt` (known, dated).
//! The file-level rules (licence known, delivery download-only, pin kind valid) are checked by `iyakki-relcheck licence-check`.

use std::path::PathBuf;

use iyakki_agent::clis::CATALOG;
use iyakki_agent::quickinstall::Item;
use iyakki_agent::store::builtin;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Row {
    id: String,
    license: String,
    pin_kind: String,
    pin: String,
}

fn rows() -> Vec<Row> {
    let text = std::fs::read_to_string(repo().join("legal/downloads.txt")).expect("legal/downloads.txt exists");
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let f: Vec<&str> = l.split('|').map(str::trim).collect();
            assert_eq!(f.len(), 7, "bad row: {l}");
            Row { id: f[0].into(), license: f[1].into(), pin_kind: f[3].into(), pin: f[4].into() }
        })
        .collect()
}

/// `legal/catalogue-gaps.txt`: known places where the screen does not show what the gate wants, one row each (id | reason | date).
fn listed_gaps() -> Vec<String> {
    let text = std::fs::read_to_string(repo().join("legal/catalogue-gaps.txt")).unwrap_or_default();
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let f: Vec<&str> = l.split('|').map(str::trim).collect();
            assert!(f.len() == 3 && f[1].len() >= 12 && f[2].len() == 10, "bad catalogue-gaps row: {l}");
            f[0].to_string()
        })
        .collect()
}

/// The source files that hold the pins.
fn sources() -> String {
    let dir = repo().join("crates/iyakki-agent/src");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".rs") && !n.ends_with("_tests.rs") && ["quickinstall", "gpu_", "voicepack", "fwextra", "regsetup", "clis"].iter().any(|k| n.starts_with(k)))).collect();
    files.sort();
    files.iter().map(|f| std::fs::read_to_string(f).unwrap_or_default()).collect::<Vec<_>>().join("\n")
}

#[test]
fn every_quick_install_item_and_cli_has_an_inventory_row_and_no_row_is_left_over() {
    let rows = rows();
    let mut want: Vec<String> = Item::ALL.iter().map(|i| i.id().to_string()).collect();
    want.extend(CATALOG.iter().map(|d| format!("cli:{}", d.id)));
    for w in &want {
        assert!(rows.iter().any(|r| &r.id == w), "legal/downloads.txt has no row for `{w}` (add its licence, delivery and pin)");
    }
    for r in &rows {
        assert!(want.contains(&r.id), "legal/downloads.txt row `{}` matches no Quick install item or CLI any more", r.id);
    }
    // the items the owner named
    for must in ["clickhouse", "gitea", "graph-db", "prometheus"] {
        assert!(rows.iter().any(|r| r.id == must), "{must}");
    }
}

#[test]
fn pins_in_the_inventory_are_the_pins_in_the_code() {
    let src = sources();
    for r in rows() {
        match r.pin_kind.as_str() {
            "digest" => {
                let hex = r.pin.trim_start_matches("sha256:");
                assert!(src.contains(hex), "`{}`: digest {hex} is not in the source any more (the pin changed: re-verify and update legal/downloads.txt)", r.id);
            }
            "version" => assert!(src.contains(&r.pin), "`{}`: version pin `{}` is not in the source any more", r.id, r.pin),
            _ => {}
        }
    }
}

#[test]
fn a_tool_without_a_checksum_is_not_listed_as_verified() {
    let rows = rows();
    for d in CATALOG {
        let r = rows.iter().find(|r| r.id == format!("cli:{}", d.id)).unwrap_or_else(|| panic!("cli:{} has no row", d.id));
        let system = matches!(d.source, iyakki_agent::clis::Source::System);
        if let Some(iyakki_agent::clis::Checksum::Fixed(_)) = d.checksum {
            assert!(!d.pin.is_empty(), "cli:{}: a fixed checksum belongs to one pinned version", d.id);
        }
        match (d.checksum.is_some(), system) {
            (true, _) => assert!(matches!(r.pin_kind.as_str(), "checksum" | "digest" | "version" | "commit"), "cli:{} has a checksum in clis.rs, the row says `{}`", d.id, r.pin_kind),
            (false, false) => {
                // every tool without a vendor checksum is pinned to an exact version in clis.rs and its screen warns (integrity_warning)
                assert!(!d.pin.is_empty(), "cli:{} has no checksum and no pinned version in clis.rs", d.id);
                assert!(d.integrity_warning().is_some_and(|w| w.starts_with("Warning")), "cli:{}", d.id);
                assert_eq!(r.pin_kind, "none-vendor", "cli:{} has NO vendor checksum; the row must say none-vendor so it is listed for review", d.id);
                assert_eq!(r.pin, d.pin, "cli:{}: the row's pinned version is not the one in clis.rs", d.id);
            }
            (false, true) => assert_eq!(r.pin_kind, "n/a", "cli:{} comes with the system", d.id),
        }
    }
}

#[test]
fn every_item_shows_a_licence_and_the_restricted_ones_say_they_are_not_shipped() {
    let rows = rows();
    let says_not_shipped = |t: &str| {
        let t = t.to_lowercase();
        ["not ship", "never ship", "downloaded", "pulls the image", "pulled", "fetched", "you supply", "nothing is installed", "not bundled", "no gpu driver code of its own", "does not ship"].iter().any(|k| t.contains(k))
    };
    let mut silent: Vec<String> = Vec::new();
    for item in Item::ALL {
        let text = item.licence();
        assert!(!text.trim().is_empty(), "{} shows no licence string", item.id());
        let shown = format!("{text} {}", item.about());
        let row = rows.iter().find(|r| r.id == item.id()).unwrap();
        let l = row.license.to_lowercase();
        let restricted = ["gpl", "sspl", "busl", "elastic", "licenseref-anthropic", "licenseref-github", "licenseref-apple", "licenseref-nvidia", "licenseref-ai4bharat", "licenseref-linux-firmware", "licenseref-debian"].iter().any(|k| l.contains(k));
        if restricted && !says_not_shipped(&shown) {
            silent.push(format!("qi:{}", item.id()));
        }
    }
    let listed = listed_gaps();
    let unlisted: Vec<&String> = silent.iter().filter(|g| !listed.contains(g)).collect();
    assert!(unlisted.is_empty(), "Quick install items with a copyleft or vendor licence whose licence text does not say they are downloaded / pulled and not shipped, and are not in legal/catalogue-gaps.txt: {unlisted:?}");
    for g in listed.iter().filter(|g| g.starts_with("qi:")) {
        assert!(silent.contains(g), "legal/catalogue-gaps.txt lists `{g}` but its text says it is not shipped now (remove the row)");
    }
    // the planned and new items by name
    for (id, needle) in [("graph-db", "GPL-3.0"), ("gitea", "MIT"), ("clickhouse", "Apache"), ("prometheus", "Apache-2.0")] {
        let item = Item::ALL.iter().find(|i| i.id() == id).unwrap();
        assert!(item.licence().contains(needle), "{id}: {}", item.licence());
    }
}

#[test]
fn built_in_store_entries_show_a_licence_except_the_listed_gaps() {
    let listed: Vec<String> = listed_gaps().into_iter().filter(|g| !g.starts_with("qi:")).collect();
    let mut gaps: Vec<String> = Vec::new();
    for b in builtin::items() {
        let e = &b.entry;
        if e.demo {
            // made-up paid apps: shown as DEMO, no file, never installable
            assert!(e.sha256.is_empty() && b.package.is_none(), "{}: a demo entry must carry no file", e.id);
            continue;
        }
        if e.license.trim().is_empty() || e.license.trim() == "-" {
            gaps.push(e.id.clone());
        } else if b.package.is_some() {
            // an entry with a file shows a version and a digest of the file
            assert!(!e.version.is_empty() && e.sha256.len() == 64, "{}: version/digest missing", e.id);
        }
    }
    let unlisted: Vec<&String> = gaps.iter().filter(|g| !listed.contains(g)).collect();
    assert!(unlisted.is_empty(), "store entries that show no licence and are not listed in legal/catalogue-gaps.txt: {unlisted:?}");
    for id in &listed {
        assert!(gaps.contains(id), "legal/catalogue-gaps.txt lists `{id}` but it shows a licence now (remove the row)");
    }
}

#[test]
fn nothing_in_the_catalogue_is_a_shipped_file_with_a_restricted_licence() {
    for b in builtin::items() {
        let l = b.entry.license.to_lowercase();
        if b.package.is_some() {
            assert!(!l.contains("gpl") && !l.contains("proprietary") && !l.contains("sspl"), "{}: a package built into the image carries `{}`", b.entry.id, b.entry.license);
        }
    }
}
