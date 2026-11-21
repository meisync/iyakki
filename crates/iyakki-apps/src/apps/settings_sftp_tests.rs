//! Settings -> Remote access -> SFTP file server: the switches, accounts, keys, the card before a start, the password-login card, the
//! host key fingerprint and its copy button. The server has a fake firewall here and listens on loopback only.

use std::sync::Mutex;

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_agent::sftp::{Firewall, Sftp};

struct Fw(Mutex<Vec<String>>);
impl Firewall for Fw {
    fn open(&self, port: u16, _expires: i64) -> Result<(), String> {
        self.0.lock().unwrap().push(format!("open {port}"));
        Ok(())
    }
    fn close(&self) -> Result<(), String> {
        self.0.lock().unwrap().push("close".into());
        Ok(())
    }
}

struct Off;
impl iyakki_proxy::Upstream for Off {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("no network in tests".into())
    }
}

fn page(tag: &str) -> (AppCx, SettingsApp, Arc<Fw>, Arc<Sftp>) {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Off));
    cx.services.on_login("root", "rootpass12", true);
    let fw = Arc::new(Fw(Mutex::new(Vec::new())));
    let s2 = cx.store.clone();
    let s3 = cx.store.clone();
    let auth = iyakki_agent::sftp::FnAuth::new(
        Arc::new(move |n, p| s2.lock().unwrap().authenticate(n, p).map(|s| s.policy).map_err(|e| e.to_string())),
        Arc::new(move |n| s3.lock().unwrap().get(n).map(|u| u.policy())),
    );
    let sftp = Arc::new(Sftp::new(cx.data_dir(), Arc::new(auth), { let a2 = cx.audit.clone(); Arc::new(move |e: &str, d: &str| a2.record("sftp", e, d)) }, fw.clone()));
    remote_page::sftp_page::TEST_SFTP.with(|t| *t.borrow_mut() = Some(Arc::new(iyakki_agent::facets_b6::SftpReal(sftp.clone()))));
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Remote);
    (cx, a, fw, sftp)
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}
fn clear_and_type(a: &mut SettingsApp, cx: &AppCx, s: &str) {
    (0..90).for_each(|_| key(a, cx, Key::Backspace));
    for c in s.chars() {
        key(a, cx, Key::Char(c));
    }
}
fn say(a: &SettingsApp) -> String {
    a.status().map(|(t, _)| t.clone()).unwrap_or_default()
}
fn card(a: &SettingsApp) -> Option<String> {
    match &a.modal {
        Some(Modal::Confirm(ConfirmKind::Sftp(_), t)) => Some(t.clone()),
        _ => None,
    }
}
fn pubkey(seed: u8) -> String {
    iyakki_sshfs::sshkey::KeyPair::from_seed([seed; 32], "laptop").public_line()
}

#[test]
fn the_page_shows_the_state_the_buttons_and_the_host_key_and_a_start_needs_an_account_and_a_way_to_log_in() {
    let (cx, mut a, fw, _s) = page("sftp-a");
    assert!(a.remote_buttons().iter().any(|b| b.0 == "Start or stop SFTP" && b.2 == 'F'));
    let g = {
        let mut g = Gfx::new(1280, 720);
        g.clear(rgb(0, 0, 0));
        a.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, 0);
        g
    };
    let texts: Vec<String> = g.text_log.iter().map(|t| t.2.clone()).collect();
    assert!(texts.iter().any(|t| t.contains("SFTP file server: off")), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("no account is allowed yet")), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("Host key fingerprint")));
    // nothing can start yet
    key(&mut a, &cx, Key::Char('F'));
    assert!(say(&a).contains("allow an account first"), "{}", say(&a));
    assert!(a.modal.is_none() && fw.0.lock().unwrap().is_empty());
    // an account is allowed, but there is still no key and no password login
    key(&mut a, &cx, Key::Char('U'));
    clear_and_type(&mut a, &cx, "root");
    key(&mut a, &cx, Key::Enter);
    assert!(say(&a).contains("SFTP accounts allowed: root"), "{}", say(&a));
    key(&mut a, &cx, Key::Char('F'));
    assert!(say(&a).contains("nobody could log in"), "{}", say(&a));
}

#[test]
fn keys_are_added_listed_by_fingerprint_and_removed_and_a_private_key_is_refused() {
    let (cx, mut a, _fw, sftp) = page("sftp-k");
    key(&mut a, &cx, Key::Char('U'));
    clear_and_type(&mut a, &cx, "root");
    key(&mut a, &cx, Key::Enter);
    // a private key is refused by the form
    key(&mut a, &cx, Key::Char('K'));
    clear_and_type(&mut a, &cx, "root");
    key(&mut a, &cx, Key::Enter);
    clear_and_type(&mut a, &cx, "-----BEGIN OPENSSH PRIVATE KEY-----");
    key(&mut a, &cx, Key::Enter);
    assert!(a.modal.is_some() || say(&a).contains("private key"), "{}", say(&a));
    key(&mut a, &cx, Key::Esc);
    assert!(sftp.status().keys.iter().all(|(_, k)| k.is_empty()));
    // a public key
    key(&mut a, &cx, Key::Char('K'));
    clear_and_type(&mut a, &cx, "root");
    key(&mut a, &cx, Key::Enter);
    clear_and_type(&mut a, &cx, &pubkey(7));
    key(&mut a, &cx, Key::Enter);
    let fp = iyakki_sshfs::sshkey::KeyPair::from_seed([7; 32], "x").fingerprint();
    assert!(say(&a).contains(&fp), "{}", say(&a));
    assert_eq!(sftp.status().keys, vec![("root".to_string(), vec![fp.clone()])]);
    let g = {
        let mut g = Gfx::new(1280, 720);
        g.clear(rgb(0, 0, 0));
        a.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, 0);
        g
    };
    assert!(g.text_log.iter().any(|t| t.2 == fp), "the fingerprint is listed");
    // and removed
    key(&mut a, &cx, Key::Char('R'));
    clear_and_type(&mut a, &cx, "root");
    key(&mut a, &cx, Key::Enter);
    clear_and_type(&mut a, &cx, &fp);
    key(&mut a, &cx, Key::Enter);
    assert!(say(&a).contains("Key removed"), "{}", say(&a));
    assert!(sftp.status().keys.iter().all(|(_, k)| k.is_empty()));
    let audit = std::fs::read_to_string(cx.audit.path_buf()).unwrap_or_default();
    assert!(audit.contains("\"event\":\"sftp.key_add\"") && audit.contains("\"event\":\"sftp.key_remove\"") && audit.contains("\"detail\":\"root SHA256:") && !audit.contains("PRIVATE") && !audit.contains("ssh-ed25519"), "{audit}");
}

#[test]
fn starting_shows_a_card_with_what_becomes_reachable_and_only_y_starts_it_and_stopping_is_at_once() {
    let (cx, mut a, fw, sftp) = page("sftp-s");
    key(&mut a, &cx, Key::Char('U'));
    clear_and_type(&mut a, &cx, "root");
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Char('K'));
    clear_and_type(&mut a, &cx, "root");
    key(&mut a, &cx, Key::Enter);
    clear_and_type(&mut a, &cx, &pubkey(9));
    key(&mut a, &cx, Key::Enter);
    // a port that is free and allowed
    sftp.configure(|c| {
        c.port = 20000 + (std::process::id() % 2000) as u16;
        Ok(())
    })
    .unwrap();
    key(&mut a, &cx, Key::Char('F'));
    let t = card(&a).expect("a card");
    assert!(t.contains("local network only") && t.contains("Accounts allowed: root") && t.contains("log in with a key.") && t.contains("stops by itself for 1 day"), "{t}");
    // n cancels: nothing listens, the firewall was not touched
    key(&mut a, &cx, Key::Char('n'));
    assert!(a.modal.is_none() && !sftp.status().running && fw.0.lock().unwrap().is_empty());
    // y starts
    key(&mut a, &cx, Key::Char('F'));
    key(&mut a, &cx, Key::Char('y'));
    assert!(say(&a).contains("The SFTP server is on at port"), "{}", say(&a));
    assert!(sftp.status().running);
    assert_eq!(fw.0.lock().unwrap().len(), 1);
    // f again stops it without a card
    key(&mut a, &cx, Key::Char('F'));
    assert!(a.modal.is_none() && !sftp.status().running);
    assert!(say(&a).contains("stopped"), "{}", say(&a));
    assert!(fw.0.lock().unwrap().iter().any(|l| l == "close"));
    let audit = std::fs::read_to_string(cx.audit.path_buf()).unwrap_or_default();
    assert!(audit.contains("\"event\":\"sftp.enable\"") && audit.contains("\"event\":\"sftp.disable\""), "{audit}");
}

#[test]
fn password_login_asks_when_turned_on_and_not_when_turned_off_and_the_time_limit_cycles() {
    let (cx, mut a, _fw, sftp) = page("sftp-p");
    key(&mut a, &cx, Key::Char('W'));
    assert!(say(&a).contains("Allow an account first"), "{}", say(&a));
    key(&mut a, &cx, Key::Char('U'));
    clear_and_type(&mut a, &cx, "root, amy");
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Char('W'));
    let t = card(&a).expect("a card");
    assert!(t.contains("guess a password") && t.contains("amy, root"), "{t}");
    key(&mut a, &cx, Key::Char('n'));
    assert!(!sftp.config().password_login);
    key(&mut a, &cx, Key::Char('W'));
    key(&mut a, &cx, Key::Char('y'));
    assert!(sftp.config().password_login);
    key(&mut a, &cx, Key::Char('W'));
    assert!(a.modal.is_none() && !sftp.config().password_login, "off needs no card");
    // the time limit goes 1 hour, 1 day, 7 days, never, round again
    let before = sftp.config().expiry_secs;
    key(&mut a, &cx, Key::Char('T'));
    assert_ne!(sftp.config().expiry_secs, before);
    for _ in 0..3 {
        key(&mut a, &cx, Key::Char('T'));
    }
    assert_eq!(sftp.config().expiry_secs, before, "four steps are a full round");
}

#[test]
fn the_host_key_fingerprint_is_copied_once_it_exists() {
    let (cx, mut a, _fw, sftp) = page("sftp-c");
    key(&mut a, &cx, Key::Char('C'));
    assert!(say(&a).contains("made when the server first starts") || say(&a).contains("fingerprint is copied"), "{}", say(&a));
    // the host key is made on first use of the status; a copy then works
    let fp = sftp.status().host_fingerprint;
    if !fp.is_empty() {
        key(&mut a, &cx, Key::Char('C'));
        assert!(say(&a).contains("fingerprint is copied"), "{}", say(&a));
    }
}

#[test]
fn the_page_and_the_status_show_the_ecdsa_host_fingerprint_beside_the_ed25519_one() {
    let (cx, a, _fw, sftp) = page("sftp-ec");
    let st = sftp.status();
    assert!(st.host_fingerprint_ec.starts_with("SHA256:") && st.host_fingerprint.starts_with("SHA256:"), "{st:?}");
    assert_ne!(st.host_fingerprint, st.host_fingerprint_ec, "two different keys");
    assert_eq!(sftp.status().host_fingerprint_ec, st.host_fingerprint_ec, "kept between looks");
    let mut lines = Vec::new();
    a.sftp_lines(&cx, &mut lines);
    let text: Vec<&str> = lines.iter().map(|l| l.0.as_str()).collect();
    assert!(text.iter().any(|l| l.contains(&st.host_fingerprint_ec) && l.contains("ECDSA")), "{text:?}");
    assert!(text.iter().any(|l| l.contains(&st.host_fingerprint) && l.contains("Ed25519")), "{text:?}");
}
