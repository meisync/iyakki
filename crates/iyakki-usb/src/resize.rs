//! Shrink an existing partition to make free space (F-INST-27), the installer's one step that changes another system's partition.
//!
//! The order is the safe one: (1) the file system is checked, (2) the file system is shrunk to the new size by the file system's own
//! tool (`e2fsck` + `resize2fs` for ext2/3/4, `ntfsresize` for NTFS; found on the computer, never shipped with IYAKKI), (3) it is
//! checked again, and only then (4) the table entry is made smaller. A power cut after step 2 leaves a partition that is bigger than
//! its file system, which every system still mounts; a power cut before it changes nothing. Nothing here runs without
//! [`ShrinkConfirmation`], and the plan is rebuilt from a fresh read of the disk whose fingerprint must match.
//!
//! Refused (in plain words, by [`plan_shrink`]): a mounted or swap partition, the running system, the stick IYAKKI booted from, an
//! EFI System Partition, an extended partition or a logical one (their own table is not written here), a file system other than
//! ext2/3/4 and NTFS, a size below what the file system needs, a size that frees less than 1 MiB, anything that is not smaller.

use crate::gpt_install::TableKind;
use crate::planner::{DiskModel, Part};
use crate::ui::human_size;
use iyakki_storage::gpt;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const MIB: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShrinkRequest {
    /// Slot number of the partition (`sda3` is 3).
    pub part: u32,
    /// The size the partition gets, in bytes (rounded down to whole MiB).
    pub new_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShrinkPlan {
    pub disk_path: String,
    pub disk_name: String,
    pub part: u32,
    pub part_name: String,
    pub fs: String,
    pub first_lba: u64,
    pub old_sectors: u64,
    pub new_sectors: u64,
    pub sector: u32,
    pub table: TableKind,
    /// The disk's fingerprint when the plan was made (the writer reads the disk again and compares).
    pub fingerprint: String,
    pub steps: Vec<String>,
    pub warnings: Vec<String>,
}

impl ShrinkPlan {
    pub fn freed_bytes(&self) -> u64 {
        (self.old_sectors - self.new_sectors) * self.sector as u64
    }
    pub fn new_bytes(&self) -> u64 {
        self.new_sectors * self.sector as u64
    }
    pub fn text(&self) -> String {
        let mut s = format!("Shrink {} ({}, {}) on {} from {} to {}; {} become free space.\n", self.part_name, self.fs, self.table.label(), self.disk_name, human_size(self.old_sectors * self.sector as u64), human_size(self.new_bytes()), human_size(self.freed_bytes()));
        for (i, st) in self.steps.iter().enumerate() {
            s.push_str(&format!("  {}. {st}\n", i + 1));
        }
        for w in &self.warnings {
            s.push_str(&format!("  ! {w}\n"));
        }
        s
    }
}

/// What proves the person agreed: the drive and the partition they were shown, and the ticked box (never a default).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShrinkConfirmation {
    pub drive: String,
    pub part: u32,
    pub new_bytes: u64,
    pub ack: bool,
}

pub fn check_shrink_confirmation(plan: &ShrinkPlan, c: &ShrinkConfirmation) -> Result<(), String> {
    if !c.ack {
        return Err("the box \"I understand that this partition's file system is changed and could be damaged by a power cut or a fault\" is not ticked; nothing was changed".into());
    }
    if c.drive != plan.disk_name || c.part != plan.part || c.new_bytes != plan.new_bytes() {
        return Err("the confirmation is for another drive, partition or size than the plan; nothing was changed".into());
    }
    Ok(())
}

/// Is `fs` one the installer can shrink?
pub fn supported(fs: &str) -> bool {
    matches!(fs, "ext2" | "ext3" | "ext4" | "ntfs")
}

/// The pure part: decide whether the request may be carried out. `min_bytes` is what the file system's own tool says it needs at
/// least (see [`FsTool::minimum`]); `None` means unknown, which is refused.
pub fn plan_shrink(d: &DiskModel, req: &ShrinkRequest, min_bytes: Option<u64>) -> Result<ShrinkPlan, String> {
    let kind = d.table.ok_or_else(|| format!("{} has no partition table; there is nothing to shrink", d.path))?;
    if d.read_only {
        return Err(format!("{} is read-only", d.path));
    }
    if d.booted_from {
        return Err(format!("{} is the stick IYAKKI started from; it is never changed", d.path));
    }
    if d.unusual_gpt {
        return Err(format!("{} has a GPT with an unusual entry array; rewriting it would change its geometry, so it is refused", d.path));
    }
    if kind == TableKind::Mbr && d.sector != 512 {
        return Err(format!("{}: MBR needs 512-byte sectors", d.path));
    }
    let p: &Part = d.part(req.part).ok_or_else(|| format!("{} has no partition number {}", d.path, req.part))?;
    if p.extended {
        return Err(format!("{} is an extended partition (a container); shrink its logical partitions with the other system's disk tool", p.name));
    }
    if kind == TableKind::Mbr && p.index > 4 {
        return Err(format!("{} is a logical partition inside an extended partition; its table is the container's own, which this installer does not write. Use the other system's disk tool", p.name));
    }
    if let Some(why) = p.in_use() {
        return Err(format!("{} is {why}; it is never shrunk while in use", p.name));
    }
    if p.is_esp() {
        return Err(format!("{} is an EFI System Partition (it holds boot loaders); it is never shrunk", p.name));
    }
    if !supported(&p.fs) {
        return Err(format!("{}: {} cannot be shrunk by the installer (supported: ext2, ext3, ext4 and NTFS). Use that system's own disk tool", p.name, if p.fs.is_empty() { "an unknown or blank file system".to_string() } else { format!("a {} file system", p.fs) }));
    }
    let ss = d.sector as u64;
    let new_bytes = req.new_bytes / MIB * MIB;
    let old_bytes = p.bytes(d.sector);
    if new_bytes == 0 {
        return Err("the new size is under 1 MiB".into());
    }
    if new_bytes >= old_bytes || old_bytes - new_bytes < MIB {
        return Err(format!("the new size ({}) must be at least 1 MiB smaller than the partition ({}); growing is not done here", human_size(new_bytes), human_size(old_bytes)));
    }
    let min = min_bytes.ok_or_else(|| format!("the minimum size of the {} file system on {} could not be found; nothing was changed", p.fs, p.name))?;
    if new_bytes < min {
        return Err(format!("{} needs at least {} for its files; {} is too small. Choose at least {}", p.name, human_size(min), human_size(new_bytes), human_size(min.div_ceil(MIB) * MIB)));
    }
    let mut warnings = vec![];
    if d.running_system {
        warnings.push(format!("{} holds the running system; only {} (not mounted) is changed", d.name, p.name));
    }
    if d.internal() {
        warnings.push("this is an internal disk: have a backup of what matters on it".into());
    }
    if p.fs == "ntfs" {
        warnings.push("Windows may check this volume at its next start (fast start must be off and it must have been shut down fully); that is normal".into());
    }
    let new_sectors = new_bytes / ss;
    let tool = if p.fs == "ntfs" { "ntfsresize" } else { "e2fsck and resize2fs" };
    Ok(ShrinkPlan {
        disk_path: d.path.clone(),
        disk_name: d.name.clone(),
        part: p.index,
        part_name: p.name.clone(),
        fs: p.fs.clone(),
        first_lba: p.first_lba,
        old_sectors: p.sectors,
        new_sectors,
        sector: d.sector,
        table: kind,
        fingerprint: d.fingerprint.clone(),
        steps: vec![
            format!("check the {} file system on {} ({tool}, nothing is repaired beyond what a safe check allows)", p.fs, p.name),
            format!("shrink the file system to {} ({tool})", human_size(new_bytes)),
            "check it again".to_string(),
            format!("make the partition table entry {} smaller; the start never moves and no other entry changes", human_size(old_bytes - new_bytes)),
            "read the disk again and compare every other partition's table entry".to_string(),
        ],
        warnings,
    })
}

/// The file system tools. The real one runs the computer's own programs; tests may put a fake in front.
pub trait FsTool {
    /// The smallest size, in bytes, the file system on `dev` (a file or a partition node) can be shrunk to.
    fn minimum(&self, fs: &str, dev: &Path) -> Result<u64, String>;
    /// Check without changing anything that matters.
    fn check(&self, fs: &str, dev: &Path) -> Result<(), String>;
    /// Shrink the file system on `dev` to `bytes`.
    fn shrink(&self, fs: &str, dev: &Path, bytes: u64) -> Result<(), String>;
}

/// The computer's own `e2fsck`, `resize2fs`, `ntfsresize` (never part of the IYAKKI image).
pub struct HostTools;

fn run(cmd: &str, args: &[&str], ok_codes: &[i32]) -> Result<String, String> {
    let o = Command::new(cmd).args(args).output().map_err(|e| format!("{cmd}: {e} (is it installed?)"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    match o.status.code() {
        Some(c) if ok_codes.contains(&c) => Ok(text),
        _ => Err(format!("{cmd} failed: {}", text.lines().last().unwrap_or("").trim())),
    }
}

/// Block size of an ext file system from its superblock (1024 << s_log_block_size).
pub fn ext_block_size(dev: &Path) -> Result<u64, String> {
    let f = File::open(dev).map_err(|e| format!("{}: {e}", dev.display()))?;
    let mut sb = [0u8; 1024];
    f.read_exact_at(&mut sb, 1024).map_err(|e| format!("{}: {e}", dev.display()))?;
    if u16::from_le_bytes([sb[56], sb[57]]) != 0xEF53 {
        return Err("no ext superblock".into());
    }
    Ok(1024u64 << u32::from_le_bytes([sb[24], sb[25], sb[26], sb[27]]).min(6))
}

/// The number after a label in a tool's text (`Estimated minimum size of the filesystem: 4242`).
pub fn number_after(text: &str, label: &str) -> Option<u64> {
    let rest = &text[text.find(label)? + label.len()..];
    rest.split(|c: char| !c.is_ascii_digit()).find(|t| !t.is_empty())?.parse().ok()
}

impl FsTool for HostTools {
    fn minimum(&self, fs: &str, dev: &Path) -> Result<u64, String> {
        let d = dev.to_str().ok_or("path")?;
        if fs == "ntfs" {
            let t = run("ntfsresize", &["--info", "--force", "--no-progress-bar", d], &[0])?;
            // "You might resize at 25165824 bytes or 25 MB (freeing 1 MB)."
            number_after(&t, "You might resize at").ok_or_else(|| "ntfsresize did not say a minimum size".to_string())
        } else {
            let t = run("resize2fs", &["-P", d], &[0])?;
            let blocks = number_after(&t, "minimum size of the filesystem:").ok_or_else(|| "resize2fs did not say a minimum size".to_string())?;
            Ok(blocks * ext_block_size(dev)?)
        }
    }
    fn check(&self, fs: &str, dev: &Path) -> Result<(), String> {
        let d = dev.to_str().ok_or("path")?;
        if fs == "ntfs" {
            run("ntfsresize", &["--info", "--force", "--no-progress-bar", d], &[0]).map(|_| ())
        } else {
            // -p: only the repairs that are safe without asking; exit 0 and 1 (errors fixed) are fine, 4 and up are not
            run("e2fsck", &["-f", "-p", d], &[0, 1]).map(|_| ())
        }
    }
    fn shrink(&self, fs: &str, dev: &Path, bytes: u64) -> Result<(), String> {
        let d = dev.to_str().ok_or("path")?;
        if fs == "ntfs" {
            run("ntfsresize", &["--force", "--no-progress-bar", "--size", &bytes.to_string(), d], &[0]).map(|_| ())
        } else {
            let k = format!("{}K", bytes / 1024);
            run("resize2fs", &[d, &k], &[0]).map(|_| ())
        }
    }
}

/// A copy of a partition's bytes in a file of its own, so the tools run on a regular file when the disk is an image file.
struct Region {
    path: PathBuf,
}

impl Drop for Region {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn copy_out(disk: &File, off: u64, len: u64, to: &Path) -> Result<(), String> {
    let out = File::create(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    while done < len {
        let n = ((len - done) as usize).min(buf.len());
        disk.read_exact_at(&mut buf[..n], off + done).map_err(|e| format!("reading the partition: {e}"))?;
        out.write_all_at(&buf[..n], done).map_err(|e| e.to_string())?;
        done += n as u64;
    }
    Ok(())
}

fn copy_back(from: &Path, len: u64, disk: &File, off: u64) -> Result<(), String> {
    let inp = File::open(from).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    while done < len {
        let n = ((len - done) as usize).min(buf.len());
        inp.read_exact_at(&mut buf[..n], done).map_err(|e| e.to_string())?;
        disk.write_all_at(&buf[..n], off + done).map_err(|e| format!("writing the partition: {e}"))?;
        done += n as u64;
    }
    Ok(())
}

/// The smallest size for the partition, asked of the tool (for a plan).
pub fn find_minimum(disk_path: &Path, d: &DiskModel, part: u32, tool: &dyn FsTool, node: Option<&Path>, scratch: &Path) -> Result<u64, String> {
    let p = d.part(part).ok_or("no such partition")?;
    if let Some(n) = node {
        return tool.minimum(&p.fs, n);
    }
    let disk = File::open(disk_path).map_err(|e| e.to_string())?;
    let r = Region { path: scratch.join(format!("iyakki-shrink-min-{}-{}", std::process::id(), part)) };
    copy_out(&disk, p.first_lba * d.sector as u64, p.bytes(d.sector), &r.path)?;
    tool.minimum(&p.fs, &r.path)
}

/// What a shrink did.
#[derive(Debug)]
pub struct ShrinkReport {
    pub freed_bytes: u64,
    pub notes: Vec<String>,
}

/// Carry the plan out on `disk_path` (a file standing in for a disk, or the disk's device). `model` is a fresh read of the disk.
/// For a device pass the partition's own node as `node`; for an image file the partition is copied out to `scratch`, shrunk there
/// and copied back (only the part the file system still uses), which is also what the tests do.
pub fn apply_shrink(disk_path: &Path, model: &DiskModel, plan: &ShrinkPlan, conf: &ShrinkConfirmation, tool: &dyn FsTool, node: Option<&Path>, scratch: &Path, proc_mounts: &str) -> Result<ShrinkReport, String> {
    check_shrink_confirmation(plan, conf)?;
    if model.fingerprint != plan.fingerprint {
        return Err("the disk's partition table changed since the plan was made; nothing was changed. Make the plan again.".into());
    }
    let p = model.part(plan.part).ok_or("the partition is gone")?;
    if p.first_lba != plan.first_lba || p.sectors != plan.old_sectors || p.fs != plan.fs {
        return Err("the partition is not what the plan describes; nothing was changed".into());
    }
    if crate::drives::parse_proc_mounts(proc_mounts).iter().any(|(dev, _)| dev.rsplit('/').next() == Some(plan.part_name.as_str())) {
        return Err(format!("{} is mounted now; unmount it first. Nothing was changed.", plan.part_name));
    }
    let ss = plan.sector as u64;
    let disk = std::fs::OpenOptions::new().read(true).write(true).open(disk_path).map_err(|e| format!("{}: {e}", disk_path.display()))?;
    let before = other_entries(model, plan.part);
    let mut notes = vec![];
    // 1-3: the file system
    let bytes = plan.new_bytes();
    match node {
        Some(n) => {
            tool.check(&plan.fs, n)?;
            tool.shrink(&plan.fs, n, bytes)?;
            tool.check(&plan.fs, n)?;
        }
        None => {
            let r = Region { path: scratch.join(format!("iyakki-shrink-{}-{}", std::process::id(), plan.part)) };
            copy_out(&disk, plan.first_lba * ss, plan.old_sectors * ss, &r.path)?;
            tool.check(&plan.fs, &r.path)?;
            tool.shrink(&plan.fs, &r.path, bytes)?;
            tool.check(&plan.fs, &r.path)?;
            copy_back(&r.path, bytes, &disk, plan.first_lba * ss)?;
            disk.sync_data().map_err(|e| e.to_string())?;
        }
    }
    notes.push(format!("the {} file system now fits {}", plan.fs, human_size(bytes)));
    // 4: the table entry
    let last = plan.first_lba + plan.new_sectors - 1;
    match plan.table {
        TableKind::Gpt => {
            let mut h = File::open(disk_path).map_err(|e| e.to_string())?;
            let mut t = gpt::read_gpt(&mut h, plan.sector, model.total_sectors()).map_err(|e| format!("the GPT cannot be read: {e}"))?.table;
            let e = t.entries.iter_mut().find(|e| e.index == plan.part && e.first_lba == plan.first_lba).ok_or("the table entry is gone")?;
            e.last_lba = last;
            t.check()?;
            let (entries, hb, hp) = (t.entries_bytes(), t.header_bytes(true), t.header_bytes(false));
            let put = |off: u64, data: &[u8]| disk.write_all_at(data, off).map_err(|e| format!("writing the table: {e}"));
            put(t.backup_entries_lba() * ss, &entries)?;
            put(t.last_lba() * ss, &hb)?;
            disk.sync_data().map_err(|e| e.to_string())?;
            put(2 * ss, &entries)?;
            disk.sync_data().map_err(|e| e.to_string())?;
            put(ss, &hp)?;
        }
        TableKind::Mbr => {
            let mut s0 = [0u8; 512];
            disk.read_exact_at(&mut s0, 0).map_err(|e| e.to_string())?;
            let o = 446 + 16 * (plan.part as usize - 1);
            let start = u32::from_le_bytes([s0[o + 8], s0[o + 9], s0[o + 10], s0[o + 11]]) as u64;
            if start != plan.first_lba {
                return Err("the table entry is not where the plan expects; nothing was written to the table".into());
            }
            s0[o + 12..o + 16].copy_from_slice(&(plan.new_sectors as u32).to_le_bytes());
            // the CHS fields of the entry are left as they are: every current system uses the LBA fields
            disk.write_all_at(&s0, 0).map_err(|e| format!("writing the table: {e}"))?;
        }
    }
    disk.sync_all().map_err(|e| e.to_string())?;
    // 5: read it back
    let meta = crate::planner::DiskMeta { name: model.name.clone(), path: model.path.clone(), model: model.model.clone(), serial: model.serial.clone(), removable: model.removable, usb: model.usb, read_only: false, swap_parts: vec![] };
    let mut h = File::open(disk_path).map_err(|e| e.to_string())?;
    let after = crate::planner::read_model(&mut h, &meta, model.size_bytes, model.sector, &[], None);
    let np = after.part(plan.part).ok_or("verification FAILED: the partition is not in the table any more")?;
    if np.first_lba != plan.first_lba || np.sectors != plan.new_sectors {
        return Err(format!("verification FAILED: the table says {}+{} sectors, expected {}+{}", np.first_lba, np.sectors, plan.first_lba, plan.new_sectors));
    }
    if other_entries(&after, plan.part) != before {
        return Err("verification FAILED: another partition's table entry changed".into());
    }
    notes.push("every other partition's table entry is unchanged".into());
    Ok(ShrinkReport { freed_bytes: plan.freed_bytes(), notes })
}

fn other_entries(d: &DiskModel, except: u32) -> Vec<(u32, u64, u64, String)> {
    d.parts.iter().filter(|p| p.index != except).map(|p| (p.index, p.first_lba, p.sectors, p.fs.clone())).collect()
}

pub const SHRINK_HELP: &str = "iyakki-usb --shrink DISK --part N --to-mib M [--yes --confirm DISK] [--target-file IMAGE]

Shrink partition N of DISK (for example sda) to M MiB so that the space becomes free for `--plan`. Without --yes this only
prints the plan and changes nothing. With --yes the same command must also carry --confirm DISK (the drive's name again); that
is the deliberate second step. Supported: ext2/3/4 and NTFS file systems on a GPT or MBR (primary partitions); the partition must
not be mounted. The computer's own e2fsck, resize2fs and ntfsresize do the file system part and must be installed. Back up first.
";

/// The disk of this machine called `name` (`sda` or `/dev/sda`), read fresh. Reading the tables needs administrator rights.
pub fn find_disk(m: &crate::plan_cli::Machine, name: &str) -> Result<DiskModel, String> {
    let mut skipped = vec![];
    let (models, _) = crate::plan_cli::discover(m, None, &mut skipped);
    models.into_iter().find(|d| d.name == name || d.path == format!("/dev/{name}")).ok_or_else(|| format!("{name} is not a disk of this computer (run as administrator)"))
}

/// A partition ready to be planned or shrunk: the disk (device or image file), its fresh model and, for a real device, the partition's node.
pub struct Located {
    pub path: PathBuf,
    pub model: DiskModel,
    pub node: Option<PathBuf>,
    pub proc_mounts: String,
}

impl Located {
    /// A disk of the machine.
    pub fn of_machine(m: &crate::plan_cli::Machine, name: &str, part: u32) -> Result<Located, String> {
        let model = find_disk(m, name)?;
        let node = Some(m.dev.join(model.part(part).map(|p| p.name.clone()).ok_or_else(|| format!("{name} has no partition number {part}"))?));
        Ok(Located { path: PathBuf::from(&model.path), model, node, proc_mounts: m.proc_mounts.clone() })
    }
    /// An image file standing in for a disk (tests and the command line's `--target-file`).
    pub fn of_file(f: &Path) -> Result<Located, String> {
        let size = std::fs::metadata(f).map_err(|e| format!("{}: {e}", f.display()))?.len();
        let meta = crate::planner::DiskMeta { name: "testfile".into(), path: f.display().to_string(), model: "test file".into(), serial: String::new(), removable: true, usb: true, ..Default::default() };
        let model = crate::planner::read_model(&mut File::open(f).map_err(|e| e.to_string())?, &meta, size, 512, &[], None);
        Ok(Located { path: f.to_path_buf(), model, node: None, proc_mounts: String::new() })
    }
    /// Plan the shrink of partition `part` to `to_mib` MiB (reads, changes nothing; the file system's own tool says its minimum).
    pub fn plan(&self, part: u32, to_mib: u64, tool: &dyn FsTool) -> Result<ShrinkPlan, String> {
        let min = find_minimum(&self.path, &self.model, part, tool, self.node.as_deref(), &std::env::temp_dir()).ok();
        plan_shrink(&self.model, &ShrinkRequest { part, new_bytes: to_mib * MIB }, min)
    }
    /// Carry a plan out. `confirm_name` must be the drive's name as the plan shows it; the caller has asked the person.
    pub fn apply(&self, plan: &ShrinkPlan, confirm_name: &str, tool: &dyn FsTool) -> Result<ShrinkReport, String> {
        let conf = ShrinkConfirmation { drive: plan.disk_name.clone(), part: plan.part, new_bytes: plan.new_bytes(), ack: confirm_name == plan.disk_name };
        apply_shrink(&self.path, &self.model, plan, &conf, tool, self.node.as_deref(), &std::env::temp_dir(), &self.proc_mounts)
    }
}

/// `iyakki-usb --shrink ...` (F-INST-27). Returns the exit code.
pub fn run_cli(args: &[String], m: &crate::plan_cli::Machine) -> Result<i32, String> {
    let mut disk = None;
    let (mut part, mut to, mut yes, mut confirm, mut target) = (None, None, false, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |what: &str| it.next().cloned().ok_or_else(|| format!("{what} needs a value"));
        match a.as_str() {
            "--shrink" => disk = Some(val("--shrink")?),
            "--part" => part = Some(val("--part")?.parse::<u32>().map_err(|_| "--part needs a whole number")?),
            "--to-mib" => to = Some(val("--to-mib")?.parse::<u64>().map_err(|_| "--to-mib needs a whole number")?),
            "--yes" => yes = true,
            "--confirm" => confirm = Some(val("--confirm")?),
            "--target-file" => target = Some(PathBuf::from(val("--target-file")?)),
            "--help" | "-h" => {
                print!("{SHRINK_HELP}");
                return Ok(0);
            }
            other => return Err(format!("unknown option {other} (see --shrink --help)")),
        }
    }
    let (part, to) = (part.ok_or("--part N is needed")?, to.ok_or("--to-mib M is needed")?);
    let (path, model) = if let Some(f) = &target {
        let size = std::fs::metadata(f).map_err(|e| format!("{}: {e}", f.display()))?.len();
        let meta = crate::planner::DiskMeta { name: "testfile".into(), path: f.display().to_string(), model: "test file".into(), serial: String::new(), removable: true, usb: true, ..Default::default() };
        let model = crate::planner::read_model(&mut File::open(f).map_err(|e| e.to_string())?, &meta, size, 512, &[], None);
        (f.clone(), model)
    } else {
        let name = disk.clone().ok_or("--shrink needs the disk name")?;
        let model = find_disk(m, &name)?;
        (PathBuf::from(&model.path), model)
    };
    let node = target.is_none().then(|| m.dev.join(model.part(part).map(|p| p.name.clone()).unwrap_or_default()));
    let scratch = std::env::temp_dir();
    let min = find_minimum(&path, &model, part, &HostTools, node.as_deref(), &scratch).ok();
    let plan = plan_shrink(&model, &ShrinkRequest { part, new_bytes: to * MIB }, min)?;
    print!("{}", plan.text());
    if !yes {
        println!("Nothing was changed (add --yes --confirm {} to do it).", plan.disk_name);
        return Ok(0);
    }
    let conf = ShrinkConfirmation { drive: plan.disk_name.clone(), part, new_bytes: plan.new_bytes(), ack: confirm.as_deref() == Some(plan.disk_name.as_str()) };
    let r = apply_shrink(&path, &model, &plan, &conf, &HostTools, node.as_deref(), &scratch, &m.proc_mounts)?;
    for n in &r.notes {
        println!("{n}");
    }
    println!("Done: {} are free. Run `iyakki-usb --plan` to install into them.", human_size(r.freed_bytes));
    Ok(0)
}
