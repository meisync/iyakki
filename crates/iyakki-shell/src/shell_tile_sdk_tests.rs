//! The SDK calls of a tile app at the shell level: the first-use card for the AI model, what happens after the person answers, the
//! answer reaching the page on its next turn, and the Info panel's words and numbers (docs/development/SDK-API.md, sections 4.7 and 6).

use super::tests::*;
use super::*;

const PAGE: &str = r#"<!doctype html><html><body><p id="out">waiting</p><script>
iyakki.llm.chat({messages: [{role: 'user', content: 'hello'}]}).then(
  (r) => { document.getElementById('out').textContent = 'answer ' + r.text; },
  (e) => { document.getElementById('out').textContent = 'error ' + e.code; });
</script></body></html>"#;

fn tick_until(sh: &mut Shell, g: &mut Gfx, want: &str) {
    for _ in 0..200 {
        for _ in 0..4 {
            sh.handle(&Event::Tick);
        }
        sh.render(g);
        if sh.tile_text_for_test().contains(want) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the page never showed {want:?}: {}", sh.tile_text_for_test());
}

#[test]
fn an_app_that_uses_the_model_asks_the_person_once_and_the_info_panel_says_what_it_may_do_and_what_it_used() {
    let (mut sh, _) = shell_with_users("tiles-sdk");
    let mut g = Gfx::new(1280, 720);
    let ws = std::env::temp_dir().join(format!("iyakki-sdktile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let dir = ws.join("asker");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("app.json"), r#"{"id":"asker","name":"Asker","icon":"🧩","capabilities":{"llm":true}}"#).unwrap();
    std::fs::write(dir.join("index.html"), PAGE).unwrap();
    login(&mut sh, "root", "rootpass12");
    let svc = sh.services_for_demo().clone();
    iyakki_agent::store::tilehub::add_dev(&svc, &iyakki_sec::policy::Role::Admin.template(), "root", &dir).unwrap();
    sh.refresh_custom();
    sh.set_group(Group::Apps);
    sh.render(&mut g);
    let items = sh.group_items();
    let i = items.iter().position(|it| matches!(it, LItem::App(e) if e.title == "Asker")).expect("the app's tile");
    sh.sel = i;
    sh.render(&mut g);
    let (w, h) = sh.size;
    let s = scale_for(w, h);
    let cap = launcher::per_page(w, h, s);
    let r = launcher::page_rects(w, h, s, items.len(), i / cap)[i % cap];
    sh.handle(&Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32));
    for _ in 0..8 {
        sh.handle(&Event::Tick);
    }
    sh.render(&mut g);
    // the card names the app and says what the model gets; nothing was asked of the model yet
    let card = sh.tile_card_text_for_test();
    assert!(card.contains("wants to use your AI model") && card.contains("no tools, no files") && card.contains("asked once"), "{card}");
    assert!(sh.tile_text_for_test().contains("waiting"));
    assert!(svc.usage.entries(None).is_empty());
    sh.handle(&Event::Key(Key::Char('y')));
    assert!(sh.tile_card_text_for_test().is_empty(), "the card closes");
    // no model is set up on this test system: the page hears Unavailable (with a code), and the failed call is counted for the app
    tick_until(&mut sh, &mut g, "error Unavailable");
    let e = svc.usage.entries(Some("root"));
    assert_eq!((e.len(), e[0].src.as_str(), e[0].cat.as_str(), e[0].errors, e[0].input), (1, "app:asker", "llm", 1, 0), "{e:?}");
    click_label_info(&mut sh, &mut g);
    let info = sh.tile_card_text_for_test();
    assert!(info.contains("use your AI model") && info.contains("daily limit"), "{info}");
    assert!(info.contains("Today: 1 call, 0 tokens"), "{info}");
    sh.handle(&Event::Key(Key::Esc));
    // the answer is kept in a file the page cannot reach (a new run does not ask again)
    let saved = svc.dir().join("appdata").join("tile-sdk").join("root").join("asker.json");
    assert!(std::fs::read_to_string(&saved).unwrap().contains("\"llm\": true"));
    let _ = std::fs::remove_dir_all(&ws);
}

fn click_label_info(sh: &mut Shell, g: &mut Gfx) {
    sh.render(g);
    let r = sh.hits().iter().find(|h| h.label == "Info").expect("the Info button").rect;
    sh.handle(&Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32));
    sh.render(g);
}
