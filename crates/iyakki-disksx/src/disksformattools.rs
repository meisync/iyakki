//! What the assistant asks the DISKS tile to format or resize, and how the shell takes the request (moved out of `iyakki_agent::disksformattools`, F-SPLIT-03).

use std::path::{Path, PathBuf};

use crate::disks::Snapshot;
use crate::disks_part::{FormatFs, PartOp};

/// What the assistant asks for (data from a model: checked before it is used).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Req {
    Format { part: String, fs: FormatFs, label: String },
    Resize { part: String, bytes: u64 },
}

impl Req {
    pub fn part(&self) -> &str {
        match self {
            Req::Format { part, .. } | Req::Resize { part, .. } => part,
        }
    }
}

pub fn fs_arg(s: &str) -> Result<FormatFs, String> {
    match s.trim().to_lowercase().as_str() {
        "ext4" => Ok(FormatFs::Ext4),
        "fat32" | "fat" | "vfat" => Ok(FormatFs::Fat32),
        "exfat" => Ok(FormatFs::Exfat),
        "ntfs" => Ok(FormatFs::Ntfs),
        other => Err(format!("\"{other}\" is not a file system IYAKKI can make: ext4, fat32, exfat or ntfs")),
    }
}

pub fn who(user: &str) -> String {
    user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect()
}

fn request_path(dir: &Path, user: &str) -> PathBuf {
    dir.join(format!("disks-format-request-{}", who(user)))
}

pub fn fs_word(f: FormatFs) -> &'static str {
    match f {
        FormatFs::Ext4 => "ext4",
        FormatFs::Fat32 => "fat32",
        FormatFs::Exfat => "exfat",
        FormatFs::Ntfs => "ntfs",
    }
}

/// Ask the shell of `user` to show the review for `req`. One line of plain text, no confirmation in it.
pub fn request(dir: &Path, user: &str, req: &Req) {
    let line = match req {
        Req::Format { part, fs, label } => format!("format\t{part}\t{}\t{label}", fs_word(*fs)),
        Req::Resize { part, bytes } => format!("resize\t{part}\t{bytes}"),
    };
    let _ = std::fs::write(request_path(dir, user), line.as_bytes());
}

/// The shell takes its request (once). Anything that does not parse is dropped.
pub fn take_request(dir: &Path, user: &str) -> Option<Req> {
    let p = request_path(dir, user);
    let t = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    let mut it = t.trim_end_matches('\n').split('\t');
    let kind = it.next()?;
    let part = it.next()?.to_string();
    if part.is_empty() || part.len() > 32 || !part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
        return None;
    }
    match kind {
        "format" => {
            let fs = fs_arg(it.next()?).ok()?;
            let label = it.next().unwrap_or("").to_string();
            (label.chars().count() <= 32 && !label.chars().any(|c| c.is_control())).then_some(Req::Format { part, fs, label })
        }
        "resize" => Some(Req::Resize { part, bytes: it.next()?.parse().ok().filter(|b| *b >= 1 << 20)? }),
        _ => None,
    }
}

/// Turn a request into the engine's change: the drive and partition number the name belongs to.
pub fn to_op(snap: &Snapshot, req: &Req) -> Result<(String, PartOp), String> {
    let (disk, p) = snap.part(req.part()).map(|(d, p)| (d.name.clone(), p.number)).ok_or_else(|| format!("{}: there is no such partition (the drive may have been unplugged).", req.part()))?;
    if p == 0 {
        return Err(format!("{} is a file system straight on the drive without a partition table; make a partition table in the DISKS tile first.", req.part()));
    }
    Ok((disk, match req {
        Req::Format { fs, label, .. } => PartOp::Format { number: p, fs: *fs, label: label.clone() },
        Req::Resize { bytes, .. } => PartOp::Resize { number: p, size_bytes: *bytes },
    }))
}
