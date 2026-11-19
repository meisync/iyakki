// SPDX-License-Identifier: Apache-2.0
// Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

//! Display list version 3 (`path`, `layer`, `textc`) in the browser renderer (`displaylist.js`) under node (F-SDK-35): the list text the Rust
//! canvas writes is parsed by the JavaScript reader with the same rules (the same clusters, the same refusals by line) and drawing makes
//! the canvas calls a browser would get. Skipped, saying so, when `node` is not installed. A real browser run is `dist/browsertest/uisdksuite.py`.

use iyakki_ui_sdk::canvas::{Canvas, TextStyle};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::list::ListCanvas;
use iyakki_ui_sdk::path::Seg;

const SCRIPT: &str = r#"
const assert = require("assert");
const fs = require("fs");
const dl = require(process.argv[2]);
const list = dl.parse(fs.readFileSync(process.argv[3], "utf8"));
assert.strictEqual(list.version, 3);
assert.deepStrictEqual(list.ops.map((o) => o.op), ["layer", "path", "rect", "endlayer", "textc", "text"]);
assert.strictEqual(list.ops[0].alpha, 0.5);
assert.strictEqual(list.ops[1].segs.length, 5);
assert.deepStrictEqual(list.ops[4].clusters, ["é", "x", "\u{1F1E9}\u{1F1EA}"]);
const calls = [];
let alpha = 1; const saved = [];
const ctx = new Proxy({}, {
  get: (t, k) => {
    if (k === "globalAlpha") return alpha;
    return (...a) => { if (k === "save") saved.push(alpha); if (k === "restore") alpha = saved.pop(); calls.push([k, ...a]); return { addColorStop() {} }; };
  },
  set: (t, k, v) => { if (k === "globalAlpha") alpha = v; else calls.push(["set " + k, v]); return true; },
});
dl.draw({ width: 0, height: 0, style: {}, getContext: () => ctx }, list, 1);
const names = calls.map((c) => c[0]);
assert.ok(names.includes("quadraticCurveTo") && names.includes("closePath") && names.includes("fill") && names.includes("stroke"));
assert.ok(calls.some((c) => c[0] === "set globalAlpha" || c[0] === "save"));
const texts = calls.filter((c) => c[0] === "fillText");
assert.deepStrictEqual(texts.slice(0, 3).map((c) => [c[1], c[2]]), [["é", 20], ["x", 29], ["\u{1F1E9}\u{1F1EA}", 38]], "every cluster at its own pen");
assert.strictEqual(alpha, 1, "the layer is closed again");
const head = "iyakki-display-list 3 100 100\n";
const wants = process.argv.slice(4).map((x) => x.split("|"));
for (const [line, why] of wants) {
  assert.throws(() => dl.parse(head + line.replace(/\\n/g, "\n")), (e) => e.message.includes(why), line);
}
console.log("ok");
"#;

#[test]
fn the_browser_reader_parses_what_the_rust_canvas_writes_for_version_3_refuses_the_bad_and_draws_it() {
    if !std::process::Command::new("node").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("skipped: node is not installed");
        return;
    }
    let mut lc = ListCanvas::new(100.0, 100.0);
    lc.layer(0.5, &mut |c| {
        c.path(Some([1, 2, 3, 255]), Some(([9, 9, 9, 255], 2.0)), &[Seg::Move(0.0, 0.0), Seg::Line(10.0, 0.0), Seg::Quad(10.0, 10.0, 0.0, 10.0), Seg::Cubic(0.0, 5.0, 0.0, 2.0, 0.0, 0.0), Seg::Close]);
        c.rect(Rect::new(1.0, 1.0, 2.0, 2.0), [1, 1, 1, 255]);
    });
    lc.text_clusters(20.0, 50.0, 12.0, [0, 0, 0, 255], "e\u{0301}x\u{1F1E9}\u{1F1EA}", TextStyle::Regular, &[9.0, 9.0, 18.0]);
    lc.text(5.0, 5.0, 12.0, [0, 0, 0, 255], "plain", TextStyle::Regular);
    let text = lc.finish().to_text();
    let dir = std::env::temp_dir().join(format!("iyakki-dl-v3-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (js, list, script) = (dir.join("displaylist.js"), dir.join("list.txt"), dir.join("run.js"));
    std::fs::write(&js, iyakki_ui_preview::RENDERER).unwrap();
    std::fs::write(&list, &text).unwrap();
    std::fs::write(&script, SCRIPT).unwrap();
    let bad = [
        "path - - 0 M0:0|fill or a stroke",
        "path ff0000ff - 0 L1:1|starts with a move",
        "path ff0000ff - 0 M0:0;X|unknown path segment",
        "path ff0000ff - 0 M0:0;Q1:2|needs 4",
        "layer 1\\nclip 0 0 1 1\\nendlayer\\nendclip|endlayer without layer",
        "layer 1|never closed",
        "textc 0 0 10 000000ff regular default 0 0 5,5 abc|one advance for every cluster",
    ];
    let o = std::process::Command::new("node").arg(&script).arg(&js).arg(&list).args(bad).output().unwrap();
    assert!(o.status.success() && String::from_utf8_lossy(&o.stdout).contains("ok"), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    let _ = std::fs::remove_dir_all(&dir);
}
