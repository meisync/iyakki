//! Settings → Git and SSH keys: making and importing keys (kept in the vault, never in a file), the public half to copy, the host table,
//! tokens, passphrases, forgetting a known host, and the page drawn at four screen sizes.

use super::git_page::GitSetting;
use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};

struct Offline;
impl iyakki_agent::iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_agent::iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn typ(a: &mut SettingsApp, cx: &AppCx, s: &str) {
    for c in s.chars() {
        key(a, cx, Key::Char(c));
    }
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = crate::shell::scale_for(w, h);
    a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, cx, 0);
    g
}

fn goto(a: &mut SettingsApp, cx: &AppCx, t: Tab) {
    for _ in 0..60 {
        if a.tab() == t {
            return;
        }
        key(a, cx, Key::Right);
    }
    panic!("no {t:?} page");
}

fn status(a: &SettingsApp) -> String {
    a.status.as_ref().map(|s| s.0.clone()).unwrap_or_default()
}

fn keys_of(cx: &AppCx) -> iyakki_agent::gitkeys::GitKeys {
    SettingsApp::git_keys(cx)
}

#[test]
fn the_page_is_for_accounts_that_have_the_ide_and_makes_a_key_that_lives_in_the_vault() {
    let mut guest = admin("gitset-guest");
    guest.session.user = "gus".into();
    guest.session.policy = iyakki_sec::policy::Role::Guest.template();
    assert!(!SettingsApp::new(&guest).tabs().contains(&Tab::GitKeys) || guest.session.policy.allows_app(iyakki_sec::policy::App::Ide));

    let cx = admin("gitset");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::GitKeys);
    let g = draw(&mut a, &cx, 1280, 800);
    assert!(g.hits.iter().any(|h| h.label == "Generate key") && g.hits.iter().any(|h| h.label == "Import key"), "the buttons are there");
    // generate: name and comment
    key(&mut a, &cx, Key::Char('n'));
    assert!(matches!(&a.modal, Some(Modal::Form(FormKind::Git(GitSetting::NewKey), _))));
    typ(&mut a, &cx, "work");
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Enter);
    assert!(a.modal.is_none(), "{}", status(&a));
    assert!(status(&a).contains("Made the key work"), "{}", status(&a));
    let k = keys_of(&cx).key("work").unwrap();
    assert!(k.public.starts_with("ssh-ed25519 ") && k.fingerprint.starts_with("SHA256:"));
    // the private half is in the vault under the CLI tile's name, and in no file of the data folder
    let vault = cx.services.user_get("root", "ssh.work").unwrap();
    assert!(vault.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"));
    fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut files = vec![];
    walk(cx.services.dir(), &mut files);
    for f in files {
        let t = std::fs::read(&f).unwrap_or_default();
        assert!(!String::from_utf8_lossy(&t).contains("BEGIN OPENSSH PRIVATE KEY"), "private key text in {}", f.display());
    }
    // the page shows the key, its fingerprint and the public half (and a QR code made of squares)
    let g = draw(&mut a, &cx, 1280, 900);
    assert!(g.hits.iter().any(|h| h.label == "key  work"));
    // a second key of the same name is refused with the reason
    key(&mut a, &cx, Key::Char('n'));
    typ(&mut a, &cx, "work");
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Enter);
    assert!(matches!(&a.modal, Some(Modal::Form(..))), "the form stays open with the error");
    key(&mut a, &cx, Key::Esc);
    // copy puts the public line on the clipboard, never the private key
    key(&mut a, &cx, Key::Char('c'));
    assert!(status(&a).contains("Copied the public key"), "{}", status(&a));
}

#[test]
fn a_key_is_imported_from_text_or_from_a_file_in_scope_and_a_bad_one_is_refused_plainly() {
    let cx = admin("gitset-imp");
    let k = iyakki_agent::sshkey::KeyPair::generate("imp@x").unwrap();
    let text = k.to_openssh().to_string();
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::GitKeys);
    // pasted as one line (a text field has no line breaks): still a key
    key(&mut a, &cx, Key::Char('i'));
    typ(&mut a, &cx, "pasted");
    key(&mut a, &cx, Key::Enter);
    a.handle(&Event::Paste(text.replace('\n', " ")), &cx);
    key(&mut a, &cx, Key::Ctrl('s'));
    assert!(a.modal.is_none() && status(&a).contains("Imported the key pasted"), "{}", status(&a));
    assert_eq!(keys_of(&cx).key("pasted").unwrap().public, k.public_line());
    // from a file the account may read
    let dir = temp_dir("gitset-imp-file");
    let f = dir.join("id_ed25519");
    let k2 = iyakki_agent::sshkey::KeyPair::generate("file@x").unwrap();
    std::fs::write(&f, k2.to_openssh().as_bytes()).unwrap();
    let mut cx2 = cx.clone();
    cx2.session.policy.fs_read = vec![dir.display().to_string()];
    let mut b = SettingsApp::new(&cx2);
    goto(&mut b, &cx2, Tab::GitKeys);
    key(&mut b, &cx2, Key::Char('i'));
    typ(&mut b, &cx2, "fromfile");
    key(&mut b, &cx2, Key::Enter);
    a.handle(&Event::Paste("x".into()), &cx); // (another app's focus must not leak)
    b.handle(&Event::Paste(f.display().to_string()), &cx2);
    key(&mut b, &cx2, Key::Ctrl('s'));
    assert!(status(&b).contains("Imported the key fromfile"), "{}", status(&b));
    // a file outside the readable folders
    let mut cx3 = cx.clone();
    cx3.session.policy.fs_read = vec!["/nonexistent-readable-folder".into()];
    let mut c = SettingsApp::new(&cx3);
    goto(&mut c, &cx3, Tab::GitKeys);
    key(&mut c, &cx3, Key::Char('i'));
    typ(&mut c, &cx3, "nope");
    key(&mut c, &cx3, Key::Enter);
    c.handle(&Event::Paste(f.display().to_string()), &cx3);
    key(&mut c, &cx3, Key::Ctrl('s'));
    assert!(matches!(&c.modal, Some(Modal::Form(_, f)) if f.error.as_ref().is_some_and(|e| e.contains("outside the folders"))));
    // text that is not a key
    key(&mut c, &cx3, Key::Esc);
    key(&mut c, &cx3, Key::Char('i'));
    typ(&mut c, &cx3, "junk");
    key(&mut c, &cx3, Key::Enter);
    c.handle(&Event::Paste("hello there".into()), &cx3);
    key(&mut c, &cx3, Key::Ctrl('s'));
    assert!(matches!(&c.modal, Some(Modal::Form(_, f)) if f.error.as_ref().is_some_and(|e| e.contains("not an OpenSSH private key"))));
}

#[test]
fn hosts_get_keys_and_tokens_and_a_known_host_can_be_forgotten() {
    let cx = admin("gitset-host");
    let g = keys_of(&cx);
    g.generate("a", "a@x").unwrap();
    g.generate("b", "b@x").unwrap();
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::GitKeys);
    // host -> key: the form's key choice lists "(no key)" first, then the keys
    key(&mut a, &cx, Key::Char('h'));
    typ(&mut a, &cx, "github.com");
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Right); // a -> b
    key(&mut a, &cx, Key::Ctrl('s'));
    assert!(status(&a).contains("github.com uses the key b"), "{}", status(&a));
    assert_eq!(g.table().hosts["github.com"].key, "b");
    // a token for https
    key(&mut a, &cx, Key::Char('t'));
    typ(&mut a, &cx, "github.com");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "octo");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "ghp_TOKENVALUE123");
    key(&mut a, &cx, Key::Ctrl('s'));
    assert!(status(&a).contains("token for github.com is in your vault"), "{}", status(&a));
    assert!(g.table().hosts["github.com"].token);
    assert!(!std::fs::read_to_string(g_dir(&cx).join("git.json")).unwrap().contains("TOKENVALUE"), "the table holds no secret");
    let g2 = draw(&mut a, &cx, 1280, 900);
    assert!(g2.hits.iter().any(|h| h.label == "host github.com"));
    // remove the rule (selects the host row first)
    let rows = SettingsApp::git_rows(&cx);
    let pos = rows.iter().position(|r| matches!(r, super::git_page::GRow::Host(h, _) if h == "github.com")).unwrap();
    a.sel[a.tab] = pos;
    key(&mut a, &cx, Key::Char('x'));
    assert!(status(&a).contains("Removed the key rule and token of github.com"), "{}", status(&a));
    assert!(g.table().hosts.is_empty());
    // a known host: listed with its fingerprint, forgotten on request
    let mut blob = Vec::new();
    iyakki_agent::sshkey::put_str(&mut blob, b"ssh-ed25519");
    iyakki_agent::sshkey::put_str(&mut blob, &[5u8; 32]);
    let hk = iyakki_agent::ssh::HostKey { pattern: "git.example.org".into(), kind: "ssh-ed25519".into(), b64: iyakki_agent::story::base64_encode(&blob) };
    g.known_hosts().trust("git.example.org", 22, &[hk]).unwrap();
    let rows = SettingsApp::git_rows(&cx);
    let pos = rows.iter().position(|r| matches!(r, super::git_page::GRow::Known(h, ..) if h == "git.example.org")).unwrap();
    a.sel[a.tab] = pos;
    let g3 = draw(&mut a, &cx, 1280, 900);
    assert!(g3.hits.iter().any(|h| h.label == "trusted  git.example.org"));
    key(&mut a, &cx, Key::Char('f'));
    assert!(status(&a).contains("Forgot 1 key(s) of git.example.org"), "{}", status(&a));
    assert!(g.known_hosts().read().is_empty());
}

fn g_dir(cx: &AppCx) -> std::path::PathBuf {
    iyakki_agent::ssh::Env::for_services(&cx.services, &cx.session.user).dir
}

#[test]
fn a_passphrase_key_is_unlocked_once_and_a_delete_needs_the_name_typed() {
    if std::process::Command::new("ssh-keygen").arg("-?").output().is_err() {
        eprintln!("skipped: no ssh-keygen");
        return;
    }
    let cx = admin("gitset-pass");
    let d = temp_dir("gitset-pass-key");
    let f = d.join("k");
    assert!(std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "open sesame", "-C", "p@x", "-f"]).arg(&f).status().unwrap().success());
    let text = std::fs::read_to_string(&f).unwrap();
    let g = keys_of(&cx);
    g.import("locked", &text, Some("open sesame"), false).unwrap();
    iyakki_agent::gitkeys::shared_session().forget();
    assert!(g.pair("locked").is_err(), "locked again after the session was forgotten");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::GitKeys);
    key(&mut a, &cx, Key::Char('u'));
    typ(&mut a, &cx, "wrong");
    key(&mut a, &cx, Key::Ctrl('s'));
    assert!(matches!(&a.modal, Some(Modal::Form(_, f)) if f.error.as_ref().is_some_and(|e| e.contains("does not open"))));
    for _ in 0..5 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "open sesame");
    key(&mut a, &cx, Key::Ctrl('s'));
    assert!(status(&a).contains("unlocked for this session"), "{}", status(&a));
    assert!(g.pair("locked").is_ok());
    // delete: the wrong name does nothing
    key(&mut a, &cx, Key::Char('d'));
    typ(&mut a, &cx, "lock");
    key(&mut a, &cx, Key::Ctrl('s'));
    assert!(g.key("locked").is_some());
    key(&mut a, &cx, Key::Backspace);
    key(&mut a, &cx, Key::Backspace);
    key(&mut a, &cx, Key::Backspace);
    key(&mut a, &cx, Key::Backspace);
    typ(&mut a, &cx, "locked");
    key(&mut a, &cx, Key::Ctrl('s'));
    assert!(g.key("locked").is_none(), "{}", status(&a));
    assert!(cx.services.user_get("root", "ssh.locked").is_err());
}

#[test]
fn the_page_draws_inside_the_window_at_the_four_sizes_with_many_keys_and_a_qr_code() {
    let cx = admin("gitset-draw");
    let g = keys_of(&cx);
    for i in 0..8 {
        g.generate(&format!("key{i}"), "c@x").unwrap();
    }
    for h in ["github.com", "gitlab.com", "git.example.org"] {
        g.set_host_key(h, "key1").unwrap();
    }
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::GitKeys);
    for (w, h) in [(1280u32, 800u32), (1920, 1080), (1024, 600), (640, 360)] {
        for sel in [0usize, 5, 9] {
            a.sel[a.tab] = sel;
            let gf = draw(&mut a, &cx, w, h);
            assert!(!gf.hits.is_empty());
            for hit in gf.hits.iter().filter(|x| x.rect.w > 0.0) {
                let r = hit.rect;
                assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {:?} {:?}", hit.label, r);
            }
            assert!(gf.hits.iter().any(|x| x.label == "Generate key"), "{w}x{h}");
        }
    }
}

// ------------------------------------------------------------------------------------------------ connections

use iyakki_agent::gitconn::FakeGitHttp;
use super::git_page::GRow;

const TOK: &str = "tok_FAKEFAKEFAKEFAKE1234";

fn fake_api() -> Arc<FakeGitHttp> {
    let h = Arc::new(FakeGitHttp::default());
    h.answer("GET https://git.example.org/api/v1/user/repos", 200, &[], "[]").answer("GET https://git.example.org/api/v1/user", 200, &[], r#"{"login":"octo","name":"Octo Cat","email":"octo@example.org"}"#);
    h
}

fn settle(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..300 {
        a.handle(&Event::Tick, cx);
        if !a.git.wiz.as_ref().is_some_and(|w| w.busy()) && a.git.testing.is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Add a Gitea connection through the wizard with the keyboard.
fn add_gitea(a: &mut SettingsApp, cx: &AppCx) {
    key(a, cx, Key::Char('a'));
    assert!(a.git_wizard_open(), "the wizard opens on a");
    key(a, cx, Key::Enter); // kind: Gitea
    key(a, cx, Key::Ctrl('s')); // where: name and address
    a.git.wiz.as_mut().unwrap().set_text("name", "Home Gitea");
    a.git.wiz.as_mut().unwrap().set_text("url", "https://git.example.org");
    key(a, cx, Key::Ctrl('s'));
    key(a, cx, Key::Down);
    a.handle(&Event::Paste(TOK.into()), cx);
    key(a, cx, Key::Ctrl('s'));
    settle(a, cx);
    key(a, cx, Key::Ctrl('s')); // test passed
    key(a, cx, Key::Ctrl('s')); // commits: save
}

#[test]
fn add_connection_opens_the_wizard_from_the_page_and_saves_a_row_with_the_token_in_the_vault() {
    let cx = admin("gitconn-add");
    let mut a = SettingsApp::new(&cx);
    a.git.http = Some(fake_api());
    goto(&mut a, &cx, Tab::GitKeys);
    let g = draw(&mut a, &cx, 1280, 800);
    for l in ["Add connection", "Test", "Edit", "Make default", "Remove connection", "Generate key"] {
        assert!(g.hits.iter().any(|h| h.label == l), "button {l}");
    }
    assert!(a.has_modal() == false);
    add_gitea(&mut a, &cx);
    assert!(!a.git_wizard_open(), "{}", status(&a));
    assert!(status(&a).contains("Connected Home Gitea (git.example.org, octo)") && status(&a).contains("default"), "{}", status(&a));
    let conns = keys_of(&cx).conns();
    let c = conns.list().remove(0);
    assert_eq!((c.name.as_str(), c.account.as_str(), c.default), ("Home Gitea", "octo", true));
    assert_eq!(conns.token(&c).unwrap().as_str(), TOK);
    // the list shows it (selected), with its status, and the page draws at four sizes
    let rows = SettingsApp::git_rows(&cx);
    assert!(matches!(&rows[0], GRow::Conn(x) if x.id == c.id) && a.sel[a.tab] == 0);
    for (w, h) in [(1280, 800), (1920, 1080), (1024, 600), (640, 360)] {
        draw(&mut a, &cx, w, h);
    }
    // not in the audit, the settings or any file under the services folder
    let audit = std::fs::read_to_string(cx.audit.path()).unwrap_or_default();
    assert!(!audit.contains("FAKEFAKE"));
    let mut stack = vec![cx.services.dir().to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if !p.to_string_lossy().ends_with("vault") && !p.to_string_lossy().contains("vault") {
                assert!(!std::fs::read_to_string(&p).unwrap_or_default().contains("FAKEFAKEFAKE"), "token in {}", p.display());
            }
        }
    }
}

#[test]
fn cancel_leaves_nothing_and_the_old_key_buttons_still_work() {
    let cx = admin("gitconn-cancel");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::GitKeys);
    key(&mut a, &cx, Key::Char('a'));
    key(&mut a, &cx, Key::Enter);
    a.git.wiz.as_mut().unwrap().set_text("name", "Never");
    key(&mut a, &cx, Key::Esc);
    assert!(!a.git_wizard_open());
    assert!(status(&a).contains("Nothing was connected or stored"));
    assert!(keys_of(&cx).conns().list().is_empty());
    // while the wizard is open, other keys do not reach the page
    key(&mut a, &cx, Key::Char('a'));
    key(&mut a, &cx, Key::Char('n'));
    assert!(a.modal.is_none(), "n did not open the key form behind the wizard");
    key(&mut a, &cx, Key::Esc);
    // the key form of the old page
    key(&mut a, &cx, Key::Char('n'));
    assert!(matches!(&a.modal, Some(Modal::Form(FormKind::Git(GitSetting::NewKey), _))));
    key(&mut a, &cx, Key::Esc);
    // a connection key on a key row says what to choose
    key(&mut a, &cx, Key::Char('T'));
    assert!(status(&a).contains("Choose a connection first"));
}

#[test]
fn test_edit_make_default_and_remove_work_on_the_selected_connection() {
    let cx = admin("gitconn-ops");
    let mut a = SettingsApp::new(&cx);
    let http = fake_api();
    a.git.http = Some(http.clone());
    goto(&mut a, &cx, Tab::GitKeys);
    add_gitea(&mut a, &cx);
    let conns = keys_of(&cx).conns();
    // a second connection through the API directly, to have something to make default
    conns.add(iyakki_agent::gitconn::Connection { name: "Work".into(), url: "https://github.com".into(), kind: iyakki_agent::gitconn::Kind::GitHub, ..Default::default() }, Some("ghp_FAKEFAKEFAKEFAKEFAKE")).unwrap();
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Char('m'));
    assert!(status(&a).contains("Work is now the default"), "{} sel={} rows={}", status(&a), a.sel[a.tab], SettingsApp::git_len(&cx));
    assert_eq!(conns.default_connection().unwrap().name, "Work");
    // test the first one: 401 is red and recorded, 200 is green
    key(&mut a, &cx, Key::Up);
    let bad = Arc::new(FakeGitHttp::default());
    bad.answer("/api/v1/user", 401, &[], "{}");
    a.git.http = Some(bad);
    key(&mut a, &cx, Key::Char('T'));
    assert!(status(&a).contains("Testing Home Gitea"));
    settle(&mut a, &cx);
    assert!(a.status.as_ref().is_some_and(|s| !s.1 && s.0.contains("refused (401)")), "{:?}", a.status);
    assert!(conns.get("home-gitea").unwrap().last_test.is_some_and(|t| !t.ok));
    a.git.http = Some(http.clone());
    key(&mut a, &cx, Key::Char('T'));
    settle(&mut a, &cx);
    assert!(a.status.as_ref().is_some_and(|s| s.1 && s.0.contains("Signed in as octo")), "{:?}", a.status);
    assert!(conns.get("home-gitea").unwrap().last_test.is_some_and(|t| t.ok));
    assert!(http.requests().iter().all(|r| r.token.as_deref() == Some(TOK)));
    // edit opens the wizard on the connection
    key(&mut a, &cx, Key::Char('e'));
    assert!(a.git_wizard_open() && a.git.wiz.as_ref().unwrap().text("name") == "Home Gitea");
    key(&mut a, &cx, Key::Esc);
    // remove: the name must be typed; the token leaves the vault; the audit has the names
    key(&mut a, &cx, Key::Char('R'));
    assert!(matches!(&a.modal, Some(Modal::Form(FormKind::Git(GitSetting::RemoveConn(_)), _))));
    typ(&mut a, &cx, "wrong");
    key(&mut a, &cx, Key::Enter);
    assert!(a.modal.is_some(), "wrong name: still asking");
    for _ in 0..5 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "Home Gitea");
    key(&mut a, &cx, Key::Enter);
    assert!(a.modal.is_none(), "{}", status(&a));
    assert!(conns.get("home-gitea").is_none());
    assert!(!keys_of(&cx).conns().token_present(&iyakki_agent::gitconn::Connection { token_vault: "git.conn.home-gitea.token".into(), ..Default::default() }));
    // the audit names, not tokens
    let log = std::fs::read_to_string(cx.audit.path()).unwrap_or_default();
    for want in ["git.connect.add", "git.connect.default", "git.connect.test", "git.connect.remove"] {
        assert!(log.contains(want), "{want} in {log}");
    }
    assert!(!log.contains("FAKEFAKE"));
}

#[test]
fn a_refused_network_policy_is_said_and_no_request_is_made() {
    let mut cx = admin("gitconn-policy");
    let mut a = SettingsApp::new(&cx);
    let http = fake_api();
    a.git.http = Some(http.clone());
    goto(&mut a, &cx, Tab::GitKeys);
    add_gitea(&mut a, &cx);
    let n = http.requests().len();
    let mut policy = cx.session.policy.clone();
    policy.net_hosts = vec!["example.com".into()];
    cx.session.policy = policy;
    key(&mut a, &cx, Key::Char('T'));
    assert!(a.status.as_ref().is_some_and(|s| !s.1 && s.0.contains("may not reach git.example.org")), "{:?}", a.status);
    assert_eq!(http.requests().len(), n, "nothing was sent");
}
