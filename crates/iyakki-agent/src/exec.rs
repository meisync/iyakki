//! Running a few cloud and remote-access programs for the assistant: `ssh`, `scp`, `sftp`, `aws` and `gcloud` (the three SSH
//! programs go through [`crate::ssh`], which checks every option and keeps the host keys and private keys). No shell is involved
//! (the program and each argument are passed separately, so nothing in an argument can chain a second command), only
//! these programs are allowed, the environment is cut down to what they need to find the person's own credentials,
//! the run is time-limited and its output capped. Every call is shown to the person for approval before it runs.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The programs the assistant may start.
pub const PROGRAMS: [&str; 5] = ["ssh", "scp", "sftp", "aws", "gcloud"];
const MAX_ARGS: usize = 64;
const MAX_ARG_LEN: usize = 4096;
const MAX_OUTPUT: usize = 64 * 1024;
pub const DEFAULT_SECONDS: u64 = 60;
pub const MAX_SECONDS: u64 = 300;

/// Environment variables that are passed on: where to find programs and the person's own login for each service.
fn passes(name: &str) -> bool {
    matches!(name, "PATH" | "LANG" | "LC_ALL" | "TERM" | "TZ" | "SSH_AUTH_SOCK" | "HTTPS_PROXY" | "HTTP_PROXY" | "NO_PROXY" | "https_proxy" | "http_proxy" | "no_proxy" | "GOOGLE_APPLICATION_CREDENTIALS")
        || name.starts_with("AWS_")
        || name.starts_with("CLOUDSDK_")
        || name.starts_with("AZURE_")
        || name.starts_with("ARM_")
        || name.starts_with("KUBE")
        || name.starts_with("HELM_")
        || name.starts_with("TF_")
        || name.starts_with("K3S_")
        || matches!(name, "GH_TOKEN" | "GITHUB_TOKEN" | "GH_HOST")
}

/// A program installed from the CLI manager: its name and the host its network policy check uses
/// (`None` = it can talk to any host, so only an account allowed every host may run it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extra {
    pub name: String,
    pub host: Option<String>,
}

/// Checks a request and says which host name the person's network policy must allow for it.
pub fn check(program: &str, args: &[String]) -> Result<String, String> {
    check_with(&[], program, args)
}

pub fn check_with(extra: &[Extra], program: &str, args: &[String]) -> Result<String, String> {
    let extra_hit = extra.iter().find(|x| x.name == program);
    if !PROGRAMS.contains(&program) && extra_hit.is_none() {
        let mut all: Vec<&str> = PROGRAMS.to_vec();
        all.extend(extra.iter().map(|x| x.name.as_str()));
        return Err(format!("`{program}` is not available; the assistant may run: {}", all.join(", ")));
    }
    if args.len() > MAX_ARGS || args.iter().any(|a| a.len() > MAX_ARG_LEN || a.contains('\0')) {
        return Err(format!("at most {MAX_ARGS} arguments of {MAX_ARG_LEN} characters"));
    }
    if let Some(x) = extra_hit {
        return Ok(x.host.clone().unwrap_or_else(|| "*".into()));
    }
    Ok(match program {
        "ssh" | "scp" | "sftp" => crate::ssh::plan(program, args, crate::ssh::Mode::Person)?.host,
        "aws" => "amazonaws.com".into(),
        _ => "googleapis.com".into(),
    })
}

/// What will be run, as the person reads it in the approval prompt.
pub fn command_line(program: &str, args: &[String]) -> String {
    let q = |a: &String| if a.is_empty() || a.contains(|c: char| c.is_whitespace() || "\"'\\$`".contains(c)) { format!("'{}'", a.replace('\'', "'\\''")) } else { a.clone() };
    std::iter::once(program.to_string()).chain(args.iter().map(q)).collect::<Vec<_>>().join(" ")
}

/// Told about output as it arrives (the CLI tile shows a long run while it is still going).
pub type Sink = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

fn read_capped(mut r: impl Read + Send + 'static, sink: Option<Sink>) -> std::sync::mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Some(f) = &sink {
                        f(&String::from_utf8_lossy(&iyakki_proxy::sanitize(&buf[..n])));
                    }
                    if out.len() < MAX_OUTPUT {
                        out.extend_from_slice(&buf[..n.min(MAX_OUTPUT - out.len())]);
                    }
                }
            }
        }
        let _ = tx.send(out);
    });
    rx
}

/// Runs the program and returns what it printed with its exit status.
pub fn run(program: &str, args: &[String], cwd: &Path, home: Option<&Path>, seconds: u64) -> Result<String, String> {
    run_with(&[], program, args, cwd, home, seconds, None)
}

/// Like [`run`], also allowing the `extra` programs and putting `path_prefix` (the folder of installed tools) first
/// on the program search path.
pub fn run_with(extra: &[Extra], program: &str, args: &[String], cwd: &Path, home: Option<&Path>, seconds: u64, path_prefix: Option<&Path>) -> Result<String, String> {
    run_with_env(extra, program, args, cwd, home, seconds, path_prefix, &[])
}

/// Like [`run_with`], with some environment variables set for the program (such as the kubeconfig of the container
/// cluster).
#[allow(clippy::too_many_arguments)]
pub fn run_with_env(extra: &[Extra], program: &str, args: &[String], cwd: &Path, home: Option<&Path>, seconds: u64, path_prefix: Option<&Path>, env: &[(String, String)]) -> Result<String, String> {
    check_with(extra, program, args)?;
    if matches!(program, "ssh" | "scp" | "sftp") {
        return Err(format!("`{program}` runs through the SSH client with the person's keys and host keys; it cannot be started from here"));
    }
    if !cwd.is_dir() {
        return Err(crate::scope::explain_io(&std::io::Error::from_raw_os_error(if cwd.exists() { 20 } else { 2 }), cwd, "run a command in"));
    }
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(cwd).env_clear().stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // an explicit kubeconfig replaces whatever KUBE* the process inherited (a stale KUBECONTEXT must not steer the command elsewhere)
    let pinned = env.iter().any(|(k, _)| k == "KUBECONFIG");
    for (k, v) in std::env::vars() {
        if passes(&k) && !(pinned && k.starts_with("KUBE")) {
            cmd.env(k, v);
        }
    }
    if let Some(p) = path_prefix {
        cmd.env("PATH", format!("{}:{}", p.display(), std::env::var("PATH").unwrap_or_default()));
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    // a managed dev cluster is the default target of kubectl, k3s and helm in every program IYAKKI starts (no ~/.kube/config needed)
    if !pinned && !std::env::vars().any(|(k, _)| k == "KUBECONFIG") {
        if let Some(m) = crate::devcluster::machine_kubeconfig().filter(|m| m.is_file()) {
            cmd.env("KUBECONFIG", m);
        }
    }
    if let Some(h) = home {
        cmd.env("HOME", h);
    } else if let Ok(h) = std::env::var("HOME") {
        cmd.env("HOME", h);
    }
    run_prepared(cmd, program, seconds, None)
}

/// Starts a command that is already set up, waits for it for `seconds`, and returns what it printed with its exit status
/// (the output is capped; `sink` sees it as it arrives).
pub fn run_prepared(mut cmd: Command, program: &str, seconds: u64, sink: Option<Sink>) -> Result<String, String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut spawned = None;
    for attempt in 0..8 {
        match cmd.spawn() {
            Ok(c) => {
                spawned = Some(c);
                break;
            }
            // a program written a moment ago can be busy while another thread's fork still holds the file
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempt < 7 => std::thread::sleep(Duration::from_millis(50)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(format!("`{program}` is not installed on this machine")),
            Err(e) => return Err(e.to_string()),
        }
    }
    let mut child = spawned.ok_or_else(|| format!("could not start `{program}`"))?;
    let out = read_capped(child.stdout.take().ok_or("no output pipe")?, sink.clone());
    let err = read_capped(child.stderr.take().ok_or("no error pipe")?, sink);
    let limit = Duration::from_secs(seconds.clamp(1, MAX_SECONDS));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    };
    // a program a stopped one started may still hold the pipes open; do not wait for them
    let wait = Duration::from_millis(if status.is_some() { 5000 } else { 300 });
    let (o, e) = (out.recv_timeout(wait).unwrap_or_default(), err.recv_timeout(wait).unwrap_or_default());
    let mut text = String::from_utf8_lossy(&o).into_owned();
    if !e.is_empty() {
        text.push_str(&format!("{}[stderr]\n{}", if text.is_empty() || text.ends_with('\n') { "" } else { "\n" }, String::from_utf8_lossy(&e)));
    }
    let text = String::from_utf8_lossy(&iyakki_proxy::sanitize(text.as_bytes())).into_owned();
    match status {
        None => Err(format!("stopped after {} seconds\n{text}", limit.as_secs())),
        Some(s) if s.success() => Ok(if text.trim().is_empty() { "(no output, exit status 0)".into() } else { text }),
        Some(s) => Err(format!("exit status {}\n{text}", s.code().map_or("signal".into(), |c| c.to_string()))),
    }
}

/// Splits a line the way a shell would for quotes only (single, double, backslash); nothing is expanded or run.
pub fn split_words(line: &str) -> Result<Vec<String>, String> {
    let (mut out, mut cur, mut inword) = (Vec::new(), String::new(), false);
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\'' => {
                inword = true;
                loop {
                    match it.next() {
                        Some('\'') => break,
                        Some(x) => cur.push(x),
                        None => return Err("a quote is not closed".into()),
                    }
                }
            }
            '"' => {
                inword = true;
                loop {
                    match it.next() {
                        Some('"') => break,
                        Some('\\') => cur.push(it.next().ok_or("a quote is not closed")?),
                        Some(x) => cur.push(x),
                        None => return Err("a quote is not closed".into()),
                    }
                }
            }
            '\\' => {
                inword = true;
                cur.push(it.next().unwrap_or('\\'));
            }
            c if c.is_whitespace() => {
                if inword {
                    out.push(std::mem::take(&mut cur));
                    inword = false;
                }
            }
            c => {
                inword = true;
                cur.push(c);
            }
        }
    }
    if inword {
        out.push(cur);
    }
    Ok(out)
}

/// A web request as typed after `curl`: `[-X METHOD] [-H 'Name: value']… [-d body] [-I] url`.
#[derive(Debug)]
pub struct CurlArgs {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub head_only: bool,
}

pub fn parse_curl(line: &str) -> Result<CurlArgs, String> {
    const USAGE: &str = "usage: curl [-X METHOD] [-H 'Name: value'] [-d body] [-I] <https://address>";
    let w = split_words(line)?;
    let (mut method, mut url, mut headers, mut body, mut head) = (None::<String>, None::<String>, Vec::new(), None::<String>, false);
    let mut i = 0;
    while i < w.len() {
        let a = w[i].as_str();
        let value = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            w.get(*i).cloned().ok_or_else(|| USAGE.to_string())
        };
        match a {
            "-X" | "--request" => method = Some(value(&mut i)?.to_ascii_uppercase()),
            "-H" | "--header" => {
                let h = value(&mut i)?;
                let (k, v) = h.split_once(':').ok_or("a header looks like 'Name: value'")?;
                headers.push((k.trim().to_string(), v.trim().to_string()));
            }
            "-d" | "--data" | "--data-raw" => body = Some(value(&mut i)?),
            "-I" | "--head" => head = true,
            "-s" | "-S" | "-L" | "-k" | "-i" => {}
            f if f.starts_with('-') => return Err(format!("`{f}` is not supported. {USAGE}")),
            u => {
                if url.replace(u.to_string()).is_some() {
                    return Err(format!("one address at a time. {USAGE}"));
                }
            }
        }
        i += 1;
    }
    let method = method.unwrap_or_else(|| if head { "HEAD".into() } else if body.is_some() { "POST".into() } else { "GET".into() });
    if !["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"].contains(&method.as_str()) {
        return Err(format!("method {method} is not supported"));
    }
    if headers.iter().any(|(k, v)| k.is_empty() || k.contains(|c: char| c.is_control() || c.is_whitespace()) || v.contains(['\r', '\n'])) {
        return Err("bad header".into());
    }
    Ok(CurlArgs { method, url: url.ok_or(USAGE)?, headers, body: body.unwrap_or_default(), head_only: head })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_ssh_aws_and_gcloud_and_the_policy_host_is_named() {
        assert!(check("rm", &v(&["-rf", "/"])).unwrap_err().contains("not available"));
        assert!(check("bash", &v(&["-c", "ls"])).is_err());
        assert_eq!(check("aws", &v(&["s3", "ls"])).unwrap(), "amazonaws.com");
        assert_eq!(check("gcloud", &v(&["compute", "instances", "list"])).unwrap(), "googleapis.com");
        assert_eq!(check("ssh", &v(&["-p", "2222", "-i", "key", "bob@10.0.0.5", "uptime"])).unwrap(), "10.0.0.5");
        assert_eq!(check("ssh", &v(&["-4", "box.example.com", "ls"])).unwrap(), "box.example.com");
        assert!(check("ssh", &v(&["-p", "22"])).is_err());
    }

    #[test]
    fn ssh_options_that_run_programs_or_hop_are_refused() {
        for bad in [v(&["-o", "ProxyCommand=nc x 1", "h"]), v(&["-oProxyCommand=x", "h"]), v(&["-o", "LocalCommand=id", "h"]), v(&["-J", "jump", "h"]), v(&["-F", "/tmp/cfg", "h"]), v(&["-o", "proxyjump=x", "h"])] {
            assert!(check("ssh", &bad).is_err(), "{bad:?}");
        }
        assert!(check("ssh", &v(&["-o", "Port=22", "h"])).is_ok());
    }

    #[test]
    fn curl_lines_are_split_like_a_shell_would_but_nothing_is_run() {
        let c = parse_curl("-X post -H 'Content-Type: application/json' -d \"{\\\"a\\\": 1}\" https://example.com/x").unwrap();
        assert_eq!((c.method.as_str(), c.url.as_str(), c.body.as_str()), ("POST", "https://example.com/x", "{\"a\": 1}"));
        assert_eq!(c.headers, vec![("Content-Type".to_string(), "application/json".to_string())]);
        assert_eq!(parse_curl("https://example.com").unwrap().method, "GET");
        assert_eq!(parse_curl("-I https://example.com").unwrap().method, "HEAD");
        assert_eq!(parse_curl("-d x https://example.com").unwrap().method, "POST");
        assert_eq!(parse_curl("https://e.com; rm -rf /").unwrap_err().contains("one address"), true);
        assert!(parse_curl("-o out https://e.com").is_err() && parse_curl("--upload-file f https://e.com").is_err());
        assert!(parse_curl("-H nocolon https://e.com").is_err() && parse_curl("-d 'open https://e.com").is_err() && parse_curl("-X").is_err());
        assert!(parse_curl("").is_err());
    }

    #[test]
    fn arguments_are_quoted_for_the_approval_prompt_and_limited() {
        assert_eq!(command_line("aws", &v(&["s3", "ls", "my bucket", "it's"])), "aws s3 ls 'my bucket' 'it'\\''s'");
        assert!(check("aws", &vec!["x".to_string(); 65]).is_err());
        assert!(check("aws", &v(&["a\0b"])).is_err());
    }

    /// A stand-in `aws` on the PATH proves what is run, with which arguments and environment, and that a shell is not.
    #[test]
    fn a_program_runs_with_its_arguments_a_cut_down_environment_a_timeout_and_a_capped_output() {
        let d = std::env::temp_dir().join(format!("iyakki-exec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let script = d.join("aws");
        std::fs::write(&script, "#!/bin/sh\nif [ \"$1\" = sleep ]; then sleep 5; fi\nif [ \"$1\" = fail ]; then echo oops >&2; exit 3; fi\nif [ \"$1\" = big ]; then head -c 200000 /dev/zero | tr '\\0' x; fi\necho \"args=$# first=$1 second=$2 secret=${IYAKKI_SECRET:-none} key=${AWS_ACCESS_KEY_ID:-none} home=$HOME\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let old = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{old}", d.display()));
        std::env::set_var("IYAKKI_SECRET", "nope");
        std::env::set_var("AWS_ACCESS_KEY_ID", "AKIATEST");
        let out = run("aws", &v(&["s3 ls; echo pwned", "b"]), &d, None, 10).unwrap();
        assert!(out.contains("args=2 first=s3 ls; echo pwned second=b"), "{out}");
        assert!(out.contains("secret=none") && out.contains("key=AKIATEST"), "{out}");
        assert!(!out.contains("pwned\n"), "no shell ran the semicolon: {out}");
        // the person's home (on the data volume) is the program's HOME, whatever HOME IYAKKI itself has
        let person = d.join("person-home");
        std::fs::create_dir_all(&person).unwrap();
        let out = run("aws", &v(&["x"]), &d, Some(&person), 10).unwrap();
        assert!(out.contains(&format!("home={}", person.display())), "{out}");
        let e = run("aws", &v(&["fail"]), &d, None, 10).unwrap_err();
        assert!(e.contains("exit status 3") && e.contains("oops"), "{e}");
        let t = std::time::Instant::now();
        assert!(run("aws", &v(&["sleep"]), &d, None, 1).unwrap_err().contains("stopped after 1 seconds"));
        assert!(t.elapsed() < Duration::from_secs(4));
        assert!(run("aws", &v(&["big"]), &d, None, 10).unwrap().len() <= MAX_OUTPUT + 200);
        let _ = std::fs::remove_dir_all(&d);
    }
}
