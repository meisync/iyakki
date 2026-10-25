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
    assert!(f.app.manager && f.has_hit(&Event::Tab(T_NEWTABLE)) && f.has_hit(&Event::Tab(T_RESIZE)));
    f.ev(Event::Tab(T_RESIZE));
    assert!(f.msg().contains("Resizing is not supported yet"), "{}", f.msg());
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
    // exFAT is offered but honest
    f.chr('f');
    f.ev(Event::Tab(T_FS + 2));
    f.ev(Event::Tab(T_REVIEW));
    let Modal::Form(form) = &f.app.modal else { panic!() };
    assert!(form.error.contains("exFAT is not supported yet"), "{}", form.error);
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
    let scope = crate::filepicker::Scope::new("amy", &p, iyakki_agent::scope::FileRoots::with_home(&p, "amy", &base, home.clone()));
    let mut pk = crate::filepicker::FilePicker::new(crate::filepicker::PathKind::File, &[], "Choose", &home.to_string_lossy(), scope);
    let drives = Event::Tab(crate::filepicker::TAB_BASE + 7);
    pk.handle_event(&drives);
    assert!(pk.error.as_deref().is_some_and(|e| e.contains("drive")), "{:?}", pk.error);
    assert_eq!(pk.dir, home, "the picker did not move");
    let _ = std::fs::remove_dir_all(&base);
}
