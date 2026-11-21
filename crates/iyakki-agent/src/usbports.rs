//! The USB port monitor's once-a-second step: writes to the log what changed on the USB side. The reading itself (what the kernel sees:
//! controllers, devices, xHCI ports, the lockdown check) is `iyakki_core_local::usbports` (F-ARCH-07), re-exported here.

use std::sync::Mutex;

pub use iyakki_core_local::usbports::*;

static LAST: Mutex<Option<Snapshot>> = Mutex::new(None);
static DEBUGFS_SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Called every poll: writes the lines for what changed since the last call (the first call writes the starting state).
pub fn step() {
    if cfg!(test) {
        return;
    }
    let now = snapshot_here();
    if now.ports.is_empty() && !DEBUGFS_SAID.swap(true, std::sync::atomic::Ordering::Relaxed) && !now.controllers.is_empty() {
        log::info!("usb: port states are not available ({}); devices and controller power are still logged", if debugfs_locked_here() { "kernel lockdown is on, so the kernel's debug files are not read" } else { "debugfs is not mounted or has no xHCI ports" });
    }
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let old = last.take().unwrap_or_default();
    for line in changes(&old, &now) {
        log::info!("{line}");
    }
    *last = Some(now);
}
