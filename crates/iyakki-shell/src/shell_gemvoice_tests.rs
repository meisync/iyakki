//! The Gemini voice assistant in the shell (EXPERIMENTAL): the start key, a whole manual turn against a FAKE live connection (no
//! network, no Google), the consent refusal, a tool's card answered by a press, Esc, the microphone button and the card's size.
use super::tests::*;
use super::*;
use iyakki_shared::gemini;
use iyakki_shared::geminilive::{ConnectErr, LiveConnector, LiveLink, Recv};
use iyakki_agent::wakelisten as wl;
use iyakki_apps::apps::gemvoice::{set_test_parts, TestParts};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex as M;

/// The test parts and the stop key are process wide: these tests take turns.
pub(crate) static TURN: M<()> = M::new(());

#[derive(Default)]
struct Wire {
    inbox: M<VecDeque<String>>,
    sent: M<Vec<String>>,
    connects: AtomicUsize,
}

/// A fake live connection: answers the setup, and a scripted answer when the person is done talking.
struct Fake {
    w: Arc<Wire>,
    on_end: Vec<Value>,
    on_audio: M<Option<Vec<Value>>>,
}

impl LiveLink for Fake {
    fn send_text(&mut self, t: &str) -> Result<(), String> {
        self.w.sent.lock().unwrap().push(t.to_string());
        let v: Value = serde_json::from_str(t).unwrap_or(Value::Null);
        let mut q = self.w.inbox.lock().unwrap();
        if v.get("setup").is_some() {
            q.push_back(json!({"setupComplete": {}}).to_string());
        }
        if v.pointer("/realtimeInput/audio").is_some() {
            if let Some(list) = self.on_audio.lock().unwrap().take() {
                q.extend(list.into_iter().map(|x| x.to_string()));
            }
        }
        if v.pointer("/realtimeInput/audioStreamEnd").is_some() || v.pointer("/realtimeInput/text").and_then(Value::as_str).is_some_and(|t| t.contains("approved on the screen")) {
            q.extend(self.on_end.iter().map(Value::to_string));
        }
        Ok(())
    }
    fn recv(&mut self, wait: std::time::Duration) -> Result<Recv, String> {
        if let Some(t) = self.w.inbox.lock().unwrap().pop_front() {
            return Ok(Recv::Text(t));
        }
        std::thread::sleep(wait.min(std::time::Duration::from_millis(5)));
        Ok(Recv::Timeout)
    }
    fn close(&mut self) {}
}

struct Conn {
    w: Arc<Wire>,
    on_end: Vec<Value>,
    on_audio: Vec<Value>,
}
impl LiveConnector for Conn {
    fn connect(&self, _url: &str) -> Result<Box<dyn LiveLink>, ConnectErr> {
        self.w.connects.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Fake { w: self.w.clone(), on_end: self.on_end.clone(), on_audio: M::new(Some(self.on_audio.clone())) }))
    }
}

#[derive(Default)]
struct MicLog {
    opens: u32,
    open: bool,
}
#[derive(Clone, Default)]
struct FakeMic(Arc<M<MicLog>>);
impl wl::Mic for FakeMic {
    fn open(&mut self) -> Result<(), String> {
        let mut l = self.0.lock().unwrap();
        l.opens += 1;
        l.open = true;
        Ok(())
    }
    fn is_open(&self) -> bool {
        self.0.lock().unwrap().open
    }
    fn drain(&mut self) -> Vec<i16> {
        vec![30; 1600]
    }
    fn alive(&mut self) -> bool {
        self.0.lock().unwrap().open
    }
    fn close(&mut self) {
        self.0.lock().unwrap().open = false;
    }
}

struct Say(Arc<M<Vec<String>>>);
impl iyakki_agent::voiceassist::Speaker for Say {
    fn say(&mut self, t: &str) -> Result<(), String> {
        self.0.lock().unwrap().push(t.to_string());
        Ok(())
    }
    fn busy(&mut self) -> bool {
        false
    }
    fn stop(&mut self) {}
    fn level(&self) -> f32 {
        0.0
    }
    fn louder(&mut self) {}
    fn quieter(&mut self) {}
}

/// One tool that asks (a stand-in for radio_play).
struct Radio(M<Vec<String>>);
impl iyakki_shared::agent::Tools for Radio {
    fn defs(&self) -> Vec<iyakki_shared::agent::ToolDef> {
        vec![iyakki_shared::agent::ToolDef { spec: iyakki_shared::chat::ToolSpec { name: "radio_play".into(), description: "Play a station.".into(), schema: json!({"type": "object", "properties": {"query": {"type": "string"}}}) }, risk: iyakki_shared::agent::Risk::Ask, untrusted: false }]
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        self.0.lock().unwrap().push(format!("{name} {args}"));
        Ok("Playing jazz.".into())
    }
    fn describe(&self, name: &str, _: &Value) -> String {
        format!("Run {name}?")
    }
}

struct Rig {
    sh: Shell,
    mic: FakeMic,
    said: Arc<M<Vec<String>>>,
    wire: Arc<Wire>,
    tools: Arc<Radio>,
    _turn: std::sync::MutexGuard<'static, ()>,
}

fn rig(tag: &str, yes: bool, on_audio: Vec<Value>, on_end: Vec<Value>) -> Rig {
    let turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    // the session of the test before (stopped by its Drop) may still be closing its microphone
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(30) {
        if !iyakki_shared::geminilive::mic_open() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    iyakki_shared::geminilive::clear_latency();
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    let dir = sh.env.data_dir().to_path_buf();
    gemini::update(&dir, |c| {
        c.voice.enabled = true;
        Ok(())
    })
    .unwrap();
    if yes {
        gemini::grant_live_on(&dir, "root", "2026-11-15").unwrap();
    }
    let (wire, mic, said, tools) = (Arc::new(Wire::default()), FakeMic::default(), Arc::new(M::new(Vec::new())), Arc::new(Radio(M::new(Vec::new()))));
    let (m2, s2) = (mic.clone(), said.clone());
    set_test_parts(Some(TestParts {
        connector: Some(Arc::new(Conn { w: wire.clone(), on_end, on_audio })),
        mic: Some(Arc::new(move || Box::new(m2.clone()))),
        speaker: Some(Arc::new(move || Box::new(Say(s2.clone())))),
        tools: Some(tools.clone()),
        key: Some("AIzaSyFAKE-KEY-0123456789".into()),
        ..Default::default()
    }));
    Rig { sh, mic, said, wire, tools, _turn: turn }
}

impl Drop for Rig {
    fn drop(&mut self) {
        iyakki_shared::geminilive::request_stop();
        set_test_parts(None);
    }
}

fn until(sh: &mut Shell, ok: impl Fn(&Shell) -> bool) -> bool {
    for _ in 0..400 {
        sh.handle(&Event::Tick);
        if ok(sh) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

fn press_key(sh: &mut Shell) {
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::VoiceAssist, true));
}

fn reply(text: &str) -> Vec<Value> {
    vec![json!({"serverContent": {"inputTranscription": {"text": "hello there"}}}), json!({"serverContent": {"outputTranscription": {"text": text}}}), json!({"serverContent": {"turnComplete": true}})]
}

fn toast(sh: &Shell) -> String {
    sh.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default()
}

#[test]
fn the_start_key_runs_a_whole_manual_turn_and_the_local_voice_reads_the_answer() {
    let mut r = rig("gv-turn", true, vec![], reply("It is ten o'clock."));
    let before = iyakki_agent::clientmedia::indicator();
    assert!(!before.active());
    press_key(&mut r.sh);
    assert!(until(&mut r.sh, |s| s.gv.open() && iyakki_shared::geminilive::mic_open()), "the session opens and the red sign is on");
    assert!(iyakki_agent::clientmedia::indicator().text().is_some_and(|t| t.contains("Gemini voice")), "the one red band names it");
    assert!(until(&mut r.sh, |_| r.wire.sent.lock().unwrap().iter().any(|m| m.contains("\"audio\""))), "the microphone goes in as audio");
    // "I am done talking" (the button) hands the turn over; the answer is read by the local voice and the session ends
    r.sh.handle(&Event::Tab(gv_impl::DONE));
    assert!(until(&mut r.sh, |s| !s.gv.open() && !iyakki_shared::geminilive::mic_open()), "it ends after the answer");
    assert_eq!(r.said.lock().unwrap().join(" "), "It is ten o'clock.");
    assert!(!r.mic.0.lock().unwrap().open, "the microphone was closed");
    assert!(std::fs::read_to_string(r.sh.env.audit.path()).unwrap_or_default().contains("voice.session"), "audited");
    assert!(!iyakki_agent::clientmedia::indicator().active());
}

#[test]
fn without_the_persons_yes_nothing_is_sent_the_line_is_shown_and_the_microphone_never_opens() {
    let mut r = rig("gv-refuse", false, vec![], vec![]);
    press_key(&mut r.sh);
    assert!(until(&mut r.sh, |s| toast(s).contains("no sound was sent to Google")), "{}", toast(&r.sh));
    assert!(toast(&r.sh).contains("EXPERIMENTAL") && toast(&r.sh).contains("local voice answers instead"));
    assert_eq!(r.wire.connects.load(Ordering::SeqCst), 0, "not even a connection");
    assert_eq!(r.mic.0.lock().unwrap().opens, 0, "the microphone never opened");
    assert!(r.wire.sent.lock().unwrap().is_empty());
}

#[test]
fn it_is_off_by_default_and_the_key_says_where_to_turn_it_on() {
    let mut r = rig("gv-off", true, vec![], vec![]);
    gemini::update(r.sh.env.data_dir(), |c| {
        c.voice.enabled = false;
        Ok(())
    })
    .unwrap();
    r.sh.gv.looked = 0;
    press_key(&mut r.sh);
    assert!(toast(&r.sh).contains("off") && toast(&r.sh).contains("Settings"), "{}", toast(&r.sh));
    assert!(!r.sh.gv.open() && r.wire.connects.load(Ordering::SeqCst) == 0);
}

#[test]
fn a_tool_that_asks_shows_a_card_and_only_the_press_on_allow_runs_it() {
    let call = vec![json!({"toolCall": {"functionCalls": [{"id": "k1", "name": "radio_play", "args": {"query": "jazz"}}]}})];
    let mut r = rig("gv-card", true, call, reply("Playing jazz now."));
    press_key(&mut r.sh);
    assert!(until(&mut r.sh, |s| !s.gv.board.pending().is_empty()), "the card is on the board");
    assert!(r.tools.0.lock().unwrap().is_empty(), "nothing ran before the press");
    // the card is drawn with Allow and Deny, inside the window, at several sizes
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
        let mut g = Gfx::new(w, h);
        gv_impl::render(&r.sh.gv, &mut g, w, h, crate::ui::scale_for(w, h));
        for id in [gv_impl::ALLOW, gv_impl::DENY] {
            let hit = g.hits.iter().find(|x| x.ev == Event::Tab(id)).unwrap_or_else(|| panic!("{w}x{h}: button {id}"));
            assert!(hit.rect.x >= -0.5 && hit.rect.x + hit.rect.w <= w as f32 + 0.5 && hit.rect.y >= -0.5 && hit.rect.y + hit.rect.h <= h as f32 + 0.5, "{w}x{h}: {:?}", hit.rect);
        }
    }
    r.sh.handle(&Event::Tab(gv_impl::ALLOW));
    assert!(until(&mut r.sh, |_| !r.said.lock().unwrap().is_empty()), "the answer follows the result");
    assert!(r.tools.0.lock().unwrap().iter().any(|x| x.starts_with("radio_play")), "it ran after the press");
    assert!(std::fs::read_to_string(r.sh.env.audit.path()).unwrap_or_default().contains("voice.card"));
}

#[test]
fn deny_runs_nothing() {
    let call = vec![json!({"toolCall": {"functionCalls": [{"id": "k2", "name": "radio_play", "args": {}}]}})];
    let mut r = rig("gv-deny", true, call, vec![]);
    press_key(&mut r.sh);
    assert!(until(&mut r.sh, |s| !s.gv.board.pending().is_empty()));
    r.sh.handle(&Event::Tab(gv_impl::DENY));
    assert!(until(&mut r.sh, |s| s.gv.board.pending().is_empty()));
    std::thread::sleep(std::time::Duration::from_millis(150));
    assert!(r.tools.0.lock().unwrap().is_empty());
}

#[test]
fn esc_ends_the_session_and_closes_the_microphone_and_the_key_again_does_too() {
    let mut r = rig("gv-esc", true, vec![], vec![]);
    press_key(&mut r.sh);
    assert!(until(&mut r.sh, |s| s.gv.open() && iyakki_shared::geminilive::mic_open()));
    r.sh.handle(&Event::Key(Key::Esc));
    assert!(until(&mut r.sh, |s| !s.gv.open() && !iyakki_shared::geminilive::mic_open()), "Esc ends it");
    assert!(!r.mic.0.lock().unwrap().open);
    // the key again starts one, and again ends it
    press_key(&mut r.sh);
    assert!(until(&mut r.sh, |s| s.gv.open()));
    // the same key reported twice within a heartbeat acts once (the Fn-key rule): a person's second press is later than that
    for _ in 0..3 {
        r.sh.handle(&Event::Tick);
    }
    press_key(&mut r.sh);
    let ok = until(&mut r.sh, |s| !s.gv.open());
    assert!(ok, "audit: {} state: {:?}", std::fs::read_to_string(r.sh.env.audit.path()).unwrap_or_default(), r.sh.gv.running.as_ref().map(|x| x.state()));
}

#[test]
fn the_microphone_button_is_offered_only_when_the_assistant_is_on_and_starts_a_session() {
    let mut r = rig("gv-mic", true, vec![], vec![]);
    for _ in 0..70 {
        r.sh.handle(&Event::Tick);
    }
    assert!(r.sh.gv.offered);
    let mut g = Gfx::new(1280, 720);
    r.sh.render(&mut g);
    let hit = r.sh.hits().iter().find(|x| x.ev == Event::Tab(gv_impl::MIC)).expect("the Mic button");
    assert!(hit.rect.x + hit.rect.w <= 1280.0 && hit.rect.y + hit.rect.h <= 720.0);
    r.sh.handle(&Event::Tab(gv_impl::MIC));
    assert!(until(&mut r.sh, |s| s.gv.open()));
    // once open, the same corner shows Done talking and Stop
    let mut g = Gfx::new(1280, 720);
    r.sh.render(&mut g);
    assert!(r.sh.hits().iter().any(|x| x.ev == Event::Tab(gv_impl::STOP)) && r.sh.hits().iter().any(|x| x.ev == Event::Tab(gv_impl::DONE)));
    r.sh.handle(&Event::Tab(gv_impl::STOP));
    assert!(until(&mut r.sh, |s| !s.gv.open()));
    // switched off: no button
    gemini::update(r.sh.env.data_dir(), |c| {
        c.voice.enabled = false;
        Ok(())
    })
    .unwrap();
    r.sh.gv.looked = 0;
    for _ in 0..3 {
        r.sh.handle(&Event::Tick);
    }
    let mut g = Gfx::new(1280, 720);
    r.sh.render(&mut g);
    assert!(!r.sh.hits().iter().any(|x| x.ev == Event::Tab(gv_impl::MIC)));
}

#[test]
fn the_wake_phrase_hands_off_only_when_everything_is_opted_in_and_chimes() {
    let mut r = rig("gv-wake", true, vec![], vec![]);
    assert!(!r.sh.gv_wake_handoff(), "the hand-off is off by default: the Voice tile gets the wake as before");
    gemini::update(r.sh.env.data_dir(), |c| {
        c.voice.wake_handoff = true;
        Ok(())
    })
    .unwrap();
    for _ in 0..70 {
        r.sh.handle(&Event::Tick);
    }
    assert!(r.sh.gv_wake_handoff());
    assert!(until(&mut r.sh, |s| s.gv.open()));
    assert!(std::fs::read_to_string(r.sh.env.audit.path()).unwrap_or_default().contains("opened by the wake phrase"));
}
