// The browser as the hardware of a session: clipboard images and HTML, sound out (WebAudio), microphone and webcam (getUserMedia).
// The server side and the wire format are in crates/iyakki-agent/src/browsermedia.rs. Served at /media.js; it also loads under
// node (for the crate's tests) with stand-ins passed in `env`.
//
// Rules: the microphone, the camera and reading the clipboard start only from a real click or key (an event with isTrusted),
// are off until then, and stop with one tap; the clipboard is never read without a user gesture; everything needs a secure
// context (https or localhost); the server may forbid a kind, and then the switch says so. Nothing is recorded or kept here.
"use strict";
(function (root) {
  const CH = { mic: 1, camera: 2, image: 3, html: 4 };
  const OUT = { audio: 0x10, image: 0x11, html: 0x12 };
  const MIC_RATE = 16000, MIC_LIMIT_S = 300, MAX_CAMERA = 256 * 1024, MAX_IMAGE = 4 * 1024 * 1024, MAX_HTML = 1024 * 1024;
  const KINDS = ["mic", "camera", "clipread"];
  const SINKS = ["browser", "both", "mac"];

  // ---- pure helpers (tested under node)
  function tagged(ch, bytes) {
    const u = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    const m = new Uint8Array(1 + u.length);
    m[0] = ch; m.set(u, 1);
    return m;
  }
  // Float samples at `rate` to 16-bit PCM at 16 kHz (averaging neighbours when shrinking, which is a plain low-pass).
  function toPcm16k(f32, rate) {
    const ratio = rate / MIC_RATE, n = Math.max(0, Math.floor(f32.length / ratio)), out = new Int16Array(n);
    for (let i = 0; i < n; i++) {
      const a = Math.floor(i * ratio), b = Math.max(a + 1, Math.min(f32.length, Math.floor((i + 1) * ratio)));
      let s = 0; for (let k = a; k < b; k++) s += f32[k];
      const v = Math.max(-1, Math.min(1, s / (b - a)));
      out[i] = v < 0 ? Math.round(v * 32768) : Math.round(v * 32767);
    }
    return out;
  }
  // A sound message of the server: [0x10, channels, rate u32, seq u32, 16-bit PCM...] or null when it is not one.
  function parseAudio(buf) {
    const u = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
    if (u.length < 10 || u[0] !== OUT.audio) return null;
    const dv = new DataView(u.buffer, u.byteOffset, u.byteLength), channels = u[1], rate = dv.getUint32(2, true), seq = dv.getUint32(6, true);
    if (channels < 1 || channels > 2 || rate < 8000 || rate > 96000) return null;
    const bytes = (u.length - 10) - ((u.length - 10) % (2 * channels));
    if (bytes <= 0) return null;
    const samples = new Int16Array(bytes / 2);
    for (let i = 0; i < samples.length; i++) samples[i] = dv.getInt16(10 + i * 2, true);
    return { channels, rate, seq, samples };
  }
  function secureOk(env) {
    if (env.isSecureContext === false) return { ok: false, why: "The browser treats this page as not secure, so it hides the camera and microphone. Open the https:// address (accept the certificate warning once), or open the page on this computer itself." };
    return { ok: true, why: "" };
  }
  // The exact sentence for a getUserMedia failure. `what` is "camera" or "microphone"; `policyOk` is false when the page's own
  // Permissions-Policy forbids it. The browser's own words (`e.message`) are added for the cases that have no better text.
  function explain(what, e, policyOk) {
    const name = (e && e.name) || "Error", msg = (e && e.message) ? String(e.message) : "";
    if (policyOk === false) return "This page's Permissions-Policy blocks the " + what + ". This is a fault of the page: tell the administrator.";
    if (name === "NotAllowedError" || name === "PermissionDeniedError") {
      if (/system/i.test(msg)) return "The computer's operating system blocks your browser from the " + what + ". Allow the browser under Privacy settings (Camera or Microphone), then try again.";
      return "The browser refused access to the " + what + " (you pressed Block, or blocked this site before). Click the lock or camera icon at the left of the address bar, set the " + what + " to Allow, and click again.";
    }
    if (name === "NotFoundError" || name === "DevicesNotFoundError") return "No " + what + " was found on your computer. Plug one in (or check it is switched on) and click again.";
    if (name === "NotReadableError" || name === "TrackStartError") return "Your " + what + " cannot be read: another program or browser tab is probably using it. Close it and click again.";
    if (name === "OverconstrainedError" || name === "ConstraintNotSatisfiedError") return "Your " + what + " cannot give the picture this page asks for.";
    if (name === "SecurityError") return "The browser refuses the " + what + " for security reasons: the page is not in a secure context.";
    if (name === "AbortError") return "The " + what + " could not be started (the browser aborted it). Try again.";
    return "Could not start the " + what + " (" + name + (msg ? ": " + msg.slice(0, 120) : "") + ").";
  }

  // ---- sound out: PCM chunks into a WebAudio queue with a small jitter buffer
  const TARGET_LEAD = 0.12, MAX_LEAD = 0.6;
  function makePlayer(env) {
    const p = { ctx: null, gain: null, next: 0, volume: 0.8, muted: false, underruns: 0, dropped: 0, played: 0, started: false };
    p.start = function () {           // from a click: browsers only allow sound after one
      if (!env.AudioContext) return false;
      if (!p.ctx) { p.ctx = new env.AudioContext({ latencyHint: "interactive" }); p.gain = p.ctx.createGain(); p.gain.connect(p.ctx.destination); p.apply(); }
      if (p.ctx.resume) p.ctx.resume();
      p.started = true;
      return true;
    };
    p.stop = function () { p.started = false; p.next = 0; p.lastSeq = -1; if (p.ctx && p.ctx.suspend) p.ctx.suspend(); };
    p.apply = function () { if (p.gain) p.gain.gain.value = p.muted ? 0 : p.volume; };
    p.setVolume = function (v) { p.volume = Math.max(0, Math.min(1, v)); p.apply(); };
    p.setMuted = function (m) { p.muted = !!m; p.apply(); };
    p.lastSeq = -1; p.lost = 0;
    // sequence numbers count up by one; a gap means the server dropped old sound (it never delays it), which is only counted
    p.seen = function (seq) {
      if (p.lastSeq >= 0 && seq !== ((p.lastSeq + 1) >>> 0)) p.lost += ((seq - p.lastSeq - 1) >>> 0) % 1000000;
      p.lastSeq = seq;
    };
    p.stats = function () { return { underruns: p.underruns, dropped: p.dropped, lost: p.lost, played: p.played }; };
    p.push = function (a) {
      if (!p.started || !p.ctx) return false;
      const frames = a.samples.length / a.channels, buf = p.ctx.createBuffer(a.channels, frames, a.rate);
      for (let c = 0; c < a.channels; c++) {
        const d = buf.getChannelData(c);
        for (let i = 0; i < frames; i++) d[i] = a.samples[i * a.channels + c] / 32768;
      }
      const now = p.ctx.currentTime;
      if (p.next < now + 0.02) { p.next = now + TARGET_LEAD; p.underruns++; }          // ran dry: build the lead again
      else if (p.next - now > MAX_LEAD) { p.dropped++; return false; }                // too far ahead: drop, never lag
      const src = p.ctx.createBufferSource();
      src.buffer = buf; src.connect(p.gain); src.start(p.next);
      p.next += buf.duration; p.played++;
      return true;
    };
    return p;
  }

  // ---- the controller: state, switches, and the three channels
  function create(env, io) {
    const st = {
      secure: secureOk(env),
      allowed: { mic: false, camera: false, clipread: false, audio: true },
      on: { mic: false, camera: false, clipread: false },
      sound: false, sink: "browser", level: 0, micSeconds: 0, notice: "", ask: "", pendingRead: false, pendingCopy: null,
    };
    const player = makePlayer(env);
    let mic = null, cam = null;
    const sentOn = { mic: -1e9, camera: -1e9, clipread: -1e9 };
    // false only when the browser says the page's own Permissions-Policy forbids the feature (Chrome and Edge know; others say nothing)
    const policyAllows = (what) => {
      try {
        const pp = env.document && (env.document.permissionsPolicy || env.document.featurePolicy);
        return pp && pp.allowsFeature ? pp.allowsFeature(what === "camera" ? "camera" : "microphone") !== false : true;
      } catch (e) { return true; }
    };
    const changed = () => io.onChange && io.onChange(st);
    const trusted = (ev) => !!ev && ev.isTrusted === true;
    const say = (t) => { st.notice = t; changed(); };

    function stopMic() {
      if (!mic) return;
      const m = mic; mic = null;
      try { m.node.onaudioprocess = null; m.node.disconnect(); m.src.disconnect(); } catch (e) {}
      try { m.stream.getTracks().forEach((t) => t.stop()); } catch (e) {}
      try { m.ctx.close(); } catch (e) {}
      if (st.on.mic) { st.on.mic = false; io.sendJson({ t: "media", kind: "mic", on: false }); }
      st.level = 0; changed();
    }
    function stopCamera() {
      if (!cam) return;
      const c = cam; cam = null;
      clearInterval(c.timer);
      try { c.stream.getTracks().forEach((t) => t.stop()); } catch (e) {}
      if (c.video) { try { c.video.srcObject = null; } catch (e) {} }
      if (st.on.camera) { st.on.camera = false; io.sendJson({ t: "media", kind: "camera", on: false }); }
      changed();
    }

    async function startMic() {
      const stream = await env.nav.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } });
      const ctx = new env.AudioContext(), src = ctx.createMediaStreamSource(stream), node = ctx.createScriptProcessor(4096, 1, 1);
      mic = { stream, ctx, src, node, acc: [], accLen: 0, t0: env.now() };
      node.onaudioprocess = (e) => {
        if (!mic) return;
        const f = e.inputBuffer.getChannelData(0);
        let peak = 0; for (let i = 0; i < f.length; i++) peak = Math.max(peak, Math.abs(f[i]));
        st.level = peak;
        const pcm = toPcm16k(f, ctx.sampleRate);
        mic.acc.push(pcm); mic.accLen += pcm.length;
        if (mic.accLen >= 4000) {                       // about a quarter of a second
          const all = new Int16Array(mic.accLen); let o = 0; for (const a of mic.acc) { all.set(a, o); o += a.length; }
          mic.acc = []; mic.accLen = 0;
          io.sendBinary(tagged(CH.mic, new Uint8Array(all.buffer)));
        }
        st.micSeconds = Math.floor((env.now() - mic.t0) / 1000);
        if (st.micSeconds >= MIC_LIMIT_S) { say("The microphone stopped after five minutes."); stopMic(); } else changed();
      };
      src.connect(node); node.connect(ctx.destination);   // a script processor only runs while it is connected
    }

    async function startCamera() {
      const stream = await env.nav.mediaDevices.getUserMedia({ video: { width: { ideal: 640 }, height: { ideal: 480 }, frameRate: { ideal: 5 } } });
      const video = env.document.createElement("video"), canvas = env.document.createElement("canvas");
      video.muted = true; video.playsInline = true; video.srcObject = stream;
      cam = { stream, video, canvas, quality: 0.7, busy: false, timer: null };   // set first: a failure below still releases the camera
      if (video.play) await video.play();
      cam.timer = env.setInterval(() => {
        if (!cam || cam.busy || !video.videoWidth) return;
        const w = Math.min(640, video.videoWidth), h = Math.round(video.videoHeight * w / video.videoWidth);
        canvas.width = w; canvas.height = h;
        canvas.getContext("2d").drawImage(video, 0, 0, w, h);
        cam.busy = true;
        canvas.toBlob(async (b) => {
          try {
            if (!b || !cam) return;
            const u = new Uint8Array(await b.arrayBuffer());
            if (u.length > MAX_CAMERA) { cam.quality = Math.max(0.2, cam.quality * 0.8); return; }   // too big: lower the quality, skip this one
            io.sendBinary(tagged(CH.camera, u));
          } finally { if (cam) cam.busy = false; }
        }, "image/jpeg", cam.quality);
      }, 250);
    }

    // The switch in the page. `ev` must be the person's own click or key.
    async function toggle(kind, ev) {
      if (KINDS.indexOf(kind) < 0) return false;
      if (st.ask === kind) { st.ask = ""; changed(); }
      if (st.on[kind]) {                                // one tap to stop
        if (kind === "mic") stopMic(); else if (kind === "camera") stopCamera(); else { st.on.clipread = false; io.sendJson({ t: "media", kind, on: false }); changed(); }
        return true;
      }
      if (!trusted(ev)) { say("Switch it on with a click or a key press."); return false; }
      if (!st.secure.ok) { say(st.secure.why); return false; }
      if (!st.allowed[kind]) { say("This session does not allow it."); return false; }
      try {
        if (kind === "mic" || kind === "camera") {
          if (!env.nav.mediaDevices || !env.nav.mediaDevices.getUserMedia) { say("This browser has no camera or microphone interface (navigator.mediaDevices is missing). " + (st.secure.ok ? "Use a current Chrome, Firefox, Edge or Safari." : st.secure.why)); return false; }
          if (kind === "mic") await startMic(); else await startCamera();
        } else if (!env.nav.clipboard || !env.nav.clipboard.read) { say("This browser cannot read the clipboard."); return false; }
        st.on[kind] = true; st.notice = ""; sentOn[kind] = env.now();
        io.sendJson({ t: "media", kind, on: true });
        changed();
        return true;
      } catch (e) {
        if (kind === "mic") stopMic(); else if (kind === "camera") stopCamera();
        const what = kind === "camera" ? "camera" : kind === "mic" ? "microphone" : "clipboard";
        if (kind === "clipread") say("The browser did not let the clipboard be read.");
        else say(explain(what, e, policyAllows(what)));
        return false;
      }
    }

    function startSound(ev) {
      if (!trusted(ev)) { say("Start the sound with a click."); return false; }
      if (!player.start()) { say("This browser cannot play the session's sound."); return false; }
      st.sound = true; io.sendJson({ t: "sink", mode: st.sink }); io.sendJson({ t: "audio", on: true }); changed();
      return true;
    }
    // "Play sound on": this computer (the browser), both, or the Mac only. Sent at once when the sound is on; else with Start sound.
    function setSink(mode) {
      if (SINKS.indexOf(mode) < 0) return false;
      st.sink = mode;
      if (st.sound) io.sendJson({ t: "sink", mode });
      // "The Mac only": nothing comes to the page, so its player is idle (it is switched off here and the server sends nothing)
      changed();
      return true;
    }
    function stopSound() { player.stop(); st.sound = false; io.sendJson({ t: "audio", on: false }); changed(); }

    // ---- clipboard
    async function copyIn(item) {                       // a PNG or HTML message of the server to the browser's clipboard
      if (!env.nav.clipboard || !env.nav.clipboard.write || !env.ClipboardItem) return false;
      const type = item.kind === "image" ? "image/png" : "text/html";
      try {
        await env.nav.clipboard.write([new env.ClipboardItem({ [type]: new env.Blob([item.bytes], { type }) })]);
        st.pendingCopy = null; changed(); return true;
      } catch (e) { st.pendingCopy = item; changed(); return false; }   // no gesture yet: the "Copy" button finishes it
    }
    async function finishCopy(ev) { if (trusted(ev) && st.pendingCopy) return copyIn(st.pendingCopy); return false; }

    // Ctrl+V or the context menu's Paste: the browser hands over what the person pasted; images and HTML go by binary messages.
    async function onPaste(e) {
      const cd = e.clipboardData; if (!cd) return false;
      let handled = false;
      if (st.allowed.clipread) {
        for (const f of (cd.files || [])) {
          if (f.type === "image/png" && f.size <= MAX_IMAGE) { io.sendBinary(tagged(CH.image, new Uint8Array(await f.arrayBuffer()))); handled = true; break; }
        }
        const html = cd.getData && cd.getData("text/html");
        if (html && new env.TextEncoder().encode(html).length <= MAX_HTML) { io.sendBinary(tagged(CH.html, new env.TextEncoder().encode(html))); handled = true; }
      }
      const text = cd.getData && cd.getData("text");
      if (text) { io.sendJson({ t: "paste", s: text }); handled = true; }
      return handled;
    }

    // The shell asks for the browser's clipboard: read only with the switch on and a user gesture; otherwise wait for a click.
    async function onReadRequest() {
      if (!st.on.clipread || !env.nav.clipboard) { say("Clipboard sharing is off. Switch it on in Browser devices, or press Ctrl+V."); return false; }
      const active = env.nav.userActivation ? env.nav.userActivation.isActive : false;
      if (!active) { st.pendingRead = true; changed(); return false; }
      return readClipboard({ isTrusted: true });
    }
    async function readClipboard(ev) {
      if (!trusted(ev) || !st.on.clipread || !env.nav.clipboard) return false;
      st.pendingRead = false;
      try {
        let sent = false;
        for (const it of await env.nav.clipboard.read()) {
          if (it.types.indexOf("image/png") >= 0) {
            const b = await it.getType("image/png"); if (b.size <= MAX_IMAGE) { io.sendBinary(tagged(CH.image, new Uint8Array(await b.arrayBuffer()))); sent = true; }
          }
          if (it.types.indexOf("text/html") >= 0) {
            const b = await it.getType("text/html"); if (b.size <= MAX_HTML) { io.sendBinary(tagged(CH.html, new Uint8Array(await b.arrayBuffer()))); sent = true; }
          }
          if (it.types.indexOf("text/plain") >= 0) {
            const t = await (await it.getType("text/plain")).text(); if (t) { io.sendJson({ t: "paste", s: t }); sent = true; }
          }
        }
        changed(); return sent;
      } catch (e) { say("The browser did not let the clipboard be read."); return false; }
    }

    // ---- messages from the server
    function onJson(j) {
      if (j.t === "media_state") {
        for (const k of KINDS) { st.allowed[k] = !!(j.allowed && j.allowed[k]); }
        st.allowed.audio = !(j.allowed && j.allowed.audio === false);
        if (SINKS.indexOf(j.sink) >= 0) st.sink = j.sink;
        // the server is the authority on what is on (a limit or a reconnect switches things off)
        // (a state message that crossed our own "on" on the way is ignored for a moment)
        const settled = (k) => env.now() - sentOn[k] > 1500;
        if (st.on.mic && !j.mic && settled("mic")) stopMic();
        if (st.on.camera && !j.camera && settled("camera")) stopCamera();
        if (settled("clipread")) st.on.clipread = st.on.clipread && !!j.clipread;
        if (st.sound && !st.allowed.audio) stopSound();
        changed(); return true;
      }
      if (j.t === "media_ask") {                         // a tile (VOICE, CAMERA) wants it: the person's click on the page's button allows it
        if (KINDS.indexOf(j.kind) >= 0 && !st.on[j.kind]) { st.ask = j.kind; changed(); }
        return true;
      }
      if (j.t === "media_denied") { say(j.why || "Not allowed."); return true; }
      if (j.t === "media_limit") { say("The microphone stopped after five minutes."); stopMic(); return true; }
      return false;
    }
    // A binary message of the server that is not a picture: sound or a clipboard item. True when it was one of those.
    function onBinary(buf) {
      const u = new Uint8Array(buf), t = u[0];
      if (t === OUT.audio) { const a = parseAudio(u); if (a && st.sound) { player.seen(a.seq); player.push(a); } return true; }
      if (t === OUT.image && u.length > 8 && u.length - 1 <= MAX_IMAGE) { copyIn({ kind: "image", bytes: u.slice(1) }); return true; }
      if (t === OUT.html && u.length > 1 && u.length - 1 <= MAX_HTML) { copyIn({ kind: "html", bytes: u.slice(1) }); return true; }
      return false;
    }
    function disconnected() { stopMic(); stopCamera(); st.on.clipread = false; st.ask = ""; st.sound = false; player.stop(); changed(); }

    return { state: st, player, toggle, startSound, stopSound, setSink, onPaste, onReadRequest, readClipboard, finishCopy, onJson, onBinary, disconnected, copyIn };
  }

  const api = { create, tagged, toPcm16k, parseAudio, makePlayer, secureOk, explain, SINKS, CH, OUT, MIC_LIMIT_S };
  root.IyakkiMedia = api;
  if (typeof module !== "undefined" && module.exports) module.exports = api;
})(typeof self !== "undefined" ? self : globalThis);
