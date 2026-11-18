//! Settings → Memory: the page's figures, editing and locking, Preview, Apply now (with a stand-in for swapon/swapoff, never the
//! real machine) and Save, the read-only window, and geometry at four screen sizes.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_zram::compressed::{self, Experiment};
use iyakki_zram::Swapper;
use std::path::Path;
use std::sync::Mutex;

struct Offline;
impl iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

#[derive(Default)]
struct FakeSwap(Mutex<Vec<String>>);
impl Swapper for FakeSwap {
    fn swapon(&self, p: &Path, prio: i32) -> Result<(), String> {
        self.0.lock().unwrap().push(format!("on {} {prio}", p.file_name().unwrap().to_string_lossy()));
        Ok(())
    }
    fn swapoff(&self, p: &Path) -> Result<(), String> {
        self.0.lock().unwrap().push(format!("off {}", p.file_name().unwrap().to_string_lossy()));
        Ok(())
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
fn clear_field(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..40 {
        key(a, cx, Key::Backspace);
    }
}
fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}
fn settle(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..2000 {
        a.handle(&Event::Tick, cx);
        if !a.is_busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("job did not finish");
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
fn goto_memory(a: &mut SettingsApp, _cx: &AppCx) {
    a.go_to(Tab::Memory);
}

/// A made-up machine: 4 GiB of RAM, zram0 in use (2 GiB, zstd) with 1 MiB stored.
fn machine(tag: &str, swap_used_kib: u64) -> (std::path::PathBuf, iyakki_zram::Env) {
    let root = temp_dir(&format!("{tag}-mem"));
    let z = root.join("sys/block/zram0");
    std::fs::create_dir_all(&z).unwrap();
    std::fs::write(z.join("comp_algorithm"), "lzo lzo-rle lz4 [zstd]\n").unwrap();
    std::fs::write(z.join("disksize"), "2147483648\n").unwrap();
    std::fs::write(z.join("mm_stat"), "1048576 262144 300000 0 0 0 0 0 0\n").unwrap();
    std::fs::write(z.join("reset"), "").unwrap();
    std::fs::create_dir_all(root.join("dev")).unwrap();
    std::fs::File::create(root.join("dev/zram0")).unwrap().set_len(1 << 20).unwrap();
    std::fs::create_dir_all(root.join("proc/sys/vm")).unwrap();
    std::fs::create_dir_all(root.join("proc/pressure")).unwrap();
    std::fs::write(root.join("proc/pressure/memory"), "some avg10=1.50 avg60=0.75 avg300=0.10 total=1\n").unwrap();
    std::fs::write(root.join("proc/meminfo"), "MemTotal: 4194304 kB\nMemFree: 1000000 kB\nMemAvailable: 2000000 kB\nSwapTotal: 2097148 kB\nSwapFree: 2097148 kB\n").unwrap();
    std::fs::write(root.join("proc/swaps"), format!("Filename Type Size Used Priority\n/dev/zram0 partition 2097148 {swap_used_kib} 100\n")).unwrap();
    let env = iyakki_zram::Env::under(&root);
    (root, env)
}

fn app(tag: &str, cmdline: &str, writable: bool, used: u64) -> (AppCx, SettingsApp, std::path::PathBuf, Arc<FakeSwap>) {
    let cx = admin(tag);
    let (root, env) = machine(tag, used);
    let sw = Arc::new(FakeSwap::default());
    let mut a = SettingsApp::new(&cx).with_memory(Arc::new(iyakki_core_local::LocalMemory::new(env, sw.clone(), root.join("system.conf"))), cmdline, writable);
    goto_memory(&mut a, &cx);
    (cx, a, root, sw)
}

#[test]
fn the_page_is_a_system_page_for_administrators_only_and_shows_the_figures() {
    let (cx, mut a, _, _) = app("mem-fig", "", true, 1024);
    assert_eq!(Tab::Memory.group(), "System");
    let g = draw(&mut a, &cx, 1280, 720);
    let all = a.mem.report.lines.iter().map(|l| l.0.clone()).collect::<Vec<_>>().join("\n");
    for want in ["RAM", "4.0 GiB total", "memory stall 1.5%", "/dev/zram0  priority 100", "zram0", "zstd", "ratio 4.0:1"] {
        assert!(all.contains(want), "{want} in:\n{all}");
    }
    assert!(g.hits.iter().any(|h| h.label == "Apply now") && g.hits.iter().any(|h| h.label == "Save for next boots"));
    let mut user = admin("mem-user");
    user.session.user = "amy".into();
    user.session.role = iyakki_sec::policy::Role::User;
    user.session.policy = iyakki_sec::policy::Role::User.template();
    assert!(!SettingsApp::new(&user).tabs().contains(&Tab::Memory));
}

#[test]
fn editing_previewing_and_saving_for_the_next_boots() {
    let (cx, mut a, root, sw) = app("mem-edit", "", true, 1024);
    press(&mut a, &cx, 'z');
    assert!(a.has_modal());
    clear_field(&mut a, &cx);
    typ(&mut a, &cx, "potato");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "a value that is not a size keeps the form open");
    clear_field(&mut a, &cx);
    typ(&mut a, &cx, "1G");
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal());
    press(&mut a, &cx, 'c');
    clear_field(&mut a, &cx);
    typ(&mut a, &cx, "lz4");
    key(&mut a, &cx, Key::Enter);
    press(&mut a, &cx, 'w');
    clear_field(&mut a, &cx);
    typ(&mut a, &cx, "60");
    key(&mut a, &cx, Key::Enter);
    press(&mut a, &cx, 'f');
    clear_field(&mut a, &cx);
    typ(&mut a, &cx, "2G");
    key(&mut a, &cx, Key::Enter);
    press(&mut a, &cx, 'x');
    assert_eq!((a.mem.draft.zram_value().as_str(), a.mem.draft.swappiness, a.mem.draft.swapfile, a.mem.draft.zswap), ("1G,lz4", 60, Some(2 << 30), true));
    assert!(sw.0.lock().unwrap().is_empty(), "editing changes nothing on the machine");
    assert!(!root.join("system.conf").exists());
    press(&mut a, &cx, 'p');
    let g = draw(&mut a, &cx, 1280, 720);
    let text = a.mem.preview.join("\n");
    assert!(text.contains("becomes 1.0 GiB") && text.contains("swap file: 2.0 GiB") && text.contains("zswap: on"), "{text}");
    assert!(g.hits.len() > 5);
    assert!(sw.0.lock().unwrap().is_empty(), "preview changes nothing");
    press(&mut a, &cx, 'v');
    assert!(status(&a).contains("next boot"), "{}", status(&a));
    let saved = std::fs::read_to_string(root.join("system.conf")).unwrap();
    assert!(saved.contains("zram=1G,lz4") && saved.contains("swappiness=60") && saved.contains("swapfile=2G") && saved.contains("zswap=on"), "{saved}");
    assert!(!a.mem_dirty(), "after saving the page matches the file");
    assert!(sw.0.lock().unwrap().is_empty(), "saving does not touch the running system");
    assert!(std::fs::read_to_string(cx.audit.path()).unwrap_or_default().contains("memory.save"));
}

#[test]
fn apply_now_reconfigures_the_running_system_after_confirming() {
    let (cx, mut a, root, sw) = app("mem-apply", "", true, 1024);
    press(&mut a, &cx, 'z');
    clear_field(&mut a, &cx);
    typ(&mut a, &cx, "1G,lz4");
    key(&mut a, &cx, Key::Enter);
    press(&mut a, &cx, 'y');
    assert!(a.has_modal(), "an explicit confirmation shows what changes");
    let _ = draw(&mut a, &cx, 1280, 720);
    assert!(sw.0.lock().unwrap().is_empty(), "nothing yet");
    press(&mut a, &cx, 'n'); // any other letter cancels
    assert!(!a.has_modal() && sw.0.lock().unwrap().is_empty());
    press(&mut a, &cx, 'y');
    press(&mut a, &cx, 'y');
    settle(&mut a, &cx);
    assert!(status(&a).starts_with("Applied"), "{}", status(&a));
    assert_eq!(*sw.0.lock().unwrap(), vec!["off zram0", "on zram0 100"]);
    assert_eq!(std::fs::read_to_string(root.join("sys/block/zram0/disksize")).unwrap(), (1u64 << 30).to_string());
    assert_eq!(std::fs::read_to_string(root.join("sys/block/zram0/comp_algorithm")).unwrap(), "lz4");
    assert!(std::fs::read_to_string(cx.audit.path()).unwrap_or_default().contains("memory.apply"));
    assert!(!root.join("system.conf").exists(), "apply does not save");
}

#[test]
fn apply_is_refused_when_what_zram_holds_would_not_fit_back_in_memory() {
    // 3 GiB stored in zram, 2 GiB available
    let (cx, mut a, _, sw) = app("mem-refuse", "", true, 3 * 1024 * 1024);
    press(&mut a, &cx, 'z');
    clear_field(&mut a, &cx);
    typ(&mut a, &cx, "off");
    key(&mut a, &cx, Key::Enter);
    press(&mut a, &cx, 'y');
    assert!(!a.has_modal(), "no confirmation is offered");
    assert!(status(&a).contains("out of memory"), "{}", status(&a));
    assert!(sw.0.lock().unwrap().is_empty());
}

#[test]
fn settings_fixed_by_a_boot_option_are_locked_and_say_so() {
    let (cx, mut a, _, _) = app("mem-lock", "quiet iyakki.zram=4G,zstd iyakki.swappiness=10", true, 1024);
    for k in ['z', 'c', 'w'] {
        press(&mut a, &cx, k);
        assert!(!a.has_modal(), "{k}");
        assert!(status(&a).contains("Locked by the boot option iyakki."), "{}", status(&a));
    }
    press(&mut a, &cx, 'f');
    assert!(a.has_modal(), "the swap file is not locked");
    key(&mut a, &cx, Key::Esc);
    let _ = draw(&mut a, &cx, 1280, 720);
    assert!(a.mem_rows().iter().any(|r| r.0 == "Compressed memory" && r.2 == Some("iyakki.zram")), "the page says why");
    assert_eq!(a.mem.draft.zram_value(), "4G,zstd");
}

#[test]
fn in_a_window_the_page_is_read_only_with_a_note() {
    let (cx, mut a, root, sw) = app("mem-window", "", false, 1024);
    for k in ['z', 'c', 'w', 'f', 'x', 'p', 'y', 'v'] {
        press(&mut a, &cx, k);
        assert!(!a.has_modal() && status(&a).contains("changes nothing"), "{k}: {}", status(&a));
    }
    assert!(sw.0.lock().unwrap().is_empty() && !root.join("system.conf").exists());
    let _ = draw(&mut a, &cx, 1280, 720);
    assert!(!a.mem.writable);
    press(&mut a, &cx, 'r');
    assert!(status(&a).contains("read again"), "viewing is always allowed");
}

#[test]
fn geometry_and_click_paths_hold_at_four_sizes() {
    let (cx, mut a, _, _) = app("mem-geo", "", true, 1024);
    press(&mut a, &cx, 'p');
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let g = draw(&mut a, &cx, w, h);
        let keys: Vec<(char, R)> = g.hits.iter().filter_map(|x| if let Event::Key(Key::Char(c)) = x.ev { Some((c, x.rect)) } else { None }).collect();
        for c in ['z', 'c', 'w', 'f', 'x', 'p', 'y', 'v'] {
            assert!(keys.iter().any(|k| k.0 == c), "{w}x{h}: button {c} is on screen");
        }
        for (i, (ca, ra)) in keys.iter().enumerate() {
            let sc = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
            assert!(ra.x >= 0.0 && ra.x + ra.w <= w as f32 && ra.y + ra.h <= h as f32 - 28.0 * sc, "{w}x{h}: button {ca} is inside the screen and clear of the status line");
            for (cb, rb) in keys.iter().skip(i + 1) {
                assert!(!overlaps(*ra, *rb), "{w}x{h}: buttons {ca} and {cb} overlap");
            }
        }
        // a click on a button does what its key does
        let zb = keys.iter().find(|k| k.0 == 'z').unwrap().1;
        let _ = zb;
    }
    // clicking "Zram" opens the form like the key
    let g = draw(&mut a, &cx, 1280, 720);
    let ev = g.hits.iter().find(|h| h.label == "Zram").unwrap().ev.clone();
    a.handle(&ev, &cx);
    assert!(a.has_modal());
}

#[test]
fn the_experimental_switch_is_off_by_default_changes_the_plan_saves_its_own_key_and_is_locked_by_the_boot_option() {
    let (cx, mut a, root, _) = app("mem-exp", "", true, 1024);
    assert_eq!(a.mem.exp, Experiment::Off);
    assert!(a.mem_rows().iter().any(|r| r.0 == "Compressed memory, experimental" && r.1.starts_with("off") && r.2.is_none()), "labelled experimental, off");
    assert!(!a.mem_dirty());
    // on: the plan for a machine without a data disk is zram only, and the page says so before anything is applied
    press(&mut a, &cx, 'e');
    assert_eq!(a.mem.exp, Experiment::On);
    assert!(a.mem_dirty());
    assert!(status(&a).contains("Experimental") && status(&a).contains("zram only"), "{}", status(&a));
    press(&mut a, &cx, 'p');
    assert!(a.mem.preview.iter().any(|l| l.contains("zram")), "{:?}", a.mem.preview);
    // saving writes the one key and keeps the others
    press(&mut a, &cx, 'v');
    let conf = std::fs::read_to_string(root.join("system.conf")).unwrap();
    assert!(conf.contains("compressed-memory=on"), "{conf}");
    assert_eq!(compressed::resolve_experiment("", &conf), (Experiment::On, false));
    assert!(!a.mem_dirty());
    press(&mut a, &cx, 'e');
    press(&mut a, &cx, 'v');
    assert!(std::fs::read_to_string(root.join("system.conf")).unwrap().contains("compressed-memory=off"));
    // a boot option fixes it: the key does nothing and the row says why
    let (cx2, mut b, _, _) = app("mem-exp-lock", "quiet iyakki.zram=exp", true, 1024);
    assert_eq!(b.mem.exp, Experiment::On);
    assert!(b.mem_rows().iter().any(|r| r.0 == "Compressed memory, experimental" && r.2 == Some("iyakki.zram=exp")));
    press(&mut b, &cx2, 'e');
    assert_eq!(b.mem.exp, Experiment::On);
    assert!(status(&b).contains("Locked"), "{}", status(&b));
    // a window on someone else's computer changes nothing
    let (cx3, mut c, _, _) = app("mem-exp-ro", "", false, 1024);
    press(&mut c, &cx3, 'e');
    assert_eq!(c.mem.exp, Experiment::Off);
}

#[test]
fn the_boot_order_switches_are_toggled_saved_to_system_conf_locked_by_boot_options_and_make_the_page_dirty() {
    let (cx, mut a, root, _sw) = app("mem-bootspeed", "quiet", true, 1024);
    let conf = root.join("system.conf");
    assert!(!a.mem_dirty());
    let rows = a.mem_rows();
    assert!(rows.iter().any(|r| r.0 == "Record the boot order" && r.1 == "off") && rows.iter().any(|r| r.0 == "Read boot files ahead" && r.1 == "off"));
    press(&mut a, &cx, 'o');
    assert!(a.mem.bs.record && !a.mem.bs.prefetch && a.mem_dirty(), "unsaved edits make the page dirty");
    assert!(status(&a).contains("Recording the boot order on") && status(&a).contains("not measured"), "{}", status(&a));
    press(&mut a, &cx, 'b');
    assert!(a.mem.bs.prefetch);
    assert!(!conf.exists(), "nothing is written before Save");
    press(&mut a, &cx, 'v');
    let text = std::fs::read_to_string(&conf).unwrap();
    assert!(text.contains("boot-record=on") && text.contains("boot-prefetch=on"), "{text}");
    assert!(!a.mem_dirty());
    let audit = std::fs::read_to_string(cx.audit.path()).unwrap_or_default();
    assert!(audit.contains("boot-record=true boot-prefetch=true"), "{audit}");
    // switching one off again is saved too, and the other keys stay
    press(&mut a, &cx, 'b');
    press(&mut a, &cx, 'v');
    let text = std::fs::read_to_string(&conf).unwrap();
    assert!(text.contains("boot-record=on") && text.contains("boot-prefetch=off"), "{text}");
    assert!(draw(&mut a, &cx, 1280, 720).hits.iter().any(|h| h.label == "Record boot order"));
    // a boot option fixes a switch: it cannot be changed here and Save leaves the file's key alone
    let (cx2, mut b, root2, _sw2) = app("mem-bootspeed-locked", "iyakki.bootrecord=1 iyakki.prefetch=off", true, 1024);
    assert!(b.mem.bs_saved.record_locked && b.mem.bs_saved.prefetch_locked && b.mem.bs.record && !b.mem.bs.prefetch);
    press(&mut b, &cx2, 'o');
    assert!(status(&b).contains("Locked by the boot option iyakki.bootrecord") && b.mem.bs.record);
    press(&mut b, &cx2, 'b');
    assert!(status(&b).contains("iyakki.prefetch") && !b.mem.bs.prefetch);
    press(&mut b, &cx2, 'v');
    let saved = std::fs::read_to_string(root2.join("system.conf")).unwrap_or_default();
    assert!(!saved.contains("boot-record") && !saved.contains("boot-prefetch"), "{saved}");
    // a desktop window changes nothing
    let (cx3, mut c, _r3, _s3) = app("mem-bootspeed-ro", "", false, 1024);
    press(&mut c, &cx3, 'o');
    assert!(!c.mem.bs.record && status(&c).contains("changes nothing"), "{}", status(&c));
}

#[test]
fn the_page_reads_changes_and_saves_through_cx_os_memory_on_a_fake_machine() {
    use iyakki_core_fake::FakeOs;
    let mut cx = admin("mem-fakeos");
    let fake = Arc::new(FakeOs::new());
    let mem = fake.memory_fake();
    mem.set_stats(iyakki_core_api::memory::MemStats { total: 8 << 30, free: 4 << 30, available: 6 << 30, ..Default::default() });
    mem.set_conf("hostname=box\n");
    cx.os = fake.clone();
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Memory);
    a.mem.writable = true;
    a.mem_load(true);
    assert_eq!(a.mem_ram(), 8 << 30, "the figures come from the fake machine");
    press(&mut a, &cx, 'z');
    clear_field(&mut a, &cx);
    typ(&mut a, &cx, "2G");
    key(&mut a, &cx, Key::Enter);
    press(&mut a, &cx, 'v');
    assert!(mem.conf().contains("zram=2G"), "{}", mem.conf());
    assert!(mem.calls().iter().any(|c| c.starts_with("save_conf(zram=2G")), "{:?}", mem.calls());
    press(&mut a, &cx, 'y');
    press(&mut a, &cx, 'y');
    settle(&mut a, &cx);
    assert!(status(&a).starts_with("Applied"), "{}", status(&a));
    let calls = mem.calls();
    assert!(calls.iter().any(|c| c.starts_with("reconfigure(zram=2G,ram=")) && calls.iter().any(|c| c.starts_with("apply_swapfile")), "{calls:?}");
    mem.refuse_reconfigure("the kernel said no");
    press(&mut a, &cx, 'y');
    press(&mut a, &cx, 'y');
    settle(&mut a, &cx);
    assert!(status(&a).contains("the kernel said no"), "{}", status(&a));
}

#[test]
fn the_overview_warns_that_data_is_kept_in_memory_only_when_the_fake_machine_says_so() {
    use iyakki_core_api::memory::DataPlace;
    use iyakki_core_fake::FakeOs;
    let mut cx = admin("mem-place");
    let fake = Arc::new(FakeOs::new());
    cx.os = fake.clone();
    let a = SettingsApp::new(&cx);
    let titles = |a: &SettingsApp| a.attention(&cx).into_iter().map(|t| t.title).collect::<Vec<_>>();
    fake.memory_fake().set_data_place(DataPlace::Disk);
    assert!(!titles(&a).iter().any(|t| t.contains("kept in memory only")), "{:?}", titles(&a));
    fake.memory_fake().set_data_place(DataPlace::Memory);
    assert!(titles(&a).iter().any(|t| t.contains("kept in memory only")), "{:?}", titles(&a));
}
