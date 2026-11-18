//! The on-screen forms of the encrypted home: the part of Settings -> Encrypted home that decides what happens when a button is pressed.
//! The shell only draws it. It is here, as plain logic, so every path is tested without a screen.
//!
//! Rules that hold in every state: the sign-in password and the recovery key are typed into masked fields and go from there straight to
//! the functions in this crate; they are never stored in a request file, a message to the assistant, an audit line or a log. The chat
//! tools (`encrypted_home_*`) only leave a [`Request`] (one word), which makes the shell open the matching form.

use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use crate::migrate::{self, Plan, Step};
use crate::{EncHome, HomeError, Secret, State};

/// What the assistant can ask the screen to open. One word in a file; no secret ever.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Create,
    Unlock,
    ChangeRecovery,
    AdminSlotOn,
    AdminSlotOff,
    Migrate,
}

impl Request {
    pub fn word(self) -> &'static str {
        match self {
            Request::Create => "create",
            Request::Unlock => "unlock",
            Request::ChangeRecovery => "recovery",
            Request::AdminSlotOn => "admin-on",
            Request::AdminSlotOff => "admin-off",
            Request::Migrate => "migrate",
        }
    }
    pub fn from_word(w: &str) -> Option<Request> {
        [Request::Create, Request::Unlock, Request::ChangeRecovery, Request::AdminSlotOn, Request::AdminSlotOff, Request::Migrate].into_iter().find(|r| r.word() == w.trim())
    }
}

fn who(user: &str) -> String {
    user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect()
}

fn request_path(dir: &Path, user: &str) -> PathBuf {
    dir.join(format!("enchome-request-{}", who(user)))
}

/// Leave a request for the shell of `user`.
pub fn leave_request(dir: &Path, user: &str, r: Request) {
    let _ = std::fs::write(request_path(dir, user), r.word().as_bytes());
}

/// The shell takes its request (once).
pub fn take_request(dir: &Path, user: &str) -> Option<Request> {
    let p = request_path(dir, user);
    let t = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    Request::from_word(&t)
}

/// What a move is doing now, in plain words, once `step` is done (shown as progress while it runs).
pub fn step_words(step: Step) -> &'static str {
    match step {
        Step::Planned => "Making the encrypted container (step 1 of 7)",
        Step::ContainerMade => "Opening the new folder (step 2 of 7)",
        Step::StagingMounted => "Copying your files (step 3 of 7)",
        Step::Copied => "Checking every file against the original (step 4 of 7)",
        Step::Verified => "Closing the new folder (step 5 of 7)",
        Step::StagingUnmounted => "Putting the old folder aside (step 6 of 7)",
        Step::PlainRenamed => "Switching over (step 7 of 7)",
        Step::Switched => "Finishing",
    }
}

/// What the form needs from the rest of the system.
pub struct Ctx<'a> {
    pub env: &'a EncHome,
    pub user: &'a str,
    /// Whether this is the person's sign-in password (the account code's own check, with its own throttle).
    pub verify_signin: &'a dyn Fn(&str) -> bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Create,
    Unlock,
    /// Open with the recovery key because the password does not work; then set the sign-in password as the home's password.
    UnlockWithRecovery,
    ChangeRecovery,
    AdminSlot { enable: bool },
    Migrate,
}

/// Where the form is.
pub enum Flow {
    Idle,
    /// A masked field (or two). `stage` 0 asks the first, 1 the second (the new password after a recovery key, the administrator's password).
    Ask { purpose: Purpose, stage: u8, first: Zeroizing<String>, second: Zeroizing<String> },
    /// The migration plan is on screen; pressing the button starts it.
    MigratePlan { password: Zeroizing<String>, plan: Plan },
    /// The recovery key is on screen, once, with a field to type it back. It counts as saved only when the typed text matches.
    ShowRecovery { text: Zeroizing<String>, typed: Zeroizing<String> },
    Done,
}

pub struct Form {
    pub flow: Flow,
    /// A plain sentence for the screen: the last error or the result.
    pub notice: Option<String>,
    /// The last `submit` ended in an error (the notice says which).
    pub failed: bool,
}

impl Default for Form {
    fn default() -> Form {
        Form { flow: Flow::Idle, notice: None, failed: false }
    }
}

impl Form {
    pub fn is_idle(&self) -> bool {
        matches!(self.flow, Flow::Idle | Flow::Done)
    }

    /// Open the form for something the person pressed (or the assistant requested).
    pub fn start(&mut self, p: Purpose, cx: &Ctx) {
        self.notice = None;
        self.failed = false;
        let st = cx.env.status(cx.user);
        let refuse = match (p, st.state) {
            (Purpose::Create | Purpose::Migrate, State::Locked | State::Open | State::Damaged) => Some("This account already has an encrypted home."),
            (Purpose::Unlock | Purpose::UnlockWithRecovery | Purpose::ChangeRecovery | Purpose::AdminSlot { .. }, State::NotSetUp) => Some("There is no encrypted home yet; make one first."),
            (Purpose::ChangeRecovery | Purpose::AdminSlot { .. }, State::Damaged) => Some("The encrypted home's header is damaged."),
            (Purpose::Unlock | Purpose::UnlockWithRecovery, State::Open) => Some("The encrypted home is already open."),
            _ => None,
        };
        if let Some(r) = refuse {
            self.flow = Flow::Idle;
            self.failed = true;
            self.notice = Some(r.into());
            return;
        }
        self.flow = Flow::Ask { purpose: p, stage: 0, first: Zeroizing::new(String::new()), second: Zeroizing::new(String::new()) };
    }

    pub fn cancel(&mut self) {
        self.flow = Flow::Idle;
    }

    /// Type into the field that is asked for now.
    pub fn type_text(&mut self, s: &str) {
        match &mut self.flow {
            Flow::Ask { stage, first, second, .. } => {
                if *stage == 0 { first.push_str(s) } else { second.push_str(s) }
            }
            Flow::ShowRecovery { typed, .. } => typed.push_str(s),
            _ => {}
        }
    }

    /// Type a field the caller owns: the caller's copy is wiped here once it is in the form.
    pub fn type_owned(&mut self, s: String) {
        let s = Zeroizing::new(s);
        self.type_text(&s);
    }

    pub fn backspace(&mut self) {
        match &mut self.flow {
            Flow::Ask { stage, first, second, .. } => {
                let f = if *stage == 0 { first } else { second };
                f.pop();
            }
            Flow::ShowRecovery { typed, .. } => {
                typed.pop();
            }
            _ => {}
        }
    }

    fn fail(&mut self, e: HomeError) {
        self.failed = true;
        self.notice = Some(e.to_string());
    }

    /// Whether the next `submit` does slow work (Argon2id, the container, the copy) and so belongs on a background job. Only typing the
    /// recovery key back is quick.
    pub fn needs_background(&self) -> bool {
        matches!(self.flow, Flow::Ask { .. } | Flow::MigratePlan { .. })
    }

    /// Whether the next `submit` is the move itself (the long one, which reports its steps).
    pub fn is_move(&self) -> bool {
        matches!(self.flow, Flow::MigratePlan { .. })
    }

    /// Enter / the main button.
    pub fn submit(&mut self, cx: &Ctx) {
        self.submit_with(cx, &mut |_s: Step| true);
    }

    /// Like `submit`, and `step` hears each step of a move as it is done (return false to stop). Runs on any thread: nothing here draws.
    pub fn submit_with(&mut self, cx: &Ctx, step: &mut dyn FnMut(Step) -> bool) {
        self.failed = false;
        let flow = std::mem::replace(&mut self.flow, Flow::Idle);
        match flow {
            Flow::Idle | Flow::Done => {}
            Flow::Ask { purpose, stage, first, second } => self.submit_ask(cx, purpose, stage, first, second),
            Flow::MigratePlan { password, plan } => {
                let r = migrate::run(cx.env, cx.user, &password, step);
                match r {
                    Ok(rep) => {
                        self.notice = Some(format!("{} files ({}) were copied, checked one by one and switched over. Your old folder is kept as {}; remove it from this page once you have saved the recovery key and are happy with the new one.", rep.files, crate::human(rep.bytes), rep.plain_copy.display()));
                        self.flow = Flow::ShowRecovery { text: rep.recovery.0, typed: Zeroizing::new(String::new()) };
                    }
                    Err(e) => {
                        self.fail(e);
                        self.flow = Flow::MigratePlan { password, plan };
                    }
                }
            }
            Flow::ShowRecovery { text, typed } => match cx.env.confirm_recovery(cx.user, &typed) {
                Ok(()) => {
                    self.notice = Some("The recovery key is saved. Keep it somewhere safe and away from this computer: with it a forgotten password does not lose your files; without it they are gone.".into());
                    self.flow = Flow::Done;
                }
                Err(e) => {
                    self.fail(e);
                    self.flow = Flow::ShowRecovery { text, typed: Zeroizing::new(String::new()) };
                }
            },
        }
    }

    fn back(&mut self, purpose: Purpose, stage: u8, first: Zeroizing<String>, second: Zeroizing<String>, e: HomeError) {
        self.fail(e);
        self.flow = Flow::Ask { purpose, stage, first, second };
    }

    fn submit_ask(&mut self, cx: &Ctx, purpose: Purpose, stage: u8, first: Zeroizing<String>, second: Zeroizing<String>) {
        let wrong_signin = || HomeError::NotConfirmed("That is not your sign-in password.".into());
        match (purpose, stage) {
            (Purpose::Create, _) => {
                if !(cx.verify_signin)(&first) {
                    return self.back(purpose, 0, Zeroizing::new(String::new()), second, wrong_signin());
                }
                match cx.env.create(cx.user, &first, None) {
                    Ok(rk) => {
                        self.notice = Some("Your encrypted home is made and open. Write down the recovery key below, then type it back to confirm.".into());
                        self.flow = Flow::ShowRecovery { text: rk.0, typed: Zeroizing::new(String::new()) };
                    }
                    Err(e) => {
                        self.fail(e);
                    }
                }
            }
            (Purpose::Unlock, _) => match cx.env.unlock(cx.user, &Secret::password(&first)) {
                Ok(_) => {
                    self.notice = Some("The encrypted home is open.".into());
                    self.flow = Flow::Done;
                }
                Err(e) => self.back(purpose, 0, Zeroizing::new(String::new()), second, e),
            },
            (Purpose::UnlockWithRecovery, 0) => {
                // the key is checked for typing mistakes before anything is tried
                match iyakki_volcrypt::recovery::RecoveryKey::parse(&first) {
                    Ok(_) => self.flow = Flow::Ask { purpose, stage: 1, first, second },
                    Err(e) => self.back(purpose, 0, first, second, HomeError::NotConfirmed(format!("The recovery key is not written correctly: {e}."))),
                }
            }
            (Purpose::UnlockWithRecovery, _) => {
                if !(cx.verify_signin)(&second) {
                    return self.back(purpose, 1, first, Zeroizing::new(String::new()), HomeError::NotConfirmed("That is not your sign-in password. Type the password you sign in with now; the home will use it from here on.".into()));
                }
                match cx.env.reset_password_with(cx.user, &Secret::recovery(&first), &second).and_then(|_| cx.env.unlock(cx.user, &Secret::password(&second))) {
                    Ok(_) => {
                        self.notice = Some("The encrypted home is open and now opens with your current sign-in password.".into());
                        self.flow = Flow::Done;
                    }
                    Err(e) => self.back(purpose, 0, Zeroizing::new(String::new()), Zeroizing::new(String::new()), e),
                }
            }
            (Purpose::ChangeRecovery, _) => {
                if !(cx.verify_signin)(&first) {
                    return self.back(purpose, 0, Zeroizing::new(String::new()), second, wrong_signin());
                }
                match cx.env.change_recovery(cx.user, &first) {
                    Ok(rk) => {
                        self.notice = Some("A new recovery key was made; the old one no longer works. Write it down and type it back.".into());
                        self.flow = Flow::ShowRecovery { text: rk.0, typed: Zeroizing::new(String::new()) };
                    }
                    Err(e) => self.back(purpose, 0, Zeroizing::new(String::new()), second, e),
                }
            }
            (Purpose::AdminSlot { enable: false }, _) => {
                if !(cx.verify_signin)(&first) {
                    return self.back(purpose, 0, Zeroizing::new(String::new()), second, wrong_signin());
                }
                match cx.env.set_admin_slot(cx.user, &first, None) {
                    Ok(()) => {
                        self.notice = Some("Administrators can no longer open your home. Only your password and your recovery key do.".into());
                        self.flow = Flow::Done;
                    }
                    Err(e) => self.back(purpose, 0, Zeroizing::new(String::new()), second, e),
                }
            }
            (Purpose::AdminSlot { enable: true }, 0) => {
                if !(cx.verify_signin)(&first) {
                    return self.back(purpose, 0, Zeroizing::new(String::new()), second, wrong_signin());
                }
                self.flow = Flow::Ask { purpose, stage: 1, first, second };
            }
            (Purpose::AdminSlot { enable: true }, _) => match cx.env.set_admin_slot(cx.user, &first, Some(&second)) {
                Ok(()) => {
                    self.notice = Some("An administrator slot was added: whoever knows that password can open and read your home, and reset your password without losing it. You can remove it here at any time.".into());
                    self.flow = Flow::Done;
                }
                Err(e) => self.back(purpose, 1, first, Zeroizing::new(String::new()), e),
            },
            (Purpose::Migrate, _) => {
                if !(cx.verify_signin)(&first) {
                    return self.back(purpose, 0, Zeroizing::new(String::new()), second, wrong_signin());
                }
                match migrate::plan(cx.env, cx.user) {
                    Ok(plan) => {
                        self.notice = Some(plan.describe());
                        if !plan.ok() {
                            self.failed = true;
                        }
                        self.flow = if plan.ok() { Flow::MigratePlan { password: first, plan } } else { Flow::Idle };
                    }
                    Err(e) => self.fail(e),
                }
            }
        }
    }
}

/// The form is handed to a background thread and back; the secrets in it are zeroized wherever it ends.
#[allow(dead_code)]
fn _form_is_send() {
    fn needs_send<T: Send>() {}
    needs_send::<Form>();
}
