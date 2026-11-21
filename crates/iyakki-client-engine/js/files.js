// File transfer of the IYAKKI client page (EXPERIMENTAL, F-CLIENT-01): browse the person's home folder on the machine, download a file, upload a file.
// Hand-written, no libraries, no DOM of its own beyond the elements it is given, so a node test drives it with fakes.
//
// Rules (docs/development/CLIENT-SPLIT-DESIGN.md 7d): nothing starts without a click; the machine asks on its own screen (one card for browsing, one
// for every file that goes either way) and a no is shown in words; at most 8 MB; a transfer stops when the connection ends. The control messages are
// JSON texts (`file_list`, `file_get`, `file_put`, `file_end`, `file_cancel`) that this module sends itself; the machine answers with `file_*` texts and
// the bytes travel as binary chunks: stream 6 to this page and stream 7 from it, each `[id: u32 big endian][bytes]`.

export const TAG_FILE_OUT = 6, TAG_FILE_IN = 7;
export const CHUNK = 32 * 1024, MAX_FILE = 8 * 1024 * 1024;
const WORDS = { not_granted: "Not allowed for this device: ", no_card: "Nobody at the machine to ask: ", card_declined: "The machine said no: ", too_big: "Too big: ", busy: "Wait: ", unsupported: "This machine does not do that: ", capability_missing: "Not offered: ", role_denied: "Not allowed: " };

const sizeText = (n) => (n < 1024 ? n + " B" : n < 1048576 ? (n / 1024).toFixed(1) + " KB" : (n / 1048576).toFixed(1) + " MB");
export const join = (dir, name) => (dir ? dir + "/" + name : name);
export const parent = (dir) => (dir.includes("/") ? dir.slice(0, dir.lastIndexOf("/")) : "");

/** `send(obj)` sends a JSON text, `sendBinary(tag, bytes)` a tagged chunk, `save(name, blobParts)` hands a finished download to the person. */
export function createFiles({ send, sendBinary, toast = () => {}, save, doc = globalThis.document, ui = null }) {
  const st = { caps: [], dir: "", entries: [], truncated: false, next: 1, down: null, up: null, busy: "" };
  const has = () => st.caps.includes("files.transfer");
  const say = (t) => { st.busy = t; if (ui && ui.state) ui.state.textContent = t; };

  function render() {
    if (!ui) return;
    ui.row.hidden = !has();
    if (!has()) return;
    ui.dir.textContent = "/" + st.dir;
    ui.up.hidden = st.dir === "";
    ui.list.textContent = "";
    for (const e of st.entries) {
      const li = doc.createElement("li"), b = doc.createElement("button");
      b.type = "button";
      b.textContent = e.is_dir ? e.name + "/" : e.name + " (" + sizeText(e.size) + ")";
      b.onclick = () => (e.is_dir ? list(join(st.dir, e.name)) : download(join(st.dir, e.name)));
      li.appendChild(b); ui.list.appendChild(li);
    }
    if (st.truncated) { const li = doc.createElement("li"); li.textContent = "and more files (only the first are shown)"; ui.list.appendChild(li); }
  }

  function list(dir = st.dir) {
    if (!has()) return false;
    say("Asking the machine to show your home folder (answer the card on the machine)");
    send({ t: "file_list", dir });
    return true;
  }

  function download(path) {
    if (!has() || st.down) { if (has()) toast("One download at a time."); return false; }
    const id = st.next++;
    st.down = { id, name: path.slice(path.lastIndexOf("/") + 1), size: 0, parts: [], got: 0 };
    say("Asking the machine to send " + st.down.name + " (answer the card on the machine)");
    send({ t: "file_get", id, path });
    return true;
  }

  /** Upload `file` (a File or anything with `name`, `size` and `arrayBuffer()`) into the folder shown. */
  async function upload(file) {
    if (!has() || st.up) { toast("One upload at a time."); return false; }
    if (file.size > MAX_FILE) { toast("A file is at most 8 MB."); return false; }
    const id = st.next++;
    st.up = { id, name: file.name, bytes: new Uint8Array(await file.arrayBuffer()), at: 0 };
    say("Asking the machine to save " + file.name + " (answer the card on the machine)");
    send({ t: "file_put", id, path: join(st.dir, file.name), size: file.size });
    return true;
  }

  function sendChunks(u) {
    for (; u.at < u.bytes.length; u.at += CHUNK) {
      const part = u.bytes.subarray(u.at, Math.min(u.at + CHUNK, u.bytes.length));
      const m = new Uint8Array(part.length + 4);
      new DataView(m.buffer).setUint32(0, u.id);
      m.set(part, 4);
      sendBinary(TAG_FILE_IN, m);
    }
    send({ t: "file_end", id: u.id });
  }

  const fail = (m) => {
    say("");
    toast((WORDS[m.code] || "") + m.message, 7000);
    if (st.down && (m.id === st.down.id)) st.down = null;
    if (st.up && (m.id === st.up.id)) st.up = null;
  };

  /** A `file_*` text of the machine (already parsed). Returns true when it was one. */
  function onMessage(m) {
    if (!m || typeof m.t !== "string" || !m.t.startsWith("file_")) return false;
    switch (m.t) {
      case "file_list": st.dir = m.dir; st.entries = m.entries || []; st.truncated = !!m.truncated; say(""); render(); break;
      case "file_get_ok": if (st.down && st.down.id === m.id) { st.down.size = m.size; say("Receiving " + st.down.name); } break;
      case "file_done": if (st.down && st.down.id === m.id) {
        const d = st.down; st.down = null; say("");
        if (d.got !== d.size) toast("The file came out the wrong size and was not saved.");
        else { save(d.name, d.parts); toast("Received " + d.name + "."); }
      } break;
      case "file_put_ok": if (st.up && st.up.id === m.id) { say("Sending " + st.up.name); sendChunks(st.up); } break;
      case "file_saved": if (st.up && st.up.id === m.id) { const n = st.up.name; st.up = null; say(""); toast("Saved " + n + " on the machine."); list(st.dir); } break;
      case "file_err": fail(m); break;
      default: break;
    }
    return true;
  }

  /** A binary message; true when it was a chunk of a download (the page then does not hand it to the picture decoder). */
  function onBinary(buf) {
    const u = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
    if (u[0] !== TAG_FILE_OUT) return false;
    if (u.length < 5 || !st.down) return true;
    const id = new DataView(u.buffer, u.byteOffset, u.byteLength).getUint32(1);
    if (id !== st.down.id) return true;
    const part = u.slice(5);
    if (st.down.got + part.length > st.down.size) { send({ t: "file_cancel", id }); st.down = null; toast("More bytes came than announced: the download was stopped."); return true; }
    st.down.parts.push(part); st.down.got += part.length;
    return true;
  }

  /** The connection ended: nothing half-done is kept. */
  function reset() {
    st.down = null; st.up = null; st.entries = []; st.dir = ""; say("");
    render();
  }

  if (ui) {
    ui.up.onclick = () => list(parent(st.dir));
    ui.refresh.onclick = () => list(st.dir);
    ui.pick.onchange = () => { const f = ui.pick.files && ui.pick.files[0]; if (f) upload(f); ui.pick.value = ""; };
  }
  return { setCaps(caps) { st.caps = caps || []; render(); }, list, download, upload, onMessage, onBinary, reset, state: st };
}
