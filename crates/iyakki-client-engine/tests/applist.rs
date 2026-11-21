//! The display-list mode of the engine against a fake instance (D-CLIENT-PROTO-DISPLAYLIST, F-SDK-29): offered in hello, asked for only when the
//! machine agreed and the person is signed in, and the machine's list messages reach the page as records.

use iyakki_client_engine::engine::{Config, Engine, Out, Phase};
use iyakki_client_engine::host::Host;
use iyakki_client_engine::sign;
use iyakki_core_api::client::{self as proto, AppMsg, ClientMsg, ServerMsg};

const SEED: [u8; 32] = [3; 32];
const NONCE: &str = "00112233445566778899aabbccddeeff";

fn cfg() -> Config {
    Config { token: None, public: sign::hex(&sign::public_from_seed(&SEED)), name: "Maya's phone".into(), device: Some("dev1".into()), client: "test".into() }
}

fn sent(e: &mut Engine) -> (Vec<ClientMsg>, Vec<Out>) {
    let (mut msgs, mut rest) = (vec![], vec![]);
    for o in e.take_out() {
        match o {
            Out::Send { text } => msgs.push(proto::decode_client(&text).unwrap()),
            other => rest.push(other),
        }
    }
    (msgs, rest)
}

fn tell(e: &mut Engine, m: ServerMsg) {
    e.on_text(&proto::encode_server(&m));
}

fn tell_app(e: &mut Engine, m: AppMsg) {
    e.on_text(&proto::encode_app(&m));
}

fn live(caps: &[&str]) -> Engine {
    let mut e = Engine::new(cfg());
    e.connected();
    sent(&mut e);
    tell(&mut e, ServerMsg::HelloOk { v: 1, caps: caps.iter().map(|c| c.to_string()).collect(), challenge: "chal1".into(), tile: 64, max_message: proto::MAX_TEXT, deprecations: vec![] });
    sent(&mut e);
    assert!(e.sign_with_seed(&sign::hex(&SEED), 1_700_000_000, NONCE));
    sent(&mut e);
    tell(&mut e, ServerMsg::Authed { device: "dev1".into(), name: "Maya's phone".into() });
    assert!(e.sign_in("ann", "right"));
    sent(&mut e);
    tell(&mut e, ServerMsg::SignedIn { person: "ann".into() });
    sent(&mut e);
    tell(&mut e, ServerMsg::PictureOk);
    assert_eq!(e.phase(), Phase::Live);
    e
}

const BASE: &[&str] = &["picture", "input", "events", "calls"];
const WITH_LIST: &[&str] = &["picture", "input", "events", "calls", "display.list"];

#[test]
fn hello_offers_the_display_list_but_never_needs_it() {
    let mut e = Engine::new(cfg());
    e.connected();
    let (m, _) = sent(&mut e);
    let ClientMsg::Hello { offered, needed, .. } = &m[0] else { panic!() };
    assert!(offered.iter().any(|c| c == "display.list"));
    assert!(!needed.iter().any(|c| c == "display.list"), "picture mode stays the default and works without it");
}

#[test]
fn an_app_is_asked_for_only_when_the_machine_agreed_and_the_person_is_signed_in() {
    let mut old = live(BASE);
    assert!(!old.app_ready() && !old.app_open("sample", 800, 600) && !old.app_input("key enter") && !old.app_close(), "an older machine: nothing is sent");
    assert!(sent(&mut old).0.is_empty());
    let mut e = Engine::new(cfg());
    e.connected();
    assert!(!e.app_open("sample", 800, 600), "not even connected");
    let mut e = live(WITH_LIST);
    assert!(e.app_ready());
    assert!(!e.app_open("", 800, 600) && !e.app_open(&"x".repeat(65), 800, 600));
    assert!(e.app_open("sample", 800, 600) && e.app_input("pointer down 1 10 10 5") && e.app_close());
    assert!(!e.app_input(&"k".repeat(proto::MAX_TEXT)), "an input too long for one message is not sent");
    let (m, _) = sent(&mut e);
    assert_eq!(m, vec![ClientMsg::AppOpen { app: "sample".into(), w: 800, h: 600 }, ClientMsg::AppInput { lines: "pointer down 1 10 10 5".into() }, ClientMsg::AppClose]);
}

#[test]
fn the_list_messages_of_the_machine_reach_the_page_as_records() {
    let mut e = live(WITH_LIST);
    let list = "iyakki-display-list 1 800 600\n";
    tell_app(&mut e, AppMsg::AppList { app: "sample".into(), text: list.into() });
    tell_app(&mut e, AppMsg::AppErr { code: "denied".into(), message: "your role cannot open that".into() });
    tell_app(&mut e, AppMsg::AppClosed { app: "sample".into(), reason: "left".into() });
    let (_, rest) = sent(&mut e);
    let evs: Vec<&str> = rest.iter().filter_map(|o| if let Out::App { ev, .. } = o { Some(ev.as_str()) } else { None }).collect();
    assert_eq!(evs, vec!["app_list", "app_err", "app_closed"]);
    let Some(Out::App { body, .. }) = rest.iter().find(|o| matches!(o, Out::App { ev, .. } if ev == "app_list")) else { panic!() };
    assert_eq!((body["app"].as_str(), body["text"].as_str()), (Some("sample"), Some(list)));
    assert!(matches!(e.phase(), Phase::Live), "a refused app does not end the session");
    // a broken list message is a notice, not a crash or a refusal that ends the session
    e.on_text(r#"{"t":"app_list","app":"sample"}"#);
    e.on_text(&format!(r#"{{"t":"app_closed","app":"{}","reason":""}}"#, "x".repeat(proto::MAX_TEXT)));
    let (_, rest) = sent(&mut e);
    assert_eq!(rest.iter().filter(|o| matches!(o, Out::Notice { .. })).count(), 2);
    assert!(matches!(e.phase(), Phase::Live));
}

#[test]
fn the_json_host_layer_has_the_app_commands_and_refuses_them_when_not_ready() {
    let mut h = Host::new();
    let public = sign::hex(&sign::public_from_seed(&SEED));
    assert!(h.init(&format!(r#"{{"public":"{public}","name":"n","device":"dev1"}}"#)));
    assert_eq!(h.command(r#"{"c":"app_open","app":"sample","w":800,"h":600}"#), -1, "not signed in");
    {
        let e = h.engine().unwrap();
        *e = live(WITH_LIST);
    }
    assert_eq!(h.command(r#"{"c":"app_open","app":"sample"}"#), -1, "no size is no request");
    assert_eq!(h.command(r#"{"c":"app_open","app":"sample","w":800,"h":600}"#), 0);
    assert_eq!(h.command(r#"{"c":"app_input","lines":"key enter"}"#), 0);
    assert_eq!(h.command(r#"{"c":"app_close"}"#), 0);
    let mut texts = vec![];
    while h.next_out() > 0 {
        texts.push(String::from_utf8(h.reply().to_vec()).unwrap());
    }
    assert!(texts.iter().any(|t| t.contains("app_open")) && texts.iter().any(|t| t.contains("key enter")), "{texts:?}");
}

#[test]
fn file_transfer_is_offered_never_needed_and_the_machines_file_texts_reach_the_page_unread() {
    let mut e = Engine::new(cfg());
    e.connected();
    let (m, _) = sent(&mut e);
    let ClientMsg::Hello { offered, needed, .. } = &m[0] else { panic!() };
    assert!(offered.iter().any(|c| c == "files.transfer") && !needed.iter().any(|c| c == "files.transfer"));
    let mut e = live(WITH_LIST);
    e.on_text(r#"{"t":"file_list","dir":"Docs","entries":[{"name":"a.txt","is_dir":false,"size":3}],"truncated":false}"#);
    e.on_text(r#"{"t":"file_err","id":1,"code":"card_declined","message":"the person said no"}"#);
    let (_, rest) = sent(&mut e);
    let evs: Vec<(&str, Option<&str>)> = rest.iter().filter_map(|o| if let Out::App { ev, body } = o { Some((ev.as_str(), body["code"].as_str())) } else { None }).collect();
    assert_eq!(evs, vec![("file_list", None), ("file_err", Some("card_declined"))]);
    // a file text over an ordinary message is refused, not passed on
    e.on_text(&format!(r#"{{"t":"file_list","dir":"{}","entries":[],"truncated":false}}"#, "x".repeat(proto::MAX_TEXT)));
    let (_, rest) = sent(&mut e);
    assert!(rest.iter().any(|o| matches!(o, Out::Notice { .. })) && !rest.iter().any(|o| matches!(o, Out::App { .. })));
    assert!(matches!(e.phase(), Phase::Live));
}
