//! The programs the core starts for the agent, one named operation each (F-ARCH-07, item 4 of docs/development/CORE-LOCAL-EXTRACTION-PLAN.md): the
//! agent says what it wants (set a mixer control, read the kernel log) and this is the one place that runs `amixer` and `dmesg`. Each has a
//! form that takes the program, so a test starts a stand-in script and never touches this computer's sound or log.

use std::process::{Command, Stdio};

/// Why a mixer call failed: the program did not start, or it ran and said no.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AmixerFailure {
    /// `true`: the program could not be started (not installed). `false`: it ran and ended with an error.
    pub not_started: bool,
    /// The program's own words (its error output, trimmed), or the start error.
    pub text: String,
}

/// Run `program args...` as the mixer (output kept); the output text, or why it failed.
pub fn mixer_text_with(program: &str, args: &[String]) -> Result<String, AmixerFailure> {
    let out = Command::new(program).args(args).stdin(Stdio::null()).output().map_err(|e| AmixerFailure { not_started: true, text: e.to_string() })?;
    if out.status.success() { Ok(String::from_utf8_lossy(&out.stdout).into_owned()) } else { Err(AmixerFailure { not_started: false, text: String::from_utf8_lossy(&out.stderr).trim().to_string() }) }
}

/// [`mixer_text_with`] on the system's `amixer`.
pub fn amixer_text(args: &[String]) -> Result<String, AmixerFailure> {
    mixer_text_with("amixer", args)
}

/// Run `program args...` with no output kept: `Ok` when it ended well, otherwise a sentence (`amixer: <start error>` or `amixer ended with <status>`).
pub fn amixer_quiet_with(program: &str, args: &[String]) -> Result<(), String> {
    let st = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map_err(|e| format!("amixer: {e}"))?;
    if st.success() { Ok(()) } else { Err(format!("amixer ended with {st}")) }
}

/// [`amixer_quiet_with`] on the system's `amixer`.
pub fn amixer_quiet(args: &[String]) -> Result<(), String> {
    amixer_quiet_with("amixer", args)
}

/// The kernel's ring buffer as `dmesg` prints it; empty when it cannot be read.
pub fn dmesg() -> String {
    Command::new("dmesg").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// `git status --porcelain -uall` in `dir`: its output when git ran and ended well, `None` otherwise (git missing, not a repository).
pub fn git_status_porcelain(dir: &std::path::Path) -> Option<String> {
    git_status_porcelain_with("git", dir)
}

/// [`git_status_porcelain`] with the program given (a test passes a stand-in).
pub fn git_status_porcelain_with(program: &str, dir: &std::path::Path) -> Option<String> {
    let o = Command::new(program).current_dir(dir).args(["status", "--porcelain", "-uall"]).output().ok()?;
    if o.status.success() { Some(String::from_utf8_lossy(&o.stdout).into_owned()) } else { None }
}

/// A GStreamer tool (`gst-launch-1.0`, `gst-inspect-1.0`) run with its own plugin registry file so it never touches the system one: the finished output,
/// or the start error.
pub fn gst_run(program: &str, args: &[String], registry: &std::path::Path) -> Result<std::process::Output, String> {
    Command::new(program).args(args).env("GST_REGISTRY_1_0", registry).output().map_err(|e| e.to_string())
}

/// `true` when the GStreamer tool ran with `args` and ended well ([`gst_run`]).
pub fn gst_ok(program: &str, args: &[String], registry: &std::path::Path) -> bool {
    gst_run(program, args, registry).map(|o| o.status.success()).unwrap_or(false)
}

/// The video-site extractor: run with a cleared environment, `HOME` and the working folder set to `out`, and a fixed `PATH`; the finished output.
pub fn run_extractor(exe: &std::path::Path, args: &[String], out: &std::path::Path) -> std::io::Result<std::process::Output> {
    Command::new(exe).args(args).env_clear().env("HOME", out).env("PATH", "/usr/bin:/bin").current_dir(out).output()
}

/// Start `program args...` in its own process group with no input or output kept (opening the system browser); the child, for the caller to reap.
pub fn open_detached(program: &std::path::Path, args: &[String]) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    Command::new(program).args(args).process_group(0).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
}

/// Start `sh -c <line>` (the one place a command line is run through a shell for the pending-work ledger) with no input, output and errors to the
/// given files, in `cwd` when it is a folder; the child, for the caller to wait on.
pub fn shell_line_spawn(line: &str, cwd: &str, stdout: std::fs::File, stderr: std::fs::File) -> std::io::Result<std::process::Child> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(line).stdin(Stdio::null()).stdout(stdout).stderr(stderr);
    if std::path::Path::new(cwd).is_dir() {
        cmd.current_dir(cwd);
    }
    cmd.spawn()
}

/// A `git` command for `dir` that never asks anything: no inherited repository variables, no terminal prompt, no editor, no optional locks, and a fixed
/// author and no signing for the commits IYAKKI makes itself (parallel tasks). The caller adds its arguments and runs it.
pub fn git_command(dir: &std::path::Path) -> Command {
    git_command_with("git", dir)
}

/// [`git_command`] with the program given (a test passes a stand-in).
pub fn git_command_with(program: &str, dir: &std::path::Path) -> Command {
    let mut c = Command::new(program);
    c.current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["-c", "user.name=IYAKKI", "-c", "user.email=iyakki@localhost", "-c", "commit.gpgsign=false", "-c", "core.quotepath=off"]);
    c
}

/// `true` when `git --version` runs and ends well.
pub fn git_available() -> bool {
    Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Run `program args...` in `dir` with its output kept (a workspace's own test command); the finished output, or the start error.
pub fn run_in_dir(dir: &std::path::Path, program: &str, args: &[String]) -> std::io::Result<std::process::Output> {
    Command::new(program).args(args).current_dir(dir).output()
}

/// Run `kubectl` with `args` (the leading flags first); `stdin` is written to it when given, otherwise it reads nothing. The finished output, or the
/// start or write error as a string (the caller words it for the person).
pub fn kubectl_run(args: &[String], stdin: Option<&[u8]>) -> Result<std::process::Output, String> {
    kubectl_run_with("kubectl", args, stdin)
}

/// [`kubectl_run`] with the program given (a test passes a stand-in).
pub fn kubectl_run_with(program: &str, args: &[String], stdin: Option<&[u8]>) -> Result<std::process::Output, String> {
    use std::io::Write;
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    if stdin.is_none() {
        return cmd.output().map_err(|e| e.to_string());
    }
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    if let Some(data) = stdin {
        child.stdin.take().ok_or("kubectl: no stdin")?.write_all(data).map_err(|e| e.to_string())?;
    }
    child.wait_with_output().map_err(|e| e.to_string())
}

/// Start `kubectl` with `args` and keep its output pipe for the caller to read line by line (port forwarding); no input, errors dropped.
pub fn kubectl_spawn_lines(args: &[String]) -> std::io::Result<std::process::Child> {
    kubectl_spawn_lines_with("kubectl", args)
}

/// [`kubectl_spawn_lines`] with the program given.
pub fn kubectl_spawn_lines_with(program: &str, args: &[String]) -> std::io::Result<std::process::Child> {
    Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
}

/// `depmod <release>` (the module index of a kernel release): `Ok` when it ended well, otherwise its own words.
pub fn depmod(release: &str) -> Result<(), String> {
    let o = Command::new("depmod").arg(release).output().map_err(|e| format!("depmod: {e}"))?;
    if o.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&o.stderr).trim().to_string()) }
}

/// The text `program --version` prints with a cleared environment, empty when it cannot be run.
pub fn version_text(program: &std::path::Path) -> String {
    Command::new(program).arg("--version").env_clear().output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// Start `program args...` with no input and both its output and its error pipes kept (a probe whose text the caller reads); the child.
pub fn spawn_capture(program: &str, args: &[&str]) -> std::io::Result<std::process::Child> {
    Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
}

/// Start the system's raw-audio player (`argv` as the sound code built it, program first) with its input piped and its output and errors dropped
/// (priming a control, playing a short silence); the child, for the caller to write to and end.
pub fn spawn_raw_player(argv: &[String]) -> std::io::Result<std::process::Child> {
    Command::new(&argv[0]).args(&argv[1..]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
}

/// Start a media player on `url` (`argv` is the program and its leading arguments) in its own process group with every stream dropped; the child.
pub fn spawn_media(argv: &[String], url: &str) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    Command::new(&argv[0]).args(&argv[1..]).arg(url).process_group(0).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
}

/// Run `program args...` and keep its output (the tile emulator beside the program, or on the path); the finished output, or the start error.
pub fn run_program(program: &std::path::Path, args: &[String]) -> std::io::Result<std::process::Output> {
    Command::new(program).args(args).output()
}

/// `true` when `program -version` ran and ended well (a media tool's smoke test), with every stream dropped.
pub fn accepts_dash_version(program: &str) -> bool {
    Command::new(program).arg("-version").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// The text `ffmpeg -hide_banner -encoders` prints with a cleared environment; `None` when it did not run or ended with an error.
pub fn ffmpeg_encoder_list(exe: &std::path::Path) -> Option<String> {
    let out = Command::new(exe).args(["-hide_banner", "-encoders"]).env_clear().stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    if out.status.success() { Some(String::from_utf8_lossy(&out.stdout).into_owned()) } else { None }
}

/// Load a kernel module by name with `modprobe`, nothing kept; a failure is ignored (the caller opens the thing it needed and reports that).
pub fn modprobe_quiet(module: &str) {
    let _ = Command::new("modprobe").arg(module).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// Ask a process to end with `kill -TERM <pid>` (the last resort of a child that did not end by itself).
pub fn terminate(pid: u32) {
    let _ = Command::new("kill").args(["-TERM", &pid.to_string()]).status();
}

/// End a whole process group with `kill -KILL -<pgid>` (the group a child was started leading); the answer is ignored, the group may be gone already.
pub fn kill_group(pgid: u32) {
    let _ = Command::new("kill").args(["-KILL", &format!("-{pgid}")]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

// ---------------------------------------------------------------- F-ARCH-07 step 3, batch A (KI-729): the long-lived and prepared starters

/// `ssh-keyscan`: the host keys a server presents: connects and leaves without logging in (`ssh-keyscan -T 10`), standard input closed.
pub fn ssh_keyscan(program: &std::path::Path, host: &str, port: &str) -> std::io::Result<std::process::Output> {
    Command::new(program).args(["-T", "10", "-p", port, "-t", "ed25519,ecdsa,rsa", "--", host]).stdin(Stdio::null()).output()
}

/// The OpenSSH client set up to learn a server's host key into `known_hosts` and leave (no key offered, no forwarding).
pub fn ssh_probe_openssh_command(client: &std::path::Path, known_hosts: &std::path::Path, port: &str, host: &str) -> Command {
    let mut c = Command::new(client);
    c.args(["-F", "/dev/null", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=accept-new", "-o", "HashKnownHosts=no", "-o", "GlobalKnownHostsFile=/dev/null", "-o", "ConnectTimeout=10", "-o", "IdentityFile=/dev/null", "-o", "IdentitiesOnly=yes", "-o", "PreferredAuthentications=publickey", "-o", "ClearAllForwardings=yes", "-o"]);
    c.arg(format!("UserKnownHostsFile={}", known_hosts.display())).args(["-p", port, "--", host, "true"]);
    c
}

/// Dropbear's client set up the same way (`-y` accepts the key into the throw-away home).
pub fn ssh_probe_dropbear_command(client: &std::path::Path, port: &str, host: &str) -> Command {
    let mut c = Command::new(client);
    c.args(["-y", "-o", "BatchMode=yes", "-p", port, host, "true"]);
    c
}

/// `ssh-keygen -p -N ""` over a stored key: the passphrase reaches it only through the environment and an askpass script.
pub fn ssh_keygen_unlock_command(keygen: &std::path::Path, key: &std::path::Path, askpass: &std::path::Path, passphrase: &str, path_env: &str) -> Command {
    let mut c = Command::new(keygen);
    c.args(["-p", "-N", "", "-f"]).arg(key).env_clear().env("SSH_ASKPASS", askpass).env("SSH_ASKPASS_REQUIRE", "force").env("IYAKKI_SSH_PASSPHRASE", passphrase).env("PATH", path_env);
    c
}

/// `ssh-keygen -t ed25519` for a new key without passphrase, in a throw-away home.
pub fn ssh_keygen_new_command(keygen: &std::path::Path, comment: &str, file: &std::path::Path, path_env: &str, home: &std::path::Path) -> Command {
    let mut c = Command::new(keygen);
    c.args(["-q", "-t", "ed25519", "-N", "", "-C", comment, "-f"]).arg(file).env_clear().env("PATH", path_env).env("HOME", home);
    c
}

/// `dropbearkey -t ed25519` (Dropbear images).
pub fn dropbearkey_new_command(program: &std::path::Path, comment: &str, file: &std::path::Path, path_env: &str) -> Command {
    let mut c = Command::new(program);
    c.args(["-t", "ed25519", "-C", comment, "-f"]).arg(file).env_clear().env("PATH", path_env);
    c
}

/// `ssh`, `scp` or `sftp` for the person: the argument list, the folder, and exactly the environment given (nothing else is passed on).
pub fn ssh_session_command(program: &std::path::Path, argv: &[String], cwd: &std::path::Path, envs: &[(String, String)]) -> Command {
    let mut c = Command::new(program);
    c.args(argv).current_dir(cwd).env_clear().envs(envs.iter().map(|(k, v)| (k, v)));
    c
}

/// One udev step (`udevd --daemon`, `udevadm trigger|settle`): no input, output appended to the log file when there is one; waits for the program.
pub fn udev_run(program: impl AsRef<std::ffi::OsStr>, args: &[&str], log: Option<&std::fs::File>) -> std::io::Result<std::process::ExitStatus> {
    let mut c = Command::new(program);
    c.args(args).stdin(Stdio::null());
    match log.and_then(|f| f.try_clone().ok()) {
        Some(f) => c.stdout(f),
        None => c.stdout(Stdio::null()),
    };
    match log.and_then(|f| f.try_clone().ok()) {
        Some(f) => c.stderr(f),
        None => c.stderr(Stdio::null()),
    };
    c.status()
}

/// The legacy web engine's compositor: own process group, cleared environment plus `envs`, in the profile folder, output to `log`.
pub fn webengine_spawn(argv: &[String], envs: &[(String, String)], cwd: &std::path::Path, log: std::fs::File) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    let (prog, rest) = argv.split_first().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "an empty command"))?;
    let err = log.try_clone()?;
    let mut c = Command::new(prog);
    c.args(rest).env_clear().envs(envs.iter().map(|(k, v)| (k, v))).current_dir(cwd);
    c.process_group(0).stdin(Stdio::null()).stdout(log).stderr(err);
    c.spawn()
}

/// `ps -o stat=,lstart= -p <pid>` (the process-state fallback where there is no `/proc`): the text, or None.
pub fn ps_state_text(pid: u32) -> Option<String> {
    ps_text_with("ps", &["-o", "stat=,lstart=", "-p", &pid.to_string()])
}

/// `ps -o comm= -p <pid>`: the program name text, or None.
pub fn ps_name_text(pid: u32) -> Option<String> {
    ps_text_with("ps", &["-o", "comm=", "-p", &pid.to_string()])
}

/// The text a `ps`-like program printed when it succeeded.
pub fn ps_text_with(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A detached job: `sh -c <wrapper> sh <exit file> <command...>` in its own session and folder, output to the two files, environment cleared plus `envs`.
pub fn job_spawn(wrapper: &str, exit_file: &std::path::Path, command: &[String], cwd: &std::path::Path, envs: &[(String, String)], stdout: std::fs::File, stderr: std::fs::File) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    let mut c = Command::new("sh");
    c.arg("-c").arg(wrapper).arg("sh").arg(exit_file).args(command);
    c.current_dir(cwd).stdin(Stdio::null()).stdout(stdout).stderr(stderr).env_clear().envs(envs.iter().map(|(k, v)| (k, v)));
    // SAFETY: setsid is async-signal-safe and the only thing done between fork and exec.
    unsafe {
        c.pre_exec(|| if libc::setsid() < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) });
    }
    c.spawn()
}

/// A program run for its exit status only (standard streams inherited), such as IYAKKI's own `dev files push`.
pub fn run_status(program: &std::path::Path, args: &[String]) -> std::io::Result<std::process::ExitStatus> {
    Command::new(program).args(args).status()
}

/// A network-setup tool (`ip`, `modprobe`) with a cleared environment and fixed `PATH`; stdin text when given. Returns its stdout, or a sentence.
pub fn sys_tool_run(path: &std::path::Path, name: &str, args: &[&str], stdin: Option<&str>) -> Result<String, String> {
    use std::io::Write;
    let mut c = Command::new(path);
    c.args(args).env_clear().env("PATH", "/usr/sbin:/sbin:/usr/bin:/bin").stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut child = c.spawn().map_err(|e| format!("could not start {name}: {e}"))?;
    if let (Some(text), Some(mut si)) = (stdin, child.stdin.take()) {
        si.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("{name} {}: {}", args.first().copied().unwrap_or(""), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// The OpenVPN program the person downloaded, as a child with a cleared environment and both output pipes the caller reads.
pub fn openvpn_spawn(binary: &std::path::Path, args: &[String]) -> std::io::Result<std::process::Child> {
    Command::new(binary).args(args).env_clear().stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
}

/// The command for a sound player or recorder: own process group, cleared environment plus `envs`, optionally in a folder.
pub fn audio_command(argv: &[String], envs: &[(String, String)], cwd: Option<&std::path::Path>) -> Command {
    use std::os::unix::process::CommandExt;
    let mut c = Command::new(&argv[0]);
    c.process_group(0);
    c.args(&argv[1..]).env_clear().envs(envs.iter().map(|(k, v)| (k, v)));
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    c
}

/// ffmpeg turning a file into a stream for the DLNA server: cleared environment, no input, errors dropped, output pipe read by the caller.
pub fn ffmpeg_stream_spawn(exe: &std::path::Path, args: &[String]) -> std::io::Result<std::process::Child> {
    Command::new(exe).args(args).env_clear().stdin(Stdio::null()).stderr(Stdio::null()).stdout(Stdio::piped()).spawn()
}

/// The player of the framebuffer tile: a configured process that takes (or does not take) standard input, own process group; the caller spawns it with its own retry.
pub fn player_command(program: &str, args: &[String], stdin_pipe: bool) -> Command {
    use std::os::unix::process::CommandExt;
    let mut c = Command::new(program);
    c.args(args).process_group(0).stdin(if stdin_pipe { Stdio::piped() } else { Stdio::null() }).stdout(if stdin_pipe { Stdio::null() } else { Stdio::piped() }).stderr(Stdio::piped());
    c
}

/// `ip -4 route show default dev <client>`: its text, or None.
pub fn ip_route_show_default(client: &str) -> Option<String> {
    ip_route_show_default_with("ip", client)
}

pub fn ip_route_show_default_with(program: &str, client: &str) -> Option<String> {
    let out = Command::new(program).args(["-4", "route", "show", "default", "dev", client]).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// `ip -4 route <args>`: true when it succeeded.
pub fn ip_route(args: &[&str]) -> bool {
    ip_route_with("ip", args)
}

pub fn ip_route_with(program: &str, args: &[&str]) -> bool {
    Command::new(program).arg("-4").arg("route").args(args).status().is_ok_and(|s| s.success())
}

/// Tests that start stand-in scripts take this lock for their whole body: a program another test just wrote can be "text file busy" while a
/// forked child of a parallel test still holds it open, and one test at a time removes the race (the callers retry on it in production).
#[cfg(test)]
pub(crate) fn spawn_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn script(tag: &str, body: &str) -> String {
        let d = std::env::temp_dir().join(format!("iyakki-starters-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("fake");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.display().to_string()
    }

    #[test]
    fn a_mixer_that_answers_gives_its_text_one_that_refuses_gives_its_error_and_a_missing_one_is_not_started() {
        let _serial = crate::starters::spawn_lock();
        let args = vec!["sget".to_string(), "Master".to_string()];
        assert_eq!(mixer_text_with(&script("ok", "echo \"$@\""), &args).unwrap().trim(), "sget Master");
        let e = mixer_text_with(&script("no", "echo ' no such control ' >&2; exit 1"), &args).unwrap_err();
        assert_eq!(e, AmixerFailure { not_started: false, text: "no such control".into() });
        assert!(mixer_text_with("/nonexistent/amixer", &args).unwrap_err().not_started);
    }

    #[test]
    fn the_quiet_form_says_ok_or_a_sentence() {
        let _serial = crate::starters::spawn_lock();
        assert!(amixer_quiet_with(&script("q-ok", "exit 0"), &[]).is_ok());
        assert!(amixer_quiet_with(&script("q-no", "exit 3"), &[]).unwrap_err().starts_with("amixer ended with "));
        assert!(amixer_quiet_with("/nonexistent/amixer", &[]).unwrap_err().starts_with("amixer: "));
    }

    #[test]
    fn git_status_gives_the_text_of_a_working_program_and_none_for_a_failing_or_missing_one() {
        let _serial = crate::starters::spawn_lock();
        let d = std::env::temp_dir();
        assert_eq!(git_status_porcelain_with(&script("g-ok", "echo ' M a.rs'"), &d).unwrap().trim(), "M a.rs");
        assert!(git_status_porcelain_with(&script("g-no", "exit 128"), &d).is_none());
        assert!(git_status_porcelain_with("/nonexistent/git", &d).is_none());
    }

    #[test]
    fn a_gst_tool_gets_its_own_registry_and_a_missing_one_is_an_error() {
        let _serial = crate::starters::spawn_lock();
        let reg = std::path::Path::new("/some/registry.bin");
        let o = gst_run(&script("gst-env", "echo \"$GST_REGISTRY_1_0 $@\""), &["x".into()], reg).unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "/some/registry.bin x");
        assert!(gst_ok(&script("gst-ok", "exit 0"), &[], reg));
        assert!(!gst_ok(&script("gst-no", "exit 1"), &[], reg));
        assert!(!gst_ok("/nonexistent/gst", &[], reg));
    }

    #[test]
    fn the_extractor_runs_in_its_folder_with_a_cleared_environment() {
        let _serial = crate::starters::spawn_lock();
        let d = std::env::temp_dir().join(format!("iyakki-extractor-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let o = run_extractor(std::path::Path::new(&script("ex", "echo \"$HOME|$PATH|$(pwd)|${IYAKKI_NOT_PASSED:-none}\"")), &[], &d).unwrap();
        let line = String::from_utf8_lossy(&o.stdout).trim().to_string();
        assert!(line.starts_with(&format!("{}|/usr/bin:/bin|", d.display())) && line.ends_with("|none"), "{line}");
    }

    #[test]
    fn the_browser_opener_starts_and_a_missing_one_is_an_error() {
        let _serial = crate::starters::spawn_lock();
        let mut c = open_detached(std::path::Path::new(&script("open", "exit 0")), &["https://example.org".into()]).unwrap();
        assert!(c.wait().unwrap().success());
        assert!(open_detached(std::path::Path::new("/nonexistent/open"), &[]).is_err());
    }

    #[test]
    fn the_shell_line_writes_to_the_given_files_and_runs_in_the_folder_when_there_is_one() {
        let _serial = crate::starters::spawn_lock();
        let d = std::env::temp_dir().join(format!("iyakki-shellline-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let log = d.join("out.log");
        let f = std::fs::File::create(&log).unwrap();
        let mut c = shell_line_spawn("pwd; echo oops >&2; exit 3", d.to_str().unwrap(), f.try_clone().unwrap(), f).unwrap();
        assert_eq!(c.wait().unwrap().code(), Some(3));
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("iyakki-shellline") && text.contains("oops"), "{text}");
    }

    #[test]
    fn the_git_command_is_quiet_and_signed_off_and_a_test_command_runs_in_its_folder() {
        let _serial = crate::starters::spawn_lock();
        let d = std::env::temp_dir();
        let o = git_command_with(&script("gitcmd", "echo \"$GIT_TERMINAL_PROMPT|$GIT_EDITOR|$GIT_OPTIONAL_LOCKS|${GIT_DIR:-unset}|$*\""), &d).arg("status").output().unwrap();
        let line = String::from_utf8_lossy(&o.stdout).trim().to_string();
        assert!(line.starts_with("0|true|0|") && line.contains("user.name=IYAKKI") && line.contains("commit.gpgsign=false") && line.ends_with("status"), "{line}");
        let o = run_in_dir(&d, &script("indir", "pwd; echo \"$1\""), &["arg".into()]).unwrap();
        assert!(String::from_utf8_lossy(&o.stdout).ends_with("arg\n"));
        assert!(run_in_dir(&d, "/nonexistent/prog", &[]).is_err());
    }

    #[test]
    fn kubectl_gets_its_input_when_given_and_a_missing_program_is_an_error() {
        let _serial = crate::starters::spawn_lock();
        let a = vec!["apply".to_string(), "-f".to_string(), "-".to_string()];
        let o = kubectl_run_with(&script("kc-in", "cat; echo \"|$*\""), &a, Some(b"manifest")).unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout), "manifest|apply -f -\n");
        let o = kubectl_run_with(&script("kc-no", "echo bad >&2; exit 1"), &a, None).unwrap();
        assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).trim() == "bad");
        assert!(kubectl_run_with("/nonexistent/kubectl", &a, None).is_err());
        assert!(kubectl_run_with("/nonexistent/kubectl", &a, Some(b"x")).is_err());
    }

    #[test]
    fn the_forwarding_child_has_its_output_pipe_and_a_missing_program_is_an_error() {
        let _serial = crate::starters::spawn_lock();
        let mut c = kubectl_spawn_lines_with(&script("kc-fw", "echo Forwarding from 127.0.0.1:1"), &[]).unwrap();
        let mut text = String::new();
        std::io::Read::read_to_string(c.stdout.as_mut().unwrap(), &mut text).unwrap();
        c.wait().unwrap();
        assert!(text.starts_with("Forwarding"));
        assert!(kubectl_spawn_lines_with("/nonexistent/kubectl", &[]).is_err());
    }

    #[test]
    fn the_version_text_comes_from_a_cleared_environment_and_is_empty_for_a_missing_program() {
        let _serial = crate::starters::spawn_lock();
        assert_eq!(version_text(std::path::Path::new(&script("ver", "echo \"git version 2.1 ${HOME:-nohome}\""))).trim(), "git version 2.1 nohome");
        assert_eq!(version_text(std::path::Path::new("/nonexistent/git")), "");
    }

    #[test]
    fn the_probe_the_raw_player_and_the_media_player_start_what_they_are_given() {
        let _serial = crate::starters::spawn_lock();
        let mut c = spawn_capture(&script("cap", "echo out; echo err >&2"), &[]).unwrap();
        let (mut so, mut se) = (String::new(), String::new());
        std::io::Read::read_to_string(c.stdout.as_mut().unwrap(), &mut so).unwrap();
        std::io::Read::read_to_string(c.stderr.as_mut().unwrap(), &mut se).unwrap();
        assert_eq!((so.trim(), se.trim()), ("out", "err"));
        assert!(spawn_capture("/nonexistent/probe", &[]).is_err());

        let mut p = spawn_raw_player(&[script("raw", "cat >/dev/null"), "-q".into()]).unwrap();
        std::io::Write::write_all(p.stdin.as_mut().unwrap(), &[0u8; 64]).unwrap();
        drop(p.stdin.take());
        assert!(p.wait().unwrap().success());
        assert!(spawn_raw_player(&["/nonexistent/aplay".into()]).is_err());

        let mut m = spawn_media(&[script("media", "test \"$1\" = https://example.org/a")], "https://example.org/a").unwrap();
        assert!(m.wait().unwrap().success());
        assert!(spawn_media(&["/nonexistent/player".into()], "x").is_err());
    }

    #[test]
    fn a_program_is_run_for_its_output_and_the_version_and_encoder_probes_say_yes_or_no() {
        let _serial = crate::starters::spawn_lock();
        let o = run_program(std::path::Path::new(&script("run", "echo \"$@\"")), &["a".into(), "b".into()]).unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "a b");
        assert!(run_program(std::path::Path::new("/nonexistent/emulator"), &[]).is_err());
        assert!(accepts_dash_version(&script("v-ok", "test \"$1\" = -version")));
        assert!(!accepts_dash_version(&script("v-no", "exit 1")));
        assert!(!accepts_dash_version("/nonexistent/ffmpeg"));
        assert_eq!(ffmpeg_encoder_list(std::path::Path::new(&script("enc", "echo ' V....D mpeg2video'"))).unwrap().trim(), "V....D mpeg2video");
        assert!(ffmpeg_encoder_list(std::path::Path::new(&script("enc-no", "exit 2"))).is_none());
        assert!(ffmpeg_encoder_list(std::path::Path::new("/nonexistent/ffmpeg")).is_none());
    }

    #[test]
    fn a_whole_group_ends_with_its_leader_and_a_missing_group_is_ignored() {
        let _serial = crate::starters::spawn_lock();
        use std::os::unix::process::CommandExt;
        let mut c = Command::new("sleep").arg("30").process_group(0).spawn().unwrap();
        kill_group(c.id());
        assert!(!c.wait().unwrap().success());
        kill_group(0x7fff_fffe);
    }

    #[test]
    fn terminate_ends_a_child_and_a_missing_module_is_ignored() {
        let _serial = crate::starters::spawn_lock();
        let mut c = Command::new("sleep").arg("30").spawn().unwrap();
        terminate(c.id());
        let st = c.wait().unwrap();
        assert!(!st.success());
        modprobe_quiet("iyakki_no_such_module_for_a_test");
    }

    fn lines(o: &std::process::Output) -> String {
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    #[test]
    fn the_ssh_starters_build_the_argument_lists_and_environment_the_agent_relies_on() {
        let _serial = crate::starters::spawn_lock();
        let p = |s: &str| std::path::PathBuf::from(s);
        let o = ssh_keyscan(&p(&script("scan", "echo \"$@\"")), "h.example", "2222").unwrap();
        assert_eq!(lines(&o), "-T 10 -p 2222 -t ed25519,ecdsa,rsa -- h.example");
        assert!(ssh_keyscan(&p("/nonexistent/ssh-keyscan"), "h", "22").is_err());

        let o = ssh_probe_openssh_command(&p(&script("po", "echo \"$@\"")), &p("/t/kh"), "22", "h").output().unwrap();
        let l = lines(&o);
        assert!(l.contains("StrictHostKeyChecking=accept-new") && l.contains("ClearAllForwardings=yes") && l.contains("UserKnownHostsFile=/t/kh") && l.ends_with("-p 22 -- h true"), "{l}");
        let o = ssh_probe_dropbear_command(&p(&script("pd", "echo \"$@\"")), "22", "h").output().unwrap();
        assert_eq!(lines(&o), "-y -o BatchMode=yes -p 22 h true");

        let o = ssh_keygen_unlock_command(&p(&script("ku", "echo \"$*|$SSH_ASKPASS|$SSH_ASKPASS_REQUIRE|$IYAKKI_SSH_PASSPHRASE|$PATH|${HOME:-nohome}\"")), &p("/k/key"), &p("/k/askpass"), "pw", "/bin:/usr/bin").output().unwrap();
        assert_eq!(lines(&o), "-p -N  -f /k/key|/k/askpass|force|pw|/bin:/usr/bin|nohome");
        let o = ssh_keygen_new_command(&p(&script("kn", "echo \"$*|$HOME|$PATH\"")), "me@x", &p("/k/f"), "/bin", &p("/k/home")).output().unwrap();
        assert_eq!(lines(&o), "-q -t ed25519 -N  -C me@x -f /k/f|/k/home|/bin");
        let o = dropbearkey_new_command(&p(&script("dk", "echo \"$*|$PATH\"")), "c", &p("/k/f"), "/bin").output().unwrap();
        assert_eq!(lines(&o), "-t ed25519 -C c -f /k/f|/bin");
    }

    #[test]
    fn an_ssh_session_gets_only_the_environment_it_is_given_in_the_folder_it_is_given() {
        let _serial = crate::starters::spawn_lock();
        let d = std::env::temp_dir();
        let envs = vec![("HOME".to_string(), "/the/home".to_string()), ("TERM".to_string(), "xterm".to_string())];
        let o = ssh_session_command(std::path::Path::new(&script("sess", "echo \"$*|$HOME|$TERM|${USER:-nouser}|$(pwd)\"")), &["-p".into(), "22".into()], &d, &envs).output().unwrap();
        let l = lines(&o);
        assert!(l.starts_with("-p 22|/the/home|xterm|nouser|"), "{l}");
        assert!(ssh_session_command(std::path::Path::new("/nonexistent/ssh"), &[], &d, &[]).output().is_err());
    }

    #[test]
    fn a_udev_step_writes_to_the_log_or_nowhere_and_reports_how_it_ended() {
        let _serial = crate::starters::spawn_lock();
        let d = std::env::temp_dir().join(format!("iyakki-udev-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let log = std::fs::File::create(d.join("log")).unwrap();
        let st = udev_run(std::path::Path::new(&script("udev", "echo \"$@\"; echo e >&2; exit 4")), &["trigger", "--action=add"], Some(&log)).unwrap();
        assert_eq!(st.code(), Some(4));
        assert_eq!(std::fs::read_to_string(d.join("log")).unwrap(), "trigger --action=add\ne\n");
        assert!(udev_run(std::path::Path::new(&script("udev2", "exit 0")), &[], None).unwrap().success());
        assert!(udev_run(std::path::Path::new("/nonexistent/udevd"), &[], None).is_err());
    }

    #[test]
    fn the_web_engine_runs_in_its_own_group_with_its_environment_and_log() {
        let _serial = crate::starters::spawn_lock();
        let d = std::env::temp_dir().join(format!("iyakki-web-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let log = std::fs::File::create(d.join("log")).unwrap();
        let envs = vec![("XDG_RUNTIME_DIR".to_string(), "/run/x".to_string())];
        let mut c = webengine_spawn(&[script("cage", "echo \"$1|$XDG_RUNTIME_DIR|${HOME:-nohome}|$(pwd)|$(ps -o pgid= -p $$ | tr -d ' ')|$$\"; echo err >&2"), "-s".into()], &envs, &d, log).unwrap();
        let pid = c.id();
        assert!(c.wait().unwrap().success());
        let text = std::fs::read_to_string(d.join("log")).unwrap();
        assert!(text.contains("-s|/run/x|nohome|") && text.contains(&format!("|{pid}|{pid}")) && text.contains("err"), "{text}");
        let log = std::fs::File::create(d.join("log2")).unwrap();
        assert!(webengine_spawn(&[], &[], &d, log).is_err());
    }

    #[test]
    fn the_ps_readers_give_text_for_a_working_program_and_none_otherwise() {
        let _serial = crate::starters::spawn_lock();
        assert_eq!(ps_text_with(&script("ps", "echo \"S Mon Jan 1 $*\""), &["-p", "7"]).unwrap().trim(), "S Mon Jan 1 -p 7");
        assert!(ps_text_with(&script("ps-no", "exit 1"), &[]).is_none());
        assert!(ps_text_with("/nonexistent/ps", &[]).is_none());
        // the real one: our own process exists, a huge pid does not
        #[cfg(unix)]
        {
            assert!(ps_name_text(std::process::id()).is_some_and(|n| !n.trim().is_empty()));
            assert!(ps_state_text(std::process::id()).is_some());
            assert!(ps_state_text(u32::MAX - 1).is_none());
        }
    }

    #[test]
    fn a_job_runs_in_its_own_session_and_folder_with_the_given_environment_and_files() {
        let _serial = crate::starters::spawn_lock();
        let d = std::env::temp_dir().join(format!("iyakki-job-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let (out, err) = (std::fs::File::create(d.join("out")).unwrap(), std::fs::File::create(d.join("err")).unwrap());
        let wrap = "f=\"$1\"; shift; \"$@\"; c=$?; printf '%s' \"$c\" > \"$f\"; exit \"$c\"";
        let cmd: Vec<String> = ["sh", "-c", "echo \"$IYAKKI_JOB_ID|${HOME:-nohome}|$(pwd)\"; echo oops >&2; exit 5"].iter().map(|s| s.to_string()).collect();
        let envs = vec![("IYAKKI_JOB_ID".to_string(), "j1".to_string())];
        let mut c = job_spawn(wrap, &d.join("exit"), &cmd, &d, &envs, out, err).unwrap();
        assert_eq!(c.wait().unwrap().code(), Some(5));
        assert_eq!(std::fs::read_to_string(d.join("exit")).unwrap(), "5");
        assert!(std::fs::read_to_string(d.join("out")).unwrap().starts_with("j1|nohome|") && std::fs::read_to_string(d.join("err")).unwrap().contains("oops"));
    }

    #[test]
    fn a_status_only_run_a_system_tool_and_the_vpn_child_report_what_happened() {
        let _serial = crate::starters::spawn_lock();
        let p = |s: &str| std::path::PathBuf::from(s);
        assert_eq!(run_status(&p(&script("st", "exit 3")), &[]).unwrap().code(), Some(3));
        assert!(run_status(&p("/nonexistent/iyakki"), &[]).is_err());

        let ok = sys_tool_run(&p(&script("sys", "cat; echo \"|$PATH|${HOME:-nohome}|$*\"")), "ip", &["link", "add"], Some("in")).unwrap();
        assert_eq!(ok, "in|/usr/sbin:/sbin:/usr/bin:/bin|nohome|link add\n");
        assert_eq!(sys_tool_run(&p(&script("sys-no", "echo ' bad ' >&2; exit 2")), "ip", &["addr", "x"], None).unwrap_err(), "ip addr: bad");
        assert!(sys_tool_run(&p("/nonexistent/ip"), "ip", &[], None).unwrap_err().starts_with("could not start ip"));

        let mut c = openvpn_spawn(&p(&script("ovpn", "echo \"$*|${HOME:-nohome}\"; echo e >&2")), &["--config".into(), "x".into()]).unwrap();
        let (mut so, mut se) = (String::new(), String::new());
        std::io::Read::read_to_string(c.stdout.as_mut().unwrap(), &mut so).unwrap();
        std::io::Read::read_to_string(c.stderr.as_mut().unwrap(), &mut se).unwrap();
        assert_eq!((so.trim(), se.trim()), ("--config x|nohome", "e"));
        assert!(openvpn_spawn(&p("/nonexistent/openvpn"), &[]).is_err());
    }

    #[test]
    fn the_audio_stream_and_player_commands_set_up_groups_environment_and_pipes() {
        let _serial = crate::starters::spawn_lock();
        let d = std::env::temp_dir();
        let envs = vec![("LANG".to_string(), "C.UTF-8".to_string())];
        let o = audio_command(&[script("aud", "echo \"$*|$LANG|${HOME:-nohome}|$(pwd)\""), "-q".into()], &envs, Some(&d)).output().unwrap();
        assert!(lines(&o).starts_with("-q|C.UTF-8|nohome|"));

        let mut c = ffmpeg_stream_spawn(std::path::Path::new(&script("ff", "echo \"${HOME:-nohome} $*\"; echo hidden >&2")), &["-i".into(), "a".into()]).unwrap();
        let mut so = String::new();
        std::io::Read::read_to_string(c.stdout.as_mut().unwrap(), &mut so).unwrap();
        c.wait().unwrap();
        assert_eq!(so.trim(), "nohome -i a");
        assert!(c.stderr.is_none());
        assert!(ffmpeg_stream_spawn(std::path::Path::new("/nonexistent/ffmpeg"), &[]).is_err());

        let mut c = player_command(&script("pl", "cat >/dev/null; echo done >&2"), &[], true).spawn().unwrap();
        assert!(c.stdout.is_none() && c.stdin.is_some());
        drop(c.stdin.take());
        let mut se = String::new();
        std::io::Read::read_to_string(c.stderr.as_mut().unwrap(), &mut se).unwrap();
        assert_eq!(se.trim(), "done");
        let mut c = player_command(&script("pl2", "echo frame"), &[], false).spawn().unwrap();
        let mut so = String::new();
        std::io::Read::read_to_string(c.stdout.as_mut().unwrap(), &mut so).unwrap();
        assert_eq!(so.trim(), "frame");
        assert!(c.stdin.is_none());
    }

    #[test]
    fn the_route_starters_name_the_table_and_report_success() {
        let _serial = crate::starters::spawn_lock();
        assert_eq!(ip_route_show_default_with(&script("rs", "echo \"$*\""), "wg0").unwrap().trim(), "-4 route show default dev wg0");
        assert!(ip_route_show_default_with("/nonexistent/ip", "wg0").is_none());
        assert!(ip_route_with(&script("ra", "test \"$1 $2 $3\" = '-4 route add'"), &["add", "x"]));
        assert!(!ip_route_with(&script("rn", "exit 1"), &[]));
        assert!(!ip_route_with("/nonexistent/ip", &[]));
    }
}
