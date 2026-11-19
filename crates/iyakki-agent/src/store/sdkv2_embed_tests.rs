use super::*;

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
