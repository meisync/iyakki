// SPDX-License-Identifier: Apache-2.0
// Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

//! The web UI's tenant control (`src/webservice/ui/tenant.js`) under node with stand-ins for `fetch`-level API answers and the page:
//! what is drawn from the server's list, that a single tenant gets no drop-down, that a switch posts to the server and then clears the
//! caches and draws again, that a refused switch changes nothing, and that names are text. Skipped (passes, saying so) when `node`
//! is not installed. This is NOT a real browser.

const DRIVER: &str = r##"
const path = require("path");
const Tenant = require(path.join(process.argv[2], "tenant.js"));
const App = require(path.join(process.argv[2], "app.js"));
let bad = 0;
const ok = (c, m) => { console.log((c ? "OK  " : "BAD ") + m); if (!c) bad++; };

const node = (tag) => ({ tag, attrs: {}, kids: [], text: "", className: "", listeners: {}, value: "", disabled: false,
  setAttribute(k, v) { this.attrs[k] = v; }, appendChild(c) { this.kids.push(c); return c; },
  addEventListener(t, f) { this.listeners[t] = f; }, set textContent(v) { this.text = v; this.kids = []; }, get textContent() { return this.text; } });
const doc = { createElement: node, createTextNode: (t) => ({ text: t }) };
const el = App.make(doc);
function store(init) { const m = Object.assign({}, init); return { m, get length() { return Object.keys(m).length; }, key: (i) => Object.keys(m)[i] || null, getItem: (k) => (k in m ? m[k] : null), setItem: (k, v) => { m[k] = String(v); }, removeItem: (k) => { delete m[k]; } }; }
function fakeApi(script) {
  const log = [];
  return { log, get: async (p) => { log.push("GET " + p); return script["GET " + p].shift ? script["GET " + p].shift() : script["GET " + p]; },
    post: async (p, b) => { log.push("POST " + p + " " + JSON.stringify(b)); return script["POST " + p].shift ? script["POST " + p].shift() : script["POST " + p]; } };
}
const two = (cur) => ({ ok: true, status: 200, data: { current: cur, can_switch: true, tenants: [
  { id: "acme", name: "Acme <b>Ltd</b>", role: "member", current: cur === "acme" }, { id: "beta", name: "Beta", role: "owner", current: cur === "beta" }] } });
const one = { ok: true, status: 200, data: { current: "this-machine", can_switch: false, tenants: [{ id: "this-machine", name: "This machine", role: "admin", current: true }] } };
const find = (n, tag) => (n.tag === tag ? n : (n.kids || []).map((k) => find(k, tag)).find(Boolean));

(async () => {
  // shape: only what the server said; nothing without an id, a name and a role
  const s = Tenant.shape({ current: "b", can_switch: true, tenants: [{ id: "a", name: "A", role: "member" }, { id: "b", name: "B", role: "owner" }, { id: "x", name: "no role" }, null, { name: "no id", role: "r" }] });
  ok(s.list.length === 2 && s.current.id === "b" && s.canSwitch, "shape keeps complete entries and picks the current one");
  ok(!Tenant.shape({ can_switch: false, tenants: [{ id: "a", name: "A", role: "m", current: true }] }).canSwitch && !Tenant.shape({ can_switch: true, tenants: [{ id: "a", name: "A", role: "m" }] }).canSwitch, "no switch with one tenant, even if told");
  ok(Tenant.shape(null).current === null && Tenant.shape({}).list.length === 0, "an empty answer draws nothing");
  ok(Tenant.failText({ status: 404 }).includes("not available") && Tenant.failText({ status: 429 }).includes("Too many") && Tenant.failText({ status: 400, message: "m" }) === "m", "failure words");

  // one tenant: name and badge, no drop-down
  let slot = node("span");
  let api = fakeApi({ "GET /api/v1/tenants": one });
  const cur = await Tenant.mount({ api, el, slot, storage: store({}) });
  ok(cur.id === "this-machine" && slot.kids.length === 2 && slot.kids[0].text === "This machine" && slot.kids[1].text === "admin" && !find(slot, "select"), "one tenant: name and role badge, nothing to switch");

  // several: a drop-down with an accessible name, names as text
  slot = node("span");
  const st = store({ "iyakki.token": "tok", "iyakki.cache.files": "x", "iyakki.cache.flows": "y", other: "z" });
  const said = [], changed = [], switched = []; let where = null;
  api = fakeApi({ "GET /api/v1/tenants": [two("acme"), two("beta")], "POST /api/v1/tenant/switch": { ok: true, status: 200, data: { switched: true, tenant: { id: "beta", name: "Beta", role: "owner" }, url: null } } });
  Tenant.onChange((t) => changed.push(t.id));
  await Tenant.mount({ api, el, slot, say: (t, k) => said.push([t, k]), storage: st, location: { assign: (u) => { where = u; } }, onSwitched: async (t) => { switched.push(t.id); } });
  const sel = find(slot, "select");
  ok(sel && sel.attrs["aria-label"] === "Switch tenant" && sel.kids.length === 2 && sel.value === "acme", "a drop-down with a name, on the current tenant");
  ok(slot.kids[0].text === "Acme <b>Ltd</b>" && sel.kids[0].text === "Acme <b>Ltd</b> (member)", "the tenant name is set as text, never as markup");
  sel.value = "beta";
  await sel.listeners.change();
  ok(api.log.includes('POST /api/v1/tenant/switch {"tenant":"beta"}'), "the switch is asked of the server with the id");
  ok(!("iyakki.cache.files" in st.m) && !("iyakki.cache.flows" in st.m) && st.m["iyakki.token"] === "tok" && st.m.other === "z", "caches under our prefix are cleared; the token and others stay");
  ok(changed.join() === "beta" && switched.join() === "beta" && where === null, "pages are told, the page is drawn again, no navigation for a tenant served here");
  ok(slot.kids[0].text === "Beta" && slot.kids[1].text === "owner", "the bar now shows the new tenant from the API");
  ok(said.some((x) => x[0].includes("Beta") && x[1] === "ok"), "the person is told");

  // a refused switch changes nothing and draws what is true now
  slot = node("span");
  const st2 = store({ "iyakki.cache.files": "x" });
  const said2 = []; let sw2 = 0;
  api = fakeApi({ "GET /api/v1/tenants": [two("acme"), two("acme")], "POST /api/v1/tenant/switch": { ok: false, status: 404, data: null, message: "x" } });
  await Tenant.mount({ api, el, slot, say: (t, k) => said2.push([t, k]), storage: st2, onSwitched: async () => { sw2++; } });
  const sel2 = find(slot, "select"); sel2.value = "beta"; await sel2.listeners.change();
  ok(st2.m["iyakki.cache.files"] === "x" && sw2 === 0 && said2[0][1] === "err" && said2[0][0].includes("not available"), "a 404 leaves caches and page alone and says why");
  ok(api.log.filter((l) => l === "GET /api/v1/tenants").length === 2, "the list is read again after a refusal");

  // another address: navigate there, do not draw here
  slot = node("span"); where = null;
  api = fakeApi({ "GET /api/v1/tenants": two("acme"), "POST /api/v1/tenant/switch": { ok: true, status: 200, data: { switched: true, tenant: { id: "beta", name: "Beta", role: "owner" }, url: "https://beta.example.org/ui/" } } });
  await Tenant.mount({ api, el, slot, storage: store({}), location: { assign: (u) => { where = u; } } });
  const sel3 = find(slot, "select"); sel3.value = "beta"; await sel3.listeners.change();
  ok(where === "https://beta.example.org/ui/", "a tenant at another address is opened there");
  slot = node("span"); where = null;
  api = fakeApi({ "GET /api/v1/tenants": two("acme"), "POST /api/v1/tenant/switch": { ok: true, status: 200, data: { switched: true, tenant: { id: "beta", name: "Beta", role: "owner" }, url: "http://evil.example/" } } });
  await Tenant.mount({ api, el, slot, storage: store({}), location: { assign: (u) => { where = u; } } });
  const sel4 = find(slot, "select"); sel4.value = "beta"; await sel4.listeners.change();
  ok(where === null, "only an https address is ever followed");

  // errors and sign-out
  slot = node("span");
  ok((await Tenant.mount({ api: fakeApi({ "GET /api/v1/tenants": { ok: false, status: 404, data: null } }), el, slot })) === null && slot.kids.length === 0, "no tenant route: no control");
  const st5 = store({ "iyakki.cache.a": "1", "iyakki.token": "t" }); slot.kids.push(1);
  Tenant.clear(slot, st5);
  ok(slot.kids.length === 0 && !("iyakki.cache.a" in st5.m) && st5.m["iyakki.token"] === "t", "sign-out empties the control and the caches");

  console.log(bad ? "FAILED " + bad : "ALL OK");
  process.exit(bad ? 1 : 0);
})();
"##;

#[test]
fn the_tenant_control_shows_the_tenant_switches_through_the_server_and_clears_what_it_cached() {
    let ok = std::process::Command::new("node").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP: node is not installed");
        return;
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/webservice/ui");
    let dir = std::env::temp_dir().join(format!("iyakki-webui-tenant-js-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["app.js", "tenant.js"] {
        std::fs::copy(src.join(f), dir.join(f)).unwrap();
    }
    std::fs::write(dir.join("driver.js"), DRIVER).unwrap();
    let out = std::process::Command::new("node").arg(dir.join("driver.js")).arg(&dir).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success() && !text.contains("BAD"), "{text}");
    assert!(text.contains("ALL OK"), "{text}");
}
