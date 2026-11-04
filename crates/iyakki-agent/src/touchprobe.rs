//! The touch check of the startup list: a USB touch screen that is there at boot is detached and attached once, in software, as soon as the
//! boot has settled.
//!
//! Why: a touch monitor on USB-C resets its USB hub when its display link retrains early in the boot, and its panel came back in a state where
//! it answered nothing until the monitor was plugged in again by hand (found on a Mac mini with a USB-C touch monitor; the shell's own
//! logs showed the panel open and silent). Toggling `authorized` of that one USB device is the same re-enumeration a plug makes, without
//! touching the hub (the boot drive may hang on the same one). The shell opens the new input nodes by itself (`input.rs`, background scan).
//!
//! [`step`] starts the one-time re-attach in a thread when it is due; [`state`] is what the startup rules read.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The boot has to be this old before the panel is re-attached: the display link and the hub resets are over by then.
pub const SETTLE_SECS: u64 = 10;

/// A touch panel on USB: the USB device directory in sysfs, and the name the input layer gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panel {
    pub usb: PathBuf,
    pub name: String,
}

/// What the startup rules read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TouchFact {
    /// USB touch panels the kernel lists.
    pub panels: usize,
    /// All of them have been re-attached.
    pub attached: bool,
}

struct Ledger {
    /// USB device directories already re-attached, or being re-attached, this boot.
    done: Vec<PathBuf>,
    /// Re-attaches still running.
    busy: usize,
}

static LEDGER: Mutex<Ledger> = Mutex::new(Ledger { done: Vec::new(), busy: 0 });

/// The USB touch panels under `sys` (`/sys`): input devices called "...touch..." that sit on a USB device. One entry per USB device.
pub fn panels(sys: &Path) -> Vec<Panel> {
    let mut out: Vec<Panel> = Vec::new();
    let Ok(dir) = std::fs::read_dir(sys.join("class/input")) else { return out };
    let mut names: Vec<_> = dir.flatten().map(|e| e.path()).collect();
    names.sort();
    for p in names {
        let Some(file) = p.file_name().and_then(|f| f.to_str()) else { continue };
        if !file.starts_with("input") {
            continue;
        }
        let name = std::fs::read_to_string(p.join("name")).unwrap_or_default().trim().to_string();
        if !name.to_ascii_lowercase().contains("touch") {
            continue;
        }
        let Ok(real) = std::fs::canonicalize(&p) else { continue };
        let Some(usb) = real.ancestors().find(|a| a.join("idVendor").exists()) else { continue };
        if !out.iter().any(|x| x.usb == usb) {
            out.push(Panel { usb: usb.to_path_buf(), name });
        }
    }
    out
}

/// What the rules read: the panels now, and whether every one of them has had its re-attach.
pub fn state_of(found: &[Panel], ledger_done: &[PathBuf], busy: usize) -> TouchFact {
    TouchFact { panels: found.len(), attached: busy == 0 && found.iter().all(|p| ledger_done.contains(&p.usb)) }
}

pub fn state() -> TouchFact {
    let l = LEDGER.lock().unwrap_or_else(|e| e.into_inner());
    state_of(&panels(Path::new("/sys")), &l.done, l.busy)
}

/// Detach and attach one USB device (`authorized` 0 then 1). The pause lets the kernel finish removing the old input nodes.
pub fn reattach(usb: &Path) -> std::io::Result<()> {
    // a touch panel that is let to sleep in the first seconds of the boot is one that comes up half awake: keep its power on
    let _ = std::fs::write(usb.join("power/control"), "on");
    let auth = usb.join("authorized");
    std::fs::write(&auth, "0")?;
    std::thread::sleep(std::time::Duration::from_millis(400));
    std::fs::write(&auth, "1")
}

/// A second press within this many seconds of the first goes a step further (the monitor's hub).
pub const ESCALATE_SECS: u64 = 180;

static LAST_PRESS: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// The step of this press of the Probe button: 1 (the panel), or 2 (the panel and the hub it hangs on) when the last press was a moment ago.
pub fn next_level() -> u8 {
    let mut l = LAST_PRESS.lock().unwrap_or_else(|e| e.into_inner());
    let again = l.is_some_and(|t| t.elapsed().as_secs() < ESCALATE_SECS);
    *l = Some(std::time::Instant::now());
    if again { 2 } else { 1 }
}

/// The USB hub a device hangs on: the nearest parent that is a USB device itself.
pub fn hub_of(usb: &Path) -> Option<PathBuf> {
    usb.parent().and_then(|p| p.ancestors().find(|a| a.join("idVendor").exists())).map(Path::to_path_buf)
}

/// The Probe touch screen button: switch every USB touch panel off and on (step 1); step 2 does the same to the hub each one hangs on,
/// which is closest to unplugging the monitor's cable (what woke a panel that stayed silent after the boot). Blocks about a second. Returns
/// the line to show.
pub fn probe(sys: &Path, level: u8) -> String {
    let found = panels(sys);
    if found.is_empty() {
        return "No USB touch screen is listed by the kernel. Check the monitor's USB cable; a touch screen that is not on USB cannot be probed here.".into();
    }
    let mut said: Vec<String> = Vec::new();
    for p in &found {
        match reattach(&p.usb) {
            Ok(()) => said.push(format!("{} switched off and on", p.name)),
            Err(e) => said.push(format!("{}: {e}", p.name)),
        }
        if level >= 2 {
            match hub_of(&p.usb) {
                Some(h) => match reattach(&h) {
                    Ok(()) => said.push("its USB hub switched off and on".into()),
                    Err(e) => said.push(format!("its USB hub: {e}")),
                },
                None => said.push("no hub above it to reset".into()),
            }
        }
    }
    log::info!("touch: probe step {level}: {}", said.join("; "));
    let tail = if level >= 2 {
        "Devices on that hub (the boot drive, if it hangs there) can disconnect for a moment. Wait five seconds and try touch."
    } else {
        "Wait two seconds and try touch. Still nothing: press the button again within three minutes to reset the monitor's USB hub too."
    };
    format!("Touch probe: {}. {tail}", said.join(", "))
}

/// Start the re-attach of every panel that has not had one, when the boot is `uptime_secs` old (not before [`SETTLE_SECS`]). Never waits.
pub fn step(uptime_secs: u64) {
    if uptime_secs < SETTLE_SECS {
        return;
    }
    let found = panels(Path::new("/sys"));
    let mut l = LEDGER.lock().unwrap_or_else(|e| e.into_inner());
    for p in found {
        if l.done.contains(&p.usb) {
            continue;
        }
        l.done.push(p.usb.clone());
        l.busy += 1;
        std::thread::spawn(move || {
            match reattach(&p.usb) {
                Ok(()) => log::info!("touch: {} re-attached once after the boot settled ({})", p.name, p.usb.display()),
                Err(e) => log::info!("touch: {} could not be re-attached ({e}); it keeps the state it came up in", p.name),
            }
            LEDGER.lock().unwrap_or_else(|e| e.into_inner()).busy -= 1;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    /// A sysfs with a USB device `1-5.3` that carries two input nodes, and a keyboard on another one.
    fn sys(t: &Path) {
        let usb = t.join("devices/usb1/1-5/1-5.3");
        put(&usb.join("idVendor"), "27c0\n");
        for (n, name) in [(4, "wch.cn TouchScreen Touchscreen"), (5, "wch.cn TouchScreen")] {
            put(&usb.join(format!("1-5.3:1.{n}/input/input{n}/name")), &format!("{name}\n"));
            std::fs::create_dir_all(t.join("class/input")).unwrap();
            std::os::unix::fs::symlink(usb.join(format!("1-5.3:1.{n}/input/input{n}")), t.join(format!("class/input/input{n}"))).unwrap();
        }
        let kb = t.join("devices/usb1/1-14/1-14.2.4");
        put(&kb.join("idVendor"), "2222\n");
        put(&kb.join("1-14.2.4:1.0/input/input7/name"), "USB Keyboard\n");
        std::os::unix::fs::symlink(kb.join("1-14.2.4:1.0/input/input7"), t.join("class/input/input7")).unwrap();
    }

    #[test]
    fn a_touch_monitor_with_two_input_nodes_is_one_panel_and_a_keyboard_is_none() {
        let t = tempfile_dir("tp1");
        sys(&t);
        let p = panels(&t);
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].usb.ends_with("1-5.3"));
    }

    #[test]
    fn no_touch_screen_means_no_panels_and_the_state_says_so() {
        let t = tempfile_dir("tp2");
        std::fs::create_dir_all(t.join("class/input")).unwrap();
        assert_eq!(state_of(&panels(&t), &[], 0), TouchFact { panels: 0, attached: true });
    }

    #[test]
    fn the_panel_counts_as_attached_only_when_it_has_had_its_turn_and_nothing_is_running() {
        let t = tempfile_dir("tp3");
        sys(&t);
        let p = panels(&t);
        assert!(!state_of(&p, &[], 0).attached);
        assert!(!state_of(&p, &[p[0].usb.clone()], 1).attached, "still running");
        assert!(state_of(&p, &[p[0].usb.clone()], 0).attached);
    }

    #[test]
    fn reattach_keeps_the_power_on_and_writes_zero_then_one_to_authorized() {
        let t = tempfile_dir("tp4");
        std::fs::write(t.join("authorized"), "1").unwrap();
        std::fs::create_dir_all(t.join("power")).unwrap();
        std::fs::write(t.join("power/control"), "auto").unwrap();
        reattach(&t).unwrap();
        assert_eq!(std::fs::read_to_string(t.join("power/control")).unwrap(), "on");
        assert_eq!(std::fs::read_to_string(t.join("authorized")).unwrap(), "1");
    }

    #[test]
    fn probe_step_one_touches_the_panel_only_and_step_two_the_hub_as_well() {
        let t = tempfile_dir("tp5");
        sys(&t);
        let usb = t.join("devices/usb1/1-5/1-5.3");
        let hub = t.join("devices/usb1/1-5");
        put(&hub.join("idVendor"), "1a40\n");
        for d in [&usb, &hub] {
            put(&d.join("authorized"), "1");
        }
        let said = probe(&t, 1);
        assert!(said.contains("switched off and on") && !said.contains("hub switched"), "{said}");
        assert_eq!(hub_of(&usb), Some(hub.clone()));
        let said = probe(&t, 2);
        assert!(said.contains("its USB hub switched off and on"), "{said}");
        assert_eq!(std::fs::read_to_string(hub.join("authorized")).unwrap(), "1", "the hub is left on");
    }

    #[test]
    fn probe_without_a_touch_screen_says_so() {
        let t = tempfile_dir("tp6");
        std::fs::create_dir_all(t.join("class/input")).unwrap();
        assert!(probe(&t, 1).starts_with("No USB touch screen"));
    }

    #[test]
    fn the_second_press_goes_a_step_further() {
        *LAST_PRESS.lock().unwrap() = None;
        assert_eq!(next_level(), 1);
        assert_eq!(next_level(), 2);
    }

    fn tempfile_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-touchprobe-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
