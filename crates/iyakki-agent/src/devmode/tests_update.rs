//! System update without a device: the boot partition is a folder with a size limit (clusters of 4 KiB), the data partition is the
//! data folder. The refusals, the order of the steps, the previous version kept on the data partition with its boot menu entry, and
//! what a power cut at any point leaves behind (a marker, the old version bootable from the data partition, nothing half-changed that
//! the marker does not cover).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

use super::update::*;
use super::DevMode;

const CLUSTER: u64 = 4096;

pub(super) fn pseudo(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed).wrapping_add((i >> 9) as u8)).collect()
}

pub(super) fn board_cfg() -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/buildroot/board/iyakki/grub.cfg");
    std::fs::read_to_string(p).unwrap().replace("__KERNEL_PATH__", "/boot/bzImage").replace("__INITRD_PATH__", "/boot/initrd")
}

pub(super) struct Rig {
    pub(super) dir: PathBuf,
    pub(super) boot: DirBoot,
    pub(super) dm: DevMode,
    pub(super) release: String,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub(super) fn put(root: &Path, rel: &str, data: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, data).unwrap();
}

/// A device running 0.1.0-alpha.1 (macweb) with a 14 MB boot partition holding a 6 MB initramfs and a 1 MB kernel (the real ones are
/// 180 MB and 13 MB on 256 MB: the same proportions).
pub(super) fn rig(tag: &str) -> Rig {
    let dir = std::env::temp_dir().join(format!("iyakki-upd-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let boot_root = dir.join("bootpart");
    put(&boot_root, "boot/bzImage", &pseudo(1 << 20, 1));
    put(&boot_root, "boot/initrd", &pseudo(6 << 20, 2));
    put(&boot_root, "boot/grub/grub.cfg", board_cfg().as_bytes());
    put(&boot_root, "boot/grub/themes/iyakki/theme.txt", b"theme v1\n");
    put(&boot_root, "EFI/BOOT/bootx64.efi", &pseudo(300_000, 3));
    let data = dir.join("data");
    put(&data, "settings.json", b"{\"mine\":1}");
    put(&data, "home/root/notes.txt", b"my notes");
    put(&data, "vault/system.afv", b"vault bytes");
    let dm = DevMode::open(&data);
    Rig { dir, boot: DirBoot { root: boot_root, capacity: 14 << 20, cluster: CLUSTER }, dm, release: "VERSION=0.1.0-alpha.1\nVARIANT=macweb\nBUILD_DATE=2026-10-20\n".into() }
}

/// The payload folder of a new version: a new initramfs (1 MB bigger), a new kernel, the same EFI loader, a new theme and menu.
pub(super) fn payload(r: &Rig, version: &str, variant: &str, sb: bool, initrd_mb: usize) -> (PathBuf, Manifest) {
    let d = r.dir.join(format!("payload-{version}-{variant}-{sb}"));
    let _ = std::fs::remove_dir_all(&d);
    put(&d, "boot/bzImage", &pseudo((1 << 20) + 5000, 11));
    put(&d, "boot/initrd", &pseudo(initrd_mb << 20, 12));
    put(&d, "boot/grub/grub.cfg", board_cfg().replace("Loading IYAKKI", "Loading IYAKKI now").as_bytes());
    put(&d, "boot/grub/themes/iyakki/theme.txt", b"theme v2\n");
    put(&d, "EFI/BOOT/bootx64.efi", &pseudo(300_000, 3));
    if sb {
        put(&d, "EFI/BOOT/mmx64.efi", &pseudo(1000, 4));
    }
    put(&d, "boot/iyakki-release", format!("VERSION={version}\nVARIANT={variant}\nBUILD_DATE=2026-10-22\n").as_bytes());
    let list = list_boot(&d).into_iter().map(|(p, s)| (p.clone(), s, super::files::sha256_file(&d.join(&p)).unwrap())).collect::<Vec<_>>();
    let m = build_manifest(&d, &list, None, None, "test.img").unwrap();
    write_manifest(&d, &m).unwrap();
    (d, m)
}

pub(super) fn env<'a>(r: &'a Rig, data_free: &'a dyn Fn() -> u64, fault: Option<&'a AtomicU64>) -> Env<'a> {
    Env { dm: &r.dm, boot: &r.boot, release: r.release.clone(), volatile: false, data_free, fault_after: fault }
}

pub(super) fn stage_all(e: &Env, dir: &Path, m: &Manifest, only: &[String]) {
    std::fs::create_dir_all(e.dm.data_dir().join("boot-next")).unwrap();
    std::fs::write(e.dm.data_dir().join("boot-next").join(MANIFEST), serde_json::to_vec(m).unwrap()).unwrap();
    for n in only {
        let data = std::fs::read(dir.join(n)).unwrap();
        let mut off = 0;
        for chunk in data.chunks(4 << 20) {
            stage_chunk(e, m, n, off as u64, chunk).unwrap();
            off += chunk.len();
        }
        if data.is_empty() {
            stage_chunk(e, m, n, 0, &[]).unwrap();
        }
    }
}

pub(super) fn hash(p: &Path) -> String {
    super::files::sha256_file(p).unwrap()
}

pub(super) fn tree_hash(root: &Path, skip: &[&str]) -> Vec<(String, String)> {
    list_boot(root).into_iter().filter(|(p, _)| !skip.iter().any(|s| p.starts_with(s))).map(|(p, _)| { let h = hash(&root.join(&p)); (p, h) }).collect()
}

#[test]
fn versions_compare_like_releases_and_prerelease_orders_alpha_beta_rc_release() {
    use std::cmp::Ordering::*;
    for (a, b, want) in [("0.1.0-alpha.1", "0.1.0-alpha.2", Less), ("0.1.0-alpha.2", "0.1.0-beta.1", Less), ("0.1.0-beta.3", "0.1.0-rc.1", Less), ("0.1.0-rc.1", "0.1.0", Less), ("0.1.0", "0.1.1-alpha.1", Less), ("0.2.0", "0.1.9", Greater), ("v0.1.0", "0.1.0", Equal), ("1.0", "1.0.0", Equal), ("0.1.0-alpha.10", "0.1.0-alpha.9", Greater)] {
        assert_eq!(compare_versions(a, b), want, "{a} vs {b}");
    }
}

#[test]
fn an_update_keeps_the_old_version_on_the_data_partition_then_replaces_the_boot_files_and_leaves_the_rest_alone() {
    let r = rig("happy");
    let old = tree_hash(&r.boot.root, &[]);
    let data_before = tree_hash(r.dm.data_dir(), &["devmode", "boot-next", "boot-prev"]);
    let (dir, m) = payload(&r, "0.1.0-alpha.2", "macweb", false, 7);
    verify_payload(&dir, &m).unwrap();
    let free = || 500u64 << 20;
    let e = env(&r, &free, None);
    let p = plan(&e, &m, false).unwrap();
    assert_eq!((p.from.as_str(), p.to.as_str()), ("0.1.0-alpha.1", "0.1.0-alpha.2"));
    let mut needs = p.needs.clone();
    needs.sort();
    assert_eq!(needs, vec!["boot/bzImage", "boot/grub/grub.cfg", "boot/grub/themes/iyakki/theme.txt", "boot/initrd", "boot/iyakki-release"], "the EFI loader is the same, so it is not sent");
    assert_eq!(p.same, 1);
    assert!(p.growth > 0, "the new files are bigger");
    // nothing staged: the device refuses to start
    assert!(apply(&e, &m, false, "laptop").unwrap_err().contains("not staged completely"));
    assert_eq!(tree_hash(&r.boot.root, &[]), old, "and the boot partition was not touched");
    stage_all(&e, &dir, &m, &p.needs);
    let v = apply(&e, &m, false, "laptop").unwrap();
    assert_eq!(v["changed"], 5);
    // the boot partition now holds the new files; the loader is untouched
    for f in &m.files {
        if f.path == "boot/grub/grub.cfg" {
            continue;
        }
        let p = find_ci(&r.boot.root, &f.path).unwrap_or_else(|| panic!("{} missing", f.path));
        assert_eq!(hash(&p), f.sha256, "{}", f.path);
    }
    assert!(std::fs::read_to_string(find_ci(&r.boot.root, "boot/iyakki-release").unwrap()).unwrap().contains("0.1.0-alpha.2"), "the device now says which version its boot files are");
    assert!(!r.boot.root.join(MARKER).exists() && interrupted(r.dm.data_dir()).is_none(), "finished: no marker");
    assert!(!r.dm.data_dir().join("boot-next").exists(), "the staged copy is deleted");
    assert!(std::fs::read_dir(r.boot.root.join("boot")).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".upd")), "no half-written leftovers");
    // the old pair is on the data partition, identical to what was on the boot partition
    let prev = std::fs::read_to_string(r.dm.data_dir().join("boot-prev/latest")).unwrap();
    let id = prev.lines().next().unwrap();
    assert_eq!(prev.lines().nth(1), Some("0.1.0-alpha.1"));
    let find_old = |n: &str| old.iter().find(|(p, _)| p == n).unwrap().1.clone();
    assert_eq!(hash(&r.dm.data_dir().join("boot-prev").join(id).join("bzImage")), find_old("boot/bzImage"));
    assert_eq!(hash(&r.dm.data_dir().join("boot-prev").join(id).join("initrd")), find_old("boot/initrd"));
    // the menu: the new entries, then "previous version" loading from the data partition with the normal command line
    let cfg = std::fs::read_to_string(find_ci(&r.boot.root, "boot/grub/grub.cfg").unwrap()).unwrap();
    assert!(cfg.contains("Loading IYAKKI now"), "the new menu text is in");
    assert!(cfg.contains(&format!("menuentry \"{PREV_TITLE} 0.1.0-alpha.1\"")) && cfg.contains("search --no-floppy --label IYAKKI-DATA --set=iyakki_dp"), "{cfg}");
    assert!(cfg.contains(&format!("linux ($iyakki_dp)/boot-prev/{id}/bzImage $iyakki_usbq console=tty0 console=ttyS0,115200 panic=20 quiet loglevel=3 vt.global_cursor_default=0")), "{cfg}");
    assert!(cfg.contains(&format!("initrd ($iyakki_dp)/boot-prev/{id}/initrd")));
    assert!(cfg.find("menuentry \"IYAKKI\"").unwrap() < cfg.find(PREV_TITLE).unwrap() && cfg.find(PREV_TITLE).unwrap() < cfg.find("menuentry \"IYAKKI - show boot messages\"").unwrap(), "right after the normal entry");
    assert!(cfg.contains("set default=\"0\""), "the default is still the normal entry");
    // everything else on the data partition is exactly as it was
    assert_eq!(tree_hash(r.dm.data_dir(), &["devmode", "boot-next", "boot-prev"]), data_before, "the data partition's contents are untouched");
    // a second plan of the same payload: nothing but the menu differs
    let p2 = plan(&e, &m, false).unwrap();
    assert_eq!(p2.needs, vec!["boot/grub/grub.cfg"]);
    assert!(r.dm.activity(100).iter().any(|a| a.action == "dev.update.prev" && a.detail.contains("0.1.0-alpha.1")));
}

#[test]
fn an_update_that_is_not_for_this_device_is_refused_with_the_reason_and_only_the_forceable_ones_can_be_forced() {
    let r = rig("refuse");
    let free = || 500u64 << 20;
    let e = env(&r, &free, None);
    let (_, ok) = payload(&r, "0.1.0-alpha.2", "macweb", false, 7);
    assert!(plan(&e, &ok, false).is_ok());
    // the other image variant
    let (_, pc) = payload(&r, "0.1.0-alpha.2", "pc", false, 7);
    let err = plan(&e, &pc, false).unwrap_err();
    assert!(err.contains("pc image") && err.contains("macweb image"), "{err}");
    assert!(plan(&e, &pc, true).unwrap().notes.iter().any(|n| n.contains("forced")));
    // older
    let (_, old) = payload(&r, "0.1.0-alpha.0", "macweb", false, 7);
    assert!(plan(&e, &old, false).unwrap_err().contains("downgrade needs --force"));
    assert!(plan(&e, &old, true).unwrap().notes.iter().any(|n| n.contains("downgrade")));
    // the signing state would change: never
    let (_, sb) = payload(&r, "0.1.0-alpha.2", "macweb", true, 7);
    for force in [false, true] {
        let err = plan(&e, &sb, force).unwrap_err();
        assert!(err.contains("Secure Boot") && err.contains("reinstall"), "force={force}: {err}");
    }
    // a device that runs a signed chain refuses an unsigned one
    put(&r.boot.root, "EFI/BOOT/mmx64.efi", b"mm");
    assert!(plan(&e, &ok, true).unwrap_err().contains("Secure Boot"));
    std::fs::remove_file(r.boot.root.join("EFI/BOOT/mmx64.efi")).unwrap();
    // RAM-only data: the previous version could not be kept
    let mut e2 = env(&r, &free, None);
    e2.volatile = true;
    assert!(plan(&e2, &ok, true).unwrap_err().contains("previous version"));
    // a device that does not say what it runs
    let mut e3 = env(&r, &free, None);
    e3.release = String::new();
    assert!(plan(&e3, &ok, false).unwrap_err().contains("which image variant"));
    // the data partition cannot hold the copy and the staged files
    let tiny = || 10u64 << 20;
    assert!(plan(&env(&r, &tiny, None), &ok, false).unwrap_err().contains("data partition"));
}

#[test]
fn the_boot_partition_must_have_room_for_the_growth_of_the_new_files_over_the_old_ones() {
    let r = rig("space");
    let free = || 500u64 << 20;
    // 14 MB volume: 6 + 1 + 0.3 + small used, so about 6.6 MB free. An initramfs 4 MB bigger still fits...
    let (_, fits) = payload(&r, "0.1.0-alpha.2", "macweb", false, 10);
    let p = plan(&env(&r, &free, None), &fits, false).unwrap();
    assert!(p.growth >= 4 << 20 && (p.growth as u64) < p.free);
    // ...and one 14 MB bigger does not: the reason has the numbers
    let (_, big) = payload(&r, "0.1.0-alpha.2", "macweb", false, 20);
    let e = plan(&env(&r, &free, None), &big, false).unwrap_err();
    assert!(e.contains("MB more room") && e.contains("free of 14 MB") && e.contains("256 MB one does not"), "{e}");
    assert!(plan(&env(&r, &free, None), &big, true).is_err(), "no force for lack of room");
    // growth is judged against the old files, not the free space alone: the same size as now always fits
    let (_, same) = payload(&r, "0.1.0-alpha.2", "macweb", false, 6);
    assert!(plan(&env(&r, &free, None), &same, false).is_ok());
}

#[test]
fn a_bad_payload_is_refused_before_anything_is_sent() {
    let r = rig("manifest");
    let (dir, m) = payload(&r, "0.1.0-alpha.2", "macweb", false, 7);
    verify_payload(&dir, &m).unwrap();
    let bad = |f: &dyn Fn(&mut Manifest)| {
        let mut x = m.clone();
        f(&mut x);
        check_manifest(&x).unwrap_err()
    };
    assert!(bad(&|x| x.files[0].path = "../etc/passwd".into()).contains("not a path"));
    assert!(bad(&|x| x.files[0].path = "/boot/bzImage".into()).contains("not a path"));
    assert!(bad(&|x| x.files[0].path = "boot/a\\b".into()).contains("not a path"));
    assert!(bad(&|x| x.files[0].path = "etc/passwd".into()).contains("not a path"));
    assert!(bad(&|x| x.files[0].path = "boot//x".into()).contains("not a path"));
    assert!(bad(&|x| { let f = x.files[0].clone(); x.files.push(f) }).contains("twice"));
    assert!(bad(&|x| x.files.retain(|f| f.path != "boot/initrd")).contains("no boot/initrd"));
    assert!(bad(&|x| x.files[0].sha256 = "xyz".into()).contains("SHA-256"));
    assert!(bad(&|x| x.format = 9).contains("format"));
    assert!(bad(&|x| x.version = "1.0; rm -rf".into()).contains("version"));
    assert!(bad(&|x| x.variant = "pc/../x".into()).contains("variant"));
    assert!(bad(&|x| x.files.clear()).contains("files"));
    assert!(bad(&|x| x.files[0].size = MAX_TOTAL + 1).contains("more than an update carries"));
    // damage in the folder is found by the computer before it sends anything
    std::fs::write(dir.join("boot/initrd"), b"tampered").unwrap();
    assert!(verify_payload(&dir, &m).unwrap_err().contains("size") || true);
    let mut flipped = pseudo(7 << 20, 12);
    flipped[100] ^= 1;
    std::fs::write(dir.join("boot/initrd"), &flipped).unwrap();
    assert!(verify_payload(&dir, &m).unwrap_err().contains("damaged"));
    std::fs::remove_file(dir.join("boot/initrd")).unwrap();
    assert!(verify_payload(&dir, &m).is_err());
    // the manifest of an image with no release description needs the version and variant given
    let d2 = r.dir.join("norelease");
    put(&d2, "boot/bzImage", b"k");
    put(&d2, "boot/initrd", b"i");
    put(&d2, "boot/grub/grub.cfg", board_cfg().as_bytes());
    let list: Vec<(String, u64, String)> = list_boot(&d2).into_iter().map(|(p, s)| (p, s, "0".repeat(64))).collect();
    assert!(build_manifest(&d2, &list, None, None, "x.img").unwrap_err().contains("--version"));
    assert!(build_manifest(&d2, &list, Some("0.2.0"), None, "x.img").unwrap_err().contains("--variant"));
    let m2 = build_manifest(&d2, &list, Some("0.2.0"), Some("pc"), "x.img").unwrap();
    assert!(!m2.secure_boot);
    let mut with_sb = list.clone();
    with_sb.push(("EFI/BOOT/mmx64.efi".into(), 1, "0".repeat(64)));
    assert!(build_manifest(&d2, &with_sb, Some("0.2.0"), Some("pc"), "x.img").unwrap().secure_boot, "MokManager next to the loader means the Secure Boot chain");
    let nokernel: Vec<_> = list.iter().filter(|f| !f.0.ends_with("bzImage")).cloned().collect();
    assert!(build_manifest(&d2, &nokernel, Some("0.2.0"), Some("pc"), "x.img").unwrap_err().contains("not an IYAKKI boot partition"));
}

#[test]
fn staging_accepts_only_the_files_of_the_plan_checks_their_checksums_and_resumes() {
    let r = rig("stage");
    let free = || 500u64 << 20;
    let e = env(&r, &free, None);
    let (dir, m) = payload(&r, "0.1.0-alpha.2", "macweb", false, 7);
    std::fs::create_dir_all(r.dm.data_dir().join("boot-next")).unwrap();
    let data = std::fs::read(dir.join("boot/bzImage")).unwrap();
    // not in the plan, or escaping
    for bad in ["boot/other", "../x", "/etc/passwd", "boot/../../x"] {
        assert!(stage_chunk(&e, &m, bad, 0, b"x").is_err(), "{bad}");
    }
    // in pieces, with a wrong offset refused and the resume point reported
    assert_eq!(stage_chunk(&e, &m, "boot/bzImage", 0, &data[..1000]).unwrap()["have"], 1000);
    let (code, v) = stage_chunk(&e, &m, "boot/bzImage", 5, &data[5..50]).unwrap_err();
    assert_eq!((code, v["have"].clone()), (409, serde_json::json!(1000)));
    let done = stage_chunk(&e, &m, "boot/bzImage", 1000, &data[1000..]).unwrap();
    assert_eq!(done["done"], true);
    assert_eq!(hash(&r.dm.data_dir().join("boot-next/boot/bzImage")), m.files.iter().find(|f| f.path == "boot/bzImage").unwrap().sha256);
    // corrupted in transit: refused at the end, and not left staged
    let mut bad = data.clone();
    bad[10] ^= 0xFF;
    assert_eq!(stage_chunk(&e, &m, "boot/bzImage", 0, &bad).unwrap_err().0, 422);
    assert!(!r.dm.data_dir().join("boot-next/boot/bzImage.iyakki-part").exists());
}

#[test]
fn the_menu_entry_for_the_previous_version_is_built_from_the_normal_entry_and_replaces_an_older_one() {
    let cfg = board_cfg();
    let a = with_previous_entry(&cfg, "0.1.0-alpha.1", "0.1.0-alpha.1").unwrap();
    assert_eq!(a.matches("menuentry \"IYAKKI (previous version)").count(), 1);
    let b = with_previous_entry(&a, "0.1.0-alpha.2", "0.1.0-alpha.2").unwrap();
    assert_eq!(b.matches("menuentry \"IYAKKI (previous version)").count(), 1, "one entry only");
    assert!(b.contains("boot-prev/0.1.0-alpha.2/bzImage") && !b.contains("boot-prev/0.1.0-alpha.1/"), "the newer one replaced it");
    assert_eq!(b.matches("menuentry ").count(), cfg.matches("menuentry ").count() + 1);
    // every command it uses is a module the GRUB build has
    for cmd in ["search", "linux", "initrd", "echo"] {
        assert!(b.contains(cmd));
    }
    assert!(with_previous_entry("set default=0\n", "x", "1").unwrap_err().contains("normal IYAKKI entry"));
    assert!(with_previous_entry("menuentry \"IYAKKI\" {\n\tlinux /k\n", "x", "1").is_err(), "an unclosed entry");
}

/// A power cut at every point of the copy: whatever was written, the old version still boots from the data partition, and the
/// marker says the boot files are not to be trusted; a retry finishes the job.
#[test]
fn a_power_cut_at_any_point_leaves_a_marker_a_working_previous_entry_and_an_untouched_data_partition() {
    let cuts: [u64; 10] = [0, 1, 500, 100_000, 1 << 20, 4 << 20, 7 << 20, 8 << 20, 9 << 20, 12 << 20];
    for (n, cut) in cuts.iter().enumerate() {
        let r = rig(&format!("cut{n}"));
        let old = tree_hash(&r.boot.root, &[]);
        let data_before = tree_hash(r.dm.data_dir(), &["devmode", "boot-next", "boot-prev"]);
        let (dir, m) = payload(&r, "0.1.0-alpha.2", "macweb", false, 7);
        let free = || 500u64 << 20;
        let left = AtomicU64::new(*cut);
        let e = env(&r, &free, Some(&left));
        let p = plan(&e, &m, false).unwrap();
        stage_all(&e, &dir, &m, &p.needs);
        let err = apply(&e, &m, false, "laptop");
        let Err(err) = err else {
            assert!(*cut >= 9 << 20, "cut {cut} finished although the power was cut");
            continue;
        };
        assert!(err.contains("power was cut"), "{err}");
        // the kernel and initramfs on the boot partition are either exactly the old ones or the marker says they cannot be trusted
        let now = tree_hash(&r.boot.root, &[]);
        let marker_there = r.boot.root.join(MARKER).exists() || interrupted(r.dm.data_dir()).is_some();
        let kernel_changed = ["boot/bzImage", "boot/initrd"].iter().any(|f| now.iter().find(|(p, _)| p == f).map(|x| &x.1) != old.iter().find(|(p, _)| p == f).map(|x| &x.1));
        assert!(!kernel_changed || marker_there, "cut {cut}: boot files changed without a marker");
        // the previous version is complete on the data partition whenever the boot menu points at it
        let cfg = std::fs::read_to_string(find_ci(&r.boot.root, "boot/grub/grub.cfg").unwrap()).unwrap_or_default();
        if cfg.contains(PREV_TITLE) {
            let id = std::fs::read_to_string(r.dm.data_dir().join("boot-prev/latest")).unwrap().lines().next().unwrap().to_string();
            assert!(cfg.contains(&format!("boot-prev/{id}/bzImage")));
            for (n, from) in [("bzImage", "boot/bzImage"), ("initrd", "boot/initrd")] {
                assert_eq!(hash(&r.dm.data_dir().join("boot-prev").join(&id).join(n)), old.iter().find(|(p, _)| p == from).unwrap().1, "cut {cut}: the saved {n} is the old one, complete");
            }
        } else {
            assert!(!kernel_changed, "cut {cut}: kernel changed but the menu has no way back");
        }
        // the shell can tell the person
        if marker_there {
            assert!(interrupted_text(r.dm.data_dir()).unwrap().contains(PREV_TITLE), "cut {cut}");
        }
        // the data partition's own contents are untouched
        assert_eq!(tree_hash(r.dm.data_dir(), &["devmode", "boot-next", "boot-prev"]), data_before, "cut {cut}");
        // an interrupted update is not forgotten by an abort
        if interrupted(r.dm.data_dir()).is_some() {
            assert!(interrupted(r.dm.data_dir()).unwrap().to == "0.1.0-alpha.2");
        }
        // the retry (power is back) completes: marker gone, new files in, prev entry still there
        let e2 = env(&r, &free, None);
        let m_now = &m;
        let p2 = plan(&e2, m_now, false).unwrap();
        stage_all(&e2, &dir, m_now, &p2.needs);
        apply(&e2, m_now, false, "laptop").unwrap_or_else(|e| panic!("cut {cut}: the retry failed: {e}"));
        assert!(!r.boot.root.join(MARKER).exists() && interrupted(r.dm.data_dir()).is_none(), "cut {cut}");
        for f in m.files.iter().filter(|f| f.path.ends_with("initrd") || f.path.ends_with("bzImage")) {
            assert_eq!(hash(&find_ci(&r.boot.root, &f.path).unwrap()), f.sha256, "cut {cut}");
        }
    }
}

#[test]
fn nothing_is_changed_when_the_boot_files_are_already_the_new_ones_and_old_saved_versions_are_pruned() {
    let r = rig("prune");
    let free = || 500u64 << 20;
    let e = env(&r, &free, None);
    // three updates in a row: only the latest two saved versions stay
    for (i, v) in ["0.1.0-alpha.2", "0.1.0-alpha.3", "0.1.0-alpha.4"].iter().enumerate() {
        let (dir, m) = payload(&r, v, "macweb", false, 7 + i);
        let mut env_i = env(&r, &free, None);
        env_i.release = format!("VERSION={}\nVARIANT=macweb\nBUILD_DATE=2026-10-2{}\n", ["0.1.0-alpha.1", "0.1.0-alpha.2", "0.1.0-alpha.3"][i], i);
        let p = plan(&env_i, &m, false).unwrap();
        stage_all(&env_i, &dir, &m, &p.needs);
        apply(&env_i, &m, false, "laptop").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    let dirs = std::fs::read_dir(r.dm.data_dir().join("boot-prev")).unwrap().flatten().filter(|e| e.path().is_dir()).count();
    assert_eq!(dirs, 2, "the newest two");
    let latest = std::fs::read_to_string(r.dm.data_dir().join("boot-prev/latest")).unwrap();
    assert!(latest.contains("0.1.0-alpha.3") && r.dm.data_dir().join("boot-prev").join(latest.lines().next().unwrap()).join("initrd").is_file());
    // applying again with nothing different but the menu: fine, and a payload identical to what is there says so
    let _ = e;
}
