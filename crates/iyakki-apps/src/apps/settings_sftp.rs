//! Settings -> Remote access -> SFTP file server (F-FILE-06): the switches, the accounts, the keys, the host key fingerprint with a copy
//! button, and the approval card. The work is `iyakki_agent::sftp` (the same code the assistant's `sftp_set` calls); this file is the page.
//!
//! * Administrators only. Stopping is immediate; starting and allowing account passwords show a card that says what becomes reachable
//!   (port, accounts, how they log in, until when, local network only) and need a `y`.
//! * Public keys are pasted as one `ssh-ed25519 ...` line (a private key or a password is refused by the form, as it is for the tool).
//! * Every change is audited as `sftp.*` with the person's name (no key text, no password).


use super::*;
use crate::apps::settings::network_port::NetworkSettingsPort;
use iyakki_shared::sftp::{EXPIRIES};
use iyakki_shared::sftp;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SftpAct {
    Start,
    PasswordOn,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SftpForm {
    Accounts,
    AddKey,
    RemoveKey,
}

#[cfg(any(test, feature = "testing"))]
#[allow(unused_imports)]
pub(crate) use crate::apps::settings::network_port::TEST_SFTP;

fn expiry_words(secs: u64) -> String {
    if secs == 0 { "until you stop it".into() } else { format!("for {}", EXPIRIES.iter().find(|(_, s)| *s == secs).map_or(format!("{secs} s"), |(l, _)| l.to_string())) }
}

/// What starting the server makes reachable, in plain words (the card).
pub(super) fn start_card(c: &sftp::Config) -> String {
    format!(
        "Start the SFTP file server? It opens port {} to the local network only. Accounts allowed: {}. They log in with {}. Each account sees only what its own file rules allow. It stops by itself {}. Press y to start, n to cancel.",
        c.port,
        c.users.join(", "),
        if c.password_login { "a key or the account password" } else { "a key" },
        expiry_words(c.expiry_secs)
    )
}

pub(super) fn password_card(c: &sftp::Config) -> String {
    format!(
        "Let the accounts ({}) log in to SFTP with their account password as well as keys? Anyone on the local network could then try to guess a password (four wrong tries end a connection, and too many from one address are blocked for a while). Keys are safer. Press y to allow, n to cancel.",
        c.users.join(", ")
    )
}

impl SettingsApp {
    /// The buttons of the SFTP part, added to the Remote page's row.
    pub(super) fn sftp_buttons(&self) -> Vec<Btn> {
        vec![
            ("Start or stop SFTP", ACCENT, 'F'),
            ("SFTP accounts", TEXT, 'U'),
            ("SFTP password login", TEXT, 'W'),
            ("Add SFTP key", TEXT, 'K'),
            ("Remove SFTP key", TEXT, 'R'),
            ("SFTP time limit", TEXT, 'T'),
            ("Copy host key fingerprint", TEXT, 'C'),
        ]
    }

    /// The lines of the SFTP part of the page (state, accounts with their key fingerprints, host key).
    pub(crate) fn sftp_lines(&self, p: &dyn NetworkSettingsPort, lines: &mut Vec<(String, f32, [u8; 4], Face)>) {
        let st = p.sftp_server().status();
        lines.push(("SFTP file server".into(), 14.0, TEXT, Face::Bold));
        for (i, l) in p.sftp_status_lines().into_iter().take(2).enumerate() {
            lines.push((l, 13.0, if i == 0 { TEXT } else { MUTED }, Face::Sans));
        }
        lines.push((format!("Stops by itself {}. Port {}.", expiry_words(st.config.expiry_secs), st.config.port), 13.0, MUTED, Face::Sans));
        if st.running {
            for a in &st.addresses {
                lines.push((format!("sftp -P {} NAME@{a}", st.port), 13.0, ACCENT, Face::Mono));
            }
        }
        for (user, fps) in &st.keys {
            let t = if fps.is_empty() { format!("{user}: no key yet (K adds one)") } else { format!("{user}: {} key{}", fps.len(), if fps.len() == 1 { "" } else { "s" }) };
            lines.push((t, 13.0, MUTED, Face::Sans));
            for fp in fps {
                lines.push((fp.clone(), 12.0, DIM, Face::Mono));
            }
        }
        if let Some(p) = Self::sftp_problem(&st) {
            lines.push((format!("Not ready: {p}."), 13.0, rgb(255, 196, 94), Face::Sans));
        }
        lines.push(("Host key fingerprint: compare it with what your SFTP program shows the first time. C copies it.".into(), 12.0, DIM, Face::Sans));
        lines.push((if st.host_fingerprint.is_empty() { "(made when the server first starts)".into() } else { format!("{} (Ed25519)", st.host_fingerprint) }, 13.0, TEXT, Face::Mono));
        if !st.host_fingerprint_rsa.is_empty() {
            lines.push((format!("{} (RSA, only for very old clients)", st.host_fingerprint_rsa), 13.0, TEXT, Face::Mono));
        }
        if !st.host_fingerprint_ec.is_empty() {
            lines.push((format!("{} (ECDSA P-256, for clients that ask for it)", st.host_fingerprint_ec), 13.0, TEXT, Face::Mono));
        }
        lines.push(("Each account sees only what its own file rules allow. Starting asks first; it stops by itself after the time limit.".into(), 12.0, DIM, Face::Sans));
    }

    fn sftp_problem(st: &sftp::Status) -> Option<String> {
        if st.running {
            return None;
        }
        if st.config.users.is_empty() {
            return Some("no account is allowed yet (U)".into());
        }
        if !st.config.password_login && st.keys.iter().all(|(_, k)| k.is_empty()) {
            return Some("nobody could log in: add a key (K) or allow password login (W)".into());
        }
        None
    }

    /// Keys of the SFTP part; true when the key was one of ours.
    pub(super) fn sftp_key(&mut self, k: &Key, p: &dyn NetworkSettingsPort) -> bool {
        let Key::Char(c) = k else { return false };
        if !matches!(c, 'F' | 'U' | 'W' | 'K' | 'R' | 'T' | 'C') {
            return false;
        }
        if !self.admin(p) {
            return true;
        }
        let sftp = p.sftp_server();
        let cfg = sftp.config();
        match c {
            'F' => {
                if sftp.status().running {
                    sftp.disable(p.acting_user());
                    self.ok("The SFTP server is stopped and its firewall opening is closed.");
                } else if let Some(p) = sftp.problem() {
                    self.fail(format!("Not started: {p}."));
                } else {
                    self.modal = Some(Modal::Confirm(ConfirmKind::Sftp(SftpAct::Start), start_card(&cfg)));
                }
            }
            'W' => {
                if cfg.password_login {
                    match sftp.configure(|c| {
                        c.password_login = false;
                        Ok(())
                    }) {
                        Ok(_) => {
                            p.audit("sftp.password_login", "off");
                            self.ok("SFTP logins with an account password are off: keys only.");
                        }
                        Err(e) => self.fail(e),
                    }
                } else if cfg.users.is_empty() {
                    self.fail("Allow an account first (U).");
                } else {
                    self.modal = Some(Modal::Confirm(ConfirmKind::Sftp(SftpAct::PasswordOn), password_card(&cfg)));
                }
            }
            'U' => self.open_form(FormKind::Sftp(SftpForm::Accounts), Form::new("Accounts that may log in to SFTP").text("Accounts", &cfg.users.join(", "), "names separated by commas; empty: nobody")),
            'K' => self.open_form(
                FormKind::Sftp(SftpForm::AddKey),
                Form::new("Add a public key for SFTP").text("Account", cfg.users.first().map_or(p.acting_user(), |u| u), "an account name").text("Public key", "", "one line: ssh-ed25519 AAAA... comment. Never a private key."),
            ),
            'R' => self.open_form(FormKind::Sftp(SftpForm::RemoveKey), Form::new("Remove an SFTP key").text("Account", cfg.users.first().map_or(p.acting_user(), |u| u), "an account name").text("Fingerprint", "", "SHA256:... as listed on this page")),
            'T' => {
                let i = EXPIRIES.iter().position(|(_, s)| *s == cfg.expiry_secs).map_or(0, |i| (i + 1) % EXPIRIES.len());
                let secs = EXPIRIES[i].1;
                match sftp.configure(|c| {
                    c.expiry_secs = secs;
                    Ok(())
                }) {
                    Ok(_) => {
                        p.audit("sftp.expiry", &secs.to_string());
                        self.ok(format!("The SFTP server will stop by itself {} (counted from the next start).", expiry_words(secs)));
                    }
                    Err(e) => self.fail(e),
                }
            }
            _ => {
                let fp = sftp.status().host_fingerprint;
                if fp.is_empty() {
                    self.fail("The host key is made when the server first starts: start it once, then copy.");
                } else {
                    crate::ui::clipboard::copy(&fp);
                    self.ok("The host key fingerprint is copied.");
                }
            }
        }
        true
    }

    pub(in crate::apps) fn sftp_confirm(&mut self, p: &dyn NetworkSettingsPort, act: SftpAct) {
        if !self.admin(p) {
            return;
        }
        let sftp = p.sftp_server();
        match act {
            SftpAct::Start => match sftp.enable(p.acting_user()) {
                Ok(st) => self.ok(format!("The SFTP server is on at port {}{}.", st.port, if st.until == 0 { String::new() } else { " and stops by itself".into() })),
                Err(e) => self.fail(e),
            },
            SftpAct::PasswordOn => match sftp.configure(|c| {
                c.password_login = true;
                Ok(())
            }) {
                Ok(_) => {
                    p.audit("sftp.password_login", "on");
                    self.ok("SFTP logins with an account password are allowed, besides keys.");
                }
                Err(e) => self.fail(e),
            },
        }
    }

    pub(in crate::apps) fn sftp_submit(&mut self, p: &dyn NetworkSettingsPort, k: &SftpForm, form: &mut Form) -> Result<String, String> {
        if !self.admin(p) {
            return Err("Only administrators can change the SFTP server.".into());
        }
        let sftp = p.sftp_server();
        let plain = |s: &str| !(s.contains("PRIVATE KEY") || s.contains("-----BEGIN") || s.len() > 700);
        match k {
            SftpForm::Accounts => {
                let users: Vec<String> = form.value(0).split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                let c = sftp.configure(|c| {
                    c.users = users;
                    Ok(())
                })?;
                p.audit("sftp.users", &c.users.join(","));
                Ok(if c.users.is_empty() { "Nobody may log in to SFTP now.".into() } else { format!("SFTP accounts allowed: {}.", c.users.join(", ")) })
            }
            SftpForm::AddKey => {
                let (user, line) = (form.value(0).trim().to_string(), form.value(1));
                if !plain(&line) {
                    return Err("That is not a public key line. Never paste a private key.".into());
                }
                if !iyakki_sec::policy::valid_username(&user) {
                    return Err("The account must be a name: lower-case letters, digits, - and _.".into());
                }
                let fp = sftp.add_key(&user, &line)?;
                p.audit("sftp.key_add", &format!("{user} {fp}"));
                Ok(format!("Key {fp} added for {user}."))
            }
            SftpForm::RemoveKey => {
                let (user, fp) = (form.value(0).trim().to_string(), form.value(1));
                if !iyakki_sec::policy::valid_username(&user) {
                    return Err("The account must be a name: lower-case letters, digits, - and _.".into());
                }
                sftp.remove_key(&user, &fp)?;
                p.audit("sftp.key_remove", &format!("{user} {}", fp.trim()));
                Ok(format!("Key removed for {user}."))
            }
        }
    }
}
