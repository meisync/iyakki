// Tests of the glue and the real WebAssembly module in node (run: node --test crates/iyakki-client-engine/js/test/glue.test.mjs).
// Needs the committed module (dist/client-engine/build.sh). Shares golden vectors with the Rust tests: signatures (tests/golden/sig.json) and a
// frame from the instance's encoder (tests/golden/frame.bin).
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createHash, webcrypto } from "node:crypto";
import { IyakkiEngine, hex, unhex, fitSize, defaultFitMode } from "../engine.js";

const dir = new URL("../../", import.meta.url);
const read = (p) => readFileSync(new URL(p, dir));
const wasm = read("assets/iyakki-client-engine.wasm");
const sig = JSON.parse(read("tests/golden/sig.json")).vectors;
const frameMeta = JSON.parse(read("tests/golden/frame.json"));
const frame = new Uint8Array(read("tests/golden/frame.bin"));

const HELLO_OK = (challenge) => JSON.stringify({ t: "hello_ok", v: 1, caps: ["picture", "input", "events", "calls"], challenge, tile: 64, max_message: 65536 });
const sends = (recs) => recs.filter((r) => r.k === "send").map((r) => JSON.parse(r.text));

async function engineFor(v, extra = {}) {
  const e = await IyakkiEngine.load(wasm);
  assert.ok(e.init({ token: null, public: v.public, name: "n", device: v.device, client: "test", ...extra }));
  return e;
}

test("the module has no imports and starts", async () => {
  assert.deepEqual(WebAssembly.Module.imports(new WebAssembly.Module(wasm)), []);
  const e = await IyakkiEngine.load(wasm);
  assert.equal(e.cmd({ c: "version" }).engine, "iyakki-client-engine");
  assert.equal(e.cmd({ c: "connected" }), false, "nothing before init");
});

test("the wasm module signs exactly the golden vectors (seed path)", async () => {
  for (const v of sig) {
    const e = await engineFor(v);
    assert.equal(e.publicFromSeed(v.seed), v.public);
    e.cmd({ c: "connected" });
    e.drain();
    e.onText(HELLO_OK(v.challenge));
    const recs = e.drain();
    assert.ok(recs.some((r) => r.k === "sign" && r.device === v.device && r.challenge === v.challenge));
    assert.equal(e.cmd({ c: "sign_seed", seed: v.seed, ts: v.ts, nonce: v.nonce }), true);
    const auth = sends(e.drain()).find((m) => m.t === "auth");
    assert.equal(auth.authorization, v.authorization);
  }
});

test("the WebCrypto path: the engine's text, signed by the browser's Ed25519, gives the golden header", async () => {
  const subtle = webcrypto.subtle;
  for (const v of sig) {
    const e = await engineFor(v);
    e.cmd({ c: "connected" });
    e.drain();
    e.onText(HELLO_OK(v.challenge));
    e.drain();
    const t = e.cmd({ c: "begin_sign", ts: v.ts, nonce: v.nonce });
    assert.equal(t.text, v.text);
    // a PKCS#8 wrapper around the seed, as WebCrypto imports it
    const pkcs8 = Uint8Array.from([0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20, ...unhex(v.seed)]);
    const key = await subtle.importKey("pkcs8", pkcs8, { name: "Ed25519" }, false, ["sign"]);
    const s = hex(new Uint8Array(await subtle.sign({ name: "Ed25519" }, key, new TextEncoder().encode(t.text))));
    assert.equal(e.cmd({ c: "finish_sign", sig: s }), true);
    assert.equal(sends(e.drain()).find((m) => m.t === "auth").authorization, v.authorization);
  }
});

test("the wasm module decodes the golden frame to the same pixels as the instance's decoder", async () => {
  const v = sig[0];
  const e = await engineFor(v);
  e.cmd({ c: "connected" }); e.drain();
  e.onText(HELLO_OK("c")); e.drain();
  e.cmd({ c: "sign_seed", seed: v.seed, ts: v.ts, nonce: v.nonce }); e.drain();
  e.onText(JSON.stringify({ t: "authed", device: v.device, name: "n" })); e.drain();
  assert.equal(e.cmd({ c: "sign_in", user: "ann", password: "right" }), true);
  const m = sends(e.drain()).find((x) => x.t === "sign_in");
  assert.equal(m.user, "ann");
  e.onText(JSON.stringify({ t: "signed_in", person: "ann" }));
  const opened = sends(e.drain());
  assert.ok(opened.some((x) => x.t === "picture_open" && x.deflate === true));
  e.onText(JSON.stringify({ t: "picture_ok" })); e.drain();
  const bin = new Uint8Array(frame.length + 1);
  bin[0] = 1; bin.set(frame, 1);
  e.onBinary(bin);
  const recs = e.drain();
  assert.ok(sends(recs).some((x) => x.t === "ack"), "acknowledged");
  const f = e.framebuffer();
  assert.equal(f.w, frameMeta.w); assert.equal(f.h, frameMeta.h);
  assert.equal(createHash("sha256").update(f.data).digest("hex"), frameMeta.rgba_sha256);
  // input maps onto the machine's screen and arrives as the page's JSON
  const p = e.cmd({ c: "map", x: f.w / 2, y: f.h / 2, w: f.w, h: f.h });
  assert.deepEqual(p, { x: Math.round(f.w / 2), y: Math.round(f.h / 2) });
  e.cmd({ c: "pointer", kind: "down", x: p.x, y: p.y });
  e.cmd({ c: "key", key: "a", code: "KeyA", ctrl: false, alt: false, shift: false });
  const inputs = sends(e.drain()).filter((x) => x.t === "input").map((x) => JSON.parse(x.msg));
  assert.deepEqual(inputs.map((i) => i.t), ["down", "key"]);
  // the password never comes back out
  assert.ok(!JSON.stringify(recs).includes("right"));
});

test("garbage in is refused, not fatal", async () => {
  const e = await engineFor(sig[0]);
  for (const bad of [{ c: "nope" }, { c: "pointer" }, { c: "sign_seed", seed: "00", ts: 1, nonce: "x" }, { c: "finish_sign", sig: "00" }]) assert.equal(e.cmd(bad), false);
  e.onText("not json"); e.onBinary(new Uint8Array([1, 2, 3])); e.onBinary(new Uint8Array([]));
  assert.ok(e.drain().every((r) => r.k === "notice"));
});

test("the wasm module draws a display list (F-SDK-22): picture, accessibility tree, hit, input lines and refusals", async () => {
  const e = await IyakkiEngine.load(wasm);
  const list = "iyakki-display-list 1 40 20\nrect 0 0 40 20 102030ff\nrect 4 4 10 10 ff0000ff\ntext 2 2 10 ffffffff bold hi%20there\nhit 4 4 10 10 7 button Go\n";
  const r = e.uiLoad(list, 2);
  assert.equal(r.w, 80);
  assert.equal(r.h, 40);
  assert.deepEqual(r.a11y, [{ name: "Go", role: "button", action: 7, x: 8, y: 8, w: 20, h: 20 }]);
  assert.equal(r.texts[0].text, "hi there");
  assert.equal(r.texts[0].size, 20);
  const fb = e.uiFramebuffer();
  const at = (x, y) => Array.from(fb.data.subarray((y * fb.w + x) * 4, (y * fb.w + x) * 4 + 4));
  assert.deepEqual(at(1, 1), [16, 32, 48, 255]);
  assert.deepEqual(at(10, 10), [255, 0, 0, 255]);
  assert.deepEqual(e.uiHit(12, 12), { action: 7, line: "action 7" });
  assert.deepEqual(e.uiHit(60, 30), { action: null });
  assert.equal(e.uiLine({ kind: "pointer", phase: "down", x: 12, y: 12, t: 3 }), "ptr 0 down 6 6 3");
  assert.equal(e.uiLine({ kind: "key", name: "enter" }), "key enter");
  assert.equal(e.uiLine({ kind: "key", name: "f13" }), null);
  const bad = e.uiLoad("iyakki-display-list 4 1 1\n", 1);
  assert.match(bad.error, /newer/);
  assert.equal(e.uiFramebuffer().version, fb.version, "a refused list keeps the picture");
  assert.match(e.uiLoad("iyakki-display-list 1 10 10\nsparkle 1\n", 1).error, /line 2.*sparkle/);
});

test("a phone held upright can fill the window's height and pan sideways; a landscape window shows the whole picture", () => {
  assert.deepEqual(fitSize(390, 844, 1280, 800, "whole"), [390, 243]);
  assert.deepEqual(fitSize(390, 844, 1280, 800, "fill"), [1350, 844]);
  assert.deepEqual(fitSize(1000, 500, 1280, 800, "fill"), fitSize(1000, 500, 1280, 800, "whole"), "never smaller than the whole picture");
  assert.deepEqual(fitSize(0, 10, 10, 10, "whole"), [0, 0]);
  assert.equal(defaultFitMode(390, 844, 1280, 800), "fill");
  assert.equal(defaultFitMode(844, 390, 1280, 800), "whole");
  assert.equal(defaultFitMode(1000, 1100, 1280, 800), "fill");
  assert.equal(defaultFitMode(1000, 1150, 1280, 1000), "whole");
});
