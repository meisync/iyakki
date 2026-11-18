//! Starting the privileged writer from the window installer, and reading what it says.
//!
//! The window installer never runs as root and never sees a password: it starts the writer (`iyakki-usb --apply-plan FILE`) through
//! `pkexec` (polkit asks the person, in the desktop's own dialog) or, where there is no polkit agent, tells the person the
//! `sudo` command to run in a terminal. Nothing is stored. The writer re-checks everything itself ([`crate::plan_cli`]).

use std::path::{Path, PathBuf};

/// How to gain administrator rights for one command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Elevation {
    Pkexec(PathBuf),
    /// No graphical way: the person runs this command in a terminal.
    Manual,
}

/// Find `pkexec` in `path_var` (a PATH string) when a graphical session exists.
pub fn find_elevation(path_var: &str, graphical: bool) -> Elevation {
    if graphical {
        for d in path_var.split(':').filter(|d| !d.is_empty()) {
            let p = Path::new(d).join("pkexec");
            if p.is_file() {
                return Elevation::Pkexec(p);
            }
        }
    }
    Elevation::Manual
}

/// The writer program: next to the window installer, else on the PATH.
pub fn writer_program(own_exe: &Path, path_var: &str) -> Option<PathBuf> {
    if let Some(dir) = own_exe.parent() {
        let p = dir.join("iyakki-usb");
        if p.is_file() {
            return Some(p);
        }
    }
    path_var.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join("iyakki-usb")).find(|p| p.is_file())
}

/// The command (program and arguments) that applies `plan_file`.
pub fn apply_command(e: &Elevation, writer: &Path, plan_file: &Path) -> Option<Vec<String>> {
    match e {
        Elevation::Pkexec(p) => Some(vec![p.display().to_string(), writer.display().to_string(), "--apply-plan".into(), plan_file.display().to_string(), "--machine".into()]),
        Elevation::Manual => None,
    }
}

/// The command (program and arguments) that lists the disks as JSON for `image`.
pub fn list_command(e: &Elevation, writer: &Path, image: Option<&Path>) -> Option<Vec<String>> {
    let mut args = vec![writer.display().to_string(), "--plan".into(), "--list".into(), "--json".into()];
    if let Some(i) = image {
        args.push(i.display().to_string());
    }
    match e {
        Elevation::Pkexec(p) => {
            let mut v = vec![p.display().to_string()];
            v.extend(args);
            Some(v)
        }
        Elevation::Manual => None,
    }
}

/// What the manual route shows the person.
pub fn manual_instructions(writer: &Path, plan_file: &Path) -> String {
    format!("No graphical administrator prompt (polkit) is available. In a terminal run:\n  sudo {} --apply-plan {}\nThe plan file already holds your typed confirmation.", writer.display(), plan_file.display())
}

/// One line the writer prints with `--machine`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Progress { phase: String, done: u64, total: u64 },
    Done(u64),
    Note(String),
}

pub fn parse_line(l: &str) -> Option<Event> {
    let l = l.trim();
    if let Some(r) = l.strip_prefix("@progress ") {
        let mut it = r.split_whitespace();
        let phase = it.next()?.to_string();
        return Some(Event::Progress { phase, done: it.next()?.parse().ok()?, total: it.next()?.parse().ok()? });
    }
    if let Some(r) = l.strip_prefix("@done ") {
        return Some(Event::Done(r.trim().parse().ok()?));
    }
    l.strip_prefix("@note ").map(|r| Event::Note(r.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_commands_go_through_pkexec_and_never_carry_a_password() {
        let e = Elevation::Pkexec("/usr/bin/pkexec".into());
        let c = apply_command(&e, Path::new("/opt/iyakki/iyakki-usb"), Path::new("/run/user/1000/plan.json")).unwrap();
        assert_eq!(c, ["/usr/bin/pkexec", "/opt/iyakki/iyakki-usb", "--apply-plan", "/run/user/1000/plan.json", "--machine"]);
        let l = list_command(&e, Path::new("/opt/iyakki/iyakki-usb"), Some(Path::new("/tmp/i.img"))).unwrap();
        assert_eq!(&l[1..5], ["/opt/iyakki/iyakki-usb", "--plan", "--list", "--json"]);
        assert!(!l.iter().chain(c.iter()).any(|a| a.contains("pass") || a == "-S"));
        assert_eq!(apply_command(&Elevation::Manual, Path::new("/x"), Path::new("/y")), None);
        assert!(manual_instructions(Path::new("/x/iyakki-usb"), Path::new("/y/p.json")).contains("sudo /x/iyakki-usb --apply-plan /y/p.json"));
    }

    #[test]
    fn pkexec_needs_a_graphical_session_and_the_writer_is_found_beside_the_window() {
        let dir = std::env::temp_dir().join(format!("iyakki-elev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pkexec"), "").unwrap();
        std::fs::write(dir.join("iyakki-usb"), "").unwrap();
        let p = dir.display().to_string();
        assert_eq!(find_elevation(&p, true), Elevation::Pkexec(dir.join("pkexec")));
        assert_eq!(find_elevation(&p, false), Elevation::Manual);
        assert_eq!(find_elevation("/nonexistent", true), Elevation::Manual);
        assert_eq!(writer_program(&dir.join("iyakki-installer"), ""), Some(dir.join("iyakki-usb")));
        assert_eq!(writer_program(Path::new("/nonexistent/iyakki-installer"), &p), Some(dir.join("iyakki-usb")));
        assert_eq!(writer_program(Path::new("/nonexistent/x"), "/nonexistent"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_writers_lines_are_parsed() {
        assert_eq!(parse_line("@progress writing 5 10"), Some(Event::Progress { phase: "writing".into(), done: 5, total: 10 }));
        assert_eq!(parse_line("@done 123\n"), Some(Event::Done(123)));
        assert_eq!(parse_line("@note UEFI boot entry added"), Some(Event::Note("UEFI boot entry added".into())));
        assert_eq!(parse_line("garbage"), None);
        assert_eq!(parse_line("@progress writing x 1"), None);
    }
}
