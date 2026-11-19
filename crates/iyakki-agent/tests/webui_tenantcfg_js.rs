//! The logic of the tenant settings page (`src/webservice/ui/tenantcfg.js`) under node: state badges, the secret box (never echoes),
//! the acknowledgement list sent as `ack_warnings`, the Save gate, the k3s matrix, the page gate, and a page drawn against a fake API
//! (an approval is only sent by the person's own click; a typed secret leaves the box after saving). Skipped (passes, saying so)
//! when `node` is not installed. This is NOT a real browser.

const DRIVER: &str = r##"
const path = require("path");
const dir = process.argv[2];
const App = require(path.join(dir, "app.js"));
const T = require(path.join(dir, "tenantcfg.js"));
let bad = 0;
const ok = (c, m) => { console.log((c ? "OK  " : "BAD ") + m); if (!c) bad++; };
const j = JSON.stringify;

(async () => {
  // state badges
  ok(T.stateBadge({ state: "ready", source: "api" }).cls === "ok" && T.stateBadge({ state: "secret_missing" }).text === "secret missing" && T.stateBadge({ state: "vault_locked" }).text === "vault locked" && T.stateBadge({ state: "disabled" }).cls === "muted" && T.stateBadge({ state: "invalid" }).cls === "err", "state badges");
  ok(T.stateBadge({ state: "ready", source: "file", managed: true }).locked && T.stateBadge({ state: "ready", source: "file" }).source === "file" && !T.stateBadge({ state: "ready", source: "api" }).locked && T.stateBadge({ state: "weird" }).cls === "muted" && T.stateBadge(null).text === "unknown", "source and managed lock");
  // the secret box never echoes
  ok(T.secretLabel("[set]") === "[set]" && T.secretLabel("[vault locked]") === "[vault locked]" && T.secretLabel("[not set]") === "[not set]" && T.secretLabel("vault:team/x/key") === "[not set]" && T.secretLabel("sk-realkey123456789") === "[not set]", "secret label only ever shows the state");
  const secretField = { name: "token", type: "secret_ref agent.claude", secret: true };
  ok(T.toInput(secretField, "sk-realkey123456789") === "" && T.toInput(secretField, "[set]") === "", "a secret field is never filled in");
  // field types and conversion
  ok(T.fieldType({ type: "bool" }).kind === "bool" && T.fieldType({ type: "int 0..100" }).max === 100 && T.fieldType({ type: "one of a|b" }).options.length === 2 && T.fieldType({ type: "list(Host) <= 5 x 80" }).kind === "list" && T.fieldType(secretField).kind === "secret" && T.fieldType({ type: "text(Plain) <= 5" }).kind === "text", "field types");
  ok(j(T.fromInput({ type: "list(Host) <= 5 x 80" }, "a.example\n b.example, ")) === '["a.example","b.example"]' && T.fromInput({ type: "int 0..9" }, "7") === 7 && T.fromInput({ type: "int 0..9" }, "x") === "x" && T.fromInput({ type: "text" }, "  ") === undefined && T.fromInput({ type: "bool" }, "", true) === true, "inputs to values");
  // body building: secrets only in `secrets`, only when typed
  const fields = [{ name: "mode", type: "one of per_person|shared" }, { name: "terms_ack", type: "bool" }, secretField, { name: "allowed_models", type: "list(Model) <= 5 x 40" }];
  const warnings = [{ code: "data.leaves", severity: "warn" }, { code: "models.unrestricted", severity: "warn" }, { code: "info.key.scope", severity: "info" }];
  const form = { enabled: true, values: { mode: "shared", terms_ack: true, allowed_models: "claude-*" }, secrets: { token: "sk-typed-secret-123456" }, namespace: "web", name: "", warnings };
  const b = T.buildBody(fields, form, { "data.leaves": true });
  ok(b.secrets.token === "sk-typed-secret-123456" && !("token" in b.values) && !j(b.values).includes("sk-typed") && b.namespace === "web" && !("name" in b) && b.values.terms_ack === true && j(b.values.allowed_models) === '["claude-*"]', "the typed secret goes only into secrets");
  ok(j(b.ack_warnings) === '["data.leaves"]', "only ticked warnings are acknowledged");
  const b2 = T.buildBody(fields, Object.assign({}, form, { secrets: { token: "" } }), {});
  ok(j(b2.secrets) === "{}" && j(b2.ack_warnings) === "[]", "an empty secret box sends nothing (the stored reference stays)");
  // acknowledgements
  ok(j(T.ackNeeded(warnings)) === '["data.leaves","models.unrestricted"]', "every warn needs a tick, info does not");
  ok(j(T.ackList(warnings, { "data.leaves": true, "models.unrestricted": true, "gone.code": true })) === '["data.leaves","models.unrestricted"]', "a tick for a warning that is no longer listed is dropped");
  ok(!T.canSave(form, warnings, { "data.leaves": true }) && T.canSave(form, warnings, { "data.leaves": true, "models.unrestricted": true }) && T.canSave({ enabled: false }, warnings, {}), "Save needs every tick only when switching on");
  ok(!T.canSave(form, warnings.concat([{ code: "spend.cap.missing", severity: "block" }]), { "data.leaves": true, "models.unrestricted": true }) && T.hasBlock([{ severity: "block" }]), "a block cannot be saved");
  ok(T.sortWarnings(warnings.concat([{ code: "a", severity: "block" }]))[0].code === "a", "blocks come first");
  // matrix
  const mx = T.k3sMatrix([{ kind: "k3s.cluster", namespace: "web", name: "prod", state: "ready" }, { kind: "k3s.cluster", namespace: "web", name: "dev" }, { kind: "k3s.cluster", namespace: "api", name: "qa" }, { kind: "agent.claude", name: "default" }], null);
  ok(j(mx.envs) === '["dev","preprod","prod","qa"]' && mx.rows.length === 2 && mx.rows[0].team === "api" && mx.rows[1].cells[0] !== null && mx.rows[1].cells[1] === null && mx.rows[1].cells[2].state === "ready", "team by environment matrix");
  // gate
  ok(T.mayUse({ tenant_role: "owner" }) && T.mayUse({ tenant_role: "admin" }) && !T.mayUse({ tenant_role: "member", namespaces: [{ name: "a", role: "editor", joined: true }] }) && T.mayUse({ tenant_role: "member", namespaces: [{ name: "a", role: "admin", joined: true }] }) && !T.mayUse({ tenant_role: "member", namespaces: [{ name: "a", role: "admin", joined: false }] }) && !T.mayUse(null), "only tenant and team administrators");
  ok(T.path("k3s.cluster", "web", "prod") === "/api/v1/tenant/config/k3s.cluster/web/prod" && T.path("agent.claude", null, "default") === "/api/v1/tenant/config/agent.claude" && T.path("models.provider", null, "a b") === "/api/v1/tenant/config/models.provider/a%20b", "paths are encoded");
  ok(T.approvalText({ requester: "ann", kind: "k3s.cluster", item: "prod", team: "web", escalated: true }).includes("escalated") && j(T.parseNames("a, b\n c")) === '["a","b","c"]', "approval text and names");

  // ---- the page against a fake API
  function node(tag) {
    return { tag, attrs: {}, kids: [], _t: "", className: "", listeners: {}, value: "", checked: false,
      setAttribute(k, v) { this.attrs[k] = v; }, appendChild(c) { this.kids.push(c); return c; }, addEventListener(t, f) { this.listeners[t] = f; },
      set textContent(v) { this._t = v; this.kids = []; }, get textContent() { return this._t; } };
  }
  const doc = { createElement: node, createTextNode: (t) => ({ tag: "#text", _t: t, kids: [] }), body: node("body"), hidden: false };
  const walk = (n, f) => { f(n); for (const k of n.kids || []) walk(k, f); };
  const all = (n) => { const out = []; walk(n, (x) => out.push(x)); return out; };
  const texts = (n) => all(n).map((x) => x._t || "").join(" | ");
  const buttons = (n, label) => all(n).filter((x) => x.tag === "button" && x._t === label);
  const el = App.make(doc);
  function ctxFor(answers, calls) {
    const rep = (m, p) => answers[m + " " + p.split("?")[0]] || { ok: false, status: 404, message: "Not found", data: null };
    return { el, doc, me: {}, say: (t, k) => { calls.say = [t, k]; }, poll() {}, messageFor: App.messageFor,
      api: { get: async (p) => { calls.push(["GET", p]); return rep("GET", p); }, post: async (p, b) => { calls.push(["POST", p, b]); return rep("POST", p); },
             call: async (m, p, b) => { calls.push([m, p, b]); return rep(m, p); }, authHeaders: () => ({}) } };
  }
  {
    const calls = [], host = node("div");
    await T.page().render(ctxFor({ "GET /api/v1/tenant": { ok: true, status: 200, data: { tenant_role: "member", namespaces: [] } } }, calls), host);
    ok(texts(host).includes("administrators") && all(host).some((x) => x.attrs && x.attrs.role === "alert") && !calls.some((c) => c[1].startsWith("/api/v1/tenant/config")), "a member sees a denied state and no config is requested");
  }
  {
    const calls = [], host = node("div");
    await T.page().render(ctxFor({ "GET /api/v1/tenant": { ok: false, status: 403, message: "Not allowed: no access" } }, calls), host);
    ok(texts(host).includes("Not allowed: no access"), "denied from the server is shown");
  }
  const cfg = { ok: true, status: 200, data: { version: 3, vault: "unlocked", file_error: false,
    items: [{ kind: "agent.claude", name: "default", namespace: null, enabled: true, managed: true, source: "file", state: "ready", reasons: [], can: { write: false } },
            { kind: "agent.copilot", name: "default", namespace: null, enabled: true, managed: false, source: "api", state: "secret_missing", reasons: ["token is not in the vault"], can: { write: true }, values: { token: "[not set]" } }],
    registry: { kinds: [{ kind: "agent.claude", summary: "s", fields: [{ name: "token", type: "secret_ref agent.claude", secret: true, help: "" }] }, { kind: "agent.copilot", summary: "s", fields: [{ name: "mode", type: "one of per_person|shared", help: "" }, { name: "token", type: "secret_ref agent.copilot", secret: true, help: "" }] }] } } };
  const base = { "GET /api/v1/tenant": { ok: true, status: 200, data: { tenant_role: "owner", namespaces: [] } }, "GET /api/v1/tenant/config": cfg,
    "GET /api/v1/tenant/approvals": { ok: true, status: 200, data: { approvals: [{ id: "ap1", kind: "k3s.cluster", team: "web", item: "prod", requester: "ann", expires: 1700000000 }] } },
    "GET /api/v1/tenant/config/versions": { ok: true, status: 200, data: { current: 3, versions: [{ version: 2, at: 1700000000, by: "root", summary: "put", items: 1 }] } } };
  {
    const calls = [], host = node("div");
    await T.page().render(ctxFor(base, calls), host);
    const t = texts(host);
    ok(t.includes("ready") && t.includes("secret missing") && t.includes("managed") && t.includes("source: file") && t.includes("source: api") && t.includes("token is not in the vault"), "items with state, source and the managed lock");
    ok(buttons(host, "Edit").length === 1, "a managed or read-only item has no Edit button");
    ok(!calls.some((c) => c[0] === "POST"), "nothing is posted on load");
    const ap = buttons(host, "Approvals (1)")[0]; ap.listeners.click();
    ok(texts(host).includes("ann asks for k3s.cluster prod in team web") && !calls.some((c) => c[0] === "POST"), "an approval is listed and not decided without a click");
    const yes = buttons(host, "Approve")[0]; await yes.listeners.click();
    ok(calls.filter((c) => c[0] === "POST").length === 1 && calls.find((c) => c[0] === "POST")[1] === "/api/v1/tenant/approvals/ap1/approve", "approve is sent by the person's click, once");
  }
  {
    const calls = [], host = node("div");
    await T.page().render(ctxFor(Object.assign({}, base, { "PUT /api/v1/tenant/config/agent.copilot": { ok: true, status: 200, data: { version: 4 } } }), calls), host);
    buttons(host, "Edit")[0].listeners.click();
    const pw = all(host).filter((x) => x.tag === "input" && x.attrs.type === "password")[0];
    ok(pw && pw.value === "" && texts(host).includes("[not set]") && !texts(host).includes("vault:"), "the secret box is empty and shows only its state");
    pw.value = "sk-typed-secret-123456"; pw.listeners.input();
    const save = buttons(host, "Save")[0]; await save.listeners.click();
    const put = calls.find((c) => c[0] === "PUT");
    ok(put && put[2].secrets.token === "sk-typed-secret-123456" && !JSON.stringify(put[2].values).includes("sk-typed") && pw.value === "", "a typed secret is sent once in secrets and the box is cleared");
  }
  console.log(bad ? "FAILED " + bad : "ALL OK");
  process.exit(bad ? 1 : 0);
})();
"##;

#[test]
fn the_tenant_settings_page_logic_follows_its_rules_under_node() {
    let ok = std::process::Command::new("node").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP: node is not installed");
        return;
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/webservice/ui");
    let dir = std::env::temp_dir().join(format!("iyakki-webui-tcfg-js-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["app.js", "common.js", "files.js", "tenantcfg.js"] {
        std::fs::copy(src.join(f), dir.join(f)).unwrap();
    }
    std::fs::write(dir.join("driver.js"), DRIVER).unwrap();
    let out = std::process::Command::new("node").arg(dir.join("driver.js")).arg(&dir).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success() && !text.contains("BAD"), "{text}");
    assert!(text.contains("ALL OK"), "{text}");
}
