//! Golden file: one namespace of the knowledge base (`*.afkb`) as this release writes it (F-CORE-23). The fixture
//! `tests/compat/alpha3/knowledge-base-v1.hex` was recorded once by the ignored test below (its output saved to the file) and is never edited; every later release must
//! open it. Made-up documents, the built-in hashing embedder.

use iyakki_memx::embed::HashEmbed;
use iyakki_memx::kb::KnowledgeBase;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const NS: &str = "user/golden";
const FILE: &str = "knowledge-base-v1.hex";

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/alpha3").join(FILE)
}

fn fixture() -> Vec<u8> {
    let t = std::fs::read_to_string(fixture_path()).expect("the golden knowledge-base file is in the repository");
    let h: String = t.split_whitespace().collect();
    (0..h.len() / 2).map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).unwrap()).collect()
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-memx-compat-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn open(dir: &Path) -> KnowledgeBase {
    KnowledgeBase::open(dir, Arc::new(HashEmbed::default()))
}

/// Writes the fixture (once, when the release is cut; never edited after).
#[test]
#[ignore = "records the golden file: cargo test -p iyakki-memx --test compat_alpha3 record -- --ignored --nocapture"]
fn record_alpha3_knowledge_base() {
    let d = scratch("rec");
    let mut kb = open(&d);
    kb.add_document(NS, "Garden notes", "notes/garden.txt", "Tomatoes need sun and water. Basil grows well beside tomatoes.", BTreeMap::new()).unwrap();
    kb.add_document(NS, "Bread", "notes/bread.txt", "Knead the dough, let it rise twice, bake until the crust is brown.", BTreeMap::new()).unwrap();
    let file = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "afkb")).unwrap();
    let b = std::fs::read(file).unwrap();
    let hex: String = b.chunks(48).map(|c| c.iter().map(|x| format!("{x:02x}")).collect::<String>() + "\n").collect();
    println!("BEGIN-FIXTURE\n{hex}END-FIXTURE");
}

fn put_fixture(dir: &Path, bytes: &[u8]) -> PathBuf {
    let p = dir.join("golden.afkb");
    std::fs::write(&p, bytes).unwrap();
    p
}

#[test]
fn the_namespace_file_alpha3_wrote_opens_lists_its_documents_searches_and_takes_a_new_one() {
    let d = scratch("open");
    let p = put_fixture(&d, &fixture());
    let before = std::fs::read(&p).unwrap();
    let mut kb = open(&d);
    assert!(kb.load_errors.is_empty(), "{:?}", kb.load_errors);
    assert_eq!(kb.namespaces(), vec![NS.to_string()]);
    let mut titles: Vec<String> = kb.docs(NS).into_iter().map(|d| d.title).collect();
    titles.sort();
    assert_eq!(titles, ["Bread", "Garden notes"]);
    let hits = kb.search(&[NS], "when do I bake the dough", 3);
    assert!(hits.first().is_some_and(|h| h.text.contains("dough")), "{hits:?}");
    assert_eq!(std::fs::read(&p).unwrap(), before, "reading changes nothing");
    // a new document goes on top of the old ones and the old ones stay
    kb.add_document(NS, "Tea", "notes/tea.txt", "Steep the leaves for three minutes.", BTreeMap::new()).unwrap();
    assert_eq!(kb.docs(NS).len(), 3);
    let again = open(&d);
    assert_eq!(again.docs(NS).len(), 3);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_flipped_header_a_cut_file_and_a_foreign_file_are_reported_not_loaded_and_never_touched() {
    let b = fixture();
    for (name, bytes) in [("foreign", b"not a knowledge base at all".to_vec()), ("cut", b[..b.len() / 2].to_vec()), ("magic", {
        let mut m = b.clone();
        m[0] ^= 0xFF;
        m
    })] {
        let d = scratch(name);
        let p = put_fixture(&d, &bytes);
        let kb = open(&d);
        assert!(kb.namespaces().is_empty(), "{name}: nothing is loaded from it");
        assert_eq!(kb.load_errors.len(), 1, "{name}: the damage is reported, not dropped silently");
        assert_eq!(std::fs::read(&p).unwrap(), bytes, "{name}: the file is left as it is");
        let _ = std::fs::remove_dir_all(&d);
    }
}
