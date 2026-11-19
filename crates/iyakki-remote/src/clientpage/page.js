// The page behind a client's one-time link (EXPERIMENTAL). It holds no policy: the machine decides what this device and this person may do.
// It talks to nothing but the machine that served it (strict CSP: connect-src 'self'), and it never writes a token, a password or a message to
// the console. The engine (WebAssembly) speaks the protocol, decodes the picture and builds the input messages; this file is the host:
// socket, keys, forms, canvas, touch.

import { IyakkiEngine, randomHex, fitSize, defaultFitMode } from "/connect/engine.js";
import * as keys from "/connect/keys.js";

const $ = (id) => document.getElementById(id);

// The link token travels in the fragment, so it never reaches a server log. Take it out of the address bar and the history at once.
let token = null;
try { token = decodeURIComponent(location.hash.slice(1)) || null; } catch { token = null; }
if (location.hash) { try { history.replaceState(null, "", location.pathname + location.search); } catch { /* the token is in memory anyway */ } }

const CLIENT = "iyakki-web-client/0.1";
let engine = null, rec = null, ws = null, creds = null;
let retryTimer = null, backoff = 1000, delayOverride = 0, opened = 0, everHello = false, stopped = false, lastVersion = -1, drawQueued = false;
let toastTimer = null;
// the media half (sound, clipboard, devices) is a separate file; a page whose host does not serve it simply has no media buttons
let media = null, agreed = null;
const sendBinary = (tag, bytes) => { if (!ws || ws.readyState !== 1) return; const m = new Uint8Array(bytes.length + 1); m[0] = tag; m.set(bytes, 1); ws.send(m); };

// ------------------------------------------------------------------------------------------------------------------ small helpers

function status(text) { $("status").textContent = text; }
function show(...ids) { for (const id of ["enrol", "signin", "problem"]) $(id).hidden = !ids.includes(id); }
function toast(text, ms = 4000) {
  const t = $("toast");
  t.textContent = text; t.hidden = false;
  clearTimeout(toastTimer); toastTimer = setTimeout(() => { t.hidden = true; }, ms);
}
function problem(title, message, { retry = false, forget = false } = {}) {
  $("ptitle").textContent = title; $("pmsg").textContent = message;
  $("retry").hidden = !retry; $("forget").hidden = !forget;
  $("card").hidden = false; $("stage").hidden = true;
  show("problem");
}
function showCard() { $("card").hidden = false; $("stage").hidden = true; }
function showStage() { $("card").hidden = true; $("stage").hidden = false; fit(); $("screen").focus({ preventScroll: true }); }
function keyLine(el) { if (rec) el.textContent = keys.describeKey(rec); }

// ------------------------------------------------------------------------------------------------------------------ the engine's records

function pump() {
  if (!engine) return;
  for (const o of engine.drain()) onOut(o);
}

function onOut(o) {
  switch (o.k) {
    case "send": if (ws && ws.readyState === 1) ws.send(o.text); break;
    case "sign": sign(); break;
    case "device":
      if (rec) {
        rec.device = o.id;
        keys.saveDevice(rec).catch(() => toast("This browser would not keep the device key, so a new link is needed each time.", 9000));
      }
      break;
    case "phase": onPhase(o.phase, o.text); break;
    case "caps": agreed = o.caps; if (media) media.setCaps(agreed); break;
    case "media": if (media) media.onMedia(o); break;
    case "frame": queueDraw(); break;
    case "event": toast(o.topic + ": " + o.body); break;
    case "notice": toast(o.text); break;
    case "pong": break;
    case "fail": onFail(o); break;
    case "bye": onBye(o); break;
    default: break; // call results: this page makes no calls yet
  }
}

function onPhase(phase, text) {
  status(text);
  const inStage = !$("stage").hidden;
  if (phase === "sign_in") {
    if (creds) { engine.cmd({ c: "sign_in", user: creds.user, password: creds.password }); pump(); return; } // a dropped connection: no new question
    showCard(); show("signin"); keyLine($("keyinfo2")); $("pass").value = "";
    ($("user").value ? $("pass") : $("user")).focus();
  } else if (phase === "live") {
    backoff = 1000; delayOverride = 0; everHello = true;
    $("live").textContent = "Connected";
    showStage(); sendSize();
  } else if (phase === "connecting" || phase === "enrolling" || phase === "proving" || phase === "opening") {
    everHello = true;
    // while the picture is on screen a reconnect keeps it there, dimmed by the label, instead of blanking it
    if (inStage) $("live").textContent = "Reconnecting";
    else if (phase !== "opening") { showCard(); show(); }
  } else if (phase === "closed") {
    $("live").textContent = "Reconnecting";
  }
}

function onFail(o) {
  delayOverride = o.retry_s ? o.retry_s * 1000 : 0;
  if (o.code === "sign_in_refused") {
    creds = null; show("signin"); $("card").hidden = false; $("stage").hidden = true;
    status(o.title + ". " + o.message); $("pass").value = ""; $("pass").focus();
    return;
  }
  if (o.code === "not_signed_in") { show("signin"); status(o.message); return; }
  const forget = o.action === "forget_device";
  if (forget) { keys.forgetDevice(); rec = null; }
  if (o.fatal || forget) { stopped = true; problem(o.title, o.message, { retry: o.action === "reload", forget: false }); return; }
  // a refusal that trying again can fix: say it and let the reconnect run
  if ($("stage").hidden) status(o.title + ". " + o.message); else toast(o.title + ": " + o.message, 7000);
}

function onBye(o) {
  creds = null;
  if (o.action === "forget_device") { keys.forgetDevice(); rec = null; }
  if (o.reason === "revoked" || o.reason === "disabled" || o.reason === "signed_out") {
    stopped = true;
    problem(o.title, o.message + (o.reason === "signed_out" ? " Press Try again to sign in." : ""), { retry: o.reason === "signed_out" });
  }
  // any other reason: the socket closes and the reconnect runs
}

// ------------------------------------------------------------------------------------------------------------------ the device proof

async function sign() {
  try {
    const ts = Math.floor(Date.now() / 1000), nonce = randomHex(16);
    if (rec.mode === "webcrypto") {
      const t = engine.cmd({ c: "begin_sign", ts, nonce });
      if (!t) throw new Error("no text");
      engine.cmd({ c: "finish_sign", sig: await keys.signText(rec, t.text) });
    } else if (!engine.cmd({ c: "sign_seed", seed: rec.seed, ts, nonce })) throw new Error("no signature");
  } catch {
    stopped = true;
    problem("This device could not sign", "The browser would not sign with this device's key. Forget this device and connect with a new link.", { forget: true });
    try { ws && ws.close(); } catch { /* closing */ }
  }
  pump();
}

// ------------------------------------------------------------------------------------------------------------------ the socket

function connect() {
  clearTimeout(retryTimer);
  if (stopped || !engine) return;
  const url = (location.protocol === "https:" ? "wss://" : "ws://") + location.host + "/client/ws";
  let sock;
  try { sock = new WebSocket(url); } catch { return later(); }
  ws = sock; sock.binaryType = "arraybuffer"; opened = 0;
  sock.onopen = () => { opened = 1; engine.cmd({ c: "connected" }); pump(); };
  sock.onmessage = (m) => {
    if (sock !== ws) return;
    if (typeof m.data === "string") engine.onText(m.data); else if (!(media && media.onBinary(m.data))) engine.onBinary(m.data);
    pump();
  };
  sock.onclose = () => {
    if (sock !== ws) return;
    ws = null;
    if (media) media.reset(); // nothing keeps running without the connection: sound stops, the camera and microphone lights go off
    if (engine) { engine.cmd({ c: "closed" }); pump(); }
    const st = engine && engine.cmd({ c: "state" });
    if (stopped || (st && !st.reconnect)) return;
    if (!opened && !everHello) status("The machine does not answer. Check that Connect a client is switched on in Settings and that you are on the same network. Trying again.");
    else if (!$("stage").hidden) $("live").textContent = "Reconnecting";
    later();
  };
  sock.onerror = () => { /* onclose follows */ };
}

function later() {
  const d = delayOverride || backoff;
  delayOverride = 0; backoff = Math.min(backoff * 2, 15000);
  clearTimeout(retryTimer); retryTimer = setTimeout(connect, d);
}

function reconnectNow() {
  if (stopped || ws || !engine) return;
  clearTimeout(retryTimer); backoff = 1000; connect();
}
window.addEventListener("online", reconnectNow);
document.addEventListener("visibilitychange", () => { if (!document.hidden) { reconnectNow(); if (engine && ws) queueDraw(true); } });

// ------------------------------------------------------------------------------------------------------------------ the picture

const cv = $("screen"), ctx = cv.getContext("2d", { alpha: false });
let zoom = 1, tx = 0, ty = 0, fitMode = null;

function fit() {
  const st = engine && engine.cmd({ c: "state" }), [sw, sh] = st ? st.screen : [0, 0];
  if (!sw || !sh) return;
  if (!fitMode) fitMode = defaultFitMode(window.innerWidth, window.innerHeight, sw, sh);
  const [fw, fh] = fitSize(window.innerWidth, window.innerHeight, sw, sh, fitMode);
  cv.style.width = fw + "px"; cv.style.height = fh + "px";
  $("fit").textContent = fitMode === "fill" ? "Whole picture" : "Fill height";
  applyZoom();
}
function applyZoom() {
  zoom = Math.max(1, Math.min(5, zoom));
  const w = cv.offsetWidth * zoom, h = cv.offsetHeight * zoom, vw = window.innerWidth, vh = window.innerHeight;
  if (zoom === 1 && fitMode !== "fill") { tx = 0; ty = 0; }
  const ox = w <= vw ? (vw - w) / 2 : Math.min(0, Math.max(vw - w, tx)), oy = h <= vh ? (vh - h) / 2 : Math.min(0, Math.max(vh - h, ty));
  if (w > vw) tx = ox; if (h > vh) ty = oy;
  cv.style.transform = "translate(" + ox + "px," + oy + "px) scale(" + zoom + ")";
}

function queueDraw(force) {
  if (force) lastVersion = -1;
  if (drawQueued) return;
  drawQueued = true;
  requestAnimationFrame(() => {
    drawQueued = false;
    const fb = engine && engine.framebuffer();
    if (!fb || fb.version === lastVersion) return;
    lastVersion = fb.version;
    if (cv.width !== fb.w || cv.height !== fb.h) { cv.width = fb.w; cv.height = fb.h; fit(); }
    const img = new ImageData(fb.data, fb.w, fb.h), hold = media && media.lipDelay ? media.lipDelay() : 0;
    // lip-sync: while the machine's sound plays here, the picture waits as long as the sound does (never more than 150 ms)
    if (hold > 0) setTimeout(() => ctx.putImageData(img, 0, 0), hold); else ctx.putImageData(img, 0, 0);
  });
}

function sendSize() {
  if (!engine) return;
  engine.cmd({ c: "size", w: window.innerWidth, h: window.innerHeight }); pump();
}
let rs = null;
window.addEventListener("resize", () => { clearTimeout(rs); rs = setTimeout(() => { if (!$("stage").hidden) { sendSize(); fit(); } }, 200); });

// ------------------------------------------------------------------------------------------------------------------ input

const pos = (e) => {
  const r = cv.getBoundingClientRect();
  return engine.cmd({ c: "map", x: e.clientX - r.left, y: e.clientY - r.top, w: r.width, h: r.height });
};
// the position of a touch point (a record of the fingers map holds x and y, a pointer event holds clientX and clientY)
const posOf = (f) => pos({ clientX: f.x, clientY: f.y });
const put = (kind, p) => { if (p) { engine.cmd({ c: "pointer", kind, x: p.x, y: p.y }); pump(); } };
let pending = null, lastTouch = -1e9; // not 0: performance.now() is small just after the page opened, and a click then would count as a touch echo
const fromTouch = () => performance.now() - lastTouch < 800;

cv.addEventListener("mousemove", (e) => {
  if (fromTouch() || !engine) return;
  const first = !pending; pending = pos(e);
  if (first) requestAnimationFrame(() => { put("move", pending); pending = null; });
});
cv.addEventListener("mousedown", (e) => {
  if (fromTouch() || !engine) return;
  cv.focus(); e.preventDefault();
  engine.cmd({ c: "shift", on: e.shiftKey });
  put(e.button === 2 ? "right" : "down", pos(e));
});
window.addEventListener("mouseup", (e) => { if (fromTouch() || !engine || $("stage").hidden) return; if (e.button === 0) put("up", pos(e)); });
cv.addEventListener("contextmenu", (e) => e.preventDefault());
cv.addEventListener("wheel", (e) => {
  if (!engine) return;
  e.preventDefault();
  engine.cmd({ c: "wheel", d: -Math.sign(e.deltaY) * Math.max(1, Math.min(10, Math.round(Math.abs(e.deltaY) / 40))) }); pump();
}, { passive: false });

cv.addEventListener("keydown", (e) => {
  if (!engine) return;
  if (e.key === "Control" || e.key === "Shift" || e.key === "Alt" || e.key === "Meta") return;
  e.preventDefault();
  engine.cmd({ c: "key", key: e.key, code: e.code, ctrl: e.ctrlKey || e.metaKey, alt: e.altKey, shift: e.shiftKey }); pump();
});

// touch: tap = click, long press = right click, drag = drag, two fingers = scroll, pinch = zoom (and pan while zoomed)
const fingers = new Map();
let g = null;
const mid = () => { const a = [...fingers.values()]; return { x: (a[0].x + a[1].x) / 2, y: (a[0].y + a[1].y) / 2, d: Math.hypot(a[0].x - a[1].x, a[0].y - a[1].y) }; };
cv.addEventListener("pointerdown", (e) => {
  if (e.pointerType === "mouse" || !engine) return;
  e.preventDefault(); lastTouch = performance.now();
  try { cv.setPointerCapture(e.pointerId); } catch { /* not capturable */ }
  fingers.set(e.pointerId, { x0: e.clientX, y0: e.clientY, x: e.clientX, y: e.clientY });
  if (fingers.size === 1) {
    const f = fingers.get(e.pointerId);
    g = { mode: "wait", lp: setTimeout(() => { if (g && g.mode === "wait") { g.mode = "long"; put("move", posOf(f)); put("right", posOf(f)); } }, 550) };
  } else if (g && fingers.size === 2) {
    clearTimeout(g.lp);
    if (g.mode === "drag") put("up", posOf([...fingers.values()][0]));
    const m = mid();
    g = { mode: "two", sub: null, d0: m.d, z0: zoom, ly: m.y, lx: m.x, y0: m.y, acc: 0 };
  }
});
cv.addEventListener("pointermove", (e) => {
  if (e.pointerType === "mouse" || !engine) return;
  const f = fingers.get(e.pointerId); if (!f || !g) return;
  e.preventDefault(); lastTouch = performance.now();
  f.x = e.clientX; f.y = e.clientY;
  if (g.mode === "wait" && fingers.size === 1 && Math.hypot(f.x - f.x0, f.y - f.y0) > 10) {
    clearTimeout(g.lp); g.mode = "drag";
    engine.cmd({ c: "shift", on: false });
    const p0 = pos({ clientX: f.x0, clientY: f.y0 });
    put("move", p0); put("down", p0);
  }
  if (g.mode === "drag") { const first = !pending; pending = posOf(f); if (first) requestAnimationFrame(() => { put("move", pending); pending = null; }); }
  else if (g.mode === "two" && fingers.size === 2) {
    const m = mid();
    if (!g.sub) {
      if (Math.abs(m.d - g.d0) > 24 && Math.abs(m.d - g.d0) > Math.abs(m.y - g.y0)) g.sub = "pinch";
      else if (Math.abs(m.y - g.y0) > 12 || zoom > 1 || fitMode === "fill") g.sub = "scroll";
    }
    if (g.sub === "pinch") { zoom = g.z0 * m.d / g.d0; applyZoom(); }
    else if (g.sub === "scroll") {
      if (zoom > 1 || fitMode === "fill") { tx += m.x - g.lx; ty += m.y - g.ly; applyZoom(); }
      else { g.acc += m.y - g.ly; while (Math.abs(g.acc) >= 40) { engine.cmd({ c: "wheel", d: g.acc > 0 ? 1 : -1 }); pump(); g.acc -= Math.sign(g.acc) * 40; } }
    }
    g.lx = m.x; g.ly = m.y;
  }
});
function touchEnd(e) {
  if (e.pointerType === "mouse" || !engine) return;
  const f = fingers.get(e.pointerId); if (!f) return;
  e.preventDefault(); lastTouch = performance.now();
  fingers.delete(e.pointerId);
  if (g && g.mode === "wait" && fingers.size === 0) {
    clearTimeout(g.lp);
    if (e.type === "pointerup") { const p = posOf(f); put("move", p); engine.cmd({ c: "shift", on: false }); put("down", p); put("up", p); }
  } else if (g && g.mode === "drag") put("up", posOf(f));
  if (fingers.size === 0) { if (g) clearTimeout(g.lp); g = null; }
}
cv.addEventListener("pointerup", touchEnd);
cv.addEventListener("pointercancel", touchEnd);

// the on-screen keyboard of a phone: text typed into a hidden box becomes key presses
const ime = $("ime");
ime.addEventListener("beforeinput", (e) => {
  e.preventDefault();
  if (!engine) return;
  if (e.inputType === "insertText" || e.inputType === "insertCompositionText") engine.cmd({ c: "text", s: e.data || "" });
  else if (e.inputType === "insertLineBreak" || e.inputType === "insertParagraph") engine.cmd({ c: "key", key: "Enter" });
  else if (e.inputType === "deleteContentBackward") engine.cmd({ c: "key", key: "Backspace" });
  else if (e.inputType === "deleteContentForward") engine.cmd({ c: "key", key: "Delete" });
  pump();
});
ime.addEventListener("keydown", (e) => {
  if (engine && e.key.length > 1 && e.key !== "Unidentified" && e.key !== "Process" && e.key !== "Dead") {
    e.preventDefault(); engine.cmd({ c: "key", key: e.key, code: e.code, ctrl: e.ctrlKey, shift: e.shiftKey }); pump();
  }
});
$("kbd").addEventListener("click", () => { ime.tabIndex = 0; ime.focus(); });
$("fit").addEventListener("click", () => { fitMode = fitMode === "fill" ? "whole" : "fill"; tx = 0; ty = 0; fit(); });
$("full").addEventListener("click", () => { const el = document.documentElement; (document.fullscreenElement ? document.exitFullscreen() : el.requestFullscreen && el.requestFullscreen()).catch(() => {}); });
$("bye").addEventListener("click", () => {
  creds = null; stopped = true; clearTimeout(retryTimer);
  if (media) media.reset();
  engine.cmd({ c: "bye" }); pump();
  try { ws && ws.close(); } catch { /* closing */ }
  problem("Disconnected", "You disconnected this browser. The device stays added; press Try again to sign in again.", { retry: true });
});
$("bar").addEventListener("pointerdown", () => { $("bar").classList.add("on"); setTimeout(() => $("bar").classList.remove("on"), 4000); });

// ------------------------------------------------------------------------------------------------------------------ forms

function start() {
  stopped = false; backoff = 1000; everHello = false;
  engine.init({ token, public: rec.public, name: rec.name, device: rec.device, client: CLIENT });
  token = null; // the engine holds it until it is sent; this page keeps nothing
  status("Connecting"); show(); showCard();
  connect();
}

$("enrol").addEventListener("submit", async (e) => {
  e.preventDefault();
  const name = $("devname").value.trim().slice(0, 40);
  if (!name) return;
  try { rec = await keys.newKey(engine, name); } catch { problem("No key", "This browser could not make a device key.", {}); return; }
  start();
});

$("signin").addEventListener("submit", (e) => {
  e.preventDefault();
  const user = $("user").value.trim(), password = $("pass").value;
  $("pass").value = "";
  if (!user || !password) return;
  creds = $("keep").checked ? { user, password } : null;
  if (!engine.cmd({ c: "sign_in", user, password })) status("Not ready to sign in yet.");
  pump();
});

$("retry").addEventListener("click", () => {
  if (!rec) { location.reload(); return; }
  start();
});
$("forget").addEventListener("click", async () => { await keys.forgetDevice(); rec = null; problem("Device forgotten", "This browser no longer has a device key. Open a new link from Settings on the IYAKKI machine to add it again."); });

// ------------------------------------------------------------------------------------------------------------------ boot

async function boot() {
  if (!window.WebAssembly || !window.indexedDB || !window.crypto || !crypto.getRandomValues) {
    return problem("This browser is too old", "This page needs WebAssembly, IndexedDB and random numbers from the browser.");
  }
  try {
    const r = await fetch("/connect/engine.wasm", { credentials: "omit", cache: "no-store" });
    if (!r.ok) throw new Error("wasm");
    engine = await IyakkiEngine.load(await r.arrayBuffer());
  } catch {
    return problem("The client engine did not load", "The page could not start its engine. Reload; if it keeps failing, this machine's build has no client engine.", { retry: true });
  }
  import("/connect/media.js").then((m) => {
    media = m.createMedia({ engine, pump, toast, send: { binary: sendBinary, buffered: () => (ws ? ws.bufferedAmount : 0) } });
    if (agreed) media.setCaps(agreed);
  }).catch(() => { /* no media in this host: the page works without it */ });
  rec = await keys.loadDevice();
  if (token) {
    // a new link: add this browser as a device (this replaces the device saved here, if any; the old one stays in the machine's list until removed)
    status("Ready to add this device");
    $("devname").value = defaultName();
    const probe = await keys.webCryptoEd25519();
    $("keyinfo").textContent = keys.heldByProgram ? keys.describeKey(null) : probe ? "Key: the browser will keep it so that it cannot be read out of the browser." : "Key: this browser cannot keep a non-extractable key here (it needs a current browser over https), so the key is kept in the page's storage instead. That is weaker: any script of this page could read it.";
    show("enrol");
  } else if (rec && rec.device) {
    start();
  } else {
    stopped = true;
    problem("No link", "This browser has no saved device and the address has no link. Open the link shown in Settings, Remote access, Connect a client on the IYAKKI machine.");
  }
}

function defaultName() {
  const ua = navigator.userAgent || "";
  const os = /Android/.test(ua) ? "Android" : /iPhone|iPad/.test(ua) ? "iPhone or iPad" : /Windows/.test(ua) ? "Windows" : /Mac/.test(ua) ? "Mac" : /Linux/.test(ua) ? "Linux" : "";
  const br = /Edg\//.test(ua) ? "Edge" : /Firefox\//.test(ua) ? "Firefox" : /Chrome\//.test(ua) ? "Chrome" : /Safari\//.test(ua) ? "Safari" : "Browser";
  return (br + (os ? " on " + os : "")).slice(0, 40);
}

boot();
