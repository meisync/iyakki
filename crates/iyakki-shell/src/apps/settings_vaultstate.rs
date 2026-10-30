//! Settings -> Vault: the ONE place in Settings that says whether a vault is locked. Nobody is signed in without typing the password
//! (there is no automatic sign-in), so a vault is locked only when it could not be opened with that password; this page says so in
//! plain words, names the passwords that wait for it (`iyakki_agent::vaultpending`: where to read them, never a value) and how to open it.
//! Other pages and messages that need a vault only point here ("Settings → Vault").

use std::path::Path;

/// The status sentence of a vault (the text after "Status: ") and whether it is the good case.
pub(super) fn status_words(system: bool, exists: bool, unlocked: bool) -> (String, bool) {
    match (system, exists, unlocked) {
        (true, false, _) => ("not created yet (created when an admin first signs in)".into(), false),
        (true, true, true) => ("unlocked".into(), true),
        (true, true, false) => ("LOCKED: sign in as an admin with access".into(), false),
        (false, _, true) => ("unlocked with your sign-in password".into(), true),
        (false, _, false) => ("LOCKED: sign in with your password, or press u to unlock it here (or R to start a fresh one)".into(), false),
    }
}

/// One sentence naming what waits for this person's locked vault, or none: it is shown only while the vault is locked.
pub(super) fn waiting_line(data: &Path, user: &str, unlocked: bool) -> Option<String> {
    if unlocked {
        return None;
    }
    let what: Vec<String> = iyakki_agent::vaultpending::list(data).into_iter().filter(|p| p.user == user).map(|p| format!("{} password", p.what)).collect();
    if what.is_empty() {
        return None;
    }
    Some(format!("Waiting for this vault: {}. They are kept safe in the cluster and move in on their own once you unlock it.", what.join(", ")))
}
