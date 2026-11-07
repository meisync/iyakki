//! Accounts, authentication, role administration and the audit log.
//!
//! * Passwords are hashed with Argon2id (PHC string format), never stored or logged in clear.
//! * The store is a JSON file written atomically with mode 0600.
//! * Administration needs the `manage_users` permission, and the last admin can never be removed
//!   or demoted (no lock-out).
//! * Repeated failed logins are throttled per account.
//! * Security-relevant events go to a hash-chained audit log (edits/truncation are detectable).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iyakki_sec::policy::{valid_username, Policy, PolicyOverride, Role};
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AccountError {
    #[error("invalid username (lowercase letters, digits, - and _; must start with a letter; max 32)")]
    BadUsername,
    #[error("password must be at least 8 characters")]
    WeakPassword,
    #[error("user already exists")]
    Exists,
    #[error("no such user")]
    NoSuchUser,
    #[error("invalid username or password")]
    InvalidCredentials,
    #[error("too many attempts; try again in {0} s")]
    Throttled(u64),
    #[error("not permitted")]
    Forbidden,
    #[error("cannot remove or demote the last administrator")]
    LastAdmin,
    #[error("storage error: {0}")]
    Storage(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserRecord {
    pub name: String,
    pub hash: String,
    pub role: Role,
    #[serde(default)]
    pub overrides: PolicyOverride,
    /// Fields a newer release wrote for this account: kept, and written back on the next save.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl UserRecord {
    pub fn policy(&self) -> Policy {
        Policy::resolve(self.role, &self.overrides)
    }
}

/// An authenticated login.
#[derive(Clone, Debug)]
pub struct Session {
    pub user: String,
    pub role: Role,
    pub policy: Policy,
}

#[derive(Serialize, Deserialize, Default)]
struct StoreFile {
    users: Vec<UserRecord>,
    /// Fields of the file itself that a newer release wrote: kept, and written back.
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

pub struct UserStore {
    path: PathBuf,
    users: Vec<UserRecord>,
    extra: serde_json::Map<String, serde_json::Value>,
    failures: HashMap<String, (u32, Instant)>,
    dummy_hash: String,
    /// What the file looked like when it was last read or written (when it changed and how big it was), to notice
    /// accounts added from outside (`iyakki user add` on the machine) while this program keeps running.
    stamp: Option<(std::time::SystemTime, u64)>,
}

fn file_stamp(p: &Path) -> Option<(std::time::SystemTime, u64)> {
    std::fs::metadata(p).ok().and_then(|m| Some((m.modified().ok()?, m.len())))
}

fn hash_password(pw: &str) -> Result<String, AccountError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default().hash_password(pw.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| AccountError::Storage(e.to_string()))
}

fn verify(pw: &str, phc: &str) -> bool {
    PasswordHash::new(phc).is_ok_and(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
}

impl UserStore {
    pub fn open(path: &Path) -> Result<UserStore, AccountError> {
        let (users, extra) = match std::fs::read_to_string(path) {
            Ok(t) => match serde_json::from_str::<StoreFile>(&t) {
                Ok(f) => (f.users, f.extra),
                Err(e) => {
                    // The file stays where it is (moving it away would make the machine look new and offer the first-run setup to
                    // anybody): sign-in is blocked, and a copy is kept for repair, with a line for the audit log.
                    let copy = iyakki_schema::copy_aside(path, iyakki_schema::now_secs());
                    iyakki_schema::report(format!("accounts {}: damaged ({e}); the file is left as it is, sign-in is blocked until it is repaired{}", path.display(), copy.map(|c| format!(" (a copy is kept as {})", c.display())).unwrap_or_default()));
                    return Err(AccountError::Storage(format!("corrupt user file: {e}")));
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Vec::new(), Default::default()),
            Err(e) => return Err(AccountError::Storage(e.to_string())),
        };
        Ok(UserStore { path: path.into(), users, extra, failures: HashMap::new(), dummy_hash: hash_password("not-a-real-password")?, stamp: file_stamp(path) })
    }

    /// Read the accounts again if the file was changed by something else since this program last looked.
    pub fn refresh(&mut self) {
        let now = file_stamp(&self.path);
        if now == self.stamp {
            return;
        }
        self.stamp = now;
        match std::fs::read_to_string(&self.path) {
            Ok(t) => {
                if let Ok(f) = serde_json::from_str::<StoreFile>(&t) {
                    self.users = f.users;
                    self.extra = f.extra;
                } else {
                    // damaged while running: the accounts already read stay in use, the file is kept as it is, and a copy is kept
                    let copy = iyakki_schema::copy_aside(&self.path, iyakki_schema::now_secs());
                    iyakki_schema::report(format!("accounts {}: damaged while running; the accounts already read stay in use{}", self.path.display(), copy.map(|c| format!(" (a copy is kept as {})", c.display())).unwrap_or_default()));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => self.users.clear(),
            Err(_) => {}
        }
    }

    fn save(&mut self) -> Result<(), AccountError> {
        use std::os::unix::fs::OpenOptionsExt;
        let io = |e: std::io::Error| AccountError::Storage(e.to_string());
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d).map_err(io)?;
        }
        let tmp = self.path.with_extension("tmp");
        let json = serde_json::to_string_pretty(&StoreFile { users: self.users.clone(), extra: self.extra.clone() }).map_err(|e| AccountError::Storage(e.to_string()))?;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(io)?;
        f.write_all(json.as_bytes()).and_then(|_| f.sync_all()).map_err(io)?;
        std::fs::rename(&tmp, &self.path).map_err(io)?;
        self.stamp = file_stamp(&self.path);
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.users.is_empty()
    }

    pub fn list(&self) -> &[UserRecord] {
        &self.users
    }

    pub fn get(&self, name: &str) -> Option<&UserRecord> {
        self.users.iter().find(|u| u.name == name)
    }

    fn admin_count(&self) -> usize {
        self.users.iter().filter(|u| u.role == Role::Admin).count()
    }

    fn add(&mut self, name: &str, pw: &str, role: Role) -> Result<(), AccountError> {
        self.refresh(); // another window may have changed the accounts: never save over their change
        if !valid_username(name) {
            return Err(AccountError::BadUsername);
        }
        if pw.chars().count() < 8 {
            return Err(AccountError::WeakPassword);
        }
        if self.get(name).is_some() {
            return Err(AccountError::Exists);
        }
        self.users.push(UserRecord { name: name.into(), hash: hash_password(pw)?, role, overrides: Default::default(), extra: Default::default() });
        self.save()
    }

    /// Add an account on behalf of whoever administers this machine from its command line (`iyakki user add`), where there
    /// is no signed-in person to ask: reaching the command line already means owning the machine and its data folder.
    pub fn add_local(&mut self, name: &str, pw: &str, role: Role) -> Result<(), AccountError> {
        self.add(name, pw, role)
    }

    /// First-run setup: creates the initial administrator. Only works while no users exist.
    pub fn create_first_admin(&mut self, name: &str, pw: &str) -> Result<(), AccountError> {
        if !self.is_empty() {
            return Err(AccountError::Forbidden);
        }
        self.add(name, pw, Role::Admin)
    }

    fn throttle_left(&self, name: &str) -> u64 {
        match self.failures.get(name) {
            Some((n, last)) if *n >= 3 => {
                let wait = Duration::from_secs((1u64 << (*n - 3).min(6)).min(60));
                wait.saturating_sub(last.elapsed()).as_secs_f64().ceil() as u64
            }
            _ => 0,
        }
    }

    pub fn authenticate(&mut self, name: &str, pw: &str) -> Result<Session, AccountError> {
        self.refresh();
        let left = self.throttle_left(name);
        if left > 0 {
            return Err(AccountError::Throttled(left));
        }
        // Always run one Argon2 verification, so unknown names cost the same as wrong passwords.
        let (hash, found) = match self.get(name) {
            Some(u) => (u.hash.clone(), true),
            None => (self.dummy_hash.clone(), false),
        };
        if verify(pw, &hash) && found {
            self.failures.remove(name);
            let u = self.get(name).expect("found");
            return Ok(Session { user: u.name.clone(), role: u.role, policy: u.policy() });
        }
        let e = self.failures.entry(name.to_string()).or_insert((0, Instant::now()));
        e.0 += 1;
        e.1 = Instant::now();
        Err(AccountError::InvalidCredentials)
    }

    fn require_admin(actor: &Session) -> Result<(), AccountError> {
        if actor.policy.manage_users { Ok(()) } else { Err(AccountError::Forbidden) }
    }

    pub fn create_user(&mut self, actor: &Session, name: &str, pw: &str, role: Role) -> Result<(), AccountError> {
        Self::require_admin(actor)?;
        self.add(name, pw, role)
    }

    pub fn delete_user(&mut self, actor: &Session, name: &str) -> Result<(), AccountError> {
        Self::require_admin(actor)?;
        self.refresh(); // another window may have changed the accounts: never save over their change
        let u = self.get(name).ok_or(AccountError::NoSuchUser)?;
        if u.role == Role::Admin && self.admin_count() <= 1 {
            return Err(AccountError::LastAdmin);
        }
        self.users.retain(|u| u.name != name);
        self.save()
    }

    pub fn set_role(&mut self, actor: &Session, name: &str, role: Role) -> Result<(), AccountError> {
        Self::require_admin(actor)?;
        self.refresh(); // another window may have changed the accounts: never save over their change
        let admins = self.admin_count();
        let u = self.users.iter_mut().find(|u| u.name == name).ok_or(AccountError::NoSuchUser)?;
        if u.role == Role::Admin && role != Role::Admin && admins <= 1 {
            return Err(AccountError::LastAdmin);
        }
        u.role = role;
        self.save()
    }

    pub fn set_overrides(&mut self, actor: &Session, name: &str, ov: PolicyOverride) -> Result<(), AccountError> {
        Self::require_admin(actor)?;
        self.refresh(); // another window may have changed the accounts: never save over their change
        self.users.iter_mut().find(|u| u.name == name).ok_or(AccountError::NoSuchUser)?.overrides = ov;
        self.save()
    }

    /// Changing your OWN password always requires the current one (even for admins, so a left-open
    /// session cannot be used to take over the account). Admins can reset *other* people's passwords
    /// without it.
    pub fn set_password(&mut self, actor: &Session, name: &str, old: Option<&str>, new: &str) -> Result<(), AccountError> {
        if new.chars().count() < 8 {
            return Err(AccountError::WeakPassword);
        }
        let own = actor.user == name;
        if own || !actor.policy.manage_users {
            let ok = own && old.zip(self.get(name)).is_some_and(|(o, u)| verify(o, &u.hash));
            if !ok {
                return Err(AccountError::Forbidden);
            }
        }
        self.refresh();
        let h = hash_password(new)?;
        self.users.iter_mut().find(|u| u.name == name).ok_or(AccountError::NoSuchUser)?.hash = h;
        self.save()
    }
}

// ---------------------------------------------------------------- audit log

pub struct AuditLog {
    path: PathBuf,
}

/// One writer at a time: reading the last hash and appending must be atomic, or two events in the same second share a `prev`.
static AUDIT_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn user_of_system() -> &'static str {
    "system"
}

#[derive(Serialize, Deserialize)]
struct AuditLine {
    ts: u64,
    user: String,
    event: String,
    detail: String,
    prev: String,
    hash: String,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn line_hash(l: &AuditLine) -> String {
    hex(&iyakki_ledger::hash_bytes(format!("{}|{}|{}|{}|{}", l.ts, l.user, l.event, l.detail, l.prev).as_bytes()))
}

impl AuditLog {
    pub fn new(path: &Path) -> AuditLog {
        AuditLog { path: path.into() }
    }

    fn read(&self) -> Vec<AuditLine> {
        std::fs::read_to_string(&self.path).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    /// Best effort: auditing must never take the system down.
    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn record(&self, user: &str, event: &str, detail: &str) {
        // One writer at a time: reading the last hash and appending must be atomic,
        // or two events in the same second share a `prev` and the chain reads as broken.
        let _guard = AUDIT_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // what the data stores had to say while they loaded (an upgrade, a file moved aside) goes into the chain first
        for note in iyakki_schema::drain_notes() {
            self.append(user_of_system(), "data.store", &note);
        }
        self.append(user, event, detail);
    }

    /// Write the sentences the data stores left while they loaded (see `iyakki_schema::report`), if any. The shell calls it at start.
    pub fn flush_notes(&self) {
        let _g = AUDIT_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for note in iyakki_schema::drain_notes() {
            self.append(user_of_system(), "data.store", &note);
        }
    }

    /// Append one chained line. The whole of "read the last hash, write the line, flush it to disk" happens under an exclusive
    /// advisory lock (`flock`) on the log file itself, so two processes (the shell, the core, the command line) cannot both take the
    /// same `prev`. The last line is read again under the lock, never remembered from before. The line format is unchanged.
    fn append(&self, user: &str, event: &str, detail: &str) {
        use std::io::{Read, Seek, SeekFrom};
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::io::AsRawFd;
        if let Some(d) = self.path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let Ok(mut f) = std::fs::OpenOptions::new().create(true).read(true).append(true).mode(0o600).open(&self.path) else {
            return;
        };
        // The lock goes when `f` is closed (or by LOCK_UN below). If locking is impossible (a file system without flock) the append
        // still happens: the in-process mutex of `record` keeps threads apart, as before.
        let fd = f.as_raw_fd();
        let locked = loop {
            // SAFETY: `fd` is the open descriptor of `f`, valid for the whole function.
            if unsafe { libc::flock(fd, libc::LOCK_EX) } == 0 {
                break true;
            }
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                break false;
            }
        };
        let mut raw = Vec::new();
        let _ = f.seek(SeekFrom::Start(0));
        let _ = f.read_to_end(&mut raw);
        let text = String::from_utf8_lossy(&raw);
        let prev = text.lines().filter_map(|l| serde_json::from_str::<AuditLine>(l).ok()).last().map_or_else(|| "0".repeat(64), |l| l.hash);
        // a line cut short by a power loss must not swallow this one
        let cut = !raw.is_empty() && raw.last() != Some(&b'\n');
        let mut l = AuditLine {
            ts: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
            user: user.into(),
            event: event.into(),
            detail: detail.chars().filter(|c| !c.is_control()).take(300).collect(),
            prev,
            hash: String::new(),
        };
        l.hash = line_hash(&l);
        // one write call for the whole line, then to disk before the lock is let go
        let out = format!("{}{}\n", if cut { "\n" } else { "" }, serde_json::to_string(&l).unwrap_or_default());
        if f.write_all(out.as_bytes()).is_ok() {
            let _ = f.sync_data();
        }
        if locked {
            // SAFETY: as above.
            unsafe { libc::flock(fd, libc::LOCK_UN) };
        }
    }

    /// Start a new log after the chain reads BROKEN: the old file is kept beside it as `audit.log.broken-<time>` (nothing is deleted)
    /// and the new chain begins with a line that names it. Under the same file lock as an append, so a writer in another process
    /// either finishes before the move or writes into the new file. Refused while the chain is intact (nothing to repair).
    pub fn start_new_log(&self) -> Result<PathBuf, String> {
        use std::os::unix::io::AsRawFd;
        let _guard = AUDIT_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if self.verify().is_ok() {
            return Err("the audit log chain is intact: there is nothing to start over".into());
        }
        let f = std::fs::OpenOptions::new().read(true).append(true).open(&self.path).map_err(|e| e.to_string())?;
        // SAFETY: `f` is open for the whole function; the lock goes when it is closed.
        unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) };
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let mut kept = self.path.clone().into_os_string();
        kept.push(format!(".broken-{ts}"));
        let kept = PathBuf::from(kept);
        std::fs::rename(&self.path, &kept).map_err(|e| e.to_string())?;
        drop(f);
        let name = kept.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        self.append(user_of_system(), "audit.restart", &format!("a new log was started; the broken one is kept as {name}"));
        Ok(kept)
    }

    /// Verify the chain; returns the number of entries.
    pub fn verify(&self) -> Result<usize, usize> {
        let mut prev = "0".repeat(64);
        let lines = self.read();
        for (i, l) in lines.iter().enumerate() {
            if l.prev != prev || line_hash(l) != l.hash {
                return Err(i);
            }
            prev = l.hash.clone();
        }
        Ok(lines.len())
    }

    /// Most recent `n` entries, newest first, as display strings.
    pub fn recent(&self, n: usize) -> Vec<String> {
        self.read().iter().rev().take(n).map(|l| format!("{} {:<8} {} {}", l.ts, l.user, l.event, l.detail)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_sec::policy::App;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-acct-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn store_with_admin(tag: &str) -> (UserStore, Session, PathBuf) {
        let d = tmp(tag);
        let mut s = UserStore::open(&d.join("users.json")).unwrap();
        s.create_first_admin("root", "correct horse").unwrap();
        let admin = s.authenticate("root", "correct horse").unwrap();
        (s, admin, d)
    }

    #[test]
    fn first_admin_then_login() {
        let (mut s, admin, d) = store_with_admin("login");
        assert_eq!(admin.role, Role::Admin);
        assert!(admin.policy.manage_users && admin.policy.allows_app(App::Users));
        assert_eq!(s.create_first_admin("x", "longenough"), Err(AccountError::Forbidden));
        assert_eq!(s.authenticate("root", "wrong").unwrap_err(), AccountError::InvalidCredentials);
        assert_eq!(s.authenticate("nobody", "whatever1").unwrap_err(), AccountError::InvalidCredentials);
        // persisted, hashed, and private
        let text = std::fs::read_to_string(d.join("users.json")).unwrap();
        assert!(text.contains("$argon2id$") && !text.contains("correct horse"));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(d.join("users.json")).unwrap().permissions().mode() & 0o777, 0o600);
        let s2 = UserStore::open(&d.join("users.json")).unwrap();
        assert_eq!(s2.list().len(), 1);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn validation() {
        let d = tmp("valid");
        let mut s = UserStore::open(&d.join("u.json")).unwrap();
        assert_eq!(s.create_first_admin("Bad Name", "longenough"), Err(AccountError::BadUsername));
        assert_eq!(s.create_first_admin("ok", "short"), Err(AccountError::WeakPassword));
        assert!(s.is_empty());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn only_admins_manage_users() {
        let (mut s, admin, d) = store_with_admin("rbac");
        s.create_user(&admin, "alice", "alicepass1", Role::User).unwrap();
        s.create_user(&admin, "gus", "guestpass1", Role::Guest).unwrap();
        assert_eq!(s.create_user(&admin, "alice", "alicepass1", Role::User), Err(AccountError::Exists));
        let alice = s.authenticate("alice", "alicepass1").unwrap();
        assert_eq!(alice.role, Role::User);
        assert_eq!(s.create_user(&alice, "eve", "evepass123", Role::Admin), Err(AccountError::Forbidden));
        assert_eq!(s.set_role(&alice, "alice", Role::Admin), Err(AccountError::Forbidden));
        assert_eq!(s.delete_user(&alice, "gus"), Err(AccountError::Forbidden));
        assert_eq!(s.set_overrides(&alice, "alice", Default::default()), Err(AccountError::Forbidden));
        // an override can never turn a user into an admin-level account
        let sneaky = PolicyOverride { allow_apps: vec![App::Users], ..Default::default() };
        s.set_overrides(&admin, "alice", sneaky).unwrap();
        let a2 = s.authenticate("alice", "alicepass1").unwrap();
        assert!(!a2.policy.manage_users && !a2.policy.allows_app(App::Users));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn last_admin_is_protected() {
        let (mut s, admin, d) = store_with_admin("lastadmin");
        assert_eq!(s.delete_user(&admin, "root"), Err(AccountError::LastAdmin));
        assert_eq!(s.set_role(&admin, "root", Role::User), Err(AccountError::LastAdmin));
        s.create_user(&admin, "bob", "bobspass12", Role::Admin).unwrap();
        s.set_role(&admin, "root", Role::Operator).unwrap();
        assert_eq!(s.delete_user(&admin, "nobody"), Err(AccountError::NoSuchUser));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn password_changes() {
        let (mut s, admin, d) = store_with_admin("pw");
        s.create_user(&admin, "alice", "alicepass1", Role::User).unwrap();
        s.create_user(&admin, "bob", "bobspass12", Role::User).unwrap();
        let alice = s.authenticate("alice", "alicepass1").unwrap();
        assert_eq!(s.set_password(&alice, "alice", Some("wrongold"), "brandnew123"), Err(AccountError::Forbidden));
        assert_eq!(s.set_password(&alice, "bob", Some("bobspass12"), "brandnew123"), Err(AccountError::Forbidden));
        assert_eq!(s.set_password(&alice, "alice", Some("alicepass1"), "short"), Err(AccountError::WeakPassword));
        s.set_password(&alice, "alice", Some("alicepass1"), "brandnew123").unwrap();
        assert!(s.authenticate("alice", "brandnew123").is_ok());
        s.set_password(&admin, "bob", None, "adminreset1").unwrap();
        assert!(s.authenticate("bob", "adminreset1").is_ok());
        // an admin changing their OWN password still has to prove the current one
        assert_eq!(s.set_password(&admin, "root", None, "newrootpass1"), Err(AccountError::Forbidden));
        assert_eq!(s.set_password(&admin, "root", Some("not-the-password"), "newrootpass1"), Err(AccountError::Forbidden));
        s.set_password(&admin, "root", Some("correct horse"), "newrootpass1").unwrap();
        assert!(s.authenticate("root", "newrootpass1").is_ok());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn repeated_failures_are_throttled() {
        let (mut s, _a, d) = store_with_admin("throttle");
        for _ in 0..3 {
            assert_eq!(s.authenticate("root", "nope").unwrap_err(), AccountError::InvalidCredentials);
        }
        // even the right password is refused during the lockout
        assert!(matches!(s.authenticate("root", "correct horse"), Err(AccountError::Throttled(n)) if n >= 1));
        let _ = std::fs::remove_dir_all(d);
    }

    /// Not a test of its own: the child processes of `two_processes_and_two_threads_...` run this one (the test binary itself, told by
    /// the environment which log to append to). Without the variables it does nothing.
    #[test]
    fn audit_append_helper_process() {
        let (Ok(path), Ok(n), Ok(who)) = (std::env::var("IYAKKI_AUDIT_TEST_LOG"), std::env::var("IYAKKI_AUDIT_TEST_N"), std::env::var("IYAKKI_AUDIT_TEST_WHO")) else {
            return;
        };
        let a = AuditLog::new(Path::new(&path));
        for i in 0..n.parse::<usize>().unwrap() {
            a.record(&who, "race.test", &format!("{who} {i}"));
        }
    }

    #[test]
    fn a_broken_chain_is_kept_aside_and_a_new_log_starts_that_names_it() {
        let d = tmp("newlog");
        let log = AuditLog::new(&d.join("audit.log"));
        log.record("root", "login", "admin");
        assert!(log.start_new_log().is_err(), "an intact chain is not started over");
        log.record("root", "login", "admin");
        // damage the first line the way an old race did
        let text = std::fs::read_to_string(log.path()).unwrap();
        let damaged = text.replacen("\"login\"", "\"logout\"", 1);
        std::fs::write(log.path(), &damaged).unwrap();
        assert!(log.verify().is_err());
        let kept = log.start_new_log().unwrap();
        assert!(kept.file_name().unwrap().to_string_lossy().starts_with("audit.log.broken-") && kept.exists(), "the old log is kept");
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), damaged, "unchanged");
        assert_eq!(log.verify(), Ok(1), "a new, intact chain with one line");
        assert!(log.recent(1)[0].contains("audit.restart") && log.recent(1)[0].contains("audit.log.broken-"));
        log.record("root", "login", "admin");
        assert_eq!(log.verify(), Ok(2));
    }

    #[test]
    fn two_processes_and_two_threads_appending_at_once_keep_one_unbroken_chain() {
        let d = tmp("audit-race");
        let path = d.join("audit.log");
        let exe = std::env::current_exe().unwrap();
        // two child processes, 750 lines each
        let mut kids: Vec<std::process::Child> = ["procA", "procB"]
            .iter()
            .map(|who| {
                std::process::Command::new(&exe)
                    .args(["accounts::tests::audit_append_helper_process", "--exact", "--test-threads=1"])
                    .env("IYAKKI_AUDIT_TEST_LOG", &path)
                    .env("IYAKKI_AUDIT_TEST_N", "750")
                    .env("IYAKKI_AUDIT_TEST_WHO", who)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .unwrap()
            })
            .collect();
        // and two threads in this process, each with its own handle, calling `append` directly: that skips the in-process mutex,
        // so only the file lock keeps them (and the children) apart
        let threads: Vec<_> = ["threadA", "threadB"]
            .iter()
            .map(|who| {
                let path = path.clone();
                let who = who.to_string();
                std::thread::spawn(move || {
                    let a = AuditLog::new(&path);
                    for i in 0..250 {
                        a.append(&who, "race.test", &format!("{who} {i}"));
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        for k in &mut kids {
            assert!(k.wait().unwrap().success());
        }
        let a = AuditLog::new(&path);
        assert_eq!(a.verify(), Ok(2000), "every append is in the chain, none shares a prev");
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2000);
        for who in ["procA", "procB"] {
            assert_eq!(text.lines().filter(|l| l.contains(&format!("\"user\":\"{who}\""))).count(), 750, "{who}");
        }
        for who in ["threadA", "threadB"] {
            assert_eq!(text.lines().filter(|l| l.contains(&format!("\"user\":\"{who}\""))).count(), 250, "{who}");
        }
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_line_cut_short_by_a_power_loss_does_not_break_the_next_append_under_the_lock() {
        let d = tmp("audit-cut");
        let path = d.join("audit.log");
        let a = AuditLog::new(&path);
        a.record("root", "login", "ok");
        a.record("root", "logout", "ok");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"ts\":1,\"user\":\"root\",\"ev");
        std::fs::write(&path, text).unwrap();
        a.record("root", "login", "again");
        assert_eq!(a.verify(), Ok(3), "the cut line is skipped, the new line chains to the last whole one");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn audit_chain_detects_tampering() {
        let d = tmp("audit");
        let a = AuditLog::new(&d.join("audit.log"));
        a.record("root", "login", "ok");
        a.record("alice", "app.denied", "users\nforged line");
        a.record("root", "user.create", "bob");
        assert_eq!(a.verify(), Ok(3));
        assert_eq!(a.recent(1).len(), 1);
        assert!(!a.recent(3)[1].contains('\n'));
        let text = std::fs::read_to_string(d.join("audit.log")).unwrap();
        std::fs::write(d.join("audit.log"), text.replace("\"alice\"", "\"mallory\"")).unwrap();
        assert_eq!(a.verify(), Err(1));
        // dropping the first entry breaks the chain too
        let kept: Vec<&str> = text.lines().skip(1).collect();
        std::fs::write(d.join("audit.log"), kept.join("\n") + "\n").unwrap();
        assert_eq!(a.verify(), Err(0));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn an_account_added_from_outside_while_the_program_runs_is_noticed_at_the_next_sign_in() {
        let d = std::env::temp_dir().join(format!("iyakki-accounts-outside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let path = d.join("users.json");
        let mut running = UserStore::open(&path).unwrap();
        assert!(running.is_empty());
        // `iyakki user add` is another process with its own copy of the store
        let mut cli = UserStore::open(&path).unwrap();
        cli.add_local("ann", "a long password", Role::Admin).unwrap();
        assert!(running.is_empty(), "not looked at yet");
        let s = running.authenticate("ann", "a long password").expect("the new account is seen");
        assert_eq!((s.user.as_str(), s.role), ("ann", Role::Admin));
        assert!(!running.is_empty());
        // a second one, and a removed file
        cli.add_local("bob", "another long one", Role::User).unwrap();
        running.refresh();
        assert_eq!(running.list().len(), 2);
        std::fs::remove_file(&path).unwrap();
        running.refresh();
        assert!(running.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
