use super::*;
use std::cell::{Cell, RefCell};
use std::os::unix::fs::symlink;

struct Fake {
    writes: RefCell<Vec<(String, String)>>,
    runs: RefCell<Vec<String>>,
    reopens: Cell<u32>,
    waits: Cell<u32>,
    /// the wait with this number (1 based) sees an event
    event_on_wait: Option<u32>,
    logs: RefCell<Vec<String>>,
    /// writes to files that contain this text fail
    fail: Option<&'static str>,
}

impl Fake {
    fn new(event_on_wait: Option<u32>) -> Fake {
        Fake { writes: Default::default(), runs: Default::default(), reopens: Cell::new(0), waits: Cell::new(0), event_on_wait, logs: Default::default(), fail: None }
    }
    fn wrote(&self) -> Vec<String> {
        self.writes.borrow().iter().map(|(p, v)| format!("{}={v}", p.rsplit('/').next().unwrap_or(p))).collect()
    }
    fn wrote_full(&self) -> Vec<(String, String)> {
        self.writes.borrow().clone()
    }
}

impl Env for Fake {
    fn read(&self, p: &Path) -> Option<String> {
        std::fs::read_to_string(p).ok()
    }
    fn write(&self, p: &Path, v: &str) -> io::Result<()> {
        let s = p.display().to_string();
        if self.fail.is_some_and(|f| s.contains(f)) {
            return Err(io::Error::other("denied"));
        }
        self.writes.borrow_mut().push((s, v.to_string()));
        Ok(())
    }
    fn list(&self, d: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        v.sort();
        v
    }
    fn resolve(&self, p: &Path) -> Option<PathBuf> {
        std::fs::canonicalize(p).ok()
    }
    fn sleep(&self, _: Duration) {}
    fn events(&self) -> u64 {
        0
    }
    fn wait_events(&self, _: u64, _: Duration) -> bool {
        self.waits.set(self.waits.get() + 1);
        self.event_on_wait == Some(self.waits.get())
    }
    fn request_reopen(&self) {
        self.reopens.set(self.reopens.get() + 1);
    }
    fn run(&self, prog: &str, args: &[&str]) -> bool {
        self.runs.borrow_mut().push(format!("{prog} {}", args.join(" ")));
        true
    }
    fn log(&self, l: &str) {
        self.logs.borrow_mut().push(l.to_string());
    }
}

fn put(p: &Path, s: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

fn root(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-touchrecover-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

const MT: &str = "20000000000000\n"; // ABS_MT_POSITION_X (bit 53)
const DIRECT: &str = "2\n";
fn kbd_caps() -> String {
    format!("{:x}\n", (1u64 << 30) | (1u64 << 28))
}

struct Usb {
    class: &'static str,
    iface_class: &'static str,
    iface_proto: &'static str,
    driver: Option<&'static str>,
    name: &'static str,
    props: &'static str,
    node: &'static str,
}

impl Default for Usb {
    fn default() -> Self {
        Usb { class: "00", iface_class: "03", iface_proto: "00", driver: Some("hid-multitouch"), name: "ILITEK ILITEK-TP", props: DIRECT, node: "1-5.2" }
    }
}

/// A USB touch screen below a root hub: returns (sys, device dir).
fn usb_tree(tag: &str, u: Usb) -> (PathBuf, PathBuf) {
    let sys = root(tag);
    let dev = sys.join("devices/pci0000:00/0000:00:14.0/usb1/1-5").join(u.node);
    put(&dev.join("idVendor"), "27c0\n");
    put(&dev.join("idProduct"), "0859\n");
    put(&dev.join("bDeviceClass"), &format!("{}\n", u.class));
    let iface = dev.join(format!("{}:1.0", u.node));
    put(&iface.join("bInterfaceClass"), &format!("{}\n", u.iface_class));
    put(&iface.join("bInterfaceProtocol"), &format!("{}\n", u.iface_proto));
    let hid = "0003:27C0:0859.0001";
    let inp = iface.join(hid).join("input/input7");
    put(&inp.join("name"), &format!("{}\n", u.name));
    put(&inp.join("capabilities/abs"), MT);
    put(&inp.join("capabilities/key"), "400\n");
    put(&inp.join("properties"), u.props);
    std::fs::create_dir_all(sys.join("class/input")).unwrap();
    symlink(&inp, sys.join("class/input/input7")).unwrap();
    std::fs::create_dir_all(sys.join("bus/hid/drivers/hid-multitouch")).unwrap();
    std::fs::create_dir_all(sys.join("bus/hid/drivers/hid-generic")).unwrap();
    std::fs::create_dir_all(sys.join("bus/hid/devices").join(hid)).unwrap();
    if let Some(d) = u.driver {
        symlink(sys.join("bus/hid/drivers").join(d), sys.join("bus/hid/devices").join(hid).join("driver")).unwrap();
    }
    (sys, dev)
}

fn i2c_tree(tag: &str, driver: &str) -> PathBuf {
    let sys = root(tag);
    let client = sys.join("devices/pci0000:00/0000:00:15.0/i2c_designware.0/i2c-0/i2c-ELAN0001:00");
    let hid = "0018:04F3:2A0B.0002";
    let inp = client.join(hid).join("input/input9");
    put(&inp.join("name"), "ELAN0001:00 04F3:2A0B\n");
    put(&inp.join("capabilities/abs"), MT);
    put(&inp.join("capabilities/key"), "400\n");
    put(&inp.join("properties"), DIRECT);
    std::fs::create_dir_all(sys.join("class/input")).unwrap();
    symlink(&inp, sys.join("class/input/input9")).unwrap();
    std::fs::create_dir_all(sys.join("bus/hid/drivers/hid-multitouch")).unwrap();
    std::fs::create_dir_all(sys.join("bus/hid/devices").join(hid)).unwrap();
    symlink(sys.join("bus/hid/drivers/hid-multitouch"), sys.join("bus/hid/devices").join(hid).join("driver")).unwrap();
    std::fs::create_dir_all(sys.join("bus/hid/drivers/hid-multitouch").join("x")).unwrap();
    std::fs::remove_dir_all(sys.join("bus/hid/drivers/hid-multitouch").join("x")).unwrap();
    std::fs::create_dir_all(sys.join("bus/i2c/drivers").join(driver)).unwrap();
    std::fs::create_dir_all(sys.join("bus/i2c/devices/i2c-ELAN0001:00")).unwrap();
    symlink(sys.join("bus/i2c/drivers").join(driver), sys.join("bus/i2c/devices/i2c-ELAN0001:00/driver")).unwrap();
    sys
}

fn w() -> Duration {
    Duration::from_secs(1)
}

#[test]
fn usb_ladder_runs_in_order_and_each_step_is_logged() {
    let (sys, _) = usb_tree("order", Usb::default());
    let f = Fake::new(None);
    let o = run_ladder(&f, &sys, false, w());
    assert_eq!(o.tried, vec![Rung::Reopen, Rung::HidRebind, Rung::UsbReset]);
    assert_eq!(o.fixed_by, None);
    assert_eq!(f.reopens.get(), 1);
    assert_eq!(f.wrote(), ["unbind=0003:27C0:0859.0001", "bind=0003:27C0:0859.0001", "control=on", "authorized=0", "authorized=1"].map(String::from));
    let logs = f.logs.borrow().join("\n");
    assert!(logs.contains("step Reopen") && logs.contains("step HidRebind") && logs.contains("step UsbReset"), "{logs}");
    assert!(logs.contains("no event after UsbReset"), "{logs}");
    assert!(describe(&o).contains("No touch event was seen"));
}

#[test]
fn the_ladder_stops_at_the_first_step_after_which_an_event_arrives() {
    let (sys, _) = usb_tree("stop", Usb::default());
    let f = Fake::new(Some(2));
    let o = run_ladder(&f, &sys, false, w());
    assert_eq!(o.tried, vec![Rung::Reopen, Rung::HidRebind]);
    assert_eq!(o.fixed_by, Some(Rung::HidRebind));
    assert!(f.wrote().iter().all(|x| !x.starts_with("authorized")), "no USB reset when the HID rebind helped");
    assert!(describe(&o).starts_with("Touch events arrived after: unbind and bind the HID driver"));
    let f = Fake::new(Some(1));
    assert_eq!(run_ladder(&f, &sys, false, w()).tried, vec![Rung::Reopen]);
    assert!(f.wrote().is_empty(), "re-opening writes nothing to sysfs");
}

#[test]
fn usb_reset_falls_back_to_the_usb_driver_and_never_leaves_it_unauthorized() {
    let (sys, _) = usb_tree("fallback", Usb::default());
    let mut f = Fake::new(None);
    f.fail = Some("authorized");
    let o = run_ladder(&f, &sys, false, w());
    assert!(o.tried.contains(&Rung::UsbReset));
    let all = f.wrote_full();
    assert!(all.iter().any(|(p, v)| p.ends_with("bus/usb/drivers/usb/unbind") && v == "1-5.2"), "{all:?}");
    assert!(all.iter().any(|(p, v)| p.ends_with("bus/usb/drivers/usb/bind") && v == "1-5.2"), "{all:?}");
}

#[test]
fn i2c_ladder_rebinds_the_i2c_client_and_can_reload_the_module_when_asked() {
    let sys = i2c_tree("i2c", "i2c_hid_acpi");
    put(&sys.join("module/hid_multitouch/initstate"), "live\n");
    let f = Fake::new(None);
    let o = run_ladder(&f, &sys, true, w());
    assert_eq!(o.tried, vec![Rung::Reopen, Rung::HidRebind, Rung::I2cRebind, Rung::ModuleReload]);
    let all = f.wrote_full();
    assert!(all.iter().any(|(p, v)| p.ends_with("i2c/drivers/i2c_hid_acpi/unbind") && v == "i2c-ELAN0001:00"), "{all:?}");
    assert!(all.iter().any(|(p, v)| p.ends_with("i2c/drivers/i2c_hid_acpi/bind") && v == "i2c-ELAN0001:00"));
    assert_eq!(*f.runs.borrow(), vec!["modprobe -r hid_multitouch", "modprobe hid_multitouch"]);
    // not manual: no module reload
    let f = Fake::new(None);
    assert_eq!(run_ladder(&f, &sys, false, w()).tried, vec![Rung::Reopen, Rung::HidRebind, Rung::I2cRebind]);
    assert!(f.runs.borrow().is_empty());
}

#[test]
fn the_module_is_not_reloaded_when_it_is_built_in_or_serves_another_device() {
    let sys = i2c_tree("modbuiltin", "i2c_hid_acpi");
    let f = Fake::new(None);
    let o = run_ladder(&f, &sys, true, w());
    assert!(o.problems.iter().any(|p| p.contains("built into the kernel")), "{:?}", o.problems);
    assert!(f.runs.borrow().is_empty());
    // a touchpad bound to the same driver
    put(&sys.join("module/hid_multitouch/initstate"), "live\n");
    std::fs::create_dir_all(sys.join("bus/hid/drivers/hid-multitouch/0018:06CB:CE26.0003")).unwrap();
    let f = Fake::new(None);
    let o = run_ladder(&f, &sys, true, w());
    assert!(o.problems.iter().any(|p| p.contains("not a touch screen") && p.contains("0018:06CB:CE26.0003")), "{:?}", o.problems);
    assert!(f.runs.borrow().is_empty());
}

#[test]
fn a_hub_a_keyboard_a_disk_interface_or_a_foreign_driver_is_refused_and_nothing_is_written() {
    let cases: Vec<(&str, Usb, &str)> = vec![
        ("hub", Usb { class: "09", ..Usb::default() }, "hub"),
        ("kbdproto", Usb { iface_proto: "01", ..Usb::default() }, "keyboard interface"),
        ("disk", Usb { iface_class: "08", ..Usb::default() }, "non-HID interface"),
        ("driver", Usb { driver: Some("hid-generic"), ..Usb::default() }, "not hid-multitouch"),
        ("nodriver", Usb { driver: None, ..Usb::default() }, "no driver"),
    ];
    for (tag, u, want) in cases {
        let (sys, _) = usb_tree(&format!("refuse-{tag}"), u);
        let f = Fake::new(None);
        let o = run_ladder(&f, &sys, true, w());
        assert!(o.tried.is_empty(), "{tag}: {:?}", o.tried);
        assert!(o.problems.iter().any(|p| p.contains(want)), "{tag}: {:?}", o.problems);
        assert!(f.wrote_full().is_empty() && f.reopens.get() == 0, "{tag}");
    }
}

#[test]
fn a_device_that_also_types_or_sits_on_a_root_port_is_refused() {
    // a second input below the same USB device that has letters
    let (sys, dev) = usb_tree("alsokbd", Usb::default());
    let inp2 = dev.join("1-5.2:1.0/0003:27C0:0859.0001/input/input8");
    put(&inp2.join("name"), "keys\n");
    put(&inp2.join("capabilities/key"), &kbd_caps());
    symlink(&inp2, sys.join("class/input/input8")).unwrap();
    let f = Fake::new(None);
    let o = run_ladder(&f, &sys, false, w());
    assert!(o.problems.iter().any(|p| p.contains("provides a keyboard")), "{:?}", o.problems);
    assert!(f.wrote_full().is_empty());
    // a root hub name is not a device we reset
    assert!(!is_usb_device_name("usb1") && !is_usb_device_name("1-5:1.0") && is_usb_device_name("1-5.2.1") && is_usb_device_name("2-1"));
    // an I2C adapter is not a client; an I2C client on a foreign driver is refused
    assert!(!is_i2c_client_name("i2c-0") && is_i2c_client_name("i2c-ELAN0001:00") && is_i2c_client_name("0-0038"));
    let sys = i2c_tree("i2cforeign", "intel-lpss");
    let f = Fake::new(None);
    let o = run_ladder(&f, &sys, true, w());
    assert!(o.problems.iter().any(|p| p.contains("not i2c_hid")), "{:?}", o.problems);
    assert!(f.wrote_full().is_empty());
}

#[test]
fn touch_pads_keyboards_and_other_buses_are_not_touch_screens() {
    // a pad: multi-touch but not direct and called a pad
    let (sys, _) = usb_tree("pad", Usb { props: "5\n", name: "Apple Internal Keyboard / Trackpad", ..Usb::default() });
    assert!(discover(&Fake::new(None), &sys).is_empty());
    let (sys, _) = usb_tree("pad2", Usb { props: "5\n", name: "Synaptics Touchpad", ..Usb::default() });
    assert!(discover(&Fake::new(None), &sys).is_empty());
    let (sys, _) = usb_tree("ok", Usb::default());
    let d = discover(&Fake::new(None), &sys);
    assert_eq!(d.len(), 1);
    assert_eq!((d[0].bus, d[0].ident.as_str(), d[0].hid.as_str()), (Bus::Usb, "27c0:0859", "0003:27C0:0859.0001"));
    // no devices at all
    assert!(discover(&Fake::new(None), &root("none")).is_empty());
    assert!(describe(&run_ladder(&Fake::new(None), &root("none2"), true, w())).contains("No touch screen"));
}

#[test]
fn detection_waits_the_grace_period_then_reports_silence_and_a_replug_starts_again() {
    let mut w = Watch::default();
    assert_eq!(w.observe(5, false, 0), Verdict::Absent);
    assert_eq!(w.observe(10, true, 0), Verdict::Waiting(GRACE_SECS));
    assert_eq!(w.observe(10 + GRACE_SECS - 1, true, 0), Verdict::Waiting(1));
    assert_eq!(w.observe(10 + GRACE_SECS, true, 0), Verdict::Silent);
    // a ladder ran: counted again
    w.restart(200, 0);
    assert_eq!(w.observe(201, true, 0), Verdict::Waiting(GRACE_SECS - 1));
    // an event ends it for good
    assert_eq!(w.observe(210, true, 3), Verdict::Working);
    assert_eq!(w.observe(900, true, 3), Verdict::Working);
    // the device goes away and comes back: a new count, events before do not count
    assert_eq!(w.observe(901, false, 3), Verdict::Absent);
    assert_eq!(w.observe(902, true, 3), Verdict::Waiting(GRACE_SECS));
    assert_eq!(w.observe(902 + GRACE_SECS, true, 3), Verdict::Silent);
}

#[test]
fn automatic_tries_are_few_and_back_off_and_manual_presses_are_spaced() {
    let mut l = Limiter::default();
    assert!(l.allow(false, 100).is_ok());
    assert!(l.allow(false, 150).unwrap_err().contains("130 s"));
    assert!(l.allow(false, 100 + AUTO_GAPS[1]).is_ok());
    assert!(l.allow(false, 100 + AUTO_GAPS[1] + 899).is_err());
    assert!(l.allow(false, 100 + AUTO_GAPS[1] + AUTO_GAPS[2]).is_ok());
    assert!(l.allow(false, 1_000_000).unwrap_err().contains("used up"));
    // manual is separate from automatic
    assert!(l.allow(true, 5000).is_ok());
    assert!(l.allow(true, 5000 + MANUAL_GAP - 1).unwrap_err().contains("wait 1 s"));
    let mut t = 5000;
    for _ in 1..MANUAL_PER_HOUR {
        t += MANUAL_GAP;
        assert!(l.allow(true, t).is_ok());
    }
    assert!(l.allow(true, t + MANUAL_GAP).unwrap_err().contains("last hour"));
    assert!(l.allow(true, 5000 + 3600 + 1).is_ok(), "the hour passes");
}

#[test]
fn capability_bits_and_switches() {
    assert!(cap_bit(MT, ABS_MT_POSITION_X) && !cap_bit(MT, 0x36));
    // two words: the high word first
    assert!(cap_bit("1 0", 64) && !cap_bit("1 0", 0) && cap_bit("0 1", 0));
    assert!(!cap_bit("", 3) && !cap_bit("zz", 3));
    assert!(cap_bit(&kbd_caps(), KEY_A) && cap_bit(&kbd_caps(), KEY_ENTER) && !cap_bit("400", KEY_A));
    assert!(auto_enabled("quiet splash") && !auto_enabled("quiet iyakki.touchrecover=off"));
    assert_eq!(ladder(Bus::Usb, false).len(), 3);
    assert_eq!(ladder(Bus::I2c, true).last(), Some(&Rung::ModuleReload));
    assert!(!ladder(Bus::Usb, false).contains(&Rung::ModuleReload));
}

#[test]
fn a_manual_run_on_a_usb_panel_ends_with_the_display_link_retrain_and_always_writes_detect_last() {
    let (sys, _) = usb_tree("linkretrain", Usb::default());
    put(&sys.join("class/drm/card0-DP-1/status"), "connected\n");
    put(&sys.join("class/drm/card0-DP-2/status"), "disconnected\n");
    put(&sys.join("class/drm/card0-HDMI-A-2/status"), "connected\n");
    assert_eq!(ladder(Bus::Usb, true).last(), Some(&Rung::LinkRetrain), "manual, USB: the link is the last step");
    assert!(!ladder(Bus::Usb, false).contains(&Rung::LinkRetrain), "the automatic run never blinks the screen");
    assert!(!ladder(Bus::I2c, true).contains(&Rung::LinkRetrain), "a tablet panel has no link to retrain");
    let f = Fake::new(None);
    let o = run_ladder(&f, &sys, true, w());
    assert_eq!(o.tried.last(), Some(&Rung::LinkRetrain), "{:?}", o.tried);
    let link: Vec<String> = f.wrote().into_iter().filter(|w| w.starts_with("status=")).collect();
    assert_eq!(link, vec!["status=off", "status=on", "status=detect"], "only the connected DisplayPort connector is touched, and detect is written last");
    // a write that is refused still ends with detect (the connector is never left forced) and says so
    let mut g = Fake::new(None);
    g.fail = Some("card0-DP-1/status");
    let r = super::link_retrain(&g, &sys);
    assert!(r.is_err());
    // nothing connected: a plain sentence, no write
    let none = root("linkretrain-none");
    put(&none.join("class/drm/card0-DP-1/status"), "disconnected\n");
    let h = Fake::new(None);
    assert_eq!(super::link_retrain(&h, &none), Err("no connected DisplayPort connector".to_string()));
    assert!(h.wrote().is_empty());
}
