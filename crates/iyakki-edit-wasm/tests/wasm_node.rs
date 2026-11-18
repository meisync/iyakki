//! The compiled WebAssembly module of the editor core loaded by node and driven the way a page would drive it (F-WEB-05): text
//! written into the module's memory, a buffer edited and undone, search, highlighting and a diff read back as JSON, the module with
//! no imports at all. It needs the `.wasm` built with `cargo build --release --target wasm32-unknown-unknown -p iyakki-edit-wasm`
//! (its path in `IYAKKI_EDIT_WASM`, else the target folder's usual place); without node or without the file the test passes saying
//! it was skipped. NOT a browser.

const DRIVER: &str = r##"
const fs = require("fs");
let bad = 0;
const ok = (c, m) => { console.log((c ? "OK  " : "BAD ") + m); if (!c) bad++; };
(async () => {
  const bytes = fs.readFileSync(process.argv[2]);
  const mod = await WebAssembly.compile(bytes);
  ok(WebAssembly.Module.imports(mod).length === 0, "the module needs no imports");
  const x = (await WebAssembly.instantiate(mod, {})).exports;
  const enc = new TextEncoder(), dec = new TextDecoder();
  const mem = () => new Uint8Array(x.memory.buffer);
  function put(s) { const b = enc.encode(s); const p = x.ed_alloc(b.length); mem().set(b, p); return [p, b.length]; }
  const res = (n) => dec.decode(mem().slice(x.ed_result(), x.ed_result() + n));
  const text = (h) => res(x.ed_text(h));
  let [p, n] = put("let a = 1;\nlet b = \"é\";\n");
  const h = x.ed_new(p, n);
  ok(h > 0 && text(h) === "let a = 1;\nlet b = \"é\";\n", "a buffer from text (multi-byte characters survive)");
  ok(x.ed_cursor(h, 0, 3) === 0, "the cursor is placed");
  [p, n] = put("tt");
  ok(x.ed_insert(h, p, n) === 0 && text(h).startsWith("lettt a = 1;"), "text is inserted at the cursor");
  ok(x.ed_undo(h) === 0 && text(h).startsWith("let a = 1;") && x.ed_undo(h) === 1, "undo, and a refusal when there is nothing left");
  ok(x.ed_redo(h) === 0 && text(h).startsWith("lettt"), "redo");
  let [q, qn] = put("LET");
  let len = x.ed_find_all(h, q, qn, 0);
  ok(JSON.parse(res(len)).length === 2, "case-insensitive search finds both lines");
  let [l, ln] = put("main.rs");
  len = x.ed_highlight(h, l, ln, 1);
  const spans = JSON.parse(res(len));
  ok(spans.some((s) => s[2] === "String") && spans.some((s) => s[2] === "Keyword"), "a line is highlighted with named kinds");
  const [a, an] = put("one\ntwo\n"), [b, bn] = put("one\n2\n");
  const d = JSON.parse(res(x.ed_diff(a, an, b, bn)));
  ok(d.length === 1 && d[0].old[0] === "two" && d[0].new[0] === "2", "a diff comes back as hunks");
  ok(x.ed_new(0, 5) === 0, "an address of nothing makes no buffer");
  x.ed_free(h);
  ok(x.ed_has(h) === 0 && x.ed_insert(h, p, n) === 1, "a freed buffer is gone and refuses edits");
  console.log(bad ? "FAILED " + bad : "ALL OK");
  process.exit(bad ? 1 : 0);
})();
"##;

fn wasm_path() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("IYAKKI_EDIT_WASM") {
        return Some(p.into());
    }
    let target = std::env::var("CARGO_TARGET_DIR").map(std::path::PathBuf::from).unwrap_or_else(|_| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    ["release", "debug"].iter().map(|p| target.join("wasm32-unknown-unknown").join(p).join("iyakki_edit_wasm.wasm")).find(|p| p.exists())
}

#[test]
fn the_compiled_module_is_driven_from_javascript_like_a_page_would() {
    let node = std::process::Command::new("node").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    let Some(wasm) = wasm_path() else {
        eprintln!("SKIP: build the module first (cargo build --release --target wasm32-unknown-unknown -p iyakki-edit-wasm)");
        return;
    };
    if !node {
        eprintln!("SKIP: node is not installed");
        return;
    }
    let dir = std::env::temp_dir().join(format!("iyakki-edit-wasm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("driver.js"), DRIVER).unwrap();
    let out = std::process::Command::new("node").arg(dir.join("driver.js")).arg(&wasm).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success() && !text.contains("BAD"), "{text}");
    assert!(text.contains("ALL OK"), "{text}");
}
