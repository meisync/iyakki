//! F-LIFE-20 (lifecycle audit P-14): the files that keep a person's persistent state are written with
//! `iyakki_schema::write_atomic` (temp file, fsync, rename), never with a plain `std::fs::write` or a temp-and-rename without fsync.
//! The scan reads the production part of each file (everything before `#[cfg(test)]`).

const FILES: &[&str] = &[
    "iyakki-shared/src/bundle_members.rs",
    "iyakki-shared/src/quickinstall_ollama.rs",
    "iyakki-shared/src/lockscreen.rs",
    "iyakki-shared/src/media.rs",
    "iyakki-shared/src/browserplugins.rs",
    "iyakki-shared/src/webaccess.rs",
    "iyakki-agent/src/fleet_context.rs",
    "iyakki-agent/src/localreg.rs",
    "iyakki-agent/src/embedbrowser.rs",
    "iyakki-agent/src/datamigrate.rs",
];

#[test]
fn persistent_state_files_are_written_through_the_atomic_writer() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut bad = Vec::new();
    for f in FILES {
        let text = std::fs::read_to_string(root.join(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
        let prod = text.split("#[cfg(test)]").next().unwrap_or("");
        for (i, line) in prod.lines().enumerate() {
            if line.contains("fs::write(") || (line.contains("fs::rename(") && !line.contains("remove")) {
                bad.push(format!("{f}:{}: {}", i + 1, line.trim()));
            }
        }
    }
    // datamigrate renames whole old files aside on purpose (a move, not a write of state)
    bad.retain(|l| !l.starts_with("iyakki-agent/src/datamigrate.rs") || !l.contains("rename(&a, &b)"));
    assert!(bad.is_empty(), "plain writers of persistent state:\n{}", bad.join("\n"));
}
