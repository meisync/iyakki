// SPDX-License-Identifier: Apache-2.0
// Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

//! The web UI's tile renderer and tile page (`src/webservice/ui/{tilerender,tiles}.js`) under node.
//!
//! GOLDEN: the Rust reference renderer (`iyakki_tile_host::render`) draws a set of display lists (the example counter at several
//! states and a synthetic list with every operation, clips, alpha, rounded corners, lines, every printable ASCII character, a few
//! non-ASCII ones); the same lists, exported with `export::to_json` (what the service sends), are drawn by `tilerender.js`; the two
//! RGBA buffers must be equal pixel for pixel (tolerance 0: both are integer algorithms). Also: the nearest-neighbour enlargement,
//! the canvas drawing with a stand-in canvas, hit lookup, and the page's controller (queue, requests, reconnect, stop and Reload,
//! keyboard, pointer, text input with composition, wheel) with stand-ins for the API and the page.
//! Skipped (passes, saying so) when `node` is not installed. This is NOT a real browser: see `real_browser_screenshot_if_available`.

use iyakki_tile_abi::{Color, DisplayList, Event, Op, Rect};
use iyakki_tile_host::{export, render};
use std::path::{Path, PathBuf};

const BG: Color = Color(200, 210, 220, 255);

fn text(x: i32, y: i32, size: u32, face: u8, c: Color, t: &str) -> Op {
    Op::Text { x, y, face, size, color: c, text: t.into() }
}

fn counter_lists() -> Vec<(String, DisplayList, i32, i32)> {
    let mut out = vec![];
    let mut c = iyakki_tile_example::Counter::new();
    out.push(("counter-0".to_string(), c.frame(), 320, 200));
    for _ in 0..3 {
        c.event(&Event::Hit { id: 1 });
    }
    c.event(&Event::TextInput { text: "Hello, tile! 42?".into() });
    out.push(("counter-typed".to_string(), c.frame(), 320, 200));
    c.event(&Event::Resize { w: 480, h: 260, scale_pct: 100 });
    out.push(("counter-resized".to_string(), c.frame(), 480, 260));
    out
}

fn synthetic() -> (DisplayList, i32, i32) {
    let mut l = DisplayList::new();
    l.push(Op::Rect { r: Rect::new(0, 0, 220, 170), color: Color(250, 250, 250, 255) });
    l.push(Op::Rect { r: Rect::new(10, 10, 60, 40), color: Color(200, 30, 30, 255) });
    l.push(Op::Rect { r: Rect::new(40, 25, 60, 40), color: Color(30, 30, 200, 128) }); // alpha over two colours
    l.push(Op::Rect { r: Rect::new(-10, 150, 40, 40), color: Color(0, 120, 0, 255) }); // partly off the surface
    for (i, rad) in [0, 1, 3, 8, 100].into_iter().enumerate() {
        l.push(Op::RoundRect { r: Rect::new(110 + (i as i32 % 3) * 34, 10 + (i as i32 / 3) * 34, 30, 30), radius: rad, color: Color(20 + 40 * i as u8, 90, 160, 255) });
    }
    for (i, t) in [0, 1, 3, 100].into_iter().enumerate() {
        l.push(Op::Outline { r: Rect::new(10 + i as i32 * 40, 70, 36, 24), thickness: t, color: Color(10, 10, 10, 255) });
    }
    l.push(Op::Line { x1: 5, y1: 100, x2: 100, y2: 140, thickness: 1, color: Color(255, 0, 255, 255) });
    l.push(Op::Line { x1: 100, y1: 100, x2: 5, y2: 108, thickness: 3, color: Color(0, 200, 200, 200) });
    l.push(Op::Line { x1: 60, y1: 100, x2: 60, y2: 100, thickness: 4, color: Color(0, 0, 0, 255) }); // a point
    l.push(Op::Line { x1: 150, y1: 140, x2: 110, y2: 100, thickness: 2, color: Color(120, 60, 0, 255) });
    l.push(Op::PushClip { r: Rect::new(110, 100, 60, 30) });
    l.push(Op::Rect { r: Rect::new(100, 90, 200, 20), color: Color(255, 200, 0, 255) });
    l.push(Op::PushClip { r: Rect::new(130, 105, 100, 100) }); // nested: the intersection counts
    l.push(text(112, 106, 14, 0, Color(0, 0, 0, 255), "CLIPPED TEXT RUNS OFF"));
    l.push(Op::PopClip);
    l.push(Op::RoundRect { r: Rect::new(150, 110, 40, 40), radius: 12, color: Color(0, 0, 0, 255) });
    l.push(Op::PopClip);
    l.push(Op::PopClip); // an extra pop does nothing
    l.push(Op::Image { handle: 7, r: Rect::new(180, 130, 30, 30) });
    l.push(Op::Hit { id: 3, r: Rect::new(0, 0, 50, 50) });
    l.push(text(4, 150, 7, 0, Color(0, 0, 0, 255), "SizeSeven"));
    l.push(text(4, 112, 10, 1, Color(40, 40, 40, 255), "Mono 10 face 1"));
    l.push(text(4, 125, 16, 2, Color(160, 0, 0, 255), "Bold 16"));
    l.push(text(4, 158, 33, 0, Color(0, 0, 0, 90), "Big"));
    (l, 220, 170)
}

fn glyph_sheet(size: u32) -> (DisplayList, i32, i32) {
    let all: String = (0x20u8..0x7f).map(|b| b as char).collect();
    let mut l = DisplayList::new();
    l.push(Op::Rect { r: Rect::new(0, 0, 700, 60), color: Color(255, 255, 255, 255) });
    let half = all.chars().count() / 2;
    l.push(text(2, 2, size, 0, Color(0, 0, 0, 255), &all.chars().take(half).collect::<String>()));
    l.push(text(2, 2 + size as i32 + 4, size, 0, Color(0, 0, 0, 255), &all.chars().skip(half).collect::<String>()));
    l.push(text(2, 2 + 2 * (size as i32 + 4), size, 0, Color(0, 0, 90, 255), "h\u{e9}llo \u{2603} \u{65e5}\u{672c} \u{1F600} end"));
    (l, 700, 60)
}

fn all_cases() -> Vec<(String, DisplayList, i32, i32)> {
    let mut v = counter_lists();
    let (l, w, h) = synthetic();
    v.push(("synthetic".into(), l, w, h));
    for s in [7u32, 14] {
        let (l, w, h) = glyph_sheet(s);
        v.push((format!("glyphs-{s}"), l, w, h));
    }
    v.push(("empty".into(), DisplayList::new(), 40, 30));
    v
}

fn write_cases(dir: &Path) {
    let mut names = vec![];
    for (name, list, w, h) in all_cases() {
        std::fs::write(dir.join(format!("{name}.json")), export::to_json(&list, w, h)).unwrap();
        std::fs::write(dir.join(format!("{name}.rgba")), render::render(&list, w, h, BG).rgba).unwrap();
        names.push(name);
    }
    std::fs::write(dir.join("cases.txt"), names.join("\n")).unwrap();
}

fn node_available() -> bool {
    std::process::Command::new("node").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn work_dir(tag: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("iyakki-webui-tiles-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_node(dir: &Path, driver: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/webservice/ui");
    for f in ["tilerender.js", "tiles.js"] {
        std::fs::copy(src.join(f), dir.join(f)).unwrap();
    }
    std::fs::write(dir.join("driver.js"), driver).unwrap();
    let out = std::process::Command::new("node").arg(dir.join("driver.js")).arg(dir).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success() && !text.contains("BAD"), "{text}");
    assert!(text.contains("ALL OK"), "{text}");
    text
}

const GOLDEN: &str = r##"
const fs = require("fs"), path = require("path");
const dir = process.argv[2];
const R = require(path.join(dir, "tilerender.js"));
let bad = 0;
const ok = (c, m) => { console.log((c ? "OK  " : "BAD ") + m); if (!c) bad++; };
const bg = [200, 210, 220, 255];
const names = fs.readFileSync(path.join(dir, "cases.txt"), "utf8").split("\n");
let total = 0, maxDiff = 0, differing = 0;
for (const n of names) {
  const frame = JSON.parse(fs.readFileSync(path.join(dir, n + ".json"), "utf8"));
  const want = fs.readFileSync(path.join(dir, n + ".rgba"));
  const got = R.render(frame.ops, frame.width, frame.height, bg);
  let d = 0, px = 0;
  ok(got.rgba.length === want.length, n + ": same buffer size (" + want.length + ")");
  for (let i = 0; i < want.length; i += 4) {
    let pd = 0;
    for (let k = 0; k < 4; k++) pd = Math.max(pd, Math.abs(got.rgba[i + k] - want[i + k]));
    if (pd > 0) px++;
    d = Math.max(d, pd);
  }
  total += want.length / 4; maxDiff = Math.max(maxDiff, d); differing += px;
  ok(d === 0, n + ": pixels equal to the Rust reference renderer (" + px + " of " + want.length / 4 + " differ, max channel difference " + d + ")");
  if (n !== "empty") {
    let nonbg = 0;
    for (let i = 0; i < want.length; i += 4) if (want[i] !== 200 || want[i + 1] !== 210 || want[i + 2] !== 220) nonbg++;
    ok(nonbg > 0, n + ": something was drawn (so equal is not empty-equals-empty)");
  }
}
console.log("compared " + total + " pixels in " + names.length + " pictures: " + differing + " differ, max difference " + maxDiff);

// ---- enlargement keeps every pixel
{
  const img = R.render([{ op: "rect", x: 1, y: 1, w: 2, h: 1, color: "#ff000080" }], 4, 3, bg);
  const up = R.upscale(img, 3);
  let same = up.w === 12 && up.h === 9;
  for (let y = 0; y < 9 && same; y++) for (let x = 0; x < 12; x++) for (let k = 0; k < 4; k++) if (up.rgba[(y * 12 + x) * 4 + k] !== img.rgba[((y / 3 | 0) * 4 + (x / 3 | 0)) * 4 + k]) same = false;
  ok(same, "upscale by 3 repeats each pixel");
  ok(R.upscale(img, 1) === img, "upscale by 1 is the same picture");
}
// ---- hostile / odd input never throws
{
  let threw = false;
  try {
    R.render([null, 5, "x", { op: "rect" }, { op: "rect", x: 0, y: 0, w: 5, h: 5, color: "red" }, { op: "text", x: 0, y: 0, size: 0, color: "#000000", text: "a" }, { op: "text", x: 0, y: 0, size: 9999, color: "#000000", text: "a" }, { op: "text", x: 0, y: 0, size: 8, color: "#000000", text: 7 }, { op: "nope" }, { op: "pop_clip" }, { op: "round_rect", x: 0, y: 0, w: -4, h: 3, radius: 5, color: "#000000" }, { op: "outline", x: 0, y: 0, w: 3, h: 3, thickness: -2, color: "#000000" }], 10, 10, bg);
    R.render("nonsense", 10, 10, bg); R.render([], -5, 3.7, bg);
  } catch (e) { threw = true; console.log(e.stack); }
  ok(!threw, "bad ops, bad numbers and a list that is not a list are skipped, not thrown on");
  ok(R.parseColor("#0a0b0c")[3] === 255 && R.parseColor("#0a0b0c80")[3] === 128 && R.parseColor("0a0b0c") === null && R.parseColor("#12345") === null && R.parseColor(5) === null, "colours");
}
// ---- hit lookup: topmost in draw order
{
  const ops = [{ op: "hit", id: 1, x: 0, y: 0, w: 100, h: 100 }, { op: "hit", id: 2, x: 10, y: 10, w: 20, h: 20 }, { op: "rect", x: 0, y: 0, w: 5, h: 5, color: "#000000" }];
  ok(R.hitAt(ops, 15, 15) === 2 && R.hitAt(ops, 50, 50) === 1 && R.hitAt(ops, 150, 150) === null && R.hitAt(null, 1, 1) === null, "hitAt finds the topmost region");
}
// ---- drawing on a canvas stand-in
{
  global.ImageData = class { constructor(d, w, h) { this.data = d; this.width = w; this.height = h; } };
  const puts = [];
  const canvas = { width: 0, height: 0, style: {}, attrs: {}, setAttribute(k, v) { this.attrs[k] = v; }, getContext: () => ({ putImageData: (id, x, y) => puts.push([id, x, y]) }) };
  const frame = JSON.parse(fs.readFileSync(path.join(dir, "counter-0.json"), "utf8"));
  const sz = R.draw(canvas, frame, 2, bg);
  ok(sz.w === 320 && sz.h === 200 && canvas.width === 640 && canvas.height === 400 && canvas.style.width === "320px" && canvas.style.height === "200px", "draw: device ratio 2 makes 640x400 pixels shown at 320x200");
  ok(puts.length === 1 && puts[0][0].width === 640 && puts[0][0].data.length === 640 * 400 * 4, "draw: one picture put on the canvas");
  R.draw(canvas, frame, 1.4, bg); ok(canvas.width === 320, "draw: a fractional ratio rounds to a whole one");
  R.draw(canvas, { width: 999999, height: 0, ops: [] }, 1, bg); ok(canvas.width === 4096 && canvas.height === 1, "draw: sizes are clamped");
}
console.log(bad ? "FAILED" : "ALL OK");
process.exit(bad ? 1 : 0);
"##;

#[test]
fn the_js_renderer_draws_the_same_pixels_as_the_rust_reference_renderer() {
    if !node_available() {
        eprintln!("SKIP: node is not installed (the JS renderer was not compared with the Rust one)");
        return;
    }
    let dir = work_dir("golden");
    write_cases(&dir);
    let out = run_node(&dir, GOLDEN);
    println!("{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The font table of the JS renderer is the Rust one: every printable ASCII character drawn at size 7 (the 5x7 cell, 6 pixels
/// advance) is in the `glyphs-7` case above; this test makes sure that case has all 95 characters.
#[test]
fn the_glyph_sheet_covers_every_printable_ascii_character() {
    let (l, _, _) = glyph_sheet(7);
    let mut chars = String::new();
    for op in &l.ops {
        if let Op::Text { text, .. } = op {
            chars.push_str(text);
        }
    }
    for b in 0x20u8..0x7f {
        assert!(chars.contains(b as char), "{}", b as char);
    }
}

const PAGE: &str = r##"
const fs = require("fs"), path = require("path");
const dir = process.argv[2];
const T = require(path.join(dir, "tiles.js"));
let bad = 0;
const ok = (c, m) => { console.log((c ? "OK  " : "BAD ") + m); if (!c) bad++; };
const tick = () => new Promise((r) => setImmediate(r));

(async () => {
  // ---- pure parts
  ok(T.keyCode("Backspace") === 8 && T.keyCode("Enter") === 13 && T.keyCode("Escape") === 27 && T.keyCode("ArrowLeft") === 0x100 && T.keyCode("ArrowDown") === 0x103, "named keys use the ABI codes");
  ok(T.keyCode("a") === 97 && T.keyCode("A") === 65 && T.keyCode("~") === 126 && T.keyCode(" ") === 32 && T.keyCode("é") === null && T.keyCode("Dead") === null && T.keyCode("Shift") === null && T.keyCode(undefined) === null, "printable ASCII by its character; the rest is not a key");
  ok(T.keyCode("toString") === null && T.keyCode("__proto__") === null && T.keyCode("constructor") === null, "a key name cannot reach the object's own members");
  ok(T.keyCode("F1") === 0x111 && T.keyCode("F12") === 0x11c && T.keyCode("F13") === null, "function keys");
  ok(T.modBits({ shiftKey: true, ctrlKey: false, altKey: true, metaKey: false }) === 5 && T.modBits({ metaKey: true }) === 8 && T.modBits({}) === 0, "modifier bits");
  const rect = { left: 100, top: 50, width: 160, height: 100 };
  ok(JSON.stringify(T.toSurface(180, 100, rect, 320, 200)) === '{"x":160,"y":100}', "a point is scaled from page pixels to the tile surface");
  ok(JSON.stringify(T.toSurface(100.2, 50.4, rect, 160, 100)) === '{"x":0,"y":0}' && T.toSurface(1e12, -1e12, rect, 160, 100).x === 32768 && T.toSurface(NaN, 0, rect, 1, 1).x !== undefined, "rounded, clamped to the ABI range");
  ok(T.toSurface(5, 5, { left: 0, top: 0, width: 0, height: 0 }, 10, 10).x === 5, "a zero-sized canvas does not divide by zero");
  let w = T.wheelKeys(0, 120);
  ok(w.events.length === 2 && w.events[0].code === 0x103 && w.events[0].down && !w.events[1].down && w.rest === 20, "a wheel step down is the Down arrow");
  w = T.wheelKeys(w.rest, 60);
  ok(w.events.length === 0 && w.rest === 80, "small wheel moves add up");
  w = T.wheelKeys(0, -250);
  ok(w.events.length === 4 && w.events[0].code === 0x102 && w.rest === -50, "up and several steps");
  ok(T.wheelKeys(0, 1e9).events.length === 8 && T.wheelKeys(0, NaN).events.length === 0, "a huge wheel delta is bounded");
  ok([0, 1, 2, 3, 4, 9, -1].map(T.retryDelay).join() === "1000,2000,4000,8000,15000,15000,1000", "reconnect pause grows to 15 s");
  const q = T.newQueue();
  q.push({ type: "pointer_down", x: 1, y: 1, button: 0 }); q.push({ type: "pointer_move", x: 2, y: 2 }); q.push({ type: "pointer_move", x: 3, y: 3 }); q.push({ type: "pointer_move", x: 4, y: 4 });
  q.push({ type: "resize", w: 5, h: 5, scale_pct: 100 }); q.push({ type: "pointer_up", x: 4, y: 4, button: 0 }); q.push({ type: "resize", w: 6, h: 6, scale_pct: 100 });
  const took = q.take();
  ok(took.map((e) => e.type).join() === "pointer_down,pointer_move,pointer_up,resize" && took[1].x === 4 && took[3].w === 6 && q.size() === 0, "moves collapse to the newest, one resize is kept, the order of the rest stays");
  for (let i = 0; i < 500; i++) q.push({ type: "key", code: 65, mods: 0, down: true });
  ok(q.size() === 100, "the queue is bounded");

  // ---- the controller with stand-ins
  const posts = [], timers = [], states = [];
  let reply = () => ({ ok: true, status: 200, data: { frame: { width: 320, height: 200, ops: [] } } });
  const api = { post: async (p, b) => { posts.push([p, JSON.parse(JSON.stringify(b))]); return reply(b); } };
  const drawn = [];
  const renderer = { parseColor: (c) => c, draw: (cv, fr, dpr, bg) => { drawn.push([fr, dpr, bg]); return { w: fr.width, h: fr.height }; } };
  const mk = (extra) => T.openTile(Object.assign({ api, canvas: {}, tile: "counter", width: 320, height: 200, renderer, devicePixelRatio: 2, matchMedia: () => ({ matches: true }), setTimeout: (f, ms) => { timers.push([f, ms]); return timers.length; }, clearTimeout: (id) => { timers[id - 1] = null; }, onState: (s, t) => states.push([s, t]) }, extra || {}));
  const fire = async () => { const t = timers.filter(Boolean).pop(); timers.length = 0; if (t) t[0](); await tick(); await tick(); };

  let t = mk();
  t.start();
  ok(states[0][0] === "starting" && timers.length === 1 && timers[0][1] === 0, "start asks for the first picture at once");
  await fire();
  ok(posts.length === 1 && posts[0][0] === "/api/v1/tiles/counter/events", "the request goes to the tile's events route");
  ok(posts[0][1].width === 320 && posts[0][1].height === 200 && posts[0][1].scale_pct === 200 && posts[0][1].dark === true && Array.isArray(posts[0][1].events) && posts[0][1].events.length === 0, "the first request carries the size, the ratio and the theme");
  ok(drawn.length === 1 && drawn[0][1] === 2 && drawn[0][2] === "#181818" && states[states.length - 1][0] === "live", "the picture is drawn at the device ratio on the theme's background");

  // events while a request is out are held, merged and sent after
  let release;
  reply = () => new Promise((r) => { release = () => r({ ok: true, status: 200, data: { frame: { width: 320, height: 200, ops: [] } } }); });
  t.send({ type: "pointer_down", x: 70, y: 88, button: 0 });
  await fire();
  ok(posts.length === 2 && posts[1][1].events.length === 1, "a click is sent");
  t.send({ type: "pointer_move", x: 1, y: 1 }); t.send({ type: "pointer_move", x: 2, y: 2 }); t.send({ type: "pointer_up", x: 2, y: 2, button: 0 });
  await fire();
  ok(posts.length === 2, "only one request is out at a time");
  reply = () => ({ ok: true, status: 200, data: { frame: { width: 320, height: 200, ops: [] } } });
  release(); await tick(); await tick(); await fire();
  ok(posts.length === 3 && posts[2][1].events.map((e) => e.type).join() === "pointer_move,pointer_up" && posts[2][1].events[0].x === 2, "what came meanwhile is sent together, moves collapsed");

  // connection lost: nothing is lost, the pause grows, then it goes on
  reply = () => ({ ok: false, status: 0, data: null, message: "x" });
  t.send({ type: "text", text: "a" }); t.send({ type: "text", text: "b" });
  await fire();
  ok(states[states.length - 1][0] === "reconnecting" && /lost/.test(states[states.length - 1][1]) && timers[0][1] === 1000, "a lost connection says so and retries after one second");
  const afterFail = posts.length;
  await fire();
  ok(posts.length === afterFail + 1 && posts[afterFail][1].events.map((e) => e.text).join("") === "ab", "the same events are sent again, in order");
  ok(timers[0][1] === 2000, "the second failure waits two seconds");
  reply = () => ({ ok: true, status: 200, data: { frame: { width: 320, height: 200, ops: [] } } });
  await fire();
  ok(states[states.length - 1][0] === "live", "back to live when the service answers");
  reply = () => ({ ok: false, status: 429, data: null, message: "x" });
  t.send({ type: "tick", dt_ms: 1 }); await fire();
  ok(states[states.length - 1][0] === "reconnecting" && /Too many/.test(states[states.length - 1][1]) && timers[0][1] === 2000, "too many requests: a short wait, no loop");

  // a refused or crashed tile: plain message, no retry, Reload starts it again with reset
  reply = () => ({ ok: false, status: 502, data: { error: { message: "The app was stopped: it used too much computing time for one step." } }, message: "The app was stopped: it used too much computing time for one step." });
  await fire();
  const stopped = states[states.length - 1];
  ok(stopped[0] === "stopped" && /stopped/.test(stopped[1]) && timers.filter(Boolean).length === 0, "a crashed tile shows the service's sentence and does not loop");
  const n1 = posts.length;
  t.send({ type: "key", code: 65, mods: 0, down: true }); await fire();
  ok(posts.length === n1, "a stopped tile sends nothing");
  reply = () => ({ ok: true, status: 200, data: { frame: { width: 320, height: 200, ops: [] } } });
  t.reload(); await fire();
  ok(posts.length === n1 + 1 && posts[n1][1].reset === true && states[states.length - 1][0] === "live", "Reload asks the service to start again");
  t.send({ type: "key", code: 65, mods: 0, down: true }); await fire();
  ok(posts[posts.length - 1][1].reset === undefined, "reset is asked for once");
  reply = () => ({ ok: false, status: 501, data: null, message: "This app was built for tile interface version 2, but this system speaks version 1. Update the app or the system." });
  t.send({ type: "tick", dt_ms: 1 }); await fire();
  ok(/version 2/.test(states[states.length - 1][1]) && states[states.length - 1][0] === "stopped", "a refused tile (wrong interface version) says why");
  reply = () => ({ ok: false, status: 401, data: null, message: "Sign in again." });
  t.reload(); await fire();
  ok(states[states.length - 1][0] !== "live", "after a sign-out nothing is drawn");

  // resize and the size of later requests
  reply = () => ({ ok: true, status: 200, data: { frame: { width: 320, height: 200, ops: [] } } });
  t = mk(); t.start(); await fire();
  t.resize(500.4, 300.6); await fire();
  ok(posts[posts.length - 1][1].events[0].type === "resize" && posts[posts.length - 1][1].events[0].w === 500 && posts[posts.length - 1][1].events[0].h === 301 && posts[posts.length - 1][1].events[0].scale_pct === 200, "resize is a whole-number event with the ratio");
  ok(mk({ width: 99999, height: -3 }).size().w === 4096 && mk({ width: 99999, height: -3 }).size().h === 1, "the size is clamped");
  t.stop(); t.send({ type: "tick", dt_ms: 1 }); const nStop = posts.length; await fire();
  ok(posts.length === nStop, "after stop nothing more is sent");

  // ---- the page: DOM stand-ins
  const node = (tag) => ({ tag, attrs: {}, kids: [], text: "", className: "", listeners: {}, value: "", hidden: false, style: {}, clientWidth: 600,
    setAttribute(k, v) { this.attrs[k] = v; }, appendChild(c) { this.kids.push(c); return c; },
    addEventListener(t, f) { this.listeners[t] = f; }, getBoundingClientRect() { return { left: 10, top: 20, width: 320, height: 200 }; },
    setPointerCapture() { this.captured = true; }, focus() { this.focused = true; }, getContext() { return { putImageData() {} }; },
    set textContent(v) { this.text = v; if (v === "") this.kids = []; }, get textContent() { return this.text; } });
  const doc = { createElement: node, createTextNode: (x) => ({ text: x, tag: "#text" }), defaultView: null };
  const App = require(path.join(dir, "app.js"));
  const el = App.make(doc);
  const winListeners = {}, removed = [];
  const win = { devicePixelRatio: 1, setTimeout: (f, ms) => { timers.push([f, ms]); return timers.length; }, clearTimeout: () => {}, matchMedia: () => ({ matches: false }),
    addEventListener: (t, f) => { winListeners[t] = f; }, removeEventListener: (t) => removed.push(t) };
  doc.defaultView = win;
  const all = (n) => [n].concat((n.kids || []).flatMap(all));
  const leaves = [], said = [];
  const baseCtx = (apiObj) => ({ api: apiObj, el, say: (m) => said.push(m), poll: () => () => {}, onLeave: (f) => leaves.push(f), me: { user: "ann" }, doc, messageFor: App.messageFor });
  // the renderer the page uses is the real one, with a canvas stand-in
  global.ImageData = class { constructor(d, w, h) { this.data = d; this.width = w; this.height = h; } };
  global.IyakkiTileRender = require(path.join(dir, "tilerender.js"));
  const pagePosts = [];
  const counter = JSON.parse(fs.readFileSync(path.join(dir, "counter-0.json"), "utf8"));
  const pageApi = { get: async () => ({ ok: true, status: 200, data: { tiles: [{ id: "counter", title: "Counter (example tile)", abi: 1, width: 320, height: 200 }] } }),
    post: async (p, b) => { pagePosts.push([p, b]); return { ok: true, status: 200, data: { frame: counter } }; } };
  const page = T.page();
  ok(page.id === "tiles" && page.title === "Tiles" && page.available({ user: "x" }) && !page.available(null), "the page is offered to every signed-in person");
  const host = node("div");
  await page.render(baseCtx(pageApi), host);
  const nodes = all(host);
  const canvas = nodes.find((n) => n.tag === "canvas"), input = nodes.find((n) => n.tag === "textarea");
  ok(canvas && input && nodes.filter((n) => n.tag === "button").some((b) => b.text === "Counter (example tile)"), "a canvas, a hidden text box and a button per tile");
  ok(canvas.attrs["aria-label"] === "Counter (example tile)" && canvas.attrs.tabindex === "0" && input.attrs["aria-label"], "the canvas and the text box have names");
  while (timers.filter(Boolean).length) await fire();
  ok(pagePosts.length >= 1 && pagePosts[0][1].width <= 320 && canvas.width === 320, "the tile started and its picture is on the canvas");

  const last = () => pagePosts[pagePosts.length - 1][1].events;
  const evs = [];
  const flush = async () => { await fire(); };
  canvas.listeners.pointerdown({ clientX: 80, clientY: 108, button: 0, pointerId: 1, preventDefault() {} });
  ok(input.focused && canvas.captured, "a press focuses the text box and captures the pointer");
  canvas.listeners.pointerup({ clientX: 80, clientY: 108, button: 0 });
  await flush();
  ok(JSON.stringify(last()) === JSON.stringify([{ type: "pointer_down", button: 0, x: 70, y: 88 }, { type: "pointer_up", button: 0, x: 70, y: 88 }]), "mouse positions become surface positions");
  canvas.listeners.pointermove({ clientX: 80, clientY: 108 });
  await flush();
  ok(!/pointer_move/.test(JSON.stringify(last())), "moving without a button sends nothing");
  ok(canvas.className.includes("tile-hot"), "over a hit region the cursor is a pointer (class)");
  canvas.listeners.pointerdown({ clientX: 80, clientY: 108, button: 0, pointerId: 1, preventDefault() {} });
  canvas.listeners.pointermove({ clientX: 90, clientY: 118 });
  canvas.listeners.pointermove({ clientX: 91, clientY: 119 });
  canvas.listeners.pointercancel({});
  canvas.listeners.pointerup({ clientX: 91, clientY: 119, button: 0 });
  await flush();
  ok(JSON.stringify(last().map((e) => e.type)) === '["pointer_down","pointer_move"]' && last()[1].x === 81, "drag: only the newest move; a cancelled press sends no release");
  let prevented = 0;
  canvas.listeners.wheel({ deltaY: 100, deltaMode: 0, preventDefault() { prevented++; } });
  canvas.listeners.wheel({ deltaY: 10, deltaMode: 0, preventDefault() { prevented++; } });
  await flush();
  ok(last().length === 2 && last()[0].code === 0x103 && prevented === 1, "the wheel is Down arrow presses and the page does not scroll for them");
  input.listeners.keydown({ key: "Backspace", shiftKey: false, preventDefault() { prevented += 10; } });
  input.listeners.keyup({ key: "Backspace", preventDefault() {} });
  await flush();
  ok(JSON.stringify(last()) === JSON.stringify([{ type: "key", code: 8, mods: 0, down: true }, { type: "key", code: 8, mods: 0, down: false }]) && prevented === 11, "Backspace is key 8, down and up");
  input.listeners.keydown({ key: "Tab", preventDefault() { prevented += 100; } });
  await flush();
  ok(prevented === 11, "Tab still moves focus out of the tile");
  input.value = "x"; input.listeners.input({ isComposing: false });
  await flush();
  ok(JSON.stringify(last()) === JSON.stringify([{ type: "text", text: "x" }]) && input.value === "", "typed text is sent as text and the box is emptied");
  input.listeners.compositionstart({});
  input.value = "に"; input.listeners.input({ isComposing: true });
  input.listeners.keydown({ key: "Process", keyCode: 229, isComposing: true });
  input.listeners.keydown({ key: "a", keyCode: 65, isComposing: true });
  const n2 = pagePosts.length;
  await flush();
  ok(pagePosts.length === n2, "while an input method is composing, nothing is sent");
  input.listeners.compositionend({ data: "日本" });
  await flush();
  ok(pagePosts.length === n2 + 1 && JSON.stringify(last()) === JSON.stringify([{ type: "text", text: "日本" }]), "composition: nothing while composing, the finished text once");
  const lenBefore = pagePosts.length;
  win.devicePixelRatio = 1;
  winListeners.resize && winListeners.resize();
  await flush();
  ok(pagePosts.length >= lenBefore, "a window resize is handled");
  leaves.forEach((f) => f());
  ok(removed.includes("resize"), "leaving the page ends the tile and removes the window listener");
  const before = pagePosts.length;
  canvas.listeners.pointerdown({ clientX: 80, clientY: 108, button: 0, pointerId: 1, preventDefault() {} }); await flush();
  ok(pagePosts.length === before, "a tile that was left sends nothing");

  // an empty list and a failing list
  const h2 = node("div");
  await T.page().render(baseCtx({ get: async () => ({ ok: true, status: 200, data: { tiles: [] } }), post: async () => ({}) }), h2);
  ok(all(h2).some((n) => /No tiles/.test(n.text || "")), "no tiles: a plain message");
  const h3 = node("div");
  await T.page().render(baseCtx({ get: async () => ({ ok: false, status: 500, data: null, message: "The service had a problem (500)." }), post: async () => ({}) }), h3);
  ok(all(h3).some((n) => /problem/.test(n.text || "")), "a failing list shows the service's message");
  const all3 = all(h3).filter((n) => n.tag === "canvas");
  ok(all3.length === 0, "and draws nothing");

  console.log(bad ? "FAILED" : "ALL OK");
  process.exit(bad ? 1 : 0);
})().catch((e) => { console.log("BAD exception " + e.stack); process.exit(1); });
"##;

#[test]
fn the_tiles_page_follows_its_rules_under_node() {
    if !node_available() {
        eprintln!("SKIP: node is not installed (the Tiles page controller was not run)");
        return;
    }
    let dir = work_dir("page");
    write_cases(&dir);
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/webservice/ui");
    std::fs::copy(src.join("app.js"), dir.join("app.js")).unwrap();
    let out = run_node(&dir, PAGE);
    println!("{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A bonus when a real, unwrapped Firefox is on this machine (the snap wrapper of Ubuntu is skipped): headless, draws the same JSON with `tilerender.js` on a real canvas and the
/// screenshot is compared with the Rust reference picture. Skipped (passes, saying so) when there is no Firefox or it cannot run
/// headless here (a sandboxed package may refuse files outside its own folders).
#[test]
fn real_browser_screenshot_if_available() {
    // find a browser by looking in PATH (nothing is started to ask for its version: Ubuntu's `firefox` is a shell wrapper around the snap
    // that edits the desktop's default-browser settings on every start, and the snap cannot read these files anyway)
    let find = |name: &str| std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|f| f.is_file()));
    let found = ["firefox", "chromium", "chromium-browser", "google-chrome"].into_iter().find_map(|b| find(b).map(|p| (b, p)));
    let Some((browser, path)) = found else {
        eprintln!("SKIP: no real browser on this machine");
        return;
    };
    let wrapper = std::fs::read(&path).map(|b| b.starts_with(b"#!")).unwrap_or(true);
    if wrapper {
        eprintln!("SKIP: {browser} here ({}) is a wrapper script, not a browser binary: no real-browser comparison was run", path.display());
        return;
    }
    if browser != "firefox" || !node_available() {
        eprintln!("SKIP: only the Firefox flow is written ({browser}); node is needed to read the PNG");
        return;
    }
    let dir = work_dir("browser");
    write_cases(&dir);
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/webservice/ui");
    std::fs::copy(src.join("tilerender.js"), dir.join("tilerender.js")).unwrap();
    let json = std::fs::read_to_string(dir.join("counter-typed.json")).unwrap();
    // the page draws the frame at scale 1 on the same background and shows nothing else
    let html = format!(
        "<!doctype html><meta charset=utf-8><style>html,body{{margin:0;padding:0;background:#c8d2dc}}canvas{{display:block}}</style><canvas id=c></canvas><script src=\"tilerender.js\"></script><script>var f={json};IyakkiTileRender.draw(document.getElementById('c'),f,1,[200,210,220,255]);</script>"
    );
    std::fs::write(dir.join("page.html"), html).unwrap();
    let shot = dir.join("shot.png");
    let prof = dir.join("profile");
    std::fs::create_dir_all(&prof).unwrap();
    let out = std::process::Command::new("timeout")
        .args(["60", "firefox", "--headless", "--no-remote", "--profile"])
        .arg(&prof)
        .arg("--window-size=320,200")
        .arg("--screenshot")
        .arg(&shot)
        .arg(format!("file://{}", dir.join("page.html").display()))
        .output();
    let ran = out.as_ref().map(|o| o.status.success()).unwrap_or(false) && shot.exists();
    if !ran {
        eprintln!("SKIP: Firefox could not take a headless screenshot here: {:?}", out.map(|o| String::from_utf8_lossy(&o.stderr).chars().take(300).collect::<String>()));
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    std::fs::write(dir.join("cmp.js"), BROWSER_CMP).unwrap();
    let r = std::process::Command::new("node").arg(dir.join("cmp.js")).arg(&shot).arg(dir.join("counter-typed.rgba")).output().unwrap();
    let text = String::from_utf8_lossy(&r.stdout).into_owned() + &String::from_utf8_lossy(&r.stderr);
    println!("{text}");
    assert!(r.status.success() && text.contains("ALL OK"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Reads a PNG (8-bit RGB/RGBA, no interlace) with node's zlib and compares the top-left 320x200 with the Rust RGBA.
const BROWSER_CMP: &str = r##"
const fs = require("fs"), zlib = require("zlib");
const png = fs.readFileSync(process.argv[2]), want = fs.readFileSync(process.argv[3]);
let p = 8, w = 0, h = 0, ct = 0, idat = [];
while (p < png.length) {
  const len = png.readUInt32BE(p), type = png.toString("ascii", p + 4, p + 8), d = png.subarray(p + 8, p + 8 + len);
  if (type === "IHDR") { w = d.readUInt32BE(0); h = d.readUInt32BE(4); ct = d[9]; if (d[8] !== 8 || d[12] !== 0) throw new Error("unsupported png"); }
  if (type === "IDAT") idat.push(d);
  p += 12 + len;
}
const bpp = ct === 6 ? 4 : 3, raw = zlib.inflateSync(Buffer.concat(idat)), stride = w * bpp, out = Buffer.alloc(w * h * bpp);
for (let y = 0; y < h; y++) {
  const f = raw[y * (stride + 1)];
  for (let x = 0; x < stride; x++) {
    const a = x >= bpp ? out[y * stride + x - bpp] : 0, b = y ? out[(y - 1) * stride + x] : 0, c = x >= bpp && y ? out[(y - 1) * stride + x - bpp] : 0;
    let v = raw[y * (stride + 1) + 1 + x];
    if (f === 1) v += a; else if (f === 2) v += b; else if (f === 3) v += (a + b) >> 1;
    else if (f === 4) { const pa = Math.abs(b - c), pb = Math.abs(a - c), pc = Math.abs(a + b - 2 * c); v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c; }
    out[y * stride + x] = v & 255;
  }
}
let diff = 0, n = 0;
for (let y = 0; y < 200; y++) for (let x = 0; x < 320; x++) {
  n++;
  let bad = false;
  for (let k = 0; k < 3; k++) if (Math.abs(out[(y * w + x) * bpp + k] - want[(y * 320 + x) * 4 + k]) > 2) bad = true;
  if (bad) diff++;
}
console.log("browser screenshot " + w + "x" + h + ": " + diff + " of " + n + " pixels differ by more than 2");
console.log(diff === 0 ? "ALL OK" : "BAD screenshot differs");
process.exit(diff === 0 ? 0 : 1);
"##;
