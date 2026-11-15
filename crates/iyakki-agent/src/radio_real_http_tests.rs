//! The radio player against a real, independent HTTP server (Python's `http.server`, started on loopback by the test) instead of the fake streamer:
//! an MP3 stream and an HLS playlist with two MP3 segments are fetched with the real network code, decoded by our MP3 decoder, and the sound
//! reaches the play command. Skipped (with a message) when there is no `python3`.

use crate::radio::{RadioPlayer, RadioState, Station};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Server {
    child: Child,
    port: u16,
    dir: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn silence(frames: usize) -> Vec<u8> {
    let mut v = Vec::new();
    for _ in 0..frames {
        let mut f = vec![0u8; 417];
        f[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
        v.extend_from_slice(&f);
    }
    v
}

fn serve(tag: &str, files: &[(&str, Vec<u8>)]) -> Option<Server> {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skipped: no python3 on this machine");
        return None;
    }
    let dir = std::env::temp_dir().join(format!("iyakki-radio-py-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (n, b) in files {
        std::fs::write(dir.join(n), b).unwrap();
    }
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let child = Command::new("python3").args(["-m", "http.server", &port.to_string(), "--bind", "127.0.0.1"]).current_dir(&dir).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()?;
    let s = Server { child, port, dir };
    let t0 = Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        if t0.elapsed() > Duration::from_secs(10) {
            eprintln!("skipped: the Python server did not start");
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Some(s)
}

fn play_and_collect(station: &Station, out: &std::path::Path, want_bytes: u64) -> Vec<RadioState> {
    // the sound must go to the play command named here, not to a mixer another test turned on
    let _route = crate::mixer::pin_command_route();
    let p = RadioPlayer::default();
    p.set_streamer(Arc::new(iyakki_proxy::https::UreqStreamer::default()));
    let allow: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(|_| true);
    p.play(station, allow, &format!("tee {}", out.display()));
    let mut seen: Vec<RadioState> = Vec::new();
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(40) {
        let s = p.state();
        if seen.last() != Some(&s) {
            seen.push(s);
        }
        if std::fs::metadata(out).map_or(0, |m| m.len()) >= want_bytes {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    p.stop();
    seen
}

#[test]
fn an_mp3_stream_from_a_real_http_server_is_fetched_decoded_and_played() {
    let frames = 60;
    let Some(srv) = serve("mp3", &[("live.mp3", silence(frames))]) else { return };
    let out = srv.dir.join("pcm.wav");
    let st = Station::new("Loopback FM", &format!("http://127.0.0.1:{}/live.mp3", srv.port));
    // a frame of 1152 stereo samples is 4608 bytes of sound; the player writes a WAV header first
    let seen = play_and_collect(&st, &out, 44 + 40 * 4608);
    assert!(seen.iter().any(|s| matches!(s, RadioState::Playing { station, .. } if station == "Loopback FM")), "{seen:?}");
    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(&bytes[..4], b"RIFF", "the play command got a WAV");
    assert!(bytes.len() as u64 >= 44 + 40 * 4608, "the decoded sound reached the play command: {} bytes", bytes.len());
    // silence in, silence out: every sample after the header is zero
    assert!(bytes[44..].iter().all(|b| *b == 0));
}

#[test]
fn an_hls_playlist_with_mp3_segments_from_a_real_http_server_is_followed_to_its_end() {
    let list = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:1.0,\nseg0.mp3\n#EXTINF:1.0,\nseg1.mp3\n#EXT-X-ENDLIST\n";
    let Some(srv) = serve("hls", &[("hls.m3u8", list.as_bytes().to_vec()), ("seg0.mp3", silence(40)), ("seg1.mp3", silence(40))]) else { return };
    let out = srv.dir.join("pcm.wav");
    let st = Station::new("Loopback HLS", &format!("http://127.0.0.1:{}/hls.m3u8", srv.port));
    // both segments: 80 frames of sound
    let seen = play_and_collect(&st, &out, 44 + 70 * 4608);
    assert!(seen.iter().any(|s| matches!(s, RadioState::Playing { station, .. } if station == "Loopback HLS")), "{seen:?}");
    let len = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    assert!(len >= 44 + 70 * 4608, "the sound of both segments reached the play command: {len} bytes");
}

#[test]
fn a_missing_stream_on_the_real_server_is_a_failure_the_person_can_read() {
    let Some(srv) = serve("missing", &[("other.mp3", silence(5))]) else { return };
    let out = srv.dir.join("pcm.wav");
    let _ = std::fs::File::create(&out).and_then(|mut f| f.write_all(b""));
    let st = Station::new("Nowhere FM", &format!("http://127.0.0.1:{}/nothing.mp3", srv.port));
    let p = RadioPlayer::default();
    p.set_streamer(Arc::new(iyakki_proxy::https::UreqStreamer::default()));
    let allow: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(|_| true);
    p.set_retry_wait(Duration::from_millis(50));
    p.play(&st, allow, "tee /dev/null");
    let t0 = Instant::now();
    let mut why = String::new();
    while t0.elapsed() < Duration::from_secs(20) {
        if let RadioState::Failed { why: w, .. } = p.state() {
            why = w;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    p.stop();
    assert!(why.contains("404"), "the server's 404 is told: {why:?}");
}
