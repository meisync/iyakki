//! The one page the server serves: a sign-in form and the canvas the screen is drawn on.

/// The decoder (also served at `/decoder.js`; used on the main thread or in a worker).
pub const DECODER_JS: &str = include_str!("decoder.js");

/// Sent with the page: it (and only it) may ask for the camera, the microphone and the clipboard, and play sound.
pub const PERMISSIONS_POLICY: &str = "camera=(self), microphone=(self), autoplay=(self), clipboard-read=(self), clipboard-write=(self), fullscreen=(self)";
/// The browser devices bridge (clipboard images and HTML, sound, microphone, camera); served at `/media.js`.
pub const MEDIA_JS: &str = include_str!("media.js");

pub const INDEX: &str = concat!(r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>IYAKKI</title>
<style>
  :root { color-scheme: dark; }
  html, body { margin: 0; height: 100%; background: #090b14; color: #e6e9f5; font-family: system-ui, "Segoe UI", Roboto, sans-serif; overflow: hidden; }
  #login { position: fixed; inset: 0; display: flex; align-items: center; justify-content: center; background: linear-gradient(#141930, #090b14); }
  #login form { width: 320px; max-width: calc(100vw - 32px); background: #1e253e; border-radius: 14px; padding: 28px; box-shadow: 0 10px 40px #0008; }
  #login h1 { margin: 0 0 4px; font-size: 26px; letter-spacing: 2px; }
  #login p { margin: 0 0 18px; color: #9aa3c4; font-size: 14px; }
  #login input { width: 100%; box-sizing: border-box; margin: 0 0 12px; padding: 11px 12px; border-radius: 8px; border: 1px solid #3a4470; background: #121730; color: inherit; font-size: 16px; }
  #login input:focus { outline: 2px solid #6c8cff; border-color: transparent; }
  #login button { width: 100%; padding: 12px; border: 0; border-radius: 8px; background: #5b7cff; color: #fff; font-size: 16px; cursor: pointer; }
  #login button:disabled { opacity: .6; cursor: wait; }
  #err { min-height: 20px; margin-top: 10px; color: #ff8a8a; font-size: 14px; }
  #screen { display: block; outline: none; cursor: default; touch-action: none; transform-origin: 0 0; user-select: none; -webkit-user-select: none; }
  #status { position: fixed; top: 8px; right: 12px; padding: 4px 10px; border-radius: 6px; background: #000a; font-size: 13px; display: none; }
  #gear { position: fixed; right: 0; top: 50%; width: 26px; height: 46px; margin-top: -23px; border: 0; border-radius: 8px 0 0 8px; background: #1e253e; color: #e6e9f5; font-size: 17px; opacity: .3; cursor: pointer; padding: 0; }
  #gear:hover, #gear:focus-visible, #gear.on { opacity: .95; }
  #panel { position: fixed; right: 32px; top: 50%; transform: translateY(-50%); width: 250px; max-width: calc(100vw - 48px); box-sizing: border-box; padding: 14px; border-radius: 12px; background: #1e253eee; box-shadow: 0 8px 30px #000a; font-size: 14px; }
  #panel h2 { margin: 0 0 10px; font-size: 15px; }
  #panel label { display: block; margin: 8px 0; }
  #panel select, #panel button { font: inherit; color: inherit; background: #121730; border: 1px solid #3a4470; border-radius: 6px; padding: 6px 8px; }
  #panel select { width: 100%; margin-top: 4px; }
  #panel .row { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 10px; }
  #panel small { display: block; margin-top: 8px; color: #9aa3c4; }
  #stats { position: fixed; left: 8px; top: 8px; margin: 0; padding: 6px 9px; border-radius: 6px; background: #000b; color: #b9f3c3; font: 12px/1.35 ui-monospace, Menlo, Consolas, monospace; pointer-events: none; white-space: pre; }
  #ime { position: fixed; left: 0; bottom: 0; width: 1px; height: 1px; opacity: 0; border: 0; padding: 0; resize: none; }
  #panel h3 { margin: 14px 0 4px; font-size: 14px; }
  #panel .dev { display: flex; align-items: center; gap: 6px; margin: 6px 0; }
  #panel .dev button { flex: 1; text-align: left; }
  #panel .dev button.on { border-color: #ff6b6b; color: #ffb4b4; }
  #panel input[type=range] { width: 100%; }
  #rec { position: fixed; left: 50%; top: 8px; transform: translateX(-50%); display: flex; gap: 8px; z-index: 2147483647; }
  #rec button { font: 13px system-ui, sans-serif; color: #fff; background: #b3202a; border: 0; border-radius: 14px; padding: 5px 12px; cursor: pointer; box-shadow: 0 2px 10px #0008; }
  #rec button::before { content: "\25CF  "; animation: blink 1.2s infinite; }
  @keyframes blink { 50% { opacity: .3; } }
  #askdev { position: fixed; left: 50%; top: 44px; transform: translateX(-50%); font: 14px system-ui, sans-serif; color: #fff; background: #b3202a; border: 2px solid #fff; border-radius: 16px; padding: 8px 18px; cursor: pointer; z-index: 2147483646; }
  #askclip { position: fixed; left: 50%; bottom: 14px; transform: translateX(-50%); font: 14px system-ui, sans-serif; color: #fff; background: #3b55c9; border: 0; border-radius: 16px; padding: 8px 16px; cursor: pointer; z-index: 5; }
  #deverr { position: fixed; left: 50%; top: 84px; transform: translateX(-50%); max-width: min(560px, calc(100vw - 32px)); font: 14px/1.4 system-ui, sans-serif; color: #fff; background: #7a1f27; border: 2px solid #fff; border-radius: 12px; padding: 10px 16px; cursor: pointer; z-index: 2147483646; box-shadow: 0 4px 18px #000a; }
  [hidden] { display: none !important; }
</style>
</head>
<body>
<div id="login">
  <form id="form" autocomplete="on">
    <h1>IYAKKI</h1>
    <p>An OS for Agents. Sign in with your account on this machine.</p>
    <input id="user" name="username" autocomplete="username" placeholder="Name" autocapitalize="none" spellcheck="false" required>
    <input id="pass" name="password" type="password" autocomplete="current-password" placeholder="Password" required>
    <button id="go" type="submit">Sign in</button>
    <div id="err" role="alert"></div>
  </form>
</div>
<canvas id="screen" tabindex="0" hidden></canvas>
<div id="status"></div>
<button id="gear" type="button" aria-label="Display settings" title="Display settings (Ctrl+Alt+Q)" hidden>&#9881;</button>
<div id="panel" hidden role="dialog" aria-label="Display settings">
  <h2>Display</h2>
  <label>Picture quality
    <select id="q">
      <option value="auto">Automatic</option>
      <option value="lossless">Lossless</option>
      <option value="balanced">Balanced</option>
      <option value="low">Low bandwidth</option>
      <option value="verylow">Very low</option>
    </select>
  </label>
  <label><input type="checkbox" id="saver"> Data saver</label>
  <label><input type="checkbox" id="showstats"> Statistics (Ctrl+Alt+S)</label>
  <div class="row"><button id="fs" type="button">Full screen</button><button id="kbd" type="button">Keyboard</button><button id="rz" type="button">Reset zoom</button></div>
  <small id="lvl"></small>
  <h3>Browser devices</h3>
  <div class="dev"><button id="dsound" type="button">Start sound</button></div>
  <label id="dsinkrow">Play sound on
    <select id="dsink">
      <option value="browser">This computer (the browser)</option>
      <option value="both">Both</option>
      <option value="mac">The Mac only</option>
    </select>
  </label>
  <label id="dvolrow" hidden>Volume <input type="range" id="dvol" min="0" max="100" value="80"></label>
  <div class="dev"><button id="dmic" type="button">Microphone: off</button></div>
  <div class="dev"><button id="dcam" type="button">Camera: off</button></div>
  <div class="dev"><button id="dclip" type="button">Clipboard reading: off</button></div>
  <small id="dnote">Each one is off until you switch it on, and you can stop it with one tap.</small>
</div>
<div id="rec" hidden role="status"></div>
<div id="deverr" hidden role="alert" title="Click to dismiss"></div>
<button id="askdev" type="button" hidden></button>
<button id="askclip" type="button" hidden></button>
<pre id="stats" hidden></pre>
<textarea id="ime" autocapitalize="none" autocomplete="off" autocorrect="off" spellcheck="false" aria-hidden="true" tabindex="-1"></textarea>
<script src="/decoder.js"></script>
<script src="/media.js"></script>
<script>
"use strict";
const $ = (id) => document.getElementById(id);
const cv = $("screen");
let csrf = null, ws = null, closing = false, retry = null, chain = Promise.resolve();

function showLogin(msg) {
  closing = true;
  if (ws) { try { ws.close(); } catch (e) {} ws = null; }
  media.disconnected();
  cv.hidden = true; $("status").style.display = "none"; $("gear").hidden = true; $("panel").hidden = true; $("stats").hidden = true;
  $("login").hidden = false; $("err").textContent = msg || "";
  $("go").disabled = false; $("pass").value = ""; (($("user").value ? $("pass") : $("user"))).focus();
}
function status(t) { const s = $("status"); s.textContent = t || ""; s.style.display = t ? "block" : "none"; }

// ---- browser devices (clipboard images and HTML, sound, microphone, camera): media.js, the rules are in that file
function sendBinary(u8) { if (ws && ws.readyState === 1) ws.send(u8); }
const media = IyakkiMedia.create(
  { nav: navigator, document, AudioContext: window.AudioContext || window.webkitAudioContext, ClipboardItem: window.ClipboardItem, Blob, TextEncoder,
    isSecureContext: window.isSecureContext, now: () => Date.now(), setInterval: (f, ms) => setInterval(f, ms) },
  { sendJson: (o) => send(o), sendBinary, onChange: (s) => renderDevices(s) });
function mmss(n) { return Math.floor(n / 60) + ":" + String(n % 60).padStart(2, "0"); }
function renderDevices(s) {
  const dev = (id, label, on, allowed, extra) => {
    const b = $(id); b.classList.toggle("on", on);
    b.textContent = label + (on ? ": on" + (extra || "") + " (tap to stop)" : !s.secure.ok ? ": needs https" : !allowed ? ": not allowed" : ": off");
    b.disabled = !on && !allowed;     // (not for an insecure page: the click then says exactly why)
  };
  dev("dmic", "Microphone", s.on.mic, s.allowed.mic, " " + mmss(s.micSeconds) + " of 5:00");
  dev("dcam", "Camera", s.on.camera, s.allowed.camera);
  dev("dclip", "Clipboard reading", s.on.clipread, s.allowed.clipread);
  $("dsound").textContent = s.sound ? "Stop sound" : "Start sound"; $("dsound").classList.remove("on");
  $("dsound").disabled = !s.allowed.audio || !media.player.ctx && !(window.AudioContext || window.webkitAudioContext);
  $("dvolrow").hidden = !s.sound;
  $("dsink").value = s.sink;
  $("dsink").disabled = !s.allowed.audio;
  const de = $("deverr"); de.textContent = s.notice || ""; de.hidden = !s.notice;
  $("dnote").textContent = s.notice || (s.secure.ok ? "Each one is off until you switch it on, and you can stop it with one tap." : s.secure.why);
  const rec = $("rec"); rec.textContent = "";
  for (const [on, label, id] of [[s.on.mic, "Recording: microphone " + mmss(s.micSeconds), "dmic"], [s.on.camera, "Recording: camera", "dcam"]]) {
    if (!on) continue;
    const b = document.createElement("button"); b.type = "button"; b.textContent = label + " (tap to stop)"; b.onclick = (e) => media.toggle(id === "dmic" ? "mic" : "camera", e);
    rec.appendChild(b);
  }
  rec.hidden = !rec.firstChild;
  const ad = $("askdev");
  ad.hidden = !s.ask;
  ad.textContent = s.ask === "mic" ? "A program wants your microphone. Click to allow." : s.ask === "camera" ? "A program wants your camera. Click to allow." : "Click to allow.";
  const ask = $("askclip");
  ask.hidden = !(s.pendingRead || s.pendingCopy);
  ask.textContent = s.pendingRead ? "Click to paste from your clipboard" : "Click to copy to your clipboard";
}
$("dmic").onclick = (e) => media.toggle("mic", e);
$("dcam").onclick = (e) => media.toggle("camera", e);
$("dclip").onclick = (e) => media.toggle("clipread", e);
$("dsink").onchange = () => media.setSink($("dsink").value);
$("deverr").onclick = () => { media.state.notice = ""; renderDevices(media.state); };
$("dsound").onclick = (e) => (media.state.sound ? media.stopSound() : media.startSound(e));
$("dvol").oninput = () => media.player.setVolume($("dvol").value / 100);
$("askdev").onclick = (e) => { const k = media.state.ask; if (k) media.toggle(k, e); cv.focus(); };
$("askclip").onclick = (e) => { if (media.state.pendingRead) media.readClipboard(e); else media.finishCopy(e); $("askclip").hidden = true; cv.focus(); };
renderDevices(media.state);

async function login(ev) {
  ev.preventDefault(); spPrime();
  $("go").disabled = true; $("err").textContent = "";
  try {
    const r = await fetch("/api/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ user: $("user").value, password: $("pass").value }) });
    const j = await r.json();
    if (!r.ok) { $("err").textContent = j.error || "Could not sign in"; $("go").disabled = false; $("pass").value = ""; return; }
    start(j.csrf);
  } catch (e) { $("err").textContent = "Could not reach the server"; $("go").disabled = false; }
}

"##,
// the browser voice of the page (shared with the desktop window's voice helper page)
include_str!("../../iyakki-agent/src/speech.js"),
r##"function lsGet(k) { try { return localStorage.getItem(k); } catch (e) { return null; } }
function lsSet(k, v) { try { localStorage.setItem(k, v); } catch (e) {} }

// ---- window size, fitted to the screen (a phone shows the 640x400 minimum scaled down; pinch to zoom)
let W = 0, H = 0, transferred = false, zoom = 1, tx = 0, ty = 0;
function size() {
  W = Math.max(640, Math.min(3840, Math.floor(window.innerWidth))); H = Math.max(400, Math.min(2160, Math.floor(window.innerHeight)));
  if (transferred) { if (painter) painter.resize(W, H); }
  else if (cv.width !== W || cv.height !== H) { cv.width = W; cv.height = H; }
  fit();
  return [W, H];
}
function fit() {
  const f = Math.min(1, window.innerWidth / W, window.innerHeight / H);
  cv.style.width = Math.floor(W * f) + "px"; cv.style.height = Math.floor(H * f) + "px";
}
function applyZoom() {
  zoom = Math.max(1, Math.min(4, zoom));
  if (zoom === 1) { tx = 0; ty = 0; }
  const w = cv.offsetWidth * zoom, h = cv.offsetHeight * zoom;
  tx = Math.min(0, Math.max(window.innerWidth - w, tx)); ty = Math.min(0, Math.max(window.innerHeight - h, ty));
  cv.style.transform = zoom === 1 ? "" : "translate(" + tx + "px," + ty + "px) scale(" + zoom + ")";
}
function send(o) { if (ws && ws.readyState === 1) ws.send(JSON.stringify(o)); }

// ---- the picture: version 2 tiles (decoded on the main thread, or in a worker on an OffscreenCanvas), or PNG pieces from an old server
let painter = null, v2 = false, legacyCtx = null, rxBytes = 0, rxFrames = 0, decodeMs = 0, lastRtt = null, srv = null, pingTimer = null, statTimer = null;
const hasInflate = IyakkiDecoder.canInflate();
const canWorker = typeof Worker === "function" && typeof OffscreenCanvas === "function" && typeof cv.transferControlToOffscreen === "function" && hasInflate && lsGet("iyakki.noworker") !== "1";

function workerPainter(off) {
  const w = new Worker("/decoder.js"), waiting = new Map();
  let id = 0, ok = false;
  w.onmessage = (e) => {
    const m = e.data;
    if (m.t === "ready") { ok = true; return; }
    const cb = waiting.get(m.id);
    if (cb) { waiting.delete(m.id); m.err ? cb.rej(new Error(m.err)) : cb.res(m); }
  };
  // the canvas cannot be taken back from a worker that failed to start: reload once without workers
  w.onerror = () => { if (!ok) { lsSet("iyakki.noworker", "1"); location.reload(); } };
  w.postMessage({ t: "init", canvas: off }, [off]);
  return {
    worker: true,
    paint: (buf) => new Promise((res, rej) => { const i = ++id; waiting.set(i, { res, rej }); w.postMessage({ t: "frame", id: i, buf }, [buf]); }),
    resize: (wd, ht) => w.postMessage({ t: "size", w: wd, h: ht }),
  };
}

function startPainter() {
  if (painter) return;
  if (canWorker && !legacyCtx && !transferred) {
    const off = cv.transferControlToOffscreen();
    transferred = true;
    painter = workerPainter(off);
    painter.resize(W, H);
  } else {
    painter = new IyakkiDecoder.Painter(cv, (w, h) => { const c = document.createElement("canvas"); c.width = w; c.height = h; return c; });
    painter.worker = false;
  }
}

function drawLegacy(m) {
  if (!legacyCtx) legacyCtx = cv.getContext("2d", { alpha: false });
  const dv = new DataView(m), x = dv.getUint16(0, true), y = dv.getUint16(2, true);
  const blob = new Blob([m.slice(8)], { type: "image/png" });
  chain = chain.then(() => createImageBitmap(blob)).then((b) => { legacyCtx.drawImage(b, x, y); b.close && b.close(); }).catch(() => {});
}

function onFrame(m) {
  const t0 = performance.now();
  rxFrames++;
  chain = chain.then(() => painter.paint(m)).then(
    (r) => { decodeMs = decodeMs * 0.8 + (performance.now() - t0) * 0.2; send({ t: "ack", n: r.seq }); },
    () => { send({ t: "full" }); });
}

// ---- display settings and statistics
const qsel = $("q"), saver = $("saver"), showStats = $("showstats");
const LEVELS = { lossless: "Lossless", balanced: "Balanced", low: "Low bandwidth", verylow: "Very low" };
function sendQuality() { lsSet("iyakki.q", qsel.value); lsSet("iyakki.saver", saver.checked ? "1" : "0"); send({ t: "quality", level: qsel.value, saver: saver.checked }); }
qsel.value = lsGet("iyakki.q") || "auto"; if (!qsel.value) qsel.value = "auto";
saver.checked = lsGet("iyakki.saver") === "1"; showStats.checked = lsGet("iyakki.stats") === "1";
qsel.onchange = saver.onchange = sendQuality;
showStats.onchange = () => { lsSet("iyakki.stats", showStats.checked ? "1" : "0"); renderStats(); };
$("gear").onclick = () => { const p = $("panel"); p.hidden = !p.hidden; $("gear").classList.toggle("on", !p.hidden); cv.focus(); };
$("rz").onclick = () => { zoom = 1; applyZoom(); };
const fsOk = document.documentElement.requestFullscreen || document.documentElement.webkitRequestFullscreen;
if (!fsOk) $("fs").hidden = true;
$("fs").onclick = () => {
  if (document.fullscreenElement || document.webkitFullscreenElement) (document.exitFullscreen || document.webkitExitFullscreen).call(document);
  else (document.documentElement.requestFullscreen || document.documentElement.webkitRequestFullscreen).call(document.documentElement);
};
$("kbd").onclick = () => { $("ime").focus(); };

let rxPrev = 0, fpsPrev = 0, rxRate = 0, fpsRate = 0;
function renderStats() {
  const el = $("stats");
  el.hidden = !showStats.checked || cv.hidden;
  if (el.hidden) return;
  const kb = (n) => (n >= 1e6 ? (n / 1e6).toFixed(2) + " MB/s" : (n / 1e3).toFixed(1) + " kB/s");
  el.textContent = "level   " + (srv ? (LEVELS[srv.level] || srv.level) + (srv.auto ? " (auto)" : "") + (srv.saver ? ", saver" : "") : v2 ? "-" : "legacy PNG") +
    "\nreceive " + kb(rxRate) + "\nframes  " + fpsRate.toFixed(0) + " /s\nround trip " + (lastRtt === null ? "-" : lastRtt.toFixed(0) + " ms") +
    "\ndecode  " + decodeMs.toFixed(1) + " ms" + (painter ? (painter.worker ? " (worker)" : " (page)") : "") + (srv && srv.bps ? "\nlink    " + kb(srv.bps) : "");
}
function tickStats() {
  rxRate = rxBytes - rxPrev; fpsRate = rxFrames - fpsPrev; rxPrev = rxBytes; fpsPrev = rxFrames;
  renderStats();
  if (srv) $("lvl").textContent = "Now: " + (LEVELS[srv.level] || srv.level) + (srv.auto ? " (chosen automatically)" : "");
}

function start(token) {
  csrf = token; closing = false;
  $("login").hidden = true; cv.hidden = false; $("gear").hidden = false; size(); cv.focus();
  clearInterval(statTimer); statTimer = setInterval(tickStats, 1000);
  connect();
}

function connect() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  ws = new WebSocket(proto + "://" + location.host + "/ws?csrf=" + encodeURIComponent(csrf));
  ws.binaryType = "arraybuffer";
  v2 = false; srv = null;
  ws.onopen = () => {
    status(""); const [w, h] = size(); send({ t: "size", w, h }); spVoices();
    send({ t: "hello", v: 2, caps: { deflate: hasInflate } });
    clearInterval(pingTimer);
    pingTimer = setInterval(() => { const o = { t: "ping", ts: Math.round(performance.now()) }; if (lastRtt !== null) o.rtt = lastRtt; send(o); }, 2000);
  };
  ws.onmessage = (m) => {
    if (typeof m.data === "string") {
      rxBytes += m.data.length;
      const j = JSON.parse(m.data);
      if (j.t === "copy" && navigator.clipboard) navigator.clipboard.writeText(j.s).catch(() => {});
      if (typeof j.t === "string" && j.t.indexOf("media_") === 0) media.onJson(j);
      if (j.t === "cursor") cv.style.cursor = j.c;
      if (j.t === "readclip") media.onReadRequest();
      if (j.t === "bye") showLogin("Signed out.");
      if (j.t === "hello_ok" && j.v >= 2) { v2 = true; startPainter(); size(); sendQuality(); }
      if (j.t === "stats") { srv = j; renderStats(); }
      if (j.t === "pong") lastRtt = Math.max(0, performance.now() - j.ts);
      if (typeof j.t === "string" && j.t.indexOf("speak") === 0) spMessage(j);
      return;
    }
    rxBytes += m.data.byteLength;
    // sound and clipboard items share the connection with the picture; the picture's messages start with 2, theirs with 0x10 or more
    if (v2 && new Uint8Array(m.data, 0, 1)[0] >= 0x10) { media.onBinary(m.data); return; }
    if (v2) onFrame(m.data); else drawLegacy(m.data);
  };
  ws.onclose = async () => {
    if (closing) return;
    status("Reconnecting…"); media.disconnected();
    clearInterval(pingTimer);
    clearTimeout(retry);
    retry = setTimeout(async () => {
      const r = await fetch("/api/session").catch(() => null);
      if (r && r.status === 401) return showLogin("Your session ended. Sign in again.");
      if (r && r.ok) { csrf = (await r.json()).csrf; connect(); } else { ws = null; retry = setTimeout(() => ws || connect(), 1500); }
    }, 1000);
  };
}

// ---- mouse (touch is below). The pointer's shape is the browser's own, so it moves without waiting for the server.
const pos = (e) => { const r = cv.getBoundingClientRect(); return { x: Math.round((e.clientX - r.left) * cv.width / r.width), y: Math.round((e.clientY - r.top) * cv.height / r.height) }; };
let pending = null, lastTouch = 0;
const fromTouch = () => performance.now() - lastTouch < 800;
cv.addEventListener("mousemove", (e) => { if (fromTouch()) return; if (!pending) requestAnimationFrame(() => { send({ t: "move", ...pending }); pending = null; }); pending = pos(e); });
cv.addEventListener("mousedown", (e) => { if (fromTouch()) return; spPrime(); cv.focus(); e.preventDefault(); send({ t: "shift", on: e.shiftKey }); send({ t: e.button === 2 ? "right" : "down", ...pos(e) }); });
window.addEventListener("mouseup", (e) => { if (fromTouch()) return; if (e.button === 0 && !cv.hidden) send({ t: "up", ...pos(e) }); });
cv.addEventListener("contextmenu", (e) => e.preventDefault());
cv.addEventListener("wheel", (e) => { e.preventDefault(); const d = -Math.sign(e.deltaY) * Math.max(1, Math.min(10, Math.round(Math.abs(e.deltaY) / 40))); send({ t: "wheel", d }); }, { passive: false });
cv.addEventListener("keydown", (e) => {
  spPrime();
  const ctrl = e.ctrlKey || e.metaKey;
  if (e.ctrlKey && e.altKey && (e.key === "s" || e.key === "S")) { e.preventDefault(); showStats.checked = !showStats.checked; showStats.onchange(); return; }
  if (e.ctrlKey && e.altKey && (e.key === "q" || e.key === "Q")) { e.preventDefault(); $("gear").click(); return; }
  if (ctrl && !e.shiftKey && (e.key === "v" || e.key === "V")) return; // the paste event carries the text
  if (e.key === "Control" || e.key === "Shift" || e.key === "Alt" || e.key === "Meta") return;
  e.preventDefault();
  send({ t: "key", key: e.key, code: e.code, ctrl, alt: e.altKey, shift: e.shiftKey });
});
document.addEventListener("paste", (e) => { if (cv.hidden || !e.clipboardData) return; e.preventDefault(); media.onPaste(e); });

// ---- touch: tap = click, long press = right click, drag = drag, two fingers = scroll (pinch = zoom, and pan while zoomed)
const fingers = new Map();
let g = null;
const mid = () => { const a = [...fingers.values()]; return { x: (a[0].x + a[1].x) / 2, y: (a[0].y + a[1].y) / 2, d: Math.hypot(a[0].x - a[1].x, a[0].y - a[1].y) }; };
cv.addEventListener("pointerdown", (e) => {
  if (e.pointerType === "mouse") return;
  e.preventDefault(); lastTouch = performance.now(); spPrime(); cv.setPointerCapture(e.pointerId);
  fingers.set(e.pointerId, { x0: e.clientX, y0: e.clientY, x: e.clientX, y: e.clientY, t0: performance.now() });
  if (fingers.size === 1) {
    const f = fingers.get(e.pointerId);
    g = { mode: "wait", lp: setTimeout(() => { if (g && g.mode === "wait") { g.mode = "long"; send({ t: "move", ...pos(f) }); send({ t: "right", ...pos(f) }); } }, 550) };
  } else if (g && fingers.size === 2) {
    clearTimeout(g.lp);
    if (g.mode === "drag") send({ t: "up", ...pos([...fingers.values()][0]) });
    const m = mid();
    g = { mode: "two", sub: null, d0: m.d, z0: zoom, ly: m.y, lx: m.x, y0: m.y, acc: 0 };
  }
});
cv.addEventListener("pointermove", (e) => {
  if (e.pointerType === "mouse") return;
  const f = fingers.get(e.pointerId); if (!f || !g) return;
  e.preventDefault(); lastTouch = performance.now();
  f.x = e.clientX; f.y = e.clientY;
  if (g.mode === "wait" && fingers.size === 1 && Math.hypot(f.x - f.x0, f.y - f.y0) > 10) {
    clearTimeout(g.lp); g.mode = "drag";
    send({ t: "shift", on: false }); send({ t: "move", ...pos({ clientX: f.x0, clientY: f.y0 }) }); send({ t: "down", ...pos({ clientX: f.x0, clientY: f.y0 }) });
  }
  if (g.mode === "drag") { if (!pending) requestAnimationFrame(() => { send({ t: "move", ...pending }); pending = null; }); pending = pos(f); }
  else if (g.mode === "two" && fingers.size === 2) {
    const m = mid();
    if (!g.sub) {
      if (Math.abs(m.d - g.d0) > 24 && Math.abs(m.d - g.d0) > Math.abs(m.y - g.y0)) g.sub = "pinch";
      else if (Math.abs(m.y - g.y0) > 12 || zoom > 1) g.sub = "scroll";
    }
    if (g.sub === "pinch") {
      const z = Math.max(1, Math.min(4, g.z0 * m.d / g.d0)), k = z / zoom;
      tx = m.x - (m.x - tx) * k; ty = m.y - (m.y - ty) * k; zoom = z; applyZoom();
    } else if (g.sub === "scroll") {
      if (zoom > 1) { tx += m.x - g.lx; ty += m.y - g.ly; applyZoom(); }
      else { g.acc += m.y - g.ly; while (Math.abs(g.acc) >= 40) { send({ t: "wheel", d: g.acc > 0 ? 1 : -1 }); g.acc -= Math.sign(g.acc) * 40; } }
    }
    g.lx = m.x; g.ly = m.y;
  }
});
function touchEnd(e) {
  if (e.pointerType === "mouse") return;
  const f = fingers.get(e.pointerId); if (!f) return;
  e.preventDefault(); lastTouch = performance.now();
  fingers.delete(e.pointerId);
  if (g && g.mode === "wait" && fingers.size === 0) {
    clearTimeout(g.lp);
    if (e.type === "pointerup") { const p = pos(f); send({ t: "move", ...p }); send({ t: "shift", on: false }); send({ t: "down", ...p }); send({ t: "up", ...p }); }
  } else if (g && g.mode === "drag") send({ t: "up", ...pos(f) });
  if (fingers.size === 0) { if (g) clearTimeout(g.lp); g = null; }
}
cv.addEventListener("pointerup", touchEnd);
cv.addEventListener("pointercancel", touchEnd);

// ---- the on-screen keyboard (phones): text typed into a hidden box becomes key presses
const ime = $("ime");
ime.addEventListener("beforeinput", (e) => {
  e.preventDefault();
  if (e.inputType === "insertText" || e.inputType === "insertCompositionText") { for (const c of e.data || "") send({ t: "key", key: c }); }
  else if (e.inputType === "insertLineBreak" || e.inputType === "insertParagraph") send({ t: "key", key: "Enter" });
  else if (e.inputType === "deleteContentBackward") send({ t: "key", key: "Backspace" });
  else if (e.inputType === "deleteContentForward") send({ t: "key", key: "Delete" });
});
ime.addEventListener("keydown", (e) => { if (e.key.length > 1 && e.key !== "Unidentified" && e.key !== "Process" && e.key !== "Dead") { e.preventDefault(); send({ t: "key", key: e.key, ctrl: e.ctrlKey, shift: e.shiftKey }); } });

let rs = null;
window.addEventListener("resize", () => { if (cv.hidden) return; clearTimeout(rs); rs = setTimeout(() => { const [w, h] = size(); send({ t: "size", w, h }); applyZoom(); }, 200); });
$("form").addEventListener("submit", login);

fetch("/api/session").then(async (r) => { if (r.ok) start((await r.json()).csrf); else showLogin(""); }).catch(() => showLogin(""));
</script>
</body>
</html>
"##);
