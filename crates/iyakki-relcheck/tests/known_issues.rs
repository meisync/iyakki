//! KNOWN-ISSUES.md structure (iyakki-relcheck known-issues): ids unique, one class each, Closes by and Row fields, rows exist,
//! cited ids resolve, and the UNTRIAGED / dangling ratchet (docs/development/known-issues-ratchet.txt) only goes down.

use std::path::Path;

#[test]
fn known_issues_follow_the_structure_and_the_ratchet() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let r = iyakki_relcheck::knownissues::check_repo(&root);
    println!("known-issues: {} ({} fixed)", r.counts.line(), r.fixed);
    assert!(r.problems.is_empty(), "{}", r.problems.join("\n"));
}
