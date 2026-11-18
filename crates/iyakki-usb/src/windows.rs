//! Windows host support for the USB writer (F-PKG-07), the part that needs no Windows: reading the disk list PowerShell gives, the
//! safety rules for the system disk, and the ORDER of the raw write (lock every volume, take the disk offline, write sector-aligned
//! blocks, flush, read back, unlock) as a procedure over a small [`Win32`] trait. Tests run it on Linux with a fake.
//!
//! What is not here: the real [`Win32`] implementation (`CreateFileW` on `\\.\PhysicalDriveN` and `\\.\X:`, `DeviceIoControl` with
//! `FSCTL_LOCK_VOLUME`, `FSCTL_DISMOUNT_VOLUME`, `IOCTL_DISK_SET_DISK_ATTRIBUTES`, `WriteFile` with `FILE_FLAG_NO_BUFFERING`) and the
//! UAC re-launch. They need a Windows toolchain target and a Windows machine to prove anything; neither exists on the build hosts, so
//! nothing unproven was written (docs/INSTALLER.md, Windows).
//!
//! The disk list comes from one fixed PowerShell command ([`LIST_COMMAND`]); the tool never builds a command from user text.

use crate::drives::Drive;
use crate::json::Json;

/// The command that lists the disks; its output is what [`parse_disks`] reads.
pub const LIST_COMMAND: &str = "Get-Disk | ForEach-Object { [pscustomobject]@{ Number = $_.Number; Name = $_.FriendlyName; Size = $_.Size; Bus = [string]$_.BusType; System = $_.IsSystem; Boot = $_.IsBoot; ReadOnly = $_.IsReadOnly; Sector = $_.LogicalSectorSize; Letters = @(Get-Partition -DiskNumber $_.Number -ErrorAction SilentlyContinue | Where-Object DriveLetter | ForEach-Object { [string]$_.DriveLetter }) } } | ConvertTo-Json -Compress";

/// `\\.\PhysicalDrive3` for disk number 3.
pub fn physical_path(n: u32) -> String {
    format!("\\\\.\\PhysicalDrive{n}")
}

/// The disk number in `\\.\PhysicalDriveN`; anything else (a volume, a file, a trailing character) is not accepted.
pub fn parse_physical(path: &str) -> Option<u32> {
    let n = path.strip_prefix("\\\\.\\PhysicalDrive")?;
    (!n.is_empty() && n.len() <= 4 && n.bytes().all(|b| b.is_ascii_digit()) && (n == "0" || !n.starts_with('0'))).then(|| n.parse().ok()).flatten()
}

/// `C:` for the letter `c` (a letter only; nothing else is ever built into a volume path).
pub fn volume_path(letter: char) -> Option<String> {
    letter.is_ascii_alphabetic().then(|| format!("\\\\.\\{}:", letter.to_ascii_uppercase()))
}

/// A drive-letter volume the running Windows lives on.
pub fn is_system_mount(mp: &str) -> bool {
    let b = mp.as_bytes();
    b.len() == 10 && b[0].is_ascii_alphabetic() && &mp[1..3] == ":\\" && mp[3..].eq_ignore_ascii_case("windows")
}

/// PowerShell's `ConvertTo-Json` writes one disk as an object, many as an array, a single letter as a string and none as nothing.
/// A byte order mark in front is dropped.
pub fn parse_disks(text: &str, system_drive: &str) -> Result<Vec<Drive>, String> {
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() {
        return Ok(vec![]);
    }
    let j = Json::parse(text).map_err(|e| format!("the disk list is not readable: {e}"))?;
    let items: Vec<&Json> = match &j {
        Json::Arr(a) => a.iter().collect(),
        Json::Obj(_) => vec![&j],
        _ => return Err("the disk list is not a list of disks".into()),
    };
    let sys = system_drive.trim_end_matches('\\').trim_end_matches(':');
    let sys_letter = (sys.len() == 1 && sys.as_bytes()[0].is_ascii_alphabetic()).then(|| sys.to_ascii_uppercase()).unwrap_or_else(|| "C".into());
    let mut out = Vec::new();
    for d in items {
        let num = d.get("Number").and_then(Json::as_u64).and_then(|n| u32::try_from(n).ok()).ok_or("a disk without a number")?;
        let bus = d.get("Bus").and_then(Json::as_str).unwrap_or("").to_string();
        let letters: Vec<String> = match d.get("Letters") {
            Some(Json::Arr(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
            Some(Json::Str(s)) => vec![s.clone()],
            _ => vec![],
        };
        let dev = physical_path(num);
        let mut mounts: Vec<(String, String)> = letters.iter().filter(|l| l.len() == 1 && l.as_bytes()[0].is_ascii_alphabetic()).map(|l| (dev.clone(), format!("{}:\\", l.to_ascii_uppercase()))).collect();
        if d.get("System").and_then(Json::as_bool).unwrap_or(false) || d.get("Boot").and_then(Json::as_bool).unwrap_or(false) {
            mounts.push((dev.clone(), format!("{sys_letter}:\\Windows")));
        }
        let usb = bus.eq_ignore_ascii_case("USB");
        out.push(Drive {
            path: dev,
            name: format!("PhysicalDrive{num}"),
            model: d.get("Name").and_then(Json::as_str).filter(|s| !s.trim().is_empty()).unwrap_or("disk").trim().to_string(),
            size_bytes: d.get("Size").and_then(Json::as_u64).unwrap_or(0),
            removable: usb,
            usb,
            read_only: d.get("ReadOnly").and_then(Json::as_bool).unwrap_or(false),
            mounts,
            swap: false,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// The logical sector size PowerShell reported for disk `number` (512 when it said nothing usable).
pub fn sector_size(text: &str, number: u32) -> u32 {
    let text = text.trim_start_matches('\u{feff}').trim();
    let Ok(j) = Json::parse(text) else { return 512 };
    let items: Vec<&Json> = match &j {
        Json::Arr(a) => a.iter().collect(),
        o @ Json::Obj(_) => vec![o],
        _ => vec![],
    };
    items.into_iter().find(|d| d.get("Number").and_then(Json::as_u64) == Some(number as u64)).and_then(|d| d.get("Sector").and_then(Json::as_u64)).filter(|s| matches!(*s, 512 | 4096)).map_or(512, |s| s as u32)
}

/// What the raw write needs from Windows. The real implementation is the part this file does not have (see the header).
pub trait Win32 {
    /// Is the process elevated (an administrator with the prompt accepted)?
    fn is_elevated(&self) -> bool;
    /// Lock a volume (`\\.\X:`) so no other program writes to it, then dismount it.
    fn lock_and_dismount(&mut self, volume: &str) -> Result<(), String>;
    fn unlock(&mut self, volume: &str);
    /// Take the disk offline (so Windows does not remount the volumes while the partition table changes) / bring it back.
    fn set_offline(&mut self, disk: u32, offline: bool) -> Result<(), String>;
    /// Write `buf` at `offset` of the raw disk; both are multiples of the sector size.
    fn write_at(&mut self, disk: u32, offset: u64, buf: &[u8]) -> Result<(), String>;
    fn read_at(&mut self, disk: u32, offset: u64, buf: &mut [u8]) -> Result<(), String>;
    fn flush(&mut self, disk: u32) -> Result<(), String>;
}

/// Write `image` (already in memory: small images and tests) with [`write_stream`].
pub fn write_raw(w: &mut dyn Win32, drive: &Drive, sector: u32, image: &[u8], block: usize) -> Result<u64, String> {
    write_stream(w, drive, sector, &mut std::io::Cursor::new(image), image.len() as u64, block, None, &mut |_, _| {})
}

/// Write `len` bytes from `src` to the start of the disk the drive stands for: refuse unless elevated, lock and dismount EVERY volume
/// first (one failure ends it before a byte is written), take the disk offline, write in `block`-byte pieces rounded up to the sector
/// size (the last piece is padded with zeros), flush, read the disk back piece by piece and compare its SHA-256 with that of what was
/// written, then bring the disk online and unlock in reverse order. The disk comes back online and the volumes are unlocked on every
/// path out, including failures. Memory stays at one block however large the image is. `patch` may change the FIRST block before it is
/// written (the install template's boot sector); the hash is of the patched bytes. `progress(done, total)` is called after each piece
/// of the write and of the read-back (the two passes count one after the other: total is 2 x len).
#[allow(clippy::too_many_arguments)]
pub fn write_stream(w: &mut dyn Win32, drive: &Drive, sector: u32, src: &mut dyn std::io::Read, len: u64, block: usize, patch: Option<&dyn Fn(&mut [u8]) -> Result<(), String>>, progress: &mut dyn FnMut(u64, u64)) -> Result<u64, String> {
    use sha2::{Digest, Sha256};
    let disk = parse_physical(&drive.path).ok_or("not a PhysicalDrive path")?;
    if !matches!(sector, 512 | 4096) || block == 0 || block % sector as usize != 0 {
        return Err("the block size must be a multiple of the disk's sector size (512 or 4096)".into());
    }
    if !w.is_elevated() {
        return Err("administrator rights are needed: start the writer again and accept the Windows prompt".into());
    }
    if len > drive.size_bytes {
        return Err("the image is larger than the disk".into());
    }
    let mut locked: Vec<String> = Vec::new();
    let mut offline = false;
    let result = (|| {
        for (_, mp) in &drive.mounts {
            let letter = mp.chars().next().filter(|c| c.is_ascii_alphabetic()).ok_or("a volume without a letter")?;
            let v = volume_path(letter).ok_or("a bad drive letter")?;
            if locked.contains(&v) {
                continue;
            }
            w.lock_and_dismount(&v).map_err(|e| format!("{mp} could not be locked ({e}): close the programs using it and try again"))?;
            locked.push(v);
        }
        w.set_offline(disk, true).map_err(|e| format!("the disk could not be taken offline: {e}"))?;
        offline = true;
        let mut sum = Sha256::new();
        let mut off = 0u64;
        let mut buf = vec![0u8; block];
        while off < len {
            let n = ((len - off) as usize).min(block);
            src.read_exact(&mut buf[..n]).map_err(|e| format!("the image could not be read at {off}: {e}"))?;
            if off == 0 {
                if let Some(p) = patch {
                    p(&mut buf[..n])?;
                }
            }
            sum.update(&buf[..n]);
            let padded = n.div_ceil(sector as usize) * sector as usize;
            buf[n..padded].fill(0);
            w.write_at(disk, off, &buf[..padded]).map_err(|e| format!("write at {off}: {e}"))?;
            off += n as u64;
            progress(off, len * 2);
        }
        w.flush(disk)?;
        let want = sum.finalize();
        let mut back = Sha256::new();
        let mut off = 0u64;
        while off < len {
            let n = ((len - off) as usize).min(block);
            let padded = n.div_ceil(sector as usize) * sector as usize;
            w.read_at(disk, off, &mut buf[..padded]).map_err(|e| format!("read back at {off}: {e}"))?;
            back.update(&buf[..n]);
            off += n as u64;
            progress(len + off, len * 2);
        }
        if back.finalize() != want {
            return Err("the disk does not hold what was written (the read-back differs)".into());
        }
        Ok(len)
    })();
    if offline {
        let _ = w.set_offline(disk, false);
    }
    for v in locked.iter().rev() {
        w.unlock(v);
    }
    result
}

/// A [`Win32`] over an ordinary file standing for the disk: lets the whole procedure (alignment, padding, read-back) run for real on any
/// system against a temporary file. `elevated` and the event list are plain fields so a test can see what happened.
pub struct FileDisk {
    pub file: std::fs::File,
    pub elevated: bool,
    pub sector: u64,
    pub events: Vec<String>,
}

impl Win32 for FileDisk {
    fn is_elevated(&self) -> bool {
        self.elevated
    }
    fn lock_and_dismount(&mut self, v: &str) -> Result<(), String> {
        self.events.push(format!("lock {v}"));
        Ok(())
    }
    fn unlock(&mut self, v: &str) {
        self.events.push(format!("unlock {v}"));
    }
    fn set_offline(&mut self, d: u32, off: bool) -> Result<(), String> {
        self.events.push(format!("offline {d} {off}"));
        Ok(())
    }
    fn write_at(&mut self, _: u32, off: u64, buf: &[u8]) -> Result<(), String> {
        use std::io::{Seek, SeekFrom, Write};
        if off % self.sector != 0 || buf.len() as u64 % self.sector != 0 {
            return Err("not sector aligned".into());
        }
        self.file.seek(SeekFrom::Start(off)).and_then(|_| self.file.write_all(buf)).map_err(|e| e.to_string())
    }
    fn read_at(&mut self, _: u32, off: u64, buf: &mut [u8]) -> Result<(), String> {
        use std::io::{Read, Seek, SeekFrom};
        if off % self.sector != 0 || buf.len() as u64 % self.sector != 0 {
            return Err("not sector aligned".into());
        }
        self.file.seek(SeekFrom::Start(off)).and_then(|_| self.file.read_exact(buf)).map_err(|e| e.to_string())
    }
    fn flush(&mut self, _: u32) -> Result<(), String> {
        self.file.sync_all().map_err(|e| e.to_string())
    }
}

/// The command line that starts this program again with administrator rights through the UAC prompt (PowerShell's `Start-Process
/// -Verb RunAs`). Every argument is quoted for PowerShell as a single-quoted string (a quote inside is doubled), so no text from the
/// caller can end the string or start another command; the person sees the Windows prompt, and nothing is typed or stored.
pub fn elevate_command(exe: &str, args: &[String]) -> Result<Vec<String>, String> {
    let bad = |s: &str| s.is_empty() || s.chars().any(|c| c.is_control());
    if bad(exe) || args.iter().any(|a| bad(a)) {
        return Err("the program path and its arguments must be plain text".into());
    }
    let q = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let list = args.iter().map(|a| q(a)).collect::<Vec<_>>().join(",");
    let ps = if args.is_empty() { format!("Start-Process -FilePath {} -Verb RunAs -Wait", q(exe)) } else { format!("Start-Process -FilePath {} -ArgumentList {list} -Verb RunAs -Wait", q(exe)) };
    Ok(vec!["powershell.exe".into(), "-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), ps])
}

/// Run [`LIST_COMMAND`] with the system's PowerShell and read the disks. Uses only `std::process`; on other systems it says so.
pub fn list_drives() -> Result<Vec<Drive>, String> {
    #[cfg(windows)]
    {
        let o = std::process::Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", LIST_COMMAND]).output().map_err(|e| format!("powershell: {e}"))?;
        if !o.status.success() {
            return Err(format!("powershell failed: {}", String::from_utf8_lossy(&o.stderr).trim()));
        }
        parse_disks(&String::from_utf8_lossy(&o.stdout), &std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()))
    }
    #[cfg(not(windows))]
    {
        Err("Windows only".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drives::{verdict, Verdict};
    use std::path::Path;

    const TWO: &str = "\u{feff}[{\"Number\":0,\"Name\":\"Samsung SSD 970\",\"Size\":1000204886016,\"Bus\":\"NVMe\",\"System\":true,\"Boot\":true,\"ReadOnly\":false,\"Sector\":512,\"Letters\":[\"C\",\"D\"]},{\"Number\":2,\"Name\":\"SanDisk Cruzer\",\"Size\":31000000000,\"Bus\":\"USB\",\"System\":false,\"Boot\":false,\"ReadOnly\":false,\"Sector\":512,\"Letters\":\"E\"}]";

    #[test]
    fn paths_are_exactly_physical_drives_and_letters() {
        assert_eq!(physical_path(3), "\\\\.\\PhysicalDrive3");
        for (p, n) in [("\\\\.\\PhysicalDrive0", Some(0)), ("\\\\.\\PhysicalDrive12", Some(12)), ("\\\\.\\PhysicalDrive", None), ("\\\\.\\PhysicalDrive1x", None), ("\\\\.\\PhysicalDrive01", None), ("\\\\.\\C:", None), ("C:\\x", None), ("\\\\.\\PhysicalDrive99999", None)] {
            assert_eq!(parse_physical(p), n, "{p}");
        }
        assert_eq!(volume_path('e').as_deref(), Some("\\\\.\\E:"));
        assert_eq!(volume_path('1'), None);
        assert_eq!(volume_path(':'), None);
    }

    #[test]
    fn powershell_lists_are_read_in_all_their_shapes_and_the_system_disk_is_refused() {
        let d = parse_disks(TWO, "C:").unwrap();
        assert_eq!(d.len(), 2);
        let (sys, stick) = (&d[0], &d[1]);
        assert_eq!((stick.path.as_str(), stick.usb, stick.size_bytes, stick.model.as_str()), ("\\\\.\\PhysicalDrive2", true, 31_000_000_000, "SanDisk Cruzer"));
        assert_eq!(stick.mounts, vec![("\\\\.\\PhysicalDrive2".to_string(), "E:\\".to_string())], "one letter comes as a plain string");
        assert!(matches!(verdict(sys, Path::new("D:\\iyakki.iso")), Verdict::Refused(r) if r.contains("running system")), "the Windows disk is refused");
        assert_eq!(verdict(stick, Path::new("D:\\iyakki.iso")), Verdict::Ok);
        // (the "image is on this stick" rule compares path components, which only a Windows build of `Path` splits at a backslash)
        // one disk is an object, no disks is nothing, junk is an error
        let one = parse_disks("{\"Number\":1,\"Name\":\"X\",\"Size\":5,\"Bus\":\"USB\",\"System\":false,\"Boot\":false,\"ReadOnly\":false,\"Letters\":null}", "C:\\").unwrap();
        assert_eq!((one.len(), one[0].mounts.len()), (1, 0));
        assert!(parse_disks("", "C:").unwrap().is_empty());
        assert!(parse_disks("not json", "C:").is_err());
        assert!(parse_disks("[{\"Name\":\"no number\"}]", "C:").is_err());
        assert!(is_system_mount("C:\\Windows") && is_system_mount("d:\\WINDOWS") && !is_system_mount("C:\\Windows\\x") && !is_system_mount("E:\\"));
        assert_eq!(sector_size(TWO, 2), 512);
        assert_eq!(sector_size("[{\"Number\":4,\"Sector\":4096}]", 4), 4096);
        assert_eq!(sector_size("[{\"Number\":4,\"Sector\":123}]", 4), 512, "an odd number is not believed");
    }

    #[derive(Default)]
    struct Fake {
        elevated: bool,
        disk: Vec<u8>,
        events: Vec<String>,
        fail_lock: Option<String>,
        fail_write_at: Option<u64>,
        corrupt: bool,
        sector: usize,
    }
    impl Win32 for Fake {
        fn is_elevated(&self) -> bool {
            self.elevated
        }
        fn lock_and_dismount(&mut self, v: &str) -> Result<(), String> {
            if self.fail_lock.as_deref() == Some(v) {
                return Err("in use".into());
            }
            self.events.push(format!("lock {v}"));
            Ok(())
        }
        fn unlock(&mut self, v: &str) {
            self.events.push(format!("unlock {v}"));
        }
        fn set_offline(&mut self, d: u32, off: bool) -> Result<(), String> {
            self.events.push(format!("offline {d} {off}"));
            Ok(())
        }
        fn write_at(&mut self, _: u32, off: u64, buf: &[u8]) -> Result<(), String> {
            assert!(off as usize % self.sector == 0 && buf.len() % self.sector == 0, "aligned: {off} {}", buf.len());
            if self.fail_write_at == Some(off) {
                return Err("I/O error".into());
            }
            self.events.push(format!("write {off} {}", buf.len()));
            let end = off as usize + buf.len();
            if self.disk.len() < end {
                self.disk.resize(end, 0);
            }
            self.disk[off as usize..end].copy_from_slice(buf);
            if self.corrupt && off == 0 {
                self.disk[1] ^= 1;
            }
            Ok(())
        }
        fn read_at(&mut self, _: u32, off: u64, buf: &mut [u8]) -> Result<(), String> {
            buf.copy_from_slice(&self.disk[off as usize..off as usize + buf.len()]);
            Ok(())
        }
        fn flush(&mut self, _: u32) -> Result<(), String> {
            self.events.push("flush".into());
            Ok(())
        }
    }

    fn stick() -> Drive {
        parse_disks(TWO, "C:").unwrap().remove(1)
    }

    #[test]
    fn the_write_locks_first_writes_aligned_blocks_verifies_and_unlocks() {
        let image: Vec<u8> = (0..1300u32).map(|i| (i % 251) as u8).collect();
        let mut w = Fake { elevated: true, sector: 512, ..Default::default() };
        assert_eq!(write_raw(&mut w, &stick(), 512, &image, 1024), Ok(1300));
        assert_eq!(&w.disk[..1300], &image[..]);
        assert!(w.disk[1300..].iter().all(|b| *b == 0) && w.disk.len() == 1536, "the last piece is padded to a sector");
        assert_eq!(w.events, ["lock \\\\.\\E:", "offline 2 true", "write 0 1024", "write 1024 512", "flush", "offline 2 false", "unlock \\\\.\\E:"]);
    }

    #[test]
    fn nothing_is_written_when_not_elevated_or_a_volume_will_not_lock_and_all_is_undone_on_failure() {
        let image = vec![7u8; 2048];
        let mut w = Fake { elevated: false, sector: 512, ..Default::default() };
        assert!(write_raw(&mut w, &stick(), 512, &image, 1024).unwrap_err().contains("administrator"));
        assert!(w.events.is_empty());
        let mut two_vol = stick();
        two_vol.mounts.push((two_vol.path.clone(), "F:\\".into()));
        let mut w = Fake { elevated: true, sector: 512, fail_lock: Some("\\\\.\\F:".into()), ..Default::default() };
        let e = write_raw(&mut w, &two_vol, 512, &image, 1024).unwrap_err();
        assert!(e.contains("F:\\ could not be locked"), "{e}");
        assert_eq!(w.events, ["lock \\\\.\\E:", "unlock \\\\.\\E:"], "E was locked, F failed, nothing written, E unlocked, never offline");
        let mut w = Fake { elevated: true, sector: 512, fail_write_at: Some(1024), ..Default::default() };
        assert!(write_raw(&mut w, &stick(), 512, &image, 1024).unwrap_err().contains("write at 1024"));
        assert!(w.events.ends_with(&["offline 2 false".to_string(), "unlock \\\\.\\E:".to_string()]), "{:?}", w.events);
        let mut w = Fake { elevated: true, sector: 512, corrupt: true, ..Default::default() };
        assert!(write_raw(&mut w, &stick(), 512, &image, 1024).unwrap_err().contains("does not hold what was written"));
        assert!(w.events.last().unwrap().starts_with("unlock"));
    }

    #[test]
    fn a_4096_byte_sector_disk_gets_4096_aligned_pieces_and_bad_arguments_are_refused() {
        let image = vec![9u8; 5000];
        let mut w = Fake { elevated: true, sector: 4096, ..Default::default() };
        assert_eq!(write_raw(&mut w, &stick(), 4096, &image, 4096), Ok(5000));
        assert_eq!(w.disk.len(), 8192);
        let mut w = Fake { elevated: true, sector: 512, ..Default::default() };
        assert!(write_raw(&mut w, &stick(), 4096, &image, 1024).is_err(), "a block that is not a multiple of the sector");
        assert!(write_raw(&mut w, &stick(), 1000, &image, 1000).is_err(), "an odd sector size");
        let mut small = stick();
        small.size_bytes = 100;
        assert!(write_raw(&mut w, &small, 512, &image, 1024).unwrap_err().contains("larger"));
        let mut bad = stick();
        bad.path = "\\\\.\\C:".into();
        assert!(write_raw(&mut w, &bad, 512, &image, 1024).is_err(), "a volume is never written as a disk");
        assert!(w.events.is_empty());
    }

    #[test]
    fn a_large_image_streams_in_blocks_with_a_patched_first_block_and_a_file_disk_holds_exactly_it() {
        let dir = std::env::temp_dir().join(format!("iyakki-usb-win-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let disk_path = dir.join("disk.img");
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&disk_path).unwrap();
        let image: Vec<u8> = (0..3_000_001u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
        let mut w = FileDisk { file, elevated: true, sector: 512, events: vec![] };
        let mut d = stick();
        d.size_bytes = 10_000_000;
        let mut marks = Vec::new();
        let patch = |b: &mut [u8]| -> Result<(), String> {
            b[0] = 0xEB;
            Ok(())
        };
        let n = write_stream(&mut w, &d, 512, &mut std::io::Cursor::new(&image), image.len() as u64, 1 << 20, Some(&patch), &mut |done, total| marks.push((done, total))).unwrap();
        assert_eq!(n, 3_000_001);
        let on_disk = std::fs::read(&disk_path).unwrap();
        assert_eq!(on_disk.len() % 512, 0, "the last sector is padded");
        assert_eq!(on_disk[0], 0xEB, "the patched boot byte");
        assert_eq!(&on_disk[1..3_000_001], &image[1..], "everything else is the image, byte for byte");
        assert!(on_disk[3_000_001..].iter().all(|b| *b == 0));
        assert!(marks.windows(2).all(|p| p[0].0 <= p[1].0) && marks.last().unwrap().0 == marks.last().unwrap().1, "progress rises to the end: {marks:?}");
        assert_eq!(w.events.first().unwrap(), "lock \\\\.\\E:");
        assert_eq!(w.events.last().unwrap(), "unlock \\\\.\\E:");
        // a source that is shorter than it said stops before the read-back
        let mut w2 = FileDisk { file: std::fs::OpenOptions::new().read(true).write(true).open(&disk_path).unwrap(), elevated: true, sector: 512, events: vec![] };
        let e = write_stream(&mut w2, &d, 512, &mut std::io::Cursor::new(vec![1u8; 100]), 5000, 1024, None, &mut |_, _| {}).unwrap_err();
        assert!(e.contains("could not be read"), "{e}");
        assert_eq!(w2.events.last().unwrap(), "unlock \\\\.\\E:", "unlocked after the failure too");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_uac_command_quotes_every_argument_so_no_text_can_start_another_command() {
        let c = elevate_command("C:\\Program Files\\IYAKKI\\iyakki-usb.exe", &["--apply-plan".into(), "C:\\Users\\O'Neil\\plan.json".into(), "x'; Remove-Item -Recurse C:\\ ; '".into()]).unwrap();
        assert_eq!(&c[..4], ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command"]);
        assert_eq!(c[4], "Start-Process -FilePath 'C:\\Program Files\\IYAKKI\\iyakki-usb.exe' -ArgumentList '--apply-plan','C:\\Users\\O''Neil\\plan.json','x''; Remove-Item -Recurse C:\\ ; ''' -Verb RunAs -Wait");
        assert!(elevate_command("", &[]).is_err() && elevate_command("a.exe", &["x\ny".into()]).is_err());
        assert!(elevate_command("a.exe", &[]).unwrap()[4].ends_with("-Verb RunAs -Wait"));
        #[cfg(not(windows))]
        assert!(list_drives().is_err());
    }
}
