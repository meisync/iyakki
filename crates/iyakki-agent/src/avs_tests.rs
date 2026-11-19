use super::*;
use std::cell::RefCell;

struct FakeAmazon {
    forms: RefCell<Vec<Vec<(String, String)>>>,
    answers: RefCell<Vec<(u16, String)>>,
}

impl Http for FakeAmazon {
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, String), String> {
        assert!(url.starts_with("https://api.amazon.com/"), "{url}");
        self.forms.borrow_mut().push(form.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
        Ok(self.answers.borrow_mut().remove(0))
    }
}

fn amazon(answers: Vec<(u16, &str)>) -> FakeAmazon {
    FakeAmazon { forms: RefCell::new(vec![]), answers: RefCell::new(answers.into_iter().map(|(s, b)| (s, b.to_string())).collect()) }
}

const TOK: &str = r#"{"access_token":"Atza|new","refresh_token":"Atzr|new","expires_in":3600,"token_type":"bearer"}"#;

#[test]
fn device_code_linking_waits_slows_down_expires_and_ends_with_tokens() {
    let a = amazon(vec![(200, r#"{"device_code":"dc1","user_code":"AB12","verification_uri":"https://amazon.com/us/code","interval":5,"expires_in":600}"#), (400, r#"{"error":"authorization_pending"}"#), (400, r#"{"error":"slow_down"}"#), (400, r#"{"error":"expired_token"}"#), (400, r#"{"error":"access_denied"}"#), (200, TOK), (500, "boom")]);
    let c = start_linking(&a, "amzn1.application-oa2-client.x", "iyakki-test", "serial-1").unwrap();
    assert_eq!((c.user_code.as_str(), c.interval_secs), ("AB12", 5));
    let sent = a.forms.borrow()[0].clone();
    assert!(sent.iter().any(|(k, v)| k == "scope_data" && v.contains("\"productID\":\"iyakki-test\"") && v.contains("serial-1")), "{sent:?}");
    let polls: Vec<Poll> = (0..5).map(|_| poll_linking(&a, "cid", &c).unwrap()).collect();
    assert_eq!(&polls[..4], [Poll::Waiting, Poll::SlowDown, Poll::Expired, Poll::Denied]);
    assert!(matches!(&polls[4], Poll::Linked(t) if t.access == "Atza|new" && t.refresh == "Atzr|new"));
    assert!(poll_linking(&a, "cid", &c).is_err(), "an unknown failure is an error, not a retry");
    assert!(start_linking(&a, "", "p", "s").unwrap_err().contains("registers the product"));
}

#[test]
fn linking_refuses_an_answer_with_a_plain_http_address_or_no_code() {
    let a = amazon(vec![(200, r#"{"device_code":"d","user_code":"U","verification_uri":"http://evil.example/x"}"#), (200, "{}"), (503, "")]);
    for _ in 0..3 {
        assert!(start_linking(&a, "c", "p", "s").is_err());
    }
}

#[test]
fn a_dead_refresh_token_asks_for_linking_again() {
    let a = amazon(vec![(200, TOK), (400, r#"{"error":"invalid_grant"}"#)]);
    assert_eq!(refresh(&a, "c", "r").unwrap().access, "Atza|new");
    assert!(refresh(&a, "c", "r").unwrap_err().contains("link the device again"));
}

#[test]
fn the_recognize_request_is_a_json_part_and_an_audio_part_inside_the_limit() {
    let ev = recognize_event("m1", "d1");
    assert_eq!(ev["event"]["payload"]["format"], AUDIO_FORMAT);
    let (ct, body) = build_recognize(&ev, &[1, 2, 3, 0xff, 0]).unwrap();
    let parts = parse_multipart(&ct.replace("form-data", "related"), &body).unwrap();
    assert_eq!(parts.len(), 2);
    assert!(parts[0].header("Content-Disposition").unwrap().contains("metadata") && std::str::from_utf8(&parts[0].body).unwrap().contains("\"Recognize\""));
    assert_eq!(parts[1].body, [1, 2, 3, 0xff, 0], "binary audio survives, including bytes that look like text");
    assert!(build_recognize(&ev, &[]).is_err() && build_recognize(&ev, &vec![0; MAX_AUDIO + 1]).is_err());
}

fn answer(boundary: &str, speak: bool, extra: &str) -> Vec<u8> {
    let mut b = Vec::new();
    let mut p = |h: &str, body: &[u8]| {
        b.extend_from_slice(format!("--{boundary}\r\n{h}\r\n\r\n").as_bytes());
        b.extend_from_slice(body);
        b.extend_from_slice(b"\r\n");
    };
    if speak {
        p("Content-Type: application/json; charset=UTF-8", br#"{"directive":{"header":{"namespace":"SpeechSynthesizer","name":"Speak","messageId":"a"},"payload":{"url":"cid:audio1","token":"tok-1","format":"AUDIO_MPEG"}}}"#);
        p("Content-Type: application/octet-stream\r\nContent-ID: <audio1>", &[0xff, 0xfb, 0x90, 0x00, b'\r', b'\n', 1, 2]);
    }
    if !extra.is_empty() {
        p("Content-Type: application/json", extra.as_bytes());
    }
    b.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    b
}

#[test]
fn an_answer_gives_the_spoken_audio_whether_alexa_wants_more_and_what_was_ignored() {
    let extra = r#"{"directive":{"header":{"namespace":"SpeechRecognizer","name":"ExpectSpeech","messageId":"b"},"payload":{"timeoutInMilliseconds":5000}}}"#;
    let parts = parse_multipart("multipart/related; boundary=\"xyz\"", &answer("xyz", true, extra)).unwrap();
    let t = interpret(&parts).unwrap();
    assert_eq!(t.speech, vec![vec![0xff, 0xfb, 0x90, 0x00, b'\r', b'\n', 1, 2]]);
    assert!(t.expect_speech);
    assert_eq!(t.report.len(), 2);
    assert_eq!(t.report[1]["event"]["header"]["name"], "SpeechFinished");
    assert_eq!(t.report[0]["event"]["payload"]["token"], "tok-1");
    let other = r#"{"directive":{"header":{"namespace":"Alerts","name":"SetAlert","messageId":"c"},"payload":{}}}"#;
    let t = interpret(&parse_multipart("multipart/related; boundary=b2", &answer("b2", false, other)).unwrap()).unwrap();
    assert_eq!((t.ignored, t.speech.len(), t.expect_speech), (vec!["Alerts.SetAlert".to_string()], 0, false));
}

#[test]
fn broken_answers_are_errors_never_guesses() {
    assert!(parse_multipart("multipart/related", b"x").is_err());
    assert!(parse_multipart("multipart/related; boundary=b", b"--b\r\nContent-Type: a\r\n\r\nno end").is_err());
    let bad_audio = answer("b", true, "");
    let mut parts = parse_multipart("multipart/related; boundary=b", &bad_audio).unwrap();
    parts.pop();
    assert!(interpret(&parts).unwrap_err().contains("missing"), "a Speak whose audio is not in the answer");
    let no_token = serde_json::json!({"directive": {"header": {"namespace": "SpeechSynthesizer", "name": "Speak"}, "payload": {"url": "cid:x"}}});
    assert!(parse_directive(&no_token).is_err() && parse_directive(&serde_json::json!({})).is_err());
}

struct FakeAvs {
    seen: Vec<(String, String, usize)>,
    first_403: bool,
}

impl Transport for FakeAvs {
    fn post_event(&mut self, bearer: &str, ct: &str, body: &[u8]) -> Result<(u16, String, Vec<u8>), String> {
        self.seen.push((bearer.into(), ct.into(), body.len()));
        if self.first_403 && self.seen.len() == 1 {
            return Ok((403, String::new(), vec![]));
        }
        Ok((200, "multipart/related; boundary=q".into(), answer("q", true, "")))
    }
}

#[test]
fn a_turn_sends_the_recording_with_the_bearer_token_and_renews_an_expired_sign_in_once() {
    let a = amazon(vec![(200, TOK)]);
    let mut s = Session { client_id: "cid".into(), tokens: Tokens { access: "old".into(), refresh: "r".into(), expires_in: 3600 }, age_secs: 0, http: &a };
    let mut t = FakeAvs { seen: vec![], first_403: true };
    let turn = s.turn(&mut t, &[7u8; 3200]).unwrap();
    assert_eq!(turn.speech.len(), 1);
    assert_eq!(t.seen.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["old", "Atza|new"], "refused once, renewed, sent again");
    assert!(t.seen[0].1.starts_with("multipart/form-data; boundary=") && t.seen[0].2 > 3200);
    // a sign-in about to expire is renewed before sending
    let a2 = amazon(vec![(200, TOK)]);
    let mut s2 = Session { client_id: "cid".into(), tokens: Tokens { access: "old".into(), refresh: "r".into(), expires_in: 3600 }, age_secs: 3560, http: &a2 };
    let mut t2 = FakeAvs { seen: vec![], first_403: false };
    s2.turn(&mut t2, &[1u8; 100]).unwrap();
    assert_eq!(t2.seen[0].0, "Atza|new");
    // a second refusal after renewing is an error that says what to do
    let a3 = amazon(vec![(200, TOK)]);
    struct Always403;
    impl Transport for Always403 {
        fn post_event(&mut self, _: &str, _: &str, _: &[u8]) -> Result<(u16, String, Vec<u8>), String> {
            Ok((403, String::new(), vec![]))
        }
    }
    let mut s3 = Session { client_id: "cid".into(), tokens: Tokens { access: "old".into(), refresh: "r".into(), expires_in: 3600 }, age_secs: 0, http: &a3 };
    assert!(s3.turn(&mut Always403, &[1u8; 10]).unwrap_err().contains("link the device again"));
}
