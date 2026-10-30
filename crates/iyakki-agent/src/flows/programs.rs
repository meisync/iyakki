//! Which programs a flow's command may start. The default is an allowlist: the CLI tile's installed programs, the tile's own bin
//! folder, and a short set of safe utilities. An administrator can add more (by name or full path); some things are refused
//! whatever the list says (`sudo`, `dd`, `mount`, `mkfs`, `chown` and `chmod` on system paths, `rm -rf` on `/` or a home root).

use std::path::{Path, PathBuf};

pub const SAFE: [&str; 26] = ["jq", "grep", "sed", "awk", "cat", "head", "tail", "sort", "uniq", "wc", "date", "echo", "test", "true", "false", "sleep", "tr", "cut", "printf", "base64", "sha256sum", "tar", "gzip", "git", "python3", "node"];
const REFUSED: [&str; 6] = ["sudo", "su", "doas", "dd", "mount", "umount"];
const SYSTEM_PATHS: [&str; 12] = ["/", "/etc", "/usr", "/bin", "/sbin", "/boot", "/lib", "/var", "/sys", "/proc", "/dev", "/root"];

/// The text a refused program starts with, so the screen can offer "allow this program".
pub const NOT_ALLOWED: &str = "is not an allowed program";

pub struct Programs {
    file: PathBuf,
}

fn base(p: &str) -> String {
    p.rsplit('/').next().unwrap_or(p).to_ascii_lowercase()
}

fn system_path(a: &str) -> bool {
    let t = a.trim_end_matches('/');
    let t = if t.is_empty() { "/" } else { t };
    let home_root = t == "/home" || (t.starts_with("/home/") && !t[6..].contains('/'));
    home_root || a.starts_with('/') && SYSTEM_PATHS.iter().any(|s| t == *s || (*s != "/" && t.starts_with(&format!("{s}/")))) || a == "~" || a.starts_with("~/") || a == "$HOME"
}

/// What is refused whatever the list says; `None` when the command is not one of those.
pub fn always_refused(prog: &str, args: &[String]) -> Option<String> {
    let b = base(prog);
    if REFUSED.contains(&b.as_str()) || b == "mkfs" || b.starts_with("mkfs.") {
        return Some(format!("`{b}` changes the machine itself and is never allowed in a flow"));
    }
    let paths = || args.iter().filter(|a| !a.starts_with('-'));
    if matches!(b.as_str(), "chown" | "chmod") && paths().any(|a| system_path(a)) {
        return Some(format!("`{b}` on a system path is never allowed in a flow"));
    }
    if b == "rm" && args.iter().any(|a| a.starts_with('-') && (a.contains('r') || a.contains('R')) && a.contains('f') || a == "--recursive") && paths().any(|a| system_path(a) || a == "*" || a == "/*" || a == "~/*") {
        return Some("`rm -rf` on / or a home folder is never allowed in a flow".into());
    }
    None
}

impl Programs {
    pub fn new(file: &Path) -> Programs {
        Programs { file: file.to_path_buf() }
    }

    /// The names and paths an administrator added.
    pub fn extra(&self) -> Vec<String> {
        std::fs::read_to_string(&self.file).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["allow"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())).unwrap_or_default()
    }

    fn save(&self, v: &[String]) -> Result<(), String> {
        if let Some(d) = self.file.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        std::fs::write(&self.file, serde_json::json!({"allow": v}).to_string()).map_err(|e| e.to_string())
    }

    pub fn add(&self, name: &str) -> Result<(), String> {
        let n = name.trim();
        if n.is_empty() || n.len() > 200 || n.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err("a program is a name or a full path, with no spaces".into());
        }
        if always_refused(n, &[]).is_some() {
            return Err(format!("`{n}` can never be allowed"));
        }
        let mut v = self.extra();
        if !v.iter().any(|x| x == n) {
            v.push(n.to_string());
        }
        self.save(&v)
    }

    pub fn remove(&self, name: &str) -> Result<(), String> {
        let v: Vec<String> = self.extra().into_iter().filter(|x| x != name).collect();
        self.save(&v)
    }

    /// May `prog` run? `installed` says whether a CLI-tile program of that name exists; `bin` is the tile's bin folder.
    pub fn check(&self, prog: &str, args: &[String], installed: &dyn Fn(&str) -> bool, bin: &Path) -> Result<(), String> {
        if let Some(why) = always_refused(prog, args) {
            return Err(why);
        }
        let b = base(prog);
        if SAFE.contains(&b.as_str()) && !prog.contains('/') {
            return Ok(());
        }
        if installed(&b) && !prog.contains('/') {
            return Ok(());
        }
        if !prog.contains('/') && bin.join(&b).exists() {
            return Ok(());
        }
        if self.extra().iter().any(|x| x == prog || (!x.contains('/') && *x == b && !prog.contains('/'))) {
            return Ok(());
        }
        // the image ships neither: web calls belong to the HTTP request step (in-house client, host policy, audit)
        let hint = if matches!(b.as_str(), "curl" | "wget") { " Use the HTTP request step for web calls." } else { "" };
        Err(format!("`{prog}` {NOT_ALLOWED}: flows may start the CLI tile's programs and a few safe utilities.{hint} An administrator can allow `{prog}`."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(tag: &str) -> Programs {
        let d = std::env::temp_dir().join(format!("iyakki-programs-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        Programs::new(&d.join("programs.json"))
    }

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_default_is_an_allowlist_and_admins_extend_it() {
        let ps = p("a");
        let none = |_: &str| false;
        let bin = Path::new("/nonexistent-bin");
        for ok in ["jq", "tar", "git", "python3", "echo"] {
            assert!(ps.check(ok, &[], &none, bin).is_ok(), "{ok}");
        }
        let e = ps.check("terraform", &[], &none, bin).unwrap_err();
        assert!(e.contains(NOT_ALLOWED));
        // curl and wget are not on the image: the message points to the HTTP request step, and an administrator can still allow them
        for web in ["curl", "wget"] {
            let e = ps.check(web, &[], &none, bin).unwrap_err();
            assert!(e.contains(NOT_ALLOWED) && e.contains("HTTP request step"), "{e}");
        }
        assert!(ps.check("terraform", &[], &|n| n == "terraform", bin).is_ok(), "an installed CLI-tile program");
        assert!(ps.check("/usr/bin/curl", &[], &none, bin).is_err(), "a path is not a name on the list");
        ps.add("terraform").unwrap();
        assert!(ps.check("terraform", &[], &none, bin).is_ok());
        ps.add("/opt/tools/mytool").unwrap();
        assert!(ps.check("/opt/tools/mytool", &[], &none, bin).is_ok());
        assert!(ps.check("/opt/other/mytool", &[], &none, bin).is_err());
        assert_eq!(ps.extra().len(), 2);
        ps.remove("terraform").unwrap();
        assert!(ps.check("terraform", &[], &none, bin).is_err());
        assert!(ps.add("sudo").is_err() && ps.add("a b").is_err() && ps.add("").is_err());
    }

    #[test]
    fn some_things_are_refused_whatever_the_list_says() {
        let ps = p("b");
        ps.add("rm").unwrap();
        ps.add("chmod").unwrap();
        let none = |_: &str| false;
        let bin = Path::new("/nonexistent-bin");
        for (prog, args) in [("sudo", vec!["ls"]), ("su", vec![]), ("dd", vec!["if=/dev/zero"]), ("mount", vec![]), ("umount", vec![]), ("mkfs.ext4", vec!["/dev/x"]), ("chown", vec!["root", "/etc/passwd"]), ("chmod", vec!["777", "/usr/bin"]), ("rm", vec!["-rf", "/"]), ("rm", vec!["-fr", "~"]), ("rm", vec!["-rf", "/home"]), ("rm", vec!["-rf", "/*"])] {
            assert!(ps.check(prog, &a(&args), &none, bin).is_err(), "{prog} {args:?}");
        }
        // the same programs on harmless paths are fine once allowed
        assert!(ps.check("rm", &a(&["-rf", "build"]), &none, bin).is_ok());
        assert!(ps.check("chmod", &a(&["+x", "run.sh"]), &none, bin).is_ok());
    }
}
