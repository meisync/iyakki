// Checks a built client engine module: no imports at all (nothing can reach outside the sandbox), the exports the glue calls, and a size
// limit. With a second file, also compares the two modules' export lists (the committed one must match a fresh build).
import { readFileSync } from "node:fs";
const WANT = ["memory", "iy_alloc", "iy_free", "iy_init", "iy_cmd", "iy_on_text", "iy_on_binary", "iy_next_out", "iy_reply_ptr", "iy_fb_ptr", "iy_fb_w", "iy_fb_h", "iy_fb_version"];
const LIMIT = 600 * 1024;
const info = (f) => {
  const bytes = readFileSync(f);
  const m = new WebAssembly.Module(bytes);
  return { size: bytes.length, imports: WebAssembly.Module.imports(m).map((i) => i.module + "." + i.name), exports: WebAssembly.Module.exports(m).map((e) => e.name).filter((n) => n === "memory" || n.startsWith("iy_")).sort() };
};
const [a, b] = process.argv.slice(2);
const x = info(a);
const fail = (msg) => { console.error("client engine check failed: " + msg); process.exit(1); };
if (x.imports.length) fail("the module imports " + x.imports.join(", "));
for (const w of WANT) if (!x.exports.includes(w)) fail("missing export " + w);
if (x.size > LIMIT) fail("module is " + x.size + " bytes, limit " + LIMIT);
if (b) {
  const y = info(b);
  if (JSON.stringify(x.exports) !== JSON.stringify(y.exports)) fail("the committed module's exports differ from a fresh build; run dist/client-engine/build.sh");
}
const same = b && readFileSync(a).equals(readFileSync(b));
console.log("client engine ok: " + x.size + " bytes, no imports, " + x.exports.length + " exports" + (b ? (same ? ", committed module is byte for byte a fresh build" : ", committed module differs from a fresh build in bytes (same exports; toolchain change?)") : ""));
