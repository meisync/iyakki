// Node tests of the page's file transfer (F-CLIENT-01): fake sockets, a fake DOM, no browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createFiles, join, parent, CHUNK, MAX_FILE, TAG_FILE_OUT, TAG_FILE_IN } from "../files.js";

function rig(extra = {}) {
  const sent = [], bin = [], toasts = [], saved = [];
  const f = createFiles({ send: (o) => sent.push(o), sendBinary: (t, b) => bin.push([t, b]), toast: (m) => toasts.push(m), save: (n, p) => saved.push([n, p]), ...extra });
  f.setCaps(["picture", "files.transfer"]);
  return { f, sent, bin, toasts, saved };
}
const chunk = (id, bytes) => { const m = new Uint8Array(5 + bytes.length); m[0] = TAG_FILE_OUT; new DataView(m.buffer).setUint32(1, id); m.set(bytes, 5); return m; };

test("nothing is asked of a machine that did not agree to file transfer", async () => {
  const { f, sent, toasts } = rig();
  f.setCaps(["picture"]);
  assert.equal(f.list(""), false); assert.equal(f.download("a.txt"), false);
  assert.equal(await f.upload({ name: "a", size: 1, arrayBuffer: async () => new ArrayBuffer(1) }), false);
  assert.deepEqual(sent, []); assert.ok(toasts.length >= 0);
});

test("a listing is asked for, shown and walked into and out of", () => {
  const { f, sent } = rig();
  f.list("");
  assert.deepEqual(sent[0], { t: "file_list", dir: "" });
  assert.equal(f.onMessage({ t: "file_list", dir: "", entries: [{ name: "Docs", is_dir: true, size: 0 }, { name: "a.txt", is_dir: false, size: 12 }], truncated: false }), true);
  assert.equal(f.state.entries.length, 2);
  f.list(join(f.state.dir, "Docs"));
  assert.deepEqual(sent[1], { t: "file_list", dir: "Docs" });
  assert.equal(parent("Docs/deep"), "Docs"); assert.equal(parent("Docs"), ""); assert.equal(join("", "x"), "x");
  assert.equal(f.onMessage({ t: "pong" }), false, "a message that is not a file one is not taken");
});

test("a download collects its chunks in order, checks the size and hands the file over only when whole", () => {
  const { f, sent, saved, toasts } = rig();
  f.download("Docs/a.txt");
  assert.deepEqual(sent[0], { t: "file_get", id: 1, path: "Docs/a.txt" });
  assert.equal(f.download("b"), false, "a second download while one is going is refused");
  f.onMessage({ t: "file_get_ok", id: 1, size: 6 });
  assert.equal(f.onBinary(chunk(1, new Uint8Array([1, 2, 3]))), true);
  assert.equal(f.onBinary(chunk(9, new Uint8Array([9]))), true, "a chunk of another transfer is swallowed, not mixed in");
  assert.equal(f.onBinary(chunk(1, new Uint8Array([4, 5, 6]))), true);
  f.onMessage({ t: "file_done", id: 1 });
  assert.equal(saved.length, 1); assert.equal(saved[0][0], "a.txt");
  assert.deepEqual(saved[0][1].flatMap((p) => [...p]), [1, 2, 3, 4, 5, 6]);
  assert.equal(f.onBinary(new Uint8Array([1, 2, 3])), false, "another stream is the picture's");
  // a short file is not saved
  f.download("c.txt"); f.onMessage({ t: "file_get_ok", id: 2, size: 5 }); f.onBinary(chunk(2, new Uint8Array([1])));
  f.onMessage({ t: "file_done", id: 2 });
  assert.equal(saved.length, 1); assert.match(toasts.at(-1), /wrong size/);
  // more bytes than announced stops the download
  f.download("d.txt"); f.onMessage({ t: "file_get_ok", id: 3, size: 2 }); f.onBinary(chunk(3, new Uint8Array([1, 2, 3])));
  assert.deepEqual(sent.at(-1), { t: "file_cancel", id: 3 }); assert.equal(f.state.down, null);
});

test("an upload waits for the machine's yes, then sends 32 KB chunks with the id and ends with file_end", async () => {
  const { f, sent, bin, toasts } = rig();
  const body = new Uint8Array(CHUNK * 2 + 10).map((_, i) => i % 251);
  f.state.dir = "Docs";
  assert.equal(await f.upload({ name: "big.bin", size: body.length, arrayBuffer: async () => body.buffer }), true);
  assert.deepEqual(sent[0], { t: "file_put", id: 1, path: "Docs/big.bin", size: body.length });
  assert.equal(bin.length, 0, "nothing is sent before the machine said yes");
  assert.equal(await f.upload({ name: "x", size: 1, arrayBuffer: async () => new ArrayBuffer(1) }), false, "one upload at a time");
  f.onMessage({ t: "file_put_ok", id: 1 });
  assert.equal(bin.length, 3);
  assert.ok(bin.every(([t]) => t === TAG_FILE_IN));
  assert.equal(new DataView(bin[0][1].buffer).getUint32(0), 1);
  assert.deepEqual([...bin.flatMap(([, b]) => [...b.subarray(4)])], [...body]);
  assert.deepEqual(sent.at(-1), { t: "file_end", id: 1 });
  f.onMessage({ t: "file_saved", id: 1, path: "Docs/big.bin" });
  assert.match(toasts.at(-1), /Saved big.bin/);
  assert.deepEqual(sent.at(-1), { t: "file_list", dir: "Docs" }, "the folder is shown again");
  assert.equal(await f.upload({ name: "huge", size: MAX_FILE + 1, arrayBuffer: async () => new ArrayBuffer(0) }), false);
  assert.match(toasts.at(-1), /8 MB/);
});

test("a no from the machine is shown in words and ends that transfer; a dropped connection throws half-done work away", async () => {
  const { f, toasts } = rig();
  f.download("a.txt");
  f.onMessage({ t: "file_err", id: 1, code: "card_declined", message: "the person said no" });
  assert.match(toasts.at(-1), /^The machine said no: /); assert.equal(f.state.down, null);
  await f.upload({ name: "u", size: 2, arrayBuffer: async () => new ArrayBuffer(2) });
  assert.ok(f.state.up);
  f.reset();
  assert.equal(f.state.up, null); assert.deepEqual(f.state.entries, []);
});

test("the folder is drawn as buttons: a folder opens, a file downloads, Up goes to the parent, a pick starts an upload", async () => {
  const mk = (tag) => ({ tag, hidden: false, textContent: "", children: [], style: {}, appendChild(c) { this.children.push(c); }, set textContentSet(v) {} });
  const els = { row: mk("div"), dir: mk("p"), up: mk("button"), refresh: mk("button"), list: mk("ul"), pick: { files: [{ name: "n.txt", size: 3, arrayBuffer: async () => new ArrayBuffer(3) }], value: "x" }, state: mk("p") };
  Object.defineProperty(els.list, "textContent", { set(v) { this.children = []; }, get() { return ""; } });
  const doc = { createElement: (tag) => ({ tag, children: [], appendChild(c) { this.children.push(c); } }) };
  const { f, sent } = rig({ ui: els, doc });
  f.setCaps(["files.transfer"]);
  assert.equal(els.row.hidden, false);
  f.onMessage({ t: "file_list", dir: "Docs", entries: [{ name: "deep", is_dir: true, size: 0 }, { name: "a.txt", is_dir: false, size: 2048 }], truncated: true });
  assert.equal(els.dir.textContent, "/Docs"); assert.equal(els.up.hidden, false);
  const items = els.list.children; // two entries and the "and more" line
  assert.equal(items.length, 3);
  assert.equal(items[0].children[0].textContent, "deep/"); assert.equal(items[1].children[0].textContent, "a.txt (2.0 KB)");
  items[0].children[0].onclick(); assert.deepEqual(sent.at(-1), { t: "file_list", dir: "Docs/deep" });
  items[1].children[0].onclick(); assert.deepEqual(sent.at(-1), { t: "file_get", id: 1, path: "Docs/a.txt" });
  els.up.onclick(); assert.deepEqual(sent.at(-1), { t: "file_list", dir: "" });
  els.pick.onchange(); await new Promise((r) => setImmediate(r));
  assert.deepEqual(sent.at(-1), { t: "file_put", id: 2, path: "Docs/n.txt", size: 3 }); assert.equal(els.pick.value, "");
  f.setCaps([]); assert.equal(els.row.hidden, true, "no row on a machine that does not serve files");
});
