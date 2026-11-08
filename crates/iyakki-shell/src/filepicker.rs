//! The file picker lives in `iyakki-uikit` (`filepicker.rs`: the browser, the path chip, drops). What stays here is the part that knows
//! the account: [`scope_for`] turns the signed-in account's policy and file roots into the picker's `Scope`, and [`set_session`] tells
//! the pickers who is signed in. Everything else is re-exported under the old path (`crate::filepicker::...`).

use std::path::{Path, PathBuf};
use std::rc::Rc;

use iyakki_agent::scope::FileRoots;
use iyakki_sec::policy::Policy;

pub use iyakki_uikit::filepicker::*;

struct AccountRules {
    user: String,
    policy: Policy,
    roots: FileRoots,
}

impl ScopeRules for AccountRules {
    fn home(&self) -> &Path {
        &self.roots.home
    }

    fn can_read(&self, real: &Path) -> bool {
        self.policy.allows_read(&self.user, &real.to_string_lossy()) || self.roots.mount_of(real).is_some()
    }

    fn can_write(&self, real: &Path) -> bool {
        self.roots.write_ok(&self.policy, &self.user, real)
    }

    fn expand(&self, text: &str) -> PathBuf {
        let t = self.roots.expand(text);
        let p = PathBuf::from(&t);
        if p.is_absolute() { p } else { self.roots.home.join(p) }
    }

    fn explain_io(&self, e: &std::io::Error, path: &Path, action: &str) -> String {
        iyakki_agent::scope::explain_io(e, path, action)
    }
}

/// What this account may read and write.
pub fn scope_for(user: &str, policy: &Policy, roots: FileRoots) -> Scope {
    Scope::new(Rc::new(AccountRules { user: user.into(), policy: policy.clone(), roots }))
}

/// The shell tells the pickers who is signed in (and clears it at sign-out).
pub fn set_session(who: Option<(&str, &Policy, &Path)>) {
    set_session_scope(who.map(|(u, p, d)| {
        let (user, policy, data_dir) = (u.to_string(), p.clone(), d.to_path_buf());
        Rc::new(move || scope_for(&user, &policy, FileRoots::new(&policy, &user, &data_dir))) as Rc<dyn Fn() -> Scope>
    }));
}
