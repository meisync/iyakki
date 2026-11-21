use super::*;
use std::sync::Mutex;

fn who() -> Who {
    Who { app: "docs".into(), user: "alice".into() }
}

fn rig(allowed: &'static [&'static str]) -> (EmbedBoard, EmbedEngine) {
    let board = EmbedBoard::default();
    let files: AppFile = Arc::new(|app, f| (app == "docs" && f == "help.md").then(|| b"# Help\nhello".to_vec()));
    let e = EmbedEngine::new(board.clone(), files, Arc::new(move |h| allowed.contains(&h)));
    (board, e)
}

#[test]
fn a_markdown_embed_shows_text_the_app_sent_or_a_file_inside_the_app_and_nothing_else() {
    let (board, e) = rig(&[]);
    e.call(&who(), "embed.mount", &json!({"id": 1, "kind": "markdown", "target": "box", "options": {"file": "help.md"}})).unwrap();
    assert_eq!(board.snapshot()[0].text, "# Help\nhello");
    e.call(&who(), "embed.update", &json!({"id": 1, "options": {"text": "new"}})).unwrap();
    assert_eq!(board.snapshot()[0].text, "new");
    assert!(e.call(&who(), "embed.mount", &json!({"id": 2, "kind": "markdown", "target": "b", "options": {"file": "../secret.md"}})).is_err());
    assert!(e.call(&who(), "embed.mount", &json!({"id": 2, "kind": "markdown", "target": "b", "options": {"text": "x".repeat(MAX_MARKDOWN + 1)}})).is_err());
    assert_eq!(board.snapshot().len(), 1);
}

#[test]
fn a_browser_embed_may_visit_only_what_the_app_declared_and_the_persons_policy_allows_with_its_own_cookies() {
    let (board, e) = rig(&["docs.example.org"]);
    let mount = |url: &str, id: u32| e.call(&who(), "embed.mount", &json!({"id": id, "kind": "browser", "target": {"x": 0, "y": 0, "w": 300, "h": 200}, "options": {"url": url}, "hosts": ["docs.example.org", "tracker.example.net"]}));
    mount("https://docs.example.org/a", 1).unwrap();
    let v = board.snapshot().remove(0);
    assert_eq!(v.hosts, ["docs.example.org"], "the intersection: the policy does not allow the second");
    assert_eq!(v.profile, "embed:alice:docs");
    let blocked = mount("https://tracker.example.net/x", 2).unwrap_err();
    assert_eq!(blocked.code, code::BLOCKED);
    // nothing allowed at all: refused in words
    let (_b, none) = rig(&[]);
    let r = none.call(&who(), "embed.mount", &json!({"id": 1, "kind": "browser", "target": "x", "options": {"url": "https://docs.example.org"}, "hosts": ["docs.example.org"]}));
    assert_eq!(r.unwrap_err().code, code::BLOCKED);
    assert_eq!(allowed_hosts(&["a".into(), "b".into()], &|h| h == "b"), ["b"]);
}

#[test]
fn calls_wait_for_the_painter_which_takes_them_once_and_an_unmount_removes_the_viewer() {
    let (board, e) = rig(&[]);
    e.call(&who(), "embed.mount", &json!({"id": 5, "kind": "dash", "target": "c", "options": {"dashboard": "ops"}})).unwrap();
    for i in 0..25 {
        e.call(&who(), "embed.call", &json!({"id": 5, "method": "setRange", "args": {"n": i}})).unwrap();
    }
    let calls = board.take_calls(5);
    assert_eq!((calls.len(), calls[0].1["n"].as_u64(), calls[19].1["n"].as_u64()), (20, Some(5), Some(24)), "the newest 20 are kept");
    assert!(board.take_calls(5).is_empty());
    assert!(e.call(&who(), "embed.call", &json!({"id": 9, "method": "focus"})).is_err());
    e.call(&who(), "embed.unmount", &json!({"id": 5})).unwrap();
    assert!(board.snapshot().is_empty());
}

// ---- what the media and dashboard viewers show

struct FakePlay(Mutex<(String, u64)>, Mutex<u32>);
impl Backend for FakePlay {
    fn call(&self, _: &Who, op: &str, _: &Value) -> Result<Value, SdkError> {
        assert_eq!(op, "media.state");
        *self.1.lock().unwrap() += 1;
        let (s, p) = self.0.lock().unwrap().clone();
        if s == "none" {
            return Err(SdkError::new(code::UNAVAILABLE, "no player on this computer"));
        }
        Ok(json!({"state": s, "position": p}))
    }
}

struct FakeDashFeed(Mutex<u32>);
impl super::super::sdkv2_engines::DashPort for FakeDashFeed {
    fn sources(&self, _: &str) -> Vec<super::super::sdkv2_engines::SourceInfo> {
        vec![]
    }
    fn query(&self, user: &str, source: &str, text: &str, _: &Value, limit: usize) -> Result<(Value, Vec<Value>), String> {
        *self.0.lock().unwrap() += 1;
        assert_eq!((user, source, text, limit), ("alice", "prom", "up", DASH_ROWS + 1));
        Ok((json!([{"name": "host"}, {"name": "n"}]), (0..12).map(|i| json!([format!("h{i}"), i])).collect()))
    }
    fn add_panel(&self, _: &str, _: &str, _: &str, _: &str, _: &str, _: &str) -> Result<String, String> {
        Err("no".into())
    }
    fn panel_queries(&self, _: &str, dashboard: &str) -> Result<Vec<(String, String, String)>, String> {
        match dashboard {
            "ops" => Ok(vec![("CPU".into(), "prom".into(), "up".into()), ("Memory".into(), "prom".into(), "mem".into())]),
            "empty" => Ok(vec![]),
            other => Err(format!("there is no dashboard called {other}")),
        }
    }
}

fn view(id: u32, kind: &str, options: Value) -> EmbedView {
    EmbedView { id, kind: kind.into(), target: json!("x"), options, text: String::new(), hosts: vec![], profile: String::new(), calls: vec![] }
}

fn feeds() -> (EmbedFeeds, Arc<FakePlay>, Arc<FakeDashFeed>, Arc<Mutex<u64>>) {
    let play = Arc::new(FakePlay(Mutex::new(("playing".into(), 42)), Mutex::new(0)));
    let dash = Arc::new(FakeDashFeed(Mutex::new(0)));
    let clock = Arc::new(Mutex::new(1_000u64));
    let c = clock.clone();
    (EmbedFeeds::new(play.clone(), dash.clone(), Arc::new(move || *c.lock().unwrap())), play, dash, clock)
}

#[test]
fn a_media_viewer_shows_the_apps_playback_state_and_asks_the_player_at_most_twice_a_second() {
    let (f, play, _, clock) = feeds();
    let v = view(3, "media", json!({"file": "music/song.mp3"}));
    let d = f.data(&who(), &v).unwrap();
    assert_eq!((d.heading.as_str(), d.lines.clone(), d.progress), ("Playing · song.mp3", vec!["0:42".to_string()], Some((42, 0))));
    f.data(&who(), &v);
    assert_eq!(*play.1.lock().unwrap(), 1, "a second look in the same half second is answered from what was kept");
    *clock.lock().unwrap() += 600;
    *play.0.lock().unwrap() = ("paused".into(), 61);
    let d = f.data(&who(), &v).unwrap();
    assert_eq!((d.heading.as_str(), d.lines[0].as_str()), ("Paused · song.mp3", "1:01"));
    *clock.lock().unwrap() += 600;
    *play.0.lock().unwrap() = ("none".into(), 0);
    let d = f.data(&who(), &v).unwrap();
    assert_eq!(d.lines, ["no player on this computer"], "no player: the viewer says so in words");
    assert!(f.data(&who(), &view(4, "markdown", json!({}))).is_none() && f.data(&who(), &view(5, "browser", json!({}))).is_none(), "no data of its own for the other kinds");
}

#[test]
fn a_dashboard_viewer_shows_the_first_panels_rows_cut_to_eight_and_reads_every_five_seconds() {
    let (f, _, dash, clock) = feeds();
    let v = view(7, "dash", json!({"dashboard": "ops"}));
    let d = f.data(&who(), &v).unwrap();
    assert_eq!(d.heading, "ops · CPU");
    assert_eq!(d.lines[0], "host   n");
    assert_eq!(d.lines[1], "h0   0");
    assert_eq!((d.lines.len(), d.lines.last().unwrap().as_str()), (DASH_ROWS + 2, "and more rows"));
    *clock.lock().unwrap() += 4_000;
    f.data(&who(), &v);
    assert_eq!(*dash.0.lock().unwrap(), 1);
    *clock.lock().unwrap() += 1_500;
    f.data(&who(), &v);
    assert_eq!(*dash.0.lock().unwrap(), 2, "after five seconds it is read again");
    // another dashboard under the same viewer number is not shown the old picture; refusals are shown in words, never thrown
    assert_eq!(f.data(&who(), &view(7, "dash", json!({"dashboard": "empty"}))).unwrap().lines, ["this dashboard has no panels yet"]);
    assert_eq!(f.data(&who(), &view(8, "dash", json!({"dashboard": "nope"}))).unwrap().lines, ["there is no dashboard called nope"]);
    f.forget("docs", 7);
}
