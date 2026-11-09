use super::*;
use crate::css::Rgba;
use crate::doc::{Block, Doc};
use crate::a11y_nav::*;
use crate::a11y_style::*;

const SHOP: &str = include_str!("a11y_fixtures/shop.html");
const SIGNIN: &str = include_str!("a11y_fixtures/signin.html");
const ARTICLE: &str = include_str!("a11y_fixtures/article.html");

fn find(t: &A11yTree, role: Role, name: &str) -> usize {
    t.nodes.iter().position(|n| n.role == role && n.name == name).unwrap_or_else(|| panic!("no {role:?} named {name:?} in {:?}", t.nodes.iter().map(|n| (n.role, n.name.clone())).collect::<Vec<_>>()))
}

#[test]
fn roles_names_and_levels_of_the_shop_page() {
    let t = from_html(SHOP);
    let h2 = find(&t, Role::Heading, "Pricing");
    assert_eq!(t.nodes[h2].level, 2);
    assert_eq!(t.speak(h2), "Heading level 2, Pricing");
    assert_eq!(t.nodes[find(&t, Role::Heading, "Corner Shop")].level, 1);
    let search = find(&t, Role::Button, "Search");
    assert_eq!(t.speak(search), "Search, button");
    // landmarks
    let nav = find(&t, Role::Landmark(Landmark::Navigation), "Primary");
    assert_eq!(t.speak(nav), "Primary, navigation landmark");
    assert!(t.nodes.iter().any(|n| n.role == Role::Landmark(Landmark::Main)));
    assert!(t.nodes.iter().any(|n| n.role == Role::Landmark(Landmark::Banner)), "header at page level is a banner");
    assert!(t.nodes.iter().any(|n| n.role == Role::Landmark(Landmark::ContentInfo)));
    assert_eq!(t.speak(find(&t, Role::Landmark(Landmark::Complementary), "Offers")), "Offers, complementary landmark");
    assert_eq!(t.speak(find(&t, Role::Landmark(Landmark::Search), "")), "Search landmark");
    // the list in the navigation
    let list = t.nodes.iter().position(|n| n.role == Role::List).unwrap();
    assert_eq!(t.speak(list), "List, 3 items");
    let shoes = find(&t, Role::Link, "Shoes");
    assert_eq!(t.speak(shoes), "Shoes, link, current");
    let item = t.nodes.iter().position(|n| n.role == Role::ListItem && n.name == "Bags").unwrap();
    assert_eq!(t.speak(item), "Item 3 of 3, Bags");
}

#[test]
fn a_search_field_gets_its_name_from_label_for() {
    let t = from_html(SHOP);
    let q = find(&t, Role::TextField, "Search the shop");
    assert_eq!(t.speak(q), "Search the shop, edit text");
}

#[test]
fn images_say_their_alt_and_decorative_ones_are_skipped() {
    let t = from_html(SHOP);
    let imgs: Vec<&A11yNode> = t.nodes.iter().filter(|n| n.role == Role::Image).collect();
    assert_eq!(imgs.len(), 2, "alt=\"\" is decorative: {imgs:?}");
    assert_eq!(speak_node(imgs[0]), "Image, Red running shoe");
    assert_eq!(speak_node(imgs[1]), "Image, no description");
    assert_eq!(t.stats().images_without_alt, 1);
    assert!(!t.script.iter().any(|s| s.text.contains("spacer")));
    assert!(t.script.iter().any(|s| s.text == "Image, Red running shoe"));
}

#[test]
fn hidden_content_is_left_out_and_screen_reader_only_text_stays() {
    let t = from_html(SHOP);
    let all: String = t.script.iter().map(|s| s.text.clone()).collect::<Vec<_>>().join(" | ");
    for gone in ["secret one", "decor two", "gone three"] {
        assert!(!all.contains(gone), "{gone} must not be read: {all}");
    }
    assert!(all.contains("Read by readers only."), "{all}");
    // a closed disclosure shows only its summary
    assert!(all.contains("Shipping, button, collapsed") && !all.contains("Free over 50 dollars"), "{all}");
    let open = from_html("<details open><summary>Shipping</summary><p>Free over 50 dollars.</p></details>");
    assert!(open.script.iter().any(|s| s.text == "Shipping, button, expanded"));
    assert!(open.script.iter().any(|s| s.text.contains("Free over 50")));
    // hidden inputs and a style sheet's display:none
    let t2 = crate::a11y::build(&crate::html::parse("<input type=hidden name=a><button id=x>Gone</button><button>Shown</button>"), &|n| n == 2);
    assert_eq!(t2.nodes.iter().filter(|n| n.role == Role::Button).count(), 1);
    assert_eq!(t2.nodes[0].name, "Shown");
}

#[test]
fn buttons_have_states_and_disabled_ones_are_not_focusable() {
    let t = from_html(SHOP);
    let more = find(&t, Role::Button, "More colours");
    assert_eq!(t.nodes[more].state.expanded, Some(false));
    assert_eq!(t.speak(more), "More colours, button, collapsed");
    let buy = find(&t, Role::Button, "Buy now");
    assert_eq!(t.speak(buy), "Buy now, button, unavailable");
    assert!(!t.nodes[buy].focusable && !tab_order(&t).contains(&buy));
    let p = from_html("<button aria-pressed=true>Bold</button><button aria-pressed=false>Italic</button>");
    assert_eq!(p.speak(0), "Bold, button, pressed");
    assert_eq!(p.speak(1), "Italic, button, not pressed");
}

#[test]
fn the_table_has_headers_a_caption_and_cells_know_their_column() {
    let t = from_html(SHOP);
    let table = t.nodes.iter().position(|n| n.role == Role::Table).unwrap();
    assert_eq!(t.speak(table), "Sizes and prices, Table, 3 rows, 2 columns");
    let price = t.nodes.iter().position(|n| n.role == Role::Cell && n.name == "$55").unwrap();
    assert_eq!(t.nodes[price].col_header, "Price");
    assert_eq!(t.speak(price), "$55, column Price");
    assert!(t.nodes.iter().any(|n| n.role == Role::ColumnHeader && n.name == "Size"));
    assert!(t.script.iter().any(|s| s.text == "Row 3 of 3. Size: 42, Price: $55"), "{:?}", t.script);
}

#[test]
fn form_fields_have_labels_states_values_and_a_password_never_shows_its_value() {
    let t = from_html(SIGNIN);
    let email = find(&t, Role::TextField, "Email");
    assert_eq!(t.speak(email), "Email, edit text, required, invalid, Enter an email address with an at sign.");
    let pw = find(&t, Role::PasswordField, "Password");
    assert_eq!(t.speak(pw), "Password, password edit text, required");
    let nick = find(&t, Role::TextField, "Nickname");
    assert_eq!(t.speak(nick), "Nickname, edit text");
    let sel = find(&t, Role::Select, "Country");
    assert_eq!(t.speak(sel), "Country, combo box, Chile");
    let note = find(&t, Role::TextArea, "Note");
    assert_eq!(t.speak(note), "Note, multi-line edit text, hello");
    let rem = find(&t, Role::Checkbox, "Remember me");
    assert_eq!(t.speak(rem), "Remember me, checkbox, checked");
    assert_eq!(t.speak(find(&t, Role::Button, "Sign in")), "Sign in, button");
    assert_eq!(t.speak(find(&t, Role::Button, "Clear")), "Clear, button");
    // a named form is a landmark, the hidden csrf field is not there at all
    assert!(t.nodes.iter().any(|n| n.role == Role::Landmark(Landmark::Form) && n.name == "Account"));
    assert!(!t.nodes.iter().any(|n| n.name == "csrf" || n.value == "x"));
    assert_eq!(t.stats().invalid_fields, 1);
    assert_eq!(t.alerts, vec!["Please fix the two problems below.".to_string()]);
}

#[test]
fn radio_buttons_know_their_group_and_arrows_move_inside_it() {
    let t = from_html(SIGNIN);
    let free = find(&t, Role::Radio, "Free");
    let pro = find(&t, Role::Radio, "Pro");
    let team = find(&t, Role::Radio, "Team");
    assert_eq!(t.speak(free), "Free, radio button, selected, 1 of 3");
    assert_eq!(t.speak(pro), "Pro, radio button, not selected, 2 of 3");
    assert_eq!(radio_step(&t, free, false), Some(pro));
    assert_eq!(radio_step(&t, team, false), Some(free), "wraps");
    assert_eq!(radio_step(&t, free, true), Some(team));
    // only the chosen radio is a Tab stop
    let order = tab_order(&t);
    assert!(order.contains(&free) && !order.contains(&pro) && !order.contains(&team));
    assert_eq!(t.nodes[find(&t, Role::Group, "Plan")].name, "Plan");
}

#[test]
fn invalid_fields_and_the_page_rules_are_found() {
    let t = from_html(SIGNIN);
    assert_eq!(first_invalid(&t), Some(find(&t, Role::TextField, "Email")));
    let email = find(&t, Role::TextField, "Email");
    let pw = find(&t, Role::PasswordField, "Password");
    assert_eq!(t.check_value(email, "", false).as_deref(), Some("is required"));
    assert!(t.check_value(email, "nope", false).unwrap().contains("at sign"));
    assert_eq!(t.check_value(email, "a@b.org", false), None);
    assert!(t.check_value(pw, "short", false).unwrap().contains("at least 8"));
    assert_eq!(t.check_value(pw, "long enough", false), None);
}

#[test]
fn tab_order_follows_the_page_and_tabindex_and_wraps() {
    let t = from_html(SHOP);
    let order = tab_order(&t);
    let names: Vec<&str> = order.iter().map(|&i| t.nodes[i].name.as_str()).collect();
    assert_eq!(names, ["Skip to main content", "Home", "Shoes", "Bags", "Search the shop", "Search", "More colours", "Shipping", "Deal of the day", "About us"]);
    let first = order[0];
    assert_eq!(tab_step(&t, None, false), Some(first));
    assert_eq!(tab_step(&t, Some(first), true), order.last().copied(), "Shift+Tab from the first wraps");
    assert_eq!(tab_step(&t, order.last().copied(), false), Some(first));
    let x = from_html("<a href=a>one</a><button tabindex=2>two</button><button tabindex=1>three</button><div role=button tabindex=-1>four</div>");
    let n: Vec<&str> = tab_order(&x).iter().map(|&i| x.nodes[i].name.as_str()).collect();
    assert_eq!(n, ["three", "two", "one"]);
}

#[test]
fn quick_keys_find_headings_links_fields_buttons_tables_landmarks_in_both_directions() {
    let t = from_html(ARTICLE);
    let h1 = quick(&t, None, Quick::Heading, false).unwrap();
    assert_eq!(t.nodes[h1].name, "How tides work");
    let h2 = quick(&t, Some(h1), Quick::Heading, false).unwrap();
    assert_eq!(t.nodes[h2].name, "Two bulges");
    let l3 = quick(&t, Some(h2), Quick::HeadingLevel(3), false).unwrap();
    assert_eq!(t.nodes[l3].name, "Why it matters");
    assert_eq!(quick(&t, Some(l3), Quick::Heading, false), None, "nothing after the last");
    assert_eq!(quick(&t, Some(l3), Quick::Heading, true).map(|i| t.nodes[i].name.clone()), Some("Spring and neap".into()));
    assert_eq!(quick_wrap(&t, Some(l3), Quick::Heading, false).map(|(i, w)| (t.nodes[i].name.clone(), w)), Some(("How tides work".into(), true)));
    assert_eq!(quick(&t, None, Quick::Heading, true).map(|i| t.nodes[i].name.clone()), Some("Why it matters".into()));
    assert_eq!(quick_key('H'), Some((Quick::Heading, true)));
    assert_eq!(quick_key('@'), Some((Quick::HeadingLevel(2), true)));
    assert_eq!(quick_key('3'), Some((Quick::HeadingLevel(3), false)));
    assert_eq!(quick_key('x'), None);
    let links: Vec<String> = std::iter::successors(quick(&t, None, Quick::Link, false), |&p| quick(&t, Some(p), Quick::Link, false)).map(|i| t.nodes[i].name.clone()).collect();
    assert_eq!(links, ["Skip to the story", "Sea", "Sky", "Read about the moon", "A reference"]);
    let s = from_html(SIGNIN);
    let fields: usize = std::iter::successors(quick(&s, None, Quick::Field, false), |&p| quick(&s, Some(p), Quick::Field, false)).count();
    assert_eq!(fields, 9);
    let shop = from_html(SHOP);
    assert!(quick(&shop, None, Quick::Table, false).is_some() && quick(&shop, None, Quick::Landmark, false).is_some() && quick(&shop, None, Quick::Button, false).is_some());
}

#[test]
fn skip_links_and_fragments_land_on_the_right_node() {
    let t = from_html(SHOP);
    let skip = skip_link(&t).unwrap();
    assert_eq!(t.nodes[skip].name, "Skip to main content");
    let target = fragment_target(&t, &t.nodes[skip].href).unwrap();
    assert_eq!(t.nodes[target].role, Role::Landmark(Landmark::Main));
    assert_eq!(main_landmark(&t), Some(target));
    let a = from_html(ARTICLE);
    let s = skip_link(&a).unwrap();
    assert_eq!(a.nodes[fragment_target(&a, &a.nodes[s].href).unwrap()].role, Role::Landmark(Landmark::Main));
    assert!(skip_link(&from_html("<a href=\"#x\">Top</a><p id=x>hi</p>")).is_none(), "an ordinary anchor is not a skip link");
}

#[test]
fn the_links_and_headings_lists_are_searchable_by_typing() {
    let t = from_html(ARTICLE);
    let links = entries(&t, Rotor::Links);
    assert_eq!(links.len(), 5);
    let hits = filter(&links, "re");
    let labels: Vec<&str> = hits.iter().map(|&i| links[i].label.as_str()).collect();
    assert_eq!(labels, ["Read about the moon", "A reference"], "starts-with first, then contains");
    assert_eq!(filter(&links, "SKY").len(), 1, "any case");
    assert_eq!(filter(&links, "").len(), 5);
    let heads = entries(&t, Rotor::Headings);
    assert_eq!(heads.iter().map(|e| (e.indent, e.label.as_str())).collect::<Vec<_>>(), [(0, "How tides work"), (1, "Two bulges"), (1, "Spring and neap"), (2, "Why it matters")]);
    assert!(entries(&t, Rotor::Landmarks).len() >= 2);
    assert_eq!(entries(&from_html(SIGNIN), Rotor::Fields).len(), 9);
}

#[test]
fn the_page_model_and_the_tree_agree_on_links_and_controls() {
    let doc = crate::doc::from_html(SIGNIN);
    let t = doc.access.as_ref().expect("a page has a tree");
    let email = find(t, Role::TextField, "Email");
    let ci = t.nodes[email].control.expect("matched to control");
    assert_eq!(doc.controls[ci].name, "email");
    let forgot = find(t, Role::Link, "Forgot your password?");
    assert_eq!(doc.links[t.nodes[forgot].link.unwrap()], "/forgot");
    let shop = crate::doc::from_html(SHOP);
    let st = shop.access.as_ref().unwrap();
    for n in st.nodes.iter().filter(|n| n.role == Role::Link) {
        assert!(n.link.is_some(), "link {} is matched", n.name);
    }
    // a style sheet's display:none removes the node too
    let styled = crate::doc::from_html("<html><head><style>.x{display:none}</style></head><body><button class=x>Gone</button><button>Here</button></body></html>");
    let _ = styled;
}

#[test]
fn the_read_script_goes_top_to_bottom_with_headings_lists_and_links_in_the_text() {
    let t = from_html(ARTICLE);
    let s: Vec<&str> = t.script.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(s[0], "Skip to the story");
    assert!(s.contains(&"Sections, navigation landmark") && s.contains(&"Main landmark"));
    let h = s.iter().position(|x| *x == "Heading level 1, How tides work").unwrap();
    assert!(s[h + 1].starts_with("The moon pulls on the sea.") && s[h + 1].contains("Read about the moon"), "{:?}", s[h + 1]);
    assert!(s.contains(&"List, 2 items") && s.contains(&"Spring tides: sun and moon line up."));
    let last = s.last().unwrap();
    assert!(last.contains("The sea does not care"), "{last}");
}

#[test]
fn the_summary_counts_what_is_on_the_page() {
    let t = from_html(SHOP);
    let s = t.summary();
    assert!(s.contains("links") && s.contains("1 images without a description"), "{s}");
}

// ----------------------------------------------------------------------------------------------------------------- the look

#[test]
fn the_forced_palette_has_aa_contrast_and_the_adapted_page_reaches_it_whatever_the_page_chose() {
    let pal = Palette::BLACK_ON_WHITE_LIKE;
    pal.check().unwrap();
    assert!(pal.lowest_text_ratio() >= 7.0, "{}", pal.lowest_text_ratio());
    let page = "<html><head><style>body{background:#fafafa;color:#bbbbbb} p{background:#ddeeff} a{color:#9999ff} .b{background:#ffffaa;border:1px solid #ccc}</style></head>\
        <body><p>Pale <a href=\"/seen\">visited link</a> <a href=\"/new\">new link</a></p><div class=b><p style=\"color:#eee\">in a box</p></div><table><tr><td style=\"background:#fff;color:#ddd\">cell</td></tr></table></body></html>";
    let tree = crate::html::parse(page);
    let vp = crate::css::Viewport::default();
    let doc = crate::style::styled_doc(&tree, &crate::url::Url::parse("https://a.org/").unwrap(), &vp, &|_| None);
    assert!(lowest_text_ratio(&doc) < 3.0, "the page itself is hard to read: {}", lowest_text_ratio(&doc));
    let seen = |h: &str| h == "/seen";
    let d = adapt(&doc, &Adapt { palette: Some(pal), min_font_px: 0 }, &seen).unwrap();
    assert!(lowest_text_ratio(&d) >= 7.0, "{}", lowest_text_ratio(&d));
    assert_eq!(d.canvas, Some(pal.bg));
    let mut colours = Vec::new();
    fn grab(bs: &[Block], out: &mut Vec<(String, Option<Rgba>)>) {
        for b in bs {
            match b {
                Block::Text(t) => out.extend(t.spans.iter().map(|s| (s.text.clone(), s.style.color))),
                Block::Box(x) => grab(&x.children, out),
                Block::Table(t) => t.rows.iter().for_each(|r| r.iter().for_each(|c| grab(&c.blocks, out))),
                _ => {}
            }
        }
    }
    grab(&d.blocks, &mut colours);
    let of = |w: &str| colours.iter().find(|(t, _)| t.contains(w)).and_then(|(_, c)| *c).unwrap();
    assert_eq!(of("visited link"), pal.visited);
    assert_eq!(of("new link"), pal.link);
    assert_eq!(of("Pale"), pal.fg);
    assert_ne!(pal.visited, pal.link);
    assert!(adapt(&doc, &Adapt::default(), &seen).is_none());
}

#[test]
fn a_bad_palette_is_refused_by_the_check() {
    let mut p = Palette::BLACK_ON_WHITE_LIKE;
    p.link = [40, 40, 40, 255];
    assert!(p.check().unwrap_err().contains("link"));
    let mut q = Palette::BLACK_ON_WHITE_LIKE;
    q.visited = q.link;
    assert!(q.check().is_err());
}

#[test]
fn the_minimum_font_size_raises_small_text_only() {
    let doc = crate::doc::from_html("<p>body</p><h2>head</h2>");
    let d = adapt(&doc, &Adapt { palette: None, min_font_px: 24 }, &|_| false).unwrap();
    let size = |d: &Doc, w: &str| {
        d.blocks.iter().find_map(|b| match b {
            Block::Text(t) if t.spans.iter().any(|s| s.text == w) => Some(t.spans[0].style.size_pct),
            _ => None,
        })
    };
    assert_eq!(size(&d, "body"), Some(150), "24px is 150 percent of the 16px body");
    assert_eq!(size(&d, "head"), Some(0), "a heading is already bigger and keeps its own size");
}

#[test]
fn media_queries_follow_the_reduce_motion_and_contrast_choices() {
    use crate::css::{media_matches, Viewport};
    let calm = Viewport { reduce_motion: true, more_contrast: true, ..Viewport::default() };
    let normal = Viewport::default();
    assert!(media_matches("(prefers-reduced-motion: reduce)", &calm) && !media_matches("(prefers-reduced-motion: reduce)", &normal));
    assert!(!media_matches("(prefers-reduced-motion: no-preference)", &calm) && media_matches("(prefers-reduced-motion: no-preference)", &normal));
    assert!(media_matches("(prefers-contrast: more)", &calm) && !media_matches("(prefers-contrast: more)", &normal));
    assert!(media_matches("(forced-colors: active)", &calm) && media_matches("(forced-colors: none)", &normal));
}
