//! Pages that refresh from the event stream (F-WEB-51, F-WEBSVC-09) under node: the page protocol's `poll` gets `(why, live)`, the
//! kinds `run` and `flowrun` count as pushes, the Runs page refreshes at once on a push or on the timer while the stream is up and
//! keeps its own counting only as the fallback, and the Flows page reloads the open flow's runs on a push without showing "loading"
//! again. Same method as `webui_schedules_js.rs`: skipped (passes, saying so) when `node` is not installed. NOT a real browser.

const DRIVER: &str = r##"
const path = require("path");
const dir = process.argv[2];
const App = require(path.join(dir, "app.js"));
const L = require(path.join(dir, "flows.js"));
const R = require(path.join(dir, "runs.js"));
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
function ctxFor(answers, calls) {
  return { el, doc, me: { profile: "cloud" }, say: () => {}, poll: (f, ms, opts) => { calls.polls.push({ f, ms, opts }); return () => {}; }, messageFor: App.messageFor,
    api: { get: async (p) => { calls.push(["GET", p]); return answers["GET " + p.split("?")[0]] || { ok: false, status: 404, message: "Not found", data: null }; },
           post: async (p, b) => { calls.push(["POST", p, b]); return answers["POST " + p] || { ok: true, status: 200, data: {} }; }, authHeaders: () => ({}) } };
}
const newCalls = () => { const c = []; c.polls = []; return c; };
const count = (calls, p) => calls.filter((c) => c[0] === "GET" && c[1].split("?")[0] === p).length;

(async () => {
  // the kinds the stream carries for runs count as pushes, and not faster than the gap allows
  ok(App.pushDue({ type: "run" }, 1e6) && App.pushDue({ type: "flowrun" }, 1e6) && App.pushDue({ type: "changed" }, 1e6), "run, flowrun and changed refresh a page");
  // a message inside the gap is not lost: it refreshes once the gap is over (found by the real-browser run: an approval that came 100 ms after
  // the refresh of a new run left the Runs page without its card for 30 s)
  ok(App.pushDelay({ type: "approval" }, 1e6) === 0 && App.pushDelay({ type: "approval" }, 100) > 0 && App.pushDelay({ type: "approval" }, 100) <= App.EVENT_GAP_MS - 100 + 50, "a push inside the gap is delayed, not dropped");
  ok(App.pushDelay({ type: "nonsense" }, 1e6) === -1 && App.pushDelay(null, 0) === -1, "an unknown kind never refreshes");
  ok(!App.pushDue({ type: "run" }, 0) && !App.pushDue({ type: "nonsense" }, 1e6) && !App.pushDue(null, 1e6), "not within the gap, and not for an unknown kind");

  {
    const calls = newCalls(), host = node("div");
    const answers = { "GET /api/v1/chat/runs": { ok: true, status: 200, data: { runs: [] } }, "GET /api/v1/approvals": { ok: true, status: 200, data: { approvals: [] } } };
    await R.page().render(ctxFor(answers, calls), host);
    ok(calls.polls.length === 1 && calls.polls[0].opts && calls.polls[0].opts.push === true, "the Runs page asks to be refreshed by the stream");
    const f = calls.polls[0].f;
    const n0 = count(calls, "/api/v1/chat/runs");
    await f("push", true);
    ok(count(calls, "/api/v1/chat/runs") === n0 + 1 && count(calls, "/api/v1/approvals") === n0 + 1, "a push refreshes the list and the approvals at once");
    await f("timer", true);
    ok(count(calls, "/api/v1/chat/runs") === n0 + 2, "the safety-net timer, with the stream up, refreshes the same way");
    // without the stream the old counting is the fallback: an idle page refreshes on its eighth tick
    const before = count(calls, "/api/v1/chat/runs");
    for (let i = 0; i < 7; i++) await f("timer", false);
    ok(count(calls, "/api/v1/chat/runs") === before, "an idle page does not refresh on the first seven timer ticks");
    await f("timer", false);
    ok(count(calls, "/api/v1/chat/runs") === before + 1, "and refreshes on the eighth");
  }
  {
    const calls = newCalls(), host = node("div");
    const answers = {
      "GET /api/v1/flows": { ok: true, status: 200, data: { flows: [{ id: "f1", name: "Nightly", nodes: 3, published: true }] } },
      "GET /api/v1/flows/f1": { ok: true, status: 200, data: { id: "f1" } },
      "GET /api/v1/flows/f1/runs": { ok: true, status: 200, data: { runs: [{ id: "r9", status: "running", done: 1, total: 3 }], more: false, page: 0 } },
    };
    await L.page().render(ctxFor(answers, calls), host);
    ok(calls.polls.length === 1 && calls.polls[0].opts && calls.polls[0].opts.push === true, "the Flows page asks to be refreshed by the stream");
    const f = calls.polls[0].f;
    await f("push", true);
    ok(count(calls, "/api/v1/flows/f1/runs") === 0, "with no flow open a push asks nothing about runs");
    await buttons(host, "View")[0].listeners.click();
    const n0 = count(calls, "/api/v1/flows/f1/runs");
    ok(n0 >= 1 && texts(host).includes("r9, running"), "opening a flow shows its runs");
    answers["GET /api/v1/flows/f1/runs"] = { ok: true, status: 200, data: { runs: [{ id: "r9", status: "succeeded", done: 3, total: 3 }], more: false, page: 0 } };
    const seen = [];
    const origAppend = host.appendChild.bind(host);
    host.appendChild = (c) => { seen.push(1); return origAppend(c); };
    await f("push", true);
    ok(count(calls, "/api/v1/flows/f1/runs") === n0 + 1 && texts(host).includes("r9, done, 3/3 steps") && !texts(host).includes("r9, running"), "a push reloads the open flow's runs and the page shows the new state");
  }
  console.log(bad ? "FAILED " + bad : "ALL OK");
  process.exit(bad ? 1 : 0);
})();
"##;

#[test]
fn pages_refresh_from_the_event_stream_and_fall_back_to_their_timers_under_node() {
    let ok = std::process::Command::new("node").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP: node is not installed");
        return;
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/webservice/ui");
    let dir = std::env::temp_dir().join(format!("iyakki-webui-push-js-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["app.js", "common.js", "flows.js", "runs.js"] {
        std::fs::copy(src.join(f), dir.join(f)).unwrap();
    }
    std::fs::write(dir.join("driver.js"), DRIVER).unwrap();
    let out = std::process::Command::new("node").arg(dir.join("driver.js")).arg(&dir).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success() && !text.contains("BAD"), "{text}");
    assert!(text.contains("ALL OK"), "{text}");
}
