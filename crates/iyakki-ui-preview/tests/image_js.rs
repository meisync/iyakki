// SPDX-License-Identifier: Apache-2.0
// Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

//! The `image` operation of the display list in the browser renderer (`displaylist.js`) under node (F-SDK-164): the list text that the Rust
//! canvas writes is parsed by the JavaScript reader, the bad ones are refused by line, and drawing puts the picture into the rectangle
//! (RGBA through ImageData, JPEG through the browser's own decoder, stood in for here). The checks run in the script (node's `assert`).
//! Skipped, saying so, when `node` is not installed. A real browser run is `dist/browsertest/uisdksuite.py`.

use iyakki_ui_sdk::canvas::{Canvas, Image, ImageFormat};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::list::ListCanvas;
use std::sync::Arc;

const SCRIPT: &str = r#"
const assert = require("assert");
const fs = require("fs");
const dl = require(process.argv[2]);
const list = dl.parse(fs.readFileSync(process.argv[3], "utf8"));
assert.deepStrictEqual(list.ops.map((o) => o.op), ["image", "image"]);
assert.strictEqual(list.ops[0].format, "rgba"); assert.strictEqual(list.ops[0].pw, 2); assert.strictEqual(list.ops[1].seq, 3);
const calls = []; let redraws = 0;
globalThis.document = { createElement: () => ({ width: 0, height: 0, getContext: () => ({ putImageData: (d) => calls.push(["put", d.width, d.height, d.data[0], d.data[3]]) }) }) };
globalThis.ImageData = class { constructor(data, w, h) { this.data = data; this.width = w; this.height = h; } };
globalThis.Blob = class { constructor(parts, o) { this.parts = parts; this.type = o.type; } };
globalThis.createImageBitmap = (b) => { calls.push(["decode", b.type, b.parts[0].length]); return Promise.resolve({ bitmap: true }); };
const ctx = new Proxy({ drawImage: (...a) => calls.push(["draw", typeof a[0], a[1], a[2], a[3], a[4]]) }, { get: (t, k) => (k in t ? t[k] : () => ({ addColorStop() {} })), set: () => true });
const canvas = { width: 0, height: 0, style: {}, getContext: () => ctx };
list.onImage = () => { redraws++; };
dl.draw(canvas, list, 1);
const first = calls.slice();
assert.deepStrictEqual(first[0], ["put", 2, 1, 5, 255]);
assert.deepStrictEqual(first[1], ["draw", "object", 1, 2, 30, 20]);
assert.deepStrictEqual(first[2], ["decode", "image/jpeg", 6]);
setTimeout(() => {
  assert.strictEqual(redraws, 1, "the page is told to redraw when the JPEG has decoded");
  dl.draw(canvas, list, 1);
  const after = calls.slice(first.length);
  assert.ok(after.length === 2 && after.every((c) => c[0] === "draw"), JSON.stringify(after));
  const wants = ["unknown image format", "bad image data", "bad image data", "unknown or malformed"];
  process.argv.slice(4).forEach((l, i) => {
    assert.throws(() => dl.parse("iyakki-display-list 1 100 100\n" + l + "\n"), (e) => e.message.includes("line 2") && e.message.includes(wants[i]), "bad line " + i);
  });
  console.log("ok");
}, 20);
"#;

#[test]
fn the_browser_reader_parses_what_the_rust_canvas_writes_refuses_the_bad_and_draws_the_pictures() {
    if !std::process::Command::new("node").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("skipped: node is not installed");
        return;
    }
    let mut lc = ListCanvas::new(100.0, 100.0);
    lc.image(Rect::new(1.0, 2.0, 30.0, 20.0), &Image { format: ImageFormat::Rgba, width: 2, height: 1, seq: 9, data: Arc::new(vec![5, 6, 7, 255, 1, 1, 1, 255]) });
    lc.image(Rect::new(40.0, 2.0, 30.0, 20.0), &Image { format: ImageFormat::Jpeg, width: 4, height: 4, seq: 3, data: Arc::new(vec![0xFF, 0xD8, 1, 2, 0xFF, 0xD9]) });
    let text = lc.finish().to_text();
    let dir = std::env::temp_dir().join(format!("iyakki-dl-image-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (js, list, script) = (dir.join("displaylist.js"), dir.join("list.txt"), dir.join("run.js"));
    std::fs::write(&js, iyakki_ui_preview::RENDERER).unwrap();
    std::fs::write(&list, &text).unwrap();
    std::fs::write(&script, SCRIPT).unwrap();
    let good = text.lines().nth(1).unwrap().to_string();
    let bad = [good.replace(" rgba ", " png "), good.replace(" 2 1 9 ", " 0 1 9 "), good.rsplit_once(' ').unwrap().0.to_string() + " not*base64!", "image 1 2".to_string()];
    let o = std::process::Command::new("node").arg(&script).arg(&js).arg(&list).args(&bad).output().unwrap();
    assert!(o.status.success() && String::from_utf8_lossy(&o.stdout).contains("ok"), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    let _ = std::fs::remove_dir_all(&dir);
}
