// Browser voice: the server sends the pieces of a story that are being read, this page says them with the browser's own
// speech synthesis (Web Speech API) and reports back. Nothing else is ever spoken.
const SP = { cur: null, id: null, primed: false };
function spOk() { return typeof window.speechSynthesis !== "undefined" && typeof window.SpeechSynthesisUtterance !== "undefined"; }
function spVoices() {
  if (!spOk()) { send({ t: "sp_voices", ok: false, voices: [] }); return; }
  send({ t: "sp_voices", ok: true, voices: speechSynthesis.getVoices().map((v) => ({ n: v.name, l: v.lang, d: v.localService })) });
}
// Some browsers only speak after a click or key press inside the page: say a silent word on the first one
function spPrime() {
  if (SP.primed || !spOk()) return;
  SP.primed = true;
  try { const u = new SpeechSynthesisUtterance(" "); u.volume = 0; speechSynthesis.speak(u); } catch (e) {}
}
function spSpeak(j) {
  if (!spOk()) { send({ t: "sp_error", id: j.id, e: "this browser has no speech synthesis" }); return; }
  const u = new SpeechSynthesisUtterance(String(j.text || ""));
  if (j.lang) u.lang = j.lang;
  if (j.voice) { const v = speechSynthesis.getVoices().find((x) => x.name === j.voice); if (v) { u.voice = v; u.lang = v.lang; } }
  u.rate = Math.max(0.5, Math.min(2, Number(j.rate) || 1));
  u.onstart = () => send({ t: "sp_start", id: j.id });
  u.onend = () => { if (SP.cur === u) { SP.cur = null; SP.id = null; } send({ t: "sp_end", id: j.id }); };
  u.onerror = (e) => {
    if (SP.cur === u) { SP.cur = null; SP.id = null; }
    if (e.error === "interrupted" || e.error === "canceled") return; // we asked for it
    const why = e.error === "not-allowed" ? "the browser blocked speech until you click inside the page; click once and press play again" : (e.error || "speech failed");
    send({ t: "sp_error", id: j.id, e: why });
  };
  u.onboundary = (e) => { if (e.name === undefined || e.name === "word") send({ t: "sp_boundary", id: j.id, c: e.charIndex || 0 }); };
  SP.cur = u; SP.id = j.id; // keeps a reference too: some browsers drop an utterance nobody holds
  speechSynthesis.cancel(); // a new piece replaces the one still being said
  setTimeout(() => { if (SP.cur === u) speechSynthesis.speak(u); }, 40); // speak right after cancel() is dropped by some browsers
}
function spMessage(j) {
  if (j.t === "speak") spSpeak(j);
  else if (j.t === "speak_cancel") { if (j.id == null || j.id === SP.id) { SP.cur = null; SP.id = null; if (spOk()) speechSynthesis.cancel(); } }
  else if (j.t === "speak_pause") { if (spOk()) speechSynthesis.pause(); }
  else if (j.t === "speak_resume") { if (spOk()) speechSynthesis.resume(); }
  else if (j.t === "speak_voices") spVoices();
}
if (spOk() && speechSynthesis.addEventListener) speechSynthesis.addEventListener("voiceschanged", () => spVoices()); // voices load late
