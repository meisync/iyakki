//! The home-screen layout at the shell level: moving tiles (keyboard, right-click menu, pick up and drop on a chip), undo and reset,
//! groups the person makes, renames and deletes, apps that name a group, roles, the number keys, and the chip row at five sizes.

use super::tests::*;
use super::layout_impl::Card;
use super::*;
use iyakki_agent::layout::Layout;
use iyakki_agent::skills::Scope;
use iyakki_agent::store::tilehub::TileEntry;

fn alice() -> (Shell, Gfx) {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let (mut sh, _) = shell_with_users(&format!("lay{}", N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "alice", "alicepass1");
    sh.render(&mut g);
    (sh, g)
}

fn names(sh: &Shell) -> Vec<String> {
    sh.group_items()
        .iter()
        .filter_map(|it| match it {
            LItem::Sys(t) => Some(t.label.to_string()),
            LItem::App(e) => Some(e.title.clone()),
            LItem::Empty => None,
        })
        .collect()
}

fn in_group(sh: &mut Shell, id: &str) -> Vec<String> {
    let g = sh.groups().into_iter().find(|g| g.id == id).unwrap_or_else(|| panic!("no group {id}: {:?}", sh.groups()));
    sh.set_group(g);
    names(sh)
}

fn gid(sh: &Shell, label: &str) -> String {
    sh.groups().into_iter().find(|g| g.label() == label).unwrap_or_else(|| panic!("no group {label}")).id
}

/// Walk the open layout menu to the entry `id` and press Enter.
fn pick(sh: &mut Shell, id: &str) {
    let ids: Vec<String> = sh.lay.menu.as_ref().expect("a menu is open").1.items.iter().map(|i| i.id.clone()).collect();
    assert!(ids.iter().any(|i| i == id), "no entry {id} in {ids:?}");
    for _ in 0..ids.len() + 1 {
        if sh.lay.menu.as_ref().is_some_and(|m| m.1.items[m.1.sel].id == id) {
            break;
        }
        key(sh, Key::Down);
    }
    key(sh, Key::Enter);
}

fn menu_ids(sh: &Shell) -> Vec<String> {
    sh.lay.menu.as_ref().map(|m| m.1.items.iter().map(|i| i.id.clone()).collect()).unwrap_or_default()
}

fn entry(name: &str, title: &str, group: &str) -> TileEntry {
    TileEntry { key: format!("tile/shared/{name}"), name: name.into(), title: title.into(), icon: String::new(), icon_picture: None, group: group.into(), version: "1.0.0".into(), publisher: String::new(), trust: "Community".into(), dev: false, dir: None, scope: Scope::Shared, caps: Default::default() }
}

/// Give the launcher these custom apps without the store (they are not refreshed away for a while).
fn with_apps(sh: &mut Shell, g: &mut Gfx, apps: Vec<TileEntry>) {
    sh.custom = apps;
    sh.custom_at = sh.tick.max(1);
    sh.render(g);
}

fn saved(sh: &Shell, user: &str) -> Layout {
    Layout::load(sh.env.services.dir(), user)
}

#[test]
fn m_moves_the_selected_tile_through_a_list_and_undo_puts_it_back_once() {
    let (mut sh, mut g) = alice();
    assert_eq!(sh.current_group(), Group::Home);
    assert_eq!(names(&sh)[0], "CHAT");
    key(&mut sh, Key::Char('m'));
    let ids = menu_ids(&sh);
    assert!(!ids.contains(&"to:home".to_string()), "the group it is in is not offered: {ids:?}");
    assert!(ids.contains(&"to:work".to_string()) && ids.contains(&"new".to_string()), "{ids:?}");
    pick(&mut sh, "to:work");
    assert!(sh.lay.menu.is_none());
    assert!(sh.layout_notice().is_some_and(|n| n.contains("Moved CHAT to Build")), "{:?}", sh.layout_notice());
    assert!(!names(&sh).contains(&"CHAT".to_string()));
    assert!(in_group(&mut sh, "work").contains(&"CHAT".to_string()));
    assert_eq!(saved(&sh, "alice").placed.get("sys:chat").map(String::as_str), Some("work"), "kept for this person");
    assert!(saved(&sh, "root").placed.is_empty(), "and only for them");
    // the notice and its Undo button are on screen
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.label == "Undo"));
    key(&mut sh, Key::Char('u'));
    assert!(in_group(&mut sh, "home").contains(&"CHAT".to_string()));
    assert!(saved(&sh, "alice").placed.is_empty());
    key(&mut sh, Key::Char('u'));
    assert!(in_group(&mut sh, "home").contains(&"CHAT".to_string()), "undo is one step; a second u does nothing");
    // the Undo button works with the pointer too
    sh.set_group(Group::Home);
    key(&mut sh, Key::Char('m'));
    pick(&mut sh, "to:system");
    click(&mut sh, &mut g, "Undo");
    assert!(in_group(&mut sh, "home").contains(&"CHAT".to_string()));
}

#[test]
fn right_click_menu_moves_picks_up_and_resets_and_a_chip_takes_a_dropped_tile() {
    let (mut sh, mut g) = alice();
    let s = scale_for(1280, 720);
    let n = names(&sh).len();
    let r = launcher::page_rects(1280, 720, s, n, 0)[1];
    let (x, y) = ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
    let second = names(&sh)[1].clone();
    sh.handle(&Event::MouseRight(x, y));
    assert_eq!(menu_ids(&sh), vec!["move", "pickup", "", "reset-tile", "reset-all"]);
    assert!(!sh.lay.menu.as_ref().unwrap().1.is_enabled(3), "nothing to reset yet");
    assert!(sh.context_menu().is_none(), "the tile's menu replaces the usual one");
    // Move to group…
    pick(&mut sh, "move");
    assert!(menu_ids(&sh).contains(&"to:system".to_string()));
    pick(&mut sh, "to:system");
    assert!(in_group(&mut sh, "system").contains(&second));
    // the tile has a position now: Reset position
    sh.set_group(Group::System);
    sh.render(&mut g);
    let at = names(&sh).iter().position(|n| *n == second).unwrap();
    let r = launcher::page_rects(1280, 720, s, names(&sh).len(), 0)[at];
    sh.handle(&Event::MouseRight((r.x + 8.0) as i32, (r.y + 8.0) as i32));
    assert!(sh.lay.menu.as_ref().unwrap().1.is_enabled(3));
    pick(&mut sh, "reset-tile");
    assert!(!in_group(&mut sh, "system").contains(&second));
    assert!(in_group(&mut sh, "home").contains(&second));
    // pick up, then click a chip
    sh.set_group(Group::Home);
    sh.render(&mut g);
    sh.handle(&Event::MouseRight(x, y));
    pick(&mut sh, "pickup");
    assert!(sh.lay.carry.is_some());
    sh.render(&mut g);
    click(&mut sh, &mut g, "group Build");
    assert!(sh.lay.carry.is_none());
    assert!(in_group(&mut sh, "work").contains(&second));
    // Reset layout puts everything back
    sh.set_group(Group::Work);
    sh.render(&mut g);
    let at = names(&sh).iter().position(|n| *n == second).unwrap();
    let r = launcher::page_rects(1280, 720, s, names(&sh).len(), 0)[at];
    sh.handle(&Event::MouseRight((r.x + 8.0) as i32, (r.y + 8.0) as i32));
    pick(&mut sh, "reset-all");
    assert!(saved(&sh, "alice").placed.is_empty());
    assert!(in_group(&mut sh, "home").contains(&second));
    // carrying with the keyboard: Tab to the group, Enter drops; Esc puts it back
    sh.set_group(Group::Home);
    sh.render(&mut g);
    key(&mut sh, Key::Char('m'));
    pick(&mut sh, "to:work");
    sh.set_group(Group::Work);
    sh.lay.carry = Some("sys:chat".into());
    key(&mut sh, Key::Esc);
    assert!(sh.lay.carry.is_none());
    sh.lay.carry = Some("sys:chat".into());
    key(&mut sh, Key::Tab);
    key(&mut sh, Key::BackTab);
    assert_eq!(sh.current_group(), Group::Work);
    key(&mut sh, Key::Enter);
    assert!(sh.lay.carry.is_none());
}

#[test]
fn new_groups_are_named_with_checks_and_can_be_made_from_the_plus_chip_or_the_move_list() {
    let (mut sh, mut g) = alice();
    click(&mut sh, &mut g, "new group");
    assert!(matches!(sh.lay.card, Some(Card::Name { .. })));
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.label == "Create") && sh.hits().iter().any(|h| h.label == "Cancel"));
    let err = |sh: &Shell| match &sh.lay.card {
        Some(Card::Name { err, .. }) => err.clone(),
        _ => String::from("<no card>"),
    };
    key(&mut sh, Key::Enter);
    assert!(err(&sh).contains("name"), "empty: {}", err(&sh));
    typ(&mut sh, "WORK");
    key(&mut sh, Key::Enter);
    assert!(err(&sh).contains("already"), "a built-in name in any case: {}", err(&sh));
    for _ in 0..4 {
        key(&mut sh, Key::Backspace);
    }
    typ(&mut sh, "12345678901234567");
    key(&mut sh, Key::Enter);
    assert!(err(&sh).contains("16"), "17 characters: {}", err(&sh));
    key(&mut sh, Key::Backspace);
    key(&mut sh, Key::Enter);
    assert!(sh.lay.card.is_none(), "16 characters are fine");
    assert_eq!(sh.current_group().name, "1234567890123456");
    assert_eq!(sh.groups().len(), 6);
    // empty custom groups show, with a hint instead of tiles
    assert_eq!(sh.group_items().len(), 1);
    assert!(matches!(sh.group_items()[0], LItem::Empty));
    sh.render(&mut g);
    key(&mut sh, Key::Enter);
    assert!(sh.toast_text().is_some_and(|t| t.contains("Nothing here yet")));
    key(&mut sh, Key::Char('m'));
    assert!(sh.lay.menu.is_none(), "there is no tile to move in an empty group");
    // a duplicate of the person's own group, in another case
    click(&mut sh, &mut g, "new group");
    typ(&mut sh, "1234567890123456");
    key(&mut sh, Key::Enter);
    assert!(err(&sh).contains("already"));
    key(&mut sh, Key::Esc);
    assert!(sh.lay.card.is_none());
    // from the move list: the tile goes into the new group
    sh.set_group(Group::Home);
    key(&mut sh, Key::Char('m'));
    pick(&mut sh, "new");
    typ(&mut sh, "Games");
    key(&mut sh, Key::Enter);
    assert!(sh.layout_notice().is_some_and(|n| n.contains("Games") && n.contains("CHAT")), "{:?}", sh.layout_notice());
    assert_eq!(sh.current_group().name, "Games");
    assert_eq!(names(&sh), vec!["CHAT"]);
    // saved, and back after signing out and in (the shown group too)
    key(&mut sh, Key::Char('o'));
    login(&mut sh, "alice", "alicepass1");
    assert_eq!(sh.current_group().name, "Games");
    assert_eq!(names(&sh), vec!["CHAT"]);
    // and the limit of 12
    for i in 0..12 {
        let _ = sh.layout.create_group(&format!("G{i}"));
    }
    assert_eq!(sh.layout.groups.len(), 12);
    click(&mut sh, &mut g, "new group");
    typ(&mut sh, "One more");
    key(&mut sh, Key::Enter);
    assert!(err(&sh).contains("12"), "{}", err(&sh));
}

#[test]
fn a_group_is_renamed_and_deleted_with_a_card_and_built_in_groups_have_no_such_entries() {
    let (mut sh, mut g) = alice();
    // built-in groups: the menu has no rename or delete
    for b in ["home", "web-media", "work", "system"] {
        let _ = in_group(&mut sh, b);
        key(&mut sh, Key::Char('g'));
        let ids = menu_ids(&sh);
        assert!(ids.contains(&"new".to_string()) && !ids.iter().any(|i| i == "delete" || i == "rename" || i == "left" || i == "right"), "{b}: {ids:?}");
        key(&mut sh, Key::Esc);
        assert!(sh.lay.menu.is_none());
        // a right click on its chip is the same
        let r = chip(&sh, &g, sh.current_group().label());
        sh.handle(&Event::MouseRight((r.x + 4.0) as i32, (r.y + 4.0) as i32));
        assert!(!menu_ids(&sh).contains(&"delete".to_string()), "{b}");
        key(&mut sh, Key::Esc);
    }
    // a group with two tiles in it
    let id = {
        let o = sh.layout.clone();
        let id = sh.layout.create_group("Games").unwrap();
        let _ = o;
        id
    };
    sh.layout.place("sys:chat", &id, "home").unwrap();
    sh.layout.place("sys:rag", &id, "home").unwrap();
    sh.render(&mut g);
    let _ = in_group(&mut sh, &id);
    key(&mut sh, Key::Char('g'));
    assert!(menu_ids(&sh).contains(&"delete".to_string()) && menu_ids(&sh).contains(&"rename".to_string()));
    pick(&mut sh, "rename");
    typ(&mut sh, "x");
    for _ in 0..6 {
        key(&mut sh, Key::Backspace);
    }
    typ(&mut sh, "Fun");
    key(&mut sh, Key::Enter);
    assert_eq!(sh.current_group().name, "Fun");
    assert_eq!(names(&sh).len(), 2);
    // delete: the card says how many tiles go back, and the safe button is the default
    key(&mut sh, Key::Char('g'));
    pick(&mut sh, "delete");
    sh.render(&mut g);
    assert!(matches!(sh.lay.card, Some(Card::Delete { n: 2, .. })));
    assert!(sh.hits().iter().any(|h| h.label == "Delete group") && sh.hits().iter().any(|h| h.label == "Keep it"));
    key(&mut sh, Key::Enter);
    assert!(sh.lay.card.is_none() && sh.layout.groups.len() == 1, "Enter keeps the group");
    key(&mut sh, Key::Char('g'));
    pick(&mut sh, "delete");
    key(&mut sh, Key::Esc);
    assert_eq!(sh.layout.groups.len(), 1, "Esc keeps it");
    key(&mut sh, Key::Char('g'));
    pick(&mut sh, "delete");
    click(&mut sh, &mut g, "Delete group");
    assert!(sh.layout.groups.is_empty() && saved(&sh, "alice").groups.is_empty());
    assert!(sh.layout_notice().is_some_and(|n| n.contains("2 tiles went back")), "{:?}", sh.layout_notice());
    assert!(in_group(&mut sh, "home").contains(&"CHAT".to_string()) && sh.current_group() == Group::Home);
    // nothing was uninstalled: the tiles are all there
    let mut all = Vec::new();
    for gr in sh.groups() {
        sh.set_group(gr);
        all.extend(names(&sh));
    }
    assert!(all.contains(&"CHAT".to_string()) && all.contains(&"RAG".to_string()));
    // undo brings the group and its tiles back
    key(&mut sh, Key::Char('u'));
    assert_eq!(sh.layout.groups.len(), 1);
    assert!(!in_group(&mut sh, "home").contains(&"CHAT".to_string()));
}

fn chip(sh: &Shell, g: &Gfx, label: &str) -> R {
    let row = sh.chip_row();
    let gs = sh.groups();
    let _ = g;
    row.chips.iter().find(|(i, _)| gs[*i].label() == label).unwrap_or_else(|| panic!("no chip {label}")).1
}

#[test]
fn an_app_that_names_a_group_goes_there_or_the_person_is_asked_once_and_their_move_wins() {
    let (mut sh, mut g) = alice();
    // an existing group (built-in, any case) and none named: no question
    with_apps(&mut sh, &mut g, vec![entry("calc", "Calc", "system"), entry("plain", "Plain", ""), entry("odd", "Odd", "Web & MEDIA")]);
    assert!(sh.lay.card.is_none());
    assert!(in_group(&mut sh, "system").contains(&"Calc".to_string()));
    assert!(in_group(&mut sh, "apps").contains(&"Plain".to_string()));
    assert!(in_group(&mut sh, "web-media").contains(&"Odd".to_string()));
    // a group that does not exist: the card, not silently
    with_apps(&mut sh, &mut g, vec![entry("chess", "Chess", "Games"), entry("go", "Go", "games")]);
    assert!(matches!(&sh.lay.card, Some(Card::Ask { asked, .. }) if asked == "Games"));
    assert!(sh.layout.groups.is_empty(), "nothing is created before the person says so");
    assert!(in_group(&mut sh, "apps").contains(&"Chess".to_string()), "meanwhile it is in Apps");
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.label == "Create") && sh.hits().iter().any(|h| h.label == "Put it in Apps"));
    click(&mut sh, &mut g, "Create");
    assert!(sh.lay.card.is_none());
    assert_eq!(sh.layout.groups.len(), 1);
    let games = gid(&sh, "Games");
    assert!(in_group(&mut sh, &games).contains(&"Chess".to_string()));
    assert!(names(&sh).contains(&"Go".to_string()), "the second app finds the group by name: no second question");
    sh.render(&mut g);
    assert!(sh.lay.card.is_none());
    // the other answer, remembered per app
    with_apps(&mut sh, &mut g, vec![entry("tune", "Tune", "Music")]);
    assert!(matches!(&sh.lay.card, Some(Card::Ask { .. })));
    click(&mut sh, &mut g, "Put it in Apps");
    assert!(sh.layout.groups.len() == 1 && in_group(&mut sh, "apps").contains(&"Tune".to_string()));
    sh.render(&mut g);
    assert!(sh.lay.card.is_none(), "not asked again");
    sh.refresh_custom();
    assert!(sh.lay.card.is_none());
    // Esc is the safe answer too
    with_apps(&mut sh, &mut g, vec![entry("zed", "Zed", "Edits")]);
    key(&mut sh, Key::Esc);
    assert!(sh.layout.groups.len() == 1 && sh.lay.card.is_none());
    assert_eq!(sh.layout.app_groups["zed"].target, "apps");
    // the person's move wins over a changed `group` in an update
    with_apps(&mut sh, &mut g, vec![entry("chess", "Chess", "Games")]);
    let _ = in_group(&mut sh, &games);
    key(&mut sh, Key::Char('m'));
    pick(&mut sh, "to:system");
    with_apps(&mut sh, &mut g, vec![entry("chess", "Chess", "Work")]);
    assert!(in_group(&mut sh, "system").contains(&"Chess".to_string()), "an update naming Work does not move it back");
    with_apps(&mut sh, &mut g, vec![entry("chess", "Chess", "Brand new group")]);
    assert!(sh.lay.card.is_none(), "and a moved app is not asked about");
    assert!(in_group(&mut sh, "system").contains(&"Chess".to_string()));
    // a name that cannot be a group falls back to Apps, with the reason
    with_apps(&mut sh, &mut g, vec![entry("long", "Long", "A group name that is far too long")]);
    click(&mut sh, &mut g, "Create");
    assert!(in_group(&mut sh, "apps").contains(&"Long".to_string()));
    assert!(sh.toast_text().is_some_and(|t| t.contains("16")), "{:?}", sh.toast_text());
}

#[test]
fn deleting_the_group_an_app_asked_for_sends_it_to_apps_without_asking_again() {
    let (mut sh, mut g) = alice();
    with_apps(&mut sh, &mut g, vec![entry("chess", "Chess", "Games")]);
    click(&mut sh, &mut g, "Create");
    let games = gid(&sh, "Games");
    let _ = in_group(&mut sh, &games);
    key(&mut sh, Key::Char('g'));
    pick(&mut sh, "delete");
    assert!(matches!(sh.lay.card, Some(Card::Delete { n: 1, .. })));
    click(&mut sh, &mut g, "Delete group");
    sh.render(&mut g);
    assert!(sh.lay.card.is_none());
    assert!(in_group(&mut sh, "apps").contains(&"Chess".to_string()));
}

#[test]
fn tiles_a_person_cannot_see_are_never_shown_after_a_move_and_moving_changes_no_permission() {
    let (mut sh, mut g) = alice();
    // a layout file that puts the Users tile (admins only) in Home for a plain user
    let mut l = Layout::default();
    l.place("sys:users", "home", "system").unwrap();
    l.place("sys:nope", "home", "system").unwrap();
    l.save(sh.env.services.dir(), "alice");
    key(&mut sh, Key::Char('o'));
    login(&mut sh, "alice", "alicepass1");
    sh.render(&mut g);
    let mut all = Vec::new();
    for gr in sh.groups() {
        sh.set_group(gr);
        all.extend(names(&sh));
    }
    assert!(!all.contains(&"USERS".to_string()), "{all:?}");
    // an admin with the same placement does see it, where the file says
    key(&mut sh, Key::Char('o'));
    let mut l = Layout::default();
    l.place("sys:users", "home", "system").unwrap();
    l.save(sh.env.services.dir(), "root");
    login(&mut sh, "root", "rootpass12");
    sh.render(&mut g);
    assert!(in_group(&mut sh, "home").contains(&"USERS".to_string()));
    // a guest can move tiles but a locked tile stays locked: the tile is still refused
    key(&mut sh, Key::Char('o'));
    login(&mut sh, "gus", "guestpass1");
    sh.render(&mut g);
    let _ = in_group(&mut sh, "work");
    let i = names(&sh).iter().position(|n| n == "CLI").unwrap();
    sh.sel = i;
    key(&mut sh, Key::Char('m'));
    pick(&mut sh, "to:home");
    let _ = in_group(&mut sh, "home");
    let i = names(&sh).iter().position(|n| n == "CLI").unwrap();
    sh.sel = i;
    key(&mut sh, Key::Enter);
    assert_eq!(sh.screen_name(), "launcher");
    assert!(sh.toast_text().is_some_and(|t| t.contains("guest")), "{:?}", sh.toast_text());
}

#[test]
fn the_number_keys_follow_the_tile_table_whichever_group_a_tile_was_moved_to() {
    let (mut sh, _g) = alice();
    key(&mut sh, Key::Char('m'));
    pick(&mut sh, "to:system");
    assert!(!names(&sh).contains(&"CHAT".to_string()));
    key(&mut sh, Key::Char('1'));
    assert_eq!(sh.screen_name(), "app:chat", "1 still opens the first tile of the table");
}

#[test]
fn reordering_inside_a_group_with_brackets_is_remembered() {
    let (mut sh, _g) = alice();
    let before = names(&sh);
    key(&mut sh, Key::Char(']'));
    let after = names(&sh);
    assert_eq!((after[0].clone(), after[1].clone()), (before[1].clone(), before[0].clone()));
    assert_eq!(sh.sel, 1);
    key(&mut sh, Key::Char('['));
    assert_eq!(names(&sh), before);
    key(&mut sh, Key::Char(']'));
    key(&mut sh, Key::Char('u'));
    assert_eq!(names(&sh), before, "undo");
}

#[test]
fn the_chip_row_fits_and_reaches_every_group_at_five_sizes_with_many_groups() {
    for (w, h) in [(360u32, 740u32), (390, 844), (800, 480), (1280, 720), (1920, 1080)] {
        let (mut sh, _) = shell_with_users(&format!("lay-chips-{w}"));
        let mut g = Gfx::new(w, h);
        login(&mut sh, "alice", "alicepass1");
        for i in 0..12 {
            sh.layout.create_group(&format!("Group {i:02}")).unwrap();
        }
        sh.render(&mut g);
        for target in 0..sh.groups().len() {
            let gr = sh.groups()[target].clone();
            sh.set_group(gr.clone());
            sh.render(&mut g);
            let chips: Vec<_> = sh.hits().iter().filter(|h| h.label.starts_with("group ")).map(|h| (h.label.clone(), h.rect)).collect();
            assert!(chips.iter().any(|(l, _)| *l == format!("group {}", gr.label())), "{w}x{h}: the shown group {} has a chip", gr.label());
            for (l, r) in &chips {
                assert!(r.x >= 0.0 && r.x + r.w <= w as f32, "{w}x{h}: {l} {r:?}");
            }
            let plus = sh.hits().iter().find(|h| h.label == "new group").expect("the + chip").rect;
            assert!(plus.x + plus.w <= w as f32, "{w}x{h}: + fits");
            let hidden = sh.groups().len() - chips.len();
            assert_eq!(sh.hits().iter().any(|h| h.label == "more groups"), hidden > 0, "{w}x{h}: … only when something is hidden");
        }
        // a hidden group is reached through the … chip
        sh.set_group(Group::Home);
        sh.render(&mut g);
        if sh.hits().iter().any(|h| h.label == "more groups") {
            click(&mut sh, &mut g, "more groups");
            let ids = menu_ids(&sh);
            assert!(!ids.is_empty() && ids.iter().all(|i| i.starts_with("show:")), "{w}x{h}: {ids:?}");
            let last = ids.last().unwrap().clone();
            pick(&mut sh, &last);
            assert_eq!(format!("show:{}", sh.current_group().id), last, "{w}x{h}");
            sh.render(&mut g);
            assert!(sh.hits().iter().any(|h| h.label == format!("group {}", sh.current_group().label())));
        } else {
            assert!(w >= 1280, "{w}x{h}: 16 groups should not fit a small screen");
        }
    }
}

#[test]
fn the_help_overlay_and_the_hint_line_mention_moving_tiles() {
    let (sh, _g) = alice();
    let (_, lines) = sh.help_lines();
    let all = lines.join("\n");
    assert!(all.contains("m · move the selected tile") && all.contains("+ chip") && all.contains("u · undo"), "{all}");
    assert!(all.contains("Ask & Create, Build, Media, Data, Devices & System, Apps"));
}

#[test]
fn the_default_layout_draws_the_five_chips_where_chip_rects_puts_them() {
    let (sh, g) = alice();
    let labels: Vec<String> = sh.groups().iter().map(|g| g.label().to_string()).collect();
    assert_eq!(labels, vec!["Ask & Create", "Build", "Media", "Data", "Devices & System"], "Apps shows with the first installed tile");
    let l: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    let want = launcher::chip_rects(scale_for(g.width(), g.height()), &l);
    let got: Vec<R> = sh.chip_row().chips.iter().map(|(_, r)| *r).collect();
    assert_eq!(got, want, "the Tab order and the default look are unchanged by the + chip");
    assert!(sh.layout.is_default());
}

// ---------------------------------------------------------------- default groups, Apps, reset, persistence, sizes

#[test]
fn the_default_grouping_covers_every_tile_exactly_once_and_apps_is_empty_until_something_is_installed() {
    let (mut sh, _) = shell_with_users("lay-default");
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "root", "rootpass12");
    sh.render(&mut g);
    assert!(sh.layout.is_default());
    let mut seen: Vec<String> = Vec::new();
    for gr in sh.groups() {
        seen.extend(in_group(&mut sh, &gr.id));
    }
    let mut want: Vec<String> = TILES.iter().filter(|t| t.group.is_some()).map(|t| t.label.to_string()).collect();
    seen.sort();
    want.sort();
    assert_eq!(seen, want, "every tile in exactly one group");
    assert!(!seen.contains(&"SETTINGS".to_string()));
    assert_eq!(sh.groups().iter().map(|x| x.label().to_string()).collect::<Vec<_>>(), ["Ask & Create", "Build", "Media", "Data", "Devices & System"]);
    assert_eq!(in_group(&mut sh, "home"), ["CHAT", "RAG", "SKILLS", "CALENDAR", "TALES", "VOICE", "LAB", "LEARN"]);
    for l in ["AI IDE", "GIT", "FLOWS", "K8S", "EDIT"] {
        assert!(in_group(&mut sh, "work").contains(&l.to_string()), "{l} is in Build");
    }
    // the same for every role: each tile they have is in one group
    for who in [("alice", "alicepass1"), ("gus", "guestpass1")] {
        key(&mut sh, Key::Char('o'));
        login(&mut sh, who.0, who.1);
        sh.render(&mut g);
        let mut seen: Vec<String> = Vec::new();
        for gr in sh.groups() {
            seen.extend(in_group(&mut sh, &gr.id));
        }
        let mut want: Vec<String> = sh.visible_tiles().iter().map(|t| t.label.to_string()).collect();
        seen.sort();
        want.sort();
        assert_eq!(seen, want, "{who:?}");
    }
}

#[test]
fn a_tile_installed_later_lands_in_apps_without_a_restart_and_a_named_group_wins() {
    let (mut sh, mut g) = alice();
    assert!(!sh.groups().iter().any(|x| x.id == "apps"), "no Apps tab yet");
    with_apps(&mut sh, &mut g, vec![entry("calc", "Calc", "")]);
    let apps = sh.groups().into_iter().find(|x| x.id == "apps").expect("the Apps group appears by itself");
    assert_eq!(apps.label(), "Apps");
    assert_eq!(in_group(&mut sh, "apps"), ["Calc"]);
    assert!(sh.lay.card.is_none(), "no question: Apps is the default");
    // a second tile arrives while the shell runs
    with_apps(&mut sh, &mut g, vec![entry("calc", "Calc", ""), entry("clock", "Clock", "")]);
    assert_eq!(in_group(&mut sh, "apps"), ["Calc", "Clock"]);
    // an app that names a built-in group goes there instead
    with_apps(&mut sh, &mut g, vec![entry("calc", "Calc", ""), entry("tbl", "Tbl", "Data")]);
    assert!(in_group(&mut sh, "data").contains(&"Tbl".to_string()));
    assert_eq!(in_group(&mut sh, "apps"), ["Calc"]);
    // removing the last one hides the tab again; the person can move a tile out of Apps and back
    sh.set_group(Group::Home);
    key(&mut sh, Key::Char('m'));
    pick(&mut sh, "to:apps");
    assert!(in_group(&mut sh, "apps").contains(&"CHAT".to_string()));
    with_apps(&mut sh, &mut g, vec![]);
    assert!(sh.groups().iter().any(|x| x.id == "apps"), "CHAT is still there");
}

#[test]
fn the_arrangement_is_saved_per_person_and_survives_a_restart_and_reset_to_default_puts_the_tiles_back() {
    let (mut sh, mut g) = alice();
    click(&mut sh, &mut g, "new group");
    typ(&mut sh, "Daily");
    key(&mut sh, Key::Enter);
    let daily = gid(&sh, "Daily");
    sh.set_group(Group::Home);
    key(&mut sh, Key::Char('m'));
    pick(&mut sh, &format!("to:{daily}"));
    // rename it
    let _ = in_group(&mut sh, &daily);
    key(&mut sh, Key::Char('g'));
    pick(&mut sh, "rename");
    for _ in 0..5 {
        key(&mut sh, Key::Backspace);
    }
    typ(&mut sh, "Routine");
    key(&mut sh, Key::Enter);
    assert_eq!(gid(&sh, "Routine"), daily);
    let on_disk = saved(&sh, "alice");
    assert_eq!(on_disk.groups.len(), 1);
    assert_eq!(on_disk.groups[0].label, "Routine");
    assert_eq!(on_disk.placed.get("sys:chat"), Some(&daily));
    // a restart: sign out and in again reads the file
    key(&mut sh, Key::Char('o'));
    login(&mut sh, "alice", "alicepass1");
    sh.render(&mut g);
    assert_eq!(in_group(&mut sh, &daily), ["CHAT"]);
    assert_eq!(gid(&sh, "Routine"), daily);
    // Reset to default layout: the tile goes home (the group stays), and the menu says so
    let _ = in_group(&mut sh, &daily);
    key(&mut sh, Key::Char('g'));
    assert!(sh.lay.menu.as_ref().unwrap().1.items.iter().any(|i| i.id == "reset-all" && i.label == "Reset to default layout"));
    pick(&mut sh, "reset-all");
    assert!(saved(&sh, "alice").placed.is_empty());
    assert!(in_group(&mut sh, "home").contains(&"CHAT".to_string()));
    // delete the group: nothing disappears
    let _ = in_group(&mut sh, &daily);
    key(&mut sh, Key::Char('g'));
    pick(&mut sh, "delete");
    click(&mut sh, &mut g, "Delete group");
    assert!(saved(&sh, "alice").groups.is_empty());
    assert!(in_group(&mut sh, "home").contains(&"CHAT".to_string()));
    // another person is untouched
    assert!(saved(&sh, "root").is_default());
}

#[test]
fn the_chips_and_tiles_fit_four_screen_sizes_with_six_built_in_groups_and_ones_of_the_persons_own() {
    for (w, h) in [(1280u32, 720u32), (1024, 600), (800, 480), (480, 600)] {
        let (mut sh, _) = shell_with_users(&format!("lay-size-{w}"));
        login(&mut sh, "root", "rootpass12");
        sh.resize_for_test(w, h);
        let mut g = Gfx::new(w, h);
        sh.custom = vec![entry("calc", "Calc", "")];
        sh.custom_at = sh.tick.max(1);
        sh.layout.create_group("Daily").unwrap();
        sh.layout.create_group("Games").unwrap();
        let s = scale_for(w, h);
        for gr in sh.groups() {
            sh.set_group(gr.clone());
            sh.render(&mut g);
            let row = sh.chip_row();
            let mut chips: Vec<R> = row.chips.iter().map(|(_, r)| *r).collect();
            chips.extend(row.more);
            chips.push(row.plus);
            for (i, a) in chips.iter().enumerate() {
                assert!(a.x >= 0.0 && a.x + a.w <= w as f32, "{w}x{h} {}: chip {a:?}", gr.id);
                assert!(chips[i + 1..].iter().all(|b| a.x + a.w <= b.x + 0.5), "{w}x{h}: chips overlap");
            }
            assert!(row.chips.iter().any(|(i, _)| sh.groups()[*i] == gr), "{w}x{h}: the shown group has a chip");
            let n = sh.group_items().len();
            let cap = launcher::per_page(w, h, s);
            for p in 0..launcher::page_count(n, cap) {
                let rs = launcher::page_rects(w, h, s, n, p);
                for (i, a) in rs.iter().enumerate() {
                    assert!(a.x >= 0.0 && a.x + a.w <= w as f32 && a.y + a.h <= h as f32, "{w}x{h} {}: tile {a:?}", gr.id);
                    assert!(chips.iter().all(|c| c.y + c.h <= a.y), "{w}x{h}: a tile under the chips");
                    assert!(rs[i + 1..].iter().all(|b| a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5 || a.y + a.h <= b.y || b.y + b.h <= a.y), "{w}x{h}: tiles overlap");
                }
            }
        }
    }
}
