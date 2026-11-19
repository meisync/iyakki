//! A manual check against a program that is not ours: `ffmpeg` (when it is installed on the developer's machine) serves a test
//! picture as Motion-JPEG over HTTP, and the client must read it. Run with `cargo test -p iyakki-rtsp --test ffmpeg_manual -- --ignored`.
//! ffmpeg is never shipped; this only uses the copy the developer has.

use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::Duration;

use iyakki_rtsp::httpmjpeg::HttpMjpeg;
use iyakki_rtsp::url;

#[test]
#[ignore = "needs ffmpeg on the machine"]
fn ffmpegs_motion_jpeg_over_http_is_read() {
    let port = 18764;
    let mut child = match Command::new("ffmpeg")
        .args(["-loglevel", "error", "-re", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=10", "-f", "mpjpeg", "-content_type", "multipart/x-mixed-replace;boundary=ffmpeg", "-listen", "1", &format!("http://127.0.0.1:{port}/video.mjpg")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ffmpeg is not installed: {e}");
            return;
        }
    };
    std::thread::sleep(Duration::from_millis(1200));
    let u = url::parse(&format!("http://127.0.0.1:{port}/video.mjpg")).unwrap();
    let r = HttpMjpeg::open(
        Box::new(move || {
            let s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| iyakki_rtsp::Error::Io(e.to_string()))?;
            s.set_read_timeout(Some(Duration::from_secs(3))).ok();
            Ok(s)
        }),
        &u,
        None,
    );
    let result = (|| {
        let mut h = r.map_err(|e| format!("open: {e}"))?;
        for i in 0..5 {
            let j = h.next_jpeg().map_err(|e| format!("picture {i}: {e}"))?;
            if j[..2] != [0xFF, 0xD8] || j[j.len() - 2..] != [0xFF, 0xD9] {
                return Err(format!("picture {i} is not a whole JPEG"));
            }
        }
        Ok(())
    })();
    let _ = child.kill();
    let _ = child.wait();
    result.unwrap();
}
