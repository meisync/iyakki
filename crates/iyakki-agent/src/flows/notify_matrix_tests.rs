//! The notify node's Matrix channel on the agent host, against the fake homeserver: the account in the vault, the room by name, the
//! picture only from this run's camera folder, the dry run, and a refusal turning into one INBOX warning. No real account.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iyakki_flow::host::{ChannelNotice, Host, Notice};
use iyakki_sec::policy::{Policy, Role};
use zeroize::Zeroizing;

use super::*;
use crate::msg::fake_hs::*;
use crate::msg::{Auth, MessageService};

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

struct Rig {
    env: Arc<FlowEnv>,
    svc: Arc<Services>,
    user: String,
    dir: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn rig(tag: &str) -> Rig {
    let dir = std::env::temp_dir().join(format!("iyakki-nm-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let slow: Arc<dyn iyakki_proxy::Upstream> = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(10)));
    let svc = Services::open(&dir.join("svc"), Arc::new(Dead), slow).unwrap();
    let user = format!("nm{}", tag.replace(|c: char| !c.is_ascii_alphanumeric(), ""));
    svc.on_login(&user, "passw0rd-long-enough", false);
    let mut p: Policy = Role::User.template();
    p.net_hosts = vec!["*".into()];
    let env = Arc::new(FlowEnv::build_in(&svc, &user, &p, false, &dir.join("home")));
    Rig { env, svc, user, dir }
}

fn notice(_r: &Rig) -> Notice {
    Notice { title: "Possible person".into(), body: "at the front door, 02:14".into(), level: "warn".into(), flow_id: "f1".into(), run_id: "run1".into(), needs_you: false }
}

fn chan(attach: &str, dry: bool) -> ChannelNotice {
    ChannelNotice { channels: vec!["matrix".into()], room: String::new(), attach: attach.into(), dry_run: dry, dedupe_key: "f1:run1:n".into() }
}

fn add_account(r: &Rig, hs: &FakeHs) {
    let ms = MessageService::for_services(&r.svc, &r.user, Arc::new(|_, _| {}));
    ms.add_matrix("Home", &hs.url(), Auth::Password { user: "alice".into(), password: Zeroizing::new(PASSWORD.into()) }, "").unwrap();
}

#[test]
fn without_an_account_the_run_gets_one_plain_sentence_and_the_inbox_one_warning() {
    let r = rig("noacct");
    let e = r.env.host.notify_channels(&notice(&r), &chan("", false)).unwrap_err();
    assert!(e.contains("no messaging account"), "{e}");
    r.env.host.notify_channels(&notice(&r), &chan("", false)).unwrap_err();
    let notes: Vec<_> = r.svc.inbox.list(&r.user).into_iter().filter(|n| n.title == "Message not sent").collect();
    assert_eq!(notes.len(), 1, "one note, not one per run");
    let other = ChannelNotice { channels: vec!["sms".into()], ..chan("", false) };
    assert!(r.env.host.notify_channels(&notice(&r), &other).unwrap_err().contains("not available"));
}

#[test]
fn the_title_and_text_go_to_the_saved_room_once_per_run_and_node() {
    let hs = FakeHs::start();
    let r = rig("send");
    add_account(&r, &hs);
    let lines = r.env.host.notify_channels(&notice(&r), &chan("", false)).unwrap();
    assert!(lines[0].contains("accepted by the homeserver"), "{lines:?}");
    r.env.host.notify_channels(&notice(&r), &chan("", false)).unwrap();
    let s = hs.st.lock().unwrap();
    assert_eq!(s.events.len(), 1, "the same key (flow, run, node) is one message");
    let body: serde_json::Value = serde_json::from_slice(&s.events[0].2).unwrap();
    assert_eq!(body["body"], "Possible person\nat the front door, 02:14");
}

#[test]
fn only_a_picture_of_this_runs_camera_folder_can_be_attached() {
    let hs = FakeHs::start();
    let r = rig("pic");
    add_account(&r, &hs);
    let host = &r.env.host;
    // a file somewhere else, even a real picture, is refused and nothing is uploaded
    let elsewhere = r.dir.join("secret.jpg");
    std::fs::write(&elsewhere, [0xFF, 0xD8, 0xFF, 0xD9]).unwrap();
    let e = host.notify_channels(&notice(&r), &chan(&elsewhere.display().to_string(), false)).unwrap_err();
    assert!(e.contains("this run") || e.contains("no picture"), "{e}");
    assert!(hs.st.lock().unwrap().uploads.is_empty());
    // the same name in another run's folder is not this run's either
    let other = host.cam_folder("run2");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("p.jpg"), [0xFF, 0xD8, 0xFF, 0xD9]).unwrap();
    std::fs::create_dir_all(host.cam_folder("run1")).unwrap();
    assert!(host.notify_channels(&notice(&r), &chan(&other.join("p.jpg").display().to_string(), false)).is_err());
    // and this run's own picture goes
    let mine = host.cam_folder("run1").join("photo-1.jpg");
    std::fs::write(&mine, [0xFF, 0xD8, 0xFF, 0xD9]).unwrap();
    host.notify_channels(&notice(&r), &chan(&mine.display().to_string(), false)).unwrap();
    let s = hs.st.lock().unwrap();
    assert_eq!(s.uploads.len(), 1);
    assert_eq!(s.events.len(), 2, "the text and the picture");
}

#[test]
fn a_dry_run_sends_nothing_and_an_encrypted_room_is_refused_with_a_warning() {
    let hs = FakeHs::start();
    let r = rig("dry");
    add_account(&r, &hs);
    let before = hs.reqs().len();
    let l = r.env.host.notify_channels(&notice(&r), &chan("", true)).unwrap();
    assert!(l[0].contains("dry run") && l[0].contains("nothing was sent"));
    assert_eq!(hs.reqs().len(), before);
    let ms = MessageService::for_services(&r.svc, &r.user, Arc::new(|_, _| {}));
    let acc = ms.accounts().pick("").unwrap();
    ms.accounts().add_room(&acc.id, "family", ENC).unwrap();
    let c = ChannelNotice { room: "family".into(), ..chan("", false) };
    let e = r.env.host.notify_channels(&notice(&r), &c).unwrap_err();
    assert!(e.contains("end-to-end encrypted"), "{e}");
    assert_eq!(hs.count("PUT", "/send/"), 0);
    assert!(r.svc.inbox.list(&r.user).iter().any(|n| n.title == "Message not sent" && n.body.contains("encrypted")));
}

#[test]
fn the_accounts_network_policy_decides_which_server_a_flow_may_reach() {
    let hs = FakeHs::start();
    let r = rig("policy");
    add_account(&r, &hs);
    let mut p: Policy = Role::User.template();
    p.net_hosts = vec!["only.example.org".into()];
    let strict = FlowEnv::build_in(&r.svc, &r.user, &p, false, &r.dir.join("home2"));
    let before = hs.reqs().len();
    let e = strict.host.notify_channels(&notice(&r), &chan("", false)).unwrap_err();
    assert!(e.contains("may not reach 127.0.0.1"), "{e}");
    assert_eq!(hs.reqs().len(), before, "nothing was sent");
}

#[test]
fn an_imported_flow_sends_nothing_outside_until_switched_on_and_a_second_message_waits_for_the_cool_down() {
    let hs = FakeHs::start();
    let r = rig("switch");
    add_account(&r, &hs);
    let host = &r.env.host;
    // an imported flow: its outside channel is off, the sentence says so, nothing reaches the homeserver
    let mut imp = iyakki_flow::model::Flow::new("imp", "Imported");
    imp.nodes.push(iyakki_flow::model::Node::new("s", "start", serde_json::json!({})));
    let got = r.env.import_json(&imp.to_json(), "user:me").unwrap();
    let n = Notice { flow_id: got.id.clone(), ..notice(&r) };
    let before = hs.reqs().len();
    let e = host.notify_channels(&n, &chan("", false)).unwrap_err();
    assert!(e.contains("not allowed to send outside messages"), "{e}");
    assert_eq!(hs.reqs().len(), before);
    assert!(r.svc.inbox.list(&r.user).iter().any(|x| x.title == "Message not sent"), "one INBOX warning");
    // the person switches it on: it goes
    host.chan.set(&got.id, true).unwrap();
    host.notify_channels(&n, &chan("", false)).unwrap();
    assert_eq!(hs.st.lock().unwrap().events.len(), 1);
    // a different message from the same flow within the cool-down is refused; the same one again is the same event
    let other = ChannelNotice { dedupe_key: "imp:run2:n".into(), ..chan("", false) };
    let e = host.notify_channels(&n, &other).unwrap_err();
    assert!(e.contains("one every 5 minutes"), "{e}");
    host.notify_channels(&n, &chan("", false)).unwrap();
    assert_eq!(hs.st.lock().unwrap().events.len(), 1, "the retry is not a second message");
    // a flow made here is on from the start
    let mine = Notice { flow_id: "mine".into(), ..notice(&r) };
    host.notify_channels(&mine, &ChannelNotice { dedupe_key: "mine:run1:n".into(), ..chan("", false) }).unwrap();
    assert_eq!(hs.st.lock().unwrap().events.len(), 2);
}
