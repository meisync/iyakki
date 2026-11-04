//! The IYAKKI menu on its own: keys, clicks, the pages, and that it fits at four screen sizes. The shell tests drive it through
//! the top bar.

use super::*;
use crate::devpop::{Item as DevItem, Jump, Level, Snapshot};

const SIZES: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480)];

fn opened() -> BrandMenu {
    let mut m = BrandMenu::new();
    m.set_about(vec![("IYAKKI, an OS for agents".into(), crate::about::Style::Heading), ("Version\t0.1.0".into(), crate::about::Style::Fact)]);
    m.toggle();
    m
}

#[test]
fn keys_move_over_the_entries_open_pages_go_back_and_close() {
    let mut m = opened();
    assert!(m.open && m.page == Page::Menu && m.sel() == 0);
    assert_eq!(m.handle_key(&Key::Down), Outcome::Nothing);
    assert_eq!(m.sel(), 1);
    assert_eq!(m.handle_key(&Key::Up), Outcome::Nothing);
    assert_eq!(m.handle_key(&Key::Up), Outcome::Nothing, "wraps");
    assert_eq!(m.sel(), m.shown().len() - 1);
    assert!(!m.shown().iter().any(|e| e.0 == Page::Developer), "Developer mode is for administrators");
    assert_eq!(m.handle_key(&Key::Enter), Outcome::Jump("cloud", String::new()), "the last entry opens the Cloud dialog");
    assert_eq!(m.handle_key(&Key::Up), Outcome::Nothing);
    assert_eq!(m.handle_key(&Key::Enter), Outcome::Jump("settings", String::new()), "the gear entry opens Settings");
    assert_eq!(m.handle_key(&Key::Up), Outcome::Nothing);
    assert_eq!(m.handle_key(&Key::Enter), Outcome::Opened(Page::Mac));
    assert_eq!(m.handle_key(&Key::Esc), Outcome::Nothing, "Esc goes back a page first");
    assert_eq!((m.page, m.sel()), (Page::Menu, 4), "back on the entry it came from");
    // the letters
    // the old Quick install page is gone: its entry (and key) opens the Store on the Setup tab
    assert_eq!(m.handle_key(&Key::Char('q')), Outcome::Jump("store", "setup".into()));
    assert_eq!(m.page, Page::Menu, "no page of its own is opened");
    assert_eq!(m.handle_key(&Key::Char('A')), Outcome::Opened(Page::About));
    m.handle_key(&Key::Esc);
    assert_eq!(m.handle_key(&Key::Char('s')), Outcome::Jump("settings", String::new()));
    assert_eq!(m.handle_hit(&Event::Tab(ENTRY + 5)), Outcome::Jump("settings", String::new()));
    assert_eq!(m.handle_key(&Key::Char('c')), Outcome::Jump("cloud", String::new()), "c opens Cloud");
    assert_eq!(m.handle_key(&Key::Char('z')), Outcome::Nothing);
    assert_eq!(m.handle_key(&Key::Esc), Outcome::Close, "Esc on the first page closes");
    assert_eq!(m.handle_key(&Key::F(10)), Outcome::Close);
}

#[test]
fn quick_install_is_an_entry_that_opens_the_store_setup_tab_and_no_page_of_it_remains() {
    let mut m = opened();
    let i = m.shown().iter().position(|e| e.3 == 'q').expect("the entry is still listed, so old habits work");
    assert_eq!(m.shown()[i].1, "Quick install");
    assert!(m.shown()[i].2.contains("Store"), "it says where it goes: {}", m.shown()[i].2);
    assert_eq!(m.handle_hit(&Event::Tab(ENTRY + i)), Outcome::Jump("store", "setup".into()));
    assert_eq!(m.open_page(Page::Setup), Outcome::Jump("store", "setup".into()));
    assert_eq!(m.page, Page::Menu);
    // the GPU page's way back is the Setup tab too
    m.open_page(Page::Gpu);
    assert_eq!(m.handle_key(&Key::Esc), Outcome::Jump("store", "setup".into()));
    // rendering the menu never draws an install list
    let mut g = Gfx::new(1280, 720);
    m.render(&mut g, 1280, 720, 0.9, 0);
    assert!(!g.hits.iter().any(|h| h.label == "Installer runs"));
}

#[test]
fn the_media_page_takes_typing_plays_the_file_and_explains_a_missing_player() {
    let mut m = opened();
    assert_eq!(m.handle_key(&Key::Char('m')), Outcome::Opened(Page::Media));
    assert_eq!(m.handle_key(&Key::Enter), Outcome::Nothing, "nothing typed yet");
    assert!(m.note.is_some());
    for c in "/tmp/a.mp4".chars() {
        assert_eq!(m.handle_key(&Key::Char(c)), Outcome::Nothing, "letters are typed, not shortcuts");
    }
    assert_eq!(m.handle_key(&Key::Enter), Outcome::Play("/tmp/a.mp4".into()));
    assert_eq!(m.handle_hit(&Event::Tab(STOP)), Outcome::StopMedia);
    assert_eq!(m.handle_hit(&Event::Tab(OPEN_MEDIA)), Outcome::Jump("media", String::new()));
    m.handle_hit(&Event::Paste(" x".into()));
    assert_eq!(m.path.value, "/tmp/a.mp4 x");
    // players: which one is used, and what to do without one
    let found = players_found(&|p: &str| (p == "mpv" || p == "ffplay").then(|| format!("/usr/bin/{p}")));
    assert_eq!(found.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), ["ffplay", "mpv"], "ffplay first");
    let (t, ok) = players_text(&found);
    assert!(ok && t.starts_with("Player: ffplay (/usr/bin/ffplay)") && t.contains("also mpv"), "{t}");
    let (t, ok) = players_text(&[]);
    assert!(!ok && t.contains("Install the media player") && t.contains("Setup") && t.contains("MP3"), "{t}");
    let own = players_found(&|p: &str| (p == "ffmpeg" || p == "mpv").then(|| format!("/data/cli/bin/{p}")));
    assert_eq!(own[0].0, "ffmpeg", "the in-house player comes first");
    let (t, ok) = players_text(&own);
    assert!(ok && t.contains("IYAKKI's own") && t.contains("/data/cli/bin/ffmpeg"), "{t}");
    // without ffmpeg the page offers to start the install job
    assert_eq!(m.handle_hit(&Event::Tab(INSTALL_PLAYER)), Outcome::Install(Item::Ffmpeg));
    m.players.clear();
    let mut g = Gfx::new(1280, 720);
    m.render(&mut g, 1280, 720, 0.9, 0);
    assert!(g.hits.iter().any(|h| h.label == "Install the media player"), "{:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
}

#[test]
fn the_mac_page_reports_wifi_t2_audio_bluetooth_and_the_camera_and_leads_to_the_drivers() {
    let mut m = opened();
    assert_eq!(m.handle_key(&Key::Char('d')), Outcome::Opened(Page::Mac));
    assert_eq!(m.handle_key(&Key::Enter), Outcome::Jump("store", "drivers".into()));
    assert_eq!(m.handle_key(&Key::Char('d')), Outcome::Jump("store", "drivers".into()));
    assert_eq!(m.handle_hit(&Event::Tab(DEVICES)), Outcome::Jump("devices", String::new()));
    let looking = mac_rows(None);
    assert_eq!(looking.len(), 4);
    assert!(looking.iter().all(|r| r.state == "looking..."));
    let it = |t: &str, d: &str, l: Level| DevItem { title: t.into(), detail: d.into(), level: l, jump: Jump::None, acts: vec![] };
    let snap = Snapshot { items: vec![it("Wi-Fi", "a radio is there but no driver has taken it", Level::Missing), it("Sound", "output Apple T2 Audio Speaker (automatic)", Level::Ok), it("Camera", "USB Camera", Level::Ok)], driver_help: vec![("Broadcom Wi-Fi firmware".into(), "firmware missing".into())], at: 1 };
    let rows = mac_rows(Some(&snap));
    assert_eq!(rows.iter().map(|r| r.title).collect::<Vec<_>>(), ["Wi-Fi firmware", "T2 audio", "Bluetooth", "USB webcam"]);
    assert_eq!(rows[0].level, Level::Missing);
    assert!(rows[0].state.contains("firmware missing") && rows[0].how.contains("mac-wifi-firmware-from-macos.sh") && rows[0].how.contains("Apple's download"));
    assert!(rows[1].state.contains("Apple T2 Audio"));
    assert_eq!((rows[2].state.as_str(), rows[2].level), ("not found", Level::Missing));
    assert_eq!(rows[3].state, "USB Camera");
}

#[test]
fn the_about_page_scrolls_and_stays_inside_its_lines() {
    let mut m = BrandMenu::new();
    let mut lines = vec![("IYAKKI, an OS for agents".to_string(), crate::about::Style::Heading)];
    lines.extend((0..200).map(|i| (format!("crate-{i} 1.0.0"), crate::about::Style::Item)));
    m.set_about(lines);
    m.toggle();
    m.handle_key(&Key::Char('a'));
    let mut g = Gfx::new(1280, 720);
    m.render(&mut g, 1280, 720, 0.9, 0);
    let first = m.scroll;
    m.handle_key(&Key::PageDown);
    assert!(m.scroll > first);
    m.handle_hit(&Event::Scroll(-1));
    let after_wheel = m.scroll;
    m.handle_key(&Key::End);
    m.render(&mut g, 1280, 720, 0.9, 0);
    let end = m.scroll;
    assert!(end > after_wheel);
    m.handle_key(&Key::Down);
    assert_eq!(m.scroll, end, "never past the end");
    m.handle_key(&Key::Home);
    assert_eq!(m.scroll, 0);
}

#[test]
fn every_page_fits_on_screen_with_clickable_regions_inside_the_panel_at_four_sizes() {
    for (w, h) in SIZES {
        let s = crate::shell::scale_for(w, h);
        for page in [Page::Menu, Page::About, Page::Media, Page::Mac] {
          for sel in [0usize, 10] {
            let mut m = BrandMenu::new();
            m.set_about(crate::about::this_machine());
            m.toggle();
            m.anchor = (40.0 * s, 76.0 * s);
            m.open_page(page);
            m.sel = if sel == 0 { 0 } else { sel.min(m.shown().len() - 1) };
            let mut g = Gfx::new(w, h);
            m.render(&mut g, w, h, s, 0);
            let p = m.panel(w, h, s);
            assert!(p.x >= 0.0 && p.y >= 0.0 && p.x + p.w <= w as f32 + 0.5 && p.y + p.h <= h as f32 + 0.5, "{w}x{h} {page:?}: {p:?}");
            assert!(!g.hits.is_empty(), "{w}x{h} {page:?}");
            for hit in &g.hits {
                let r = hit.rect;
                assert!(r.x >= p.x - 0.5 && r.y >= p.y - 0.5 && r.x + r.w <= p.x + p.w + 0.5 && r.y + r.h <= p.y + p.h + 0.5, "{w}x{h} {page:?}: {} {r:?} outside {p:?}", hit.label);
            }
            assert!(g.hits.iter().any(|h| h.ev == Event::Tab(CLOSE)), "{w}x{h} {page:?}: Close");
            if page == Page::Menu {
                for i in 0..m.shown().len() {
                    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(ENTRY + i) && h.rect.h >= 30.0), "{w}x{h}: entry {i}");
                }
            }
            if page == Page::Mac {
                assert!(g.hits.iter().any(|h| h.ev == Event::Tab(DRIVERS)), "{w}x{h}: Drivers button");
                for id in [MAC_DOWNLOAD, MAC_FOLDER, MAC_IMAGE, MAC_PROMPT, DEVICES] {
                    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(id) && h.rect.h >= 24.0 * s), "{w}x{h}: button {id}");
                }
                assert!(g.hits.iter().any(|h| h.ev == Event::Focus(MAC_PATH) && h.rect.h >= 28.0 * s), "{w}x{h}: the path field");
            }
          }
        }
    }
}

#[test]
fn an_administrator_sees_developer_mode_and_it_opens_its_settings_page() {
    let mut m = opened();
    m.admin = true;
    let i = m.shown().iter().position(|e| e.0 == Page::Developer).expect("listed for an administrator");
    assert_eq!(m.handle_hit(&Event::Tab(ENTRY + i)), Outcome::Jump("settings", "tab:developer".into()));
    assert_eq!(m.handle_key(&Key::Char('v')), Outcome::Jump("settings", "tab:developer".into()));
}

fn mac_snapshot() -> Snapshot {
    let it = |t: &str, d: &str, l: Level| DevItem { title: t.into(), detail: d.into(), level: l, jump: Jump::None, acts: vec![] };
    Snapshot { items: vec![it("Wi-Fi", "no Wi-Fi interface found", Level::Missing), it("Sound", "Apple T2 Audio Speaker", Level::Ok), it("Bluetooth", "no adapter found", Level::Missing), it("Camera", "none found", Level::Absent)], driver_help: vec![], at: 1 }
}

#[test]
fn the_mac_page_starts_the_firmware_job_from_apple_a_folder_or_a_disk_image_and_copies_the_prompt() {
    use iyakki_agent::macfw::Source;
    let mut m = opened();
    m.open_page(Page::Mac);
    m.mac = Some(mac_snapshot());
    assert_eq!(m.handle_hit(&Event::Tab(MAC_DOWNLOAD)), Outcome::MacFirmware(Source::Download));
    assert_eq!(m.handle_key(&Key::Char('a')), Outcome::MacFirmware(Source::Download));
    // the path field: empty -> a hint and the field takes focus
    assert_eq!(m.handle_hit(&Event::Tab(MAC_FOLDER)), Outcome::Nothing);
    assert!(m.note.as_ref().unwrap().0.contains("Click the field"));
    // a folder
    let dir = std::env::temp_dir().join(format!("iyakki-brand-mac-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("macOS.dmg");
    std::fs::write(&img, b"x").unwrap();
    m.handle_hit(&Event::FocusAt(MAC_PATH, 4.0));
    m.handle_hit(&Event::Paste(dir.display().to_string()));
    assert_eq!(m.mac_path.value, dir.display().to_string());
    assert_eq!(m.handle_key(&Key::Enter), Outcome::MacFirmware(Source::Folder(dir.clone())), "Enter in the field: a folder is a folder");
    m.handle_hit(&Event::FocusAt(MAC_PATH, 4.0));
    m.mac_path.value = img.display().to_string();
    assert_eq!(m.handle_key(&Key::Enter), Outcome::MacFirmware(Source::Image(img.clone())), "a file is a disk image");
    // the wrong kind is said, nothing starts
    assert_eq!(m.handle_hit(&Event::Tab(MAC_FOLDER)), Outcome::Nothing);
    assert!(m.note.as_ref().unwrap().0.contains("is not a folder"));
    m.mac_path.value = dir.display().to_string();
    assert_eq!(m.handle_hit(&Event::Tab(MAC_IMAGE)), Outcome::Nothing);
    assert!(m.note.as_ref().unwrap().0.contains("is not a file"));
    m.mac_path.value = "/no/such/place".into();
    assert_eq!(m.handle_hit(&Event::Tab(MAC_FOLDER)), Outcome::Nothing);
    // typing goes to the field only while it has the focus; Esc lets go, the letter keys work again
    m.handle_hit(&Event::FocusAt(MAC_PATH, 0.0));
    assert_eq!(m.handle_key(&Key::Char('d')), Outcome::Nothing);
    assert!(m.mac_path.value.ends_with('d') || m.mac_path.value.contains('d'));
    assert_eq!(m.handle_key(&Key::Esc), Outcome::Nothing);
    assert_eq!(m.page, Page::Mac, "Esc only leaves the field");
    assert_eq!(m.handle_key(&Key::Char('d')), Outcome::Jump("store", "drivers".into()));
    // one run at a time
    m.mac_busy = true;
    assert_eq!(m.handle_hit(&Event::Tab(MAC_DOWNLOAD)), Outcome::Nothing);
    assert!(m.note.as_ref().unwrap().0.contains("being installed already"));
    // the prompt button copies the prompt text
    m.handle_hit(&Event::Tab(MAC_PROMPT));
    assert!(m.note.as_ref().unwrap().0.starts_with("Copied") && m.note.as_ref().unwrap().1);
    assert!(iyakki_agent::macfw::CHAT_PROMPT.contains("GOAL:") && iyakki_agent::macfw::CHAT_PROMPT.contains("DO NOT:") && iyakki_agent::macfw::CHAT_PROMPT.contains("Mac drivers"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_mac_page_says_whose_files_these_are_and_which_model_and_board_id_the_download_asks_for() {
    let t = mac_notice("Macmini8,1", "");
    assert!(t.contains("These files are Apple's") && t.contains("never uploaded") && t.contains("not part of IYAKKI"), "{t}");
    assert!(t.contains("Mac-7BA5B2DFE22DDD8C") && !t.contains("not yet tried") && !t.contains("no T2 chip"));
    assert!(mac_notice("MacBookPro15,1", "").contains("not yet tried on a real one"));
    assert!(mac_notice("Macmini7,1", "").contains("not in IYAKKI's table") && mac_notice("Macmini7,1", "").contains("Download is not offered"));
    assert_eq!(mac_notice("", ""), iyakki_agent::macfw::NOTICE, "no DMI: just the notice");
    // the 2017 13-inch MacBook Pro: no T2 chip, its board id is from its own DMI and verified
    let t = mac_notice("MacBookPro14,1", "Mac-B4831CEBD52A0C4C");
    assert!(t.contains("MacBookPro14,1") && t.contains("Mac-B4831CEBD52A0C4C") && t.contains("This Mac has no T2 chip.") && !t.contains("not yet tried"), "{t}");
    // a Mac that is not in the table is allowed, with its own board id, and told so
    let t = mac_notice("iMac18,3", "Mac-77EB7D7DAF985301");
    assert!(t.contains("not on IYAKKI's verified list") && t.contains("Mac-77EB7D7DAF985301") && t.contains("no T2 chip") && !t.contains("Download is not offered"), "{t}");
    // no table entry and no readable board name: refused, with the way out
    assert!(mac_notice("iMac18,3", "garbage").contains("Download is not offered"));
}

#[test]
fn the_mac_rows_for_a_mac_without_t2_say_that_wifi_works_without_apples_files() {
    let t2 = mac_rows_for(Some(&mac_snapshot()), true);
    let no = mac_rows_for(Some(&mac_snapshot()), false);
    assert!(t2[0].how.contains("Apple's firmware is not shipped") && !t2[0].how.contains("no T2 chip"));
    assert!(no[0].how.contains("no T2 chip") && no[0].how.contains("works without") && no[0].how.contains("NVRAM and CLM"), "{}", no[0].how);
    assert!(no[2].how.contains("UART chip") && no[2].how.contains("ROM state") && !no[2].how.contains("Mac mini"), "{}", no[2].how);
    assert_eq!(mac_rows(None).len(), mac_rows_for(None, false).len());
}

#[test]
fn the_mac_page_fits_with_a_result_a_running_job_and_a_long_model_at_four_sizes() {
    for (w, h) in SIZES {
        let s = crate::shell::scale_for(w, h);
        for variant in 0..4 {
            let mut m = opened();
            m.anchor = (40.0 * s, 76.0 * s);
            m.open_page(Page::Mac);
            m.mac = Some(mac_snapshot());
            m.mac_model = ["Macmini8,1", "MacBookPro14,1", "Macmini7,1", ""][variant].into();
            m.mac_board = ["", "Mac-B4831CEBD52A0C4C", "", ""][variant].into();
            m.mac_last = (variant % 2 == 0).then(|| iyakki_agent::macfw::Outcome { installed: true, headline: "Wi-Fi: wlan0 appeared \u{b7} Bluetooth firmware is installed, but the controller still has no address".into(), ..Default::default() });
            m.mac_busy = variant == 1;
            m.mac_bt_held = variant != 3;
            m.mac_leftover = if variant == 2 { 2_300_000_000 } else { 0 };
            m.note = Some(("x".repeat(300), false));
            let mut g = Gfx::new(w, h);
            m.render(&mut g, w, h, s, 0);
            let p = m.panel(w, h, s);
            for hit in &g.hits {
                let r = hit.rect;
                assert!(r.x >= p.x - 0.5 && r.y >= p.y - 0.5 && r.x + r.w <= p.x + p.w + 0.5 && r.y + r.h <= p.y + p.h + 0.5, "{w}x{h} v{variant}: {} {r:?} outside {p:?}", hit.label);
            }
            assert!(g.hits.iter().any(|h| h.ev == Event::Tab(MAC_DOWNLOAD)), "{w}x{h} v{variant}");
            assert_eq!(g.hits.iter().any(|h| h.ev == Event::Tab(MAC_BTLOAD)), variant != 3, "{w}x{h} v{variant}: the Bluetooth button is there while the drivers are held");
            // the buttons do not overlap each other
            let b: Vec<_> = g.hits.iter().filter(|h| matches!(h.ev, Event::Tab(MAC_DOWNLOAD | MAC_FOLDER | MAC_IMAGE | MAC_PROMPT | MAC_CLEAN | MAC_BTLOAD | DRIVERS | DEVICES))).collect();
            for (i, a) in b.iter().enumerate() {
                for c in &b[i + 1..] {
                    let overlap = a.rect.x < c.rect.x + c.rect.w - 0.5 && c.rect.x < a.rect.x + a.rect.w - 0.5 && a.rect.y < c.rect.y + c.rect.h - 0.5 && c.rect.y < a.rect.y + a.rect.h - 0.5;
                    assert!(!overlap, "{w}x{h} v{variant}: {} overlaps {}", a.label, c.label);
                }
            }
        }
    }
}

#[test]
fn a_kept_download_shows_its_size_on_a_delete_button_that_asks_the_shell_to_clear_it() {
    let (w, h) = (1280, 720);
    let s = crate::shell::scale_for(w, h);
    let mut m = opened();
    m.anchor = (40.0 * s, 76.0 * s);
    m.open_page(Page::Mac);
    m.mac = Some(mac_snapshot());
    let mut g = Gfx::new(w, h);
    m.render(&mut g, w, h, s, 0);
    assert!(!g.hits.iter().any(|x| x.ev == Event::Tab(MAC_CLEAN)), "nothing kept, no button");
    assert_eq!(m.handle_hit(&Event::Tab(MAC_CLEAN)), Outcome::Nothing);
    m.mac_leftover = 2_300_000_000;
    let mut g = Gfx::new(w, h);
    m.render(&mut g, w, h, s, 0);
    let b = g.hits.iter().find(|x| x.ev == Event::Tab(MAC_CLEAN)).expect("the button");
    assert!(b.label.contains("Delete download") && b.label.contains("2193 MB"), "{}", b.label);
    assert_eq!(m.handle_hit(&Event::Tab(MAC_CLEAN)), Outcome::MacClean);
    m.mac_busy = true;
    assert_eq!(m.handle_hit(&Event::Tab(MAC_CLEAN)), Outcome::Nothing, "not while a job runs");
}

#[test]
fn load_the_bluetooth_driver_anyway_is_offered_only_while_the_drivers_are_held_and_says_what_the_chip_lacks() {
    let mut m = opened();
    m.open_page(Page::Mac);
    m.mac = Some(mac_snapshot());
    assert_eq!(m.handle_hit(&Event::Tab(MAC_BTLOAD)), Outcome::Nothing, "nothing held: the button does nothing");
    assert_eq!(m.handle_key(&Key::Char('l')), Outcome::Nothing);
    assert!(m.note.as_ref().unwrap().0.contains("nothing to load"));
    m.mac_bt_held = true;
    assert_eq!(m.handle_hit(&Event::Tab(MAC_BTLOAD)), Outcome::MacBtLoad);
    assert_eq!(m.handle_key(&Key::Char('l')), Outcome::MacBtLoad);
    // the Bluetooth row of the page says plainly what macOS lacks for the BCM4364
    let rows = mac_rows(Some(&mac_snapshot()));
    let bt = rows.iter().find(|r| r.title == "Bluetooth").unwrap();
    assert!(bt.how.contains("has none in macOS") && bt.how.contains("ROM state (limited)") && bt.how.contains("USB Bluetooth adapter is the reliable option"), "{}", bt.how);
    for w in ["firmware file in macOS", "ROM state (limited)", "USB Bluetooth adapter is the reliable option"] {
        assert!(iyakki_agent::macfw::NO_BCM4364_FILE.contains(w), "{w}");
    }
    assert!(iyakki_agent::macfw::LOAD_ANYWAY_HELP.contains("cannot pair"));
}
