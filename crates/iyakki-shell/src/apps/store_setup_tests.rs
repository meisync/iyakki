use super::*;
use crate::apps::store::{Section, StoreApp};
use crate::apps::{App, AppCx};
use iyakki_sec::policy::Role;

fn cx(tag: &str, role: Role) -> AppCx {
    super::super::tests::cx(tag, role)
}

fn full_machine() -> Machine {
    Machine { model: "MacBookPro14,1".into(), gpus: 1, nvidia: true, ram_mb: 16_000, camera: true, fw: Item::ALL.into_iter().filter(|i| i.fw_def().is_some() || *i == Item::FwHardware).collect(), ..Default::default() }
}

/// A PC with nothing special: no hardware tile, nothing recommended once k3s is there.
fn plain_machine() -> Machine {
    Machine { ram_mb: 16_000, ..Default::default() }
}

/// Facts from a fake probe: everything missing except what is listed.
fn facts(some: &[(Item, Presence)]) -> Facts {
    let mut f = Facts::default();
    for it in Item::ALL {
        f.presence.push((it, some.iter().find(|(x, _)| *x == it).map(|(_, p)| p.clone()).unwrap_or(Presence::Missing)));
    }
    f
}

fn rig(tag: &str, role: Role, m: Machine, f: Facts) -> (StoreApp, AppCx) {
    let cx = cx(tag, role);
    let mut a = StoreApp::new(&cx);
    a.setup.set_machine(m);
    a.setup.set_facts(f);
    a.setup.launch = Some(qi::Launch { exe: "/usr/bin/true".into(), data: cx.services.dir().join("qi") });
    a.handle(&Event::Tab(4), &cx);
    assert!((a.section == Section::Setup), "the Setup tab opens by its tab");
    // the first drawing does this; do it now, so what a test sets afterwards (jobs, failed, signed in) is kept
    a.setup.enter(&cx);
    (a, cx)
}

fn draw_in(a: &mut StoreApp, cx: &AppCx, area: R, w: u32, h: u32) -> Gfx {
    let s = crate::shell::scale_for(w, h);
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    a.render(&mut g, area, s, cx, 3);
    g
}

fn draw(a: &mut StoreApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let s = crate::shell::scale_for(w, h);
    let hh = 46.0 * s;
    draw_in(a, cx, R::new(0.0, hh, w as f32, h as f32 - hh), w, h)
}

fn key(a: &mut StoreApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn idx(i: Item) -> usize {
    Item::ALL.iter().position(|x| *x == i).unwrap()
}

/// Open the popup of `item` and press its primary button.
fn primary(a: &mut StoreApp, cx: &AppCx, item: Item) {
    a.setup.open_popup(item);
    a.handle(&Event::Tab(PRIMARY + idx(item)), cx);
}

fn titles(a: &StoreApp) -> Vec<&'static str> {
    a.setup.sections().iter().map(|s| s.gk.title()).collect()
}

const SIZES: [(u32, u32); 6] = [(360, 740), (800, 480), (1280, 720), (1920, 1080), (2560, 1600), (2880, 1800)];

fn job(item: Item, state: iyakki_agent::jobs::State, progress: Option<f32>) -> Job {
    Job {
        id: "j1".into(),
        label: format!("Install {}", item.title()),
        command: format!("/usr/bin/iyakki quick-install {} --data /d", item.id()),
        cwd: "/d".into(),
        owner: "root".into(),
        workspace: qi::WORKSPACE.into(),
        pid: 1,
        state,
        started_ms: 10,
        ended_ms: 0,
        progress,
        last_line: String::new(),
        last_active_ms: 0,
        dismissed: false,
    }
}

fn hit_inside(g: &Gfx, area: R, what: &str) {
    for h in &g.hits {
        let r = h.rect;
        assert!(r.x >= area.x - 1.0 && r.y >= area.y - 1.0 && r.x + r.w <= area.x + area.w + 1.0 && r.y + r.h <= area.y + area.h + 1.0, "{what}: `{}` sticks out of {area:?}: {r:?}", h.label);
    }
}

fn has_hit(g: &Gfx, ev: Event) -> bool {
    g.hits.iter().any(|h| h.ev == ev)
}

fn labels(g: &Gfx) -> Vec<String> {
    g.hits.iter().map(|h| h.label.clone()).collect()
}

/// Where a tile is on the screen now (after a drawing).
fn tile_rect(a: &StoreApp, item: Item) -> R {
    let v = a.setup.list.view;
    let (_, r) = a.setup.spots.iter().find(|(i, _)| *i == item).copied().expect("the tile is laid out");
    R::new(v.x + r.x, v.y + r.y - a.setup.list.offset, r.w, r.h)
}

/// How many pixels of `r` are (nearly) this colour.
fn count(g: &Gfx, r: R, c: [u8; 4], tol: i32) -> usize {
    let (w, h) = (g.width() as i32, g.height() as i32);
    let mut n = 0;
    for y in (r.y.max(0.0) as i32)..((r.y + r.h).min(h as f32) as i32) {
        for x in (r.x.max(0.0) as i32)..((r.x + r.w).min(w as f32) as i32) {
            let o = ((y * w + x) * 4) as usize;
            let p = &g.rgba()[o..o + 3];
            if (0..3).all(|k| (p[k] as i32 - c[k] as i32).abs() <= tol) {
                n += 1;
            }
        }
    }
    n
}

fn top_right(r: R) -> R {
    R::new(r.x + r.w * 0.55, r.y, r.w * 0.45, r.h * 0.34)
}

fn bottom_left(r: R) -> R {
    R::new(r.x, r.y + r.h * 0.7, r.w * 0.5, r.h * 0.3)
}

fn bottom_right(r: R) -> R {
    R::new(r.x + r.w * 0.8, r.y + r.h * 0.7, r.w * 0.2, r.h * 0.3)
}

/// A mixed machine: every state shows up somewhere.
fn mixed(tag: &str, role: Role) -> (StoreApp, AppCx) {
    let f = facts(&[
        (Item::K3s, Presence::Installed("k3s v1.31.4+k3s1 · running".into())),
        (Item::Node, Presence::Installed("version 26.1.0".into())),
        (Item::Python, Presence::Elsewhere("/usr/bin/python3".into())),
        (Item::Lemonade, Presence::Installed("answering on 127.0.0.1:13305 (2 model(s))".into())),
        (Item::Kokoro, Presence::Installed("kokoro-v1 downloaded in Lemonade".into())),
        (Item::ClaudeCode, Presence::Installed("version 2.1.7".into())),
        (Item::Copilot, Presence::Installed("version 0.0.400".into())),
        (Item::DuckDb, Presence::Earlier("cluster down".into())),
        (Item::Ffmpeg, Presence::Installed("version 7.1".into())),
    ]);
    let (mut a, cx) = rig(tag, role, full_machine(), f);
    a.setup.latest = vec![(Item::ClaudeCode, qi::Latest::Newer("v2.2.0".into()))];
    a.setup.failed = vec![Item::Registry];
    a.setup.jobs = vec![job(Item::Elasticsearch, iyakki_agent::jobs::State::Running, Some(40.0))];
    (a, cx)
}

// ------------------------------------------------------------------------------------------------ the missing k3s tile

#[test]
fn an_installed_and_running_k3s_has_its_tile_with_a_tick_and_running_at_every_size() {
    // The owner's report: k3s installed and running, but not to be seen in Setup. The old page listed "Recommended" (what is missing)
    // open and every other group folded: with k3s there, nothing was recommended, so all groups were folded and k3s sat inside the
    // folded "Get started". Now no category folds: every tile is on the page.
    let f = facts(&[(Item::K3s, Presence::Installed("k3s v1.31.4+k3s1 · running".into()))]);
    for (w, h) in SIZES {
        let (mut a, cx) = rig(&format!("su-k3s-{w}"), Role::Admin, plain_machine(), f.clone());
        assert!(a.setup.recommended(Item::K3s).is_none(), "nothing recommended is missing, so the old default page had no open group");
        let g = draw(&mut a, &cx, w, h);
        assert!(has_hit(&g, Event::Tab(TILE + idx(Item::K3s))), "{w}x{h}: the k3s tile is on the first screen");
        let c = a.setup.card(Item::K3s);
        assert_eq!((c.chip.as_str(), c.running, c.version.as_str()), ("Running", Some(true), "v1.31.4+k3s1"));
        assert_eq!(c.state, CardState::Installed);
        let t = tile_rect(&a, Item::K3s);
        assert!(count(&g, top_right(t), OK, 10) > 20, "{w}x{h}: the tick is drawn");
        assert!(count(&g, bottom_left(t), OK, 10) > 20, "{w}x{h}: the green dot and the word Running are drawn");
        assert_eq!(view::status_text(&c).0, "Running · v1.31.4+k3s1");
    }
}

#[test]
fn the_page_has_every_item_once_and_no_folded_headings() {
    let (a, _cx) = rig("su-groups", Role::Admin, plain_machine(), facts(&[]));
    assert_eq!(titles(&a), vec!["Get started", "AI servers and models", "Voices", "Coding agents", "Data services", "Media tools", "Linux apps"], "no Hardware category: nothing in it is shown here");
    let (mac, _cx2) = rig("su-groups-mac", Role::Admin, Machine { model: "Macmini8,1".into(), ram_mb: 16_000, ..Default::default() }, facts(&[]));
    assert!(titles(&mac).contains(&"Hardware"), "a Mac has its firmware tile");
    let secs = a.setup.sections();
    assert_eq!(secs[0].items, vec![Item::Node, Item::Python, Item::K3s], "k3s is in its own category even though it is recommended (it carries a star)");
    assert!(a.setup.recommended(Item::K3s).is_some());
    let mut all: Vec<Item> = secs.iter().flat_map(|s| s.items.clone()).collect();
    all.sort();
    let mut want = a.setup.eligible();
    want.sort();
    assert_eq!(all, want, "every item exactly once");
    assert_eq!(a.setup.stops().len(), want.len(), "the keyboard reaches every tile");
    assert_eq!(a.setup.categories().len(), 7);
}

#[test]
fn hardware_tiles_are_shown_only_with_the_hardware_and_never_in_cloud_mode() {
    let none = Machine { ram_mb: 16_000, ..Default::default() };
    let (a, _cx) = rig("su-hw1", Role::Admin, none.clone(), facts(&[]));
    let e = a.setup.eligible();
    assert!(!e.contains(&Item::GpuVulkan) && !e.contains(&Item::GpuNvidia) && !e.contains(&Item::MacFirmware));
    let (a, _cx) = rig("su-hw2", Role::Admin, Machine { gpus: 1, ..none.clone() }, facts(&[]));
    assert!(a.setup.eligible().contains(&Item::GpuVulkan) && !a.setup.eligible().contains(&Item::GpuNvidia));
    let (a, _cx) = rig("su-hw3", Role::Admin, Machine { gpus: 1, nvidia: true, model: "iMac18,3".into(), ..none.clone() }, facts(&[]));
    assert!(a.setup.eligible().contains(&Item::GpuNvidia) && a.setup.eligible().contains(&Item::MacFirmware));
    assert!(titles(&a).contains(&"Hardware"));
    assert!(a.setup.recommended(Item::GpuNvidia).is_some(), "a missing hardware item is recommended: its tile has the star");
    let (a, _cx) = rig("su-hw4", Role::Admin, Machine { cloud: true, ..full_machine() }, facts(&[]));
    assert!(a.setup.eligible().iter().all(|i| !i.is_hardware()), "cloud mode hides the hardware tiles");
    assert!(!titles(&a).contains(&"Hardware"));
    // a popup cannot be opened for a tile this machine does not show
    let (mut a, _cx) = rig("su-hw5", Role::Admin, Machine { cloud: true, ..full_machine() }, facts(&[]));
    a.setup.open_popup(Item::MacFirmware);
    assert!(a.setup.popup.is_none());
}

#[test]
fn every_item_has_a_tile_and_each_state_has_its_chip_and_button() {
    use iyakki_agent::jobs::State;
    let f = facts(&[
        (Item::Node, Presence::Installed("v26".into())),
        (Item::K3s, Presence::Installed("v1.31".into())),
        (Item::DuckDb, Presence::Earlier("cluster down".into())),
        (Item::Python, Presence::Elsewhere("/usr/bin/python3".into())),
        (Item::Elasticsearch, Presence::Elsewhere("other cluster".into())),
        (Item::Ollama, Presence::Installed("0.5".into())),
    ]);
    let (mut a, _cx) = rig("su-cards", Role::Admin, full_machine(), f);
    assert_eq!(a.setup.eligible().len(), Item::ALL.len(), "a Mac with an NVIDIA GPU shows every item");
    for it in Item::ALL {
        let c = a.setup.card(it);
        assert_eq!(c.item, it);
        assert!(!c.chip.is_empty() && !c.label.is_empty(), "{it:?}");
        assert!(!icon_label(it).is_empty());
    }
    let chip = |a: &StoreApp, it: Item| a.setup.card(it).chip;
    assert_eq!(chip(&a, Item::Registry), "Not installed");
    assert_eq!(chip(&a, Item::Node), "Installed");
    assert_eq!(chip(&a, Item::K3s), "Running", "a service that is up says Running");
    assert_eq!(chip(&a, Item::Ollama), "Installed", "nothing tells whether Ollama's server is up: only installed");
    assert_eq!(chip(&a, Item::DuckDb), "Installed earlier, not running");
    assert_eq!(chip(&a, Item::Elasticsearch), "Found on this machine");
    assert_eq!(a.setup.card(Item::Registry).act, Act::Install);
    assert_eq!(a.setup.card(Item::Node).act, Act::Open);
    assert_eq!(a.setup.card(Item::DuckDb).act, Act::Reinstall);
    assert_eq!(a.setup.card(Item::Python).act, Act::OwnCopy, "something found by hand: own copy is a choice");
    assert_eq!(a.setup.card(Item::K3s).act, Act::Open, "a running cluster opens the K8S tile");
    assert_eq!(open_target(Item::K3s), Some(("k8s", "")));
    // the cluster stopped: Start (the supervisor), starting: no button yet
    a.setup.live_k3s = Some(Svc::Stopped);
    let c = a.setup.card(Item::K3s);
    assert_eq!((c.chip.as_str(), c.act, c.label.as_str(), c.running), ("Stopped", Act::Start, "Start", Some(false)));
    a.setup.live_k3s = Some(Svc::Starting);
    let c = a.setup.card(Item::K3s);
    assert_eq!((c.chip.as_str(), c.act), ("Starting…", Act::None));
    a.setup.live_k3s = None;
    // failed, and the retry
    a.setup.failed = vec![Item::Registry];
    let c = a.setup.card(Item::Registry);
    assert_eq!((c.chip.as_str(), c.act, c.label.as_str()), ("Failed", Act::Retry, "Retry"));
    // installing with progress, with the log as its button
    a.setup.jobs = vec![job(Item::Copilot, State::Running, Some(40.0))];
    let c = a.setup.card(Item::Copilot);
    assert_eq!((c.chip.as_str(), c.act), ("Installing 40 %", Act::Log));
    a.setup.jobs = vec![job(Item::Copilot, State::Running, None)];
    assert_eq!(a.setup.card(Item::Copilot).chip, "Installing…");
    // installed but not runnable
    let mut f2 = facts(&[(Item::DuckDb, Presence::Installed("v1".into()))]);
    f2.unhealthy = vec![(Item::DuckDb, "no such library".into())];
    a.setup.set_facts(f2);
    a.setup.jobs.clear();
    let c = a.setup.card(Item::DuckDb);
    assert_eq!(c.chip, "Installed, not runnable");
    assert!(c.notes.iter().any(|n| n.0.contains("no such library") && n.1));
    assert_eq!(c.label, "Reinstall (repair)");
    // before the facts arrive
    a.setup.facts = None;
    assert_eq!(a.setup.card(Item::Node).chip, "Checking…");
    assert_eq!(a.setup.card(Item::Node).act, Act::None);
}

fn icon_label(i: Item) -> &'static str {
    // every item has a badge picture and a category colour
    let _ = (icon_of(i), Gk::of(i).accent());
    i.title()
}

#[test]
fn versions_are_read_from_the_presence_line() {
    assert_eq!(version_of("k3s v1.31.4+k3s1 · running"), "v1.31.4+k3s1");
    assert_eq!(version_of("version 0.5"), "v0.5");
    assert_eq!(version_of("v26"), "v26");
    assert_eq!(version_of("answering on 127.0.0.1:13305 (3 model(s))"), "", "an address is not a version");
    assert_eq!(version_of(""), "");
    assert_eq!(version_of("installed by Quick install"), "");
}

#[test]
fn recommendations_follow_the_machine() {
    let none = |_: Item| false;
    let rec = |m: &Machine, present: &dyn Fn(Item) -> bool| -> Vec<Item> { Item::ALL.into_iter().filter(|i| m.recommend(*i, present).is_some()).collect() };
    let mac = Machine { model: "Macmini8,1".into(), ram_mb: 8192, ..Default::default() };
    assert_eq!(rec(&mac, &none), vec![Item::K3s, Item::MacFirmware]);
    let pc = Machine { gpus: 1, nvidia: true, ram_mb: 32_000, ..Default::default() };
    assert_eq!(rec(&pc, &none), vec![Item::K3s, Item::GpuVulkan, Item::GpuNvidia]);
    assert_eq!(pc.recommend(Item::K3s, &none), Some("Needed by Lemonade, Kokoro, DuckDB and Elasticsearch"));
    assert_eq!(rec(&pc, &|i| i == Item::K3s), vec![Item::GpuVulkan, Item::GpuNvidia]);
    let small = Machine { ram_mb: 4096, ..pc.clone() };
    assert_eq!(rec(&small, &none), vec![Item::K3s]);
    assert!(small.heavy_note(Item::Lemonade).unwrap().contains("not recommended"));
    assert!(small.heavy_note(Item::Node).is_none());
    let cloud = Machine { cloud: true, ..pc.clone() };
    assert_eq!(rec(&cloud, &none), vec![Item::K3s]);
    let win = Machine { window: true, ..pc.clone() };
    assert!(!rec(&win, &none).contains(&Item::K3s));
    let (a, _cx) = rig("su-rec", Role::Admin, pc, facts(&[]));
    assert_eq!(a.setup.card(Item::K3s).reason, Some("Needed by Lemonade, Kokoro, DuckDB and Elasticsearch"));
    assert_eq!(a.setup.card(Item::Node).reason, None);
}

#[test]
fn dependencies_are_said_on_the_tile_and_in_the_popup() {
    let (a, _cx) = rig("su-deps", Role::Admin, full_machine(), facts(&[]));
    assert_eq!(a.setup.card(Item::Elasticsearch).deps.as_deref(), Some("Installs k3s first"));
    assert_eq!(a.setup.card(Item::Kokoro).deps.as_deref(), Some("Installs k3s, Lemonade first"));
    assert_eq!(a.setup.card(Item::K3s).deps, None);
    let (a, _cx) = rig("su-deps2", Role::Admin, full_machine(), facts(&[(Item::K3s, Presence::Installed("v1".into()))]));
    assert_eq!(a.setup.card(Item::Elasticsearch).deps, None, "k3s is there");
    assert_eq!(a.setup.card(Item::Kokoro).deps.as_deref(), Some("Installs Lemonade first"));
    // what depends on k3s
    let by: Vec<Item> = a.setup.needed_by(Item::K3s);
    assert!(by.contains(&Item::Lemonade) && by.contains(&Item::DuckDb) && by.contains(&Item::Elasticsearch) && !by.contains(&Item::Node));
}

fn rows_of(a: &StoreApp, cx: &AppCx, item: Item) -> Vec<(String, String)> {
    let mut g = Gfx::new(1280, 720);
    let card = a.setup.card(item);
    let _ = cx;
    a.setup
        .popup_body(&mut g, &card, a.setup.details.contains(&item), 560.0, 1.0)
        .into_iter()
        .filter_map(|l| match l {
            view::Line::Row(k, v, _) => Some((k, v.join(" "))),
            _ => None,
        })
        .collect()
}

#[test]
fn the_popup_says_what_it_is_size_licence_place_needs_and_memory() {
    let (mut a, cx) = rig("su-pop-body", Role::Admin, full_machine(), facts(&[]));
    let rows = rows_of(&a, &cx, Item::Elasticsearch);
    let get = |k: &str| rows.iter().find(|r| r.0 == k).map(|r| r.1.clone());
    assert_eq!(get("Size and time"), Some(size_text(Item::Elasticsearch)));
    assert_eq!(get("Licence"), Some(Item::Elasticsearch.licence().to_string()));
    assert_eq!(get("Where it goes"), Some(where_text(Item::Elasticsearch).to_string()));
    assert_eq!(get("Needs").as_deref(), Some("Installs k3s first"));
    assert!(get("Memory").is_some_and(|m| m.contains("GB")));
    let rows = rows_of(&a, &cx, Item::K3s);
    assert!(rows.iter().any(|r| r.0 == "Needed by" && r.1.contains("Lemonade") && r.1.contains("DuckDB")), "{rows:?}");
    assert!(rows.iter().any(|r| r.0 == "Licence" && r.1.contains("Apache-2.0")));
    // with k3s there, "Needs" says it is already there
    let (a2, cx2) = rig("su-pop-body2", Role::Admin, full_machine(), facts(&[(Item::K3s, Presence::Installed("v1".into()))]));
    let rows = rows_of(&a2, &cx2, Item::Elasticsearch);
    assert!(rows.iter().any(|r| r.0 == "Needs" && r.1.contains("already on this machine")), "{rows:?}");
    // Details adds the technical lines
    a.setup.details.insert(Item::K3s);
    let rows = rows_of(&a, &cx, Item::K3s);
    assert!(rows.iter().any(|r| r.0 == "Item id" && r.1 == "k3s") && rows.iter().any(|r| r.0 == "Same as typing" && r.1 == "iyakki quick-install k3s"), "{rows:?}");
}

#[test]
fn the_popup_status_line_and_the_one_button_for_each_state() {
    use iyakki_agent::jobs::State;
    // (facts, what to set, item, status line, primary label)
    let mut cases: Vec<(&str, Vec<(Item, Presence)>, Item, &str, &str)> = vec![
        ("missing", vec![], Item::Registry, "Not installed", "Install"),
        ("installed", vec![(Item::Node, Presence::Installed("v26".into()))], Item::Node, "Installed v26", "Open"),
        ("running", vec![(Item::K3s, Presence::Installed("k3s v1.31.4 · running".into()))], Item::K3s, "Installed v1.31.4 · Running", "Open"),
        ("stopped", vec![(Item::K3s, Presence::Installed("k3s v1.31.4 · stopped".into()))], Item::K3s, "Installed v1.31.4 · Stopped", "Start"),
        ("earlier", vec![(Item::DuckDb, Presence::Earlier("cluster down".into()))], Item::DuckDb, "Installed earlier, not running", "Reinstall"),
        ("elsewhere", vec![(Item::Python, Presence::Elsewhere("/usr/bin/python3".into()))], Item::Python, "Found on this machine", "Install own copy"),
        ("guide", vec![], Item::MacHost, "Guide", "Open guide"),
        ("camera", vec![], Item::MacCamera, "Not installed", "Install camera driver and firmware"),
        ("service", vec![(Item::Registry, Presence::Installed("deployed".into()))], Item::Registry, "Installed · Running", "Reinstall"),
    ];
    for (name, f, item, line, label) in cases.drain(..) {
        let (mut a, cx) = rig(&format!("su-st-{name}"), Role::Admin, full_machine(), facts(&f));
        a.setup.open_popup(item);
        let g = draw(&mut a, &cx, 1280, 720);
        let c = a.setup.card(item);
        assert_eq!(view::status_line(&c).0, line, "{name}");
        let b = g.hits.iter().find(|h| h.ev == Event::Tab(PRIMARY + idx(item))).unwrap_or_else(|| panic!("{name}: the primary button"));
        assert_eq!(b.label, label, "{name}");
        assert!(has_hit(&g, Event::Tab(POP_CLOSE)) && labels(&g).contains(&"View log".to_string()) && labels(&g).contains(&"Details".to_string()), "{name}");
    }
    // an upgrade
    let (mut a, cx) = rig("su-st-up", Role::Admin, full_machine(), facts(&[(Item::DuckDb, Presence::Installed("v1.1.0".into()))]));
    a.setup.latest = vec![(Item::DuckDb, qi::Latest::Newer("v1.2.0".into()))];
    a.setup.open_popup(Item::DuckDb);
    let g = draw(&mut a, &cx, 1280, 720);
    let c = a.setup.card(Item::DuckDb);
    assert_eq!(view::status_line(&c), ("Installed v1.1.0 · Update available v1.2.0".to_string(), AMBER));
    assert!(labels(&g).contains(&"Upgrade to 1.2.0".to_string()) && labels(&g).contains(&"Open".to_string()), "Upgrade is the primary one, Open stays beside it: {:?}", labels(&g));
    // failed and installing
    a.setup.latest.clear();
    a.setup.failed = vec![Item::DuckDb];
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(labels(&g).contains(&"Retry".to_string()));
    a.setup.failed.clear();
    a.setup.jobs = vec![job(Item::DuckDb, State::Running, Some(30.0))];
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(PRIMARY + idx(Item::DuckDb)) && h.label == "View log"));
    // signed out coding agent
    let (mut a, cx) = rig("su-st-sign", Role::Admin, full_machine(), facts(&[(Item::ClaudeCode, Presence::Installed("2.1.7".into()))]));
    a.setup.signin = vec![(Item::ClaudeCode, crate::agent_login::State::SignedOut)];
    a.setup.open_popup(Item::ClaudeCode);
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(labels(&g).contains(&"Sign in".to_string()));
}

#[test]
fn every_state_marker_is_drawn_on_the_tile() {
    let (mut a, cx) = mixed("su-markers", Role::Admin); // tall frame: with the newer items the Registry tile is below 1080 px
    let g = draw(&mut a, &cx, 1920, 2400);
    let t = |it: Item| tile_rect(&a, it);
    // installed: the tick; missing: none
    assert!(count(&g, top_right(t(Item::Node)), OK, 10) > 20, "tick on an installed tool");
    assert_eq!(count(&g, top_right(t(Item::Registry)), OK, 10) + count(&g, top_right(t(Item::MacHost)), OK, 10), 0, "no tick on a missing one");
    // running: the green dot in the status row, more than a plain installed tool's first letter
    assert!(count(&g, bottom_left(t(Item::K3s)), OK, 10) > count(&g, R::new(t(Item::Node).x, t(Item::Node).y + t(Item::Node).h * 0.7, 12.0, t(Item::Node).h * 0.3), OK, 10) + 10);
    // update available: the amber pill (the up arrow and UPDATE)
    assert!(count(&g, top_right(t(Item::ClaudeCode)), AMBER, 8) > 40, "the Update pill");
    assert_eq!(count(&g, top_right(t(Item::Copilot)), AMBER, 8), 0, "no pill without a newer version");
    assert_eq!(a.setup.card(Item::ClaudeCode).update.as_deref(), Some("2.2.0"));
    assert_eq!(view::status_text(&a.setup.card(Item::ClaudeCode)).0, "Update to 2.2.0");
    // failed: the red warning
    assert!(count(&g, top_right(t(Item::Registry)), ERR, 10) > 20, "the warning on a failed install");
    // earlier and elsewhere: an amber ring
    assert!(count(&g, top_right(t(Item::DuckDb)), AMBER, 8) > 10 && count(&g, top_right(t(Item::Python)), AMBER, 8) > 10);
    // installing: the progress bar, in the store's accent (scroll to the tile first)
    let r = a.setup.spots.iter().find(|(i, _)| *i == Item::Elasticsearch).map(|(_, r)| *r).unwrap();
    a.setup.list.show(r.y, r.y + r.h);
    let g = draw(&mut a, &cx, 1920, 2400);
    let t = |it: Item| tile_rect(&a, it);
    let acc = crate::apps::store::ACCENT;
    assert!(count(&g, R::new(t(Item::Elasticsearch).x, t(Item::Elasticsearch).y + t(Item::Elasticsearch).h * 0.85, t(Item::Elasticsearch).w, t(Item::Elasticsearch).h * 0.15), acc, 10) > 30, "the bar");
    // recommended: the star at the bottom right; the same corner of an ordinary tile is empty
    let (mut b, cxb) = rig("su-markers2", Role::Admin, full_machine(), facts(&[(Item::Node, Presence::Installed("v26".into()))]));
    let gb = draw(&mut b, &cxb, 1920, 1080);
    assert!(count(&gb, bottom_right(tile_rect(&b, Item::K3s)), AMBER, 8) > 5, "the star on a recommended tile");
    assert_eq!(count(&gb, bottom_right(tile_rect(&b, Item::Node)), AMBER, 8), 0);
}
// ------------------------------------------------------------------------------------------------ search, filters, categories

#[test]
fn search_filter_and_counts_narrow_the_tiles() {
    let (mut a, cx) = rig("su-search", Role::Admin, full_machine(), facts(&[(Item::Node, Presence::Installed("v26".into())), (Item::K3s, Presence::Installed("v1".into()))]));
    let all = a.setup.counts();
    assert_eq!(all, (Item::ALL.len(), Item::ALL.len()));
    key(&mut a, &cx, Key::Char('/'));
    assert!(a.setup.searching);
    for c in "voice".chars() {
        key(&mut a, &cx, Key::Char(c));
    }
    let n = a.setup.counts().0;
    assert!(n >= 2 && n < all.0, "{n}");
    let shown: Vec<Item> = a.setup.stops();
    assert!(shown.contains(&Item::Kokoro) && !shown.contains(&Item::Node));
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(has_hit(&g, Event::Tab(TILE + idx(Item::Kokoro))), "Kokoro's tile is on screen");
    key(&mut a, &cx, Key::Esc);
    assert!(a.setup.query.value.is_empty() && a.setup.counts() == all);
    key(&mut a, &cx, Key::Esc);
    assert!(!a.setup.searching);
    // the filters: Installed, Not installed, Recommended, Updates; f cycles
    a.setup.searching = false;
    key(&mut a, &cx, Key::Char('f'));
    assert_eq!(a.setup.filter, Filter::Installed);
    let shown = a.setup.stops();
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert!(shown.contains(&Item::Node) && shown.contains(&Item::K3s));
    key(&mut a, &cx, Key::Char('f'));
    assert_eq!(a.setup.filter, Filter::NotInstalled);
    assert_eq!(a.setup.counts().0, Item::ALL.len() - 2);
    key(&mut a, &cx, Key::Char('f'));
    assert_eq!(a.setup.filter, Filter::Recommended);
    let shown = a.setup.stops();
    assert!(shown.contains(&Item::GpuVulkan) && shown.contains(&Item::MacFirmware) && !shown.contains(&Item::K3s));
    key(&mut a, &cx, Key::Char('f'));
    assert_eq!(a.setup.filter, Filter::Updates);
    assert!(a.setup.stops().is_empty(), "no lookup has found a newer version");
    a.setup.latest = vec![(Item::Node, qi::Latest::Newer("v27".into())), (Item::K3s, qi::Latest::Current("v1".into()))];
    assert_eq!(a.setup.stops(), vec![Item::Node], "only the installed one with a newer version");
    key(&mut a, &cx, Key::Char('f'));
    assert_eq!(a.setup.filter, Filter::All);
    // the chips are clickable
    let g = draw(&mut a, &cx, 1280, 720);
    let h = g.hits.iter().find(|h| h.ev == Event::Tab(FILTER + 1)).expect("the Installed chip").ev.clone();
    a.handle(&h, &cx);
    assert_eq!(a.setup.filter, Filter::Installed);
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(labels(&g).contains(&"Updates".to_string()));
    // nothing matches: a plain line and Esc clears
    a.handle(&Event::Tab(FILTER), &cx);
    key(&mut a, &cx, Key::Char('/'));
    for c in "zzzz".chars() {
        key(&mut a, &cx, Key::Char(c));
    }
    assert!(a.setup.sections().is_empty() && a.setup.counts().0 == 0);
    draw(&mut a, &cx, 800, 480);
}

#[test]
fn the_category_bar_narrows_the_page_and_the_keys_cycle_it() {
    let (mut a, cx) = rig("su-cats", Role::Admin, full_machine(), facts(&[(Item::K3s, Presence::Installed("v1".into()))]));
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(has_hit(&g, Event::Tab(CAT)), "All");
    for k in 1..=Gk::ALL.len() {
        assert!(has_hit(&g, Event::Tab(CAT + k)), "category {k}");
    }
    // a click on Voices: only the two voices, one heading
    let voices = Gk::ALL.iter().position(|g| *g == Gk::Voices).unwrap() + 1;
    a.handle(&Event::Tab(CAT + voices), &cx);
    assert_eq!(a.setup.cat, Some(Gk::Voices));
    assert_eq!(a.setup.stops(), vec![Item::Kokoro, Item::VoicePack]);
    assert_eq!(titles(&a), vec!["Voices"]);
    assert_eq!(a.setup.counts(), (2, Item::ALL.len()), "the two voices (Kokoro and the voice pack)");
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(has_hit(&g, Event::Tab(TILE + idx(Item::Kokoro))) && !has_hit(&g, Event::Tab(TILE + idx(Item::K3s))));
    // the category and the filter work together
    a.handle(&Event::Tab(FILTER + 1), &cx);
    assert!(a.setup.stops().is_empty());
    a.handle(&Event::Tab(FILTER), &cx);
    // c goes to the next category, and after the last back to All; Esc steps back out
    a.handle(&Event::Tab(CAT), &cx);
    assert_eq!(a.setup.cat, None);
    key(&mut a, &cx, Key::Char('c'));
    assert_eq!(a.setup.cat, Some(Gk::GetStarted));
    for _ in 0..a.setup.categories().len() - 1 {
        key(&mut a, &cx, Key::Char('c'));
    }
    assert_eq!(a.setup.cat, Some(Gk::Hardware).filter(|_| false).or(a.setup.categories().last().copied()));
    key(&mut a, &cx, Key::Char('c'));
    assert_eq!(a.setup.cat, None);
    key(&mut a, &cx, Key::Char('c'));
    key(&mut a, &cx, Key::Esc);
    assert_eq!(a.setup.cat, None, "Esc leaves the category");
}

// ------------------------------------------------------------------------------------------------ installing

#[test]
fn install_all_recommended_asks_with_sizes_then_starts_the_jobs() {
    let mac = Machine { model: "Macmini8,1".into(), ram_mb: 8192, ..Default::default() };
    let (mut a, cx) = rig("su-all", Role::Admin, mac, facts(&[]));
    let c = a.setup.plan_all();
    assert_eq!(c.steps, vec![Item::K3s, Item::MacFirmware]);
    assert_eq!(c.title, "Install 2 recommended items?");
    let lines = Setup::confirm_lines(&c);
    assert_eq!(lines[0], "1. k3s · about 70 MB · 2 to 5 minutes");
    assert_eq!(lines[1], "2. Mac Wi-Fi and Bluetooth firmware (from your Mac) · about 1.5 GB · 5 to 15 minutes");
    assert_eq!(lines[2], "Total download: about 1.6 GB");
    a.handle(&Event::Tab(INSTALL_ALL), &cx);
    assert!(a.setup.confirm.is_some());
    let g = draw(&mut a, &cx, 800, 480);
    assert!(has_hit(&g, Event::Tab(CONFIRM_YES)) && has_hit(&g, Event::Tab(CONFIRM_NO)));
    key(&mut a, &cx, Key::Esc);
    assert!(a.setup.confirm.is_none() && qi::install_jobs(&cx.services.jobs, "root").is_empty());
    key(&mut a, &cx, Key::Char('a'));
    assert!(a.setup.confirm.is_some());
    key(&mut a, &cx, Key::Char('y'));
    assert!(a.setup.confirm.is_none());
    let items: Vec<Item> = qi::install_jobs(&cx.services.jobs, "root").iter().filter_map(qi::item_of).collect();
    assert!(items.contains(&Item::K3s) && items.contains(&Item::MacFirmware) && items.len() == 2, "{items:?}");
    assert!(a.setup.note.as_ref().is_some_and(|n| n.1 && n.0.contains("2 install step")), "{:?}", a.setup.note);
    let (mut a, cx) = rig("su-all2", Role::Admin, Machine { ram_mb: 8192, ..Default::default() }, facts(&[(Item::K3s, Presence::Installed("v1".into()))]));
    a.handle(&Event::Tab(INSTALL_ALL), &cx);
    assert!(a.setup.confirm.is_none() && a.setup.note.is_some());
}

#[test]
fn install_all_in_this_category_has_the_same_review() {
    let (mut a, cx) = rig("su-cat-all", Role::Admin, full_machine(), facts(&[(Item::DuckDb, Presence::Installed("v1".into()))]));
    // Data: DuckDB is there; Elasticsearch, OpenSearch, Prometheus, ClickHouse, the graph database, the two object stores and the registry are missing and need k3s
    assert_eq!(a.setup.category_targets(Gk::Data), vec![Item::Elasticsearch, Item::OpenSearch, Item::Prometheus, Item::ClickHouse, Item::GraphDb, Item::SeaweedFs, Item::RustFs, Item::Registry]);
    let c = a.setup.plan_category(Gk::Data);
    assert_eq!(c.title, "Install 8 items in Data services?");
    assert_eq!(c.steps[0], Item::K3s, "what they need comes first");
    // the popup of a Data tile has the button, with the count
    a.setup.open_popup(Item::DuckDb);
    let g = draw(&mut a, &cx, 1280, 720);
    let b = g.hits.iter().find(|h| h.ev == Event::Tab(POP_ALL)).expect("Install all in this category");
    assert_eq!(b.label, "Install all in Data (8)");
    a.handle(&Event::Tab(POP_ALL), &cx);
    assert!(a.setup.confirm.is_some());
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(has_hit(&g, Event::Tab(CONFIRM_YES)), "the review is drawn over the popup");
    key(&mut a, &cx, Key::Esc);
    assert!(a.setup.confirm.is_none() && a.setup.popup.is_some(), "Esc cancels the review only");
    assert!(qi::install_jobs(&cx.services.jobs, "root").is_empty());
    key(&mut a, &cx, Key::Char('a'));
    key(&mut a, &cx, Key::Enter);
    let items: Vec<Item> = qi::install_jobs(&cx.services.jobs, "root").iter().filter_map(qi::item_of).collect();
    assert!(items.contains(&Item::K3s) && items.contains(&Item::Elasticsearch) && items.contains(&Item::Registry), "{items:?}");
    // a category with nothing missing: a plain answer
    let (mut b, cxb) = rig("su-cat-all2", Role::Admin, full_machine(), facts(&[(Item::Ffmpeg, Presence::Installed("v7".into())), (Item::FfmpegLibs, Presence::Installed("6.1.5".into()))]));
    b.setup.ask_category(Gk::Media);
    assert!(b.setup.confirm.is_none() && b.setup.note.as_ref().is_some_and(|n| n.1 && n.0.contains("Nothing in Media tools")));
    let _ = cxb;
}

#[test]
fn buttons_start_the_right_job_and_open_the_right_place() {
    let (mut a, cx) = rig("su-act", Role::Admin, full_machine(), facts(&[(Item::Lemonade, Presence::Installed("v1".into())), (Item::Ffmpeg, Presence::Installed("v7".into()))]));
    primary(&mut a, &cx, Item::Elasticsearch);
    let mut labels: Vec<String> = qi::install_jobs(&cx.services.jobs, "root").into_iter().map(|j| j.label).collect();
    labels.sort();
    assert_eq!(labels.len(), 2, "{labels:?}");
    assert!(labels.iter().any(|l| l.starts_with("Install k3s (1/2")) && labels.iter().any(|l| l.starts_with("Install Elasticsearch (2/2")), "{labels:?}");
    assert!(a.setup.note.as_ref().unwrap().0.contains("Started"));
    // Open on an installed tile asks the shell for the right app (and the popup goes away)
    primary(&mut a, &cx, Item::Ffmpeg);
    assert_eq!(App::take_request(&mut a), Some(("media".into(), String::new())));
    assert!(a.setup.popup.is_none());
    primary(&mut a, &cx, Item::Lemonade);
    assert_eq!(App::take_request(&mut a), Some(("settings".into(), "modelhost:pc".into())));
    primary(&mut a, &cx, Item::MacHost);
    assert_eq!(App::take_request(&mut a), Some(("settings".into(), "modelhost:mac".into())));
    a.setup.open_popup(Item::GpuVulkan);
    a.handle(&Event::Tab(MORE + idx(Item::GpuVulkan)), &cx);
    assert_eq!(App::take_request(&mut a), Some(("brand:gpu".into(), String::new())));
    a.setup.open_popup(Item::MacFirmware);
    a.handle(&Event::Tab(MORE + idx(Item::MacFirmware)), &cx);
    assert_eq!(App::take_request(&mut a), Some(("brand:mac".into(), String::new())));
    a.setup.open_popup(Item::LinuxApps);
    a.handle(&Event::Tab(MORE + idx(Item::LinuxApps)), &cx);
    assert_eq!(App::take_request(&mut a), Some(("brand:linux".into(), String::new())));
    // the log of a running install, and the popup's View log, open the installer list
    a.setup.jobs = vec![job(Item::Copilot, iyakki_agent::jobs::State::Running, Some(10.0))];
    primary(&mut a, &cx, Item::Copilot);
    assert_eq!(App::take_request(&mut a), Some(("installs".into(), String::new())));
    a.handle(&Event::Tab(POP_LOG), &cx);
    assert_eq!(App::take_request(&mut a), Some(("installs".into(), String::new())));
    // a running cluster opens the K8S tile
    let (mut a, cx) = rig("su-act3", Role::Admin, full_machine(), facts(&[(Item::K3s, Presence::Installed("k3s v1 · running".into()))]));
    primary(&mut a, &cx, Item::K3s);
    assert_eq!(App::take_request(&mut a), Some(("k8s".into(), String::new())));
}

#[test]
fn something_found_by_hand_is_replaced_only_when_asked_twice() {
    let (mut a, cx) = rig("su-own", Role::Admin, full_machine(), facts(&[(Item::Node, Presence::Elsewhere("v26 (/usr/bin/node)".into()))]));
    primary(&mut a, &cx, Item::Node);
    assert!(qi::install_jobs(&cx.services.jobs, "root").is_empty(), "the first press only asks");
    assert!(a.setup.note.as_ref().unwrap().0.contains("leaves it alone"));
    a.handle(&Event::Tab(PRIMARY + idx(Item::Node)), &cx);
    assert_eq!(qi::install_jobs(&cx.services.jobs, "root").len(), 1, "the second installs IYAKKI's own copy");
}

#[test]
fn start_and_stop_go_through_the_cluster_supervisor_and_only_for_an_administrator() {
    let f = facts(&[(Item::K3s, Presence::Installed("k3s v1.31.4 · running".into()))]);
    let (mut a, cx) = rig("su-ss", Role::Admin, full_machine(), f.clone());
    a.setup.open_popup(Item::K3s);
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(has_hit(&g, Event::Tab(STOP + idx(Item::K3s))), "a running cluster offers Stop (never as the main button)");
    assert_eq!(g.hits.iter().find(|h| h.ev == Event::Tab(PRIMARY + idx(Item::K3s))).unwrap().label, "Open");
    // the supervisor answers (nothing is installed by IYAKKI on the made-up data folder): its words come back on the note
    a.handle(&Event::Tab(STOP + idx(Item::K3s)), &cx);
    a.setup.tick(&cx);
    assert!(a.setup.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("no cluster is installed")), "{:?}", a.setup.note);
    // stopped: Start is the main button
    a.setup.live_k3s = Some(Svc::Stopped);
    let g = draw(&mut a, &cx, 1280, 720);
    assert_eq!(g.hits.iter().find(|h| h.ev == Event::Tab(PRIMARY + idx(Item::K3s))).unwrap().label, "Start");
    assert!(!has_hit(&g, Event::Tab(STOP + idx(Item::K3s))));
    a.setup.note = None;
    a.handle(&Event::Tab(PRIMARY + idx(Item::K3s)), &cx);
    assert!(a.setup.note.as_ref().is_some_and(|n| n.0.contains("no cluster is installed")), "{:?}", a.setup.note);
    // an ordinary account: no Stop, no job, "Ask an administrator"
    for role in [Role::User, Role::Operator] {
        let (mut a, cx) = rig(&format!("su-ss-{}", role.name()), role, full_machine(), f.clone());
        if cx.session.policy.manage_settings {
            continue;
        }
        a.setup.open_popup(Item::K3s);
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(!has_hit(&g, Event::Tab(STOP + idx(Item::K3s))));
        a.handle(&Event::Tab(STOP + idx(Item::K3s)), &cx);
        assert!(a.setup.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("administrator")), "{:?}", a.setup.note);
        a.setup.live_k3s = Some(Svc::Stopped);
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(a.setup.popup_button_list(Item::K3s).first() == Some(&("Ask an administrator".to_string(), false)) && !has_hit(&g, Event::Tab(PRIMARY + idx(Item::K3s))));
    }
}

#[test]
fn a_user_without_rights_sees_read_only_tiles() {
    for role in [Role::User, Role::Operator] {
        let tag = format!("su-role-{}", role.name());
        let (mut a, cx) = rig(&tag, role, full_machine(), facts(&[(Item::Ffmpeg, Presence::Installed("v7".into()))]));
        if cx.session.policy.manage_settings {
            continue;
        }
        a.setup.open_popup(Item::K3s);
        a.handle(&Event::Tab(PRIMARY + idx(Item::K3s)), &cx);
        assert!(qi::install_jobs(&cx.services.jobs, &cx.session.user).is_empty());
        assert!(a.setup.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("administrator")), "{:?}", a.setup.note);
        let g = draw(&mut a, &cx, 1280, 720);
        assert_eq!(a.setup.popup_button_list(Item::K3s).first(), Some(&("Ask an administrator".to_string(), false)), "a read-only popup says whom to ask");
        assert!(!has_hit(&g, Event::Tab(PRIMARY + idx(Item::K3s))), "an Install button is not offered");
        assert!(!has_hit(&g, Event::Tab(POP_ALL)), "no install all in this category");
        key(&mut a, &cx, Key::Esc);
        key(&mut a, &cx, Key::Char('a'));
        assert!(a.setup.confirm.is_none());
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(!has_hit(&g, Event::Tab(INSTALL_ALL)), "no Install all button");
        // looking and opening are fine
        a.setup.open_popup(Item::Ffmpeg);
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(has_hit(&g, Event::Tab(PRIMARY + idx(Item::Ffmpeg))), "Open stays");
        assert!(has_hit(&g, Event::Tab(DETAILS + idx(Item::Ffmpeg))), "details stay");
    }
    let (mut a, cx) = rig("su-role-admin", Role::Admin, full_machine(), facts(&[]));
    draw(&mut a, &cx, 800, 480);
    assert!(a.setup.admin);
}

// ------------------------------------------------------------------------------------------------ keys

#[test]
fn the_keyboard_walks_the_tiles_opens_the_popup_and_closes_it() {
    let (mut a, cx) = rig("su-keys", Role::Admin, plain_machine(), facts(&[]));
    draw(&mut a, &cx, 1280, 720);
    let first = a.setup.stops()[0];
    a.setup.cursor = first;
    // Right walks through every tile in order
    let mut seen = vec![first];
    for _ in 0..40 {
        key(&mut a, &cx, Key::Right);
        draw(&mut a, &cx, 1280, 720);
        if seen.last() == Some(&a.setup.cursor) {
            break;
        }
        seen.push(a.setup.cursor);
    }
    assert_eq!(seen, a.setup.stops(), "Right reaches every tile once");
    // Left walks back; Down and Up move by a row (three columns at 1280 wide)
    key(&mut a, &cx, Key::Home);
    assert_eq!(a.setup.cursor, first);
    assert!(a.setup.cols >= 3, "three or four columns at 1280x720");
    key(&mut a, &cx, Key::Down);
    assert_ne!(a.setup.cursor, first);
    let at = a.setup.cursor;
    key(&mut a, &cx, Key::Up);
    assert_eq!(a.setup.cursor, first, "Up goes back above it");
    assert_ne!(at, a.setup.cursor);
    key(&mut a, &cx, Key::Left);
    assert_eq!(a.setup.cursor, first, "nothing before the first");
    key(&mut a, &cx, Key::End);
    assert_eq!(Some(&a.setup.cursor), a.setup.stops().last());
    // Enter opens the popup, Esc closes it, the number keys are not ours
    key(&mut a, &cx, Key::Home);
    key(&mut a, &cx, Key::Enter);
    assert_eq!(a.setup.popup, Some(first));
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(has_hit(&g, Event::Tab(POP_CLOSE)));
    key(&mut a, &cx, Key::Char('d'));
    assert!(a.setup.details.contains(&first));
    key(&mut a, &cx, Key::Esc);
    assert!(a.setup.popup.is_none() && a.setup.cat.is_none());
    let before = (a.setup.cursor, a.setup.filter);
    for c in '0'..='9' {
        key(&mut a, &cx, Key::Char(c));
    }
    assert_eq!((a.setup.cursor, a.setup.filter), before);
    // i presses the main button of the tile, d opens the details, l asks for the installer list, Tab goes to the next section
    key(&mut a, &cx, Key::Char('i'));
    assert_eq!(qi::install_jobs(&cx.services.jobs, "root").iter().filter_map(qi::item_of).collect::<Vec<_>>(), vec![first]);
    key(&mut a, &cx, Key::Char('l'));
    assert_eq!(App::take_request(&mut a), Some(("installs".into(), String::new())));
    key(&mut a, &cx, Key::Char('d'));
    assert!(a.setup.popup.is_some() && a.setup.details.contains(&first));
    key(&mut a, &cx, Key::Esc);
    key(&mut a, &cx, Key::Tab);
    assert!(!(a.section == Section::Setup));
    a.handle(&Event::Tab(4), &cx);
    // Esc leaves (the note first)
    key(&mut a, &cx, Key::Esc);
    assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), crate::apps::Flow::Back));
    // a click on a tile opens its popup; a click outside the popup closes it, a click on it does not
    a.handle(&Event::Tab(TILE + idx(Item::Node)), &cx);
    assert_eq!(a.setup.popup, Some(Item::Node));
    a.handle(&Event::Tab(POP_PANEL), &cx);
    assert_eq!(a.setup.popup, Some(Item::Node));
    a.handle(&Event::Tab(POP_CLOSE), &cx);
    assert!(a.setup.popup.is_none());
}

#[test]
fn every_card_of_the_catalogue_opens_its_popup_when_clicked() {
    // the click codes once stopped at 32 items, so the last firmware cards (MediaTek, Hardware firmware) did nothing
    let (mut a, cx) = rig("su-every-card", Role::Admin, full_machine(), facts(&[]));
    a.handle(&Event::Tab(4), &cx);
    for it in Item::ALL {
        a.setup.popup = None;
        a.handle(&Event::Tab(TILE + idx(it)), &cx);
        assert_eq!(a.setup.popup, Some(it), "a click on the {it:?} card opens its popup");
    }
}

#[test]
fn the_voice_tiles_set_up_kokoro_opens_the_popup_of_kokoro() {
    let (mut a, cx) = rig("su-deeplink", Role::Admin, full_machine(), facts(&[]));
    a.setup.query.set("zzz");
    a.setup.cat = Some(Gk::Media);
    a.open_item("kokoro", &cx);
    assert_eq!(a.section, Section::Setup);
    assert_eq!(a.setup.popup, Some(Item::Kokoro), "the popup of Kokoro is open");
    assert!(a.setup.query.value.is_empty() && a.setup.cat.is_none(), "no search or category hides it");
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(has_hit(&g, Event::Tab(PRIMARY + idx(Item::Kokoro))));
    a.open_item("setup:ollama", &cx);
    assert_eq!(a.setup.popup, Some(Item::Ollama));
    a.open_item("setup", &cx);
    assert_eq!(a.setup.popup, Some(Item::Ollama), "plain setup keeps the page as it is");
}
// ------------------------------------------------------------------------------------------------ the grid

fn rects_overlap(a: R, b: R) -> bool {
    a.x < b.x + b.w - 0.5 && b.x < a.x + a.w - 0.5 && a.y < b.y + b.h - 0.5 && b.y < a.y + a.h - 0.5
}

/// Every tile is inside the list, no two overlap, and the columns are what the width allows.
fn check_grid(a: &StoreApp, what: &str) {
    let v = a.setup.list.view;
    let inner = v.w - 18.0 * (v.h / v.h.max(1.0)) * 0.0; // the bar's strip is kept free (see below)
    let _ = inner;
    let spots = &a.setup.spots;
    assert!(!spots.is_empty(), "{what}: tiles are laid out");
    for (i, (it, r)) in spots.iter().enumerate() {
        assert!(r.x >= -0.01 && r.x + r.w <= v.w + 0.01, "{what}: {it:?} leaves the list: {r:?} in width {}", v.w);
        assert!(r.w > 20.0 && r.h > 20.0, "{what}: {it:?} has a size: {r:?}");
        for (jt, q) in spots.iter().skip(i + 1) {
            assert!(!rects_overlap(*r, *q), "{what}: {it:?} {r:?} overlaps {jt:?} {q:?}");
        }
    }
    let widest = spots.iter().map(|(_, r)| r.x + r.w).fold(0.0f32, f32::max);
    assert!(widest <= v.w - 4.0, "{what}: the last column keeps clear of the scroll bar strip ({widest} of {})", v.w);
}

#[test]
fn the_tile_grid_adapts_its_columns_and_never_overlaps_or_leaves_the_rect() {
    // columns by width: one on a phone, up to six on a big screen
    let want = [(360u32, 1usize, 1usize), (800, 2, 3), (1280, 3, 4), (1920, 5, 6), (2560, 5, 6), (2880, 6, 6)];
    let mut last = 0;
    for ((w, h), (_, lo, hi)) in SIZES.into_iter().zip(want) {
        let (mut a, cx) = rig(&format!("su-grid-{w}"), Role::Admin, full_machine(), facts(&[(Item::K3s, Presence::Installed("v1".into()))]));
        let s = crate::shell::scale_for(w, h);
        let area = R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s);
        let g = draw(&mut a, &cx, w, h);
        hit_inside(&g, area, &format!("{w}x{h}"));
        assert!((lo..=hi).contains(&a.setup.cols), "{w}x{h}: columns {} not in {lo}..={hi}", a.setup.cols);
        assert!(a.setup.cols >= last, "wider screens never get fewer columns");
        last = a.setup.cols;
        check_grid(&a, &format!("{w}x{h}"));
        // the same grid with a category, a search, and the popup open
        a.setup.cat = Some(Gk::Data);
        let g = draw(&mut a, &cx, w, h);
        hit_inside(&g, area, &format!("{w}x{h} category"));
        check_grid(&a, &format!("{w}x{h} category"));
        a.setup.cat = None;
        for it in Item::ALL {
            a.setup.details.insert(it);
        }
        for it in [Item::K3s, Item::Kokoro, Item::MacCamera, Item::LinuxApps] {
            a.setup.open_popup(it);
            let g = draw(&mut a, &cx, w, h);
            hit_inside(&g, area, &format!("{w}x{h} popup {it:?}"));
            assert!(has_hit(&g, Event::Tab(POP_CLOSE)));
        }
        a.setup.popup = None;
        // the review fits
        a.setup.confirm = Some(a.setup.plan_all());
        let g = draw(&mut a, &cx, w, h);
        hit_inside(&g, R::new(0.0, 0.0, w as f32, h as f32), &format!("{w}x{h} review"));
        assert!(has_hit(&g, Event::Tab(CONFIRM_YES)));
    }
}

#[test]
fn the_tile_grid_fits_half_and_thirty_seventy_panes() {
    let (w, h) = (1920u32, 1080u32);
    for (frac, left) in [(0.5f32, true), (0.5, false), (0.3, true), (0.7, false), (0.3, false), (0.7, true)] {
        let pw = (w as f32 * frac).round();
        let s = crate::shell::scale_for(pw as u32, h);
        let (mut a, cx) = rig(&format!("su-pane-{frac}-{left}"), Role::Admin, full_machine(), facts(&[]));
        let area = R::new(if left { 0.0 } else { w as f32 - pw }, 46.0 * s, pw, h as f32 - 46.0 * s);
        for step in 0..30 {
            let g = draw_in(&mut a, &cx, area, w, h);
            hit_inside(&g, area, &format!("pane {frac} left={left} step {step}"));
            check_grid(&a, &format!("pane {frac} left={left}"));
            if a.setup.list.offset >= a.setup.list.max {
                break;
            }
            a.setup.list.by(300.0);
        }
        assert!(a.setup.list.view.x >= area.x - 1.0 && a.setup.list.view.x + a.setup.list.view.w <= area.x + area.w + 1.0);
        a.setup.open_popup(Item::K3s);
        let g = draw_in(&mut a, &cx, area, w, h);
        hit_inside(&g, area, &format!("pane {frac} left={left} popup"));
    }
}

#[test]
fn the_wheel_the_trackpad_and_the_scroll_bar_scroll_the_page() {
    for (w, h) in SIZES {
        let (mut a, cx) = rig(&format!("su-scroll-{w}"), Role::Admin, full_machine(), facts(&[]));
        draw(&mut a, &cx, w, h);
        let l = a.setup.list.clone();
        if !l.scrolls() {
            // a very large screen may hold every tile at once
            assert!(w >= 2560, "{w}x{h}: the page scrolls on a screen this small");
            continue;
        }
        let (inx, iny) = ((l.view.x + 40.0) as i32, (l.view.y + l.view.h / 2.0) as i32);
        a.handle(&Event::MouseMove(inx, iny), &cx);
        a.handle(&Event::Scroll(-1), &cx);
        draw(&mut a, &cx, w, h);
        assert!((a.setup.list.offset - 3.0 * l.row).abs() < 0.5, "{w}x{h}: {} vs {}", a.setup.list.offset, 3.0 * l.row);
        a.handle(&Event::Scroll(1), &cx);
        assert!(a.setup.list.offset.abs() < 0.5, "and back");
        // the wheel with the pointer outside the list does nothing
        a.handle(&Event::MouseMove(inx, (l.view.y - 6.0) as i32), &cx);
        a.handle(&Event::Scroll(-3), &cx);
        assert!(a.setup.list.offset.abs() < 0.5, "{w}x{h}: wheel outside the list");
        // the trackpad: two fingers produce the same Event::Scroll through the touchpad driver
        let range = crate::touchpad::PadRange { x_min: -5087, x_max: 8270, y_min: -182, y_max: 6749, major_max: 5000 };
        let cfg = crate::touchpad::PadConfig { tap_to_click: true };
        let mut pad = crate::touchpad::Touchpad::new();
        let mut now = 1000u64;
        let mut events: Vec<Event> = Vec::new();
        let mut acc = 0.0f32; // the pad's raw travel as classic wheel notches (the real path goes through the scroll engine)
        let mut feed = |pad: &mut crate::touchpad::Touchpad, now: &mut u64, ms: u64, fingers: &[(i32, i32, i32, i32, i32)]| {
            *now += ms;
            for &(slot, id, x, y, major) in fingers {
                for (code, value) in [(0x2f_u16, slot), (0x39, id), (0x35, x), (0x36, y), (0x30, major)] {
                    if id < 0 && code != 0x2f && code != 0x39 {
                        continue;
                    }
                    for o in pad.feed(crate::input::Raw { typ: crate::input::EV_ABS, code, value }, *now, &range, &cfg, 1920) {
                        if let crate::touchpad::PadOut::Scroll(d) = o {
                            acc -= d;
                            let n = (acc / crate::touchpad::SCROLL_STEP) as i32;
                            acc -= n as f32 * crate::touchpad::SCROLL_STEP;
                            if n != 0 {
                                events.push(Event::Scroll(n));
                            }
                        }
                    }
                }
            }
            for o in pad.feed(crate::input::Raw { typ: crate::input::EV_SYN, code: 0, value: 0 }, *now, &range, &cfg, 1920) {
                if let crate::touchpad::PadOut::Scroll(d) = o {
                    acc -= d;
                    let n = (acc / crate::touchpad::SCROLL_STEP) as i32;
                    acc -= n as f32 * crate::touchpad::SCROLL_STEP;
                    if n != 0 {
                        events.push(Event::Scroll(n));
                    }
                }
            }
        };
        feed(&mut pad, &mut now, 0, &[(0, 1, 1000, 3000, 600), (1, 2, 3000, 3000, 600)]);
        feed(&mut pad, &mut now, 50, &[(0, 1, 1000, 3000, 600), (1, 2, 3000, 3000, 600)]);
        for i in 1..=10 {
            feed(&mut pad, &mut now, 16, &[(0, 1, 1000, 3000 + i * 100, 600), (1, 2, 3000, 3000 + i * 100, 600)]);
        }
        assert!(!events.is_empty(), "the pad produced scroll events");
        a.handle(&Event::MouseMove(inx, iny), &cx);
        for e in &events {
            a.handle(e, &cx);
        }
        assert!(a.setup.list.offset > 0.0, "{w}x{h}: two-finger scroll moves the page ({:?})", events);
        let moved = a.setup.list.offset;
        for e in &events {
            if let Event::Scroll(n) = e {
                a.handle(&Event::Scroll(-n), &cx);
            }
        }
        assert!(a.setup.list.offset < moved);
        // the scroll bar: a press on the thumb and a drag to the bottom, then a press low on the track
        let g = draw(&mut a, &cx, w, h);
        let bar = g.hits.iter().find(|x| x.label == "scroll bar").expect("a bar is drawn and clickable");
        assert_eq!(bar.ev, Event::Area(BAR));
        let r = bar.rect;
        assert!(r.x + r.w <= a.setup.list.view.x + a.setup.list.view.w + 1.0 && r.h <= a.setup.list.view.h + 1.0);
        a.handle(&Event::AreaAt(BAR, 3.0, 2.0), &cx);
        assert!(a.setup.list.dragging());
        a.handle(&Event::MouseMove(inx, 100_000), &cx);
        assert!((a.setup.list.offset - a.setup.list.max).abs() < 1.0, "dragged to the end");
        a.handle(&Event::MouseUp(inx, 100_000), &cx);
        assert!(!a.setup.list.dragging());
        a.setup.list.to_top();
        draw(&mut a, &cx, w, h);
        a.handle(&Event::AreaAt(BAR, 3.0, r.h * 0.9), &cx);
        a.handle(&Event::MouseUp(0, 0), &cx);
        assert!(a.setup.list.offset > a.setup.list.max * 0.5, "{w}x{h}: a press low on the track jumps down");
        // PageDown / PageUp
        a.setup.list.to_top();
        key(&mut a, &cx, Key::PageDown);
        assert!(a.setup.list.offset > 0.0);
        key(&mut a, &cx, Key::PageUp);
        assert_eq!(a.setup.list.offset, 0.0);
        // with the popup open the wheel does not move the page behind it
        a.setup.open_popup(Item::K3s);
        a.handle(&Event::MouseMove(inx, iny), &cx);
        a.handle(&Event::Scroll(-2), &cx);
        assert_eq!(a.setup.list.offset, 0.0, "{w}x{h}: the page behind the popup stays");
    }
}

#[test]
fn the_popup_body_scrolls_when_it_does_not_fit() {
    let (mut a, cx) = rig("su-popscroll", Role::Admin, full_machine(), facts(&[]));
    a.setup.details.insert(Item::MacCamera);
    a.setup.open_popup(Item::MacCamera);
    let g = draw(&mut a, &cx, 360, 300);
    let l = a.setup.pop_list.clone();
    assert!(l.scrolls(), "the long camera text does not fit a phone: it scrolls inside the popup");
    assert!(g.hits.iter().any(|h| h.label == "scroll bar" && h.ev == Event::Area(POP_BAR)));
    a.handle(&Event::MouseMove((l.view.x + 20.0) as i32, (l.view.y + 20.0) as i32), &cx);
    a.handle(&Event::Scroll(-1), &cx);
    assert!(a.setup.pop_list.offset > 0.0);
    // the buttons stay where they are (not part of the scrolled body)
    let g2 = draw(&mut a, &cx, 360, 300);
    let b = |g: &Gfx| g.hits.iter().find(|h| h.ev == Event::Tab(PRIMARY + idx(Item::MacCamera))).map(|h| h.rect);
    assert_eq!(b(&g), b(&g2));
    // keys scroll it too
    key(&mut a, &cx, Key::PageUp);
    assert_eq!(a.setup.pop_list.offset, 0.0);
}

#[test]
fn the_cursor_tile_is_kept_in_view_when_the_arrows_walk_down_the_page() {
    let (mut a, cx) = rig("su-follow", Role::Admin, full_machine(), facts(&[]));
    draw(&mut a, &cx, 800, 480);
    for _ in 0..40 {
        key(&mut a, &cx, Key::Down);
        draw(&mut a, &cx, 800, 480);
        let v = a.setup.list.view;
        let r = tile_rect(&a, a.setup.cursor);
        assert!(r.y >= v.y - 1.0 && r.y + r.h <= v.y + v.h + 1.0 || r.h > v.h, "the cursor tile is on screen: {r:?} in {v:?}");
    }
    for _ in 0..40 {
        key(&mut a, &cx, Key::Up);
        draw(&mut a, &cx, 800, 480);
        let v = a.setup.list.view;
        let r = tile_rect(&a, a.setup.cursor);
        assert!(r.y >= v.y - 1.0 && r.y + r.h <= v.y + v.h + 1.0 || r.h > v.h, "going up: {r:?} in {v:?}");
    }
}

// ------------------------------------------------------------------------------------------------ the rest of the old cards' rules

#[test]
fn no_user_facing_text_still_sends_people_to_the_menu_for_quick_install() {
    // grep-style: the sources of the crates, not the tests, docs or this file's own explanations
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut bad = Vec::new();
    for krate in ["iyakki-shell/src", "iyakki-agent/src", "iyakki-core/src"] {
        walk(&root.join(krate), &mut |p, text| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with("_tests.rs") || name == "tests.rs" || name == "store_setup.rs" || name == "store_setup_view.rs" {
                return;
            }
            for (i, l) in text.lines().enumerate() {
                let l = l.replace("\\u{2192}", "\u{2192}");
                let l = l.as_str();
                let t = l.trim_start();
                if t.starts_with("//") {
                    continue;
                }
                if l.contains("IYAKKI menu \u{2192} Quick install") || l.contains("IYAKKI menu -> Quick install") || l.contains("IYAKKI menu (Quick install)") || l.contains("Use Quick install on the IYAKKI machine") || l.contains("(Quick install \u{2192}") || l.contains("Quick install > ") {
                    bad.push(format!("{}:{}: {}", p.display(), i + 1, l.trim()));
                }
            }
        });
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

fn walk(dir: &std::path::Path, f: &mut dyn FnMut(&std::path::Path, &str)) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, f);
        } else if p.extension().is_some_and(|x| x == "rs") {
            if let Ok(t) = std::fs::read_to_string(&p) {
                f(&p, &t);
            }
        }
    }
}

#[test]
fn the_screen_code_has_no_unwrap_on_geometry() {
    // the picture and the model are crash-proof: no `.unwrap()` / `.expect(` outside the tests (computed clamps are `clamp_safe`:
    // the crate-wide scan in tests/safeclamp_scan.rs checks every file)
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/apps");
    for f in ["store_setup_view.rs", "store_setup.rs"] {
        let text = std::fs::read_to_string(dir.join(f)).unwrap();
        let code = text.split("#[cfg(test)]").next().unwrap();
        for (i, l) in code.lines().enumerate() {
            if l.trim_start().starts_with("//") {
                continue;
            }
            assert!(!l.contains(".unwrap()") && !l.contains(".expect("), "{f}:{}: {l}", i + 1);
        }
    }
}

#[test]
fn coding_agents_offer_sign_in_log_out_and_upgrade_and_runtimes_are_named_in_the_order() {
    let (mut a, cx) = rig("su-agents", Role::Admin, full_machine(), facts(&[(Item::ClaudeCode, Presence::Installed("2.1.7".into())), (Item::Copilot, Presence::Installed("0.0.400".into())), (Item::DuckDb, Presence::Installed("v1.1.0".into()))]));
    a.setup.signin = vec![(Item::ClaudeCode, crate::agent_login::State::SignedOut), (Item::Copilot, crate::agent_login::State::SignedIn("me@example.test".into()))];
    assert_eq!((a.setup.card(Item::ClaudeCode).act, a.setup.card(Item::ClaudeCode).label.as_str()), (Act::SignIn, "Sign in"));
    let _ = crate::agent_login::take_request();
    primary(&mut a, &cx, Item::ClaudeCode);
    assert_eq!(crate::agent_login::take_request(), Some(("claude-code", false)));
    let c = a.setup.card(Item::Copilot);
    assert!(c.notes.iter().any(|n| n.0 == "Signed in as me@example.test") && c.act == Act::Open);
    a.setup.open_popup(Item::Copilot);
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(labels(&g).contains(&"Log out".to_string()));
    a.handle(&Event::Tab(LOGOUT + idx(Item::Copilot)), &cx);
    assert_eq!(crate::agent_login::take_request(), Some(("copilot", true)));
    // an installed tool with a newer version is offered Upgrade, and Open stays beside it
    a.setup.latest = vec![(Item::DuckDb, qi::Latest::Newer("v1.2.0".into()))];
    let c = a.setup.card(Item::DuckDb);
    assert_eq!((c.act, c.label.as_str()), (Act::Update, "Upgrade to 1.2.0"));
    assert!(c.notes.iter().any(|n| n.0.contains("newer version")));
    a.setup.open_popup(Item::DuckDb);
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(labels(&g).contains(&"Upgrade to 1.2.0".to_string()) && has_hit(&g, Event::Tab(MORE + idx(Item::DuckDb))), "Upgrade and Open");
    a.handle(&Event::Tab(PRIMARY + idx(Item::DuckDb)), &cx);
    assert!(qi::install_jobs(&cx.services.jobs, "root").iter().any(|j| j.label.starts_with("Upgrade DuckDB")), "an installed item is upgraded, not installed again");
    let mut f = facts(&[]);
    f.runtimes = vec![(Item::Python, vec!["uv"])];
    a.setup.set_facts(f);
    assert_eq!(a.setup.card(Item::Python).deps.as_deref(), Some("Installs uv first"));
}

#[test]
fn a_desktop_window_never_installs_a_cluster_but_installs_the_player() {
    let m = Machine { window: true, ram_mb: 16_000, ..Default::default() };
    let (mut a, cx) = rig("su-window", Role::Admin, m, facts(&[]));
    for it in [Item::K3s, Item::Elasticsearch, Item::Lemonade] {
        let c = a.setup.card(it);
        assert!(c.blocked && c.notes.iter().any(|n| n.0.contains("desktop window") && n.0.contains("Store")), "{it:?}");
        primary(&mut a, &cx, it);
    }
    assert!(qi::install_jobs(&cx.services.jobs, "root").is_empty());
    assert!(a.setup.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("desktop window")));
    assert!(a.setup.recommended(Item::K3s).is_none(), "k3s is not recommended in a window");
    assert!(a.setup.category_targets(Gk::Data).is_empty(), "install all in a category leaves the cluster items out");
    primary(&mut a, &cx, Item::Ffmpeg);
    assert_eq!(qi::install_jobs(&cx.services.jobs, "root").len(), 1, "the player installs fine");
    // the tile and popup still draw, and say why
    a.setup.open_popup(Item::K3s);
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(!g.hits.iter().any(|h| h.ev == Event::Tab(PRIMARY + idx(Item::K3s))), "the blocked button is shown but not pressable");
}

#[test]
fn the_mac_model_host_is_a_guide_and_linux_apps_has_its_own_page() {
    let (mut a, cx) = rig("su-guide", Role::Admin, full_machine(), facts(&[]));
    let c = a.setup.card(Item::MacHost);
    assert_eq!((c.chip.as_str(), c.act, c.label.as_str()), ("Guide", Act::Open, "Open guide"));
    assert!(size_text(Item::MacHost).starts_with("no download"));
    a.setup.open_popup(Item::LinuxApps);
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(has_hit(&g, Event::Tab(PRIMARY + idx(Item::LinuxApps))) && g.hits.iter().any(|h| h.ev == Event::Tab(MORE + idx(Item::LinuxApps)) && h.label == "Open its page"));
}

#[test]
fn the_setup_ids_sit_above_every_id_the_store_uses() {
    for id in [FILTER, INSTALL_ALL, CONFIRM_YES, SEARCH, BAR, POP_CLOSE, POP_ALL, POP_LOG, POP_BAR, POP_PANEL, CAT, PRIMARY, DETAILS, MORE, TILE, LOGOUT, DELETE, STOP] {
        assert!((20_000..21_000).contains(&id));
    }
    // and the ranges do not run into each other
    let ranges = [(FILTER, 5), (INSTALL_ALL, 1), (CONFIRM_YES, 2), (SEARCH, 1), (BAR, 1), (POP_CLOSE, 5), (CAT, 9), (PRIMARY, 32), (DETAILS, 32), (MORE, 32), (TILE, 32), (LOGOUT, 32), (DELETE, 1), (STOP, 32), (BUILD, 32)];
    for (i, (a, n)) in ranges.iter().enumerate() {
        for (b, m) in ranges.iter().skip(i + 1) {
            assert!(a + n <= *b || b + m <= *a, "{a}+{n} runs into {b}+{m}");
        }
    }
}

// ------------------------------------------------------------------------------------------------ the MacBook camera tile

/// Put a result of the camera firmware job on the (made-up) data folder and a kept download of `kept` bytes, then read them again.
fn camera_files(a: &mut StoreApp, cx: &AppCx, o: Option<iyakki_agent::camfw::Outcome>, kept: usize) {
    let data = cx.services.dir();
    if let Some(o) = o {
        std::fs::create_dir_all(iyakki_agent::camfw::work_dir(data)).unwrap();
        std::fs::write(iyakki_agent::camfw::result_path(data), serde_json::to_string(&o).unwrap()).unwrap();
    }
    if kept > 0 {
        let d = iyakki_agent::camfw::work_dir(data).join("download");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("part1.xz"), vec![7u8; kept]).unwrap();
    }
    a.setup.enter(cx);
}

fn done_outcome(installed: bool, headline: &str) -> iyakki_agent::camfw::Outcome {
    iyakki_agent::camfw::Outcome { installed, firmware_version: if installed { "1.2.3".into() } else { String::new() }, headline: headline.into(), ..Default::default() }
}

#[test]
fn the_camera_tile_needs_the_camera_and_is_hidden_in_cloud_mode() {
    let none = Machine { model: "MacBookPro14,1".into(), ram_mb: 16_000, ..Default::default() };
    let (a, _cx) = rig("su-cam1", Role::Admin, none.clone(), facts(&[]));
    assert!(!a.setup.eligible().contains(&Item::MacCamera), "a Mac without the 14e4:1570 camera: no tile");
    assert!(a.setup.sections().iter().all(|s| !s.items.contains(&Item::MacCamera)));
    let (a, _cx) = rig("su-cam2", Role::Admin, Machine { camera: true, ..none.clone() }, facts(&[]));
    assert!(a.setup.eligible().contains(&Item::MacCamera));
    assert_eq!(Gk::of(Item::MacCamera), Gk::Hardware);
    let hw = a.setup.sections().into_iter().find(|s| s.gk == Gk::Hardware).unwrap();
    assert!(hw.items.contains(&Item::MacCamera), "in the Hardware category");
    let (a, _cx) = rig("su-cam3", Role::Admin, Machine { camera: true, cloud: true, ..none }, facts(&[]));
    assert!(!a.setup.eligible().contains(&Item::MacCamera), "cloud mode hides it");
    assert!(Item::MacCamera.is_hardware() && !Item::MacCamera.runs_as_job());
}

#[test]
fn a_firmware_pack_tile_needs_its_hardware_is_hidden_in_cloud_mode_and_says_what_degrades() {
    let packs: Vec<Item> = Item::ALL.into_iter().filter(|i| i.fw_def().is_some()).collect();
    assert_eq!(packs.len(), 5);
    // wrong hardware: no tile for any pack
    let (a, _cx) = rig("su-fw1", Role::Admin, plain_machine(), facts(&[]));
    for p in &packs {
        assert!(!a.setup.eligible().contains(p), "{p:?}: no matching hardware, no tile");
    }
    assert!(a.setup.sections().iter().all(|s| s.items.iter().all(|i| i.fw_def().is_none())));
    // a Realtek NIC: exactly that tile, in the Hardware category
    let nic = Machine { ram_mb: 16_000, fw: vec![Item::FwRealtekNet], ..Default::default() };
    let (a, _cx) = rig("su-fw2", Role::Admin, nic.clone(), facts(&[]));
    assert!(a.setup.eligible().contains(&Item::FwRealtekNet) && !a.setup.eligible().contains(&Item::FwBroadcomNet));
    assert_eq!(Gk::of(Item::FwRealtekNet), Gk::Hardware);
    let hw = a.setup.sections().into_iter().find(|s| s.gk == Gk::Hardware).unwrap();
    assert!(hw.items.contains(&Item::FwRealtekNet) && !hw.items.contains(&Item::FwQualcommBt));
    // the card: one button that reads the licence first, the chip, the size, what is lost without it, no recommendation (never installed behind the back)
    let c = a.setup.card(Item::FwRealtekNet);
    assert_eq!((c.chip.as_str(), c.label.as_str(), c.act), ("Not installed", "Read the licence and install", Act::Install));
    assert!(c.notes.iter().any(|(n, _)| n.starts_with("Without it:")), "{:?}", c.notes);
    assert!(a.setup.plan_all().targets.iter().all(|t| t.fw_def().is_none()), "Install all recommended never installs a restricted pack");
    assert_eq!(size_text(Item::FwRealtekNet), "about 1 MB · about a minute");
    assert!(Item::FwRealtekNet.licence().contains("hexadecimal or equivalent format") && where_text(Item::FwRealtekNet).contains("survives restarts"));
    // installed: the chip says so and the button repairs
    let f = facts(&[(Item::FwRealtekNet, Presence::Installed("30 files, linux-firmware 1a935bfb33".into()))]);
    let (a, _cx) = rig("su-fw3", Role::Admin, nic.clone(), f);
    let c = a.setup.card(Item::FwRealtekNet);
    assert_eq!((c.chip.as_str(), c.act), ("Installed", Act::Reinstall));
    // cloud mode hides it
    let (a, _cx) = rig("su-fw4", Role::Admin, Machine { cloud: true, ..nic }, facts(&[]));
    assert!(!a.setup.eligible().contains(&Item::FwRealtekNet), "cloud mode hides it");
    assert!(Item::FwRealtekNet.is_hardware() && !Item::FwRealtekNet.runs_as_job());
}

#[test]
fn the_camera_tile_states_and_the_one_button() {
    use iyakki_agent::jobs::State;
    let m = Machine { camera: true, ram_mb: 16_000, ..Default::default() };
    let (mut a, cx) = rig("su-cam4", Role::Admin, m.clone(), facts(&[]));
    let c = a.setup.card(Item::MacCamera);
    assert_eq!((c.chip.as_str(), c.act, c.label.as_str()), ("Not installed", Act::Install, "Install camera driver and firmware"));
    assert!(c.deps.is_none() && c.reason.is_none() && c.kept == 0);
    assert_eq!(size_text(Item::MacCamera), "about 17.6 MB · about a minute");
    assert!(Item::MacCamera.licence().contains("Apple") && Item::MacCamera.licence().contains("GPL-2.0") && Item::MacCamera.licence().contains("never ships"));
    assert!(Item::MacCamera.about().contains("Apple's update server") && Item::MacCamera.about().contains("17.6 MB"));
    primary(&mut a, &cx, Item::MacCamera);
    let jobs = qi::install_jobs(&cx.services.jobs, "root");
    assert_eq!(jobs.len(), 1, "{:?}", a.setup.note);
    assert!(qi::is_camera_job(&jobs[0]) && jobs[0].command.contains("quick-install mac-camera"), "{}", jobs[0].command);
    assert!(a.setup.note.as_ref().is_some_and(|n| n.1 && n.0.contains("Camera firmware") && n.0.contains("17.6 MB")), "{:?}", a.setup.note);
    a.setup.jobs = vec![job(Item::MacCamera, State::Running, Some(40.0))];
    let c = a.setup.card(Item::MacCamera);
    assert_eq!((c.chip.as_str(), c.act), ("Installing 40 %", Act::Log));
    camera_files(&mut a, &cx, Some(done_outcome(false, "Apple's server did not answer")), 0);
    a.setup.jobs.clear();
    a.setup.failed = vec![Item::MacCamera];
    let c = a.setup.card(Item::MacCamera);
    assert_eq!((c.chip.as_str(), c.act, c.label.as_str()), ("Failed", Act::Retry, "Retry"));
    assert!(c.notes.iter().any(|n| n.0.contains("did not answer") && n.1));
    let f = facts(&[(Item::MacCamera, Presence::Installed("firmware 1.2.3".into()))]);
    a.setup.set_facts(f);
    camera_files(&mut a, &cx, Some(done_outcome(true, "Camera firmware installed: the camera answers")), 0);
    a.setup.jobs.clear();
    a.setup.failed.clear();
    let c = a.setup.card(Item::MacCamera);
    assert_eq!((c.chip.as_str(), c.act, c.label.as_str()), ("Installed", Act::Open, "Open camera"));
    assert!(c.notes.iter().any(|n| n.0.contains("the camera answers") && !n.1));
    primary(&mut a, &cx, Item::MacCamera);
    assert_eq!(App::take_request(&mut a), Some(("camera".into(), String::new())));
    a.setup.facts = None;
    assert_eq!(a.setup.card(Item::MacCamera).chip, "Checking…");
}

#[test]
fn the_camera_tile_deletes_its_kept_download_and_only_for_an_administrator() {
    let m = Machine { camera: true, ram_mb: 16_000, ..Default::default() };
    let (mut a, cx) = rig("su-cam5", Role::Admin, m.clone(), facts(&[]));
    camera_files(&mut a, &cx, None, 4096);
    assert_eq!(a.setup.card(Item::MacCamera).kept, 4096);
    a.setup.open_popup(Item::MacCamera);
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(has_hit(&g, Event::Tab(DELETE)), "Delete download is offered in the popup");
    a.handle(&Event::Tab(DELETE), &cx);
    assert_eq!(iyakki_agent::camfw::leftovers(cx.services.dir()), 0);
    assert_eq!(a.setup.camera_kept, 0);
    assert!(a.setup.note.as_ref().is_some_and(|n| n.1 && n.0.contains("Deleted")), "{:?}", a.setup.note);
    assert_eq!(a.setup.card(Item::MacCamera).kept, 0, "no kept download, no button");
    for role in [Role::User, Role::Operator] {
        let (mut a, cx) = rig(&format!("su-cam6-{}", role.name()), role, m.clone(), facts(&[]));
        if cx.session.policy.manage_settings {
            continue;
        }
        camera_files(&mut a, &cx, None, 2048);
        primary(&mut a, &cx, Item::MacCamera);
        assert!(qi::install_jobs(&cx.services.jobs, &cx.session.user).is_empty(), "no job without the right");
        assert!(a.setup.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("administrator")));
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(!has_hit(&g, Event::Tab(PRIMARY + idx(Item::MacCamera))), "the install button is not offered");
        a.handle(&Event::Tab(DELETE), &cx);
        assert!(iyakki_agent::camfw::leftovers(cx.services.dir()) > 0, "the download stays");
        assert!(a.setup.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("administrator")));
    }
}

#[test]
fn the_camera_tile_and_popup_draw_with_details_and_a_progress_bar_at_every_size_and_pane() {
    use iyakki_agent::jobs::State;
    for (w, h) in SIZES {
        let (mut a, cx) = rig(&format!("su-cam7-{w}"), Role::Admin, Machine { camera: true, ram_mb: 16_000, ..Default::default() }, facts(&[]));
        camera_files(&mut a, &cx, Some(done_outcome(false, "The firmware could not be built: a part did not match its checksum")), 8_000_000);
        a.setup.details.insert(Item::MacCamera);
        a.setup.jobs = vec![job(Item::MacCamera, State::Running, Some(55.0))];
        let s = crate::shell::scale_for(w, h);
        let area = R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s);
        for state in 0..3 {
            match state {
                0 => {}
                1 => a.setup.jobs.clear(),
                _ => a.setup.failed = vec![Item::MacCamera],
            }
            let g = draw(&mut a, &cx, w, h);
            hit_inside(&g, area, &format!("{w}x{h} camera tile state {state}"));
            a.setup.focus_item(Item::MacCamera);
            let g = draw(&mut a, &cx, w, h);
            hit_inside(&g, area, &format!("{w}x{h} camera popup state {state}"));
            let want = PRIMARY + idx(Item::MacCamera);
            assert!(has_hit(&g, Event::Tab(want)), "{w}x{h} state {state}: the one button is reachable");
            assert_eq!(has_hit(&g, Event::Tab(DELETE)), state != 0, "{w}x{h} state {state}: Delete download only when no job runs");
            a.setup.popup = None;
        }
    }
    let (w, h) = (1920u32, 1080u32);
    let s = crate::shell::scale_for(w / 2, h);
    for left in [true, false] {
        let (mut a, cx) = rig(&format!("su-cam8-{left}"), Role::Admin, Machine { camera: true, ram_mb: 16_000, ..Default::default() }, facts(&[]));
        camera_files(&mut a, &cx, None, 3000);
        a.setup.details.insert(Item::MacCamera);
        let half = w as f32 / 2.0;
        let area = R::new(if left { 0.0 } else { half }, 46.0 * s, half, h as f32 - 46.0 * s);
        a.setup.open_popup(Item::MacCamera);
        let g = draw_in(&mut a, &cx, area, w, h);
        hit_inside(&g, area, &format!("camera pane left={left}"));
    }
}

// ------------------------------------------------------------------------------------------------ pictures

/// Pictures of the page for a person to look at: `IYAKKI_PICTURES=/some/folder cargo test -p iyakki-shell --lib store_setup_tests::pictures -- --ignored`.
#[test]
#[ignore]
fn pictures() {
    let Ok(dir) = std::env::var("IYAKKI_PICTURES") else { return };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, w, h) in [("phone", 360u32, 740u32), ("small", 800, 480), ("wide", 1280, 720), ("full", 1920, 1080), ("big", 2560, 1600)] {
        let (mut a, cx) = mixed(&format!("su-pic-{name}"), Role::Admin);
        let g = draw(&mut a, &cx, w, h);
        g.pm.save_png(dir.join(format!("{name}-tiles.png"))).unwrap();
        a.setup.cat = Some(Gk::Data);
        let g = draw(&mut a, &cx, w, h);
        g.pm.save_png(dir.join(format!("{name}-category.png"))).unwrap();
        a.setup.cat = None;
        for (tag, it) in [("k3s", Item::K3s), ("claude", Item::ClaudeCode), ("es", Item::Elasticsearch), ("registry", Item::Registry), ("camera", Item::MacCamera)] {
            a.setup.open_popup(it);
            let g = draw(&mut a, &cx, w, h);
            g.pm.save_png(dir.join(format!("{name}-popup-{tag}.png"))).unwrap();
        }
        a.setup.popup = None;
    }
    // read-only, the review, and the two panes
    let (mut a, cx) = mixed("su-pic-user", Role::User);
    a.setup.open_popup(Item::K3s);
    draw(&mut a, &cx, 1280, 720).pm.save_png(dir.join("readonly-popup.png")).unwrap();
    let (mut a, cx) = mixed("su-pic-review", Role::Admin);
    a.setup.confirm = Some(a.setup.plan_all());
    draw(&mut a, &cx, 1280, 720).pm.save_png(dir.join("review.png")).unwrap();
    let (w, h) = (1920u32, 1080u32);
    for (frac, left, tag) in [(0.5f32, true, "pane-5050-left"), (0.3, true, "pane-3070-left"), (0.7, false, "pane-3070-right")] {
        let pw = (w as f32 * frac).round();
        let s = crate::shell::scale_for(pw as u32, h);
        let (mut a, cx) = mixed(&format!("su-pic-{tag}"), Role::Admin);
        let area = R::new(if left { 0.0 } else { w as f32 - pw }, 46.0 * s, pw, h as f32 - 46.0 * s);
        draw_in(&mut a, &cx, area, w, h).pm.save_png(dir.join(format!("{tag}.png"))).unwrap();
        a.setup.open_popup(Item::K3s);
        draw_in(&mut a, &cx, area, w, h).pm.save_png(dir.join(format!("{tag}-popup.png"))).unwrap();
    }
}

#[test]
fn card_state_follows_the_cluster_not_an_old_failed_job() {
    let installed = Presence::Installed("running in the cluster".into());
    // healthy Deployment + a failed newest job (a failed Retry, an old outage) = Installed
    assert_eq!(base_state(None, true, false, true, None, Some(&installed)), CardState::Installed);
    // even when the marker is gone, the object is what counts (the facts upgrade the presence)
    assert_eq!(base_state(None, true, false, true, None, Some(&installed)), CardState::Installed);
    // no object + failed job = Failed
    assert_eq!(base_state(None, true, false, false, None, Some(&Presence::Missing)), CardState::Failed);
    // a running job is Installing, healthy or not
    assert_eq!(base_state(Some(Some(10.0)), true, false, true, None, Some(&installed)), CardState::Installing(Some(10.0)));
    // nothing known, no job
    assert_eq!(base_state(None, false, false, false, None, Some(&Presence::Missing)), CardState::Missing);
}

#[test]
fn an_item_installed_earlier_whose_last_attempt_failed_shows_failed_with_the_reason_unless_it_is_healthy_now() {
    let why = "Network problem: no data from ghcr.io for 60 s";
    let earlier = Presence::Earlier(format!("{}{why}", iyakki_agent::quickinstall::LAST_FAILED));
    // the record alone (the job list is gone after a restart) makes the card Failed, with Retry and the reason as a note
    let (a, _cx) = rig("su-lastfail", Role::Admin, full_machine(), facts(&[(Item::Registry, earlier.clone())]));
    let c = a.setup.card(Item::Registry);
    assert_eq!((c.chip.as_str(), c.act, c.label.as_str()), ("Failed", Act::Retry, "Retry"));
    assert!(c.notes.iter().any(|n| n.0.contains("Installed earlier; the last attempt failed") && n.0.contains(why) && n.1), "{:?}", c.notes);
    // the same record on an item that the cluster shows healthy right now: Installed (the facts replace the presence, as gather_facts does)
    let mut f = facts(&[(Item::Elasticsearch, Presence::Installed("http://elasticsearch.iyakki-data.svc:9200".into()))]);
    f.healthy = vec![Item::Elasticsearch];
    let (mut a, _cx) = rig("su-lastfail2", Role::Admin, full_machine(), f);
    a.setup.failed = vec![Item::Elasticsearch];
    assert_eq!(a.setup.card(Item::Elasticsearch).chip, "Running", "healthy: a failed Retry does not turn a running service into Failed");
    // a plain "installed earlier, not running" without a failed attempt is unchanged
    let (a, _cx) = rig("su-lastfail3", Role::Admin, full_machine(), facts(&[(Item::DuckDb, Presence::Earlier("cluster down".into()))]));
    assert_eq!(a.setup.card(Item::DuckDb).chip, "Installed earlier, not running");
}
