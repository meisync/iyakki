# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""Stand-ins for the few node modules the client engine's ES-module tests import (`node:test`, `node:assert/strict`, `node:fs`,
`node:crypto`), served to Chrome through an import map, so the unmodified `js/test/*.test.mjs` run in a real browser."""

TEST_JS = r"""
export const tests = [];
export function test(name, fn) { tests.push({ name, fn }); }
export default test;
export async function runAll() {
  const out = [];
  for (const t of tests) {
    try { await t.fn(); out.push({ name: t.name, ok: true }); }
    catch (e) { out.push({ name: t.name, ok: false, error: String(e && e.stack || e) }); }
  }
  return out;
}
"""

ASSERT_JS = r"""
function isObj(x) { return x !== null && typeof x === 'object'; }
function deepEq(a, b) {
  if (Object.is(a, b)) return true;
  if (!isObj(a) || !isObj(b)) return false;
  if (ArrayBuffer.isView(a) || ArrayBuffer.isView(b)) {
    if (!(ArrayBuffer.isView(a) && ArrayBuffer.isView(b)) || a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (!Object.is(a[i], b[i])) return false;
    return true;
  }
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const ka = Object.keys(a), kb = Object.keys(b);
  if (ka.length !== kb.length) return false;
  for (const k of ka) if (!Object.prototype.hasOwnProperty.call(b, k) || !deepEq(a[k], b[k])) return false;
  return true;
}
const fail = (m, dflt) => { throw new Error(m || dflt); };
const show = (v) => { try { return JSON.stringify(v); } catch { return String(v); } };
const assert = (c, m) => { if (!c) fail(m, 'assertion failed'); };
assert.ok = (c, m) => { if (!c) fail(m, 'expected a truthy value, got ' + show(c)); };
assert.equal = (a, b, m) => { if (!Object.is(a, b) && a !== b) fail(m, show(a) + ' !== ' + show(b)); };
assert.notEqual = (a, b, m) => { if (a === b) fail(m, 'both are ' + show(a)); };
assert.deepEqual = (a, b, m) => { if (!deepEq(a, b)) fail(m, show(a) + ' is not deeply equal to ' + show(b)); };
assert.match = (s, re, m) => { if (!re.test(s)) fail(m, show(s) + ' does not match ' + re); };
assert.throws = (f, _e, m) => { let t = false; try { f(); } catch { t = true; } if (!t) fail(m, 'did not throw'); };
assert.rejects = async (p, _e, m) => { let t = false; try { await (typeof p === 'function' ? p() : p); } catch { t = true; } if (!t) fail(m, 'did not reject'); };
export default assert;
"""

# a synchronous SHA-256 (FIPS 180-4), because the node test calls createHash("sha256").update(bytes).digest("hex") without awaiting
CRYPTO_JS = r"""
const K = new Uint32Array([0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2]);
export function sha256(bytes) {
  const h = new Uint32Array([0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19]);
  const n = bytes.length, padded = new Uint8Array(((n + 9 + 63) >> 6) << 6);
  padded.set(bytes); padded[n] = 0x80;
  const dv = new DataView(padded.buffer);
  dv.setUint32(padded.length - 8, Math.floor((n * 8) / 4294967296)); dv.setUint32(padded.length - 4, (n * 8) >>> 0);
  const w = new Uint32Array(64);
  const rr = (x, k) => (x >>> k) | (x << (32 - k));
  for (let o = 0; o < padded.length; o += 64) {
    for (let i = 0; i < 16; i++) w[i] = dv.getUint32(o + i * 4);
    for (let i = 16; i < 64; i++) { const s0 = rr(w[i-15],7) ^ rr(w[i-15],18) ^ (w[i-15] >>> 3), s1 = rr(w[i-2],17) ^ rr(w[i-2],19) ^ (w[i-2] >>> 10); w[i] = (w[i-16] + s0 + w[i-7] + s1) >>> 0; }
    let [a,b,c,d,e,f,g,hh] = h;
    for (let i = 0; i < 64; i++) {
      const S1 = rr(e,6) ^ rr(e,11) ^ rr(e,25), ch = (e & f) ^ (~e & g), t1 = (hh + S1 + ch + K[i] + w[i]) >>> 0;
      const S0 = rr(a,2) ^ rr(a,13) ^ rr(a,22), mj = (a & b) ^ (a & c) ^ (b & c), t2 = (S0 + mj) >>> 0;
      hh = g; g = f; f = e; e = (d + t1) >>> 0; d = c; c = b; b = a; a = (t1 + t2) >>> 0;
    }
    h[0]+=a; h[1]+=b; h[2]+=c; h[3]+=d; h[4]+=e; h[5]+=f; h[6]+=g; h[7]+=hh;
  }
  return [...h].map((x) => x.toString(16).padStart(8, '0')).join('');
}
export function createHash(alg) {
  if (alg !== 'sha256') throw new Error('only sha256');
  const parts = [];
  return { update(d) { parts.push(new Uint8Array(d.buffer ? d.buffer.slice(d.byteOffset, d.byteOffset + d.byteLength) : d)); return this; },
           digest() { const all = new Uint8Array(parts.reduce((s, p) => s + p.length, 0)); let o = 0; for (const p of parts) { all.set(p, o); o += p.length; } return sha256(all); } };
}
export const webcrypto = globalThis.crypto;
"""

# readFileSync of a URL or path, from files the runner page fetched before the test module was imported
FS_JS = r"""
const files = window.__preloaded || {};
export function readFileSync(p, enc) {
  const key = String(p instanceof URL ? p.pathname : p);
  const hit = Object.keys(files).find((k) => key.endsWith(k));
  if (!hit) throw new Error('ENOENT ' + key);
  const b = files[hit];
  // node's Buffer turns into text when JSON.parse converts it: the same here
  class Buf extends Uint8Array { toString() { return new TextDecoder().decode(this); } }
  return enc ? new TextDecoder().decode(b) : new Buf(b);
}
"""

ASSERT_JS_PATH = "/shim/assert.js"


def runner_html(test_module, preload):
    """The page that preloads `preload` (URL paths), maps the node modules to the shims, imports `test_module` and runs it."""
    import json
    return ("""<!doctype html><meta charset="utf-8"><title>esm tests</title>
<script type="importmap">{"imports": {"node:test": "/shim/test.js", "node:assert/strict": "/shim/assert.js", "node:fs": "/shim/fs.js", "node:crypto": "/shim/crypto.js"}}</script>
<script>
window.__preloaded = {};
window.__ready = (async () => {
  for (const p of %s) { const r = await fetch(p); if (!r.ok) throw new Error('preload ' + p); window.__preloaded[p] = new Uint8Array(await r.arrayBuffer()); }
  await import(%s);
  const t = await import('/shim/test.js');
  return await t.runAll();
})();
</script>""" % (json.dumps(preload), json.dumps(test_module)))


EXTRA = {
    "/shim/test.js": ("text/javascript", TEST_JS.encode()),
    "/shim/assert.js": ("text/javascript", ASSERT_JS.encode()),
    "/shim/crypto.js": ("text/javascript", CRYPTO_JS.encode()),
    "/shim/fs.js": ("text/javascript", FS_JS.encode()),
}
