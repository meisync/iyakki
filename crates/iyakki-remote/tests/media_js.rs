//! The page's browser-devices code (`src/media.js`) under node with stand-ins for `navigator`, WebAudio and the camera.
//! Skipped (passes, saying so) when `node` is not installed. Nothing is installed or downloaded; the files go in a temporary
//! folder that is removed afterwards. This is NOT a real browser, microphone or camera.

const DRIVER: &str = r##"
const path = require("path");
const M = require(path.join(process.argv[2], "media.js"));
let bad = 0;
const ok = (c, m) => { console.log((c ? "OK  " : "BAD ") + m); if (!c) bad++; };
const trusted = { isTrusted: true }, synthetic = { isTrusted: false };

function rig(over) {
  const sent = [], bin = [], stopped = [];
  const track = { stop() { stopped.push(1); } };
  const stream = { getTracks: () => [track] };
  let procs = [];
  class Ctx {
    constructor() { this.sampleRate = 48000; this.currentTime = 0; this.destination = {}; this.created = []; }
    createMediaStreamSource() { return { connect() {}, disconnect() {} }; }
    createScriptProcessor() { const p = { connect() {}, disconnect() {}, onaudioprocess: null }; procs.push(p); return p; }
    createGain() { return { gain: { value: 1 }, connect() {} }; }
    createBuffer(ch, n, rate) { const d = Array.from({ length: ch }, () => new Float32Array(n)); return { duration: n / rate, getChannelData: (c) => d[c] }; }
    createBufferSource() { const s = { start(t) { s.at = t; ctx_starts.push(t); }, connect() {} }; return s; }
    resume() {} suspend() {} close() {}
  }
  const ctx_starts = [];
  let t = 0;
  const calls = [];
  const env = Object.assign({
    nav: { mediaDevices: { getUserMedia: async (c) => { calls.push(c); return stream; } }, clipboard: { read: async () => [], write: async () => {} } },
    document: { createElement: () => ({ getContext: () => ({ drawImage() {} }), play: async () => {}, toBlob() {} }) },
    AudioContext: Ctx, ClipboardItem: class { constructor(o) { this.o = o; } }, Blob: class { constructor(p, o) { this.p = p; } },
    TextEncoder, isSecureContext: true, now: () => t, setInterval: () => 1,
  }, over || {});
  const changes = [];
  const m = M.create(env, { sendJson: (o) => sent.push(o), sendBinary: (b) => bin.push(b), onChange: (s) => changes.push(JSON.stringify(s.on)) });
  return { m, sent, bin, stopped, procs, env, calls, ctx_starts, tick: (ms) => { t += ms; } };
}
const allow = (r, a) => r.m.onJson({ t: "media_state", mic: false, camera: false, clipread: false, allowed: Object.assign({ mic: true, camera: true, clipread: true, audio: true }, a || {}) });

(async () => {
  // framing helpers
  ok(M.tagged(1, [7, 8])[0] === 1 && M.tagged(1, [7, 8]).length === 3, "tagged adds the channel byte");
  const pcm = M.toPcm16k(new Float32Array(480).fill(0.5), 48000);
  ok(pcm.length === 160 && pcm[0] === 16384, "48 kHz to 16 kHz keeps the level");
  ok(M.toPcm16k(new Float32Array([2, -2]), 16000)[0] === 32767 && M.toPcm16k(new Float32Array([2, -2]), 16000)[1] === -32768, "clipped, not wrapped");
  const msg = new Uint8Array(10 + 8); msg[0] = 0x10; msg[1] = 2; new DataView(msg.buffer).setUint32(2, 48000, true); new DataView(msg.buffer).setInt16(10, -16384, true);
  const a = M.parseAudio(msg); ok(a && a.channels === 2 && a.rate === 48000 && a.samples.length === 4 && a.samples[0] === -16384, "parses a sound message");
  ok(M.parseAudio(new Uint8Array([0x10, 3, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1])) === null && M.parseAudio(new Uint8Array(4)) === null, "refuses bad sound messages");

  // nothing starts without a click, or with the policy against it, or off https
  let r = rig(); allow(r);
  ok(await r.m.toggle("mic", synthetic) === false && r.calls.length === 0 && r.sent.length === 0, "a script cannot switch the microphone on");
  ok(await r.m.toggle("mic", undefined) === false && r.calls.length === 0, "no event, no microphone");
  r = rig(); allow(r, { mic: false });
  ok(await r.m.toggle("mic", trusted) === false && r.calls.length === 0 && /not allow/.test(r.m.state.notice), "policy deny: no getUserMedia call, and it says so");
  r = rig({ isSecureContext: false }); allow(r);
  ok(await r.m.toggle("camera", trusted) === false && r.calls.length === 0 && /secure/.test(r.m.state.notice), "plain http: says it needs https");

  // microphone: click, chunks, indicator, one tap to stop
  r = rig(); allow(r);
  ok(await r.m.toggle("mic", trusted) === true && r.m.state.on.mic && r.sent[0].t === "media" && r.sent[0].on === true, "click switches the microphone on and tells the server");
  ok(r.calls[0].audio && !r.calls[0].video, "asks for audio only");
  const proc = r.procs[0];
  const frame = () => ({ inputBuffer: { getChannelData: () => new Float32Array(4096).fill(0.25) } });
  for (let i = 0; i < 4; i++) proc.onaudioprocess(frame());
  ok(r.bin.length >= 1 && r.bin[0][0] === 1 && r.bin[0].length % 2 === 1, "PCM chunks go out as channel 1 with an even body");
  ok(r.m.state.level > 0.2, "the level meter moves");
  r.tick(301000); proc.onaudioprocess(frame());
  ok(!r.m.state.on.mic && r.sent[r.sent.length - 1].on === false && r.stopped.length === 1, "five minutes stop it and release the device");
  r = rig(); allow(r); await r.m.toggle("mic", trusted);
  await r.m.toggle("mic", undefined);
  ok(!r.m.state.on.mic && r.stopped.length === 1 && r.sent[r.sent.length - 1].on === false, "one tap stops it (no gesture needed to stop)");
  r = rig(); allow(r); await r.m.toggle("mic", trusted);
  r.m.onJson({ t: "media_limit", kind: "mic" });
  ok(!r.m.state.on.mic && r.stopped.length === 1, "the server's limit notice stops it");
  r = rig(); allow(r); await r.m.toggle("mic", trusted); r.m.disconnected();
  ok(!r.m.state.on.mic && r.stopped.length === 1, "a lost connection releases the microphone");
  r = rig({ nav: { mediaDevices: { getUserMedia: async () => { const e = new Error("x"); e.name = "NotAllowedError"; throw e; } } } }); allow(r);
  ok(await r.m.toggle("mic", trusted) === false && !r.m.state.on.mic && /refused/.test(r.m.state.notice) && r.sent.length === 0, "a refusal in the browser leaves it off and tells nobody it is on");

  // camera
  r = rig(); allow(r);
  ok(await r.m.toggle("camera", trusted) === true && r.calls[0].video && !r.calls[0].audio && r.m.state.on.camera, "camera on with video only");
  await r.m.toggle("camera", trusted);
  ok(!r.m.state.on.camera && r.stopped.length === 1, "tap again stops it and releases the camera");

  // a state message that crosses our own "on" does not switch it off, a later one does
  r = rig(); allow(r); await r.m.toggle("camera", trusted);
  r.m.onJson({ t: "media_state", mic: false, camera: false, clipread: false, allowed: { camera: true } });
  ok(r.m.state.on.camera, "a stale state is ignored for a moment");
  r.tick(2000); r.m.onJson({ t: "media_state", mic: false, camera: false, clipread: false, allowed: { camera: true } });
  ok(!r.m.state.on.camera, "the server decides afterwards");

  // sound: only after a click, then queued with a lead, never lagging
  r = rig(); allow(r);
  ok(r.m.startSound(synthetic) === false && !r.m.state.sound, "sound does not start without a click");
  ok(r.m.startSound(trusted) === true && r.m.state.sound && r.sent[0].t === "sink" && r.sent[1].t === "audio" && r.sent[1].on === true, "a click starts the sound (route first)");
  const snd = new Uint8Array(10 + 4 * 480); snd[0] = 0x10; snd[1] = 2; new DataView(snd.buffer).setUint32(2, 48000, true);
  ok(r.m.onBinary(snd) === true && r.ctx_starts.length === 1 && r.ctx_starts[0] >= 0.1, "the first chunk waits for the jitter lead");
  for (let i = 0; i < 100; i++) r.m.onBinary(snd);
  ok(r.m.player.dropped > 0 && r.m.player.next - r.m.player.ctx.currentTime <= 0.65, "a burst is dropped, not queued forever");
  r.m.player.setVolume(2); ok(r.m.player.gain.gain.value === 1, "volume is clamped");
  r.m.player.setMuted(true); ok(r.m.player.gain.gain.value === 0, "mute");
  r.m.stopSound(); ok(!r.m.state.sound && r.sent[r.sent.length - 1].on === false, "stop sound tells the server");

  // sequence gaps are counted (the server drops old sound; the page never waits for it), a stop starts counting afresh
  r = rig(); allow(r); r.m.startSound(trusted);
  const mk = (seq) => { const m = new Uint8Array(10 + 4 * 480); m[0] = 0x10; m[1] = 2; new DataView(m.buffer).setUint32(2, 48000, true); new DataView(m.buffer).setUint32(6, seq, true); return m; };
  r.m.onBinary(mk(0)); r.m.onBinary(mk(1)); r.m.onBinary(mk(5));
  ok(r.m.player.stats().lost === 3 && r.m.player.stats().played >= 2, "three missing chunks are counted as lost");
  r.m.stopSound(); ok(r.m.player.lastSeq === -1, "stop forgets the sequence");
  r.m.startSound(trusted); const before = r.m.player.stats().lost;
  r.m.onBinary(mk(0xFFFFFFFE)); r.m.onBinary(mk(0xFFFFFFFF)); r.m.onBinary(mk(0));
  ok(r.m.player.stats().lost === before, "the sequence wraps without a false loss");
  r = rig(); allow(r);
  r.m.onBinary(mk(0)); ok(r.ctx_starts.length === 0, "sound that arrives while the page has not started it is ignored (silence)");

  // a tile asks for a source: the page shows its Allow button, and only the person's click starts it
  r = rig(); allow(r);
  r.m.onJson({ t: "media_ask", kind: "mic" });
  ok(r.m.state.ask === "mic" && r.calls.length === 0 && !r.m.state.on.mic, "asked: the button shows, nothing is switched on by the ask");
  ok(await r.m.toggle("mic", synthetic) === false && r.calls.length === 0, "a script cannot answer the ask");
  ok(await r.m.toggle("mic", trusted) === true && r.m.state.on.mic && r.m.state.ask === "", "the person's click starts it and clears the ask");
  r.m.onJson({ t: "media_ask", kind: "mic" });
  ok(r.m.state.ask === "", "already on: no ask");
  r.m.onJson({ t: "media_ask", kind: "laser" }); ok(r.m.state.ask === "", "an unknown kind is ignored");
  r.m.disconnected(); ok(r.m.state.ask === "" && !r.m.state.on.mic, "a lost connection clears everything");

  // clipboard: reading needs the switch and a gesture
  r = rig(); allow(r);
  await r.m.onReadRequest();
  ok(/off/.test(r.m.state.notice) && r.bin.length === 0, "the shell asking is not enough while the switch is off");
  ok(await r.m.toggle("clipread", trusted) === true, "switch on");
  r.env.nav.userActivation = { isActive: false };
  let reads = 0; r.env.nav.clipboard.read = async () => { reads++; return [{ types: ["image/png", "text/plain"], getType: async (t) => t === "image/png" ? { size: 3, arrayBuffer: async () => new Uint8Array([1, 2, 3]).buffer } : { text: async () => "hi" } }]; };
  await r.m.onReadRequest();
  ok(reads === 0 && r.m.state.pendingRead, "without a user gesture the clipboard is not read; the page asks for a click");
  ok(await r.m.readClipboard(synthetic) === false && reads === 0, "a script's click does not count");
  ok(await r.m.readClipboard(trusted) === true && reads === 1 && r.bin[0][0] === 3 && r.sent.some((o) => o.t === "paste" && o.s === "hi"), "a click reads image and text and sends them");
  r.env.nav.userActivation = { isActive: true }; await r.m.onReadRequest();
  ok(reads === 2, "with an active gesture it reads straight away");

  // paste events: text always, rich items only when allowed
  r = rig(); allow(r, { clipread: false });
  const ev = { clipboardData: { files: [{ type: "image/png", size: 4, arrayBuffer: async () => new Uint8Array([137, 80, 78, 71]).buffer }], getData: (k) => (k === "text/html" ? "<b>x</b>" : k === "text" ? "plain" : "") } };
  await r.m.onPaste(ev);
  ok(r.bin.length === 0 && r.sent.some((o) => o.t === "paste" && o.s === "plain"), "policy deny: only the text is sent");
  r = rig(); allow(r); await r.m.onPaste(ev);
  ok(r.bin.length === 2 && r.bin[0][0] === 3 && r.bin[1][0] === 4, "images and HTML go as channels 3 and 4");
  const big = { clipboardData: { files: [{ type: "image/png", size: 5 * 1024 * 1024, arrayBuffer: async () => new ArrayBuffer(5) }], getData: () => "" } };
  r = rig(); allow(r); await r.m.onPaste(big);
  ok(r.bin.length === 0, "an image over the limit is not sent");

  // clipboard to the browser: written, or kept for a click when the browser wants a gesture
  r = rig(); allow(r); const w = []; r.env.nav.clipboard.write = async (x) => w.push(x);
  const png = new Uint8Array(20); png[0] = 0x11; r.m.onBinary(png); await new Promise((q) => setTimeout(q, 5));
  ok(w.length === 1, "a PNG from the session is written to the clipboard");
  r = rig(); allow(r); r.env.nav.clipboard.write = async () => { throw new Error("NotAllowed"); };
  r.m.onBinary(png); await new Promise((q) => setTimeout(q, 5));
  ok(r.m.state.pendingCopy && r.m.state.pendingCopy.kind === "image", "without a gesture the item waits for a click");
  console.log(bad ? "FAILED" : "ALL OK");
  process.exit(bad ? 1 : 0);
})();
"##;

#[test]
fn the_pages_browser_devices_code_follows_its_rules_under_node() {
    let ok = std::process::Command::new("node").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP: node is not installed");
        return;
    }
    let dir = std::env::temp_dir().join(format!("iyakki-media-js-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("media.js"), iyakki_remote::page::MEDIA_JS).unwrap();
    std::fs::write(dir.join("driver.js"), DRIVER).unwrap();
    let out = std::process::Command::new("node").arg(dir.join("driver.js")).arg(&dir).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success() && !text.contains("BAD"), "{text}");
    assert!(text.contains("ALL OK"), "{text}");
}

#[test]
fn the_page_carries_the_switches_the_indicator_and_the_script() {
    let p = iyakki_remote::page::INDEX;
    for id in ["dmic", "dcam", "dclip", "dsound", "dvol", "rec", "askclip", "askdev"] {
        assert!(p.contains(&format!("id=\"{id}\"")), "{id}");
    }
    assert!(p.contains("<script src=\"/media.js\">") && p.contains("media.onReadRequest()"), "the page uses the bridge");
    assert!(!p.contains("readText"), "the clipboard is no longer read without the switch and a gesture");
    // nothing starts by itself: no getUserMedia call outside media.js
    assert!(!p.contains("getUserMedia"));
    assert!(iyakki_remote::page::MEDIA_JS.contains("isTrusted"));
}

const DRIVER_ERRORS: &str = r##"
const path = require("path");
const M = require(path.join(process.argv[2], "media.js"));
let bad = 0;
const ok = (c, m) => { console.log((c ? "OK  " : "BAD ") + m); if (!c) bad++; };
const trusted = { isTrusted: true };
function rig(over, fail) {
  const sent = [], track = { stop() {} }, stream = { getTracks: () => [track] };
  class Ctx { constructor() { this.destination = {}; } createGain() { return { gain: { value: 1 }, connect() {} }; } resume() {} suspend() {} close() {} }
  const env = Object.assign({
    nav: { mediaDevices: { getUserMedia: async () => { if (fail) { const e = new Error(fail.message || "x"); e.name = fail.name; throw e; } return stream; } } },
    document: { createElement: () => ({ getContext: () => ({ drawImage() {} }), play: async () => {}, toBlob() {} }) },
    AudioContext: Ctx, isSecureContext: true, now: () => 0, setInterval: () => 1, TextEncoder, Blob: class {},
  }, over || {});
  const m = M.create(env, { sendJson: (o) => sent.push(o), sendBinary() {}, onChange() {} });
  m.onJson({ t: "media_state", mic: false, camera: false, clipread: false, allowed: { mic: true, camera: true, clipread: true, audio: true } });
  return { m, sent };
}
(async () => {
  const cases = [
    ["NotAllowedError", "", /refused access to the camera.*address bar/s, "permission denied"],
    ["NotAllowedError", "Permission denied by system", /operating system blocks.*camera/s, "blocked by the operating system"],
    ["NotFoundError", "", /No camera was found/, "no camera"],
    ["NotReadableError", "", /cannot be read.*another program/s, "camera busy"],
    ["OverconstrainedError", "", /cannot give the picture/, "constraints"],
    ["SomethingNew", "odd", /Could not start the camera \(SomethingNew: odd\)/, "unknown names carry the browser's words"],
  ];
  for (const [name, message, re, label] of cases) {
    const r = rig(null, { name, message });
    const res = await r.m.toggle("camera", trusted);
    ok(res === false && !r.m.state.on.camera && re.test(r.m.state.notice) && !r.sent.some((o) => o.on === true), label + ": " + r.m.state.notice);
  }
  let r = rig({ isSecureContext: false });
  ok(await r.m.toggle("camera", trusted) === false && /not secure/.test(r.m.state.notice) && /https:\/\//.test(r.m.state.notice) && !r.m.state.secure.ok, "insecure context: says so and what to open");
  r = rig({ isSecureContext: false, nav: {} });
  ok(await r.m.toggle("mic", trusted) === false && /not secure/.test(r.m.state.notice), "insecure context is reported before a missing mediaDevices");
  r = rig({ nav: {} });
  ok(await r.m.toggle("camera", trusted) === false && /navigator\.mediaDevices is missing/.test(r.m.state.notice), "no mediaDevices on a secure page");
  r = rig({ document: { permissionsPolicy: { allowsFeature: (f) => f !== "camera" }, createElement: () => ({}) } }, { name: "NotAllowedError" });
  ok(await r.m.toggle("camera", trusted) === false && /Permissions-Policy blocks the camera/.test(r.m.state.notice), "a page policy that forbids the camera is named");
  r = rig(null, { name: "NotAllowedError" });
  ok(await r.m.toggle("mic", trusted) === false && /refused access to the microphone/.test(r.m.state.notice), "same words for the microphone");
  ok(/NotAllowedError/.test(M.explain("camera", { name: "Weird" }, true)) === false && /Weird/.test(M.explain("camera", { name: "Weird" }, true)), "explain is pure");

  // the sound route: default browser, sent before the sound starts, changes sent at once while it plays, bad values ignored
  r = rig();
  ok(r.m.state.sink === "browser", "default route is the browser");
  r.m.setSink("both");
  ok(r.m.state.sink === "both" && r.sent.length === 0, "chosen before the sound starts: nothing is sent yet");
  ok(r.m.startSound(trusted) === true && r.sent[0].t === "sink" && r.sent[0].mode === "both" && r.sent[1].t === "audio" && r.sent[1].on === true, "Start sound sends the route first");
  r.m.setSink("mac");
  ok(r.sent[2].t === "sink" && r.sent[2].mode === "mac", "a change while playing is sent");
  ok(r.m.setSink("loud") === false && r.m.state.sink === "mac", "an unknown route is ignored");
  r.m.onJson({ t: "media_state", sink: "browser", allowed: { audio: true } });
  ok(r.m.state.sink === "browser", "the server's state shows the route");
  r.m.onJson({ t: "media_state", sink: "nonsense", allowed: { audio: true } });
  ok(r.m.state.sink === "browser", "a nonsense route from the server is ignored");
  r.m.disconnected();
  ok(!r.m.state.sound && r.m.state.sink === "browser", "a lost connection stops the sound; the choice stays for the session");
  console.log(bad ? "FAILED" : "ALL OK");
  process.exit(bad ? 1 : 0);
})();
"##;

#[test]
fn camera_errors_are_exact_and_the_sound_route_is_sent_under_node() {
    let ok = std::process::Command::new("node").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP: node is not installed");
        return;
    }
    let dir = std::env::temp_dir().join(format!("iyakki-media-js-err-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("media.js"), iyakki_remote::page::MEDIA_JS).unwrap();
    std::fs::write(dir.join("driver.js"), DRIVER_ERRORS).unwrap();
    let out = std::process::Command::new("node").arg(dir.join("driver.js")).arg(&dir).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success() && !text.contains("BAD"), "{text}");
    assert!(text.contains("ALL OK"), "{text}");
}

#[test]
fn the_page_has_the_sound_route_control_and_a_visible_error_line() {
    let p = iyakki_remote::page::INDEX;
    assert!(p.contains("id=\"dsink\"") && p.contains("This computer (the browser)") && p.contains(">Both<") && p.contains("The Mac only"));
    assert!(p.contains("value=\"browser\"") && p.contains("value=\"both\"") && p.contains("value=\"mac\""));
    assert!(p.contains("id=\"deverr\"") && p.contains("role=\"alert\""), "device errors are shown outside the closed settings panel");
    assert!(p.contains("media.setSink("));
}
