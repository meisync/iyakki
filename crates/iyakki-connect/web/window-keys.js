// The device key of the window program. EXPERIMENTAL.
//
// The instance serves its own copy of this file at /connect/keys.js (the key of a browser tab). When the page is opened by
// `iyakki-connect window`, the local program serves THIS file at the same address instead, with the same exports: the private key stays in the
// program, and this page only asks it to sign (POST /local/sign) over the loopback channel that the launch token and a cookie protect.
// Nothing here can read the key, and nothing is kept in the browser (its profile is a temporary folder that is deleted afterwards).

import { unhex } from "/connect/engine.js";

let csrf = "";

async function local(path, body) {
  const init = body === undefined
    ? { cache: "no-store", credentials: "same-origin" }
    : { method: "POST", cache: "no-store", credentials: "same-origin", headers: { "content-type": "application/json", "x-csrf": csrf }, body: JSON.stringify(body) };
  const r = await fetch("/local/" + path, init);
  if (!r.ok) throw new Error("local " + r.status);
  return r.json();
}

async function info() { const j = await local("device"); csrf = j.csrf; return j; }

export const storageAvailable = () => true;

/** The device the program holds for this instance, or null: { mode, public, device, name, priv }. */
export async function loadDevice() {
  try { const j = await info(); return j.public ? { mode: "webcrypto", public: j.public, device: j.device || null, name: "This computer", priv: null } : null; } catch { return null; }
}
/** The page tells the program the device id the instance gave; the program checks it against what the instance sent. */
export async function saveDevice(rec) { await local("device", { device: rec.device }); }
export async function forgetDevice() { try { await local("forget", {}); } catch { /* nothing to forget */ } }

export async function webCryptoEd25519() { return true; }

/** The program's key (a link is being used, so the program made a new one that it keeps once the instance has enrolled it). */
export async function newKey(_engine, name) {
  const j = await info();
  return { mode: "webcrypto", public: j.public, device: null, name, priv: null };
}

/** The program signs `text` after checking that it is a sign-in for a challenge it saw come from the instance. 128 hex digits. */
export async function signText(_rec, text) { return (await local("sign", { text })).sig; }

/** The page tells the person the truth about where the key is: here it is held by the program, not by the browser. */
export const heldByProgram = true;

export const describeKey = () => "Key: held by the iyakki-connect program on this computer, not by this page.";

export { unhex };
