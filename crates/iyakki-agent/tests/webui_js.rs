//! The web UI's JavaScript (`src/webservice/ui/*.js`) under node with stand-ins for `fetch` and the page: the API client, the
//! error texts, the page choice, and what each page offers a role (the server still decides). Skipped (passes, saying so) when
//! `node` is not installed. Nothing is installed or downloaded; the driver is written to a temporary folder and removed after.
//! This is NOT a real browser.

const DRIVER: &str = r##"
const path = require("path");
const dir = process.argv[2];
const App = require(path.join(dir, "app.js"));
const Cloud = require(path.join(dir, "cloud.js"));
const Remote = require(path.join(dir, "remote.js"));
let bad = 0;
const ok = (c, m) => { console.log((c ? "OK  " : "BAD ") + m); if (!c) bad++; };

function fakeDoc() {
  const node = (tag) => ({ tag, attrs: {}, kids: [], text: "", className: "", listeners: {},
    setAttribute(k, v) { this.attrs[k] = v; }, appendChild(c) { this.kids.push(c); return c; },
    addEventListener(t, f) { this.listeners[t] = f; }, set textContent(v) { this.text = v; this.kids = []; }, get textContent() { return this.text; } });
  return { createElement: node, createTextNode: (t) => ({ text: t }) };
}
function fakeStorage() { const m = {}; return { getItem: (k) => (k in m ? m[k] : null), setItem: (k, v) => { m[k] = String(v); }, removeItem: (k) => { delete m[k]; }, m }; }
function fakeFetch(answers, log) {
  return async (url, init) => {
    log.push({ url, init });
    const a = answers[init.method + " " + url];
    if (a === "throw") throw new Error("offline");
    const [status, body] = a || [404, { error: { code: "not_found", message: "not found" } }];
    return { status, json: async () => body };
  };
}

(async () => {
  // errors read the same everywhere
  ok(App.messageFor(0, null).includes("Cannot reach"), "offline text");
  ok(App.messageFor(403, { error: { message: "no access" } }) === "Not allowed: no access", "403 shows the server's reason");
  ok(App.messageFor(403, null) === "Not allowed for your role.", "403 without a body");
  ok(App.messageFor(400, { error: { message: "bad port" } }) === "bad port", "400 shows the server's message");
  ok(App.messageFor(500, null).includes("500"), "500 does not show internals");

  // page choice
  const pages = [{ id: "cloud" }, { id: "remote" }];
  ok(App.pageFromHash("#/remote", pages).id === "remote", "hash picks a page");
  ok(App.pageFromHash("#/nope", pages).id === "cloud" && App.pageFromHash("", pages).id === "cloud", "unknown or empty falls back to the first");
  ok(App.pageFromHash("#/cloud", []) === null, "no page available");

  // el(): text is text, never markup
  const el = App.make(fakeDoc());
  const n = el("p", { text: "<img src=x onerror=alert(1)>", class: "a", hidden: false, disabled: true, onclick: () => {} }, ["x", null, false]);
  ok(n.text === "<img src=x onerror=alert(1)>" && n.tag === "p" && n.kids.length === 1 && n.attrs.disabled === "" && !("hidden" in n.attrs) && typeof n.listeners.click === "function", "el sets text as text and skips empty attributes");

  // the client: token header, no cookies, sign-in keeps the token, 401 signs out, offline is a message
  const st = fakeStorage(), log = [];
  const api = App.createApi({ storage: st, fetch: fakeFetch({
    "POST /api/v1/login": [200, { token: "tok1", user: "root" }],
    "GET /api/v1/whoami": [200, { user: "root" }],
    "GET /api/v1/tenant": [401, { error: { code: "unauthorized", message: "x" } }],
    "GET /api/v1/webaccess": [403, { error: { code: "denied", message: "administrators only" } }],
    "GET /api/v1/off": "throw",
  }, log) });
  let outs = 0; api.setSignedOut(() => { outs++; });
  ok(!api.hasToken(), "no token before sign-in");
  const bad1 = await api.login("root", "x"); ok(bad1.ok && api.hasToken() && st.m[App.TOKEN_KEY] === "tok1", "sign-in keeps the token in the tab's storage");
  ok(log[0].init.headers.Authorization === undefined && log[0].init.credentials === "omit", "the sign-in call sends no token and no cookies");
  await api.get("/api/v1/whoami"); ok(log[1].init.headers.Authorization === "Bearer tok1", "later calls send the bearer token");
  const r403 = await api.get("/api/v1/webaccess"); ok(!r403.ok && r403.status === 403 && r403.message === "Not allowed: administrators only" && api.hasToken(), "a 403 is shown, the session stays");
  const off = await api.get("/api/v1/off"); ok(off.status === 0 && off.message.includes("Cannot reach"), "offline is a message, not a crash");
  const r401 = await api.get("/api/v1/tenant"); ok(r401.status === 401 && !api.hasToken() && outs === 1, "a 401 ends the session and shows the sign-in");
  const body = await api.post("/api/v1/webaccess", { on: true }); ok(log[log.length - 1].init.body === '{"on":true}' && log[log.length - 1].init.headers["Content-Type"] === "application/json", "posts JSON");

  // cloud page: what each role is offered
  const owner = { tenant_role: "owner" }, admin = { tenant_role: "admin" }, member = { tenant_role: "member" };
  ok(JSON.stringify(Cloud.tenantRoleChoices(owner)) === '["member","admin","owner"]', "an owner may make owners");
  ok(JSON.stringify(Cloud.tenantRoleChoices(admin)) === '["member","admin"]', "an administrator may not offer owner");
  ok(Cloud.tenantRoleChoices(member).length === 0 && !Cloud.canManageNamespaces(member) && Cloud.canManageNamespaces(admin), "a member manages nothing");
  ok(Cloud.canManageNsMembers({ joined: true, role: "admin" }) && !Cloud.canManageNsMembers({ joined: true, role: "editor" }) && !Cloud.canManageNsMembers({ joined: false, role: null }), "only a namespace administrator manages its members");
  ok(Cloud.canJoin(admin, { joined: false }) && !Cloud.canJoin(member, { joined: false }) && !Cloud.canJoin(admin, { joined: true }), "only a tenant administrator may join, and only a namespace they are not in");
  ok(Cloud.validName("ann") && !Cloud.validName("") && !Cloud.validName("a b") && !Cloud.validName("a/b") && !Cloud.validName("x".repeat(65)), "names");
  ok(Cloud.rotationText({ rotation: { rotated: ["a"], pending: ["tenant"] } }).includes("1 pending") && Cloud.rotationText({}) === "", "rotation sentence");
  const cp = Cloud.page(); ok(cp.available({ profile: "cloud" }) && !cp.available({ profile: "agent-api" }) && !cp.available(null), "Cloud access exists on a pod only");
  const rp = Remote.page(); ok(rp.available({ profile: "agent-api", admin: true }) && !rp.available({ profile: "agent-api", admin: false }) && !rp.available({ profile: "cloud", admin: true }), "Remote access is for administrators of a machine with a desktop");

  // remote page
  ok(Remote.parsePort("").port === 0 && Remote.parsePort(" 9443 ").port === 9443, "port: empty is the default");
  ok(!Remote.parsePort("80").ok && !Remote.parsePort("abc").ok && !Remote.parsePort("70000").ok && !Remote.parsePort("-1").ok && !Remote.parsePort("12345678").ok, "port: refuses low, non-numeric and too large");
  ok(Remote.fingerprintLines(Array.from({ length: 32 }, (_, i) => i.toString(16).padStart(2, "0")).join(":")).length === 4 && Remote.fingerprintLines(null).length === 0, "fingerprint in lines of eight");
  ok(Remote.stateWord({ listening: false, error: "", on: false }).cls === "on" && Remote.stateWord({ listening: false, error: "in use", on: true, status: "not listening: in use" }).cls === "err", "state words");

  console.log(bad ? "FAILED " + bad : "ALL OK");
  process.exit(bad ? 1 : 0);
})();
"##;

#[test]
fn the_web_ui_code_follows_its_rules_under_node() {
    let ok = std::process::Command::new("node").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP: node is not installed");
        return;
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/webservice/ui");
    let dir = std::env::temp_dir().join(format!("iyakki-webui-js-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["app.js", "cloud.js", "remote.js"] {
        std::fs::copy(src.join(f), dir.join(f)).unwrap();
    }
    std::fs::write(dir.join("driver.js"), DRIVER).unwrap();
    let out = std::process::Command::new("node").arg(dir.join("driver.js")).arg(&dir).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success() && !text.contains("BAD"), "{text}");
    assert!(text.contains("ALL OK"), "{text}");
}
