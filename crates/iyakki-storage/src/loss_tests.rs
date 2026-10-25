//! The loss watcher with fakes only: no real device, mount or file of the machine is touched.

use super::*;
use crate::LABEL;
use std::cell::RefCell;

const UUID: &str = "11111111-2222-3333-4444-555555555555";

struct Fake {
    mounts: RefCell<String>,
    disk: RefCell<DiskState>,
    errors: RefCell<u64>,
    probes: RefCell<Vec<Probe>>,
    cands: RefCell<Vec<Candidate>>,
    up: RefCell<u64>,
}

fn mountinfo(dev: &str, mm: &str, ro: bool) -> String {
    let o = if ro { "ro" } else { "rw" };
    format!("36 25 {mm} / /var/lib/iyakki {o},nosuid,nodev,relatime shared:1 - ext4 /dev/{dev} {o}\n40 25 {mm} /home /home {o},nosuid,nodev,relatime - ext4 /dev/{dev} {o}\n")
}

fn cand(name: &str, mm: &str, uuid: &str, label: &str, mounted: bool) -> Candidate {
    Candidate { name: name.into(), majmin: mm.into(), uuid: uuid.into(), label: label.into(), mounted }
}

impl Fake {
    fn new() -> Fake {
        Fake { mounts: RefCell::new(mountinfo("sda3", "8:3", false)), disk: RefCell::new(DiskState::Running), errors: RefCell::new(0), probes: RefCell::new(Vec::new()), cands: RefCell::new(vec![cand("sda3", "8:3", UUID, LABEL, true)]), up: RefCell::new(100) }
    }
    fn gone(&self) {
        *self.disk.borrow_mut() = DiskState::Gone;
        self.cands.borrow_mut().clear();
    }
    fn returns(&self, name: &str, mm: &str, uuid: &str, label: &str) {
        self.cands.borrow_mut().push(cand(name, mm, uuid, label, false));
    }
}

impl Env for Fake {
    fn mountinfo(&self) -> String {
        self.mounts.borrow().clone()
    }
    fn disk_state(&self, disk: &str) -> DiskState {
        // the new node's disk is a different one: only the baseline disk is looked at
        assert_eq!(disk, "sda");
        *self.disk.borrow()
    }
    fn ext4_errors(&self, _dev: &str) -> u64 {
        *self.errors.borrow()
    }
    fn probe(&self) -> Probe {
        let mut p = self.probes.borrow_mut();
        if p.is_empty() { Probe::Ok } else { p.remove(0) }
    }
    fn candidates(&self) -> Vec<Candidate> {
        self.cands.borrow().clone()
    }
    fn identity(&self, dev: &str) -> Option<(String, String)> {
        (dev == "sda3").then(|| (UUID.into(), LABEL.into()))
    }
    fn majmin(&self, dev: &str) -> String {
        if dev == "sda3" { "8:3".into() } else { "8:19".into() }
    }
    fn uptime_secs(&self) -> u64 {
        *self.up.borrow()
    }
}

#[derive(Default)]
struct Acts {
    log: RefCell<Vec<String>>,
    fail_mount_stage: bool,
    fail_move: bool,
    fail_direct: bool,
    fail_bind: bool,
}

impl Actions for Acts {
    fn mount(&self, dev: &str, target: &str) -> Result<(), String> {
        self.log.borrow_mut().push(format!("mount {dev} {target}"));
        if (target == STAGE_DIR && self.fail_mount_stage) || (target == DATA_DIR && self.fail_direct) {
            return Err("EUCLEAN: journal replay failed".into());
        }
        Ok(())
    }
    fn lazy_unmount(&self, t: &str) -> Result<(), String> {
        self.log.borrow_mut().push(format!("umount {t}"));
        Ok(())
    }
    fn move_mount(&self, from: &str, to: &str) -> Result<(), String> {
        self.log.borrow_mut().push(format!("move {from} {to}"));
        if self.fail_move { Err("EINVAL".into()) } else { Ok(()) }
    }
    fn bind(&self, s: &str, t: &str) -> Result<(), String> {
        self.log.borrow_mut().push(format!("bind {s} {t}"));
        if self.fail_bind { Err("ENOENT".into()) } else { Ok(()) }
    }
    fn make_dirs(&self) -> Result<(), String> {
        self.log.borrow_mut().push("dirs".into());
        Ok(())
    }
    fn stop_components(&self) -> Vec<String> {
        self.log.borrow_mut().push("stop k3s".into());
        vec!["k3s".into()]
    }
    fn start_components(&self, stopped: &[String]) {
        self.log.borrow_mut().push(format!("start {}", stopped.join(",")));
    }
}

fn watching() -> (Watcher, Fake) {
    let f = Fake::new();
    let mut w = Watcher::new();
    assert!(w.learn(&f));
    (w, f)
}

fn lost_with_return() -> (Watcher, Fake) {
    let (mut w, f) = watching();
    f.gone();
    assert_eq!(w.tick(&f, 1000).unwrap().to, State::Lost);
    *f.mounts.borrow_mut() = mountinfo("sda3", "8:3", false); // the dead mount stays listed
    f.returns("sdb3", "8:19", UUID, LABEL);
    assert_eq!(w.tick(&f, 1500).unwrap().to, State::Reattachable);
    (w, f)
}

#[test]
fn a_healthy_drive_stays_healthy_and_learns_its_identity() {
    let (mut w, f) = watching();
    let b = w.baseline.clone().unwrap();
    assert_eq!((b.dev.as_str(), b.uuid.as_str(), b.label.as_str(), b.home_bound), ("sda3", UUID, LABEL, true));
    for t in 0..10 {
        assert!(w.tick(&f, t * 1000).is_none());
    }
    assert!(w.state.healthy() && w.ring.is_empty());
}

#[test]
fn nothing_is_watched_when_the_data_lives_in_ram() {
    let f = Fake::new();
    *f.mounts.borrow_mut() = "36 25 0:20 / /var/lib/iyakki rw - tmpfs tmpfs rw\n".into();
    let mut w = Watcher::new();
    assert!(!w.learn(&f));
    assert!(w.tick(&f, 0).is_none());
}

#[test]
fn a_vanished_disk_is_lost_at_once_with_a_ring_event() {
    let (mut w, f) = watching();
    *f.up.borrow_mut() = 3088;
    f.gone();
    let c = w.tick(&f, 1).unwrap();
    assert_eq!((c.from, c.to), (State::Healthy, State::Lost));
    assert_eq!(w.lost_at, Some(3088));
    assert!(w.ring.all()[0].text.contains("drive lost at uptime 3088 s"), "{:?}", w.ring.all());
    // repeated looks do not repeat the event
    w.tick(&f, 1000);
    assert_eq!(w.ring.len(), 1);
}

#[test]
fn an_offline_disk_counts_as_lost() {
    let (mut w, f) = watching();
    *f.disk.borrow_mut() = DiskState::Offline;
    assert_eq!(w.tick(&f, 1).unwrap().to, State::Lost);
}

#[test]
fn eio_needs_two_failed_probes_in_a_row() {
    let (mut w, f) = watching();
    *f.probes.borrow_mut() = vec![Probe::Failed(5), Probe::Ok, Probe::Failed(5), Probe::Failed(5)];
    assert!(w.tick(&f, 0).is_none(), "one failure is a hiccup");
    assert!(w.tick(&f, 1000).is_none());
    assert!(w.tick(&f, 2000).is_none());
    assert_eq!(w.tick(&f, 3000).unwrap().to, State::Errors);
}

#[test]
fn a_probe_that_times_out_is_an_error_and_never_blocks_the_look() {
    let (mut w, f) = watching();
    *f.probes.borrow_mut() = vec![Probe::TimedOut, Probe::TimedOut];
    w.tick(&f, 0);
    assert_eq!(w.tick(&f, 1000).unwrap().to, State::Errors);
    assert!(w.ring.all().iter().any(|e| e.text.contains("does not answer")));
}

#[test]
fn a_read_only_remount_is_reported() {
    let (mut w, f) = watching();
    *f.mounts.borrow_mut() = mountinfo("sda3", "8:3", true);
    assert_eq!(w.tick(&f, 0).unwrap().to, State::ReadOnly);
}

#[test]
fn new_ext4_errors_are_reported_but_old_ones_are_not() {
    let f = Fake::new();
    *f.errors.borrow_mut() = 4;
    let mut w = Watcher::new();
    w.learn(&f);
    assert!(w.tick(&f, 0).is_none(), "errors from before do not count");
    *f.errors.borrow_mut() = 5;
    assert_eq!(w.tick(&f, 1000).unwrap().to, State::Errors);
}

#[test]
fn the_same_uuid_on_a_new_node_is_reattachable_and_auto_waits_three_seconds() {
    let (mut w, _f) = lost_with_return();
    assert!(!w.auto_due(1500 + 2999));
    assert!(w.auto_due(1500 + 3000));
    assert!(w.can_reattach());
    assert!(w.ring.all().iter().any(|e| e.text.contains("sdb3 has the same UUID")));
    let _ = &mut w;
}

#[test]
fn a_different_uuid_is_never_reattached() {
    let (mut w, f) = watching();
    f.gone();
    w.tick(&f, 0);
    f.returns("sdb3", "8:19", "99999999-0000-0000-0000-000000000000", "OTHER");
    assert!(w.tick(&f, 1000).is_none());
    assert_eq!(w.state, State::Lost);
    assert!(!w.can_reattach());
    assert!(w.reattach(&f, &Acts::default(), 9000, true).is_err());
}

#[test]
fn the_right_label_with_another_uuid_is_refused_and_said_once() {
    let (mut w, f) = watching();
    f.gone();
    w.tick(&f, 0);
    f.returns("sdb3", "8:19", "99999999-0000-0000-0000-000000000000", LABEL);
    for t in 1..5 {
        w.tick(&f, t * 1000);
    }
    assert_eq!(w.state, State::Lost);
    let acts = Acts::default();
    assert!(w.reattach(&f, &acts, 99_000, true).is_err());
    assert!(acts.log.borrow().is_empty(), "nothing was mounted or unmounted");
    assert_eq!(w.ring.all().iter().filter(|e| e.text.contains("another UUID")).count(), 1);
}

#[test]
fn a_node_that_is_already_mounted_is_not_a_candidate() {
    let (mut w, f) = watching();
    f.gone();
    w.tick(&f, 0);
    f.cands.borrow_mut().push(cand("sdb3", "8:19", UUID, LABEL, true));
    assert!(w.tick(&f, 1000).is_none());
    assert_eq!(w.state, State::Lost);
}

#[test]
fn a_flapping_device_waits_and_the_timer_starts_over() {
    let (mut w, f) = lost_with_return();
    // the new node disappears again
    f.cands.borrow_mut().clear();
    assert_eq!(w.tick(&f, 2500).unwrap().to, State::Lost);
    assert!(!w.auto_due(10_000));
    // and comes back: three seconds count from the new appearance
    f.returns("sdc3", "8:35", UUID, LABEL);
    assert_eq!(w.tick(&f, 6000).unwrap().to, State::Reattachable);
    assert!(!w.auto_due(8999));
    assert!(w.auto_due(9000));
    // a node that changes number under us also restarts the timer
    f.cands.borrow_mut().clear();
    f.returns("sdd3", "8:51", UUID, LABEL);
    w.tick(&f, 9500);
    assert!(!w.auto_due(9501 + 100));
}

#[test]
fn reattach_follows_the_safe_order_and_ends_healthy() {
    let (mut w, f) = lost_with_return();
    let acts = Acts::default();
    // after the new mount the probe works
    w.reattach(&f, &acts, 5000, false).unwrap();
    let log = acts.log.borrow().clone();
    assert_eq!(log, ["stop k3s", "mount /dev/sdb3 /run/iyakki-reattach", "umount /var/lib/iyakki", "umount /home", "move /run/iyakki-reattach /var/lib/iyakki", "bind /var/lib/iyakki/home /home", "dirs", "start k3s"].map(String::from), "{log:?}");
    assert!(w.state.healthy());
    assert_eq!(w.reattached, 1);
    assert_eq!(w.baseline.as_ref().unwrap().dev, "sdb3");
    assert!(w.note.contains("reconnected"));
    assert!(w.ring.all().last().unwrap().text.contains("drive reconnected as sdb3"));
}

#[test]
fn nested_stale_mounts_go_first() {
    let (mut w, f) = lost_with_return();
    *f.mounts.borrow_mut() = format!("{}50 36 8:3 /x /var/lib/iyakki/sub rw - ext4 /dev/sda3 rw\n", mountinfo("sda3", "8:3", false));
    let acts = Acts::default();
    w.reattach(&f, &acts, 5000, true).unwrap();
    let um: Vec<String> = acts.log.borrow().iter().filter(|l| l.starts_with("umount")).cloned().collect();
    assert_eq!(um[0], "umount /var/lib/iyakki/sub");
    assert!(um.len() == 3);
}

#[test]
fn auto_reattach_refuses_before_the_device_is_steady_but_the_button_does_not() {
    let (mut w, f) = lost_with_return();
    let acts = Acts::default();
    assert!(w.reattach(&f, &acts, 1600, false).is_err());
    assert!(acts.log.borrow().is_empty());
    assert_eq!(w.state, State::Reattachable);
    assert!(w.reattach(&f, &acts, 1600, true).is_ok());
}

#[test]
fn a_failed_mount_changes_nothing_and_falls_back_to_the_banner() {
    let (mut w, f) = lost_with_return();
    let acts = Acts { fail_mount_stage: true, ..Default::default() };
    let e = w.reattach(&f, &acts, 9000, true).unwrap_err();
    assert!(e.contains("could not be mounted") && e.contains("journal replay failed"), "{e}");
    assert!(matches!(w.state, State::Failed(_)));
    assert!(!acts.log.borrow().iter().any(|l| l.starts_with("umount") || l.starts_with("move") || l.starts_with("bind")), "the stale mounts are kept: {:?}", acts.log.borrow());
    let b = banner(&Snapshot { state: w.state.clone(), ..Default::default() }).unwrap();
    assert!(b.buttons.contains(&Button::Restart) && b.buttons.contains(&Button::Check));
    // a failed state is not retried automatically
    assert!(!w.auto_due(99_999));
    assert!(w.tick(&f, 100_000).is_none());
}

#[test]
fn a_refused_move_falls_back_to_a_direct_mount_and_a_second_failure_shows_the_banner() {
    let (mut w, f) = lost_with_return();
    let acts = Acts { fail_move: true, ..Default::default() };
    w.reattach(&f, &acts, 9000, true).unwrap();
    assert!(acts.log.borrow().iter().any(|l| l == "mount /dev/sdb3 /var/lib/iyakki"));
    let (mut w2, f2) = lost_with_return();
    let acts2 = Acts { fail_move: true, fail_direct: true, ..Default::default() };
    assert!(w2.reattach(&f2, &acts2, 9000, true).is_err());
    assert!(matches!(w2.state, State::Failed(_)));
}

#[test]
fn a_home_bind_failure_and_a_failed_write_probe_are_reported() {
    let (mut w, f) = lost_with_return();
    assert!(w.reattach(&f, &Acts { fail_bind: true, ..Default::default() }, 9000, true).unwrap_err().contains("/home"));
    let (mut w, f) = lost_with_return();
    *f.probes.borrow_mut() = vec![Probe::Failed(5)];
    assert!(w.reattach(&f, &Acts::default(), 9000, true).unwrap_err().contains("writing still fails"));
}

#[test]
fn a_healthy_mount_is_never_moved() {
    let (mut w, f) = watching();
    f.returns("sdb3", "8:19", UUID, LABEL);
    assert!(w.reattach(&f, &Acts::default(), 0, true).is_err());
    assert!(w.state.healthy());
}

#[test]
fn the_button_with_no_drive_visible_says_to_check_the_cable() {
    let (mut w, f) = watching();
    f.gone();
    w.tick(&f, 0);
    let e = w.reattach(&f, &Acts::default(), 0, true).unwrap_err();
    assert!(e.contains("USB-C cable"));
    assert_eq!(w.state, State::Lost);
}

#[test]
fn a_hiccup_that_heals_on_the_same_device_goes_back_to_healthy() {
    let (mut w, f) = watching();
    *f.probes.borrow_mut() = vec![Probe::Failed(5), Probe::Failed(5)];
    w.tick(&f, 0);
    assert_eq!(w.tick(&f, 1000).unwrap().to, State::Errors);
    assert_eq!(w.tick(&f, 2000).unwrap().to, State::Healthy);
    assert!(w.ring.all().last().unwrap().text.contains("answers again"));
}

#[test]
fn the_ring_is_bounded_flushed_once_and_only_when_healthy() {
    let mut r = Ring::default();
    for i in 0..(RING_MAX + 50) {
        r.push(i as u64, format!("e{i}"));
    }
    assert_eq!(r.len(), RING_MAX);
    assert_eq!(r.all()[0].text, "e50");
    assert_eq!(r.take_unflushed().len(), RING_MAX);
    assert!(r.take_unflushed().is_empty());
    r.push(1, "new\nline");
    let e = r.take_unflushed();
    assert_eq!(e.len(), 1);
    assert!(!e[0].text.contains('\n'));
}

#[test]
fn the_ring_reaches_the_audit_hook_only_after_the_drive_is_back() {
    let _g = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut w, f) = lost_with_return();
    publish(&w, None, None);
    assert!(take_events_for_audit().is_empty(), "the audit log is on the dead drive");
    w.reattach(&f, &Acts::default(), 9000, true).unwrap();
    publish(&w, None, None);
    let ev = take_events_for_audit();
    assert!(ev.iter().any(|e| e.text.contains("drive lost")) && ev.iter().any(|e| e.text.contains("reconnected")), "{ev:?}");
    assert!(take_events_for_audit().is_empty(), "each event once");
    // leave the shared state clean
    publish(&Watcher::new(), None, None);
}

static GLOBAL: Mutex<()> = Mutex::new(());

#[test]
fn app_errors_are_reworded_only_while_the_drive_is_not_healthy() {
    let healthy = Snapshot::default();
    let lost = Snapshot { state: State::Lost, ..Default::default() };
    for m in ["Could not save the photo: Input/output error (os error 5)", "touch: I/O error", "This chat's work folder is missing: /home/admin/work/x", "Claude failed to load: /var/lib/iyakki/claude.json: No such file or directory"] {
        assert_eq!(reword_with(&healthy, m), m, "healthy: unchanged");
        assert!(reword_with(&lost, m).contains("drive is not available"), "{m}");
    }
    // unrelated failures stay as they are
    assert_eq!(reword_with(&lost, "The model returned an empty answer."), "The model returned an empty answer.");
    assert_eq!(reword_with(&lost, "could not load https://example.com"), "could not load https://example.com");
    assert!(reword_with(&Snapshot { state: State::ReadOnly, ..Default::default() }, "x: Read-only file system (os error 30)").contains("read-only"));
}

#[test]
fn explain_for_path_only_when_unhealthy_and_on_the_volume_or_a_dead_device_errno() {
    let lost = Snapshot { state: State::Lost, ..Default::default() };
    assert!(explain_for_path_with(&Snapshot::default(), Path::new("/home/admin/x"), Some(5)).is_none());
    assert!(explain_for_path_with(&lost, Path::new("/home/admin/Pictures/a.jpg"), Some(2)).is_some());
    assert!(explain_for_path_with(&lost, Path::new("/tmp/x"), Some(2)).is_none());
    assert!(explain_for_path_with(&lost, Path::new("/tmp/x"), Some(5)).is_some());
}

#[test]
fn the_banner_says_it_in_plain_words_and_has_the_right_buttons() {
    assert!(banner(&Snapshot::default()).is_none());
    let b = banner(&Snapshot { state: State::Lost, ..Default::default() }).unwrap();
    assert!(b.body.contains("disconnected for a moment") && b.body.contains("USB-C cable") && b.body.contains("may be lost"));
    assert_eq!(b.buttons, [Button::Reconnect, Button::Restart]);
    assert!(b.sticky);
    assert!(banner(&Snapshot { state: State::Reconnecting, ..Default::default() }).unwrap().buttons.is_empty());
    let f = banner(&Snapshot { state: State::Failed("boom".into()), check: "e2fsck: clean".into(), ..Default::default() }).unwrap();
    assert!(f.body.contains("boom") && f.body.contains("e2fsck: clean"));
}

#[test]
fn super_block_parsing_and_node_names() {
    let mut b = vec![0u8; 2048];
    b[1024 + 0x38] = 0x53;
    b[1024 + 0x39] = 0xEF;
    for i in 0..16 {
        b[1024 + 0x68 + i] = i as u8 + 1;
    }
    b[1024 + 0x78..1024 + 0x78 + 11].copy_from_slice(b"IYAKKI-DATA");
    assert_eq!(parse_super(&b), Some(("01020304-0506-0708-090a-0b0c0d0e0f10".into(), "IYAKKI-DATA".into())));
    assert_eq!(parse_super(&[0u8; 2048]), None);
    assert_eq!(parse_super(&[0u8; 10]), None);
    assert_eq!((disk_of("sda3").as_str(), disk_of("nvme0n1p3").as_str(), disk_of("mmcblk0p2").as_str(), disk_of("sdb").as_str()), ("sda", "nvme0n1", "mmcblk0", "sdb"));
    assert_eq!(node_name("/dev/sdb3"), "sdb3");
}

#[test]
fn mountinfo_with_spaces_and_optional_fields_parses() {
    let t = parse_mountinfo("36 25 8:3 / /mnt/a\\040b rw,relatime shared:1 master:2 - ext4 /dev/sda3 rw,errors=remount-ro\nbroken line\n");
    assert_eq!(t.len(), 1);
    assert_eq!((t[0].point.as_str(), t[0].source.as_str(), t[0].read_only()), ("/mnt/a b", "/dev/sda3", false));
}

// ---- the USB link, from a made-up /sys

fn put(root: &Path, rel: &str, v: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, format!("{v}\n")).unwrap();
}

fn fake_sys(tag: &str, port_path: &[&str], speed: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("iyakki-loss-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut dev = root.join("devices/pci0000:00/0000:00:14.0/usb2");
    for p in port_path {
        dev = dev.join(p);
    }
    let name = port_path.last().unwrap();
    put(&dev, "speed", speed);
    put(&dev, "idVendor", "04e8");
    put(&dev, "idProduct", "4001");
    put(&dev, "product", "PSSD T7");
    put(&dev, "manufacturer", "Samsung");
    put(&dev, "power/control", "auto");
    let iface = dev.join(format!("{name}:1.0"));
    std::fs::create_dir_all(iface.join("host6/target6:0:0/6:0:0:0/block/sda")).unwrap();
    std::fs::create_dir_all(root.join("bus/usb/drivers/uas")).unwrap();
    std::os::unix::fs::symlink(root.join("bus/usb/drivers/uas"), iface.join("driver")).unwrap();
    std::fs::create_dir_all(root.join("block")).unwrap();
    std::os::unix::fs::symlink(iface.join("host6/target6:0:0/6:0:0:0/block/sda"), root.join("block/sda")).unwrap();
    root
}

#[test]
fn the_usb_link_of_a_directly_plugged_ssd() {
    let sys = fake_sys("direct", &["2-2"], "10000");
    let l = usb_link(&sys, "sda").unwrap();
    assert_eq!((l.port.as_str(), l.speed_mbps, l.behind_hub, l.driver.as_str(), l.id.as_str()), ("2-2", 10000, false, "uas", "04e8:4001"));
    assert_eq!(l.speed_text(), "SuperSpeed+ 10 Gbit/s");
    assert!(link_advice(Some(&l)).is_empty());
    assert!(link_line(Some(&l)).contains("USB port 2-2, directly on the computer at SuperSpeed+ 10 Gbit/s with driver uas"));
    // power: autosuspend off for the device
    let done = keep_usb_awake(&sys, "sda");
    assert_eq!(done.len(), 1);
    assert_eq!(std::fs::read_to_string(sys.join("devices/pci0000:00/0000:00:14.0/usb2/2-2/power/control")).unwrap(), "on");
    let _ = std::fs::remove_dir_all(&sys);
}

#[test]
fn a_slow_link_behind_a_busy_hub_is_warned_about() {
    let sys = fake_sys("hub", &["2-1", "2-1.3"], "480");
    // another device on the same hub, and the hub itself is class 09
    put(&sys, "devices/pci0000:00/0000:00:14.0/usb2/2-1/2-1.4/bDeviceClass", "00");
    put(&sys, "devices/pci0000:00/0000:00:14.0/usb2/2-1/bDeviceClass", "09");
    let l = usb_link(&sys, "sda").unwrap();
    assert!(l.behind_hub && l.slow());
    assert_eq!(l.hub_siblings, 1);
    assert_eq!(l.speed_text(), "High-speed 480 Mbit/s");
    let a = link_advice(Some(&l));
    assert!(a.contains("USB 2 speed") && a.contains("behind a USB hub") && a.contains("1 other"), "{a}");
    // the hub and the device get runtime PM "on"
    assert_eq!(keep_usb_awake(&sys, "sda").len(), 1, "only directories that have power/control");
    let _ = std::fs::remove_dir_all(&sys);
}

#[test]
fn a_disk_that_is_not_usb_has_no_link() {
    let root = std::env::temp_dir().join(format!("iyakki-loss-{}-nvme", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("devices/pci0000:00/nvme/nvme0/nvme0n1")).unwrap();
    std::fs::create_dir_all(root.join("block")).unwrap();
    std::os::unix::fs::symlink(root.join("devices/pci0000:00/nvme/nvme0/nvme0n1"), root.join("block/nvme0n1")).unwrap();
    assert!(usb_link(&root, "nvme0n1").is_none());
    assert!(usb_link(&root, "missing").is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_compatible_usb_mode_quirk_is_built_from_the_drive_id_only() {
    assert_eq!(compat_quirk("04E8:4001").as_deref(), Some("usb-storage.quirks=04e8:4001:u"));
    assert_eq!(compat_quirk("zz:1"), None);
    assert_eq!(compat_quirk("04e8:4001; reboot"), None);
    assert!(compat_file_text("usb-storage.quirks=04e8:4001:u").contains("set iyakki_usbq=\"usb-storage.quirks=04e8:4001:u\""));
    assert!(compat_active("quiet usb-storage.quirks=04e8:4001:u"));
    assert!(!compat_active("quiet"));
}
