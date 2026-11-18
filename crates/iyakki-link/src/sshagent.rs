//! Signing with a key that only an `ssh-agent` holds (F-CONN-04), in-house: the agent protocol (draft-miller-ssh-agent) over the Unix
//! socket in `SSH_AUTH_SOCK`. Only `ssh-ed25519` identities are used. Whatever the agent answers is checked here against the public
//! key, so a wrong or hostile agent cannot make a signature that is accepted by mistake. What is signed is the `IYAKKI-SIG` text
//! (`iyakki-vault-v2` ...), which can never be an SSH login signature (those start with a session identifier).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ed25519_dalek::{Signature, VerifyingKey};

use crate::sigscheme::{put_string, Rd};

const FAILURE: u8 = 5;
const REQUEST_IDENTITIES: u8 = 11;
const IDENTITIES_ANSWER: u8 = 12;
const SIGN_REQUEST: u8 = 13;
const SIGN_RESPONSE: u8 = 14;
/// A signature answer is about a hundred bytes; anything over this is not an agent (KI-109).
const MAX_SIGN_FRAME: usize = 16 * 1024;
/// A key list holds a hundred bytes a key and at most 1024 keys are read; anything over this is not an agent.
const MAX_LIST_FRAME: usize = 64 * 1024;
/// The whole exchange (connect, request, answer) must be over in this long: a stalling or slow-dripping agent cannot hold a sign-in longer.
const DEADLINE: Duration = Duration::from_secs(5);

/// One ssh-ed25519 identity an agent offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub public: [u8; 32],
    pub comment: String,
}

fn frame(body: &[u8]) -> Vec<u8> {
    let mut v = (body.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(body);
    v
}

pub fn identities_request() -> Vec<u8> {
    frame(&[REQUEST_IDENTITIES])
}

pub fn sign_request(public: &[u8; 32], data: &[u8]) -> Vec<u8> {
    let mut blob = Vec::new();
    put_string(&mut blob, b"ssh-ed25519");
    put_string(&mut blob, public);
    let mut b = vec![SIGN_REQUEST];
    put_string(&mut b, &blob);
    put_string(&mut b, data);
    b.extend_from_slice(&0u32.to_be_bytes()); // flags: none (plain ed25519)
    frame(&b)
}

/// The ssh-ed25519 identities in an `IDENTITIES_ANSWER` body (keys of other types are skipped).
pub fn parse_identities(body: &[u8]) -> Result<Vec<Identity>, String> {
    match body.first() {
        Some(&IDENTITIES_ANSWER) => {}
        Some(&FAILURE) => return Err("the agent refused to list its keys".into()),
        _ => return Err("the agent gave an answer that is not a key list".into()),
    }
    let mut r = Rd(&body[1..]);
    let n = r.u32().ok_or("the key list is cut short")?;
    let mut out = Vec::new();
    for _ in 0..n.min(1024) {
        let blob = r.string().ok_or("the key list is cut short")?;
        let comment = r.string().ok_or("the key list is cut short")?;
        let mut b = Rd(blob);
        if b.string() == Some(b"ssh-ed25519") {
            if let Some(k) = b.string().and_then(|k| <[u8; 32]>::try_from(k).ok()) {
                out.push(Identity { public: k, comment: String::from_utf8_lossy(comment).chars().filter(|c| !c.is_control()).take(64).collect() });
            }
        }
    }
    Ok(out)
}

/// The 64-byte signature in a `SIGN_RESPONSE` body.
pub fn parse_sign_response(body: &[u8]) -> Result<[u8; 64], String> {
    match body.first() {
        Some(&SIGN_RESPONSE) => {}
        Some(&FAILURE) => return Err("the agent would not sign (the key may be locked or need a confirmation on its side)".into()),
        _ => return Err("the agent gave an answer that is not a signature".into()),
    }
    let mut r = Rd(&body[1..]);
    let sig = r.string().ok_or("the signature is cut short")?;
    let mut s = Rd(sig);
    if s.string() != Some(b"ssh-ed25519") {
        return Err("the agent signed with another algorithm".into());
    }
    s.string().and_then(|b| <[u8; 64]>::try_from(b).ok()).ok_or_else(|| "the signature is not 64 bytes".to_string())
}

/// What is left of the deadline, or the words for having run out.
fn left(end: Instant) -> Result<Duration, String> {
    end.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()).ok_or_else(|| "the ssh-agent took too long to answer".to_string())
}

/// Fill `buf` from the agent before `end`, whatever the pace of the bytes.
fn read_by(s: &mut UnixStream, buf: &mut [u8], end: Instant) -> Result<(), String> {
    let mut got = 0;
    while got < buf.len() {
        let _ = s.set_read_timeout(Some(left(end)?));
        match s.read(&mut buf[got..]) {
            Ok(0) => return Err("the ssh-agent closed the connection before it finished its answer".into()),
            Ok(n) => got += n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return Err("the ssh-agent took too long to answer".into()),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("the ssh-agent: {e}")),
        }
    }
    Ok(())
}

/// An `ssh-agent` reached over a Unix socket.
#[derive(Debug)]
pub struct Agent {
    sock: PathBuf,
}

impl Agent {
    pub fn at(sock: &Path) -> Agent {
        Agent { sock: sock.to_path_buf() }
    }

    /// The agent named by `SSH_AUTH_SOCK` (given, so tests need no environment).
    pub fn from_env_value(v: Option<&str>) -> Result<Agent, String> {
        match v.filter(|v| !v.is_empty()) {
            Some(p) => Ok(Agent::at(Path::new(p))),
            None => Err("no ssh-agent is running (SSH_AUTH_SOCK is not set): start one and add the key with ssh-add".into()),
        }
    }

    pub fn sock(&self) -> &Path {
        &self.sock
    }

    fn call(&self, req: &[u8], max_frame: usize) -> Result<Vec<u8>, String> {
        let end = Instant::now() + DEADLINE;
        let mut s = UnixStream::connect(&self.sock).map_err(|e| format!("the ssh-agent at {} did not answer: {e}", self.sock.display()))?;
        // (a failure to set a timeout is ignored: macOS refuses it on a socket whose peer has already hung up, and a read then ends at once)
        let _ = s.set_write_timeout(Some(left(end)?));
        s.write_all(req).map_err(|e| format!("the ssh-agent: {e}"))?;
        let mut len = [0u8; 4];
        read_by(&mut s, &mut len, end)?;
        let n = u32::from_be_bytes(len) as usize;
        if n == 0 || n > max_frame {
            return Err("the ssh-agent sent a frame of a wrong size".into());
        }
        let mut body = vec![0u8; n];
        read_by(&mut s, &mut body, end)?;
        Ok(body)
    }

    pub fn ed25519_keys(&self) -> Result<Vec<Identity>, String> {
        parse_identities(&self.call(&identities_request(), MAX_LIST_FRAME)?)
    }

    /// Ask the agent to sign `data` with `public`, and check the answer against the public key.
    pub fn sign(&self, public: &[u8; 32], data: &[u8]) -> Result<[u8; 64], String> {
        let sig = parse_sign_response(&self.call(&sign_request(public, data), MAX_SIGN_FRAME)?)?;
        let vk = VerifyingKey::from_bytes(public).map_err(|_| "not a valid Ed25519 key".to_string())?;
        vk.verify_strict(data, &Signature::from_bytes(&sig)).map_err(|_| "the agent's signature does not match the key: refused".to_string())?;
        Ok(sig)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sigscheme::DeviceKey;
    use std::os::unix::net::UnixListener;

    /// A pretend agent holding `key`: answers one request per connection, `n` times.
    fn fake_agent(dir: &Path, key: DeviceKey, lie: bool, n: usize) -> PathBuf {
        let sock = dir.join("agent.sock");
        let l = UnixListener::bind(&sock).unwrap();
        std::thread::spawn(move || {
            for _ in 0..n {
                let Ok((mut c, _)) = l.accept() else { return };
                let mut len = [0u8; 4];
                if c.read_exact(&mut len).is_err() {
                    continue;
                }
                let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
                let _ = c.read_exact(&mut body);
                let reply = match body[0] {
                    REQUEST_IDENTITIES => {
                        let mut r = vec![IDENTITIES_ANSWER];
                        r.extend_from_slice(&2u32.to_be_bytes());
                        // an RSA-looking key first: skipped
                        let mut rsa = Vec::new();
                        put_string(&mut rsa, b"ssh-rsa");
                        put_string(&mut rsa, b"xx");
                        put_string(&mut r, &rsa);
                        put_string(&mut r, b"old");
                        let mut blob = Vec::new();
                        put_string(&mut blob, b"ssh-ed25519");
                        put_string(&mut blob, &key.public());
                        put_string(&mut r, &blob);
                        put_string(&mut r, b"laptop");
                        r
                    }
                    SIGN_REQUEST => {
                        let mut rd = Rd(&body[1..]);
                        let _blob = rd.string().unwrap();
                        let data = rd.string().unwrap().to_vec();
                        let sig = if lie { [7u8; 64] } else { key.sign_bytes(&data).unwrap() };
                        let mut inner = Vec::new();
                        put_string(&mut inner, b"ssh-ed25519");
                        put_string(&mut inner, &sig);
                        let mut r = vec![SIGN_RESPONSE];
                        put_string(&mut r, &inner);
                        r
                    }
                    _ => vec![FAILURE],
                };
                let _ = c.write_all(&frame(&reply));
            }
        });
        sock
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-agent-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_agent_lists_its_ed25519_keys_and_signs_and_the_request_verifies() {
        let d = tmp("ok");
        let secret = DeviceKey::from_seed([9; 32]);
        let public = secret.public();
        let sock = fake_agent(&d, secret, false, 3);
        let a = Agent::at(&sock);
        let ids = a.ed25519_keys().unwrap();
        assert_eq!(ids, vec![Identity { public, comment: "laptop".into() }]);
        let key = DeviceKey::from_agent(public, sock);
        assert!(key.is_agent() && key.seed_hex().is_none() && key.public() == public);
        let h = key.sign_request("ann", "POST", "/api/login-key", 1000, b"").unwrap();
        let r = crate::sigscheme::parse_authorization(&h).unwrap();
        assert!(r.verify(&public, "POST", "/api/login-key", b""));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_lying_agent_is_refused_and_a_missing_agent_is_explained() {
        let d = tmp("lie");
        let secret = DeviceKey::from_seed([9; 32]);
        let public = secret.public();
        let sock = fake_agent(&d, secret, true, 1);
        let e = Agent::at(&sock).sign(&public, b"data").unwrap_err();
        assert!(e.contains("does not match"), "{e}");
        assert!(Agent::from_env_value(None).unwrap_err().contains("SSH_AUTH_SOCK"));
        assert!(Agent::at(&d.join("nothing.sock")).ed25519_keys().unwrap_err().contains("did not answer"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn answers_that_are_not_what_was_asked_are_errors() {
        assert!(parse_identities(&[FAILURE]).is_err() && parse_identities(&[]).is_err() && parse_identities(&[IDENTITIES_ANSWER, 0, 0, 0, 5]).is_err());
        assert!(parse_sign_response(&[FAILURE]).unwrap_err().contains("would not sign"));
        assert!(parse_sign_response(&[SIGN_RESPONSE, 0, 0, 0]).is_err());
        let mut inner = Vec::new();
        put_string(&mut inner, b"ssh-rsa");
        put_string(&mut inner, &[0; 64]);
        let mut r = vec![SIGN_RESPONSE];
        put_string(&mut r, &inner);
        assert!(parse_sign_response(&r).unwrap_err().contains("another algorithm"));
    }
}

#[cfg(test)]
#[path = "sshagent_fuzz_tests.rs"]
mod fuzz;
