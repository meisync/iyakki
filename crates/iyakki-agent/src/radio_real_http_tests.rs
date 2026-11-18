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

/// IYAKKI's own helper (`iyakki-audiodec`, built by `cargo test -p iyakki-audiodec` into the same target folder), found next to this
/// test program's own folder.
fn helper_program() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    // next to the test program's folder, or a few folders up (the target folder's layout differs between setups)
    exe.ancestors().skip(1).take(5).map(|d| d.join("iyakki-audiodec")).find(|p| p.is_file()).map(|p| p.display().to_string())
}

#[test]
fn an_ogg_vorbis_stream_from_a_real_http_server_goes_through_the_real_helper_and_plays_the_reference_sound() {
    let Some(helper) = helper_program() else {
        eprintln!("skipped: the helper program is not built (cargo test -p iyakki-audiodec builds it)");
        return;
    };
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../iyakki-audiodec/tests/fixtures");
    let ogg = std::fs::read(fixtures.join("stereo-22k.ogg")).unwrap();
    let reference: Vec<i16> = std::fs::read(fixtures.join("stereo-22k.ref.s16")).unwrap().chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
    let Some(srv) = serve("ogg", &[("live.ogg", ogg)]) else { return };
    let out = srv.dir.join("pcm.wav");
    let _route = crate::mixer::pin_command_route();
    let p = RadioPlayer::default();
    p.set_streamer(Arc::new(iyakki_proxy::https::UreqStreamer::default()));
    p.set_aac_factory(Arc::new(move || crate::aacpipe::PipeDecoder::spawn(&helper, crate::aacpipe::Kind::Helper).map(|d| Box::new(d) as Box<dyn crate::audiofmt::AacDecoder>)));
    let allow: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(|_| true);
    p.play(&Station::new("Loopback Ogg", &format!("http://127.0.0.1:{}/live.ogg", srv.port)), allow, &format!("tee {}", out.display()));
    let want = 44 + 2 * 12_000;
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(40) && std::fs::metadata(&out).map_or(0, |m| m.len()) < want {
        std::thread::sleep(Duration::from_millis(20));
    }
    let state = p.state();
    p.stop();
    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(&bytes[..4], b"RIFF", "the play command got a WAV (state {state:?})");
    let (rate, ch) = (u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]), u16::from_le_bytes([bytes[22], bytes[23]]));
    assert_eq!((rate, ch), (22050, 2), "the format the helper told reached the play command");
    let got: Vec<i16> = bytes[44..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
    let n = got.len().min(reference.len()).min(20_000);
    assert!(n >= 10_000, "{n} samples arrived");
    // the player scales by the person's volume: find the gain that fits best, then the rest must be rounding
    let (mut ab, mut bb) = (0f64, 0f64);
    for i in 0..n {
        ab += got[i] as f64 * reference[i] as f64;
        bb += reference[i] as f64 * reference[i] as f64;
    }
    let g = ab / bb;
    let resid: f64 = (0..n).map(|i| (got[i] as f64 - g * reference[i] as f64).powi(2)).sum::<f64>() / n as f64;
    let sig: f64 = (0..n).map(|i| (g * reference[i] as f64).powi(2)).sum::<f64>() / n as f64;
    eprintln!("ogg through the helper and the radio: gain {g:.3}, error {:.2} dB below the sound", 10.0 * (sig / resid.max(1e-9)).log10());
    assert!(g > 0.2 && g < 1.5, "{g}");
    assert!(sig / resid.max(1e-9) > 1e4, "the sound that played is the reference sound (signal to error {})", sig / resid.max(1e-9));
}

#[test]
fn an_opus_stream_from_a_real_http_server_goes_through_the_real_helper_and_plays_the_reference_sound() {
    let Some(helper) = helper_program() else {
        eprintln!("skipped: the helper program is not built (cargo test -p iyakki-audiodec builds it)");
        return;
    };
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../iyakki-audiodec/tests/fixtures/opus");
    // a hybrid file (speech layer and CELT together) and a CELT stereo file: the radio finds Opus by the first packet whatever the server says
    for (name, rate_ch) in [("hybrid-mono-fb-20ms", (48000u32, 1u16)), ("celt-stereo-20ms", (48000, 2))] {
        // a loaded host can starve the player or the mixer for a moment and drop a block of the stream; a real defect fails every attempt
        crate::mixer::retried_under_load(3, || {
            let opus = std::fs::read(fixtures.join(format!("{name}.opus"))).unwrap();
            let reference: Vec<i16> = std::fs::read(fixtures.join(format!("{name}.ref.s16"))).unwrap().chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
            let Some(srv) = serve("opus", &[("live.ogg", opus)]) else { return };
            let out = srv.dir.join("pcm.wav");
            let _route = crate::mixer::pin_command_route();
            let p = RadioPlayer::default();
            p.set_streamer(Arc::new(iyakki_proxy::https::UreqStreamer::default()));
            let h = helper.clone();
            p.set_aac_factory(Arc::new(move || crate::aacpipe::PipeDecoder::spawn(&h, crate::aacpipe::Kind::Helper).map(|d| Box::new(d) as Box<dyn crate::audiofmt::AacDecoder>)));
            let allow: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(|_| true);
            p.play(&Station::new("Loopback Opus", &format!("http://127.0.0.1:{}/live.ogg", srv.port)), allow, &format!("tee {}", out.display()));
            let want = 44 + 2 * reference.len() as u64 * 8 / 10;
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_secs(40) && std::fs::metadata(&out).map_or(0, |m| m.len()) < want {
                std::thread::sleep(Duration::from_millis(20));
            }
            let state = p.state();
            p.stop();
            let bytes = std::fs::read(&out).unwrap();
            assert_eq!(&bytes[..4], b"RIFF", "{name}: the play command got a WAV (state {state:?})");
            let (rate, ch) = (u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]), u16::from_le_bytes([bytes[22], bytes[23]]));
            assert_eq!((rate, ch), rate_ch, "{name}: the format the helper told reached the play command");
            let got: Vec<i16> = bytes[44..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
            let n = got.len().min(reference.len());
            assert!(n >= reference.len() / 2, "{name}: {n} samples arrived");
            let (mut ab, mut bb) = (0f64, 0f64);
            for i in 0..n {
                ab += got[i] as f64 * reference[i] as f64;
                bb += reference[i] as f64 * reference[i] as f64;
            }
            let g = ab / bb;
            let resid: f64 = (0..n).map(|i| (got[i] as f64 - g * reference[i] as f64).powi(2)).sum::<f64>() / n as f64;
            let sig: f64 = (0..n).map(|i| (g * reference[i] as f64).powi(2)).sum::<f64>() / n as f64;
            eprintln!("{name} through the helper and the radio: gain {g:.3}, error {:.2} dB below the sound", 10.0 * (sig / resid.max(1e-9)).log10());
            assert!(g > 0.2 && g < 1.5, "{name}: {g}");
            assert!(sig / resid.max(1e-9) > 1e4, "{name}: the sound that played is the reference sound (signal to error {})", sig / resid.max(1e-9));
        });
    }
}
