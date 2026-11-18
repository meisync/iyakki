//! The add-on installer: a local package, an installed ffmpeg's libraries, a mirror; every refusal; no network unless a mirror is used.

use crate::avaddon::{self, Addon, DEFAULT_SONAMES};
use crate::avaddon_install::*;
use std::path::{Path, PathBuf};

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-avinst-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn names() -> Vec<String> {
    DEFAULT_SONAMES.iter().map(|s| s.to_string()).collect()
}

/// A minimal 64-bit ELF shared object that needs `needed`, followed by `payload`.
fn elf(machine: u16, needed: &[&str], payload: &[u8]) -> Vec<u8> {
    let mut strtab = vec![0u8];
    let mut offs = Vec::new();
    for n in needed {
        offs.push(strtab.len() as u64);
        strtab.extend_from_slice(n.as_bytes());
        strtab.push(0);
    }
    let dynsz = (needed.len() + 2) * 16;
    let strtab_off = 64 + 2 * 56 + dynsz;
    let total = strtab_off + strtab.len() + payload.len() + 2048;
    let mut b = vec![0u8; 64];
    b[..4].copy_from_slice(b"\x7fELF");
    b[4] = 2;
    b[5] = 1;
    b[16..18].copy_from_slice(&3u16.to_le_bytes());
    b[18..20].copy_from_slice(&machine.to_le_bytes());
    b[32..40].copy_from_slice(&64u64.to_le_bytes());
    b[54..56].copy_from_slice(&56u16.to_le_bytes());
    b[56..58].copy_from_slice(&2u16.to_le_bytes());
    let ph = |t: u32, off: u64, sz: u64| {
        let mut p = vec![0u8; 56];
        p[..4].copy_from_slice(&t.to_le_bytes());
        p[8..16].copy_from_slice(&off.to_le_bytes());
        p[16..24].copy_from_slice(&off.to_le_bytes());
        p[32..40].copy_from_slice(&sz.to_le_bytes());
        p
    };
    b.extend(ph(1, 0, total as u64));
    b.extend(ph(2, (64 + 112) as u64, dynsz as u64));
    for o in offs {
        b.extend_from_slice(&1u64.to_le_bytes());
        b.extend_from_slice(&o.to_le_bytes());
    }
    b.extend_from_slice(&5u64.to_le_bytes());
    b.extend_from_slice(&(strtab_off as u64).to_le_bytes());
    b.extend_from_slice(&[0u8; 16]);
    b.extend(strtab);
    b.extend_from_slice(payload);
    b.resize(total, 0);
    b
}

const DECODERS: &[u8] = b"\0h264\0\0aac\0\0mp3float\0";

/// An installed ffmpeg's folder with the six libraries (libavcodec carries the decoders and `config`).
fn cli_folder(dir: &Path, machine: u16, needed: &[&str], config: &str, skip: &[&str]) {
    std::fs::create_dir_all(dir.join("ffmpeg-6.1/lib")).unwrap();
    for s in DEFAULT_SONAMES.iter().filter(|s| !skip.contains(*s)) {
        let payload = if s.starts_with("libavcodec") { [DECODERS, b"\0", config.as_bytes(), b"\0"].concat() } else { Vec::new() };
        std::fs::write(dir.join("ffmpeg-6.1/lib").join(s), elf(machine, needed, &payload)).unwrap();
    }
}

const LGPL: &str = "--prefix=/usr --disable-gpl --disable-nonfree --enable-shared";

fn places(data: &Path) -> Places {
    Places { data: data.into(), media_root: data.join("media"), dev_drop: None, boot_packages: None, machine: "x86_64".into(), system_dirs: vec![], sonames: names() }
}

fn no_network(_: &str, _: &Path) -> Result<String, String> {
    panic!("the network was used")
}

fn notice(configure: &str) -> String {
    format!("FFmpeg version: 6.1.5\nLicence: LGPL\n\nConfigure line:\n  ./configure {configure}\n")
}

/// A package archive (the layout package-av-addon.sh makes), built with the host's tar.
fn archive(dir: &Path, name: &str, sonames: &[&str], configure: &str) -> PathBuf {
    let stage = dir.join(format!("stage-{name}"));
    let top = stage.join("ffmpeg-libs-6.1.5");
    std::fs::create_dir_all(top.join("lib")).unwrap();
    for s in sonames {
        std::fs::write(top.join("lib").join(s), vec![5u8; 4096]).unwrap();
    }
    std::fs::write(top.join("COPYING.LGPLv2.1"), "lgpl").unwrap();
    std::fs::write(top.join("SOURCE-OFFER.txt"), "offer").unwrap();
    std::fs::write(top.join("NOTICE"), notice(configure)).unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let out = dir.join(name);
    assert!(std::process::Command::new("tar").arg("czf").arg(&out).arg("-C").arg(&stage).arg("ffmpeg-libs-6.1.5").status().unwrap().success());
    out
}

fn six() -> Vec<&'static str> {
    DEFAULT_SONAMES.to_vec()
}

fn lib_ready(data: &Path) -> bool {
    let plugins = data.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    std::fs::write(plugins.join("libgstlibav.so"), b"x").unwrap();
    avaddon::detect_at(&plugins, &avaddon::addon_dir(data).join("lib"), &names()) == Addon::Ready
}

// ---------------------------------------------------------------- an installed ffmpeg

#[test]
fn shared_musl_looking_libraries_of_an_installed_ffmpeg_pass_and_are_copied_not_linked() {
    let d = temp("cli-ok");
    cli_folder(&d.join("cli/pkgs/ffmpeg"), 0x3e, &["libc.so", "libavutil.so.58"], LGPL, &[]);
    std::fs::write(d.join("cli/pkgs/ffmpeg/COPYING.LGPLv2.1"), "lgpl text").unwrap();
    let p = places(&d);
    let pl = plan(&p);
    assert!(matches!(pl.source, Some(Source::InstalledFfmpeg { .. })), "{:?}", pl.review);
    let text = pl.review.join("\n");
    assert!(text.contains("Use the libraries of your installed ffmpeg") && text.contains("--disable-gpl") && text.contains(CONFIRM_TEXT), "{text}");
    // nothing without the confirmation
    assert!(run(&p, false, &no_network).unwrap_err().contains("not confirmed"));
    assert!(!avaddon::addon_dir(&d).exists());
    let said = run(&p, true, &no_network).unwrap();
    assert!(said.contains("installed ffmpeg"));
    let lib = avaddon::addon_dir(&d).join("lib");
    for s in DEFAULT_SONAMES {
        let m = std::fs::symlink_metadata(lib.join(s)).unwrap();
        assert!(m.file_type().is_file(), "{s} is a copy, not a link");
    }
    assert!(avaddon::addon_dir(&d).join("COPYING.LGPLv2.1").is_file(), "the legal file next to the libraries is copied");
    assert!(std::fs::read_to_string(avaddon::addon_dir(&d).join("NOTICE")).unwrap().contains("--disable-gpl"), "a NOTICE is made");
    assert!(lib_ready(&d));
    assert!(installed_note(&d).unwrap().starts_with("FFmpeg version: unknown"));
}

#[test]
fn a_static_single_file_ffmpeg_is_reported_honestly() {
    let d = temp("cli-static");
    std::fs::create_dir_all(d.join("cli/pkgs/ffmpeg/ffmpeg-7.0-static")).unwrap();
    std::fs::write(d.join("cli/pkgs/ffmpeg/ffmpeg-7.0-static/ffmpeg"), elf(0x3e, &[], b"")).unwrap();
    let pl = plan(&places(&d));
    assert!(pl.source.is_none());
    let text = pl.review.join("\n");
    assert!(text.contains(STATIC_TEXT) && text.contains("/local/packages") && text.contains("mirror"), "{text}");
    assert!(text.contains(RECIPE_COMMAND));
}

#[test]
fn glibc_gpl_wrong_processor_missing_dependency_and_wrong_sonames_are_refused() {
    let refused = |name: &str, machine: u16, needed: &[&str], config: &str, skip: &[&str]| {
        let d = temp(name);
        cli_folder(&d.join("f"), machine, needed, config, skip);
        match check_cli_ffmpeg(&d.join("f"), "x86_64", &names(), &[]) {
            CliLibs::Refused(w) => w,
            other => panic!("{name}: {other:?}"),
        }
    };
    assert!(refused("glibc", 0x3e, &["libc.so.6"], LGPL, &[]).contains("glibc"));
    assert!(refused("ldlinux", 0x3e, &["ld-linux-x86-64.so.2"], LGPL, &[]).contains("glibc"));
    assert!(refused("gpl", 0x3e, &["libc.so"], "--prefix=/usr --enable-gpl --enable-libx264", &[]).contains("--enable-gpl"));
    assert!(refused("nonfree", 0x3e, &["libc.so"], "--prefix=/usr --enable-nonfree --enable-libfdk-aac", &[]).contains("nonfree"));
    assert!(refused("arm", 0xb7, &["libc.so"], LGPL, &[]).contains("this processor"));
    assert!(refused("missingdep", 0x3e, &["libc.so", "libx264.so.164"], LGPL, &[]).contains("libx264.so.164"));
    assert!(refused("fewer", 0x3e, &["libc.so"], LGPL, &["libswscale.so.7"]).contains("lacks libswscale.so.7"));
    // no decoders in libavcodec
    let d = temp("nodec");
    cli_folder(&d.join("f"), 0x3e, &["libc.so"], LGPL, &[]);
    std::fs::write(d.join("f/ffmpeg-6.1/lib/libavcodec.so.60"), elf(0x3e, &["libc.so"], b"\0--prefix=/usr --disable-gpl\0")).unwrap();
    assert!(matches!(check_cli_ffmpeg(&d.join("f"), "x86_64", &names(), &[]), CliLibs::Refused(w) if w.contains("decoders")));
    // other sonames (another FFmpeg major version) are simply not the libraries asked for
    let d = temp("other");
    std::fs::create_dir_all(d.join("f")).unwrap();
    std::fs::write(d.join("f/libavcodec.so.61"), elf(0x3e, &[], b"")).unwrap();
    assert_eq!(check_cli_ffmpeg(&d.join("f"), "x86_64", &names(), &[]), CliLibs::NotInstalled);
}

#[test]
fn a_link_out_of_the_ffmpeg_folder_is_not_a_library() {
    let d = temp("cli-link");
    cli_folder(&d.join("f"), 0x3e, &["libc.so"], LGPL, &["libswscale.so.7"]);
    let outside = d.join("outside.so");
    std::fs::write(&outside, elf(0x3e, &["libc.so"], b"")).unwrap();
    std::os::unix::fs::symlink(&outside, d.join("f/ffmpeg-6.1/lib/libswscale.so.7")).unwrap();
    assert!(matches!(check_cli_ffmpeg(&d.join("f"), "x86_64", &names(), &[]), CliLibs::Refused(w) if w.contains("lacks libswscale.so.7")));
}

#[test]
fn the_elf_reader_reads_the_needed_libraries_and_refuses_other_files() {
    let e = read_elf(&elf(0x3e, &["libc.so", "libz.so.1"], b"")).unwrap();
    assert_eq!((e.machine, e.shared, e.needed), (0x3e, true, vec!["libc.so".to_string(), "libz.so.1".to_string()]));
    assert!(read_elf(b"#!/bin/sh\n").is_err() && read_elf(b"").is_err());
    assert_eq!(configure_in(b"\0junk\0--prefix=/usr --disable-gpl\0x").as_deref(), Some("--prefix=/usr --disable-gpl"));
}

// ---------------------------------------------------------------- local packages

#[test]
fn a_local_package_is_inspected_shown_confirmed_and_installed_without_the_network() {
    let d = temp("local-ok");
    let pk = local_dir(&d);
    archive(&pk, "ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz", &six(), LGPL);
    let p = places(&d);
    let pl = plan(&p);
    assert!(matches!(pl.source, Some(Source::Local(_))));
    let text = pl.review.join("\n");
    assert!(text.contains("FFmpeg version: 6.1.5") && text.contains("--disable-gpl") && text.contains("Found:") && text.ends_with(CONFIRM_TEXT), "{text}");
    assert!(run(&p, false, &no_network).is_err() && !avaddon::addon_dir(&d).exists(), "no install without the confirmation");
    run(&p, true, &no_network).unwrap();
    assert!(lib_ready(&d) && avaddon::addon_dir(&d).join("NOTICE").is_file());
    assert!(!d.join("addons/.inspect").exists(), "scratch is removed");
}

#[test]
fn a_package_for_another_ffmpeg_version_or_with_a_gpl_notice_or_without_legal_files_is_refused() {
    let d = temp("local-bad");
    let pk = local_dir(&d);
    archive(&pk, "ffmpeg-libs-7.0-linux-musl-x86_64.tar.gz", &["libavcodec.so.61", "libavformat.so.61"], LGPL);
    let pl = plan(&places(&d));
    assert!(pl.source.is_none() && pl.review.join("\n").contains("another FFmpeg version"), "{:?}", pl.review);
    let d = temp("local-gpl");
    archive(&local_dir(&d), "ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz", &six(), "--prefix=/usr --enable-gpl --disable-nonfree");
    assert!(plan(&places(&d)).review.join("\n").contains("--enable-gpl"));
    let d = temp("local-nodisable");
    archive(&local_dir(&d), "ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz", &six(), "--prefix=/usr");
    assert!(plan(&places(&d)).source.is_none());
    // not a gzip tar at all
    let d = temp("local-junk");
    std::fs::create_dir_all(local_dir(&d)).unwrap();
    std::fs::write(local_dir(&d).join("ffmpeg-libs-1-linux-musl-x86_64.tar.gz"), b"junk").unwrap();
    assert!(plan(&places(&d)).source.is_none());
}

#[test]
fn scanning_does_not_follow_a_link_out_of_the_folder_and_ignores_other_names_and_processors() {
    let d = temp("scan");
    let outside = d.join("elsewhere");
    let real = archive(&outside, "ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz", &six(), LGPL);
    let pk = local_dir(&d);
    std::fs::create_dir_all(&pk).unwrap();
    std::os::unix::fs::symlink(&real, pk.join("ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz")).unwrap();
    std::fs::write(pk.join("other.tar.gz"), b"x").unwrap();
    std::fs::write(pk.join("ffmpeg-libs-6.1.5-linux-musl-aarch64.tar.gz"), b"x").unwrap();
    assert!(find_local(&package_dirs(&d, &d.join("media")), "x86_64").is_empty(), "a linked file is skipped");
    // a plain file is found; removable media is scanned too (the folders are fixed names)
    std::fs::copy(&real, pk.join("ffmpeg-libs-6.1.4-linux-musl-x86_64.tar.gz")).unwrap();
    let stick = d.join("media/usb1/iyakki-packages");
    std::fs::create_dir_all(&stick).unwrap();
    std::fs::copy(&real, stick.join("ffmpeg-libs-6.1.6-linux-musl-x86_64.tar.gz")).unwrap();
    let found = find_local(&package_dirs(&d, &d.join("media")), "x86_64");
    assert_eq!(found.len(), 2);
    assert!(found.last().unwrap().ends_with("ffmpeg-libs-6.1.6-linux-musl-x86_64.tar.gz"), "the newest name is last");
    assert_eq!(find_local(&package_dirs(&d, &d.join("media")), "aarch64").len(), 1);
}

#[test]
fn the_developer_drop_folder_is_used_only_for_files_that_root_owns() {
    use std::os::unix::fs::MetadataExt;
    let d = temp("devdrop");
    let drop = d.join("dev-drop");
    let real = archive(&d.join("src"), "ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz", &six(), LGPL);
    std::fs::create_dir_all(&drop).unwrap();
    std::fs::copy(&real, drop.join("ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz")).unwrap();
    let mut p = places(&d);
    p.dev_drop = Some(drop.clone());
    let me = std::fs::metadata(&drop).unwrap().uid();
    let found = plan(&p);
    if me == 0 {
        assert!(matches!(found.source, Some(Source::Local(_))), "a root-owned file in the drop folder is used");
    } else {
        // not root here: the same file is ignored, exactly the rule that keeps a sandboxed shell's file out
        assert!(!matches!(found.source, Some(Source::Local(_))), "a file that root does not own is ignored: {:?}", found.review);
    }
    // no drop folder configured: nothing is looked at
    p.dev_drop = None;
    assert!(!matches!(plan(&p).source, Some(Source::Local(_))));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_package_on_the_boot_volume_is_found_and_a_link_there_is_not() {
    let d = temp("bootpk");
    let real = archive(&d.join("src"), "ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz", &six(), LGPL);
    let boot = d.join("boot-volume/iyakki-packages");
    std::fs::create_dir_all(&boot).unwrap();
    let mut p = places(&d);
    p.boot_packages = Some(boot.clone());
    assert!(!matches!(plan(&p).source, Some(Source::Local(_))), "an empty folder offers nothing");
    std::fs::copy(&real, boot.join("ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz")).unwrap();
    assert!(matches!(plan(&p).source, Some(Source::Local(_))), "the stick's own folder is used");
    std::fs::remove_file(boot.join("ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz")).unwrap();
    std::os::unix::fs::symlink(&real, boot.join("ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz")).unwrap();
    assert!(!matches!(plan(&p).source, Some(Source::Local(_))), "a link is never followed out of the folder");
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------- the mirror

#[test]
fn a_mirror_address_is_https_only_and_its_hash_is_checked_when_one_is_set() {
    assert!(validate_mirror("http://mirror.corp/ffmpeg.tar.gz", "").is_err());
    assert!(validate_mirror("https://user:pw@mirror.corp/x", "").is_err());
    assert!(validate_mirror("https://mirror.corp/a b", "").is_err());
    assert!(validate_mirror("https:///x", "").is_err());
    assert!(validate_mirror("https://mirror.corp/x.tar.gz", "abc").is_err());
    let good = validate_mirror("https://mirror.corp/x.tar.gz", &"AB".repeat(32)).unwrap();
    assert_eq!(good.sha256, "ab".repeat(32));
    let d = temp("mirror");
    assert!(load_mirror(&d).is_none(), "no default address");
    save_mirror(&d, "https://mirror.corp/x.tar.gz", "").unwrap();
    assert_eq!(load_mirror(&d).unwrap().url, "https://mirror.corp/x.tar.gz");
    assert!(save_mirror(&d, "http://x/y", "").is_err());
}

#[test]
fn a_mirror_download_is_checked_like_a_local_package_and_pinned_only_when_a_hash_is_set() {
    let build = temp("mirror-src");
    let a = archive(&build, "m.tar.gz", &six(), LGPL);
    let bytes = std::fs::read(&a).unwrap();
    let sha = crate::clis::sha256_hex(&bytes);
    let fetch = |_: &str, f: &Path| -> Result<String, String> {
        std::fs::write(f, &bytes).unwrap();
        Ok(crate::clis::sha256_hex(&bytes))
    };
    // no hash set: accepted after the full check, the review says it is not pinned
    let d = temp("mirror-nohash");
    save_mirror(&d, "https://mirror.corp/x.tar.gz", "").unwrap();
    let pl = plan(&places(&d));
    assert!(pl.review.join("\n").contains("not pinned"));
    run(&places(&d), true, &fetch).unwrap();
    assert!(lib_ready(&d));
    // the right hash
    let d = temp("mirror-hash");
    save_mirror(&d, "https://mirror.corp/x.tar.gz", &sha).unwrap();
    run(&places(&d), true, &fetch).unwrap();
    // a wrong hash installs nothing
    let d = temp("mirror-wrong");
    save_mirror(&d, "https://mirror.corp/x.tar.gz", &"0".repeat(64)).unwrap();
    assert!(run(&places(&d), true, &fetch).unwrap_err().contains("does not match"));
    assert!(!avaddon::addon_dir(&d).exists());
    // a download that is not an add-on installs nothing
    let junk = |_: &str, f: &Path| -> Result<String, String> {
        std::fs::write(f, b"junk").unwrap();
        Ok("x".into())
    };
    let d = temp("mirror-junk");
    save_mirror(&d, "https://mirror.corp/x.tar.gz", "").unwrap();
    assert!(run(&places(&d), true, &junk).is_err() && !avaddon::addon_dir(&d).exists());
}

#[test]
fn with_nothing_supplied_the_review_says_where_to_put_the_file_and_the_recipe() {
    let d = temp("nothing");
    let pl = plan(&places(&d));
    assert!(pl.source.is_none());
    let t = pl.review.join("\n");
    assert!(t.contains("/local/packages") && t.contains("Settings -> Browser add-on") && t.contains("does not provide FFmpeg") && t.contains(RECIPE_COMMAND), "{t}");
    assert!(run(&places(&d), true, &no_network).is_err());
}

#[test]
fn the_notice_checks_read_the_version_and_the_configure_line() {
    assert!(check_notice(&notice(LGPL)).is_ok());
    assert!(check_notice(&notice("--prefix=/usr --enable-gpl --disable-nonfree")).is_err());
    assert!(check_notice(&notice("--prefix=/usr --disable-gpl")).is_err(), "--disable-nonfree is required too");
    assert!(check_notice("Licence: LGPL").is_err());
    assert!(check_notice("FFmpeg version: 6\n").is_err());
}

/// With `IYAKKI_AV_ADDON_ARCHIVE=<the .tar.gz made by dist/buildroot/package-av-addon.sh>`: the real archive as a local package goes through the
/// whole installer and its own check script. Skipped without it.
#[test]
fn the_real_archive_installs_end_to_end_as_a_local_package() {
    let Some(path) = std::env::var_os("IYAKKI_AV_ADDON_ARCHIVE") else {
        eprintln!("SKIP: IYAKKI_AV_ADDON_ARCHIVE is not set");
        return;
    };
    let d = temp("real");
    std::fs::create_dir_all(local_dir(&d)).unwrap();
    std::fs::copy(&path, local_dir(&d).join("ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz")).unwrap();
    let p = places(&d);
    assert!(matches!(plan(&p).source, Some(Source::Local(_))));
    run(&p, true, &no_network).unwrap();
    assert!(lib_ready(&d));
    let out = std::process::Command::new(avaddon::addon_dir(&d).join("bin/iyakki-av-check")).env_remove("IYAKKI_NONE").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("add-on complete: 6 libraries"), "{text}");
}
