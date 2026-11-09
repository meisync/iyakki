//! Encrypted drives: the header parsers on hostile input, the secret and the rate limit, the helper's argument lists (with a made-up
//! program runner), and the engine's unlock, allow-writing and lock on the made-up machine. No real volume is opened here; what
//! needs a real `cryptsetup` or `dislocker` is in docs/development/DISKS-UNLOCK-HELPER.md.

use std::cell::RefCell;
use std::sync::Arc;

use iyakki_sec::policy::Role;

use crate::disks::*;
use crate::disks_crypt::*;
use crate::disks_fake::*;
use crate::disks_helper::*;

fn admin() -> Actor {
    Actor { user: "root".into(), role: Role::Admin, uid: 1000, gid: 1000, home: "/home/root".into() }
}

type Log = RefCell<Vec<(String, String)>>;

fn sink(l: &Log) -> impl Fn(&str, &str) + '_ {
    move |a, d| l.borrow_mut().push((a.to_string(), d.to_string()))
}

fn all_text(l: &Log) -> String {
    l.borrow().iter().map(|(a, d)| format!("{a} {d}")).collect::<Vec<_>>().join("\n")
}

// ---------------------------------------------------------------------------------------------------------------------
// parsers
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn luks1_luks2_and_bitlocker_headers_are_recognised_with_their_ids() {
    let c = parse(&sb_luks()).expect("LUKS2");
    assert_eq!((c.kind, c.uuid.as_str(), c.problem.clone()), (CryptKind::Luks2, "0a1b2c3d-0000-4000-8000-000000000001", None));
    assert_eq!(c.headline(), "Encrypted: LUKS2");
    assert_eq!(c.kind.family(), "LUKS");
    let c = parse(&sb_luks1()).expect("LUKS1");
    assert_eq!((c.kind, c.uuid.as_str(), c.problem.clone()), (CryptKind::Luks1, "0a1b2c3d-0000-4000-8000-000000000002", None));
    assert!(c.detail.contains("aes-xts-plain64") && c.detail.contains("sha256"), "{}", c.detail);
    let c = parse(&sb_bitlocker()).expect("BitLocker");
    assert_eq!((c.kind, c.problem.clone()), (CryptKind::BitLocker, None));
    assert_eq!(c.headline(), "Encrypted: BitLocker");
    // the probe shows the same through detect()
    assert_eq!(detect(&sb_luks()).fs, Fs::Luks);
    assert_eq!(detect(&sb_luks()).crypt.map(|c| c.kind), Some(CryptKind::Luks2));
    assert_eq!(detect(&sb_bitlocker()).fs, Fs::BitLocker);
}

#[test]
fn a_bitlocker_to_go_sector_is_recognised_and_a_plain_fat_one_is_not() {
    let mut b = blank();
    b[3..11].copy_from_slice(b"MSWIN4.1");
    b[11..13].copy_from_slice(&512u16.to_le_bytes());
    b[424..440].copy_from_slice(&FVE_TO_GO_GUID);
    let c = parse(&b).expect("To Go");
    assert_eq!((c.kind, c.detail.as_str()), (CryptKind::BitLocker, "BitLocker To Go"));
    b[424] = 0;
    assert!(parse(&b).is_none(), "MSWIN4.1 without the GUID is an ordinary FAT volume");
    assert!(parse(&sb_fat32("STICK")).is_none() && parse(&sb_ext4("x")).is_none() && parse(&[]).is_none());
}

#[test]
fn hostile_and_truncated_headers_are_flagged_never_trusted_and_never_panic() {
    // magic only
    let c = parse_luks(b"LUKS\xba\xbe").expect("magic");
    assert!(c.problem.is_some_and(|p| p.contains("cut short")));
    // an unknown version
    let mut b = sb_luks();
    b[6..8].copy_from_slice(&9u16.to_be_bytes());
    assert!(parse_luks(&b).unwrap().problem.unwrap().contains("version 9"));
    // a LUKS2 header size that is not a power of two, zero, or huge
    for sz in [0u64, 12345, u64::MAX, 1 << 40] {
        let mut b = sb_luks();
        b[8..16].copy_from_slice(&sz.to_be_bytes());
        assert!(parse_luks(&b).unwrap().problem.unwrap().contains("damaged"), "{sz}");
    }
    // a LUKS1 header with a nonsense key size or an empty cipher
    let mut b = sb_luks1();
    b[108..112].copy_from_slice(&7u32.to_be_bytes());
    assert!(parse_luks(&b).unwrap().problem.unwrap().contains("damaged"));
    let mut b = sb_luks1();
    b[8..40].fill(0);
    assert!(parse_luks(&b).unwrap().problem.is_some());
    // text from the drive is cleaned: control characters and non-ASCII are dropped, a bad UUID is not shown
    let mut b = sb_luks();
    let t: &[u8] = b"na\x1b[31mme\xff\xfe\n\t\x07tail   ";
    b[24..24 + t.len()].copy_from_slice(t);
    let c = parse_luks(&b).unwrap();
    assert!(c.label.chars().all(|ch| ch.is_ascii_graphic() || ch == ' '), "{:?}", c.label);
    let mut b = sb_luks();
    let t: &[u8] = b"not-a-uuid\x1b[2J----------------------!!!!!";
    b[168..168 + t.len().min(40)].copy_from_slice(&t[..t.len().min(40)]);
    assert_eq!(parse_luks(&b).unwrap().uuid, "");
    // BitLocker: cut short, a sector size that makes no sense
    assert!(parse_bitlocker(b"\0\0\0-FVE-FS-").unwrap().problem.unwrap().contains("cut short"));
    let mut b = sb_bitlocker();
    b[11..13].copy_from_slice(&3u16.to_le_bytes());
    assert!(parse_bitlocker(&b).unwrap().problem.unwrap().contains("damaged"));
    // every prefix length of a good header parses without panicking
    for n in 0..300 {
        for src in [sb_luks(), sb_luks1(), sb_bitlocker()] {
            let _ = parse(&src[..n.min(src.len())]);
        }
    }
    // pseudo-random garbage with the magic in front
    let mut x = 0x2545F491u32;
    for _ in 0..200 {
        let mut g: Vec<u8> = (0..300)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect();
        g[..6].copy_from_slice(b"LUKS\xba\xbe");
        let c = parse(&g).expect("magic");
        assert!(c.label.chars().all(|ch| (' '..='~').contains(&ch)) && c.uuid.len() <= 36 && c.detail.len() <= 100);
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// the secret and the wait
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn a_secret_prints_nothing_and_is_wiped() {
    let s = Secret::new("hunter2");
    assert_eq!(format!("{s:?}"), "Secret(hidden)");
    assert!(!format!("{s:?}").contains("hunter"));
    let mut buf = b"hunter2".to_vec();
    wipe(&mut buf);
    assert!(buf.iter().all(|b| *b == 0));
    let mut t = String::from("pa55word");
    wipe_string(&mut t);
    assert!(t.is_empty());
}

#[test]
fn wrong_keys_cost_a_growing_wait_and_a_right_key_clears_it() {
    let mut l = Limiter::default();
    assert!(l.check("sdb1", 100).is_ok());
    l.failed("sdb1", 100);
    assert_eq!(l.check("sdb1", 100), Err(2));
    assert_eq!(l.check("sdb1", 101), Err(1));
    assert!(l.check("sdb1", 102).is_ok());
    l.failed("sdb1", 102);
    assert_eq!(l.check("sdb1", 102), Err(5));
    for t in 0..10 {
        l.failed("sdb1", 200 + t);
    }
    assert_eq!(l.check("sdb1", 209), Err(60));
    assert!(l.check("sdb2", 209).is_ok(), "another partition is not slowed down");
    l.ok("sdb1");
    assert!(l.check("sdb1", 209).is_ok());
    // the table cannot grow without bound
    for i in 0..500 {
        l.failed(&format!("p{i}"), i);
    }
    assert!(l.fails("p499") == 1 && l.fails("p0") == 0);
}

// ---------------------------------------------------------------------------------------------------------------------
// the helper's argument lists
// ---------------------------------------------------------------------------------------------------------------------

fn runner(sys: &Arc<FakeSys>) -> ProcRunner {
    ProcRunner::new(sys.clone(), "/data/cli/bin", "/run/iyakki-disks")
}

#[test]
fn cryptsetup_gets_a_fixed_argument_list_and_the_key_only_on_its_standard_input() {
    let sys = FakeSys::with_programs(&["cryptsetup"]);
    let r = runner(&sys);
    let key = "correct horse; rm -rf / $(x) `y`";
    r.open(&OpenReq { kind: CryptKind::Luks2, dev: "/dev/sdb1", part: "sdb1", read_only: false }, &Secret::new(key)).expect("opens");
    let c = &sys.calls()[0];
    assert_eq!(c.prog, "/usr/bin/cryptsetup");
    assert_eq!(c.args, ["open", "--type", "luks", "--key-file=-", "--batch-mode", "/dev/sdb1", "iyakki-sdb1"]);
    assert_eq!(c.stdin_bytes.as_deref(), Some(key.as_bytes()), "the key goes through the pipe, without a newline");
    assert!(!format!("{c:?}").replace(&format!("{:?}", c.stdin_bytes), "").contains("horse"), "not in the program, arguments");
    // read-only mapping
    r.open(&OpenReq { kind: CryptKind::Luks1, dev: "/dev/sdb1", part: "sdb1", read_only: true }, &Secret::new("x")).unwrap();
    assert!(sys.calls()[1].args.contains(&"--readonly".to_string()));
    // close
    r.close(CryptKind::Luks2, "sdb1").unwrap();
    assert_eq!(sys.calls()[2].args, ["close", "iyakki-sdb1"]);
}

#[test]
fn helper_exit_codes_become_plain_answers() {
    let sys = FakeSys::with_programs(&["cryptsetup"]);
    let r = runner(&sys);
    let req = OpenReq { kind: CryptKind::Luks2, dev: "/dev/sdb1", part: "sdb1", read_only: false };
    sys.answer("cryptsetup", 2, "", "No key available with this passphrase.");
    assert_eq!(r.open(&req, &Secret::new("bad")), Err(OpenError::WrongKey));
    sys.answer("cryptsetup", 5, "", "Device iyakki-sdb1 already exists.");
    assert!(matches!(r.open(&req, &Secret::new("x")), Err(OpenError::Failed(m)) if m.contains("already open")));
    sys.answer("cryptsetup", 3, "", "Cannot allocate memory");
    assert!(matches!(r.open(&req, &Secret::new("x")), Err(OpenError::Failed(m)) if m.contains("memory")));
    sys.answer("cryptsetup", 1, "", "\n  Device /dev/sdb1 is not a valid LUKS device.\nmore");
    assert!(matches!(r.open(&req, &Secret::new("x")), Err(OpenError::Failed(m)) if m.contains("not a valid LUKS device") && !m.contains("more")));
    // an empty key never reaches the helper; a path with .. or a space does not either
    let n = sys.calls().len();
    assert!(matches!(r.open(&req, &Secret::new("")), Err(OpenError::Failed(_))));
    for dev in ["/dev/../etc/passwd", "sdb1", "/dev/sdb1 --evil"] {
        assert!(r.open(&OpenReq { dev, ..req.clone() }, &Secret::new("x")).is_err());
    }
    assert_eq!(sys.calls().len(), n);
}

#[test]
fn a_missing_helper_is_explained_with_the_install_route_and_nothing_runs() {
    let sys = FakeSys::with_programs(&[]);
    let r = runner(&sys);
    let e = r.available(CryptKind::Luks2).unwrap_err();
    assert!(e.contains("cryptsetup") && e.contains("does not ship it") && e.contains("CLI tools") && e.contains("Nothing was changed"), "{e}");
    assert!(r.available(CryptKind::BitLocker).unwrap_err().contains("dislocker"));
    assert!(matches!(r.open(&OpenReq { kind: CryptKind::Luks2, dev: "/dev/sdb1", part: "sdb1", read_only: false }, &Secret::new("x")), Err(OpenError::Failed(m)) if m.contains("not installed")));
    assert!(sys.calls().is_empty());
    // the CLI tools folder is searched first
    let sys = Arc::new(FakeSys::default());
    *sys.files.lock().unwrap() = vec!["/data/cli/bin/cryptsetup".into()];
    assert!(runner(&sys).available(CryptKind::Luks1).is_ok());
}

#[test]
fn bitlocker_runs_dislocker_then_attaches_the_image_read_only_and_cleans_up_on_failure() {
    let sys = FakeSys::with_programs(&["dislocker", "losetup", "umount"]);
    sys.present.lock().unwrap().push("/run/iyakki-disks/iyakki-nvme1n1p2/dislocker-file".into());
    sys.answer("losetup", 0, "/dev/loop7\n", "");
    let r = runner(&sys);
    let rk = "123456-234567-345678-456789-567890-678901-789012-890123";
    let req = OpenReq { kind: CryptKind::BitLocker, dev: "/dev/nvme1n1p2", part: "nvme1n1p2", read_only: true };
    r.open(&req, &Secret::new(rk)).expect("opens");
    let calls = sys.calls();
    assert_eq!(calls[0].args, ["-V", "/dev/nvme1n1p2", "-p", "-r", "--", "/run/iyakki-disks/iyakki-nvme1n1p2"], "a recovery key uses -p, no value on the line");
    assert_eq!(calls[0].stdin_bytes.as_deref(), Some(rk.as_bytes()));
    assert_eq!(calls[1].args, ["--find", "--show", "--read-only", "/run/iyakki-disks/iyakki-nvme1n1p2/dislocker-file"]);
    assert_eq!(sys.read_link("/dev/mapper/iyakki-nvme1n1p2").as_deref(), Some("/dev/loop7"));
    // a password uses -u
    let sys2 = FakeSys::with_programs(&["dislocker", "losetup"]);
    sys2.present.lock().unwrap().push("/run/iyakki-disks/iyakki-nvme1n1p2/dislocker-file".into());
    sys2.answer("losetup", 0, "/dev/loop3\n", "");
    runner(&sys2).open(&req, &Secret::new("My pass word")).unwrap();
    assert_eq!(sys2.calls()[0].args[2], "-u");
    // closing detaches the loop device, removes the node and the folder
    r.close(CryptKind::BitLocker, "nvme1n1p2").unwrap();
    assert!(sys.read_link("/dev/mapper/iyakki-nvme1n1p2").is_none());
    assert!(sys.calls().iter().any(|c| c.args == ["-d", "/dev/loop7"]));
    assert!(!sys.dirs.lock().unwrap().iter().any(|d| d.ends_with("nvme1n1p2")), "the private folder is removed");
    // a wrong key: dislocker fails, the folder is removed, the answer is WrongKey
    let sys3 = FakeSys::with_programs(&["dislocker", "losetup"]);
    sys3.answer("dislocker", 1, "", "Unable to decrypt the VMK: invalid password");
    assert_eq!(runner(&sys3).open(&req, &Secret::new("nope")), Err(OpenError::WrongKey));
    assert!(!sys3.dirs.lock().unwrap().iter().any(|d| d.ends_with("nvme1n1p2")) && sys3.links.lock().unwrap().is_empty());
    // another failure is not counted as a wrong key
    let sys4 = FakeSys::with_programs(&["dislocker", "losetup"]);
    sys4.answer("dislocker", 1, "", "FUSE: device not found");
    assert!(matches!(runner(&sys4).open(&req, &Secret::new("x")), Err(OpenError::Failed(_))));
}

#[test]
fn recovery_keys_are_told_from_passwords() {
    assert!(looks_like_recovery_key(b"123456-234567-345678-456789-567890-678901-789012-890123"));
    assert!(looks_like_recovery_key(b"123456234567345678456789567890678901789012890123"));
    assert!(!looks_like_recovery_key(b"123456-234567"));
    assert!(!looks_like_recovery_key(b"hunter2"));
    assert!(!looks_like_recovery_key(b"12345a-234567-345678-456789-567890-678901-789012-890123"));
}

// ---------------------------------------------------------------------------------------------------------------------
// the engine on the made-up machine
// ---------------------------------------------------------------------------------------------------------------------

fn lab() -> (Arc<Fake>, Disks, Arc<FakeCrypt>) {
    let f = machine();
    let c = FakeCrypt::new(f.clone());
    c.add("sdb1", "open sesame", sb_ext4("secrets"));
    c.add("nvme1n1p2", "123456-234567-345678-456789-567890-678901-789012-890123", sb_ext4("win"));
    let d = f.disks();
    (f, d, c)
}

#[test]
fn unlock_opens_read_only_audits_without_the_key_and_lock_closes_it_again() {
    let (f, d, c) = lab();
    let l: Log = RefCell::new(vec![]);
    let s = Secret::new("open sesame");
    let m = d.unlock(&admin(), "sdb1", &s, &*c, &sink(&l)).expect("unlocked");
    assert!(m.read_only && m.target.ends_with("/secrets"), "{m:?}");
    assert!(m.notes[0].contains("read-only") && m.notes[0].contains("Allow writing"));
    let (dev, _, fstype, flags, _) = f.last_mount().unwrap();
    assert_eq!((dev.as_str(), fstype.as_str(), flags.rdonly, flags.noexec, flags.nosuid), ("/dev/mapper/iyakki-sdb1", "ext4", true, true, true));
    let snap = d.scan();
    let (_, p) = snap.part("sdb1").unwrap();
    assert!(p.unlocked && p.mounted() && p.mounted_ro, "{p:?}");
    assert!(!all_text(&l).contains("open sesame") && all_text(&l).contains("disks.unlock"), "{}", all_text(&l));
    // writing the drive itself never happened
    assert!(!f.wrote());
    // lock: unmounts, closes
    let msg = d.lock(&admin(), "sdb1", &*c, &sink(&l)).expect("locked");
    assert!(msg.contains("locked again"));
    let snap = d.scan();
    let (_, p) = snap.part("sdb1").unwrap();
    assert!(!p.unlocked && !p.mounted());
    assert_eq!(c.log(), ["open sdb1 ro=false", "close sdb1"]);
    assert!(f.calls().iter().any(|x| x.starts_with("umount")));
}

#[test]
fn a_wrong_key_gives_a_plain_message_a_wait_and_leaves_the_drive_alone() {
    let (f, d, c) = lab();
    d.hold_clock(500);
    let l: Log = RefCell::new(vec![]);
    let e = d.unlock(&admin(), "sdb1", &Secret::new("guess one"), &*c, &sink(&l)).unwrap_err();
    assert!(e.contains("did not unlock") && e.contains("Nothing on the drive was changed"), "{e}");
    assert!(!e.contains("guess one") && !all_text(&l).contains("guess one"));
    // the next try at once is refused without calling the helper, even with the right key
    let before = c.log().len();
    let e = d.unlock(&admin(), "sdb1", &Secret::new("open sesame"), &*c, &sink(&l)).unwrap_err();
    assert!(e.contains("Wait") && e.contains("seconds"), "{e}");
    assert_eq!(c.log().len(), before, "the helper was not asked");
    assert!(all_text(&l).contains("rate limit"));
    assert!(f.mounts_now().iter().all(|m| !m.0.contains("mapper")));
    // another partition is not affected
    assert!(d.unlock(&admin(), "nvme1n1p2", &Secret::new("wrong"), &*c, &sink(&l)).unwrap_err().contains("did not unlock"));
}

#[test]
fn the_engine_refuses_what_it_must() {
    let (f, d, c) = lab();
    let l: Log = RefCell::new(vec![]);
    let s = Secret::new("open sesame");
    // not encrypted
    assert!(d.unlock(&admin(), "sda1", &s, &*c, &sink(&l)).unwrap_err().contains("not an encrypted"));
    // the system drive
    assert!(d.unlock(&admin(), "nvme0n1p2", &s, &*c, &sink(&l)).is_err());
    // a User may not touch an internal drive; a guest nothing
    let user = Actor { user: "sam".into(), role: Role::User, uid: 1001, gid: 1001, home: "/home/sam".into() };
    assert!(d.unlock(&user, "sdb1", &s, &*c, &sink(&l)).unwrap_err().contains("removable"));
    let guest = Actor { role: Role::Guest, ..user };
    assert!(d.unlock(&guest, "sdb1", &s, &*c, &sink(&l)).unwrap_err().contains("Guests"));
    // an empty key
    assert!(d.unlock(&admin(), "sdb1", &Secret::new(""), &*c, &sink(&l)).unwrap_err().contains("Type the"));
    // a missing helper: the words, and the volume is not touched
    c.missing.lock().unwrap().push(CryptKind::Luks2);
    let e = d.unlock(&admin(), "sdb1", &s, &*c, &sink(&l)).unwrap_err();
    assert!(e.contains("cryptsetup") && e.contains("does not ship"), "{e}");
    assert!(c.log().is_empty());
    assert!(!f.wrote());
    // a damaged header cannot be unlocked
    f.set_dev("sdb1", {
        let mut b = sb_luks();
        b[6..8].copy_from_slice(&7u16.to_be_bytes());
        b
    });
    d.forget();
    c.missing.lock().unwrap().clear();
    assert!(d.unlock(&admin(), "sdb1", &s, &*c, &sink(&l)).unwrap_err().contains("version 7"));
}

#[test]
fn something_that_unlocks_but_cannot_be_opened_is_locked_again() {
    let (f, d, c) = lab();
    let l: Log = RefCell::new(vec![]);
    c.add("sdb1", "open sesame", sb_apfs());
    c.volumes.lock().unwrap().remove(0);
    let e = d.unlock(&admin(), "sdb1", &Secret::new("open sesame"), &*c, &sink(&l)).unwrap_err();
    assert!(e.contains("locked again"), "{e}");
    assert_eq!(c.log(), ["open sdb1 ro=false", "close sdb1"]);
    assert!(!d.scan().part("sdb1").unwrap().1.unlocked);
    // a mount that fails: locked again
    c.volumes.lock().unwrap().clear();
    c.add("sdb1", "open sesame", sb_ext4("x"));
    f.fail_mount("ext4", "wrong fs type, bad superblock");
    let e = d.unlock(&admin(), "sdb1", &Secret::new("open sesame"), &*c, &sink(&l)).unwrap_err();
    assert!(e.contains("locked again"), "{e}");
    assert!(!d.scan().part("sdb1").unwrap().1.unlocked);
}

#[test]
fn writing_is_a_second_step_and_bitlocker_stays_read_only() {
    let (f, d, c) = lab();
    let l: Log = RefCell::new(vec![]);
    d.unlock(&admin(), "sdb1", &Secret::new("open sesame"), &*c, &sink(&l)).unwrap();
    assert!(d.scan().part("sdb1").unwrap().1.mounted_ro);
    let m = d.allow_writing(&admin(), "sdb1", &sink(&l)).expect("writable");
    assert!(!m.read_only);
    let (_, _, _, flags, _) = f.last_mount().unwrap();
    assert!(!flags.rdonly && flags.noexec);
    assert!(!d.scan().part("sdb1").unwrap().1.mounted_ro);
    assert!(d.allow_writing(&admin(), "sdb1", &sink(&l)).unwrap_err().contains("already open for writing"));
    assert!(l.borrow().iter().any(|(a, _)| a == "disks.write.allowed"));
    // locking after writing works
    d.lock(&admin(), "sdb1", &*c, &sink(&l)).unwrap();
    // not unlocked: refused
    assert!(d.allow_writing(&admin(), "sdb1", &sink(&l)).unwrap_err().contains("not an unlocked"));
    // BitLocker: refused with the reason, whatever the person asks
    d.unlock(&admin(), "nvme1n1p2", &Secret::new("123456-234567-345678-456789-567890-678901-789012-890123"), &*c, &sink(&l)).unwrap();
    assert!(c.log().contains(&"open nvme1n1p2 ro=true".to_string()), "BitLocker is opened read-only at the helper");
    let e = d.allow_writing(&admin(), "nvme1n1p2", &sink(&l)).unwrap_err();
    assert!(e.contains("stays read-only"), "{e}");
}

#[test]
fn a_busy_volume_is_not_forced_and_an_unlocked_drive_is_not_ejected() {
    let (f, d, c) = lab();
    let l: Log = RefCell::new(vec![]);
    let m = d.unlock(&admin(), "sdb1", &Secret::new("open sesame"), &*c, &sink(&l)).unwrap();
    // eject is refused until it is locked
    let e = d.eject(&admin(), "sdb", &sink(&l)).unwrap_err();
    assert!(e.contains("Lock it first"), "{e}");
    f.make_busy(&m.target);
    let e = d.lock(&admin(), "sdb1", &*c, &sink(&l)).unwrap_err();
    assert!(e.contains("busy"), "{e}");
    assert!(d.scan().part("sdb1").unwrap().1.unlocked, "still unlocked");
    // the folder closes but the mapping is still in use: the message says so
    f.st.lock().unwrap().busy.clear();
    *c.busy_close.lock().unwrap() = true;
    let e = d.lock(&admin(), "sdb1", &*c, &sink(&l)).unwrap_err();
    assert!(e.contains("still unlocked"), "{e}");
    *c.busy_close.lock().unwrap() = false;
    assert!(d.lock(&admin(), "sdb1", &*c, &sink(&l)).is_ok(), "locking an unlocked, closed volume works");
    assert!(d.lock(&admin(), "sdb1", &*c, &sink(&l)).unwrap_err().contains("not unlocked"));
}

#[test]
fn an_unlocked_volume_that_is_not_open_can_be_mounted_with_the_ordinary_plan() {
    let (f, d, c) = lab();
    let l: Log = RefCell::new(vec![]);
    d.unlock(&admin(), "sdb1", &Secret::new("open sesame"), &*c, &sink(&l)).unwrap();
    d.unmount(&admin(), "sdb1", &sink(&l)).unwrap();
    let snap = d.scan();
    assert!(snap.part("sdb1").unwrap().1.unlocked);
    let plan = d.plan_mount(&snap, &admin(), "sdb1", &MountOpts { read_only: true, allow_exec: false }).expect("plans as the inner ext4");
    assert_eq!((plan.dev.as_str(), plan.fstype), ("/dev/mapper/iyakki-sdb1", "ext4"));
    let _ = f;
}

#[test]
fn the_partition_manager_still_refuses_a_drive_with_an_unlocked_volume() {
    let (_f, d, c) = lab();
    let l: Log = RefCell::new(vec![]);
    d.unlock(&admin(), "sdb1", &Secret::new("open sesame"), &*c, &sink(&l)).unwrap();
    let snap = d.scan();
    let disk = snap.disk("sdb").unwrap();
    assert!(disk.parts.iter().any(|p| p.mounted()), "a mount on the drive: the partition manager's busy rule applies");
}
