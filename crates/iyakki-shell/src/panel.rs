//! Switching the panel itself: the backlight (`/sys/class/backlight/*/brightness`) and the framebuffer's blank call (`FBIOBLANK`),
//! for the screen saver's dim and screen-off stages (`shell_saver.rs`).
//!
//! Rules that keep a dark screen from ever being a trap:
//!
//! * the exact brightness before we touched it is written to a small file (`/run/iyakki/backlight-restore`, RAM only) *before* the
//!   backlight is changed; a shell that crashed and was restarted puts it back at its first heartbeat ([`restore_leftover`]);
//! * a shell that starts on a panel whose backlight is 0 raises it ([`ensure_visible`]): nobody chooses a dark panel on purpose
//!   (the brightness keys stop at the lowest step), and a warm reboot after a crash can keep the register at 0;
//! * waking restores the backlight first, then the framebuffer is unblanked, and only then is the scene drawn.
//!
//! Everything takes the `/sys` folder and the restore file as arguments, so the tests run on fake folders.

use crate::fnkeys::{panel_backlights, Light};
use std::path::{Path, PathBuf};

/// Where the previous brightness is kept while the panel is dimmed or off (a RAM folder: nothing is written to a drive).
pub const RESTORE_FILE: &str = "/run/iyakki/backlight-restore";

/// `fb_blank` levels (linux/fb.h).
pub const FB_BLANK_UNBLANK: i32 = 0;
pub const FB_BLANK_POWERDOWN: i32 = 4;
/// The ioctl number.
pub const FBIOBLANK: u64 = 0x4611;

/// Something the blank call can be made on (the real framebuffer, or a recording fake in the tests).
pub trait BlankIoctl {
    fn blank(&mut self, level: i32) -> Result<(), String>;
}

/// Power the screen down (`off`) or bring it back. An error is returned for the log; the caller never depends on it (the scene is
/// black anyway).
pub fn blank_screen(dev: &mut dyn BlankIoctl, off: bool) -> Result<(), String> {
    dev.blank(if off { FB_BLANK_POWERDOWN } else { FB_BLANK_UNBLANK })
}

/// The blank call on an open framebuffer's file descriptor.
pub fn ioctl_blank(fd: std::os::unix::io::RawFd, level: i32) -> Result<(), String> {
    // SAFETY: FBIOBLANK takes the blank level as its integer argument, no pointer is involved.
    let r = unsafe { libc::ioctl(fd, FBIOBLANK as _, level as libc::c_ulong) };
    if r < 0 {
        Err(format!("FBIOBLANK {level}: {}", std::io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

impl BlankIoctl for crate::fb::Framebuffer {
    fn blank(&mut self, level: i32) -> Result<(), String> {
        match self.raw_fd() {
            Some(fd) => ioctl_blank(fd, level),
            None => Err("the screen is not ours right now".into()),
        }
    }
}

/// What was changed: a backlight folder and the value it had.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Held {
    dir: PathBuf,
    value: u32,
}

/// The backlight of the machine's panel, with the memory of what it was.
pub struct Panel {
    sys: PathBuf,
    restore_file: PathBuf,
    held: Vec<Held>,
}

fn read_u32(p: &Path) -> Option<u32> {
    std::fs::read_to_string(p).ok()?.trim().parse().ok()
}

fn write_value(dir: &Path, v: u32) -> bool {
    std::fs::write(dir.join("brightness"), v.to_string()).is_ok()
}

impl Panel {
    pub fn new(sys: &Path, restore_file: &Path) -> Panel {
        Panel { sys: sys.to_path_buf(), restore_file: restore_file.to_path_buf(), held: Vec::new() }
    }

    /// The machine's real folders.
    pub fn real() -> Panel {
        Panel::new(Path::new("/sys"), Path::new(RESTORE_FILE))
    }

    /// Whether this machine has a backlight to dim (a desktop monitor has none).
    pub fn has_backlight(&self) -> bool {
        !panel_backlights(&self.sys).is_empty()
    }

    /// Whether the backlight is changed right now.
    pub fn is_changed(&self) -> bool {
        !self.held.is_empty()
    }

    fn write_restore_file(&self) {
        let mut text = String::new();
        for h in &self.held {
            text.push_str(&format!("{} {}\n", h.dir.display(), h.value));
        }
        if let Some(parent) = self.restore_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = self.restore_file.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &self.restore_file);
        }
    }

    /// Remember what the lights are now (once, before the first change).
    fn remember(&mut self) -> bool {
        if self.is_changed() {
            return true;
        }
        let lights: Vec<Light> = panel_backlights(&self.sys);
        // a panel that is already dark has nothing to dim or to restore
        self.held = lights.into_iter().filter(|l| l.value > 0).map(|l| Held { dir: l.dir, value: l.value }).collect();
        self.write_restore_file();
        self.is_changed()
    }

    /// Dim to `pct` percent of what it was (never below the lowest visible step, never above what it was).
    pub fn dim(&mut self, pct: u32) -> bool {
        if !self.remember() {
            return false;
        }
        let pct = pct.min(100);
        for h in &self.held {
            let max = read_u32(&h.dir.join("max_brightness")).unwrap_or(h.value).max(1);
            let target = (u64::from(h.value) * u64::from(pct) / 100) as u32;
            let floor = (max / 50).max(1).min(h.value);
            write_value(&h.dir, target.max(floor).min(h.value));
        }
        true
    }

    /// Backlight to 0.
    pub fn off(&mut self) -> bool {
        if !self.remember() {
            return false;
        }
        for h in &self.held {
            write_value(&h.dir, 0);
        }
        true
    }

    /// Put every backlight back exactly where it was and forget the restore file.
    pub fn restore(&mut self) {
        for h in self.held.drain(..) {
            write_value(&h.dir, h.value);
        }
        let _ = std::fs::remove_file(&self.restore_file);
    }
}

/// A shell that crashed while the screen was dimmed or off left the file behind: put the brightness back and remove it.
/// Returns how many backlights were restored.
pub fn restore_leftover(restore_file: &Path) -> usize {
    let Ok(text) = std::fs::read_to_string(restore_file) else { return 0 };
    let mut n = 0;
    for line in text.lines() {
        let Some((dir, value)) = line.rsplit_once(' ') else { continue };
        let (dir, Ok(value)) = (Path::new(dir), value.trim().parse::<u32>()) else { continue };
        // only a backlight folder: the file is in a RAM folder, but it is never trusted to point anywhere else
        if dir.join("max_brightness").is_file() && dir.join("brightness").is_file() && write_value(dir, value) {
            n += 1;
        }
    }
    let _ = std::fs::remove_file(restore_file);
    n
}

/// A panel that is at 0 when the shell starts is raised to about 60 % of its maximum. Returns whether one was raised.
pub fn ensure_visible(sys: &Path) -> bool {
    let mut raised = false;
    for l in panel_backlights(sys) {
        if l.value == 0 {
            let v = ((u64::from(l.max) * 60 / 100) as u32).max(1).min(l.max);
            raised |= write_value(&l.dir, v);
        }
    }
    raised
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_sys(tag: &str, value: u32, max: u32) -> (PathBuf, PathBuf) {
        let d = std::env::temp_dir().join(format!("iyakki-panel-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let bl = d.join("class/backlight/intel_backlight");
        std::fs::create_dir_all(&bl).expect("dir");
        std::fs::write(bl.join("brightness"), value.to_string()).expect("write");
        std::fs::write(bl.join("max_brightness"), max.to_string()).expect("write");
        (d.clone(), d.join("run/iyakki/backlight-restore"))
    }

    fn now(sys: &Path) -> u32 {
        read_u32(&sys.join("class/backlight/intel_backlight/brightness")).expect("brightness")
    }

    #[test]
    fn dimming_and_switching_off_remember_the_exact_value_and_waking_restores_it() {
        let (sys, file) = fake_sys("exact", 777, 1000);
        let mut p = Panel::new(&sys, &file);
        assert!(p.has_backlight());
        assert!(p.dim(25));
        assert_eq!(now(&sys), 194, "a quarter of 777");
        assert!(file.is_file(), "the old value is on file before anything else can go wrong");
        assert!(p.off());
        assert_eq!(now(&sys), 0);
        let on_file = std::fs::read_to_string(&file).expect("file");
        assert!(on_file.trim_end().ends_with(" 777"), "{on_file}: the value before the first change, not the dimmed one");
        p.restore();
        assert_eq!(now(&sys), 777, "exactly what it was");
        assert!(!file.exists() && !p.is_changed());
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn dimming_never_goes_below_the_lowest_step_or_above_the_old_value() {
        let (sys, file) = fake_sys("floor", 30, 1000);
        let mut p = Panel::new(&sys, &file);
        p.dim(10);
        assert_eq!(now(&sys), 20, "the lowest visible step (2 % of the maximum), not 3");
        p.restore();
        let (sys2, file2) = fake_sys("floor2", 10, 1000);
        let mut p2 = Panel::new(&sys2, &file2);
        p2.dim(50);
        assert!(now(&sys2) <= 10 && now(&sys2) >= 1, "already below the step: never raised above what it was");
        p2.restore();
        assert_eq!(now(&sys2), 10);
        let _ = (std::fs::remove_dir_all(&sys), std::fs::remove_dir_all(&sys2));
    }

    #[test]
    fn a_crash_while_off_is_repaired_by_the_restarted_shell_from_the_file() {
        let (sys, file) = fake_sys("crash", 640, 1000);
        {
            let mut p = Panel::new(&sys, &file);
            p.off();
            // the process dies here: no restore, the Panel is simply gone
        }
        assert_eq!(now(&sys), 0);
        assert!(file.is_file());
        assert_eq!(restore_leftover(&file), 1);
        assert_eq!(now(&sys), 640);
        assert!(!file.exists(), "used once");
        assert_eq!(restore_leftover(&file), 0, "nothing left to do");
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn a_restore_file_never_writes_anywhere_but_into_a_backlight_folder() {
        let (sys, file) = fake_sys("trust", 500, 1000);
        let victim = sys.join("victim");
        std::fs::create_dir_all(&victim).expect("dir");
        std::fs::write(victim.join("brightness"), "keep").expect("write");
        std::fs::create_dir_all(file.parent().expect("parent")).expect("dir");
        std::fs::write(&file, format!("{} 9\n{}/class/backlight/intel_backlight 321\nnonsense\n", victim.display(), sys.display())).expect("write");
        assert_eq!(restore_leftover(&file), 1);
        assert_eq!(std::fs::read_to_string(victim.join("brightness")).expect("read"), "keep");
        assert_eq!(now(&sys), 321);
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn a_shell_that_starts_on_a_dark_panel_raises_it() {
        let (sys, _) = fake_sys("dark", 0, 1000);
        assert!(ensure_visible(&sys));
        assert_eq!(now(&sys), 600);
        assert!(!ensure_visible(&sys), "a lit panel is left alone");
        assert_eq!(now(&sys), 600);
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn a_machine_without_a_backlight_changes_nothing_and_a_dark_panel_is_not_remembered() {
        let sys = std::env::temp_dir().join(format!("iyakki-panel-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&sys);
        std::fs::create_dir_all(&sys).expect("dir");
        let mut p = Panel::new(&sys, &sys.join("restore"));
        assert!(!p.has_backlight() && !p.dim(50) && !p.off());
        p.restore();
        let (sys2, file2) = fake_sys("already-dark", 0, 100);
        let mut p2 = Panel::new(&sys2, &file2);
        assert!(!p2.off(), "already 0: nothing to switch off or bring back");
        let _ = (std::fs::remove_dir_all(&sys), std::fs::remove_dir_all(&sys2));
    }

    struct Fake {
        calls: Vec<i32>,
        fail: bool,
    }
    impl BlankIoctl for Fake {
        fn blank(&mut self, level: i32) -> Result<(), String> {
            self.calls.push(level);
            if self.fail { Err("no".into()) } else { Ok(()) }
        }
    }

    #[test]
    fn the_blank_call_uses_the_kernels_levels_and_reports_failures() {
        let mut f = Fake { calls: vec![], fail: false };
        blank_screen(&mut f, true).expect("off");
        blank_screen(&mut f, false).expect("on");
        assert_eq!(f.calls, vec![FB_BLANK_POWERDOWN, FB_BLANK_UNBLANK]);
        assert_eq!((FB_BLANK_UNBLANK, FB_BLANK_POWERDOWN, FBIOBLANK), (0, 4, 0x4611), "linux/fb.h");
        let mut bad = Fake { calls: vec![], fail: true };
        assert!(blank_screen(&mut bad, true).is_err());
        // a closed descriptor is an error, never a crash
        assert!(ioctl_blank(-1, FB_BLANK_POWERDOWN).is_err());
    }
}
