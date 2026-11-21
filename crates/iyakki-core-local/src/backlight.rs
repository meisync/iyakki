//! Backlights and keyboard lights: the one reader and writer of `<sys>/class/backlight/*` and `<sys>/class/leds/*` (the
//! `brightness` and `max_brightness` files). The Fn keys, the Touch Bar sliders, Settings and the screen saver ask here; the
//! step curves and the on-screen display stay with them. Every function takes the `/sys` folder, so the tests use made-up trees.

use std::path::{Path, PathBuf};

/// A backlight or keyboard LED: `<dir>/brightness` and `<dir>/max_brightness`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Light {
    pub name: String,
    pub dir: PathBuf,
    pub value: u32,
    pub max: u32,
}

impl Light {
    pub fn fraction(&self) -> f32 {
        if self.max == 0 { 0.0 } else { (self.value as f32 / self.max as f32).clamp(0.0, 1.0) }
    }
    pub fn percent(&self) -> u32 {
        (self.fraction() * 100.0).round() as u32
    }
}

/// A number in a small kernel file (trimmed), None when the file is missing or not a number.
pub fn read_u32(p: &Path) -> Option<u32> {
    std::fs::read_to_string(p).ok()?.trim().parse().ok()
}

fn lights_in(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<Light> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<Light> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !keep(&name) {
                return None;
            }
            let d = e.path();
            let max = read_u32(&d.join("max_brightness"))?;
            let value = read_u32(&d.join("brightness"))?;
            (max > 0).then_some(Light { name, dir: d, value, max })
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// Lower is better: the panel's own driver before the generic ACPI one.
fn panel_rank(name: &str) -> u32 {
    const ORDER: [&str; 7] = ["intel_backlight", "amdgpu_bl", "radeon_bl", "nvidia", "nv_backlight", "apple-panel-bl", "gmux_backlight"];
    ORDER.iter().position(|p| name.starts_with(p)).map(|i| i as u32).unwrap_or(if name.starts_with("acpi_video") { 100 } else { 50 })
}

/// Every backlight of the machine (the Touch Bar's included), best panel first.
pub fn all_backlights(sys: &Path) -> Vec<Light> {
    let mut v = lights_in(&sys.join("class/backlight"), |_| true);
    v.sort_by_key(|l| (panel_rank(&l.name), l.name.clone()));
    v
}

/// The backlights that dim the screen: the Touch Bar has one of its own (`appletb_backlight`) that is not the display.
pub fn panel_backlights(sys: &Path) -> Vec<Light> {
    all_backlights(sys).into_iter().filter(|l| !l.name.starts_with("appletb")).collect()
}

/// Keyboard backlights (`smc::kbd_backlight` on Macs, `tpacpi::kbd_backlight`, `asus::kbd_backlight`, ...).
pub fn kbd_backlights(sys: &Path) -> Vec<Light> {
    lights_in(&sys.join("class/leds"), |n| n.contains("kbd_backlight") || n.contains("kbd_illum"))
}

/// Write a brightness value into `<dir>/brightness`; true when the write was accepted.
pub fn write_value(dir: &Path, v: u32) -> bool {
    std::fs::write(dir.join("brightness"), v.to_string()).is_ok()
}

/// Write the brightness of a light; the error is a sentence for the screen (`what` is "brightness" or "keyboard backlight").
pub fn write_light(l: &Light, v: u32, what: &str) -> Result<(), String> {
    std::fs::write(l.dir.join("brightness"), v.to_string()).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!("Not allowed to change the {what} (needs root)")
        } else {
            format!("Could not change the {what}: {e}")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(tag: &str, lights: &[(&str, &str, u32, u32)]) -> PathBuf {
        let sys = std::env::temp_dir().join(format!("iyakki-backlight-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&sys);
        for (class, name, value, max) in lights {
            let d = sys.join("class").join(class).join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("brightness"), format!("{value}\n")).unwrap();
            std::fs::write(d.join("max_brightness"), format!("{max}\n")).unwrap();
        }
        sys
    }

    #[test]
    fn the_panels_own_driver_comes_before_the_acpi_one_and_the_touch_bar_is_not_a_panel() {
        let sys = tree("rank", &[("backlight", "acpi_video0", 5, 10), ("backlight", "intel_backlight", 50, 100), ("backlight", "appletb_backlight", 1, 2)]);
        let names: Vec<_> = all_backlights(&sys).into_iter().map(|l| l.name).collect();
        assert_eq!(names, ["intel_backlight", "appletb_backlight", "acpi_video0"]);
        let panel: Vec<_> = panel_backlights(&sys).into_iter().map(|l| l.name).collect();
        assert_eq!(panel, ["intel_backlight", "acpi_video0"]);
        assert_eq!(panel_backlights(&sys)[0].percent(), 50);
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn keyboard_lights_are_found_by_name_and_a_light_without_a_range_is_skipped() {
        let sys = tree("kbd", &[("leds", "smc::kbd_backlight", 0, 255), ("leds", "input3::capslock", 0, 1), ("leds", "dead::kbd_backlight", 0, 0)]);
        let k = kbd_backlights(&sys);
        assert_eq!(k.len(), 1);
        assert_eq!(k[0].name, "smc::kbd_backlight");
        assert!(kbd_backlights(&sys.join("nowhere")).is_empty());
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn a_value_is_written_and_a_refusal_is_a_sentence() {
        let sys = tree("write", &[("backlight", "intel_backlight", 50, 100)]);
        let l = panel_backlights(&sys).remove(0);
        write_light(&l, 70, "brightness").unwrap();
        assert_eq!(read_u32(&l.dir.join("brightness")), Some(70));
        assert!(write_value(&l.dir, 20));
        assert_eq!(read_u32(&l.dir.join("brightness")), Some(20));
        let gone = Light { name: "x".into(), dir: sys.join("missing"), value: 0, max: 1 };
        let e = write_light(&gone, 1, "keyboard backlight").unwrap_err();
        assert!(e.starts_with("Could not change the keyboard backlight"), "{e}");
        assert!(!write_value(&gone.dir, 1));
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn fractions_are_clamped_and_an_empty_range_is_zero() {
        let l = Light { name: "a".into(), dir: PathBuf::new(), value: 300, max: 100 };
        assert_eq!(l.fraction(), 1.0);
        let z = Light { max: 0, ..l };
        assert_eq!(z.percent(), 0);
    }
}
