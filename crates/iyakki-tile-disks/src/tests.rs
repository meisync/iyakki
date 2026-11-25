//! The DISKS tile against a fake port and a canned engine: no drive, no core, no shell (the shell's own tests run the tile on the core's
//! engine over a made-up machine, `iyakki-apps::apps::disks_tile_tests`).

use super::*;
use iyakki_disks_model::{Fs, MountFlags, Mounted, Transport};
use iyakki_uikit::gfx::{rgb, Gfx};
use std::sync::atomic::{AtomicUsize, Ordering};

fn part(name: &str, n: u32, mounted: bool) -> Part {
    Part {
        name: name.into(),
        path: format!("/dev/{name}"),
        number: n,
        start_bytes: 1 << 20,
        size_bytes: 8_000_000_000,
        fs: Fs::Ext4,
        label: "STICK".into(),
        uuid: String::new(),
        mountpoints: if mounted { vec!["/home/ada/media/STICK".into()] } else { vec![] },
        space: None,
        mounted_ro: false,
        swap_in_use: false,
        system: false,
        type_name: String::new(),
        crypt: None,
        unlocked: false,
    }
}

fn stick() -> Disk {
    Disk {
        name: "sdb".into(),
        path: "/dev/sdb".into(),
        vendor: "Acme".into(),
        model: "Stick".into(),
        serial: "SER12345".into(),
        size_bytes: 8_000_000_000,
        sector: 512,
        removable: true,
        transport: Transport::Usb,
        read_only: false,
        table: Table::Gpt,
        table_notes: vec![],
        parts: vec![part("sdb1", 1, false)],
        system: false,
        system_why: String::new(),
        holders: vec![],
    }
}

/// A canned engine: one USB stick; a mount is recorded and answered.
struct Canned {
    mounts: AtomicUsize,
}

impl DiskEngine for Canned {
    fn scan(&self) -> Snapshot {
        Snapshot { disks: vec![stick()], support: vec![] }
    }
    fn fingerprint(&self) -> u64 {
        7
    }
    fn plan_mount(&self, _s: &Snapshot, _w: &Actor, part: &str, o: &MountOpts) -> Result<MountPlan, String> {
        Ok(MountPlan { dev: format!("/dev/{part}"), target: "/home/ada/media/STICK".into(), fstype: "ext4", flags: MountFlags { rdonly: o.read_only, ..Default::default() }, data: String::new(), forced_ro: None })
    }
    fn mount(&self, _w: &Actor, _p: &str, _o: &MountOpts, audit: iyakki_disks_model::AuditFn) -> Result<Mounted, String> {
        self.mounts.fetch_add(1, Ordering::SeqCst);
        audit("disk.mount", "drive=sdb");
        Ok(Mounted { target: "/home/ada/media/STICK".into(), read_only: false, notes: vec![] })
    }
}

struct Port {
    engine: Arc<Canned>,
    role: Role,
    audit: Arc<Mutex<Vec<(String, String)>>>,
    unlock: Mutex<Option<String>>,
}

impl Port {
    fn new(role: Role) -> Port {
        Port { engine: Arc::new(Canned { mounts: AtomicUsize::new(0) }), role, audit: Arc::new(Mutex::new(vec![])), unlock: Mutex::new(None) }
    }
}

impl DisksPort for Port {
    fn engine(&self) -> Arc<dyn DiskEngine> {
        self.engine.clone()
    }
    fn who(&self) -> Actor {
        Actor { user: "ada".into(), role: self.role, uid: 1000, gid: 1000, home: "/home/ada".into() }
    }
    fn audit_sink(&self) -> Arc<dyn Fn(&str, &str) + Send + Sync> {
        let log = self.audit.clone();
        Arc::new(move |a: &str, d: &str| log.lock().unwrap().push((a.to_string(), d.to_string())))
    }
    fn take_unlock_request(&self) -> Option<String> {
        self.unlock.lock().unwrap().take()
    }
    fn take_format_request(&self) -> Option<FormatRequest> {
        None
    }
}

#[test]
fn sizes_are_parsed_in_powers_of_1024_and_bad_ones_are_explained() {
    assert_eq!(parse_size("").unwrap(), None);
    assert_eq!(parse_size("8 GB").unwrap(), Some(8 * 1024 * 1024 * 1024));
    assert_eq!(parse_size("500MB").unwrap(), Some(500 * 1024 * 1024));
    assert_eq!(parse_size("2 GiB").unwrap(), Some(2 * 1024 * 1024 * 1024));
    assert_eq!(parse_size("1.5 g").unwrap(), Some(1610612736));
    assert!(parse_size("abc").unwrap_err().contains("is not a size"));
    assert!(parse_size("5 PB").unwrap_err().contains("unit"));
    assert!(parse_size("0.1 MB").unwrap_err().contains("at least 1 MB"));
}

#[test]
fn sizes_read_like_the_drive_makers_write_them() {
    assert_eq!(human_bytes(512), "512 B");
    assert_eq!(human_bytes(8_000_000_000), "8.0 GB");
    assert_eq!(human_bytes(1_500), "1.5 KB");
}

#[test]
fn the_tile_lists_the_drive_the_port_engine_shows_and_plans_its_mount() {
    let port = Port::new(Role::Admin);
    let t = DisksTile::new(&port, true);
    assert!(t.loaded, "an inline tile has scanned by the time it is built");
    assert_eq!(t.snap.disks.len(), 1);
    assert_eq!(t.finger, 7);
    assert_eq!(t.plan.as_ref().unwrap().target, "/home/ada/media/STICK");
    assert_eq!(t.title(), "Disks");
}

#[test]
fn mounting_runs_on_the_engine_and_the_audit_line_goes_through_the_port() {
    let port = Port::new(Role::User);
    let mut t = DisksTile::new(&port, true);
    t.handle_event(&Event::Tab(T_MOUNT), &port);
    assert_eq!(port.engine.mounts.load(Ordering::SeqCst), 1);
    assert_eq!(port.audit.lock().unwrap().as_slice(), &[("disk.mount".to_string(), "drive=sdb".to_string())]);
    let (m, ok) = t.msg.clone().expect("the answer is shown");
    assert!(ok && m.contains("STICK"), "{m}");
}

#[test]
fn only_an_administrator_sees_the_partition_manager() {
    let user = Port::new(Role::User);
    let mut t = DisksTile::new(&user, true);
    t.handle_event(&Event::Tab(T_ADV), &user);
    assert!(!t.manager, "a user never gets the manager");
    let admin = Port::new(Role::Admin);
    let mut t = DisksTile::new(&admin, true);
    t.handle_event(&Event::Tab(T_ADV), &admin);
    assert!(t.manager);
}

#[test]
fn the_assistants_unlock_request_opens_the_password_box_once_the_drives_are_listed() {
    let port = Port::new(Role::Admin);
    *port.unlock.lock().unwrap() = Some("sdb1".into());
    let mut t = DisksTile::new(&port, true);
    t.tick();
    // sdb1 is not encrypted in the canned machine: the box does not open, the request is used up
    assert!(port.unlock.lock().unwrap().is_none());
    assert!(matches!(t.modal, Modal::None));
}

#[test]
fn escape_goes_back_from_the_list_and_the_tile_draws_at_two_sizes_without_panic() {
    let port = Port::new(Role::Admin);
    let mut t = DisksTile::new(&port, true);
    for (w, h, s) in [(1280u32, 720u32, 1.0f32), (320, 480, 0.5), (1920, 1080, 2.25)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(8, 10, 22));
        g.clear_hits();
        t.render(&mut g, R::new(0.0, 0.0, w as f32, h as f32), s, 1);
    }
    assert_eq!(t.handle_event(&Event::Key(Key::Esc), &port), Flow::Back);
}
