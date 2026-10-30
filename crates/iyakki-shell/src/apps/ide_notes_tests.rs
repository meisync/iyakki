//! Tests of the Context tab of the AI IDE: keyboard and mouse paths, the files it writes, agent notes and their review, and the layout at
//! the four sizes. A child module of `ide_tests`, so it shares that file's world, project and render helpers.

use iyakki_agent::notes::{self, Kind, NewNote, Status, Who};

use super::super::ide_layout::Pane;
use super::super::ide_notes::{self, NotesAct};
use super::*;

fn setup(tag: &str) -> (World, IdeApp, PathBuf) {
    let w = world(tag);
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    cur_mut(&mut app).notes.poll_every = Duration::ZERO;
    app.show_main(MainTab::Notes);
    (w, app, d)
}

fn person() -> Who {
    Who::Person("Casey".into())
}

fn add(d: &Path, title: &str, text: &str, kind: Kind, pinned: bool) -> String {
    notes::edit(d, |n| n.add(NewNote { pinned, ..NewNote::new(title, text, kind) }, &person(), iyakki_agent::clock::now_unix())).unwrap().id
}

fn labels(g: &Gfx) -> Vec<String> {
    g.hits.iter().map(|h| h.label.clone()).collect()
}

fn click(app: &mut IdeApp, cx: &AppCx, label: &str, wd: u32, ht: u32) {
    let g = render_at(app, cx, wd, ht);
    let h = g.hits.iter().rev().find(|h| h.label == label || (label.ends_with('*') && h.label.starts_with(&label[..label.len() - 1]))).unwrap_or_else(|| panic!("no control {label:?} in {:?}", labels(&g)));
    let ev = h.ev.clone();
    app.handle(&ev, cx);
}

fn reload(app: &mut IdeApp, cx: &AppCx) {
    ticks(app, cx, 2);
}

#[test]
fn notes_is_a_main_tab_after_tasks_with_its_own_glyph_a_count_and_a_shortcut() {
    assert_eq!(MainTab::ALL.iter().position(|t| *t == MainTab::Notes), Some(3), "right after Tasks");
    assert_eq!(super::super::ide_ui::main_glyph(MainTab::Notes), crate::fx::Glyph::Note);
    let (w, mut app, d) = setup("notes-tab");
    let g = render_at(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(T_MAIN + 3) && h.label == "Context"), "{:?}", labels(&g));
    add(&d, "Use sqlite", "One file.", Kind::Decision, false);
    add(&d, "Never use sudo", "The harness refuses it.", Kind::Constraint, true);
    reload(&mut app, &w.cx);
    let g = render_at(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(T_MAIN + 3) && h.label == "Context 2"), "the tab counts the notes: {:?}", labels(&g));
    assert!(g.hits.iter().any(|h| h.label == "open the Context tab"), "the top bar says '2 notes in context' and opens the tab");
    assert_eq!(cur(&app).notes.chip().as_deref(), Some("2 in context"));
    // the chip opens the tab from another tab; Ctrl+Shift+Y toggles it
    app.show_main(MainTab::Editor);
    click(&mut app, &w.cx, "open the Context tab", 1280, 720);
    assert_eq!(cur(&app).main_tab, MainTab::Notes);
    assert_eq!(cur(&app).owner(), Owner::Notes);
    key(&mut app, &w.cx, Key::Ctrl('Y'));
    assert_eq!(cur(&app).main_tab, MainTab::Editor);
    key(&mut app, &w.cx, Key::Ctrl('Y'));
    assert_eq!(cur(&app).main_tab, MainTab::Notes);
    // the pinned one is listed first
    assert_eq!(cur(&app).notes.selected().unwrap().title, "Never use sudo");
}

#[test]
fn a_note_is_written_edited_pinned_archived_restored_and_deleted_with_the_keyboard_and_the_files_follow() {
    let (w, mut app, d) = setup("notes-keys");
    key(&mut app, &w.cx, Key::Char('n'));
    assert_eq!(cur(&app).owner(), Owner::NotesText, "the form owns the keyboard: letters are typed");
    typ(&mut app, &w.cx, "Use port 8081");
    key(&mut app, &w.cx, Key::Tab);
    typ(&mut app, &w.cx, "8080 is the registry.");
    key(&mut app, &w.cx, Key::Enter);
    typ(&mut app, &w.cx, "Second line");
    key(&mut app, &w.cx, Key::Ctrl('k'));
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(!cur(&app).notes.editing(), "{:?}", cur(&app).notes.status);
    let (n, p) = notes::load(&d);
    assert!(p.is_none());
    let x = n.get("N-001").unwrap();
    assert_eq!((x.title.as_str(), x.text.as_str(), x.kind, x.author.as_str(), x.by_agent, x.reviewed), ("Use port 8081", "8080 is the registry.\nSecond line", Kind::Constraint, "tester", false, true));
    let md = std::fs::read_to_string(d.join(".iyakki/CONTEXT.md")).unwrap();
    assert!(md.contains("### N-001 Use port 8081") && md.contains("Second line"), "{md}");
    // edit: e opens the form filled in
    key(&mut app, &w.cx, Key::Char('e'));
    assert!(cur(&app).notes.editing());
    typ(&mut app, &w.cx, "!");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert_eq!(notes::load(&d).0.get("N-001").unwrap().title, "Use port 8081!");
    // Esc leaves the form without saving
    key(&mut app, &w.cx, Key::Char('e'));
    typ(&mut app, &w.cx, "zzz");
    key(&mut app, &w.cx, Key::Esc);
    assert!(!cur(&app).notes.editing() && notes::load(&d).0.get("N-001").unwrap().title == "Use port 8081!");
    assert_eq!(cur(&app).owner(), Owner::Notes);
    // pin, archive, restore
    key(&mut app, &w.cx, Key::Char('p'));
    assert!(notes::load(&d).0.get("N-001").unwrap().pinned);
    key(&mut app, &w.cx, Key::Char('a'));
    assert_eq!(notes::load(&d).0.get("N-001").unwrap().status, Status::Archived);
    assert!(!notes::load(&d).0.get("N-001").unwrap().pinned, "an archived note is not pinned");
    assert!(cur(&app).notes.selected().is_none(), "the list shows active notes");
    key(&mut app, &w.cx, Key::Char('v'));
    assert_eq!(cur(&app).notes.selected().unwrap().id, "N-001", "v shows the archived ones");
    key(&mut app, &w.cx, Key::Char('a'));
    assert_eq!(notes::load(&d).0.get("N-001").unwrap().status, Status::Active);
    key(&mut app, &w.cx, Key::Char('v'));
    key(&mut app, &w.cx, Key::Char('v'));
    // delete asks: n keeps, y deletes
    key(&mut app, &w.cx, Key::Char('x'));
    let g = render_at(&mut app, &w.cx, 1280, 720);
    assert!(labels(&g).contains(&"Delete".to_string()) && labels(&g).contains(&"Keep".to_string()), "{:?}", labels(&g));
    key(&mut app, &w.cx, Key::Esc);
    assert_eq!(notes::load(&d).0.notes.len(), 1, "asked, then kept");
    key(&mut app, &w.cx, Key::Char('x'));
    key(&mut app, &w.cx, Key::Char('y'));
    assert!(notes::load(&d).0.notes.is_empty());
    assert!(std::fs::read_to_string(d.join(".iyakki/CONTEXT.md")).unwrap().contains("No notes yet"));
}

#[test]
fn a_note_with_a_secret_is_refused_in_the_form_and_nothing_is_written() {
    let (w, mut app, d) = setup("notes-secret");
    key(&mut app, &w.cx, Key::Char('n'));
    typ(&mut app, &w.cx, "Registry login");
    key(&mut app, &w.cx, Key::Tab);
    typ(&mut app, &w.cx, "password: hunter2xyz99");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(cur(&app).notes.editing(), "the form stays open so nothing typed is lost");
    let g = render_at(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Save"));
    assert!(!notes::json_path(&d).exists(), "nothing is written");
    // fixing the text saves it
    for _ in 0..22 {
        key(&mut app, &w.cx, Key::Backspace);
    }
    typ(&mut app, &w.cx, "the login is in the vault as registry");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(!cur(&app).notes.editing());
    assert_eq!(notes::load(&d).0.notes.len(), 1);
}

#[test]
fn an_agent_note_arrives_with_a_toast_a_badge_and_a_review_and_trusting_changes_how_agents_read_it() {
    let (w, mut app, d) = setup("notes-agent");
    add(&d, "Person rule", "keep", Kind::Convention, false);
    reload(&mut app, &w.cx);
    assert_eq!(cur(&app).notes.counts(), (1, 0, 0));
    iyakki_agent::notetools::call(&d, "Agent 2", "note_add", &serde_json::json!({"title": "Config is in app.toml", "text": "Not in env vars.", "kind": "gotcha", "pinned": true})).unwrap();
    reload(&mut app, &w.cx);
    assert_eq!(cur(&app).notes.counts(), (2, 0, 1));
    assert!(cur(&app).status.as_ref().is_some_and(|(m, ok)| *ok && m.contains("Agent 2 added to the project context: Config is in app.toml")), "a toast: {:?}", cur(&app).status);
    let g = render_at(&mut app, &w.cx, 1280, 720);
    let l = labels(&g);
    assert!(l.iter().any(|x| x.contains("show the entries agents added")) && l.contains(&"Mark all reviewed".to_string()), "{l:?}");
    assert!(g.hits.iter().any(|h| h.label == "open the Context tab"), "the top bar chip shows it too");
    // select it: the agent's note can be trusted or reviewed, and its pin request is only a request
    let idx = cur(&app).notes.selected().map(|n| n.id.clone());
    assert_eq!(idx.as_deref(), Some("N-001"));
    key(&mut app, &w.cx, Key::Down);
    assert_eq!(cur(&app).notes.selected().unwrap().author, "Agent 2");
    assert!(!cur(&app).notes.selected().unwrap().pinned && cur(&app).notes.selected().unwrap().pin_requested);
    let block = notes::context_block(&d, 2000);
    assert!(block.contains("context written earlier by agents") && block.find("Person rule").unwrap() < block.find("Config is in app.toml").unwrap());
    key(&mut app, &w.cx, Key::Char('t'));
    let block = notes::context_block(&d, 2000);
    assert!(!block.contains("context written earlier by agents"), "a trusted note is an instruction: {block}");
    assert_eq!(cur(&app).notes.counts().2, 0, "trusting is a review");
    key(&mut app, &w.cx, Key::Char('t'));
    assert!(notes::context_block(&d, 2000).contains("context written earlier by agents"));
    key(&mut app, &w.cx, Key::Char('r'));
    assert_eq!(cur(&app).notes.counts().2, 0);
    // t on a person's note says no
    key(&mut app, &w.cx, Key::Up);
    key(&mut app, &w.cx, Key::Char('t'));
    assert!(cur(&app).notes.status.as_ref().is_some_and(|(m, ok)| !*ok && m.contains("written by an agent")));
    // the agent setting cycles on, ask, off and turning it off stops agents
    assert_eq!(cur(&app).notes.settings().agents_may_add, notes::AgentsMayAdd::On);
    key(&mut app, &w.cx, Key::Char('g'));
    key(&mut app, &w.cx, Key::Char('g'));
    assert_eq!(notes::load(&d).0.settings.agents_may_add, notes::AgentsMayAdd::Off);
    assert!(iyakki_agent::notetools::call(&d, "Agent 2", "note_add", &serde_json::json!({"title": "A late one", "kind": "gotcha"})).unwrap_err().contains("turned off"));
}

#[test]
fn filters_search_and_the_share_switch_work_by_key_and_by_click() {
    let (w, mut app, d) = setup("notes-filter");
    add(&d, "Use sqlite", "One file.", Kind::Decision, false);
    add(&d, "Never sudo", "Refused.", Kind::Constraint, true);
    add(&d, "Run make test", "Not cargo test.", Kind::Gotcha, false);
    iyakki_agent::notetools::call(&d, "Claude Code", "note_add", &serde_json::json!({"title": "Port 9000 is taken", "kind": "gotcha"})).unwrap();
    reload(&mut app, &w.cx);
    let shown = |app: &IdeApp| cur(app).notes.filter.clone();
    let n_shown = |app: &mut IdeApp| -> usize {
        let g = render_at(app, &w.cx, 1280, 720);
        g.hits.iter().filter(|h| h.label.starts_with("entry N-")).count()
    };
    assert_eq!(n_shown(&mut app), 4);
    key(&mut app, &w.cx, Key::Char('k'));
    assert_eq!(shown(&app).kind, Some(Kind::Decision));
    assert_eq!(n_shown(&mut app), 1);
    key(&mut app, &w.cx, Key::Char('k'));
    key(&mut app, &w.cx, Key::Char('k'));
    key(&mut app, &w.cx, Key::Char('k'));
    assert_eq!(shown(&app).kind, Some(Kind::Gotcha));
    assert_eq!(n_shown(&mut app), 2);
    // authors cycle in name order: Casey, then Claude Code
    key(&mut app, &w.cx, Key::Char('u'));
    assert_eq!(shown(&app).author.as_deref(), Some("Casey"));
    key(&mut app, &w.cx, Key::Char('u'));
    assert_eq!(shown(&app).author.as_deref(), Some("Claude Code"));
    assert_eq!(n_shown(&mut app), 1);
    // Esc clears every filter
    key(&mut app, &w.cx, Key::Esc);
    assert_eq!(shown(&app), notes::Filter::default());
    key(&mut app, &w.cx, Key::Char('f'));
    assert_eq!(n_shown(&mut app), 1, "pinned only");
    key(&mut app, &w.cx, Key::Char('f'));
    key(&mut app, &w.cx, Key::Char('w'));
    assert_eq!(n_shown(&mut app), 1, "new by agent only");
    key(&mut app, &w.cx, Key::Esc);
    // the search box takes typing and Enter leaves it
    key(&mut app, &w.cx, Key::Char('/'));
    assert_eq!(cur(&app).owner(), Owner::NotesText);
    typ(&mut app, &w.cx, "make");
    key(&mut app, &w.cx, Key::Enter);
    assert_eq!(cur(&app).owner(), Owner::Notes);
    assert_eq!(n_shown(&mut app), 1);
    key(&mut app, &w.cx, Key::Esc);
    assert_eq!(n_shown(&mut app), 4);
    // a click on a filter does what its key does
    click(&mut app, &w.cx, "kind: all", 1280, 720);
    assert_eq!(shown(&app).kind, Some(Kind::Decision));
    key(&mut app, &w.cx, Key::Esc);
    // the share switch writes the managed block and never touches the rest of the file
    std::fs::write(d.join("CLAUDE.md"), "# Mine\n\nKeep this.\n").unwrap();
    click(&mut app, &w.cx, "Share*", 1280, 720);
    let t = std::fs::read_to_string(d.join("CLAUDE.md")).unwrap();
    assert!(t.starts_with("# Mine\n\nKeep this.") && t.contains(notes::BEGIN) && t.contains("Never sudo") && !t.contains("Port 9000"), "{t}");
    click(&mut app, &w.cx, "Share*", 1280, 720);
    assert_eq!(std::fs::read_to_string(d.join("CLAUDE.md")).unwrap().trim_end(), "# Mine\n\nKeep this.");
}

#[test]
fn clicks_select_create_save_and_cancel() {
    let (w, mut app, d) = setup("notes-mouse");
    add(&d, "First note", "alpha", Kind::Reference, false);
    add(&d, "Second note", "beta", Kind::Gotcha, false);
    reload(&mut app, &w.cx);
    click(&mut app, &w.cx, "entry N-002", 1280, 720);
    assert_eq!(cur(&app).notes.selected().unwrap().id, "N-002");
    click(&mut app, &w.cx, "Pin", 1280, 720);
    assert!(notes::load(&d).0.get("N-002").unwrap().pinned);
    click(&mut app, &w.cx, "New entry", 1280, 720);
    assert!(cur(&app).notes.editing());
    // the title, text and source areas take clicks
    let g = render_at(&mut app, &w.cx, 1280, 720);
    for ev in [Event::Focus(ide_notes::FOCUS_TITLE), Event::Area(ide_notes::TEXT_AREA), Event::Focus(ide_notes::FOCUS_SOURCE)] {
        assert!(g.hits.iter().any(|h| h.ev == ev), "{:?} {:?}", ev, labels(&g));
    }
    typ(&mut app, &w.cx, "Made with the mouse");
    click(&mut app, &w.cx, "Save", 1280, 720);
    assert!(!cur(&app).notes.editing());
    assert!(notes::load(&d).0.notes.iter().any(|n| n.title == "Made with the mouse"));
    click(&mut app, &w.cx, "New entry", 1280, 720);
    typ(&mut app, &w.cx, "Dropped");
    click(&mut app, &w.cx, "Cancel", 1280, 720);
    assert!(!notes::load(&d).0.notes.iter().any(|n| n.title == "Dropped"));
}

#[test]
fn hand_edits_to_the_notes_file_show_up_and_a_broken_file_is_explained_not_overwritten() {
    let (w, mut app, d) = setup("notes-hand");
    add(&d, "Written in the tab", "x", Kind::Decision, false);
    reload(&mut app, &w.cx);
    std::thread::sleep(Duration::from_millis(20));
    let mut t = std::fs::read_to_string(notes::json_path(&d)).unwrap();
    t = t.replace("Written in the tab", "Edited by hand in git");
    std::fs::write(notes::json_path(&d), t).unwrap();
    reload(&mut app, &w.cx);
    assert_eq!(cur(&app).notes.selected().unwrap().title, "Edited by hand in git");
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(notes::json_path(&d), "{ this is not json").unwrap();
    reload(&mut app, &w.cx);
    let g = render_at(&mut app, &w.cx, 1280, 720);
    assert!(cur(&app).notes.selected().is_none());
    key(&mut app, &w.cx, Key::Char('n'));
    typ(&mut app, &w.cx, "Lost?");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(std::fs::read_to_string(notes::json_path(&d)).unwrap().contains("this is not json"), "the broken file was not overwritten");
    assert!(d.join(".iyakki/context.json.broken").exists());
    let _ = g;
}

#[test]
fn a_read_only_workspace_can_read_notes_but_not_change_them() {
    let (w, mut app, d) = setup("notes-ro");
    add(&d, "Keep me", "x", Kind::Decision, false);
    reload(&mut app, &w.cx);
    cur_mut(&mut app).ro = Some(iyakki_agent::workspace::LockInfo { user: "other".into(), session: "s".into(), pid: 1, host: "h".into(), time: 0, opened: 0 });
    // the tab hands the lock to the notes on a tick: wait for that (a fixed two ticks was too few on a loaded machine)
    until(&mut app, &w.cx, "the notes know the workspace is read-only", |a| cur(a).notes.read_only);
    key(&mut app, &w.cx, Key::Char('n'));
    assert!(!cur(&app).notes.editing());
    key(&mut app, &w.cx, Key::Char('p'));
    assert!(!notes::load(&d).0.get("N-001").unwrap().pinned);
    assert!(cur(&app).notes.status.as_ref().is_some_and(|(m, ok)| !*ok && m.contains("Read-only")));
}

#[test]
fn the_tab_never_overlaps_or_leaves_the_screen_at_the_four_sizes_in_every_density_and_the_detail_becomes_a_page() {
    use super::super::ide_chrome::Density;
    let (w, mut app, d) = setup("notes-geometry");
    for i in 0..9 {
        add(&d, &format!("A rather long note title number {i} that needs clipping to fit"), &"A longer text that wraps over several lines in a narrow pane. ".repeat(6), Kind::ALL[i % 6], i == 1);
    }
    iyakki_agent::notetools::call(&d, "Agent 2", "note_add", &serde_json::json!({"title": "From an agent with a quite long title for the badge", "text": "short", "kind": "gotcha"})).unwrap();
    reload(&mut app, &w.cx);
    let mut bad: Vec<String> = Vec::new();
    let (mut saw_narrow, mut saw_wide) = (false, false);
    for density in Density::ALL {
        app.set_density(density);
        for (wd, ht) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            for (state, page) in [("list", false), ("page", true), ("edit", false)] {
                app.show_main(MainTab::Notes);
                cur_mut(&mut app).notes.page = page;
                if state == "edit" {
                    key(&mut app, &w.cx, Key::Char('n'));
                    typ(&mut app, &w.cx, "Draft");
                } else if cur(&app).notes.editing() {
                    key(&mut app, &w.cx, Key::Esc);
                }
                ticks(&mut app, &w.cx, 1);
                let g = render_at(&mut app, &w.cx, wd, ht);
                let tag = format!("{} {wd}x{ht} {state}", density.name());
                let hits: Vec<_> = g.hits.iter().filter(|h| h.label != "divider" && h.rect.w > 0.0 && h.rect.h > 0.0).collect();
                for h in &hits {
                    let r = h.rect;
                    if r.x < -0.5 || r.y < -0.5 || r.x + r.w > wd as f32 + 0.5 || r.y + r.h > ht as f32 + 0.5 {
                        bad.push(format!("{tag}: {:?} is clipped by the screen edge ({:?})", h.label, r));
                    }
                    if matches!(h.ev, Event::Sub(ide_notes::SUB, _)) && r.h < 19.5 {
                        bad.push(format!("{tag}: {:?} is only {:.1} px high", h.label, r.h));
                    }
                }
                let small: Vec<_> = hits.iter().filter(|h| h.rect.w * h.rect.h < (wd * ht) as f32 * 0.04 && matches!(h.ev, Event::Sub(ide_notes::SUB, _))).collect();
                for (i, x) in small.iter().enumerate() {
                    for y in &small[i + 1..] {
                        if partly_overlap(&x.rect, &y.rect) {
                            bad.push(format!("{tag}: {:?} {:?} overlaps {:?} {:?}", x.label, x.rect, y.label, y.rect));
                        }
                    }
                }
                let page = page || state == "edit";
                let cards = hits.iter().filter(|h| h.label.starts_with("entry N-")).count();
                let detail = hits.iter().any(|h| h.label == "Edit" || h.label == "Save");
                let narrow = cur(&app).notes.narrow();
                if narrow && ((page && cards > 0) || (!page && detail)) {
                    bad.push(format!("{tag}: narrow pane draws {cards} cards and detail={detail}"));
                }
                if state != "edit" && !narrow && (cards == 0 || !detail) {
                    bad.push(format!("{tag}: wide pane lacks the list or the detail (cards {cards}, detail {detail})"));
                }
                if state == "edit" && !hits.iter().any(|h| h.label == "Save") {
                    bad.push(format!("{tag}: the form has no Save button (editing {}, owner {:?}, status {:?})", cur(&app).notes.editing(), cur(&app).owner(), cur(&app).notes.status));
                }
                if state == "edit" {
                    for ev in [Event::Focus(ide_notes::FOCUS_TITLE), Event::Area(ide_notes::TEXT_AREA), Event::Focus(ide_notes::FOCUS_SOURCE)] {
                        if !g.hits.iter().any(|h| h.ev == ev) {
                            bad.push(format!("{tag}: the form lacks {ev:?}"));
                        }
                    }
                    key(&mut app, &w.cx, Key::Esc);
                }
                saw_narrow |= narrow;
                saw_wide |= !narrow;
            }
        }
    }
    cur_mut(&mut app).notes.page = false;
    assert!(saw_wide && saw_narrow, "both layouts were exercised (narrow {saw_narrow}, wide {saw_wide})");
    assert!(bad.is_empty(), "{} problems:\n{}", bad.len(), bad.iter().take(40).cloned().collect::<Vec<_>>().join("\n"));
    let _ = NotesAct::None;
}

fn say(app: &mut IdeApp, cx: &AppCx, text: &str) -> bool {
    app.focus_pane(Pane::Chat);
    cur_mut(app).chat.prefill(text);
    let i = app.active.unwrap();
    app.try_context_intent(i, cx)
}

fn last_reply(app: &IdeApp) -> String {
    cur(app).chat.messages().last().map(|m| m.content.clone()).unwrap_or_default()
}

#[test]
fn saying_remember_in_the_chat_changes_the_context_before_any_model_and_can_be_undone() {
    let (w, mut app, d) = setup("ctx-say");
    assert!(say(&mut app, &w.cx, "remember that we use pnpm"), "nothing is left to send to a model");
    let (n, _) = notes::load(&d);
    let e = n.get("N-001").unwrap();
    assert_eq!((e.title.as_str(), e.author.as_str(), e.by_agent, e.reviewed, e.kind), ("We use pnpm", "tester", false, true, Kind::Decision), "the person's own, trusted, not 'new by agent'");
    assert!(last_reply(&app).starts_with("Added to the project context: N-001 we use pnpm") && last_reply(&app).contains("Undo"), "{}", last_reply(&app));
    assert_eq!(cur(&app).chat.input_text(), "", "the composer is cleared");
    // the chip and the key undo it
    let g = render_at(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Undo context change"));
    click(&mut app, &w.cx, "Undo context change", 1280, 720);
    assert!(notes::load(&d).0.notes.is_empty());
    assert!(cur(&app).ctx_undo.is_none() && !render_at(&mut app, &w.cx, 1280, 720).hits.iter().any(|h| h.label == "Undo context change"));
    // pin that: the entry touched last
    assert!(say(&mut app, &w.cx, "note: never edit generated/ files"));
    assert!(say(&mut app, &w.cx, "pin that"));
    assert!(notes::load(&d).0.get("N-001").unwrap().pinned);
    app.show_main(MainTab::Notes);
    key(&mut app, &w.cx, Key::Char('z'));
    assert!(!notes::load(&d).0.get("N-001").unwrap().pinned, "z in the Context tab undoes the pin");
    assert!(say(&mut app, &w.cx, "what is in the context?"));
    assert!(last_reply(&app).contains("1 active entry") && last_reply(&app).contains("N-001"), "{}", last_reply(&app));
}

#[test]
fn an_ambiguous_forget_asks_with_numbered_options_and_the_number_answers_it() {
    let (w, mut app, d) = setup("ctx-ask");
    add(&d, "We use pnpm", "", Kind::Decision, false);
    add(&d, "pnpm workspaces live in packages", "", Kind::Reference, false);
    assert!(say(&mut app, &w.cx, "forget the pnpm note"));
    assert!(last_reply(&app).contains("Which one should I forget?") && last_reply(&app).contains("\n1. N-001") && last_reply(&app).contains("\n2. N-002"), "{}", last_reply(&app));
    assert_eq!(notes::load(&d).0.active().count(), 2, "nothing yet");
    assert!(say(&mut app, &w.cx, "2"));
    assert_eq!(notes::load(&d).0.get("N-002").unwrap().status, Status::Archived);
    assert_eq!(notes::load(&d).0.get("N-001").unwrap().status, Status::Active);
    assert!(cur(&app).ctx_pending.is_none());
    // an answer that is not a number drops the question and is an ordinary message
    assert!(say(&mut app, &w.cx, "forget the pnpm note"));
    assert!(!say(&mut app, &w.cx, "no thanks, something else"));
    assert!(cur(&app).ctx_pending.is_none());
}

#[test]
fn a_rule_with_a_task_keeps_the_task_for_the_model_and_commentary_and_secrets_are_left_alone() {
    let (w, mut app, d) = setup("ctx-rest");
    assert!(!say(&mut app, &w.cx, "Remember that we use pnpm. Now add a login page."), "the task still goes to the model");
    assert_eq!(cur(&app).chat.input_text(), "Now add a login page.");
    assert_eq!(notes::load(&d).0.notes.len(), 1);
    for t in ["I remember when we used cvs", "note that this is slow", "never mind", "forget it"] {
        assert!(!say(&mut app, &w.cx, t), "{t}");
        assert_eq!(cur(&app).chat.input_text(), t, "untouched");
    }
    assert_eq!(notes::load(&d).0.notes.len(), 1);
    // a credential: not stored, said so, and the message goes on to the chat's own credential handling
    assert!(!say(&mut app, &w.cx, "remember: password: hunter2xyz99"));
    assert!(cur(&app).status.as_ref().is_some_and(|(m, ok)| !*ok && m.contains("credential")), "{:?}", cur(&app).status);
    assert_eq!(notes::load(&d).0.notes.len(), 1);
}
