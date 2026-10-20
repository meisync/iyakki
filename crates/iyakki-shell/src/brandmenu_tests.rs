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
    assert_eq!(m.handle_key(&Key::Enter), Outcome::Jump("settings", String::new()), "the gear entry opens Settings");
    assert_eq!(m.handle_key(&Key::Up), Outcome::Nothing);
    assert_eq!(m.handle_key(&Key::Enter), Outcome::Opened(Page::Mac));
    assert_eq!(m.handle_key(&Key::Esc), Outcome::Nothing, "Esc goes back a page first");
    assert_eq!((m.page, m.sel()), (Page::Menu, 3), "back on the entry it came from");
    // the letters
    assert_eq!(m.handle_key(&Key::Char('q')), Outcome::Opened(Page::Install));
    m.handle_key(&Key::Esc);
    assert_eq!(m.handle_key(&Key::Char('A')), Outcome::Opened(Page::About));
    m.handle_key(&Key::Esc);
    assert_eq!(m.handle_key(&Key::Char('s')), Outcome::Jump("settings", String::new()));
    assert_eq!(m.handle_hit(&Event::Tab(ENTRY + 4)), Outcome::Jump("settings", String::new()));
    assert_eq!(m.handle_key(&Key::Char('z')), Outcome::Nothing);
    assert_eq!(m.handle_key(&Key::Esc), Outcome::Close, "Esc on the first page closes");
    assert_eq!(m.handle_key(&Key::F(10)), Outcome::Close);
}

fn row(i: Item) -> usize {
    Item::ALL.iter().position(|x| *x == i).unwrap()
}

#[test]
fn quick_install_shows_the_order_installs_the_chosen_item_and_refuses_in_a_window() {
    let mut m = opened();
    m.handle_key(&Key::Char('q'));
    m.presence = Item::ALL.iter().map(|i| (*i, Presence::Missing)).collect();
    // the first row is the first runtime; choose Elasticsearch by clicking its row
    assert_eq!(m.install_order(), vec![Item::Node]);
    assert_eq!(m.handle_hit(&Event::Tab(IROW + row(Item::Elasticsearch))), Outcome::Nothing);
    assert_eq!(m.install_order(), vec![Item::K3s, Item::Elasticsearch], "k3s first");
    assert_eq!(m.handle_key(&Key::Enter), Outcome::Install(Item::Elasticsearch));
    assert_eq!(m.handle_key(&Key::Char('i')), Outcome::Install(Item::Elasticsearch));
    assert_eq!(m.handle_key(&Key::Char('r')), Outcome::ShowInstalls);
    m.presence[row(Item::K3s)].1 = Presence::Installed("native k3s v1.31".into());
    assert_eq!(m.install_order(), vec![Item::Elasticsearch], "k3s there: not again");
    // clicks: a row chooses, the button installs
    assert_eq!(m.handle_hit(&Event::Tab(IROW + row(Item::DuckDb))), Outcome::Nothing);
    assert_eq!(m.handle_hit(&Event::Tab(INSTALL_BTN)), Outcome::Install(Item::DuckDb));
    // a desktop window never installs a cluster (the other sections still install)
    m.window = true;
    assert_eq!(m.handle_key(&Key::Enter), Outcome::Nothing);
    assert!(m.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("desktop window")));
    assert_eq!(m.handle_hit(&Event::Tab(IROW + row(Item::ClaudeCode))), Outcome::Nothing);
    assert_eq!(m.handle_key(&Key::Enter), Outcome::Install(Item::ClaudeCode));
    // the voice pair: Kokoro brings Lemonade (and k3s under it) first
    m.handle_hit(&Event::Tab(IROW + row(Item::Kokoro)));
    m.presence[row(Item::K3s)].1 = Presence::Missing;
    assert_eq!(m.install_order(), vec![Item::K3s, Item::Lemonade, Item::Kokoro]);
    // a failed install is offered again as Retry; an installed one with a newer version as Upgrade
    m.handle_hit(&Event::Tab(IROW + row(Item::DuckDb)));
    m.presence[row(Item::DuckDb)].1 = Presence::Installed("version v1.1.0".into());
    m.latest = vec![(Item::DuckDb, Latest::Newer("v1.2.0".into()))];
    let mut g = Gfx::new(1280, 720);
    m.window = false;
    m.render(&mut g, 1280, 720, 0.9, 0);
    assert!(g.hits.iter().any(|h| h.label == "Upgrade DuckDB to 1.2.0"), "{:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    m.failed = vec![Item::DuckDb];
    m.render(&mut g, 1280, 720, 0.9, 0);
    assert!(g.hits.iter().any(|h| h.label == "Retry DuckDB"));
}

#[test]
fn quick_install_has_a_media_section_whose_button_installs_the_player_even_in_a_window() {
    let mut m = opened();
    m.handle_key(&Key::Char('q'));
    m.presence = Item::ALL.iter().map(|i| (*i, Presence::Missing)).collect();
    m.window = true;
    assert_eq!(m.handle_hit(&Event::Tab(IROW + row(Item::Ffmpeg))), Outcome::Nothing);
    assert_eq!(m.install_order(), vec![Item::Ffmpeg], "no cluster, no runtime first");
    let mut g = Gfx::new(1280, 720);
    m.render(&mut g, 1280, 720, 0.9, 0);
    let labels: Vec<String> = g.hits.iter().map(|h| h.label.clone()).collect();
    assert!(labels.iter().any(|l| l == "Install Media player (ffmpeg)"), "{labels:?}");
    assert_eq!(m.handle_hit(&Event::Tab(INSTALL_BTN)), Outcome::Install(Item::Ffmpeg));
    m.presence[row(Item::Ffmpeg)].1 = Presence::Installed("version latest".into());
    m.render(&mut g, 1280, 720, 0.9, 0);
    assert!(g.hits.iter().any(|h| h.label.starts_with("Update Media player")));
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
    assert!(!ok && t.contains("Install the media player") && t.contains("Quick install") && t.contains("MP3"), "{t}");
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
    assert!(rows[0].state.contains("firmware missing") && rows[0].how.contains("mac-wifi-firmware-from-macos.sh") && rows[0].how.contains("Install from folder or USB"));
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
        for page in [Page::Menu, Page::About, Page::Install, Page::Media, Page::Mac] {
          for sel in [0usize, 10] {
            let mut m = BrandMenu::new();
            m.set_about(crate::about::this_machine());
            m.toggle();
            m.anchor = (40.0 * s, 76.0 * s);
            m.open_page(page);
            m.sel = if page == Page::Install { sel } else { 0 };
            m.presence = Item::ALL.iter().map(|i| (*i, Presence::Missing)).collect();
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
            if page == Page::Install {
                assert!(g.hits.iter().any(|h| h.ev == Event::Tab(INSTALL_BTN)), "{w}x{h}: Install button");
                assert!(g.hits.iter().any(|h| h.ev == Event::Tab(IROW + sel)), "{w}x{h}: the chosen row {sel} is in view");
            }
            if page == Page::Mac {
                assert!(g.hits.iter().any(|h| h.ev == Event::Tab(DRIVERS)), "{w}x{h}: Drivers button");
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

#[test]
fn a_tool_found_on_the_machine_is_shown_as_installed_and_replaced_only_when_asked_twice() {
    let mut m = opened();
    m.handle_key(&Key::Char('q'));
    m.presence = Item::ALL.iter().map(|i| (*i, Presence::Missing)).collect();
    let i = row(Item::Node);
    m.presence[i].1 = Presence::Elsewhere("v26.12.0 (/var/lib/iyakki/cli/bin/node)".into());
    m.handle_hit(&Event::Tab(IROW + i));
    // first press: a note, nothing installed
    assert_eq!(m.handle_key(&Key::Enter), Outcome::Nothing);
    assert!(m.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("leaves it alone")), "{:?}", m.note);
    // second press: the deliberate choice
    assert_eq!(m.handle_key(&Key::Enter), Outcome::Install(Item::Node));
    // the row says it is there with the version
    let mut g = Gfx::new(1280, 720);
    m.render(&mut g, 1280, 720, 0.9, 0);
    assert!(g.hits.iter().any(|h| h.label == "Install Node.js (own copy)"), "{:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
}

#[test]
fn a_tool_that_cannot_run_is_flagged_and_offered_a_repair_and_the_order_names_uv_and_python() {
    let mut m = opened();
    m.handle_key(&Key::Char('q'));
    m.presence = Item::ALL.iter().map(|i| (*i, Presence::Missing)).collect();
    let d = row(Item::DuckDb);
    m.presence[d].1 = Presence::Installed("version v1.1.0".into());
    m.unhealthy = vec![(Item::DuckDb, "DuckDB is installed but cannot run here: no such library".into())];
    m.handle_hit(&Event::Tab(IROW + d));
    let mut g = Gfx::new(1280, 720);
    m.render(&mut g, 1280, 720, 0.9, 0);
    assert!(g.hits.iter().any(|h| h.label == "Reinstall DuckDB (repair)"), "{:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    // DuckDB on a machine with nothing: k3s, then uv and Python 3 (what the catalogue installs under it), then DuckDB
    m.unhealthy.clear();
    m.presence[d].1 = Presence::Missing;
    m.runtimes = vec![(Item::Python, vec!["uv"])];
    let titles = quickinstall::order_titles_with(&m.install_order(), &|i| m.runtimes.iter().find(|(x, _)| *x == i).map(|(_, v)| v.clone()).unwrap_or_default());
    assert_eq!(titles, ["k3s", "uv", "Python 3", "DuckDB"]);
}

#[test]
fn quick_install_shows_login_and_the_account_only_for_an_installed_coding_agent() {
    let mut m = opened();
    m.handle_key(&Key::Char('q'));
    m.presence = Item::ALL.iter().map(|i| (*i, Presence::Missing)).collect();
    let draw = |m: &mut BrandMenu| {
        let mut g = Gfx::new(1280, 720);
        g.clear(crate::gfx::rgb(0, 0, 0));
        m.render(&mut g, 1280, 720, 1.0, 0);
        g
    };
    m.handle_hit(&Event::Tab(IROW + row(Item::ClaudeCode)));
    assert!(!draw(&mut m).hits.iter().any(|h| h.ev == Event::Tab(crate::agent_login::LOGIN_TAB)), "not installed: no Login");
    m.presence[row(Item::ClaudeCode)].1 = Presence::Installed("version 2.1.7".into());
    m.signin = vec![(Item::ClaudeCode, crate::agent_login::State::SignedOut)];
    let g = draw(&mut m);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(crate::agent_login::LOGIN_TAB)), "installed: Login");
    assert!(!g.hits.iter().any(|h| h.ev == Event::Tab(crate::agent_login::LOGOUT_TAB)), "nobody signed in: no Logout");
    m.signin = vec![(Item::ClaudeCode, crate::agent_login::State::SignedIn("me@example.test".into()))];
    let g = draw(&mut m);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(crate::agent_login::LOGOUT_TAB)), "signed in: Logout too");
    // Copilot installed, nothing known yet: Login for the other agent
    m.handle_hit(&Event::Tab(IROW + row(Item::Copilot)));
    m.presence[row(Item::Copilot)].1 = Presence::Installed("version 0.0.400".into());
    let g = draw(&mut m);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(crate::agent_login::LOGIN_TAB + 1)));
    // an unrelated item never gets one
    m.handle_hit(&Event::Tab(IROW + row(Item::DuckDb)));
    assert!(!draw(&mut m).hits.iter().any(|h| matches!(h.ev, Event::Tab(n) if crate::agent_login::agent_of_tab(n).is_some())));
}

#[test]
fn the_login_buttons_stay_inside_the_panel_at_every_size() {
    for (w, h) in SIZES {
        let s = crate::shell::scale_for(w, h);
        for it in [Item::ClaudeCode, Item::Copilot] {
            let mut m = BrandMenu::new();
            m.set_about(crate::about::this_machine());
            m.toggle();
            m.anchor = (40.0 * s, 76.0 * s);
            m.open_page(Page::Install);
            m.presence = Item::ALL.iter().map(|i| (*i, Presence::Missing)).collect();
            m.handle_hit(&Event::Tab(IROW + row(it)));
            m.presence[row(it)].1 = Presence::Installed("version 1".into());
            m.signin = vec![(it, crate::agent_login::State::SignedIn("someone@example.test".into()))];
            let mut g = Gfx::new(w, h);
            m.render(&mut g, w, h, s, 0);
            let p = m.panel(w, h, s);
            let hits: Vec<_> = g.hits.iter().filter(|x| matches!(x.ev, Event::Tab(n) if crate::agent_login::agent_of_tab(n).is_some())).collect();
            assert_eq!(hits.len(), 2, "{w}x{h} {it:?}: Login and Logout");
            for x in hits {
                assert!(x.rect.x >= p.x && x.rect.y >= p.y && x.rect.x + x.rect.w <= p.x + p.w + 0.5 && x.rect.y + x.rect.h <= p.y + p.h + 0.5, "{w}x{h} {it:?}: {:?} outside {p:?}", x.rect);
            }
        }
    }
}
