//! Settings -> Encrypted home (F-STORE-05, docs/development/DATA-VOLUME-ENCRYPTION.md): the person's private folder as an encrypted container
//! that opens at sign-in with the sign-in password. This page shows the state in plain words, makes the home (with the recovery key shown
//! ONCE and typed back before it counts as saved), opens and locks it, makes a new recovery key, allows or removes the optional administrator
//! slot, moves an existing plain home (opt-in, with a plan first), and removes the old plain copy once the person agrees.
//!
//! What happens on each press is decided by `iyakki_agent::enchome::form` (plain logic, tested without a screen); this file only draws the
//! state and turns that logic's steps into masked-field forms. Passwords and the recovery key go from the field straight into that logic and
//! never into an audit line, a message, the assistant or a log. The assistant's tools (`encrypted_home_*`) only leave a one-word request
//! that opens the same form here (`ench_tick`).

use super::*;
use iyakki_agent::enchome::form::{self as ef, Ctx, Flow, Purpose, Request};
use iyakki_agent::enchome::{migrate, EncHome, SlotInfo, State};

#[derive(Default)]
pub(super) struct EncPanel {
    flow: ef::Form,
    /// What the last drawing showed (state, has an administrator slot, an old plain copy waits): the buttons follow it.
    shown: Option<(State, bool, bool)>,
}

const EXPLAIN_PROTECTED: &str = "Protected: the files in your private folder, your browser data and keys, if this computer, stick or drive is lost or stolen while you are signed out or it is switched off. Nobody can read them without your password or recovery key.";
const EXPLAIN_NOT: &str = "Not protected: a computer that is signed in and unlocked (the keys are in memory then); the boot partition; system files, logs, service data and other people's folders, which stay readable until whole-drive encryption exists; and anything you copy out of the folder. A weak password weakens all of it.";
const EXPLAIN_RECOVERY: &str = "The recovery key is shown once. Without the password AND the recovery key the files cannot be opened by anyone, ever: there is no backdoor and no copy. Keep the key away from this computer.";

impl SettingsApp {
    fn ench_status(cx: &AppCx) -> iyakki_agent::enchome::Status {
        EncHome::for_status(cx.services.dir()).status(&cx.session.user)
    }

    fn ench_run<R>(&mut self, cx: &AppCx, f: impl FnOnce(&mut ef::Form, &Ctx) -> R) -> R {
        let env = EncHome::real(cx.services.dir());
        let user = cx.session.user.clone();
        let verify = |p: &str| cx.store.lock().map(|mut s| s.authenticate(&user, p).is_ok()).unwrap_or(false);
        let ctx = Ctx { env: &env, user: &user, verify_signin: &verify };
        f(&mut self.ench.flow, &ctx)
    }

    /// The form for one step of a purpose; `error` is shown on it.
    fn ench_ask_form(purpose: Purpose, stage: u8, note: Option<&str>, error: Option<&str>) -> Form {
        let mut f = match (purpose, stage) {
            (Purpose::Create, _) => Form::new("Make my home encrypted").with_note(&format!("{EXPLAIN_PROTECTED} {EXPLAIN_RECOVERY} Making the folder takes a few seconds.")).secret("Your sign-in password", "it also opens the folder from now on"),
            (Purpose::Unlock, _) => Form::new("Open my encrypted home").secret("Password", "your sign-in password"),
            (Purpose::UnlockWithRecovery, 0) => Form::new("Open with the recovery key").with_note("Type the recovery key you wrote down when the folder was made (spaces and lower case are fine).").secret("Recovery key", "14 groups of 4 letters and digits"),
            (Purpose::UnlockWithRecovery, _) => Form::new("Open with the recovery key").with_note("The key is right. Now type the password you sign in with today: the folder will open with it from now on.").secret("Your current sign-in password", ""),
            (Purpose::ChangeRecovery, _) => Form::new("New recovery key").with_note("The old recovery key stops working. A new one is shown once; you type it back to confirm.").secret("Your sign-in password", ""),
            (Purpose::AdminSlot { enable: false }, _) => Form::new("Remove the administrator slot").with_note("Afterwards only your password and your recovery key open the folder.").secret("Your sign-in password", ""),
            (Purpose::AdminSlot { enable: true }, 0) => Form::new("Allow an administrator slot").with_note("Whoever knows the administrator's password can open and READ this folder and reset your sign-in password without losing it. You can remove the slot here at any time.").secret("Your sign-in password", ""),
            (Purpose::AdminSlot { enable: true }, _) => Form::new("Allow an administrator slot").with_note("Choose the password the administrator will use for this folder (it is not their sign-in password).").secret("Administrator's password for this folder", "").secret("Repeat it", ""),
            (Purpose::Migrate, _) => Form::new("Move my existing files").with_note("The next screen shows exactly what will be copied. Nothing is changed before you press Move.").secret("Your sign-in password", ""),
        };
        if let Some(n) = note {
            f.note = format!("{}{}{n}", f.note, if f.note.is_empty() { "" } else { " " });
        }
        f.error = error.map(|e| e.to_string());
        f
    }

    fn ench_recovery_form(key: &str, note: Option<&str>, error: Option<&str>) -> Form {
        let mut f = Form::new("Your recovery key").with_note(&format!("Write this key down now, on paper, away from this computer. It is shown only this once.   {key}   Then type it below to prove it is written correctly. {EXPLAIN_RECOVERY}")).text("Type the recovery key here", "", "");
        if let Some(n) = note {
            f.note = format!("{n} {}", f.note);
        }
        f.error = error.map(|e| e.to_string());
        f
    }

    /// The modal for where the logic is now, or `None` when it is finished (the result goes to the status line).
    fn ench_next(&mut self) -> Option<Modal> {
        let fl = &self.ench.flow;
        let note = fl.notice.as_deref().filter(|_| !fl.failed);
        let err = fl.notice.as_deref().filter(|_| fl.failed);
        match &fl.flow {
            Flow::Ask { purpose, stage, .. } => Some(Modal::Form(FormKind::EncHome, Self::ench_ask_form(*purpose, *stage, note, err))),
            Flow::MigratePlan { plan, .. } => Some(Modal::Confirm(ConfirmKind::EncMove, format!("{}  Press y to move, Esc to cancel.", plan.describe()))),
            Flow::ShowRecovery { text, .. } => Some(Modal::Form(FormKind::EncHome, Self::ench_recovery_form(text, note, err))),
            Flow::Idle | Flow::Done => {
                if let Some(n) = fl.notice.clone() {
                    if fl.failed { self.fail(n) } else { self.ok(n) }
                }
                None
            }
        }
    }

    pub(super) fn ench_start(&mut self, cx: &AppCx, p: Purpose) {
        self.ench_run(cx, |f, c| f.start(p, c));
        let user = cx.session.user.clone();
        cx.audit.record(&user, "enchome.form", &format!("{p:?}"));
        self.modal = self.ench_next();
    }

    /// A form step was submitted. Returns the form to keep open when the step failed.
    pub(super) fn ench_submit(&mut self, cx: &AppCx, form: &mut Form) -> Option<Modal> {
        match &self.ench.flow.flow {
            Flow::Ask { purpose: Purpose::AdminSlot { enable: true }, stage: 1, .. } if form.raw(0) != form.raw(1) || form.raw(0).is_empty() => {
                form.error = Some("The two passwords are not the same (or empty).".into());
                return Some(Modal::Form(FormKind::EncHome, form.clone()));
            }
            Flow::Ask { .. } | Flow::ShowRecovery { .. } => {}
            _ => return None,
        }
        let typed = form.raw(0);
        let was_migrating = matches!(self.ench.flow.flow, Flow::MigratePlan { .. });
        self.ench.flow.type_text(&typed);
        self.ench_run(cx, |f, c| f.submit(c));
        let user = cx.session.user.clone();
        if !self.ench.flow.failed {
            let what = match &self.ench.flow.flow {
                Flow::ShowRecovery { .. } => "recovery key shown",
                Flow::Done => "done",
                _ => "step",
            };
            cx.audit.record(&user, "enchome.step", what);
        } else {
            cx.audit.record(&user, "enchome.step.failed", "");
        }
        let _ = was_migrating;
        self.ench_next()
    }

    /// `y` on the migration plan, or on removing the old plain copy.
    pub(super) fn ench_confirm(&mut self, cx: &AppCx, kind: ConfirmKind) {
        let user = cx.session.user.clone();
        match kind {
            ConfirmKind::EncMove => {
                self.ench_run(cx, |f, c| f.submit(c));
                cx.audit.record(&user, "enchome.move", if self.ench.flow.failed { "failed" } else { "done" });
                self.modal = self.ench_next();
            }
            ConfirmKind::EncRemovePlain => {
                let env = EncHome::real(cx.services.dir());
                match migrate::remove_plain_copy(&env, &user) {
                    Ok(m) => {
                        cx.audit.record(&user, "enchome.plain.removed", "");
                        self.ok(m)
                    }
                    Err(e) => self.fail(e.to_string()),
                }
            }
            _ => {}
        }
    }

    /// Every heartbeat: a request the assistant left (one word) opens the matching form here.
    pub(super) fn ench_tick(&mut self, cx: &AppCx) {
        if self.modal.is_some() || self.ticks % 10 != 0 {
            return;
        }
        let Some(r) = ef::take_request(cx.services.dir(), &cx.session.user) else { return };
        if let Some(i) = self.tabs.iter().position(|t| *t == Tab::EncHome) {
            self.tab = i;
            self.show_list = false;
        }
        let p = match r {
            Request::Create => Purpose::Create,
            Request::Unlock => Purpose::Unlock,
            Request::ChangeRecovery => Purpose::ChangeRecovery,
            Request::AdminSlotOn => Purpose::AdminSlot { enable: true },
            Request::AdminSlotOff => Purpose::AdminSlot { enable: false },
            Request::Migrate => Purpose::Migrate,
        };
        self.ench_start(cx, p);
    }

    pub(super) fn handle_ench(&mut self, k: &Key, cx: &AppCx) {
        let Key::Char(c) = k else { return };
        let st = Self::ench_status(cx);
        match (*c, st.state) {
            ('c', State::NotSetUp) => self.ench_start(cx, Purpose::Create),
            ('m', State::NotSetUp) => self.ench_start(cx, Purpose::Migrate),
            ('u', State::Locked) => self.ench_start(cx, Purpose::Unlock),
            ('r', State::Locked) => self.ench_start(cx, Purpose::UnlockWithRecovery),
            ('l', State::Open) => {
                let env = EncHome::real(cx.services.dir());
                let user = cx.session.user.clone();
                match env.lock(&user) {
                    Ok(()) => {
                        cx.audit.record(&user, "enchome.lock", "");
                        self.ok("The encrypted home is locked. It opens again at your next sign-in, or with Open.")
                    }
                    Err(e) => self.fail(e.to_string()),
                }
            }
            ('k', State::Locked | State::Open) => self.ench_start(cx, Purpose::ChangeRecovery),
            ('a', State::Locked | State::Open) => self.ench_start(cx, Purpose::AdminSlot { enable: !st.has_admin_slot() }),
            ('s', State::Locked | State::Open) => {
                let env = EncHome::for_status(cx.services.dir());
                let on = !st.meta.lock_with_screen;
                match env.set_lock_with_screen(&cx.session.user, on) {
                    Ok(()) => {
                        cx.audit.record(&cx.session.user, "enchome.lock_with_screen", if on { "on" } else { "off" });
                        self.ok(if on { "The folder now closes when the screen locks; the lock screen password opens it again." } else { "The folder stays open when the screen locks (it closes at sign-out)." })
                    }
                    Err(e) => self.fail(e.to_string()),
                }
            }
            ('x', State::Open) if st.plain_copy.is_some() => {
                self.modal = Some(Modal::Confirm(ConfirmKind::EncRemovePlain, format!("Remove the old plain copy of your folder? {} Press y to remove it.", migrate::FLASH_NOTE)));
            }
            _ => {}
        }
    }

    pub(super) fn ench_buttons(&self) -> Vec<Btn> {
        let (state, admin, plain) = self.ench.shown.unwrap_or((State::NotSetUp, false, false));
        match state {
            State::NotSetUp => vec![("Make my home encrypted", ACCENT, 'c'), ("Move my existing files", TEXT, 'm')],
            State::Locked => vec![("Open", ACCENT, 'u'), ("Open with recovery key", TEXT, 'r'), ("New recovery key", TEXT, 'k'), (if admin { "Remove administrator slot" } else { "Allow administrator slot" }, TEXT, 'a'), ("Close with screen lock", TEXT, 's')],
            State::Open => {
                let mut v = vec![("Lock now", ACCENT, 'l'), ("New recovery key", TEXT, 'k'), (if admin { "Remove administrator slot" } else { "Allow administrator slot" }, TEXT, 'a'), ("Close with screen lock", TEXT, 's')];
                if plain {
                    v.push(("Remove old plain copy", ERR, 'x'));
                }
                v
            }
            State::Damaged => vec![],
        }
    }

    pub(super) fn render_ench(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let st = Self::ench_status(cx);
        self.ench.shown = Some((st.state, st.has_admin_slot(), st.plain_copy.is_some()));
        let open = EncHome::for_status(cx.services.dir()).is_open(&cx.session.user);
        let mut lines: Vec<(String, [u8; 4], bool)> = vec![];
        let (head, col) = match st.state {
            State::NotSetUp => ("Not encrypted".to_string(), TEXT),
            State::Locked => ("Encrypted and locked".to_string(), TEXT),
            State::Open => ("Encrypted and open".to_string(), OK),
            State::Damaged => ("Damaged: cannot be opened".to_string(), ERR),
        };
        lines.push((head, col, true));
        lines.push((iyakki_agent::enchometools::describe_status(&st, open), TEXT, false));
        if let Some(b) = iyakki_agent::enchome::take_banner(&cx.session.user) {
            lines.push((b, ERR, false));
        }
        if let Some(sz) = st.size_bytes {
            lines.push((format!("Container: {} (a file on the data drive, up to {} of files; the file is sparse, so it only takes the space you use).", st.container.display(), iyakki_agent::enchome::human(sz)), MUTED, false));
        }
        if st.state == State::NotSetUp {
            lines.push(("Make my home encrypted starts an empty encrypted folder (it needs your folder to be empty). Move my existing files copies what you have, checks every file against the original, and only then switches; your old folder is kept until you agree to remove it. Nothing is ever moved by itself.".into(), MUTED, false));
        }
        lines.push((EXPLAIN_PROTECTED.into(), MUTED, false));
        lines.push((EXPLAIN_NOT.into(), MUTED, false));
        lines.push((EXPLAIN_RECOVERY.into(), MUTED, false));
        lines.push(("Your sign-in password is also the folder's password. If an administrator resets your sign-in password, the folder stays sealed until you use the recovery key (or an administrator slot you allowed). The administrator is warned before they reset.".into(), MUTED, false));
        let mut y = a.y;
        let limit = a.y + a.h;
        for (text, col, bold) in lines {
            let (size, face) = if bold { (17.0 * s, Face::Bold) } else { (13.5 * s, Face::Sans) };
            for l in g.wrap(size, &text, face, a.w) {
                if y + size * 1.3 > limit {
                    return;
                }
                g.text(a.x, y, size, col, &l, face);
                y += size * 1.45;
            }
            y += 6.0 * s;
        }
    }
}
