// Node tests of the audio worklet's logic and of the media half of the page (fake DOM, fake audio, fake engine). No browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { PlayQueue, MicChunker, resample, RATE, MAX_QUEUE_MS, PREFILL_MS, MIC_CHUNK } from "../audio-worklet.js";
import { createMedia } from "../media.js";

const chunk = (v, frames = 960) => new Float32Array(frames * 2).fill(v);
const quantum = () => [new Float32Array(128), new Float32Array(128)];

test("the player holds at most 100 ms and drops the oldest sound first", () => {
  const q = new PlayQueue();
  for (let i = 0; i < 12; i++) q.push(i, chunk((i + 1) / 100));
  assert.equal(q.queuedMs(), MAX_QUEUE_MS);
  assert.ok(q.dropped > 0);
  const [l, r] = quantum();
  q.pull(l, r);
  // the oldest 7 chunks (values 0.01 to 0.07) are gone, the newest 5 (0.08 to 0.12) remain
  assert.ok(Math.abs(l[0] - 0.08) < 1e-6, String(l[0]));
});

test("playing starts after the prefill and a dry queue gives silence, never old sound", () => {
  const q = new PlayQueue();
  const [l, r] = quantum();
  q.push(0, chunk(0.5, 480)); // 10 ms: less than the prefill
  assert.equal(q.pull(l, r), 0);
  assert.ok(l.every((x) => x === 0));
  q.push(1, chunk(0.5, 480)); q.push(2, chunk(0.5, 480)); q.push(3, chunk(0.5, 480)); // 40 ms in all
  assert.equal(q.pull(l, r), 128);
  assert.equal(l[0], 0.5);
  while (q.len > 0) q.pull(l, r);
  assert.equal(q.pull(l, r), 0, "dry: silence");
  assert.equal(q.underruns, 1);
  assert.ok(l.every((x) => x === 0) && r.every((x) => x === 0));
});

test("a flush empties the queue at once so a pause or a stop does not play on", () => {
  const q = new PlayQueue();
  for (let i = 0; i < 4; i++) q.push(i, chunk(0.3));
  const [l, r] = quantum();
  assert.equal(q.pull(l, r), 128);
  q.flush();
  assert.equal(q.queuedMs(), 0);
  assert.equal(q.pull(l, r), 0);
  assert.ok(l.every((x) => x === 0));
  // sound after the flush starts clean, and the sequence numbers are not compared with the old ones
  q.push(500, chunk(0.2)); q.push(501, chunk(0.2));
  assert.equal(q.lost, 0);
  assert.equal(q.pull(l, r), 128);
});

test("a gap in the sequence numbers is counted and not filled in", () => {
  const q = new PlayQueue();
  q.push(10, chunk(0.1)); q.push(11, chunk(0.1)); q.push(15, chunk(0.1));
  assert.equal(q.lost, 3);
  assert.equal(q.queuedMs(), 60);
  q.push(0xffffffff, chunk(0.1)); q.push(0, chunk(0.1)); // wraps without counting a loss
  assert.equal(q.lost, 3 + ((0xffffffff - 16) >>> 0));
});

test("a context at another rate gets the sound resampled", () => {
  const q = new PlayQueue(44100);
  q.push(0, chunk(0.25)); q.push(1, chunk(0.25));
  const [l] = quantum();
  assert.equal(q.pull(l, new Float32Array(128)), 128);
  assert.ok(Math.abs(l[10] - 0.25) < 1e-6);
  assert.ok(Math.abs(q.queuedMs() - 40 + (128 / 44.1)) < 1.5);
  assert.equal(resample(new Float32Array(960 * 2), 48000, 16000).length, 320 * 2);
});

test("the microphone is cut into 40 ms chunks of 16 kHz 16-bit mono", () => {
  const c = new MicChunker(48000);
  const got = [];
  const block = new Float32Array(128).fill(0.5);
  for (let i = 0; i < 400; i++) c.push(block, (d) => got.push(d));
  const samples = 400 * 128 / 3;
  assert.equal(got.length, Math.floor(samples / MIC_CHUNK));
  assert.ok(got.every((g) => g.length === MIC_CHUNK && g[100] === Math.round(0.5 * 32767)));
  const loud = new Float32Array(128).fill(3); // clipped, not wrapped
  const c2 = new MicChunker(16000); const out = [];
  for (let i = 0; i < 6; i++) c2.push(loud, (d) => out.push(d));
  assert.equal(out[0][0], 32767);
});

// ---------------------------------------------------------------------------------------------------------- the page half with fakes

const setNav = (v) => Object.defineProperty(globalThis, "navigator", { value: v, configurable: true, writable: true });

function fakeDoc() {
  const els = {};
  const mk = (id) => (els[id] ??= { id, hidden: false, textContent: "", value: "", listeners: {}, addEventListener(t, f) { this.listeners[t] = f; }, getContext() { return { drawImage() {} }; }, select() {}, appendChild(c) { (this.kids ??= []).push(c); } });
  return { els, getElementById: (id) => mk(id), createElement: () => mk("tmp" + Math.random()) };
}

function setup(caps) {
  const doc = fakeDoc();
  for (const id of ["sound-row", "clip-row", "cam-row", "offer-row", "offer-camera", "offer-mic", "devices", "shared", "panel", "clip-box", "camview"]) doc.getElementById(id).hidden = true;
  doc.getElementById("sound-side").value = "client";
  const cmds = [], toasts = [], sent = [];
  const engine = { cmd: (c) => { cmds.push(c); return true; } };
  const media = createMedia({ engine, pump() {}, toast: (t) => toasts.push(t), send: { binary: (tag, b) => sent.push([tag, b]), buffered: () => 0 }, doc });
  media.setCaps(caps);
  return { doc, cmds, toasts, sent, media };
}

test("only the buttons the machine can serve are shown", () => {
  const a = setup(["picture", "input"]);
  assert.equal(a.doc.els["devices"].hidden, true);
  const b = setup(["picture", "audio.out", "clipboard.text"]);
  assert.equal(b.doc.els["devices"].hidden, false);
  assert.equal(b.doc.els["sound-row"].hidden, false);
  assert.equal(b.doc.els["clip-row"].hidden, false);
  assert.equal(b.doc.els["cam-row"].hidden, true);
  assert.equal(b.doc.els["offer-row"].hidden, true);
});

test("the clipboard buttons send a confirmed command only on a press", async () => {
  const s = setup(["clipboard.text"]);
  assert.equal(s.cmds.length, 0, "nothing at start");
  setNav({ clipboard: { readText: async () => "héllo", writeText: async () => {} } });
  await s.doc.els["clip-send"].listeners.click();
  assert.deepEqual(s.cmds, [{ c: "clip_put", text: "héllo", confirmed: true }]);
  s.doc.els["clip-get"].listeners.click();
  assert.deepEqual(s.cmds[1], { c: "clip_get", confirmed: true });
  // too long is refused before anything leaves the page
  globalThis.navigator.clipboard.readText = async () => "a".repeat(65537);
  await s.doc.els["clip-send"].listeners.click();
  assert.equal(s.cmds.length, 2);
  assert.match(s.toasts.at(-1), /64 KB/);
  // text from the machine goes to the clipboard; when the browser refuses it is shown in a box
  s.media.onMedia({ ev: "clipboard_text", body: { text: "from the machine" } });
  globalThis.navigator.clipboard.writeText = async () => { throw new Error("denied"); };
  s.media.onMedia({ ev: "clipboard_text", body: { text: "second" } });
  await new Promise((r) => setTimeout(r, 10));
  assert.equal(s.doc.els["clip-text"].value, "second");
  assert.equal(s.doc.els["clip-box"].hidden, false);
  setNav(undefined);
});

test("sound frames reach the worklet and a flush from the machine clears it", () => {
  const s = setup(["audio.out"]);
  const posted = [];
  s.media.state.node = { port: { postMessage: (m) => posted.push(m) } };
  const pcm = new Int16Array(1920).fill(16384);
  const msg = new Uint8Array(1 + 4 + pcm.byteLength);
  msg[0] = 2; new DataView(msg.buffer).setUint32(1, 77, true); msg.set(new Uint8Array(pcm.buffer), 5);
  assert.equal(s.media.onBinary(msg.buffer), true);
  assert.equal(posted[0].t, "pcm"); assert.equal(posted[0].seq, 77);
  assert.equal(posted[0].data.length, 1920); assert.equal(posted[0].data[0], 0.5);
  s.media.onMedia({ ev: "audio_flush", body: {} });
  assert.deepEqual(posted[1], { t: "flush" });
  assert.equal(s.media.onBinary(new Uint8Array([1, 2, 3]).buffer), false, "the picture is not for this module");
  // a ragged payload is ignored
  assert.equal(s.media.onBinary(new Uint8Array([2, 0, 0, 0, 0, 1]).buffer), true);
  assert.equal(posted.length, 2);
});

test("a refused offer releases the browser's camera at once and the red band shows only live sharing", async () => {
  const s = setup(["client.camera", "client.microphone"]);
  let stopped = 0;
  const track = { stop: () => stopped++ };
  setNav({ mediaDevices: { getUserMedia: async () => ({ getTracks: () => [track] }) } });
  await s.doc.els["offer-camera"].listeners.click();
  assert.deepEqual(s.cmds.at(-1), { c: "offer_open", kind: "camera" });
  assert.equal(s.doc.els["shared"].hidden, true, "not shown before the machine accepted");
  s.media.onMedia({ ev: "media_err", body: { what: "offer", code: "card_declined", message: "you said no" } });
  assert.equal(stopped, 1, "the browser's light goes off");
  assert.match(s.toasts.at(-1), /machine said no/);
  assert.equal(s.media.state.offers.camera, null);
  setNav(undefined);
});

test("reset stops the sound and every offer", async () => {
  const s = setup(["audio.out", "client.microphone"]);
  let stopped = 0, disconnected = 0;
  setNav({ mediaDevices: { getUserMedia: async () => ({ getTracks: () => [{ stop: () => stopped++ }] }) } });
  await s.doc.els["offer-mic"].listeners.click();
  s.media.state.ctx = { close() { disconnected++; } }; s.media.state.node = { disconnect() {}, port: { postMessage() {} } }; s.media.state.playing = true;
  s.media.reset();
  assert.equal(stopped, 1); assert.equal(disconnected, 1);
  assert.equal(s.media.state.playing, false); assert.equal(s.media.state.offers.microphone, null);
  assert.equal(s.doc.els["shared"].hidden, true);
  setNav(undefined);
});

test("the Devices button asks for the camera list and a picker shows names, watching with the picked id", () => {
  const s = setup(["camera.frames"]);
  s.doc.els["devices"].listeners.click();
  assert.deepEqual(s.cmds, [{ c: "camera_list" }]);
  s.media.onMedia({ ev: "camera_list", body: { cameras: [{ unit: "usb:video0", name: "Desk camera" }] } });
  assert.equal(s.doc.els["cam-pick"].hidden, false);
  assert.equal(s.doc.els["cam-unit"].hidden, true, "the typed id is gone once the list is known");
  assert.equal(s.doc.els["cam-pick"].kids[0].textContent, "Desk camera");
  s.doc.els["cam-pick"].value = "usb:video0";
  s.doc.els["cam-btn"].listeners.click();
  assert.deepEqual(s.cmds.at(-1), { c: "camera_open", unit: "usb:video0" });
  // an empty list says so
  s.media.onMedia({ ev: "camera_list", body: { cameras: [] } });
  assert.equal(s.doc.els["cam-pick"].hidden, true);
  assert.match(s.doc.els["cam-state"].textContent, /No camera/);
});
