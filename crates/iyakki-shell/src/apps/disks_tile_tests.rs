//! The DISKS tile on a made-up machine (`iyakki_agent::disks_fake`): drawing at every size and in panes, keys, roles, the partition
//! manager's confirmations, hot-plug. Nothing here sees a real drive.

use std::sync::Arc;

use iyakki_agent::disks_fake::{blank_stick, machine, Fake};
use iyakki_sec::policy::{Policy, Role};

use super::disks_tile::*;
use super::tests::cx_with_admin_store;
use super::{App, AppCx, Flow};
use crate::gfx::{rgb, Gfx};
use crate::ui::{Event, Key, R};

struct Fx {
    app: DisksApp,
    cx: AppCx,
    f: Arc<Fake>,
    g: Gfx,
    area: R,
    s: f32,
    tick: u64,
}

fn fixture(tag: &str, role: Role, w: u32, h: u32, s: f32) -> Fx {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let mut cx = cx_with_admin_store(&format!("{tag}-{}", N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    cx.session.role = role;
    cx.session.policy = Policy::resolve(role, &Default::default());
    let f = machine();
    blank_stick(&f);
    let mut app = DisksApp::with_engine(&cx, Arc::new(f.disks()), true);
    app.poll_every = 1;
    // the tests do not wait five seconds; the countdown has its own tests
    app.cancel_window = std::time::Duration::ZERO;
    Fx { app, cx, f, g: Gfx::new(w, h), area: R::new(0.0, 0.0, w as f32, h as f32), s, tick: 0 }
}

fn admin() -> Fx {
    fixture("disks", Role::Admin, 1280, 720, 1.0)
}

impl Fx {
    fn draw(&mut self) {
        self.tick += 1;
        self.g.clear(rgb(8, 10, 22));
        self.g.clear_hits();
        self.app.render(&mut self.g, self.area, self.s, &self.cx, self.tick);
    }

    fn key(&mut self, k: Key) {
        self.app.handle(&Event::Key(k), &self.cx);
        self.draw();
    }

    fn chr(&mut self, c: char) {
        self.key(Key::Char(c));
    }

    fn type_text(&mut self, t: &str) {
        for c in t.chars() {
            self.chr(c);
        }
    }

    fn ev(&mut self, e: Event) -> Flow {
        let r = self.app.handle(&e, &self.cx);
        self.draw();
        r
    }

    fn pick(&mut self, disk: &str) {
        let i = self.app.snap.disks.iter().position(|d| d.name == disk).unwrap_or_else(|| panic!("no drive {disk}"));
        self.ev(Event::Row(i));
        self.draw();
    }

    fn has_hit(&self, ev: &Event) -> bool {
        self.g.hits.iter().any(|h| &h.ev == ev)
    }

    fn msg(&self) -> String {
        self.app.msg.as_ref().map(|m| m.0.clone()).unwrap_or_default()
    }

    fn audit(&self) -> String {
        std::fs::read_to_string(self.cx.audit.path()).unwrap_or_default()
    }
}

/// Every click region is inside the area; no two cross without one holding the other (a button on a row is fine).
fn assert_hits_sane(f: &Fx, what: &str) {
    let a = f.area;
    let hits = &f.g.hits;
    for h in hits {
        let r = h.rect;
        assert!(r.w > 0.0 && r.h > 0.0, "{what}: {:?} is empty", h.label);
        assert!(r.x >= a.x - 0.5 && r.y >= a.y - 0.5 && r.x + r.w <= a.x + a.w + 0.5 && r.y + r.h <= a.y + a.h + 0.5, "{what}: {:?} {:?} leaves {:?}", h.label, r, a);
    }
    for (i, p) in hits.iter().enumerate() {
        for q in &hits[i + 1..] {
            let (x0, y0) = (p.rect.x.max(q.rect.x), p.rect.y.max(q.rect.y));
            let (x1, y1) = ((p.rect.x + p.rect.w).min(q.rect.x + q.rect.w), (p.rect.y + p.rect.h).min(q.rect.y + q.rect.h));
            if x1 - x0 > 1.0 && y1 - y0 > 1.0 {
                let holds = |o: &R, n: &R| n.x >= o.x - 0.5 && n.y >= o.y - 0.5 && n.x + n.w <= o.x + o.w + 0.5 && n.y + n.h <= o.y + o.h + 0.5;
                assert!(holds(&p.rect, &q.rect) || holds(&q.rect, &p.rect), "{what}: {:?} {:?} overlaps {:?} {:?}", p.label, p.rect, q.label, q.rect);
            }
        }
    }
    assert!(f.g.rgba().iter().any(|b| *b != 0), "{what}: nothing drawn");
}

const SIZES: [(u32, u32, f32); 6] = [(360, 740, 1.0), (800, 480, 1.0), (1280, 720, 1.0), (1920, 1080, 1.5), (2560, 1600, 2.0), (2880, 1800, 2.25)];

#[test]
fn the_tile_lists_every_drive_and_selects_the_first() {
    let mut f = admin();
    f.draw();
    let names: Vec<&str> = f.app.snap.disks.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["mmcblk0", "nvme0n1", "nvme1n1", "sda", "sdb", "sdc"]);
    assert_eq!(f.app.title(), "Disks");
    assert!(!f.app.help().is_empty() && f.app.hint().contains("m mount"));
    // one row per drive, an eject button on every drive that can be ejected (not the system drive)
    let rows = f.g.hits.iter().filter(|h| h.label == "drive").count();
    assert_eq!(rows, 6);
    let ejects = f.g.hits.iter().filter(|h| h.label == "eject").count();
    assert_eq!(ejects, 5, "the system drive has none");
    assert!(!f.has_hit(&Event::Tab(T_EJECT_ROW + 1)), "nvme0n1 holds the system");
}

#[test]
fn it_draws_inside_its_area_at_every_size_page_and_state() {
    for (w, h, s) in SIZES {
        let mut f = fixture("disks-size", Role::Admin, w, h, s);
        f.draw();
        assert_hits_sane(&f, &format!("{w}x{h} list"));
        f.pick("sda");
        assert_hits_sane(&f, &format!("{w}x{h} usb"));
        f.chr('m');
        f.draw();
        assert_hits_sane(&f, &format!("{w}x{h} mounted + message"));
        f.chr('p');
        assert_hits_sane(&f, &format!("{w}x{h} properties"));
        f.pick("sdb");
        assert_hits_sane(&f, &format!("{w}x{h} encrypted and mac volumes"));
        f.pick("nvme0n1");
        assert_hits_sane(&f, &format!("{w}x{h} system drive"));
        f.pick("sdc");
        f.chr('a');
        assert!(f.app.manager, "{w}x{h}");
        assert_hits_sane(&f, &format!("{w}x{h} manager"));
        for (key, name) in [('n', "new table form"), ('\u{1b}', "closed")] {
            if key == '\u{1b}' {
                f.key(Key::Esc);
            } else {
                f.chr(key);
            }
            assert_hits_sane(&f, &format!("{w}x{h} {name}"));
        }
        f.chr('n');
        f.key(Key::Enter);
        assert!(matches!(f.app.modal, Modal::Review(_)), "{w}x{h}");
        assert_hits_sane(&f, &format!("{w}x{h} review"));
        f.type_text("sdc");
        assert_hits_sane(&f, &format!("{w}x{h} review typed"));
        f.key(Key::Esc);
        f.chr('a');
        f.chr('a');
        assert!(matches!(f.app.modal, Modal::Form(_)));
        assert_hits_sane(&f, &format!("{w}x{h} add form"));
    }
}

#[test]
fn it_draws_inside_a_pane_that_does_not_start_at_the_corner() {
    for (x, y, w, h) in [(640.0f32, 60.0f32, 640.0f32, 600.0f32), (0.0, 360.0, 1280.0, 360.0), (320.0, 0.0, 320.0, 720.0)] {
        let mut f = admin();
        f.g = Gfx::new(1280, 720);
        f.area = R::new(x, y, w, h);
        f.draw();
        assert_hits_sane(&f, &format!("pane {x},{y} {w}x{h}"));
        f.pick("sdb");
        assert_hits_sane(&f, "pane detail");
        f.pick("sdc");
        f.chr('a');
        assert_hits_sane(&f, "pane manager");
    }
}

#[test]
fn a_small_screen_shows_the_list_then_the_drive_and_back() {
    let mut f = fixture("disks-phone", Role::Admin, 360, 740, 1.0);
    f.draw();
    assert!(f.g.hits.iter().any(|h| h.label == "drive") && !f.g.hits.iter().any(|h| h.label == "partition row"));
    f.pick("sda");
    assert!(f.g.hits.iter().any(|h| h.label == "partition row") && !f.g.hits.iter().any(|h| h.label == "drive"));
    assert!(f.has_hit(&Event::Tab(T_BACK)));
    f.ev(Event::Tab(T_BACK));
    assert!(f.g.hits.iter().any(|h| h.label == "drive"));
    // Esc walks back the same way and then leaves
    f.pick("sda");
    assert!(matches!(f.app.handle(&Event::Key(Key::Esc), &f.cx), Flow::Stay));
    f.draw();
    assert!(f.g.hits.iter().any(|h| h.label == "drive"));
    assert!(matches!(f.app.handle(&Event::Key(Key::Esc), &f.cx), Flow::Back));
}

#[test]
fn the_keyboard_walks_the_drives_filters_mounts_unmounts_and_ejects() {
    let mut f = admin();
    f.draw();
    assert_eq!(f.app.disk().unwrap().name, "mmcblk0");
    f.key(Key::Down);
    assert_eq!(f.app.disk().unwrap().name, "nvme0n1");
    f.key(Key::Up);
    f.key(Key::Up);
    assert_eq!(f.app.disk().unwrap().name, "mmcblk0", "stops at the top");
    // the filter
    f.chr('/');
    f.type_text("sandisk");
    assert_eq!(f.app.visible().len(), 1);
    assert_eq!(f.app.disk().unwrap().name, "sda", "the choice follows the filter");
    f.key(Key::Enter);
    f.key(Key::Esc);
    assert!(f.app.filter.value.is_empty(), "Esc clears the filter");
    f.chr('/');
    f.type_text("nvme");
    assert_eq!(f.app.visible().len(), 2);
    f.key(Key::Esc);
    f.key(Key::Esc);
    f.pick("sda");
    // into the partitions, then mount, unmount
    f.key(Key::Right);
    f.chr('m');
    assert!(f.msg().starts_with("Opened at /home/root/media/STICK"), "{}", f.msg());
    assert!(f.app.part().unwrap().mounted());
    assert!(f.f.mounts_now().iter().any(|m| m.1 == "/home/root/media/STICK"));
    f.chr('o');
    assert!(f.msg().contains("/home/root/media/STICK") && f.msg().contains("copied"), "{}", f.msg());
    f.chr('u');
    assert!(f.msg().contains("is closed"), "{}", f.msg());
    assert!(!f.app.part().unwrap().mounted());
    f.key(Key::Enter);
    assert!(f.app.part().unwrap().mounted(), "Enter on a closed partition opens it");
    f.chr('e');
    assert_eq!(f.msg(), "You can remove it now.");
    assert!(f.app.snap.disk("sda").is_none(), "an ejected drive leaves the list");
    assert!(f.audit().contains("disks.mount") && f.audit().contains("disks.eject") && f.audit().contains("serial=4C530001230815ABCD"), "{}", f.audit());
}

#[test]
fn the_eject_button_of_a_row_and_the_mount_buttons_work_by_clicking() {
    let mut f = admin();
    f.draw();
    f.pick("sda");
    let i = f.app.snap.disks.iter().position(|d| d.name == "sda").unwrap();
    f.ev(Event::Tab(T_MOUNT));
    assert!(f.msg().starts_with("Opened at"), "{}", f.msg());
    assert!(f.has_hit(&Event::Tab(T_UNMOUNT)) && f.has_hit(&Event::Tab(T_OPEN)) && !f.has_hit(&Event::Tab(T_MOUNT)), "Mount is greyed once it is open");
    f.ev(Event::Tab(T_EJECT_ROW + i));
    assert_eq!(f.msg(), "You can remove it now.");
    // the read-only switch and the exec switch
    f.pick("mmcblk0");
    assert!(f.has_hit(&Event::Tab(T_RO)) && f.has_hit(&Event::Tab(T_EXEC)), "an administrator sees both switches");
}

#[test]
fn a_busy_drive_names_what_uses_it() {
    let mut f = admin();
    f.pick("sda");
    f.key(Key::Right);
    f.chr('m');
    f.f.add_process(42, "vlc", "/home/root/media/STICK", &[]);
    f.f.make_busy("/home/root/media/STICK");
    f.chr('u');
    assert!(f.msg().contains("vlc (process 42)") && f.msg().contains("Nothing was forced"), "{}", f.msg());
    assert!(f.app.msg.as_ref().unwrap().1 == false);
    f.draw();
    assert!(f.has_hit(&Event::Tab(T_DISMISS)));
    f.ev(Event::Tab(T_DISMISS));
    assert!(f.app.msg.is_none());
}

#[test]
fn what_cannot_be_opened_says_why_in_plain_words() {
    let mut f = admin();
    f.pick("nvme1n1");
    f.draw();
    assert!(!f.has_hit(&Event::Tab(T_MOUNT)), "no ntfs3 driver in the made-up kernel");
    f.chr('m');
    assert!(f.msg().contains("ntfs3 driver"), "{}", f.msg());
    f.key(Key::Right);
    f.key(Key::Down);
    f.chr('m');
    assert!(f.msg().contains("BitLocker") && f.msg().contains("unlock needs the key"), "{}", f.msg());
    f.pick("sdb");
    f.key(Key::Right);
    f.chr('m');
    assert!(f.msg().contains("LUKS"), "{}", f.msg());
    f.key(Key::Down);
    f.chr('m');
    assert!(f.msg().contains("APFS"), "{}", f.msg());
    assert!(!f.f.calls().iter().any(|c| c.starts_with("mount")));
    // the system volumes cannot be closed
    f.pick("nvme0n1");
    f.key(Key::Right);
    f.draw();
    assert!(!f.has_hit(&Event::Tab(T_UNMOUNT)) && !f.has_hit(&Event::Tab(T_EJECT)));
    f.chr('u');
    f.chr('e');
    assert!(f.msg().contains("cannot be ejected") || f.msg().contains("running system"), "{}", f.msg());
    assert!(!f.f.calls().iter().any(|c| c.starts_with("umount") || c.starts_with("sys ")));
}

#[test]
fn roles_decide_what_the_buttons_do() {
    // a user: removable drives only, no switches for programs, no partition manager
    let mut f = fixture("disks-user", Role::User, 1280, 720, 1.0);
    f.pick("sda");
    f.draw();
    assert!(f.has_hit(&Event::Tab(T_MOUNT)) && f.has_hit(&Event::Tab(T_EJECT)));
    assert!(!f.has_hit(&Event::Tab(T_ADV)) && !f.has_hit(&Event::Tab(T_EXEC)));
    f.chr('a');
    assert!(f.msg().contains("Only an administrator"), "{}", f.msg());
    assert!(!f.app.manager);
    f.pick("sdb");
    f.draw();
    assert!(!f.has_hit(&Event::Tab(T_MOUNT)) && !f.has_hit(&Event::Tab(T_EJECT)), "an internal drive is not for a user");
    assert!(!f.g.hits.iter().any(|h| h.label == "eject" && matches!(h.ev, Event::Tab(n) if f.app.snap.disks.get(n - T_EJECT_ROW).is_some_and(|d| d.name == "sdb"))));
    f.chr('m');
    assert!(f.msg().contains("removable"), "{}", f.msg());
    // an operator: any drive, still no partition manager and no programs switch
    let mut f = fixture("disks-op", Role::Operator, 1280, 720, 1.0);
    f.pick("sdb");
    f.draw();
    assert!(f.has_hit(&Event::Tab(T_EJECT)) && !f.has_hit(&Event::Tab(T_ADV)) && !f.has_hit(&Event::Tab(T_EXEC)));
    f.ev(Event::Tab(T_EXEC));
    assert!(!f.app.allow_exec);
    // a guest gets nothing
    let mut f = fixture("disks-guest", Role::Guest, 1280, 720, 1.0);
    f.draw();
    assert!(f.g.hits.is_empty(), "no click region for a guest");
    f.chr('m');
    f.chr('e');
    assert!(!f.f.calls().iter().any(|c| c.starts_with("mount") || c.starts_with("umount")));
}

#[test]
fn the_administrator_programs_switch_lets_programs_run() {
    let mut f = admin();
    f.pick("sda");
    f.ev(Event::Tab(T_EXEC));
    assert!(f.app.allow_exec);
    f.chr('m');
    let (_, _, _, flags, _) = f.f.last_mount().unwrap();
    assert!(!flags.noexec && flags.nosuid && flags.nodev);
    f.chr('u');
    f.ev(Event::Tab(T_EXEC));
    f.ev(Event::Tab(T_RO));
    f.chr('m');
    let (_, _, _, flags, _) = f.f.last_mount().unwrap();
    assert!(flags.noexec && flags.rdonly);
}

#[test]
fn the_partition_manager_asks_for_the_name_shows_the_plan_and_needs_a_second_press() {
    let mut f = admin();
    f.pick("sdc");
    f.draw();
    assert!(f.has_hit(&Event::Tab(T_ADV)), "the way in is a button, the view starts closed");
    assert!(!f.app.manager && !f.has_hit(&Event::Tab(T_NEWTABLE)));
    f.ev(Event::Tab(T_ADV));
    assert!(f.app.manager && f.has_hit(&Event::Tab(T_NEWTABLE)));
    f.ev(Event::Tab(T_RESIZE));
    assert!(f.msg().contains("no partitions yet"), "{}", f.msg());
    f.ev(Event::Tab(T_NEWTABLE));
    let Modal::Form(form) = &f.app.modal else { panic!("the form") };
    assert_eq!(form.kind, FormKind::NewTable);
    f.ev(Event::Tab(T_REVIEW));
    let Modal::Review(r) = &f.app.modal else { panic!("the review") };
    let text = r.lines.join("\n");
    assert!(text.contains("sdc (Generic Flash Drive, 134.2 MB, serial SERIAL-SDC-9F3A)") && text.contains("The plan:"), "{text}");
    assert!(!f.f.wrote(), "the review is a dry run");
    assert!(!f.has_hit(&Event::Tab(T_APPLY)), "Apply is off until the name is typed");
    // a wrong name keeps it off, and Enter on the field does not apply
    f.type_text("sdd");
    assert!(!f.has_hit(&Event::Tab(T_APPLY)));
    f.key(Key::Enter);
    assert!(!f.f.wrote());
    for _ in 0..3 {
        f.key(Key::Backspace);
    }
    f.type_text("9f3a");
    assert!(f.has_hit(&Event::Tab(T_APPLY)), "the last 4 of the serial also confirm");
    f.key(Key::Enter);
    assert!(!f.f.wrote(), "the first Enter only moves to the button");
    let Modal::Review(r) = &f.app.modal else { panic!() };
    assert_eq!(r.focus, 2);
    f.key(Key::Enter);
    assert!(f.f.wrote(), "the second press applies");
    assert!(f.msg().starts_with("Done:"), "{}", f.msg());
    assert!(matches!(f.app.modal, Modal::None));
    assert_eq!(f.app.snap.disk("sdc").unwrap().table, iyakki_agent::disks::Table::Gpt);
    let a = f.audit();
    assert!(a.contains("disks.partition.apply") && a.contains("disks.partition.done") && a.contains("drive=sdc size=134217728 serial=SERIAL-SDC-9F3A"), "{a}");
}

#[test]
fn adding_formatting_and_deleting_through_the_forms() {
    let mut f = admin();
    f.pick("sdc");
    f.chr('a');
    f.chr('n');
    f.key(Key::Enter);
    f.type_text("sdc");
    f.ev(Event::Tab(T_APPLY));
    assert!(f.msg().starts_with("Done"), "{}", f.msg());
    // add 40 MB of ext4 called data
    f.chr('a');
    f.type_text("40 MB");
    f.key(Key::Tab);
    f.type_text("data");
    f.key(Key::Enter);
    f.type_text("sdc");
    f.ev(Event::Tab(T_APPLY));
    assert!(f.msg().starts_with("Done"), "{}", f.msg());
    let d = f.app.snap.disk("sdc").unwrap();
    assert_eq!((d.parts.len(), d.parts[0].fs, d.parts[0].label.as_str(), d.parts[0].size_bytes), (1, iyakki_agent::disks::Fs::Ext4, "data", 40 << 20));
    // a form with a bad size explains itself and stays open
    f.chr('a');
    f.type_text("lots");
    f.key(Key::Enter);
    let Modal::Form(form) = &f.app.modal else { panic!() };
    assert!(form.error.contains("is not a size"), "{}", form.error);
    f.key(Key::Esc);
    // exFAT is offered; without its program the plan says which one is missing
    f.chr('f');
    f.ev(Event::Tab(T_FS + 2));
    f.ev(Event::Tab(T_REVIEW));
    let Modal::Form(form) = &f.app.modal else { panic!() };
    assert!(form.error.contains("mkfs.exfat") && form.error.contains("not installed"), "{}", form.error);
    f.key(Key::Esc);
    // delete the partition
    f.chr('d');
    assert!(matches!(f.app.modal, Modal::Review(_)));
    f.type_text("sdc");
    f.ev(Event::Tab(T_APPLY));
    assert!(f.app.snap.disk("sdc").unwrap().parts.is_empty(), "{}", f.msg());
}

#[test]
fn the_system_drive_cannot_even_open_the_manager() {
    let mut f = admin();
    f.pick("nvme0n1");
    f.draw();
    assert!(!f.has_hit(&Event::Tab(T_ADV)), "no button on the system drive");
    f.chr('a');
    assert!(!f.app.manager && f.msg().contains("never changed"), "{}", f.msg());
    f.ev(Event::Tab(T_ADV));
    assert!(!f.app.manager);
    // even a review forced by the engine's own plan call is refused
    f.app.review(iyakki_agent::disks_part::PartOp::NewTable(iyakki_agent::disks::Table::Gpt));
    assert!(matches!(f.app.modal, Modal::None) && !f.f.wrote());
    // a mounted drive is refused with the way out
    f.pick("sda");
    f.chr('m');
    f.chr('a');
    f.chr('n');
    assert!(f.msg().contains("Unmount (or eject) it first"), "{}", f.msg());
    assert!(matches!(f.app.modal, Modal::None));
    assert!(!f.f.calls().iter().any(|c| c.starts_with("write") || c.starts_with("zero")));
}

#[test]
fn a_new_usb_drive_is_announced_and_selected() {
    let mut f = admin();
    f.draw();
    let _ = crate::ui::clipboard::take_toast();
    f.f.add_disk("sdx", 2_000_000, true, "/sys/devices/x/usb9/9-1/block/sdx", "Kingston", "DataTraveler", 512);
    f.f.add_part("sdx", "sdx1", 1, 2048, 1_000_000, iyakki_agent::disks_fake::sb_fat32("PHOTOS"));
    f.ev(Event::Tick);
    f.ev(Event::Tick);
    assert!(f.app.snap.disk("sdx").is_some());
    assert_eq!(f.app.disk().unwrap().name, "sdx", "the new drive is chosen");
    assert!(f.msg().contains("USB drive connected: Kingston DataTraveler"), "{}", f.msg());
    let t = crate::ui::clipboard::take_toast().unwrap_or_default();
    assert!(t.contains("USB drive connected"), "{t}");
    // unplugged: it goes, and the choice stays valid
    f.f.st.lock().unwrap().files.retain(|k, _| !k.starts_with("/sys/block/sdx/"));
    f.f.st.lock().unwrap().dirs.remove("/sys/block/sdx");
    f.ev(Event::Tick);
    f.ev(Event::Tick);
    assert!(f.app.snap.disk("sdx").is_none() && f.app.disk().is_some());
    f.draw();
}

#[test]
fn sizes_are_parsed_in_plain_units() {
    assert_eq!(parse_size("").unwrap(), None);
    assert_eq!(parse_size("500 MB").unwrap(), Some(500 << 20));
    assert_eq!(parse_size("2gb").unwrap(), Some(2 << 30));
    assert_eq!(parse_size("1.5 G").unwrap(), Some(3 << 29));
    assert_eq!(parse_size("64").unwrap(), Some(64 << 20), "no unit means MB");
    assert_eq!(parse_size("1 TiB").unwrap(), Some(1 << 40));
    assert!(parse_size("lots").is_err() && parse_size("5 parsecs").is_err() && parse_size("0 MB").is_err() && parse_size("-3 MB").is_err());
}

#[test]
fn nothing_panics_on_a_machine_with_no_drives_or_a_collapsed_area() {
    let mut f = admin();
    f.app = DisksApp::with_engine(&f.cx, Arc::new(Fake::new().disks()), true);
    f.draw();
    assert!(f.app.snap.disks.is_empty());
    for k in [Key::Down, Key::Up, Key::Enter, Key::Right, Key::Left, Key::Char('m'), Key::Char('u'), Key::Char('e'), Key::Char('a'), Key::Char('p'), Key::Char('/'), Key::Esc, Key::Char('d')] {
        f.key(k);
    }
    for (w, h) in [(1.0f32, 1.0f32), (40.0, 30.0), (200.0, 5.0), (0.0, 0.0), (10_000.0, 10.0)] {
        let mut g = Gfx::new(400, 300);
        let mut f2 = admin();
        f2.pick("sda");
        f2.app.render(&mut g, R::new(0.0, 0.0, w, h), 2.25, &f2.cx, 3);
        f2.chr('a');
        f2.app.render(&mut g, R::new(0.0, 0.0, w, h), 0.5, &f2.cx, 4);
    }
}

#[test]
fn settings_storage_and_the_file_picker_lead_to_the_drives() {
    // Settings -> Storage: `d` asks the shell to open DISKS
    // (the Storage page itself is for administrators)
    for (role, opens) in [(Role::Admin, true), (Role::Operator, false), (Role::User, false), (Role::Guest, false)] {
        let mut cx = cx_with_admin_store("disks-settings");
        cx.session.role = role;
        cx.session.policy = Policy::resolve(role, &Default::default());
        let mut a = super::settings::SettingsApp::new(&cx);
        a.open_item("tab:storage", &cx);
        a.handle(&Event::Key(Key::Char('d')), &cx);
        let r = a.take_request();
        if opens {
            assert_eq!(r, Some(("disks".to_string(), String::new())), "{role:?}");
        } else {
            assert!(r.is_none(), "{role:?}");
        }
    }
    // the file picker: a Drives button; without an open drive (or without the right to read /media) it says so and stays put
    let base = std::env::temp_dir().join(format!("pk-drives-{}", std::process::id()));
    let home = base.join("home/amy");
    std::fs::create_dir_all(&home).unwrap();
    let mut p = Role::User.template();
    p.fs_read = vec![home.display().to_string()];
    p.fs_write = vec![];
    let scope = crate::filepicker::scope_for("amy", &p, iyakki_agent::scope::FileRoots::with_home(&p, "amy", &base, home.clone()));
    let mut pk = crate::filepicker::FilePicker::new(crate::filepicker::PathKind::File, &[], "Choose", &home.to_string_lossy(), scope);
    let drives = Event::Tab(crate::filepicker::TAB_BASE + 7);
    pk.handle_event(&drives);
    assert!(pk.error.as_deref().is_some_and(|e| e.contains("drive")), "{:?}", pk.error);
    assert_eq!(pk.dir, home, "the picker did not move");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn the_pickers_drives_button_goes_to_the_open_drives_of_the_account_and_is_drawn_inside_the_window_at_four_sizes() {
    let base = std::env::temp_dir().join(format!("pk-drives2-{}", std::process::id()));
    let home = base.join("home/amy");
    let media = home.join("media");
    std::fs::create_dir_all(media.join("STICK")).unwrap();
    let mut p = Role::User.template();
    p.fs_read = vec![home.display().to_string()];
    p.fs_write = vec![];
    let mk = |p: &Policy| crate::filepicker::scope_for("amy", p, iyakki_agent::scope::FileRoots::with_home(p, "amy", &base, home.clone()));
    let drives = Event::Tab(crate::filepicker::TAB_BASE + 7);
    // an open drive and the right to read the folder: the picker goes there
    let mut pk = crate::filepicker::FilePicker::new(crate::filepicker::PathKind::File, &[], "Choose", &home.to_string_lossy(), mk(&p));
    pk.handle_event(&drives);
    assert_eq!(pk.dir, std::fs::canonicalize(&media).unwrap_or(media.clone()), "{:?}", pk.error);
    assert!(pk.error.is_none());
    // no right to read it: it says so and stays
    let mut none = p.clone();
    none.fs_read = vec![];
    let mut pk2 = crate::filepicker::FilePicker::new(crate::filepicker::PathKind::File, &[], "Choose", &home.to_string_lossy(), mk(&none));
    let before = pk2.dir.clone();
    pk2.handle_event(&drives);
    assert_eq!(pk2.dir, before);
    // the button is drawn whole, inside the window, at the four sizes
    for (w, h, s) in [(1280u32, 720u32, 1.0f32), (1920, 1080, 1.5), (1024, 600, 0.8), (800, 480, 0.62)] {
        let mut g = Gfx::new(w, h);
        pk.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, rgb(90, 200, 160), false);
        let hit = g.hits.iter().find(|x| x.label.contains("Drives")).unwrap_or_else(|| panic!("{w}x{h}: no Drives button: {:?}", g.hits.iter().map(|x| &x.label).collect::<Vec<_>>()));
        let r = hit.rect;
        assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 && r.y + r.h <= h as f32, "{w}x{h}: {r:?}");
    }
    let _ = std::fs::remove_dir_all(&base);
}

// ---------------------------------------------------------------------------------------------------------------------
// encrypted volumes: unlock, allow writing, lock
// ---------------------------------------------------------------------------------------------------------------------

fn with_helper(f: &mut Fx) -> Arc<iyakki_agent::disks_helper::FakeCrypt> {
    use iyakki_agent::disks_helper::FakeCrypt;
    let c = FakeCrypt::new(f.f.clone());
    c.add("sdb1", "open sesame", iyakki_agent::disks_fake::sb_ext4("secrets"));
    c.add("nvme1n1p2", "123456-234567-345678-456789-567890-678901-789012-890123", iyakki_agent::disks_fake::sb_ext4("win"));
    f.app.crypt = c.clone();
    c
}

fn choose_sdb1(f: &mut Fx) {
    f.pick("sdb");
    f.key(Key::Right);
    assert_eq!(f.app.part().map(|p| p.name.clone()).as_deref(), Some("sdb1"));
}

fn modal_is_unlock(f: &Fx) -> bool {
    matches!(f.app.modal, Modal::Unlock(_))
}

#[test]
fn an_encrypted_partition_offers_unlock_and_says_what_it_is() {
    let mut f = admin();
    with_helper(&mut f);
    choose_sdb1(&mut f);
    f.draw();
    assert_eq!(f.app.part().and_then(|p| p.crypt.as_ref()).map(|c| c.headline()).as_deref(), Some("Encrypted: LUKS2"));
    assert!(f.has_hit(&Event::Tab(T_UNLOCK)), "Unlock is a button");
    assert!(!f.has_hit(&Event::Tab(T_LOCK)) && !f.has_hit(&Event::Tab(T_ALLOW_WRITE)), "nothing to lock or write yet");
    assert!(!f.has_hit(&Event::Tab(T_MOUNT)), "a locked volume is not mounted plainly");
    // an ordinary partition has none of these buttons
    f.pick("sda");
    f.draw();
    assert!(!f.has_hit(&Event::Tab(T_UNLOCK)));
    // the BitLocker volume
    f.pick("nvme1n1");
    f.key(Key::Right);
    f.key(Key::Down);
    f.draw();
    assert_eq!(f.app.part().and_then(|p| p.crypt.as_ref()).map(|c| c.headline()).as_deref(), Some("Encrypted: BitLocker"));
    assert!(f.has_hit(&Event::Tab(T_UNLOCK)));
}

#[test]
fn the_password_box_is_masked_unlocks_read_only_then_writing_and_lock_are_separate_steps() {
    let mut f = admin();
    let c = with_helper(&mut f);
    choose_sdb1(&mut f);
    f.chr('k');
    assert!(modal_is_unlock(&f));
    if let Modal::Unlock(u) = &f.app.modal {
        assert!(u.field.mask, "the box is masked");
    }
    f.draw();
    assert!(f.has_hit(&Event::Tab(T_DO_UNLOCK)) && f.has_hit(&Event::Tab(T_CANCEL)));
    assert_hits_sane(&f, "unlock box");
    f.type_text("open sesame");
    f.key(Key::Enter);
    assert!(!modal_is_unlock(&f), "the box closes and its text is gone");
    assert!(f.msg().contains("Opened at") && f.msg().contains("read-only"), "{}", f.msg());
    let p = f.app.part().cloned().unwrap();
    assert!(p.unlocked && p.mounted() && p.mounted_ro);
    // the secret is not in the audit log, the message or anything the tile keeps
    assert!(!f.audit().contains("open sesame") && !f.msg().contains("open sesame"));
    assert!(f.audit().contains("disks.unlock"), "{}", f.audit());
    // buttons now
    f.draw();
    assert!(f.has_hit(&Event::Tab(T_LOCK)) && f.has_hit(&Event::Tab(T_ALLOW_WRITE)) && !f.has_hit(&Event::Tab(T_UNLOCK)));
    // writing is its own press
    f.ev(Event::Tab(T_ALLOW_WRITE));
    assert!(f.msg().contains("Open for writing"), "{}", f.msg());
    assert!(!f.app.part().unwrap().mounted_ro);
    // lock
    f.ev(Event::Tab(T_LOCK));
    assert!(f.msg().contains("locked again"), "{}", f.msg());
    let p = f.app.part().unwrap();
    assert!(!p.unlocked && !p.mounted());
    assert_eq!(c.log(), ["open sdb1 ro=false", "close sdb1"]);
    assert!(!f.f.wrote(), "no byte of any drive was written");
}

#[test]
fn a_wrong_password_is_a_plain_message_and_the_next_try_has_to_wait() {
    let mut f = admin();
    let c = with_helper(&mut f);
    f.app.engine.hold_clock(1000);
    choose_sdb1(&mut f);
    f.chr('k');
    f.type_text("not it");
    f.key(Key::Enter);
    assert!(f.msg().contains("did not unlock") && !f.msg().contains("not it"), "{}", f.msg());
    assert!(!f.audit().contains("not it"));
    assert!(!f.app.part().unwrap().unlocked);
    f.chr('k');
    f.type_text("open sesame");
    f.key(Key::Enter);
    assert!(f.msg().contains("Wait") && f.msg().contains("seconds"), "{}", f.msg());
    assert_eq!(c.log().len(), 1, "the helper was asked once");
}

#[test]
fn escape_and_cancel_close_the_box_and_an_empty_box_asks_for_the_password() {
    let mut f = admin();
    let c = with_helper(&mut f);
    choose_sdb1(&mut f);
    f.chr('k');
    f.type_text("abc");
    f.key(Key::Esc);
    assert!(matches!(f.app.modal, Modal::None));
    f.chr('k');
    f.key(Key::Enter);
    assert!(modal_is_unlock(&f), "an empty box stays open");
    if let Modal::Unlock(u) = &f.app.modal {
        assert!(u.error.contains("Type the password"), "{}", u.error);
    }
    f.ev(Event::Tab(T_CANCEL));
    assert!(matches!(f.app.modal, Modal::None));
    assert!(c.log().is_empty());
}

#[test]
fn a_bitlocker_volume_takes_a_recovery_key_and_stays_read_only() {
    let mut f = admin();
    let c = with_helper(&mut f);
    f.pick("nvme1n1");
    f.key(Key::Right);
    f.key(Key::Down);
    f.chr('k');
    f.type_text("123456-234567-345678-456789-567890-678901-789012-890123");
    f.key(Key::Enter);
    assert!(f.msg().contains("Opened at"), "{}", f.msg());
    assert_eq!(c.log()[0], "open nvme1n1p2 ro=true");
    f.draw();
    assert!(!f.has_hit(&Event::Tab(T_ALLOW_WRITE)), "no writing for BitLocker");
    f.chr('w');
    assert!(f.msg().contains("Writing can be allowed only"), "{}", f.msg());
}

#[test]
fn without_the_helper_program_the_tile_says_how_to_get_it_and_opens_no_box() {
    let mut f = admin();
    choose_sdb1(&mut f);
    f.draw();
    assert!(!f.has_hit(&Event::Tab(T_UNLOCK)), "the button is grey");
    f.chr('k');
    assert!(matches!(f.app.modal, Modal::None));
    assert!(f.msg().contains("cryptsetup") && f.msg().contains("does not ship it") && f.msg().contains("CLI tools"), "{}", f.msg());
}

#[test]
fn people_who_may_not_use_the_drive_get_no_box() {
    let mut f = fixture("disks-user", Role::User, 1280, 720, 1.0);
    with_helper(&mut f);
    choose_sdb1(&mut f);
    f.chr('k');
    assert!(matches!(f.app.modal, Modal::None));
    assert!(f.msg().contains("removable"), "{}", f.msg());
}

#[test]
fn the_assistants_request_opens_the_box_for_that_partition_once() {
    let mut f = admin();
    with_helper(&mut f);
    f.pick("sda");
    iyakki_agent::disksunlocktools::request(f.cx.services.dir(), &f.cx.session.user, "sdb1");
    for _ in 0..40 {
        f.ev(Event::Tick);
    }
    assert!(modal_is_unlock(&f), "the box is open");
    assert_eq!(f.app.part().map(|p| p.name.clone()).as_deref(), Some("sdb1"), "the partition is chosen");
    f.key(Key::Esc);
    for _ in 0..40 {
        f.ev(Event::Tick);
    }
    assert!(matches!(f.app.modal, Modal::None), "one request, one box");
}

#[test]
fn the_unlock_box_draws_inside_the_window_at_every_size() {
    for (w, h, s) in [(1280u32, 720u32, 1.0f32), (800, 480, 1.0), (480, 320, 0.8), (1920, 1080, 1.5)] {
        let mut f = fixture("disks-box", Role::Admin, w, h, s);
        with_helper(&mut f);
        choose_sdb1(&mut f);
        f.chr('k');
        f.type_text("x");
        f.draw();
        assert_hits_sane(&f, &format!("unlock box {w}x{h}"));
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// resize, exFAT and the cancel window
// ---------------------------------------------------------------------------------------------------------------------

/// The blank stick with a GPT and one 40 MiB ext4 partition "data", the programs made up.
fn stick_with_data() -> Fx {
    use iyakki_agent::disks::Table;
    use iyakki_agent::disks_part::{Confirm, FormatFs, PartOp};
    let mut f = admin();
    let sys = iyakki_agent::disks_helper::FakeSys::with_programs(&["e2fsck", "resize2fs", "mkfs.exfat", "mkntfs", "ntfsresize"]);
    f.app.engine.set_tools(Arc::new(iyakki_agent::disks_fmt::ProcTools::new(sys, "/none")));
    let (a, c) = (f.app.who.clone(), Confirm { typed: "sdc".into(), second: true });
    f.app.engine.apply_partition(&a, "sdc", &PartOp::NewTable(Table::Gpt), &c, &|_, _| {}).unwrap();
    f.app.engine.apply_partition(&a, "sdc", &PartOp::Add { size_bytes: Some(40 << 20), fs: Some(FormatFs::Ext4), label: "data".into() }, &c, &|_, _| {}).unwrap();
    f.app.engine.forget();
    f.ev(Event::Tab(T_REFRESH));
    f.pick("sdc");
    f
}

fn open_resize(f: &mut Fx, size: &str) {
    f.ev(Event::Tab(T_ADV));
    f.app.select_part(0);
    f.draw();
    assert!(f.has_hit(&Event::Tab(T_RESIZE)), "Resize is a real button once a partition is chosen");
    f.chr('z');
    assert!(matches!(&f.app.modal, Modal::Form(x) if x.kind == FormKind::Resize));
    f.type_text(size);
    f.key(Key::Enter);
    assert!(matches!(f.app.modal, Modal::Review(_)), "{}", f.msg());
}

#[test]
fn resize_shows_the_dry_run_then_needs_the_typed_name_and_waits_out_a_visible_countdown() {
    let mut f = stick_with_data();
    open_resize(&mut f, "80 MB");
    let Modal::Review(r) = &f.app.modal else { panic!() };
    let text = r.lines.join("\n");
    assert!(text.contains("Resize a partition") && text.contains("Grow the file system") && text.contains("If the power fails"), "{text}");
    let writes = f.f.calls().len();
    // a plain Yes, or nothing, never starts it
    for wrong in ["", "yes"] {
        if let Modal::Review(r) = &mut f.app.modal {
            r.field.set(wrong);
        }
        f.ev(Event::Tab(T_APPLY));
        assert!(f.app.countdown_left().is_none() && f.f.calls().len() == writes, "{wrong:?}");
    }
    // the label, typed exactly, starts the countdown (not the change)
    f.app.cancel_window = std::time::Duration::from_secs(5);
    if let Modal::Review(r) = &mut f.app.modal {
        r.field.set("data");
    }
    f.draw();
    assert!(f.has_hit(&Event::Tab(T_APPLY)));
    f.ev(Event::Tab(T_APPLY));
    assert_eq!(f.app.countdown_left(), Some(5));
    assert!(f.app.job.is_none() && f.f.calls().len() == writes, "nothing is written during the window");
    assert!(!f.has_hit(&Event::Tab(T_APPLY)) && f.has_hit(&Event::Tab(T_CANCEL)), "only Cancel is offered while it counts down");
    // typing does nothing then; Cancel stops it
    f.type_text("x");
    assert_eq!(f.app.countdown_left(), Some(5));
    f.ev(Event::Tab(T_CANCEL));
    assert!(matches!(f.app.modal, Modal::None) && f.msg().contains("Cancelled") && f.f.calls().len() == writes);
    assert_eq!(f.app.snap.disk("sdc").unwrap().parts[0].size_bytes, 40 << 20);
}

#[test]
fn escape_cancels_the_countdown_and_the_end_of_it_starts_the_change() {
    let mut f = stick_with_data();
    f.app.cancel_window = std::time::Duration::from_secs(5);
    open_resize(&mut f, "80 MB");
    let writes = f.f.calls().len();
    let arm = |f: &mut Fx| {
        if let Modal::Review(r) = &mut f.app.modal {
            r.field.set("sdc1");
        }
        f.ev(Event::Tab(T_APPLY));
    };
    arm(&mut f);
    assert!(f.app.countdown_left().is_some());
    f.key(Key::Esc);
    assert!(matches!(f.app.modal, Modal::None) && f.f.calls().len() == writes && f.msg().contains("Cancelled"));
    // again, and this time the window runs out
    open_resize(&mut f, "80 MB");
    arm(&mut f);
    f.ev(Event::Tick);
    assert!(f.app.countdown_left().is_some(), "the window has not ended yet");
    if let Modal::Review(r) = &mut f.app.modal {
        r.armed = Some(std::time::Instant::now() - std::time::Duration::from_secs(6));
    }
    f.ev(Event::Tick);
    f.ev(Event::Tick);
    assert!(f.f.calls().len() > writes, "{}", f.msg());
    assert_eq!(f.app.snap.disk("sdc").unwrap().parts[0].size_bytes, 80 << 20, "{}", f.msg());
    assert!(f.audit().contains("disks.partition.apply"), "{}", f.audit());
}

#[test]
fn what_the_assistant_asked_for_opens_the_review_but_the_person_does_the_confirming() {
    use iyakki_agent::disks_part::FormatFs;
    let mut f = stick_with_data();
    let user = f.cx.session.user.clone();
    let writes = f.f.calls().len();
    iyakki_agent::disksformattools::request(f.cx.services.dir(), &user, &iyakki_agent::disksformattools::Req::Format { part: "sdc1".into(), fs: FormatFs::Exfat, label: "BK".into() });
    for _ in 0..40 {
        f.ev(Event::Tick);
        if matches!(f.app.modal, Modal::Review(_)) {
            break;
        }
    }
    let Modal::Review(r) = &f.app.modal else { panic!("the review is waiting: {}", f.msg()) };
    assert!(r.lines.join("\n").contains("Everything on /dev/sdc1") && r.lines.join("\n").contains("mkfs.exfat"));
    assert!(r.field.value.is_empty() && r.armed.is_none(), "nothing is typed or confirmed for the person");
    assert_eq!(f.f.calls().len(), writes, "only the dry run so far");
    f.ev(Event::Tab(T_APPLY));
    assert_eq!(f.f.calls().len(), writes, "and pressing Apply without the name still does nothing");
}
