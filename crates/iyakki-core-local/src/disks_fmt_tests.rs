//! Tests of formatting as exFAT or NTFS and of resizing (`disks_fmt.rs`, `disks_part.rs`). Made-up drives and a made-up program runner
//! only: no real format, partition or resize command is ever started, and no real disk or loop device is touched.

use std::cell::RefCell;
use std::sync::Arc;

use iyakki_sec::policy::Role;

use crate::disks::*;
use crate::disks_fake::*;
use crate::disks_fmt::*;
use crate::disks_helper::{FakeCrypt, FakeSys, ProcOut};
use crate::disks_part::*;

fn admin() -> Actor {
    Actor { user: "root".into(), role: Role::Admin, uid: 1000, gid: 1000, home: "/home/root".into() }
}

fn confirm(typed: &str) -> Confirm {
    Confirm { typed: typed.into(), second: true }
}

type Log = RefCell<Vec<(String, String)>>;

fn sink(l: &Log) -> impl Fn(&str, &str) + '_ {
    move |a, d| l.borrow_mut().push((a.to_string(), d.to_string()))
}

fn all_text(l: &Log) -> String {
    l.borrow().iter().map(|(a, d)| format!("{a} {d}")).collect::<Vec<_>>().join("\n")
}

/// The made-up programs: they answer from a script and, like the real ones, leave a file system behind.
struct Sim {
    env: Arc<Fake>,
    inner: ProcTools,
}

impl FsTools for Sim {
    fn find(&self, t: Tool) -> Option<String> {
        self.inner.find(t)
    }
    fn run(&self, t: Tool, args: &[String]) -> Result<ProcOut, String> {
        let out = self.inner.run(t, args)?;
        if out.code == 0 {
            let dev = args.last().cloned().unwrap_or_default();
            match t {
                Tool::MkfsExfat => self.env.dev_write(&dev, 0, &sb_exfat()).unwrap(),
                Tool::Mkntfs => self.env.dev_write(&dev, 0, &sb_ntfs()).unwrap(),
                _ => {}
            }
        }
        Ok(out)
    }
}

const ALL: [&str; 5] = ["mkfs.exfat", "mkntfs", "e2fsck", "resize2fs", "ntfsresize"];

/// A blank stick with a GPT, the engine with the made-up programs, and the programs' record.
fn lab(programs: &[&str]) -> (Arc<Fake>, Disks, Arc<FakeSys>) {
    let f = machine();
    blank_stick(&f);
    let d = f.disks();
    let sys = FakeSys::with_programs(programs);
    d.set_tools(Arc::new(Sim { env: f.clone(), inner: ProcTools::new(sys.clone(), "/none") }));
    d.apply_partition(&admin(), "sdc", &PartOp::NewTable(Table::Gpt), &confirm("sdc"), &|_, _| {}).unwrap();
    (f, d, sys)
}

fn go(d: &Disk_, op: PartOp, typed: &str, l: &Log) -> Result<String, String> {
    d.apply_partition(&admin(), "sdc", &op, &confirm(typed), &sink(l))
}

type Disk_ = Disks;

fn add(d: &Disks, mb: u64, fs: FormatFs, label: &str) {
    d.apply_partition(&admin(), "sdc", &PartOp::Add { size_bytes: Some(mb << 20), fs: Some(fs), label: label.into() }, &confirm("sdc"), &|_, _| {}).unwrap();
}

fn size_of(d: &Disks, part: &str) -> u64 {
    d.forget();
    d.scan().part(part).unwrap().1.size_bytes
}

fn fs_of(d: &Disks, part: &str) -> Fs {
    d.forget();
    d.scan().part(part).unwrap().1.fs
}

// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn argument_lists_are_fixed_checked_and_never_a_shell_string() {
    assert_eq!(mkfs_exfat_args("/dev/sdc1", "BACKUP").unwrap(), ["-L", "BACKUP", "/dev/sdc1"]);
    assert_eq!(mkfs_exfat_args("/dev/sdc1", "").unwrap(), ["/dev/sdc1"]);
    assert_eq!(mkntfs_args("/dev/sdc1", "Data").unwrap(), ["-Q", "-F", "-L", "Data", "/dev/sdc1"]);
    assert_eq!(e2fsck_args("/dev/sdc1").unwrap(), ["-f", "-p", "/dev/sdc1"]);
    assert_eq!(resize2fs_args("/dev/sdc1", Some(40 << 20)).unwrap(), ["/dev/sdc1", "40960K"]);
    assert_eq!(resize2fs_args("/dev/sdc1", None).unwrap(), ["/dev/sdc1"]);
    assert_eq!(ntfsresize_args("/dev/sdc1", Some(1 << 30)).unwrap(), ["--force", "--size", "1073741824", "/dev/sdc1"]);
    assert_eq!(ntfsresize_args("/dev/sdc1", None).unwrap(), ["--force", "/dev/sdc1"]);
    for bad in ["/dev/../etc/passwd", "/etc/passwd", "/dev/sdc1; reboot", "/dev/sdc1 -x", "sdc1", "/dev/", "/dev/$(id)"] {
        assert!(dev_ok(bad).is_err(), "{bad}");
        assert!(mkfs_exfat_args(bad, "x").is_err() && mkntfs_args(bad, "x").is_err() && resize2fs_args(bad, None).is_err(), "{bad}");
    }
    assert!(mkfs_exfat_args("/dev/sdc1", "TWELVE-CHARS").is_err(), "exFAT labels have 11 characters at most");
    assert!(mkfs_exfat_args("/dev/sdc1", "a\nb").is_err() && mkntfs_args("/dev/sdc1", "a\u{7}").is_err(), "no control characters");
}

#[test]
fn the_smallest_size_and_the_exit_codes_are_read_carefully() {
    assert_eq!(parse_min_bytes(Tool::Resize2fs, "resize2fs 1.47\nEstimated minimum size of the filesystem: 5000\n", 4096), Some(5000 * 4096));
    assert_eq!(parse_min_bytes(Tool::Ntfsresize, "Cluster size : 4096 bytes\nYou might resize at 123456789 bytes or 124 MB (freeing 5 MB).\n", 1), Some(123_456_789));
    for junk in ["", "Estimated minimum size of the filesystem: lots", "nothing useful"] {
        assert_eq!(parse_min_bytes(Tool::Resize2fs, junk, 1024), None, "{junk}");
        assert_eq!(parse_min_bytes(Tool::Ntfsresize, junk, 1), None, "{junk}");
    }
    assert!(exit_ok(Tool::E2fsck, 0) && exit_ok(Tool::E2fsck, 1) && !exit_ok(Tool::E2fsck, 4) && !exit_ok(Tool::Mkntfs, 1));
}

#[test]
fn the_dry_run_names_what_will_be_erased_and_starts_nothing() {
    let (f, d, sys) = lab(&ALL);
    add(&d, 40, FormatFs::Fat32, "backup");
    let calls = f.calls().len();
    let plan = d.plan_partition(&admin(), "sdc", &PartOp::Format { number: 1, fs: FormatFs::Exfat, label: "BACKUP".into() }).unwrap();
    let text = plan.render();
    assert!(text.contains("Everything on /dev/sdc1 (41.9 MB, label BACKUP) will be erased."), "{text}");
    assert!(text.contains("mkfs.exfat") && text.contains("Format partition 1 as exFAT"), "{text}");
    assert!(plan.writes_anything());
    assert_eq!(f.calls().len(), calls, "the dry run changes nothing");
    assert!(sys.calls().is_empty(), "and starts no program");
    // delete says the same thing in the same words
    let del = d.plan_partition(&admin(), "sdc", &PartOp::Delete { number: 1 }).unwrap().render();
    assert!(del.contains("Everything on /dev/sdc1") && del.contains("will be erased"), "{del}");
}

#[test]
fn exfat_and_ntfs_are_made_by_the_programs_with_fixed_arguments_and_then_checked() {
    let (_f, d, sys) = lab(&ALL);
    add(&d, 40, FormatFs::Fat32, "old");
    let l: Log = RefCell::new(vec![]);
    // the typed name must be the drive, the partition or the label, exactly
    for wrong in ["", "yes", "Yes", "y", "sdc2", "OLDER"] {
        assert!(go(&d, PartOp::Format { number: 1, fs: FormatFs::Exfat, label: "X".into() }, wrong, &l).is_err(), "{wrong:?}");
    }
    assert!(sys.calls().is_empty(), "nothing started before a correct confirmation");
    let m = go(&d, PartOp::Format { number: 1, fs: FormatFs::Exfat, label: "BACKUP".into() }, "OLD", &l).unwrap();
    assert!(m.starts_with("Done"), "{m}");
    let c = sys.calls();
    assert_eq!(c.len(), 1);
    assert!(c[0].prog.ends_with("/mkfs.exfat") && c[0].args == ["-L", "BACKUP", "/dev/sdc1"] && c[0].stdin_bytes.is_none(), "{c:?}");
    assert_eq!(fs_of(&d, "sdc1"), Fs::Exfat);
    // NTFS, confirmed with the partition's device name this time
    go(&d, PartOp::Format { number: 1, fs: FormatFs::Ntfs, label: "Data".into() }, "sdc1", &l).unwrap();
    assert_eq!(sys.calls()[1].args, ["-Q", "-F", "-L", "Data", "/dev/sdc1"]);
    assert_eq!(fs_of(&d, "sdc1"), Fs::Ntfs);
    // the audit lines name the drive and the program, no argument list and nothing else
    let a = all_text(&l);
    assert!(a.contains("disks.partition.tool") && a.contains("tool=mkfs.exfat") && a.contains("disks.partition.done"), "{a}");
    assert!(!a.contains("BACKUP") && !a.contains("-L"), "{a}");
}

#[test]
fn a_new_partition_can_be_made_as_exfat_in_one_step() {
    let (_f, d, _sys) = lab(&ALL);
    add(&d, 60, FormatFs::Exfat, "photos");
    let s = d.scan();
    let p = s.part("sdc1").unwrap().1;
    assert_eq!((p.fs, p.type_name.as_str()), (Fs::Exfat, "Microsoft basic data"));
}

#[test]
fn without_the_program_the_plan_says_how_to_get_it_and_nothing_changes() {
    let (f, d, sys) = lab(&["e2fsck"]);
    add(&d, 40, FormatFs::Fat32, "old");
    let before = f.dev_bytes("sdc");
    let n = f.calls().len();
    let l: Log = RefCell::new(vec![]);
    for (fs, prog) in [(FormatFs::Exfat, "mkfs.exfat"), (FormatFs::Ntfs, "mkntfs")] {
        let e = go(&d, PartOp::Format { number: 1, fs, label: "x".into() }, "sdc", &l).unwrap_err();
        assert!(e.contains(prog) && e.contains("not installed") && e.contains("does not ship it") && e.contains("Nothing was changed"), "{e}");
    }
    let e = go(&d, PartOp::Add { size_bytes: None, fs: Some(FormatFs::Exfat), label: String::new() }, "sdc", &l).unwrap_err();
    assert!(e.contains("mkfs.exfat"), "{e}");
    assert_eq!(f.dev_bytes("sdc"), before);
    assert_eq!(f.calls().len(), n);
    assert!(sys.calls().is_empty());
    // an engine that was never given any programs says the same
    let bare = f.disks();
    assert!(bare.plan_partition(&admin(), "sdc", &PartOp::Format { number: 1, fs: FormatFs::Ntfs, label: String::new() }).unwrap_err().contains("mkntfs"));
}

#[test]
fn a_failing_program_stops_the_plan_and_is_reported() {
    let (_f, d, sys) = lab(&ALL);
    add(&d, 40, FormatFs::Fat32, "old");
    sys.answer("mkfs.exfat", 1, "", "mkfs.exfat: failed to write\nmore");
    let l: Log = RefCell::new(vec![]);
    let e = go(&d, PartOp::Format { number: 1, fs: FormatFs::Exfat, label: "x".into() }, "sdc", &l).unwrap_err();
    assert!(e.contains("mkfs.exfat failed") && e.contains("failed to write") && e.contains("look at the drive"), "{e}");
    assert!(all_text(&l).contains("disks.partition.failed"));
}

#[test]
fn a_program_that_claims_success_but_leaves_nothing_is_caught_by_looking_again() {
    let (f, d, sys) = lab(&ALL);
    add(&d, 40, FormatFs::Fat32, "old");
    // the plain runner without the effect of the real program
    d.set_tools(Arc::new(ProcTools::new(sys.clone(), "/none")));
    let l: Log = RefCell::new(vec![]);
    let e = go(&d, PartOp::Format { number: 1, fs: FormatFs::Exfat, label: "x".into() }, "sdc", &l).unwrap_err();
    assert!(e.contains("looking at the drive again shows something else") && e.contains("instead of exFAT"), "{e}");
    assert!(all_text(&l).contains("disks.partition.verify_failed"));
    let _ = f;
}

#[test]
fn confirmation_takes_the_drive_the_partition_or_the_exact_label_never_a_plain_yes() {
    let (_f, d, _sys) = lab(&ALL);
    add(&d, 40, FormatFs::Ext4, "Work Disk");
    let snap = d.scan();
    let disk = snap.disk("sdc").unwrap();
    let op = PartOp::Format { number: 1, fs: FormatFs::Exfat, label: String::new() };
    assert_eq!(accepted_names(disk, &op), ["sdc", "9F3A", "sdc1", "Work Disk"]);
    for ok in ["sdc", "SDC1", "9F3A", "Work Disk", "  sdc1  "] {
        assert!(check_confirmation_for(disk, &op, &confirm(ok)).is_ok(), "{ok}");
    }
    for bad in ["", "yes", "Yes", "ok", "work disk", "Work", "sdc2", "sdb"] {
        assert!(check_confirmation_for(disk, &op, &confirm(bad)).is_err(), "{bad}");
    }
    assert!(check_confirmation_for(disk, &op, &Confirm { typed: "sdc1".into(), second: false }).unwrap_err().contains("second confirmation"));
    // a partition's label does not confirm a whole-table change
    assert!(check_confirmation_for(disk, &PartOp::NewTable(Table::Gpt), &confirm("Work Disk")).is_err());
}

// ---------------------------------------------------------------------------------------------------------------------
// resizing
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn growing_an_ext4_partition_writes_the_table_first_then_grows_the_file_system() {
    let (f, d, sys) = lab(&ALL);
    add(&d, 40, FormatFs::Ext4, "data");
    let plan = d.plan_partition(&admin(), "sdc", &PartOp::Resize { number: 1, size_bytes: 80 << 20 }).unwrap();
    let text = plan.render();
    let (table, grow) = (text.find("Write the GPT header").unwrap(), text.find("Grow the file system").unwrap());
    assert!(table < grow && text.contains("Check the ext4 file system") && text.contains("Nothing is lost"), "{text}");
    assert!(text.contains("If the power fails"), "{text}");
    let n = f.calls().len();
    assert!(sys.calls().is_empty(), "the dry run starts nothing");
    let l: Log = RefCell::new(vec![]);
    go(&d, PartOp::Resize { number: 1, size_bytes: 80 << 20 }, "data", &l).unwrap();
    assert!(f.calls().len() > n);
    assert_eq!(size_of(&d, "sdc1"), 80 << 20);
    let c = sys.calls();
    assert_eq!(c.iter().map(|x| x.args.clone()).collect::<Vec<_>>(), [vec!["-f", "-p", "/dev/sdc1"], vec!["/dev/sdc1"]]);
    assert!(c[0].prog.ends_with("/e2fsck") && c[1].prog.ends_with("/resize2fs"));
}

#[test]
fn shrinking_checks_the_smallest_size_first_and_resizes_the_file_system_before_the_table() {
    let (_f, d, sys) = lab(&ALL);
    add(&d, 80, FormatFs::Ext4, "data");
    sys.answer("resize2fs", 0, "Estimated minimum size of the filesystem: 5000\n", "");
    let text = d.plan_partition(&admin(), "sdc", &PartOp::Resize { number: 1, size_bytes: 40 << 20 }).unwrap().render();
    let (chk, min, shrink, table) = (text.find("Check the ext4").unwrap(), text.find("resize2fs -P").unwrap(), text.find("Shrink the file system").unwrap(), text.find("Write the GPT header").unwrap());
    assert!(chk < min && min < shrink && shrink < table, "{text}");
    assert!(sys.calls().is_empty());
    let l: Log = RefCell::new(vec![]);
    go(&d, PartOp::Resize { number: 1, size_bytes: 40 << 20 }, "sdc1", &l).unwrap();
    assert_eq!(size_of(&d, "sdc1"), 40 << 20);
    let args: Vec<Vec<String>> = sys.calls().iter().map(|c| c.args.clone()).collect();
    assert_eq!(args, [vec!["-f", "-p", "/dev/sdc1"], vec!["-P", "/dev/sdc1"], vec!["/dev/sdc1", "40960K"]]);
}

#[test]
fn a_shrink_that_would_cut_off_files_stops_before_anything_is_written() {
    let (f, d, sys) = lab(&ALL);
    add(&d, 80, FormatFs::Ext4, "data");
    let before = f.dev_bytes("sdc");
    // the files need about 100 MB
    sys.answer("resize2fs", 0, "Estimated minimum size of the filesystem: 100000\n", "");
    let l: Log = RefCell::new(vec![]);
    let e = go(&d, PartOp::Resize { number: 1, size_bytes: 40 << 20 }, "sdc1", &l).unwrap_err();
    assert!(e.contains("need at least") && e.contains("Nothing was resized"), "{e}");
    assert_eq!(f.dev_bytes("sdc"), before, "the table and the file system are untouched");
    assert_eq!(size_of(&d, "sdc1"), 80 << 20);
    // an answer that cannot be understood is a refusal too
    sys.answer("resize2fs", 0, "garbled", "");
    let e = go(&d, PartOp::Resize { number: 1, size_bytes: 40 << 20 }, "sdc1", &l).unwrap_err();
    assert!(e.contains("Could not tell how small"), "{e}");
    assert_eq!(f.dev_bytes("sdc"), before);
}

#[test]
fn ntfs_is_checked_measured_and_shrunk_before_the_table_and_grown_after_it() {
    let (f, d, sys) = lab(&ALL);
    add(&d, 80, FormatFs::Fat32, "x");
    // make it an NTFS volume by formatting, then shrink and grow it
    let l: Log = RefCell::new(vec![]);
    go(&d, PartOp::Format { number: 1, fs: FormatFs::Ntfs, label: "win".into() }, "sdc", &l).unwrap();
    let seen = sys.calls().len();
    sys.answer("ntfsresize", 0, "", "");
    sys.answer("ntfsresize", 0, "You might resize at 10000000 bytes or 10 MB\n", "");
    go(&d, PartOp::Resize { number: 1, size_bytes: 40 << 20 }, "sdc1", &l).unwrap();
    let args: Vec<Vec<String>> = sys.calls()[seen..].iter().map(|c| c.args.clone()).collect();
    assert_eq!(args, [vec!["--check", "/dev/sdc1"], vec!["--info", "/dev/sdc1"], vec!["--force", "--size", "41943040", "/dev/sdc1"]]);
    assert_eq!(size_of(&d, "sdc1"), 40 << 20);
    let seen = sys.calls().len();
    go(&d, PartOp::Resize { number: 1, size_bytes: 100 << 20 }, "sdc1", &l).unwrap();
    let args: Vec<Vec<String>> = sys.calls()[seen..].iter().map(|c| c.args.clone()).collect();
    assert_eq!(args, [vec!["--check", "/dev/sdc1"], vec!["--force", "/dev/sdc1"]]);
    assert_eq!(size_of(&d, "sdc1"), 100 << 20);
    let _ = f;
}

#[test]
fn what_cannot_be_resized_safely_is_refused_in_plain_words() {
    let (f, d, sys) = lab(&ALL);
    add(&d, 40, FormatFs::Fat32, "fat");
    add(&d, 40, FormatFs::Ext4, "ext");
    let before = f.dev_bytes("sdc");
    let l: Log = RefCell::new(vec![]);
    // FAT32: no resize program
    let e = go(&d, PartOp::Resize { number: 1, size_bytes: 20 << 20 }, "sdc", &l).unwrap_err();
    assert!(e.contains("cannot resize a FAT32 volume") && e.contains("Nothing was changed"), "{e}");
    // something that is not recognised could hold data: a shrink could cut it off
    f.dev_write("/dev/sdc2", 0, &[0xABu8; 8192]).unwrap();
    d.forget();
    let e = go(&d, PartOp::Resize { number: 2, size_bytes: 20 << 20 }, "sdc", &l).unwrap_err();
    assert!(e.contains("cannot tell what is on sdc2"), "{e}");
    // a file system the programs do not know
    f.dev_write("/dev/sdc2", 0, &sb_btrfs("x")).unwrap();
    d.forget();
    let e = go(&d, PartOp::Resize { number: 2, size_bytes: 20 << 20 }, "sdc", &l).unwrap_err();
    assert!(e.contains("cannot resize a"), "{e}");
    // no room: partition 1 is followed by partition 2
    f.dev_write("/dev/sdc1", 0, &sb_ext4("x")).unwrap();
    d.forget();
    let e = go(&d, PartOp::Resize { number: 1, size_bytes: 60 << 20 }, "sdc", &l).unwrap_err();
    assert!(e.contains("can be used for partition 1") && e.contains("in the way"), "{e}");
    // too small, same size, a partition that is not there
    for (n, size, needle) in [(1u32, 100u64, "at least 1 MiB"), (1, 40 << 20, "already"), (9, 20 << 20, "no partition 9")] {
        let e = go(&d, PartOp::Resize { number: n, size_bytes: size }, "sdc", &l).unwrap_err();
        assert!(e.contains(needle), "{e}");
    }
    assert_eq!(f.dev_bytes("sdc")[..1 << 20], before[..1 << 20], "the table is untouched");
    assert!(sys.calls().is_empty(), "no program was started by any refusal");
}

#[test]
fn a_size_is_rounded_up_to_a_whole_mebibyte_and_the_layout_stays_aligned() {
    let (_f, d, _sys) = lab(&ALL);
    add(&d, 40, FormatFs::Ext4, "data");
    let plan = d.plan_partition(&admin(), "sdc", &PartOp::Resize { number: 1, size_bytes: 41 * (1 << 20) + 1 }).unwrap();
    assert!(plan.after.iter().any(|l| l.starts_with("1 ") && l.contains("44.0 MB")), "{:?}", plan.after);
    let l: Log = RefCell::new(vec![]);
    go(&d, PartOp::Resize { number: 1, size_bytes: 41 * (1 << 20) + 1 }, "data", &l).unwrap();
    let (s, e) = (d.scan().part("sdc1").unwrap().1.start_bytes, size_of(&d, "sdc1"));
    assert!(s % (1 << 20) == 0 && e % (1 << 20) == 0 && e == 42 << 20, "{s} {e}");
}

#[test]
fn the_system_disk_busy_drives_and_unlocked_volumes_are_refused_for_format_and_resize() {
    let f = machine();
    let d = f.disks();
    d.set_tools(Arc::new(Sim { env: f.clone(), inner: ProcTools::new(FakeSys::with_programs(&ALL), "/none") }));
    let n = f.calls().len();
    let l: Log = RefCell::new(vec![]);
    let run = |disk: &str, op: PartOp| d.apply_partition(&admin(), disk, &op, &confirm(disk), &sink(&l));
    let e = run("nvme0n1", PartOp::Format { number: 2, fs: FormatFs::Exfat, label: "x".into() }).unwrap_err();
    assert!(e.contains("never changed"), "{e}");
    let e = run("nvme0n1", PartOp::Resize { number: 2, size_bytes: 1 << 30 }).unwrap_err();
    assert!(e.contains("never changed"), "{e}");
    // an unlocked encrypted volume is open and held: refused
    let c = FakeCrypt::new(f.clone());
    c.add("sdb1", "pw", sb_ext4("secret"));
    d.unlock(&admin(), "sdb1", &crate::disks_crypt::Secret::new("pw"), &*c, &|_, _| {}).unwrap();
    let e = run("sdb", PartOp::Format { number: 1, fs: FormatFs::Ntfs, label: "x".into() }).unwrap_err();
    assert!(e.contains("open at") || e.contains("unlocked") || e.contains("in use"), "{e}");
    assert!(!f.calls()[n..].iter().any(|c| c.starts_with("write") || c.starts_with("zero")), "{:?}", f.calls());
    // only an administrator
    let e = d.apply_partition(&Actor { user: "sam".into(), role: Role::User, uid: 1001, gid: 1001, home: "/home/sam".into() }, "sdc", &PartOp::Reread, &confirm("sdc"), &|_, _| {}).unwrap_err();
    assert!(e.contains("administrator") || e.contains("no such drive"), "{e}");
}
