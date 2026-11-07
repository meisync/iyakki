// SPDX-License-Identifier: Apache-2.0
// The page of `iyakki-connect serve`: one WebSocket to the program on this computer; the program carries it to the tenant. The page
// carries no credentials (a cookie it cannot read, and a CSRF value that only this origin can fetch).
// Pictures are the tenant's own tile stream, decoded in a worker (decoder.js, the same file the tenant's own page uses) onto an
// OffscreenCanvas; input goes back as small binary messages; clipboard, sound, microphone and camera are media.js, unchanged.
"use strict";
const $ = (id) => document.getElementById(id);
const cv = $("screen");
let cfg = null, ws = null, closing = false, retry = null, chain = Promise.resolve(), fatal = false;

// ---- the wire: binary input (the program turns it into the tenant's JSON), JSON for the rest
const T = { move: 0x20, down: 0x21, up: 0x22, right: 0x23, wheel: 0x24, key: 0x25, shift: 0x26 };
const enc = new TextEncoder();
function sendJson(o) { if (ws && ws.readyState === 1) ws.send(JSON.stringify(o)); }
function sendBinary(u8) { if (ws && ws.readyState === 1) ws.send(u8); }
function xy(t, p) { const b = new Uint8Array(5), dv = new DataView(b.buffer); b[0] = t; dv.setUint16(1, p.x, true); dv.setUint16(3, p.y, true); sendBinary(b); }
function sendKey(key, code, ctrl, alt, shift) {
  const k = enc.encode(key).slice(0, 32), c = enc.encode(code || "").slice(0, 32), b = new Uint8Array(3 + k.length + c.length);
  b[0] = T.key; b[1] = (ctrl ? 1 : 0) | (shift ? 2 : 0) | (alt ? 4 : 0); b[2] = k.length; b.set(k, 3); b.set(c, 3 + k.length); sendBinary(b);
}
function status(t) { const s = $("status"); s.textContent = t || ""; s.style.display = t ? "block" : "none"; }

// ---- browser devices: media.js
const media = IyakkiMedia.create(
  { nav: navigator, document, AudioContext: window.AudioContext || window.webkitAudioContext, ClipboardItem: window.ClipboardItem, Blob, TextEncoder,
    isSecureContext: window.isSecureContext, now: () => Date.now(), setInterval: (f, ms) => setInterval(f, ms) },
  { sendJson, sendBinary, onChange: (s) => renderDevices(s) });
function mmss(n) { return Math.floor(n / 60) + ":" + String(n % 60).padStart(2, "0"); }
function renderDevices(s) {
  const dev = (id, label, on, allowed, extra) => {
    const b = $(id); b.classList.toggle("on", on);
    b.textContent = label + (on ? ": on" + (extra || "") + " (tap to stop)" : !s.secure.ok ? ": needs a secure page" : !allowed ? ": not allowed" : ": off");
    b.disabled = !on && (!s.secure.ok || !allowed);
  };
  dev("dmic", "Microphone", s.on.mic, s.allowed.mic, " " + mmss(s.micSeconds) + " of 5:00");
  dev("dcam", "Camera", s.on.camera, s.allowed.camera);
  dev("dclip", "Clipboard reading", s.on.clipread, s.allowed.clipread);
  $("dsound").textContent = s.sound ? "Stop sound" : "Start sound";
  $("dsound").disabled = !s.allowed.audio || !(window.AudioContext || window.webkitAudioContext);
  $("dvolrow").hidden = !s.sound;
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
$("dsound").onclick = (e) => (media.state.sound ? media.stopSound() : media.startSound(e));
$("dvol").oninput = () => media.player.setVolume($("dvol").value / 100);
$("askdev").onclick = (e) => { const k = media.state.ask; if (k) media.toggle(k, e); cv.focus(); };
$("askclip").onclick = (e) => { if (media.state.pendingRead) media.readClipboard(e); else media.finishCopy(e); $("askclip").hidden = true; cv.focus(); };
renderDevices(media.state);

// ---- window size
let W = 0, H = 0;
function size() {
  W = Math.max(640, Math.min(3840, Math.floor(window.innerWidth))); H = Math.max(400, Math.min(2160, Math.floor(window.innerHeight)));
  if (painter) painter.resize(W, H);
  const f = Math.min(1, window.innerWidth / W, window.innerHeight / H);
  cv.style.width = Math.floor(W * f) + "px"; cv.style.height = Math.floor(H * f) + "px";
  return [W, H];
}

// ---- the picture: decoded in a worker on an OffscreenCanvas (the main thread only where the browser has neither)
let painter = null, transferred = false, rxBytes = 0, rxFrames = 0, decodeMs = 0, lastRtt = null, srv = null, pingTimer = null, statTimer = null;
const hasInflate = IyakkiDecoder.canInflate();
const canWorker = typeof Worker === "function" && typeof OffscreenCanvas === "function" && typeof cv.transferControlToOffscreen === "function" && hasInflate;
function workerPainter(off) {
  const w = new Worker("/decoder.js"), waiting = new Map();
  let id = 0;
  w.onmessage = (e) => {
    const m = e.data;
    if (m.t === "ready") return;
    const cb = waiting.get(m.id);
    if (cb) { waiting.delete(m.id); m.err ? cb.rej(new Error(m.err)) : cb.res(m); }
  };
  w.postMessage({ t: "init", canvas: off }, [off]);
  return {
    worker: true,
    paint: (buf) => new Promise((res, rej) => { const i = ++id; waiting.set(i, { res, rej }); w.postMessage({ t: "frame", id: i, buf }, [buf]); }),
    resize: (wd, ht) => w.postMessage({ t: "size", w: wd, h: ht }),
  };
}
function startPainter() {
  if (painter) return;
  if (canWorker && !transferred) {
    transferred = true;
    painter = workerPainter(cv.transferControlToOffscreen());
  } else {
    painter = new IyakkiDecoder.Painter(cv, (w, h) => { const c = document.createElement("canvas"); c.width = w; c.height = h; return c; });
    painter.worker = false;
  }
  painter.resize(W, H);
}
function onFrame(m) {
  const t0 = performance.now();
  rxFrames++;
  chain = chain.then(() => painter.paint(m)).then(
    (r) => { decodeMs = decodeMs * 0.8 + (performance.now() - t0) * 0.2; window.__frames = (window.__frames || 0) + 1; window.__lastSeq = r.seq; (window.__ft = window.__ft || []).push(performance.now()); sendJson({ t: "ack", n: r.seq }); },
    () => sendJson({ t: "full" }));
}

// ---- display settings and statistics
const qsel = $("q"), saver = $("saver"), showStats = $("showstats");
const LEVELS = { lossless: "Lossless", balanced: "Balanced", low: "Low bandwidth", verylow: "Very low" };
function sendQuality() { sendJson({ t: "quality", level: qsel.value, saver: saver.checked }); }
qsel.onchange = saver.onchange = sendQuality;
showStats.onchange = renderStats;
$("gear").onclick = () => { const p = $("panel"); p.hidden = !p.hidden; $("gear").classList.toggle("on", !p.hidden); cv.focus(); };
const fsOk = document.documentElement.requestFullscreen || document.documentElement.webkitRequestFullscreen;
if (!fsOk) $("fs").hidden = true;
$("fs").onclick = () => {
  if (document.fullscreenElement || document.webkitFullscreenElement) (document.exitFullscreen || document.webkitExitFullscreen).call(document);
  else (document.documentElement.requestFullscreen || document.documentElement.webkitRequestFullscreen).call(document.documentElement);
};
let rxPrev = 0, fpsPrev = 0, rxRate = 0, fpsRate = 0;
function renderStats() {
  const el = $("stats");
  el.hidden = !showStats.checked;
  if (el.hidden) return;
  const kb = (n) => (n >= 1e6 ? (n / 1e6).toFixed(2) + " MB/s" : (n / 1e3).toFixed(1) + " kB/s");
  el.textContent = "level   " + (srv ? (LEVELS[srv.level] || srv.level) + (srv.auto ? " (auto)" : "") : "-") + "\nreceive " + kb(rxRate) + "\nframes  " + fpsRate.toFixed(0) + " /s" +
    "\nround trip " + (lastRtt === null ? "-" : lastRtt.toFixed(0) + " ms") + "\ndecode  " + decodeMs.toFixed(1) + " ms" + (painter ? (painter.worker ? " (worker)" : " (page)") : "");
}
function tickStats() { rxRate = rxBytes - rxPrev; fpsRate = rxFrames - fpsPrev; rxPrev = rxBytes; fpsPrev = rxFrames; renderStats(); }

// ---- the connection to the program on this computer
function connect() {
  ws = new WebSocket("ws://" + location.host + "/ws");
  ws.binaryType = "arraybuffer";
  srv = null;
  ws.onopen = () => {
    status("");
    const [w, h] = size(); sendJson({ t: "size", w, h });
    sendJson({ t: "hello", v: 2, caps: { deflate: hasInflate } });
    clearInterval(pingTimer);
    pingTimer = setInterval(() => { const o = { t: "ping", ts: Math.round(performance.now()) }; if (lastRtt !== null) o.rtt = lastRtt; sendJson(o); }, 2000);
  };
  ws.onmessage = (m) => {
    if (typeof m.data === "string") {
      rxBytes += m.data.length;
      let j; try { j = JSON.parse(m.data); } catch (e) { return; }
      if (j.t === "copy" && navigator.clipboard) navigator.clipboard.writeText(String(j.s)).catch(() => {});
      else if (typeof j.t === "string" && j.t.indexOf("media_") === 0) media.onJson(j);
      else if (j.t === "cursor") cv.style.cursor = String(j.c).replace(/[^a-z-]/g, "").slice(0, 24) || "default";
      else if (j.t === "readclip") media.onReadRequest();
      else if (j.t === "bye") { fatal = true; status("Signed out of the tenant. Close this tab."); }
      else if (j.t === "connect_error") { fatal = !!j.final; status(String(j.msg || "Could not connect").slice(0, 300)); }
      else if (j.t === "hello_ok" && j.v >= 2) { startPainter(); size(); sendQuality(); }
      else if (j.t === "stats") { srv = j; renderStats(); }
      else if (j.t === "pong") lastRtt = Math.max(0, performance.now() - j.ts);
      return;
    }
    rxBytes += m.data.byteLength;
    if (new Uint8Array(m.data, 0, 1)[0] >= 0x10) { media.onBinary(m.data); return; }
    if (painter) onFrame(m.data);
  };
  ws.onclose = () => {
    clearInterval(pingTimer); media.disconnected();
    if (closing || fatal) return;
    status("Reconnecting…");
    clearTimeout(retry); retry = setTimeout(connect, 1500);
  };
}

// ---- mouse
const pos = (e) => { const r = cv.getBoundingClientRect(); return { x: Math.max(0, Math.min(65535, Math.round((e.clientX - r.left) * cv.width / r.width))), y: Math.max(0, Math.min(65535, Math.round((e.clientY - r.top) * cv.height / r.height))) }; };
let pending = null;
cv.addEventListener("mousemove", (e) => { if (!pending) requestAnimationFrame(() => { xy(T.move, pending); pending = null; }); pending = pos(e); });
cv.addEventListener("mousedown", (e) => { cv.focus(); e.preventDefault(); sendBinary(Uint8Array.of(T.shift, e.shiftKey ? 1 : 0)); xy(e.button === 2 ? T.right : T.down, pos(e)); });
window.addEventListener("mouseup", (e) => { if (e.button === 0) xy(T.up, pos(e)); });
cv.addEventListener("contextmenu", (e) => e.preventDefault());
cv.addEventListener("wheel", (e) => { e.preventDefault(); const d = -Math.sign(e.deltaY) * Math.max(1, Math.min(10, Math.round(Math.abs(e.deltaY) / 40))); sendBinary(Uint8Array.of(T.wheel, d & 255)); }, { passive: false });
cv.addEventListener("keydown", (e) => {
  const ctrl = e.ctrlKey || e.metaKey;
  if (e.ctrlKey && e.altKey && (e.key === "s" || e.key === "S")) { e.preventDefault(); showStats.checked = !showStats.checked; renderStats(); return; }
  if (e.ctrlKey && e.altKey && (e.key === "q" || e.key === "Q")) { e.preventDefault(); $("gear").click(); return; }
  if (ctrl && !e.shiftKey && (e.key === "v" || e.key === "V")) return; // the paste event carries the text
  if (e.key === "Control" || e.key === "Shift" || e.key === "Alt" || e.key === "Meta") return;
  e.preventDefault();
  sendKey(e.key, e.code, ctrl, e.altKey, e.shiftKey);
});
document.addEventListener("paste", (e) => { if (!e.clipboardData) return; e.preventDefault(); media.onPaste(e); });
let rs = null;
window.addEventListener("resize", () => { clearTimeout(rs); rs = setTimeout(() => { const [w, h] = size(); sendJson({ t: "size", w, h }); }, 200); });

// ---- files (only with --share): a folder of this computer and the tenant's folder; the program does the work and asks before replacing
let csrf = "";
async function api(method, path, body) {
  const r = await fetch(path, { method, headers: { "X-Csrf": csrf, "Content-Type": "application/json" }, body: body ? JSON.stringify(body) : undefined, credentials: "same-origin" });
  let j = {}; try { j = await r.json(); } catch (e) {}
  return { status: r.status, body: j };
}
function confirmReplace(text) {
  return new Promise((res) => {
    const box = $("ask"); $("asktext").textContent = text; box.hidden = false; $("askno").focus();
    const done = (v) => { box.hidden = true; $("askno").onclick = $("askyes").onclick = null; cv.focus(); res(v); };
    $("askno").onclick = () => done(false); $("askyes").onclick = () => done(true);
  });
}
let where = { side: "local", path: "" };
async function listing(side, path) {
  where = { side, path };
  const r = await api("GET", "/fs/" + side + "/list?path=" + encodeURIComponent(path));
  const ul = $("flist"); ul.textContent = "";
  if (r.status !== 200) { $("fnote").textContent = r.body.error || "Could not list the folder."; return; }
  $("fnote").textContent = (side === "local" ? "This computer: " : "The tenant: ") + "/" + path;
  const add = (label, act) => { const li = document.createElement("li"), b = document.createElement("button"); b.type = "button"; b.textContent = label; b.onclick = act; li.appendChild(b); ul.appendChild(li); return li; };
  if (path) add("..", () => listing(side, path.split("/").slice(0, -1).join("/")));
  for (const e of r.body.entries) {
    const full = path ? path + "/" + e.name : e.name;
    if (e.kind === "dir") add(e.name + "/", () => listing(side, full));
    else if (e.kind === "file") {
      const li = add(e.name + " (" + e.size + ")", async () => {
        const dir = side === "local" ? "send" : "fetch";
        const body = side === "local" ? { local: full, remote: e.name } : { remote: full, local: e.name };
        let x = await api("POST", "/fs/" + dir, body);
        if (x.status === 409 && x.body.exists) { if (!(await confirmReplace(x.body.error))) return; x = await api("POST", "/fs/" + dir, { ...body, replace: true }); }
        $("fnote").textContent = x.status === 200 ? (side === "local" ? "Sent " : "Saved ") + e.name : (x.body.error || "Failed");
      });
      li.title = side === "local" ? "Send to the tenant's top folder" : "Save into the shared folder";
    }
  }
}
$("fl").onclick = () => listing("local", ""); $("fr").onclick = () => listing("remote", "");

(async function boot() {
  try {
    const r = await fetch("/api/config", { credentials: "same-origin" });
    if (!r.ok) { status("This page was not opened with the link the program printed."); return; }
    cfg = await r.json(); csrf = cfg.csrf;
    if (cfg.share) { $("files").hidden = false; $("fnote").textContent = cfg.write ? "The shared folder can be read and written." : "The shared folder is read-only here."; }
  } catch (e) { status("The program is not answering."); return; }
  size(); cv.focus(); statTimer = setInterval(tickStats, 1000);
  connect();
})();
