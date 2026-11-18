//! `LocalHardware`: the core's [`Hardware`] on this machine, over `iyakki-devctl`. The roots and the control seam are parameters, so a
//! test points it at a made-up `/sys` and `/proc` with its own control.

use std::sync::Arc;

use iyakki_core_api::hardware::{btstate, mac, Card, Control, Hardware, Link, Rfkill, Roots, Screen, Volume};

pub struct LocalHardware {
    roots: Roots,
    ctl: Arc<dyn Control>,
}

impl LocalHardware {
    /// This machine.
    pub fn system() -> LocalHardware {
        LocalHardware { roots: Roots::real(), ctl: Arc::new(iyakki_devctl::System) }
    }
    /// Any roots and control (tests, and the shell's `DeviceEnv`).
    pub fn new(roots: Roots, ctl: Arc<dyn Control>) -> LocalHardware {
        LocalHardware { roots, ctl }
    }
}

impl Hardware for LocalHardware {
    fn links(&self) -> Vec<Link> {
        iyakki_devctl::links(&self.roots)
    }
    fn ipv4(&self) -> Vec<(String, String)> {
        self.ctl.ipv4()
    }
    fn rfkill(&self) -> Vec<Rfkill> {
        iyakki_devctl::rfkill_list(&self.roots)
    }
    fn set_rfkill(&self, id: &str, blocked: bool) -> Result<(), String> {
        iyakki_devctl::rfkill_set(&self.roots, id, blocked)
    }
    fn broadcom_wifi_without_driver(&self) -> bool {
        iyakki_devctl::broadcom_wifi_without_driver(&self.roots)
    }
    fn screen(&self) -> Screen {
        iyakki_devctl::screen(&self.roots)
    }
    fn set_brightness(&self, name: &str, percent: u32) -> Result<(), String> {
        iyakki_devctl::set_brightness(&self.roots, name, percent)
    }
    fn cards(&self) -> Vec<(Card, Option<Volume>)> {
        iyakki_devctl::cards(&self.roots).into_iter().map(|c| { let v = self.ctl.volume(c.index); (c, v) }).collect()
    }
    fn mac_infos(&self, cfg: &mac::MacConfig) -> Vec<mac::MacInfo> {
        mac::infos(&*self.ctl, &self.roots, cfg)
    }
    fn bluetooth(&self) -> btstate::BtState {
        btstate::read(&*self.ctl, &self.roots)
    }
    fn cs8409_line(&self) -> Option<String> {
        iyakki_drivers::cs8409::missing_line(&iyakki_drivers::cs8409::Roots { proc_: self.roots.proc_.clone(), sys: self.roots.sys.clone(), lib_modules: "/lib/modules".into() })
    }
    fn set_hostname(&self, name: &str, etc_hostname: &std::path::Path) -> Result<(), String> {
        iyakki_devctl::apply_hostname(name, etc_hostname)
    }
    fn mac_summary(&self) -> Option<String> {
        mac::summary_line(&self.roots)
    }
    fn hardware_report(&self) -> Vec<String> {
        hardware_report_at(&self.roots.sys, &self.roots.proc_)
    }
}

fn human_bytes(b: u64) -> String {
    if b < 1024 {
        format!("{b} B")
    } else if b < 1 << 20 {
        format!("{:.1} KB", b as f64 / 1024.0)
    } else if b < 1 << 30 {
        format!("{:.1} MB", b as f64 / (1u64 << 20) as f64)
    } else {
        format!("{:.2} GB", b as f64 / (1u64 << 30) as f64)
    }
}

/// The machine report, the PCI devices with their drivers and the loaded drivers, read from `sys` and `proc`.
pub fn hardware_report_at(sys: &std::path::Path, proc_: &std::path::Path) -> Vec<String> {
    let report = iyakki_hwinfo::collect(&iyakki_hwinfo::Roots { sys: sys.to_path_buf(), proc: proc_.to_path_buf() });
    let mut out: Vec<String> = report.to_text().lines().map(str::to_string).collect();
    let rd = |p: std::path::PathBuf| std::fs::read_to_string(p).unwrap_or_default().trim().to_string();
    let mut devices = Vec::new();
    if let Ok(dir) = std::fs::read_dir(sys.join("bus/pci/devices")) {
        for e in dir.flatten() {
            let (vendor, device, class) = (rd(e.path().join("vendor")), rd(e.path().join("device")), rd(e.path().join("class")));
            let driver = std::fs::read_link(e.path().join("driver")).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "(no driver)".into());
            devices.push(format!("  {}  {} {}  class {}  {}", e.file_name().to_string_lossy(), vendor.trim_start_matches("0x"), device.trim_start_matches("0x"), class.trim_start_matches("0x"), driver));
        }
    }
    devices.sort();
    out.push("PCI DEVICES".into());
    if devices.is_empty() {
        out.push("  none (this machine has no PCI bus, or it is not readable)".into());
    }
    out.extend(devices);
    out.push(String::new());
    let mut modules: Vec<(String, u64, String)> = std::fs::read_to_string(proc_.join("modules"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (name, size) = (f.next()?, f.next()?.parse::<u64>().ok()?);
            let _count = f.next()?;
            let by = f.next().unwrap_or("-").trim_end_matches(',').to_string();
            Some((name.to_string(), size, if by == "-" { String::new() } else { by }))
        })
        .collect();
    modules.sort();
    out.push("LOADED DRIVERS".into());
    if modules.is_empty() {
        out.push("  none loaded as modules (the drivers are built into the kernel)".into());
    }
    for (name, size, by) in modules {
        out.push(format!("  {name:<28} {:>9}  {by}", human_bytes(size)));
    }
    out
}

#[cfg(test)]
mod report_tests {
    use super::*;

    #[test]
    fn the_report_lists_pci_devices_with_drivers_and_modules() {
        let d = std::env::temp_dir().join(format!("iyakki-hwrep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let dev = d.join("sys/bus/pci/devices/0000:00:02.0");
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(dev.join("vendor"), "0x8086\n").unwrap();
        std::fs::write(dev.join("device"), "0x1234\n").unwrap();
        std::fs::write(dev.join("class"), "0x030000\n").unwrap();
        std::fs::create_dir_all(d.join("sys/bus/pci/drivers/i915")).unwrap();
        std::os::unix::fs::symlink(d.join("sys/bus/pci/drivers/i915"), dev.join("driver")).unwrap();
        std::fs::create_dir_all(d.join("proc")).unwrap();
        std::fs::write(d.join("proc/modules"), "snd_hda 49152 2 snd_core,snd_pcm, Live 0x0\nvfat 20480 1 - Live 0x0\n").unwrap();
        let lines = hardware_report_at(&d.join("sys"), &d.join("proc"));
        assert!(lines.iter().any(|l| l.contains("0000:00:02.0") && l.contains("8086 1234") && l.ends_with("i915")), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("  snd_hda") && l.contains("snd_core,snd_pcm")), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("  vfat")));
        let _ = std::fs::remove_dir_all(&d);
    }
    /// F-SYS-02 against this computer's real `/sys` and `/proc`, judged by the real tools `lspci -k` and `lsmod`: every PCI device the
    /// tool lists is in the report with the driver the kernel says is in use, and every loaded module the tool lists is in the list of
    /// loaded drivers. Skipped, and said so, where the tools or a PCI bus are missing.
    #[test]
    fn the_report_of_the_real_machine_agrees_with_lspci_and_lsmod() {
        let run = |c: &str, a: &[&str]| std::process::Command::new(c).args(a).output().ok().filter(|o| !o.stdout.is_empty()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
        let (Some(pci), Some(mods)) = (run("lspci", &["-D", "-k"]), run("lsmod", &[])) else {
            eprintln!("skipped: lspci or lsmod is missing or shows nothing");
            return;
        };
        let report = hardware_report_at(std::path::Path::new("/sys"), std::path::Path::new("/proc"));
        let mut slots = 0;
        let mut slot = String::new();
        for l in pci.lines() {
            if let Some(first) = l.split_whitespace().next().filter(|w| !l.starts_with('\t') && w.matches(':').count() == 2) {
                slot = first.to_string();
                slots += 1;
                assert!(report.iter().any(|r| r.trim_start().starts_with(&slot)), "device {slot} is missing from the report");
            } else if let Some(d) = l.trim().strip_prefix("Kernel driver in use: ") {
                let row = report.iter().find(|r| r.trim_start().starts_with(&slot)).expect("device row");
                assert!(row.trim_end().ends_with(d.trim()), "lspci says {slot} uses {d}, the report says: {row}");
            }
        }
        assert!(slots > 0, "lspci listed no device");
        for l in mods.lines().skip(1) {
            let Some(name) = l.split_whitespace().next() else { continue };
            assert!(report.iter().any(|r| r.starts_with(&format!("  {name} "))), "module {name} is loaded but not in the report");
        }
    }
}
