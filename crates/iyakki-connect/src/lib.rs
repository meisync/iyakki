//! `iyakki-connect`: the small desktop connector (docs/development/CONNECTOR-APP-PLAN.md). This step: look at a tenant's certificate
//! (`probe`), remember it trust-on-first-use (`pin`, `unpin`, `pins`) and check a sign-in against the remembered certificate (`login`).
//! `serve` shows the desktop in the installed browser (a page on 127.0.0.1, `serve.rs`), `--share DIR` exchanges files (`share.rs`), and
//! `key` / `login --key` use a device key instead of a password (`devkey.rs`, plan 3a).
//!
//! Secrets: a password is read from standard input (one line) or typed at a prompt without echo; it is never taken from the command line.
//! A certificate that differs from the pinned one is a hard stop (exit status 2): the password is not sent and the pin is never
//! replaced silently (`unpin` first, on purpose).

pub mod browser;
pub mod devkey;
pub mod instances;
pub mod keystore;
pub mod link;
pub mod pins;
pub mod serve;
pub mod share;
pub mod voice;
pub mod voicecapture;
pub mod window;
pub mod windowcmd;

use std::io::{BufRead, Write};
use std::time::Duration;

use iyakki_link::certinfo::read_cert;
use iyakki_link::cloudclient::{probe_der, Client, Pin, Status, Target};
use iyakki_link::rawlink::{Auth, FilesApi, LinkError, RawWs};
use iyakki_link::sigscheme::DeviceKey;

pub const USAGE: &str = "usage: iyakki-connect <command>\n  probe HOST[:PORT]          show the certificate (names, validity, SHA-256) without trusting it\n  pin HOST[:PORT] [--yes]    remember the certificate (trust on first use); a changed one is refused\n  unpin HOST[:PORT]          forget a pin (the next pin is a new first use)\n  pins                       list the pinned tenants\n  tenants HOST[:PORT] USER   list the tenants this account may use (* marks the one the sign-in is in); the password is read from stdin\n  voice HOST[:PORT] USER [--return LISTEN ADVERTISE]     (EXPERIMENTAL) send this computer's microphone to My IYAKKI on the instance, only after you answer its card here; the password is read from stdin, then your answer\n  switch HOST[:PORT] USER TENANT   work in another tenant this account belongs to (the server checks it and says what it stopped and locked); the password is read from stdin
  login HOST[:PORT] USER     check a sign-in; the password is read from stdin or typed (no echo), never from arguments\n  login HOST[:PORT] USER --key   the same with the device key (nothing secret is sent)\n  key new [--force]          make this device's Ed25519 key (mode 600 in the configuration folder)\n  key enroll HOST[:PORT] USER   sign in with the password once and enrol this device's key (then use --key)\n  key show                   print the public key (ssh-ed25519 line) for the tenant's owner to enrol\n  key import-ssh PATH [--force]   use an unencrypted OpenSSH ssh-ed25519 private key (no RSA)\n  key import-agent [NAME] [--force]   sign with an ssh-ed25519 key an ssh-agent holds (SSH_AUTH_SOCK); the secret stays in the agent\n  serve HOST[:PORT] --user NAME [--key] [--share DIR [--write]] [--open] [--port N]\n                             show the desktop in your browser at a link on 127.0.0.1 (one-time token); --share DIR lets\n                             the page exchange files with that one folder (read-only unless --write; needs a password sign-in)\n  window LINK|ADDRESS|NAME [--name NAME] [--yes] [--browser PATH] [--no-browser]   (EXPERIMENTAL)\n                             open a client window for an IYAKKI instance in your installed browser (application mode, private profile): LINK is\n                             the one-time link from Settings (https://host/connect#token, with ?pin= it needs no question); the device key stays here\n  devices                    list the instances this computer has a device on\n  devices forget NAME        forget one (deletes its key here; revoke the device on the instance too)\n";

/// Exit status of a hard stop because the certificate is not the pinned one.
pub const EXIT_PIN_CHANGED: i32 = 2;

#[derive(Debug, PartialEq, Eq)]
pub enum Fail {
    /// Anything that went wrong or was refused (status 1).
    Error(String),
    /// The certificate changed (status 2).
    PinChanged(String),
}

impl Fail {
    pub fn status(&self) -> i32 {
        match self {
            Fail::Error(_) => 1,
            Fail::PinChanged(_) => EXIT_PIN_CHANGED,
        }
    }
    pub fn text(&self) -> &str {
        match self {
            Fail::Error(m) | Fail::PinChanged(m) => m,
        }
    }
}

impl From<String> for Fail {
    fn from(s: String) -> Fail {
        Fail::Error(s)
    }
}

/// What a run needs from its surroundings.
pub struct Ctx {
    pub store: pins::Store,
    pub timeout: Duration,
    /// Standard input is a terminal (so a confirmation can be asked).
    pub interactive: bool,
}

/// Run one command. Output goes to `out`; the password or the confirmation is read from `input`.
pub fn run(args: &[String], ctx: &Ctx, input: &mut dyn BufRead, out: &mut dyn Write) -> Result<(), Fail> {
    let say = |out: &mut dyn Write, s: String| writeln!(out, "{s}").map_err(|e| Fail::Error(e.to_string()));
    let (cmd, rest) = args.split_first().ok_or_else(|| Fail::Error(USAGE.into()))?;
    let flag = |f: &str| rest.iter().any(|a| a == f);
    let pos: Vec<&String> = rest.iter().filter(|a| !a.starts_with("--")).collect();
    let ps: Vec<&str> = pos.iter().map(|s| s.as_str()).collect();
    match (cmd.as_str(), ps.as_slice()) {
        ("probe", [addr]) => {
            let der = probe_der(addr, ctx.timeout)?;
            say(out, describe(addr, &der))
        }
        ("pin", [addr]) => {
            let der = probe_der(addr, ctx.timeout)?;
            let shown = fingerprint(&der);
            match ctx.store.get(addr)? {
                Some(p) if p == shown => say(out, format!("{} is already pinned to this certificate ({}).", pins::key(addr)?, shown.display())),
                Some(p) => Err(Fail::PinChanged(changed(addr, p, shown))),
                None => {
                    say(out, describe(addr, &der))?;
                    if !flag("--yes") {
                        if !ctx.interactive {
                            return Err(Fail::Error("not pinned: compare the fingerprint above with the one the tenant's owner gave you, then run again with --yes".into()));
                        }
                        write!(out, "Trust this certificate for {}? [y/N] ", pins::key(addr)?).and_then(|_| out.flush()).map_err(|e| Fail::Error(e.to_string()))?;
                        let mut a = String::new();
                        input.read_line(&mut a).map_err(|e| Fail::Error(e.to_string()))?;
                        if !matches!(a.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                            return Err(Fail::Error("not pinned".into()));
                        }
                    }
                    ctx.store.set(addr, shown)?;
                    say(out, format!("pinned {} to {}", pins::key(addr)?, shown.display()))
                }
            }
        }
        ("unpin", [addr]) => {
            if ctx.store.remove(addr)? {
                say(out, format!("{} is no longer pinned", pins::key(addr)?))
            } else {
                Err(Fail::Error(format!("{} was not pinned", pins::key(addr)?)))
            }
        }
        ("devices", rest) => windowcmd::devices(rest, ctx, out),
        ("pins", []) => {
            let all = ctx.store.list()?;
            if all.is_empty() {
                return say(out, "nothing is pinned yet".into());
            }
            for (a, p) in all {
                say(out, format!("{a}  {}", p.display()))?;
            }
            Ok(())
        }
        ("login", [addr, user]) if flag("--key") => {
            let pin = ctx.store.get(addr)?.ok_or_else(|| Fail::Error(format!("{} is not pinned: run `iyakki-connect pin {addr}` first", addr)))?;
            let key = devkey::KeyFile::new(ctx.store.dir()).load()?.ok_or_else(|| Fail::Error("there is no device key yet: run `iyakki-connect key new`".into()))?;
            let target = Target { addr: addr.to_string(), pin };
            let auth = Auth::Key { user: user.trim().to_string(), key };
            match RawWs::open(&target, &auth) {
                Ok(ws) => ws.close(),
                Err(LinkError::PinChanged(m)) => return Err(Fail::PinChanged(format!("{m}. Nothing was sent. If you expected the change, run `iyakki-connect unpin {addr}` and pin again after checking the new fingerprint."))),
                Err(LinkError::Refused(m)) => return Err(Fail::Error(format!("sign-in refused: {m}"))),
                Err(LinkError::Other(m)) => return Err(Fail::Error(m)),
            }
            say(out, format!("signed in to {} as {} with the device key", pins::key(addr)?, user.trim()))
        }
        ("key", ["new"]) => {
            let kf = devkey::KeyFile::new(ctx.store.dir());
            let key = DeviceKey::generate()?;
            kf.store(&key, flag("--force"))?;
            say(out, format!("made a device key\n{}\nGive the public line to the tenant's owner to enrol. The secret never leaves this computer.", devkey::show(&key)))
        }
        ("key", ["enroll", addr, user]) => {
            let pin = ctx.store.get(addr)?.ok_or_else(|| Fail::Error(format!("{addr} is not pinned: run `iyakki-connect pin {addr}` first")))?;
            let key = devkey::KeyFile::new(ctx.store.dir()).load()?.ok_or_else(|| Fail::Error("there is no device key yet: run `iyakki-connect key new`".into()))?;
            let mut password = String::new();
            input.read_line(&mut password).map_err(|e| Fail::Error(e.to_string()))?;
            let password = password.trim_end_matches(['\r', '\n']).to_string();
            if password.is_empty() {
                return Err(Fail::Error("no password given (type it at the prompt or pipe it in on standard input)".into()));
            }
            let target = Target { addr: addr.to_string(), pin };
            match iyakki_link::rawlink::enroll_key(&target, user, &password, &key.ssh_public_line("iyakki-connect")) {
                Ok(fp) => say(out, format!("enrolled the device key {fp} for {} on {}", user.trim(), pins::key(addr)?)),
                Err(LinkError::PinChanged(m)) => Err(Fail::PinChanged(m)),
                Err(e) => Err(Fail::Error(e.text().to_string())),
            }
        }
        ("key", ["show"]) => {
            let key = devkey::KeyFile::new(ctx.store.dir()).load()?.ok_or_else(|| Fail::Error("there is no device key yet: run `iyakki-connect key new`".into()))?;
            say(out, devkey::show(&key))
        }
        ("key", ["import-agent"]) | ("key", ["import-agent", _]) => {
            let selector = rest.iter().find(|a| !a.starts_with("--") && a.as_str() != "import-agent" && a.as_str() != "key").map(String::as_str);
            let key = devkey::import_agent(std::env::var("SSH_AUTH_SOCK").ok().as_deref(), selector)?;
            devkey::KeyFile::new(ctx.store.dir()).store(&key, flag("--force"))?;
            say(out, format!("will sign with the key the ssh-agent holds (the secret stays in the agent)\n{}", devkey::show(&key)))
        }
        ("key", ["import-ssh", path]) => {
            let key = devkey::import_ssh(std::path::Path::new(path))?;
            devkey::KeyFile::new(ctx.store.dir()).store(&key, flag("--force"))?;
            say(out, format!("imported the key\n{}", devkey::show(&key)))
        }
        ("voice", [addr, user, rest @ ..]) => {
            // `--return LISTEN ADVERTISE`: play the answer's sound here too (the address the cloud machine can reach this computer at)
            let ret_args = match rest {
                [] => None,
                [flag, listen, advertise] if *flag == "--return" => Some((listen.to_string(), advertise.to_string())),
                _ => return Err(Fail::Error(USAGE.into())),
            };
            let pin = ctx.store.get(addr)?.ok_or_else(|| Fail::Error(format!("{addr} is not pinned: run `iyakki-connect pin {addr}` first")))?;
            let mut password = String::new();
            input.read_line(&mut password).map_err(|e| Fail::Error(e.to_string()))?;
            let password = password.trim_end_matches(['\r', '\n']).to_string();
            if password.is_empty() {
                return Err(Fail::Error("no password given (type it at the prompt or pipe it in on standard input)".into()));
            }
            let target = Target { addr: addr.to_string(), pin };
            let fail = |e: LinkError| match e {
                LinkError::PinChanged(m) => Fail::PinChanged(m),
                LinkError::Refused(m) => Fail::Error(format!("sign-in refused: {m}")),
                LinkError::Other(m) => Fail::Error(m),
            };
            let api = FilesApi::login(&target, user.trim(), &password).map_err(fail)?;
            let offer = api.voice_offer().map_err(fail)?;
            // Ctrl+C ends the session (the same flag `serve` waits on)
            unsafe {
                libc::signal(libc::SIGINT, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
            }
            let mic: Box<dyn iyakki_audiolink::Mic> = Box::new(voicecapture::CommandMic::for_this_system().map_err(Fail::Error)?);
            let ret = match ret_args {
                None => None,
                Some((listen, advertise)) => {
                    let end = std::sync::Arc::new(voice::ReturnEnd::new(&listen, &advertise).map_err(Fail::Error)?);
                    let sink = Box::new(voicecapture::CommandSink::for_this_system().map_err(Fail::Error)?);
                    let api = std::sync::Arc::new(api);
                    Some(voice::Return { end, sink, tell: Box::new(move |a, o| api.voice_return(a, o).map_err(|e| match e { LinkError::PinChanged(m) | LinkError::Refused(m) | LinkError::Other(m) => m })) })
                }
            };
            voice::session_with_return(offer, input, out, mic, &STOP, ret).map_err(Fail::Error)
        }
        ("tenants", [addr, user]) => {
            let pin = ctx.store.get(addr)?.ok_or_else(|| Fail::Error(format!("{addr} is not pinned: run `iyakki-connect pin {addr}` first")))?;
            let mut password = String::new();
            input.read_line(&mut password).map_err(|e| Fail::Error(e.to_string()))?;
            let password = password.trim_end_matches(['\r', '\n']).to_string();
            if password.is_empty() {
                return Err(Fail::Error("no password given (type it at the prompt or pipe it in on standard input)".into()));
            }
            let target = Target { addr: addr.to_string(), pin };
            let fail = |e: LinkError| match e {
                LinkError::PinChanged(m) => Fail::PinChanged(m),
                LinkError::Refused(m) => Fail::Error(format!("sign-in refused: {m}")),
                LinkError::Other(m) => Fail::Error(m),
            };
            let api = FilesApi::login(&target, user.trim(), &password).map_err(fail)?;
            let list = api.tenants().map_err(fail)?;
            if list.is_empty() {
                return say(out, "no tenant is open to this account".into());
            }
            for t in list {
                say(out, format!("{} {}  {}  ({})", if t.current { "*" } else { " " }, t.id, t.name, t.role))?;
            }
            Ok(())
        }
        ("switch", [addr, user, tenant]) => {
            let pin = ctx.store.get(addr)?.ok_or_else(|| Fail::Error(format!("{addr} is not pinned: run `iyakki-connect pin {addr}` first")))?;
            let mut password = String::new();
            input.read_line(&mut password).map_err(|e| Fail::Error(e.to_string()))?;
            let password = password.trim_end_matches(['\r', '\n']).to_string();
            if password.is_empty() {
                return Err(Fail::Error("no password given (type it at the prompt or pipe it in on standard input)".into()));
            }
            let target = Target { addr: addr.to_string(), pin };
            let fail = |e: LinkError| match e {
                LinkError::PinChanged(m) => Fail::PinChanged(m),
                LinkError::Refused(m) => Fail::Error(format!("sign-in refused: {m}")),
                LinkError::Other(m) => Fail::Error(m),
            };
            let api = FilesApi::login(&target, user.trim(), &password).map_err(fail)?;
            let sw = api.switch_tenant(tenant.trim()).map_err(fail)?;
            say(out, format!("now in {} ({}), you are {}; before: {}", sw.tenant.id, sw.tenant.name, sw.tenant.role, if sw.previous.is_empty() { "-" } else { &sw.previous }))?;
            say(out, format!("the server stopped {} running chat(s){}", sw.runs_stopped, if sw.vault_locked { " and locked the previous tenant's vault" } else { "" }))?;
            if !sw.url.is_empty() {
                say(out, format!("this tenant is also served at {}", sw.url))?;
            }
            Ok(())
        }
        ("login", [addr, user]) => {
            let pin = ctx.store.get(addr)?.ok_or_else(|| Fail::Error(format!("{} is not pinned: run `iyakki-connect pin {addr}` first", addr)))?;
            let mut password = String::new();
            input.read_line(&mut password).map_err(|e| Fail::Error(e.to_string()))?;
            let password = password.trim_end_matches(['\r', '\n']).to_string();
            if password.is_empty() {
                return Err(Fail::Error("no password given (type it at the prompt or pipe it in on standard input)".into()));
            }
            let target = Target { addr: addr.to_string(), pin };
            let client = Client::connect(&target, user, &password, (1280, 720))?;
            let result = match client.status() {
                Status::Refused(m) => Err(Fail::Error(format!("sign-in refused: {m}"))),
                Status::Closed(m) if m.contains("is not the one that was trusted") => Err(Fail::PinChanged(format!("{m}. Nothing was sent. If you expected the change, run `iyakki-connect unpin {addr}` and pin again after checking the new fingerprint."))),
                Status::Closed(m) => Err(Fail::Error(m)),
                Status::Connecting | Status::Live | Status::Reconnecting => Ok(()),
            };
            client.close();
            result?;
            say(out, format!("signed in to {} as {}", pins::key(addr)?, user.trim()))
        }
        _ => Err(Fail::Error(USAGE.into())),
    }
}

fn fingerprint(der: &[u8]) -> Pin {
    iyakki_link::cloudclient::pin_of(der)
}

pub(crate) fn changed(addr: &str, pinned: Pin, shown: Pin) -> String {
    format!(
        "STOP: the certificate of {addr} changed.\n  pinned: {}\n  shown:  {}\nEither the tenant renewed or replaced its certificate, or this is not the tenant. Nothing was changed. Ask the owner for the new fingerprint; if it matches, `iyakki-connect unpin {addr}` and pin again.",
        pinned.display(),
        shown.display()
    )
}

/// The lines `probe` and `pin` show.
pub fn describe(addr: &str, der: &[u8]) -> String {
    let mut s = format!("certificate of {addr}\n");
    match read_cert(der) {
        Some(i) => {
            s.push_str(&format!("  names:      {}\n", if i.names.is_empty() { "(none)".into() } else { i.names.join(", ") }));
            s.push_str(&format!("  valid from: {}\n  valid to:   {}\n", date(i.not_before), date(i.not_after)));
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
            if now < i.not_before {
                s.push_str("  WARNING: not valid yet\n");
            } else if now >= i.not_after {
                s.push_str("  WARNING: expired\n");
            }
        }
        None => s.push_str("  (the certificate's names and dates could not be read)\n"),
    }
    s.push_str(&format!("  SHA-256:    {}", fingerprint(der).display()));
    s
}

/// `2026-11-04 12:00 UTC` from Unix seconds.
pub fn date(t: i64) -> String {
    let (days, secs) = (t.div_euclid(86400), t.rem_euclid(86400));
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", secs / 3600, secs % 3600 / 60)
}

/// Read one line from the terminal without echo (restores the terminal on every path). `None` when it cannot be done.
pub fn read_password_no_echo(prompt: &str) -> Option<String> {
    use std::io::Read;
    let _ = std::io::stderr().write_all(prompt.as_bytes());
    let _ = std::io::stderr().flush();
    let mut old: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(0, &mut old) } != 0 {
        return None;
    }
    let mut quiet = old;
    quiet.c_lflag &= !libc::ECHO;
    if unsafe { libc::tcsetattr(0, libc::TCSANOW, &quiet) } != 0 {
        return None;
    }
    let mut line = Vec::new();
    let mut b = [0u8; 1];
    while std::io::stdin().read(&mut b).map(|n| n == 1).unwrap_or(false) && b[0] != b'\n' {
        line.push(b[0]);
    }
    unsafe { libc::tcsetattr(0, libc::TCSANOW, &old) };
    let _ = std::io::stderr().write_all(b"\n");
    Some(String::from_utf8_lossy(&line).into_owned())
}

/// Value of `--name VALUE` in `args`.
fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// `serve HOST[:PORT] --user NAME [--key] [--share DIR [--write]] [--open] [--port N]`: start the page server. The password (unless `--key`)
/// is read from `input`. Returns the running server (the caller prints its link and waits).
pub fn serve_start(args: &[String], ctx: &Ctx, input: &mut dyn BufRead) -> Result<serve::Server, Fail> {
    let rest = args.get(1..).unwrap_or(&[]);
    let value_opts = ["--user", "--share", "--port"];
    let mut pos = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        if value_opts.contains(&rest[i].as_str()) {
            i += 2;
        } else {
            if !rest[i].starts_with("--") {
                pos.push(&rest[i]);
            }
            i += 1;
        }
    }
    let [addr] = pos.as_slice() else { return Err(Fail::Error(USAGE.into())) };
    let flag = |f: &str| rest.iter().any(|a| a == f);
    let user = opt(rest, "--user").ok_or_else(|| Fail::Error("--user NAME is needed".into()))?.trim().to_string();
    let port: u16 = match opt(rest, "--port") {
        Some(p) => p.parse().map_err(|_| Fail::Error(format!("{p:?} is not a port")))?,
        None => 0,
    };
    let pin = ctx.store.get(addr)?.ok_or_else(|| Fail::Error(format!("{addr} is not pinned: run `iyakki-connect pin {addr}` first")))?;
    let target = Target { addr: addr.to_string(), pin };
    if flag("--write") && opt(rest, "--share").is_none() {
        return Err(Fail::Error("--write only makes sense with --share DIR".into()));
    }
    let share = match opt(rest, "--share") {
        Some(d) => Some(share::Share::new(std::path::Path::new(d), flag("--write"))?),
        None => None,
    };
    let (auth, files) = if flag("--key") {
        if share.is_some() {
            return Err(Fail::Error("--share needs a password sign-in: the tenant's file routes do not take the device key yet".into()));
        }
        let key = devkey::KeyFile::new(ctx.store.dir()).load()?.ok_or_else(|| Fail::Error("there is no device key yet: run `iyakki-connect key new`".into()))?;
        (Auth::Key { user, key }, None)
    } else {
        let mut password = String::new();
        input.read_line(&mut password).map_err(|e| Fail::Error(e.to_string()))?;
        let password = password.trim_end_matches(['\r', '\n']).to_string();
        if password.is_empty() {
            return Err(Fail::Error("no password given (type it at the prompt or pipe it in on standard input), or use --key".into()));
        }
        // sign in once now: a wrong password or a changed certificate stops here, before a link is printed
        let files = if share.is_some() {
            match FilesApi::login(&target, &user, &password) {
                Ok(f) => Some(f),
                Err(LinkError::PinChanged(m)) => return Err(Fail::PinChanged(m)),
                Err(e) => return Err(Fail::Error(format!("the tenant's files: {}", e.text()))),
            }
        } else {
            None
        };
        (Auth::Password { user, password: zeroize::Zeroizing::new(password) }, files)
    };
    serve::Server::start(serve::Options { target, auth, share, files, port }).map_err(Fail::Error)
}

/// Open the link in the installed browser without a shell and without the token in any process list: a small redirect file (mode 600,
/// removed after a few seconds) is opened instead of the link itself.
pub fn open_in_browser(url: &str, dir: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(|| dir.to_path_buf());
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&base).map_err(|e| e.to_string())?;
    let mut rnd = [0u8; 6];
    iyakki_sec::random_fill(&mut rnd).map_err(|_| "no random numbers".to_string())?;
    let file = base.join(format!("iyakki-open-{}.html", iyakki_link::sigscheme::hex(&rnd)));
    let html = format!("<!doctype html><meta charset=utf-8><meta http-equiv=refresh content=\"0;url={url}\"><title>IYAKKI</title><p>Opening IYAKKI... <a href=\"{url}\">continue</a>");
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    f.write_all(html.as_bytes()).map_err(|e| e.to_string())?;
    drop(f);
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let started = std::process::Command::new(opener).arg(&file).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
    let cleanup = file.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(20));
        let _ = std::fs::remove_file(cleanup);
    });
    started.map(|_| ()).map_err(|e| format!("cannot run {opener}: {e}. Open the link by hand"))
}

/// The program: returns the exit status.
pub fn main_with(args: &[String]) -> i32 {
    let dir = match pins::config_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("iyakki-connect: {e}");
            return 1;
        }
    };
    let tty = unsafe { libc::isatty(0) } == 1;
    let ctx = Ctx { store: pins::Store::new(&dir), timeout: Duration::from_secs(8), interactive: tty };
    let mut stdout = std::io::stdout();
    let wants_password = !args.iter().any(|a| a == "--key") && (matches!(args.first().map(String::as_str), Some("login" | "serve" | "voice")) || (args.first().map(String::as_str) == Some("key") && args.get(1).map(String::as_str) == Some("enroll")));
    let mut input: Box<dyn BufRead> = if tty && wants_password {
        match read_password_no_echo("Password: ") {
            // the password first, then the terminal itself (`voice` asks its card there)
            Some(p) => Box::new(std::io::BufReader::new(std::io::Read::chain(std::io::Cursor::new(format!("{p}\n")), std::io::stdin()))),
            None => {
                eprintln!("iyakki-connect: cannot read the password from the terminal");
                return 1;
            }
        }
    } else {
        Box::new(std::io::BufReader::new(std::io::stdin()))
    };
    if args.first().map(String::as_str) == Some("serve") {
        return match serve_start(args, &ctx, &mut input) {
            Ok(server) => {
                let url = server.url();
                println!("Open this link in your browser (it works once, on this computer only):\n  {url}");
                if args.iter().any(|a| a == "--open") {
                    if let Err(e) = open_in_browser(&url, &dir) {
                        eprintln!("iyakki-connect: {e}");
                    }
                }
                println!("Press Ctrl+C to stop.");
                wait_for_stop(&server);
                0
            }
            Err(f) => {
                eprintln!("iyakki-connect: {}", f.text());
                f.status()
            }
        };
    }
    if args.first().map(String::as_str) == Some("window") {
        return window_main(args, &ctx, &mut input, &mut stdout);
    }
    match run(args, &ctx, &mut input, &mut stdout) {
        Ok(()) => 0,
        Err(f) => {
            eprintln!("iyakki-connect: {}", f.text());
            f.status()
        }
    }
}

static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Wait for Ctrl+C or a termination request, then let the connections sign out.
fn wait_for_stop(server: &serve::Server) {
    unsafe {
        libc::signal(libc::SIGINT, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
    while !STOP.load(std::sync::atomic::Ordering::Relaxed) && !server.stopped() {
        std::thread::sleep(Duration::from_millis(100));
    }
    server.stop();
    std::thread::sleep(Duration::from_millis(800));
}

/// `window`: start the loopback window server, open the browser on it, wait until the browser closes (or Ctrl+C), clean up.
fn window_main(args: &[String], ctx: &Ctx, input: &mut dyn BufRead, out: &mut dyn Write) -> i32 {
    let prepared = match windowcmd::prepare(args, ctx, input, out) {
        Ok(p) => p,
        Err(f) => {
            eprintln!("iyakki-connect: {}", f.text());
            return f.status();
        }
    };
    let w = &prepared.window;
    unsafe {
        libc::signal(libc::SIGINT, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
    let mut cleanup: Option<std::path::PathBuf> = None;
    let mut child: Option<(std::process::Child, bool)> = None;
    if prepared.print_only {
        println!("Open this address in a browser on this computer (it works once, on this computer only):\n  {}", w.launch_url());
    } else {
        let started = browser::find_here(prepared.browser.as_deref()).and_then(|b| {
            let (profile, start) = browser::prepare_profile(&browser::profile_base(), w.launch_url(), b.kind)?;
            cleanup = start.parent().map(|p| p.to_path_buf());
            let c = browser::command(&b, &profile, &start, &prepared.browser_args);
            if let Some(n) = c.note {
                eprintln!("iyakki-connect: {n}");
            }
            println!("Opening {} for {} ...", b.name, prepared.name);
            let child = std::process::Command::new(&c.program).args(&c.args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map_err(|e| format!("cannot start {}: {e}", c.program.display()))?;
            // a browser that is only asked to open a file (Safari) returns at once: then the window lives until Ctrl+C
            Ok((child, b.kind != browser::Kind::Safari))
        });
        match started {
            Ok(c) => child = Some(c),
            Err(e) => {
                eprintln!("iyakki-connect: {e}");
                if let Some(d) = &cleanup {
                    let _ = std::fs::remove_dir_all(d);
                }
                return 1;
            }
        }
        if !child.as_ref().is_some_and(|c| c.1) {
            println!("Press Ctrl+C to stop.");
        }
    }
    loop {
        if STOP.load(std::sync::atomic::Ordering::Relaxed) || w.stopped() {
            break;
        }
        if let Some((c, true)) = child.as_mut() {
            if c.try_wait().map(|s| s.is_some()).unwrap_or(true) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    w.stop();
    if let Some((c, _)) = child.as_mut() {
        let _ = c.kill();
        let _ = c.wait();
    }
    std::thread::sleep(Duration::from_millis(300));
    if let Some(d) = cleanup {
        let _ = std::fs::remove_dir_all(d);
    }
    match w.failure() {
        Some(m) => {
            eprintln!("iyakki-connect: STOP: {m}");
            EXIT_PIN_CHANGED
        }
        None => 0,
    }
}
