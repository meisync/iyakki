//! Keep the USB controllers behind Thunderbolt ports awake: the timing and the log line. The sysfs reads and writes (which controllers
//! sleep, the switch to `on`, the boot-stage self-test) are `iyakki_core_local::usbawake` (F-ARCH-07), re-exported here; its header says why.

use std::path::Path;

pub use iyakki_core_local::usbawake::*;

static RUNS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Called every poll with the uptime: acts at 8 s (the Thunderbolt drivers have bound by then) and once more at 40 s (a controller that
/// appeared later). Does nothing when the command line switches it off.
pub fn step(uptime_secs: u64) {
    if cfg!(test) {
        return;
    }
    let done = RUNS.load(std::sync::atomic::Ordering::Relaxed);
    let due = (done == 0 && uptime_secs >= 8) || (done == 1 && uptime_secs >= 40);
    if !due {
        return;
    }
    RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if switched_off_here() {
        return;
    }
    let sleeping = sleeping_controllers(Path::new("/"));
    if sleeping.is_empty() {
        return;
    }
    let kept = keep_awake(&sleeping);
    log::info!("usb: {} USB controller(s) behind the Thunderbolt bus were left to runtime power management; kept awake: {}", sleeping.len(), kept.join(" "));
}

