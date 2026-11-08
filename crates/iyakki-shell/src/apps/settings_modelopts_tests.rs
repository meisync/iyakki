//! Settings -> Models: the provider's own model list in a searchable chooser, the plain "not offered" warning, and the request options
//! of a model (one row each) edited by key and by click, drawn at four screen sizes. A fake HTTP client answers; no network.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_agent::iyakki_proxy::{Upstream, UpstreamResponse};
use iyakki_agent::modelopts;
use iyakki_sec::policy::Role;

const NIM: &str = "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning";
const OLD: &str = "nvidia/meta/llama2-70b";
const LIST: &str = r#"{"data":[{"id":"nvidia/nemotron-3-nano-omni-30b-a3b-reasoning"},{"id":"meta/llama-3.3-70b-instruct"},{"id":"mistralai/mistral-large"}]}"#;

struct Fake {
    status: std::sync::Mutex<u16>,
    seen: std::sync::Mutex<Vec<(String, String)>>,
}
impl Upstream for Fake {
    fn request(&self, m: &str, url: &str, _: &[(String, String)], _: &[u8]) -> Result<UpstreamResponse, String> {
        self.seen.lock().unwrap().push((m.into(), url.into()));
        let st = *self.status.lock().unwrap();
        Ok(UpstreamResponse { status: st, body: if st == 200 { LIST.as_bytes().to_vec() } else { b"{\"detail\":\"Not found\"}".to_vec() } })
    }
}

thread_local! {
    static HELD: std::cell::RefCell<Option<std::sync::MutexGuard<'static, ()>>> = const { std::cell::RefCell::new(None) };
}

/// The session's model-list cache is one global map keyed by provider, and every test here uses "nvidia": one test at a time, the lock held until the test's thread ends.
fn one_at_a_time() {
    static M: std::sync::Mutex<()> = std::sync::Mutex::new(());
    HELD.with(|h| *h.borrow_mut() = Some(M.lock().unwrap_or_else(|e| e.into_inner())));
    modelopts::forget_cached("nvidia");
}

fn rig(tag: &str) -> (AppCx, Arc<Fake>) {
    one_at_a_time();
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    let f = Arc::new(Fake { status: std::sync::Mutex::new(200), seen: Default::default() });
    cx.services = services_in(&d, f.clone());
    cx.services.on_login("root", "rootpass12", true);
    let mut p = ProviderCfg::new("nvidia", Kind::Nvidia);
    p.key_ref = Some("env:IYAKKI_TEST_MODELOPTS_UI_KEY".into());
    std::env::set_var("IYAKKI_TEST_MODELOPTS_UI_KEY", "nvapi-made-up-key-for-tests-0123456789");
    p.default_model = Some(OLD.into());
    cx.services.settings.lock().unwrap().upsert_provider(p).unwrap();
    cx.services.settings.lock().unwrap().set_chat(Some(ModelChoice { provider: "nvidia".into(), model: OLD.into() })).unwrap();
    modelopts::forget_cached("nvidia");
    (cx, f)
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}
fn typ(a: &mut SettingsApp, cx: &AppCx, s: &str) {
    for c in s.chars() {
        key(a, cx, Key::Char(c));
    }
}
fn settle(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..500 {
        a.handle(&Event::Tick, cx);
        if !a.is_busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("job did not finish");
}
fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}
fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = crate::shell::scale_for(w, h);
    a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, cx, 0);
    g
}
fn models_page(cx: &AppCx) -> SettingsApp {
    SettingsApp::new(cx).on(Tab::Models)
}

#[test]
fn list_models_asks_the_provider_and_the_pick_is_saved_like_the_models_picker_does() {
    let (cx, f) = rig("mo-list");
    let mut a = models_page(&cx);
    key(&mut a, &cx, Key::Char('L'));
    settle(&mut a, &cx);
    assert!(a.has_modal(), "the chooser is open: {}", status(&a));
    assert_eq!(f.seen.lock().unwrap().last().unwrap(), &("GET".to_string(), "https://integrate.api.nvidia.com/v1/models".to_string()));
    typ(&mut a, &cx, "nemotron");
    key(&mut a, &cx, Key::Enter);
    let chat = cx.services.settings.lock().unwrap().get().chat.clone().unwrap();
    assert_eq!((chat.provider.as_str(), chat.model.as_str()), ("nvidia", NIM));
    assert_eq!(cx.services.settings.lock().unwrap().get().providers[0].default_model.as_deref(), Some(NIM));
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.chat_model") && l.contains(NIM)));
    assert!(!cx.audit.recent(50).iter().any(|l| l.contains("made-up-key")), "the key is never logged");
    // the list is kept for the session
    assert_eq!(modelopts::cached_models("nvidia").unwrap().len(), 3);
}

#[test]
fn embedding_list_saves_as_the_embedding_model() {
    let (cx, _) = rig("mo-emb");
    let mut a = models_page(&cx);
    key(&mut a, &cx, Key::Char('E'));
    settle(&mut a, &cx);
    typ(&mut a, &cx, "mistral");
    key(&mut a, &cx, Key::Enter);
    let e = cx.services.settings.lock().unwrap().get().embedding.clone().unwrap();
    assert_eq!((e.provider.as_str(), e.model.as_str()), ("nvidia", "mistralai/mistral-large"));
}

#[test]
fn a_failed_list_says_why_in_plain_words_and_a_missing_model_is_warned_about() {
    let (cx, f) = rig("mo-fail");
    *f.status.lock().unwrap() = 404;
    let mut a = models_page(&cx);
    key(&mut a, &cx, Key::Char('L'));
    settle(&mut a, &cx);
    assert!(!a.has_modal());
    let s = status(&a);
    assert!(s.starts_with("nvidia: ") && s.contains("did not find that address"), "{s}");
    assert!(a.status().is_some_and(|x| !x.1), "shown as a failure");
    *f.status.lock().unwrap() = 401;
    key(&mut a, &cx, Key::Char('L'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("refused the key") && status(&a).contains("Settings -> Models"), "{}", status(&a));
    // a good list: the current model (the old one) is not in it
    *f.status.lock().unwrap() = 200;
    key(&mut a, &cx, Key::Char('L'));
    settle(&mut a, &cx);
    key(&mut a, &cx, Key::Esc);
    assert!(!a.has_modal());
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label.contains(&format!("The model {OLD} is not offered by this provider for this key"))), "{:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    // after picking one that is in the list the warning is gone
    cx.services.settings.lock().unwrap().set_chat(Some(ModelChoice { provider: "nvidia".into(), model: NIM.into() })).unwrap();
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(!g.hits.iter().any(|h| h.label.contains("is not offered")));
}

#[test]
fn the_options_are_rows_edited_by_key_and_saved_only_for_that_model() {
    let (cx, _) = rig("mo-opt");
    let mut a = models_page(&cx);
    key(&mut a, &cx, Key::Char('o'));
    assert!(a.has_modal());
    // first row: max_tokens
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "65536");
    key(&mut a, &cx, Key::Enter);
    assert!(status(&a).contains("max_tokens 65536"), "{}", status(&a));
    // temperature, with a value that is refused: the form stays open and says so
    key(&mut a, &cx, Key::Char('o'));
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "7");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "a refused value keeps the form open");
    key(&mut a, &cx, Key::Backspace);
    typ(&mut a, &cx, "0.6");
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal(), "{}", status(&a));
    let o = modelopts::get(cx.services.dir(), "nvidia", OLD);
    assert_eq!((o.max_tokens, o.temperature, o.top_p, o.reasoning_budget), (Some(65536), Some(0.6), None, None));
    assert!(modelopts::get(cx.services.dir(), "nvidia", NIM).is_empty(), "another model has none");
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.model_options") && l.contains("temperature")));
    // clearing
    key(&mut a, &cx, Key::Char('o'));
    for _ in 0..4 {
        key(&mut a, &cx, Key::Down);
    }
    key(&mut a, &cx, Key::Enter);
    assert!(modelopts::get(cx.services.dir(), "nvidia", OLD).is_empty(), "{}", status(&a));
}

#[test]
fn the_budget_row_says_when_the_provider_does_not_take_it_and_without_a_chat_model_there_is_a_hint() {
    let (cx, _) = rig("mo-note");
    let mut p = ProviderCfg::new("openai", Kind::OpenAi);
    p.key_ref = Some("env:IYAKKI_TEST_MODELOPTS_UI_KEY".into());
    cx.services.settings.lock().unwrap().upsert_provider(p).unwrap();
    let mut a = models_page(&cx);
    key(&mut a, &cx, Key::Down); // the second provider
    key(&mut a, &cx, Key::Char('o'));
    assert!(!a.has_modal() && status(&a).contains("Choose a chat model"), "{}", status(&a));
    cx.services.settings.lock().unwrap().update(|s| s.providers[1].default_model = Some("gpt-4o".into())).unwrap();
    key(&mut a, &cx, Key::Char('o'));
    assert!(a.has_modal());
    let rows = format!("{:?}", MoRowsProbe::rows(&cx));
    assert!(rows.contains("not used by this provider"), "{rows}");
}

struct MoRowsProbe;
impl MoRowsProbe {
    fn rows(cx: &AppCx) -> Vec<String> {
        let p = SettingsApp::providers(cx).into_iter().find(|p| p.id == "openai").unwrap();
        SettingsApp::mo_rows(cx, &p, "gpt-4o")
    }
}

#[test]
fn non_administrators_cannot_list_or_change() {
    let (mut cx, _) = rig("mo-user");
    cx.session.user = "amy".into();
    cx.session.role = Role::User;
    cx.session.policy = Role::User.template();
    let mut a = models_page(&cx);
    key(&mut a, &cx, Key::Char('L'));
    assert!(!a.is_busy() && !a.has_modal());
    key(&mut a, &cx, Key::Char('o'));
    if a.has_modal() {
        key(&mut a, &cx, Key::Enter);
        assert!(!matches!(a.modal, Some(Modal::Form(..))), "no form for a person who cannot change settings");
    }
    assert!(modelopts::get(cx.services.dir(), "nvidia", OLD).is_empty());
}

#[test]
fn the_page_has_the_option_controls_inside_the_screen_at_four_sizes_and_they_work_by_click() {
    let (cx, _) = rig("mo-draw");
    for (w, h, scale) in [(1280u32, 720u32, 1.0f32), (1920, 1080, 1.5), (1024, 600, 0.8), (800, 480, 0.62)] {
        let _ = scale;
        let scale = crate::shell::scale_for(w, h);
        let mut a = models_page(&cx);
        let g = draw(&mut a, &cx, w, h);
        for hit in g.hits.iter().filter(|x| x.rect.w > 0.0 && x.rect.h > 0.0) {
            let r = hit.rect;
            assert!(r.x >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {:?} {r:?}", hit.label);
        }
        let find = |g: &Gfx, k: char| g.hits.iter().find(|x| x.ev == Event::Key(Key::Char(k))).map(|x| x.rect);
        for k in ['L', 'o'] {
            let r = find(&g, k).unwrap_or_else(|| panic!("{w}x{h}: no control for {k}"));
            assert!(r.h >= 18.0 * scale, "{w}x{h}: {k} is {} px tall", r.h);
        }
        // a click on Options opens the chooser
        let ev = g.hits.iter().find(|x| x.ev == Event::Key(Key::Char('o'))).unwrap().ev.clone();
        a.handle(&ev, &cx);
        assert!(a.has_modal(), "{w}x{h}");
        let g = draw(&mut a, &cx, w, h);
        for hit in g.hits.iter().filter(|x| x.rect.w > 0.0 && x.rect.h > 0.0) {
            let r = hit.rect;
            assert!(r.x >= -0.5 && r.x + r.w <= w as f32 + 0.5, "{w}x{h} chooser: {:?} {r:?}", hit.label);
        }
        key(&mut a, &cx, Key::Esc);
    }
}
