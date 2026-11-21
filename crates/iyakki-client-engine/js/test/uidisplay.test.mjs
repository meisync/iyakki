// Node tests of the shared display-list page code (F-SDK-29): a fake engine, a fake canvas and a fake DOM. No browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { mountDisplayList, keyLine, ariaRole, appForm, panePlan, createAppLink, LIST_APPS } from "../uidisplay.js";

globalThis.ImageData = class { constructor(data, w, h) { this.data = data; this.width = w; this.height = h; } };

function fakeEngine(over = {}) {
  const e = {
    loads: [],
    uiLoad(text, scale) { this.loads.push([text, scale]); return over.load ? over.load(text) : { a11y: [{ role: "item", name: "Open", action: 7, x: 10, y: 20, w: 100, h: 40 }, { role: "button", name: "Go", action: 8, x: 0, y: 0, w: 5, h: 5 }], texts: [1, 2] }; },
    uiGlyphs() { return over.glyphs === undefined ? { texts: [1] } : over.glyphs; },
    uiFramebuffer() { return { data: new Uint8Array(2 * 2 * 4), w: 2, h: 2 }; },
    uiLine(o) { return o.kind === "key" && o.name === "none" ? null : "line:" + JSON.stringify(o); },
  };
  return e;
}
function fakeCanvas() {
  const l = {};
  return { width: 0, height: 0, style: {}, put: [], listeners: l, getContext() { return { putImageData: (img) => this.put.push(img) }; }, getBoundingClientRect: () => ({ left: 10, top: 20 }), addEventListener(n, f) { l[n] = f; } };
}
function fakeHost() {
  const kids = [];
  return { kids, style: {}, set textContent(v) { kids.length = 0; }, appendChild(c) { kids.push(c); } };
}
const doc = { createElement: () => ({ attrs: {}, style: {}, setAttribute(k, v) { this.attrs[k] = v; } }) };

test("a frame is drawn at the module's picture size, the accessibility elements are made and the glyph step runs once", () => {
  const engine = fakeEngine(), canvas = fakeCanvas(), host = fakeHost();
  const errs = [];
  const sent = [];
  const ui = mountDisplayList({ engine, canvas, ariaHost: host, io: { send: async (l) => { sent.push(l); return "next"; } }, scale: 2, doc, onError: (m) => errs.push(m) });
  ui.show("list v1");
  assert.deepEqual(engine.loads, [["list v1", 2]]);
  assert.equal(canvas.width, 2); assert.equal(canvas.style.width, "1px");
  assert.equal(canvas.put.length, 1);
  assert.equal(ui.state.frames, 1); assert.equal(ui.state.left, 1, "the runs the module could not compose are counted from the glyph step");
  assert.equal(host.kids.length, 2);
  assert.equal(host.kids[0].attrs.role, "listitem"); assert.equal(host.kids[1].attrs.role, "button");
  assert.equal(host.kids[0].attrs["aria-label"], "Open");
  assert.match(host.kids[0].style.cssText, /left:5px;top:10px;width:50px;height:20px/);
  assert.equal(host.style.pointerEvents, "none");
  assert.deepEqual(errs, [""]);
});

test("a list the module refuses leaves the old picture, says why and counts no frame", () => {
  const engine = fakeEngine({ load: () => ({ error: "newer than this page" }) }), canvas = fakeCanvas(), errs = [];
  const ui = mountDisplayList({ engine, canvas, ariaHost: null, io: { send: async () => "" }, doc, onError: (m) => errs.push(m) });
  ui.show("x");
  assert.equal(canvas.put.length, 0); assert.equal(ui.state.frames, 0);
  assert.equal(ui.state.error, "The display list was refused: newer than this page");
  assert.deepEqual(errs, ["The display list was refused: newer than this page"]);
});

test("pointer and key events become input lines that are sent, and the answer is the next frame", async () => {
  const engine = fakeEngine(), canvas = fakeCanvas(), sent = [];
  const ui = mountDisplayList({ engine, canvas, ariaHost: null, io: { send: async (l) => { sent.push(l); return "again"; } }, scale: 2, doc, now: () => 5 });
  canvas.listeners.pointerdown({ clientX: 15, clientY: 30 });
  await new Promise((r) => setImmediate(r));
  assert.deepEqual(sent[0], ['line:{"kind":"pointer","phase":"down","x":10,"y":20,"t":5}']);
  assert.equal(engine.loads.at(-1)[0], "again");
  let prevented = 0;
  ui.onKey({ key: "Enter", preventDefault() { prevented++; } });
  ui.onKey({ key: "a", ctrlKey: true, preventDefault() { prevented++; } });
  ui.onKey({ key: "x", target: { getAttribute: () => "button" }, preventDefault() { prevented++; } });
  await new Promise((r) => setImmediate(r));
  assert.equal(sent.length, 2, "a chord and a key on an accessibility element are not sent from the page");
  assert.equal(prevented, 1);
});

test("a transport that fails is reported and does not throw out of the event handler", async () => {
  const engine = fakeEngine(), canvas = fakeCanvas(), errs = [];
  mountDisplayList({ engine, canvas, ariaHost: null, io: { send: async () => { throw new Error("no link"); } }, doc, onError: (m) => errs.push(m) });
  canvas.listeners.pointerup({ clientX: 10, clientY: 20 });
  await new Promise((r) => setImmediate(r));
  assert.match(errs.at(-1), /no link/);
});

test("key names and roles map as the preview page always did", () => {
  const e = fakeEngine();
  assert.match(keyLine(e, { key: "ArrowLeft" }), /"name":"left"/);
  assert.match(keyLine(e, { key: "é" }), /"char":"é"/);
  assert.equal(keyLine(e, { key: "Shift" }), null);
  assert.equal(keyLine(e, { key: "c", metaKey: true }), null);
  assert.equal(ariaRole("field"), "textbox"); assert.equal(ariaRole("image"), "img"); assert.equal(ariaRole("button"), "button");
});

// ---- the client page's apps: the window and phone forms and the link to the engine

test("a narrow page or an upright touch page is the phone form, a wide one is a window", () => {
  assert.equal(appForm({ width: 390, height: 844, touch: true }), "phone");
  assert.equal(appForm({ width: 600, height: 400 }), "phone", "600 wide is still a phone");
  assert.equal(appForm({ width: 820, height: 1180, touch: true }), "phone", "an upright tablet");
  assert.equal(appForm({ width: 820, height: 1180, touch: false }), "window", "an upright desktop window is not a phone");
  assert.equal(appForm({ width: 1440, height: 900, touch: false }), "window");
  assert.equal(appForm({ width: 1180, height: 820, touch: true }), "window", "a tablet on its side");
});

test("the phone pane fills the page under the title bar and the window pane is centred and bounded", () => {
  const p = panePlan({ width: 390, height: 844, touch: true });
  assert.deepEqual([p.form, p.w, p.h, p.left, p.top, p.title], ["phone", 390, 800, 0, 44, 44]);
  const w = panePlan({ width: 1920, height: 1080 });
  assert.deepEqual([w.form, w.w, w.h], ["window", 1100, 760]);
  assert.equal(w.left, 410); assert.equal(w.top, w.winTop + 44);
  assert.ok(w.winTop >= 0 && w.winTop + 44 + w.h <= 1080, "inside the page");
  const small = panePlan({ width: 700, height: 500 });
  assert.deepEqual([small.form, small.w, small.h], ["window", 668, 424]);
  for (const [width, height] of [[1, 1], [50, 50], [100000, 100000]]) {
    const q = panePlan({ width, height });
    assert.ok(q.w >= 100 && q.w <= 4096 && q.h >= 100 && q.h <= 4096, `the machine's limits hold for ${width}x${height}`);
  }
});

function fakeLinkEngine(refuse = false) {
  const sent = [];
  return { sent, cmd(o) { sent.push(o); return !refuse; } };
}

test("requests wait in order for their frames, a refusal rejects the oldest with the machine's sentence and the app ending rejects the rest", async () => {
  const engine = fakeLinkEngine(), closed = [], refused = [];
  let pumps = 0;
  const link = createAppLink({ engine, pump: () => pumps++, onClosed: (r) => closed.push(r), onRefused: (m) => refused.push(m) });
  const first = link.openApp("sample", 800, 600);
  assert.deepEqual(engine.sent[0], { c: "app_open", app: "sample", w: 800, h: 600 });
  assert.equal(link.open, "sample");
  link.onRecord({ k: "app", ev: "app_list", body: { app: "sample", text: "frame 1" } });
  assert.equal(await first, "frame 1");
  const a = link.io.send(["l1", "l2"]), b = link.io.send(["l3"]);
  assert.deepEqual(engine.sent[1], { c: "app_input", lines: "l1\nl2" });
  link.onRecord({ k: "app", ev: "app_list", body: { text: "frame 2" } });
  link.onRecord({ k: "app", ev: "app_err", body: { code: "bad_request", message: "line 1: unknown input" } });
  assert.equal(await a, "frame 2");
  await assert.rejects(b, /line 1: unknown input/);
  link.onRecord({ k: "app", ev: "app_err", body: { message: "nobody waits for this" } });
  assert.deepEqual(refused, ["nobody waits for this"], "a refusal with no request is shown, not lost");
  const c = link.io.send(["l4"]);
  link.onRecord({ k: "app", ev: "app_closed", body: { app: "sample", reason: "left" } });
  await assert.rejects(c, /closed/);
  assert.deepEqual(closed, ["left"]); assert.equal(link.open, null);
  await assert.rejects(link.io.send(["x"]), /No app is open/);
  assert.ok(pumps >= 3, "the engine's records are pumped after each message");
  link.onRecord({ k: "media", ev: "x" }); link.onRecord(null); // records that are not an app's are ignored
});

test("an engine that does not agree to apps refuses at once and the page is not left waiting; closing sends one close message", async () => {
  const link = createAppLink({ engine: fakeLinkEngine(true) });
  await assert.rejects(link.openApp("sample", 800, 600), /did not agree/);
  assert.equal(link.open, null, "a refused open leaves no app open");
  const engine = fakeLinkEngine();
  const l2 = createAppLink({ engine });
  assert.equal(l2.close(), false, "nothing open, nothing sent");
  const p = l2.openApp("markdown", 400, 300); l2.onRecord({ k: "app", ev: "app_list", body: { text: "t" } }); await p;
  assert.equal(l2.close(), true);
  assert.deepEqual(engine.sent.at(-1), { c: "app_close" });
  assert.deepEqual(LIST_APPS.map((a) => a.id), ["sample", "markdown"]);
});

test("the wheel, a resize and the heartbeat become the SDK's input lines", async () => {
  const engine = fakeEngine(), canvas = fakeCanvas(), sent = [];
  engine.uiLine = (o) => "line:" + JSON.stringify(o);
  const ui = mountDisplayList({ engine, canvas, ariaHost: null, io: { send: async (l) => { sent.push(l[0]); return "f"; } }, doc });
  let prevented = 0;
  canvas.listeners.wheel({ deltaY: -120, preventDefault() { prevented++; } }); // up: towards older content, three notches
  canvas.listeners.wheel({ deltaY: 40, preventDefault() { prevented++; } });
  canvas.listeners.wheel({ deltaY: 0, preventDefault() { prevented++; } });    // nothing to scroll
  ui.resize(390, 800); ui.tick(1500);
  await new Promise((r) => setImmediate(r));
  assert.deepEqual(sent, ['line:{"kind":"wheel","n":3}', 'line:{"kind":"wheel","n":-1}', 'line:{"kind":"resize","w":390,"h":800}', 'line:{"kind":"tick","t":1500}']);
  assert.equal(prevented, 2, "the page does not scroll under the app");
});

test("a dropped connection rejects what waits and forgets the open app", async () => {
  const link = createAppLink({ engine: fakeLinkEngine() });
  const p = link.openApp("sample", 800, 600);
  link.reset();
  await assert.rejects(p, /connection ended/);
  assert.equal(link.open, null);
});
