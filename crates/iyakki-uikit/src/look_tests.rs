//! Looks (F-LOOK-01..04): the data model, the file format v2 and its round trip, the safety rules, the built-in Looks and the example
//! files. Pure data: nothing is drawn and nothing touches the shell.

use super::*;
use crate::{fx, theme, ui, wallpaper};

const MIDNIGHT: &str = include_str!("../../../docs/examples/looks/midnight-glass.look");
const PAPER: &str = include_str!("../../../docs/examples/looks/paper-list.look");
const OLD: &str = include_str!("../../../docs/examples/looks/old-colours.look");

fn problems(text: &str) -> Vec<LookProblem> {
    parse(text).expect_err("the text should be refused")
}

#[test]
fn a_look_nobody_chose_is_what_the_shell_uses_today() {
    let l = Look::default();
    // the colours are exactly the Dark constants the shell draws with
    assert_eq!(l.theme.id, theme::ThemeId::Dark);
    assert_eq!(format!("{:?}", l.theme), format!("{:?}", theme::DARK));
    assert_eq!((l.theme.bg_top, l.theme.card, l.theme.text, l.theme.border), (ui::BG_TOP, ui::CARD, ui::TEXT, ui::BORDER));
    assert_eq!((l.theme.panel, l.theme.amber), (fx::PANEL, fx::AMBER));
    assert_eq!(l.theme.ring, 1.0);
    // the theme's own wallpaper, the paged island, the screen choosing the columns, full motion
    assert_eq!(l.wallpaper, wallpaper::Id::default());
    assert_eq!(l.wallpaper, wallpaper::Id::Theme);
    assert_eq!((l.launcher.layout, l.launcher.columns, l.launcher.island), (LauncherLayout::Paged, Columns::Auto, true));
    assert_eq!((l.icons.shape, l.icons.style, l.icons.labels), (IconShape::Squircle, IconStyle::Gradient, true));
    assert_eq!(l.motion, Motion::Full);
    assert!(l.widgets.is_empty());
    // the owner's decision: a dock at the bottom, the bar at the top, so they cannot meet
    assert_eq!((l.dock.position, l.topbar.position), (DockPos::Bottom, BarPos::Top));
    // and it passes every rule without anything having to be put back
    let a = validate(&l).expect("the default Look is valid");
    assert!(a.fixes.is_empty(), "{:?}", a.fixes);
    assert_eq!(a.look, l);
}

#[test]
fn every_part_survives_text_and_back() {
    let mut all = Look::builtin();
    let mut odd = Look::default();
    odd.name = "Odd one-2.x".into();
    odd.theme = *theme::ThemeId::Light.theme();
    odd.theme.ring = 2.25;
    odd.theme.accent = [1, 2, 3, 200];
    odd.wallpaper = wallpaper::Id::Meadow;
    odd.icons = Icons { shape: IconShape::Circle, size: IconSize::Px(77), style: IconStyle::ThreeD, labels: false, set: "builtin".into() };
    odd.launcher = Launcher { layout: LauncherLayout::List, columns: Columns::Fixed(8), density: Density::Roomy, page_dots: false, groups: Groups::Sidebar, island: false };
    odd.dock = Dock { position: DockPos::Right, items: vec!["chat".into(), "my-app.2".into()], running: false, size: 100, magnify: true, autohide: true };
    odd.topbar = TopBar { on: false, position: BarPos::Bottom, items: BarItem::ALL.iter().rev().copied().collect(), size: 50, footer_hints: false };
    odd.menu = MenuPart { entries: MenuEntry::ALL.iter().rev().copied().collect(), position: MenuPos::Bottom };
    odd.widgets = vec![Widget { kind: WidgetKind::Tokens, size: WidgetSize::Large }, Widget { kind: WidgetKind::Attention, size: WidgetSize::Small }];
    odd.motion = Motion::Reduced;
    all.push(odd);
    for l in all {
        let text = l.to_text();
        let back = parse(&text).unwrap_or_else(|e| panic!("{}: {}\n{text}", l.name, problems_text(&e)));
        assert!(back.fixes.is_empty(), "{}: {:?}", l.name, back.fixes);
        assert_eq!(back.look, l, "{} changed on the way through text", l.name);
        assert_eq!(back.look.to_text(), text);
    }
}

#[test]
fn a_v1_theme_file_is_a_look_with_only_colours() {
    let v1 = "# a calm blue\nname = Midnight blue\nbase = dark\nbg_top = #0b1020\nbg_bottom = #05070f\ncard = #131a30\ntext = #f2f4ff\nring = 1.5\n";
    let a = parse(v1).unwrap();
    let u = usertheme::parse(v1).unwrap();
    assert_eq!(format!("{:?}", a.look.theme), format!("{:?}", u.theme), "the colours are what usertheme reads");
    assert_eq!(a.look.name, "Midnight blue");
    let d = Look::default();
    assert_eq!((a.look.icons.clone(), a.look.launcher.clone(), a.look.dock.clone(), a.look.topbar.clone()), (d.icons, d.launcher, d.dock, d.topbar));
    assert!(a.fixes.is_empty());
    // a v1 file the old reader refuses (bad contrast) is refused here too
    assert!(parse("name = Faint\ntext = #202020\n").is_err());
    // the shipped colour-only example
    assert_eq!(parse(OLD).unwrap().look.theme.ring, 2.0);
}

#[test]
fn the_example_files_load() {
    let m = parse(MIDNIGHT).unwrap_or_else(|e| panic!("{}", problems_text(&e)));
    assert!(m.fixes.is_empty(), "{:?}", m.fixes);
    let l = m.look;
    assert_eq!((l.icons.style, l.icons.shape, l.icons.size), (IconStyle::Glass, IconShape::Circle, IconSize::L));
    assert_eq!((l.dock.position, l.dock.items.len(), l.dock.magnify), (DockPos::Left, 4, true));
    assert_eq!(l.launcher.columns, Columns::Fixed(5));
    assert_eq!(l.widgets[0], Widget { kind: WidgetKind::Attention, size: WidgetSize::Large });
    assert_eq!((l.wallpaper, l.motion), (wallpaper::Id::Dusk, Motion::Reduced));
    let p = parse(PAPER).unwrap_or_else(|e| panic!("{}", problems_text(&e))).look;
    assert_eq!((p.launcher.layout, p.topbar.position, p.dock.position, p.icons.size), (LauncherLayout::List, BarPos::Bottom, DockPos::Off, IconSize::Px(40)));
    assert_eq!(p.theme.id, theme::ThemeId::Light);
}

#[test]
fn the_three_built_in_looks_are_valid_and_differ_as_designed() {
    let all = Look::builtin();
    assert_eq!(all.iter().map(|l| l.key()).collect::<Vec<_>>(), ["default", "compact", "classic"]);
    for l in &all {
        let a = validate(l).unwrap_or_else(|e| panic!("{}: {}", l.name, problems_text(&e)));
        assert!(a.fixes.is_empty());
    }
    let (d, c, k) = (Look::default(), Look::compact(), Look::classic());
    assert_eq!((c.icons.size, c.launcher.density, c.launcher.columns, c.topbar.on), (IconSize::S, Density::Compact, Columns::Fixed(7), true));
    assert_eq!((k.launcher.layout, k.launcher.island, k.dock.position), (LauncherLayout::Grid, false, DockPos::Off));
    assert_eq!(format!("{:?}", c.theme), format!("{:?}", d.theme), "the built-ins change layout, not colours");
    for style in IconStyle::ALL {
        let mut l = Look::default();
        l.icons.style = *style;
        assert!(validate(&l).is_ok(), "{}", style.key());
    }
}

#[test]
fn essential_controls_are_put_back_and_the_report_says_so() {
    let a = parse("name = Bare\n[topbar]\ntopbar_items = [clock]\n[menu]\nmenu = [home, apps]\n").unwrap();
    for e in ESSENTIAL_BAR {
        assert!(a.look.topbar.items.contains(&e), "{}", e.key());
    }
    for e in ESSENTIAL_MENU {
        assert!(a.look.menu.entries.contains(&e), "{}", e.key());
    }
    assert_eq!(a.look.topbar.items[0], BarItem::Clock, "what the file wrote keeps its place");
    assert_eq!(a.fixes.len(), 5 + 4);
    assert!(a.fixes.iter().any(|f| f.contains("`bell`") && f.contains("top bar")), "{:?}", a.fixes);
    assert!(a.fixes.iter().any(|f| f.contains("`sign_out`") && f.contains("menu")));
    // an empty list is the extreme case; the same happens from code
    let mut l = Look::default();
    l.topbar.items.clear();
    l.menu.entries.clear();
    let v = validate(&l).unwrap();
    assert_eq!(v.look.topbar.items.len(), 5);
    assert_eq!(v.fixes.len(), 9);
    // the Look is not refused for it
    assert!(parse("name = Bare\n[topbar]\ntopbar = off\n").unwrap().fixes.is_empty());
}

#[test]
fn bad_files_are_refused_whole_with_the_line_and_the_reason() {
    let cases: [(&str, usize, &str); 12] = [
        ("name = A\n[launcher]\ncolumns = 2\n", 3, "out of range"),
        ("name = A\n[launcher]\ncolumns = 9\n", 3, "out of range"),
        ("name = A\n[dock]\ndock_size = 500\n", 3, "out of range"),
        ("name = A\n[topbar]\ntopbar_size = 5\n", 3, "out of range"),
        ("name = A\n[icons]\nicon_size = 4\n", 3, "out of range"),
        ("name = A\n[icons]\nicon_shape = star\n", 3, "not one of"),
        ("name = A\n[topbar]\ntopbar_items = [clock, run_script]\n", 3, "run_script"),
        ("name = A\n[menu]\nmenu = [home, format_disk]\n", 3, "format_disk"),
        ("name = A\n[sparkle]\n", 2, "not a section"),
        ("name = A\n[icons]\ncolumns = 4\n", 3, "belongs in [launcher]"),
        ("name = A\nname = B\n", 2, "written twice"),
        ("name = A\n[dock]\ndock_items = [Chat Bar]\n", 3, "app id"),
    ];
    for (text, line, word) in cases {
        let p = problems(text);
        assert!(p.iter().any(|x| x.line == line && x.message.contains(word)), "{text:?} -> {p:?}");
        assert!(p[0].to_string().starts_with(&format!("line {line}")) || p.len() > 1);
    }
    // all the problems come back together, not just the first
    let p = problems("name = A\n[icons]\nicon_shape = star\nicon_style = jelly\n[launcher]\ncolumns = 1\n");
    assert_eq!(p.iter().map(|x| x.line).collect::<Vec<_>>(), [3, 4, 6]);
    // a file with no name, text that is no line at all, the whole-file rules
    assert!(problems("base = dark\n")[0].message.contains("name"));
    assert!(problems("name = A\njust words\n")[0].message.contains("key = value"));
    assert_eq!(problems(&"x".repeat(MAX_BYTES as usize + 1))[0].line, 0);
    assert!(problems(&"# c\n".repeat(MAX_LINES + 5)).iter().any(|x| x.message.contains("lines")));
    assert!(problems(&format!("name = A\n[dock]\ndock_items = [{}]\n", vec!["a"; 3].join(",") + &",b".repeat(MAX_LIST)))[0].message.contains("at most"));
    assert!(problems("name = A\n[dock]\ndock_items = [a, a]\n")[0].message.contains("twice"));
    assert!(problems("name = A\n[topbar]\ntopbar_items = [clock, clock]\n")[0].message.contains("twice"));
    assert!(problems("name = A\n[widgets]\nwidgets = [vitals:XL]\n")[0].message.contains("XL"));
    assert!(problems("name = A\n[icons]\nicon_set = fancy\n")[0].message.contains("built-in"));
}

#[test]
fn colours_keep_the_contrast_rules_and_the_dock_and_bar_may_not_meet() {
    let p = problems("name = Faint\n[colours]\nbase = dark\ntext = #202020\n");
    assert!(p.iter().any(|x| x.message.contains("4.5")), "{p:?}");
    let p = problems("name = Faint\n[colours]\nbase = dark\nborder = #1f222b\n");
    assert!(p.iter().any(|x| x.message.contains("border")), "{p:?}");
    let p = problems("name = Clash\n[topbar]\ntopbar_position = bottom\n[dock]\ndock = bottom\n");
    assert!(p.iter().any(|x| x.line == 3 && x.message.contains("both be at the bottom")), "{p:?}");
    assert!(parse("name = Fine\n[topbar]\ntopbar_position = bottom\n[dock]\ndock = left\n").is_ok());
    // a Look made in code is held to the same rules
    let mut l = Look::default();
    l.launcher.columns = Columns::Fixed(12);
    l.topbar.size = 0;
    assert_eq!(validate(&l).unwrap_err().len(), 2);
}

#[test]
fn comments_blank_lines_and_odd_spacing_are_fine() {
    let a = parse("\n# hello\n  name  =   Spaced out   \n\n[ icons ]   \n  icon_labels=off # no labels\n[launcher]\ncolumns = auto\n").unwrap();
    assert_eq!(a.look.name, "Spaced out");
    assert!(!a.look.icons.labels);
    assert_eq!(parse("name = A\n[widgets]\nwidgets = []\n[dock]\ndock_items = [chat, ]\n").unwrap().look.dock.items, ["chat"]);
}

/// Small deterministic generator (no dependency).
fn next(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

#[test]
fn nothing_the_parser_is_given_can_panic() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    // random bytes of every length up to a bit over the size cap, and random picks of the format's own tokens
    for round in 0..400 {
        let len = (next(&mut seed) % (MAX_BYTES + 200)) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| next(&mut seed) as u8).collect();
        let _ = parse(&String::from_utf8_lossy(&bytes));
        let toks = ["[", "]", "=", ",", "\n", "#", ":", " ", "name", "dock", "[dock]", "columns", "[icons]", "9", "-1", "999999999999", "on", "\u{fffd}", "\u{1f600}", "[a,,b]"];
        let s: String = (0..(round % 80)).map(|_| toks[(next(&mut seed) % toks.len() as u64) as usize]).collect();
        let _ = parse(&s);
    }
    // every truncation of every example and every built-in
    let mut texts = vec![MIDNIGHT.to_string(), PAPER.to_string(), OLD.to_string()];
    texts.extend(Look::builtin().iter().map(|l| l.to_text()));
    for t in texts {
        for cut in 0..=t.len() {
            let _ = parse(&String::from_utf8_lossy(&t.as_bytes()[..cut]));
        }
        // one byte of the text damaged at a time
        for at in (0..t.len()).step_by(7) {
            let mut b = t.clone().into_bytes();
            b[at] = next(&mut seed) as u8;
            let _ = parse(&String::from_utf8_lossy(&b));
        }
    }
}
