// Draws an app's display list with the engine module and makes its input lines (F-SDK-29). One module for every page that shows an app of
// the UI SDK: the preview page (`?engine=wasm`) and the page behind a client's one-time link (`/connect`). It holds no DOM of its own: the
// canvas, the accessibility host, the element maker and the transport are handed in, so a node test drives it with fakes.
//
//   const ui = mountDisplayList({ engine, canvas, ariaHost, io, scale, doc, canvasFor, onError });
//   ui.show(await io.frame());   // the first frame; every answer of `io.send` is shown the same way
//
// `io.send(lines)` posts input lines and resolves with the next display list text. Nothing here reads a cookie, opens a socket or loads a file.

const ROLES = { item: "listitem", field: "textbox", image: "img" };
const KEYS = { Enter: "enter", Backspace: "backspace", Delete: "delete", Escape: "esc", Tab: "tab", ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right" };

/** The role the page gives an accessibility element: the SDK's word mapped to the ARIA one, others kept as they are. */
export const ariaRole = (role) => ROLES[role] || role;

/** The input line a key makes, or null for a key the app does not take (modifier chords and unknown names). */
export function keyLine(engine, ev) {
  if (KEYS[ev.key]) return engine.uiLine({ kind: "key", name: KEYS[ev.key] });
  if (ev.key && ev.key.length === 1 && !ev.ctrlKey && !ev.metaKey) return engine.uiLine({ kind: "key", name: "char", char: ev.key });
  return null;
}

export function mountDisplayList({ engine, canvas, ariaHost, io, scale = 1, doc = globalThis.document, canvasFor, onError = () => {}, now = (ev) => Math.round(ev.timeStamp) }) {
  const state = { frames: 0, left: null, error: null };
  if (ariaHost && ariaHost.style) ariaHost.style.pointerEvents = "none"; // the picture takes the mouse; the elements are for keyboards and readers
  const ctx = canvas.getContext("2d");
  const fail = (m) => { state.error = m; onError(m); };
  const send = (lines) => io.send(lines.filter(Boolean)).then(show).catch((err) => fail(String(err)));
  function show(text) {
    const r = engine.uiLoad(text, scale);
    if (r.error) { fail("The display list was refused: " + r.error); return r; }
    const done = (canvasFor ? engine.uiGlyphs(undefined, canvasFor) : engine.uiGlyphs()) || r; // the page hands the module the glyphs it lacks
    const fb = engine.uiFramebuffer();
    if (fb) {
      canvas.width = fb.w; canvas.height = fb.h;
      canvas.style.width = fb.w / scale + "px"; canvas.style.height = fb.h / scale + "px";
      ctx.putImageData(new globalThis.ImageData(new Uint8ClampedArray(fb.data), fb.w, fb.h), 0, 0);
    }
    state.left = done.texts ? done.texts.length : 0; state.frames++; state.error = null;
    onError("");
    if (ariaHost) {
      ariaHost.textContent = "";
      for (const n of r.a11y || []) {
        const d = doc.createElement("div");
        d.setAttribute("role", ariaRole(n.role)); d.setAttribute("aria-label", n.name); d.tabIndex = 0;
        d.style.cssText = "position:absolute;left:" + n.x / scale + "px;top:" + n.y / scale + "px;width:" + n.w / scale + "px;height:" + n.h / scale + "px";
        d.onkeydown = (ev) => { if (ev.key === "Enter" || ev.key === " ") { ev.preventDefault(); send([engine.uiLine({ kind: "action", n: n.action })]); } };
        ariaHost.appendChild(d);
      }
    }
    return r;
  }
  const at = (ev) => { const b = canvas.getBoundingClientRect(); return [(ev.clientX - b.left) * scale, (ev.clientY - b.top) * scale]; };
  const pointer = (phase) => (ev) => { const [x, y] = at(ev); send([engine.uiLine({ kind: "pointer", phase, x, y, t: now(ev) })]); };
  canvas.addEventListener("pointerdown", pointer("down"));
  canvas.addEventListener("pointerup", pointer("up"));
  /** The wheel scrolls the app: positive is towards older content, as the SDK's wheel line says. */
  const onWheel = (ev) => {
    const n = -Math.sign(ev.deltaY) * Math.max(1, Math.min(10, Math.round(Math.abs(ev.deltaY) / 40)));
    if (!n || Number.isNaN(n)) return;
    if (ev.preventDefault) ev.preventDefault();
    send([engine.uiLine({ kind: "wheel", n })]);
  };
  canvas.addEventListener("wheel", onWheel, { passive: false });
  /** Keys go to the app from anywhere on the page except from an accessibility element (it handles Enter and Space itself). */
  const onKey = (ev) => {
    if (ev.target && ev.target.getAttribute && ev.target.getAttribute("role")) return;
    const l = keyLine(engine, ev);
    if (l) { ev.preventDefault(); send([l]); }
  };
  /** The pane changed size (the page was resized or turned): units, not pixels. */
  const resize = (w, h) => send([engine.uiLine({ kind: "resize", w, h })]);
  /** The heartbeat the apps pace themselves by (milliseconds since the app opened). */
  const tick = (t) => send([engine.uiLine({ kind: "tick", t })]);
  return { show, send, onKey, onWheel, resize, tick, state };
}

// ------------------------------------------------------------------------------------------------------------------ the client page's apps (F-SDK-29, F-CORE-30)
// The page behind a client's one-time link shows an SDK app of the machine in display-list mode (the additive message pair `app_open`,
// `app_input`, `app_close`; answers `app_list`, `app_closed`, `app_err`). Two forms: a window on a desktop browser (a title bar with a Close
// button over a pane, the machine's picture still behind it) and the phone form (the pane fills the page under a title bar). The plan is pure
// arithmetic and the link is a queue; neither touches the DOM, a socket or a clock, so node tests drive them.

/** The apps a client may open (the machine decides which of them this person may have, and refuses the others in words). */
export const LIST_APPS = [{ id: "sample", label: "SAMPLE" }, { id: "markdown", label: "Markdown viewer" }];

const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
const TITLE = 44;

/** `phone` for a narrow page or an upright touch page, else `window`. */
export function appForm({ width, height, touch = false }) {
  return width <= 600 || (touch && height > width && width <= 820) ? "phone" : "window";
}

/** Where the pane goes and how many logical units it holds: { form, w, h, left, top, winTop, title }. The pane is 100 to 4096 units each way. */
export function panePlan({ width, height, touch = false }) {
  const form = appForm({ width, height, touch });
  if (form === "phone") {
    return { form, w: clamp(Math.floor(width), 100, 4096), h: clamp(Math.floor(height - TITLE), 100, 4096), left: 0, top: TITLE, winTop: 0, title: TITLE };
  }
  const w = clamp(Math.floor(Math.min(width - 32, 1100)), 100, 4096), h = clamp(Math.floor(Math.min(height - TITLE - 32, 760)), 100, 4096);
  const winTop = Math.max(0, Math.floor((height - (h + TITLE)) / 2));
  return { form, w, h, left: Math.max(0, Math.floor((width - w) / 2)), top: winTop + TITLE, winTop, title: TITLE };
}

/**
 * The link between the page and the engine for one open app. `engine.cmd` makes the messages; the page calls `onRecord` with every `app`
 * record the engine hands out (`o.k === "app"`). Requests wait in order for their frames: a frame resolves the oldest, a refusal rejects it
 * (with the machine's sentence), and the app ending rejects what still waits and calls `onClosed(reason)`.
 */
export function createAppLink({ engine, pump = () => {}, onClosed = () => {}, onRefused = () => {} }) {
  const waiting = [];
  let open = null;
  const ask = (cmd) => new Promise((resolve, reject) => {
    if (!engine.cmd(cmd)) return reject(new Error("The machine did not agree to show apps here."));
    waiting.push({ resolve, reject });
    pump();
  });
  return {
    get open() { return open; },
    /** Open `app` in a pane of w by h units; resolves with the first frame (display list text). */
    openApp(app, w, h) { open = app; return ask({ c: "app_open", app, w, h }).catch((e) => { open = null; throw e; }); },
    /** The `io` of `mountDisplayList`: `send(lines)` posts input lines and resolves with the next frame. */
    io: {
      send(lines) { return open ? ask({ c: "app_input", lines: lines.join("\n") }) : Promise.reject(new Error("No app is open.")); },
    },
    close() {
      if (!open) return false;
      engine.cmd({ c: "app_close" });
      pump();
      return true;
    },
    /** The connection ended: the machine closed the app with the session, and what still waits for a frame never gets one. */
    reset() {
      open = null;
      for (const w of waiting.splice(0)) w.reject(new Error("The connection ended."));
    },
    onRecord(o) {
      if (!o || o.k !== "app") return;
      const b = o.body || {};
      if (o.ev === "app_list") {
        const w = waiting.shift();
        if (w) w.resolve(b.text || "");
      } else if (o.ev === "app_err") {
        const w = waiting.shift();
        if (w) w.reject(new Error(b.message || "The machine refused."));
        else onRefused(b.message || "The machine refused.");
      } else if (o.ev === "app_closed") {
        open = null;
        for (const w of waiting.splice(0)) w.reject(new Error("The app was closed."));
        onClosed(b.reason || "closed");
      }
    },
  };
}
