//! The page's decoder (`src/decoder.js`) against messages the Rust encoder made, run under node with a stand-in canvas.
//! Skipped (passes, saying so) when `node` is not installed or has no `DecompressionStream`. Nothing is installed or
//! downloaded; the files go in a temporary folder that is removed afterwards. It does not replace a real browser.

use iyakki_remote::codec::*;

const DRIVER: &str = r##"
const fs = require("fs"); const path = require("path");
const dir = process.argv[2];
if (typeof DecompressionStream !== "function") { console.log("SKIP no DecompressionStream"); process.exit(0); }
const { Painter } = require(path.join(dir, "decoder.js"));
function fake(w, h) {
  const c = { width: w, height: h, buf: new Uint8ClampedArray(w * h * 4) };
  c.getContext = () => ({
    createImageData: (tw, th) => ({ width: tw, height: th, data: new Uint8ClampedArray(tw * th * 4) }),
    putImageData: (img, x, y) => { for (let r = 0; r < img.height; r++) for (let q = 0; q < img.width; q++) for (let k = 0; k < 4; k++) c.buf[((y + r) * c.width + x + q) * 4 + k] = img.data[(r * img.width + q) * 4 + k]; },
    drawImage() {},
  });
  return c;
}
(async () => {
  const cases = JSON.parse(fs.readFileSync(path.join(dir, "cases.json")));
  let bad = 0;
  for (const cs of cases) {
    const msg = fs.readFileSync(path.join(dir, cs.msg)), want = fs.readFileSync(path.join(dir, cs.want));
    const main = fake(cs.screen_w, cs.screen_h); let back = null;
    const p = new Painter(main, (w, h) => (back = fake(w, h)));
    const r = await p.paint(new Uint8Array(msg));
    const got = cs.scaled ? back.buf : main.buf;
    let worst = 0; for (let i = 0; i < want.length; i++) worst = Math.max(worst, Math.abs(got[i] - want[i]));
    const ok = got.length === want.length && worst <= cs.tol && r.tiles === cs.tiles;
    console.log((ok ? "OK " : "BAD ") + cs.name + " worst=" + worst);
    if (!ok) bad++;
  }
  // hostile input must throw, not hang or crash
  for (const m of [new Uint8Array(5), new Uint8Array(40).fill(2)]) { try { await new Painter(fake(10, 10)).paint(m); console.log("BAD accepted"); bad++; } catch (e) { console.log("OK refused"); } }
  process.exit(bad ? 1 : 0);
})();
"##;

fn photo(w: usize, h: usize) -> Vec<u8> {
    let mut s = 1u32;
    (0..w * h)
        .flat_map(|i| {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = (s >> 28) as i32 - 8;
            let (x, y) = ((i % w) as i32, (i / w) as i32);
            let c = |v: i32| (v + n).clamp(0, 255) as u8;
            [c(x * 255 / w as i32), c(y * 255 / h as i32), c(100 + (x + y) / 4), 255]
        })
        .collect()
}

#[test]
fn the_pages_decoder_draws_what_the_rust_encoder_sent() {
    let ok = std::process::Command::new("node").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP: node is not installed");
        return;
    }
    let dir = std::env::temp_dir().join(format!("iyakki-remote-js-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("decoder.js"), iyakki_remote::page::DECODER_JS).unwrap();
    std::fs::write(dir.join("driver.js"), DRIVER).unwrap();
    let (w, h) = (150usize, 90usize);
    let mut img = photo(w, h);
    // UI-like left part (palette tiles) and a solid area
    for y in 0..h {
        for x in 0..64 {
            let v = if (x + y) % 5 == 0 { [230, 230, 240] } else { [20, 24, 40] };
            img[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&v);
        }
    }
    let mut cases = Vec::new();
    for (i, level) in Level::ALL.into_iter().enumerate() {
        let mut st = Streamer::new();
        st.set_source(w, h, img.clone());
        let e = st.encode(&Opts { level, caps: Caps { deflate: true }, budget: usize::MAX, now_ms: 0 }).unwrap();
        let mut canvas = Canvas::default();
        decode_frame(&e.msg, &mut canvas).unwrap();
        std::fs::write(dir.join(format!("m{i}.bin")), &e.msg).unwrap();
        std::fs::write(dir.join(format!("w{i}.rgba")), &canvas.rgba).unwrap();
        cases.push(serde_json::json!({ "name": level.name(), "msg": format!("m{i}.bin"), "want": format!("w{i}.rgba"), "screen_w": w, "screen_h": h, "scaled": level.scale() > 1, "tol": 1, "tiles": e.tiles }));
    }
    std::fs::write(dir.join("cases.json"), serde_json::Value::Array(cases).to_string()).unwrap();
    let out = std::process::Command::new("node").arg(dir.join("driver.js")).arg(&dir).output().expect("node runs");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("{text}");
    if text.starts_with("SKIP") {
        return;
    }
    assert!(out.status.success() && !text.contains("BAD"), "{text}\n{}", String::from_utf8_lossy(&out.stderr));
}
