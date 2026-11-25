//! The Apps and AI Settings pages on the fake port of lane ST-B (`settings_b_fakes_tests.rs`): a change is made and audited, someone who is not an
//! administrator is refused and the refusal is audited, a refused or empty answer is shown in words, and the page draws at several sizes.
//! The settings, the audit lines, the person and the clock are in memory; nothing sleeps. One module per page.

use super::b_fakes_tests::FakeB;
use super::super::settings_port::SettingsDataPort;
use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};
use std::sync::atomic::Ordering;

const SIZES: [(u32, u32); 5] = [(360, 740), (800, 480), (1280, 720), (1920, 1080), (480, 600)];

fn app_on(tag: &str, tab: Tab) -> SettingsApp {
    let cx = cx_with_admin_store(tag);
    let mut a = SettingsApp::new(&cx);
    a.go_to(tab);
    a
}

fn failed(a: &SettingsApp) -> Option<String> {
    a.status().filter(|s| !s.1).map(|s| s.0.clone())
}

fn said(a: &SettingsApp) -> Option<String> {
    a.status().filter(|s| s.1).map(|s| s.0.clone())
}

fn blank(w: u32, h: u32) -> (Gfx, R) {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    (g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0))
}

mod app_events {
    use super::*;
    use iyakki_core_api::appevents::AppEvents;

    fn p_with_two_registrations(tag: &str) -> FakeB {
        let p = FakeB::new(true, temp_dir(tag));
        p.events.grant("notes", "events.network");
        p.events.grant("notes", "events.power");
        p.events.register("amy", "notes", "connectivity.changed", "").unwrap();
        p.events.register("amy", "notes", "peers.changed", "").unwrap();
        p
    }

    #[test]
    fn the_switch_goes_through_the_core_and_is_audited_by_names() {
        let p = p_with_two_registrations("b-ev-switch");
        let mut a = app_on("b-ev-switch-app", Tab::AppEvents);
        assert_eq!(SettingsApp::appevents_len(&p), 2);
        a.handle_appevents(&Key::Enter, &p);
        assert!(!SettingsApp::appevents_rows(&p)[0].enabled);
        assert_eq!(p.audited(), vec![("appevents.switch".to_string(), "notes connectivity.changed off".to_string())]);
        assert!(said(&a).is_some_and(|m| m.contains("no longer hears connectivity.changed")));
        a.appevents_sub(0, 't', &p);
        assert!(SettingsApp::appevents_rows(&p)[0].enabled);
    }

    #[test]
    fn taking_the_permission_away_ends_what_needed_it_and_is_audited() {
        let p = p_with_two_registrations("b-ev-perm");
        let mut a = app_on("b-ev-perm-app", Tab::AppEvents);
        a.handle_appevents(&Key::Char('p'), &p);
        assert!(said(&a).is_some_and(|m| m.contains("can no longer use events.network")), "{:?}", a.status());
        assert!(p.audited().iter().any(|(a, s)| a == "appevents.permission" && s.contains("events.network taken away")));
        assert!(SettingsApp::appevents_rows(&p).is_empty(), "both registrations needed that scope");
    }

    #[test]
    fn nothing_chosen_and_a_refusal_of_the_core_are_told_in_words() {
        let p = FakeB::new(true, temp_dir("b-ev-none"));
        let mut a = app_on("b-ev-none-app", Tab::AppEvents);
        a.handle_appevents(&Key::Char('t'), &p);
        assert_eq!(failed(&a).as_deref(), Some("Nothing is chosen."));
        a.handle_appevents(&Key::Char('p'), &p);
        assert_eq!(failed(&a).as_deref(), Some("Nothing is chosen."));
        assert!(p.audited().is_empty(), "nothing happened, nothing is audited");
    }

    #[test]
    fn the_page_draws_empty_and_with_rows_at_five_sizes() {
        let empty = FakeB::new(true, temp_dir("b-ev-draw0"));
        let full = p_with_two_registrations("b-ev-draw1");
        let mut a = app_on("b-ev-draw-app", Tab::AppEvents);
        for (w, h) in SIZES {
            let (mut g, r) = blank(w, h);
            a.render_appevents(&mut g, r, 1.0, &empty);
            let (mut g, r) = blank(w, h);
            a.render_appevents(&mut g, r, 1.0, &full);
            assert!(g.hits.iter().any(|x| x.ev == Event::Sub(0, Key::Char('t'))), "{w}x{h}: the switch of the first row");
        }
    }
}

mod app_access {
    use super::*;
    use iyakki_core_api::appevents::AppEvents;
    use iyakki_shared::accessbroker::appcaps::{record_approval, ApprovalBook, CapabilityRequest};

    fn approved(tag: &str, admin: bool) -> FakeB {
        let p = FakeB::new(admin, temp_dir(tag));
        let store = iyakki_agent::store::Store::open(&p.dir);
        let mut book = iyakki_agent::store::approvals::FileBook(&store);
        let reqs: Vec<CapabilityRequest> = ["camera.view", "notify"].iter().map(|c| CapabilityRequest { name: (*c).into(), why: "x".into(), param: None, extra: Default::default() }).collect();
        book.put("tile/user/amy/notes", record_approval("amy", "notes", "1.2", "abc", 5, &reqs));
        p
    }

    #[test]
    fn revoking_asks_the_broker_audits_and_tells_the_app() {
        let p = approved("b-aa-revoke", true);
        p.events.grant("notes", "camera.view");
        let mut a = app_on("b-aa-revoke-app", Tab::AppAccess);
        assert_eq!(SettingsApp::appaccess_len(&p), 2);
        a.handle_appaccess(&Key::Char('r'), &p);
        assert_eq!(*p.access_calls.lock().unwrap(), vec![("tile/user/amy/notes".to_string(), "camera.view".to_string(), None)]);
        assert!(p.audited().iter().any(|(a, s)| a == "access.revoke" && s.starts_with("notes 1.2 camera.view")));
        assert!(said(&a).is_some_and(|m| m.contains("can no longer use camera.view")));
        let told = p.events.poll("amy", "notes", 10);
        assert!(told.iter().all(|e| e.name == "app.permissions") || told.is_empty());
    }

    #[test]
    fn someone_who_is_not_an_administrator_changes_nothing_and_the_refusal_is_audited() {
        let p = approved("b-aa-user", false);
        let mut a = app_on("b-aa-user-app", Tab::AppAccess);
        a.handle_appaccess(&Key::Char('r'), &p);
        a.handle_appaccess(&Key::Char('c'), &p);
        a.handle_appaccess(&Key::Char('e'), &p);
        assert!(p.access_calls.lock().unwrap().is_empty());
        assert_eq!(p.audited().iter().filter(|(a, _)| a == "settings.denied").count(), 3);
        assert!(failed(&a).is_some_and(|m| m.contains("administrators")));
        assert!(!a.has_modal(), "no limit form for a person who may not change it");
    }

    #[test]
    fn a_refusal_of_the_broker_is_shown_and_not_audited_as_done() {
        let p = approved("b-aa-refuse", true);
        *p.access_refusal.lock().unwrap() = Some("The access table cannot be written.".into());
        let mut a = app_on("b-aa-refuse-app", Tab::AppAccess);
        a.handle_appaccess(&Key::Char('r'), &p);
        assert_eq!(failed(&a).as_deref(), Some("The access table cannot be written."));
        assert!(p.audited().is_empty());
    }

    #[test]
    fn a_time_limit_is_checked_passed_to_the_broker_and_audited() {
        let p = approved("b-aa-limit", true);
        let mut a = app_on("b-aa-limit-app", Tab::AppAccess);
        let form = |v: &str| Form::new("Time limit").text("Hours", v, "");
        assert!(a.appaccess_limit_submit(&p, "tile/user/amy/notes", "notify", &form("0")).is_err());
        assert!(a.appaccess_limit_submit(&p, "tile/user/amy/notes", "notify", &form("9000")).is_err());
        assert!(p.access_calls.lock().unwrap().is_empty(), "a bad number never reaches the broker");
        let ok = a.appaccess_limit_submit(&p, "tile/user/amy/notes", "notify", &form("24")).unwrap();
        assert!(ok.contains("24 more hour"));
        let none = a.appaccess_limit_submit(&p, "tile/user/amy/notes", "notify", &form("")).unwrap();
        assert!(none.contains("no time limit"));
        assert_eq!(p.access_calls.lock().unwrap().iter().map(|c| c.2).collect::<Vec<_>>(), vec![Some(24), None]);
        assert!(p.audited().iter().any(|(a, s)| a == "access.limit" && s.ends_with("24 h")));
        p.admin.store(false, Ordering::SeqCst);
        assert!(a.appaccess_limit_submit(&p, "tile/user/amy/notes", "notify", &form("5")).is_err());
    }

    #[test]
    fn clearing_the_log_is_audited_and_the_page_draws_at_five_sizes() {
        let p = approved("b-aa-clear", true);
        let mut a = app_on("b-aa-clear-app", Tab::AppAccess);
        a.handle_appaccess(&Key::Char('c'), &p);
        assert!(p.audited().iter().any(|(a, _)| a == "access.log.clear"));
        assert!(said(&a).is_some_and(|m| m.contains("empty")));
        for (w, h) in SIZES {
            let (mut g, r) = blank(w, h);
            a.render_appaccess(&mut g, r, 1.0, &p);
        }
    }
}

mod host_access {
    use super::*;

    #[test]
    fn changing_the_mode_of_a_shared_folder_is_audited_and_listed() {
        let _g = iyakki_shared::hostfiles::test_guard();
        let base = temp_dir("b-host-base");
        let p = FakeB::new(true, temp_dir("b-host-mode"));
        std::fs::create_dir_all(base.join("Music")).unwrap();
        iyakki_shared::hostfiles::set_enabled(true);
        iyakki_shared::hostfiles::set_host_home(Some(std::fs::canonicalize(&base).unwrap()));
        let mut m = iyakki_shared::hostfiles::HostMounts::load(&p.dir, &p.user);
        m.add(&std::fs::canonicalize(&base).unwrap(), "~/Music", None, false).unwrap();
        let mut a = app_on("b-host-mode-app", Tab::HostAccess);
        assert_eq!(SettingsApp::host_rows(&p).len(), 1);
        a.handle_host(&Key::Char('m'), &p);
        assert!(SettingsApp::host_rows(&p)[0].0.write);
        assert_eq!(p.audited(), vec![("host.mount.mode".to_string(), "host:Music read-write".to_string())]);
        a.handle_host(&Key::Char('m'), &p);
        assert!(!SettingsApp::host_rows(&p)[0].0.write);
        // the audit trail shows the lines the page's own port gives back
        a.handle_host(&Key::Char('l'), &p);
        for (w, h) in SIZES {
            let (mut g, r) = blank(w, h);
            a.render_host(&mut g, r, 1.0, &p);
        }
        iyakki_shared::hostfiles::set_host_home(None);
        iyakki_shared::hostfiles::set_enabled(false);
    }

    #[test]
    fn off_the_desktop_window_the_page_says_so_and_keys_do_nothing() {
        let _g = iyakki_shared::hostfiles::test_guard();
        iyakki_shared::hostfiles::set_enabled(false);
        let p = FakeB::new(true, temp_dir("b-host-off"));
        let mut a = app_on("b-host-off-app", Tab::HostAccess);
        a.handle_host(&Key::Char('a'), &p);
        a.handle_host(&Key::Char('m'), &p);
        assert!(!a.host_dialog_open() && p.audited().is_empty());
        for (w, h) in SIZES {
            let (mut g, r) = blank(w, h);
            a.render_host(&mut g, r, 1.0, &p);
        }
    }

    #[test]
    fn the_dialogs_get_the_fake_environment_of_the_person() {
        let _g = iyakki_shared::hostfiles::test_guard();
        iyakki_shared::hostfiles::set_enabled(true);
        let p = FakeB::new(true, temp_dir("b-host-env"));
        let mut a = app_on("b-host-env-app", Tab::HostAccess);
        a.handle_host(&Key::Char('a'), &p);
        assert!(a.host_dialog_open(), "the share dialog opens over the page");
        iyakki_shared::hostfiles::set_enabled(false);
    }
}

mod rag_order {
    use super::*;
    use iyakki_shared::ragraph;
    use iyakki_shared::ragrerank::Mode;

    #[test]
    fn graph_rag_and_the_reranker_are_saved_in_the_data_folder_and_audited() {
        let p = FakeB::new(true, temp_dir("b-ro-save"));
        let mut a = app_on("b-ro-save-app", Tab::Rag);
        a.handle_rag_order(&Key::Char('g'), &p);
        assert!(ragraph::is_on(&p.dir));
        a.handle_rag_order(&Key::Char('o'), &p);
        assert_eq!(Mode::load(&p.dir), Mode::Lexical);
        a.handle_rag_order(&Key::Char('o'), &p);
        assert_eq!(Mode::load(&p.dir), Mode::Off);
        assert_eq!(p.audited().iter().map(|(a, s)| format!("{a}:{s}")).collect::<Vec<_>>().first().map(String::as_str), Some("settings.rag:graph on"));
        assert_eq!(p.audited().len(), 3);
    }

    #[test]
    fn the_model_form_opens_with_the_saved_name_and_a_submit_is_audited() {
        let p = FakeB::new(true, temp_dir("b-ro-model"));
        let mut a = app_on("b-ro-model-app", Tab::Rag);
        a.handle_rag_order(&Key::Char('O'), &p);
        assert!(a.has_modal());
        let form = Form::new("Reranking model").text("Model name", "bge-reranker", "");
        let msg = a.rag_rerank_submit(&p, &form).unwrap();
        assert!(msg.to_lowercase().contains("rerank") || !msg.is_empty());
        assert_eq!(Mode::load(&p.dir), Mode::Model { model: "bge-reranker".into() });
        assert!(p.audited().iter().any(|(a, s)| a == "settings.rag" && s.starts_with("rerank")));
    }

    #[test]
    fn the_two_rows_draw_in_one_and_two_columns_at_five_sizes() {
        let p = FakeB::new(true, temp_dir("b-ro-draw"));
        let mut a = app_on("b-ro-draw-app", Tab::Rag);
        for (w, h) in SIZES {
            for two in [false, true] {
                let (mut g, r) = blank(w, h);
                let used = a.render_rag_order(&mut g, r, 1.0, &p, true, two);
                assert!(used > 0.0);
                assert!(g.hits.iter().any(|x| x.ev == Event::Key(Key::Char('g'))), "{w}x{h}: Graph RAG switch");
            }
        }
    }
}

mod skills {
    use super::*;
    use iyakki_shared::skills::Scope;

    const MD: &str = "---\nname: meeting-notes\ndescription: Summarise a meeting\ntriggers: meeting, notes\n---\nList decisions first.\n";

    fn page(tag: &str) -> SettingsApp {
        app_on(tag, Tab::Skills)
    }

    #[test]
    fn a_personal_skill_is_added_listed_and_audited() {
        let p = FakeB::new(false, temp_dir("b-sk-add"));
        let mut a = page("b-sk-add-app");
        assert_eq!(a.add_skill(&p, MD, false).unwrap(), "Saved mine skill `meeting-notes`.");
        let l = p.skills_list_for_test();
        assert_eq!((l.len(), l[0].body.as_str(), l[0].triggers.len()), (1, "List decisions first.", 2));
        assert_eq!(p.audited(), vec![("skills.add".to_string(), "mine:meeting-notes".to_string())]);
    }

    #[test]
    fn shared_skills_need_the_permission_and_the_refusal_is_audited() {
        let p = FakeB::new(false, temp_dir("b-sk-shared"));
        p.manage_skills.store(false, Ordering::SeqCst);
        let mut a = page("b-sk-shared-app");
        assert_eq!(a.add_skill(&p, MD, true).unwrap_err(), "You may not edit the shared skills.");
        assert_eq!(p.audited(), vec![("settings.denied".to_string(), "skills.shared".to_string())]);
        p.manage_skills.store(true, Ordering::SeqCst);
        assert!(a.add_skill(&p, MD, true).unwrap().contains("shared skill"));
    }

    #[test]
    fn a_refusal_of_the_skill_store_comes_back_as_words() {
        let p = FakeB::new(true, temp_dir("b-sk-bad"));
        let mut a = page("b-sk-bad-app");
        assert!(a.add_skill(&p, "no front matter", false).is_err());
        assert!(p.audited().is_empty());
    }

    #[test]
    fn a_file_is_read_only_from_where_the_person_may_read() {
        let p = FakeB::new(false, temp_dir("b-sk-file"));
        p.files.lock().unwrap().insert("/home/amy/tone.md".into(), MD.replace("meeting-notes", "tone"));
        let md = SettingsApp::read_skill_file(&p, " /home/amy/tone.md ").unwrap();
        assert!(md.contains("name: tone"));
        assert!(SettingsApp::read_skill_file(&p, "/etc/passwd").is_err());
    }

    #[test]
    fn deleting_a_shared_skill_asks_only_when_allowed_and_a_user_is_told_why_not() {
        let p = FakeB::new(true, temp_dir("b-sk-del"));
        let mut a = page("b-sk-del-app");
        a.add_skill(&p, MD, true).unwrap();
        p.manage_skills.store(false, Ordering::SeqCst);
        a.handle_skills(&Key::Char('d'), &p);
        assert!(!a.has_modal());
        assert!(failed(&a).is_some_and(|m| m.contains("Only operators and administrators")));
        p.manage_skills.store(true, Ordering::SeqCst);
        a.handle_skills(&Key::Char('d'), &p);
        assert!(a.has_modal(), "deleting asks first");
    }

    #[test]
    fn the_page_draws_empty_and_with_a_skill_at_five_sizes() {
        let p = FakeB::new(true, temp_dir("b-sk-draw"));
        let mut a = page("b-sk-draw-app");
        for (w, h) in SIZES {
            let (mut g, r) = blank(w, h);
            a.render_skills(&mut g, r, 1.0, &p);
        }
        a.add_skill(&p, MD, false).unwrap();
        for (w, h) in SIZES {
            let (mut g, r) = blank(w, h);
            a.render_skills(&mut g, r, 1.0, &p);
        }
        let _ = Scope::Shared;
    }
}

mod alexa_page {
    use super::*;
    use iyakki_shared::alexa;

    fn set_up(p: &FakeB) {
        std::fs::write(p.dir.join(alexa::CONFIG_FILE), r#"{"client_id":"id1","product_id":"p1"}"#).unwrap();
    }

    #[test]
    fn with_no_owner_product_the_switch_and_the_card_refuse_in_words() {
        let p = FakeB::new(true, temp_dir("b-al-closed"));
        let mut a = app_on("b-al-closed-app", Tab::Alexa);
        a.handle_alexa(&Key::Char('o'), &p);
        assert!(failed(&a).is_some_and(|m| m.contains("no client id")));
        a.handle_alexa(&Key::Char('C'), &p);
        assert!(!a.has_modal(), "no card for a product that does not exist");
        assert!(!alexa::load(&p.dir).enabled && p.audited().is_empty());
    }

    #[test]
    fn the_switch_is_for_administrators_and_a_change_is_audited() {
        let p = FakeB::new(false, temp_dir("b-al-switch"));
        set_up(&p);
        let mut a = app_on("b-al-switch-app", Tab::Alexa);
        a.handle_alexa(&Key::Char('o'), &p);
        assert!(failed(&a).is_some_and(|m| m.contains("administrator")));
        assert!(!alexa::load(&p.dir).enabled);
        p.admin.store(true, Ordering::SeqCst);
        a.handle_alexa(&Key::Char('o'), &p);
        assert!(alexa::load(&p.dir).enabled);
        assert_eq!(p.audited(), vec![("settings.alexa".to_string(), "switched on".to_string())]);
    }

    #[test]
    fn the_card_starts_on_no_a_yes_is_kept_and_taking_it_back_asks_nothing() {
        let p = FakeB::new(false, temp_dir("b-al-consent"));
        set_up(&p);
        let mut a = app_on("b-al-consent-app", Tab::Alexa);
        a.handle_alexa(&Key::Char('C'), &p);
        assert!(a.has_modal());
        let no = SettingsApp::alexa_consent_form();
        a.alexa_consent_submit(&p, &no);
        assert!(alexa::consent_date(&p.dir, "amy").is_none(), "Enter alone never agrees");
        let mut yes = SettingsApp::alexa_consent_form();
        yes.set_choice(0, 1);
        a.alexa_consent_submit(&p, &yes);
        assert!(alexa::consent_date(&p.dir, "amy").is_some());
        a.handle_alexa(&Key::Char('C'), &p);
        assert!(alexa::consent_date(&p.dir, "amy").is_none());
        let acts: Vec<String> = p.audited().into_iter().map(|(_, s)| s).collect();
        assert_eq!(acts, vec!["consent given for the voice", "consent taken back"]);
    }

    #[test]
    fn the_page_draws_at_five_sizes_with_the_rows_the_fake_shows() {
        let p = FakeB::new(true, temp_dir("b-al-draw"));
        let mut a = app_on("b-al-draw-app", Tab::Alexa);
        for (w, h) in SIZES {
            let (mut g, r) = blank(w, h);
            a.render_alexa(&mut g, r, 1.0, &p);
        }
        assert!(a.alexa_row_text(&p, a.alexa_rows()[0]).1.contains("Alexa"));
    }
}

mod gemini_card {
    use super::*;
    use super::super::apps_port::SecretsPort;
    use iyakki_shared::gemini::{self, ConsentState};

    fn setup_form(a: &mut SettingsApp, p: &FakeB) -> Form {
        a.gem_open_setup(p);
        match a.modal.take() {
            Some(Modal::Form(FormKind::GeminiSetup, f)) => f,
            _ => panic!("the setup card did not open"),
        }
    }

    const KEY: &str = "AIzaSyFAKE-KEY-0123456789abcdef";

    #[test]
    fn setup_stores_the_key_by_name_saves_the_choices_audits_and_asks_for_the_yes() {
        let p = FakeB::new(true, temp_dir("b-gm-setup"));
        let mut a = app_on("b-gm-setup-app", Tab::Models);
        let mut f = setup_form(&mut a, &p);
        f.set_value(0, KEY);
        f.set_choice(4, 1);
        let next = a.gem_submit(&p, &mut f);
        assert!(matches!(next, Some(Modal::Form(FormKind::GeminiConsent, _))), "no yes yet: the consent card follows");
        assert!(p.secret_has_ref(gemini::KEY_REF), "the key is in the vault");
        let s = p.settings_get();
        assert!(s.chat.as_ref().is_some_and(|m| m.provider == gemini::PROVIDER_ID));
        let acts: Vec<String> = p.audited().into_iter().map(|(a, _)| a).collect();
        assert_eq!(acts, vec!["vault.system.set", "settings.gemini"]);
        assert!(!p.audited().iter().any(|(_, s)| s.contains(KEY)), "the key is never in the audit log");
    }

    #[test]
    fn a_short_key_a_locked_vault_and_a_person_who_is_not_an_administrator_change_nothing() {
        let p = FakeB::new(true, temp_dir("b-gm-bad"));
        let mut a = app_on("b-gm-bad-app", Tab::Models);
        let mut f = setup_form(&mut a, &p);
        f.set_value(0, "short");
        assert!(matches!(a.gem_submit(&p, &mut f), Some(Modal::Form(FormKind::GeminiSetup, g)) if g.error.as_deref().is_some_and(|e| e.contains("does not look like"))));
        f.set_value(0, KEY);
        p.vault_locked.store(true, Ordering::SeqCst);
        assert!(matches!(a.gem_submit(&p, &mut f), Some(Modal::Form(FormKind::GeminiSetup, g)) if g.error.as_deref().is_some_and(|e| e.contains("vault is locked"))));
        assert!(p.settings_get().providers.is_empty() && p.audited().is_empty());
        p.admin.store(false, Ordering::SeqCst);
        assert!(a.gem_submit(&p, &mut f).is_none());
        assert!(p.audited().iter().any(|(a, _)| a == "settings.denied"));
    }

    #[test]
    fn the_yes_is_a_person_s_card_and_taking_it_back_is_audited() {
        let p = FakeB::new(true, temp_dir("b-gm-yes"));
        let mut a = app_on("b-gm-yes-app", Tab::Models);
        assert_eq!(SettingsApp::gem_chip(&p).0, "nothing sent: no yes yet");
        let no = SettingsApp::gem_consent_form(&p);
        a.gem_consent_submit(&p, &no);
        assert!(matches!(gemini::consent_state(&p.dir, "amy"), ConsentState::Missing));
        let mut yes = SettingsApp::gem_consent_form(&p);
        yes.set_choice(0, 1);
        a.gem_consent_submit(&p, &yes);
        assert!(matches!(gemini::consent_state(&p.dir, "amy"), ConsentState::Given { .. }));
        assert_eq!(SettingsApp::gem_chip(&p).0, "sends to Google: you said yes");
        a.gem_revoke(&p);
        assert!(matches!(gemini::consent_state(&p.dir, "amy"), ConsentState::Missing));
        assert!(p.audited().iter().any(|(_, s)| s == "consent taken back"));
    }

    #[test]
    fn the_test_request_waits_for_the_yes_and_then_goes_through_the_port() {
        let p = FakeB::new(true, temp_dir("b-gm-test"));
        let mut a = app_on("b-gm-test-app", Tab::Models);
        a.gem_test(&p);
        assert!(matches!(a.modal, Some(Modal::Form(FormKind::GeminiConsent, _))), "no yes: the card, nothing sent");
        a.modal = None;
        gemini::grant(&p.dir, "amy").unwrap();
        *p.ping_answer.lock().unwrap() = Err("429 quota".into());
        a.gem_test(&p);
        assert!(a.is_busy(), "the request runs as a job");
        let cx = cx_with_admin_store("b-gm-test-cx");
        let mut n = 0;
        while a.is_busy() && n < 2_000_000 {
            a.handle(&Event::Tick, &cx);
            n += 1;
            std::thread::yield_now();
        }
        assert!(failed(&a).is_some_and(|m| m.contains("429")), "{:?}", a.status());
    }
}

mod gemini_voice {
    use super::*;
    use super::super::geminivoice_page::GvRow;
    use super::super::apps_port::SecretsPort;
    use iyakki_shared::gemini::{self, ConsentState};

    #[test]
    fn the_switch_and_a_step_are_saved_audited_and_for_administrators_only() {
        let p = FakeB::new(true, temp_dir("b-gv-switch"));
        let mut a = app_on("b-gv-switch-app", Tab::GeminiVoice);
        assert!(!gemini::load(&p.dir).voice.enabled);
        a.handle_geminivoice(&Key::Char('o'), &p);
        assert!(gemini::load(&p.dir).voice.enabled);
        a.handle_geminivoice(&Key::Char('k'), &p);
        assert!(p.audited().iter().all(|(a, _)| a == "settings.gemini_voice") && p.audited().len() == 2);
        p.admin.store(false, Ordering::SeqCst);
        a.handle_geminivoice(&Key::Char('o'), &p);
        assert!(gemini::load(&p.dir).voice.enabled, "unchanged");
        assert!(failed(&a).is_some_and(|m| m.contains("dministrator")), "{:?}", a.status());
    }

    #[test]
    fn the_microphone_card_starts_on_no_and_only_a_yes_is_kept() {
        let p = FakeB::new(false, temp_dir("b-gv-card"));
        let mut a = app_on("b-gv-card-app", Tab::GeminiVoice);
        a.handle_geminivoice(&Key::Char('C'), &p);
        assert!(a.has_modal());
        a.gv_consent_submit(&p, &SettingsApp::gv_consent_form(&p));
        assert!(matches!(gemini::live_consent_state(&p.dir, "amy"), ConsentState::Missing));
        let mut yes = SettingsApp::gv_consent_form(&p);
        yes.set_choice(0, 1);
        a.gv_consent_submit(&p, &yes);
        assert!(matches!(gemini::live_consent_state(&p.dir, "amy"), ConsentState::Given { .. }));
        assert!(p.audited().iter().any(|(_, s)| s.starts_with("consent given for the microphone")));
    }

    #[test]
    fn the_rows_say_whether_the_key_is_stored_and_the_page_draws_at_five_sizes() {
        let p = FakeB::new(true, temp_dir("b-gv-draw"));
        let a = app_on("b-gv-draw-app", Tab::GeminiVoice);
        assert!(a.gv_row_text(&p, GvRow::Switch).1.contains("Off"));
        p.secret_set("gemini-key", "x", true).unwrap();
        a.gv_row_text(&p, GvRow::Switch);
        let mut a = a;
        for (w, h) in SIZES {
            let (mut g, r) = blank(w, h);
            a.render_geminivoice(&mut g, r, 1.0, &p);
        }
    }
}

mod model_options {
    use super::*;
    use iyakki_agent::modelopts;
    use iyakki_shared::providers::{Kind, ProviderCfg};
    use iyakki_shared::settings::ModelChoice;
    use super::super::mo_page::{MoForm, MoList};

    fn with_provider(tag: &str, admin: bool) -> (FakeB, ProviderCfg) {
        let p = FakeB::new(admin, temp_dir(tag));
        let mut prov = ProviderCfg::new("lan", Kind::Compat);
        prov.base_url = Some("http://127.0.0.1:9/v1".into());
        prov.default_model = Some("big-70b".into());
        p.settings_update(&mut |s| {
            s.providers = vec![prov.clone()];
            s.chat = Some(ModelChoice { provider: "lan".into(), model: "big-70b".into() });
        })
        .unwrap();
        (p, prov)
    }

    #[test]
    fn the_models_list_opens_a_chooser_and_a_failure_or_an_empty_list_is_told() {
        let (p, prov) = with_provider("b-mo-list", true);
        let mut a = app_on("b-mo-list-app", Tab::Models);
        a.mo_list_done(&p, MoList { provider: "lan".into(), embed: false, result: p.model_list.lock().unwrap().clone() });
        assert!(matches!(a.modal, Some(Modal::Pick(PickKind::Chat(_), _))));
        a.modal = None;
        a.mo_list_done(&p, MoList { provider: "lan".into(), embed: true, result: Ok(vec!["e5".into()]) });
        assert!(matches!(a.modal, Some(Modal::Pick(PickKind::Embed(_), _))));
        a.modal = None;
        a.mo_list_done(&p, MoList { provider: "lan".into(), embed: false, result: Ok(vec![]) });
        assert!(failed(&a).is_some_and(|m| m.contains("listed no models")));
        a.mo_list_done(&p, MoList { provider: "lan".into(), embed: false, result: Err("401".into()) });
        assert_eq!(failed(&a).as_deref(), Some("lan: 401"));
        a.mo_list(&p, &prov, false);
        assert!(a.is_busy(), "the list is asked for as a job through the port's fetcher");
    }

    #[test]
    fn an_option_is_checked_saved_in_the_data_folder_and_audited_and_a_user_is_refused() {
        let (p, _prov) = with_provider("b-mo-save", true);
        let mut a = app_on("b-mo-save-app", Tab::Models);
        let f = MoForm::Field("lan".into(), "big-70b".into(), "temperature".into());
        let ok = a.mo_submit(&p, &f, &Form::new("t").text("temperature", "0.5", "")).unwrap();
        assert!(ok.contains("temperature"));
        assert_eq!(modelopts::get(&p.dir, "lan", "big-70b").show("temperature").split(' ').next(), Some("0.5"));
        assert_eq!(p.audited(), vec![("settings.model_options".to_string(), "lan/big-70b temperature".to_string())]);
        assert!(a.mo_submit(&p, &f, &Form::new("t").text("temperature", "9", "")).is_err(), "out of range");
        p.admin.store(false, Ordering::SeqCst);
        assert!(a.mo_submit(&p, &f, &Form::new("t").text("temperature", "0.1", "")).is_err());
        assert!(p.audited().iter().any(|(a, _)| a == "settings.denied"));
        assert_eq!(modelopts::get(&p.dir, "lan", "big-70b").show("temperature").split(' ').next(), Some("0.5"));
    }

    #[test]
    fn clearing_the_options_is_audited_and_the_rows_draw_at_five_sizes() {
        let (p, prov) = with_provider("b-mo-clear", true);
        let mut a = app_on("b-mo-clear-app", Tab::Models);
        let f = MoForm::Field("lan".into(), "big-70b".into(), "top_p".into());
        a.mo_submit(&p, &f, &Form::new("t").text("top_p", "0.9", "")).unwrap();
        assert_eq!(SettingsApp::mo_rows(&p, &prov, "big-70b").len() > 1, true);
        a.mo_pick(&p, "lan", "Clear all options for this model");
        assert!(said(&a).is_some_and(|m| m.contains("cleared")));
        assert!(!modelopts::get(&p.dir, "lan", "big-70b").has("top_p"));
        for (w, h) in SIZES {
            let (mut g, r) = blank(w, h);
            a.mo_render(&mut g, r, 1.0, &p, &prov, true);
        }
    }
}

mod model_router {
    use super::*;
    use super::super::router_page::RouterAct;
    use iyakki_shared::router_registry::{Caps, Cost, Profile, Role, Zone};

    fn profile(id: &str) -> Profile {
        let (provider, model) = id.split_once('/').unwrap();
        Profile { id: id.into(), provider: provider.into(), model: model.into(), role: Role::Executor, caps: Caps { tools: true, vision: false, audio: false, context: 8192 }, cost: Cost::Low, zone: Zone::Computer, local: true, enabled: true }
    }

    fn rig(tag: &str, admin: bool) -> (FakeB, SettingsApp) {
        let p = FakeB::new(admin, temp_dir(tag));
        *p.router.profiles.lock().unwrap() = vec![profile("lan/big-70b"), profile("lan/small-7b")];
        (p, app_on(&format!("{tag}-app"), Tab::ModelRouter))
    }

    #[test]
    fn a_change_asks_first_and_then_does_what_the_chat_tool_does() {
        let (p, mut a) = rig("b-rt-ask", true);
        assert_eq!(a.router_len(&p), 2);
        a.handle_router(&Key::Char('r'), &p);
        let Some(Modal::Confirm(ConfirmKind::Router(act), text)) = a.modal.take() else { panic!("a card asks first") };
        assert!(text.contains("lan/big-70b") && p.router.asked.lock().unwrap().is_empty(), "nothing changes before the yes");
        a.router_confirm(&p, act);
        let asked = p.router.asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].0, "set");
        assert_eq!(asked[0].1["model"]["id"], "lan/big-70b");
        assert_eq!(said(&a).as_deref(), Some("set done"));
    }

    #[test]
    fn a_refusal_of_the_core_is_shown_and_a_user_changes_nothing() {
        let (p, mut a) = rig("b-rt-refuse", true);
        *p.router.refusal.lock().unwrap() = Some("The core said no.".into());
        a.router_confirm(&p, RouterAct::Strategy("fast".into()));
        assert_eq!(failed(&a).as_deref(), Some("The core said no."));
        p.admin.store(false, Ordering::SeqCst);
        a.router_confirm(&p, RouterAct::Remove("fast".into()));
        a.handle_router(&Key::Char('o'), &p);
        assert!(p.router.asked.lock().unwrap().is_empty());
        assert!(!a.has_modal());
        assert!(p.audited().iter().any(|(a, _)| a == "settings.denied"));
        a.handle_router(&Key::Char('k'), &p);
        assert_eq!(p.router.asked.lock().unwrap()[0].0, "cancel", "stopping a run is free for everyone");
    }

    #[test]
    fn the_budget_is_checked_and_an_unchanged_one_says_so() {
        let (p, mut a) = rig("b-rt-budget", true);
        let b = p.router.config_for_test();
        let form = |t: &str, c: &str, at: &str, e: &str| Form::new("b").text("Tokens", t, "").text("Calls", c, "").text("Tries", at, "").text("Rounds", e, "");
        assert!(a.router_budget_submit(&p, &form("x", "1", "1", "1")).is_err());
        assert_eq!(a.router_budget_submit(&p, &form(&b.0, &b.1, &b.2, &b.3)).unwrap_err(), "Nothing changed.");
        assert!(a.router_budget_submit(&p, &form("1000", "3", "2", "1")).is_ok());
        assert!(matches!(a.modal, Some(Modal::Confirm(ConfirmKind::Router(RouterAct::Set(_)), _))));
    }

    #[test]
    fn a_script_is_read_only_from_inside_the_home_folder_and_shown_in_full_first() {
        let (p, mut a) = rig("b-rt-script", true);
        std::fs::create_dir_all(p.dir.join("home")).unwrap();
        std::fs::write(p.dir.join("home/s.js"), "function decide(i) { return null; }").unwrap();
        let f = |n: &str, path: &str| Form::new("s").text("Name", n, "").text("File", path, "");
        assert!(a.router_script_submit(&p, &f("Bad Name", "s.js")).is_err());
        assert!(a.router_script_submit(&p, &f("default", "s.js")).is_err());
        assert!(a.router_script_submit(&p, &f("mine", "../s.js")).unwrap_err().contains("inside your own folder"));
        assert!(a.router_script_submit(&p, &f("mine", "nope.js")).unwrap_err().contains("not there"));
        a.router_script_submit(&p, &f("mine", "s.js")).unwrap();
        assert!(a.router.review.as_ref().is_some_and(|(n, src)| n == "mine" && src.contains("decide")));
        a.handle_router(&Key::Char('y'), &p);
        let Some(Modal::Confirm(ConfirmKind::Router(act), _)) = a.modal.take() else { panic!("adding asks first") };
        a.router_confirm(&p, act);
        assert_eq!(p.router.asked.lock().unwrap()[0].0, "strategy_add");
    }

    #[test]
    fn the_page_draws_with_and_without_models_and_with_a_run_going_at_five_sizes() {
        let (p, mut a) = rig("b-rt-draw", true);
        for (w, h) in SIZES {
            let (mut g, r) = blank(w, h);
            a.render_router(&mut g, r, 1.0, &p);
        }
        *p.router.progress.lock().unwrap() = Some("step 2 of 5".into());
        p.router.profiles.lock().unwrap().clear();
        for (w, h) in SIZES {
            let (mut g, r) = blank(w, h);
            a.render_router(&mut g, r, 1.0, &p);
        }
        a.handle_router(&Key::Char('r'), &p);
        assert!(failed(&a).is_some_and(|m| m.contains("no models yet")));
    }
}

mod assistant {
    use super::*;
    use super::super::assistant_page::Row;
    use iyakki_memx::history::History;

    fn remember(p: &FakeB, ws: &str, chat: &str, n: usize) {
        let h = History::new(p.kb.clone(), &p.user, ws, Some(chat));
        for i in 0..n {
            h.record("turn", "", &format!("{ws} {chat} step {i}: the registry address is 10.0.0.{i}"));
        }
    }

    #[test]
    fn the_history_switch_is_saved_and_audited_and_busy_settings_are_told() {
        let p = FakeB::new(false, temp_dir("b-as-switch"));
        let mut a = app_on("b-as-switch-app", Tab::Assistant);
        assert!(!p.settings_get().no_workspace_history);
        a.handle_assistant(&Key::Char('h'), &p);
        assert!(p.settings_get().no_workspace_history);
        assert!(said(&a).is_some_and(|m| m.contains("no longer keeps")));
        a.handle_assistant(&Key::Enter, &p);
        assert!(!p.settings_get().no_workspace_history);
        assert_eq!(p.audited().iter().map(|(a, s)| format!("{a}:{s}")).collect::<Vec<_>>(), ["settings.assistant.history:off", "settings.assistant.history:on"]);
        p.busy.store(true, Ordering::SeqCst);
        a.handle_assistant(&Key::Char('h'), &p);
        assert!(failed(&a).is_some_and(|m| m.contains("busy")));
        assert_eq!(p.audited().len(), 2);
    }

    #[test]
    fn forgetting_needs_the_same_key_twice_touches_only_that_and_is_audited() {
        let p = FakeB::new(false, temp_dir("b-as-forget"));
        remember(&p, "web-shop", "s_one", 3);
        remember(&p, "data-lab", "s_one", 4);
        let mut a = app_on("b-as-forget-app", Tab::Assistant);
        a.handle_assistant(&Key::Down, &p); // data-lab
        a.handle_assistant(&Key::Char('f'), &p);
        assert!(said_or_failed(&a).contains("Press F again"));
        a.handle_assistant(&Key::Char('f'), &p);
        assert!(said_or_failed(&a).contains("Forgot 4 entries"));
        let left: Vec<String> = History::workspaces_of(&*p.kb, "amy").into_iter().map(|w| w.name).collect();
        assert_eq!(left, vec!["web-shop"]);
        assert!(p.audited().iter().any(|(a, s)| a == "settings.assistant.forget" && s.contains("workspace data-lab")));
    }

    fn said_or_failed(a: &SettingsApp) -> String {
        a.status().map(|s| s.0.clone()).unwrap_or_default()
    }

    #[test]
    fn the_limits_step_for_an_administrator_and_a_plain_account_is_refused() {
        let p = FakeB::new(true, temp_dir("b-as-limits"));
        let mut a = app_on("b-as-limits-app", Tab::Assistant);
        let rows = a.asst_rows(&p);
        let at = rows.iter().position(|r| matches!(r, Row::Limits(None))).unwrap();
        a.sel[a.tab] = at;
        a.handle_assistant(&Key::Char('t'), &p);
        let after = p.settings_get().think_limit_secs;
        assert!(after > 0, "one step up from none");
        assert!(p.audited().iter().any(|(a, _)| a == "settings.assistant.limits"));
        p.admin.store(false, Ordering::SeqCst);
        a.handle_assistant(&Key::Char('t'), &p);
        assert_eq!(p.settings_get().think_limit_secs, after);
        assert!(failed(&a).is_some_and(|m| m.contains("Only an administrator")));
        a.handle_assistant(&Key::Char('0'), &p);
        assert_eq!(p.settings_get().think_limit_secs, after);
    }

    #[test]
    fn the_waiting_rows_step_the_ledger_settings_and_audit_them() {
        let p = FakeB::new(false, temp_dir("b-as-wait"));
        let mut a = app_on("b-as-wait-app", Tab::Assistant);
        let rows = a.asst_rows(&p);
        let at = rows.iter().position(|r| matches!(r, Row::Waiting("max_wakes_per_day"))).expect("a row for the wakes a day");
        a.sel[a.tab] = at;
        let before = p.ledger.settings.lock().unwrap().max_wakes_per_day;
        a.handle_assistant(&Key::Char('+'), &p);
        assert_eq!(p.ledger.settings.lock().unwrap().max_wakes_per_day, before + 5);
        assert!(p.audited().iter().any(|(a, s)| a == "pending.settings" && s.starts_with("max_wakes_per_day")));
        let at = rows.iter().position(|r| matches!(r, Row::Waiting("auto_continue"))).unwrap();
        a.sel[a.tab] = at;
        let was = p.ledger.settings.lock().unwrap().auto_continue;
        a.handle_assistant(&Key::Enter, &p);
        assert_eq!(p.ledger.settings.lock().unwrap().auto_continue, !was);
    }

    #[test]
    fn the_page_draws_with_a_history_and_a_provider_at_five_sizes() {
        let p = FakeB::new(true, temp_dir("b-as-draw"));
        remember(&p, "web-shop", "s_one", 2);
        let mut lan = iyakki_shared::providers::ProviderCfg::new("lan", iyakki_shared::providers::Kind::Compat);
        lan.base_url = Some("http://127.0.0.1:9/v1".into());
        p.settings_update(&mut |s| s.providers = vec![lan.clone()]).unwrap();
        let mut a = app_on("b-as-draw-app", Tab::Assistant);
        assert!(a.asst_rows(&p).iter().any(|r| matches!(r, Row::Limits(Some(id)) if id == "lan")));
        for (w, h) in SIZES {
            let (mut g, r) = blank(w, h);
            a.render_assistant(&mut g, r, 1.0, &p);
        }
    }
}

/// Run the jobs a page started to the end (they are threads: the page picks their answers up on a tick). The page's own application of the
/// answer uses a throwaway shell context here; the tests that care about the answer call the page's `..._job_done` with the port instead.
fn finish_jobs(a: &mut SettingsApp) {
    let cx = cx_with_admin_store("b-finish-jobs");
    let mut spins = 0u32;
    while a.is_busy() && spins < 5_000_000 {
        a.handle(&Event::Tick, &cx);
        spins += 1;
        std::thread::yield_now();
    }
    assert!(!a.is_busy(), "the job did not finish");
}

mod model_host {
    use super::*;
    use super::super::modelhost_page::MhOut;
    use iyakki_shared::modelhost::{Host, Hosts, Preset};

    fn host(id: &str, in_cluster: bool) -> Host {
        Host { id: format!("mac-{id}"), nickname: id.into(), preset: Preset::Mac, address: "127.0.0.1".into(), port: 9, in_cluster, public_ok: false }
    }

    fn opened(tag: &str, admin: bool) -> (FakeB, SettingsApp) {
        let p = FakeB::new(admin, temp_dir(tag));
        Hosts::new(&p.dir).upsert(host("studio", false)).unwrap();
        let mut a = app_on(&format!("{tag}-app"), Tab::Models);
        a.mh_open(&p, true);
        (p, a)
    }

    #[test]
    fn only_administrators_open_the_page_and_it_lists_the_hosts_of_the_data_folder() {
        let (_p, a) = opened("b-mh-user", false);
        assert!(!a.mh_is_open());
        assert!(failed(&a).is_some_and(|m| m.contains("Only an administrator")));
        let (p, mut a) = opened("b-mh-admin", true);
        assert!(a.mh_is_open());
        for (w, h) in SIZES {
            let (mut g, r) = blank(w, h);
            a.mh_render(&mut g, r, 1.0, &p, 0);
            assert!(g.hits.iter().any(|x| x.label == "Close"), "{w}x{h}");
        }
        assert!(a.mh_event(&Event::Key(Key::Esc), &p) && !a.mh_is_open());
    }

    #[test]
    fn a_bad_form_and_an_empty_address_are_told_and_nothing_is_audited_or_started() {
        let (p, mut a) = opened("b-mh-form", true);
        a.mh_event(&Event::Key(Key::Char('t')), &p);
        assert!(failed(&a).is_some(), "no address yet");
        assert!(!a.is_busy() && p.audited().is_empty());
        let mut f = Form::new("h").text("Nickname", "!!!", "").text("Address", "", "").secret("Key", "").text("Outside", "", "");
        assert!(a.mh_submit(&mut f).is_err());
        let mut f = Form::new("h").text("Nickname", "studio", "").text("Address", "127.0.0.1", "").secret("Key", "").text("Outside", "", "");
        assert!(a.mh_submit(&mut f).unwrap().contains("Press Test"));
    }

    #[test]
    fn the_test_and_the_scan_are_audited_by_name_and_run_as_jobs_and_a_user_is_refused() {
        let (p, mut a) = opened("b-mh-scan", true);
        a.mh.scan_override = Some((vec![], Default::default()));
        a.mh_event(&Event::Key(Key::Char('s')), &p);
        assert!(a.is_busy());
        assert_eq!(p.audited(), vec![("settings.modelhost.scan".to_string(), "local /24".to_string())]);
        finish_jobs(&mut a);
        a.mh_job_done(&p, MhOut::Scan(Ok(vec![])));
        assert!(said(&a).is_some_and(|m| m.contains("No model server answered")));
        p.admin.store(false, Ordering::SeqCst);
        a.mh_event(&Event::Key(Key::Char('s')), &p);
        a.mh_event(&Event::Key(Key::Char('t')), &p);
        assert_eq!(p.audited().iter().filter(|(a, _)| a == "settings.modelhost.scan").count(), 1);
        assert!(p.audited().iter().any(|(a, _)| a == "settings.denied"));
    }

    #[test]
    fn publishing_goes_to_the_clusters_kubectl_and_removing_to_the_ops_and_both_are_audited() {
        let (p, mut a) = opened("b-mh-cluster", true);
        a.mh_event(&Event::Tab(7100 + 1), &p); // the first host, publish
        assert!(a.is_busy());
        finish_jobs(&mut a);
        a.mh_job_done(&p, MhOut::Msg(Ok("done".into())));
        assert!(p.mhost.kubectl.lock().unwrap().iter().any(|c| c.iter().any(|w| w == "apply" || w == "create")), "{:?}", p.mhost.kubectl.lock().unwrap());
        a.mh_event(&Event::Tab(7100 + 4), &p); // remove
        finish_jobs(&mut a);
        a.mh_job_done(&p, MhOut::Msg(Ok("done".into())));
        assert_eq!(*p.mhost.removed.lock().unwrap(), vec!["mac-studio".to_string()]);
        let acts: Vec<String> = p.audited().into_iter().map(|(_, s)| s).collect();
        assert_eq!(acts, vec!["mac-studio publish", "mac-studio remove"]);
        a.mh_job_done(&p, MhOut::Msg(Err("The cluster is not there.".into())));
        assert_eq!(failed(&a).as_deref(), Some("The cluster is not there."));
    }

    #[test]
    fn a_test_of_a_saved_host_reads_its_key_through_the_port_and_a_refused_remove_is_shown() {
        let (p, mut a) = opened("b-mh-key", true);
        p.mhost.keys.lock().unwrap().insert("mac-studio".into(), "sekret".into());
        a.mh_event(&Event::Tab(7100), &p); // test
        assert!(a.is_busy());
        finish_jobs(&mut a);
        *p.mhost.remove_refusal.lock().unwrap() = Some("The settings are read only.".into());
        a.mh_job_done(&p, MhOut::Msg(Err("The settings are read only.".into())));
        assert_eq!(failed(&a).as_deref(), Some("The settings are read only."));
    }
}

mod agents {
    use super::*;
    use super::super::apps_port::AgentsPort;
    use iyakki_extagentx::{Auth, Mode, Status};

    fn ready(version: &str) -> Status {
        Status { installed: true, version: version.into(), auth: Auth::SignedIn("amy".into()), mode: Mode::Bridged, chip: "ready".into(), fix: String::new() }
    }

    fn rig(tag: &str, admin: bool) -> (FakeB, SettingsApp) {
        let p = FakeB::new(admin, temp_dir(tag));
        p.agents.status.lock().unwrap().insert(iyakki_extagentx::CLAUDE_CODE.into(), ready("2.1.0"));
        (p, app_on(&format!("{tag}-app"), Tab::Agents))
    }

    #[test]
    fn the_list_has_both_built_in_agents_and_a_custom_one_is_saved_audited_and_removed_after_a_question() {
        let (p, mut a) = rig("b-ag-custom", true);
        assert_eq!(SettingsApp::agent_ids(&p).len(), 2);
        let f = Form::new("c").text("Id", "aider", "").text("Name", "Aider", "").text("Command", "aider --message {prompt}", "").text("Key", "", "").text("Models", "", "");
        let r = a.agents_submit(&p, &FormKind::AgentCustom(None), &f).unwrap();
        assert!(r.unwrap().contains("aider"));
        assert_eq!(SettingsApp::agent_ids(&p).len(), 3);
        a.sel[a.tab] = 2;
        a.handle_agents(&Key::Char('R'), &p);
        let Some(Modal::Confirm(kind, _)) = a.modal.take() else { panic!("removing asks first") };
        assert!(a.agents_confirm(&p, &kind));
        assert_eq!(SettingsApp::agent_ids(&p).len(), 2);
        assert!(p.audited().iter().any(|(a, s)| a == "settings.agents" && s.contains("Removed `aider`")));
    }

    #[test]
    fn a_choice_is_saved_audited_and_a_user_is_refused_but_may_sign_in() {
        let (p, mut a) = rig("b-ag-choice", true);
        a.handle_agents(&Key::Char('u'), &p);
        assert_eq!(p.agents_cfg().default.as_deref(), Some(iyakki_extagentx::CLAUDE_CODE));
        a.handle_agents(&Key::Char('x'), &p);
        assert!(!p.agents_cfg().claude.enabled);
        a.handle_agents(&Key::Char('U'), &p);
        assert_eq!(p.agents_cfg().default, None);
        assert_eq!(p.audited().len(), 3);
        p.admin.store(false, Ordering::SeqCst);
        a.handle_agents(&Key::Char('u'), &p);
        assert_eq!(p.agents_cfg().default, None);
        assert!(p.audited().iter().any(|(a, _)| a == "settings.denied"));
        a.handle_agents(&Key::Char('l'), &p);
        assert_eq!(crate::agent_login::take_request(), Some((iyakki_extagentx::CLAUDE_CODE, false)), "a plain person may sign in: the dialog is asked for");
    }

    #[test]
    fn auto_needs_a_question_and_the_model_is_checked_and_remembered() {
        let (p, mut a) = rig("b-ag-auto", true);
        a.handle_agents(&Key::Char('a'), &p);
        let Some(Modal::Confirm(kind, _)) = a.modal.take() else { panic!("Auto asks first") };
        assert!(!p.agents_cfg().claude.allow_auto);
        assert!(a.agents_confirm(&p, &kind));
        assert!(p.agents_cfg().claude.allow_auto);
        let id = iyakki_extagentx::CLAUDE_CODE.to_string();
        let bad = a.agents_submit(&p, &FormKind::AgentModel(id.clone()), &Form::new("m").text("Model id", "bad model; rm", "")).unwrap();
        assert!(bad.unwrap_err().contains("letters, digits"));
        let ok = a.agents_submit(&p, &FormKind::AgentModel(id), &Form::new("m").text("Model id", "claude-sonnet-5", "")).unwrap();
        assert!(ok.unwrap().contains("remembered"));
        assert_eq!(p.agents_cfg().claude.model, "claude-sonnet-5");
        assert!(p.agents_cfg().claude.custom_models.contains(&"claude-sonnet-5".to_string()));
    }

    #[test]
    fn detect_asks_the_status_cache_and_install_shows_the_plan_and_runs_only_after_the_yes() {
        let (p, mut a) = rig("b-ag-install", true);
        a.handle_agents(&Key::Char('d'), &p);
        assert_eq!(*p.agents.refreshed.lock().unwrap(), 1);
        a.handle_agents(&Key::Char('i'), &p);
        assert!(said(&a).is_some_and(|m| m.contains("already installed")));
        p.agents.status.lock().unwrap().clear();
        *p.agents.install_plan.lock().unwrap() = Some("npm install -g the-agent".into());
        a.handle_agents(&Key::Char('i'), &p);
        let Some(Modal::Confirm(kind, text)) = a.modal.take() else { panic!("install asks first") };
        assert!(text.contains("npm install -g the-agent") && p.agents.installed.lock().unwrap().is_empty());
        assert!(a.agents_confirm(&p, &kind));
        assert_eq!(*p.agents.installed.lock().unwrap(), vec!["claude-code".to_string()]);
        assert!(p.audited().iter().any(|(a, s)| a == "cli.install" && s == "claude-code"));
        *p.agents.install_refusal.lock().unwrap() = Some("no network".into());
        a.agents_confirm(&p, &kind);
        assert_eq!(failed(&a).as_deref(), Some("no network"));
    }

    #[test]
    fn the_test_runs_on_a_worker_through_the_port_and_only_after_the_yes() {
        let (p, mut a) = rig("b-ag-test", true);
        a.handle_agents(&Key::Char('t'), &p);
        let Some(Modal::Confirm(kind, text)) = a.modal.take() else { panic!("the test asks first") };
        assert!(text.contains("ONE tiny request") && p.agents.tests.lock().unwrap().is_empty());
        assert!(a.agents_confirm(&p, &kind));
        assert!(a.is_busy());
        super::finish_jobs(&mut a);
        assert_eq!(p.agents.tests.lock().unwrap().len(), 1);
        assert!(p.audited().iter().any(|(a, s)| a == "agents.test" && s == iyakki_extagentx::CLAUDE_CODE));
    }

    #[test]
    fn the_page_draws_in_every_state_at_five_sizes() {
        let (p, mut a) = rig("b-ag-draw", true);
        p.agents.status.lock().unwrap().insert(iyakki_extagentx::COPILOT.into(), Status { installed: true, version: String::new(), auth: Auth::SignedOut, mode: Mode::Advisor, chip: "sign in".into(), fix: "Run gh auth login.".into() });
        for (w, h) in SIZES {
            let (mut g, r) = blank(w, h);
            a.render_agents(&mut g, r, 1.0, &p);
            a.sel[a.tab] = 1;
            let (mut g, r) = blank(w, h);
            a.render_agents(&mut g, r, 1.0, &p);
            a.sel[a.tab] = 0;
        }
        assert_eq!(SettingsApp::agent_ids(&p).len(), 2);
    }
}

mod rag {
    use super::*;
    use super::super::apps_port::RagPort;
    use iyakki_shared::rag_ext::ExternalRag;
    use iyakki_shared::settings::ExtEntry;

    fn backend(id: &str) -> ExtEntry {
        ExtEntry { enabled: true, backend: ExternalRag::Http { id: id.into(), url: "http://rag.corp/search".into(), key_ref: None } }
    }

    #[test]
    fn the_sliders_step_through_the_settings_port_and_each_change_is_audited() {
        let p = FakeB::new(true, temp_dir("b-rag-steps"));
        let mut a = app_on("b-rag-steps-app", Tab::Rag);
        let top = p.settings_get().rag.top_k;
        a.handle_rag(&Key::Char(']'), &p);
        assert_eq!(p.settings_get().rag.top_k, top + 1);
        a.handle_rag(&Key::Char('['), &p);
        assert_eq!(p.settings_get().rag.top_k, top);
        let chunk = p.settings_get().rag.chunk_max;
        a.handle_rag(&Key::Char('='), &p);
        assert_eq!(p.settings_get().rag.chunk_max, (chunk + 100).min(4000));
        assert_eq!(p.audited().len(), 3);
        assert!(p.audited().iter().all(|(a, _)| a == "settings.rag"));
        p.busy.store(true, Ordering::SeqCst);
        a.handle_rag(&Key::Char(']'), &p);
        assert!(failed(&a).is_some_and(|m| m.contains("busy")));
        assert_eq!(p.audited().len(), 3, "a change that did not happen is not audited");
    }

    #[test]
    fn a_person_who_is_not_an_administrator_changes_nothing_but_may_reindex_their_own() {
        let p = FakeB::new(false, temp_dir("b-rag-user"));
        let mut a = app_on("b-rag-user-app", Tab::Rag);
        let top = p.settings_get().rag.top_k;
        a.handle_rag(&Key::Char(']'), &p);
        a.handle_rag(&Key::Char('u'), &p);
        assert_eq!(p.settings_get().rag.top_k, top);
        assert!(p.audited().iter().filter(|(a, _)| a == "settings.denied").count() == 2);
        assert_eq!(SettingsApp::own_namespaces(&p), vec!["user/amy".to_string()]);
        a.handle_rag(&Key::Char('r'), &p);
        assert!(a.is_busy(), "re-embedding runs as a job");
        super::finish_jobs(&mut a);
    }

    #[test]
    fn an_external_backend_is_listed_switched_and_tested_on_a_worker_through_the_port() {
        let p = FakeB::new(true, temp_dir("b-rag-ext"));
        p.settings_update(&mut |s| s.put_external(backend("corp"))).unwrap();
        let mut a = app_on("b-rag-ext-app", Tab::Rag);
        a.handle_rag(&Key::Char('x'), &p);
        assert!(!p.settings_get().external_rag[0].enabled);
        a.handle_rag(&Key::Char('d'), &p);
        assert!(matches!(a.modal, Some(Modal::Confirm(ConfirmKind::External(_), _))), "removing asks first");
        a.modal = None;
        *p.backend_answer.lock().unwrap() = Err("401 unauthorised".into());
        a.handle_rag(&Key::Char('t'), &p);
        assert!(a.is_busy());
        super::finish_jobs(&mut a);
        assert_eq!(*p.backend_tests.lock().unwrap(), vec!["corp".to_string()]);
    }

    #[test]
    fn indexing_goes_to_the_persons_own_folder_or_the_shared_one_for_an_administrator() {
        let p = FakeB::new(false, temp_dir("b-rag-ingest"));
        std::fs::create_dir_all(p.dir.join("docs/amy")).unwrap();
        std::fs::write(p.dir.join("docs/amy/a.md"), "The registry address is 10.0.0.9.").unwrap();
        let mut a = app_on("b-rag-ingest-app", Tab::Rag);
        a.handle_rag(&Key::Char('i'), &p);
        assert!(a.is_busy());
        super::finish_jobs(&mut a);
        let kb = p.kb();
        assert!(kb.namespaces().iter().any(|n| n == "user/amy"), "{:?}", kb.namespaces());
    }

    #[test]
    fn the_page_draws_for_both_kinds_of_person_with_a_backend_and_a_background_reindex_at_five_sizes() {
        let p = FakeB::new(true, temp_dir("b-rag-draw"));
        p.settings_update(&mut |s| s.put_external(backend("corp"))).unwrap();
        let mut a = app_on("b-rag-draw-app", Tab::Rag);
        for admin in [true, false] {
            p.admin.store(admin, Ordering::SeqCst);
            for reindexing in [false, true] {
                p.reindexing.store(reindexing, Ordering::SeqCst);
                for (w, h) in SIZES {
                    let (mut g, r) = blank(w, h);
                    a.render_rag(&mut g, r, 1.0, &p);
                }
            }
        }
    }
}

mod models_and_lemonade {
    use super::*;
    use super::super::apps_port::{ModelsPort, SecretsPort};
    use super::super::lemonade_page::{LemAct, LemPick};
    use iyakki_shared::lemonade::{Model, ModelType};
    use iyakki_shared::providers::{Kind, ProviderCfg};

    fn model(id: &str, kind: ModelType, downloaded: bool) -> Model {
        Model { id: id.into(), kind, recipe: "llamacpp".into(), checkpoint: format!("org/{id}"), labels: vec![], downloaded, loaded: false, suggested: false, size_gb: Some(2.0), image_defaults: None }
    }

    fn rig(tag: &str, admin: bool) -> (FakeB, SettingsApp) {
        let p = FakeB::new(admin, temp_dir(tag));
        let mut lem = ProviderCfg::new("lemon", Kind::Lemonade);
        lem.base_url = Some("http://192.168.1.20:13305".into());
        let mut local = ProviderCfg::new("local", Kind::Compat);
        local.base_url = Some("http://127.0.0.1:9/v1".into());
        local.models = vec!["small-7b".into()];
        p.settings_update(&mut |s| s.providers = vec![lem.clone(), local.clone()]).unwrap();
        p.lem.models.lock().unwrap().insert("lemon".into(), vec![model("qwen-chat", ModelType::Chat, true), model("e5", ModelType::Embeddings, true), model("whisper", ModelType::SpeechToText, true), model("big", ModelType::Chat, false)]);
        (p, app_on(&format!("{tag}-app"), Tab::Models))
    }

    fn pick(a: &mut SettingsApp, p: &dyn ModelsPort, action: &str) {
        a.lem_pick(p, LemPick::Action("lemon".into(), "qwen-chat".into()), action);
    }

    #[test]
    fn choosing_a_lemonade_model_for_chat_is_saved_audited_and_a_user_is_told_why_not() {
        let (p, mut a) = rig("b-lm-chat", true);
        pick(&mut a, &p, "Use as chat model");
        let s = p.settings_get();
        assert_eq!(s.chat.as_ref().map(|c| (c.provider.as_str(), c.model.as_str())), Some(("lemon", "qwen-chat")));
        assert_eq!(s.providers[0].default_model.as_deref(), Some("qwen-chat"));
        assert_eq!(p.audited(), vec![("settings.chat_model".to_string(), "lemon/qwen-chat".to_string())]);
        p.admin.store(false, Ordering::SeqCst);
        a.lem_pick(&p, LemPick::Model("lemon".into()), "embeddings · e5");
        assert!(failed(&a).is_some_and(|m| m.contains("Only an administrator")));
    }

    #[test]
    fn the_embedding_choice_starts_the_background_reindex_and_a_busy_settings_file_is_told() {
        let (p, mut a) = rig("b-lm-embed", true);
        a.lem_pick(&p, LemPick::Action("lemon".into(), "e5".into()), "Use for embeddings (RAG)");
        assert_eq!(p.settings_get().embedding.map(|c| c.model), Some("e5".to_string()));
        assert!(p.lem.reindex_started.load(Ordering::SeqCst));
        assert!(said(&a).is_some_and(|m| m.contains("re-embedded in the background")));
        p.busy.store(true, Ordering::SeqCst);
        a.lem_pick(&p, LemPick::Action("lemon".into(), "e5".into()), "Use for embeddings (RAG)");
        assert!(failed(&a).is_some_and(|m| m.contains("busy")));
    }

    #[test]
    fn a_server_on_another_machine_asks_before_any_speech_goes_there_and_the_yes_is_audited() {
        let (p, mut a) = rig("b-lm-stt", true);
        pick_model(&mut a, &p, "whisper", "Use as speech-to-text");
        assert!(matches!(a.modal, Some(Modal::Confirm(ConfirmKind::Lemonade(LemAct::RemoteStt(..)), _))), "a card first");
        assert!(p.lem.called().is_empty(), "nothing is sent before the yes");
        a.modal = None;
        a.lem_confirm(&p, LemAct::RemoteStt("lemon".into(), "whisper".into()));
        assert_eq!(p.lem.called(), vec!["allow http://192.168.1.20:13305", "stt lemon/whisper"]);
        assert!(p.audited().iter().any(|(a, _)| a == "settings.voice.remote_ok"));
        a.modal = None;
        pick_model(&mut a, &p, "whisper", "Use as speech-to-text");
        assert!(a.modal.is_none(), "once allowed, the server needs no second card");
        assert!(p.lem.called().iter().any(|c| c == "stt lemon/whisper"));
    }

    fn pick_model(a: &mut SettingsApp, p: &dyn ModelsPort, model: &str, action: &str) {
        a.lem_pick(p, LemPick::Action("lemon".into(), model.into()), action);
    }

    #[test]
    fn pull_load_and_unload_ask_first_audit_and_show_a_refusal() {
        let (p, mut a) = rig("b-lm-pull", true);
        pick_model(&mut a, &p, "big", "Pull (download 2.0 GB)…");
        assert!(matches!(a.modal.take(), Some(Modal::Confirm(ConfirmKind::Lemonade(LemAct::Pull(..)), _))));
        a.lem_confirm(&p, LemAct::Pull("lemon".into(), "big".into()));
        assert_eq!(p.lem.called(), vec!["pull amy lemon/big"]);
        *p.lem.refusal.lock().unwrap() = Some("The server is not reachable.".into());
        a.lem_confirm(&p, LemAct::Pull("lemon".into(), "big".into()));
        assert_eq!(failed(&a).as_deref(), Some("The server is not reachable."));
        a.lem_confirm(&p, LemAct::Load("lemon".into(), "qwen-chat".into()));
        assert!(a.is_busy());
        super::finish_jobs(&mut a);
        assert!(p.lem.called().iter().any(|c| c == "load lemon/qwen-chat"));
        let acts: Vec<String> = p.audited().into_iter().map(|(a, _)| a).collect();
        assert_eq!(acts, vec!["settings.lemonade.pull", "settings.lemonade.pull", "settings.lemonade.load"]);
        p.admin.store(false, Ordering::SeqCst);
        a.lem_confirm(&p, LemAct::Pull("lemon".into(), "big".into()));
        assert_eq!(p.lem.called().iter().filter(|c| c.starts_with("pull")).count(), 1);
    }

    #[test]
    fn a_provider_is_switched_off_audited_and_only_by_an_administrator_and_the_tool_routing_switch_is_the_machines() {
        let (p, mut a) = rig("b-md-switch", true);
        a.sel[a.tab] = 1;
        a.handle_models(&Key::Char('x'), &p);
        assert!(!p.settings_get().providers[1].enabled);
        assert!(said(&a).is_some_and(|m| m.contains("local disabled")));
        a.handle_models(&Key::Char('T'), &p);
        assert!(p.settings_get().no_tool_routing);
        assert!(p.audited().iter().any(|(a, s)| a == "settings.tool_routing" && s == "off"));
        a.handle_models(&Key::Char('P'), &p);
        assert!(p.settings_get().providers[1].native_pdf_output);
        p.admin.store(false, Ordering::SeqCst);
        a.handle_models(&Key::Char('x'), &p);
        a.handle_models(&Key::Char('T'), &p);
        assert!(p.settings_get().no_tool_routing, "a person's key does not change the machine's switch");
        assert!(!p.settings_get().providers[1].enabled);
    }

    #[test]
    fn a_nvidia_key_is_checked_stored_by_name_and_the_provider_saved_before_the_test_starts() {
        let (p, mut a) = rig("b-md-nvidia", true);
        assert!(a.save_nvidia(&p, "").is_err());
        assert!(a.save_nvidia(&p, "nvapi short").is_err());
        assert!(p.secrets.lock().unwrap().is_empty() && p.audited().is_empty());
        let key = "nvapi-0123456789abcdef0123456789abcdef";
        assert!(a.save_nvidia(&p, key).unwrap().contains("NVIDIA NIM added"));
        assert!(p.secret_has_ref("vault:nvidia-key"));
        assert!(p.settings_get().providers.iter().any(|x| x.id == "nvidia"));
        assert!(!p.audited().iter().any(|(_, s)| s.contains(key)));
        p.vault_locked.store(true, Ordering::SeqCst);
        assert!(a.save_nvidia(&p, key).unwrap_err().contains("vault is locked"));
    }

    #[test]
    fn the_provider_form_saves_a_new_provider_and_refuses_a_second_with_the_same_name() {
        let (p, mut a) = rig("b-md-form", true);
        let mut f = SettingsApp::provider_form(&p, None);
        f.set_value(0, "cloud");
        f.set_choice(1, Kind::ALL.iter().position(|k| *k == Kind::Compat).unwrap());
        f.set_value(2, "http://10.0.0.5:8000/v1");
        a.save_provider(&p, &f, None).unwrap();
        assert!(p.settings_get().providers.iter().any(|x| x.id == "cloud"));
        assert!(p.audited().iter().any(|(a, s)| a == "settings.provider.save" && s.starts_with("cloud")));
        assert!(a.save_provider(&p, &f, None).unwrap_err().contains("already exists"));
    }

    #[test]
    fn the_test_buttons_say_why_nothing_can_be_tested_and_run_the_chat_on_a_worker() {
        let (p, mut a) = rig("b-md-test", true);
        *p.ai.check_refusal.lock().unwrap() = Some("no key".into());
        a.test_provider(&p, "lemon");
        assert_eq!(failed(&a).as_deref(), Some("lemon: no key"));
        a.test_chat(&p);
        assert!(failed(&a).is_some_and(|m| m.contains("Choose a chat model first")));
        p.settings_update(&mut |s| s.chat = Some(iyakki_shared::settings::ModelChoice { provider: "local".into(), model: "small-7b".into() })).unwrap();
        a.test_chat(&p);
        assert!(a.is_busy());
        super::finish_jobs(&mut a);
        assert_eq!(*p.ai.chats.lock().unwrap(), vec![("local".to_string(), "small-7b".to_string())]);
    }

    #[test]
    fn the_page_draws_with_and_without_providers_for_both_kinds_of_person_at_five_sizes() {
        let (p, mut a) = rig("b-md-draw", true);
        for admin in [true, false] {
            p.admin.store(admin, Ordering::SeqCst);
            for sel in [0usize, 1] {
                a.sel[a.tab] = sel;
                for (w, h) in SIZES {
                    let (mut g, r) = blank(w, h);
                    a.render_models(&mut g, r, 1.0, &p);
                }
            }
        }
        p.settings_update(&mut |s| s.providers.clear()).unwrap();
        let (mut g, r) = blank(800, 480);
        a.render_models(&mut g, r, 1.0, &p);
    }
}

mod voice {
    use super::*;
    use super::super::apps_port::{SecretsPort, VoicePort};
    use iyakki_shared::voice::SpeechEngine;

    fn rig(tag: &str, admin: bool) -> (FakeB, SettingsApp) {
        (FakeB::new(admin, temp_dir(tag)), app_on(&format!("{tag}-app"), Tab::Voice))
    }

    #[test]
    fn the_switches_are_saved_in_the_settings_file_and_each_is_said() {
        let (p, mut a) = rig("b-vo-switch", true);
        let before = p.voice_ops().voice_settings();
        a.handle_voice(&Key::Char('a'), &p);
        assert_eq!(p.voice_ops().voice_settings().auto_send, !before.auto_send);
        a.handle_voice(&Key::Char('K'), &p);
        assert_eq!(p.voice_ops().voice_settings().keep_listening, !before.keep_listening);
        a.handle_voice(&Key::Char('r'), &p);
        assert_eq!(p.voice_ops().voice_settings().speak_replies, !before.speak_replies);
        assert!(said(&a).is_some_and(|m| m.contains("Read replies aloud")));
        p.busy.store(true, Ordering::SeqCst);
        a.handle_voice(&Key::Char('a'), &p);
        assert!(failed(&a).is_some_and(|m| m.contains("busy")));
    }

    #[test]
    fn a_person_who_is_not_an_administrator_changes_nothing_and_the_refusal_is_audited() {
        let (p, mut a) = rig("b-vo-user", false);
        let before = p.voice_ops().voice_settings();
        for c in ['a', 'u', 'K', 'W', 'r', 'S', 'V', 'e', 'f'] {
            a.handle_voice(&Key::Char(c), &p);
        }
        assert_eq!(p.voice_ops().voice_settings(), before);
        assert!(p.audited().iter().filter(|(a, _)| a == "settings.denied").count() >= 9);
        p.voice_allowed.store(false, Ordering::SeqCst);
        a.handle_voice(&Key::Char('m'), &p);
        assert!(p.audited().iter().any(|(a, s)| a == "voice.denied" && s == "test"));
        assert!(failed(&a).is_some_and(|m| m.contains("not enabled for your account")));
    }

    #[test]
    fn the_engine_forms_save_an_endpoint_keep_its_key_in_the_vault_and_audit_by_name() {
        let (p, mut a) = rig("b-vo-engine", true);
        let key = "sk-test-0123456789abcdef";
        let f = Form::new("e").choice("Type", &["HTTP", "Command"], 0).text("URL", "http://127.0.0.1:8080/v1/audio/transcriptions", "").text("Model", "tiny.en", "").secret("Key", "").text("Language", "en", "");
        let mut f = f;
        f.set_value(3, key);
        a.save_speech_engine(&p, &f, true).unwrap();
        assert!(matches!(p.voice_ops().voice_settings().stt, Some(SpeechEngine::Http { .. })));
        assert!(p.secret_has_ref("vault:stt-key"));
        assert!(!p.audited().iter().any(|(_, s)| s.contains(key)));
        assert!(p.audited().iter().any(|(a, _)| a == "settings.voice.stt"));
        let bad = Form::new("e").choice("Type", &["HTTP", "Command"], 0).text("URL", "not a url", "").text("Model", "", "").secret("Key", "").text("Voice", "", "");
        assert!(a.save_speech_engine(&p, &bad, false).is_err());
        let browser = Form::new("e").choice("Type", &["HTTP", "Command", "Browser"], 2).text("URL", "", "").text("Model", "", "").secret("Key", "").text("Voice", "", "");
        a.save_speech_engine(&p, &browser, false).unwrap();
        assert!(matches!(p.voice_ops().voice_settings().tts, Some(SpeechEngine::Browser)));
        p.vault_locked.store(true, Ordering::SeqCst);
        assert!(a.save_speech_engine(&p, &f, true).unwrap_err().contains("vault is locked"));
    }

    #[test]
    fn the_wording_fixer_goes_through_the_port_and_re_checking_the_engines_asks_the_host_tts() {
        let (p, mut a) = rig("b-vo-fix", true);
        a.handle_voice(&Key::Char('W'), &p);
        assert!(p.voice_ops().voice_settings().fix_wording);
        assert!(p.audited().iter().any(|(a, s)| a == iyakki_shared::dictfix::AUDIT && s == "on"));
        a.handle_voice(&Key::Char('k'), &p);
        assert!(failed(&a).is_some_and(|m| m.contains("switched off in this build")));
        p.ai.tts.enabled.store(true, Ordering::SeqCst);
        a.handle_voice(&Key::Char('k'), &p);
        assert_eq!(p.ai.tts.rechecked.load(Ordering::SeqCst), 1);
        assert!(said(&a).is_some_and(|m| m.contains("Looking for speech engines")));
    }

    #[test]
    fn cycling_the_pause_and_the_speed_and_the_highlight_is_saved_and_audited() {
        let (p, mut a) = rig("b-vo-cycle", true);
        let v = p.voice_ops().voice_settings();
        a.handle_voice(&Key::Char('U'), &p);
        a.handle_voice(&Key::Char('f'), &p);
        a.handle_voice(&Key::Char('h'), &p);
        let w = p.voice_ops().voice_settings();
        assert_ne!(w.pause_ms, v.pause_ms);
        assert_ne!(w.speed_pct, v.speed_pct);
        assert_ne!(w.highlight, v.highlight);
        let acts: Vec<String> = p.audited().into_iter().map(|(a, _)| a).collect();
        assert_eq!(acts, vec!["settings.voice.pause", "settings.voice.speed", "settings.voice.highlight"]);
    }

    #[test]
    fn the_page_draws_with_and_without_a_found_engine_for_both_kinds_of_person_at_five_sizes() {
        let (p, mut a) = rig("b-vo-draw", true);
        for admin in [true, false] {
            p.admin.store(admin, Ordering::SeqCst);
            for tts_enabled in [false, true] {
                p.ai.tts.enabled.store(tts_enabled, Ordering::SeqCst);
                for (w, h) in SIZES {
                    let (mut g, r) = blank(w, h);
                    a.render_voice(&mut g, r, 1.0, &p);
                }
            }
        }
        assert!(p.ai.tts.ensured.load(Ordering::SeqCst) > 0, "the page asks the engines to be looked for");
    }
}

mod wake_phrase {
    use super::*;
    use super::super::apps_port::VoicePort;

    fn rig(tag: &str, admin: bool) -> (FakeB, SettingsApp) {
        (FakeB::new(admin, temp_dir(tag)), app_on(&format!("{tag}-app"), Tab::Voice))
    }

    #[test]
    fn the_wake_phrase_is_opt_in_audited_by_name_and_only_for_an_administrator() {
        let (p, mut a) = rig("b-wk-on", true);
        assert!(!p.voice_ops().voice_settings().my_wake);
        a.handle_voice(&Key::Char('M'), &p);
        assert!(p.voice_ops().voice_settings().my_wake);
        assert!(said(&a).is_some_and(|m| m.contains("experimental")));
        a.handle_voice(&Key::Char('M'), &p);
        assert!(!p.voice_ops().voice_settings().my_wake);
        assert_eq!(p.audited().iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>(), ["wake-on", "wake-off"]);
        p.admin.store(false, Ordering::SeqCst);
        a.handle_voice(&Key::Char('M'), &p);
        assert!(!p.voice_ops().voice_settings().my_wake);
    }

    #[test]
    fn a_refusal_of_the_wake_service_is_shown_and_nothing_is_audited() {
        let (p, mut a) = rig("b-wk-refuse", true);
        *p.voice.refusal.lock().unwrap() = Some("My IYAKKI is off, or there is no trained model file.".into());
        a.handle_voice(&Key::Char('M'), &p);
        assert!(failed(&a).is_some_and(|m| m.contains("trained model")));
        assert!(p.audited().is_empty() && !p.voice_ops().voice_settings().my_wake);
    }

    #[test]
    fn sensitivity_cycles_and_the_three_experimental_switches_save_and_audit() {
        let (p, mut a) = rig("b-wk-sens", true);
        let s0 = p.voice_ops().voice_settings().my_wake_sens;
        a.handle_voice(&Key::Char('N'), &p);
        assert_eq!(p.voice_ops().voice_settings().my_wake_sens, (s0.min(2) + 1) % 3);
        let v = p.voice_ops().voice_settings();
        for c in ['T', 'L', 'A', 'E'] {
            a.handle_voice(&Key::Char(c), &p);
        }
        let w = p.voice_ops().voice_settings();
        assert_eq!((w.my_wake_confirm, w.my_cloud_stream, w.spoken_approvals, w.echo_cancel), (!v.my_wake_confirm, !v.my_cloud_stream, !v.spoken_approvals, !v.echo_cancel));
        assert_eq!(p.audited().len(), 5);
        assert!(p.audited().iter().all(|(a, _)| a.starts_with("voice.") || a.starts_with("settings.")), "{:?}", p.audited());
        let rows = SettingsApp::vo_wake_rows(&p);
        assert_eq!(rows.len(), 2);
        assert!(!SettingsApp::vo_echo_row(&p).1.is_empty() && !SettingsApp::vo_spoken_row(&p).1.is_empty());
    }
}
