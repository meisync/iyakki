//! A small in-memory ssh-agent for git, and the helper that hands git an HTTPS token without putting it in a URL, a file or a log.
//!
//! One [`GitAgent`] lives for the duration of a git job (or a batch of them). It owns:
//!
//! * `agent.sock`: a unix socket that speaks the ssh-agent protocol (draft-miller-ssh-agent) well enough for OpenSSH's `ssh` to
//!   authenticate with an ed25519 key: *request identities* and *sign request*; every other request gets *failure*. The private keys
//!   exist only in this process's memory (they are wiped when the agent is dropped); a signature leaves, the key does not. Git is
//!   started with `SSH_AUTH_SOCK=<this socket>`.
//! * `askpass.sock` and a symbolic link `iyakki-git-askpass` to the running program: `GIT_ASKPASS` points at the link, which (see
//!   `iyakki-core`) asks the socket for the answer to git's prompt (`Username for 'https://host'`, `Password for 'https://user@host'`)
//!   and prints it. A prompt for a host without a token is answered with an empty line, so git fails instead of waiting for a person.
//!
//! The folder holding the sockets is private (0700) and lives on the data volume, never in a workspace or `/tmp`. When the agent is
//! dropped the sockets and the folder are removed. Nothing here is a login: the person decided to start the job, and every signature
//! is reported to `on_sign` (the caller audits it; the key itself is never reported).

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use zeroize::Zeroizing;

use crate::sshkey::{put_str, put_u32, KeyPair, Reader};

pub const SSH_AGENT_FAILURE: u8 = 5;
pub const SSH_AGENTC_REQUEST_IDENTITIES: u8 = 11;
pub const SSH_AGENT_IDENTITIES_ANSWER: u8 = 12;
pub const SSH_AGENTC_SIGN_REQUEST: u8 = 13;
pub const SSH_AGENT_SIGN_RESPONSE: u8 = 14;
const MAX_MESSAGE: usize = 256 * 1024;
/// A unix socket path must fit in `sun_path` (108 bytes including the NUL).
pub const MAX_SOCKET_PATH: usize = 100;
/// The name the askpass link carries: `iyakki-core` acts as the askpass client when it is started under this name.
pub const ASKPASS_NAME: &str = "iyakki-git-askpass";
pub const ASKPASS_ENV: &str = "IYAKKI_GIT_ASKPASS_SOCK";

/// A key the agent may sign with. `id` is the person's name for it.
#[derive(Clone)]
pub struct AgentKey {
    pub id: String,
    pub pair: KeyPair,
}

/// What `GIT_ASKPASS` may answer: one username and token per HTTPS host.
#[derive(Clone, Default)]
pub struct TokenEntry {
    pub host: String,
    pub user: String,
    pub token: Zeroizing<String>,
}

/// Called with (key id, fingerprint) for every signature made. Never receives key material.
pub type OnSign = Arc<dyn Fn(&str, &str) + Send + Sync>;
/// Called with the host for every token handed out.
pub type OnToken = Arc<dyn Fn(&str) + Send + Sync>;

pub struct GitAgent {
    dir: PathBuf,
    stop: Arc<AtomicBool>,
    handles: Vec<std::thread::JoinHandle<()>>,
}

impl GitAgent {
    /// Starts serving. `dir` is created (0700) and removed again on drop; the sockets are inside it.
    pub fn start(dir: &Path, keys: Vec<AgentKey>, tokens: Vec<TokenEntry>, on_sign: OnSign, on_token: OnToken) -> Result<GitAgent, String> {
        let sock = dir.join("agent.sock");
        let ask = dir.join("askpass.sock");
        if sock.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(format!("the folder for the key socket is too deep ({} bytes, at most {MAX_SOCKET_PATH}): {}", sock.as_os_str().len(), dir.display()));
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&sock);
        let _ = std::fs::remove_file(&ask);
        let l1 = bind_private(&sock)?;
        let l2 = bind_private(&ask)?;
        // the askpass link: the program that is running now, under the name it acts as the client by
        let link = dir.join(ASKPASS_NAME);
        let _ = std::fs::remove_file(&link);
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::os::unix::fs::symlink(exe, &link);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let keys = Arc::new(keys);
        let tokens = Arc::new(tokens);
        let mut handles = Vec::new();
        {
            let (stop, keys, on_sign) = (stop.clone(), keys.clone(), on_sign.clone());
            handles.push(std::thread::spawn(move || accept_loop(l1, stop, move |s| serve_agent(s, &keys, &on_sign))));
        }
        {
            let (stop, tokens, on_token) = (stop.clone(), tokens.clone(), on_token.clone());
            handles.push(std::thread::spawn(move || accept_loop(l2, stop, move |s| serve_askpass(s, &tokens, &on_token))));
        }
        Ok(GitAgent { dir: dir.into(), stop, handles })
    }

    pub fn ssh_sock(&self) -> PathBuf {
        self.dir.join("agent.sock")
    }
    pub fn askpass_sock(&self) -> PathBuf {
        self.dir.join("askpass.sock")
    }
    /// The program `GIT_ASKPASS` is set to.
    pub fn askpass_program(&self) -> PathBuf {
        self.dir.join(ASKPASS_NAME)
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for GitAgent {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn bind_private(path: &Path) -> Result<UnixListener, String> {
    let l = UnixListener::bind(path).map_err(|e| format!("could not open the socket {}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    l.set_nonblocking(true).map_err(|e| e.to_string())?;
    Ok(l)
}

fn accept_loop(l: UnixListener, stop: Arc<AtomicBool>, serve: impl Fn(UnixStream) + Send + Sync + 'static) {
    let serve = Arc::new(serve);
    while !stop.load(Ordering::SeqCst) {
        match l.accept() {
            Ok((s, _)) => {
                let _ = s.set_nonblocking(false);
                let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
                let _ = s.set_write_timeout(Some(Duration::from_secs(30)));
                let serve = serve.clone();
                std::thread::spawn(move || serve(s));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

// ------------------------------------------------------------------ ssh-agent protocol

/// The answer to one agent request (without the length prefix).
pub fn answer(keys: &[AgentKey], on_sign: &OnSign, msg: &[u8]) -> Vec<u8> {
    let fail = vec![SSH_AGENT_FAILURE];
    let Some((&kind, body)) = msg.split_first() else { return fail };
    match kind {
        SSH_AGENTC_REQUEST_IDENTITIES => {
            let mut out = vec![SSH_AGENT_IDENTITIES_ANSWER];
            put_u32(&mut out, keys.len() as u32);
            for k in keys {
                put_str(&mut out, &k.pair.public_blob());
                put_str(&mut out, k.pair.comment.as_bytes());
            }
            out
        }
        SSH_AGENTC_SIGN_REQUEST => {
            let mut r = Reader { b: body };
            let (Some(blob), Some(data)) = (r.string(), r.string()) else { return fail };
            let Some(k) = keys.iter().find(|k| k.pair.public_blob() == blob) else { return fail };
            let sig = k.pair.sign_wire(data);
            on_sign(&k.id, &k.pair.fingerprint());
            let mut out = vec![SSH_AGENT_SIGN_RESPONSE];
            put_str(&mut out, &sig);
            out
        }
        _ => fail,
    }
}

fn serve_agent(mut s: UnixStream, keys: &[AgentKey], on_sign: &OnSign) {
    loop {
        let mut len = [0u8; 4];
        if s.read_exact(&mut len).is_err() {
            return;
        }
        let n = u32::from_be_bytes(len) as usize;
        if n == 0 || n > MAX_MESSAGE {
            return;
        }
        let mut msg = vec![0u8; n];
        if s.read_exact(&mut msg).is_err() {
            return;
        }
        let reply = answer(keys, on_sign, &msg);
        let mut framed = Vec::with_capacity(reply.len() + 4);
        put_u32(&mut framed, reply.len() as u32);
        framed.extend_from_slice(&reply);
        if s.write_all(&framed).is_err() {
            return;
        }
    }
}

// ------------------------------------------------------------------ askpass

/// The host in git's prompt: `Username for 'https://user@host:443': ` gives `host`.
pub fn prompt_host(prompt: &str) -> Option<(bool, String, Option<String>)> {
    let asks_user = prompt.trim_start().to_ascii_lowercase().starts_with("username");
    let q1 = prompt.find('\'')?;
    let q2 = prompt.rfind('\'')?;
    if q2 <= q1 {
        return None;
    }
    let url = &prompt[q1 + 1..q2];
    let rest = url.split_once("://").map_or(url, |x| x.1);
    let authority = rest.split('/').next().unwrap_or("");
    let (user, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u.to_string()), h),
        None => (None, authority),
    };
    let host = hostport.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map_or(hostport, |x| x.0);
    (!host.is_empty()).then(|| (asks_user, host.to_ascii_lowercase(), user))
}

/// What to answer to a prompt, or an empty string when there is nothing for that host.
pub fn askpass_answer(tokens: &[TokenEntry], prompt: &str) -> (String, Option<String>) {
    let Some((asks_user, host, url_user)) = prompt_host(prompt) else { return (String::new(), None) };
    let Some(t) = tokens.iter().find(|t| t.host == host && (url_user.is_none() || url_user.as_deref() == Some(t.user.as_str()) || t.user.is_empty())) else { return (String::new(), None) };
    if asks_user {
        (if t.user.is_empty() { "git".to_string() } else { t.user.clone() }, None)
    } else {
        (t.token.to_string(), Some(host))
    }
}

fn serve_askpass(mut s: UnixStream, tokens: &[TokenEntry], on_token: &OnToken) {
    let mut line = Vec::new();
    let mut b = [0u8; 1];
    while line.len() < 2048 {
        match s.read(&mut b) {
            Ok(1) if b[0] != b'\n' => line.push(b[0]),
            _ => break,
        }
    }
    let prompt = String::from_utf8_lossy(&line).into_owned();
    let (answer, used) = askpass_answer(tokens, &prompt);
    if let Some(h) = used {
        on_token(&h);
    }
    let _ = s.write_all(format!("{answer}\n").as_bytes());
}

/// The client side (what `iyakki-git-askpass` runs): asks the socket named by [`ASKPASS_ENV`] and prints the answer.
pub fn askpass_client(sock: &str, prompt: &str) -> Result<String, String> {
    let mut s = UnixStream::connect(sock).map_err(|e| format!("no askpass socket: {e}"))?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(20)));
    let one_line: String = prompt.chars().filter(|c| *c != '\n' && *c != '\r').collect();
    s.write_all(format!("{one_line}\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut out = String::new();
    s.read_to_string(&mut out).map_err(|e| e.to_string())?;
    Ok(out.trim_end_matches(['\n', '\r']).to_string())
}

/// For `main`: when the program was started as `iyakki-git-askpass <prompt>`, do the askpass job and return the exit code.
pub fn maybe_askpass(argv0: &str, args: &[String]) -> Option<i32> {
    if Path::new(argv0).file_name().and_then(|n| n.to_str()) != Some(ASKPASS_NAME) {
        return None;
    }
    let sock = std::env::var(ASKPASS_ENV).unwrap_or_default();
    let prompt = args.first().cloned().unwrap_or_default();
    match askpass_client(&sock, &prompt) {
        Ok(a) => {
            println!("{a}");
            Some(0)
        }
        Err(_) => Some(1),
    }
}

#[cfg(test)]
#[path = "gitagent_tests.rs"]
mod tests;
