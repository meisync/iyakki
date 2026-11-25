//! Settings pages of lane ST-D on the lane's fake port (`settings_device_port.rs`): a change saves, is audited, a person who is not an
//! administrator is refused, a refused or failed answer shows, and the page draws at several sizes. Nothing here reads the session, the
//! services or the hardware, and nothing sleeps. One module per page.

use super::*;
use super::super::settings_device_port::*;
use super::super::settings_port::{PagePort, SettingsDataPort};
use iyakki_shared::settings::Settings;

fn app_on(tab: Tab) -> SettingsApp {
    let cx = crate::apps::tests::cx_with_admin_store("dfake");
    SettingsApp::new(&cx).on(tab)
}

fn draw_at(a: &mut SettingsApp, w: u32, h: u32, f: impl FnOnce(&mut SettingsApp, &mut Gfx, R, f32)) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = crate::ui::scale_for(w, h);
    f(a, &mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s);
    g
}

const SIZES: [(u32, u32); 4] = [(640, 480), (1024, 600), (1280, 720), (1920, 1080)];

mod displays {
    use super::*;
    use iyakki_core_api::MonitorInfo;

    fn mon(id: &str, pct: u8, chosen: bool) -> MonitorInfo {
        MonitorInfo { id: Some(id.into()), label: format!("Screen {id}"), connector: "DP-1".into(), percent: Ok(pct), chosen }
    }

    #[test]
    fn a_person_who_is_not_an_administrator_is_refused_and_it_is_audited() {
        let p = FakeDevicePort::new(false, "disp-deny");
        *p.monitors.lock().unwrap() = vec![mon("a", 50, false)];
        let mut a = app_on(Tab::Displays);
        a.handle_displays(&Key::Char('e'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("administrators")));
        assert_eq!(p.audited(), vec![("settings.denied".to_string(), Tab::Displays.label().to_string())]);
        assert!(!a.disp.mons_looked, "nothing was asked of the monitors");
    }

    #[test]
    fn looking_choosing_and_dimming_a_monitor_are_audited_and_show_what_it_read_back() {
        let p = FakeDevicePort::new(true, "disp-ddc");
        let mut a = app_on(Tab::Displays);
        a.handle_displays(&Key::Char('b'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("Press e first")));
        a.handle_displays(&Key::Char('e'), &p);
        assert!(a.status().is_some_and(|s| s.0.contains("No external monitor")), "an empty list says why");
        *p.monitors.lock().unwrap() = vec![mon("a", 50, false), mon("b", 80, false)];
        a.handle_displays(&Key::Char('e'), &p);
        assert!(a.status().is_some_and(|s| s.1 && s.0.contains("2 monitor(s)")));
        a.handle_displays(&Key::Char('u'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("Choose a monitor")));
        a.handle_displays(&Key::Char('b'), &p);
        assert!(a.status().is_some_and(|s| s.1 && s.0.contains("Monitor a is chosen")));
        a.handle_displays(&Key::Char('u'), &p);
        assert!(a.status().is_some_and(|s| s.1 && s.0.contains("60%")));
        a.handle_displays(&Key::Char('j'), &p);
        a.handle_displays(&Key::Char('j'), &p);
        assert_eq!(p.monitors.lock().unwrap()[0].percent, Ok(40), "10 down twice from 60");
        assert_eq!(
            p.audited(),
            vec![
                ("settings.display.monitor_choose".to_string(), "a".to_string()),
                ("settings.display.monitor_brightness".to_string(), "60%".to_string()),
                ("settings.display.monitor_brightness".to_string(), "50%".to_string()),
                ("settings.display.monitor_brightness".to_string(), "40%".to_string()),
            ]
        );
        // b goes on to the next monitor, then to none
        a.handle_displays(&Key::Char('b'), &p);
        assert!(p.monitors.lock().unwrap()[1].chosen);
        a.handle_displays(&Key::Char('b'), &p);
        assert!(a.status().is_some_and(|s| s.0.contains("No monitor is chosen")));
    }

    #[test]
    fn a_monitor_that_refuses_shows_the_reason_and_audits_nothing() {
        let p = FakeDevicePort::new(true, "disp-refuse");
        *p.monitors.lock().unwrap() = vec![mon("a", 50, true)];
        let mut a = app_on(Tab::Displays);
        a.handle_displays(&Key::Char('e'), &p);
        *p.monitor_error.lock().unwrap() = Some("the screen did not answer".into());
        a.handle_displays(&Key::Char('u'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("did not answer")));
        a.handle_displays(&Key::Char('b'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("did not answer")));
        assert!(p.audited().is_empty());
    }

    #[test]
    fn the_screen_features_switch_goes_round_in_this_machines_data_folder() {
        let p = FakeDevicePort::new(true, "disp-switch");
        let mut a = app_on(Tab::Displays);
        let first = crate::memgate::read_switch(&p.data_dir());
        a.handle_displays(&Key::Char('s'), &p);
        let next = crate::memgate::read_switch(&p.data_dir());
        assert_ne!(first, next);
        assert!(a.status().is_some_and(|s| s.1 && s.0.contains(next.label())));
    }

    #[test]
    fn the_page_draws_the_memory_line_of_the_fake_at_several_sizes() {
        let p = FakeDevicePort::new(true, "disp-draw");
        *p.mem.lock().unwrap() = crate::memgate::MemFacts { total: 8 << 30, available: 6 << 30 };
        let mut a = app_on(Tab::Displays);
        for (w, h) in SIZES {
            let g = draw_at(&mut a, w, h, |a, g, r, s| a.render_displays(g, r, s, &p));
            let (line, _) = a.disp_gate_on(&p);
            assert!(!line.is_empty());
            assert!(g.layout_log.texts.iter().any(|t| t.3.contains("One picture on every screen") || t.3.contains("drives each display")), "{w}x{h}: the page says how the displays are driven");
        }
    }
}

mod system_sounds {
    use super::*;
    use iyakki_core_api::soundtypes::Cue;

    #[test]
    fn a_person_who_is_not_an_administrator_changes_nothing() {
        let p = FakeDevicePort::new(false, "sysnd-deny");
        let mut a = app_on(Tab::SystemSounds);
        let before = p.sysnd.lock().unwrap().clone();
        a.handle_sysnd(&Key::Char('e'), &p);
        a.handle_sysnd(&Key::Char('+'), &p);
        assert_eq!(*p.sysnd.lock().unwrap(), before);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("administrators")));
        assert!(p.audited().iter().all(|l| l.0 == "settings.denied"));
    }

    #[test]
    fn the_master_switch_a_cue_and_the_level_save_and_are_audited() {
        let p = FakeDevicePort::new(true, "sysnd-save");
        let mut a = app_on(Tab::SystemSounds);
        let was = p.sysnd.lock().unwrap().clone();
        a.handle_sysnd(&Key::Char('e'), &p); // row 0: the master switch
        assert_eq!(p.sysnd.lock().unwrap().enabled, !was.enabled);
        a.handle_sysnd(&Key::Down, &p);
        a.handle_sysnd(&Key::Down, &p); // the first cue
        a.handle_sysnd(&Key::Char('e'), &p);
        assert_eq!(p.sysnd.lock().unwrap().get(Cue::ALL[0]), !was.get(Cue::ALL[0]));
        a.handle_sysnd(&Key::Char('-'), &p);
        assert_eq!(p.sysnd.lock().unwrap().offset_db, was.offset_db - 3);
        let audit = p.audited();
        assert_eq!(audit.len(), 3);
        assert!(audit[0].0 == "settings.sysnd" && audit[0].1.starts_with("all "));
        assert!(audit[1].0 == "settings.sysnd" && audit[1].1.starts_with(Cue::ALL[0].id()));
        assert_eq!(audit[2], ("settings.sysnd.level".to_string(), p.sysnd.lock().unwrap().offset_db.to_string()));
    }

    #[test]
    fn the_level_never_leaves_its_range_and_a_refused_save_shows_why() {
        let p = FakeDevicePort::new(true, "sysnd-range");
        let mut a = app_on(Tab::SystemSounds);
        for _ in 0..30 {
            a.handle_sysnd(&Key::Char('+'), &p);
        }
        assert_eq!(p.sysnd.lock().unwrap().offset_db, iyakki_shared::syssounds::MAX_OFFSET_DB);
        *p.sysnd_refuses.lock().unwrap() = Some("The settings file cannot be written.".into());
        let n = p.audited().len();
        a.handle_sysnd(&Key::Char('-'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("cannot be written")));
        assert_eq!(p.audited().len(), n, "a refused change is not audited as a change");
    }

    #[test]
    fn a_preview_is_audited_and_plays_off_the_page_thread() {
        let p = FakeDevicePort::new(true, "sysnd-preview");
        let mut a = app_on(Tab::SystemSounds);
        a.handle_sysnd(&Key::Down, &p);
        a.handle_sysnd(&Key::Down, &p);
        a.handle_sysnd(&Key::Char('p'), &p);
        assert_eq!(p.audited(), vec![("settings.sysnd.preview".to_string(), Cue::ALL[0].id().to_string())]);
        let job = a.job.take().expect("a job was started");
        match job.rx.recv().unwrap() {
            JobOut::Msg(Ok(m)) => assert!(m.contains(Cue::ALL[0].id()), "{m}"),
            _ => panic!("expected a message"),
        }
        assert_eq!(*p.previewed.lock().unwrap(), vec![Cue::ALL[0]]);
    }

    #[test]
    fn the_page_draws_and_says_when_sound_is_muted_at_several_sizes() {
        let p = FakeDevicePort::new(true, "sysnd-draw");
        let mut a = app_on(Tab::SystemSounds);
        for muted in [false, true] {
            p.muted.store(muted, std::sync::atomic::Ordering::SeqCst);
            for (w, h) in SIZES {
                let g = draw_at(&mut a, w, h, |a, g, r, s| a.render_sysnd(g, r, s, &p));
                assert!(!g.layout_log.texts.is_empty(), "{w}x{h}");
                assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('e')) || matches!(h.ev, Event::Row(_))) || w < 700, "{w}x{h}");
            }
        }
    }
}

mod views {
    use super::*;
    use iyakki_shared::views::Views;

    fn views_of(p: &FakeDevicePort) -> Views {
        Views::load(&p.data_dir(), "amy")
    }

    #[test]
    fn a_new_person_has_the_first_view_and_the_page_lists_it_with_the_settings_rows() {
        let p = FakeDevicePort::new(false, "vw-rows");
        let rows = SettingsApp::vw_rows(&p);
        assert!(matches!(rows[0], super::super::views_page::VwRow::View { number: 1, .. }));
        assert_eq!(SettingsApp::vw_len(&p), rows.len());
        assert!(rows.len() >= 6, "a view and the five setting rows");
    }

    #[test]
    fn making_renaming_locking_and_deleting_a_view_are_kept_in_this_persons_file_and_audited() {
        let p = FakeDevicePort::new(false, "vw-edit");
        let mut a = app_on(Tab::Views);
        let form = Form::new("New view").text("Name", "Docs", "");
        assert_eq!(a.vw_submit(&p, &None, &form), Ok("Made the view Docs.".to_string()));
        let v = views_of(&p);
        assert!(v.views.iter().any(|w| w.name == "Docs"));
        let id = v.views.iter().find(|w| w.name == "Docs").unwrap().id;
        assert_eq!(a.vw_submit(&p, &Some(id), &Form::new("Rename").text("Name", "Notes", "")), Ok("Renamed to Notes.".to_string()));
        // lock it: select its row and press l
        let at = SettingsApp::vw_rows(&p).iter().position(|r| matches!(r, super::super::views_page::VwRow::View { name, .. } if name == "Notes")).unwrap();
        a.sel[a.tab] = at;
        a.handle_views(&Key::Char('l'), &p);
        assert!(views_of(&p).views.iter().find(|w| w.id == id).unwrap().locked);
        a.vw_confirm(&p, id);
        assert!(!views_of(&p).views.iter().any(|w| w.id == id));
        let audit: Vec<String> = p.audited().into_iter().map(|l| l.0).collect();
        assert_eq!(audit, ["view.edit", "view.edit", "view.lock", "view.delete"]);
    }

    #[test]
    fn a_name_that_is_refused_shows_the_reason_and_changes_nothing() {
        let p = FakeDevicePort::new(false, "vw-refuse");
        let mut a = app_on(Tab::Views);
        let before = views_of(&p).views.len();
        assert!(a.vw_submit(&p, &None, &Form::new("New view").text("Name", "   ", "")).is_err());
        assert_eq!(views_of(&p).views.len(), before);
        assert!(p.audited().is_empty());
        // the only view cannot be deleted
        let id = views_of(&p).views[0].id;
        a.vw_confirm(&p, id);
        assert!(a.status().is_some_and(|s| !s.1));
        assert_eq!(views_of(&p).views.len(), before);
    }

    #[test]
    fn the_step_keys_the_lock_return_and_the_layout_switches_save_and_the_own_keys_form_validates() {
        let p = FakeDevicePort::new(false, "vw-switches");
        let mut a = app_on(Tab::Views);
        let k0 = views_of(&p).key_set;
        a.handle_views(&Key::Char('k'), &p);
        assert_ne!(views_of(&p).key_set, k0);
        let r0 = views_of(&p).reset_on_lock;
        a.handle_views(&Key::Char('o'), &p);
        assert_eq!(views_of(&p).reset_on_lock, !r0);
        let l0 = views_of(&p).remember_layout;
        a.handle_views(&Key::Char('y'), &p);
        assert_eq!(views_of(&p).remember_layout, !l0);
        a.handle_views(&Key::Char('i'), &p);
        assert!(views_of(&p).is_single("*"));
        a.handle_views(&Key::Char('i'), &p);
        assert!(!views_of(&p).is_single("*"));
        let ok = Form::new("keys").text("Previous", "J", "").text("Next", "K", "");
        assert!(a.vw_keys_submit(&p, &ok).unwrap().contains("Own step keys"));
        assert!(views_of(&p).custom_keys.is_some());
        let bad = Form::new("keys").text("Previous", "M", "").text("Next", "K", "");
        assert!(a.vw_keys_submit(&p, &bad).is_err(), "a key the shell uses is refused");
        let audit: Vec<String> = p.audited().into_iter().map(|l| l.0).collect();
        assert_eq!(audit, ["view.layout", "view.single", "view.single", "view.keys"]);
    }

    #[test]
    fn a_template_makes_a_named_view_and_the_page_draws_at_several_sizes() {
        let p = FakeDevicePort::new(false, "vw-draw");
        let mut a = app_on(Tab::Views);
        let item = format!("{} ({})", iyakki_shared::views::TEMPLATES[0].name, iyakki_shared::views::TEMPLATES[0].tiles.iter().map(|t| t.to_uppercase()).collect::<Vec<_>>().join(", "));
        a.vw_pick(&p, &item);
        assert!(views_of(&p).views.iter().any(|w| w.name.starts_with(iyakki_shared::views::TEMPLATES[0].name)));
        for (w, h) in SIZES {
            let g = draw_at(&mut a, w, h, |a, g, r, s| a.render_views(g, r, s, &p));
            assert!(g.layout_log.texts.iter().any(|t| t.3.contains("Views (up to")), "{w}x{h}");
        }
    }
}

mod home_layout {
    use super::*;
    use super::super::homelayout_page::HlRow;
    use iyakki_shared::layout::Layout;
    use iyakki_shared::store_tilehub::{Caps, TileEntry};

    fn store_tile(name: &str, group: &str) -> TileEntry {
        TileEntry {
            key: format!("tile/shared/{name}"),
            name: name.into(),
            title: name.to_uppercase(),
            icon: String::new(),
            icon_picture: None,
            group: group.into(),
            version: "1".into(),
            publisher: "test".into(),
            trust: "test".into(),
            dev: false,
            dir: None,
            scope: iyakki_shared::skills::Scope::Shared,
            caps: Caps::default(),
            wasm: false,
        }
    }

    fn layout_of(p: &FakeDevicePort) -> Layout {
        Layout::load(&p.data_dir(), "amy")
    }

    #[test]
    fn the_rows_are_the_groups_the_tiles_of_the_shell_and_the_store_tiles_this_person_sees() {
        let p = FakeDevicePort::new(false, "hl-rows");
        *p.tiles.lock().unwrap() = vec![store_tile("calc", "tools")];
        let rows = SettingsApp::hl_rows(&p);
        let titles: Vec<String> = rows.iter().filter_map(|r| if let HlRow::Tile { title, .. } = r { Some(title.clone()) } else { None }).collect();
        assert!(titles.contains(&"CHAT".to_string()) && titles.contains(&"CALC".to_string()), "{titles:?}");
        assert!(!titles.contains(&"USERS".to_string()), "USERS is for people who manage users");
        p.manage_users.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(SettingsApp::hl_rows(&p).iter().any(|r| matches!(r, HlRow::Tile { title, .. } if title == "USERS")));
        assert_eq!(SettingsApp::hl_len(&p), SettingsApp::hl_rows(&p).len());
    }

    #[test]
    fn a_group_is_made_a_tile_is_moved_and_put_back_in_this_persons_own_layout_file() {
        let p = FakeDevicePort::new(false, "hl-move");
        *p.tiles.lock().unwrap() = vec![store_tile("calc", "tools")];
        let mut a = app_on(Tab::HomeLayout);
        assert_eq!(a.hl_submit(&p, &None, &Form::new("New group").text("Name", "Games", "")), Ok("Made the group Games.".to_string()));
        assert_eq!(p.audited(), vec![("layout.group".to_string(), "Games".to_string())]);
        let key = iyakki_shared::layout::app_key("calc");
        a.hl_pick(&p, &key, "Games");
        assert!(a.status().is_some_and(|s| s.1 && s.0.contains("Moved to Games")));
        assert!(layout_of(&p).placed.contains_key(&key));
        let at = SettingsApp::hl_rows(&p).iter().position(|r| matches!(r, HlRow::Tile { key: k, .. } if *k == key)).unwrap();
        a.sel[a.tab] = at;
        a.handle_homelayout(&Key::Char('p'), &p);
        assert!(!layout_of(&p).placed.contains_key(&key));
        assert!(a.status().is_some_and(|s| s.0.contains("back in its own group")));
    }

    #[test]
    fn a_group_name_that_is_refused_and_the_built_in_groups_are_not_changed() {
        let p = FakeDevicePort::new(false, "hl-refuse");
        let mut a = app_on(Tab::HomeLayout);
        assert!(a.hl_submit(&p, &None, &Form::new("New group").text("Name", "", "")).is_err());
        assert!(p.audited().is_empty());
        a.sel[a.tab] = 0; // the first row is a built-in group
        a.handle_homelayout(&Key::Char('x'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("cannot be deleted")));
        a.handle_homelayout(&Key::Char('r'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("keep their names")));
    }

    #[test]
    fn only_an_administrator_sets_or_clears_the_default_layout_and_both_are_audited() {
        let deny = FakeDevicePort::new(false, "hl-deny");
        let mut a = app_on(Tab::HomeLayout);
        a.handle_homelayout(&Key::Char('d'), &deny);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("administrator")));
        a.hl_confirm(&deny, &ConfirmKind::HomeDefaultSet);
        assert!(!Layout::has_default(&deny.data_dir()), "a hand-made confirm does nothing for a person who is not an administrator");

        let p = FakeDevicePort::new(true, "hl-admin");
        a.handle_homelayout(&Key::Char('c'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("No default layout")));
        a.hl_confirm(&p, &ConfirmKind::HomeDefaultSet);
        assert!(Layout::has_default(&p.data_dir()));
        a.hl_confirm(&p, &ConfirmKind::HomeDefaultClear);
        assert!(!Layout::has_default(&p.data_dir()));
        assert_eq!(p.audited(), vec![("layout.default.set".to_string(), String::new()), ("layout.default.clear".to_string(), String::new())]);
    }

    #[test]
    fn the_page_draws_at_several_sizes() {
        let p = FakeDevicePort::new(true, "hl-draw");
        *p.tiles.lock().unwrap() = vec![store_tile("calc", "tools")];
        let mut a = app_on(Tab::HomeLayout);
        for (w, h) in SIZES {
            let g = draw_at(&mut a, w, h, |a, g, r, s| a.render_homelayout(g, r, s, &p));
            assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Row(_))), "{w}x{h}: rows to click");
        }
    }
}

mod screen_saver {
    use super::*;
    use iyakki_shared::lockscreen::{self, LockConfig};
    use iyakki_shared::screensaver::{self as ss, First, SaverConfig};
    use std::sync::atomic::Ordering;

    fn rows(admin: bool) -> &'static [super::super::screensaver_page::SsRow] {
        SettingsApp::ss_rows(admin)
    }

    #[test]
    fn a_person_has_nine_cards_and_an_administrator_also_the_three_of_the_sign_in_screen() {
        let person = FakeDevicePort::new(false, "ss-rows");
        assert_eq!(SettingsApp::ss_len(&person), 9);
        let admin = FakeDevicePort::new(true, "ss-rows-a");
        assert_eq!(SettingsApp::ss_len(&admin), 12);
        assert_eq!(rows(true).len(), 12);
    }

    #[test]
    fn a_choice_is_kept_in_this_persons_own_file_and_audited() {
        let p = FakeDevicePort::new(false, "ss-person");
        let mut a = app_on(Tab::ScreenSaver);
        let want = First::ALL[1];
        a.ss_sub(0, 'b', &p); // the second choice of the first card
        assert_eq!(SaverConfig::load(&p.data_dir(), "amy").first, want);
        assert_eq!(p.audited(), vec![("settings.screensaver".to_string(), format!("when idle: {}", want.label()))]);
        assert!(a.status().is_some_and(|s| s.1 && s.0 == "Saved."));
        // Enter takes the next choice of the selected card
        a.sel[a.tab] = 0;
        a.handle_screensaver(&Key::Enter, &p);
        assert_ne!(SaverConfig::load(&p.data_dir(), "amy").first, want);
    }

    #[test]
    fn the_sign_in_screen_choices_are_the_machines_and_only_for_administrators() {
        let person = FakeDevicePort::new(false, "ss-signin-deny");
        let mut a = app_on(Tab::ScreenSaver);
        // a person cannot even reach the card; a forced click is answered, nothing is saved or audited
        a.ss_choose(super::super::screensaver_page::SsRow::SiAfter, 1, &person);
        assert!(a.status().is_some_and(|s| s.0.contains("Only administrators change the sign-in screen")));
        assert_eq!(person.settings_get().screensaver_signin.0, SaverConfig::signin_default());
        assert!(person.audited().is_empty());

        let p = FakeDevicePort::new(true, "ss-signin");
        a.ss_sub(10, 'b', &p); // SiAfter
        assert_ne!(p.settings_get().screensaver_signin.0.after_min, SaverConfig::signin_default().after_min);
        assert_eq!(p.audited().len(), 1);
        assert_eq!(p.audited()[0].0, "settings.screensaver.signin");
        // busy settings are told and nothing is audited
        p.busy.store(true, Ordering::SeqCst);
        a.ss_sub(10, 'a', &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("busy")));
        assert_eq!(p.audited().len(), 1);
    }

    #[test]
    fn a_guest_uses_the_standard_choices_and_cannot_lock() {
        let p = FakeDevicePort::new(false, "ss-guest");
        p.guest.store(true, Ordering::SeqCst);
        let mut a = app_on(Tab::ScreenSaver);
        a.ss_sub(0, 'b', &p);
        assert!(a.status().is_some_and(|s| s.0.contains("Guests use the standard choices")));
        assert_eq!(SaverConfig::load(&p.data_dir(), "amy"), SaverConfig::load(&p.data_dir(), "nobody"));
        a.ss_sub(8, 'a', &p); // lock now
        assert!(a.status().is_some_and(|s| s.0.contains("Guests have no password")));
        assert!(p.audited().is_empty());
    }

    #[test]
    fn locking_now_the_automatic_lock_and_the_preview_leave_their_requests_for_the_shell() {
        let p = FakeDevicePort::new(false, "ss-lock");
        let mut a = app_on(Tab::ScreenSaver);
        a.ss_sub(7, 'b', &p); // the automatic lock, second choice
        assert_eq!(LockConfig::load(&p.data_dir(), "amy").auto_min, lockscreen::AUTO_CHOICES[1]);
        assert!(p.audited().iter().any(|l| l.0 == "settings.lockscreen"));
        a.ss_sub(8, 'a', &p);
        assert!(a.status().is_some_and(|s| s.0.contains("Locking the screen")));
        assert!(lockscreen::take_request(&p.data_dir(), "amy"), "the shell finds the lock request");
        a.ss_sub(6, 'a', &p);
        assert!(a.status().is_some_and(|s| s.0.contains("Showing the screen saver")));
        assert!(ss::take_request(&p.data_dir(), "amy").is_some());
    }

    #[test]
    fn the_page_draws_its_cards_at_several_sizes() {
        for admin in [false, true] {
            let p = FakeDevicePort::new(admin, "ss-draw");
            let mut a = app_on(Tab::ScreenSaver);
            for (w, h) in SIZES {
                let g = draw_at(&mut a, w, h, |a, g, r, s| a.render_screensaver(g, r, s, &p));
                assert!(!g.layout_log.texts.is_empty() && !g.hits.is_empty(), "{w}x{h} admin={admin}");
            }
        }
    }
}

mod appearance {
    use super::*;
    use iyakki_uikit::look::BarItem;
    use iyakki_uikit::look_registry as reg;

    /// The type, font and wallpaper choices are process-wide in the toolkit: a test that changes them holds this lock.
    fn globals() -> iyakki_testkit::lock::Lock {
        iyakki_testkit::lock::lock("appearance-globals")
    }

    #[test]
    fn a_part_of_the_look_changes_for_this_person_only_and_is_audited() {
        let _g = globals();
        let p = FakeDevicePort::new(false, "ap-look");
        let mut a = app_on(Tab::Appearance);
        let before = SettingsApp::ap_look(&p).dock.running;
        a.handle_appearance(&Key::Char('r'), &p);
        assert_eq!(reg::current(&p.data_dir(), "amy").look.dock.running, !before);
        assert_eq!(reg::current(&p.data_dir(), "nobody").look.dock.running, before, "another person is untouched");
        assert_eq!(p.audited(), vec![("settings.display.look".to_string(), "dock running dots".to_string())]);
        let pos0 = SettingsApp::ap_look(&p).dock.position;
        a.handle_appearance(&Key::Char('d'), &p);
        assert_ne!(SettingsApp::ap_look(&p).dock.position, pos0);
    }

    #[test]
    fn a_top_bar_item_goes_and_comes_back_in_its_place() {
        let _g = globals();
        let p = FakeDevicePort::new(false, "ap-bar");
        let mut a = app_on(Tab::Appearance);
        a.handle_appearance(&Key::Char('k'), &p);
        assert!(!SettingsApp::ap_look(&p).topbar.items.contains(&BarItem::Clock));
        assert!(a.status().is_some_and(|s| s.0.contains("Clock in the top bar: hidden")));
        a.handle_appearance(&Key::Char('k'), &p);
        assert_eq!(SettingsApp::ap_look(&p).topbar.items.iter().position(|i| *i == BarItem::Clock), Some(0));
    }

    #[test]
    fn pinning_and_unpinning_apps_keeps_the_dock_list_and_stops_at_ten() {
        let _g = globals();
        let p = FakeDevicePort::new(false, "ap-dock");
        let mut a = app_on(Tab::Appearance);
        let start = SettingsApp::ap_pinned(&p);
        a.handle_appearance(&Key::Char('a'), &p);
        let after = SettingsApp::ap_pinned(&p);
        assert_eq!(after.len(), start.len() + 1);
        assert!(!start.contains(after.last().unwrap()), "the first app that was not in the dock");
        assert_eq!(p.audited().last().unwrap().0, "settings.display.dock");
        a.handle_appearance(&Key::Char('x'), &p);
        assert_eq!(SettingsApp::ap_pinned(&p), start);
        // every pinnable app in: the page says so
        *p.pinnable_apps.lock().unwrap() = start.iter().map(|id| (id.clone(), id.to_uppercase())).collect();
        a.handle_appearance(&Key::Char('a'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("dock is full")));
        a.handle_appearance(&Key::Char('o'), &p);
        assert!(reg::load_dock(&p.data_dir(), "amy").is_none(), "the dock is the usual one again");
        assert_eq!(p.audited().last().unwrap(), &("settings.display.dock".to_string(), "default".to_string()));
    }

    #[test]
    fn the_sdk_version_of_edit_and_cli_is_the_persons_choice_and_audited() {
        let p = FakeDevicePort::new(false, "ap-sdk");
        let mut a = app_on(Tab::Appearance);
        assert!(!p.sdk_app_chosen("edit"));
        a.handle_appearance(&Key::Char('t'), &p);
        assert!(p.sdk_app_chosen("edit") && !p.sdk_app_chosen("cli"));
        assert!(a.status().is_some_and(|s| s.1 && s.0.contains("next time the app is opened")));
        a.handle_appearance(&Key::Char('T'), &p);
        assert!(p.sdk_app_chosen("cli"));
        a.handle_appearance(&Key::Char('t'), &p);
        assert!(!p.sdk_app_chosen("edit"));
        let audit: Vec<String> = p.audited().into_iter().map(|l| l.1).collect();
        assert_eq!(audit, ["edit on", "cli on", "edit off"]);
    }

    #[test]
    fn type_and_font_choices_are_kept_for_this_person_and_audited() {
        let _g = globals();
        let p = FakeDevicePort::new(false, "ap-type");
        let mut a = app_on(Tab::Appearance);
        let (ui0, code0) = (iyakki_uikit::fonts::current_ui(), iyakki_uikit::fonts::current_code());
        a.handle_appearance(&Key::Char('f'), &p);
        a.handle_appearance(&Key::Char('P'), &p);
        a.handle_appearance(&Key::Char('u'), &p);
        let audit: Vec<String> = p.audited().into_iter().map(|l| l.0).collect();
        assert_eq!(audit, ["settings.display.font", "settings.display.font", "settings.display.type"]);
        // put the process-wide choices back for the tests that follow
        iyakki_uikit::fonts::set_ui(ui0);
        iyakki_uikit::fonts::set_code(code0);
    }

    #[test]
    fn the_page_draws_at_several_sizes() {
        let _g = globals();
        let p = FakeDevicePort::new(false, "ap-draw");
        let mut a = app_on(Tab::Appearance);
        for (w, h) in SIZES {
            let g = draw_at(&mut a, w, h, |a, g, r, s| a.render_appearance(g, r, s, &p));
            assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('r'))) || g.hits.iter().any(|h| matches!(h.ev, Event::Key(_))), "{w}x{h}: rows to click");
        }
    }
}

mod sound {
    use super::*;
    use iyakki_shared::voicedesk::{Mic, T2Mic};

    fn sound_app() -> SettingsApp {
        let cx = crate::apps::tests::cx_with_admin_store("dfake-snd");
        let (env, _) = super::super::tests::fake_machine("dfake-snd");
        SettingsApp::new(&cx).on(Tab::Sound).with_hardware(env)
    }

    fn mic(id: &str, label: &str) -> Mic {
        Mic { id: id.into(), label: label.into(), usb: true }
    }

    fn audit_names(p: &FakeDevicePort) -> Vec<String> {
        p.audited().into_iter().map(|l| l.0).collect()
    }

    #[test]
    fn a_person_who_is_not_an_administrator_changes_nothing() {
        let p = FakeDevicePort::new(false, "snd-deny");
        let mut a = sound_app();
        let before = p.sound.lock().unwrap().clone();
        for k in ['M', 'W', 'B', 'o', '+', '-', 'm'] {
            assert!(a.sound_key(&Key::Char(k), &p), "{k}");
        }
        assert_eq!(*p.sound.lock().unwrap(), before);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("administrators")));
        assert!(audit_names(&p).iter().all(|n| n == "settings.denied"));
        assert!(a.modal.is_none(), "no list of outputs for a person who may not choose");
    }

    #[test]
    fn the_volume_steps_mutes_unmutes_and_is_audited() {
        let p = FakeDevicePort::new(true, "snd-vol");
        let mut a = sound_app();
        let v0 = p.sound.lock().unwrap().volume_pct as i32;
        a.sound_key(&Key::Char('-'), &p);
        assert_eq!(p.sound.lock().unwrap().volume_pct as i32, (v0 - 5).max(0));
        a.sound_key(&Key::Char('m'), &p);
        assert!(p.sound.lock().unwrap().muted);
        assert!(a.status().is_some_and(|s| s.0.contains("muted")));
        a.sound_key(&Key::Char('+'), &p);
        assert!(!p.sound.lock().unwrap().muted, "louder also unmutes");
        a.sound_set_volume(&p, 250);
        assert_eq!(p.sound.lock().unwrap().volume_pct, 100, "never above 100");
        assert_eq!(audit_names(&p), ["settings.sound.volume"; 4]);
    }

    #[test]
    fn the_mixer_the_browser_route_and_the_highlight_offset_save_and_are_audited() {
        let p = FakeDevicePort::new(true, "snd-switches");
        let mut a = sound_app();
        let before = p.sound.lock().unwrap().clone();
        let (mixer0, web0) = (before.mixer, before.web_mix);
        a.sound_key(&Key::Char('M'), &p);
        assert_eq!(p.sound.lock().unwrap().mixer, !mixer0);
        a.sound_key(&Key::Char('W'), &p);
        assert_eq!(p.sound.lock().unwrap().web_mix, !web0);
        a.sound_set_highlight_offset(&p, 9000);
        assert_eq!(p.sound.lock().unwrap().highlight_offset_ms, 500, "kept in its range");
        a.sound_set_boost(&p, "card0", 6);
        assert_eq!(p.sound.lock().unwrap().boost_for("card0"), 6);
        assert_eq!(audit_names(&p), ["settings.sound.mixer", "settings.sound.web_mix", "settings.sound.highlight_offset", "settings.sound.boost"]);
    }

    #[test]
    fn a_refused_save_shows_the_reason_and_audits_nothing() {
        let p = FakeDevicePort::new(true, "snd-refuse");
        let mut a = sound_app();
        let mixer0 = p.sound.lock().unwrap().mixer;
        *p.sound_refuses.lock().unwrap() = Some("The settings file cannot be written.".into());
        a.sound_key(&Key::Char('M'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("cannot be written")));
        a.sound_pick(&p, "Automatic (follows what is connected)");
        assert!(a.status().is_some_and(|s| !s.1));
        assert!(p.audited().is_empty());
        assert_eq!(p.sound.lock().unwrap().mixer, mixer0);
    }

    #[test]
    fn choosing_the_output_opens_the_list_and_a_pick_is_kept() {
        let p = FakeDevicePort::new(true, "snd-out");
        let mut a = sound_app();
        a.sound_key(&Key::Char('o'), &p);
        assert!(matches!(a.modal, Some(Modal::Pick(PickKind::SoundOutput, _))));
        a.sound_pick(&p, "HDMI · ready  [PCH:3]");
        assert_eq!(p.sound.lock().unwrap().output, "PCH:3");
        a.sound_pick(&p, "Automatic (follows what is connected)");
        assert_eq!(p.sound.lock().unwrap().output, "");
        assert_eq!(p.audited(), vec![("settings.sound.output".to_string(), "PCH:3".to_string()), ("settings.sound.output".to_string(), "automatic".to_string())]);
    }

    #[test]
    fn the_test_the_patterns_and_the_hardware_check_run_as_jobs_and_are_audited() {
        let p = FakeDevicePort::new(true, "snd-jobs");
        let mut a = sound_app();
        let finish = |a: &mut SettingsApp| a.job.take().expect("a job was started").rx.recv().unwrap();
        a.sound_key(&Key::Char('t'), &p);
        assert!(matches!(finish(&mut a), JobOut::Msg(Ok(m)) if m.contains("test sound")));
        a.sound_pattern(&p, true);
        assert!(matches!(finish(&mut a), JobOut::Msg(Ok(m)) if m.contains("pattern")));
        a.sound_hw_check(&p, 1, 0, "Master".into(), "card1".into());
        match finish(&mut a) {
            JobOut::HwCheck(id, Ok(_)) => assert_eq!(id, "card1"),
            _ => panic!("expected the hardware check"),
        }
        assert_eq!(p.sound_jobs.lock().unwrap().len(), 3);
        assert_eq!(audit_names(&p), ["settings.sound.test", "settings.sound.pattern", "settings.sound.hwcheck"]);
        // the answer to "did the second tone sound quieter?" is kept per card
        a.sound_hw_answer(&p, &ConfirmKind::SoundHwWorks("card1".into()), false);
        assert_eq!(p.sound.lock().unwrap().volume_mode_for("card1"), iyakki_shared::audioout::VolumeMode::Software);
    }

    #[test]
    fn the_microphone_is_chosen_cycled_and_tested_and_a_silent_one_is_told() {
        let p = FakeDevicePort::new(true, "snd-mic");
        let mut a = sound_app();
        a.sound_key(&Key::Char('i'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("No microphone found")));
        *p.mics.lock().unwrap() = vec![mic("plughw:1,0", "USB mic"), mic("plughw:2,0", "Camera mic")];
        a.sound_key(&Key::Char('i'), &p);
        assert_eq!(p.voice_cfg.lock().unwrap().mic, "plughw:1,0");
        a.sound_key(&Key::Char('i'), &p);
        assert_eq!(p.voice_cfg.lock().unwrap().mic, "plughw:2,0");
        a.sound_key(&Key::Char('i'), &p);
        assert_eq!(p.voice_cfg.lock().unwrap().mic, "", "after the last one, automatic again");
        a.sound_mic_set(&p, "plughw:1,0");
        assert!(a.status().is_some_and(|s| s.1 && s.0.contains("USB mic")));
        a.sound_key(&Key::Char('r'), &p);
        assert!(matches!(a.job.take().unwrap().rx.recv().unwrap(), JobOut::Msg(Ok(m)) if m.contains("works")));
        p.mic_peak.store(10, std::sync::atomic::Ordering::SeqCst);
        a.sound_key(&Key::Char('r'), &p);
        assert!(matches!(a.job.take().unwrap().rx.recv().unwrap(), JobOut::Msg(Err(m)) if m.contains("silent")));
        assert!(p.sound_jobs.lock().unwrap()[0].contains("plughw:1,0"), "the command is aimed at the chosen input");
    }

    #[test]
    fn the_t2_microphone_asks_before_it_is_turned_on() {
        let p = FakeDevicePort::new(true, "snd-t2");
        let mut a = sound_app();
        a.sound_t2_mic_apply(&p, true);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("no T2")));
        *p.t2.lock().unwrap() = T2Mic::Off;
        a.sound_t2_mic_apply(&p, true);
        assert_eq!(*p.t2.lock().unwrap(), T2Mic::On);
        a.sound_t2_mic_apply(&p, false);
        assert_eq!(*p.t2.lock().unwrap(), T2Mic::Off);
        assert_eq!(audit_names(&p), ["settings.sound.t2mic", "settings.sound.t2mic"]);
    }

    #[test]
    fn the_bluetooth_route_asks_which_pair_and_stops_again() {
        let p = FakeDevicePort::new(true, "snd-bt");
        let mut a = sound_app();
        a.sound_bluetooth(&p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("No headphones")));
        p.bt_view.lock().unwrap().sinks = vec![("AA:BB".into(), "Buds".into())];
        a.sound_bluetooth(&p);
        assert!(matches!(a.modal, Some(Modal::Confirm(ConfirmKind::BluetoothSound(_), _))));
        a.sound_bluetooth_start(&p, "AA:BB");
        assert!(p.bt_view.lock().unwrap().active);
        a.sound_bluetooth(&p); // sending: the same row stops it
        assert!(!p.bt_view.lock().unwrap().active);
        assert_eq!(*p.bt_calls.lock().unwrap(), ["start AA:BB", "stop"]);
        assert_eq!(p.audited(), vec![("settings.sound.bluetooth".to_string(), "start".to_string()), ("settings.sound.bluetooth".to_string(), "stop".to_string())]);
    }
}

mod keyboard {
    use super::*;
    use std::sync::atomic::Ordering;

    fn kb_app(tag: &str) -> SettingsApp {
        let sys = crate::apps::tests::temp_dir(&format!("dfake-kb-{tag}-sys"));
        let w = |rel: &str, c: &str| {
            let f = sys.join(rel);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, c).unwrap();
        };
        w("module/hid_apple/parameters/fnmode", "3\n");
        w("module/hid_apple/parameters/iso_layout", "N\n");
        w("module/hid_apple/parameters/swap_opt_cmd", "N\n");
        app_on(Tab::Keyboard).with_keyboard_sys(sys)
    }

    #[test]
    fn a_person_who_is_not_an_administrator_changes_nothing_and_it_is_audited() {
        let p = FakeDevicePort::new(false, "kb-deny");
        let mut a = kb_app("deny");
        for c in ['f', 'u', 'l', 'b', 'h', 'z', 't'] {
            a.handle_keyboard(&Key::Char(c), &p);
        }
        assert_eq!(p.settings_get().keyboard, Settings::default().keyboard);
        assert!(p.audited().iter().all(|l| l.0 == "settings.denied"), "{:?}", p.audited());
        assert!(a.job.is_none(), "no job was started");
    }

    #[test]
    fn the_function_key_mode_goes_round_saves_and_is_audited() {
        let p = FakeDevicePort::new(true, "kb-fn");
        let mut a = kb_app("fn");
        let mut seen = Vec::new();
        for _ in 0..4 {
            a.handle_keyboard(&Key::Char('f'), &p);
            seen.push(p.settings_get().keyboard.fn_mode);
        }
        assert_eq!(seen, ["media", "fkeys", "auto", ""]);
        let audit: Vec<String> = p.audited().into_iter().map(|l| l.1).collect();
        assert_eq!(audit, ["fn_mode=media", "fn_mode=fkeys", "fn_mode=auto", "fn_mode="]);
        assert!(p.audited().iter().all(|l| l.0 == "settings.keyboard"));
    }

    #[test]
    fn busy_settings_are_told_and_nothing_is_audited() {
        let p = FakeDevicePort::new(true, "kb-busy");
        p.busy.store(true, Ordering::SeqCst);
        let mut a = kb_app("busy");
        a.handle_keyboard(&Key::Char('u'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("busy")));
        assert!(p.audited().is_empty());
        assert_eq!(p.settings_get().keyboard.touchbar, "");
    }

    #[test]
    fn the_touch_probe_the_reset_and_the_test_sound_run_as_jobs_and_are_audited() {
        let p = FakeDevicePort::new(true, "kb-jobs");
        p.touch_level.store(2, Ordering::SeqCst);
        let mut a = kb_app("jobs");
        let finish = |a: &mut SettingsApp| a.job.take().expect("a job was started").rx.recv().unwrap();
        a.handle_keyboard(&Key::Char('h'), &p);
        assert!(matches!(finish(&mut a), JobOut::Msg(Ok(m)) if m == "Probed at step 2."));
        a.handle_keyboard(&Key::Char('z'), &p);
        assert!(matches!(finish(&mut a), JobOut::Msg(Ok(m)) if m.contains("Reset")));
        a.handle_keyboard(&Key::Char('t'), &p);
        assert!(matches!(finish(&mut a), JobOut::Msg(Ok(m)) if m.contains("test sound")));
        let audit: Vec<(String, String)> = p.audited();
        assert_eq!(audit[0], ("settings.touch.probe".to_string(), "step 2".to_string()));
        assert_eq!(audit[1].0, "settings.touch.reset");
        assert_eq!(audit[2], ("settings.sound.test".to_string(), "keyboard page".to_string()));
        assert_eq!(p.sound_jobs.lock().unwrap().len(), 3);
    }

    #[test]
    fn the_rows_show_the_fake_machine_the_touch_screens_and_the_sound_output() {
        let p = FakeDevicePort::new(true, "kb-rows");
        *p.panels.lock().unwrap() = vec!["Panel A".into()];
        p.sound.lock().unwrap().volume_pct = 35;
        let a = kb_app("rows");
        let rows = a.kbd_rows(&p);
        let find = |l: &str| rows.iter().find(|r| r.0 == l).map(|r| r.1.clone()).unwrap_or_default();
        assert!(find("Touch screen").contains("Panel A"), "{rows:?}");
        assert!(find("Sound").contains("35%") && !find("Sound").contains("no sound output"));
        p.sound_out.store(false, Ordering::SeqCst);
        assert!(a.kbd_rows(&p).iter().any(|r| r.0 == "Sound" && r.1.contains("no sound output found")));
    }

    #[test]
    fn the_page_draws_at_several_sizes() {
        let p = FakeDevicePort::new(true, "kb-draw");
        let mut a = kb_app("draw");
        for (w, h) in SIZES {
            let g = draw_at(&mut a, w, h, |a, g, r, s| a.render_keyboard(g, r, s, &p));
            assert!(!g.layout_log.texts.is_empty(), "{w}x{h}");
        }
    }
}

mod usbc {
    use super::*;
    use iyakki_shared::directlink::fake::FakeEnv;
    use iyakki_shared::directlink::Env;
    use std::sync::Arc;

    fn setup(admin: bool, tag: &str) -> (SettingsApp, FakeDevicePort, Arc<FakeEnv>) {
        let p = FakeDevicePort::new(admin, tag);
        let env = Arc::new(FakeEnv::new());
        *p.usb_env.lock().unwrap() = Some(env.clone());
        let mut a = app_on(Tab::Developer);
        a.devel.usb.sync = true;
        (a, p, env)
    }

    #[test]
    fn the_page_gets_its_view_of_the_cable_from_the_port() {
        let (mut a, p, env) = setup(true, "usb-env");
        a.handle_usb(&Key::Char('r'), &p);
        assert!(a.devel.usb.report().is_some(), "the check ran against the port's env");
        assert!(a.devel.usb.env.is_some());
        // without an env from the port the page falls back to the made-up machine and still checks
        *p.usb_env.lock().unwrap() = None;
        let mut b = app_on(Tab::Developer);
        b.devel.usb.sync = true;
        b.handle_usb(&Key::Char('r'), &p);
        assert!(b.devel.usb.report().is_some());
        let _ = env;
    }

    #[test]
    fn the_two_switches_save_in_the_envs_config_and_are_audited() {
        let (mut a, p, env) = setup(true, "usb-switch");
        let before = env.config();
        a.handle_usb(&Key::Char('f'), &p);
        a.handle_usb(&Key::Char('m'), &p);
        let after = env.config();
        assert_eq!(after.fixed_pair, !before.fixed_pair);
        assert_eq!(after.allow_more, !before.allow_more);
        let audit = p.audited();
        assert_eq!(audit.len(), 2);
        assert!(audit.iter().all(|l| l.0 == "directlink.config"));
    }

    #[test]
    fn a_person_who_is_not_an_administrator_cannot_authorize_pair_or_switch() {
        let (mut a, p, env) = setup(false, "usb-deny");
        let before = env.config();
        for c in ['z', 'p', 's', 'f', 'm'] {
            a.handle_usb(&Key::Char(c), &p);
        }
        assert_eq!(env.config(), before);
        assert!(p.audited().iter().all(|l| l.0 == "settings.denied"), "{:?}", p.audited());
        assert!(!a.devel.usb.code_focus, "no pairing code asked for");
        // anyone may look again and watch the port
        a.handle_usb(&Key::Char('r'), &p);
        assert!(a.devel.usb.report().is_some());
    }

    #[test]
    fn authorizing_is_audited_and_the_page_draws_at_several_sizes() {
        let (mut a, p, _env) = setup(true, "usb-draw");
        a.handle_usb(&Key::Char('z'), &p);
        assert_eq!(p.audited(), vec![("directlink.authorize".to_string(), "approve the Thunderbolt device on the cable".to_string())]);
        for (w, h) in SIZES {
            let g = draw_at(&mut a, w, h, |a, g, r, s| a.render_usb(g, r, s, &p));
            assert!(!g.layout_log.texts.is_empty(), "{w}x{h}");
        }
    }
}

mod developer {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn a_person_who_is_not_an_administrator_changes_nothing_and_it_is_audited() {
        let p = FakeDevicePort::new(false, "dev-deny");
        let mut a = app_on(Tab::Developer);
        for c in ['K', 'Y', 'U', 'R', 'W', 'e'] {
            a.handle_developer(&Key::Char(c), &p);
        }
        assert!(!p.dev.core_on.load(Ordering::SeqCst) && !p.dev.relay_on.load(Ordering::SeqCst));
        assert!(!p.ui_socket.load(Ordering::SeqCst) && !p.resume_switch.load(Ordering::SeqCst) && !p.settings_get().wasm_tiles);
        assert!(p.dev.called().is_empty());
        assert!(p.audited().iter().all(|l| l.0 == "settings.denied"));
    }

    #[test]
    fn the_core_contract_the_ui_socket_the_resume_and_the_wasm_switches_save_and_are_audited() {
        let p = FakeDevicePort::new(true, "dev-switches");
        let mut a = app_on(Tab::Developer);
        a.handle_developer(&Key::Char('K'), &p);
        assert!(p.dev.core_on.load(Ordering::SeqCst));
        a.handle_developer(&Key::Char('U'), &p);
        assert!(p.ui_socket.load(Ordering::SeqCst));
        a.handle_developer(&Key::Char('R'), &p);
        assert!(p.resume_switch.load(Ordering::SeqCst));
        a.handle_developer(&Key::Char('W'), &p);
        assert!(p.settings_get().wasm_tiles && p.wasm_tiles_enabled());
        assert_eq!(
            p.audited(),
            vec![
                ("settings.developer".to_string(), "core contract on".to_string()),
                ("settings.developer".to_string(), "ui socket on".to_string()),
                ("settings.developer".to_string(), "resume after restart on".to_string()),
                ("settings.wasm_tiles".to_string(), "on".to_string()),
            ]
        );
        // pressing again switches them off
        for c in ['K', 'U', 'R', 'W'] {
            a.handle_developer(&Key::Char(c), &p);
        }
        assert!(!p.dev.core_on.load(Ordering::SeqCst) && !p.ui_socket.load(Ordering::SeqCst) && !p.resume_switch.load(Ordering::SeqCst) && !p.settings_get().wasm_tiles);
    }

    #[test]
    fn busy_settings_stop_the_wasm_switch_and_nothing_is_audited_for_it() {
        let p = FakeDevicePort::new(true, "dev-busy");
        p.busy.store(true, Ordering::SeqCst);
        let mut a = app_on(Tab::Developer);
        a.handle_developer(&Key::Char('W'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("busy")));
        assert!(p.audited().is_empty());
    }

    #[test]
    fn the_relay_needs_an_address_before_it_goes_on_and_a_refusal_shows() {
        let p = FakeDevicePort::new(true, "dev-relay");
        let mut a = app_on(Tab::Developer);
        a.handle_developer(&Key::Char('Y'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("relay's address")));
        assert!(p.audited().is_empty());
        *p.dev.relay_address.lock().unwrap() = "relay.example:443".into();
        a.handle_developer(&Key::Char('Y'), &p);
        assert!(p.dev.relay_on.load(Ordering::SeqCst));
        assert_eq!(p.audited(), vec![("settings.developer".to_string(), "relay on".to_string())]);
        *p.dev.refuse.lock().unwrap() = Some("the owner's policy does not allow this".into());
        a.handle_developer(&Key::Char('Y'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("policy")));
        assert!(p.dev.relay_on.load(Ordering::SeqCst), "a refused change leaves it as it was");
        assert_eq!(p.audited().len(), 1);
    }

    #[test]
    fn switching_developer_mode_off_is_audited() {
        let p = FakeDevicePort::new(true, "dev-off");
        p.dev.on.store(true, Ordering::SeqCst);
        let mut a = app_on(Tab::Developer);
        a.handle_developer(&Key::Char('e'), &p);
        assert!(!p.dev.on.load(Ordering::SeqCst));
        assert_eq!(p.audited(), vec![("devmode.off".to_string(), "switched off in Settings".to_string())]);
    }

    #[test]
    fn the_page_draws_off_and_on_at_several_sizes() {
        let p = FakeDevicePort::new(true, "dev-draw");
        *p.addresses.lock().unwrap() = vec![("eth0".into(), "192.168.1.5".into())];
        let mut a = app_on(Tab::Developer);
        for on in [false, true] {
            p.dev.on.store(on, Ordering::SeqCst);
            for (w, h) in SIZES {
                let g = draw_at(&mut a, w, h, |a, g, r, s| a.render_developer(g, r, s, &p));
                assert!(!g.layout_log.texts.is_empty(), "{w}x{h} on={on}");
            }
        }
    }
}

mod dev_cluster {
    use super::*;
    use super::super::devcluster::DcAct;
    use iyakki_shared::k3sdev::State;
    use std::sync::atomic::Ordering;

    fn installed(p: &FakeDevicePort) {
        *p.cluster.installed.lock().unwrap() = Some(State { version: "v1.30".into(), ..State::default() });
    }

    #[test]
    fn a_person_who_is_not_an_administrator_cannot_touch_the_machines_cluster() {
        let p = FakeDevicePort::new(false, "dc-deny");
        installed(&p);
        let mut a = app_on(Tab::DevCluster);
        for c in ['a', 'f', 'n', 'l', 'p'] {
            a.handle_devcluster(&Key::Char(c), &p);
        }
        assert!(p.cluster.called().is_empty());
        assert!(!p.cluster.autostart.load(Ordering::SeqCst) && !p.cluster.fixed_ip.load(Ordering::SeqCst) && !p.cluster.offline.load(Ordering::SeqCst));
        assert!(p.audited().iter().all(|l| l.0 == "settings.denied"), "{:?}", p.audited());
        assert!(a.dc_has_overlay() == false);
    }

    #[test]
    fn without_a_cluster_of_its_own_the_switches_say_so() {
        let p = FakeDevicePort::new(true, "dc-none");
        let mut a = app_on(Tab::DevCluster);
        for c in ['a', 'f', 'n', 'l', 'p'] {
            a.handle_devcluster(&Key::Char(c), &p);
            assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("not installed")), "{c}: {:?}", a.status());
        }
        assert!(p.cluster.called().is_empty());
    }

    #[test]
    fn autostart_the_fixed_address_and_offline_are_asked_of_the_cluster_and_a_refusal_shows() {
        let p = FakeDevicePort::new(true, "dc-switches");
        installed(&p);
        let mut a = app_on(Tab::DevCluster);
        a.handle_devcluster(&Key::Char('a'), &p);
        a.handle_devcluster(&Key::Char('f'), &p);
        a.handle_devcluster(&Key::Char('n'), &p);
        assert!(p.cluster.autostart.load(Ordering::SeqCst) && p.cluster.fixed_ip.load(Ordering::SeqCst) && p.cluster.offline.load(Ordering::SeqCst));
        assert_eq!(p.cluster.called(), ["autostart true", "fixed ip true", "offline true"]);
        a.handle_devcluster(&Key::Char('n'), &p);
        assert!(!p.cluster.offline.load(Ordering::SeqCst));
        *p.cluster.refuse.lock().unwrap() = Some("k3s refused".into());
        a.handle_devcluster(&Key::Char('f'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("k3s refused")));
    }

    #[test]
    fn the_log_opens_in_an_overlay_and_an_upgrade_needs_a_download_connection() {
        let p = FakeDevicePort::new(true, "dc-log");
        installed(&p);
        *p.cluster.log.lock().unwrap() = "level=info started\nlevel=error something broke\n".into();
        let mut a = app_on(Tab::DevCluster);
        a.handle_devcluster(&Key::Char('l'), &p);
        assert!(a.dc_has_overlay());
        a.handle_devcluster(&Key::Char('p'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("No network connection")));
        assert!(p.audited().is_empty());
    }

    #[test]
    fn start_stop_and_restart_run_as_jobs_and_name_the_person() {
        let p = FakeDevicePort::new(true, "dc-run");
        installed(&p);
        let mut a = app_on(Tab::DevCluster);
        for act in [DcAct::Start, DcAct::Stop, DcAct::Restart] {
            a.dc_confirm(&p, act);
            let out = a.job.take().expect("a job was started").rx.recv().unwrap();
            assert!(matches!(out, JobOut::Msg(Ok(_))));
        }
        assert_eq!(p.cluster.called(), ["start", "stop", "restart"]);
        // a person who is not an administrator starts nothing
        let q = FakeDevicePort::new(false, "dc-run-deny");
        a.dc_confirm(&q, DcAct::Start);
        assert!(a.job.is_none() && q.cluster.called().is_empty());
    }

    #[test]
    fn setup_needs_a_download_connection_and_the_page_draws_at_several_sizes() {
        let p = FakeDevicePort::new(true, "dc-draw");
        let mut a = app_on(Tab::DevCluster);
        a.handle_devcluster(&Key::Char('P'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("not available")), "the fake has no platform services");
        a.handle_devcluster(&Key::Char('C'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("not available")));
        for (w, h) in SIZES {
            let g = draw_at(&mut a, w, h, |a, g, r, s| a.render_devcluster(g, r, s, &p));
            assert!(!g.layout_log.texts.is_empty(), "{w}x{h}");
        }
    }
}

mod git_keys {
    use super::*;
    use super::super::git_page::{GRow, GitSetting};
    use iyakki_shared::gitconn::{Connection, Kind};

    fn make_key(a: &mut SettingsApp, p: &FakeDevicePort, name: &str) -> Result<String, String> {
        let mut f = Form::new("Generate an SSH key (ed25519)").text("Name", name, "").text("Comment", "amy@iyakki", "");
        a.git_submit(p, &GitSetting::NewKey, &mut f)
    }

    #[test]
    fn a_key_is_made_listed_used_by_a_host_and_deleted_only_when_its_name_is_typed() {
        let p = FakeDevicePort::new(false, "git-keys");
        let mut a = app_on(Tab::GitKeys);
        assert!(make_key(&mut a, &p, "work").unwrap().contains("Made the key work"));
        assert!(SettingsApp::git_rows(&p).iter().any(|r| matches!(r, GRow::Key(k) if k.id == "work")));
        let mut f = Form::new("host key").text("Host", "git.example.org", "").choice("Key", &["(no key)", "work"], 1);
        assert!(a.git_submit(&p, &GitSetting::HostKey, &mut f).unwrap().contains("uses the key work"));
        assert!(SettingsApp::git_rows(&p).iter().any(|r| matches!(r, GRow::Host(h, _) if h == "git.example.org")));
        let mut wrong = Form::new("delete").text("Type work", "nope", "");
        assert!(a.git_submit(&p, &GitSetting::DeleteKey("work".into()), &mut wrong).is_err());
        let mut right = Form::new("delete").text("Type work", "work", "");
        assert!(a.git_submit(&p, &GitSetting::DeleteKey("work".into()), &mut right).is_ok());
        assert!(!SettingsApp::git_rows(&p).iter().any(|r| matches!(r, GRow::Key(_))));
        assert!(!p.audited().is_empty(), "the key service audits what it does");
    }

    #[test]
    fn a_bad_name_is_refused_with_the_reason() {
        let p = FakeDevicePort::new(false, "git-bad");
        let mut a = app_on(Tab::GitKeys);
        assert!(make_key(&mut a, &p, "Not A Name!").is_err());
        assert!(SettingsApp::git_rows(&p).is_empty());
    }

    #[test]
    fn a_token_is_kept_in_the_vault_and_the_host_row_shows_it() {
        let p = FakeDevicePort::new(false, "git-token");
        let mut a = app_on(Tab::GitKeys);
        let mut f = Form::new("token").text("Host", "git.example.org", "").text("User", "amy", "").secret("Token", "a personal access token");
        f.set_value(2, "s3cret-token");
        assert!(a.git_submit(&p, &GitSetting::Token, &mut f).unwrap().contains("in your vault"));
        assert!(SettingsApp::git_rows(&p).iter().any(|r| matches!(r, GRow::Host(h, rule) if h == "git.example.org" && rule.token)));
        assert!(!p.audited().iter().any(|l| l.1.contains("s3cret-token")), "the token is never audited");
    }

    #[test]
    fn importing_a_key_from_a_file_outside_the_accounts_folders_is_refused() {
        let p = FakeDevicePort::new(false, "git-import");
        let mut a = app_on(Tab::GitKeys);
        let mut f = Form::new("import").text("Name", "other", "").text("Key", "/etc/shadow", "").secret("Passphrase", "").choice("kept", &["session", "vault"], 0);
        let e = a.git_submit(&p, &GitSetting::ImportKey, &mut f).unwrap_err();
        assert!(e.contains("outside the folders"), "{e}");
    }

    #[test]
    fn a_connection_to_a_host_the_account_may_not_reach_is_not_tested_and_says_why() {
        let p = FakeDevicePort::new(false, "git-reach");
        let mut a = app_on(Tab::GitKeys);
        let c = p.git_keys().conns().add(Connection { name: "mine".into(), url: "https://git.example.org".into(), kind: Kind::Gitea, ..Connection::default() }, Some("tok")).unwrap();
        p.hosts_blocked.store(true, std::sync::atomic::Ordering::SeqCst);
        a.git_test_conn(&p, &c);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("may not reach")));
        assert!(a.git.testing.is_none());
        assert!(p.git_http.lines().is_empty(), "nothing went out to the network");
    }

    #[test]
    fn the_wizard_opens_from_the_fakes_environment_and_the_page_draws_at_several_sizes() {
        let p = FakeDevicePort::new(false, "git-wiz");
        let mut a = app_on(Tab::GitKeys);
        a.handle_git(&Key::Char('a'), &p);
        assert!(a.git_wizard_open());
        for (w, h) in SIZES {
            let g = draw_at(&mut a, w, h, |a, g, r, s| a.render_git(g, r, s, &p));
            assert!(!g.layout_log.texts.is_empty() || a.git_wizard_open(), "{w}x{h}");
        }
    }
}

mod display_page {
    use super::*;

    /// The text size, theme and wallpaper are process-wide in the toolkit: a test that changes them holds this lock.
    fn globals() -> iyakki_testkit::lock::Lock {
        iyakki_testkit::lock::lock("appearance-globals")
    }

    fn display_app(tag: &str) -> SettingsApp {
        let cx = crate::apps::tests::cx_with_admin_store(tag);
        let (env, _) = super::super::tests::fake_machine(tag);
        SettingsApp::new(&cx).on(Tab::Display).with_hardware(env)
    }

    #[test]
    fn a_person_who_is_not_an_administrator_is_refused_a_row_key_and_it_is_audited() {
        let p = FakeDevicePort::new(false, "disp-deny");
        let mut a = display_app("disp-deny");
        a.handle_display(&Key::Char('+'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("administrators")));
        assert_eq!(p.audited().into_iter().map(|l| l.0).collect::<Vec<_>>(), vec!["settings.denied".to_string()]);
    }

    #[test]
    fn the_text_size_keys_save_for_this_person_and_are_audited_and_stop_at_the_ends() {
        let _g = globals();
        let p = FakeDevicePort::new(false, "disp-size");
        let mut a = display_app("disp-size");
        let before = crate::uisize::load(&p.data_dir(), &p.user());
        a.handle_display(&Key::Char('u'), &p);
        assert_eq!(crate::uisize::load(&p.data_dir(), &p.user()), (before + 1).min(crate::uisize::LEVELS.len() - 1));
        assert!(p.audited().iter().any(|l| l.0 == "settings.display.text_size"));
        for _ in 0..crate::uisize::LEVELS.len() {
            a.handle_display(&Key::Char('U'), &p);
        }
        assert_eq!(crate::uisize::load(&p.data_dir(), &p.user()), 0);
        assert!(a.status().is_some_and(|s| s.0.contains("smallest")), "the smallest size says so and changes nothing");
        crate::uisize::apply(Some(before));
    }

    #[test]
    fn a_theme_key_chooses_the_theme_is_audited_and_a_number_out_of_range_is_told() {
        let _g = globals();
        let p = FakeDevicePort::new(false, "disp-theme");
        let mut a = display_app("disp-theme");
        let before = crate::theme::current_id();
        a.handle_display(&Key::Char('2'), &p);
        assert_eq!(crate::theme::current_id(), crate::theme::ALL[1]);
        assert!(p.audited().iter().any(|l| l.0 == "settings.display.theme"));
        a.handle_display(&Key::Char('9'), &p);
        assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("no theme")));
        crate::theme::apply(Some(before));
    }

    #[test]
    fn the_wallpaper_keys_change_it_for_this_person_and_are_audited() {
        let _g = globals();
        let p = FakeDevicePort::new(false, "disp-wall");
        let mut a = display_app("disp-wall");
        let before = iyakki_uikit::wallpaper::current();
        a.handle_display(&Key::Char('w'), &p);
        assert_ne!(iyakki_uikit::wallpaper::current(), before);
        assert!(p.audited().iter().any(|l| l.0 == "settings.display.wallpaper"));
        iyakki_uikit::wallpaper::set_current(before);
    }

    #[test]
    fn look_again_changes_nothing_and_says_so() {
        let p = FakeDevicePort::new(true, "disp-look");
        let mut a = display_app("disp-look");
        a.handle_display(&Key::Char('r'), &p);
        assert!(a.status().is_some_and(|s| s.0.contains("again")));
        assert!(p.audited().is_empty());
    }

    #[test]
    fn the_page_draws_its_cards_and_rows_at_several_sizes() {
        let _g = globals();
        let p = FakeDevicePort::new(true, "disp-draw");
        for (w, h) in SIZES {
            let mut a = display_app("disp-draw");
            let g = draw_at(&mut a, w, h, |a, g, r, s| a.render_display(g, r, s, &p));
            assert_eq!((g.width(), g.height()), (w, h));
        }
    }
}

mod bluetooth_page {
    use super::*;

    fn bt_app(tag: &str) -> SettingsApp {
        let cx = crate::apps::tests::cx_with_admin_store(tag);
        let (env, _) = super::super::tests::fake_machine(tag);
        SettingsApp::new(&cx).on(Tab::Bluetooth).with_hardware(env)
    }

    #[test]
    fn a_person_who_is_not_an_administrator_does_not_reach_the_page_and_nothing_is_logged() {
        let p = FakeDevicePort::new(false, "bt-deny");
        let mut a = bt_app("bt-deny");
        assert!(!a.bluetooth_event(&Event::Key(Key::Char('e')), &p));
        assert!(p.audited().is_empty(), "a refused event is not logged");
    }

    #[test]
    fn while_the_navigation_has_focus_a_key_does_not_reach_the_page() {
        let p = FakeDevicePort::new(true, "bt-nav");
        let mut a = bt_app("bt-nav");
        a.nav_focus = true;
        assert!(!a.bluetooth_event(&Event::Key(Key::Char('e')), &p));
        assert!(p.audited().is_empty());
    }

    #[test]
    fn the_page_draws_at_several_sizes() {
        let p = FakeDevicePort::new(true, "bt-draw");
        for (w, h) in SIZES {
            let mut a = bt_app("bt-draw");
            let g = draw_at(&mut a, w, h, |a, g, r, s| {
                a.bt_tick(&p);
                a.bt.render(g, r, s)
            });
            assert_eq!((g.width(), g.height()), (w, h));
        }
    }
}

mod sound_page {
    use super::*;

    #[test]
    fn the_rows_draw_at_several_sizes_and_a_look_down_the_list_changes_nothing_on_the_port() {
        let p = FakeDevicePort::new(true, "snd-page");
        let cx = crate::apps::tests::cx_with_admin_store("snd-page");
        let (env, _) = super::super::tests::fake_machine("snd-page");
        let mut a = SettingsApp::new(&cx).on(Tab::Sound).with_hardware(env);
        a.handle_sound(&Key::Down, &p);
        assert!(p.audited().is_empty());
        for (w, h) in SIZES {
            let mut a = SettingsApp::new(&cx).on(Tab::Sound).with_hardware(super::super::tests::fake_machine("snd-page-draw").0);
            let g = draw_at(&mut a, w, h, |a, g, r, s| a.render_sound(g, r, s, &p));
            assert_eq!((g.width(), g.height()), (w, h));
        }
    }
}
