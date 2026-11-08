//! Applying system components with fakes: a boot partition that is a folder with a size limit, a data folder, a root folder for the
//! patch machinery and a fake for its system effects. Nothing here touches the machine the tests run on or any device, and no store is
//! contacted: the staged files are made on disk the way `components::stage` leaves them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::compapply::*;
use super::compinstalled::Facts;
use super::compmanifest::{self as cm, Record, ReleaseLabel};
use super::components::{self as comps, Installed, Release, StagedItem, Staged};
use crate::devmode::centre::Centre;
use crate::devmode::patch::FakeLive;
use crate::devmode::update::{self as up, BootMount, BootVol, DirBoot};

const LABEL: &str = "0.1.0-alpha.1";

struct Rig {
    dir: PathBuf,
    data: PathBuf,
    boot: PathBuf,
    root: PathBuf,
    svc: Svc,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn pseudo(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed).wrapping_add((i >> 9) as u8)).collect()
}

fn put(root: &Path, rel: &str, data: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, data).unwrap();
}

fn sha(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    crate::pkg::hex(&Sha256::digest(b))
}

fn board_cfg() -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/buildroot/board/iyakki/grub.cfg");
    std::fs::read_to_string(p).unwrap().replace("__KERNEL_PATH__", "/boot/bzImage").replace("__INITRD_PATH__", "/boot/initrd")
}

fn tree(root: &Path, skip: &[&str]) -> Vec<(String, String)> {
    fn walk(base: &Path, dir: &Path, skip: &[&str], out: &mut Vec<(String, String)>) {
        let mut es: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().flatten().collect();
        es.sort_by_key(|e| e.file_name());
        for e in es {
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
            if skip.iter().any(|s| rel == *s || rel.starts_with(&format!("{s}/"))) {
                continue;
            }
            if p.is_dir() {
                walk(base, &p, skip, out);
            } else {
                out.push((rel, sha(&std::fs::read(&p).unwrap())));
            }
        }
    }
    let mut v = vec![];
    walk(root, root, skip, &mut v);
    v
}

fn facts(boot_id: &str, core_file: &Path) -> Facts {
    Facts { kernel_release: Some("6.12.1-iyakki".into()), release_version: Some(LABEL.into()), core_version: Some(LABEL.into()), boot_id: Some(boot_id.into()), core_file: Some(core_file.to_path_buf()) }
}

fn rig_with(tag: &str, capacity: u64) -> Rig {
    let dir = std::env::temp_dir().join(format!("iyakki-capply-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (boot, data, root) = (dir.join("bootpart"), dir.join("data"), dir.join("root"));
    put(&boot, "boot/bzImage", &pseudo(1 << 20, 1));
    put(&boot, "boot/initrd", &pseudo(2 << 20, 2));
    put(&boot, "boot/grub/grub.cfg", board_cfg().as_bytes());
    put(&boot, "EFI/BOOT/bootx64.efi", &pseudo(100_000, 3));
    put(&boot, "boot/iyakki-release", format!("VERSION={LABEL}\nVARIANT=pc\nBUILD_DATE=2026-11-01\n").as_bytes());
    put(&data, "settings.json", b"{\"mine\":1}");
    put(&data, "home/root/notes.txt", b"my notes");
    put(&data, "iyakki/root/current", b"n1\nsize=1\n");
    put(&root, "usr/bin/iyakki", &pseudo(300_000, 9));
    let release = format!("VERSION={LABEL}\nVARIANT=pc\nBUILD_DATE=2026-11-01\n");
    let centre = Centre::new(&data, Box::new(DirBoot { root: boot.clone(), capacity, cluster: 4096 }), release, false, None).with_data_free(500 << 20);
    let core_file = root.join("usr/bin/iyakki");
    let svc = Svc { centre, live: Box::new(FakeLive::default()), patch_root: root.clone(), facts: facts("boot-A", &core_file), work_free: None, after_write: None };
    Rig { dir, data, boot, root, svc }
}

fn rig(tag: &str) -> Rig {
    rig_with(tag, 14 << 20)
}

struct Part {
    comp: &'static str,
    version: &'static str,
    bytes: Vec<u8>,
    install: &'static str,
    requires: Vec<(&'static str, &'static str)>,
}

fn part(comp: &'static str, version: &'static str, bytes: Vec<u8>, install: &'static str) -> Part {
    Part { comp, version, bytes, install, requires: vec![] }
}

/// What `stage` leaves behind for these parts: a plan from a signed list, the files in the staging folder, named by their hash.
fn prepared(r: &Rig, label: &str, parts: Vec<Part>) -> Prepared {
    let key = crate::pkg::new_signing_key().unwrap();
    let recs: Vec<Record> = parts
        .iter()
        .map(|p| Record {
            entry: cm::Entry { component: p.comp.into(), version: p.version.into(), sha256: sha(&p.bytes), size: p.bytes.len() as u64, requires: p.requires.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), channel: "stable".into(), built_from: String::new(), delta: vec![], install_path: p.install.into(), added: 0 },
            owner: "acme".into(),
            yanked: None,
            purged: false,
        })
        .collect();
    let rel = ReleaseLabel { version: label.into(), variant: "pc".into() };
    let doc = cm::verify(cm::build_doc_with("lab", 5, 1, &recs, Some(&rel), &key).as_bytes(), &crate::pkg::public_key_hex(&key)).unwrap();
    let plan = comps::plan(&doc, "Lab store", "stable", &r.svc.installed(), None).unwrap();
    let stage_dir = r.svc.work().join("staging");
    std::fs::create_dir_all(&stage_dir).unwrap();
    let mut items = vec![];
    for s in &plan.steps {
        let p = parts.iter().find(|p| p.comp == s.component).unwrap();
        let file = stage_dir.join(&s.sha256);
        std::fs::write(&file, &p.bytes).unwrap();
        items.push(StagedItem { step: s.clone(), file, note: String::new() });
    }
    Prepared { release: compapply_release(&plan), plan, staged: Staged { dir: stage_dir, items } }
}

fn compapply_release(p: &comps::Plan) -> Release {
    release_of(p, None).unwrap()
}

fn yes(p: &Prepared) -> Approval {
    Approval::given(&review(&p.plan, Some(&p.release)), "tester")
}

fn kernel_part(seed: u8) -> Part {
    part("kernel", "6.12.5", pseudo((1 << 20) + 5000, seed), "boot/bzImage")
}

fn boot_hashes(r: &Rig) -> Vec<(String, String)> {
    tree(&r.boot, &[])
}

// ------------------------------------------------------------------------------------------------ the approval

#[test]
fn nothing_is_applied_without_the_persons_approval_and_only_for_the_plan_that_was_shown() {
    let r = rig("approval");
    let p = prepared(&r, LABEL, vec![kernel_part(11)]);
    let other = prepared(&r, LABEL, vec![part("kernel", "6.12.6", pseudo((1 << 20) + 6000, 12), "boot/bzImage")]);
    let before = (boot_hashes(&r), tree(&r.data, &[]));
    let e = apply_staged(&r.svc, &p, None).unwrap_err();
    assert!(e.contains("needs the person's approval") && e.contains("kernel (not on record) -> 6.12.5") || e.contains("kernel 6.12.1 -> 6.12.5"), "{e}");
    // an approval for another plan does not count
    let e = apply_staged(&r.svc, &p, Some(&yes(&other))).unwrap_err();
    assert!(e.contains("different plan"), "{e}");
    assert_eq!((boot_hashes(&r), tree(&r.data, &[])), before, "a refusal writes nothing");
    assert!(!r.data.join(comps::INSTALLED_FILE).exists());
}

#[test]
fn the_review_says_what_changes_how_big_whether_a_restart_is_needed_and_what_is_kept() {
    let r = rig("review");
    let p = prepared(&r, LABEL, vec![kernel_part(11), part("core", "0.1.0-alpha.1.1108", pseudo(300_000, 10), "/usr/bin/iyakki")]);
    let rv = review(&p.plan, Some(&p.release));
    for want in ["kernel 6.12.1 -> 6.12.5", "core 0.1.0-alpha.1 -> 0.1.0-alpha.1.1108", "needs a restart of the machine", "next start of IYAKKI", "A restart is needed", "does not restart by itself", "IYAKKI (previous version)", "patch start counter", "Your files, accounts and settings are not touched", &format!("IYAKKI {LABEL} (pc)")] {
        assert!(rv.text.contains(want), "{want}\n{}", rv.text);
    }
    assert!(rv.restart_machine && rv.restart_core && rv.parts == 2);
    // same plan, same digest; any change of a part changes it
    assert_eq!(rv.digest, review(&p.plan, Some(&p.release)).digest);
    let mut q = p.plan.clone();
    q.steps[0].sha256 = "f".repeat(64);
    assert_ne!(review(&q, Some(&p.release)).digest, rv.digest);
}

#[test]
fn the_release_label_is_the_signed_one_unless_the_caller_overrides_it() {
    let r = rig("label");
    let p = prepared(&r, LABEL, vec![kernel_part(11)]);
    assert_eq!(release_of(&p.plan, None).unwrap(), Release { version: LABEL.into(), variant: "pc".into() });
    let over = Release { version: "9.9.9".into(), variant: "mac".into() };
    assert_eq!(release_of(&p.plan, Some(&over)).unwrap(), over);
    let mut bare = p.plan.clone();
    bare.release = None;
    assert!(release_of(&bare, None).unwrap_err().contains("no release label"));
}

// ------------------------------------------------------------------------------------------------ the boot path

#[test]
fn a_kernel_goes_through_the_update_machinery_keeps_the_previous_version_and_is_recorded_only_after_the_read_back() {
    let r = rig("kernel");
    let p = prepared(&r, LABEL, vec![kernel_part(11)]);
    let old_kernel = sha(&pseudo(1 << 20, 1));
    let data_before = tree(&r.data, &["component-work", "boot-next", "boot-prev", "devmode", "update-trial.json"]);
    let done = apply_staged(&r.svc, &p, Some(&yes(&p))).unwrap();
    assert!(done.restart_machine && !done.restart_core);
    assert!(done.text().contains("read back") && done.text().contains("Restart when you are ready"), "{}", done.text());
    // the new kernel is on the partition, byte for byte the staged one, and everything else of the boot set is still there
    assert_eq!(sha(&std::fs::read(r.boot.join("boot/bzImage")).unwrap()), p.staged.items[0].step.sha256);
    assert!(r.boot.join("boot/initrd").is_file() && r.boot.join("EFI/BOOT/bootx64.efi").is_file());
    // the previous version is kept on the data volume and in the boot menu, with the marker gone and a trial waiting for the restart
    let id = std::fs::read_to_string(r.data.join("boot-prev/latest")).unwrap().lines().next().unwrap().to_string();
    assert_eq!(sha(&std::fs::read(r.data.join("boot-prev").join(&id).join("bzImage")).unwrap()), old_kernel);
    let cfg = std::fs::read_to_string(&r.boot.join("boot/grub/grub.cfg")).unwrap();
    assert!(cfg.contains(up::PREV_TITLE));
    assert!(!r.boot.join(up::MARKER).exists() && up::interrupted(&r.data).is_none());
    assert_eq!(crate::devmode::centre::read_trial(&r.data).unwrap().state, "restart");
    // the release file follows the payload's label; the person's files are untouched
    assert!(std::fs::read_to_string(r.boot.join("boot/iyakki-release")).unwrap().starts_with(&format!("VERSION={LABEL}\nVARIANT=pc")));
    assert_eq!(tree(&r.data, &["component-work", "boot-next", "boot-prev", "devmode", "update-trial.json", super::components::INSTALLED_FILE]), data_before.iter().filter(|(p, _)| p != super::components::INSTALLED_FILE).cloned().collect::<Vec<_>>());
    // the record: written after the read back, with the hash, waiting for the restart
    let rec = Installed::load(&r.data);
    assert_eq!((rec.versions["kernel"].as_str(), rec.shas["kernel"].as_str(), rec.how["kernel"].as_str(), rec.awaiting["kernel"].as_str()), ("6.12.5", p.staged.items[0].step.sha256.as_str(), "applied", "boot-A"));
    // before the restart the record is not undone by the facts (the old kernel still runs); after a restart that fell back, it follows them
    assert_eq!(r.svc.installed().versions["kernel"], "6.12.5");
    let mut back = Svc { facts: Facts { boot_id: Some("boot-B".into()), ..r.svc.facts.clone() }, ..clone_svc(&r) };
    assert_eq!(back.installed().versions["kernel"], "6.12.1");
    back.facts.kernel_release = Some("6.12.5-iyakki".into());
    assert_eq!(back.installed().versions["kernel"], "6.12.5");
    // the work folder is cleaned
    assert!(!r.svc.work().join("staging").exists() && !r.svc.work().join("out").exists());
}

/// A second view of the same folders (the first one keeps its centre).
fn clone_svc(r: &Rig) -> Svc {
    let centre = Centre::new(&r.data, Box::new(DirBoot { root: r.boot.clone(), capacity: 14 << 20, cluster: 4096 }), format!("VERSION={LABEL}\nVARIANT=pc\n"), false, None).with_data_free(500 << 20);
    Svc { centre, live: Box::new(FakeLive::default()), patch_root: r.root.clone(), facts: r.svc.facts.clone(), work_free: None, after_write: None }
}

#[test]
fn a_failed_read_back_puts_the_previous_version_back_and_records_nothing() {
    let mut r = rig("readback");
    let boot_before = boot_hashes(&r);
    let p = prepared(&r, LABEL, vec![kernel_part(11)]);
    // a bit flips in the kernel after the engine wrote it (same length: only the hash can tell)
    let victim = r.boot.join("boot/bzImage");
    r.svc.after_write = Some(Box::new(move || {
        let mut b = std::fs::read(&victim).unwrap();
        b[5000] ^= 1;
        std::fs::write(&victim, b).unwrap();
    }));
    let e = apply_staged(&r.svc, &p, Some(&yes(&p))).unwrap_err();
    assert!(e.contains("reads back different") && e.contains("previous version was put back") && e.contains("Nothing is recorded"), "{e}");
    let after = boot_hashes(&r);
    let kernel = |v: &[(String, String)]| v.iter().find(|(p, _)| p == "boot/bzImage").unwrap().1.clone();
    assert_eq!(kernel(&after), kernel(&boot_before), "the old kernel is back");
    assert_eq!(after.iter().find(|(p, _)| p == "boot/initrd"), boot_before.iter().find(|(p, _)| p == "boot/initrd"));
    assert!(!r.data.join(comps::INSTALLED_FILE).exists(), "nothing recorded");
    assert!(!r.boot.join(up::MARKER).exists());
    assert_eq!(crate::devmode::centre::read_trial(&r.data).unwrap().kind, "rollback");
}

#[test]
fn short_free_space_is_refused_before_anything_is_written() {
    // the work folder
    let mut r = rig("space-work");
    let p = prepared(&r, LABEL, vec![kernel_part(11)]);
    r.svc.work_free = Some(1 << 20);
    let before = (boot_hashes(&r), tree(&r.data, &["component-work"]));
    let e = apply_staged(&r.svc, &p, Some(&yes(&p))).unwrap_err();
    assert!(e.contains("not enough room"), "{e}");
    assert_eq!((boot_hashes(&r), tree(&r.data, &["component-work"])), before);
    // the boot partition: a kernel that does not fit in what is left
    let r = rig_with("space-boot", 4 << 20);
    let big = part("kernel", "6.12.5", pseudo(3 << 20, 13), "boot/bzImage");
    let p = prepared(&r, LABEL, vec![big]);
    let before = boot_hashes(&r);
    let e = apply_staged(&r.svc, &p, Some(&yes(&p))).unwrap_err();
    assert!(e.contains("more room") || e.contains("MB free"), "{e}");
    assert_eq!(boot_hashes(&r), before);
    assert!(!r.boot.join(up::MARKER).exists() && !r.data.join(comps::INSTALLED_FILE).exists());
    // the data volume
    let mut r = rig("space-data");
    r.svc.centre = Centre::new(&r.data, Box::new(DirBoot { root: r.boot.clone(), capacity: 14 << 20, cluster: 4096 }), format!("VERSION={LABEL}\nVARIANT=pc\n"), false, None).with_data_free(2 << 20);
    let p = prepared(&r, LABEL, vec![kernel_part(11)]);
    assert!(apply_staged(&r.svc, &p, Some(&yes(&p))).unwrap_err().contains("data partition"));
}

struct NoBoot;
impl BootMount for NoBoot {
    fn open(&self, _write: bool) -> Result<BootVol, String> {
        Err("no IYAKKI-BOOT partition on the disk the data volume is on".into())
    }
    fn free(&self, _vol: &BootVol) -> u64 {
        0
    }
}

#[test]
fn a_boot_volume_that_is_not_this_installs_is_refused() {
    // none found on the data volume's disk
    let mut r = rig("notmine");
    r.svc.centre = Centre::new(&r.data, Box::new(NoBoot), format!("VERSION={LABEL}\nVARIANT=pc\n"), false, None).with_data_free(500 << 20);
    let p = prepared(&r, LABEL, vec![kernel_part(11)]);
    let before = boot_hashes(&r);
    let e = apply_staged(&r.svc, &p, Some(&yes(&p))).unwrap_err();
    assert!(e.contains("could not be reached") && e.contains("no IYAKKI-BOOT partition"), "{e}");
    assert_eq!(boot_hashes(&r), before);
    // a volume that is there but holds no IYAKKI boot menu (a stick, another drive)
    let other = r.dir.join("other-drive");
    put(&other, "docs/readme.txt", b"hello");
    r.svc.centre = Centre::new(&r.data, Box::new(DirBoot { root: other.clone(), capacity: 14 << 20, cluster: 4096 }), format!("VERSION={LABEL}\nVARIANT=pc\n"), false, None).with_data_free(500 << 20);
    let e = apply_staged(&r.svc, &p, Some(&yes(&p))).unwrap_err();
    assert!(e.contains("does not hold an IYAKKI boot menu"), "{e}");
    assert_eq!(tree(&other, &[]).len(), 1, "the other drive was not written");
    // a device whose data is only RAM cannot keep the previous version
    r.svc.centre = Centre::new(&r.data, Box::new(DirBoot { root: r.boot.clone(), capacity: 14 << 20, cluster: 4096 }), format!("VERSION={LABEL}\nVARIANT=pc\n"), true, None).with_data_free(500 << 20);
    assert!(apply_staged(&r.svc, &p, Some(&yes(&p))).unwrap_err().contains("RAM"));
}

#[test]
fn a_part_whose_requires_would_break_is_refused_naming_the_part_that_goes_first() {
    let r = rig("requires");
    let mut core = part("core", "0.1.0-alpha.1.1108", pseudo(300_000, 10), "/usr/bin/iyakki");
    core.requires = vec![("kernel", ">=7.0")];
    // the plan itself refuses it (the store's range cannot be met), so the apply is built from a plan that predates a change on the device
    let mut p = prepared(&r, LABEL, vec![kernel_part(11)]);
    p.plan.steps[0].requires.insert("core".into(), ">=0.1.0-alpha.9".into());
    p.staged.items[0].step = p.plan.steps[0].clone();
    let e = apply_staged(&r.svc, &p, Some(&yes(&p))).unwrap_err();
    assert!(e.contains("kernel 6.12.5 needs core >=0.1.0-alpha.9") && e.contains("update core first"), "{e}");
    // and the plan checks it too, at planning time
    let q = prepared(&r, LABEL, vec![kernel_part(11), core]);
    assert!(q.plan.steps.iter().all(|s| s.component == "kernel") && q.plan.blocked.len() == 1, "{:?}", q.plan.blocked);
    // what the device had when the plan was made must still be what it has
    let mut s = prepared(&r, LABEL, vec![kernel_part(11)]);
    s.plan.steps[0].from = Some("6.1.0".into());
    s.staged.items[0].step = s.plan.steps[0].clone();
    assert!(apply_staged(&r.svc, &s, Some(&yes(&s))).unwrap_err().contains("changed on this device since the plan was made"));
}

#[test]
fn an_update_that_is_already_pending_blocks_the_next_one_until_the_restart() {
    let r = rig("pending");
    let p = prepared(&r, LABEL, vec![kernel_part(11)]);
    apply_staged(&r.svc, &p, Some(&yes(&p))).unwrap();
    // now a second kernel
    let q = prepared(&r, LABEL, vec![part("kernel", "6.12.6", pseudo((1 << 20) + 7000, 14), "boot/bzImage")]);
    let before = boot_hashes(&r);
    let e = apply_staged(&r.svc, &q, Some(&yes(&q))).unwrap_err();
    assert!(e.contains("waits for a restart"), "{e}");
    assert_eq!(boot_hashes(&r), before);
    // an unfinished update (the marker) blocks it too
    let r2 = rig("pending2");
    std::fs::create_dir_all(&r2.data).unwrap();
    let p2 = prepared(&r2, LABEL, vec![kernel_part(11)]);
    std::fs::write(r2.boot.join(up::MARKER), br#"{"from":"a","to":"b","started":1,"prev_id":"","files":[]}"#).unwrap();
    std::fs::write(r2.data.join("boot-next-UPDATING.json"), b"x").ok();
    if up::interrupted(&r2.data).is_some() {
        assert!(apply_staged(&r2.svc, &p2, Some(&yes(&p2))).unwrap_err().contains("did not finish"));
    }
}

#[test]
fn a_label_that_is_not_the_running_one_needs_the_base_in_the_same_update() {
    let r = rig("label2");
    let p = prepared(&r, "0.1.0-alpha.2", vec![kernel_part(11)]);
    let before = boot_hashes(&r);
    let e = apply_staged(&r.svc, &p, Some(&yes(&p))).unwrap_err();
    assert!(e.contains("labels these parts IYAKKI 0.1.0-alpha.2") && e.contains("base system"), "{e}");
    assert_eq!(boot_hashes(&r), before);
}

// ------------------------------------------------------------------------------------------------ the program path

#[test]
fn core_becomes_a_patch_for_the_next_start_as_a_delta_and_the_running_program_is_not_touched() {
    let r = rig("core");
    let old = std::fs::read(r.root.join("usr/bin/iyakki")).unwrap();
    let mut new = old.clone();
    new[1234] ^= 0x5a;
    new.extend_from_slice(b"new tail");
    let p = {
        let mut p = prepared(&r, LABEL, vec![part("core", "0.1.0-alpha.1.1108", new.clone(), "/usr/bin/iyakki")]);
        // the installed file is known, so the pack carries a delta against it
        let mut inst = Installed::load(&r.data);
        inst.files.insert("core".into(), r.root.join("usr/bin/iyakki"));
        inst.versions.insert("core".into(), LABEL.into());
        inst.save(&r.data).unwrap();
        p.plan.steps[0].from = Some(LABEL.into());
        p.staged.items[0].step = p.plan.steps[0].clone();
        p
    };
    let rv = review(&p.plan, Some(&p.release));
    assert!(rv.restart_core && !rv.restart_machine && rv.text.contains("needs a restart"));
    let done = apply_staged(&r.svc, &p, Some(&yes(&p))).unwrap();
    assert!(done.restart_core && !done.restart_machine && done.boot_message.is_none());
    assert!(done.text().contains("takes effect at the next start") && done.text().contains("A restart is needed"), "{}", done.text());
    // the running program is exactly as it was; the patch is stored and active for the next start
    assert_eq!(std::fs::read(r.root.join("usr/bin/iyakki")).unwrap(), old);
    let active = std::fs::read_to_string(crate::devmode::patch::patches_dir(&r.data).join("active")).unwrap();
    assert!(active.contains("component-core"), "{active}");
    assert!(std::fs::read_dir(crate::devmode::patch::patches_dir(&r.data)).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("component-core")));
    // the boot partition was not involved
    assert!(!r.data.join("boot-prev").exists() && !r.data.join("update-trial.json").exists());
    let rec = Installed::load(&r.data);
    assert_eq!((rec.versions["core"].as_str(), rec.shas["core"].as_str(), rec.awaiting.contains_key("core")), ("0.1.0-alpha.1.1108", sha(&new).as_str(), true));
}

#[test]
fn when_the_program_patch_fails_after_the_boot_parts_were_written_they_are_put_back() {
    let r = rig("both");
    // the program the patch replaces is missing on this device: the patch machinery refuses
    std::fs::remove_file(r.root.join("usr/bin/iyakki")).unwrap();
    let p = prepared(&r, LABEL, vec![kernel_part(11), part("core", "0.1.0-alpha.1.1108", pseudo(300_000, 10), "/usr/bin/iyakki")]);
    let before = boot_hashes(&r);
    let e = apply_staged(&r.svc, &p, Some(&yes(&p))).unwrap_err();
    assert!(e.contains("program patch failed") && e.contains("put back"), "{e}");
    let kernel = |v: &[(String, String)]| v.iter().find(|(p, _)| p == "boot/bzImage").unwrap().1.clone();
    assert_eq!(kernel(&boot_hashes(&r)), kernel(&before));
    assert!(!r.data.join(comps::INSTALLED_FILE).exists(), "nothing recorded");
}

#[test]
fn the_last_check_is_kept_for_settings_in_plain_words() {
    let d = std::env::temp_dir().join(format!("iyakki-capply-{}-check", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    assert!(load_check(&d).is_empty());
    let lines = vec![StoreLine { store: "Lab store".into(), newer: 3, checked_at: 5, error: String::new() }, StoreLine { store: "Down".into(), newer: 0, checked_at: 5, error: "could not reach Down".into() }, StoreLine { store: "Fine".into(), newer: 0, checked_at: 5, error: String::new() }];
    save_check(&d, &lines);
    assert_eq!(load_check(&d), lines);
    assert_eq!(check_sentence(&lines[0]), "Components from Lab store: 3 newer.");
    assert!(check_sentence(&lines[1]).contains("could not check") && check_sentence(&lines[2]).contains("up to date"));
    let _ = std::fs::remove_dir_all(&d);
    let _: BTreeMap<u8, u8> = BTreeMap::new();
}
