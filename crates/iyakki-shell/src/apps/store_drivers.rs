//! The Drivers chip of the STORE tile: "Drivers for this machine" (found from the DMI product name and the PCI and USB
//! ids), the state of each, and a button per item: Install (from the catalog, from a USB stick or folder, or by extracting
//! the camera firmware from a file the person supplies), Remove, Why. It only draws and reports what was asked
//! (`Out`); the store tile does the installing through the normal review (`iyakki_agent::store::drivers`, docs/MAC-HARDWARE.md).

use iyakki_agent::store::drivers::Drivers;
use iyakki_drivers::{apply, known, Availability, Delivery, Status};

use super::ScrollLog;
use crate::fx;
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

const ACCENT: [u8; 4] = rgb(214, 236, 80);
const AMBER: [u8; 4] = fx::AMBER;
/// Event ids: row buttons are `BASE + ordinal * 4 + action`; the Why page uses the ones below.
pub const BASE: usize = 2000;
pub const WHY_BACK: usize = 2900;
pub const WHY_INSTALL: usize = 2901;
pub const WHY_REMOVE: usize = 2902;
const ACT_INSTALL: usize = 0;
const ACT_REMOVE: usize = 1;
const ACT_WHY: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    /// Install the built-in pack through the normal review.
    InstallBuiltin(String),
    /// Ask for a folder or USB stick.
    InstallFolder(String),
    /// Ask for Apple's driver file and the licence answer.
    InstallExtract(String),
    Remove { id: String, title: String },
    Say(String, bool),
}

#[derive(Clone, Debug)]
pub struct DriverRow {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub availability: Option<Availability>,
    pub status: Option<Status>,
    pub detail: String,
    pub installed: bool,
    pub off: bool,
    pub delivery: Delivery,
    pub why: Vec<String>,
}

impl DriverRow {
    fn can_install(&self) -> bool {
        self.delivery != Delivery::Info && !self.installed
    }

    fn install_label(&self) -> &'static str {
        match self.delivery {
            Delivery::Builtin => "Install",
            Delivery::Folder => "From USB…",
            Delivery::Extract => "From file…",
            Delivery::Info => "",
        }
    }
}

#[derive(Default)]
pub struct Pane {
    pub rows: Vec<DriverRow>,
    pub sel: usize,
    first: usize,
    /// The row whose Why page is open.
    pub why: Option<usize>,
    text: ScrollLog,
    pub machine: String,
}

fn status_color(s: Status) -> [u8; 4] {
    match s {
        Status::Working => OK,
        Status::FirmwareMissing => AMBER,
        Status::NeedsReload => ACCENT,
        Status::NoDriver => ERR,
    }
}

fn availability_color(a: Availability) -> [u8; 4] {
    match a {
        Availability::Standard => OK,
        Availability::AfterPack => ACCENT,
        Availability::NeedsMacImage => AMBER,
        Availability::LocalBuild => AMBER,
        Availability::NotSupported => ERR,
    }
}

impl Pane {
    /// Read the machine and the installed packs again.
    pub fn refresh(&mut self, d: &Drivers) {
        let m = d.machine();
        self.machine = if m.product.is_empty() { "this machine (no DMI information)".into() } else { format!("{} {}", m.vendor, m.product).trim().to_string() };
        let installed = d.installed();
        let mut rows: Vec<DriverRow> = Vec::new();
        for r in d.rows() {
            let rec = installed.iter().find(|i| i.id == r.known.id);
            rows.push(DriverRow {
                id: r.known.id.into(),
                title: r.known.title.into(),
                summary: r.known.summary.into(),
                availability: Some(r.known.availability),
                status: Some(r.status),
                detail: r.detail.clone(),
                installed: rec.is_some(),
                off: rec.is_some_and(|i| apply::is_disabled(&d.fs, &i.id)),
                delivery: r.known.delivery,
                why: why_lines(Some(r.known), &r.devices, rec.map(|i| i.cmdline.clone()).unwrap_or_default()),
            });
        }
        // packs that are installed but no entry applies here (another catalog's pack, or a pack for another machine)
        let have: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
        for rec in installed.iter().filter(|i| !have.contains(&i.id)) {
            rows.push(DriverRow {
                id: rec.id.clone(),
                title: rec.id.clone(),
                summary: rec.why.chars().take(140).collect(),
                availability: None,
                status: None,
                detail: format!("installed from {}", rec.source),
                installed: true,
                off: apply::is_disabled(&d.fs, &rec.id),
                delivery: Delivery::Info,
                why: vec![rec.why.clone(), String::new(), format!("Licence: {}", rec.license), format!("Files: {}", if rec.files.is_empty() { "none".into() } else { rec.files.join(", ") })],
            });
        }
        let keep = self.rows.get(self.sel).map(|r| r.id.clone());
        self.rows = rows;
        self.sel = keep.and_then(|k| self.rows.iter().position(|r| r.id == k)).unwrap_or(0).min(self.rows.len().saturating_sub(1));
        if self.why.is_some_and(|w| w >= self.rows.len()) {
            self.why = None;
        }
    }

    fn open_why(&mut self, i: usize) {
        let Some(r) = self.rows.get(i) else { return };
        self.sel = i;
        self.why = Some(i);
        self.text.clear();
        self.text.push(r.title.clone(), TEXT);
        if let Some(a) = r.availability {
            self.text.push(format!("{} · {}", a.label(), r.status.map_or(String::new(), |s| s.label().to_string())), availability_color(a));
        }
        self.text.push("", TEXT);
        for l in &r.why {
            self.text.push(l.clone(), if (l.chars().all(|c| c.is_ascii_uppercase() || c == ' ') && !l.is_empty()) || l.starts_with("OPTION ") { ACCENT } else { MUTED });
        }
        self.text.scroll = usize::MAX / 4;
    }

    fn act(&mut self, i: usize, act: usize) -> Option<Out> {
        let r = self.rows.get(i)?.clone();
        self.sel = i;
        match act {
            ACT_INSTALL => {
                if r.installed {
                    return Some(Out::Say(format!("{} is already installed.", r.title), false));
                }
                match r.delivery {
                    Delivery::Builtin => Some(Out::InstallBuiltin(r.id)),
                    Delivery::Folder => Some(Out::InstallFolder(r.id)),
                    Delivery::Extract => Some(Out::InstallExtract(r.id)),
                    Delivery::Info => Some(Out::Say(format!("{} cannot be installed here: {}", r.title, r.detail), false)),
                }
            }
            ACT_REMOVE => {
                if r.installed {
                    Some(Out::Remove { id: r.id, title: r.title })
                } else {
                    Some(Out::Say(format!("{} is not installed.", r.title), false))
                }
            }
            _ => {
                self.open_why(i);
                None
            }
        }
    }

    /// Keys and clicks. `None`: not mine (or handled, with nothing to report).
    pub fn handle(&mut self, ev: &Event) -> Option<Out> {
        if self.why.is_some() && self.text.handle(ev) {
            return None;
        }
        match ev {
            Event::Tab(WHY_BACK) => self.why = None,
            Event::Tab(WHY_INSTALL) => return self.act(self.why?, ACT_INSTALL),
            Event::Tab(WHY_REMOVE) => return self.act(self.why?, ACT_REMOVE),
            Event::Tab(n) if (BASE..BASE + 400).contains(n) => return self.act((*n - BASE) / 4, (*n - BASE) % 4),
            Event::Row(i) if self.why.is_none() => self.open_why(*i),
            Event::Scroll(d) if self.why.is_none() => self.sel = (self.sel as i32 - *d).clamp(0, self.rows.len().saturating_sub(1) as i32) as usize,
            Event::Key(k) => match (k, self.why) {
                (Key::Esc | Key::Left, Some(_)) => self.why = None,
                (Key::Up, None) => self.sel = self.sel.saturating_sub(1),
                (Key::Down, None) => self.sel = (self.sel + 1).min(self.rows.len().saturating_sub(1)),
                (Key::Enter | Key::Char('w'), None) => self.open_why(self.sel),
                (Key::Char('i'), w) => return self.act(w.unwrap_or(self.sel), ACT_INSTALL),
                (Key::Char('r'), w) => return self.act(w.unwrap_or(self.sel), ACT_REMOVE),
                (Key::Char('w'), Some(_)) => {}
                _ => {}
            },
            _ => {}
        }
        None
    }

    /// Does this pane want the Esc key (to close the Why page)?
    pub fn wants_escape(&self) -> bool {
        self.why.is_some()
    }

    fn row(&self, g: &mut Gfx, r: R, s: f32, i: usize) {
        let row = &self.rows[i];
        let hot = g.hovered(r);
        fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, i == self.sel, hot, ACCENT);
        g.hit(r, Event::Row(i), &format!("{} {}", row.title, row.detail));
        let c = row.availability.map_or(rgb(240, 120, 220), availability_color);
        let d = r.h - 16.0 * s;
        g.rrect(r.x + 10.0 * s, r.y + 8.0 * s, d, d, d * 0.22, fx::over(fx::WELL, fx::with_alpha(c, 70)));
        g.rrect_outline(r.x + 10.0 * s, r.y + 8.0 * s, d, d, d * 0.22, 1.2, fx::with_alpha(c, 200));
        let letter: String = row.title.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
        let lw = g.text_width(d * 0.5, &letter, Face::Bold);
        g.text(r.x + 10.0 * s + (d - lw) / 2.0, r.y + 8.0 * s + (d - g.line_height(d * 0.5, Face::Bold)) / 2.0, d * 0.5, fx::readable(c, fx::WELL, 4.5), &letter, Face::Bold);
        let tx = r.x + 10.0 * s + d + 12.0 * s;
        let mut rx = r.x + r.w - 8.0 * s;
        let bh = 12.0 * 2.1 * s;
        let by = r.y + (r.h - bh) / 2.0;
        let mut put = |g: &mut Gfx, label: &str, col: [u8; 4], act: usize| {
            let bw = g.text_width(12.0 * s, label, Face::Bold) + 12.0 * 1.7 * s;
            rx -= bw;
            g.button(rx, by, 12.0 * s, label, col, Event::Tab(BASE + i * 4 + act));
            rx -= 6.0 * s;
        };
        put(g, "Why", MUTED, ACT_WHY);
        if row.installed {
            put(g, "Remove", ERR, ACT_REMOVE);
        }
        if row.can_install() {
            put(g, row.install_label(), ACCENT, ACT_INSTALL);
        }
        let room = (rx - tx - 4.0 * s).max(60.0 * s);
        let title = g.fit(16.0 * s, &row.title, Face::Bold, (room - 20.0 * s).max(60.0 * s));
        let tw = g.text_width(16.0 * s, &title, Face::Bold);
        g.text(tx, r.y + 7.0 * s, 16.0 * s, TEXT, &title, Face::Bold);
        let mut bx = tx + tw + 10.0 * s;
        if let Some(st) = row.status {
            let l = st.label();
            if bx + g.text_width(12.0 * s, l, Face::Bold) + 17.0 * s < tx + room {
                bx += fx::chip(g, bx, r.y + 6.0 * s, s, l, status_color(st), false) + 6.0 * s;
            }
        }
        if row.installed {
            let l = if row.off { "installed, off" } else { "installed" };
            if bx + g.text_width(12.0 * s, l, Face::Bold) + 17.0 * s < tx + room {
                fx::chip(g, bx, r.y + 6.0 * s, s, l, OK, true);
            }
        }
        let sub = match row.availability {
            Some(a) => format!("{} · {}", a.label(), if row.summary.is_empty() { &row.detail } else { &row.summary }),
            None => row.summary.clone(),
        };
        let line = g.fit(12.0 * s, &sub, Face::Sans, room);
        g.text(tx, r.y + r.h - 8.0 * s - g.line_height(12.0 * s, Face::Sans), 12.0 * s, MUTED, &line, Face::Sans);
    }

    pub fn render(&mut self, g: &mut Gfx, body: R, s: f32) {
        let (x, mut y, w) = (body.x, body.y, body.w);
        if let Some(i) = self.why.filter(|i| *i < self.rows.len()) {
            let row = self.rows[i].clone();
            g.button(x, y, 13.0 * s, "Back (Esc)", MUTED, Event::Tab(WHY_BACK));
            let mut bx = x + g.text_width(13.0 * s, "Back (Esc)", Face::Bold) + 13.0 * 1.7 * s + 10.0 * s;
            if row.can_install() {
                bx += g.button(bx, y, 13.0 * s, &format!("{} (i)", row.install_label()), ACCENT, Event::Tab(WHY_INSTALL)) + 8.0 * s;
            }
            if row.installed {
                g.button(bx, y, 13.0 * s, "Remove (r)", ERR, Event::Tab(WHY_REMOVE));
            }
            y += 44.0 * s;
            let r = R::new(x, y, w, (body.y + body.h - y).max(40.0 * s));
            fx::panel(g, r, s, ACCENT);
            let inner = R::new(r.x + 16.0 * s, r.y + 10.0 * s, r.w - 32.0 * s, (r.h - 20.0 * s).max(16.0 * s));
            self.text.render(g, inner, 13.0 * s, Face::Sans);
            return;
        }
        let head = format!("Drivers for {}", self.machine);
        let head = g.fit(14.0 * s, &head, Face::Bold, w);
        g.text(x, y, 14.0 * s, TEXT, &head, Face::Bold);
        y += 22.0 * s;
        let sub = g.fit(12.0 * s, "Found from the machine's own ids. Nothing is compiled here; firmware you supply goes to the data volume.", Face::Sans, w);
        g.text(x, y, 12.0 * s, DIM, &sub, Face::Sans);
        y += 24.0 * s;
        if self.rows.is_empty() {
            fx::empty_state(g, R::new(x, y, w, (body.y + body.h - y).max(120.0 * s)), s, Icon::Store, "No drivers to offer for this machine", "The built-in list covers Macs. Nothing is missing that this store knows about; a signed driver pack can still be installed from a file (Settings, i).", ACCENT);
            return;
        }
        self.sel = self.sel.min(self.rows.len() - 1);
        let (rh, gap) = (58.0 * s, 4.0 * s);
        let avail = (body.y + body.h - y).max(rh);
        let fit = ((avail + gap) / (rh + gap)).floor().max(1.0) as usize;
        let mut first = self.first.min(self.rows.len().saturating_sub(1));
        if self.sel < first {
            first = self.sel;
        }
        if self.sel >= first + fit {
            first = self.sel + 1 - fit;
        }
        self.first = first;
        for i in (first..self.rows.len()).take(fit) {
            self.row(g, R::new(x, y, w, rh), s, i);
            y += rh + gap;
        }
        if self.rows.len() > fit {
            fx::scrollbar(g, R::new(x + w - 6.0 * s, body.y, 4.0 * s, body.h), s, first, fit, self.rows.len(), ACCENT);
        }
    }
}

fn why_lines(k: Option<&known::Known>, devices: &[String], cmdline: Vec<String>) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(k) = k {
        v.push("WHAT IT IS".to_string());
        v.push(k.why.to_string());
        v.push(String::new());
        v.push("WHAT YOU SUPPLY".into());
        v.push(k.supply.to_string());
        if !k.guide.is_empty() {
            v.push(String::new());
            v.push("HOW".into());
            v.extend(k.guide.lines().map(str::to_string));
        }
        v.push(String::new());
        v.push("LICENCE".into());
        v.push(k.license.to_string());
        if k.delivery == Delivery::Extract {
            v.push(String::new());
            v.push("APPLE'S LICENCE".into());
            v.push(iyakki_drivers::facetime::NOTICE.to_string());
        }
        if !devices.is_empty() {
            v.push(String::new());
            v.push("FOUND ON THIS MACHINE".into());
            v.extend(devices.iter().cloned());
        }
        v.push(String::new());
        v.push("STATUS".into());
        v.push(format!("Standard image: {}.", k.availability.label()));
    }
    if !cmdline.is_empty() {
        v.push(String::new());
        v.push("SUGGESTED KERNEL OPTIONS (NOT APPLIED)".into());
        v.extend(cmdline);
    }
    v
}


#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_drivers::Roots;

    fn fake(tag: &str, product: &str, extra: &[(&str, &str)]) -> Drivers {
        let root = std::env::temp_dir().join(format!("iyakki-pane-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut files = vec![("sys/class/dmi/id/sys_vendor", "Apple Inc.\n"), ("sys/class/dmi/id/product_name", product), ("proc/sys/kernel/osrelease", "6.12.112\n"), ("proc/modules", "btusb 1 0 - Live 0x0\ni915 1 0 - Live 0x0\n")];
        files.extend_from_slice(extra);
        for (rel, c) in files {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, c).unwrap();
        }
        Drivers::for_data(&root.join("data")).with_roots(Roots { sys: root.join("sys"), proc: root.join("proc") })
    }

    fn mini(tag: &str) -> Drivers {
        fake(tag, "Macmini7,1\n", &[("sys/bus/pci/devices/0000:00:02.0/vendor", "0x8086\n"), ("sys/bus/pci/devices/0000:00:02.0/device", "0x0a26\n"), ("sys/bus/pci/devices/0000:03:00.0/vendor", "0x14e4\n"), ("sys/bus/pci/devices/0000:03:00.0/device", "0x43a0\n"), ("sys/bus/usb/devices/1-4/idVendor", "05ac\n"), ("sys/bus/usb/devices/1-4/idProduct", "8289\n"), ("sys/bus/usb/devices/1-4/product", "Bluetooth USB Host Controller\n")])
    }

    fn draw(p: &mut Pane, w: u32, h: u32) -> Gfx {
        let s = crate::shell::scale_for(w, h);
        let mut g = Gfx::new(w, h);
        g.clear(rgb(9, 11, 20));
        let hh = 46.0 * s;
        p.render(&mut g, R::new(0.0, hh, w as f32, h as f32 - hh), s);
        g
    }

    const SIZES: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480)];

    #[test]
    fn the_mac_mini_lists_what_applies_with_the_right_buttons_and_nothing_else() {
        let mut p = Pane::default();
        p.refresh(&mini("list"));
        let ids: Vec<&str> = p.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["mac-gpu-intel", "mac-wifi-bcm4360", "mac-bt-broadcom", "mac-fan-sensors", "mac-audio-cirrus", "mac-keyboard-options"], "no camera, no T2: {ids:?}");
        assert!(p.machine.contains("Macmini7,1"));
        let row = |id: &str| p.rows.iter().find(|r| r.id == id).unwrap().clone();
        assert!(!row("mac-gpu-intel").can_install() && !row("mac-fan-sensors").can_install(), "information entries have no install button");
        let wifi = row("mac-wifi-bcm4360");
        assert!(wifi.can_install() && wifi.install_label() == "From USB…", "the Wi-Fi card takes the signed pack from a USB stick");
        assert_eq!((wifi.availability, wifi.delivery, wifi.status), (Some(Availability::LocalBuild), Delivery::Folder, Some(Status::NoDriver)));
        let why = wifi.why.join("\n");
        for need in ["HOW", "OPTION A", "build-wl-pack.sh --accept-broadcom-licence", "OPTION B", "MT7612U", "AR9271", "OPTION C", "Ethernet", "NON-FREE", "does not ship it", "tainted"] {
            assert!(why.contains(need), "Why page has {need:?}:\n{why}");
        }
        assert_eq!(row("mac-bt-broadcom").install_label(), "From USB…");
        assert_eq!(row("mac-keyboard-options").install_label(), "Install");
        assert_eq!(row("mac-bt-broadcom").status, Some(Status::FirmwareMissing));
        assert!(row("mac-bt-broadcom").why.iter().any(|l| l.contains("hcd")), "{:?}", row("mac-bt-broadcom").why);
    }

    #[test]
    fn a_generic_pc_gets_the_empty_state_and_a_macbook_gets_the_camera_with_the_apple_notice_under_why() {
        let mut p = Pane::default();
        let pc = {
            let d = fake("pc", "OptiPlex\n", &[]);
            // not an Apple vendor: nothing applies
            let root = d.roots.sys.parent().unwrap().to_path_buf();
            std::fs::write(root.join("sys/class/dmi/id/sys_vendor"), "Dell Inc.\n").unwrap();
            d
        };
        p.refresh(&pc);
        assert!(p.rows.is_empty());
        for (w, h) in SIZES {
            let g = draw(&mut p, w, h);
            assert!(g.hits.iter().all(|x| x.rect.x + x.rect.w <= w as f32 + 1.0));
        }
        let mbp = fake("mbp", "MacBookPro11,1\n", &[("sys/bus/pci/devices/0000:02:00.0/vendor", "0x14e4\n"), ("sys/bus/pci/devices/0000:02:00.0/device", "0x1570\n")]);
        p.refresh(&mbp);
        let cam = p.rows.iter().position(|r| r.id == "mac-facetimehd").expect("camera listed");
        assert_eq!(p.rows[cam].install_label(), "From file…");
        p.handle(&Event::Tab(BASE + cam * 4 + ACT_WHY));
        assert_eq!(p.why, Some(cam));
        assert!(p.rows[cam].why.iter().any(|l| l.contains("Apple's licence")) || p.rows[cam].why.iter().any(|l| l.contains("Apple's software")));
    }

    #[test]
    fn buttons_and_keys_report_what_was_asked_and_why_opens_and_closes() {
        let mut p = Pane::default();
        p.refresh(&mini("keys"));
        let at = |p: &Pane, id: &str| p.rows.iter().position(|r| r.id == id).unwrap();
        let bt = at(&p, "mac-bt-broadcom");
        assert_eq!(p.handle(&Event::Tab(BASE + bt * 4 + ACT_INSTALL)), Some(Out::InstallFolder("mac-bt-broadcom".into())));
        let kb = at(&p, "mac-keyboard-options");
        assert_eq!(p.handle(&Event::Tab(BASE + kb * 4 + ACT_INSTALL)), Some(Out::InstallBuiltin("mac-keyboard-options".into())));
        let wifi = at(&p, "mac-wifi-bcm4360");
        assert_eq!(p.handle(&Event::Tab(BASE + wifi * 4 + ACT_INSTALL)), Some(Out::InstallFolder("mac-wifi-bcm4360".into())));
        let gpu = at(&p, "mac-gpu-intel");
        assert!(matches!(p.handle(&Event::Tab(BASE + gpu * 4 + ACT_INSTALL)), Some(Out::Say(m, false)) if m.contains("cannot be installed")));
        assert!(matches!(p.handle(&Event::Tab(BASE + bt * 4 + ACT_REMOVE)), Some(Out::Say(m, false)) if m.contains("not installed")));
        // keyboard: select, Why, Esc
        p.sel = 0;
        p.handle(&Event::Key(Key::Down));
        assert_eq!(p.sel, 1);
        p.handle(&Event::Key(Key::Enter));
        assert_eq!(p.why, Some(1));
        assert!(p.wants_escape());
        p.handle(&Event::Key(Key::Esc));
        assert!(p.why.is_none() && !p.wants_escape());
        p.sel = bt;
        assert_eq!(p.handle(&Event::Key(Key::Char('i'))), Some(Out::InstallFolder("mac-bt-broadcom".into())));
    }

    #[test]
    fn rows_why_page_and_buttons_stay_inside_the_screen_and_never_overlap_at_four_sizes() {
        let mut p = Pane::default();
        p.refresh(&mini("geo"));
        for (w, h) in SIZES {
            let g = draw(&mut p, w, h);
            for hit in &g.hits {
                let r = hit.rect;
                assert!(r.x >= -1.0 && r.y >= -1.0 && r.x + r.w <= w as f32 + 1.0 && r.y + r.h <= h as f32 + 1.0, "{} sticks out {r:?} at {w}x{h}", hit.label);
            }
            let rs: Vec<R> = g.hits.iter().filter(|h| matches!(h.ev, Event::Tab(n) if n >= BASE)).map(|h| h.rect).collect();
            assert!(!rs.is_empty(), "buttons at {w}x{h}");
            for (i, a) in rs.iter().enumerate() {
                for b in &rs[i + 1..] {
                    let apart = a.y + a.h <= b.y + 0.5 || b.y + b.h <= a.y + 0.5 || a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5;
                    assert!(apart, "buttons overlap at {w}x{h}: {a:?} {b:?}");
                }
            }
            // the Why page
            let bt = p.rows.iter().position(|r| r.id == "mac-bt-broadcom").unwrap();
            p.open_why(bt);
            let g = draw(&mut p, w, h);
            assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Tab(WHY_BACK))), "Back at {w}x{h}");
            assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Tab(WHY_INSTALL))), "Install on the Why page at {w}x{h}");
            for hit in &g.hits {
                let r = hit.rect;
                assert!(r.x + r.w <= w as f32 + 1.0 && r.y + r.h <= h as f32 + 1.0, "{} sticks out on the Why page at {w}x{h}", hit.label);
            }
            p.why = None;
            // the Wi-Fi guide is the longest page: it scrolls inside its panel, nothing sticks out
            let wifi = p.rows.iter().position(|r| r.id == "mac-wifi-bcm4360").unwrap();
            p.open_why(wifi);
            let g = draw(&mut p, w, h);
            assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Tab(WHY_INSTALL))), "From USB on the Wi-Fi page at {w}x{h}");
            for hit in &g.hits {
                let r = hit.rect;
                assert!(r.x >= -1.0 && r.y >= -1.0 && r.x + r.w <= w as f32 + 1.0 && r.y + r.h <= h as f32 + 1.0, "{} sticks out on the Wi-Fi page at {w}x{h}", hit.label);
            }
            p.why = None;
        }
    }
}
