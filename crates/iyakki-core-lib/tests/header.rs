//! The C header of the library is made from the list the c-abi gate keeps (dist/release/make-core-header.sh) and the file in the crate is
//! the current one; every function in the list for this crate is really exported by the library.

use std::path::Path;

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn the_committed_header_is_what_the_script_makes_from_the_gate_list() {
    let script = root().join("dist/release/make-core-header.sh");
    let header = Path::new(env!("CARGO_MANIFEST_DIR")).join("include/iyakki_core.h");
    let r = std::process::Command::new("sh").arg(&script).arg("--check").arg(&header).status().unwrap();
    assert!(r.success(), "include/iyakki_core.h is out of date: run `sh dist/release/make-core-header.sh > crates/iyakki-core-lib/include/iyakki_core.h`");
    let text = std::fs::read_to_string(&header).unwrap();
    assert!(text.contains("iyakki_core_abi_version") && text.contains("iyakki_core_hello"));
}

#[test]
fn every_function_the_gate_lists_for_the_library_is_exported_and_the_version_function_exists() {
    let list = std::fs::read_to_string(root().join("docs/development/c-abi-symbols.txt")).unwrap();
    let syms: Vec<&str> = list.lines().filter(|l| l.starts_with("iyakki-core-lib\t")).filter_map(|l| l.split('\t').nth(1)).collect();
    assert!(syms.iter().any(|s| s.ends_with("_abi_version")), "{syms:?}");
    let src = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs")).unwrap();
    for s in syms {
        assert!(src.contains(&format!("extern \"C\" fn {s}(")), "{s} is listed but not exported");
    }
}
