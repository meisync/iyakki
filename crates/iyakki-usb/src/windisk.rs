//! The planner's disk on Windows (F-INST-12): a whole physical disk that can be read and written at any byte offset in any size, built
//! on the sector-aligned calls of the [`Win32`] trait (raw disks accept only whole sectors at sector offsets; the planner writes pieces
//! of any size). Everything here is plain Rust over the trait, so it runs for real on any system against [`crate::windows::FileDisk`];
//! the Windows build joins it to the real Win32 calls (`posix::DiskHandle`).
//!
//! Opening the disk does what the stick writer does before its first byte (`windows::write_stream`): elevation is checked, every volume of
//! the disk is locked and dismounted, the disk is taken offline; dropping the disk brings it online again and unlocks the volumes, on every
//! path out. Nothing is guessed: a volume that will not lock stops the install before anything is written.
// SPDX-License-Identifier: Apache-2.0

use std::io::{Read, Seek, SeekFrom, Write};

use crate::windows::{parse_physical, volume_path, Win32};

/// The largest aligned window moved in one call.
const WINDOW: u64 = 4 << 20;

/// Byte-addressed access to one raw disk through sector-aligned calls: a write that does not cover whole sectors reads the sector first
/// and writes it back with the new bytes in it.
pub struct AlignedDisk<W: Win32> {
    pub w: W,
    pub disk: u32,
    pub sector: u64,
    pub size: u64,
}

impl<W: Win32> AlignedDisk<W> {
    fn check(&self, off: u64, len: usize) -> std::io::Result<()> {
        if off.checked_add(len as u64).is_none_or(|e| e > self.size) {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "past the end of the disk"));
        }
        Ok(())
    }

    /// The aligned window `[start, end)` that holds the bytes from `pos` on (at most `rem` of them, at most [`WINDOW`] long).
    fn window(&self, pos: u64, rem: u64) -> (u64, u64) {
        let s = self.sector;
        let start = pos / s * s;
        let end = (pos + rem).div_ceil(s) * s;
        (start, end.min(start + WINDOW).min(self.size))
    }

    pub fn read_at(&mut self, off: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.check(off, buf.len())?;
        let io = |e: String| std::io::Error::other(e);
        let mut done = 0usize;
        while done < buf.len() {
            let pos = off + done as u64;
            let (start, end) = self.window(pos, (buf.len() - done) as u64);
            let mut tmp = vec![0u8; (end - start) as usize];
            self.w.read_at(self.disk, start, &mut tmp).map_err(io)?;
            let from = (pos - start) as usize;
            let n = (tmp.len() - from).min(buf.len() - done);
            buf[done..done + n].copy_from_slice(&tmp[from..from + n]);
            done += n;
        }
        Ok(())
    }

    pub fn write_at(&mut self, off: u64, buf: &[u8]) -> std::io::Result<()> {
        self.check(off, buf.len())?;
        let io = |e: String| std::io::Error::other(e);
        let mut done = 0usize;
        while done < buf.len() {
            let pos = off + done as u64;
            let (start, end) = self.window(pos, (buf.len() - done) as u64);
            let mut tmp = vec![0u8; (end - start) as usize];
            let from = (pos - start) as usize;
            let n = (tmp.len() - from).min(buf.len() - done);
            // a window that is not covered from end to end keeps what the disk has around the new bytes
            if from != 0 || n != tmp.len() {
                self.w.read_at(self.disk, start, &mut tmp).map_err(io)?;
            }
            tmp[from..from + n].copy_from_slice(&buf[done..done + n]);
            self.w.write_at(self.disk, start, &tmp).map_err(io)?;
            done += n;
        }
        Ok(())
    }
}

/// An opened disk: aligned access, a read/write position, the locked volumes and the offline state.
pub struct DiskCore<W: Win32> {
    pub disk: AlignedDisk<W>,
    pub pos: u64,
    locked: Vec<String>,
    offline: bool,
}

impl<W: Win32> DiskCore<W> {
    /// Check the rights, lock and dismount every volume (`mounts` are drive letters or mount points such as `E:\`), take the disk
    /// offline. On any failure everything done so far is undone and nothing has been written.
    pub fn open(w: W, path: &str, sector: u32, size: u64, mounts: &[String]) -> Result<DiskCore<W>, String> {
        let n = parse_physical(path).ok_or_else(|| format!("{path} is not a PhysicalDrive path"))?;
        if !matches!(sector, 512 | 4096) || size == 0 || size % sector as u64 != 0 {
            return Err("the disk's size and sector size are not known exactly; refused".into());
        }
        if !w.is_elevated() {
            return Err("administrator rights are needed: start the installer again and accept the Windows prompt".into());
        }
        let mut core = DiskCore { disk: AlignedDisk { w, disk: n, sector: sector as u64, size }, pos: 0, locked: Vec::new(), offline: false };
        for mp in mounts {
            let letter = mp.chars().next().filter(|c| c.is_ascii_alphabetic()).ok_or("a volume without a letter")?;
            let v = volume_path(letter).ok_or("a bad drive letter")?;
            if core.locked.contains(&v) {
                continue;
            }
            core.disk.w.lock_and_dismount(&v).map_err(|e| format!("{mp} could not be locked ({e}): close the programs using it and try again"))?;
            core.locked.push(v);
        }
        core.disk.w.set_offline(n, true).map_err(|e| format!("the disk could not be taken offline: {e}"))?;
        core.offline = true;
        Ok(core)
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.disk.w.flush(self.disk.disk).map_err(std::io::Error::other)
    }
}

impl<W: Win32> Drop for DiskCore<W> {
    fn drop(&mut self) {
        if self.offline {
            let d = self.disk.disk;
            let _ = self.disk.w.set_offline(d, false);
        }
        for v in self.locked.iter().rev() {
            self.disk.w.unlock(v);
        }
    }
}

impl<W: Win32> Read for DiskCore<W> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = (self.disk.size.saturating_sub(self.pos)).min(buf.len() as u64) as usize;
        if n == 0 {
            return Ok(0);
        }
        self.disk.read_at(self.pos, &mut buf[..n])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl<W: Win32> Write for DiskCore<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.disk.write_at(self.pos, buf)?;
        self.pos += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        DiskCore::flush(self)
    }
}

impl<W: Win32> Seek for DiskCore<W> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let (base, delta) = match to {
            SeekFrom::Start(n) => (0i128, n as i128),
            SeekFrom::Current(d) => (self.pos as i128, d as i128),
            SeekFrom::End(d) => (self.disk.size as i128, d as i128),
        };
        let p = base + delta;
        if p < 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before the start of the disk"));
        }
        self.pos = p as u64;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::FileDisk;

    fn disk(len: u64, sector: u64, elevated: bool) -> (tempfile_dir::Dir, FileDisk) {
        let d = tempfile_dir::Dir::new();
        let p = d.0.join("disk.img");
        let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&p).unwrap();
        f.set_len(len).unwrap();
        (d, FileDisk { file: f, elevated, sector, events: vec![] })
    }

    /// A private scratch folder (removed on drop), so the tests share no names.
    mod tempfile_dir {
        pub struct Dir(pub std::path::PathBuf);
        impl Dir {
            pub fn new() -> Dir {
                static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let p = std::env::temp_dir().join(format!("iyakki-windisk-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
                std::fs::create_dir_all(&p).unwrap();
                Dir(p)
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    /// A `Win32` over a file that shares its event log, and refuses to lock volume Z.
    struct Rec(FileDisk, std::rc::Rc<std::cell::RefCell<Vec<String>>>);
    impl Win32 for Rec {
        fn is_elevated(&self) -> bool {
            self.0.is_elevated()
        }
        fn lock_and_dismount(&mut self, v: &str) -> Result<(), String> {
            self.1.borrow_mut().push(format!("lock {v}"));
            if v.contains('Z') { Err("in use".into()) } else { Ok(()) }
        }
        fn unlock(&mut self, v: &str) {
            self.1.borrow_mut().push(format!("unlock {v}"));
        }
        fn set_offline(&mut self, d: u32, off: bool) -> Result<(), String> {
            self.1.borrow_mut().push(format!("offline {d} {off}"));
            Ok(())
        }
        fn write_at(&mut self, d: u32, o: u64, b: &[u8]) -> Result<(), String> {
            self.0.write_at(d, o, b)
        }
        fn read_at(&mut self, d: u32, o: u64, b: &mut [u8]) -> Result<(), String> {
            self.0.read_at(d, o, b)
        }
        fn flush(&mut self, d: u32) -> Result<(), String> {
            self.0.flush(d)
        }
    }

    fn pattern(n: usize, seed: u8) -> Vec<u8> {
        (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
    }

    #[test]
    fn unaligned_writes_keep_the_bytes_around_them_for_both_sector_sizes() {
        for sector in [512u64, 4096] {
            let (_d, fd) = disk(64 * sector, sector, true);
            let mut c = DiskCore::open(fd, r"\\.\PhysicalDrive3", sector as u32, 64 * sector, &[]).unwrap();
            let base = pattern((64 * sector) as usize, 9);
            c.write_at_all(0, &base);
            // pieces that start and end inside sectors, one inside a single sector, one over several windows
            let mut model = base.clone();
            for (off, len, seed) in [(5u64, 7usize, 1u8), (sector - 3, 9, 2), (3 * sector + 1, 2 * sector as usize + 5, 3), (10, 1, 4), (0, sector as usize, 5), (17 * sector - 1, 1, 6)] {
                let data = pattern(len, seed);
                c.disk.write_at(off, &data).unwrap();
                model[off as usize..off as usize + len].copy_from_slice(&data);
            }
            let mut back = vec![0u8; model.len()];
            c.disk.read_at(0, &mut back).unwrap();
            assert!(back == model, "sector {sector}: the disk is not what a byte-addressed write model says");
            // reads of odd pieces
            let mut piece = vec![0u8; 700];
            c.disk.read_at(sector - 100, &mut piece).unwrap();
            assert_eq!(piece, model[(sector - 100) as usize..(sector - 100) as usize + 700]);
        }
    }

    impl DiskCore<FileDisk> {
        fn write_at_all(&mut self, off: u64, data: &[u8]) {
            self.disk.write_at(off, data).unwrap();
        }
    }

    #[test]
    fn a_write_across_several_four_mebibyte_windows_and_the_end_of_the_disk_are_exact() {
        let len = 9 << 20;
        let (_d, fd) = disk(len, 512, true);
        let mut c = DiskCore::open(fd, r"\\.\PhysicalDrive1", 512, len, &[]).unwrap();
        let data = pattern(len as usize - 1000, 7);
        c.disk.write_at(300, &data).unwrap();
        let mut back = vec![0u8; data.len()];
        c.disk.read_at(300, &mut back).unwrap();
        assert!(back == data);
        // the bytes before and after stayed zero
        let mut head = vec![1u8; 300];
        c.disk.read_at(0, &mut head).unwrap();
        assert!(head.iter().all(|b| *b == 0));
        // past the end is refused, and nothing is written
        assert!(c.disk.write_at(len - 2, &[1, 2, 3]).is_err());
        assert!(c.disk.read_at(len, &mut [0u8; 1]).is_err());
    }

    #[test]
    fn it_reads_and_writes_like_a_file_so_the_table_and_filesystem_code_can_use_it() {
        let (_d, fd) = disk(1 << 20, 512, true);
        let mut c = DiskCore::open(fd, r"\\.\PhysicalDrive2", 512, 1 << 20, &[]).unwrap();
        c.seek(SeekFrom::Start(1000)).unwrap();
        c.write_all(b"hello disk").unwrap();
        c.seek(SeekFrom::Start(1000)).unwrap();
        let mut b = [0u8; 10];
        c.read_exact(&mut b).unwrap();
        assert_eq!(&b, b"hello disk");
        assert_eq!(c.seek(SeekFrom::End(-512)).unwrap(), (1 << 20) - 512);
        assert!(c.seek(SeekFrom::Current(-(1 << 30))).is_err());
        c.seek(SeekFrom::End(0)).unwrap();
        assert_eq!(c.read(&mut b).unwrap(), 0);
    }

    #[test]
    fn opening_needs_rights_locks_every_volume_once_goes_offline_and_undoes_all_on_drop_or_failure() {
        // not elevated: refused, nothing touched
        let (_d, fd) = disk(1 << 20, 512, false);
        assert!(DiskCore::open(fd, r"\\.\PhysicalDrive4", 512, 1 << 20, &[]).err().unwrap().contains("administrator"));
        // elevated: lock E and F (E named twice), offline; dropping the disk brings it online and unlocks, last locked first
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let (_d, fd) = disk(1 << 20, 512, true);
        let mounts = vec![r"E:\".to_string(), r"E:\".to_string(), r"F:\".to_string()];
        let c = DiskCore::open(Rec(fd, log.clone()), r"\\.\PhysicalDrive4", 512, 1 << 20, &mounts).unwrap();
        assert_eq!(*log.borrow(), vec![r"lock \\.\E:", r"lock \\.\F:", "offline 4 true"]);
        log.borrow_mut().clear();
        drop(c);
        assert_eq!(*log.borrow(), vec!["offline 4 false", r"unlock \\.\F:", r"unlock \\.\E:"]);
        // a volume that will not lock: the ones locked before it are unlocked, the disk never went offline
        log.borrow_mut().clear();
        let (_d, fd) = disk(1 << 20, 512, true);
        let e = DiskCore::open(Rec(fd, log.clone()), r"\\.\PhysicalDrive4", 512, 1 << 20, &[r"E:\".to_string(), r"Z:\".to_string()]).err().unwrap();
        assert!(e.contains("could not be locked"), "{e}");
        assert_eq!(*log.borrow(), vec![r"lock \\.\E:", r"lock \\.\Z:", r"unlock \\.\E:"]);
        // bad inputs
        let (_d, fd) = disk(1 << 20, 512, true);
        assert!(DiskCore::open(fd, "/dev/sdb", 512, 1 << 20, &[]).is_err());
        let (_d, fd) = disk(1 << 20, 512, true);
        assert!(DiskCore::open(fd, r"\\.\PhysicalDrive4", 1024, 1 << 20, &[]).is_err());
        let (_d, fd) = disk(1 << 20, 512, true);
        assert!(DiskCore::open(fd, r"\\.\PhysicalDrive4", 512, (1 << 20) + 1, &[]).is_err());
    }
}
