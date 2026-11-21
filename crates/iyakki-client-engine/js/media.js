// The media half of the IYAKKI client page (EXPERIMENTAL): the machine's sound played here, the text clipboard on a button press, a camera of
// the machine shown here, and this device's own camera and microphone offered to the machine. Hand-written, no libraries.
//
// Rules the page keeps (docs/development/CLIENT-SPLIT-DESIGN.md 7b): nothing starts without a click; the browser asks its own permission for the
// camera, the microphone and the clipboard; the machine asks too (a card on its screen) before it accepts an offer; whatever leaves this
// device is shown in a red band; everything stops, and the browser's camera and microphone lights go off, when the connection ends or the person
// presses the button again. The microphone is requested with the browser's echo cancellation on, so a browser in the same room as the machine
// does not feed its speakers back.

import { lipDelayMs } from "./audio-worklet.js";

const WORKLET = "/connect/audio-worklet.js";
const MAX_CLIP = 64 * 1024;
const WORDS = { in_use: "In use: ", not_granted: "Not allowed for this device: ", no_card: "Nobody at the machine to ask: ", card_declined: "The machine said no: ", consent_required: "Press the button again: ", too_big: "Too big: ", unsupported: "This machine does not do that: " };
const TAG_AUDIO = 2, TAG_CAMERA = 3, TAG_MIC_IN = 4, TAG_CAMERA_IN = 5;

export function createMedia({ engine, pump, send, toast, doc = document }) {
  const $ = (id) => doc.getElementById(id);
  const st = { caps: [], ctx: null, node: null, playing: false, side: "client", offers: { camera: null, microphone: null }, view: null };
  const has = (c) => st.caps.includes(c);

  function refresh() {
    $("sound-row").hidden = !has("audio.out");
    $("clip-row").hidden = !has("clipboard.text");
    $("cam-row").hidden = !has("camera.frames");
    $("offer-row").hidden = !(has("client.camera") || has("client.microphone"));
    $("offer-camera").hidden = !has("client.camera");
    $("offer-mic").hidden = !has("client.microphone");
    $("devices").hidden = !st.caps.some((c) => ["audio.out", "clipboard.text", "camera.frames", "client.camera", "client.microphone", "files.transfer"].includes(c));
    $("sound-btn").textContent = st.playing ? "Stop the sound here" : "Hear the machine's sound here";
    $("offer-camera").textContent = st.offers.camera ? "Stop sharing this device's camera" : "Share this device's camera with the machine";
    $("offer-mic").textContent = st.offers.microphone ? "Stop sharing this device's microphone" : "Share this device's microphone with the machine";
    const live = ["camera", "microphone"].filter((k) => st.offers[k] && st.offers[k].live);
    $("shared").hidden = live.length === 0;
    $("shared").textContent = live.length ? "Sharing this device's " + live.join(" and ") + " with the machine" : "";
  }

  // ---------------------------------------------------------------------------------------------------------------- sound

  async function startSound() {
    if (st.ctx) return;
    const AC = window.AudioContext || window.webkitAudioContext;
    if (!AC || !window.AudioWorkletNode) { toast("This browser cannot play the machine's sound (no audio worklet)."); return; }
    try {
      // created inside the click, as browsers require
      const ctx = new AC({ sampleRate: 48000, latencyHint: "interactive" });
      await ctx.resume();
      await ctx.audioWorklet.addModule(WORKLET);
      const node = new AudioWorkletNode(ctx, "iyakki-out", { numberOfInputs: 0, outputChannelCount: [2] });
      node.connect(ctx.destination);
      node.port.onmessage = (e) => {
        if (e.data && e.data.t === "stats") {
          st.stats = e.data;
          if (st.playing) $("sound-state").textContent = (st.side === "both" ? "Playing here and on the machine" : "Playing here (the machine is quiet)") + ". Queued " + e.data.queued + " ms, " + e.data.lost + " lost, " + e.data.underruns + " gaps.";
        }
      };
      st.ctx = ctx; st.node = node;
    } catch { toast("The browser would not start the sound."); stopSound(false); return; }
    st.side = $("sound-side").value === "both" ? "both" : "client";
    if (!engine.cmd({ c: "audio_open", side: st.side })) { toast("Not connected."); stopSound(false); return; }
    pump();
  }

  function stopSound(tell = true) {
    if (tell) { engine.cmd({ c: "audio_close" }); pump(); }
    try { st.node && st.node.disconnect(); } catch { /* gone */ }
    try { st.ctx && st.ctx.close(); } catch { /* gone */ }
    st.ctx = null; st.node = null; st.playing = false;
    $("sound-state").textContent = ""; refresh();
  }

  function onAudio(u8) {
    if (!st.node || u8.length < 5 + 4) return;
    const dv = new DataView(u8.buffer, u8.byteOffset + 1, u8.length - 1), seq = dv.getUint32(0, true), n = (u8.length - 5) >> 1;
    if (n === 0 || n % 2) return;
    const f = new Float32Array(n);
    for (let i = 0; i < n; i++) f[i] = dv.getInt16(4 + 2 * i, true) / 32768;
    st.node.port.postMessage({ t: "pcm", seq, data: f }, [f.buffer]);
  }

  // ---------------------------------------------------------------------------------------------------------------- the machine's camera

  async function onCameraFrame(u8) {
    if (u8.length < 1 + 1 + 4) return;
    const n = u8[1];
    if (u8.length < 2 + n + 4) return;
    const jpeg = u8.subarray(2 + n + 4);
    try {
      const bmp = await createImageBitmap(new Blob([jpeg], { type: "image/jpeg" }));
      const cv = $("camview");
      if (cv.width !== bmp.width || cv.height !== bmp.height) { cv.width = bmp.width; cv.height = bmp.height; }
      cv.getContext("2d").drawImage(bmp, 0, 0); cv.hidden = false; bmp.close && bmp.close();
    } catch { /* a bad frame is skipped */ }
  }

  // The cameras this person may watch, by name (the machine sends names and ids, never device paths). With a list the picker is shown;
  // without one the typed id stays.
  function showCameras(list) {
    const sel = $("cam-pick");
    sel.textContent = "";
    for (const c of list) {
      const o = doc.createElement("option");
      o.value = String(c.unit || ""); o.textContent = String(c.name || c.unit || "");
      sel.appendChild(o);
    }
    sel.hidden = list.length === 0;
    $("cam-unit").hidden = list.length > 0;
    $("cam-state").textContent = list.length === 0 ? "No camera of the machine is available to you." : "";
  }

  // ---------------------------------------------------------------------------------------------------------------- clipboard

  async function sendClipboard() {
    let t = "";
    try { t = await navigator.clipboard.readText(); } catch { toast("The browser did not let this page read your clipboard."); return; }
    if (!t) return toast("Your clipboard has no text.");
    if (new TextEncoder().encode(t).length > MAX_CLIP) return toast("That text is longer than 64 KB, which is the limit.");
    // the press of this button is the consent for this one use
    if (!engine.cmd({ c: "clip_put", text: t, confirmed: true })) toast("Not connected.");
    pump();
  }

  async function gotClipboard(text) {
    try { await navigator.clipboard.writeText(text); toast("The machine's clipboard is on this device's clipboard."); return; } catch { /* shown below instead */ }
    $("clip-text").value = text; $("clip-box").hidden = false;
  }

  // ---------------------------------------------------------------------------------------------------------------- this device's own camera and microphone

  async function offer(kind) {
    if (st.offers[kind]) return stopOffer(kind, true);
    if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) return toast("This browser has no access to a " + kind + " here (it needs https or localhost).");
    let stream;
    try {
      // the browser's own permission prompt
      stream = await navigator.mediaDevices.getUserMedia(kind === "camera"
        ? { video: { width: { ideal: 320 }, height: { ideal: 240 }, frameRate: { ideal: 10 } } }
        : { audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true, channelCount: 1 } });
    } catch { return toast("The browser did not give access to the " + kind + "."); }
    st.offers[kind] = { stream, live: false };
    if (!engine.cmd({ c: "offer_open", kind })) { stopOffer(kind, false); return toast("Not connected."); }
    pump(); refresh();
  }

  function stopOffer(kind, tell) {
    const o = st.offers[kind];
    if (!o) return;
    if (tell) { engine.cmd({ c: "offer_close", kind }); pump(); }
    clearInterval(o.timer);
    try { o.node && o.node.port.postMessage({ t: "stop" }); o.node && o.node.disconnect(); } catch { /* gone */ }
    try { o.ctx && o.ctx.close(); } catch { /* gone */ }
    for (const t of o.stream.getTracks()) t.stop(); // the browser's light goes off
    st.offers[kind] = null; refresh();
  }

  async function streamOffer(kind) {
    const o = st.offers[kind];
    if (!o) return;
    o.live = true; refresh();
    if (kind === "camera") {
      const v = doc.createElement("video");
      v.muted = true; v.playsInline = true; v.srcObject = o.stream;
      try { await v.play(); } catch { /* frames are taken when it plays */ }
      const cv = doc.createElement("canvas"); cv.width = 320; cv.height = 240;
      o.timer = setInterval(() => {
        if (!v.videoWidth || send.buffered() > 200000) return;
        cv.height = Math.round(320 * v.videoHeight / v.videoWidth) || 240;
        cv.getContext("2d").drawImage(v, 0, 0, cv.width, cv.height);
        cv.toBlob(async (b) => { if (b && b.size < 200000 && st.offers.camera === o) send.binary(TAG_CAMERA_IN, new Uint8Array(await b.arrayBuffer())); }, "image/jpeg", 0.6);
      }, 200); // 5 frames a second
    } else {
      try {
        const AC = window.AudioContext || window.webkitAudioContext;
        const ctx = new AC();
        await ctx.resume();
        await ctx.audioWorklet.addModule(WORKLET);
        const src = ctx.createMediaStreamSource(o.stream), node = new AudioWorkletNode(ctx, "iyakki-mic", { numberOfOutputs: 1, outputChannelCount: [1] });
        const mute = ctx.createGain(); mute.gain.value = 0; // nothing of the microphone is played here
        src.connect(node); node.connect(mute); mute.connect(ctx.destination);
        node.port.onmessage = (e) => { if (e.data && e.data.t === "mic" && st.offers.microphone === o) send.binary(TAG_MIC_IN, new Uint8Array(e.data.data.buffer)); };
        o.ctx = ctx; o.node = node;
        if (st.playing && st.side === "both") toast("The machine's sound is also playing on the machine. If it is in this room, your microphone will hear it: choose \"here only\".", 8000);
      } catch { stopOffer("microphone", true); toast("The browser would not start the microphone."); }
    }
  }

  // ---------------------------------------------------------------------------------------------------------------- what the page calls

  function onMedia(o) {
    const b = o.body || {};
    switch (o.ev) {
      case "audio_ok": st.playing = true; $("sound-state").textContent = b.side === "both" ? "Playing here and on the machine" : "Playing here (the machine is quiet)"; refresh(); break;
      case "audio_closed": if (st.ctx) stopSound(false); break;
      case "audio_flush": st.node && st.node.port.postMessage({ t: "flush" }); break;
      case "clipboard_ok": toast("Sent to the machine."); break;
      case "clipboard_text": gotClipboard(b.text || ""); break;
      case "camera_list": showCameras(b.cameras || []); break;
      case "camera_ok": $("cam-state").textContent = "Showing " + b.unit; break;
      case "camera_closed": $("cam-state").textContent = b.reason === "closed" ? "" : "Stopped: " + String(b.reason).replace("_", " "); st.view = null; $("cam-btn").textContent = "Watch"; break;
      case "offer_ok": toast("The machine accepted " + b.source + "."); streamOffer(b.kind); break;
      case "offer_closed": stopOffer(b.kind, false); break;
      case "media_err":
        toast((WORDS[b.code] || b.what + ": ") + (b.message || b.code || ""), 7000);
        if (b.what === "audio" && st.ctx && !st.playing) stopSound(false);
        if (b.what === "offer") for (const k of ["camera", "microphone"]) if (st.offers[k] && !st.offers[k].live) stopOffer(k, false);
        break;
      default: break;
    }
  }

  function onBinary(buf) {
    const u8 = new Uint8Array(buf);
    if (u8[0] === TAG_AUDIO) { onAudio(u8); return true; }
    if (u8[0] === TAG_CAMERA) { onCameraFrame(u8); return true; }
    return false;
  }

  // The connection ended (or the person disconnected): nothing keeps running, the lights go off.
  function reset() {
    if (st.ctx) stopSound(false);
    for (const k of ["camera", "microphone"]) stopOffer(k, false);
    $("cam-state").textContent = ""; st.view = null; $("cam-btn").textContent = "Watch"; $("clip-box").hidden = true;
  }

  $("devices").addEventListener("click", () => {
    $("panel").hidden = !$("panel").hidden;
    if (!$("panel").hidden && has("camera.frames")) { engine.cmd({ c: "camera_list" }); pump(); }
  });
  $("panel-close").addEventListener("click", () => { $("panel").hidden = true; });
  $("sound-btn").addEventListener("click", () => { st.playing || st.ctx ? stopSound(true) : startSound(); });
  $("sound-side").addEventListener("change", () => { if (st.playing) { st.side = $("sound-side").value === "both" ? "both" : "client"; engine.cmd({ c: "audio_open", side: st.side }); pump(); } });
  $("clip-send").addEventListener("click", sendClipboard);
  $("clip-get").addEventListener("click", () => { engine.cmd({ c: "clip_get", confirmed: true }); pump(); });
  $("clip-copy").addEventListener("click", async () => { try { await navigator.clipboard.writeText($("clip-text").value); toast("Copied."); } catch { $("clip-text").select(); } });
  $("cam-btn").addEventListener("click", () => {
    if (st.view) { engine.cmd({ c: "camera_close", unit: st.view }); st.view = null; $("cam-btn").textContent = "Watch"; $("camview").hidden = true; pump(); return; }
    const unit = ($("cam-pick").hidden ? $("cam-unit").value : $("cam-pick").value).trim();
    if (!unit) return;
    if (engine.cmd({ c: "camera_open", unit })) { st.view = unit; $("cam-btn").textContent = "Stop"; }
    pump();
  });
  $("offer-camera").addEventListener("click", () => offer("camera"));
  $("offer-mic").addEventListener("click", () => offer("microphone"));

  // how long the page should hold the picture back so that it lines up with the sound it plays (0 without sound)
  function lipDelay() {
    const c = st.ctx;
    const lat = c ? ((c.outputLatency || 0) + (c.baseLatency || 0)) * 1000 : 0;
    return lipDelayMs(!!(st.playing && st.node), st.stats ? st.stats.queued : 0, lat);
  }

  return { lipDelay, setCaps(caps) { st.caps = caps || []; refresh(); }, onMedia, onBinary, reset, state: st };
}
