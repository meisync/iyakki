//! The latency benchmark of the UI socket (F-SPLIT-03): the round trip of a small call and the hand-over of a large block, over a real unix
//! socket. The test asserts what is CORRECT (every answer is the right one, the block comes back equal) and prints the numbers; it asserts
//! no time limit, because a limit is a flaky test. The numbers go to the test output (`--nocapture`) and to `target/ui-socket-bench.txt` when
//! `IYAKKI_BENCH_OUT` names a file.

#![cfg(target_os = "linux")]

use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_core_api::contract::RateLimit;
use iyakki_core_fake::contract::FakeSettings;
use iyakki_ui_client::client::Connection;
use iyakki_ui_client::ipc;
use iyakki_ui_client::server::{serve_unix_for, settings_hello, Served};
use iyakki_ui_client::*;

fn short_dir() -> std::path::PathBuf {
    // a unix socket path must stay under about 100 bytes: a short folder of its own
    let d = std::path::PathBuf::from("/tmp").join(format!("iyk-b{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn small_calls_round_trip_and_a_large_block_is_handed_over_equal_and_both_are_measured() {
    let dir = short_dir();
    let path = dir.join("u.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let served = Served {
        hello: settings_hello("bench", RateLimit { calls: 1_000_000, per_ms: 1000 }),
        settings: Some(Arc::new(FakeSettings::sample())),
        vault: None,
        flows: None,
        assistant: None,
        cameras: None,
        rpc: None, appevents: None, ctx: CallContext::remote("bench", Transport::LocalSocket, &[Scope::SETTINGS_READ]),
    };
    // SAFETY: getuid cannot fail.
    let me = unsafe { libc::getuid() };
    let (s2, stop2) = (Arc::new(served), stop.clone());
    let server = std::thread::spawn(move || serve_unix_for(listener, s2, stop2, Some(me)).unwrap());

    let mut c = Connection::unix(&path, Duration::from_secs(10)).unwrap();
    const N: usize = 2000;
    let mut each = Vec::with_capacity(N);
    for _ in 0..N {
        let t = Instant::now();
        let v = c.call("settings.get", serde_json::json!({"key": "display.dark"})).unwrap();
        each.push(t.elapsed());
        assert_eq!(v["type"], "bool", "every answer is the right one");
    }
    each.sort();
    let (p50, p99, max) = (each[N / 2], each[N * 99 / 100], each[N - 1]);

    // the large block: 32 MB (a 4K frame) handed over as a memory file, against the same bytes sent through a plain stream
    let block: Vec<u8> = (0..32_000_000u32).map(|i| (i % 253) as u8).collect();
    let (a, b) = UnixStream::pair().unwrap();
    let t = Instant::now();
    let fd = ipc::make_bulk("frame", &block).unwrap();
    ipc::send_with_fd(&a, "frame 4000x2000 xrgb\n", fd.as_raw_fd()).unwrap();
    let (line, got) = ipc::recv_with_fd(&b, 256).unwrap();
    let handed = ipc::read_bulk(got.unwrap()).unwrap();
    let t_memfd = t.elapsed();
    assert_eq!(line, "frame 4000x2000 xrgb\n");
    assert!(handed == block, "the handed-over block is the one that was made");
    let (mut w, mut r) = UnixStream::pair().unwrap();
    let copy = block.clone();
    let t = Instant::now();
    let writer = std::thread::spawn(move || {
        use std::io::Write;
        w.write_all(&copy).unwrap();
    });
    let mut back = vec![0u8; block.len()];
    std::io::Read::read_exact(&mut r, &mut back).unwrap();
    writer.join().unwrap();
    let t_stream = t.elapsed();
    assert!(back == block);

    let report = format!("ui socket bench: {N} calls round trip p50 {p50:?} p99 {p99:?} max {max:?}; 32 MB block: memory file hand-over incl. read {t_memfd:?}, plain stream copy {t_stream:?}\n");
    eprint!("{report}");
    if let Ok(out) = std::env::var("IYAKKI_BENCH_OUT") {
        let _ = std::fs::write(out, report);
    }
    drop(c);
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_peer_that_is_not_root_or_the_ui_user_is_closed_at_once() {
    let dir = short_dir().join("p");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("u.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let served = Served { hello: settings_hello("bench", RateLimit { calls: 100, per_ms: 1000 }), settings: Some(Arc::new(FakeSettings::sample())), vault: None, flows: None, assistant: None, cameras: None, rpc: None, appevents: None, ctx: CallContext::remote("bench", Transport::LocalSocket, &[Scope::SETTINGS_READ]) };
    // SAFETY: getuid cannot fail.
    let me = unsafe { libc::getuid() };
    // the UI user is somebody else; a test that runs as root is let in anyway, so it only checks what it can
    let other = me.wrapping_add(4242);
    let (s2, stop2) = (Arc::new(served), stop.clone());
    let server = std::thread::spawn(move || serve_unix_for(listener, s2, stop2, Some(other)).unwrap());
    let mut c = Connection::unix(&path, Duration::from_secs(10)).unwrap();
    let r = c.hello();
    if me == 0 {
        assert!(r.is_ok(), "root is let in");
    } else {
        assert!(r.is_err(), "another user is closed out");
    }
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

struct Frames;
impl iyakki_ui_client::server::BulkSource for Frames {
    fn get(&self, ctx: &CallContext, name: &str) -> CallResult<Vec<u8>> {
        if !ctx.allows(Scope::SETTINGS_READ) {
            return Err(CallError::new(ErrorCode::Denied, "test.denied", "no scope"));
        }
        match name {
            "frame" => Ok((0..3_000_000u32).map(|i| (i % 249) as u8).collect()),
            _ => Err(CallError::new(ErrorCode::NotFound, "test.no_such_block", "no such block")),
        }
    }
}

#[test]
fn a_block_is_asked_for_by_name_and_arrives_as_a_memory_file_beside_the_answer_and_a_missing_one_is_an_error() {
    let dir = short_dir().join("b");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("u.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let served = Served { hello: settings_hello("bulk", RateLimit { calls: 1000, per_ms: 1000 }), settings: Some(Arc::new(FakeSettings::sample())), vault: None, flows: None, assistant: None, cameras: None, rpc: None, appevents: None, ctx: CallContext::remote("bulk", Transport::LocalSocket, &[Scope::SETTINGS_READ]) };
    let (s2, stop2) = (Arc::new(served), stop.clone());
    let server = std::thread::spawn(move || iyakki_ui_client::server::serve_unix_bulk(listener, s2, stop2, None, Some(Arc::new(Frames))).unwrap());
    let mut c = Connection::unix(&path, Duration::from_secs(10)).unwrap();
    let b = c.bulk_get("frame").unwrap();
    assert_eq!(b.len(), 3_000_000);
    assert!(b.iter().enumerate().all(|(i, v)| *v == (i as u32 % 249) as u8));
    assert_eq!(c.bulk_get("nothing").unwrap_err().code, ErrorCode::NotFound);
    // the connection goes on with ordinary calls after a block
    assert_eq!(c.call("settings.get", serde_json::json!({"key": "display.dark"})).unwrap()["type"], "bool");
    drop(c);
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}
