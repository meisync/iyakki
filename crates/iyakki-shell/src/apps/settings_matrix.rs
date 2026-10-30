//! Settings -> Notifications -> "Where to reach me": the Matrix account. A form (address of the homeserver, how to sign in, the room), a
//! test message, and removal. Sign-in and the test run in the background. The password is used once to get an access token and is not
//! kept; the token goes into the person's vault (`msg.acct.<id>.token`). Nothing here shows a token. The rules and the work are in
//! `iyakki_agent::msg`; see docs/development/MESSAGING-CONNECTORS-PLAN.md ("Matrix: what is built").

use std::sync::mpsc::Receiver;
use std::sync::Arc;

use iyakki_agent::gitkeys::AuditFn;
use iyakki_agent::msg::{Auth, MessageService};

use super::*;

/// What the page keeps between frames: one background job at a time, as (ok, plain sentence).
#[derive(Default)]
pub(super) struct MatrixPanel {
    pub(super) pending: Option<Receiver<(bool, String)>>,
}

impl SettingsApp {
    fn matrix_service(cx: &AppCx) -> MessageService {
        let audit = cx.audit.clone();
        let user = cx.session.user.clone();
        let log: AuditFn = Arc::new(move |a, d| audit.record(&user, a, d));
        MessageService::for_services(&cx.services, &cx.session.user, log)
    }

    /// One line for the page: where the person can be reached now.
    pub(super) fn matrix_reach_line(cx: &AppCx) -> String {
        let ms = Self::matrix_service(cx);
        let list = ms.accounts().list();
        match list.first() {
            None => "Where to reach me: nothing set up. Press a to connect a Matrix account (your own server or any homeserver).".into(),
            Some(a) => {
                let rooms = ms.accounts().rooms(&a.id);
                let room = rooms.first().map_or("no room yet".to_string(), |r| format!("room \"{}\"", r.name));
                format!("Where to reach me: Matrix \"{}\" ({}), {room}. t sends a test message, x removes it. Flows send through the Notify step (Also tell me by: matrix).", a.label, a.user_id)
            }
        }
    }

    pub(super) fn matrix_form(prefill_server: &str) -> Form {
        Form::new("Matrix account (rooms without encryption only)")
            .text("Name for this account", "Home", "for example Home")
            .text("Homeserver address", prefill_server, "https://matrix.example.org, or cluster://iyakki-matrix/matrix:8008 for the server from Store")
            .choice("Sign in with", &["user name and password", "an access token"], 0)
            .text("User name (not needed with a token)", "", "the part before the colon, for example alice")
            .secret("Password, or the access token", "the password is used once and not kept; the token goes into your vault")
            .text("Room id (empty: the only room you are in)", "", "!abcdef:server.org (Element: room settings, Advanced). The room must not be encrypted")
    }

    pub(super) fn matrix_open_form(&mut self, prefill_server: &str) {
        let f = Self::matrix_form(prefill_server);
        self.open_form(FormKind::Matrix, f);
    }

    /// The form was filled in: check the address and the network policy, then sign in in the background.
    pub(super) fn matrix_submit(&mut self, cx: &AppCx, form: &mut Form) -> Result<String, String> {
        if self.matrix.pending.is_some() {
            return Err("A Matrix job is still running.".into());
        }
        let (label, server) = (form.value(0), form.value(1));
        let hs = iyakki_agent::msg::matrix::check_homeserver(&server)?;
        if !hs.starts_with("cluster") && !cx.session.policy.allows_host(&iyakki_agent::gitconn::host_of(&hs)) {
            return Err(format!("Your account may not reach {} (the network policy of this account).", iyakki_agent::gitconn::host_of(&hs)));
        }
        let secret = form.raw(4);
        if secret.trim().is_empty() {
            return Err("Enter the password, or paste the access token.".into());
        }
        let auth = if form.choice_idx(2) == 0 {
            if form.value(3).is_empty() {
                return Err("Enter the user name.".into());
            }
            Auth::password(&form.value(3), secret)
        } else {
            Auth::token(secret)
        };
        let room = form.value(5);
        let ms = Self::matrix_service(cx);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = ms.add_matrix(&label, &hs, auth, &room);
            let _ = tx.send(match r {
                Ok(a) => {
                    let rooms = ms.accounts().rooms(&a.id);
                    let tail = if rooms.is_empty() { " No room is set yet: add the room id (a again) before flows can message you.".to_string() } else { String::new() };
                    (true, format!("Signed in as {} and kept the access token in your vault.{tail} Press t to send a test message.", a.user_id))
                }
                Err(e) => (false, e),
            });
        });
        self.matrix.pending = Some(rx);
        Ok("Signing in to the Matrix server…".into())
    }

    /// The heartbeat: collects the answer of the background job.
    pub(super) fn matrix_tick(&mut self) {
        let Some(rx) = &self.matrix.pending else { return };
        let Ok((ok, text)) = rx.try_recv() else { return };
        self.matrix.pending = None;
        if ok {
            self.ok(text);
        } else {
            self.fail(text);
        }
    }

    pub(super) fn matrix_test_send(&mut self, cx: &AppCx) {
        if self.matrix.pending.is_some() {
            self.fail("A Matrix job is still running.");
            return;
        }
        let ms = Self::matrix_service(cx);
        let Some(a) = ms.accounts().list().into_iter().next() else {
            self.fail("Connect a Matrix account first (press a).");
            return;
        };
        if !a.homeserver.starts_with("cluster") && !cx.session.policy.allows_host(&iyakki_agent::gitconn::host_of(&a.homeserver)) {
            self.fail(format!("Your account may not reach {} (the network policy of this account).", iyakki_agent::gitconn::host_of(&a.homeserver)));
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(match ms.test_send(&a.id, "") {
                Ok(r) => (true, format!("Test message {} ({}).", r.note, a.label)),
                Err(e) => (false, e),
            });
        });
        self.matrix.pending = Some(rx);
        self.ok("Sending a test message…");
    }

    pub(super) fn matrix_remove(&mut self, cx: &AppCx) {
        let ms = Self::matrix_service(cx);
        match ms.accounts().list().into_iter().next() {
            None => self.fail("There is no Matrix account to remove."),
            Some(a) => match ms.remove(&a.id) {
                Ok(()) => self.ok(format!("Removed the Matrix account {} and its token from your vault.", a.label)),
                Err(e) => self.fail(e),
            },
        }
    }
}
