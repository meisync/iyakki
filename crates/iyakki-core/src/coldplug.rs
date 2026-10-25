//! Repeated coldplug: drivers that create child devices.
//!
//! The first coldplug pass reads the `modalias` of every device that exists when it starts and asks `modprobe` for the drivers.
//! Some of those drivers create devices of their own while they probe: the Intel LPSS PCI driver (`intel_lpss_pci`) makes the SPI,
//! I2C and UART controllers of a laptop (`platform:pxa2xx-spi`, `platform:i2c_designware`), an MFD driver makes its cells. Those
//! children did not exist during the first scan, so nothing loaded their drivers (on a MacBookPro14,1 the Apple keyboard and
//! trackpad stayed dead for that reason). [`settle`] repeats the scan: every alias not seen before is handed to the loader, and the
//! scan goes on until a pass finds nothing new, the pass limit is reached or the time budget is spent.
//!
//! Cost on a machine without such children: one extra directory walk of `/sys/devices` (a few milliseconds) and no `modprobe`.
//! Nothing here touches the system by itself; the scan root, the loader and the pause are parameters so tests use a fake sysfs.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::boot::collect_modaliases;

/// Passes after the first one, at most.
pub const MAX_EXTRA_PASSES: usize = 3;
/// Time budget of all extra passes together (the loader's own time counts: a slow `modprobe` ends the loop).
pub const BUDGET: Duration = Duration::from_millis(3000);
/// Pause before looking again after something was loaded: a child device appears while the parent's probe runs, which is
/// usually done when `modprobe` returns, but an asynchronous probe may need a moment.
pub const PAUSE: Duration = Duration::from_millis(80);

/// The aliases already handed to the loader (or deliberately skipped).
#[derive(Default, Debug, Clone)]
pub struct Seen(HashSet<String>);

impl Seen {
    pub fn new() -> Seen {
        Seen::default()
    }
    /// Remember these as handled.
    pub fn add_all(&mut self, aliases: &[String]) {
        self.0.extend(aliases.iter().cloned());
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// What the passes did.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct Settled {
    /// Number of extra passes that found new aliases and loaded them.
    pub passes: usize,
    /// All aliases found by those passes, in the order they were found.
    pub found: Vec<String>,
}

/// The aliases under `root` that are not in `seen` yet; they are added to `seen`.
pub fn rescan(root: &Path, seen: &mut Seen) -> Vec<String> {
    let mut fresh = Vec::new();
    for a in collect_modaliases(root) {
        if seen.0.insert(a.clone()) {
            fresh.push(a);
        }
    }
    fresh
}

/// Scan again and again until nothing new shows up. `load` gets the new aliases of each pass (it is the place that decides what
/// is critical and what waits for the sign-in screen); `pause` waits between passes.
pub fn settle(root: &Path, seen: &mut Seen, max_passes: usize, budget: Duration, mut load: impl FnMut(&[String]), pause: impl Fn()) -> Settled {
    let start = Instant::now();
    let mut out = Settled::default();
    for _ in 0..max_passes {
        let fresh = rescan(root, seen);
        if fresh.is_empty() {
            break;
        }
        load(&fresh);
        out.passes += 1;
        out.found.extend(fresh);
        if start.elapsed() >= budget {
            break;
        }
        pause();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::fs;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-coldplug-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn device(root: &Path, rel: &str, alias: &str) {
        let d = root.join(rel);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("modalias"), format!("{alias}\n")).unwrap();
    }

    const LPSS: &str = "pci:v00008086d0000A129sv000106Bsd00003300bc11sc80i00";
    const SPI: &str = "platform:pxa2xx-spi";
    const SPI_ACPI: &str = "acpi:APP000D:";

    #[test]
    fn a_driver_that_creates_a_child_device_gets_the_child_loaded_in_the_second_pass() {
        let root = tmp("child");
        device(&root, "pci0000:00/0000:00:1e.3", LPSS);
        let mut seen = Seen::new();
        seen.add_all(&collect_modaliases(&root)); // the first pass, done by the caller
        let calls: RefCell<Vec<Vec<String>>> = RefCell::new(Vec::new());
        let r = root.clone();
        let res = settle(&root, &mut seen, MAX_EXTRA_PASSES, BUDGET, |new| {
            calls.borrow_mut().push(new.to_vec());
        }, || {});
        // nothing new yet: the parent's driver has not run, no modprobe call, one pass looked
        assert_eq!(res, Settled::default());
        assert!(calls.borrow().is_empty());

        // the first pass loads intel_lpss_pci, which creates the SPI controller as a child device
        device(&r, "pci0000:00/0000:00:1e.3/pxa2xx-spi.3", SPI);
        let res = settle(&root, &mut seen, MAX_EXTRA_PASSES, BUDGET, |new| calls.borrow_mut().push(new.to_vec()), || {});
        assert_eq!(res.passes, 1);
        assert_eq!(res.found, vec![SPI.to_string()]);
        assert_eq!(*calls.borrow(), vec![vec![SPI.to_string()]]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn children_of_children_follow_up_to_the_pass_limit_and_each_alias_is_loaded_once() {
        let root = tmp("chain");
        device(&root, "pci0000:00/0000:00:1e.3", LPSS);
        let mut seen = Seen::new();
        seen.add_all(&collect_modaliases(&root));
        let calls = RefCell::new(Vec::<Vec<String>>::new());
        let n = Cell::new(0);
        let r = root.clone();
        // the first pass (by the caller) loaded the parent, which created the SPI controller
        device(&root, "pci0000:00/0000:00:1e.3/pxa2xx-spi.3", SPI);
        // fake loader: each load creates the next level (the keyboard device behind the controller, then its input node)
        let res = settle(&root, &mut seen, MAX_EXTRA_PASSES, BUDGET, |new| {
            calls.borrow_mut().push(new.to_vec());
            n.set(n.get() + 1);
            match n.get() {
                1 => device(&r, "pci0000:00/0000:00:1e.3/pxa2xx-spi.3/spi2/spi-APP000D:00", SPI_ACPI),
                2 => device(&r, "pci0000:00/0000:00:1e.3/pxa2xx-spi.3/spi2/spi-APP000D:00/input", "input:b0003v05ACp0275"),
                _ => {}
            }
        }, || {});
        assert_eq!(res.passes, 3, "SPI controller, then the keyboard device, then the input node; the 4th scan is empty and stops");
        assert_eq!(res.found, vec![SPI.to_string(), SPI_ACPI.to_string(), "input:b0003v05ACp0275".to_string()]);
        // every alias exactly once
        let flat: Vec<String> = calls.borrow().iter().flatten().cloned().collect();
        let uniq: HashSet<&String> = flat.iter().collect();
        assert_eq!(flat.len(), uniq.len());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_pass_limit_and_the_time_budget_bound_the_loop() {
        let root = tmp("bound");
        let mut seen = Seen::new();
        let n = Cell::new(0);
        let r = root.clone();
        // a device that creates a new device every time it is loaded would loop for ever without the limit
        let res = settle(&root, &mut seen, 3, BUDGET, |_| {
            n.set(n.get() + 1);
            device(&r, &format!("d{}", n.get() + 1), &format!("platform:x{}", n.get() + 1));
        }, || {});
        assert_eq!(res.passes, 0, "no device at all: nothing to do");
        device(&root, "d0", "platform:x0");
        let res = settle(&root, &mut seen, 3, BUDGET, |_| {
            n.set(n.get() + 1);
            device(&r, &format!("d{}", n.get() + 10), &format!("platform:y{}", n.get() + 10));
        }, || {});
        assert_eq!(res.passes, 3);
        assert_eq!(n.get(), 3, "three loader calls, never more");
        // a zero budget stops after the first pass that loaded something
        let mut seen2 = Seen::new();
        let m = Cell::new(0);
        let res = settle(&root, &mut seen2, 10, Duration::ZERO, |_| m.set(m.get() + 1), || {});
        assert_eq!(res.passes, 1);
        assert_eq!(m.get(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn aliases_seen_in_the_first_pass_are_not_loaded_again_and_a_quiet_machine_makes_no_loader_call() {
        let root = tmp("quiet");
        for (i, a) in ["pci:v00008086d0000A130", "usb:v1D6Bp0002", "acpi:PNP0C0A:", LPSS].iter().enumerate() {
            device(&root, &format!("dev{i}"), a);
        }
        let mut seen = Seen::new();
        seen.add_all(&collect_modaliases(&root));
        assert_eq!(seen.len(), 4);
        let calls = Cell::new(0);
        let paused = Cell::new(0);
        let res = settle(&root, &mut seen, MAX_EXTRA_PASSES, BUDGET, |_| calls.set(calls.get() + 1), || paused.set(paused.get() + 1));
        assert_eq!((res.passes, calls.get(), paused.get()), (0, 0, 0), "no modprobe and no sleeping when nothing new appeared");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn it_pauses_only_after_a_pass_that_loaded_something() {
        let root = tmp("pause");
        let mut seen = Seen::new();
        device(&root, "a", "platform:a");
        let paused = Cell::new(0);
        let r = root.clone();
        let n = Cell::new(0);
        let res = settle(&root, &mut seen, 3, BUDGET, |_| {
            n.set(n.get() + 1);
            if n.get() == 1 {
                device(&r, "b", "platform:b");
            }
        }, || paused.set(paused.get() + 1));
        assert_eq!(res.passes, 2);
        assert_eq!(paused.get(), 2);
        let _ = fs::remove_dir_all(&root);
    }
}
