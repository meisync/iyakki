//! Starting a program the agent describes (F-ARCH-07 step 3, KI-729): the agent says what it wants (the program, its arguments, which
//! environment it may see, where its output goes) and this is the one place where a [`std::process::Command`] is made. A [`Launch`] is a
//! description, not a command line: nothing runs through a shell, and the environment is either inherited on purpose or cleared and
//! filled from a named list. A test starts a stand-in script, so no test touches this computer's programs.

use std::ffi::OsStr;
use std::fs::File;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Where one of the child's streams goes.
#[derive(Debug)]
pub enum Stream {
    /// Dropped (`/dev/null`).
    Null,
    /// A pipe the caller reads or writes.
    Piped,
    /// The same as this process has.
    Inherit,
    /// A file (a log the child appends to).
    File(File),
}

impl Stream {
    fn stdio(self) -> Stdio {
        match self {
            Stream::Null => Stdio::null(),
            Stream::Piped => Stdio::piped(),
            Stream::Inherit => Stdio::inherit(),
            Stream::File(f) => Stdio::from(f),
        }
    }
}

/// The variables a started program may keep from this process when its environment is cleared.
pub const PATH_AND_LOCALE: [&str; 2] = ["PATH", "LANG"];

/// A program to start: arguments, working folder, environment, streams, own process group.
#[derive(Debug)]
pub struct Launch {
    cmd: Command,
}

impl Launch {
    /// A launch of `program` with no arguments; the streams and environment are the process's own until changed.
    pub fn new(program: impl AsRef<OsStr>) -> Launch {
        Launch { cmd: Command::new(program) }
    }

    /// Add arguments.
    pub fn args<I, S>(mut self, args: I) -> Launch
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.cmd.args(args);
        self
    }

    /// Add one argument.
    pub fn arg(mut self, a: impl AsRef<OsStr>) -> Launch {
        self.cmd.arg(a);
        self
    }

    /// Run in `dir`.
    pub fn cwd(mut self, dir: impl AsRef<Path>) -> Launch {
        self.cmd.current_dir(dir);
        self
    }

    /// Start with an empty environment (nothing of this process's is passed on).
    pub fn env_clear(mut self) -> Launch {
        self.cmd.env_clear();
        self
    }

    /// Set one variable.
    pub fn env(mut self, k: impl AsRef<OsStr>, v: impl AsRef<OsStr>) -> Launch {
        self.cmd.env(k, v);
        self
    }

    /// Copy the named variables from this process, those that are set.
    pub fn pass_env<I, S>(mut self, names: I) -> Launch
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for k in names {
            if let Ok(v) = std::env::var(k.as_ref()) {
                self.cmd.env(k.as_ref(), v);
            }
        }
        self
    }

    /// The child's input.
    pub fn stdin(mut self, s: Stream) -> Launch {
        self.cmd.stdin(s.stdio());
        self
    }

    /// The child's output.
    pub fn stdout(mut self, s: Stream) -> Launch {
        self.cmd.stdout(s.stdio());
        self
    }

    /// The child's errors.
    pub fn stderr(mut self, s: Stream) -> Launch {
        self.cmd.stderr(s.stdio());
        self
    }

    /// Put the child in a process group of its own (so one signal reaches it and its children, and a terminal's Ctrl-C does not).
    pub fn own_group(mut self) -> Launch {
        use std::os::unix::process::CommandExt;
        self.cmd.process_group(0);
        self
    }

    /// The command this describes, for a caller that adds a step to run in the child before the program starts (a mount namespace, a pseudo-terminal).
    pub fn into_command(self) -> Command {
        self.cmd
    }

    /// Start it once.
    pub fn spawn(mut self) -> std::io::Result<Child> {
        self.cmd.spawn()
    }

    /// Start it, trying again for a moment on "text file busy" (a program written a moment ago can still be open for writing in a child another
    /// thread forked and that has not run its `exec` yet): up to `tries` starts, `pause` between them.
    pub fn spawn_retrying(mut self, tries: u32, pause: Duration) -> std::io::Result<Child> {
        spawn_command_retrying(&mut self.cmd, tries, pause)
    }

    /// Start it and wait for the whole output (input closed, both streams kept).
    pub fn output(mut self) -> std::io::Result<std::process::Output> {
        self.cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        spawn_command_retrying(&mut self.cmd, 51, Duration::from_millis(20))?.wait_with_output()
    }
}

/// Start an already described command, trying again on "text file busy" (see [`Launch::spawn_retrying`]).
pub fn spawn_command_retrying(cmd: &mut Command, tries: u32, pause: Duration) -> std::io::Result<Child> {
    let mut n = 0;
    loop {
        match cmd.spawn() {
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && n + 1 < tries => {
                n += 1;
                std::thread::sleep(pause);
            }
            r => return r,
        }
    }
}

/// The long-running server of a tool the person installed (k3s, Ollama): the program with `args`, a cleared environment holding only `env`, no input,
/// output and errors appended to the given log files, in a process group of its own so Stop can end it and its children together.
pub fn server_launch(program: &Path, args: &[std::ffi::OsString], env: &[(&str, std::ffi::OsString)], log: File, log_errors: File) -> Launch {
    let mut l = Launch::new(program).args(args).env_clear();
    for (k, v) in env {
        l = l.env(k, v);
    }
    l.stdin(Stream::Null).stdout(Stream::File(log)).stderr(Stream::File(log_errors)).own_group()
}

/// The search path of the programs the agent starts for a tool it installed: the tool folder first, then the usual system folders.
pub fn tool_path(bin_dir: &Path) -> String {
    format!("{}:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin", bin_dir.display())
}

/// Run `program args...` in `dir` with a cleared environment holding `env`, no input, output and errors kept; the child, for the caller to read and wait on.
pub fn spawn_clean_capture(program: &OsStr, args: &[String], dir: Option<&Path>, env: &[(String, String)]) -> std::io::Result<Child> {
    let mut l = Launch::new(program).args(args).env_clear().stdin(Stream::Null).stdout(Stream::Piped).stderr(Stream::Piped);
    if let Some(d) = dir {
        l = l.cwd(d);
    }
    for (k, v) in env {
        l = l.env(k, v);
    }
    l.spawn_retrying(51, Duration::from_millis(20))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn script(tag: &str, body: &str) -> String {
        let d = std::env::temp_dir().join(format!("iyakki-launch-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("fake");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.display().to_string()
    }

    fn text(mut c: Child) -> (String, String) {
        let (mut o, mut e) = (String::new(), String::new());
        std::io::Read::read_to_string(c.stdout.as_mut().unwrap(), &mut o).unwrap();
        std::io::Read::read_to_string(c.stderr.as_mut().unwrap(), &mut e).unwrap();
        c.wait().unwrap();
        (o, e)
    }

    #[test]
    fn a_launch_passes_arguments_the_folder_and_only_the_named_variables() {
        let _serial = crate::starters::spawn_lock();
        let d = std::env::temp_dir();
        let c = Launch::new(script("env", "echo \"$1|$2|$(pwd)|${HOME:-nohome}|${ONLY_THIS:-none}|${PATH:+path}\""))
            .args(["a", "b"])
            .cwd(&d)
            .env_clear()
            .env("ONLY_THIS", "yes")
            .pass_env(["PATH", "IYAKKI_SURELY_NOT_SET_FOR_A_TEST"])
            .stdout(Stream::Piped)
            .spawn_retrying(51, Duration::from_millis(5))
            .unwrap();
        let mut o = String::new();
        let mut c = c;
        std::io::Read::read_to_string(c.stdout.as_mut().unwrap(), &mut o).unwrap();
        c.wait().unwrap();
        let line = o.trim();
        assert!(line.starts_with("a|b|") && line.ends_with("|nohome|yes|path"), "{line}");
    }

    #[test]
    fn the_streams_go_where_they_are_told() {
        let _serial = crate::starters::spawn_lock();
        let c = Launch::new(script("streams", "cat; echo out; echo err >&2")).stdin(Stream::Piped).stdout(Stream::Piped).stderr(Stream::Piped);
        let mut c = c.spawn_retrying(51, Duration::from_millis(5)).unwrap();
        std::io::Write::write_all(c.stdin.as_mut().unwrap(), b"in\n").unwrap();
        drop(c.stdin.take());
        let (o, e) = text(c);
        assert_eq!((o.as_str(), e.as_str()), ("in\nout\n", "err\n"));

        let log = std::env::temp_dir().join(format!("iyakki-launch-log-{}", std::process::id()));
        let f = File::create(&log).unwrap();
        let g = f.try_clone().unwrap();
        let mut c = Launch::new(script("file", "echo out; echo err >&2")).stdin(Stream::Null).stdout(Stream::File(f)).stderr(Stream::File(g)).spawn_retrying(51, Duration::from_millis(5)).unwrap();
        assert!(c.wait().unwrap().success());
        let t = std::fs::read_to_string(&log).unwrap();
        assert!(t.contains("out") && t.contains("err"), "{t}");

        let o = Launch::new(script("dropped", "echo hidden; exit 4")).stdout(Stream::Null).stderr(Stream::Null).spawn_retrying(51, Duration::from_millis(5)).unwrap().wait().unwrap();
        assert_eq!(o.code(), Some(4));
    }

    #[test]
    fn the_output_form_keeps_both_streams_and_a_missing_program_is_an_error() {
        let _serial = crate::starters::spawn_lock();
        let o = Launch::new(script("output", "echo a; echo b >&2; exit 2")).output().unwrap();
        assert_eq!((o.status.code(), String::from_utf8_lossy(&o.stdout).trim(), String::from_utf8_lossy(&o.stderr).trim()), (Some(2), "a", "b"));
        assert!(Launch::new("/nonexistent/program").output().is_err());
        assert!(Launch::new("/nonexistent/program").spawn().is_err());
        assert!(Launch::new("/nonexistent/program").spawn_retrying(3, Duration::ZERO).is_err());
    }

    #[test]
    fn an_own_group_child_leads_its_group_and_the_retrying_start_starts_once_when_it_can() {
        let _serial = crate::starters::spawn_lock();
        let mut c = Launch::new(script("group", "echo $$ $(ps -o pgid= -p $$)")).own_group().stdout(Stream::Piped).spawn_retrying(5, Duration::ZERO).unwrap();
        let mut o = String::new();
        std::io::Read::read_to_string(c.stdout.as_mut().unwrap(), &mut o).unwrap();
        c.wait().unwrap();
        let mut w = o.split_whitespace();
        assert_eq!(w.next(), w.next(), "the child is the leader of its own group: {o}");
    }

    #[test]
    fn a_server_runs_with_only_its_environment_and_writes_to_its_logs() {
        let _serial = crate::starters::spawn_lock();
        let d = std::env::temp_dir().join(format!("iyakki-server-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let log = File::create(d.join("log")).unwrap();
        let log2 = log.try_clone().unwrap();
        let prog = script("server", "echo \"$*|$HOME|${PATH}|${LEAK:-none}\"; echo oops >&2");
        let mut c = server_launch(Path::new(&prog), &["serve".into(), "--x".into()], &[("HOME", "/h".into()), ("PATH", tool_path(Path::new("/opt/bin")).into())], log, log2).spawn_retrying(51, Duration::from_millis(5)).unwrap();
        assert!(c.wait().unwrap().success());
        let t = std::fs::read_to_string(d.join("log")).unwrap();
        assert!(t.contains("serve --x|/h|/opt/bin:/usr/local/sbin:") && t.contains("|none") && t.contains("oops"), "{t}");
    }

    #[test]
    fn the_clean_capture_starts_in_the_folder_with_the_given_variables() {
        let _serial = crate::starters::spawn_lock();
        let d = std::env::temp_dir();
        let c = spawn_clean_capture(OsStr::new(&script("clean", "echo \"$K|$(pwd)|${HOME:-nohome}|$1\"; echo e >&2")), &["x".into()], Some(&d), &[("K".into(), "v".into())]).unwrap();
        let (o, e) = text(c);
        assert!(o.starts_with("v|") && o.trim().ends_with("|nohome|x"), "{o}");
        assert_eq!(e, "e\n");
        assert!(spawn_clean_capture(OsStr::new("/nonexistent/prog"), &[], None, &[]).is_err());
    }
}
