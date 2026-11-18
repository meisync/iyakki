//! A small fan controller for Macs, on top of the `applesmc` hwmon files (what `mbpfan` does). F-DRIVERS-14.
//!
//! The controller reads the CPU temperatures (`coretemp`, or the `applesmc` temperature inputs when there is no
//! `coretemp`), takes the hottest one, maps it to a fan speed with a linear curve and writes it to the fans'
//! `fanN_output` after switching them to manual (`fanN_manual` = 1). Every path is below a root that the caller gives
//! (`/sys` in the image, a fake tree in the tests); the module starts no process.
//!
//! Safety rules, all covered by tests:
//! * a temperature that cannot be read, or reads as 0 or above 120 degrees, hands the fans back to the firmware
//!   (`fanN_manual` = 0): the Mac's own curve is always the fallback;
//! * the speed never goes below the fan's own `fanN_min` or above `fanN_max`;
//! * falling temperatures only lower the speed after they are `hysteresis` degrees below the step that raised it,
//!   so the fan does not hunt;
//! * [`Controller::release`] gives every fan back (called on stop, and by the service when it shuts down).

use std::fs;
use std::path::{Path, PathBuf};

/// The curve. Temperatures in whole degrees Celsius.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Curve {
    /// At or below this the fan runs at its minimum.
    pub low: i32,
    /// Between `low` and `max` the speed rises linearly; from `high` up it is already the fan's maximum scaled
    /// by the same line, kept for the report only.
    pub high: i32,
    /// At or above this the fan runs at its maximum.
    pub max: i32,
    /// Degrees a falling temperature must drop below the last raising step before the speed is lowered.
    pub hysteresis: i32,
}

impl Default for Curve {
    /// The defaults of `mbpfan`: 63 / 66 / 86.
    fn default() -> Curve {
        Curve { low: 63, high: 66, max: 86, hysteresis: 2 }
    }
}

impl Curve {
    /// A curve that makes sense: `low < high <= max`, all between 20 and 110.
    pub fn valid(&self) -> bool {
        20 <= self.low && self.low < self.high && self.high <= self.max && self.max <= 110 && (0..=10).contains(&self.hysteresis)
    }

    /// The target speed for `temp`, between `min` and `max_rpm`.
    pub fn speed(&self, temp: i32, min: u32, max_rpm: u32) -> u32 {
        if temp <= self.low || max_rpm <= min {
            return min;
        }
        if temp >= self.max {
            return max_rpm;
        }
        let span = (self.max - self.low) as u64;
        let at = (temp - self.low) as u64;
        min + ((max_rpm - min) as u64 * at / span) as u32
    }
}

/// One fan found in the hwmon directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fan {
    pub index: u32,
    pub min: u32,
    pub max: u32,
    dir: PathBuf,
}

/// What a controller step did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Fans are set to this speed for this hottest temperature.
    Set { temp: i32, rpm: Vec<u32> },
    /// Nothing changed (the hysteresis held the speed).
    Held { temp: i32 },
    /// No usable temperature; the fans were handed back to the firmware.
    Released { reason: String },
}

pub struct Controller {
    pub curve: Curve,
    fans: Vec<Fan>,
    temps: Vec<PathBuf>,
    /// The temperature of the last step that changed the speed.
    last: Option<i32>,
    manual: bool,
}

fn read_num(p: &Path) -> Option<i64> {
    fs::read_to_string(p).ok()?.trim().parse().ok()
}

/// The hwmon directory whose `name` is `name`, below `sys/class/hwmon`.
fn hwmon_named(sys: &Path, name: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(sys.join("class/hwmon"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| fs::read_to_string(p.join("name")).map(|n| n.trim() == name).unwrap_or(false))
        .collect();
    v.sort();
    v
}

impl Controller {
    /// Finds the fans of `applesmc` and the CPU temperature inputs below `sys`. `None` when there is no `applesmc`
    /// with at least one fan that has a manual switch and an output file, or when the curve is not valid.
    pub fn discover(sys: &Path, curve: Curve) -> Option<Controller> {
        if !curve.valid() {
            return None;
        }
        let smc = hwmon_named(sys, "applesmc");
        let mut fans = Vec::new();
        for dir in &smc {
            for i in 1..=8u32 {
                let (m, o) = (dir.join(format!("fan{i}_manual")), dir.join(format!("fan{i}_output")));
                if !m.exists() || !o.exists() {
                    continue;
                }
                let min = read_num(&dir.join(format!("fan{i}_min"))).unwrap_or(0).clamp(0, 20000) as u32;
                let max = read_num(&dir.join(format!("fan{i}_max"))).unwrap_or(0).clamp(0, 20000) as u32;
                if max == 0 || max < min {
                    continue;
                }
                fans.push(Fan { index: i, min, max, dir: dir.clone() });
            }
        }
        if fans.is_empty() {
            return None;
        }
        let mut temps = Vec::new();
        for dir in hwmon_named(sys, "coretemp") {
            for i in 1..=64u32 {
                let p = dir.join(format!("temp{i}_input"));
                if p.exists() {
                    temps.push(p);
                }
            }
        }
        if temps.is_empty() {
            for dir in &smc {
                for i in 1..=64u32 {
                    let p = dir.join(format!("temp{i}_input"));
                    let label = fs::read_to_string(dir.join(format!("temp{i}_label"))).unwrap_or_default();
                    if p.exists() && label.contains("CPU") {
                        temps.push(p);
                    }
                }
            }
        }
        if temps.is_empty() {
            return None;
        }
        Some(Controller { curve, fans, temps, last: None, manual: false })
    }

    pub fn fans(&self) -> &[Fan] {
        &self.fans
    }

    /// The hottest reading in whole degrees; `None` when any input cannot be read or is out of range.
    pub fn hottest(&self) -> Result<i32, String> {
        let mut hot = i32::MIN;
        for p in &self.temps {
            let milli = read_num(p).ok_or_else(|| format!("cannot read {}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))?;
            let deg = (milli / 1000) as i32;
            if deg <= 0 || deg > 120 {
                return Err(format!("implausible temperature {deg}"));
            }
            hot = hot.max(deg);
        }
        Ok(hot)
    }

    /// One control step: read, decide, write.
    pub fn step(&mut self) -> Step {
        let temp = match self.hottest() {
            Ok(t) => t,
            Err(reason) => {
                self.release();
                return Step::Released { reason };
            }
        };
        if let Some(last) = self.last {
            let lowered = temp < last;
            // Raising is immediate; lowering waits until the temperature is `hysteresis` below the last step.
            if self.manual && lowered && last - temp < self.curve.hysteresis {
                return Step::Held { temp };
            }
            if self.manual && temp == last {
                return Step::Held { temp };
            }
        }
        let mut rpm = Vec::new();
        for f in self.fans.clone() {
            let want = self.curve.speed(temp, f.min, f.max).clamp(f.min, f.max);
            let ok = fs::write(f.dir.join(format!("fan{}_manual", f.index)), "1").is_ok() && fs::write(f.dir.join(format!("fan{}_output", f.index)), want.to_string()).is_ok();
            if !ok {
                self.release();
                return Step::Released { reason: format!("cannot write fan{}", f.index) };
            }
            rpm.push(want);
        }
        self.manual = true;
        self.last = Some(temp);
        Step::Set { temp, rpm }
    }

    /// Hands every fan back to the firmware's own curve.
    pub fn release(&mut self) {
        for f in &self.fans {
            let _ = fs::write(f.dir.join(format!("fan{}_manual", f.index)), "0");
        }
        self.manual = false;
        self.last = None;
    }

    /// Read-only status for the chat tool and the Settings line.
    pub fn status(&self) -> String {
        let t = self.hottest().map(|t| format!("{t} C")).unwrap_or_else(|e| e);
        let fans: Vec<String> = self.fans.iter().map(|f| format!("fan{} {}..{} rpm, now {}", f.index, f.min, f.max, read_num(&f.dir.join(format!("fan{}_input", f.index))).unwrap_or(-1))).collect();
        format!("hottest {t}; {}; {}", fans.join("; "), if self.manual { "controlled by IYAKKI" } else { "firmware curve" })
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn tree(core: bool) -> PathBuf {
        let root = std::env::temp_dir().join(format!("iyakki-fan-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&root);
        let smc = root.join("class/hwmon/hwmon2");
        fs::create_dir_all(&smc).unwrap();
        fs::write(smc.join("name"), "applesmc\n").unwrap();
        for (k, v) in [("fan1_min", "1200"), ("fan1_max", "6200"), ("fan1_manual", "0"), ("fan1_output", "1200"), ("fan1_input", "1210"), ("temp1_label", "TC0P\n CPU"), ("temp1_input", "50000")] {
            fs::write(smc.join(k), v).unwrap();
        }
        if core {
            let ct = root.join("class/hwmon/hwmon3");
            fs::create_dir_all(&ct).unwrap();
            fs::write(ct.join("name"), "coretemp\n").unwrap();
            fs::write(ct.join("temp1_input"), "50000").unwrap();
            fs::write(ct.join("temp2_input"), "52000").unwrap();
        }
        root
    }

    fn set_temp(root: &Path, milli: &str) {
        fs::write(root.join("class/hwmon/hwmon3/temp1_input"), milli).unwrap();
    }
    fn out(root: &Path) -> String {
        fs::read_to_string(root.join("class/hwmon/hwmon2/fan1_output")).unwrap()
    }
    fn manual(root: &Path) -> String {
        fs::read_to_string(root.join("class/hwmon/hwmon2/fan1_manual")).unwrap()
    }

    #[test]
    fn curve_is_linear_and_bounded() {
        let c = Curve::default();
        assert_eq!(c.speed(40, 1200, 6200), 1200);
        assert_eq!(c.speed(63, 1200, 6200), 1200);
        assert_eq!(c.speed(86, 1200, 6200), 6200);
        assert_eq!(c.speed(100, 1200, 6200), 6200);
        assert_eq!(c.speed(74, 1200, 6200), 1200 + 5000 * 11 / 23);
        assert_eq!(c.speed(80, 3000, 3000), 3000);
        assert!(c.valid());
        assert!(!Curve { low: 70, high: 66, max: 86, hysteresis: 2 }.valid());
        assert!(!Curve { low: 63, high: 66, max: 140, hysteresis: 2 }.valid());
    }

    #[test]
    fn discovery_needs_applesmc_fan_and_a_temperature() {
        let root = tree(true);
        let c = Controller::discover(&root, Curve::default()).expect("found");
        assert_eq!(c.fans().len(), 1);
        assert_eq!((c.fans()[0].min, c.fans()[0].max), (1200, 6200));
        let _ = fs::remove_dir_all(&root);
        // No applesmc at all.
        let empty = std::env::temp_dir().join(format!("iyakki-fan-empty-{}", std::process::id()));
        fs::create_dir_all(empty.join("class/hwmon")).unwrap();
        assert!(Controller::discover(&empty, Curve::default()).is_none());
        let _ = fs::remove_dir_all(&empty);
        // Bad curve.
        let root = tree(true);
        assert!(Controller::discover(&root, Curve { low: 90, high: 91, max: 92, hysteresis: 50 }).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn falls_back_to_the_applesmc_cpu_input_without_coretemp() {
        let root = tree(false);
        let mut c = Controller::discover(&root, Curve::default()).expect("found");
        assert_eq!(c.hottest(), Ok(50));
        assert!(matches!(c.step(), Step::Set { temp: 50, .. }));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn steps_follow_the_curve_and_take_the_hottest_core() {
        let root = tree(true);
        let mut c = Controller::discover(&root, Curve::default()).unwrap();
        // Cool: minimum, manual on.
        assert_eq!(c.step(), Step::Set { temp: 52, rpm: vec![1200] });
        assert_eq!((out(&root).as_str(), manual(&root).as_str()), ("1200", "1"));
        // Hot: raised at once.
        set_temp(&root, "86000");
        assert_eq!(c.step(), Step::Set { temp: 86, rpm: vec![6200] });
        assert_eq!(out(&root), "6200");
        // One degree cooler: held by the hysteresis.
        set_temp(&root, "85000");
        assert_eq!(c.step(), Step::Held { temp: 85 });
        assert_eq!(out(&root), "6200");
        // Two degrees cooler: lowered.
        set_temp(&root, "84000");
        assert!(matches!(c.step(), Step::Set { temp: 84, .. }));
        assert_ne!(out(&root), "6200");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unreadable_or_absurd_temperature_gives_the_fans_back() {
        let root = tree(true);
        let mut c = Controller::discover(&root, Curve::default()).unwrap();
        c.step();
        assert_eq!(manual(&root), "1");
        set_temp(&root, "0");
        assert!(matches!(c.step(), Step::Released { .. }));
        assert_eq!(manual(&root), "0");
        set_temp(&root, "300000");
        assert!(matches!(c.step(), Step::Released { .. }));
        set_temp(&root, "garbage");
        assert!(matches!(c.step(), Step::Released { .. }));
        // It recovers when the reading is sane again.
        set_temp(&root, "70000");
        assert!(matches!(c.step(), Step::Set { .. }));
        assert_eq!(manual(&root), "1");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dropping_the_controller_releases_the_fans() {
        let root = tree(true);
        {
            let mut c = Controller::discover(&root, Curve::default()).unwrap();
            c.step();
            assert_eq!(manual(&root), "1");
            assert!(c.status().contains("controlled by IYAKKI"));
        }
        assert_eq!(manual(&root), "0");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn speed_never_leaves_the_fans_own_limits() {
        let root = tree(true);
        let mut c = Controller::discover(&root, Curve { low: 30, high: 40, max: 50, hysteresis: 0 }).unwrap();
        for t in [20, 35, 45, 60, 110] {
            set_temp(&root, &format!("{}000", t));
            c.step();
            let v: u32 = out(&root).parse().unwrap();
            assert!((1200..=6200).contains(&v), "{t} gave {v}");
        }
        let _ = fs::remove_dir_all(&root);
    }
}
