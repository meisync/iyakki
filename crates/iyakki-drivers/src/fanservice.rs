//! The fan service (F-DRIVERS-14): the loop that runs [`crate::fan::Controller`] every few seconds, the setting that turns it on
//! (a file `fan.json` in the data folder, **off unless a person turned it on**) and the status text the chat tool shows.
//!
//! Every path is below roots the caller gives (`/sys` and the data folder in the image, temp folders in the tests). The service
//! reports what it did as events (`take-over`, `release`, `curve`) through a callback; the core runs this loop as a supervised child
//! process (`iyakki fan-service`) and writes each event to the audit log, so there is one writer of the log.
//!
//! Safety: when the loop stops for any reason, or the setting is turned off, the controller is dropped and the fans go back to
//! the firmware's own curve.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::fan::{Controller, Curve, Step};

/// The person's choice. Off by default: nothing writes to a fan until it is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setting {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "d_low")]
    pub low: i32,
    #[serde(default = "d_high")]
    pub high: i32,
    #[serde(default = "d_max")]
    pub max: i32,
    #[serde(default = "d_hyst")]
    pub hysteresis: i32,
}

fn d_low() -> i32 {
    Curve::default().low
}
fn d_high() -> i32 {
    Curve::default().high
}
fn d_max() -> i32 {
    Curve::default().max
}
fn d_hyst() -> i32 {
    Curve::default().hysteresis
}

impl Default for Setting {
    fn default() -> Setting {
        let c = Curve::default();
        Setting { enabled: false, low: c.low, high: c.high, max: c.max, hysteresis: c.hysteresis }
    }
}

impl Setting {
    pub fn curve(&self) -> Curve {
        Curve { low: self.low, high: self.high, max: self.max, hysteresis: self.hysteresis }
    }

    pub fn describe(&self) -> String {
        format!("{}; curve: slowest up to {} C, rising to the fastest at {} C (mark {} C), lowering only {} C below the last rise", if self.enabled { "on" } else { "off" }, self.low, self.max, self.high, self.hysteresis)
    }
}

pub const SETTING_FILE: &str = "fan.json";
pub const STATUS_FILE: &str = "fan-status.txt";

/// The setting in `dir`; the default (off) when the file is missing or unreadable.
pub fn load(dir: &Path) -> Setting {
    fs::read_to_string(dir.join(SETTING_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Saves a setting whose curve is valid. A write is atomic (a temp file, then a rename).
pub fn save(dir: &Path, s: &Setting) -> Result<(), String> {
    if !s.curve().valid() {
        return Err("the curve is not valid: it needs 20 <= low < mark <= max <= 110 and a hysteresis of 0 to 10".into());
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!("{SETTING_FILE}.tmp"));
    fs::write(&tmp, serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fs::rename(&tmp, dir.join(SETTING_FILE)).map_err(|e| e.to_string())
}

/// What the service last wrote (one line), or why there is nothing.
pub fn status(dir: &Path) -> String {
    fs::read_to_string(dir.join(STATUS_FILE)).map(|t| t.trim().to_string()).unwrap_or_else(|_| "the fan service has not reported yet".into())
}

/// Whether this computer has fans the controller can drive (an `applesmc` with a fan and a temperature below `sys`).
pub fn supported(sys: &Path) -> bool {
    Controller::discover(sys, Curve::default()).is_some()
}

fn write_status(dir: &Path, text: &str) {
    let _ = fs::write(dir.join(STATUS_FILE), text);
}

fn nap(total: Duration, stop: &AtomicBool) {
    let mut left = total;
    let slice = Duration::from_millis(20);
    while !left.is_zero() && !stop.load(Ordering::SeqCst) {
        let t = left.min(slice);
        std::thread::sleep(t);
        left -= t;
    }
}

/// Runs until `stop` is set. `emit(event, detail)` is told about every change of who controls the fans.
pub fn run(sys: &Path, dir: &Path, stop: &AtomicBool, interval: Duration, emit: &mut dyn FnMut(&str, &str)) {
    let mut ctl: Option<(Controller, Setting)> = None;
    let mut unsupported_said = false;
    while !stop.load(Ordering::SeqCst) {
        let s = load(dir);
        if !s.enabled || !s.curve().valid() {
            if ctl.take().is_some() {
                emit("fan.release", "fans handed back to the firmware: the fan control is off or its curve is not valid");
            }
            write_status(dir, if s.enabled { "fan control is on but the curve is not valid; the firmware curve is in charge" } else { "fan control is off; the firmware curve is in charge" });
        } else {
            if ctl.as_ref().is_some_and(|(_, old)| old.curve() != s.curve()) {
                ctl = None;
                emit("fan.curve", &format!("curve changed: {}", s.describe()));
            }
            if ctl.is_none() {
                match Controller::discover(sys, s.curve()) {
                    Some(c) => {
                        emit("fan.take-over", &format!("{} fan(s) controlled by IYAKKI; {}", c.fans().len(), s.describe()));
                        ctl = Some((c, s));
                        unsupported_said = false;
                    }
                    None => {
                        if !unsupported_said {
                            emit("fan.unsupported", "no applesmc fan with a temperature input on this computer; nothing is written");
                            unsupported_said = true;
                        }
                        write_status(dir, "fan control is on but this computer has no applesmc fan with a temperature; nothing is written");
                    }
                }
            }
            if let Some((c, _)) = ctl.as_mut() {
                if let Step::Released { reason } = c.step() {
                    emit("fan.release", &format!("fans handed back to the firmware: {reason}"));
                    ctl = None;
                    write_status(dir, &format!("the firmware curve is in charge: {reason}"));
                } else {
                    write_status(dir, &c.status());
                }
            }
        }
        nap(interval, stop);
    }
    if ctl.take().is_some() {
        emit("fan.release", "fans handed back to the firmware: the service stopped");
    }
    write_status(dir, "the fan service is stopped; the firmware curve is in charge");
}

#[cfg(test)]
#[path = "fanservice_tests.rs"]
mod tests;
