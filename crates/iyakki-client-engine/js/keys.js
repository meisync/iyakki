// The device key of a browser client. EXPERIMENTAL.
//
// Preferred: WebCrypto Ed25519, generated non-extractable, kept in IndexedDB as a CryptoKey. Script on this page can ask it to sign while the
// page is open, but nothing can read the private key out of the browser.
// Fallback (the browser has no Ed25519 in WebCrypto, or the page is not a secure context such as plain http on a local network): a 32-byte seed
// from crypto.getRandomValues kept in IndexedDB, signed by the engine module. Any script of this origin can read it, so the page says "weaker".
// The record also holds the device id the machine gave at enrolment. One record per origin (that is, per IYAKKI instance).

import { hex, unhex, randomHex } from "./engine.js";

const DB = "iyakki-client", STORE = "kv", KEY = "device";

function db() {
  return new Promise((res, rej) => {
    const r = indexedDB.open(DB, 1);
    r.onupgradeneeded = () => r.result.createObjectStore(STORE);
    r.onsuccess = () => res(r.result);
    r.onerror = () => rej(r.error);
  });
}

async function tx(mode, f) {
  const d = await db();
  return new Promise((res, rej) => {
    const t = d.transaction(STORE, mode), s = t.objectStore(STORE), q = f(s);
    t.oncomplete = () => { d.close(); res(q && q.result); };
    t.onerror = t.onabort = () => { d.close(); rej(t.error); };
  });
}

export const storageAvailable = () => typeof indexedDB !== "undefined";

/** The saved record, or null: { mode: "webcrypto"|"seed", public, device, name, priv?, seed? }. */
export async function loadDevice() { try { return (await tx("readonly", (s) => s.get(KEY))) || null; } catch { return null; } }
export async function saveDevice(rec) { await tx("readwrite", (s) => s.put(rec, KEY)); }
export async function forgetDevice() { try { await tx("readwrite", (s) => s.delete(KEY)); } catch { /* nothing to forget */ } }

/** Does this browser sign Ed25519 with WebCrypto here? (Only in a secure context, and only in current browsers.) */
export async function webCryptoEd25519() {
  try {
    if (!globalThis.isSecureContext || !crypto.subtle) return false;
    const k = await crypto.subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"]);
    return !!(k && k.privateKey && k.publicKey);
  } catch { return false; }
}

/** Make a new key. `engine` is only used for the fallback (the public key of the seed). The record is not saved until the device is enrolled. */
export async function newKey(engine, name) {
  if (await webCryptoEd25519()) {
    const pair = await crypto.subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"]);
    const pub = hex(new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey)));
    return { mode: "webcrypto", public: pub, priv: pair.privateKey, device: null, name };
  }
  const seed = randomHex(32);
  return { mode: "seed", public: engine.publicFromSeed(seed), seed, device: null, name };
}

/** Sign `text` (the string from the engine's begin_sign) with a webcrypto record; returns 128 hex digits. */
export async function signText(rec, text) {
  const sig = await crypto.subtle.sign({ name: "Ed25519" }, rec.priv, new TextEncoder().encode(text));
  return hex(new Uint8Array(sig));
}

export const describeKey = (rec) => rec.mode === "webcrypto"
  ? "Key: held by the browser, cannot be read out of it."
  : "Key: kept in this browser's storage where any script of this page could read it (weaker). Use a current browser over https for a stronger key.";

export { unhex };
