//! Fuzz tests (deterministic, seeded) for the ssh-agent answers: mutated key lists and signature replies through the parsers, and through a
//! real Unix socket with a pretend agent that sends mutated frames. Whatever the agent says, a signature is accepted only if it is the
//! valid signature of the data by that key; the key list is bounded; nothing panics or hangs. `IYAKKI_FUZZ_CASES=20000` runs more.

use super::*;
use crate::fuzzkit::{cases, mutate, run, Rng};
use crate::sigscheme::DeviceKey;
use std::os::unix::net::UnixListener;
use std::sync::mpsc;

fn key_list(n: usize) -> Vec<u8> {
    let mut r = vec![IDENTITIES_ANSWER];
    r.extend_from_slice(&(n as u32 + 1).to_be_bytes());
    let mut rsa = Vec::new();
    put_string(&mut rsa, b"ssh-rsa");
    put_string(&mut rsa, b"xx");
    put_string(&mut r, &rsa);
    put_string(&mut r, b"old");
    for i in 0..n {
        let key = DeviceKey::from_seed([i as u8 + 1; 32]);
        let mut blob = Vec::new();
        put_string(&mut blob, b"ssh-ed25519");
        put_string(&mut blob, &key.public());
        put_string(&mut r, &blob);
        put_string(&mut r, format!("laptop {i}\u{7}\u{1b}[31m").as_bytes());
    }
    r
}

fn sign_reply(sig: &[u8; 64]) -> Vec<u8> {
    let mut inner = Vec::new();
    put_string(&mut inner, b"ssh-ed25519");
    put_string(&mut inner, sig);
    let mut r = vec![SIGN_RESPONSE];
    put_string(&mut r, &inner);
    r
}

#[test]
fn fuzz_sshagent_answers_through_the_parsers() {
    let key = DeviceKey::from_seed([9; 32]);
    let sig = key.sign_bytes(b"data").unwrap();
    let corpus = vec![key_list(3), key_list(0), sign_reply(&sig), vec![FAILURE], vec![IDENTITIES_ANSWER, 0xff, 0xff, 0xff, 0xff]];
    let n = run("sshagent parsers", 0xA6E, &corpus, 15000, Duration::from_secs(1), 32 << 20, |input| {
        if let Ok(ids) = parse_identities(input) {
            assert!(ids.len() <= 1024, "{} identities", ids.len());
            for i in ids {
                assert!(i.comment.chars().count() <= 64 && !i.comment.chars().any(|c| c.is_control()), "comment {:?}", i.comment);
            }
        }
        if let Ok(s) = parse_sign_response(input) {
            assert_eq!(s.len(), 64);
        }
    });
    assert!(n >= cases(15000));
}

/// A pretend agent that sends the next canned reply to every connection, byte for byte.
fn canned_agent(dir: &Path) -> (PathBuf, mpsc::Sender<Vec<u8>>) {
    let sock = dir.join("agent.sock");
    let l = UnixListener::bind(&sock).unwrap();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        for reply in rx {
            let Ok((mut c, _)) = l.accept() else { return };
            let mut len = [0u8; 4];
            if c.read_exact(&mut len).is_ok() {
                let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
                let _ = c.read_exact(&mut body);
            }
            let _ = c.write_all(&reply);
            // dropped: the connection closes, so a frame that promises more than it sends ends at once
        }
    });
    (sock, tx)
}

#[test]
fn fuzz_sshagent_a_signature_is_accepted_only_if_it_is_right() {
    let d = std::env::temp_dir().join(format!("iyakki-agent-fuzz-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let (sock, tx) = canned_agent(&d);
    let agent = Agent::at(&sock);
    let key = DeviceKey::from_seed([9; 32]);
    let data = b"IYAKKI-SIG\niyakki-vault-v2\nfuzz";
    let good = key.sign_bytes(data).unwrap();
    let framed = |body: &[u8]| frame(body);
    let corpus = vec![framed(&sign_reply(&good)), framed(&key_list(2)), framed(&[FAILURE]), framed(&sign_reply(&[7; 64]))];
    let mut r = Rng::new(0xA6F);
    let mut accepted = 0;
    for case in 0..cases(300) {
        let base = r.pick(&corpus).clone();
        let m = if r.chance(5) { base } else { mutate(&mut r, &base) };
        tx.send(m.clone()).unwrap();
        let t = std::time::Instant::now();
        match agent.sign(&key.public(), data) {
            Ok(sig) => {
                assert_eq!(sig, good, "case {case}: an agent answer {:02x?} gave a signature that is not the right one", &m[..m.len().min(40)]);
                accepted += 1;
            }
            Err(e) => assert!(!e.is_empty()),
        }
        assert!(t.elapsed() < Duration::from_secs(5), "case {case} took {:?}", t.elapsed());
        tx.send(m).unwrap();
        let _ = agent.ed25519_keys();
    }
    assert!(accepted > 0, "the unmutated good answer must be accepted");
    drop(tx);
    let _ = std::fs::remove_dir_all(&d);
}

/// A pretend agent that reads the request and then does `act` with the connection (KI-109).
fn odd_agent(tag: &str, act: impl FnOnce(UnixStream) + Send + 'static) -> (PathBuf, std::thread::JoinHandle<()>) {
    let d = std::env::temp_dir().join(format!("iyakki-agent-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let sock = d.join("agent.sock");
    let l = UnixListener::bind(&sock).unwrap();
    let h = std::thread::spawn(move || {
        let Ok((mut c, _)) = l.accept() else { return };
        let mut len = [0u8; 4];
        if c.read_exact(&mut len).is_ok() {
            let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
            let _ = c.read_exact(&mut body);
        }
        act(c);
    });
    (sock, h)
}

#[test]
fn fuzz_sshagent_regression_an_agent_that_says_nothing_is_given_up_on_after_five_seconds() {
    let (sock, h) = odd_agent("stall", |c| {
        std::thread::sleep(Duration::from_secs(8));
        drop(c);
    });
    let t = Instant::now();
    let e = Agent::at(&sock).sign(&DeviceKey::from_seed([9; 32]).public(), b"data").unwrap_err();
    let took = t.elapsed();
    assert!(e.contains("too long"), "{e}");
    assert!(took >= Duration::from_secs(4) && took < Duration::from_secs(7), "{took:?}");
    let _ = h.join();
}

#[test]
fn fuzz_sshagent_regression_a_slow_drip_does_not_restart_the_clock_on_every_byte() {
    // 1 byte a second, every read inside the old 10-second limit; the whole answer now has 5 seconds
    let (sock, h) = odd_agent("drip", |mut c| {
        let _ = c.write_all(&100u32.to_be_bytes());
        for _ in 0..30 {
            if c.write_all(&[SIGN_RESPONSE]).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
    let t = Instant::now();
    let e = Agent::at(&sock).ed25519_keys().unwrap_err();
    let took = t.elapsed();
    assert!(e.contains("too long"), "{e}");
    assert!(took < Duration::from_secs(7), "{took:?}");
    let _ = h.join();
}

#[test]
fn fuzz_sshagent_regression_the_reply_caps() {
    // a signature answer over 16 KB is not an agent; a key list may be bigger (up to 64 KB), a longer one is refused too
    let frame_of = |n: usize| {
        let mut v = (n as u32).to_be_bytes().to_vec();
        v.extend(std::iter::repeat_n(FAILURE, n));
        v
    };
    let key = DeviceKey::from_seed([9; 32]);
    for (size, sign_ok, list_ok) in [(1000usize, true, true), (20_000, false, true), (70_000, false, false)] {
        let big = frame_of(size);
        let (sock, h) = odd_agent("cap1", move |mut c| {
            let _ = c.write_all(&big);
        });
        let e = Agent::at(&sock).sign(&key.public(), b"d").unwrap_err();
        assert_eq!(e.contains("wrong size"), !sign_ok, "sign {size}: {e}");
        let _ = h.join();
        let big = frame_of(size);
        let (sock, h) = odd_agent("cap2", move |mut c| {
            let _ = c.write_all(&big);
        });
        let e = Agent::at(&sock).ed25519_keys().unwrap_err();
        assert_eq!(e.contains("wrong size"), !list_ok, "list {size}: {e}");
        let _ = h.join();
    }
}
