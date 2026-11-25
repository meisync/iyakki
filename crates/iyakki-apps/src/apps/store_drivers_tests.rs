//! The Drivers chip tests: the machines are made of fake sysfs folders read by the agent's driver code (the real pack side).

use super::*;
use iyakki_agent::store::drivers::Drivers;
use super::super::store_port::CoreDrivers;

/// The core read side of a made-up machine, as the Drivers chip reads it.
fn view(d: Drivers) -> CoreDrivers {
    CoreDrivers(std::sync::Arc::new(d))
}
use iyakki_core_api::device_types::driver_packs::Roots;

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
    let s = crate::ui::scale_for(w, h);
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
    p.refresh(&view(mini("list")));
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
    p.refresh(&view(pc));
    assert!(p.rows.is_empty());
    for (w, h) in SIZES {
        let g = draw(&mut p, w, h);
        assert!(g.hits.iter().all(|x| x.rect.x + x.rect.w <= w as f32 + 1.0));
    }
    let mbp = fake("mbp", "MacBookPro11,1\n", &[("sys/bus/pci/devices/0000:02:00.0/vendor", "0x14e4\n"), ("sys/bus/pci/devices/0000:02:00.0/device", "0x1570\n")]);
    p.refresh(&view(mbp));
    let cam = p.rows.iter().position(|r| r.id == "mac-facetimehd").expect("camera listed");
    assert_eq!(p.rows[cam].install_label(), "From file…");
    p.handle(&Event::Tab(BASE + cam * 4 + ACT_WHY));
    assert_eq!(p.why, Some(cam));
    assert!(p.rows[cam].why.iter().any(|l| l.contains("Apple's licence")) || p.rows[cam].why.iter().any(|l| l.contains("Apple's software")));
}

#[test]
fn buttons_and_keys_report_what_was_asked_and_why_opens_and_closes() {
    let mut p = Pane::default();
    p.refresh(&view(mini("keys")));
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
    p.refresh(&view(mini("geo")));
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
