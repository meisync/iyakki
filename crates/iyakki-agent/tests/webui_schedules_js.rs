//! The logic and the states of the Schedules page (`src/webservice/ui/schedules.js`) under node: cron/interval/once checks, the trigger
//! body, descriptions, row states, refusal text, paths, and the page's states (denied, error, empty, a list, pause, the form's
//! preview call). Same method as `webui_pages_js.rs`: skipped (passes, saying so) when `node` is not installed. NOT a real browser.

const DRIVER: &str = r##"
const path = require("path");
const dir = process.argv[2];
const App = require(path.join(dir, "app.js"));
const C = require(path.join(dir, "common.js"));
const S = require(path.join(dir, "schedules.js"));
let bad = 0;
const ok = (c, m) => { console.log((c ? "OK  " : "BAD ") + m); if (!c) bad++; };
const j = JSON.stringify;

(async () => {
  ok(S.checkCron("0 9 * * 1-5") === "" && S.checkCron("*/5 * * * *") === "" && S.checkCron("@daily") === "" && S.checkCron("0 9 * *") !== "" && S.checkCron("") !== "" && S.checkCron("0 9 * * $(x)") !== "", "cron checks");
  ok(S.checkInterval("60") === "" && S.checkInterval("59") !== "" && S.checkInterval("1.5") !== "" && S.checkInterval("x") !== "" && S.checkInterval("") !== "", "interval checks");
  ok(S.checkOnce(2000, 1000) === "" && S.checkOnce(1000, 1000) !== "" && S.checkOnce(0, 1000) !== "", "once checks");
  ok(j(S.buildTrigger({ kind: "cron", expr: " 0 9 * * * ", tz: "", id: "t1", missed: "skip", overlap: "queue" })) === j({ kind: "cron", enabled: true, missed: "skip", overlap: "queue", id: "t1", expr: "0 9 * * *", tz: "local" }), "a cron trigger body");
  ok(S.buildTrigger({ kind: "interval", every_s: "300" }).every_s === 300 && S.buildTrigger({ kind: "once", at_ms: 5 }).at_ms === 5, "interval and once bodies");
  const b = S.saveBody({ flow: "f", team: "web", kind: "interval", every_s: "120", environment: "prod", run_as: "team-service" });
  ok(b.flow === "f" && b.team === "web" && b.environment === "prod" && b.run_as === "team-service" && b.trigger.kind === "interval" && !("secret" in b), "the save body");
  ok(S.checkForm({ kind: "cron", expr: "0 9 * * *" }, 0) === "Choose a flow." && S.checkForm({ flow: "f", kind: "cron", expr: "0 9 * * *" }, 0).includes("team") && S.checkForm({ flow: "f", team: "t", kind: "cron", expr: "0 9 * * *" }, 0) === "", "the form is checked before it is sent");
  ok(S.describeTrigger({ kind: "cron", expr: "0 9 * * *", tz: "+05:30" }) === "cron 0 9 * * * (+05:30)" && S.describeTrigger({ kind: "interval", every_s: 7200 }) === "every 2 h" && S.describeTrigger({ kind: "interval", every_s: 90 }) === "every 90 s" && S.describeTrigger({ kind: "interval", every_s: 300 }) === "every 5 min", "descriptions");
  ok(S.nextText([]) === "no later time" && S.nextText([1700000000000]).includes("2023-11-14") && S.nextText([1, 2, 3, 4, 5, 6, 1700000000000]).split(",").length <= 5, "next times");
  ok(S.rowState({ paused_by_service: "max is no longer in the team" }).cls === "err" && S.rowState({ owner_check: "lower role" }).text === "owner lost access" && S.rowState({ paused: true }).text === "paused" && S.rowState({ trigger: { enabled: false } }).text === "switched off" && S.rowState({ active: false }).text === "not published yet" && S.rowState({ active: true, trigger: {} }).text === "active", "row states");
  ok(S.rowState({ paused_by_service: "token=abc123456" }).why.includes("[hidden]") && !S.rowState({ paused_by_service: "token=abc123456" }).why.includes("abc123456"), "a reason is redacted");
  const r422 = { ok: false, status: 422, message: "x", data: { error: { message: "this flow cannot run while nobody is signed in", problems: ["`a` needs a person's own Claude sign-in"] } } };
  ok(S.refusalText(r422).includes("Claude sign-in") && S.refusalText({ ok: false, message: "Not allowed" }) === "Not allowed", "a refusal says why");
  ok(S.statusLine({ leader: true, runs: { running: 1, queued: 2, blocked: 3 }, quota_refused: 4, approvals_waiting: 5 }).includes("This pod runs") && S.statusLine({ leader: false }).includes("Another pod") && S.statusLine(null) === "", "status line");
  ok(S.pausePath("a b", true) === "/api/v1/schedules/a%20b/pause" && S.pausePath("f", false).endsWith("/resume") && S.deletePath("f", "t 1") === "/api/v1/schedules/f/t%201", "paths are encoded");
  ok(S.previewPath(" 0 9 * * * ", "+05:30") === "/api/v1/schedules/preview?expr=0%209%20*%20*%20*&tz=%2B05%3A30&count=5" && S.previewPath("@daily", "").indexOf("tz=") < 0, "the preview is a read with an encoded query");
  ok(S.localToMs("2030-01-01T10:00") > 0 && S.localToMs("junk") === 0, "date input");

  function node(tag) {
    return { tag, attrs: {}, kids: [], _t: "", className: "", listeners: {}, value: "",
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
    return { el, doc, me: { profile: "cloud" }, say: (t, k) => { calls.say = [t, k]; }, poll: () => {}, messageFor: App.messageFor,
      api: { get: async (p) => { calls.push(["GET", p]); return answers["GET " + p.split("?")[0]] || { ok: false, status: 404, message: "Not found", data: null }; },
             post: async (p, b) => { calls.push(["POST", p, b]); return answers["POST " + p] || { ok: true, status: 200, data: {} }; },
             call: async (m, p) => { calls.push([m, p]); return { ok: true, status: 200, data: {} }; } } };
  }
  ok(S.page().available({ profile: "cloud" }) && !S.page().available({ profile: "agent-api" }) && !S.page().available(null), "the page exists only on the cloud profile");
  const base = { "GET /api/v1/flows": { ok: true, status: 200, data: { flows: [{ id: "nightly", name: "Nightly" }] } }, "GET /api/v1/tenant": { ok: true, status: 200, data: { namespaces: [{ name: "web" }] } }, "GET /api/v1/schedules/status": { ok: true, status: 200, data: { leader: true, runs: {} } } };
  {
    const calls = [], host = node("div");
    await S.page().render(ctxFor(Object.assign({}, base, { "GET /api/v1/schedules": { ok: false, status: 403, message: "Not allowed: no access" } }), calls), host);
    ok(texts(host).includes("Not allowed: no access") && all(host).some((x) => x.attrs && x.attrs.role === "alert"), "denied is shown");
  }
  {
    const calls = [], host = node("div");
    await S.page().render(ctxFor(Object.assign({}, base, { "GET /api/v1/schedules": { ok: false, status: 500, message: "That did not work." } }), calls), host);
    ok(texts(host).includes("That did not work.") && buttons(host, "Create schedule").length === 0, "an error is shown and no form is offered");
  }
  {
    const calls = [], host = node("div");
    await S.page().render(ctxFor(Object.assign({}, base, { "GET /api/v1/schedules": { ok: true, status: 200, data: { schedules: [], limit: 50 } } }), calls), host);
    ok(texts(host).includes("No schedules yet") && buttons(host, "Create schedule").length === 1, "empty state with the form");
    ok(all(host).some((x) => x.tag === "button" && x._t === "Create schedule" && x.attrs.disabled === undefined), "a valid form can be sent");
  }
  {
    const calls = [], host = node("div");
    const sch = [{ flow: "nightly", name: "Nightly", published: 1, active: true, trigger: { id: "t1", kind: "cron", expr: "0 9 * * *", tz: "local", enabled: true }, next: [1700000000000], paused: false, team: "web", owner: "max", run_as: "owner" },
                  { flow: "weekly", name: "Weekly", active: true, trigger: { id: "t2", kind: "interval", every_s: 3600 }, next: [], paused: true, paused_by_service: "max is no longer in the team `web`", team: "web", owner: "max", run_as: "owner" }];
    await S.page().render(ctxFor(Object.assign({}, base, { "GET /api/v1/schedules": { ok: true, status: 200, data: { schedules: sch, limit: 50 } } }), calls), host);
    const t = texts(host);
    ok(t.includes("Nightly") && t.includes("cron 0 9 * * *") && t.includes("paused by the service") && t.includes("no longer in the team") && t.includes("2 of 50"), "rows, reasons and the limit");
    ok(buttons(host, "Pause").length === 1 && buttons(host, "Resume").length === 1 && buttons(host, "Run now").length === 2 && buttons(host, "Delete").length === 2 && buttons(host, "Change").length === 2, "row actions");
    await buttons(host, "Pause")[0].listeners.click();
    ok(calls.some((c) => c[0] === "POST" && c[1] === "/api/v1/schedules/nightly/pause"), "Pause posts to the pause route");
    await buttons(host, "Delete")[0].listeners.click();
    ok(calls.some((c) => c[0] === "DELETE" && c[1] === "/api/v1/schedules/nightly/t1"), "Delete uses DELETE on the trigger");
    ok(calls.every((c) => !JSON.stringify(c).toLowerCase().includes("password")), "no secret is sent");
  }
  console.log(bad ? "FAILED " + bad : "ALL OK");
  process.exit(bad ? 1 : 0);
})();
"##;

#[test]
fn the_schedules_page_logic_and_states_follow_their_rules_under_node() {
    let ok = std::process::Command::new("node").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP: node is not installed");
        return;
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/webservice/ui");
    let dir = std::env::temp_dir().join(format!("iyakki-webui-schedules-js-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["app.js", "common.js", "schedules.js"] {
        std::fs::copy(src.join(f), dir.join(f)).unwrap();
    }
    std::fs::write(dir.join("driver.js"), DRIVER).unwrap();
    let out = std::process::Command::new("node").arg(dir.join("driver.js")).arg(&dir).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success() && !text.contains("BAD"), "{text}");
    assert!(text.contains("ALL OK"), "{text}");
}
