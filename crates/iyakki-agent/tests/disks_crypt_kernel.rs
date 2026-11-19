//! Real `cryptsetup` and dm-crypt through the DISKS tile's own helper runner (ignored by default).
//!
//! Needs root (device-mapper), `cryptsetup`, `losetup`, `mkfs.ext4` or `mke2fs`, and the kernel's `dm_crypt` and `loop`. Run it only through
//! `dist/linux-host/kernel-tests.sh`, which starts it in a private mount namespace as root. The volume is a file in a scratch folder
//! on a loop device; the mapping is named `iyakki-m80c1` / `iyakki-m80c2` and removed at the end (also when the test fails). Rows: F-DISKS-08 (LUKS1 and LUKS2).

use iyakki_agent::disks_crypt::{mapper_path, CryptKind, Secret};
use iyakki_agent::disks_helper::{CryptRunner, OpenError, OpenReq, ProcRunner};
use std::process::Command;

fn sh(cmd: &str) -> (bool, String) {
    let o = Command::new("sh").args(["-c", &format!("export PATH=$PATH:/usr/sbin:/sbin; {cmd}")]).output().expect("sh");
    (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

fn have(t: &str) -> bool {
    sh(&format!("command -v {t} >/dev/null")).0
}

struct Cleanup {
    parts: Vec<String>,
    loops: Vec<String>,
    dir: std::path::PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in &self.parts {
            let _ = sh(&format!("umount {m} 2>/dev/null; cryptsetup close {n}", m = self.dir.join(p).display(), n = iyakki_agent::disks_crypt::mapper_name(p)));
        }
        for l in &self.loops {
            let _ = sh(&format!("losetup -d {l}"));
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
#[ignore]
fn real_cryptsetup_opens_luks1_and_luks2_with_the_right_key_refuses_a_wrong_one_and_closes() {
    // SAFETY: getuid has no preconditions.
    let root = unsafe { libc::getuid() } == 0 && std::env::var("IYAKKI_IN_SANDBOX").as_deref() == Ok("1");
    if !(root && have("cryptsetup") && have("losetup") && have("mke2fs") && std::path::Path::new("/dev/mapper/control").exists()) {
        eprintln!("skipped: needs root, cryptsetup, losetup, mke2fs and device-mapper (run it through dist/linux-host/kernel-tests.sh)");
        return;
    }
    let dir = std::env::temp_dir().join(format!("iyakki-luks-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut clean = Cleanup { parts: vec![], loops: vec![], dir: dir.clone() };
    let runner = ProcRunner::real(std::path::Path::new("/nonexistent-cli-bin"));
    let key = "correct horse battery staple";
    for (kind, ty, part) in [(CryptKind::Luks2, "luks2", "m80c2"), (CryptKind::Luks1, "luks1", "m80c1")] {
        let img = dir.join(format!("{part}.img"));
        let (ok, out) = sh(&format!("truncate -s 64M {i} && printf %s '{key}' | cryptsetup luksFormat --type {ty} --batch-mode --pbkdf-force-iterations 1000 --pbkdf pbkdf2 --key-file=- {i}", i = img.display()));
        assert!(ok, "luksFormat {ty}: {out}");
        // put an ext4 file system with a known file inside, so a successful open can be told from a mapping of garbage
        let (ok, out) = sh(&format!("printf %s '{key}' | cryptsetup open --type luks --key-file=- {i} m80prep-{part} && mke2fs -q -t ext4 -L m80inside /dev/mapper/m80prep-{part} && mkdir -p {d}/prep && mount /dev/mapper/m80prep-{part} {d}/prep && echo inside-the-vault > {d}/prep/hello.txt && umount {d}/prep && cryptsetup close m80prep-{part}", i = img.display(), d = dir.display()));
        assert!(ok, "prepare {ty}: {out}");
        let (ok, lo) = sh(&format!("losetup -f --show {}", img.display()));
        assert!(ok, "losetup: {lo}");
        let dev = lo.trim().to_string();
        clean.loops.push(dev.clone());
        clean.parts.push(part.to_string());
        assert_eq!(runner.available(kind), Ok(()), "{ty}: cryptsetup is found");

        let req = OpenReq { kind, dev: &dev, part, read_only: true };
        // a wrong key: cryptsetup's exit code 2, which the runner turns into WrongKey
        let wrong = runner.open(&req, &Secret::new("not the key"));
        assert!(matches!(wrong, Err(OpenError::WrongKey)), "{ty}: a wrong key must be reported as WrongKey, got {wrong:?}");
        assert!(!std::path::Path::new(&mapper_path(part)).exists(), "{ty}: nothing was opened by the wrong key");
        // an empty key is refused before anything runs
        assert!(matches!(runner.open(&req, &Secret::new("")), Err(OpenError::Failed(_))));
        // the right key, read-only: the mapping appears, is read-only, and holds our file system and file
        runner.open(&req, &Secret::new(key)).unwrap_or_else(|e| panic!("{ty}: open with the right key: {e:?}"));
        let m = mapper_path(part);
        assert!(std::path::Path::new(&m).exists(), "{ty}: {m} appeared");
        let (ok, out) = sh(&format!("blockdev --getro {m}"));
        assert!(ok && out.trim() == "1", "{ty}: the mapping is read-only: {out}");
        let (ok, out) = sh(&format!("mkdir -p {d}/{part} && mount -o ro {m} {d}/{part} && cat {d}/{part}/hello.txt", d = dir.display()));
        assert!(ok && out.contains("inside-the-vault"), "{ty}: the file inside the volume reads back: {out}");
        let (ok, out) = sh(&format!("cryptsetup status {n}", n = iyakki_agent::disks_crypt::mapper_name(part)));
        assert!(ok && out.contains(&format!("type:    {}", ty.to_uppercase())), "{ty}: status names the type: {out}");
        // opening it again says it is already open (cryptsetup exit code 5), it does not damage anything
        let again = runner.open(&req, &Secret::new(key));
        assert!(matches!(again, Err(OpenError::Failed(ref w)) if w.contains("already open") || w.contains("in use")), "{ty}: a second open: {again:?}");
        // still in use (mounted): close is refused with the plain words; after unmount it works and the mapping is gone
        let busy = runner.close(kind, part);
        assert!(busy.as_ref().is_err_and(|w| w.contains("still in use")), "{ty}: close while mounted: {busy:?}");
        let (ok, out) = sh(&format!("umount {d}/{part}", d = dir.display()));
        assert!(ok, "{out}");
        runner.close(kind, part).unwrap_or_else(|e| panic!("{ty}: close: {e}"));
        assert!(!std::path::Path::new(&m).exists(), "{ty}: the mapping is gone after close");
        // read-write: the same key opens it writable
        let rw = OpenReq { kind, dev: &dev, part, read_only: false };
        runner.open(&rw, &Secret::new(key)).unwrap_or_else(|e| panic!("{ty}: open read-write: {e:?}"));
        let (_, ro) = sh(&format!("blockdev --getro {m}"));
        assert_eq!(ro.trim(), "0", "{ty}: opened writable");
        runner.close(kind, part).unwrap();
    }
}

#[test]
#[ignore]
fn real_mkfs_exfat_mkntfs_ntfsresize_and_resize2fs_run_through_the_disks_tools_runner() {
    use iyakki_agent::disks_fmt::*;
    // SAFETY: getuid has no preconditions.
    let root = unsafe { libc::getuid() } == 0 && std::env::var("IYAKKI_IN_SANDBOX").as_deref() == Ok("1");
    if !(root && ["mkfs.exfat", "mkntfs", "ntfsresize", "resize2fs", "e2fsck", "fsck.exfat", "blkid", "losetup", "mke2fs", "dumpe2fs"].iter().all(|t| have(t))) {
        eprintln!("skipped: needs root, mkfs.exfat, fsck.exfat, mkntfs, ntfsresize, resize2fs, e2fsck, mke2fs, dumpe2fs, blkid, losetup (run it through dist/linux-host/kernel-tests.sh)");
        return;
    }
    let dir = std::env::temp_dir().join(format!("iyakki-fmt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut clean = Cleanup { parts: vec![], loops: vec![], dir: dir.clone() };
    let img = dir.join("disk.img");
    let size: u64 = 96 << 20;
    let (ok, out) = sh(&format!("truncate -s {size} {}", img.display()));
    assert!(ok, "{out}");
    let (ok, lo) = sh(&format!("losetup -f --show {}", img.display()));
    assert!(ok, "losetup: {lo}");
    let dev = lo.trim().to_string();
    clean.loops.push(dev.clone());
    let tools = ProcTools::real(std::path::Path::new("/nonexistent-cli-bin"));
    for t in [Tool::MkfsExfat, Tool::Mkntfs, Tool::E2fsck, Tool::Resize2fs, Tool::Ntfsresize] {
        assert!(tools.find(t).is_some(), "{} is found", t.prog());
    }
    let run = |t: Tool, args: Vec<String>| tools.run(t, &args).unwrap_or_else(|e| panic!("{}: {e}", t.prog()));
    let blkid = |field: &str| sh(&format!("blkid -p -o value -s {field} {dev}")).1.trim().to_string();

    // exFAT: made with a label and clean under fsck.exfat
    let o = run(Tool::MkfsExfat, mkfs_exfat_args(&dev, "BACKUP").unwrap());
    assert!(exit_ok(Tool::MkfsExfat, o.code), "mkfs.exfat: {} {}", o.stdout, o.stderr);
    assert_eq!((blkid("TYPE").as_str(), blkid("LABEL").as_str()), ("exfat", "BACKUP"));
    assert!(sh(&format!("fsck.exfat -n {dev}")).0, "fsck.exfat finds the new volume clean");

    // NTFS: made with a label, checked, the smallest size read from `--info`, shrunk to it plus a margin, checked, grown back
    let o = run(Tool::Mkntfs, mkntfs_args(&dev, "Data").unwrap());
    assert!(exit_ok(Tool::Mkntfs, o.code), "mkntfs: {} {}", o.stdout, o.stderr);
    assert_eq!((blkid("TYPE").as_str(), blkid("LABEL").as_str()), ("ntfs", "Data"));
    let o = run(Tool::Ntfsresize, ntfsresize_check_args(&dev).unwrap());
    assert!(exit_ok(Tool::Ntfsresize, o.code), "ntfsresize --check: {} {}", o.stdout, o.stderr);
    let o = run(Tool::Ntfsresize, ntfsresize_min_args(&dev).unwrap());
    let min = parse_min_bytes(Tool::Ntfsresize, &o.stdout, 0).unwrap_or_else(|| panic!("the line the parser reads is not in ntfsresize's answer:\n{}{}", o.stdout, o.stderr));
    assert!(min > 0 && min < size, "the smallest size is plausible: {min}");
    let target = (min + (8 << 20) + 0xfffff) & !0xfffff;
    let o = run(Tool::Ntfsresize, ntfsresize_args(&dev, Some(target)).unwrap());
    assert!(exit_ok(Tool::Ntfsresize, o.code), "ntfsresize --force --size {target} did not run without a prompt: {} {}", o.stdout, o.stderr);
    // after a resize NTFS is flagged "scheduled for check" (Windows runs chkdsk at its next mount, which is expected): ntfsresize itself then wants --force to look again
    let forced = |what: &str| sh(&format!("ntfsresize --force {what} {dev} 2>&1"));
    let info = forced("--info").1;
    let vol = info.lines().find(|l| l.contains("Current volume size")).unwrap_or_else(|| panic!("no volume size line:\n{info}")).to_string();
    let vol_bytes = |l: &str| -> u64 { l.split("Current volume size:").nth(1).and_then(|r| r.split_whitespace().next()).and_then(|n| n.parse().ok()).unwrap_or(0) };
    let got = vol_bytes(&vol);
    assert!(got <= target && target - got < 16384, "the volume is about {target} bytes after the shrink (rounded down to a whole cluster): {vol}");
    assert!(forced("--check").0, "clean after the shrink");
    let o = run(Tool::Ntfsresize, ntfsresize_args(&dev, None).unwrap());
    assert!(exit_ok(Tool::Ntfsresize, o.code), "grow back: {} {}", o.stdout, o.stderr);
    let info = forced("--info").1;
    let got = vol_bytes(info.lines().find(|l| l.contains("Current volume size")).unwrap_or(""));
    assert!(got <= size && size - got < 16384, "back to the whole partition (to a whole cluster): {info}");

    // ext4: made, checked (e2fsck -f -p), the smallest size from `resize2fs -P`, shrunk, checked, grown back
    let (ok, out) = sh(&format!("mke2fs -q -F -t ext4 -b 4096 -L fmt {dev}"));
    assert!(ok, "{out}");
    let o = run(Tool::E2fsck, e2fsck_args(&dev).unwrap());
    assert!(exit_ok(Tool::E2fsck, o.code), "e2fsck: {} {}", o.stdout, o.stderr);
    let o = run(Tool::Resize2fs, resize2fs_min_args(&dev).unwrap());
    let min = parse_min_bytes(Tool::Resize2fs, &format!("{}{}", o.stdout, o.stderr), 4096).unwrap_or_else(|| panic!("the line the parser reads is not in resize2fs -P's answer:\n{}{}", o.stdout, o.stderr));
    assert!(min > 0 && min < size, "{min}");
    let target = (min + (8 << 20) + 0xfffff) & !0xfffff;
    let o = run(Tool::Resize2fs, resize2fs_args(&dev, Some(target)).unwrap());
    assert!(exit_ok(Tool::Resize2fs, o.code), "resize2fs shrink: {} {}", o.stdout, o.stderr);
    let blocks = |dev: &str| -> u64 { sh(&format!("dumpe2fs -h {dev} 2>/dev/null | sed -n 's/^Block count: *//p'")).1.trim().parse().unwrap_or(0) };
    assert_eq!(blocks(&dev) * 4096, target, "the file system has the asked size");
    assert!(exit_ok(Tool::E2fsck, run(Tool::E2fsck, e2fsck_args(&dev).unwrap()).code), "clean after the shrink");
    let o = run(Tool::Resize2fs, resize2fs_args(&dev, None).unwrap());
    assert!(exit_ok(Tool::Resize2fs, o.code), "resize2fs grow: {} {}", o.stdout, o.stderr);
    assert_eq!(blocks(&dev) * 4096, size, "grown back to the whole partition");
    assert!(exit_ok(Tool::E2fsck, run(Tool::E2fsck, e2fsck_args(&dev).unwrap()).code), "clean after the grow");
}
