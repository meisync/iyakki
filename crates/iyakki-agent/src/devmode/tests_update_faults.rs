//! Fault injection for the whole update path (beta gate 5, F-UPD-27): a power cut at each step of an update, a damaged component, a signature that
//! does not match, a full disk and an older version, each followed by a model of the next starts (the boot menu's counter and the first stage's
//! choice of root). After every fault the machine must reach a start that runs, within three starts, and the test says which version and which
//! root it is. The same scenarios run in QEMU on the real image (`dist/qemu/update-faults-test.py`); this is the part that needs no boot.

use std::path::Path;
use std::sync::atomic::AtomicU64;

use super::bootcount::{has_guard, read_state, write_state, State};
use super::centre::*;
use super::tests_update::*;
use super::update::*;

fn key() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[7u8; 32])
}

fn centre(r: &Rig, free: u64) -> Centre {
    Centre::new(r.dm.data_dir(), Box::new(DirBoot { root: r.boot.root.clone(), capacity: 14 << 20, cluster: 4096 }), r.release.clone(), false, Some(key().verifying_key())).with_data_free(free)
}

fn pack(r: &Rig, c: &Centre, version: &str) -> Found {
    let (dir, _) = payload(r, version, "macweb", false, 7);
    sign_pack(&dir, &key()).unwrap();
    read_pack(&dir, c.key()).unwrap()
}

fn menu(r: &Rig) -> String {
    std::fs::read_to_string(find_ci(&r.boot.root, "boot/grub/grub.cfg").unwrap()).unwrap()
}

fn kernel_pair(r: &Rig) -> (String, String) {
    pair_at(&r.boot.root)
}

fn pair_at(root: &Path) -> (String, String) {
    (hash(&find_ci(root, "boot/bzImage").unwrap()), hash(&find_ci(root, "boot/initrd").unwrap()))
}

fn pair_of(m: &Manifest) -> (String, String) {
    let h = |p: &str| m.files.iter().find(|f| f.path == p).unwrap().sha256.clone();
    (h("boot/bzImage"), h("boot/initrd"))
}

/// What one start of the machine did.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Start {
    /// The boot menu's previous-version entry started (by the counter, or chosen).
    previous_entry: bool,
    /// The version whose kernel and initramfs ran: `old`, `new` or the saved one.
    version: String,
    /// The system root the first stage chose: `seed`, `current` or `previous` (lean installs); empty when there is no layered root.
    root: String,
}

/// The first stage's choice (`iyakki-stage1::candidates`): the data volume's `current`, then `previous`, then the seed; an image of another
/// size than its pointer promised is a half-written one and is skipped. `skip_current`: the entry that carries `iyakki.root=previous`.
fn choose_root(r: &Rig, skip_current: bool, seed: bool) -> String {
    let ptr = |n: &str| iyakki_stage1::Pointer::parse(&std::fs::read_to_string(r.dm.data_dir().join("iyakki/root").join(n)).unwrap_or_default());
    let (cur, prev) = (if skip_current { None } else { ptr("current") }, ptr("previous"));
    let kind = |base: &str| r.dm.data_dir().join(base).join("root.sfs").is_file().then_some(iyakki_stage1::Kind::Image);
    for c in iyakki_stage1::candidates(cur.as_ref(), prev.as_ref(), &kind, seed) {
        let len = std::fs::metadata(match c.volume {
            iyakki_stage1::Volume::Data => r.dm.data_dir().join(&c.path),
            iyakki_stage1::Volume::Boot => r.boot.root.join(&c.path),
        })
        .map(|m| m.len())
        .ok();
        if len.is_none() || (c.size.is_some() && len != c.size) {
            continue;
        }
        return match c.volume {
            iyakki_stage1::Volume::Boot => "seed".into(),
            iyakki_stage1::Volume::Data if cur.as_ref().is_some_and(|p| c.path.contains(&p.id)) => "current".into(),
            _ => "previous".into(),
        };
    }
    "none".into()
}

/// One start: the menu's guard moves the counter (1 to 2 and the normal entry, 2 to 0 and the previous-version entry), then the kernel and
/// initramfs that stand there must be a whole pair (the old one, or the new one when `new_runs`).
fn start(r: &Rig, old: &(String, String), new: &(String, String), new_runs: bool, lean: bool) -> Result<Start, String> {
    let cfg = menu(r);
    let mut previous = false;
    if has_guard(&cfg) {
        match read_state(&r.boot.root) {
            State::Armed => write_state(&r.boot.root, "2")?,
            State::Trying => {
                write_state(&r.boot.root, "0")?;
                previous = true;
            }
            State::Idle => {}
        }
    }
    if previous {
        let latest = std::fs::read_to_string(r.dm.data_dir().join("boot-prev/latest")).map_err(|_| "no previous version is kept".to_string())?;
        let (id, ver) = latest.trim().split_once('\n').ok_or("bad latest")?;
        let d = r.dm.data_dir().join("boot-prev").join(id);
        if (hash(&d.join("bzImage")), hash(&d.join("initrd"))) != *old {
            return Err("the saved version is not whole".into());
        }
        let skip = cfg[cfg.find(PREV_TITLE).unwrap()..].lines().nth(1).is_some_and(|_| cfg[cfg.find(PREV_TITLE).unwrap()..].contains("iyakki.root=previous"));
        return Ok(Start { previous_entry: true, version: ver.to_string(), root: if lean { choose_root(r, skip, true) } else { String::new() } });
    }
    let have = kernel_pair(r);
    let version = if have == *old {
        "old"
    } else if have == *new {
        if !new_runs {
            return Err("the new version does not reach its first picture".into());
        }
        "new"
    } else {
        return Err("the kernel and the initramfs are not a whole pair: this start cannot run".into());
    };
    Ok(Start { previous_entry: false, version: version.into(), root: if lean { choose_root(r, false, true) } else { String::new() } })
}

/// The next starts until one runs (three at most: a counter, a failed try, the fall-back). Returns the failed starts and the one that ran.
fn settle(r: &Rig, old: &(String, String), new: &(String, String), new_runs: bool, lean: bool) -> (usize, Start) {
    let mut failed = 0;
    for _ in 0..3 {
        match start(r, old, new, new_runs, lean) {
            Ok(s) => return (failed, s),
            Err(_) => failed += 1,
        }
    }
    panic!("after {failed} failed starts the machine still does not run (boot menu:\n{})", menu(r));
}

fn lean_rig(tag: &str) -> Rig {
    let r = rig(tag);
    put(&r.boot.root, "boot/root.sfs", &pseudo(3 << 20, 90));
    r
}

fn lean_pack(r: &Rig, version: &str, seed: u8) -> (std::path::PathBuf, Manifest) {
    let (d, _) = payload(r, version, "macweb", false, 1);
    put(&d, "boot/root.sfs", &pseudo(3 << 20, seed));
    let list = list_boot(&d).into_iter().filter(|(p, _)| p != MANIFEST).map(|(p, s)| (p.clone(), s, hash(&d.join(&p)))).collect::<Vec<_>>();
    let m = build_manifest(&d, &list, None, None, "lean.img").unwrap();
    write_manifest(&d, &m).unwrap();
    (d, m)
}

fn free() -> u64 {
    500u64 << 20
}

// ---------------------------------------------------------------------------------------------------------------- power cut

#[test]
fn a_power_cut_while_the_pack_is_being_copied_leaves_the_boot_partition_alone_and_the_next_start_is_the_old_version() {
    let r = rig("f-copy");
    let old = kernel_pair(&r);
    let boot_before = tree_hash(&r.boot.root, &[]);
    let (dir, m) = payload(&r, "0.1.0-alpha.2", "macweb", false, 7);
    let f = free;
    let e = env(&r, &f, None);
    let p = plan(&e, &m, false).unwrap();
    // half of the initramfs arrives, then the power goes
    std::fs::create_dir_all(r.dm.data_dir().join("boot-next")).unwrap();
    std::fs::write(r.dm.data_dir().join("boot-next").join(MANIFEST), serde_json::to_vec(&m).unwrap()).unwrap();
    let half = std::fs::read(dir.join("boot/initrd")).unwrap();
    stage_chunk(&e, &m, "boot/initrd", 0, &half[..half.len() / 2]).unwrap();
    assert_eq!(tree_hash(&r.boot.root, &[]), boot_before, "copying to the device never touches the boot partition");
    let (failed, s) = settle(&r, &old, &pair_of(&m), true, false);
    assert_eq!((failed, s.version.as_str(), s.previous_entry), (0, "old", false));
    assert!(interrupted(r.dm.data_dir()).is_none(), "a cut copy is a resume, not an unfinished update");
    // power is back: the rest arrives and the update completes
    stage_all(&e, &dir, &m, &p.needs);
    apply(&e, &m, false, "admin").unwrap();
    let (failed, s) = settle(&r, &old, &pair_of(&m), true, false);
    assert_eq!((failed, s.version.as_str()), (0, "new"));
}

#[test]
fn a_power_cut_at_any_byte_of_the_boot_file_replacement_still_reaches_a_start_that_runs_on_its_own() {
    // the cut points of the existing sweep, but here the counter decides: the second start at the latest runs the old version or the whole new one
    let cuts: [u64; 9] = [0, 1, 500, 100_000, 1 << 20, 4 << 20, 7 << 20, 8 << 20, 9 << 20];
    for (n, cut) in cuts.iter().enumerate() {
        let r = rig(&format!("f-cut{n}"));
        let old = kernel_pair(&r);
        let (dir, m) = payload(&r, "0.1.0-alpha.2", "macweb", false, 7);
        let f = free;
        let left = AtomicU64::new(*cut);
        let e = env(&r, &f, Some(&left));
        let p = plan(&e, &m, false).unwrap();
        stage_all(&e, &dir, &m, &p.needs);
        let cut_err = apply(&e, &m, false, "admin").is_err();
        let (failed, s) = settle(&r, &old, &pair_of(&m), true, false);
        assert!(failed <= 1, "cut {cut}: {failed} failed starts, at most one before the menu falls back by itself");
        if cut_err && s.version != "new" {
            assert!(s.version == "old" || s.previous_entry, "cut {cut}: {s:?}");
        }
        if failed == 1 {
            assert!(s.previous_entry && s.version == "0.1.0-alpha.1", "cut {cut}: the fall-back names the saved version, got {s:?}");
        }
        // the person is told, in plain words, what happened
        if cut_err && interrupted(r.dm.data_dir()).is_some() {
            assert!(interrupted_text(r.dm.data_dir()).unwrap().contains(PREV_TITLE));
        }
    }
}

#[test]
fn a_power_cut_after_the_system_image_switched_but_before_the_kernel_is_replaced_still_starts() {
    let r = lean_rig("f-switch");
    let old = kernel_pair(&r);
    let (dir, m) = lean_pack(&r, "0.1.0-alpha.2", 91);
    let f = free;
    let left = AtomicU64::new(100_000);
    let e = env(&r, &f, Some(&left));
    let p = plan(&e, &m, false).unwrap();
    stage_all(&e, &dir, &m, &p.needs);
    assert!(apply(&e, &m, false, "admin").is_err());
    let id = root_id(&m).unwrap();
    assert!(std::fs::read_to_string(r.dm.data_dir().join("iyakki/root/current")).unwrap().starts_with(&id), "the pointer switched before the cut");
    let (failed, s) = settle(&r, &old, &pair_of(&m), true, true);
    assert!(failed <= 1 && s.root != "none", "{failed} {s:?}");
    assert!(interrupted_text(r.dm.data_dir()).unwrap().contains("did not finish"));
    // power back: the retry completes
    let e2 = env(&r, &f, None);
    let p2 = plan(&e2, &m, false).unwrap();
    stage_all(&e2, &dir, &m, &p2.needs);
    apply(&e2, &m, false, "admin").unwrap();
    assert!(interrupted(r.dm.data_dir()).is_none());
    assert_eq!(kernel_pair(&r), pair_of(&m));
}

#[test]
fn a_power_cut_before_the_pointer_switch_leaves_the_old_root_and_a_cut_pointer_file_is_never_read_half() {
    let r = lean_rig("f-pointer");
    let (dir, m) = lean_pack(&r, "0.1.0-alpha.2", 91);
    let f = free;
    let e = env(&r, &f, None);
    let p = plan(&e, &m, false).unwrap();
    stage_all(&e, &dir, &m, &p.needs);
    apply(&e, &m, false, "admin").unwrap();
    let roots = r.dm.data_dir().join("iyakki/root");
    let id = root_id(&m).unwrap();
    // a power cut before the switch: `current` does not exist yet, the new folder is complete beside it
    std::fs::remove_file(roots.join("current")).unwrap();
    assert_eq!(choose_root(&r, false, true), "seed", "no pointer: the seed on the boot partition starts");
    // a cut inside the pointer write leaves a `.new` file next to the old pointer; the first stage reads only the real name
    std::fs::write(roots.join("current.new"), format!("{id}\nsize=1\n")).unwrap();
    assert_eq!(choose_root(&r, false, true), "seed");
    // a pointer that promises another size than the image has (a half-written image) is skipped: the seed starts
    std::fs::write(roots.join("current"), format!("{id}\nsize={}\n", (3 << 20) + 1)).unwrap();
    assert_eq!(choose_root(&r, false, true), "seed");
    std::fs::write(roots.join("current"), format!("{id}\nsize={}\n", 3 << 20)).unwrap();
    assert_eq!(choose_root(&r, false, true), "current");
    // the previous-version entry never starts the newest root
    assert_eq!(choose_root(&r, true, true), "seed");
}

// ---------------------------------------------------------------------------------------------------------------- first start of the new version

#[test]
fn a_new_version_that_does_not_reach_its_first_picture_falls_back_to_the_saved_one_on_the_second_start() {
    let r = lean_rig("f-firstboot");
    let old = kernel_pair(&r);
    let c = centre(&r, free());
    let (dir, m) = lean_pack(&r, "0.1.0-alpha.2", 91);
    sign_pack(&dir, &key()).unwrap();
    c.install(&read_pack(&dir, c.key()).unwrap(), false, "admin").unwrap();
    assert_eq!(read_state(&r.boot.root), State::Armed);
    let new = pair_of(&m);
    // start 1: the new set does not run; start 2: the menu starts the saved version by itself, on the old root
    assert!(start(&r, &old, &new, false, true).is_err());
    let s = start(&r, &old, &new, false, true).unwrap();
    assert_eq!((s.previous_entry, s.version.as_str(), s.root.as_str()), (true, "0.1.0-alpha.1", "seed"));
    assert_eq!(read_state(&r.boot.root), State::Idle);
    // the running system says so, in words
    let rel = std::fs::read_to_string(r.boot.root.join("boot/iyakki-release")).unwrap();
    assert!(rel.contains("0.1.0-alpha.2"), "the boot partition still names the new version");
    let say = after_boot(r.dm.data_dir(), "0.1.0-alpha.1", "another-boot").unwrap();
    assert!(say.contains("fell back to the saved version"), "{say}");
    assert_eq!(read_trial(r.dm.data_dir()).unwrap().state, "fell-back");
}

#[test]
fn a_new_version_that_runs_is_marked_good_and_a_power_cut_before_the_mark_costs_one_start_on_the_old_version_and_nothing_else() {
    let r = rig("f-mark");
    let old = kernel_pair(&r);
    let c = centre(&r, free());
    let f = pack(&r, &c, "0.1.0-alpha.2");
    let new = pair_of(&f.manifest);
    c.install(&f, false, "admin").unwrap();
    let data_before = tree_hash(r.dm.data_dir(), &["devmode", "boot-next", "boot-prev", TRIAL_FILE]);
    // start 1 runs the new version, the power goes before the first picture is marked good
    let s = start(&r, &old, &new, true, false).unwrap();
    assert_eq!((s.previous_entry, s.version.as_str()), (false, "new"));
    assert_eq!(read_state(&r.boot.root), State::Trying);
    // start 2: the counter was never marked, so the saved version starts, and the person's data is untouched
    let s = start(&r, &old, &new, true, false).unwrap();
    assert!(s.previous_entry && s.version == "0.1.0-alpha.1", "{s:?}");
    assert_eq!(tree_hash(r.dm.data_dir(), &["devmode", "boot-next", "boot-prev", TRIAL_FILE]), data_before);
    // a start that does get marked: the same update again, start 1 marked good, start 2 stays on the new version
    let r2 = rig("f-mark2");
    let c2 = centre(&r2, free());
    let f2 = pack(&r2, &c2, "0.1.0-alpha.2");
    c2.install(&f2, false, "admin").unwrap();
    start(&r2, &old, &new, true, false).unwrap();
    let rel = std::fs::read_to_string(r2.boot.root.join("boot/iyakki-release")).unwrap();
    let running = Centre::new(r2.dm.data_dir(), Box::new(DirBoot { root: r2.boot.root.clone(), capacity: 14 << 20, cluster: 4096 }), rel, false, None);
    assert!(running.confirm_boot("another-boot").unwrap().contains("marked good"));
    assert_eq!(start(&r2, &old, &new, true, false).unwrap().version, "new");
}

#[test]
fn a_rollback_after_an_update_starts_the_saved_version_and_its_own_failed_start_falls_forward_by_the_same_counter() {
    let r = rig("f-roll");
    let old = kernel_pair(&r);
    let c = centre(&r, free());
    let f = pack(&r, &c, "0.1.0-alpha.2");
    let new = pair_of(&f.manifest);
    c.install(&f, false, "admin").unwrap();
    start(&r, &old, &new, true, false).unwrap();
    c.rollback("admin").unwrap();
    assert_eq!(kernel_pair(&r), old, "the saved files are back on the boot partition");
    assert_eq!(read_state(&r.boot.root), State::Armed, "a rollback is tried once like an update");
    let (failed, s) = settle(&r, &new, &old, true, false);
    assert_eq!(failed, 0);
    assert_eq!(s.version, "new", "the pair that stands on the partition now is the old release's (named `new` by this helper)");
}

// ---------------------------------------------------------------------------------------------------------------- a bad component, signature, disk, version

#[test]
fn a_damaged_component_is_refused_before_anything_is_written_and_the_next_start_is_the_old_version() {
    let r = rig("f-corrupt");
    let old = kernel_pair(&r);
    let c = centre(&r, free());
    let f = pack(&r, &c, "0.1.0-alpha.2");
    let boot_before = tree_hash(&r.boot.root, &[]);
    let mut bytes = std::fs::read(f.dir.join("boot/initrd")).unwrap();
    bytes[4 << 20] ^= 0xff;
    std::fs::write(f.dir.join("boot/initrd"), bytes).unwrap();
    let err = c.install(&f, true, "admin").unwrap_err();
    assert!(!err.is_empty());
    assert_eq!(tree_hash(&r.boot.root, &[]), boot_before);
    assert!(interrupted(r.dm.data_dir()).is_none());
    let (failed, s) = settle(&r, &old, &pair_of(&f.manifest), true, false);
    assert_eq!((failed, s.version.as_str(), s.previous_entry), (0, "old", false));
    // damaged after it was staged: the apply step reads it back against the manifest and refuses
    let (dir, m) = payload(&r, "0.1.0-alpha.3", "macweb", false, 7);
    let fr = free;
    let e = env(&r, &fr, None);
    let p = plan(&e, &m, false).unwrap();
    stage_all(&e, &dir, &m, &p.needs);
    let staged = r.dm.data_dir().join("boot-next/boot/initrd");
    let mut b = std::fs::read(&staged).unwrap();
    b[100] ^= 1;
    std::fs::write(&staged, b).unwrap();
    assert!(apply(&e, &m, false, "admin").unwrap_err().contains("not staged completely"));
    assert_eq!(tree_hash(&r.boot.root, &[]), boot_before);
}

#[test]
fn a_signature_that_does_not_match_or_comes_from_another_key_never_installs_and_changes_nothing() {
    let r = rig("f-sig");
    let old = kernel_pair(&r);
    let c = centre(&r, free());
    let boot_before = tree_hash(&r.boot.root, &[]);
    let data_before = tree_hash(r.dm.data_dir(), &[]);
    // signed by another key
    let (dir, m) = payload(&r, "0.1.0-alpha.2", "macweb", false, 7);
    sign_pack(&dir, &ed25519_dalek::SigningKey::from_bytes(&[9u8; 32])).unwrap();
    let f = read_pack(&dir, c.key()).unwrap();
    assert!(matches!(f.trust, Trust::Bad(_)));
    assert!(c.install(&f, true, "admin").is_err());
    // the signature of the right key over another manifest (the manifest changed after signing)
    sign_pack(&dir, &key()).unwrap();
    let mut text = std::fs::read_to_string(dir.join(MANIFEST)).unwrap();
    text = text.replace("0.1.0-alpha.2", "0.1.0-alpha.9");
    std::fs::write(dir.join(MANIFEST), text).unwrap();
    let f = read_pack(&dir, c.key()).unwrap();
    assert!(matches!(f.trust, Trust::Bad(_)));
    assert!(c.install(&f, true, "admin").is_err());
    assert_eq!(tree_hash(&r.boot.root, &[]), boot_before);
    assert_eq!(tree_hash(r.dm.data_dir(), &[]), data_before, "nothing was staged either");
    assert_eq!(settle(&r, &old, &pair_of(&m), true, false).1.version, "old");
}

#[test]
fn a_full_data_disk_or_a_full_boot_partition_is_a_plain_refusal_and_the_old_version_keeps_starting() {
    let r = rig("f-full");
    let old = kernel_pair(&r);
    let boot_before = tree_hash(&r.boot.root, &[]);
    // the data disk has no room for the staged copy
    let c = centre(&r, 1 << 20);
    let f = pack(&r, &c, "0.1.0-alpha.2");
    let err = c.install(&f, true, "admin").unwrap_err();
    assert!(!err.is_empty());
    assert_eq!(tree_hash(&r.boot.root, &[]), boot_before);
    // the boot partition has no room for the growth of the new files
    let small = Centre::new(r.dm.data_dir(), Box::new(DirBoot { root: r.boot.root.clone(), capacity: 7 << 20, cluster: 4096 }), r.release.clone(), false, Some(key().verifying_key())).with_data_free(free());
    assert!(small.install(&f, true, "admin").is_err());
    assert_eq!(tree_hash(&r.boot.root, &[]), boot_before);
    assert!(interrupted(r.dm.data_dir()).is_none());
    assert_eq!(settle(&r, &old, &pair_of(&f.manifest), true, false).1.version, "old");
}

#[test]
fn an_older_version_is_refused_without_a_yes_and_nothing_changes() {
    let r = rig("f-down");
    let old = kernel_pair(&r);
    let c = centre(&r, free());
    let f = pack(&r, &c, "0.1.0-alpha.0");
    let boot_before = tree_hash(&r.boot.root, &[]);
    let err = c.install(&f, false, "admin").unwrap_err();
    assert!(err.contains("confirmation"), "{err}");
    assert_eq!(tree_hash(&r.boot.root, &[]), boot_before);
    assert_eq!(settle(&r, &old, &pair_of(&f.manifest), true, false).1.version, "old");
    assert_eq!(read_state(Path::new(&r.boot.root)), State::Idle, "a refused update arms nothing");
}

#[test]
fn an_update_that_finishes_leaves_the_counter_idle_for_a_direct_apply_and_armed_for_the_update_centre() {
    let r = rig("f-arm");
    let (dir, m) = payload(&r, "0.1.0-alpha.2", "macweb", false, 7);
    let f = free;
    let e = env(&r, &f, None);
    let p = plan(&e, &m, false).unwrap();
    stage_all(&e, &dir, &m, &p.needs);
    apply(&e, &m, false, "admin").unwrap();
    assert_eq!(read_state(&r.boot.root), State::Idle, "`iyakki dev update` does not leave a counter nobody would mark good");
    let r2 = rig("f-arm2");
    let c = centre(&r2, free());
    c.install(&pack(&r2, &c, "0.1.0-alpha.2"), false, "admin").unwrap();
    assert_eq!(read_state(&r2.boot.root), State::Armed);
}

// ---------------------------------------------------------------------------------------------------------------- the loader's layout (LDR2)

/// A boot volume in the loader-default layout: the loader's folder (so the update keeps a copy of the version it replaces in `EFI/iyakki-boot/prev/`),
/// room for the files twice (the real volume is 256 MB for a 85 MB set).
fn loader_rig(tag: &str) -> Rig {
    let mut r = rig(tag);
    put(&r.boot.root, "EFI/iyakki-boot/loader.efi", &pseudo(5000, 9));
    r.boot.capacity = 40 << 20;
    r
}

/// What the loader can start now, by its own rules (`boot.cfg`: the rows `prev-plain` and `prev-oldroot` `require` their marker): the marker, and when there is
/// one, the whole old pair in `prev/`. Every file of the current set is whole (the old one or the new one), never a half file.
fn loader_rows(r: &Rig, old: &(String, String), new: &(String, String), what: &str) -> Option<String> {
    let prev = r.boot.root.join(LOADER_PREV_DIR);
    let marker = ["set-plain", "set-oldroot"].into_iter().find(|m| prev.join(m).is_file()).map(|m| m.to_string());
    let (bz, ird) = pair_at(&r.boot.root);
    assert!(bz == old.0 || bz == new.0, "{what}: the kernel is a half file");
    assert!(ird == old.1 || ird == new.1, "{what}: the initramfs is a half file");
    if let Some(m) = &marker {
        let text = std::fs::read_to_string(prev.join(m)).unwrap();
        assert!(text.contains("stamp "), "{what}: {text}");
        assert_eq!((hash(&prev.join("bzImage")), hash(&prev.join("initrd"))), *old, "{what}: the previous-version row of the loader points at a copy that is not the old set");
    } else {
        assert_eq!((bz, ird), *old, "{what}: no previous-version row, so the files of the boot volume must still be the old set");
    }
    marker
}

#[test]
fn a_power_cut_at_any_point_of_the_loaders_copy_or_the_replacement_never_leaves_a_previous_row_over_half_a_copy_and_the_machine_starts() {
    // cuts: the menu, the kernel copy (1 MB), the initramfs copy (6 MB), the marker, then the new initramfs, kernel and menu
    let cuts: [u64; 16] = [0, 1, 2000, 300_000, 1 << 20, 2 << 20, 4 << 20, 7 << 20, (7 << 20) + 20_000, 8 << 20, 10 << 20, 14 << 20, (14 << 20) + 20_000, 15 << 20, 16 << 20, 18 << 20];
    for (n, cut) in cuts.iter().enumerate() {
        let r = loader_rig(&format!("l-cut{n}"));
        let old = kernel_pair(&r);
        let (dir, m) = payload(&r, "0.1.0-alpha.2", "macweb", false, 7);
        let f = free;
        let left = AtomicU64::new(*cut);
        let e = env(&r, &f, Some(&left));
        let p = plan(&e, &m, false).unwrap();
        stage_all(&e, &dir, &m, &p.needs);
        let cut_err = apply(&e, &m, false, "admin").is_err();
        let new = pair_of(&m);
        let what = format!("cut {cut}");
        let marker = loader_rows(&r, &old, &new, &what);
        if cut_err {
            assert!(interrupted(r.dm.data_dir()).is_some() || marker.is_none(), "{what}: a cut after the marker is reported as an unfinished update");
        }
        // power is back: the same update again completes, and the loader's previous-version row is the OLD set, never a mix of the two
        let e2 = env(&r, &f, None);
        let p2 = plan(&e2, &m, false).unwrap();
        stage_all(&e2, &dir, &m, &p2.needs);
        apply(&e2, &m, false, "admin").unwrap();
        assert_eq!(kernel_pair(&r), new, "{what}: the retry finished");
        let prev = r.boot.root.join(LOADER_PREV_DIR);
        assert_eq!((hash(&prev.join("bzImage")), hash(&prev.join("initrd"))), old, "{what}: after the retry the loader's previous version is the version that was running before the update");
        let stamp = std::fs::read_to_string(prev.join("set-plain")).unwrap();
        assert!(stamp.contains("version 0.1.0-alpha.1"), "{what}: {stamp}");
        assert!(std::fs::read_dir(&prev).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".tmp")), "{what}: no leftovers after the retry");
    }
}

#[test]
fn a_power_cut_after_the_system_image_switched_and_a_retry_still_keeps_the_old_set_for_the_old_root_in_the_loaders_row_and_in_grubs_entry() {
    let r = {
        let mut r = lean_rig("l-oldroot");
        put(&r.boot.root, "EFI/iyakki-boot/loader.efi", &pseudo(5000, 9));
        r.boot.capacity = 40 << 20;
        r
    };
    let old = kernel_pair(&r);
    let (dir, m) = lean_pack(&r, "0.1.0-alpha.2", 91);
    let f = free;
    // the loader's copy of the old set (1 MB kernel, 6 MB initramfs) is written first, then the system image: the cut is inside the image copy
    let left = AtomicU64::new((7 << 20) + 100_000);
    let e = env(&r, &f, Some(&left));
    let p = plan(&e, &m, false).unwrap();
    stage_all(&e, &dir, &m, &p.needs);
    assert!(apply(&e, &m, false, "admin").is_err());
    let prev = r.boot.root.join(LOADER_PREV_DIR);
    assert!(prev.join("set-oldroot").is_file() && !prev.join("set-plain").exists(), "the cut came after the loader's copy: it names the old root");
    // power is back: the retry finds the pointer switched already and must not turn the row into one that starts the OLD kernel on the NEW root
    let e2 = env(&r, &f, None);
    let p2 = plan(&e2, &m, false).unwrap();
    stage_all(&e2, &dir, &m, &p2.needs);
    apply(&e2, &m, false, "admin").unwrap();
    assert_eq!(kernel_pair(&r), pair_of(&m));
    assert!(prev.join("set-oldroot").is_file() && !prev.join("set-plain").exists(), "after the retry the row still starts the old set on the old root");
    assert_eq!((hash(&prev.join("bzImage")), hash(&prev.join("initrd"))), old);
    let cfg = menu(&r);
    assert!(cfg[cfg.find(PREV_TITLE).unwrap()..].contains("iyakki.root=previous"), "GRUB's previous-version entry too:\n{cfg}");
}

// ---------------------------------------------------------------------------------------------------------------- the real GRUB run

/// Leaves the boot partition of a copy of a real stick in the state a fault leaves, for `dist/qemu/update-faults-test.py`, which boots the copy
/// with the real GRUB. `IYAKKI_FAULT_DIR` is the folder with the stick's boot files (`boot/bzImage`, `boot/initrd`, `boot/grub/grub.cfg`),
/// `IYAKKI_FAULT_CASE` is `control`, `done`, `cut-initrd`, `cut-kernel`, `cut-menu` or (the loader layout) `cut-prev-kernel`, `cut-prev-initrd`, `cut-prev-marker`. The "new" set is the stick's own kernel and initramfs with 16 zero
/// bytes added (both ignore them), so it is a different set that starts. Prints one `FAULT-STATE` line the script reads.
#[test]
#[ignore]
fn update_fault_state_on_a_folder_for_a_real_grub_run() {
    let (Ok(dir), Ok(case)) = (std::env::var("IYAKKI_FAULT_DIR"), std::env::var("IYAKKI_FAULT_CASE")) else {
        crate::qeprintln!("skipped: needs IYAKKI_FAULT_DIR and IYAKKI_FAULT_CASE (run it through dist/qemu/update-faults-test.py)");
        return;
    };
    let root = std::path::PathBuf::from(&dir);
    let work = std::env::temp_dir().join(format!("iyakki-fault-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let data = work.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let rel = std::fs::read_to_string(find_ci(&root, "boot/iyakki-release").expect("the stick has boot/iyakki-release")).unwrap();
    let before = pair_at(&root);
    let pack = work.join("pack");
    for n in ["boot/bzImage", "boot/initrd"] {
        let mut b = std::fs::read(find_ci(&root, n).unwrap()).unwrap();
        b.extend_from_slice(&[0u8; 16]);
        put(&pack, n, &b);
    }
    put(&pack, "boot/grub/grub.cfg", std::fs::read(find_ci(&root, "boot/grub/grub.cfg").unwrap()).unwrap().as_slice());
    put(&pack, "boot/iyakki-release", rel.replace("VERSION=", "VERSION=9.").as_bytes());
    let list = list_boot(&pack).into_iter().map(|(p, s)| (p.clone(), s, hash(&pack.join(&p)))).collect::<Vec<_>>();
    let m = build_manifest(&pack, &list, None, None, "fault.img").unwrap();
    write_manifest(&pack, &m).unwrap();
    let dm = super::DevMode::open(&data);
    // the real volume's size when the script says it (a 256 MB boot volume holds the set twice and no more): the room checks of the update are then the real ones
    let capacity = std::env::var("IYAKKI_FAULT_CAPACITY").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(1 << 34);
    let boot = DirBoot { root: root.clone(), capacity, cluster: 4096 };
    let initrd_len = std::fs::metadata(pack.join("boot/initrd")).unwrap().len();
    // the loader layout (`EFI/iyakki-boot` is there): the update first copies the set it replaces to `EFI/iyakki-boot/prev/` through the same knob, so every later cut
    // point moves by the size of that copy (LDR2). `cfg_first` is the length of the menu the update writes first (the saved menu with the previous-version entry).
    let loader = root.join("EFI/iyakki-boot").is_dir();
    let (old_bz, old_ird) = (std::fs::metadata(find_ci(&root, "boot/bzImage").unwrap()).unwrap().len(), std::fs::metadata(find_ci(&root, "boot/initrd").unwrap()).unwrap().len());
    let copy = if loader { old_bz + old_ird } else { 0 };
    let version = rel.lines().find_map(|l| l.strip_prefix("VERSION=")).unwrap_or("").trim().to_string();
    let id: String = version.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
    let cfg_first = with_previous_entry_args(&std::fs::read_to_string(find_ci(&root, "boot/grub/grub.cfg").unwrap()).unwrap(), &id, &version, "").map(|c| c.len() as u64).unwrap_or(0);
    let cut = match case.as_str() {
        "cut-initrd" => Some(copy + initrd_len / 2),
        // before the initramfs: the menu with the previous-version entry, the loader's copy, the update marker (about 300 bytes); then the initramfs whole, then 4 KB: inside the kernel
        // (LDR2: the first formula left out the menu and the marker, so the cut fell at the end of the initramfs and the run said "no mixed set")
        "cut-kernel" => Some(cfg_first + copy + 2048 + initrd_len + 4096),
        "cut-menu" => Some(copy + m.files.iter().map(|f| f.size).sum::<u64>() + 4096),
        // the power goes while the loader's copy of the set being replaced is written: inside the kernel copy, inside the initramfs copy, at the marker
        "cut-prev-kernel" => Some(cfg_first + old_bz / 2),
        "cut-prev-initrd" => Some(cfg_first + old_bz + old_ird / 2),
        "cut-prev-marker" => Some(cfg_first + old_bz + old_ird),
        _ => None,
    };
    // the stick of a full-initrd build carries a 450 MB initramfs: the data volume of that machine holds the copy of it (Q1: the fixed 500 MB of the unit tests refused the run)
    let f = || 4u64 << 30;
    let left = cut.map(AtomicU64::new);
    let result = if case == "control" {
        Ok(())
    } else {
        let e = Env { dm: &dm, boot: &boot, release: rel.clone(), volatile: false, data_free: &f, fault_after: left.as_ref() };
        let p = plan(&e, &m, true).unwrap();
        stage_all(&e, &pack, &m, &p.needs);
        apply(&e, &m, true, "admin").map(|_| ())
    };
    // a finished update is armed by the update centre, exactly as `Centre::install` does after `apply`
    if case == "done" && result.is_ok() {
        // (GRUB's own counter: the loader layout keeps GRUB only as the recovery row, so a refusal there is not a failure of the case)
        let armed = super::bootcount::arm(&root);
        assert!(armed.is_ok() || loader, "{armed:?}");
    }
    let have = pair_at(&root);
    let files = if have == before { "old" } else if have == pair_of(&m) { "new" } else { "mixed" };
    // what the loader finds in `prev/` now: the files with their sizes, and the marker text (the loader offers its row only with a marker)
    let prev_dir = root.join(LOADER_PREV_DIR);
    let mut listing: Vec<String> = std::fs::read_dir(&prev_dir).map(|d| d.flatten().map(|e| format!("{}:{}", e.file_name().to_string_lossy(), e.metadata().map(|m| m.len()).unwrap_or(0))).collect()).unwrap_or_default();
    listing.sort();
    let stamp = PREV_MARKERS.iter().find_map(|n| std::fs::read_to_string(prev_dir.join(n)).ok()).map(|t| t.replace('\n', ";")).unwrap_or_default();
    crate::qeprintln!("FAULT-STATE case={case} apply={} counter={:?} files={files} prev=[{}] marker={stamp:?}", if result.is_ok() { "ok".to_string() } else { format!("err({})", result.unwrap_err().replace('\n', " ")) }, read_state(&root), listing.join(","));
    let _ = std::fs::remove_dir_all(&work);
}

/// The upgrade scenario on real packs (`dist/qemu/upgrade-test.py`, F-UPD-29): a boot partition laid out from the OLD pack (`IYAKKI_UPGRADE_OLD_PACK`, a folder with
/// `update.json`), the NEWEST pack (`IYAKKI_UPGRADE_NEW_PACK`) signed with a throw-away key and installed by the update centre's signed path, the previous version
/// checked on the data side, the counter armed and marked good, then a rollback that puts the old kernel and initramfs back. Prints one `UPGRADE-RESULT` line.
#[test]
#[ignore]
fn upgrade_from_the_old_pack_by_the_signed_path_then_rollback_on_real_packs() {
    let (Ok(old), Ok(new)) = (std::env::var("IYAKKI_UPGRADE_OLD_PACK"), std::env::var("IYAKKI_UPGRADE_NEW_PACK")) else {
        crate::qeprintln!("skipped: needs IYAKKI_UPGRADE_OLD_PACK and IYAKKI_UPGRADE_NEW_PACK (run it through dist/qemu/upgrade-test.py)");
        return;
    };
    let work = std::env::temp_dir().join(format!("iyakki-upgrade-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let (boot, data, signed) = (work.join("boot"), work.join("data"), work.join("signed"));
    std::fs::create_dir_all(&data).unwrap();
    let old_pack = read_pack(Path::new(&old), None).expect("the old pack has a readable update.json");
    // the installed state: the old pack's files on the boot partition
    for f in &old_pack.manifest.files {
        let to = boot.join(&f.path);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(Path::new(&old).join(&f.path), to).unwrap();
    }
    let old_release = std::fs::read_to_string(find_ci(&boot, "boot/iyakki-release").unwrap_or_else(|| boot.join("none"))).unwrap_or_else(|_| format!("VERSION={}\nVARIANT={}\nBUILD_DATE=\n", old_pack.manifest.version, old_pack.manifest.variant));
    put(&boot, "boot/iyakki-release", old_release.as_bytes());
    let menu = board_cfg();
    if find_ci(&boot, "boot/grub/grub.cfg").is_none() {
        put(&boot, "boot/grub/grub.cfg", menu.as_bytes());
    }
    // the newest pack, signed with a key made for this run (the copy is made of links where the file system allows it)
    let new_manifest_text = std::fs::read_to_string(Path::new(&new).join(MANIFEST)).unwrap();
    let new_m: Manifest = serde_json::from_str(&new_manifest_text).unwrap();
    for f in &new_m.files {
        let to = signed.join(&f.path);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        let from = Path::new(&new).join(&f.path);
        if std::fs::hard_link(&from, &to).is_err() {
            std::fs::copy(&from, &to).unwrap();
        }
    }
    std::fs::write(signed.join(MANIFEST), new_manifest_text).unwrap();
    let k = ed25519_dalek::SigningKey::from_bytes(&[5u8; 32]);
    sign_pack(&signed, &k).unwrap();
    let c = Centre::new(&data, Box::new(DirBoot { root: boot.clone(), capacity: 1 << 36, cluster: 4096 }), old_release, false, Some(k.verifying_key())).with_data_free(1 << 36);
    let found = read_pack(&signed, c.key()).unwrap();
    assert_eq!(found.trust, Trust::Signed);
    let old_pair = pair_at(&boot);
    let pv = c.preview(&found).unwrap();
    let said = c.install(&found, !pv.confirm.is_empty(), "admin").unwrap();
    let new_pair = pair_at(&boot);
    let want = |p: &str| new_m.files.iter().find(|f| f.path.eq_ignore_ascii_case(p)).map(|f| f.sha256.clone());
    assert_eq!(Some(new_pair.0.clone()), want("boot/bzImage"), "the new kernel stands on the boot partition");
    assert_eq!(read_state(&boot), State::Armed, "the update centre said: {said}");
    let id = std::fs::read_to_string(data.join("boot-prev/latest")).unwrap();
    let id = id.lines().next().unwrap().to_string();
    assert_eq!((hash(&data.join("boot-prev").join(&id).join("bzImage")), hash(&data.join("boot-prev").join(&id).join("initrd"))), old_pair, "the old version is kept whole");
    // the first start of the new version marks itself good
    // The running system reads its release text from its own system image, not from the boot partition: the real packs of the 2026-11 builds carry no boot/iyakki-release (so the
    // partition still says the old version after the update). The new system's text is the pack's file when there is one, else what the manifest says it is (Q6).
    let rel = find_ci(&boot, "boot/iyakki-release").and_then(|p| std::fs::read_to_string(p).ok()).filter(|t| t.contains(&new_m.version)).unwrap_or_else(|| format!("VERSION={}\nVARIANT=pc\nBUILD_DATE=\n", new_m.version));
    let running = Centre::new(&data, Box::new(DirBoot { root: boot.clone(), capacity: 1 << 36, cluster: 4096 }), rel.clone(), false, None);
    assert!(running.confirm_boot("another-boot").is_some(), "versions {:?}, release file {rel:?}, new version {}", running.versions(), new_m.version);
    assert_eq!(read_state(&boot), State::Idle);
    // the rollback
    running.rollback("admin").unwrap();
    assert_eq!(pair_at(&boot), old_pair, "the rollback put the old kernel and initramfs back");
    crate::qeprintln!("UPGRADE-RESULT old={} new={} installed=ok kept=ok marked-good=ok rollback=ok said={}", old_pack.manifest.version, new_m.version, said.chars().take(80).collect::<String>());
    let _ = std::fs::remove_dir_all(&work);
}
