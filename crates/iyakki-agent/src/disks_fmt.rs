//! DISKS, formatting as exFAT or NTFS and resizing (F-DISKS-08, part 2): the person-installed file system programs.
//!
//! IYAKKI has no in-house exFAT or NTFS formatter and no resizer; the programs that do (`mkfs.exfat` of exfatprogs, `mkntfs` and
//! `ntfsresize` of ntfs-3g: GPL) are never shipped. They are CLI tools entries the person downloads, or the system's own copy is found
//! (`resize2fs` and `e2fsck` of e2fsprogs come with the image). Same rules as the unlock helper (`disks_helper.rs`): a fixed argument
//! list built here from checked parts, no shell, a cleared environment, nothing on standard input. This file holds the program list, the
//! argument builders, the answer parsers and the runner behind the [`FsTools`] seam; the planner (`disks_part.rs`) puts the steps in
//! the plan and the executor runs them, so the dry run, the typed confirmation and the audit are the partition manager's own.

use std::sync::Arc;

use crate::disks_helper::{ProcOut, RealSys, Sys};

/// The programs of this feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    MkfsExfat,
    Mkntfs,
    E2fsck,
    Resize2fs,
    Ntfsresize,
}

impl Tool {
    /// The program's file name.
    pub fn prog(self) -> &'static str {
        match self {
            Tool::MkfsExfat => "mkfs.exfat",
            Tool::Mkntfs => "mkntfs",
            Tool::E2fsck => "e2fsck",
            Tool::Resize2fs => "resize2fs",
            Tool::Ntfsresize => "ntfsresize",
        }
    }

    /// The CLI tools entry that provides it, and its licence.
    pub fn package(self) -> (&'static str, &'static str) {
        match self {
            Tool::MkfsExfat => ("exfatprogs", "GPL-2.0"),
            Tool::Mkntfs | Tool::Ntfsresize => ("ntfs-3g", "GPL-2.0"),
            Tool::E2fsck | Tool::Resize2fs => ("e2fsprogs", "GPL-2.0"),
        }
    }

    /// The words when the program is not there. Says plainly that IYAKKI does not ship it and that nothing was changed.
    pub fn missing_words(self, doing: &str) -> String {
        let (pkg, lic) = self.package();
        if pkg == "e2fsprogs" {
            return format!("{doing} needs the program {} (e2fsprogs), which should be part of the system image but was not found. Nothing was changed on the drive.", self.prog());
        }
        format!("{doing} needs the program {} ({pkg}, {lic} licence), which is not installed. IYAKKI does not ship it: install it from CLI tools (you give the download address and its SHA-256; nothing is fetched until you do) or with this system's package manager, then try again. Nothing was changed on the drive.", self.prog())
    }
}

/// What starts the programs. A made-up one in tests.
pub trait FsTools: Send + Sync {
    /// Where the program is, if it is on the machine.
    fn find(&self, tool: Tool) -> Option<String>;
    /// Run it with the argument list; its answer.
    fn run(&self, tool: Tool, args: &[String]) -> Result<ProcOut, String>;
}

/// No programs at all (the default of an engine until the real ones are set).
pub struct NoTools;

impl FsTools for NoTools {
    fn find(&self, _tool: Tool) -> Option<String> {
        None
    }
    fn run(&self, tool: Tool, _args: &[String]) -> Result<ProcOut, String> {
        Err(tool.missing_words("This"))
    }
}

/// The real runner: looks in the CLI tools folder, then in the system's folders; starts nothing through a shell.
pub struct ProcTools {
    sys: Arc<dyn Sys>,
    cli_bin: String,
}

impl ProcTools {
    pub fn new(sys: Arc<dyn Sys>, cli_bin: &str) -> ProcTools {
        ProcTools { sys, cli_bin: cli_bin.to_string() }
    }

    /// For the data folder of CLI tools (`<data>/cli/bin`).
    pub fn real(cli_bin: &std::path::Path) -> ProcTools {
        ProcTools::new(Arc::new(RealSys), &cli_bin.to_string_lossy())
    }
}

impl FsTools for ProcTools {
    fn find(&self, tool: Tool) -> Option<String> {
        crate::disks_helper::search_dirs(&self.cli_bin).into_iter().map(|d| format!("{d}/{}", tool.prog())).find(|p| self.sys.is_file(p))
    }

    fn run(&self, tool: Tool, args: &[String]) -> Result<ProcOut, String> {
        let prog = self.find(tool).ok_or_else(|| tool.missing_words("This"))?;
        self.sys.run(&prog, args, None)
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// argument lists
// ---------------------------------------------------------------------------------------------------------------------

/// A device path that may go into an argument list: `/dev/` and plain characters only.
pub fn dev_ok(dev: &str) -> Result<(), String> {
    let plain = dev.len() > 5 && dev.len() < 64 && dev.starts_with("/dev/") && !dev.contains("..") && dev[5..].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.'));
    if plain { Ok(()) } else { Err(format!("\"{dev}\" is not a device path IYAKKI will hand to a program.")) }
}

fn label_ok(label: &str, max: usize, fs: &str) -> Result<(), String> {
    if label.chars().count() > max {
        return Err(format!("An {fs} label has {max} characters at most."));
    }
    if label.chars().any(|c| c.is_control()) {
        return Err("A label cannot hold control characters.".into());
    }
    Ok(())
}

pub fn exfat_label(label: &str) -> Result<String, String> {
    let l = label.trim();
    label_ok(l, 11, "exFAT")?;
    Ok(l.to_string())
}

pub fn ntfs_label(label: &str) -> Result<String, String> {
    let l = label.trim();
    label_ok(l, 32, "NTFS")?;
    Ok(l.to_string())
}

pub fn mkfs_exfat_args(dev: &str, label: &str) -> Result<Vec<String>, String> {
    dev_ok(dev)?;
    let mut v = vec![];
    let l = exfat_label(label)?;
    if !l.is_empty() {
        v.extend(["-L".to_string(), l]);
    }
    v.push(dev.to_string());
    Ok(v)
}

pub fn mkntfs_args(dev: &str, label: &str) -> Result<Vec<String>, String> {
    dev_ok(dev)?;
    let mut v: Vec<String> = ["-Q", "-F"].map(String::from).to_vec();
    let l = ntfs_label(label)?;
    if !l.is_empty() {
        v.extend(["-L".to_string(), l]);
    }
    v.push(dev.to_string());
    Ok(v)
}

pub fn e2fsck_args(dev: &str) -> Result<Vec<String>, String> {
    dev_ok(dev)?;
    Ok(vec!["-f".into(), "-p".into(), dev.into()])
}

pub fn resize2fs_min_args(dev: &str) -> Result<Vec<String>, String> {
    dev_ok(dev)?;
    Ok(vec!["-P".into(), dev.into()])
}

/// Shrink to `bytes` (whole KiB), or with `None` grow to fill the partition.
pub fn resize2fs_args(dev: &str, bytes: Option<u64>) -> Result<Vec<String>, String> {
    dev_ok(dev)?;
    let mut v = vec![dev.to_string()];
    if let Some(b) = bytes {
        v.push(format!("{}K", b / 1024));
    }
    Ok(v)
}

pub fn ntfsresize_check_args(dev: &str) -> Result<Vec<String>, String> {
    dev_ok(dev)?;
    Ok(vec!["--check".into(), dev.into()])
}

pub fn ntfsresize_min_args(dev: &str) -> Result<Vec<String>, String> {
    dev_ok(dev)?;
    Ok(vec!["--info".into(), dev.into()])
}

/// Shrink to `bytes`, or with `None` grow to fill the partition.
pub fn ntfsresize_args(dev: &str, bytes: Option<u64>) -> Result<Vec<String>, String> {
    dev_ok(dev)?;
    let mut v = vec!["--force".to_string()];
    if let Some(b) = bytes {
        v.extend(["--size".to_string(), b.to_string()]);
    }
    v.push(dev.to_string());
    Ok(v)
}

// ---------------------------------------------------------------------------------------------------------------------
// answers
// ---------------------------------------------------------------------------------------------------------------------

/// The smallest size, in bytes, a program says the file system can shrink to. `block` is the ext block size.
/// `None` when the answer is not understood (the shrink is then refused).
pub fn parse_min_bytes(tool: Tool, text: &str, block: u64) -> Option<u64> {
    match tool {
        Tool::Resize2fs => {
            let line = text.lines().find(|l| l.contains("minimum size of the filesystem") || l.contains("minimum size of the file system"))?;
            let n: u64 = line.rsplit(':').next()?.trim().parse().ok()?;
            n.checked_mul(block)
        }
        Tool::Ntfsresize => {
            let line = text.lines().find(|l| l.contains("You might resize at"))?;
            let rest = line.split("You might resize at").nth(1)?;
            rest.split_whitespace().next()?.parse().ok()
        }
        _ => None,
    }
}

/// Does this exit code count as success? `e2fsck` returns 1 when it fixed something, which is fine.
pub fn exit_ok(tool: Tool, code: i32) -> bool {
    code == 0 || (tool == Tool::E2fsck && code == 1)
}

/// The first line of a program's complaint, for the person.
pub fn first_line(out: &ProcOut) -> String {
    let t = if out.stderr.trim().is_empty() { &out.stdout } else { &out.stderr };
    t.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no message").chars().take(200).collect()
}

/// A size a person or the assistant wrote: "8 GB", "500 MB", "1.5 GiB". MB/GB/TB mean 1024-based units, as in the Disks tile.
pub fn parse_size(t: &str) -> Result<u64, String> {
    let t = t.trim();
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let n: f64 = num.parse().map_err(|_| format!("\"{t}\" is not a size. Write something like 8 GB or 500 MB."))?;
    let mult: f64 = match unit.trim().to_lowercase().trim_end_matches('b').trim_end_matches('i') {
        "" | "m" => 1024.0 * 1024.0,
        "k" => 1024.0,
        "g" => 1024.0 * 1024.0 * 1024.0,
        "t" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return Err(format!("\"{t}\": the unit must be MB, GB or TB.")),
    };
    let bytes = n * mult;
    if !bytes.is_finite() || bytes < 1024.0 * 1024.0 || bytes > 1e16 {
        return Err("A partition is at least 1 MB.".into());
    }
    Ok(bytes as u64)
}
