//! The internal keyboard, trackpad and Touch Bar of a T2 MacBook: what loads them and one option.
//!
//! The devices sit behind apple-bce's virtual USB host, ten levels below `/sys/devices`, and appear seconds after boot. They
//! load by their own aliases (`hid_apple` for `hid:...v000005ACp0000027E`, `bcm5974` for the trackpad's boot-mouse interface
//! `usb:v05ACp027E...ic03isc01ip02`, `hid_appletb_bl` for the Touch Bar backlight) once the coldplug scan sees them and the
//! hotplug listener catches later ones.
//!
//! `bcm5974` prints `bad trackpad package, length: 8` at kernel debug level for every short report the T2 pad sends besides its
//! multi-touch frames (100 lines for a few seconds of use, measured on a MacBookPro16,2). The pointer, click and scroll work
//! anyway; the lines only hide real messages. `options bcm5974 debug=0` silences them.

use std::path::Path;

use crate::macboard::is_t2;

pub const CONF_NAME: &str = "iyakki-t2-input.conf";

/// The modprobe options for this machine: empty unless it is a T2 Mac.
pub fn conf_text(dmi_product: &str) -> Option<String> {
    is_t2(dmi_product).then(|| "# written at every boot by IYAKKI on a T2 Mac: bcm5974 logs every short report of the T2 trackpad\noptions bcm5974 debug=0\n".to_string())
}

/// Write the file under `modprobe_d` (`/etc/modprobe.d`, in RAM on the image) on a T2 Mac. True when written.
pub fn apply(dmi_product_file: &Path, modprobe_d: &Path) -> bool {
    let product = std::fs::read_to_string(dmi_product_file).unwrap_or_default();
    let Some(t) = conf_text(product.trim()) else { return false };
    let _ = std::fs::create_dir_all(modprobe_d);
    std::fs::write(modprobe_d.join(CONF_NAME), t).is_ok()
}

/// The Touch Bar (`05ac:8302`) behind apple-bce's USB host is often not ready when the kernel sets its configuration at boot
/// (`usb 5-6: can't set config #1, error -110`), so it stays unconfigured and hid-appletb-kbd finds nothing to bind. Measured on a
/// MacBookPro16,2: writing 2 and then 1 to its `bConfigurationValue` later configured it at once and `Apple Inc. Touch Bar Display`
/// (hid-appletb-kbd, Esc / F-keys or media keys drawn by the T2) appeared. Returns the sysfs folders of such devices under
/// `usb_devices` (`/sys/bus/usb/devices`) that have no configuration.
pub fn touchbar_unconfigured(usb_devices: &Path) -> Vec<std::path::PathBuf> {
    let read = |d: &Path, f: &str| std::fs::read_to_string(d.join(f)).unwrap_or_default().trim().to_string();
    let mut v: Vec<std::path::PathBuf> = std::fs::read_dir(usb_devices)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|d| read(d, "idVendor") == "05ac" && read(d, "idProduct") == "8302" && read(d, "bConfigurationValue").is_empty())
        .collect();
    v.sort();
    v
}

/// Try to configure every unconfigured Touch Bar: `write(path, value)` is called with the device's `bConfigurationValue` file and
/// "2", then "1" (the sequence that worked); `pause` runs between attempts. At most `attempts` rounds; stops as soon as none is left.
/// Returns how many devices were configured. A write that fails (the device times out) is only an attempt.
pub fn touchbar_configure(usb_devices: &Path, attempts: u32, write: &mut dyn FnMut(&Path, &str) -> bool, pause: &mut dyn FnMut()) -> usize {
    let first = touchbar_unconfigured(usb_devices).len();
    for i in 0..attempts {
        let todo = touchbar_unconfigured(usb_devices);
        if todo.is_empty() {
            break;
        }
        if i > 0 {
            pause();
        }
        for d in todo {
            let f = d.join("bConfigurationValue");
            let _ = write(&f, "2");
            let _ = write(&f, "1");
        }
    }
    first - touchbar_unconfigured(usb_devices).len().min(first)
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_unconfigured_touch_bar_is_given_its_configuration_and_nothing_else_is_touched() {
        use super::*;
        let d = std::env::temp_dir().join(format!("tbar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for (n, v, p, c) in [("5-6", "05ac", "8302", ""), ("5-7", "05ac", "8102", ""), ("5-8", "05ac", "8302", "1"), ("3-1", "0bda", "8153", "")] {
            std::fs::create_dir_all(d.join(n)).unwrap();
            std::fs::write(d.join(n).join("idVendor"), format!("{v}\n")).unwrap();
            std::fs::write(d.join(n).join("idProduct"), format!("{p}\n")).unwrap();
            std::fs::write(d.join(n).join("bConfigurationValue"), c).unwrap();
        }
        assert_eq!(touchbar_unconfigured(&d), vec![d.join("5-6")]);
        // the device times out on the first round and takes the sequence on the second
        let mut log = Vec::new();
        let mut round = 0;
        let mut pauses = 0;
        let n = touchbar_configure(&d, 3, &mut |f, v| {
            log.push(format!("{} {v}", f.parent().unwrap().file_name().unwrap().to_string_lossy()));
            if v == "2" {
                round += 1;
            }
            if round >= 2 && v == "1" {
                std::fs::write(f, "1\n").unwrap();
                return true;
            }
            false
        }, &mut || pauses += 1);
        assert_eq!(n, 1);
        assert_eq!(log, ["5-6 2", "5-6 1", "5-6 2", "5-6 1"]);
        assert_eq!(pauses, 1, "one pause between the two rounds, none after success");
        // nothing to do: no write at all
        assert_eq!(touchbar_configure(&d, 3, &mut |_, _| panic!("no write expected"), &mut || {}), 0);
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod tests_conf {
    use super::*;

    #[test]
    fn only_a_t2_mac_gets_the_trackpad_option() {
        let d = std::env::temp_dir().join(format!("t2in-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("product"), "MacBookPro16,2\n").unwrap();
        assert!(apply(&d.join("product"), &d.join("mp")));
        assert!(std::fs::read_to_string(d.join("mp").join(CONF_NAME)).unwrap().contains("options bcm5974 debug=0"));
        std::fs::write(d.join("product"), "MacBookPro14,1\n").unwrap();
        assert!(!apply(&d.join("product"), &d.join("mp2")), "no T2 chip, nothing written");
        assert!(!d.join("mp2").exists());
        assert!(conf_text("Latitude 7490").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
}
