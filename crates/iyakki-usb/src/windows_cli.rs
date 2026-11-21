//! The Windows build of the writer program (F-PKG-07): `iyakki-usb IMAGE --erase PhysicalDriveN` on Windows, joined to the Win32 steps
//! of [`crate::windows`] / `windows_real`. Everything that decides something is plain code over the [`Win32`] trait and a drive list, so
//! it runs on any system against a [`crate::windows::FileDisk`]; only [`run_real`] (Windows builds) touches PowerShell and the disk.
//!
//! What it does, in order: read the options (the same ones as the Unix program), look at the image (a bootable ISO or disk image;
//! the install layout, which makes partitions, is not written from Windows: the planner's job, F-INST-12), list the drives with the
//! same verdicts (`--all` for internal disks, the system disk always refused), `--list` prints and stops, writing needs `--erase NAME`
//! (naming the drive is the confirmation; no menu here), not elevated -> start again through the UAC prompt (once), then the writer
//! window's step order ([`crate::writerwin_run`]) with the locking, offline, aligned-write and read-back procedure of `write_stream`.
// SPDX-License-Identifier: Apache-2.0

use std::io::Write;
use std::path::Path;

use crate::cli::{find_drive, items_for, parse_args};
use crate::drives::{Drive, Verdict};
use crate::ui::human_size;
use crate::windows::{write_stream, Win32};
use crate::writer::{self, ImageKind, Phase, Progress, WriteReport};
use crate::writerwin::Event;
use crate::writerwin_run::{self, Job, Steps};

/// The write block: a multiple of both sector sizes.
const BLOCK: usize = 4 << 20;

/// The window's steps over the Win32 trait: `write_stream` does the locking, the offline state, the aligned writes and the read-back
/// itself, so `unmount`, `wipe_tail` and `verify` have nothing left to do (a Windows disk has no stale backup table to wipe: the
/// whole first image is written over the start, and the end is left as the image's own).
pub struct WinSteps<W: Win32> {
    pub w: W,
    /// The logical sector size of each drive by its path (512 when a drive is not listed).
    pub sectors: std::collections::HashMap<String, u32>,
}

impl<W: Win32> Steps for WinSteps<W> {
    fn unmount(&mut self, _: &Drive) -> Result<(), String> {
        Ok(())
    }
    fn write(&mut self, image: &Path, drive: &Drive, install: bool, progress: &mut dyn FnMut(Progress)) -> Result<WriteReport, String> {
        if install {
            return Err("the install layout is not written from Windows yet; write the ISO instead".into());
        }
        let mut f = std::fs::File::open(image).map_err(|e| format!("{}: {e}", image.display()))?;
        let len = f.metadata().map_err(|e| e.to_string())?.len();
        let t0 = std::time::Instant::now();
        let sector = self.sectors.get(&drive.path).copied().unwrap_or(512);
        let n = write_stream(&mut self.w, drive, sector, &mut f, len, BLOCK, None, &mut |done, total| {
            // the first half is the write, the second the read-back
            let (phase, d, t) = if done <= total / 2 { (Phase::Writing, done, total / 2) } else { (Phase::Verifying, done - total / 2, total / 2) };
            progress(Progress { phase, done: d, total: t, elapsed: t0.elapsed() });
        })?;
        Ok(WriteReport { bytes: n, sha256: String::new() })
    }
    fn wipe_tail(&mut self, _: &Drive, _: u64) -> Result<(), String> {
        Ok(())
    }
    fn verify(&mut self, _: &Drive, _: u64, _: &str, _: &mut dyn FnMut(Progress)) -> Result<(), String> {
        // the read-back already happened inside `write` (it compares the SHA-256 of what was written with what the disk returns)
        Ok(())
    }
}

/// The program's whole behaviour. `relaunch` starts the same program again through the UAC prompt with the given arguments and returns
/// its exit code; `sector_of` is the disk's logical sector size (512 or 4096). Returns the exit code.
pub fn run_with(args: &[String], drives: &[Drive], w: &mut dyn Win32, sector_of: &dyn Fn(&Drive) -> u32, relaunch: &dyn Fn(&[String]) -> Result<i32, String>, out: &mut dyn Write) -> Result<i32, String> {
    let already = args.iter().any(|a| a == "--elevated");
    let args: Vec<String> = args.iter().filter(|a| *a != "--elevated").cloned().collect();
    let o = parse_args(&args)?;
    let image = o.image.clone().ok_or("give the image file: iyakki-usb IMAGE --erase PhysicalDrive1")?;
    let info = writer::inspect_image(&image)?;
    let name = image.file_name().map_or_else(|| image.display().to_string(), |n| n.to_string_lossy().into_owned());
    if info.kind == ImageKind::Unknown {
        return Err(format!("{name} does not look like a bootable image (no ISO or disk header). Nothing was written."));
    }
    if info.kind == ImageKind::InstallTemplate || o.install {
        return Err("the install layout is written through the planner on Windows: run `iyakki-usb --plan IMAGE` (see --plan --help) instead of --install; or write the ISO. Nothing was written.".into());
    }
    let items = items_for(drives, &image, o.all);
    if o.list {
        let _ = writeln!(out, "Image: {name} ({})", human_size(info.size));
        for i in &items {
            let status = match &i.verdict {
                Verdict::Ok => "ok".to_string(),
                Verdict::Internal => "internal disk (needs --all)".to_string(),
                Verdict::Refused(r) => format!("refused: {r}"),
            };
            let _ = writeln!(out, "{:<22} {:>9}  {:<26} {status}", i.drive.path, human_size(i.drive.size_bytes), i.drive.model);
        }
        return Ok(0);
    }
    let wanted = o.erase.clone().ok_or("name the drive to erase: --erase PhysicalDrive1 (see --list). Nothing was written.")?;
    let chosen = find_drive(&items, &wanted).ok_or_else(|| format!("{wanted} is not a drive this tool offers (try --list{}). Nothing was written.", if o.all { "" } else { ", or --all for internal disks" }))?;
    match &chosen.verdict {
        Verdict::Refused(r) => return Err(format!("{} cannot be used: {r}. Nothing was written.", chosen.drive.path)),
        Verdict::Internal if !o.all => return Err(format!("{} is an internal disk. Add --all if you really mean it. Nothing was written.", chosen.drive.path)),
        _ => {}
    }
    let d = &chosen.drive;
    if d.size_bytes < info.size {
        return Err(format!("{} holds {} but the image is {}: use a bigger stick. Nothing was written.", d.path, human_size(d.size_bytes), human_size(info.size)));
    }
    if !w.is_elevated() {
        if already {
            return Err("administrator rights were not given; nothing was written".into());
        }
        let _ = writeln!(out, "Administrator rights are needed: the Windows prompt follows.");
        let mut again = args.clone();
        again.push("--elevated".into());
        return relaunch(&again);
    }
    let _ = writeln!(out, "Writing {name} ({}) to {} ({}, {}). Everything on it is erased.", human_size(info.size), d.path, human_size(d.size_bytes), d.model);
    let job = Job { image: image.clone(), drive: d.clone(), install: false, verify: false };
    let mut result: Option<Result<(), String>> = None;
    let mut steps = WinSteps { w, sectors: [(d.path.clone(), sector_of(d))].into_iter().collect() };
    writerwin_run::run(&job, &mut steps, &mut |e| match e {
        Event::Progress { phase, done, total } => {
            let _ = writeln!(out, "@progress {} {done} {total}", match phase { Phase::Writing => "writing", Phase::Verifying => "verifying", Phase::Checking => "checking", Phase::Syncing => "flushing" });
        }
        Event::Finished(r) => result = Some(r),
        _ => {}
    });
    match result {
        Some(Ok(())) => {
            let _ = writeln!(out, "Done. {} written and read back. It is safe to remove the stick.", human_size(info.size));
            Ok(0)
        }
        Some(Err(e)) => Err(e),
        None => Err("the write ended without a result; do not boot the stick".into()),
    }
}

/// The sector size of every listed disk from the PowerShell list (`list_json`), by drive path.
pub fn sector_map(list_json: &str, drives: &[Drive]) -> std::collections::HashMap<String, u32> {
    drives.iter().filter_map(|d| crate::windows::parse_physical(&d.path).map(|n| (d.path.clone(), crate::windows::sector_size(list_json, n)))).collect()
}

/// The steps of the writer window on Windows: the real Win32 calls and the real sector sizes.
#[cfg(windows)]
pub fn real_steps() -> Result<WinSteps<crate::windows_real::RealWin32>, String> {
    let drives = crate::windows::list_drives()?;
    let list_json = powershell_list();
    Ok(WinSteps { w: crate::windows_real::RealWin32::new(), sectors: sector_map(&list_json, &drives) })
}

#[cfg(windows)]
fn powershell_list() -> String {
    std::process::Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", crate::windows::LIST_COMMAND]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// The administrator prompt for a program that has a window: when not elevated, start the same program again through the UAC prompt and
/// return true (the caller then exits); elevated already, or the person refused the prompt: false. Nothing is typed or stored.
#[cfg(windows)]
pub fn elevate_self() -> Result<bool, String> {
    use crate::windows::Win32;
    if crate::windows_real::RealWin32::new().is_elevated() {
        return Ok(false);
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?.display().to_string();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = crate::windows::elevate_command(&exe, &args)?;
    std::process::Command::new(&cmd[0]).args(&cmd[1..]).status().map_err(|e| format!("the Windows prompt could not be started: {e}"))?;
    Ok(true)
}

/// The partition planner on Windows (`--plan`, `--disk`, `--apply-plan`): reading the disks needs administrator rights, so a program that
/// is not elevated starts itself again once through the UAC prompt (`--elevated` stops a loop); then the planner of every other system
/// runs, writing through the locked, offline physical drive and adding the boot entry through the firmware-variable calls.
#[cfg(windows)]
fn run_planner(args: &[String]) -> Result<i32, String> {
    let already = args.iter().any(|a| a == "--elevated");
    let args: Vec<String> = args.iter().filter(|a| *a != "--elevated").cloned().collect();
    if !crate::posix::is_root() {
        if already {
            return Err("administrator rights were not granted; nothing was written".into());
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?.display().to_string();
        let mut again = args.clone();
        again.push("--elevated".into());
        let cmd = crate::windows::elevate_command(&exe, &again)?;
        let st = std::process::Command::new(&cmd[0]).args(&cmd[1..]).status().map_err(|e| format!("the Windows prompt could not be started: {e}"))?;
        return Ok(st.code().unwrap_or(1));
    }
    crate::plan_cli::run_inner(&args, &crate::plan_cli::Machine::real(), false)
}

/// Does the command line ask for the partition planner?
pub fn wants_planner(args: &[String]) -> bool {
    args.iter().any(|a| a == "--plan" || a == "--apply-plan")
}

/// The real program on Windows: the PowerShell disk list, the Win32 calls, the UAC re-launch.
#[cfg(windows)]
pub fn run_real(args: &[String]) -> Result<i32, String> {
    if wants_planner(args) {
        return run_planner(args);
    }
    let drives = crate::windows::list_drives()?;
    let list_json = powershell_list();
    let sector_of = move |d: &Drive| crate::windows::parse_physical(&d.path).map_or(512, |n| crate::windows::sector_size(&list_json, n));
    let exe = std::env::current_exe().map_err(|e| e.to_string())?.display().to_string();
    let relaunch = move |a: &[String]| -> Result<i32, String> {
        let cmd = crate::windows::elevate_command(&exe, a)?;
        let st = std::process::Command::new(&cmd[0]).args(&cmd[1..]).status().map_err(|e| format!("the Windows prompt could not be started: {e}"))?;
        Ok(st.code().unwrap_or(1))
    };
    let mut w = crate::windows_real::RealWin32::new();
    run_with(args, &drives, &mut w, &sector_of, &relaunch, &mut std::io::stdout())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::FileDisk;
    use std::cell::RefCell;

    fn drive(n: u32, size: u64, usb: bool, mounts: &[(&str, &str)]) -> Drive {
        Drive { path: crate::windows::physical_path(n), name: format!("PhysicalDrive{n}"), model: "Stick".into(), size_bytes: size, removable: usb, usb, read_only: false, mounts: mounts.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(), swap: false }
    }

    /// An ISO-looking image of `len` bytes (the ISO 9660 marker at 0x8001).
    fn iso(dir: &Path, len: usize) -> std::path::PathBuf {
        let mut b: Vec<u8> = (0..len).map(|i| (i * 7 % 251) as u8).collect();
        b[0x8001..0x8006].copy_from_slice(b"CD001");
        let p = dir.join("t.iso");
        std::fs::write(&p, &b).unwrap();
        p
    }

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn disk_file(dir: &Path, size: u64, elevated: bool) -> FileDisk {
        let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(dir.join("disk.img")).unwrap();
        f.set_len(size).unwrap();
        FileDisk { file: f, elevated, sector: 512, events: vec![] }
    }

    #[test]
    fn list_shows_the_verdicts_and_writes_nothing() {
        let dir = crate::install::tests::tmp("wincli-list");
        let img = iso(&dir, 0x9000);
        let drives = [drive(1, 100 << 30, false, &[("\\\\.\\PhysicalDrive1", "C:\\Windows")]), drive(2, 1 << 20, true, &[])];
        let mut w = disk_file(&dir, 1 << 20, true);
        let mut out = Vec::new();
        let code = run_with(&args(&[img.to_str().unwrap(), "--list", "--all"]), &drives, &mut w, &|_| 512, &|_| panic!("no re-launch"), &mut out).unwrap();
        let t = String::from_utf8(out).unwrap();
        assert_eq!(code, 0);
        assert!(t.contains("PhysicalDrive2") && t.contains("ok") && t.contains("refused: holds the running system"), "{t}");
        assert!(w.events.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writing_needs_the_drive_named_and_refuses_the_system_disk_a_small_disk_and_the_install_layout() {
        let dir = crate::install::tests::tmp("wincli-refuse");
        let img = iso(&dir, 0x9000);
        let drives = [drive(1, 100 << 30, false, &[("\\\\.\\PhysicalDrive1", "C:\\Windows")]), drive(2, 1 << 20, true, &[]), drive(3, 4096, true, &[])];
        let mut w = disk_file(&dir, 1 << 20, true);
        let mut go = |a: &[&str]| run_with(&args(a), &drives, &mut w, &|_| 512, &|_| panic!("no re-launch"), &mut Vec::new());
        let i = img.to_str().unwrap();
        assert!(go(&[i]).unwrap_err().contains("--erase"));
        assert!(go(&[i, "--erase", "PhysicalDrive1", "--all"]).unwrap_err().contains("cannot be used"));
        assert!(go(&[i, "--erase", "PhysicalDrive9"]).unwrap_err().contains("not a drive this tool offers"));
        assert!(go(&[i, "--erase", "PhysicalDrive3"]).unwrap_err().contains("bigger stick"));
        assert!(go(&[i, "--erase", "PhysicalDrive2", "--install"]).unwrap_err().contains("through the planner"));
        assert!(w.events.is_empty(), "{:?}", w.events);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_administrator_rights_it_starts_again_through_the_prompt_once_and_writes_nothing() {
        let dir = crate::install::tests::tmp("wincli-uac");
        let img = iso(&dir, 0x9000);
        let drives = [drive(2, 1 << 20, true, &[])];
        let mut w = disk_file(&dir, 1 << 20, false);
        let seen = RefCell::new(Vec::new());
        let relaunch = |a: &[String]| -> Result<i32, String> {
            seen.borrow_mut().push(a.to_vec());
            Ok(0)
        };
        let a = args(&[img.to_str().unwrap(), "--erase", "PhysicalDrive2"]);
        assert_eq!(run_with(&a, &drives, &mut w, &|_| 512, &relaunch, &mut Vec::new()).unwrap(), 0);
        let again = seen.borrow()[0].clone();
        assert_eq!(again.last().map(String::as_str), Some("--elevated"));
        // the second start, still without rights: stop, no loop
        let e = run_with(&again, &drives, &mut w, &|_| 512, &relaunch, &mut Vec::new()).unwrap_err();
        assert!(e.contains("not given") && seen.borrow().len() == 1, "{e}");
        assert!(w.events.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_elevated_run_locks_writes_aligned_reads_back_and_reports_progress() {
        let dir = crate::install::tests::tmp("wincli-write");
        let img = iso(&dir, 0x9000 + 100);
        let want = std::fs::read(&img).unwrap();
        let drives = [drive(2, 1 << 20, true, &[("\\\\.\\PhysicalDrive2", "E:\\")])];
        let mut w = disk_file(&dir, 1 << 20, true);
        let mut out = Vec::new();
        let code = run_with(&args(&[img.to_str().unwrap(), "--erase", "PhysicalDrive2", "--elevated"]), &drives, &mut w, &|_| 512, &|_| panic!("no re-launch"), &mut out).unwrap();
        assert_eq!(code, 0);
        let t = String::from_utf8(out).unwrap();
        assert!(t.contains("@progress writing") && t.contains("@progress verifying") && t.contains("Done."), "{t}");
        assert_eq!(w.events.first().map(String::as_str), Some("lock \\\\.\\E:"));
        assert!(w.events.iter().any(|e| e == "offline 2 true") && w.events.last().map(String::as_str) == Some("unlock \\\\.\\E:"), "{:?}", w.events);
        use std::io::{Read, Seek, SeekFrom};
        let mut got = vec![0u8; want.len()];
        w.file.seek(SeekFrom::Start(0)).unwrap();
        w.file.read_exact(&mut got).unwrap();
        assert_eq!(got, want);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
