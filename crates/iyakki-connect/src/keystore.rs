//! The operating system's key store as a home for the window program's device key (F-CLIENT-02), OFF by default.
//!
//! The default stays the mode-600 key file (`devkey.rs`). With `IYAKKI_CONNECT_KEYSTORE=os` (or `--os-keystore` where a command takes it) the
//! secret seed goes into the system's store instead and the file keeps only a pointer and the public key:
//!
//! * Linux: the Secret Service through `secret-tool` (libsecret). The seed is passed on standard input, never on the command line.
//! * macOS: the Keychain through `security`. Its tool takes the secret as an argument (`-w`), so it is visible to the same user's `ps` for
//!   the instant the command runs: that is why this is opt-in and said so in `--help` of the setting.
//! * No other system has a store here (the program targets Linux and macOS; see ALPHA-7-DECISIONS.md, D-CLIENT-02).
//!
//! The tools are run through [`Runner`], so every rule here is tested with a fake and nothing touches a real keychain in tests.

use std::io::Write;
use std::process::{Command, Stdio};

/// The service name every entry carries.
pub const SERVICE: &str = "iyakki-connect";

/// One finished command.
#[derive(Debug, PartialEq, Eq)]
pub struct Ran {
    pub ok: bool,
    pub stdout: String,
}

/// Runs a program with an optional standard input (a fake in tests).
pub trait Runner: Send + Sync {
    fn run(&self, program: &str, args: &[String], stdin: Option<&str>) -> Result<Ran, String>;
}

/// Really runs the program.
pub struct Real;

impl Runner for Real {
    fn run(&self, program: &str, args: &[String], stdin: Option<&str>) -> Result<Ran, String> {
        let mut c = Command::new(program);
        c.args(args).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = c.spawn().map_err(|e| format!("{program}: {e}"))?;
        if let (Some(text), Some(mut si)) = (stdin, child.stdin.take()) {
            si.write_all(text.as_bytes()).map_err(|e| format!("{program}: {e}"))?;
        }
        let out = child.wait_with_output().map_err(|e| format!("{program}: {e}"))?;
        Ok(Ran { ok: out.status.success(), stdout: String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string() })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavour {
    SecretService,
    Keychain,
}

/// Which store this system has, from the platform name (`std::env::consts::OS`).
pub fn flavour_for(os: &str) -> Option<Flavour> {
    match os {
        "linux" => Some(Flavour::SecretService),
        "macos" => Some(Flavour::Keychain),
        _ => None,
    }
}

pub struct OsStore {
    flavour: Flavour,
    run: Box<dyn Runner>,
}

/// An account name for one instance: only letters, digits, dash and dot, so no tool can read it as an option or a path.
fn clean_account(a: &str) -> Result<&str, String> {
    if a.is_empty() || a.len() > 80 || a.starts_with('-') || !a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_') {
        return Err("a key store account is letters, digits, dash, dot or underscore".into());
    }
    Ok(a)
}

impl OsStore {
    pub fn new(flavour: Flavour, run: Box<dyn Runner>) -> OsStore {
        OsStore { flavour, run }
    }

    /// The store of this system with the real tools, when the person asked for it (`IYAKKI_CONNECT_KEYSTORE=os`).
    pub fn from_env() -> Result<Option<OsStore>, String> {
        match std::env::var("IYAKKI_CONNECT_KEYSTORE").ok().as_deref() {
            None | Some("") | Some("file") => Ok(None),
            Some("os") => flavour_for(std::env::consts::OS).map(|f| Some(OsStore::new(f, Box::new(Real)))).ok_or_else(|| "this system has no key store the program can use: keep the key in the file".to_string()),
            Some(other) => Err(format!("IYAKKI_CONNECT_KEYSTORE is `file` or `os`, not `{other}`")),
        }
    }

    pub fn get(&self, account: &str) -> Result<Option<String>, String> {
        let a = clean_account(account)?;
        let r = match self.flavour {
            Flavour::SecretService => self.run.run("secret-tool", &["lookup".into(), "service".into(), SERVICE.into(), "account".into(), a.into()], None)?,
            Flavour::Keychain => self.run.run("security", &["find-generic-password".into(), "-s".into(), SERVICE.into(), "-a".into(), a.into(), "-w".into()], None)?,
        };
        Ok(if r.ok && !r.stdout.is_empty() { Some(r.stdout) } else { None })
    }

    pub fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        let a = clean_account(account)?;
        let r = match self.flavour {
            Flavour::SecretService => self.run.run("secret-tool", &["store".into(), "--label=IYAKKI connector device key".into(), "service".into(), SERVICE.into(), "account".into(), a.into()], Some(secret))?,
            Flavour::Keychain => self.run.run("security", &["add-generic-password".into(), "-U".into(), "-s".into(), SERVICE.into(), "-a".into(), a.into(), "-w".into(), secret.into()], None)?,
        };
        if r.ok { Ok(()) } else { Err("the system key store refused the key (is it unlocked, and is secret-tool or security installed?)".into()) }
    }

    pub fn delete(&self, account: &str) -> Result<(), String> {
        let a = clean_account(account)?;
        match self.flavour {
            Flavour::SecretService => self.run.run("secret-tool", &["clear".into(), "service".into(), SERVICE.into(), "account".into(), a.into()], None)?,
            Flavour::Keychain => self.run.run("security", &["delete-generic-password".into(), "-s".into(), SERVICE.into(), "-a".into(), a.into()], None)?,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<(String, Vec<String>, Option<String>)>>,
        answer: Mutex<String>,
    }
    struct Shared(Arc<Fake>);
    impl Runner for Shared {
        fn run(&self, p: &str, a: &[String], i: Option<&str>) -> Result<Ran, String> {
            self.0.calls.lock().unwrap().push((p.into(), a.to_vec(), i.map(str::to_string)));
            Ok(Ran { ok: true, stdout: self.0.answer.lock().unwrap().clone() })
        }
    }

    #[test]
    fn linux_uses_the_secret_service_with_the_secret_on_standard_input() {
        let f = Arc::new(Fake::default());
        let s = OsStore::new(Flavour::SecretService, Box::new(Shared(f.clone())));
        s.set("abc123", "SEEDHEX").unwrap();
        *f.answer.lock().unwrap() = "SEEDHEX".into();
        assert_eq!(s.get("abc123").unwrap().as_deref(), Some("SEEDHEX"));
        s.delete("abc123").unwrap();
        let c = f.calls.lock().unwrap();
        assert_eq!(c[0].0, "secret-tool");
        assert_eq!(c[0].2.as_deref(), Some("SEEDHEX"));
        assert!(c.iter().all(|x| !x.1.iter().any(|a| a.contains("SEEDHEX"))), "the secret is never an argument on Linux");
        assert_eq!(c[1].1[0], "lookup");
        assert_eq!(c[2].1[0], "clear");
    }

    #[test]
    fn macos_uses_the_keychain_and_an_empty_answer_is_no_key() {
        let f = Arc::new(Fake::default());
        let s = OsStore::new(Flavour::Keychain, Box::new(Shared(f.clone())));
        assert_eq!(s.get("abc123").unwrap(), None);
        s.set("abc123", "SEEDHEX").unwrap();
        let c = f.calls.lock().unwrap();
        assert_eq!(c[0].0, "security");
        assert_eq!(c[0].1[0], "find-generic-password");
        assert_eq!(c[1].1[0], "add-generic-password");
    }

    #[test]
    fn an_account_that_could_be_read_as_an_option_or_a_path_is_refused_and_other_systems_have_no_store() {
        let s = OsStore::new(Flavour::SecretService, Box::new(Shared(Arc::new(Fake::default()))));
        for bad in ["", "-x", "a b", "../x", "a/b", "a;b"] {
            assert!(s.get(bad).is_err() && s.set(bad, "s").is_err() && s.delete(bad).is_err(), "{bad}");
        }
        assert_eq!(flavour_for("linux"), Some(Flavour::SecretService));
        assert_eq!(flavour_for("macos"), Some(Flavour::Keychain));
        assert_eq!(flavour_for("windows"), None);
    }
}
