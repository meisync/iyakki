//! Tests of the voice tool catalogue and dispatcher: fake tools, a fake screen (the only thing that can approve), and the ratchet over
//! the chat's real tool set of the agent crate.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use super::*;
use crate::chat::ToolSpec;

pub(crate) fn def(name: &str, risk: Risk, untrusted: bool, props: Value) -> ToolDef {
    ToolDef { spec: ToolSpec { name: name.into(), description: format!("{name} does a thing. More words."), schema: json!({"type": "object", "properties": props, "additionalProperties": false, "$schema": "x"}) }, risk, untrusted }
}

pub(crate) struct FakeTools {
    pub defs: Vec<ToolDef>,
    pub ran: Mutex<Vec<String>>,
    pub granted: Vec<String>,
}

impl FakeTools {
    pub fn standard() -> FakeTools {
        FakeTools {
            defs: vec![
                def("radio_search", Risk::Safe, false, json!({"query": {"type": "string"}})),
                def("radio_play", Risk::Ask, false, json!({"query": {"type": "string"}})),
                def("gemini_voice_set", Risk::Ask, false, json!({"enabled": {"type": "boolean"}})),
                def("web_fetch", Risk::Safe, true, json!({"url": {"type": "string"}})),
                def("ask_user", Risk::Safe, false, json!({})),
                def("blocked_by_policy", Risk::Safe, false, json!({})),
                def("radio_status", Risk::Safe, false, json!({})),
            ],
            ran: Mutex::new(Vec::new()),
            granted: vec![],
        }
    }
}

impl Tools for FakeTools {
    fn defs(&self) -> Vec<ToolDef> {
        self.defs.clone()
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        self.ran.lock().unwrap().push(format!("{name} {args}"));
        match name {
            "radio_search" => Ok((1..=60).map(|i| format!("station {i}: a long station name number {i}")).collect::<Vec<_>>().join("\n")),
            "web_fetch" => Ok("page text".into()),
            "radio_play" => Ok("Playing jazz.".into()),
            _ => Ok("done".into()),
        }
    }
    fn describe(&self, name: &str, _: &Value) -> String {
        format!("Run {name}?")
    }
    fn refusal(&self, name: &str, _: &Value) -> Option<String> {
        (name == "blocked_by_policy").then(|| "policy says no".to_string())
    }
    fn pre_approved(&self, name: &str) -> bool {
        self.granted.iter().any(|g| g == name)
    }
}

#[derive(Default)]
pub(crate) struct FakeScreen {
    pub present: Mutex<bool>,
    pub shown: Mutex<Vec<Card>>,
    pub pressed: Mutex<Vec<(String, bool)>>,
    pub withdrawn: Mutex<Vec<String>>,
}

impl FakeScreen {
    pub fn at_screen() -> Arc<FakeScreen> {
        let s = FakeScreen::default();
        *s.present.lock().unwrap() = true;
        Arc::new(s)
    }
    /// The person's press on the card.
    pub fn press(&self, allow: bool) {
        let id = self.shown.lock().unwrap().last().unwrap().id.clone();
        self.pressed.lock().unwrap().push((id, allow));
    }
}

impl Screen for FakeScreen {
    fn present(&self) -> bool {
        *self.present.lock().unwrap()
    }
    fn show(&self, c: &Card) {
        self.shown.lock().unwrap().push(c.clone());
    }
    fn answer(&self, id: &str) -> Option<bool> {
        self.pressed.lock().unwrap().iter().find(|(i, _)| i == id).map(|(_, a)| *a)
    }
    fn withdraw(&self, id: &str) {
        self.withdrawn.lock().unwrap().push(id.to_string());
    }
}

pub(crate) fn rig(tools: FakeTools, screen: Arc<FakeScreen>, auto: bool) -> (Dispatcher, Arc<FakeTools>, Arc<Mutex<Vec<String>>>, Arc<AtomicU64>) {
    let tools = Arc::new(tools);
    let audits = Arc::new(Mutex::new(Vec::new()));
    let a2 = audits.clone();
    let clock = Arc::new(AtomicU64::new(1000));
    let c2 = clock.clone();
    let opts = Opts { auto: Arc::new(move || auto), now_ms: Arc::new(move || c2.load(Ordering::SeqCst)), approval_ms: 60_000, result_chars: 200, max_calls: 40 };
    let d = Dispatcher::new(tools.clone(), screen, Arc::new(move |a, t| a2.lock().unwrap().push(format!("{a} {t}"))), opts);
    (d, tools, audits, clock)
}

#[test]
fn declarations_are_google_shaped_cut_short_and_leave_out_what_voice_cannot_use() {
    let t = FakeTools::standard();
    let c = Catalogue::build(&t.defs);
    assert!(!c.declared.contains(&"ask_user".to_string()) && c.excluded.contains(&"ask_user".to_string()));
    assert!(c.declared.contains(&"radio_play".to_string()) && c.missing(&t.defs).is_empty() && !c.routed_any());
    let d = c.decls.iter().find(|d| d["name"] == "radio_search").unwrap();
    assert_eq!(d["parameters"], json!({"type": "object", "properties": {"query": {"type": "string"}}}), "additionalProperties and $schema are dropped");
    let none = c.decls.iter().find(|d| d["name"] == "radio_status").unwrap();
    assert!(none.get("parameters").is_none(), "a tool without arguments has no parameters");
    let long = def("x_long", Risk::Safe, false, json!({}));
    let mut l = long.clone();
    l.spec.description = "w ".repeat(500);
    assert!(declaration(&l)["description"].as_str().unwrap().chars().count() <= 300);
    assert!(declaration(&def("bad name!", Risk::Safe, false, json!({}))).is_object());
    assert!(Catalogue::build(&[def("bad name!", Risk::Safe, false, json!({}))]).declared.is_empty(), "a name Google cannot take is left out");
}

#[test]
fn many_tools_use_the_router_and_every_tool_stays_reachable() {
    let mut defs: Vec<ToolDef> = (0..80).map(|i| def(&format!("misc_tool_{i}"), Risk::Safe, false, json!({"a": {"type": "string"}}))).collect();
    defs.push(def("radio_play", Risk::Ask, false, json!({})));
    defs.push(def("calendar_add", Risk::Ask, false, json!({})));
    let c = Catalogue::build(&defs);
    assert!(c.routed_any() && c.declared.len() < MAX_DECLARED);
    assert!(c.declared.contains(&"radio_play".to_string()) && c.declared.contains(&"calendar_add".to_string()), "the first set");
    assert!(c.decls.iter().any(|d| d["name"] == "find_tools") && c.decls.iter().any(|d| d["name"] == "call_tool"));
    assert!(c.missing(&defs).is_empty(), "nothing is lost: {:?}", c.missing(&defs));
    assert!(c.decls.iter().map(|d| d.to_string().len()).sum::<usize>() <= MAX_DECL_BYTES);
    assert!(c.log_line().contains("through find_tools/call_tool"));
    // through the dispatcher
    let tools = FakeTools { defs, ran: Mutex::new(vec![]), granted: vec![] };
    let (mut d, t, _, _) = rig(tools, FakeScreen::at_screen(), false);
    let r = d.call("1", "find_tools", &json!({"area": "tool_7"}));
    assert!(r.response["output"].as_str().unwrap().contains("misc_tool_7"), "{:?}", r.response);
    let r = d.call("2", "call_tool", &json!({"name": "misc_tool_7", "arguments": {"a": "x"}}));
    assert!(matches!(r.note, ToolNote::Ran { ok: true, .. }), "{:?}", r.note);
    assert!(t.ran.lock().unwrap()[0].starts_with("misc_tool_7"));
    assert!(matches!(d.call("3", "call_tool", &json!({"name": "call_tool"})).note, ToolNote::Refused { .. }));
    assert!(matches!(d.call("4", "call_tool", &json!({"name": "nope"})).note, ToolNote::Refused { .. }));
}

#[test]
fn safe_tools_run_through_the_wrappers_and_long_results_are_short_for_speaking() {
    let (mut d, t, audits, _) = rig(FakeTools::standard(), FakeScreen::at_screen(), false);
    let s = d.call("1", "radio_search", &json!({"query": "jazz"}));
    let out = s.response["output"].as_str().unwrap();
    assert!(out.chars().count() < 330 && out.contains("more lines") && out.contains("on the screen"), "{out}");
    let ToolNote::Ran { ok, by, full, .. } = &s.note else { panic!("{:?}", s.note) };
    assert!(*ok && by == "auto" && full.lines().count() == 60, "the whole text goes to the screen");
    assert_eq!(t.ran.lock().unwrap().len(), 1);
    // untrusted text is labelled
    let w = d.call("2", "web_fetch", &json!({"url": "x"}));
    assert!(w.response["note"].as_str().unwrap().contains("untrusted"));
    // policy refusal and unknown and excluded: nothing runs
    for n in ["blocked_by_policy", "no_such_tool", "ask_user"] {
        let r = d.call("3", n, &json!({}));
        assert!(matches!(r.note, ToolNote::Refused { .. }) && r.response.get("error").is_some(), "{n}");
    }
    assert_eq!(t.ran.lock().unwrap().len(), 2);
    let a = audits.lock().unwrap().join("\n");
    assert!(a.contains("voice.tool radio_search ran by auto") && a.contains("voice.tools voice tools offered") && a.contains("refused"), "{a}");
}

#[test]
fn a_tool_that_asks_waits_for_the_press_and_only_the_press_approves() {
    let screen = FakeScreen::at_screen();
    let (mut d, t, audits, _) = rig(FakeTools::standard(), screen.clone(), false);
    let s = d.call("c1", "radio_play", &json!({"query": "jazz"}));
    assert_eq!(s.response["status"], "waiting_for_the_person_on_the_screen");
    assert!(matches!(s.note, ToolNote::Waiting { .. }) && d.busy());
    assert_eq!(screen.shown.lock().unwrap()[0].summary, "Run radio_play?", "the same card text as the chat");
    assert!(t.ran.lock().unwrap().is_empty(), "nothing ran yet");
    // time passes, words are said: nothing in this module reads speech, so nothing happens
    assert!(d.poll().is_empty() && t.ran.lock().unwrap().is_empty());
    screen.press(true);
    let done = d.poll();
    assert_eq!(done.len(), 1);
    assert!(done[0].model_text.contains("approved on the screen") && done[0].model_text.contains("Playing jazz"));
    assert!(matches!(&done[0].note, ToolNote::Ran { by, .. } if by == "user"));
    assert!(!d.busy() && t.ran.lock().unwrap().len() == 1);
    assert!(audits.lock().unwrap().iter().any(|l| l.contains("radio_play waiting")) && audits.lock().unwrap().iter().any(|l| l.contains("radio_play ran by user")));
    // denied
    let _ = d.call("c2", "radio_play", &json!({}));
    screen.press(false);
    let done = d.poll();
    assert!(done[0].model_text.contains("said no") && matches!(done[0].note, ToolNote::Refused { .. }));
    assert_eq!(t.ran.lock().unwrap().len(), 1, "a denial runs nothing");
}

#[test]
fn no_answer_in_time_cancel_and_the_session_limit() {
    let screen = FakeScreen::at_screen();
    let (mut d, t, _, clock) = rig(FakeTools::standard(), screen.clone(), false);
    let _ = d.call("c1", "radio_play", &json!({}));
    clock.fetch_add(30_000, Ordering::SeqCst);
    assert!(d.poll().is_empty() && d.busy());
    clock.fetch_add(31_000, Ordering::SeqCst);
    let done = d.poll();
    assert!(done[0].model_text.contains("did not answer") && screen.withdrawn.lock().unwrap().len() == 1 && !d.busy());
    let _ = d.call("c2", "radio_play", &json!({}));
    d.cancel(&["c2".to_string()]);
    assert!(!d.busy() && screen.withdrawn.lock().unwrap().len() == 2);
    assert!(t.ran.lock().unwrap().is_empty());
    // the call limit
    let (mut d, _, _, _) = rig(FakeTools::standard(), FakeScreen::at_screen(), false);
    for i in 0..40 {
        let _ = d.call(&i.to_string(), "radio_status", &json!({}));
    }
    assert!(matches!(d.call("x", "radio_status", &json!({})).note, ToolNote::Refused { why, .. } if why.contains("limit")));
}

#[test]
fn hard_ask_tools_need_the_person_at_the_screen_and_never_ride_on_auto() {
    // nobody at the screen: refused for voice-only use, nothing shown
    let away = Arc::new(FakeScreen::default());
    let (mut d, t, _, _) = rig(FakeTools::standard(), away.clone(), true);
    let r = d.call("1", "gemini_voice_set", &json!({"enabled": true}));
    assert!(matches!(&r.note, ToolNote::Refused { why, .. } if why.contains("nobody is at the screen")), "{:?}", r.note);
    assert!(away.shown.lock().unwrap().is_empty() && t.ran.lock().unwrap().is_empty());
    // Auto mode runs a normal ask tool, but a hard-ask one still asks
    let screen = FakeScreen::at_screen();
    let (mut d, t, audits, _) = rig(FakeTools::standard(), screen.clone(), true);
    let r = d.call("2", "radio_play", &json!({}));
    assert!(matches!(&r.note, ToolNote::Ran { by, .. } if by == "auto-approved"));
    let r = d.call("3", "gemini_voice_set", &json!({"enabled": true}));
    assert!(matches!(r.note, ToolNote::Waiting { .. }) && screen.shown.lock().unwrap().len() == 1);
    assert_eq!(t.ran.lock().unwrap().len(), 1);
    assert!(audits.lock().unwrap().iter().all(|l| l.starts_with("voice.")), "every audit line is marked voice");
    // a person at the screen but not at the computer: a normal ask tool also needs the press
    let (mut d, _, _, _) = rig(FakeTools::standard(), away, false);
    assert!(matches!(d.call("4", "radio_play", &json!({})).note, ToolNote::Refused { .. }));
}

#[test]
fn an_earlier_grant_of_the_chat_covers_the_voice_call_like_in_the_chat() {
    let mut t = FakeTools::standard();
    t.granted = vec!["radio_play".into()];
    let (mut d, tools, _, _) = rig(t, Arc::new(FakeScreen::default()), false);
    let r = d.call("1", "radio_play", &json!({}));
    assert!(matches!(&r.note, ToolNote::Ran { by, .. } if by == "grant"));
    assert_eq!(tools.ran.lock().unwrap().len(), 1);
}

#[test]
fn result_shortening_keeps_whole_lines_and_counts_the_rest() {
    assert_eq!(shorten_result("  short  ", 100), "short");
    let many: String = (1..=30).map(|i| format!("line number {i}\n")).collect();
    let s = shorten_result(&many, 60);
    assert!(s.contains("line number 1") && s.contains("more lines") && !s.contains("line number 30"), "{s}");
    let one = shorten_result(&"x".repeat(500), 50);
    assert!(one.starts_with(&"x".repeat(50)) && one.contains("on the screen"));
}

/// The ratchet: every tool the chat of this account has is in what the live session can reach, or on the short exclusion list, and the
/// list can only shrink.
#[test]
fn every_tool_of_the_chat_is_in_the_voice_catalogue_or_excluded_and_the_exclusions_only_shrink() {
    use crate::agent::UserTools;
    use iyakki_sec::policy::Role;
    struct Off;
    impl iyakki_proxy::Upstream for Off {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }
    let dir = std::fs::canonicalize({
        let d = std::env::temp_dir().join(format!("iyakki-gv-ratchet-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("home")).unwrap();
        d
    })
    .unwrap();
    let svc = crate::services::Services::open(&dir.join("svc"), Arc::new(Off), Arc::new(Off)).unwrap();
    for role in [Role::User, Role::Admin] {
        let mut policy = role.template();
        policy.fs_read = vec![dir.join("home").display().to_string()];
        let tools = UserTools::new("alice", policy, svc.clone(), Arc::new(crate::cli::tests::MockHost::default()), vec![]).with_cwd(&dir.join("home"));
        let defs = tools.defs();
        assert!(defs.len() > 50, "a real tool set: {}", defs.len());
        let c = Catalogue::build(&defs);
        assert!(c.missing(&defs).is_empty(), "the voice session cannot reach: {:?}", c.missing(&defs));
        assert!(c.decls.iter().map(|d| d.to_string().len()).sum::<usize>() <= MAX_DECL_BYTES && c.declared.len() <= MAX_DECLARED);
        for d in &c.decls {
            assert!(valid_fn_name(d["name"].as_str().unwrap()));
        }
        // the excluded ones that exist in this set are exactly the list, and each has a stated reason
        for e in &c.excluded {
            assert!(is_excluded(e), "{e} was left out without being on the exclusion list");
        }
    }
    assert!(EXCLUDED.len() <= MAX_EXCLUDED, "the exclusion list may only shrink");
    assert!(EXCLUDED.iter().all(|(_, why)| why.len() > 20));
    let _ = std::fs::remove_dir_all(&dir);
}
