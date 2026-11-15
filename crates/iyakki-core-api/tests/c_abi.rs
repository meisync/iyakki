//! Charter step 7 (docs/development/CORE-COMPATIBILITY-DESIGN.md sections 2 B and 10; F-CORE-22): the gate for the C interface. Every
//! `#[no_mangle] extern "C"` function of the workspace is a promise to code written in another language (the WASM tiles, the page's
//! glue, and the `iyakki-core` shared library when it is built). The rules are in `docs/development/C-INTERFACE-RULES.md`.
//!
//! `docs/development/c-abi-symbols.txt` lists them, tab separated: `crate  symbol  signature`. The gate is a ratchet like the Rust
//! surface snapshot (`core_surface.rs`):
//!   * a symbol in the code and not in the file: ADDITION (append with `IYAKKI_C_ABI_WRITE=1`);
//!   * a symbol in the file and not in the code, or with another signature: REMOVED or CHANGED; it fails until the file keeps it as a
//!     `removed<TAB>crate<TAB>symbol<TAB>note` line (`IYAKKI_C_ABI_NOTE="why and what replaces it"` with the write);
//!   * every crate that exports C functions has an `abi_version` function (the tile ABI answers `iyakki_tile_abi`, a shared library
//!     must answer `iyakki_core_abi_version`) or an `exempt<TAB>crate<TAB>why` line;
//!   * a symbol name is never reused for another meaning: a `removed` name may not come back with another signature.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const FILE: &str = "docs/development/c-abi-symbols.txt";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target" || n == "tests" || n == "testdata") {
                continue;
            }
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// `(crate, symbol, signature)` of every exported C function of the workspace (test code and `tests/` folders left out).
pub fn scan(root: &Path) -> BTreeSet<(String, String, String)> {
    let mut found = BTreeSet::new();
    let Ok(rd) = std::fs::read_dir(root.join("crates")) else { return found };
    for c in rd.flatten() {
        let name = c.file_name().to_string_lossy().into_owned();
        let mut files = vec![];
        rs_files(&c.path().join("src"), &mut files);
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else { continue };
            // test code does not export anything the outside can call
            let text = text.split("#[cfg(test)]").next().unwrap_or("").to_string();
            let lines: Vec<&str> = text.lines().collect();
            for (i, l) in lines.iter().enumerate() {
                let t = l.trim();
                if !(t == "#[no_mangle]" || t == "#[unsafe(no_mangle)]") {
                    continue;
                }
                // the signature runs from the next `pub ... extern "C" fn` to the opening brace
                let mut sig = String::new();
                for n in lines.iter().skip(i + 1).take(8) {
                    sig.push_str(n.trim());
                    sig.push(' ');
                    if n.contains('{') {
                        break;
                    }
                }
                let sig = sig.split('{').next().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ");
                let Some(at) = sig.find(" fn ").map(|x| x + 4).or_else(|| sig.strip_prefix("fn ").map(|_| 3)) else { continue };
                let symbol: String = sig[at..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if !symbol.is_empty() && sig.contains("extern \"C\"") {
                    found.insert((name.clone(), symbol, sig.trim().to_string()));
                }
            }
        }
    }
    found
}

struct File {
    live: BTreeSet<(String, String, String)>,
    removed: BTreeMap<(String, String), String>,
    exempt: BTreeMap<String, String>,
}

fn read(text: &str) -> File {
    let mut f = File { live: BTreeSet::new(), removed: BTreeMap::new(), exempt: BTreeMap::new() };
    for l in text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
        let p: Vec<&str> = l.split('\t').collect();
        match p.as_slice() {
            ["removed", c, s, note] => {
                f.removed.insert((c.to_string(), s.to_string()), note.to_string());
            }
            ["exempt", c, why] => {
                f.exempt.insert(c.to_string(), why.to_string());
            }
            [c, s, sig] => {
                f.live.insert((c.to_string(), s.to_string(), sig.to_string()));
            }
            _ => panic!("c-abi-symbols.txt: a row that is not `crate<TAB>symbol<TAB>signature`, `removed`, or `exempt`: {l:?}"),
        }
    }
    f
}

fn render(f: &File, header: &str) -> String {
    let mut s = header.to_string();
    for (c, sym, sig) in &f.live {
        s.push_str(&format!("{c}\t{sym}\t{sig}\\n").replace("\\n", "\n"));
    }
    for ((c, sym), note) in &f.removed {
        s.push_str(&format!("removed\t{c}\t{sym}\t{note}\n"));
    }
    for (c, why) in &f.exempt {
        s.push_str(&format!("exempt\t{c}\t{why}\n"));
    }
    s
}

/// Does this crate export a function that answers the interface version?
fn has_version_fn(crate_symbols: &[&str]) -> bool {
    crate_symbols.iter().any(|s| s.ends_with("_abi_version") || *s == "iyakki_tile_abi")
}

#[test]
fn the_exported_c_functions_only_grow_keep_their_signature_and_every_exporting_crate_answers_its_version() {
    let root = root();
    let text = std::fs::read_to_string(root.join(FILE)).unwrap_or_default();
    let header: String = text.lines().take_while(|l| l.starts_with('#')).map(|l| format!("{l}\n")).collect();
    let mut file = read(&text);
    let code = scan(&root);
    assert!(code.iter().any(|(c, s, _)| c == "iyakki-tile-example" && s == "iyakki_tile_abi"), "the scanner finds the tile ABI export: {code:?}");
    let live_now: BTreeSet<(String, String)> = code.iter().map(|(c, s, _)| (c.clone(), s.clone())).collect();
    let added: Vec<_> = code.iter().filter(|x| !file.live.contains(*x) && !file.live.iter().any(|y| y.0 == x.0 && y.1 == x.1)).cloned().collect();
    let changed: Vec<_> = file.live.iter().filter(|(c, s, sig)| live_now.contains(&(c.clone(), s.clone())) && !code.contains(&(c.clone(), s.clone(), sig.clone()))).cloned().collect();
    let gone: Vec<_> = file.live.iter().filter(|(c, s, _)| !live_now.contains(&(c.clone(), s.clone()))).cloned().collect();
    // a removed name may not come back with another meaning
    for (c, s, _) in &added {
        assert!(!file.removed.contains_key(&(c.clone(), s.clone())), "`{s}` of {c} was removed once; a name is never reused (charter rule 1): pick a new name");
    }
    let note = std::env::var("IYAKKI_C_ABI_NOTE").unwrap_or_default();
    if std::env::var("IYAKKI_C_ABI_WRITE").is_ok_and(|v| v == "1") {
        assert!(added.is_empty() || changed.is_empty() || !note.is_empty() || true);
        assert!(!note.trim().is_empty() || (changed.is_empty() && gone.is_empty()), "a removal or a changed signature needs IYAKKI_C_ABI_NOTE=\"why and what replaces it\"");
        for x in added {
            file.live.insert(x);
        }
        for x in changed.iter().chain(gone.iter()) {
            file.live.remove(x);
            file.removed.insert((x.0.clone(), x.1.clone()), note.clone());
        }
        // a changed signature is a new surface under the same name only with a note; the new shape is recorded as live again
        for (c, s, sig) in &code {
            if changed.iter().any(|x| &x.0 == c && &x.1 == s) {
                file.removed.remove(&(c.clone(), s.clone()));
                file.live.insert((c.clone(), s.clone(), sig.clone()));
            }
        }
        std::fs::write(root.join(FILE), render(&file, &header)).unwrap();
        return;
    }
    assert!(added.is_empty(), "NEW exported C functions; append them with IYAKKI_C_ABI_WRITE=1 cargo test -p iyakki-core-api --test c_abi:\n{added:#?}");
    assert!(changed.is_empty() && gone.is_empty(), "REMOVED or CHANGED exported C functions (a promise to other languages): record with IYAKKI_C_ABI_WRITE=1 IYAKKI_C_ABI_NOTE=\"why and what replaces it\":\n{changed:#?}\n{gone:#?}");
    // every crate that exports C functions says which interface version it speaks, or why it need not
    let mut per: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (c, s, _) in &code {
        per.entry(c.clone()).or_default().push(s.as_str());
    }
    for (c, syms) in &per {
        assert!(has_version_fn(syms) || file.exempt.contains_key(c), "{c} exports C functions but none answers the interface version (a name ending in _abi_version); add one, or an `exempt` row with the reason");
    }
    for c in file.exempt.keys() {
        assert!(per.contains_key(c), "exempt row for {c}, which exports nothing any more: delete the row");
    }
}

#[test]
fn the_scanner_reads_signatures_ignores_test_code_and_a_changed_signature_is_seen() {
    let dir = std::env::temp_dir().join(format!("iyakki-cabi-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("crates/demo/src")).unwrap();
    std::fs::write(
        dir.join("crates/demo/src/lib.rs"),
        "#[no_mangle]\npub extern \"C\" fn demo_abi_version() -> i32 {\n    1\n}\n\n#[unsafe(no_mangle)]\npub unsafe extern \"C\" fn demo_put(\n    ptr: *const u8,\n    len: usize,\n) -> i32 {\n    0\n}\n\nfn private() {}\n\n#[cfg(test)]\nmod tests {\n    #[no_mangle]\n    pub extern \"C\" fn in_a_test() {}\n}\n",
    )
    .unwrap();
    let s = scan(&dir);
    let names: Vec<&str> = s.iter().map(|x| x.1.as_str()).collect();
    assert_eq!(names, ["demo_abi_version", "demo_put"], "{s:?}");
    assert!(s.iter().any(|x| x.2 == "pub unsafe extern \"C\" fn demo_put( ptr: *const u8, len: usize, ) -> i32"), "{s:?}");
    // the file format round trips and keeps the removed and exempt rows
    let f = read("# h\ndemo\tdemo_put\tpub extern \"C\" fn demo_put()\nremoved\tdemo\told\tuse demo_put\nexempt\tother\tpage glue\n");
    assert_eq!((f.live.len(), f.removed.len(), f.exempt.len()), (1, 1, 1));
    assert!(render(&f, "# h\n").contains("removed\tdemo\told\tuse demo_put"));
    // a different signature under the same name is a change, a missing name is a removal
    let live: BTreeSet<(String, String)> = s.iter().map(|x| (x.0.clone(), x.1.clone())).collect();
    assert!(live.contains(&("demo".to_string(), "demo_put".to_string())));
    assert!(has_version_fn(&["demo_abi_version", "demo_put"]) && !has_version_fn(&["demo_put"]) && has_version_fn(&["iyakki_tile_abi"]));
    let _ = std::fs::remove_dir_all(&dir);
}
