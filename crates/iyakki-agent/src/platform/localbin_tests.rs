// #[cfg(test)] -- this whole file is test code (localbin.rs includes it under #[cfg(test)]).
//! Finding the program a local image is built from: the ELF reader (static-pie musl, glibc dynamic, wrong CPU, not ELF, cut short),
//! the search order, the build option and the file the person chooses. Fake runner, temporary folders, nothing is built or run.

use super::*;
use crate::wsexec::FakeRunner;

const LB: LocalBinary = LocalBinary { file: "iyakki-store-server", package: "iyakki-store-server", sub: "store-server", legacy: &["iyakki"], docs: "docs/STORE.md, section 13" };

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-localbin-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A minimal ELF64 little-endian image: header, program headers, then optional interpreter and dynamic section.
fn elf(machine: u16, interp: Option<&str>, needed: bool, dynamic: bool) -> Vec<u8> {
    let nph = usize::from(interp.is_some()) + usize::from(dynamic) + 1;
    let ph_end = 64 + nph * 56;
    let interp_bytes: Vec<u8> = interp.map(|i| [i.as_bytes(), &[0]].concat()).unwrap_or_default();
    let dyn_off = ph_end + interp_bytes.len();
    let dyn_len = if dynamic { if needed { 48 } else { 32 } } else { 0 };
    let mut b = vec![0u8; dyn_off + dyn_len];
    b[..4].copy_from_slice(b"\x7fELF");
    b[4] = 2;
    b[5] = 1;
    b[18..20].copy_from_slice(&machine.to_le_bytes());
    b[32..40].copy_from_slice(&64u64.to_le_bytes());
    b[54..56].copy_from_slice(&56u16.to_le_bytes());
    b[56..58].copy_from_slice(&(nph as u16).to_le_bytes());
    let mut at = 64;
    let mut ph = |ty: u32, off: usize, len: usize, b: &mut Vec<u8>| {
        b[at..at + 4].copy_from_slice(&ty.to_le_bytes());
        b[at + 8..at + 16].copy_from_slice(&(off as u64).to_le_bytes());
        b[at + 32..at + 40].copy_from_slice(&(len as u64).to_le_bytes());
        at += 56;
    };
    ph(1, 0, 0, &mut b); // PT_LOAD
    if interp.is_some() {
        ph(3, ph_end, interp_bytes.len(), &mut b);
        b[ph_end..ph_end + interp_bytes.len()].copy_from_slice(&interp_bytes);
    }
    if dynamic {
        ph(2, dyn_off, dyn_len, &mut b);
        let mut o = dyn_off;
        if needed {
            b[o..o + 8].copy_from_slice(&1u64.to_le_bytes()); // DT_NEEDED
            o += 16;
        }
        b[o..o + 8].copy_from_slice(&0x1eu64.to_le_bytes()); // DT_FLAGS, then DT_NULL
    }
    b
}

fn put(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, bytes).unwrap();
    p
}

fn parse(b: &[u8]) -> Result<ElfInfo, ElfError> {
    parse_elf(&mut std::io::Cursor::new(b.to_vec()))
}

#[test]
fn the_elf_reader_tells_static_pie_from_dynamic_and_refuses_junk() {
    // musl static-pie: a dynamic section for its own relocations, but no interpreter and nothing needed
    let i = parse(&elf(62, None, false, true)).unwrap();
    assert!(i.is_static && i.arch == Some(Arch::X86_64) && i.dynamic_because.is_none(), "{i:?}");
    // a plain static executable
    assert!(parse(&elf(183, None, false, false)).unwrap().is_static);
    assert_eq!(parse(&elf(183, None, false, false)).unwrap().arch, Some(Arch::Aarch64));
    // glibc dynamic: a program interpreter
    let g = parse(&elf(62, Some("/lib64/ld-linux-x86-64.so.2"), true, true)).unwrap();
    assert!(!g.is_static && g.dynamic_because.unwrap().contains("ld-linux-x86-64"));
    // dynamic without an interpreter entry but with a needed library
    let n = parse(&elf(62, None, true, true)).unwrap();
    assert!(!n.is_static && n.dynamic_because.unwrap().contains("DT_NEEDED"));
    // another CPU is read, not rejected by the reader (the judge does that)
    assert_eq!(parse(&elf(40, None, false, false)).unwrap().arch, None);
    // not ELF, empty, cut short, 32-bit, big-endian
    assert_eq!(parse(b"#!/bin/sh\necho hi\n").unwrap_err(), ElfError::NotElf);
    assert_eq!(parse(b"").unwrap_err(), ElfError::NotElf);
    assert_eq!(parse(&elf(62, None, false, false)[..40]).unwrap_err(), ElfError::Truncated);
    assert_eq!(parse(&elf(62, Some("/lib/ld.so"), true, true)[..70]).unwrap_err(), ElfError::Truncated);
    let mut b32 = elf(62, None, false, false);
    b32[4] = 1;
    assert!(matches!(parse(&b32).unwrap_err(), ElfError::Unsupported(w) if w.contains("32-bit")));
    let mut be = elf(62, None, false, false);
    be[5] = 2;
    assert!(matches!(parse(&be).unwrap_err(), ElfError::Unsupported(w) if w.contains("big-endian")));
    // a huge claimed table does not make it read forever
    let mut huge = elf(62, None, false, false);
    huge[56..58].copy_from_slice(&60000u16.to_le_bytes());
    assert!(matches!(parse(&huge).unwrap_err(), ElfError::Unsupported(_)));
    // a missing file and a folder
    let d = tmp("reader");
    assert!(matches!(read_elf(&d.join("nope")).unwrap_err(), ElfError::Unreadable(_)));
    assert!(matches!(read_elf(&d).unwrap_err(), ElfError::Unreadable(_)));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn judge_accepts_static_of_the_right_cpu_and_says_why_otherwise() {
    let d = tmp("judge");
    let ok = put(&d, "ok", &elf(62, None, false, true));
    let dynamic = put(&d, "dyn", &elf(62, Some("/lib/ld-musl-x86_64.so.1"), true, true));
    let arm = put(&d, "arm", &elf(183, None, false, true));
    let text = put(&d, "text", b"#!/bin/sh\n");
    assert!(judge(&ok, Arch::X86_64).is_ok());
    assert!(judge(&dynamic, Arch::X86_64).unwrap_err().contains("dynamically linked"));
    let e = judge(&arm, Arch::X86_64).unwrap_err();
    assert!(e.contains("built for aarch64") && e.contains("cluster is x86_64"), "{e}");
    assert!(judge(&text, Arch::X86_64).unwrap_err().contains("no ELF header"));
    assert!(judge(&d.join("missing"), Arch::X86_64).unwrap_err().contains("cannot be read"));
    let _ = std::fs::remove_dir_all(&d);
}

fn source_tree(d: &Path) -> PathBuf {
    let s = d.join("src");
    put(&s, "Cargo.toml", b"[workspace]\n");
    put(&s, "crates/iyakki-store-server/Cargo.toml", b"[package]\n");
    s
}

fn fake_tools(musl: bool) -> FakeRunner {
    let r = FakeRunner::new(&["cargo", "rustup", "musl-gcc", "bash"]);
    r.answer("rustup target list --installed", if musl { "x86_64-unknown-linux-gnu\nx86_64-unknown-linux-musl\n" } else { "x86_64-unknown-linux-gnu\n" });
    r
}

#[test]
fn the_search_order_is_shipped_then_cache_then_source_and_the_running_program_is_never_a_candidate() {
    let d = tmp("order");
    let src = source_tree(&d);
    let shipped = d.join("share");
    let places = Places { shipped: vec![shipped.clone()], cache: d.join("data/store-server"), source: Some(src.clone()), build_override: None };
    let run = fake_tools(true);
    let good = elf(62, None, false, true);
    // the desktop window binary next to the others is dynamic and has another name: nothing finds it
    put(&shipped, "iyakki-desktop", &elf(62, Some("/lib64/ld-linux-x86-64.so.2"), true, true));
    let none = discover(&run, &LB, &places, Arch::X86_64);
    assert!(none.found.is_none() && none.rejected.is_empty() && none.searched.is_empty());
    // only the source tree's build: found there
    put(&src, "target/x86_64-unknown-linux-musl/release/iyakki-store-server", &good);
    assert_eq!(discover(&run, &LB, &places, Arch::X86_64).found.unwrap().origin, Origin::Source);
    // the cache beats the source tree
    put(&d, "data/store-server/iyakki-store-server-x86_64", &good);
    assert_eq!(discover(&run, &LB, &places, Arch::X86_64).found.unwrap().origin, Origin::Cache);
    // shipped beats the cache, in the arch folder, and by the arch-suffixed name
    put(&shipped, "x86_64/iyakki-store-server", &good);
    let f = discover(&run, &LB, &places, Arch::X86_64).found.unwrap();
    assert_eq!((f.origin, f.path.ends_with("x86_64/iyakki-store-server")), (Origin::Shipped, true));
    std::fs::remove_dir_all(shipped.join("x86_64")).unwrap();
    put(&shipped, "iyakki-store-server-x86_64", &good);
    assert_eq!(discover(&run, &LB, &places, Arch::X86_64).found.unwrap().origin, Origin::Shipped);
    // the arm cluster does not take the x86_64 files
    let arm = discover(&run, &LB, &places, Arch::Aarch64);
    assert!(arm.found.is_none() && arm.other_arch.is_empty(), "files are looked up by the wanted arch's names only");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_dynamic_file_is_refused_a_wrong_cpu_file_is_called_out_and_the_legacy_cache_name_still_works() {
    let d = tmp("refuse");
    let places = Places { shipped: vec![], cache: d.join("store-server"), source: None, build_override: None };
    let run = fake_tools(true);
    // a dynamic build in the cache (named like the old override) is not used
    put(&d, "store-server/iyakki", &elf(62, Some("/lib64/ld-linux-x86-64.so.2"), true, true));
    let r = discover(&run, &LB, &places, Arch::X86_64);
    assert!(r.found.is_none() && r.rejected.len() == 1 && r.rejected[0].1.contains("dynamically linked"), "{:?}", r.rejected);
    let text = r.problem(&LB, &places.cache);
    assert!(text.contains("No static") && text.contains("dynamically linked") && text.contains("dist/store-server/Dockerfile") && text.contains("release page"), "{text}");
    // a static aarch64 file where x86_64 is needed: reported as the wrong CPU, with the right name to look for
    put(&d, "store-server/iyakki-store-server-x86_64", &elf(183, None, false, true));
    let r = discover(&run, &LB, &places, Arch::X86_64);
    assert!(r.found.is_none() && r.other_arch.len() == 1 && r.other_arch[0].1 == Arch::Aarch64);
    assert!(r.problem(&LB, &places.cache).contains("static aarch64 build, but this cluster needs x86_64"));
    // a static full `iyakki` under the old name is accepted
    put(&d, "store-server/iyakki-store-server-x86_64", b"junk");
    put(&d, "store-server/iyakki", &elf(62, None, false, true));
    let r = discover(&run, &LB, &places, Arch::X86_64);
    assert!(r.found.as_ref().is_some_and(|f| f.path.ends_with("store-server/iyakki")), "{r:?}");
    assert_eq!(r.rejected.len(), 1, "the junk file is listed as not used");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn building_is_offered_only_when_the_tree_the_target_and_a_linker_exist_and_otherwise_everything_missing_is_listed() {
    let d = tmp("build");
    let src = source_tree(&d);
    let mut places = Places { shipped: vec![], cache: d.join("store-server"), source: Some(src.clone()), build_override: None };
    // all there
    let BuildState::Ready(b) = discover(&fake_tools(true), &LB, &places, Arch::X86_64).build else { panic!("expected a build") };
    assert_eq!(b.cwd, src);
    assert!(b.argv.join(" ").contains("cargo build --release --locked --target x86_64-unknown-linux-musl -p iyakki-store-server"), "{:?}", b.argv);
    assert_eq!(b.output, src.join("target/x86_64-unknown-linux-musl/release/iyakki-store-server"));
    // with the script in the tree the script is the command (it sets the cross linker)
    put(&src, "dist/store-server/build-static.sh", b"#!/bin/bash\n");
    let BuildState::Ready(b) = discover(&fake_tools(true), &LB, &places, Arch::X86_64).build else { panic!() };
    assert_eq!(&b.argv[0], "bash");
    assert!(b.argv[1].ends_with("dist/store-server/build-static.sh") && b.argv[2] == "x86_64");
    // the musl target is not installed: the exact rustup command is said
    let BuildState::Missing(m) = discover(&fake_tools(false), &LB, &places, Arch::X86_64).build else { panic!() };
    assert!(m.iter().any(|x| x.contains("rustup target add x86_64-unknown-linux-musl")), "{m:?}");
    // no linker at all
    let nolink = FakeRunner::new(&["cargo", "rustup"]);
    nolink.answer("rustup target list --installed", "x86_64-unknown-linux-musl\n");
    let BuildState::Missing(m) = build_state(&nolink, &LB, &places, Arch::X86_64) else { panic!() };
    assert!(m.iter().any(|x| x.contains("linker")), "{m:?}");
    // cargo missing is its own line, the target line is not guessed
    let BuildState::Missing(m) = build_state(&FakeRunner::new(&["musl-gcc"]), &LB, &places, Arch::X86_64) else { panic!() };
    assert!(m.iter().any(|x| x.contains("cargo")) && !m.iter().any(|x| x.contains("rustup target add")), "{m:?}");
    // a cross build for the other CPU needs a cross compiler (the fake host has none)
    let other = if Arch::host() == Arch::X86_64 { Arch::Aarch64 } else { Arch::X86_64 };
    let r = FakeRunner::new(&["cargo", "rustup", "musl-gcc"]);
    r.answer("rustup target list --installed", &format!("{}\n", other.triple()));
    let BuildState::Missing(m) = build_state(&r, &LB, &places, other) else { panic!() };
    assert!(m.iter().any(|x| x.contains("cross C compiler")), "{m:?}");
    // no source tree: say so, and the problem text still names the prebuilt route
    places.source = None;
    let r = discover(&fake_tools(true), &LB, &places, Arch::X86_64);
    assert!(matches!(&r.build, BuildState::Missing(m) if m[0].contains("source tree")));
    let t = r.problem(&LB, &places.cache);
    assert!(t.contains("iyakki-store-server-x86_64") && t.contains("Dockerfile"), "{t}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_chosen_file_is_checked_by_its_headers_copied_to_the_cache_and_never_run() {
    let d = tmp("adopt");
    let places = Places { shipped: vec![], cache: d.join("data/store-server"), source: None, build_override: None };
    // a file that would do harm if run: a shell script that leaves a marker (it is refused as not ELF, so it cannot be run by us)
    let marker = d.join("ran");
    let script = put(&d, "evil", format!("#!/bin/sh\ntouch {}\n", marker.display()).as_bytes());
    assert!(adopt(&script, &LB, &places, Arch::X86_64).unwrap_err().contains("no ELF header"));
    assert!(!marker.exists() && !places.cache.exists(), "nothing was run or copied");
    let dynamic = put(&d, "dyn", &elf(62, Some("/lib64/ld-linux-x86-64.so.2"), true, true));
    assert!(adopt(&dynamic, &LB, &places, Arch::X86_64).unwrap_err().contains("dynamically linked"));
    let arm = put(&d, "arm", &elf(183, None, false, true));
    assert!(adopt(&arm, &LB, &places, Arch::X86_64).unwrap_err().contains("cluster is x86_64"));
    let good = put(&d, "good", &elf(62, None, false, true));
    let f = adopt(&good, &LB, &places, Arch::X86_64).unwrap();
    assert_eq!((f.origin, f.arch), (Origin::Cache, Arch::X86_64));
    assert!(f.path.ends_with("data/store-server/iyakki-store-server-x86_64") && f.path.is_file());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&f.path).unwrap().permissions().mode() & 0o777, 0o755);
    // and discovery now finds it by itself
    assert!(discover(&fake_tools(true), &LB, &places, Arch::X86_64).found.is_some());
    // typed paths
    assert_eq!(expand_typed("  \"/a/b\" "), PathBuf::from("/a/b"));
    assert!(expand_typed("~/x").ends_with("x"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_source_tree_comes_from_the_data_folder_note_and_none_turns_the_search_off() {
    let d = tmp("source");
    let src = source_tree(&d);
    put(&d, "store-server/source", format!("{}\n", src.display()).as_bytes());
    assert_eq!(find_source(&d, &LB), Some(src));
    put(&d, "store-server/source", b"none");
    assert_eq!(find_source(&d, &LB), None);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn arch_words_map_both_ways() {
    assert_eq!(Arch::parse("amd64"), Some(Arch::X86_64));
    assert_eq!(Arch::parse("arm64"), Some(Arch::Aarch64));
    assert_eq!(Arch::parse("riscv64"), None);
    assert_eq!((Arch::Aarch64.node(), Arch::Aarch64.uname(), Arch::X86_64.triple()), ("arm64", "aarch64", "x86_64-unknown-linux-musl"));
    assert_eq!(Arch::from_machine(62), Some(Arch::X86_64));
    assert_eq!(Arch::from_machine(3), None);
}
