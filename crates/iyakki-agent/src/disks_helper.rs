//! DISKS, encrypted volumes: the helper that opens them. The helper is a separate program the person installs from CLI tools
//! (`cryptsetup` for LUKS, `dislocker` for BitLocker; both GPL, so IYAKKI never ships them), run as a sub-process by a fixed argument
//! list, never through a shell. The password goes to the program's standard input: it is not in an argument list (`ps` shows
//! those), not in the environment, and not in a file.
//!
//! This file is the only place of the unlock feature that starts a program or makes a device node. The program runner and the
//! file operations sit behind [`Sys`], so the tests run the whole sequence against a made-up one ([`FakeSys`]).
//! Why a helper and not our own LUKS code: `docs/development/DISKS-UNLOCK-HELPER.md`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::disks_crypt::{mapper_path, CryptKind, Secret};

/// How long a helper may run (a LUKS2 key slot made with Argon2 can take many seconds on a small machine).
pub const HELPER_TIMEOUT: Duration = Duration::from_secs(180);

/// What the helper printed and how it ended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcOut {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Why opening did not work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// The key does not open it. The only error that counts towards the rate limit.
    WrongKey,
    /// Anything else, in words for the person.
    Failed(String),
}

/// What to open.
#[derive(Clone, Debug)]
pub struct OpenReq<'a> {
    pub kind: CryptKind,
    /// `/dev/sdb1`.
    pub dev: &'a str,
    /// The partition's name (`sdb1`): the opened device is `/dev/mapper/iyakki-sdb1`.
    pub part: &'a str,
    pub read_only: bool,
}

/// Opens and closes encrypted volumes. `open` makes `/dev/mapper/iyakki-<part>` appear; `close` removes it again.
pub trait CryptRunner: Send + Sync {
    /// `Err(words)` when the helper for this kind is not on the machine (with how to get it).
    fn available(&self, kind: CryptKind) -> Result<(), String>;
    fn open(&self, req: &OpenReq, secret: &Secret) -> Result<(), OpenError>;
    fn close(&self, kind: CryptKind, part: &str) -> Result<(), String>;
}

/// What the helper runner needs from the machine.
pub trait Sys: Send + Sync {
    fn is_file(&self, path: &str) -> bool;
    /// Run a program with the given argument list, the secret (if any) on its standard input, a bare environment.
    fn run(&self, prog: &str, args: &[String], stdin: Option<&[u8]>) -> Result<ProcOut, String>;
    fn symlink(&self, target: &str, at: &str) -> Result<(), String>;
    fn read_link(&self, at: &str) -> Option<String>;
    fn remove_file(&self, at: &str) -> Result<(), String>;
    /// A folder only the owner can enter.
    fn make_private_dir(&self, path: &str) -> Result<(), String>;
    fn remove_dir(&self, path: &str) -> Result<(), String>;
    /// Whether anything is there (a block device, a link, a folder).
    fn exists(&self, path: &str) -> bool;
}

/// The real machine.
pub struct RealSys;

impl Sys for RealSys {
    fn is_file(&self, path: &str) -> bool {
        std::fs::metadata(path).is_ok_and(|m| m.is_file())
    }

    fn run(&self, prog: &str, args: &[String], stdin: Option<&[u8]>) -> Result<ProcOut, String> {
        use std::io::{Read, Write};
        use std::process::{Command, Stdio};
        let mut child = Command::new(prog)
            .args(args)
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .env("LC_ALL", "C")
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start {prog}: {e}"))?;
        if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
            // the secret is small: it fits the pipe, so this does not block; closing the pipe tells the helper the key has ended
            let _ = pipe.write_all(bytes);
        }
        let (tx, rx) = std::sync::mpsc::channel::<(bool, String)>();
        for (is_err, src) in [(false, child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>)), (true, child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>))] {
            let (tx, Some(mut r)) = (tx.clone(), src) else { continue };
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = r.by_ref().take(1 << 20).read_to_end(&mut buf);
                let _ = tx.send((is_err, String::from_utf8_lossy(&buf).into_owned()));
            });
        }
        drop(tx);
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) if start.elapsed() > HELPER_TIMEOUT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{prog} did not finish in {} seconds and was stopped", HELPER_TIMEOUT.as_secs()));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(40)),
                Err(e) => return Err(format!("{prog}: {e}")),
            }
        };
        // a helper that goes to the background (dislocker) keeps its pipes open: take what has arrived, wait only briefly
        let mut out = ProcOut { code: status.code().unwrap_or(-1), ..Default::default() };
        let until = Instant::now() + Duration::from_millis(1500);
        while let Ok((is_err, text)) = rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
            if is_err { out.stderr = text } else { out.stdout = text }
        }
        Ok(out)
    }

    fn symlink(&self, target: &str, at: &str) -> Result<(), String> {
        std::os::unix::fs::symlink(target, at).map_err(|e| format!("{at}: {e}"))
    }

    fn read_link(&self, at: &str) -> Option<String> {
        std::fs::read_link(at).ok().map(|p| p.to_string_lossy().into_owned())
    }

    fn remove_file(&self, at: &str) -> Result<(), String> {
        std::fs::remove_file(at).map_err(|e| format!("{at}: {e}"))
    }

    fn make_private_dir(&self, path: &str) -> Result<(), String> {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path).map_err(|e| format!("{path}: {e}"))
    }

    fn remove_dir(&self, path: &str) -> Result<(), String> {
        std::fs::remove_dir(path).map_err(|e| format!("{path}: {e}"))
    }

    fn exists(&self, path: &str) -> bool {
        std::fs::symlink_metadata(path).is_ok()
    }
}

/// Where a helper program may be: the folder CLI tools links installed programs into, then the system's own.
pub(crate) fn search_dirs(cli_bin: &str) -> Vec<String> {
    let mut v = vec![cli_bin.to_string()];
    v.extend(["/usr/sbin", "/usr/bin", "/sbin", "/bin", "/usr/local/sbin", "/usr/local/bin"].map(String::from));
    v
}

/// The words when a helper is missing. It names the CLI tools entry and says plainly that IYAKKI does not ship it.
pub fn missing_words(kind: CryptKind) -> String {
    let (tool, lic) = match kind {
        CryptKind::Luks1 | CryptKind::Luks2 => ("cryptsetup", "GPL-2.0"),
        CryptKind::BitLocker => ("dislocker", "GPL-2.0"),
    };
    format!("Opening {} needs the program {tool}, which is not installed. IYAKKI does not ship it ({lic} licence): install it from CLI tools (you give the download address and its SHA-256; nothing is fetched until you do) or with this system's package manager, then try again. Nothing was changed on the drive.", kind.family())
}

/// The runner that starts the helpers.
pub struct ProcRunner {
    sys: Arc<dyn Sys>,
    cli_bin: String,
    /// Where the BitLocker file system of `dislocker` is shown (one private folder per volume).
    work: String,
}

/// The folder for `dislocker`'s FUSE mount points; a tmpfs.
pub const WORK_DIR: &str = "/run/iyakki-disks";

impl ProcRunner {
    pub fn new(sys: Arc<dyn Sys>, cli_bin: &str, work: &str) -> ProcRunner {
        ProcRunner { sys, cli_bin: cli_bin.to_string(), work: work.to_string() }
    }

    /// The real runner for the data folder of CLI tools (`<data>/cli/bin`).
    pub fn real(cli_bin: &std::path::Path) -> ProcRunner {
        ProcRunner::new(Arc::new(RealSys), &cli_bin.to_string_lossy(), WORK_DIR)
    }

    fn find(&self, prog: &str) -> Option<String> {
        search_dirs(&self.cli_bin).into_iter().map(|d| format!("{d}/{prog}")).find(|p| self.sys.is_file(p))
    }

    fn fuse_dir(&self, part: &str) -> String {
        let name = crate::disks_crypt::mapper_name(part);
        format!("{}/{}", self.work, name)
    }

    fn open_luks(&self, req: &OpenReq, secret: &Secret) -> Result<(), OpenError> {
        let prog = self.find("cryptsetup").ok_or_else(|| OpenError::Failed(missing_words(req.kind)))?;
        let name = crate::disks_crypt::mapper_name(req.part);
        let mut args: Vec<String> = vec!["open".into(), "--type".into(), "luks".into(), "--key-file=-".into(), "--batch-mode".into()];
        if req.read_only {
            args.push("--readonly".into());
        }
        args.push(req.dev.to_string());
        args.push(name);
        let out = self.sys.run(&prog, &args, Some(secret.expose())).map_err(OpenError::Failed)?;
        match out.code {
            0 => Ok(()),
            // "No key available with this passphrase": cryptsetup's exit code 2
            2 => Err(OpenError::WrongKey),
            5 => Err(OpenError::Failed("The helper says the volume is already open or in use. Lock it first, then try again.".into())),
            3 => Err(OpenError::Failed("There is not enough memory to open this volume (a LUKS2 volume made with Argon2 asks for a lot). Close apps and try again.".into())),
            c => Err(OpenError::Failed(format!("cryptsetup could not open it (code {c}): {}", first_line(&out.stderr)))),
        }
    }

    fn open_bitlocker(&self, req: &OpenReq, secret: &Secret) -> Result<(), OpenError> {
        let dislocker = self.find("dislocker").ok_or_else(|| OpenError::Failed(missing_words(req.kind)))?;
        let losetup = self.find("losetup").ok_or_else(|| OpenError::Failed("The program losetup is missing from this system, so the unlocked BitLocker image cannot be attached.".into()))?;
        let dir = self.fuse_dir(req.part);
        let _ = self.sys.make_private_dir(&self.work);
        self.sys.make_private_dir(&dir).map_err(|e| OpenError::Failed(format!("Could not make the working folder: {e}")))?;
        // a 48-digit recovery key has dashes between groups of six digits; a password does not look like that
        let recovery = looks_like_recovery_key(secret.expose());
        // `-u` / `-p` without a value make dislocker ask for the password / recovery key: it reads it from standard input
        let mut args: Vec<String> = vec!["-V".into(), req.dev.to_string(), if recovery { "-p".into() } else { "-u".into() }, "-r".into(), "--".into(), dir.clone()];
        if !req.read_only {
            args.retain(|a| a != "-r");
        }
        let out = match self.sys.run(&dislocker, &args, Some(secret.expose())) {
            Ok(o) => o,
            Err(e) => {
                let _ = self.sys.remove_dir(&dir);
                return Err(OpenError::Failed(e));
            }
        };
        let file = format!("{dir}/dislocker-file");
        if out.code != 0 || !self.sys.exists(&file) {
            let _ = self.sys.remove_dir(&dir);
            let t = format!("{} {}", out.stdout, out.stderr).to_lowercase();
            // dislocker has no exit code for a wrong key; its messages say "invalid"/"unable to decrypt"/"wrong"
            return if t.contains("invalid") || t.contains("wrong") || t.contains("unable to decrypt") || t.contains("bad password") || t.contains("not valid") {
                Err(OpenError::WrongKey)
            } else {
                Err(OpenError::Failed(format!("dislocker could not open it (code {}): {}", out.code, first_line(&out.stderr))))
            };
        }
        let mut largs: Vec<String> = vec!["--find".into(), "--show".into()];
        if req.read_only {
            largs.push("--read-only".into());
        }
        largs.push(file);
        let l = match self.sys.run(&losetup, &largs, None) {
            Ok(o) if o.code == 0 && o.stdout.trim().starts_with("/dev/loop") => o.stdout.trim().to_string(),
            Ok(o) => {
                self.undo_fuse(&dir);
                return Err(OpenError::Failed(format!("The unlocked image could not be attached (losetup, code {}): {}", o.code, first_line(&o.stderr))));
            }
            Err(e) => {
                self.undo_fuse(&dir);
                return Err(OpenError::Failed(e));
            }
        };
        if let Err(e) = self.sys.symlink(&l, &mapper_path(req.part)) {
            let _ = self.sys.run(&losetup, &["-d".into(), l], None);
            self.undo_fuse(&dir);
            return Err(OpenError::Failed(e));
        }
        Ok(())
    }

    fn undo_fuse(&self, dir: &str) {
        for prog in ["fusermount3", "fusermount"] {
            if let Some(p) = self.find(prog) {
                if self.sys.run(&p, &["-u".into(), dir.to_string()], None).is_ok_and(|o| o.code == 0) {
                    break;
                }
            }
        }
        if let Some(p) = self.find("umount") {
            let _ = self.sys.run(&p, &[dir.to_string()], None);
        }
        let _ = self.sys.remove_dir(dir);
    }
}

/// Eight groups of six digits with dashes (or the 48 digits alone).
pub fn looks_like_recovery_key(b: &[u8]) -> bool {
    let digits: Vec<u8> = b.iter().copied().filter(|c| *c != b'-').collect();
    digits.len() == 48 && digits.iter().all(u8::is_ascii_digit) && b.iter().all(|c| c.is_ascii_digit() || *c == b'-')
}

fn first_line(s: &str) -> String {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no message").chars().take(160).collect()
}

impl CryptRunner for ProcRunner {
    fn available(&self, kind: CryptKind) -> Result<(), String> {
        if self.find(kind.helper()).is_some() { Ok(()) } else { Err(missing_words(kind)) }
    }

    fn open(&self, req: &OpenReq, secret: &Secret) -> Result<(), OpenError> {
        if secret.is_empty() {
            return Err(OpenError::Failed(format!("Type the {} first.", req.kind.secret_word())));
        }
        if !req.dev.starts_with("/dev/") || req.dev.contains("..") || req.dev.contains(char::is_whitespace) {
            return Err(OpenError::Failed(format!("{} is not a device path.", req.dev)));
        }
        match req.kind {
            CryptKind::Luks1 | CryptKind::Luks2 => self.open_luks(req, secret),
            CryptKind::BitLocker => self.open_bitlocker(req, secret),
        }
    }

    fn close(&self, kind: CryptKind, part: &str) -> Result<(), String> {
        match kind {
            CryptKind::Luks1 | CryptKind::Luks2 => {
                let prog = self.find("cryptsetup").ok_or_else(|| missing_words(kind))?;
                let out = self.sys.run(&prog, &["close".into(), crate::disks_crypt::mapper_name(part)], None)?;
                match out.code {
                    0 => Ok(()),
                    5 | 4 => Err("The volume is still in use. Close what uses it (or unmount it) and try again.".into()),
                    c => Err(format!("cryptsetup could not close it (code {c}): {}", first_line(&out.stderr))),
                }
            }
            CryptKind::BitLocker => {
                let at = mapper_path(part);
                let dev = self.sys.read_link(&at);
                if let (Some(l), Some(losetup)) = (&dev, self.find("losetup")) {
                    let out = self.sys.run(&losetup, &["-d".into(), l.clone()], None)?;
                    if out.code != 0 {
                        return Err(format!("The unlocked image is still in use (losetup, code {}). Close what uses it and try again.", out.code));
                    }
                }
                let _ = self.sys.remove_file(&at);
                self.undo_fuse(&self.fuse_dir(part));
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// the made-up machine for tests
// ---------------------------------------------------------------------------------------------------------------------

/// One call the fake saw: the program, the arguments and whether a secret went to its standard input (never what it was).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub prog: String,
    pub args: Vec<String>,
    pub stdin_bytes: Option<Vec<u8>>,
}

/// A made-up [`Sys`]: files that exist, scripted answers per program name, a record of every call.
#[derive(Default)]
pub struct FakeSys {
    pub files: Mutex<Vec<String>>,
    pub answers: Mutex<Vec<(String, ProcOut)>>,
    pub seen: Mutex<Vec<Seen>>,
    pub links: Mutex<Vec<(String, String)>>,
    pub dirs: Mutex<Vec<String>>,
    /// Paths that `exists` reports (the FUSE file dislocker would make).
    pub present: Mutex<Vec<String>>,
}

impl FakeSys {
    pub fn with_programs(names: &[&str]) -> Arc<FakeSys> {
        let f = FakeSys::default();
        *f.files.lock().unwrap() = names.iter().map(|n| format!("/usr/bin/{n}")).collect();
        Arc::new(f)
    }
    /// The next call of `prog` answers `out`; with none left it answers success with no output.
    pub fn answer(&self, prog: &str, code: i32, stdout: &str, stderr: &str) {
        self.answers.lock().unwrap().push((prog.to_string(), ProcOut { code, stdout: stdout.into(), stderr: stderr.into() }));
    }
    pub fn calls(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

impl Sys for FakeSys {
    fn is_file(&self, path: &str) -> bool {
        self.files.lock().unwrap().iter().any(|f| f == path)
    }
    fn run(&self, prog: &str, args: &[String], stdin: Option<&[u8]>) -> Result<ProcOut, String> {
        self.seen.lock().unwrap().push(Seen { prog: prog.to_string(), args: args.to_vec(), stdin_bytes: stdin.map(<[u8]>::to_vec) });
        let base = prog.rsplit('/').next().unwrap_or(prog);
        let mut a = self.answers.lock().unwrap();
        match a.iter().position(|(p, _)| p == base) {
            Some(i) => Ok(a.remove(i).1),
            None => Ok(ProcOut::default()),
        }
    }
    fn symlink(&self, target: &str, at: &str) -> Result<(), String> {
        self.links.lock().unwrap().push((at.to_string(), target.to_string()));
        Ok(())
    }
    fn read_link(&self, at: &str) -> Option<String> {
        self.links.lock().unwrap().iter().find(|(a, _)| a == at).map(|(_, t)| t.clone())
    }
    fn remove_file(&self, at: &str) -> Result<(), String> {
        self.links.lock().unwrap().retain(|(a, _)| a != at);
        Ok(())
    }
    fn make_private_dir(&self, path: &str) -> Result<(), String> {
        self.dirs.lock().unwrap().push(path.to_string());
        Ok(())
    }
    fn remove_dir(&self, path: &str) -> Result<(), String> {
        self.dirs.lock().unwrap().retain(|d| d != path);
        Ok(())
    }
    fn exists(&self, path: &str) -> bool {
        self.present.lock().unwrap().iter().any(|p| p == path)
    }
}

/// A runner for the engine's and the tile's tests: it does what `cryptsetup` would from the engine's point of view (the opened device
/// appears as `/dev/mapper/iyakki-<part>` holding a made-up file system) without running anything. The right key is set per
/// partition; a wrong one is [`OpenError::WrongKey`].
pub struct FakeCrypt {
    pub env: Arc<crate::disks_fake::Fake>,
    /// (partition, the key that opens it, the first bytes of what is inside).
    pub volumes: Mutex<Vec<(String, String, Vec<u8>)>>,
    /// Helpers that are missing.
    pub missing: Mutex<Vec<CryptKind>>,
    pub log: Mutex<Vec<String>>,
    pub busy_close: Mutex<bool>,
}

impl FakeCrypt {
    pub fn new(env: Arc<crate::disks_fake::Fake>) -> Arc<FakeCrypt> {
        Arc::new(FakeCrypt { env, volumes: Mutex::new(Vec::new()), missing: Mutex::new(Vec::new()), log: Mutex::new(Vec::new()), busy_close: Mutex::new(false) })
    }
    pub fn add(&self, part: &str, key: &str, inner: Vec<u8>) {
        self.volumes.lock().unwrap().push((part.into(), key.into(), inner));
    }
    pub fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl CryptRunner for FakeCrypt {
    fn available(&self, kind: CryptKind) -> Result<(), String> {
        if self.missing.lock().unwrap().contains(&kind) { Err(missing_words(kind)) } else { Ok(()) }
    }
    fn open(&self, req: &OpenReq, secret: &Secret) -> Result<(), OpenError> {
        self.log.lock().unwrap().push(format!("open {} ro={}", req.part, req.read_only));
        let vols = self.volumes.lock().unwrap();
        let Some((_, key, inner)) = vols.iter().find(|(p, _, _)| p == req.part) else { return Err(OpenError::Failed("no such volume".into())) };
        if key.as_bytes() != secret.expose() {
            return Err(OpenError::WrongKey);
        }
        let mut st = self.env.st.lock().unwrap();
        let path = mapper_path(req.part);
        let mut data = inner.clone();
        data.resize(data.len().max(4096), 0);
        st.devs.insert(path.clone(), data);
        st.canon.insert(path.clone(), path);
        Ok(())
    }
    fn close(&self, _kind: CryptKind, part: &str) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("close {part}"));
        if *self.busy_close.lock().unwrap() {
            return Err("The volume is still in use. Close what uses it (or unmount it) and try again.".into());
        }
        let mut st = self.env.st.lock().unwrap();
        let path = mapper_path(part);
        st.devs.remove(&path);
        st.canon.remove(&path);
        Ok(())
    }
}

/// Where `/dev/mapper/...` of a partition would be, as a `PathBuf` (for messages).
pub fn node(part: &str) -> PathBuf {
    PathBuf::from(mapper_path(part))
}
