//! Settings → Install to disk on a made-up machine: a boot stick `vda` (an install template), an empty target `vdb` and a mounted
//! `vdc`, all plain files in a temporary directory, so nothing real is ever opened.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use std::path::{Path, PathBuf};

struct Offline;
impl iyakki_agent::iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_agent::iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}
fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}
fn press(a: &mut SettingsApp, cx: &AppCx, c: char) {
    key(a, cx, Key::Char(c));
}
fn typ(a: &mut SettingsApp, cx: &AppCx, s: &str) {
    s.chars().for_each(|c| press(a, cx, c));
}
fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}
fn settle(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..4000 {
        a.handle(&Event::Tick, cx);
        if !a.is_busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("job did not finish");
}
fn goto_install(a: &mut SettingsApp, _cx: &AppCx) {
    a.go_to(Tab::Install);
}
fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}
fn overlaps(a: R, b: R) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

fn template(path: &Path) -> Vec<u8> {
    let mut t = vec![0u8; 8 * 1024 * 1024];
    for (i, b) in t[..440].iter_mut().enumerate() {
        *b = (i % 251) as u8 + 1;
    }
    t[440..444].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
    // like GRUB: the boot code names the core image's sector (1); its first sector ends with a block list {2, 10 sectors, 0x820}
    t[0x5C..0x64].copy_from_slice(&1u64.to_le_bytes());
    t[512 + 500..512 + 508].copy_from_slice(&2u64.to_le_bytes());
    t[512 + 508..512 + 510].copy_from_slice(&10u16.to_le_bytes());
    t[512 + 510..512 + 512].copy_from_slice(&0x820u16.to_le_bytes());
    let (p1, p2) = (446, 462);
    t[p1] = 0x80;
    t[p1 + 4] = 0xEF;
    t[p1 + 8..p1 + 12].copy_from_slice(&2048u32.to_le_bytes());
    t[p1 + 12..p1 + 16].copy_from_slice(&8192u32.to_le_bytes());
    t[p2 + 4] = 0x83;
    t[p2 + 8..p2 + 12].copy_from_slice(&10240u32.to_le_bytes());
    t[p2 + 12..p2 + 16].copy_from_slice(&2048u32.to_le_bytes());
    t[510] = 0x55;
    t[511] = 0xAA;
    let b = 2048 * 512;
    t[b + 71..b + 82].copy_from_slice(b"IYAKKI-BOOT");
    t[b + 510] = 0x55;
    t[b + 511] = 0xAA;
    t[b + 4096..b + 4200].iter_mut().enumerate().for_each(|(i, x)| *x = (i * 3 % 256) as u8);
    std::fs::write(path, &t).unwrap();
    t
}

fn add_disk(root: &Path, name: &str, sectors: u64) {
    let base = root.join(format!("sys/devices/pci0000:00/virtio/block/{name}"));
    for (f, v) in [("size", sectors.to_string()), ("removable", "0".into()), ("queue/logical_block_size", "512".into()), ("device/serial", format!("SER-{name}")), ("device/model", "Virtual Disk".into())] {
        let p = base.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("{v}\n")).unwrap();
    }
    std::fs::create_dir_all(root.join("sys/block")).unwrap();
    std::os::unix::fs::symlink(&base, root.join("sys/block").join(name)).unwrap();
}

struct Machine {
    root: PathBuf,
    env: InstallEnv,
    tpl: Vec<u8>,
}

fn machine(tag: &str, with_boot_disk: bool) -> Machine {
    let root = temp_dir(&format!("{tag}-inst"));
    std::fs::create_dir_all(root.join("dev")).unwrap();
    std::fs::create_dir_all(root.join("proc")).unwrap();
    let mut tpl = vec![];
    if with_boot_disk {
        add_disk(&root, "vda", 2 * 1024 * 1024 * 2); // 2 GiB stick
        tpl = template(&root.join("dev/vda"));
    }
    add_disk(&root, "vdb", 16_000_000); // 8.2 GB, sparse
    std::fs::File::create(root.join("dev/vdb")).unwrap().set_len(1 << 20).unwrap();
    add_disk(&root, "vdc", 16_000_000);
    std::fs::File::create(root.join("dev/vdc")).unwrap().set_len(1 << 20).unwrap();
    std::fs::write(root.join("proc/mounts"), "/dev/vdc1 /mnt/data ext4 rw 0 0\n").unwrap();
    std::fs::write(root.join("proc/swaps"), "Filename Type Size Used Priority\n").unwrap();
    let env = InstallEnv { sys: root.join("sys"), dev: root.join("dev"), mounts: root.join("proc/mounts"), swaps: root.join("proc/swaps"), template: None, uefi: false };
    Machine { root, env, tpl }
}

fn select(a: &mut SettingsApp, cx: &AppCx, name: &str) {
    let i = a.inst_candidates().iter().position(|d| d.drive.name == name).unwrap_or_else(|| panic!("{name} is not listed"));
    a.handle(&Event::Row(i), cx);
}

#[test]
fn the_page_lists_disks_but_not_the_stick_and_says_why_one_is_refused() {
    let m = machine("inst-list", true);
    let cx = admin("inst-list");
    let mut a = SettingsApp::new(&cx).with_install(m.env.clone());
    assert!(a.tabs().contains(&Tab::Install));
    goto_install(&mut a, &cx);
    let names: Vec<String> = a.inst_candidates().iter().map(|d| d.drive.name.clone()).collect();
    assert_eq!(names, ["vdb", "vdc"], "the stick we copy from is never offered");
    let vdc = a.inst_candidates().into_iter().find(|d| d.drive.name == "vdc").cloned().unwrap();
    assert!(a.inst_refusal(&vdc).unwrap().contains("mounted"), "{:?}", a.inst_refusal(&vdc));
    let vdb = a.inst_candidates().into_iter().find(|d| d.drive.name == "vdb").cloned().unwrap();
    assert_eq!(a.inst_refusal(&vdb), None);
    select(&mut a, &cx, "vdc");
    press(&mut a, &cx, 'i');
    assert!(!a.has_modal() && status(&a).contains("mounted"), "{}", status(&a));
    let _ = draw(&mut a, &cx, 1280, 720);
}

#[test]
fn the_page_is_absent_without_a_boot_stick_and_for_ordinary_users() {
    let m = machine("inst-none", false);
    let cx = admin("inst-none");
    let a = SettingsApp::new(&cx).with_install(m.env.clone());
    assert!(!a.tabs().contains(&Tab::Install), "nothing to copy from");
    let m2 = machine("inst-user", true);
    let mut user = admin("inst-user");
    user.session.user = "amy".into();
    user.session.role = iyakki_sec::policy::Role::User;
    user.session.policy = iyakki_sec::policy::Role::User.template();
    assert!(!SettingsApp::new(&user).tabs().contains(&Tab::Install));
    let _ = m2;
}

#[test]
fn the_plan_can_be_shown_without_writing_anything() {
    let m = machine("inst-plan", true);
    let cx = admin("inst-plan");
    let mut a = SettingsApp::new(&cx).with_install(m.env.clone());
    goto_install(&mut a, &cx);
    select(&mut a, &cx, "vdb");
    press(&mut a, &cx, 'p');
    let text = a.inst.plan.lines.iter().map(|l| l.0.clone()).collect::<Vec<_>>().join("\n");
    assert!(text.contains("erasing EVERYTHING") && text.contains("IYAKKI-DATA") && text.contains("read everything back"), "{text}");
    assert_eq!(std::fs::metadata(m.root.join("dev/vdb")).unwrap().len(), 1 << 20, "nothing written, not even resized");
    assert!(std::fs::read(m.root.join("dev/vdb")).unwrap().iter().all(|b| *b == 0));
}

#[test]
fn installing_needs_the_typed_name_and_size_then_writes_the_disk_with_the_choices() {
    let m = machine("inst-do", true);
    let cx = admin("inst-do");
    let mut a = SettingsApp::new(&cx).with_install(m.env.clone());
    goto_install(&mut a, &cx);
    select(&mut a, &cx, "vdb");
    press(&mut a, &cx, 'i');
    assert!(a.has_modal());
    // fields: zram, swap file, host name, name, size. The first three keep their defaults except zram and the host name.
    for _ in 0..8 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "2G,lz4");
    key(&mut a, &cx, Key::Enter); // swap file: keep "off"
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "testbox");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "vdb");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "wrong size");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "a wrong size keeps the question open");
    assert!(std::fs::read(m.root.join("dev/vdb")).unwrap().iter().all(|b| *b == 0), "nothing is written before the confirmation is right");
    for _ in 0..12 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "8.2 GB");
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal(), "{}", status(&a));
    settle(&mut a, &cx);
    assert!(status(&a).contains("IYAKKI is installed on"), "{}", status(&a));
    // only the first part of the disk is read: the install makes the file as long as the drive (8.2 GB, sparse), and reading all of it would take 8.2 GB of memory
    let disk = {
        use std::io::Read;
        let mut v = Vec::new();
        std::fs::File::open(m.root.join("dev/vdb")).unwrap().take(10240 * 512 + 65536).read_to_end(&mut v).unwrap();
        v
    };
    let copy = 10240 * 512;
    assert_eq!(&disk[512..copy], &m.tpl[512..copy], "the boot area is the stick's");
    assert_eq!(&disk[..440], &m.tpl[..440]);
    let part = &disk[10240 * 512..10240 * 512 + 65536];
    assert_eq!(iyakki_storage::classify(part), iyakki_storage::Content::Pending);
    let f = m.root.join("part.bin");
    std::fs::write(&f, part).unwrap();
    let conf = iyakki_storage::read_pending_conf(&f).unwrap();
    assert!(conf.contains("zram=2G,lz4") && conf.contains("hostname=testbox") && conf.contains("swapfile=off"), "{conf}");
    assert!(std::fs::read_to_string(cx.audit.path()).unwrap_or_default().contains("install.disk"));
    // the stick itself and the mounted disk were never opened for writing
    assert_eq!(std::fs::read(m.root.join("dev/vda")).unwrap(), m.tpl);
    assert_eq!(std::fs::metadata(m.root.join("dev/vdc")).unwrap().len(), 1 << 20);
}

#[test]
fn the_partition_table_can_be_chosen_and_the_plan_and_the_disk_follow() {
    let m = machine("inst-gpt", true);
    let cx = admin("inst-gpt");
    let mut a = SettingsApp::new(&cx).with_install(m.env.clone());
    goto_install(&mut a, &cx);
    select(&mut a, &cx, "vdb");
    // Auto on a BIOS machine with a small 512-byte disk is MBR; one press of t makes it GPT, two MBR, three Auto again
    press(&mut a, &cx, 'p');
    let text = |a: &SettingsApp| a.inst.plan.lines.iter().map(|l| l.0.clone()).collect::<Vec<_>>().join("\n");
    assert!(text(&a).contains("Partition table: MBR (auto: MBR keeps old BIOS-only PCs working)"), "{}", text(&a));
    press(&mut a, &cx, 't');
    assert!(status(&a).contains("Partition table: GPT") && status(&a).contains("BIOS boot partition"), "{}", status(&a));
    assert!(a.inst.plan.lines.is_empty(), "a plan shown for another table is cleared");
    press(&mut a, &cx, 'p');
    let t = text(&a);
    for want in ["Partition table: GPT (chosen: GPT)", "BIOS boot", "EFI System Partition", "IYAKKI-BOOT", "IYAKKI-DATA", "check both GPT copies"] {
        assert!(t.contains(want), "{want} in {t}");
    }
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Table: GPT"), "the button names the choice");
    // the UEFI case: Auto turns into GPT by itself
    press(&mut a, &cx, 't');
    press(&mut a, &cx, 't');
    assert_eq!(a.inst.table, iyakki_usb::install::TableChoice::Auto);
    let mut env = m.env.clone();
    env.uefi = true;
    let mut b = SettingsApp::new(&cx).with_install(env);
    goto_install(&mut b, &cx);
    select(&mut b, &cx, "vdb");
    press(&mut b, &cx, 'p');
    assert!(text(&b).contains("Partition table: GPT (auto: this computer was started through UEFI)"), "{}", text(&b));
    // installing with GPT chosen writes the GPT
    press(&mut a, &cx, 't');
    press(&mut a, &cx, 'i');
    assert!(a.has_modal());
    for _ in 0..8 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "auto");
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "vdb");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "8.2 GB");
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal(), "{}", status(&a));
    settle(&mut a, &cx);
    assert!(status(&a).contains("IYAKKI is installed on"), "{}", status(&a));
    let f = m.root.join("dev/vdb");
    let size = 16_000_000u64 * 512;
    let mut h = std::fs::File::open(&f).unwrap();
    let p = iyakki_storage::gpt::read_gpt(&mut h, 512, size / 512).unwrap();
    assert!(p.primary_ok && p.backup_ok && p.problems.is_empty(), "{:?}", p.problems);
    assert_eq!(p.table.entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["IYAKKI-BIOS", "IYAKKI-BOOT", "IYAKKI-DATA"]);
    assert!(std::fs::read_to_string(cx.audit.path()).unwrap_or_default().contains("table=GPT"));
    assert_eq!(std::fs::read(m.root.join("dev/vda")).unwrap(), m.tpl, "the stick was never written");
}

#[test]
fn geometry_and_click_paths_hold_at_four_sizes() {
    let m = machine("inst-geo", true);
    let cx = admin("inst-geo");
    let mut a = SettingsApp::new(&cx).with_install(m.env.clone());
    goto_install(&mut a, &cx);
    press(&mut a, &cx, 'p');
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let g = draw(&mut a, &cx, w, h);
        let sc = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
        let keys: Vec<(char, R)> = g.hits.iter().filter_map(|x| if let Event::Key(Key::Char(c)) = x.ev { Some((c, x.rect)) } else { None }).collect();
        for c in ['i', 'p', 't', 'r', 'b'] {
            assert!(keys.iter().any(|k| k.0 == c), "{w}x{h}: button {c}");
        }
        for (i, (ca, ra)) in keys.iter().enumerate() {
            assert!(ra.x >= 0.0 && ra.x + ra.w <= w as f32 && ra.y + ra.h <= h as f32 - 28.0 * sc, "{w}x{h}: {ca} inside and clear of the status line");
            for (cb, rb) in keys.iter().skip(i + 1) {
                assert!(!overlaps(*ra, *rb), "{w}x{h}: {ca} and {cb} overlap");
            }
            for r in g.hits.iter().filter(|h| matches!(h.ev, Event::Row(_))) {
                assert!(!overlaps(*ra, r.rect), "{w}x{h}: button {ca} covers a disk row");
            }
        }
        let rows: Vec<R> = g.hits.iter().filter(|h| matches!(h.ev, Event::Row(_))).map(|h| h.rect).collect();
        assert!(!rows.is_empty(), "{w}x{h}: at least one disk is shown");
        for r in &rows {
            assert!(r.x + r.w <= w as f32 && r.y + r.h <= h as f32, "{w}x{h}: row inside the screen");
        }
    }
    let g = draw(&mut a, &cx, 1280, 720);
    let ev = g.hits.iter().find(|h| h.label == "Install…").unwrap().ev.clone();
    a.handle(&ev, &cx);
    assert!(a.has_modal(), "clicking Install opens the questions for the selected disk");
}

#[test]
fn the_disk_the_system_runs_from_is_the_source_even_when_a_removable_iyakki_disk_is_plugged_in() {
    let m = "/dev/nvme0n1p2 /.stage1/boot vfat rw 0 0\n/dev/nvme0n1p3 /var/lib/iyakki ext4 rw 0 0\n";
    assert_eq!(super::install_page::running_disk(m, &["sda", "nvme0n1"]).as_deref(), Some("nvme0n1"));
    let m = "/dev/sda2 /.stage1/boot vfat rw 0 0\n";
    assert_eq!(super::install_page::running_disk(m, &["sda", "sdaa", "nvme0n1"]).as_deref(), Some("sda"));
    assert_eq!(super::install_page::running_disk("tmpfs /tmp tmpfs rw 0 0\n", &["sda"]), None);
}
