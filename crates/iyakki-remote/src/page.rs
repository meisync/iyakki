//! The one page the server serves: a sign-in form and the canvas the screen is drawn on.

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
  #screen { display: block; outline: none; cursor: default; }
  #status { position: fixed; top: 8px; right: 12px; padding: 4px 10px; border-radius: 6px; background: #000a; font-size: 13px; display: none; }
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
<script>
"use strict";
const $ = (id) => document.getElementById(id);
const cv = $("screen"), ctx = cv.getContext("2d", { alpha: false });
let csrf = null, ws = null, closing = false, retry = null, chain = Promise.resolve();

function showLogin(msg) {
  closing = true;
  if (ws) { try { ws.close(); } catch (e) {} ws = null; }
  cv.hidden = true; $("status").style.display = "none";
  $("login").hidden = false; $("err").textContent = msg || "";
  $("go").disabled = false; $("pass").value = ""; (($("user").value ? $("pass") : $("user"))).focus();
}
function status(t) { const s = $("status"); s.textContent = t || ""; s.style.display = t ? "block" : "none"; }

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
r##"function size() {
  const w = Math.max(640, Math.min(3840, Math.floor(window.innerWidth))), h = Math.max(400, Math.min(2160, Math.floor(window.innerHeight)));
  if (cv.width !== w || cv.height !== h) { cv.width = w; cv.height = h; }
  return [w, h];
}
function send(o) { if (ws && ws.readyState === 1) ws.send(JSON.stringify(o)); }

function start(token) {
  csrf = token; closing = false;
  $("login").hidden = true; cv.hidden = false; size(); cv.focus();
  connect();
}

function connect() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  ws = new WebSocket(proto + "://" + location.host + "/ws?csrf=" + encodeURIComponent(csrf));
  ws.binaryType = "arraybuffer";
  ws.onopen = () => { status(""); const [w, h] = size(); send({ t: "size", w, h }); spVoices(); };
  ws.onmessage = (m) => {
    if (typeof m.data === "string") {
      const j = JSON.parse(m.data);
      if (j.t === "copy" && navigator.clipboard) navigator.clipboard.writeText(j.s).catch(() => {});
      if (j.t === "cursor") cv.style.cursor = j.c;
      if (j.t === "readclip" && navigator.clipboard && navigator.clipboard.readText) navigator.clipboard.readText().then((s) => { if (s) send({ t: "paste", s }); }).catch(() => {});
      if (j.t === "bye") showLogin("Signed out.");
      if (typeof j.t === "string" && j.t.indexOf("speak") === 0) spMessage(j);
      return;
    }
    const dv = new DataView(m.data), x = dv.getUint16(0, true), y = dv.getUint16(2, true);
    const blob = new Blob([m.data.slice(8)], { type: "image/png" });
    chain = chain.then(() => createImageBitmap(blob)).then((b) => { ctx.drawImage(b, x, y); b.close && b.close(); }).catch(() => {});
  };
  ws.onclose = async () => {
    if (closing) return;
    status("Reconnecting…");
    clearTimeout(retry);
    retry = setTimeout(async () => {
      const r = await fetch("/api/session").catch(() => null);
      if (r && r.status === 401) return showLogin("Your session ended. Sign in again.");
      if (r && r.ok) { csrf = (await r.json()).csrf; connect(); } else { ws = null; retry = setTimeout(() => ws || connect(), 1500); }
    }, 1000);
  };
}

const pos = (e) => { const r = cv.getBoundingClientRect(); return { x: Math.round((e.clientX - r.left) * cv.width / r.width), y: Math.round((e.clientY - r.top) * cv.height / r.height) }; };
let pending = null;
cv.addEventListener("mousemove", (e) => { if (!pending) requestAnimationFrame(() => { send({ t: "move", ...pending }); pending = null; }); pending = pos(e); });
cv.addEventListener("mousedown", (e) => { spPrime(); cv.focus(); e.preventDefault(); send({ t: "shift", on: e.shiftKey }); send({ t: e.button === 2 ? "right" : "down", ...pos(e) }); });
window.addEventListener("mouseup", (e) => { if (e.button === 0 && !cv.hidden) send({ t: "up", ...pos(e) }); });
cv.addEventListener("contextmenu", (e) => e.preventDefault());
cv.addEventListener("wheel", (e) => { e.preventDefault(); const d = -Math.sign(e.deltaY) * Math.max(1, Math.min(10, Math.round(Math.abs(e.deltaY) / 40))); send({ t: "wheel", d }); }, { passive: false });
cv.addEventListener("keydown", (e) => {
  spPrime();
  const ctrl = e.ctrlKey || e.metaKey;
  if (ctrl && !e.shiftKey && (e.key === "v" || e.key === "V")) return; // the paste event carries the text
  if (e.key === "Control" || e.key === "Shift" || e.key === "Alt" || e.key === "Meta") return;
  e.preventDefault();
  send({ t: "key", key: e.key, ctrl, shift: e.shiftKey });
});
document.addEventListener("paste", (e) => { if (cv.hidden) return; const s = e.clipboardData && e.clipboardData.getData("text"); if (s) { e.preventDefault(); send({ t: "paste", s }); } });
let rs = null;
window.addEventListener("resize", () => { if (cv.hidden) return; clearTimeout(rs); rs = setTimeout(() => { const [w, h] = size(); send({ t: "size", w, h }); }, 200); });
$("form").addEventListener("submit", login);

fetch("/api/session").then(async (r) => { if (r.ok) start((await r.json()).csrf); else showLogin(""); }).catch(() => showLogin(""));
</script>
</body>
</html>
"##);
