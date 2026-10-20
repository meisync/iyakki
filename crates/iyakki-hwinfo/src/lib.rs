//! A plain-text hardware report built only from `/proc` and `/sys` (the kernel's stable interface), so it
//! works on any machine IYAKKI boots on and can be tested against fake directory trees.
//!
//! It exists for the boot hardware check: "did the keyboard, mouse and touch screen show up? what
//! resolution and monitor did we get? is it UEFI, is Secure Boot on?".

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Roots {
    pub sys: PathBuf,
    pub proc: PathBuf,
}

impl Roots {
    pub fn real() -> Roots {
        Roots { sys: "/sys".into(), proc: "/proc".into() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub sections: Vec<Section>,
    /// Things worth a user's attention ("no keyboard found").
    pub warnings: Vec<String>,
}

impl Report {
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        if !self.warnings.is_empty() {
            s.push_str("ATTENTION\n");
            for w in &self.warnings {
                s.push_str(&format!("  ! {w}\n"));
            }
            s.push('\n');
        }
        for sec in &self.sections {
            s.push_str(&sec.title.to_uppercase());
            s.push('\n');
            for l in &sec.lines {
                s.push_str(&format!("  {l}\n"));
            }
            s.push('\n');
        }
        s
    }

    pub fn section(&self, title: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.title.eq_ignore_ascii_case(title))
    }
}

fn read(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn dir_names(p: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

fn link_name(p: &Path) -> Option<String> {
    std::fs::read_link(p).ok().and_then(|t| t.file_name().map(|n| n.to_string_lossy().into_owned()))
}

// ------------------------------------------------------------------ input devices

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Keyboard,
    Mouse,
    Touchscreen,
    Touchpad,
    Tablet,
    Other,
}

impl InputKind {
    pub fn label(self) -> &'static str {
        match self {
            InputKind::Keyboard => "keyboard",
            InputKind::Mouse => "mouse",
            InputKind::Touchscreen => "touchscreen",
            InputKind::Touchpad => "touchpad",
            InputKind::Tablet => "pointer (absolute)",
            InputKind::Other => "other",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputDevice {
    pub name: String,
    pub kind: InputKind,
    pub multitouch: bool,
    pub event: Option<String>,
}

/// Bit `n` of a kernel bitmap line ("402000000 3803078f800d001 …"): 64-bit words, most significant first.
fn bit(words: &[u64], n: usize) -> bool {
    let (w, b) = (n / 64, n % 64);
    words.len() > w && words[words.len() - 1 - w] & (1u64 << b) != 0
}

fn parse_bitmap(s: &str) -> Vec<u64> {
    s.split_whitespace().filter_map(|w| u64::from_str_radix(w, 16).ok()).collect()
}

pub fn parse_input_devices(text: &str) -> Vec<InputDevice> {
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        let (mut name, mut handlers) = (String::new(), String::new());
        let (mut key, mut rel, mut abs, mut prop) = (vec![], vec![], vec![], vec![]);
        for l in block.lines() {
            if let Some(r) = l.strip_prefix("N: Name=") {
                name = r.trim().trim_matches('"').to_string();
            } else if let Some(r) = l.strip_prefix("H: Handlers=") {
                handlers = r.to_string();
            } else if let Some(r) = l.strip_prefix("B: KEY=") {
                key = parse_bitmap(r);
            } else if let Some(r) = l.strip_prefix("B: REL=") {
                rel = parse_bitmap(r);
            } else if let Some(r) = l.strip_prefix("B: ABS=") {
                abs = parse_bitmap(r);
            } else if let Some(r) = l.strip_prefix("B: PROP=") {
                prop = parse_bitmap(r);
            }
        }
        if name.is_empty() {
            continue;
        }
        let event = handlers.split_whitespace().find(|h| h.starts_with("event")).map(String::from);
        let (is_kbd, rel_xy, btn_left) = (bit(&key, 30) && bit(&key, 28), bit(&rel, 0) && bit(&rel, 1), bit(&key, 0x110));
        let abs_xy = (bit(&abs, 0) && bit(&abs, 1)) || (bit(&abs, 0x35) && bit(&abs, 0x36));
        let direct = bit(&prop, 1); // INPUT_PROP_DIRECT: touches the screen itself
        let finger_tool = bit(&key, 0x145);
        let multitouch = bit(&abs, 0x35);
        let kind = if is_kbd {
            InputKind::Keyboard
        } else if rel_xy && btn_left {
            InputKind::Mouse
        } else if abs_xy && direct {
            InputKind::Touchscreen
        } else if abs_xy && finger_tool {
            InputKind::Touchpad
        } else if abs_xy {
            InputKind::Tablet
        } else {
            InputKind::Other
        };
        out.push(InputDevice { name, kind, multitouch, event });
    }
    out
}

// ------------------------------------------------------------------ monitors (EDID)

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub name: String,
    pub preferred: Option<(u32, u32)>,
    /// Diagonal in tenths of an inch.
    pub diagonal_tenth_in: Option<u32>,
}

pub fn parse_edid(b: &[u8]) -> Option<Monitor> {
    if b.len() < 128 || b[..8] != [0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0] {
        return None;
    }
    let m = u16::from_be_bytes([b[8], b[9]]);
    let vendor: String = [(m >> 10) & 31, (m >> 5) & 31, m & 31].iter().map(|c| (b'A' + (*c as u8).saturating_sub(1)) as char).collect();
    let mut name = None;
    let mut preferred = None;
    for (i, off) in [54usize, 72, 90, 108].iter().enumerate() {
        let d = &b[*off..*off + 18];
        if d[0] == 0 && d[1] == 0 && d[3] == 0xFC {
            let t: String = d[5..18].iter().take_while(|c| **c != 0x0A && **c != 0).map(|c| *c as char).collect();
            name = Some(t.trim().to_string());
        } else if i == 0 && (d[0] != 0 || d[1] != 0) {
            let (h, v) = (d[2] as u32 | ((d[4] as u32 & 0xF0) << 4), d[5] as u32 | ((d[7] as u32 & 0xF0) << 4));
            if h > 0 && v > 0 {
                preferred = Some((h, v));
            }
        }
    }
    let (wcm, hcm) = (b[21] as f64, b[22] as f64);
    let diagonal = if wcm > 0.0 && hcm > 0.0 { Some((((wcm * wcm + hcm * hcm).sqrt() / 2.54) * 10.0).round() as u32) } else { None };
    Some(Monitor { name: name.unwrap_or_else(|| format!("{vendor} monitor")), preferred, diagonal_tenth_in: diagonal })
}

// ------------------------------------------------------------------ the report

fn firmware(r: &Roots, sec: &mut Section) -> bool {
    let efi = r.sys.join("firmware/efi").exists();
    sec.lines.push(format!("firmware: {}", if efi { "UEFI" } else { "legacy BIOS (or no firmware information)" }));
    if efi {
        let var = std::fs::read(r.sys.join("firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c"));
        let sb = match var {
            Ok(v) if v.len() >= 5 => if v[4] == 1 { "ON" } else { "off" },
            _ => "unknown",
        };
        sec.lines.push(format!("Secure Boot: {sb}"));
        let lock = read(&r.sys.join("kernel/security/lockdown"));
        let (mode, note) = secure_boot_summary(sb == "ON", lock.as_deref());
        sec.lines.push(format!("Kernel lockdown: {mode}"));
        if let Some(n) = note {
            sec.lines.push(n.to_string());
        }
    }
    efi
}

/// The lockdown mode from `/sys/kernel/security/lockdown` (`none [integrity] confidentiality`: the bracketed one is active), and
/// a plain sentence when Secure Boot is on but the kernel is not locked down (a kernel without the Secure Boot patch) or when it
/// is on and the person may need the one-time key enrolment (docs/SECURE-BOOT.md).
pub fn secure_boot_summary(secure_boot_on: bool, lockdown_file: Option<&str>) -> (String, Option<&'static str>) {
    let mode = lockdown_file
        .and_then(|t| t.split_whitespace().find(|w| w.starts_with('[') && w.ends_with(']')).map(|w| w.trim_matches(|c| c == '[' || c == ']').to_string()))
        .unwrap_or_else(|| "unknown (this kernel has no lockdown)".into());
    let note = match (secure_boot_on, mode.as_str()) {
        (true, "none") => Some("Secure Boot is on but the kernel is not locked down: unsigned drivers could load. Use an image built with the Secure Boot patch (docs/SECURE-BOOT.md)."),
        (true, _) => Some("Secure Boot is on: only drivers signed with the enrolled IYAKKI key load; driver packs with unsigned modules are refused (docs/SECURE-BOOT.md)."),
        (false, "integrity" | "confidentiality") => Some("The kernel is locked down although Secure Boot is off (lockdown= on the command line)."),
        (false, _) => None,
    };
    (mode, note)
}

pub fn collect(r: &Roots) -> Report {
    let mut rep = Report::default();
    // system
    let mut sys = Section { title: "System".into(), lines: vec![] };
    let dmi = |f: &str| read(&r.sys.join("class/dmi/id").join(f));
    let machine = [dmi("sys_vendor"), dmi("product_name"), dmi("product_version")].into_iter().flatten().collect::<Vec<_>>().join(" ");
    sys.lines.push(format!("machine: {}", if machine.is_empty() { "unknown (no DMI: virtual machine or ARM board)".to_string() } else { machine }));
    if let Some(b) = dmi("bios_version") {
        sys.lines.push(format!("firmware version: {b}{}", dmi("bios_date").map(|d| format!(" ({d})")).unwrap_or_default()));
    }
    firmware(r, &mut sys);
    sys.lines.push(format!("architecture: {}", std::env::consts::ARCH));
    if let Some(v) = read(&r.proc.join("version")) {
        sys.lines.push(format!("kernel: {}", v.split_whitespace().take(3).collect::<Vec<_>>().join(" ")));
    }
    if let Some(c) = read(&r.proc.join("cmdline")) {
        sys.lines.push(format!("boot options: {c}"));
    }
    rep.sections.push(sys);
    // cpu and memory
    let mut cpu = Section { title: "Processor and memory".into(), lines: vec![] };
    let info = read(&r.proc.join("cpuinfo")).unwrap_or_default();
    let model = info.lines().find_map(|l| l.strip_prefix("model name").or_else(|| l.strip_prefix("Hardware")).and_then(|x| x.split_once(':')).map(|(_, v)| v.trim().to_string()));
    let cores = info.lines().filter(|l| l.starts_with("processor")).count();
    cpu.lines.push(format!("cpu: {} ({} logical cores)", model.unwrap_or_else(|| "unknown".into()), cores));
    if let Some(kb) = read(&r.proc.join("meminfo")).and_then(|m| m.lines().find(|l| l.starts_with("MemTotal:")).and_then(|l| l.split_whitespace().nth(1).and_then(|n| n.parse::<u64>().ok()))) {
        cpu.lines.push(format!("memory: {:.1} GB", kb as f64 / 1_048_576.0));
    }
    rep.sections.push(cpu);
    // display
    let mut disp = Section { title: "Display".into(), lines: vec![] };
    let mut have_display = false;
    for fb in dir_names(&r.sys.join("class/graphics")).into_iter().filter(|n| n.strip_prefix("fb").is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))) {
        let d = r.sys.join("class/graphics").join(&fb);
        let size = read(&d.join("virtual_size")).map(|s| s.replace(',', "x")).unwrap_or_else(|| "?".into());
        disp.lines.push(format!("{fb}: {} {size}, {} bits/pixel", read(&d.join("name")).unwrap_or_else(|| "framebuffer".into()), read(&d.join("bits_per_pixel")).unwrap_or_else(|| "?".into())));
        have_display = true;
    }
    for card in dir_names(&r.sys.join("class/drm")).into_iter().filter(|n| n.starts_with("card") && !n.contains('-')) {
        let dev = r.sys.join("class/drm").join(&card).join("device");
        disp.lines.push(format!("{card}: driver {}, vendor {}", link_name(&dev.join("driver")).unwrap_or_else(|| "none".into()), read(&dev.join("vendor")).unwrap_or_else(|| "?".into())));
    }
    for con in dir_names(&r.sys.join("class/drm")).into_iter().filter(|n| n.starts_with("card") && n.contains('-')) {
        let d = r.sys.join("class/drm").join(&con);
        let status = read(&d.join("status")).unwrap_or_default();
        if status != "connected" {
            continue;
        }
        have_display = true;
        let modes = read(&d.join("modes")).map(|m| m.lines().take(4).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        let mon = std::fs::read(d.join("edid")).ok().and_then(|b| parse_edid(&b));
        let label = con.split_once('-').map_or(con.as_str(), |(_, n)| n);
        match mon {
            Some(m) => disp.lines.push(format!("{label}: {}{}{} · modes {modes}", m.name, m.preferred.map(|(w, h)| format!(" {w}x{h}")).unwrap_or_default(), m.diagonal_tenth_in.map(|d| format!(" {}.{} in", d / 10, d % 10)).unwrap_or_default())),
            None => disp.lines.push(format!("{label}: connected · modes {modes}")),
        }
    }
    if disp.lines.is_empty() {
        disp.lines.push("no framebuffer or display found".into());
    }
    rep.sections.push(disp);
    if !have_display {
        rep.warnings.push("No display was detected: the screen may stay black. Try the \"safe graphics\" boot entry.".into());
    }
    // input
    let mut inp = Section { title: "Input devices".into(), lines: vec![] };
    let devices = read(&r.proc.join("bus/input/devices")).map(|t| parse_input_devices(&t)).unwrap_or_default();
    for d in devices.iter().filter(|d| d.kind != InputKind::Other) {
        inp.lines.push(format!("[{}] {}{}{}", d.kind.label(), d.name, if d.multitouch { " (multi-touch)" } else { "" }, d.event.as_ref().map(|e| format!(" ({e})")).unwrap_or_default()));
    }
    let others = devices.iter().filter(|d| d.kind == InputKind::Other).count();
    if others > 0 {
        inp.lines.push(format!("{others} other input device(s) (buttons, sensors, lids)"));
    }
    if devices.is_empty() {
        inp.lines.push("none found".into());
    }
    rep.sections.push(inp);
    if !devices.iter().any(|d| d.kind == InputKind::Keyboard) {
        rep.warnings.push("No keyboard found. USB keyboards need the xHCI/HID drivers; internal laptop keyboards need i8042 or I2C-HID.".into());
    }
    if !devices.iter().any(|d| matches!(d.kind, InputKind::Mouse | InputKind::Touchscreen | InputKind::Tablet)) {
        rep.warnings.push(if devices.iter().any(|d| d.kind == InputKind::Touchpad) { "Only a touchpad was found: touchpads are not supported yet; plug in a mouse or use a touch screen." } else { "No mouse or touch screen found: the keyboard still works everywhere." }.into());
    }
    // network
    let mut net = Section { title: "Network".into(), lines: vec![] };
    for n in dir_names(&r.sys.join("class/net")).into_iter().filter(|n| n != "lo") {
        let d = r.sys.join("class/net").join(&n);
        // only real Ethernet-style links (ARPHRD_ETHER = 1); tunnels such as sit0 are not hardware
        if read(&d.join("type")).is_some_and(|t| t != "1") {
            continue;
        }
        let kind = if d.join("wireless").exists() || d.join("phy80211").exists() { "WiFi" } else { "Ethernet" };
        net.lines.push(format!("{n}: {kind}, {}, driver {}, {}", read(&d.join("operstate")).unwrap_or_else(|| "?".into()), link_name(&d.join("device/driver")).unwrap_or_else(|| "?".into()), read(&d.join("address")).unwrap_or_else(|| "no address".into())));
    }
    if net.lines.is_empty() {
        net.lines.push("no network interfaces (WiFi and some NICs need the firmware image)".into());
    }
    rep.sections.push(net);
    // storage
    let mut st = Section { title: "Storage".into(), lines: vec![] };
    for d in iyakki_storage::list(&iyakki_storage::Roots { sys_block: r.sys.join("block"), dev: "/dev".into(), mounts: r.proc.join("mounts"), require_block: false }).into_iter().filter(|d| d.parent.is_none()) {
        st.lines.push(format!("{}: {} {}{}", d.name, d.human_size(), if d.model.is_empty() { "disk".to_string() } else { d.model.clone() }, if d.removable { " (removable)" } else { "" }));
    }
    if st.lines.is_empty() {
        st.lines.push("no disks found".into());
    }
    rep.sections.push(st);
    // audio, bluetooth, usb
    let mut misc = Section { title: "Audio, Bluetooth, USB".into(), lines: vec![] };
    for l in read(&r.proc.join("asound/cards")).unwrap_or_default().lines().filter(|l| !l.starts_with(' ') && !l.trim().is_empty()) {
        misc.lines.push(format!("audio: {}", l.trim()));
    }
    if misc.lines.iter().all(|l| !l.starts_with("audio")) {
        misc.lines.push("audio: no sound cards (drivers load on demand once that feature exists)".into());
    }
    for h in dir_names(&r.sys.join("class/bluetooth")) {
        misc.lines.push(format!("bluetooth: {h}"));
    }
    let mut usb = Vec::new();
    for d in dir_names(&r.sys.join("bus/usb/devices")) {
        let p = r.sys.join("bus/usb/devices").join(&d);
        if let Some(prod) = read(&p.join("product")) {
            usb.push(format!("usb: {prod}{} ({}:{})", read(&p.join("manufacturer")).map(|m| format!(" by {m}")).unwrap_or_default(), read(&p.join("idVendor")).unwrap_or_default(), read(&p.join("idProduct")).unwrap_or_default()));
        }
    }
    usb.truncate(16);
    misc.lines.extend(usb);
    if let Some(n) = read(&r.proc.join("modules")).map(|m| m.lines().count()) {
        misc.lines.push(format!("kernel modules loaded: {n}"));
    }
    rep.sections.push(misc);
    rep
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICES: &str = "I: Bus=0011 Vendor=0001 Product=0001 Version=ab41\nN: Name=\"AT Translated Set 2 keyboard\"\nP: Phys=isa0060/serio0/input0\nH: Handlers=sysrq kbd event3 leds\nB: PROP=0\nB: EV=120013\nB: KEY=402000000 3803078f800d001 feffffdfffefffff fffffffffffffffe\n\nI: Bus=0003 Vendor=046d Product=c077 Version=0111\nN: Name=\"Logitech USB Optical Mouse\"\nH: Handlers=mouse0 event4\nB: PROP=0\nB: EV=17\nB: KEY=70000 0 0 0 0\nB: REL=903\n\nI: Bus=0003 Vendor=222a Product=0001 Version=0110\nN: Name=\"ILITEK ILITEK-TP\"\nH: Handlers=mouse1 event5\nB: PROP=2\nB: EV=b\nB: KEY=400 0 0 0 0 0\nB: ABS=260800000000003\n\nI: Bus=0011 Vendor=0002 Product=0007 Version=01b1\nN: Name=\"SynPS/2 Synaptics TouchPad\"\nH: Handlers=mouse2 event6\nB: PROP=5\nB: EV=b\nB: KEY=e520 10000 0 0 0 0\nB: ABS=3\n\nI: Bus=0019 Vendor=0000 Product=0001 Version=0000\nN: Name=\"Lid Switch\"\nH: Handlers=event0\nB: EV=21\nB: SW=1\n";

    #[test]
    fn input_devices_are_classified_from_their_capability_bitmaps() {
        let d = parse_input_devices(DEVICES);
        let kinds: Vec<(&str, InputKind, bool)> = d.iter().map(|x| (x.name.as_str(), x.kind, x.multitouch)).collect();
        assert_eq!(
            kinds,
            vec![
                ("AT Translated Set 2 keyboard", InputKind::Keyboard, false),
                ("Logitech USB Optical Mouse", InputKind::Mouse, false),
                ("ILITEK ILITEK-TP", InputKind::Touchscreen, true),
                ("SynPS/2 Synaptics TouchPad", InputKind::Touchpad, false),
                ("Lid Switch", InputKind::Other, false),
            ]
        );
        assert_eq!(d[0].event.as_deref(), Some("event3"));
        assert!(parse_input_devices("").is_empty());
        assert!(parse_input_devices("garbage\n\nmore garbage").is_empty());
    }

    fn edid(name: &str, w: (u32, u32), cm: (u8, u8)) -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b[..8].copy_from_slice(&[0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0]);
        b[8] = 0x10;
        b[9] = 0xAC; // DEL
        b[21] = cm.0;
        b[22] = cm.1;
        b[54] = 0x01; // preferred timing descriptor (non-zero pixel clock)
        b[56] = (w.0 & 0xFF) as u8;
        b[58] = ((w.0 >> 8) as u8) << 4;
        b[59] = (w.1 & 0xFF) as u8;
        b[61] = ((w.1 >> 8) as u8) << 4;
        b[72..77].copy_from_slice(&[0, 0, 0, 0xFC, 0]);
        let n = name.as_bytes();
        b[77..77 + n.len()].copy_from_slice(n);
        b[77 + n.len()] = 0x0A;
        b
    }

    #[test]
    fn edid_gives_monitor_name_native_size_and_diagonal() {
        let m = parse_edid(&edid("U2723QE", (3840, 2160), (60, 34))).unwrap();
        assert_eq!(m.name, "U2723QE");
        assert_eq!(m.preferred, Some((3840, 2160)));
        assert_eq!(m.diagonal_tenth_in, Some(272), "60x34 cm is about 27.2 inches");
        assert!(parse_edid(&[0u8; 128]).is_none(), "wrong header");
        assert!(parse_edid(&[0u8; 10]).is_none(), "too short");
        let unnamed = parse_edid(&{
            let mut b = edid("x", (1920, 1080), (0, 0));
            b[72..90].fill(0);
            b
        })
        .unwrap();
        assert_eq!(unnamed.name, "DEL monitor");
        assert_eq!(unnamed.diagonal_tenth_in, None);
    }

    fn put(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    fn fake(tag: &str) -> (Roots, PathBuf) {
        let root = std::env::temp_dir().join(format!("iyakki-hwinfo-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        (Roots { sys: root.join("sys"), proc: root.join("proc") }, root)
    }

    #[test]
    fn a_full_report_from_a_fake_machine() {
        let (r, root) = fake("full");
        put(&root, "sys/class/dmi/id/sys_vendor", "Acme\n");
        put(&root, "sys/class/dmi/id/product_name", "Nuc 12\n");
        put(&root, "sys/class/dmi/id/bios_version", "1.2.3\n");
        std::fs::create_dir_all(root.join("sys/firmware/efi/efivars")).unwrap();
        std::fs::write(root.join("sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c"), [6u8, 0, 0, 0, 1]).unwrap();
        put(&root, "proc/version", "Linux version 6.12.112 (build@host) gcc\n");
        put(&root, "proc/cmdline", "console=tty0 quiet\n");
        put(&root, "proc/cpuinfo", "processor\t: 0\nmodel name\t: Intel(R) Core(TM) i7-1260P\nprocessor\t: 1\nmodel name\t: Intel(R) Core(TM) i7-1260P\n");
        put(&root, "proc/meminfo", "MemTotal:       16384000 kB\n");
        put(&root, "sys/class/graphics/fb0/name", "simpledrmdrmfb\n");
        put(&root, "sys/class/graphics/fb0/virtual_size", "3840,2160\n");
        put(&root, "sys/class/graphics/fb0/bits_per_pixel", "32\n");
        put(&root, "sys/class/drm/card0-HDMI-A-1/status", "connected\n");
        put(&root, "sys/class/drm/card0-HDMI-A-1/modes", "3840x2160\n1920x1080\n");
        std::fs::write(root.join("sys/class/drm/card0-HDMI-A-1/edid"), edid("U2723QE", (3840, 2160), (60, 34))).unwrap();
        put(&root, "sys/class/drm/card0-DP-1/status", "disconnected\n");
        put(&root, "proc/bus/input/devices", DEVICES);
        put(&root, "sys/class/net/eth0/operstate", "up\n");
        put(&root, "sys/class/net/eth0/address", "aa:bb:cc:dd:ee:ff\n");
        put(&root, "sys/class/net/eth0/type", "1\n");
        put(&root, "sys/class/net/sit0/type", "776\n");
        put(&root, "sys/class/net/sit0/operstate", "down\n");
        put(&root, "sys/class/graphics/fbcon/name", "frame buffer console\n");
        put(&root, "sys/class/net/wlan0/operstate", "down\n");
        std::fs::create_dir_all(root.join("sys/class/net/wlan0/wireless")).unwrap();
        std::fs::create_dir_all(root.join("sys/class/net/lo")).unwrap();
        put(&root, "proc/asound/cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0x1 irq 5\n");
        put(&root, "sys/bus/usb/devices/1-1/product", "Touch Monitor\n");
        put(&root, "sys/bus/usb/devices/1-1/manufacturer", "ILITEK\n");
        put(&root, "sys/bus/usb/devices/1-1/idVendor", "222a\n");
        put(&root, "sys/bus/usb/devices/1-1/idProduct", "0001\n");
        put(&root, "proc/modules", "snd 1 0 - Live 0x0\nxhci 1 0 - Live 0x0\n");
        let rep = collect(&r);
        let text = rep.to_text();
        assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
        for want in ["machine: Acme Nuc 12", "firmware: UEFI", "Secure Boot: ON", "Kernel lockdown: unknown", "i7-1260P (2 logical cores)", "memory: 15.6 GB", "fb0: simpledrmdrmfb 3840x2160, 32 bits/pixel", "HDMI-A-1: U2723QE 3840x2160 27.2 in", "[keyboard] AT Translated Set 2 keyboard (event3)", "[mouse] Logitech USB Optical Mouse", "[touchscreen] ILITEK ILITEK-TP (multi-touch)", "[touchpad] SynPS/2", "eth0: Ethernet, up", "wlan0: WiFi, down", "audio: 0 [PCH", "usb: Touch Monitor by ILITEK (222a:0001)", "kernel modules loaded: 2"] {
            assert!(text.contains(want), "missing `{want}` in:\n{text}");
        }
        assert!(!text.contains("DP-1"), "disconnected connectors are not listed");
        assert!(!text.contains("sit0") && !text.contains("fbcon"), "tunnels and the console are not hardware:\n{text}");
        assert!(rep.section("input devices").is_some());
    }

    #[test]
    fn secure_boot_and_lockdown_are_explained_in_plain_words() {
        let (m, n) = secure_boot_summary(true, Some("none [integrity] confidentiality\n"));
        assert_eq!(m, "integrity");
        assert!(n.unwrap().contains("signed with the enrolled IYAKKI key"));
        let (m, n) = secure_boot_summary(true, Some("[none] integrity confidentiality"));
        assert_eq!(m, "none");
        assert!(n.unwrap().contains("not locked down"));
        assert_eq!(secure_boot_summary(false, Some("[none] integrity confidentiality")), ("none".to_string(), None));
        assert!(secure_boot_summary(false, None).0.contains("no lockdown"));
    }

    #[test]
    fn missing_hardware_produces_clear_warnings() {
        let (r, root) = fake("empty");
        std::fs::create_dir_all(root.join("sys")).unwrap();
        std::fs::create_dir_all(root.join("proc")).unwrap();
        let rep = collect(&r);
        assert_eq!(rep.warnings.len(), 3, "{:?}", rep.warnings);
        assert!(rep.warnings.iter().any(|w| w.contains("No display")) && rep.warnings.iter().any(|w| w.contains("No keyboard")) && rep.warnings.iter().any(|w| w.contains("No mouse or touch screen")));
        assert!(rep.to_text().starts_with("ATTENTION"));
        // a laptop with only a touchpad gets the specific advice
        put(&root, "proc/bus/input/devices", DEVICES.split("\n\n").filter(|b| b.contains("TouchPad") || b.contains("keyboard")).collect::<Vec<_>>().join("\n\n").as_str());
        let rep = collect(&r);
        assert!(rep.warnings.iter().any(|w| w.contains("Only a touchpad")), "{:?}", rep.warnings);
        let text = rep.to_text();
        assert!(text.contains("machine: unknown") && text.contains("no network interfaces") && text.contains("no disks found"));
    }

    #[test]
    fn the_real_machine_never_panics_and_has_the_basics() {
        let rep = collect(&Roots::real());
        let text = rep.to_text();
        for s in ["SYSTEM", "PROCESSOR AND MEMORY", "DISPLAY", "INPUT DEVICES", "NETWORK", "STORAGE"] {
            assert!(text.contains(s), "{s}");
        }
        assert!(text.contains("architecture:"));
    }
}
