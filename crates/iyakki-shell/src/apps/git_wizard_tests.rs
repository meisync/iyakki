//! "Connect Git or Gitea": every step with the keyboard, back, cancel at each step (nothing is kept), the token only in the vault, the
//! test for each kind against a fake API (200, 401, no write scope, unreachable, self-signed with its fingerprint), the Gitea of the
//! cluster and the "none installed" hint, the commit identity and the default.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use iyakki_agent::gitconn::{FakeGitHttp, GitHttp, NetErr, Req, Resp};
use iyakki_agent::gitkeys::{MemStore, SecretStore, Session};

use super::*;
use crate::gfx::rgb;

const TOK: &str = "tok_FAKEFAKEFAKEFAKE1234";

pub(crate) fn temp(tag: &str) -> PathBuf {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let mut r = [0u8; 3];
    iyakki_sec::random_fill(&mut r).unwrap();
    let d = Path::new(&base).join(format!("gw-{tag}-{:02x}{:02x}{:02x}", r[0], r[1], r[2]));
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub(crate) struct Fx {
    pub dir: PathBuf,
    pub store: Arc<MemStore>,
    pub log: Arc<Mutex<Vec<String>>>,
    pub http: Arc<FakeGitHttp>,
    pub keys: Arc<GitKeys>,
}

pub(crate) fn fx(tag: &str) -> Fx {
    let dir = temp(tag);
    let store = Arc::new(MemStore::default());
    let log = Arc::new(Mutex::new(Vec::new()));
    let l2 = log.clone();
    let keys = Arc::new(GitKeys::new(&dir.join("ssh"), store.clone(), Arc::new(move |a, d| l2.lock().unwrap().push(format!("{a} {d}"))), Arc::new(Session::default())));
    Fx { dir, store, log, http: Arc::new(FakeGitHttp::default()), keys }
}

impl Fx {
    pub fn env_with(&self, http: Arc<dyn GitHttp>) -> WizEnv {
        WizEnv { keys: self.keys.clone(), http, dir: self.dir.join("svc"), user: "alice".into(), egress: Arc::new(|_| Ok(())) }
    }
    pub fn env(&self) -> WizEnv {
        self.env_with(self.http.clone())
    }
    pub fn gitea_ok(&self) {
        self.http.answer("GET https://git.example.org/api/v1/user/repos", 200, &[], "[]").answer("GET https://git.example.org/api/v1/user", 200, &[], r#"{"login":"octo","name":"Octo Cat","email":"octo@example.org"}"#);
    }
}

fn press(w: &mut GitWizard, k: Key) -> WizOut {
    w.event(&Event::Key(k))
}

fn typ(w: &mut GitWizard, s: &str) {
    for c in s.chars() {
        press(w, Key::Char(c));
    }
}

/// Kind step -> Where step for a Gitea at an address, then the token step with the token pasted.
fn to_signin(w: &mut GitWizard) {
    assert_eq!(w.step, Step::Kind);
    press(w, Key::Enter);
    assert_eq!(w.step, Step::Where);
    // the first slot is the Gitea choice: an existing server
    if w.cluster.is_empty() {
        assert_eq!(w.mode, 1);
    }
    w.set_text("name", "");
    w.set_text("url", "");
    w.mode = 1;
    w.focus = 1;
    typ(w, "Home Gitea");
    press(w, Key::Enter);
    typ(w, "https://git.example.org");
    press(w, Key::Enter);
    assert_eq!(w.step, Step::SignIn, "{:?}", w.error);
}

fn to_test(w: &mut GitWizard) {
    to_signin(w);
    press(w, Key::Down); // from the way of signing in to the token
    w.event(&Event::Paste(TOK.into()));
    press(w, Key::Enter);
    assert_eq!(w.step, Step::Test, "{:?}", w.error);
    w.wait();
}

fn nothing_kept(f: &Fx) {
    assert!(f.keys.conns().list().is_empty(), "no connection");
    assert!(f.store.names().unwrap().is_empty(), "nothing in the vault: {:?}", f.store.names());
    let certs = f.keys.conns().certs_dir();
    assert!(std::fs::read_dir(certs).map(|d| d.count() == 0).unwrap_or(true), "no certificate file");
}

#[test]
fn the_wizard_walks_five_steps_by_keyboard_and_stores_the_token_in_the_vault_only() {
    let f = fx("walk");
    f.gitea_ok();
    let mut w = GitWizard::new(f.env());
    assert_eq!(w.kind_value(), Kind::Gitea, "Gitea / Forgejo is the first choice for an unknown server");
    to_test(&mut w);
    let r = w.report.clone().unwrap();
    assert!(r.ok && r.account == "octo", "{r:?}");
    assert_eq!(w.step, Step::Test);
    // the commit step is prefilled from the account
    assert!(press(&mut w, Key::Enter) == WizOut::None);
    assert_eq!(w.step, Step::Commits);
    assert_eq!((w.text("idname").as_str(), w.text("email").as_str()), ("Octo Cat", "octo@example.org"));
    assert!(w.make_default, "the first connection is the default");
    let out = press(&mut w, Key::Ctrl('s'));
    let WizOut::Done(c) = out else { panic!("{out:?} {:?}", w.error) };
    assert_eq!((c.name.as_str(), c.account.as_str(), c.token_vault.as_str(), c.identity_email.as_str(), c.default), ("Home Gitea", "octo", "git.conn.home-gitea.token", "octo@example.org", true));
    assert!(c.last_test.as_ref().is_some_and(|t| t.ok));
    assert_eq!(f.store.get("git.conn.home-gitea.token").unwrap().as_str(), TOK);
    assert_eq!(w.text("token"), "", "the token left the field");
    // not in any file, the audit or the settings
    let mut files = vec![f.dir.clone()];
    while let Some(d) = files.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            if e.path().is_dir() {
                files.push(e.path());
            } else {
                assert!(!std::fs::read_to_string(e.path()).unwrap_or_default().contains("FAKEFAKE"), "token in {}", e.path().display());
            }
        }
    }
    let log = f.log.lock().unwrap().join("\n");
    assert!(log.contains("git.connect.add Home Gitea git.example.org") && log.contains("git.connect.test Home Gitea git.example.org ok") && !log.contains("FAKEFAKE"), "{log}");
    // the request carried the token in the header field, as Gitea wants
    assert_eq!(f.http.requests()[0].token.as_deref(), Some(TOK));
    assert!(w.done && matches!(press(&mut w, Key::Enter), WizOut::None));
}

#[test]
fn kinds_set_their_own_address_and_the_digits_choose() {
    let f = fx("kinds");
    for (digit, kind, url) in [('1', Kind::GitHub, "https://github.com"), ('2', Kind::GitLab, "https://gitlab.com"), ('4', Kind::Other, "")] {
        let mut w = GitWizard::new(f.env());
        press(&mut w, Key::Char(digit));
        assert_eq!(w.kind_value(), kind);
        press(&mut w, Key::Enter);
        assert_eq!((w.step, w.text("url").as_str()), (Step::Where, url));
        assert!(w.cluster_hint().is_none());
    }
    let mut w = GitWizard::new(f.env());
    press(&mut w, Key::Up);
    press(&mut w, Key::Up);
    press(&mut w, Key::Up);
    assert_eq!(w.kind_value(), Kind::GitHub);
    press(&mut w, Key::Down);
    assert_eq!(w.kind_value(), Kind::GitLab);
    for _ in 0..9 {
        press(&mut w, Key::Down);
    }
    assert_eq!(w.kind_value(), Kind::Other, "the list stops at its end");
}

#[test]
fn back_goes_one_step_and_cancel_at_every_step_leaves_nothing() {
    for stop in 0..5 {
        let f = fx(&format!("cancel{stop}"));
        f.gitea_ok();
        let mut w = GitWizard::new(f.env());
        match stop {
            0 => {}
            1 => {
                press(&mut w, Key::Enter);
            }
            2 => to_signin(&mut w),
            3 => to_test(&mut w),
            _ => {
                to_test(&mut w);
                press(&mut w, Key::Enter);
                assert_eq!(w.step, Step::Commits);
            }
        }
        assert_eq!(w.step.n(), stop);
        if stop > 0 {
            // back, then forward again keeps what was typed
            assert!(w.can_back());
            let before = w.text("name");
            press(&mut w, Key::PageUp);
            assert_eq!(w.step.n(), stop - 1);
            assert_eq!(w.text("name"), before);
        }
        assert_eq!(press(&mut w, Key::Esc), WizOut::Cancel, "step {stop}");
        assert!(w.done);
        nothing_kept(&f);
    }
    // the buttons do the same: Cancel and Back are clickable
    let f = fx("buttons");
    let mut w = GitWizard::new(f.env());
    w.event(&Event::Tab(WIZ_BASE + 2));
    assert_eq!(w.step, Step::Where);
    w.event(&Event::Tab(WIZ_BASE + 1));
    assert_eq!(w.step, Step::Kind);
    assert!(!w.can_back());
    assert_eq!(w.event(&Event::Tab(WIZ_BASE)), WizOut::Cancel);
}

#[test]
fn a_key_the_wizard_made_goes_away_on_cancel_and_stays_when_saved() {
    let f = fx("ssh");
    let mut w = GitWizard::new(f.env());
    press(&mut w, Key::Enter);
    w.set_text("name", "Over SSH");
    w.set_text("url", "https://git.example.org");
    press(&mut w, Key::Ctrl('s'));
    assert_eq!(w.step, Step::SignIn);
    press(&mut w, Key::Right); // SSH key
    assert_eq!(w.method, Method::Ssh);
    press(&mut w, Key::Enter);
    assert_eq!(w.slot(), Some(Slot::Keys));
    // no key yet: "make a new key" is the only row, it needs a name
    w.set_text("keyname", "");
    press(&mut w, Key::Enter);
    press(&mut w, Key::Ctrl('s'));
    assert!(w.error.as_deref().unwrap_or("").contains("name"), "{:?}", w.error);
    w.set_text("keyname", "gitea");
    press(&mut w, Key::Ctrl('s'));
    assert_eq!(w.step, Step::Test, "{:?}", w.error);
    assert!(f.keys.key("gitea").is_some(), "the key exists while the wizard runs");
    w.wait();
    let r = w.report.clone().unwrap();
    assert!(r.ok && r.notes[0].contains("No token was given"), "no token: no API call, said plainly: {r:?}");
    assert!(f.http.requests().is_empty());
    assert_eq!(press(&mut w, Key::Esc), WizOut::Cancel);
    assert!(f.keys.key("gitea").is_none() && f.store.get("ssh.gitea").is_err(), "the key made for this wizard is deleted on cancel");
    // saved this time: the connection names the key and the key stays
    let mut w = GitWizard::new(f.env());
    press(&mut w, Key::Enter);
    w.set_text("name", "Over SSH");
    w.set_text("url", "https://git.example.org");
    press(&mut w, Key::Ctrl('s'));
    press(&mut w, Key::Right);
    press(&mut w, Key::Down);
    w.set_text("keyname", "gitea");
    press(&mut w, Key::Ctrl('s'));
    w.wait();
    press(&mut w, Key::Ctrl('s'));
    let WizOut::Done(c) = press(&mut w, Key::Ctrl('s')) else { panic!("{:?}", w.error) };
    assert_eq!((c.method, c.key.as_str(), c.token_vault.as_str()), (Method::Ssh, "gitea", ""));
    assert!(f.keys.key("gitea").is_some());
    // an SSH connection resolves its key for that host without a host rule
    let u = iyakki_agent::gitsite::parse("git@git.example.org:octo/app.git", Default::default()).unwrap();
    assert_eq!(f.keys.resolve(&u, &Default::default()), iyakki_agent::gitkeys::Auth::Key { id: "gitea".into(), from_workspace: false });
}

#[test]
fn test_results_are_plain_words_for_401_missing_write_scope_and_an_unreachable_server() {
    // 401: the next step is refused, "save anyway" is the way past
    let f = fx("e401");
    f.http.answer("/api/v1/user", 401, &[], r#"{"message":"bad token"}"#);
    let mut w = GitWizard::new(f.env());
    to_test(&mut w);
    assert!(w.report.as_ref().unwrap().error.as_deref().unwrap().contains("The token was refused (401)"));
    press(&mut w, Key::Enter);
    assert_eq!(w.step, Step::Test);
    assert!(w.error.as_deref().unwrap().contains("did not pass"));
    press(&mut w, Key::Char('s'));
    assert_eq!(w.step, Step::Commits, "saved anyway on request");
    let WizOut::Done(c) = press(&mut w, Key::Ctrl('s')) else { panic!() };
    assert!(c.last_test.as_ref().is_some_and(|t| !t.ok && t.text.contains("refused (401)")));
    // a test again after fixing: 'r'
    let f = fx("retest");
    f.http.answer("/api/v1/user", 401, &[], "{}");
    let mut w = GitWizard::new(f.env());
    to_test(&mut w);
    f.gitea_ok();
    // (the first rule still answers 401: a fresh fake for the second try)
    let http2 = Arc::new(FakeGitHttp::default());
    http2.answer("/api/v1/user/repos", 200, &[], "[]").answer("/api/v1/user", 200, &[], r#"{"login":"octo"}"#);
    w.env.http = http2;
    press(&mut w, Key::Char('r'));
    w.wait();
    assert!(w.report.as_ref().unwrap().ok);
    // GitHub token without the repo scope
    let f = fx("e403");
    f.http.answer("GET https://api.github.com/user", 200, &[("X-OAuth-Scopes", "read:user")], r#"{"login":"octo"}"#);
    let mut w = GitWizard::new(f.env());
    press(&mut w, Key::Char('1'));
    press(&mut w, Key::Enter);
    w.set_text("name", "GH");
    press(&mut w, Key::Ctrl('s'));
    press(&mut w, Key::Down);
    w.event(&Event::Paste("ghp_FAKEFAKEFAKEFAKEFAKE".into()));
    press(&mut w, Key::Ctrl('s'));
    w.wait();
    let e = w.report.as_ref().unwrap().error.clone().unwrap();
    assert!(e.contains("The token cannot write repositories (needs the repo scope)"), "{e}");
    assert!(!w.report.as_ref().unwrap().ok);
    // the server does not answer
    let f = fx("down");
    let mut w = GitWizard::new(f.env());
    to_test(&mut w);
    let e = w.report.as_ref().unwrap().error.clone().unwrap();
    assert!(e.starts_with("The server did not answer"), "{e}");
    assert!(!e.contains("FAKEFAKE"));
    // a policy that forbids the host: nothing is sent
    let f = fx("policy");
    let mut env = f.env();
    env.egress = Arc::new(|h| Err(format!("Your account may not reach {h} (the network policy of this account).")));
    let mut w = GitWizard::new(env);
    to_test(&mut w);
    assert!(w.report.as_ref().unwrap().error.as_deref().unwrap().contains("may not reach git.example.org"));
    assert!(f.http.requests().is_empty());
}

/// A server with a self-signed certificate: refuses until a certificate file is given.
struct SelfSigned(FakeGitHttp, Mutex<Vec<Option<PathBuf>>>);
impl GitHttp for SelfSigned {
    fn send(&self, r: &Req) -> Result<Resp, NetErr> {
        self.1.lock().unwrap().push(r.cert.clone());
        if r.cert.is_none() {
            return Err(NetErr::Tls("self-signed certificate".into()));
        }
        self.0.send(r)
    }
    fn peer_cert(&self, _: &str) -> Result<iyakki_agent::gitconn::CertInfo, String> {
        iyakki_agent::gitconn::cert_info("-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n")
    }
}

#[test]
fn a_self_signed_server_shows_its_fingerprint_and_is_trusted_only_on_request_for_this_connection() {
    let f = fx("tls");
    let inner = FakeGitHttp::default();
    inner.answer("/api/v1/user/repos", 200, &[], "[]").answer("/api/v1/user", 200, &[], r#"{"login":"octo","name":"O","email":"o@example.org"}"#);
    let http = Arc::new(SelfSigned(inner, Mutex::new(vec![])));
    let mut w = GitWizard::new(f.env_with(http.clone()));
    to_test(&mut w);
    let r = w.report.clone().unwrap();
    assert_eq!(r.tls, Tls::Untrusted);
    assert!(r.error.as_deref().unwrap().contains("certificate is not trusted"));
    let cert = w.cert.clone().expect("the fingerprint is shown for comparison");
    assert_eq!(cert.fingerprint, "SHA256:03:90:58:C6:F2:C0:CB:49:2C:53:3B:0A:4D:14:EF:77:CC:0F:78:AB:CC:CE:D5:28:7D:84:A1:A2:01:1C:FB:81");
    assert!(w.report.as_ref().is_some_and(|r| !r.ok));
    nothing_kept(&f);
    // nothing is trusted by looking: the first request had no certificate file, and verification was never switched off
    assert_eq!(http.1.lock().unwrap().as_slice(), &[None]);
    press(&mut w, Key::Char('t'));
    w.wait();
    assert!(w.report.as_ref().unwrap().ok, "{:?}", w.report);
    assert_eq!(w.report.as_ref().unwrap().tls, Tls::Pinned);
    assert!(http.1.lock().unwrap().last().unwrap().as_ref().is_some_and(|p| p.ends_with("pending-wizard.pem")));
    // cancelling before the end removes the staged certificate
    assert_eq!(press(&mut w, Key::Esc), WizOut::Cancel);
    nothing_kept(&f);
    // and saving keeps it for this connection only
    let mut w = GitWizard::new(f.env_with(http));
    to_test(&mut w);
    press(&mut w, Key::Char('t'));
    w.wait();
    press(&mut w, Key::Enter);
    let WizOut::Done(c) = press(&mut w, Key::Ctrl('s')) else { panic!("{:?}", w.error) };
    assert_eq!(c.pinned, cert.fingerprint);
    let conns = f.keys.conns();
    assert!(conns.pinned_cert(&c).is_some());
    assert!(std::fs::read_dir(conns.certs_dir()).unwrap().flatten().all(|e| e.file_name() == "home-gitea.pem"), "the staged file was renamed away");
    assert!(f.log.lock().unwrap().join("\n").contains("git.connect.trust Home Gitea git.example.org SHA256:03:90"));
}

fn install_gitea(f: &Fx, exposure: &str) {
    use iyakki_agent::platform::installed::{self, Installed};
    let mut options = iyakki_agent::platform::Options::default();
    options.set("exposure", exposure);
    let rec = Installed {
        id: "gitea-iyakki-gitea".into(),
        service: "gitea".into(),
        title: "Gitea".into(),
        kind: "git".into(),
        options,
        urls: vec![iyakki_agent::platform::UrlInfo { label: "Gitea".into(), url: "http://10.0.0.5:30300".into(), kind: "web".into() }],
        vault: vec!["svc.gitea-iyakki-gitea.token".into()],
        ..Default::default()
    };
    installed::save(&f.dir.join("svc"), "alice", &[rec]).unwrap();
}

#[test]
fn the_gitea_on_my_cluster_is_listed_and_uses_the_platform_token_without_a_copy() {
    let f = fx("cluster");
    // none installed: the hint, and no way forward
    let mut w = GitWizard::new(f.env());
    press(&mut w, Key::Enter);
    assert!(w.cluster.is_empty());
    w.mode = 0;
    assert!(w.cluster_hint().unwrap().contains("No Gitea is installed"));
    press(&mut w, Key::Ctrl('s'));
    assert_eq!(w.step, Step::Where);
    assert!(w.error.as_deref().unwrap().contains("No Gitea is installed on your cluster yet"));
    assert_eq!(w.event(&Event::Tab(WIZ_BASE + 7)), WizOut::OpenPlatform, "the Open Platform services link");
    nothing_kept(&f);
    // one installed
    install_gitea(&f, "NodePort");
    f.store.set("svc.gitea-iyakki-gitea.token", "tok_CLUSTERFAKEFAKEFAKE1").unwrap();
    f.http.answer("GET http://10.0.0.5:30300/api/v1/user/repos", 200, &[], "[]").answer("GET http://10.0.0.5:30300/api/v1/user", 200, &[], r#"{"login":"admin","name":"","email":"a@example.org"}"#);
    let mut w = GitWizard::new(f.env());
    press(&mut w, Key::Enter);
    assert_eq!((w.mode, w.cluster.len(), w.text("name").as_str()), (0, 1, "My cluster Gitea"), "the cluster's Gitea is offered first when there is one");
    assert!(w.cluster_hint().is_none());
    press(&mut w, Key::Ctrl('s'));
    assert_eq!(w.step, Step::Test, "no sign-in step: the platform services hold the token");
    w.wait();
    let r = w.report.clone().unwrap();
    assert!(r.ok && r.placement == gitconn::Placement::Cluster, "{r:?}");
    assert_eq!(f.http.requests()[0].token.as_deref(), Some("tok_CLUSTERFAKEFAKEFAKE1"));
    press(&mut w, Key::Enter);
    let WizOut::Done(c) = press(&mut w, Key::Ctrl('s')) else { panic!("{:?}", w.error) };
    assert_eq!((c.cluster.as_str(), c.token_vault.as_str(), c.url.as_str()), ("gitea-iyakki-gitea", "svc.gitea-iyakki-gitea.token", "http://10.0.0.5:30300"));
    assert!(!f.store.names().unwrap().iter().any(|n| n.starts_with("git.conn.")), "the token was not copied");
    // a cluster-internal Gitea says why it cannot be used
    let f2 = fx("cluster-ip");
    install_gitea(&f2, "ClusterIP");
    let mut w = GitWizard::new(f2.env());
    press(&mut w, Key::Enter);
    press(&mut w, Key::Ctrl('s'));
    assert!(w.error.as_deref().unwrap().contains("only reachable inside the cluster"));
}

#[test]
fn the_default_and_the_commit_identity_are_chosen_in_the_last_step_and_a_second_connection_is_not_the_default() {
    let f = fx("ident");
    f.gitea_ok();
    let mut first = GitWizard::new(f.env());
    to_test(&mut first);
    press(&mut first, Key::Enter);
    let WizOut::Done(a) = press(&mut first, Key::Ctrl('s')) else { panic!() };
    assert!(a.default);
    let mut w = GitWizard::new(f.env());
    press(&mut w, Key::Enter);
    w.set_text("name", "Second");
    press(&mut w, Key::Ctrl('s'));
    assert_eq!(w.step, Step::Where, "the address is still empty: nothing moves on");
    w.set_text("name", "Home Gitea");
    press(&mut w, Key::Ctrl('s'));
    assert!(w.error.as_deref().unwrap().contains("already a connection called"));
    w.set_text("name", "Second");
    w.set_text("url", "https://git.example.org");
    press(&mut w, Key::Ctrl('s'));
    press(&mut w, Key::Down);
    w.event(&Event::Paste(TOK.into()));
    press(&mut w, Key::Ctrl('s'));
    w.wait();
    press(&mut w, Key::Ctrl('s'));
    assert_eq!(w.step, Step::Commits);
    assert!(!w.make_default, "not the default unless asked");
    w.focus = 0;
    w.set_text("idname", "Me Myself");
    w.set_text("email", "me@example.org");
    w.focus = 2;
    press(&mut w, Key::Char('y'));
    assert!(w.make_default);
    let WizOut::Done(b) = press(&mut w, Key::Ctrl('s')) else { panic!("{:?}", w.error) };
    let conns = f.keys.conns();
    assert_eq!(conns.default_connection().unwrap().id, b.id);
    assert_eq!((conns.get(&b.id).unwrap().identity_name.as_str(), conns.get(&b.id).unwrap().identity_email.as_str()), ("Me Myself", "me@example.org"));
    assert!(!conns.get(&a.id).unwrap().default);
    assert_eq!(conns.list().len(), 2);
}

#[test]
fn editing_keeps_the_stored_token_unless_a_new_one_is_pasted_and_cannot_go_back_past_where() {
    let f = fx("edit");
    f.gitea_ok();
    let mut w = GitWizard::new(f.env());
    to_test(&mut w);
    press(&mut w, Key::Enter);
    let WizOut::Done(c) = press(&mut w, Key::Ctrl('s')) else { panic!() };
    // edit: starts at Where with the values, no Back to Kind
    let mut e = GitWizard::edit(f.env(), (*c).clone());
    assert_eq!((e.step, e.text("name").as_str(), e.text("url").as_str(), e.can_back()), (Step::Where, "Home Gitea", "https://git.example.org", false));
    press(&mut e, Key::PageUp);
    assert_eq!(e.step, Step::Where);
    e.set_text("name", "Renamed");
    press(&mut e, Key::Ctrl('s'));
    assert_eq!(e.step, Step::SignIn);
    press(&mut e, Key::Ctrl('s')); // the token field is empty: the stored token is kept and tested
    assert_eq!(e.step, Step::Test, "{:?}", e.error);
    e.wait();
    assert!(e.report.as_ref().unwrap().ok);
    assert_eq!(f.http.requests().last().unwrap().token.as_deref(), Some(TOK));
    press(&mut e, Key::Enter);
    let WizOut::Done(d) = press(&mut e, Key::Ctrl('s')) else { panic!("{:?}", e.error) };
    assert_eq!((d.id.as_str(), d.name.as_str(), d.default), (c.id.as_str(), "Renamed", true));
    assert_eq!(f.store.get("git.conn.home-gitea.token").unwrap().as_str(), TOK);
    assert_eq!(f.keys.conns().list().len(), 1);
    // a new token replaces it
    let mut e = GitWizard::edit(f.env(), (*d).clone());
    press(&mut e, Key::Ctrl('s'));
    press(&mut e, Key::Down);
    e.event(&Event::Paste("tok_REPLACEDREPLACED12".into()));
    press(&mut e, Key::Ctrl('s'));
    e.wait();
    press(&mut e, Key::Enter);
    assert!(matches!(press(&mut e, Key::Ctrl('s')), WizOut::Done(_)));
    assert_eq!(f.store.get("git.conn.home-gitea.token").unwrap().as_str(), "tok_REPLACEDREPLACED12");
    // Cancel while editing changes nothing
    let mut e = GitWizard::edit(f.env(), f.keys.conns().list()[0].clone());
    e.set_text("name", "Never saved");
    assert_eq!(press(&mut e, Key::Esc), WizOut::Cancel);
    assert_eq!(f.keys.conns().list()[0].name, "Renamed");
}

#[test]
fn the_wizard_is_drawn_at_four_sizes_on_every_step_and_the_buttons_are_there() {
    let f = fx("draw");
    f.gitea_ok();
    for (wd, ht) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (640, 360)] {
        let s = crate::shell::scale_for(wd, ht);
        let draw = |w: &mut GitWizard| {
            let mut g = Gfx::new(wd, ht);
            g.clear(rgb(0, 0, 0));
            w.render(&mut g, R::new(0.0, 0.0, wd as f32, ht as f32), s, rgb(110, 160, 255), true);
            g
        };
        let mut w = GitWizard::new(f.env());
        let g = draw(&mut w);
        assert!(g.hits.iter().any(|h| h.label == "Next") && g.hits.iter().any(|h| h.label == "Cancel"));
        press(&mut w, Key::Enter);
        draw(&mut w);
        w.set_text("url", "https://git.example.org");
        press(&mut w, Key::Ctrl('s'));
        let g = draw(&mut w);
        assert!(g.hits.iter().any(|h| h.label == "Back"));
        w.method = Method::Ssh;
        draw(&mut w);
        w.method = Method::Token;
        press(&mut w, Key::Down);
        w.event(&Event::Paste(TOK.into()));
        press(&mut w, Key::Ctrl('s'));
        w.wait();
        draw(&mut w);
        press(&mut w, Key::Ctrl('s'));
        let g = draw(&mut w);
        assert!(g.hits.iter().any(|h| h.label == "Save"), "{:?}", w.step);
    }
}
