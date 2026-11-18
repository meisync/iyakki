//! The audit trail section of the Cloud access page (`src/webservice/ui/cloud.js`) under node: the newest lines first, a page of 20,
//! an "Older entries" button while the service says more follow, the next page asked from the right offset and appended, nothing
//! offered to a plain member. Same method as `webui_schedules_js.rs`: skipped (passes, saying so) when `node` is not installed.
//! NOT a real browser.

const DRIVER: &str = r##"
const path = require("path");
const dir = process.argv[2];
const App = require(path.join(dir, "app.js"));
const Cloud = require(path.join(dir, "cloud.js"));
let bad = 0;
const ok = (c, m) => { console.log((c ? "OK  " : "BAD ") + m); if (!c) bad++; };

function node(tag) {
  return { tag, attrs: {}, kids: [], _t: "", className: "", listeners: {}, value: "",
    setAttribute(k, v) { this.attrs[k] = v; }, appendChild(c) { this.kids.push(c); return c; }, addEventListener(t, f) { this.listeners[t] = f; },
    contains() { return false; },
    set textContent(v) { this._t = v; this.kids = []; }, get textContent() { return this._t; } };
}
const doc = { createElement: node, createTextNode: (t) => ({ tag: "#text", _t: t, kids: [] }), body: node("body"), hidden: false, activeElement: null };
const walk = (n, f) => { f(n); for (const k of n.kids || []) walk(k, f); };
const all = (n) => { const out = []; walk(n, (x) => out.push(x)); return out; };
const texts = (n) => all(n).map((x) => x._t || "").join(" | ");
const buttons = (n, label) => all(n).filter((x) => x.tag === "button" && x._t === label);
const el = App.make(doc);

const lines = (from, to) => { const out = []; for (let i = from; i > to; i--) out.push({ time: 1700000000 + i * 60, change: "tenant: olga change " + i }); return out; };
function ctxFor(calls, view, pages) {
  return { el, doc, me: { profile: "cloud", admin: true }, say: (t, k) => { calls.say = [t, k]; }, poll: () => {}, messageFor: App.messageFor,
    api: { get: async (p) => {
      calls.push(["GET", p]);
      if (p === "/api/v1/tenant") return { ok: true, status: 200, data: view };
      if (p.startsWith("/api/v1/namespaces/")) return { ok: true, status: 200, data: { members: [] } };
      if (p.startsWith("/api/v1/tenant/audit")) {
        const q = new URLSearchParams(p.split("?")[1]);
        const off = Number(q.get("offset")), lim = Number(q.get("limit"));
        const total = pages.total, page = lines(total - off, Math.max(total - off - lim, 0));
        return { ok: true, status: 200, data: { lines: page, total, offset: off, limit: lim, more: off + page.length < total } };
      }
      return { ok: false, status: 404, message: "Not found", data: null };
    }, post: async () => ({ ok: true, status: 200, data: {} }) } };
}

(async () => {
  const admin = { user: "olga", tenant_role: "owner", members: [], namespaces: [] };
  {
    const calls = [], host = node("div");
    await Cloud.page().render(ctxFor(calls, admin, { total: 45 }), host);
    ok(calls.some((c) => c[1] === "/api/v1/tenant/audit?limit=20&offset=0"), "the first page asks for 20 lines from the newest");
    const t = texts(host);
    ok(t.includes("Audit trail") && t.includes("olga change 45") && t.includes("olga change 26") && !t.includes("olga change 25"), "twenty lines, newest first");
    const first = all(host).filter((x) => x.className === "mono" || (x.attrs && x.attrs.class === "mono")).map((x) => x._t);
    ok(first.length === 20 && first[0].includes("change 45") && first[19].includes("change 26"), "in order, newest on top");
    const older = buttons(host, "Older entries");
    ok(older.length === 1, "an Older entries button while more follow");
    await older[0].listeners.click();
    ok(calls.some((c) => c[1] === "/api/v1/tenant/audit?limit=20&offset=20"), "the next page starts after the lines already shown");
    const t2 = texts(host);
    ok(t2.includes("olga change 25") && t2.includes("olga change 6") && !t2.includes("olga change 5 "), "the second page is appended");
    await buttons(host, "Older entries")[0].listeners.click();
    ok(calls.some((c) => c[1] === "/api/v1/tenant/audit?limit=20&offset=40"), "and the third");
    ok(buttons(host, "Older entries").length === 0 && texts(host).includes("olga change 1"), "no button once the oldest line is shown");
  }
  {
    const calls = [], host = node("div");
    await Cloud.page().render(ctxFor(calls, admin, { total: 0 }), host);
    ok(texts(host).includes("No role changes recorded yet.") && buttons(host, "Older entries").length === 0, "an empty trail says so");
  }
  {
    const calls = [], host = node("div");
    await Cloud.page().render(ctxFor(calls, { user: "ann", tenant_role: "member", members: [], namespaces: [] }, { total: 45 }), host);
    ok(!calls.some((c) => String(c[1]).includes("/audit")) && !texts(host).includes("Audit trail"), "a plain member neither asks for the trail nor sees the section");
  }
  console.log(bad ? "FAILED " + bad : "ALL OK");
  process.exit(bad ? 1 : 0);
})();
"##;

#[test]
fn the_audit_trail_section_pages_newest_first_and_is_for_administrators_only_under_node() {
    let ok = std::process::Command::new("node").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP: node is not installed");
        return;
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/webservice/ui");
    let dir = std::env::temp_dir().join(format!("iyakki-webui-cloud-audit-js-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["app.js", "common.js", "cloud.js"] {
        std::fs::copy(src.join(f), dir.join(f)).unwrap();
    }
    std::fs::write(dir.join("driver.js"), DRIVER).unwrap();
    let out = std::process::Command::new("node").arg(dir.join("driver.js")).arg(&dir).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success() && !text.contains("BAD"), "{text}");
    assert!(text.contains("ALL OK"), "{text}");
}
