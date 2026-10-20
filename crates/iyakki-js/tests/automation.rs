//! `crates/iyakki-shell/assets/automation.js` run by the in-house JavaScript engine on small fixture pages: the accessibility-style
//! snapshot (ids, roles, names, state, diff), the action scripts (click, type, press, select, scroll, hover, wait probes) with the events
//! they fire, `read_form` on dynamic forms, shadow DOM, chat-like pages and the descriptions the consent rules read.
//! The engine has no layout, so these tests cover the attribute- and event-level behaviour; `crates/iyakki-agent`'s live tests run
//! the same file in a real WebKit.

use std::sync::Arc;

use iyakki_js::modules::run_timers;
use iyakki_js::{run_page_with, JsEngine, Limits, SiteData};
use iyakki_web::fetch::Response;
use iyakki_web::{Fetch, Url};
use serde_json::{json, Value};

const BUNDLE: &str = include_str!("../../iyakki-shell/assets/automation.js");

struct NoWeb;
impl Fetch for NoWeb {
    fn get(&self, _: &Url) -> Result<Response, String> {
        Err("offline".into())
    }
}

struct Auto<'a>(&'a mut JsEngine);

impl Auto<'_> {
    fn raw(&mut self, js: &str) -> String {
        let r = self.0.eval("test", js).unwrap_or_else(|e| panic!("script failed: {e:?}\n{js}"));
        let _ = self.0.run_jobs();
        r
    }
    fn op(&mut self, op: &str, args: Value) -> Value {
        let a = args.to_string();
        let r = self.raw(&format!("__iyakkiAuto.run({}, {})", json!(op), json!(a)));
        let _ = run_timers(self.0, 5_000, 200);
        serde_json::from_str(&r).unwrap_or_else(|e| panic!("not JSON ({e}): {r}"))
    }
    fn snap(&mut self) -> Value {
        self.op("snapshot", json!({}))
    }
    fn text(&mut self) -> String {
        self.snap()["text"].as_str().unwrap().to_string()
    }
    /// The id of the first element whose line contains `needle`.
    fn id(&mut self, role: &str, name: &str) -> String {
        let s = self.snap();
        s["els"].as_array().unwrap().iter().find(|e| e["role"] == role && e["name"].as_str().unwrap().contains(name)).unwrap_or_else(|| panic!("no {role} {name} in\n{}", s["text"])).get("id").unwrap().as_str().unwrap().to_string()
    }
}

fn drive(html: &str, f: impl FnOnce(&mut Auto)) {
    let url = Url::parse("https://app.example.org/start").unwrap();
    run_page_with(html, &url, Arc::new(NoWeb), |_| true, &SiteData::default(), Limits::default(), |e| {
        let mut a = Auto(e);
        a.raw(BUNDLE);
        assert_eq!(a.raw("String(window.__iyakkiAuto.v)"), "1");
        f(&mut a);
    });
}

const LOGIN: &str = r#"<html><head><title>Sign in</title></head><body>
<h1>Welcome back</h1>
<form id="login" action="https://app.example.org/session" method="post">
  <label for="email">Email</label><input id="email" type="email" required placeholder="you@example.org">
  <label>Password <input id="pw" type="password" name="password"></label>
  <label><input id="remember" type="checkbox"> Remember me</label>
  <button type="submit" id="go">Sign in</button>
  <button type="button" id="alt" disabled>Sign in with SSO</button>
</form>
<a id="help" href="https://help.example.org/reset?token=SECRET123">Need help?</a>
<p>No account? <a href="/signup">Create one</a> in a minute.</p>
<div id="hid" style="display:none"><button>Never seen</button></div>
<input type="hidden" name="csrf" value="x">
<script>
window.log = [];
const email = document.getElementById('email');
for (const t of ['input', 'change', 'keydown', 'keyup', 'focus']) email.addEventListener(t, (e) => log.push(t + ':' + (e.inputType || e.key || '')));
document.getElementById('login').addEventListener('submit', (e) => { e.preventDefault(); log.push('submit'); });
document.getElementById('go').addEventListener('click', () => log.push('click:go'));
</script></body></html>"#;

#[test]
fn the_snapshot_lists_interactive_and_text_elements_in_reading_order_with_stable_ids() {
    drive(LOGIN, |a| {
        let s = a.snap();
        let t = s["text"].as_str().unwrap();
        assert!(t.starts_with("Page \"Sign in\" https://app.example.org/start"), "{t}");
        assert!(t.contains("First snapshot of this page."), "{t}");
        let pos = |needle: &str| t.find(needle).unwrap_or_else(|| panic!("missing {needle} in\n{t}"));
        assert!(pos("heading(1) \"Welcome back\"") < pos("textbox \"Email\""), "reading order\n{t}");
        assert!(pos("textbox \"Email\"") < pos("textbox \"Password\""));
        assert!(pos("textbox \"Password\"") < pos("checkbox \"Remember me\""));
        assert!(pos("checkbox \"Remember me\"") < pos("button \"Sign in\""));
        assert!(t.contains("[required, type=email]") || t.contains("required"), "{t}");
        assert!(t.contains("type=password"), "{t}");
        assert!(t.contains("button \"Sign in with SSO\" [disabled]"), "{t}");
        assert!(t.contains("link \"Need help?\" -> help.example.org"), "link host only\n{t}");
        assert!(!t.contains("SECRET123") && !t.contains("token="), "the address of a link is never shown beyond its host\n{t}");
        assert!(!t.contains("Never seen"), "display:none is not listed\n{t}");
        assert!(t.contains("text \"No account?\"") && t.contains("link \"Create one\" -> app.example.org"), "{t}");
        assert!(!t.contains("csrf"), "hidden inputs are not listed");
        // ids are short, unique and stable between snapshots
        let ids1: Vec<String> = s["els"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap().to_string()).collect();
        assert!(ids1.iter().all(|i| i.starts_with('e')) && ids1[0] == "e1");
        let mut dedup = ids1.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(dedup.len(), ids1.len());
        let s2 = a.snap();
        let ids2: Vec<String> = s2["els"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap().to_string()).collect();
        assert_eq!(ids1, ids2);
        assert!(s2["text"].as_str().unwrap().contains("No changes since the last snapshot."));
        assert_eq!(s["layout"], json!(false));
    });
}

#[test]
fn typing_uses_the_native_setter_and_fires_the_events_a_framework_listens_for() {
    drive(LOGIN, |a| {
        let id = a.id("textbox", "Email");
        let r = a.op("type", json!({"id": id, "text": "ann@example.org"}));
        assert_eq!(r["ok"], json!(true), "{r}");
        assert_eq!(a.raw("document.getElementById('email').value"), "ann@example.org");
        let log = a.raw("log.join(' ')");
        assert!(log.contains("focus:") && log.contains("input:insertText") && log.contains("change:"), "{log}");
        // appends by default, replaces with clear
        a.op("type", json!({"id": id, "text": "!"}));
        assert_eq!(a.raw("document.getElementById('email').value"), "ann@example.org!");
        a.op("type", json!({"id": id, "text": "bob@example.org", "clear": true}));
        assert_eq!(a.raw("document.getElementById('email').value"), "bob@example.org");
        let t = a.text();
        assert!(t.contains("value=\"bob@example.org\""), "{t}");
        assert!(t.contains("Changes since the last snapshot: 1 changed"), "{t}");
    });
}

#[test]
fn enter_in_a_field_submits_the_form_and_a_click_fires_the_whole_pointer_sequence() {
    drive(LOGIN, |a| {
        let id = a.id("textbox", "Email");
        let r = a.op("type", json!({"id": id, "text": "x@y.org", "submit": true}));
        assert_eq!(r["submitted"], json!(true), "{r}");
        assert!(a.raw("log.join(' ')").contains("submit"));
        a.raw("window.seen = []; for (const t of ['pointerdown','mousedown','pointerup','mouseup','click']) document.getElementById('go').addEventListener(t, (e) => seen.push(t));");
        let go = a.id("button", "Sign in");
        let r = a.op("click", json!({"id": go}));
        assert_eq!(r["ok"], json!(true), "{r}");
        assert_eq!(a.raw("seen.join(',')"), "pointerdown,mousedown,pointerup,mouseup,click");
        assert!(a.raw("log.join(' ')").contains("click:go"));
        // press: key events on the focused or named element
        let r = a.op("press", json!({"key": "Escape", "id": id}));
        assert_eq!(r["ok"], json!(true));
        let log = a.raw("log.join(' ')");
        assert!(log.contains("keydown:Escape") && log.contains("keyup:Escape"), "{log}");
        let r = a.op("press", json!({"key": "Hyper"}));
        assert_eq!(r["ok"], json!(false));
    });
}

#[test]
fn stale_hidden_disabled_and_file_targets_are_refused_with_a_reason() {
    let html = LOGIN.replace("</form>", "<input type=file id=f aria-label=Upload></form>");
    drive(&html, |a| {
        let r = a.op("click", json!({"id": "e999"}));
        assert_eq!(r["ok"], json!(false));
        assert_eq!(r["stale"], json!(true));
        assert!(r["error"].as_str().unwrap().contains("Take a new snapshot"));
        let sso = a.id("button", "SSO");
        let r = a.op("click", json!({"id": sso}));
        assert_eq!(r["disabled"], json!(true), "{r}");
        let f = a.id("file", "Upload");
        let r = a.op("click", json!({"id": f}));
        assert_eq!(r["file"], json!(true), "{r}");
        let r = a.op("type", json!({"id": f, "text": "x"}));
        assert_eq!(r["ok"], json!(false));
        // an element that disappears makes its id stale
        let help = a.id("link", "Need help");
        a.raw("document.getElementById('help').remove()");
        let r = a.op("click", json!({"id": help}));
        assert_eq!(r["stale"], json!(true), "{r}");
    });
}

#[test]
fn password_otp_captcha_and_card_fields_are_never_typed_by_the_model() {
    let html = r#"<form><input id=p type=password aria-label="Password"><input id=o name=otp aria-label="Code" autocomplete="one-time-code">
      <div class="g-recaptcha" data-sitekey="k"><input id=c aria-label="Answer"></div><input id=cc aria-label="Card number" autocomplete="cc-number"><input id=n aria-label="Name"></form>"#;
    drive(html, |a| {
        for (name, kind) in [("Password", "password"), ("Code", "otp"), ("Answer", "captcha"), ("Card number", "card")] {
            let id = a.id("textbox", name);
            let d = a.op("describe", json!({"id": id}));
            assert_eq!(d["el"]["secret"], json!(kind), "{name}: {d}");
            let r = a.op("type", json!({"id": id, "text": "123456"}));
            assert_eq!(r["ok"], json!(false), "{name}");
            assert_eq!(r["secret"], json!(kind));
        }
        let n = a.id("textbox", "Name");
        assert_eq!(a.op("describe", json!({"id": n}))["el"]["secret"], json!(""));
        assert_eq!(a.op("type", json!({"id": n, "text": "Ann"}))["ok"], json!(true));
        // the shell's login path: focus without a value, then the length is checked, never the content
        let p = a.id("textbox", "Password");
        let r = a.op("fill_secret_prepare", json!({"id": p}));
        assert_eq!(r["ok"], json!(true));
        a.raw("document.getElementById('p').value = 'hunter22'");
        let r = a.op("secret_check", json!({"id": p, "length": 8}));
        assert_eq!(r["matches"], json!(true));
        assert!(!r.to_string().contains("hunter22"));
        let t = a.text();
        assert!(!t.contains("hunter22") && t.contains("(hidden, 8 chars)"), "a password is never shown in a snapshot\n{t}");
    });
}

const DYNAMIC: &str = r#"<html><head><title>Shipping</title></head><body>
<form id="ship">
  <label for="country">Country</label>
  <select id="country"><option value="">Choose...</option><option value="us">United States</option><option value="in">India</option><option value="fr">France</option></select>
  <div id="statebox" hidden>
    <label for="state">State</label><input id="state" required aria-describedby="state-err">
    <span id="state-err" role="alert"></span>
  </div>
  <label for="zip">ZIP</label><input id="zip" pattern="[0-9]{5}" required>
  <button id="next" type="submit">Continue</button>
</form>
<script>
const country = document.getElementById('country');
country.addEventListener('change', () => {
  window.changes = (window.changes || 0) + 1;
  const box = document.getElementById('statebox');
  if (country.value === 'us') setTimeout(() => box.removeAttribute('hidden'), 300); else box.setAttribute('hidden', '');
});
document.getElementById('ship').addEventListener('submit', (e) => {
  e.preventDefault();
  const s = document.getElementById('state');
  if (!s.closest('[hidden]') && !s.value) { s.setAttribute('aria-invalid', 'true'); document.getElementById('state-err').textContent = 'Please enter your state'; }
  else document.body.insertAdjacentHTML('beforeend', '<h2 id="done">Step 2: Payment</h2>');
});
</script></body></html>"#;

#[test]
fn a_dynamic_form_reveals_a_field_after_a_select_and_the_snapshot_diff_says_so() {
    drive(DYNAMIC, |a| {
        let before = a.text();
        assert!(!before.contains("\"State\""), "{before}");
        let country = a.id("select", "Country");
        let r = a.op("select", json!({"id": country, "option": "United States"}));
        assert_eq!(r["ok"], json!(true), "{r}");
        assert_eq!(r["selected"], json!("United States"));
        assert_eq!(a.raw("window.changes"), "1");
        // the state field appeared (the timer ran); re-snapshot shows it with a short id and says it is new
        let after = a.text();
        assert!(after.contains("textbox \"State\""), "{after}");
        assert!(after.contains("Changes since the last snapshot: 1 new") || after.contains("new (e"), "{after}");
        assert!(after.contains("select \"Country\" value=\"United States\""), "{after}");
        // read_form lists every field with label/type/required, in order
        let f = a.op("read_form", json!({}));
        let text = f["text"].as_str().unwrap();
        assert!(text.contains("select \"Country\"") && text.contains("options=[\"Choose...\", \"United States\", \"India\", \"France\"]"), "{text}");
        assert!(text.contains("\"State\" *required"), "{text}");
        assert!(text.contains("\"ZIP\" *required") && text.contains("pattern=[0-9]{5}"), "{text}");
        assert!(text.contains("submit: ") && text.contains("\"Continue\""), "{text}");
        assert_eq!(f["forms"][0]["fields"].as_array().unwrap().len(), 3);
        // an option that does not exist lists the real ones
        let r = a.op("select", json!({"id": country, "option": "Atlantis"}));
        assert_eq!(r["ok"], json!(false));
        assert!(r["error"].as_str().unwrap().contains("India"), "{r}");
        // changing the country back hides the field again
        a.op("select", json!({"id": country, "option": "India"}));
        let again = a.text();
        assert!(!again.contains("textbox \"State\""), "{again}");
        assert!(again.contains("1 gone"), "{again}");
    });
}

#[test]
fn validation_messages_appear_in_read_form_and_a_wizard_moves_on() {
    drive(DYNAMIC, |a| {
        let country = a.id("select", "Country");
        a.op("select", json!({"id": country, "option": "us"}));
        let zip = a.id("textbox", "ZIP");
        a.op("type", json!({"id": zip, "text": "12345", "submit": true}));
        let f = a.op("read_form", json!({}));
        let text = f["text"].as_str().unwrap();
        assert!(text.contains("!! Please enter your state"), "{text}");
        assert!(f["invalid"].as_u64().unwrap() >= 1);
        let state = a.id("textbox", "State");
        a.op("type", json!({"id": state, "text": "Ohio"}));
        a.op("type", json!({"id": zip, "text": "1", "submit": true}));
        let t = a.text();
        assert!(t.contains("heading(2) \"Step 2: Payment\""), "the next step appeared\n{t}");
        // probe: text present, element present
        let p = a.op("probe", json!({"text": "Step 2", "role": "button", "name": "Continue"}));
        assert_eq!(p["text_found"], json!(true));
        assert_eq!(p["element_count"], json!(1));
        let p = a.op("probe", json!({"text": "Nonsense"}));
        assert_eq!(p["text_found"], json!(false));
    });
}

#[test]
fn find_matches_by_role_and_name_for_the_test_runner() {
    drive(LOGIN, |a| {
        let r = a.op("find", json!({"role": "button", "name": "sign in"}));
        assert_eq!(r["count"], json!(2), "{r}");
        let r = a.op("find", json!({"role": "button", "name": "Sign in", "exact": true}));
        assert_eq!(r["count"], json!(1));
        let r = a.op("find", json!({"label": "Email"}));
        assert_eq!(r["matches"][0]["role"], json!("textbox"));
        let r = a.op("find", json!({"role": "link", "name": "nothing"}));
        assert_eq!(r["count"], json!(0));
        // hidden things are never matched
        let r = a.op("find", json!({"name": "Never seen"}));
        assert_eq!(r["count"], json!(0));
    });
}

const CHAT: &str = r#"<html><head><title>Assistant chat</title></head><body>
<main>
  <div id="log" role="log" aria-label="Conversation"></div>
  <div id="box" contenteditable="true" role="textbox" aria-label="Message the assistant" data-placeholder="Ask anything"></div>
  <button id="send" aria-label="Send message">Send</button>
</main>
<script>
const log = document.getElementById('log');
function bubble(who, text) { const d = document.createElement('div'); d.className = 'msg ' + who; d.textContent = text; log.appendChild(d); }
for (let i = 1; i <= 120; i++) bubble(i % 2 ? 'user' : 'bot', 'Earlier message number ' + i + ' ' + 'lorem ipsum dolor sit amet '.repeat(4));
document.getElementById('send').addEventListener('click', () => {
  const box = document.getElementById('box');
  const t = box.textContent.trim();
  if (!t) return;
  bubble('user', t); box.textContent = '';
  setTimeout(() => bubble('bot', 'You said: ' + t + '. The capital of France is Paris.'), 800);
});
</script></body></html>"#;

#[test]
fn a_contenteditable_chat_box_takes_text_and_the_newest_messages_survive_truncation() {
    drive(CHAT, |a| {
        let t = a.text();
        assert!(t.contains("textbox \"Message the assistant\""), "{t}");
        let bx = a.id("textbox", "Message the assistant");
        let r = a.op("type", json!({"id": bx, "text": "What is the capital of France?"}));
        assert_eq!(r["ok"], json!(true), "{r}");
        assert!(r["mode"].as_str().unwrap().starts_with("contenteditable"), "{r}");
        assert_eq!(a.raw("document.getElementById('box').textContent"), "What is the capital of France?");
        let send = a.id("button", "Send message");
        assert_eq!(a.op("click", json!({"id": send}))["ok"], json!(true));
        // the 800 ms timer ran in virtual time: the answer bubble exists; the snapshot is cut to the budget but keeps the newest messages
        let s = a.op("snapshot", json!({"max": 3000}));
        let t = s["text"].as_str().unwrap();
        assert!(t.len() <= 3200, "{} chars", t.len());
        assert!(t.contains("You said: What is the capital of France?. The capital of France is Paris."), "the newest message is kept\n{t}");
        assert!(t.contains("earlier messages left out"), "{t}");
        assert!(!t.contains("Earlier message number 1 "), "the oldest were dropped first\n{t}");
        assert!(t.contains("Message the assistant"), "interactive elements stay\n{t}");
        assert_eq!(s["truncated"], json!(true));
        let tail = a.op("chat_tail", json!({"n": 2}));
        let last = tail["containers"][0].as_array().unwrap().last().unwrap().as_str().unwrap().to_string();
        assert!(last.contains("Paris"), "{tail}");
        // the full snapshot still fits the default budget
        let full = a.snap();
        assert!(full["text"].as_str().unwrap().len() <= 12_300);
    });
}

#[test]
fn consent_facts_mark_payment_destructive_and_submitting_controls() {
    let html = r#"<h1>Checkout</h1><form action="https://pay.example.net/charge">
      <label>Card number <input name="cardnumber" autocomplete="cc-number"></label><label>CVV <input name="cvv"></label>
      <button id=pay type=submit>Place order</button><button id=cancel type=button>Back to cart</button></form>
      <button id=del>Delete account</button><a href="/orders">My orders</a><button id=sav type=button>Save draft</button>"#;
    drive(html, |a| {
        let pay = a.id("button", "Place order");
        let d = a.op("describe", json!({"id": pay}));
        assert_eq!(d["el"]["pay"], json!(true), "{d}");
        assert_eq!(d["el"]["submits"], json!(true));
        assert_eq!(d["el"]["form_action_host"], json!("pay.example.net"));
        let del = a.id("button", "Delete account");
        assert_eq!(a.op("describe", json!({"id": del}))["el"]["destructive"], json!(true));
        let back = a.id("button", "Back to cart");
        let d = a.op("describe", json!({"id": back}));
        assert_eq!(d["el"]["pay"], json!(false));
        assert_eq!(d["el"]["submits"], json!(false));
        let sav = a.id("button", "Save draft");
        let d = a.op("describe", json!({"id": sav}));
        assert_eq!((d["el"]["pay"].clone(), d["el"]["destructive"].clone()), (json!(false), json!(false)));
        let l = a.id("link", "My orders");
        assert_eq!(a.op("describe", json!({"id": l}))["el"]["role"], json!("link"));
    });
}

#[test]
fn open_shadow_roots_are_traversed_and_their_buttons_clickable() {
    let html = r#"<h1>Components</h1><x-card id="card"></x-card><iframe id="fr" src="https://ads.example.net/frame" title="Ad"></iframe>
      <script>
      const host = document.getElementById('card');
      const root = host.attachShadow({ mode: 'open' });
      root.innerHTML = '<p>Inside the shadow</p><button id="sb">Shadow action</button><label>Note <input id="si"></label>';
      root.getElementById ? 0 : 0;
      window.clicks = 0;
      root.querySelector('#sb').addEventListener('click', () => { window.clicks++; });
      </script>"#;
    drive(html, |a| {
        let t = a.text();
        assert!(t.contains("text \"Inside the shadow\""), "{t}");
        assert!(t.contains("button \"Shadow action\""), "{t}");
        assert!(t.contains("(frame from ads.example.net"), "a frame of another origin is named, its content is not read\n{t}");
        let b = a.id("button", "Shadow action");
        let r = a.op("click", json!({"id": b}));
        assert_eq!(r["ok"], json!(true), "{r}");
        assert_eq!(a.raw("String(window.clicks)"), "1");
    });
}

#[test]
fn scroll_hover_and_nav_helpers_answer_sensibly_without_layout() {
    drive(LOGIN, |a| {
        let r = a.op("scroll", json!({"dir": "down"}));
        assert_eq!(r["ok"], json!(true), "{r}");
        let id = a.id("link", "Need help");
        let r = a.op("hover", json!({"id": id}));
        assert_eq!(r["ok"], json!(true));
        let r = a.op("scroll", json!({"id": id}));
        assert_eq!(r["scrolled"], json!("into view"));
        assert_eq!(a.op("nonsense", json!({}))["ok"], json!(false));
        assert_eq!(a.raw("typeof __iyakkiAuto.run('probe', '{}')"), "string");
    });
}

#[test]
fn the_page_cannot_make_the_bundle_run_its_strings_and_selectors_stay_internal() {
    // an element whose name looks like code or a selector is only data
    let html = r#"<button id=b aria-label="'); window.pwned = 1; ('">x</button><input aria-label='a"]; window.pwned=2; ["' id=i>"#;
    drive(html, |a| {
        let t = a.text();
        assert!(t.contains("window.pwned"), "{t}");
        let id = a.id("button", "pwned");
        a.op("click", json!({"id": id}));
        let i = a.id("textbox", "pwned");
        a.op("type", json!({"id": i, "text": "'); window.pwned = 3;"}));
        a.op("find", json!({"name": "'); window.pwned = 4; ('"}));
        assert_eq!(a.raw("String(window.pwned)"), "undefined");
    });
}
