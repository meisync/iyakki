//! Portable stand-ins for the Unix-only calls the installer's planner and writer use (F-INST-12, F-PKG-07): positioned reads and writes,
//! "is this a block device", "am I root". On Unix they are the standard ones; on Windows positioned I/O is `seek_read`/`seek_write`, a
//! block device is never recognised by file type (the Windows writer opens `\\.\PhysicalDriveN` through `windows_real`) and "am I root"
//! is false (fail closed: nothing here writes a disk on Windows without the elevation steps of `windows_real`).
// SPDX-License-Identifier: Apache-2.0

use std::path::Path;

#[cfg(unix)]
pub use std::os::unix::fs::FileExt;

#[cfg(windows)]
pub trait FileExt {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<()>;
    fn write_all_at(&self, buf: &[u8], offset: u64) -> std::io::Result<()>;
}

#[cfg(windows)]
impl FileExt for std::fs::File {
    fn read_exact_at(&self, mut buf: &mut [u8], mut offset: u64) -> std::io::Result<()> {
        use std::os::windows::fs::FileExt as W;
        while !buf.is_empty() {
            match self.seek_read(buf, offset) {
                Ok(0) => return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "failed to fill whole buffer")),
                Ok(n) => {
                    buf = &mut buf[n..];
                    offset += n as u64;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    fn write_all_at(&self, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
        use std::os::windows::fs::FileExt as W;
        while !buf.is_empty() {
            match self.seek_write(buf, offset) {
                Ok(0) => return Err(std::io::Error::new(std::io::ErrorKind::WriteZero, "failed to write whole buffer")),
                Ok(n) => {
                    buf = &buf[n..];
                    offset += n as u64;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

/// The three calls the FAT writer needs from a disk or an image file.
pub trait DiskIo {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<()>;
    fn write_all_at(&self, buf: &[u8], offset: u64) -> std::io::Result<()>;
    fn sync_data(&self) -> std::io::Result<()>;
}

impl DiskIo for std::fs::File {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
        FileExt::read_exact_at(self, buf, offset)
    }
    fn write_all_at(&self, buf: &[u8], offset: u64) -> std::io::Result<()> {
        FileExt::write_all_at(self, buf, offset)
    }
    fn sync_data(&self) -> std::io::Result<()> {
        std::fs::File::sync_data(self)
    }
}

/// What the planner's writer must know to open a whole disk safely (Windows needs the exact size, the sector size and the volumes to lock;
/// Unix opens the node by its path and ignores them).
#[derive(Clone, Debug, Default)]
pub struct DiskOpen {
    pub sector: u32,
    pub size_bytes: u64,
    /// Mount points (`E:\`) of the volumes on the disk.
    pub mounts: Vec<String>,
}

/// The disk the planner's writer reads and writes: on Unix the opened device or file itself.
#[cfg(unix)]
pub type DiskHandle = std::fs::File;

/// Open `path` for reading and writing as a whole disk.
#[cfg(unix)]
pub fn open_disk(path: &Path, _o: &DiskOpen) -> Result<DiskHandle, String> {
    std::fs::OpenOptions::new().read(true).write(true).open(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// A second handle on the same disk with its own position at the start, for code that wants a plain reader.
#[cfg(unix)]
pub fn reopen(_disk: &DiskHandle, path: &Path) -> Result<DiskHandle, String> {
    std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The Windows disk: one aligned, locked, offline disk shared by every handle (the position is shared too, as for a cloned file). It is
/// brought back online and its volumes unlocked when the last handle is dropped.
#[cfg(windows)]
#[derive(Clone)]
pub struct DiskHandle(std::sync::Arc<std::sync::Mutex<crate::windisk::DiskCore<crate::windows_real::RealWin32>>>);

#[cfg(windows)]
impl DiskHandle {
    fn core(&self) -> std::sync::MutexGuard<'_, crate::windisk::DiskCore<crate::windows_real::RealWin32>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn sync_data(&self) -> std::io::Result<()> {
        self.core().flush()
    }
    pub fn sync_all(&self) -> std::io::Result<()> {
        self.core().flush()
    }
    pub fn try_clone(&self) -> std::io::Result<DiskHandle> {
        Ok(self.clone())
    }
}

#[cfg(windows)]
impl DiskIo for DiskHandle {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
        self.core().disk.read_at(offset, buf)
    }
    fn write_all_at(&self, buf: &[u8], offset: u64) -> std::io::Result<()> {
        self.core().disk.write_at(offset, buf)
    }
    fn sync_data(&self) -> std::io::Result<()> {
        self.core().flush()
    }
}

#[cfg(windows)]
impl FileExt for DiskHandle {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
        self.core().disk.read_at(offset, buf)
    }
    fn write_all_at(&self, buf: &[u8], offset: u64) -> std::io::Result<()> {
        self.core().disk.write_at(offset, buf)
    }
}

#[cfg(windows)]
impl std::io::Read for DiskHandle {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.core().read(buf)
    }
}
#[cfg(windows)]
impl std::io::Write for DiskHandle {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.core().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut *self.core())
    }
}
#[cfg(windows)]
impl std::io::Seek for DiskHandle {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        self.core().seek(to)
    }
}
#[cfg(windows)]
impl std::io::Read for &DiskHandle {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.core().read(buf)
    }
}
#[cfg(windows)]
impl std::io::Write for &DiskHandle {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.core().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut *self.core())
    }
}
#[cfg(windows)]
impl std::io::Seek for &DiskHandle {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        self.core().seek(to)
    }
}

/// Open `\\.\PhysicalDriveN` as a whole disk: rights checked, volumes locked and dismounted, disk offline (see `windisk`).
#[cfg(windows)]
pub fn open_disk(path: &Path, o: &DiskOpen) -> Result<DiskHandle, String> {
    let core = crate::windisk::DiskCore::open(crate::windows_real::RealWin32::new(), &path.display().to_string(), o.sector, o.size_bytes, &o.mounts)?;
    Ok(DiskHandle(std::sync::Arc::new(std::sync::Mutex::new(core))))
}

/// A second handle on the same disk with its own position at the start. On Windows the one locked disk is shared (a second raw open
/// would fight the offline disk); the position is reset to the start.
#[cfg(windows)]
pub fn reopen(disk: &DiskHandle, _path: &Path) -> Result<DiskHandle, String> {
    disk.core().pos = 0;
    Ok(disk.clone())
}

/// Is `p` a block device (a disk node)? Never true where there are no such files.
#[cfg(unix)]
pub fn is_block_device(p: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(p).is_ok_and(|m| m.file_type().is_block_device())
}

#[cfg(not(unix))]
pub fn is_block_device(_p: &Path) -> bool {
    false
}

/// Does this process run with administrator rights (root; on Windows an elevated process)? False on systems where this crate has no way to ask (fail closed).
#[cfg(unix)]
pub fn is_root() -> bool {
    // SAFETY: geteuid takes no arguments.
    unsafe { libc::geteuid() == 0 }
}

#[cfg(windows)]
pub fn is_root() -> bool {
    use crate::windows::Win32;
    crate::windows_real::RealWin32::new().is_elevated()
}

#[cfg(not(any(unix, windows)))]
pub fn is_root() -> bool {
    false
}

/// The plan file given to the privileged writer must belong to the person who asked for the install and not be writable by others.
#[cfg(unix)]
pub fn plan_file_owner_ok(file: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(file).map_err(|e| format!("{}: {e}", file.display()))?;
    // SAFETY: getuid takes no arguments.
    let caller = unsafe { libc::getuid() };
    let sudo_uid: Option<u32> = std::env::var("PKEXEC_UID").or_else(|_| std::env::var("SUDO_UID")).ok().and_then(|v| v.parse().ok());
    if meta.mode() & 0o022 != 0 || !(meta.uid() == caller || Some(meta.uid()) == sudo_uid) {
        return Err(format!("{} must be owned by the person who asked for the install and not writable by others; refused", file.display()));
    }
    Ok(())
}

/// Windows has no owner or mode check in this crate, so the plan file must lie inside the signed-in person's own profile folder
/// (`%USERPROFILE%`, which other accounts cannot write to by default), must not be a link, and the writer still compares the plan's digest
/// with the disk and needs the confirmation inside. On Windows the owner and the access list are checked too (`winacl`).
#[cfg(not(unix))]
pub fn plan_file_owner_ok(file: &Path) -> Result<(), String> {
    let profile = std::env::var("USERPROFILE").unwrap_or_default();
    let real = std::fs::canonicalize(file).map_err(|e| format!("{}: {e}", file.display()))?;
    if std::fs::symlink_metadata(file).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!("{} is a link; refused", file.display()));
    }
    plan_file_in_profile(&real.display().to_string(), &profile).map_err(|e| format!("{}: {e}", file.display()))?;
    #[cfg(windows)]
    crate::winacl::check_plan_file(&real).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(())
}

/// The pure rule behind the Windows plan-file check: `path` (as Windows prints it, possibly with the `\\?\` prefix) is inside `profile`.
pub fn plan_file_in_profile(path: &str, profile: &str) -> Result<(), String> {
    let norm = |s: &str| s.trim_start_matches("\\\\?\\").replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase();
    let (p, root) = (norm(path), norm(profile));
    if root.len() < 4 || root.matches('\\').count() < 2 {
        return Err("the profile folder is not known, so the plan file cannot be trusted; refused".into());
    }
    if p.starts_with(&format!("{root}\\")) && !p.split('\\').any(|c| c == "..") {
        Ok(())
    } else {
        Err("the plan file must be inside the signed-in person's own profile folder; refused".into())
    }
}

#[cfg(test)]
mod tests {
    use super::plan_file_in_profile;

    #[test]
    fn a_plan_file_is_trusted_only_inside_the_signed_in_persons_profile() {
        let profile = r"C:\Users\Pat";
        assert!(plan_file_in_profile(r"C:\Users\Pat\AppData\Local\Temp\plan.json", profile).is_ok());
        assert!(plan_file_in_profile(r"\\?\c:\users\pat\plan.json", profile).is_ok(), "the prefix and the case do not matter");
        assert!(plan_file_in_profile(r"C:\Users\Pat2\plan.json", profile).is_err(), "a sibling folder with the same start is not inside");
        assert!(plan_file_in_profile(r"C:\Users\Public\plan.json", profile).is_err());
        assert!(plan_file_in_profile(r"C:\Users\Pat\..\Public\plan.json", profile).is_err());
        assert!(plan_file_in_profile(r"C:\Users\Pat", profile).is_err(), "the folder itself is not a file in it");
        assert!(plan_file_in_profile(r"C:\plan.json", "").is_err(), "no profile known: refused");
        assert!(plan_file_in_profile(r"C:\Users\plan.json", r"C:\Users").is_err(), "a profile that is a top folder is too wide");
    }
}
