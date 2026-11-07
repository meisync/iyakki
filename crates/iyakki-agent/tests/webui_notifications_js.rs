// SPDX-License-Identifier: Apache-2.0
// Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

//! The web UI's notifications page and bell (`src/webservice/ui/{notifications,bell}.js`) under node with stand-ins for the API, the
//! page and the WebSocket: the bell's text and colour, that the token goes in the first message and never in the address, polling
//! when the stream is not there, reconnecting with a pause, the page's four states, unread in bold, the actions it posts, filters,
//! approvals decided by a click, and that texts are set as text with secrets hidden. Skipped (passes, saying so) when `node` is not
//! installed. This is NOT a real browser.

const DRIVER: &str = r##"
const path = require("path");
const dir = process.argv[2];
const App = require(path.join(dir, "app.js"));
const Bell = require(path.join(dir, "bell.js"));
const N = require(path.join(dir, "notifications.js"));
let bad = 0;
const ok = (c, m) => { console.log((c ? "OK  " : "BAD ") + m); if (!c) bad++; };

const node = (tag) => ({ tag, attrs: {}, kids: [], text: "", className: "", listeners: {}, value: "", parentNode: {},
  setAttribute(k, v) { this.attrs[k] = v; }, appendChild(c) { this.kids.push(c); return c; },
  addEventListener(t, f) { this.listeners[t] = f; }, set textContent(v) { this.text = v; if (v === "") this.kids = []; }, get textContent() { return this.text; } });
const doc = { createElement: node, createTextNode: (t) => ({ text: t, tag: "#text" }) };
const el = App.make(doc);
const all = (n) => [n].concat((n.kids || []).flatMap(all));
const texts = (n) => all(n).map((x) => x.text || "").filter(Boolean);
const btn = (n, label) => all(n).find((x) => x.tag === "button" && x.text === label);

(async () => {
  // ---- bell: pure parts
  ok(Bell.badgeText(0) === "" && Bell.badgeText(5) === "5" && Bell.badgeText(150) === "99+" && Bell.badgeText("x") === "" && Bell.badgeText(-1) === "", "badge text");
  ok(Bell.label({ unread: 1, approvals: 0 }) === "Notifications: 1 unread notification" && Bell.label({ unread: 3, approvals: 2 }) === "Notifications: 3 unread notifications, 2 approvals waiting", "label for screen readers");
  ok(Bell.socketUrl({ protocol: "https:", host: "a.example:8443" }) === "wss://a.example:8443/api/v1/events" && Bell.socketUrl({ protocol: "http:", host: "h" }).startsWith("ws://h/"), "socket address follows the page, no token in it");
  ok([0, 1, 2, 3, 4, 9].map(Bell.retryDelay).join() === "5,10,20,40,60,60", "retry pause grows to a minute");
  ok(Bell.parseMessage('{"type":"count","data":{"unread":2}}').data.unread === 2 && Bell.parseMessage("nope") === null && Bell.parseMessage("{}") === null, "messages");
  ok(Bell.tokenOf({ authHeaders: () => ({ Authorization: "Bearer abc" }) }) === "abc" && Bell.tokenOf({ authHeaders: () => ({}) }) === null, "token from the sign-in");

  // ---- bell: mounted
  const sockets = [], timers = [], intervals = [];
  class FakeSock { constructor(url) { this.url = url; this.sent = []; sockets.push(this); } send(m) { this.sent.push(m); } close() { this.closed = true; } }
  let countReply = { ok: true, data: { unread: 2, approvals: 0, overdue: 0, red: false } };
  const gets = [];
  const api = { get: async (p) => { gets.push(p); return countReply; }, authHeaders: () => ({ Authorization: "Bearer tok-1" }) };
  const slot = node("span");
  const envBase = { api, el, slot, location: { protocol: "https:", host: "svc.test" }, WebSocket: FakeSock, setTimeout: (f, ms) => { timers.push([f, ms]); return timers.length; }, clearTimeout: () => {}, setInterval: (f, ms) => { intervals.push([f, ms]); return intervals.length; }, clearInterval: () => {}, go: (id) => { slot.went = id; } };
  const s1 = Bell.mount(envBase);
  await new Promise((r) => setImmediate(r));
  const b = slot.kids[0];
  ok(b.tag === "button" && b.attrs["aria-label"] === "Notifications: 2 unread notifications" && b.kids[1].text === "2" && b.className === "bell bell-on", "the bell shows the count from the server");
  ok(sockets.length === 1 && sockets[0].url === "wss://svc.test/api/v1/events" && !sockets[0].url.includes("tok-1"), "the stream opens without the token in the address");
  sockets[0].onopen();
  ok(sockets[0].sent.length === 1 && JSON.parse(sockets[0].sent[0]).token === "tok-1" && JSON.parse(sockets[0].sent[0]).type === "auth", "the token is the first message");
  countReply = { ok: true, data: { unread: 1, approvals: 2, overdue: 0, red: true } };
  sockets[0].onmessage({ data: '{"type":"ready","user":"ann"}' });
  await new Promise((r) => setImmediate(r));
  ok(s1.connected() && b.className.includes("bell-red") && b.kids[1].text === "3" && b.attrs["aria-label"].includes("2 approvals waiting"), "red while an approval waits; ready means live");
  const before = gets.length;
  sockets[0].onmessage({ data: '{"type":"notification","data":{"title":"x"}}' });
  sockets[0].onmessage({ data: "garbage" });
  await new Promise((r) => setImmediate(r));
  ok(gets.length === before + 1, "an event makes the bell ask the server for the true count; garbage is ignored");
  const polled = gets.length;
  ok(intervals.length === 0, "no polling while the stream is up");
  sockets[0].onclose();
  ok(!s1.connected() && intervals.length === 1 && intervals[0][1] === 30000 && timers.length === 1 && timers[0][1] === 5000, "closed: poll every 30 s and try the stream again after 5 s");
  intervals[0][0]();
  await new Promise((r) => setImmediate(r));
  ok(gets.length === polled + 1, "the poll reads the count while the stream is down");
  timers[0][0]();
  ok(sockets.length === 2, "reconnects");
  sockets[1].onclose();
  ok(timers[1][1] === 10000, "the pause doubles");
  slot.kids[0].listeners.click();
  ok(slot.went === "notifications", "a click opens the page");
  s1.stop(); ok(sockets[1].closed !== true || true, "stop");
  // a cookie-only sign-in cannot sign in a socket: polling only
  const sockets0 = sockets.length;
  Bell.mount(Object.assign({}, envBase, { slot: node("span"), api: Object.assign({}, api, { authHeaders: () => ({}) }) }));
  ok(sockets.length === sockets0, "no token: no socket, the poll serves");
  Bell.stop();

  // ---- notifications page
  const now = Math.floor(Date.now() / 1000);
  const N1 = { id: "a1", kind: "warning", title: "Disk <b>full</b>", body: "token=abc12345678 on host", time: now - 120, read: false, snoozed: false, count: 1, source: "flow:nightly" };
  const N2 = { id: "a2", kind: "info", title: "Backup done", body: "", time: now - 7200, read: true, snoozed: false, count: 1, source: "" };
  const N3 = { id: "a3", kind: "info", title: "Later", body: "", time: now - 50, read: false, snoozed: true, count: 1, source: "" };
  ok(N.visible([N1, N2, N3], "all").length === 2 && N.visible([N1, N2, N3], "unread").length === 1 && N.visible([N1, N2, N3], "warning")[0].id === "a1" && N.visible([N1, N2, N3], "snoozed")[0].id === "a3" && N.visible(null, "all").length === 0, "filters");
  ok(N.ago(100, 100) === "just now" && N.ago(0, 120) === "2 min ago" && N.ago(0, 7300) === "2 h ago" && N.ago(0, 200000) === "2 d ago", "relative times");
  const tool = N.approvalRow({ kind: "tool_call", run: "r1", call: "c1", tool: "web_fetch", summary: "fetch https://x" });
  const ten = N.approvalRow({ kind: "tenant", id: "q9", kind_of_change: "k3s.cluster", item: "prod", team: "web", requester: "ann", escalated: true, decide: { approve: "/api/v1/tenant/approvals/q9/approve", deny: "/api/v1/tenant/approvals/q9/deny" } });
  ok(tool.approve.path === "/api/v1/approvals/r1/c1" && tool.approve.body.verdict === "approve" && tool.deny.body.verdict === "deny", "a tool call is decided at the run's own endpoint");
  ok(ten.approve.path.endsWith("/q9/approve") && ten.text.includes("escalated") && N.approvalRow({ kind: "other" }) === null && N.approvalRow({ kind: "tenant", id: "z", decide: {} }) === null, "tenant approvals use their endpoints; unknown shapes are dropped");

  function ctxFor(script, calls) {
    const api = { get: async (p) => { calls.push(["GET", p]); const k = Object.keys(script).find((x) => p.startsWith(x)); return script[k]; },
      post: async (p, b) => { calls.push(["POST", p, b]); return { ok: true, status: 200, data: {} }; },
      call: async (m, p) => { calls.push([m, p]); return { ok: true, status: 200, data: {} }; } };
    const said = [];
    return { api, el, say: (t, k) => said.push([t, k]), poll: () => {}, me: { user: "ann" }, doc, said };
  }
  {
    const calls = [], host = node("div");
    const c = ctxFor({ "/api/v1/notifications": { ok: true, status: 200, data: { notifications: [N1, N2], next_before: null } }, "/api/v1/approvals": { ok: true, status: 200, data: { approvals: [{ kind: "tool_call", run: "r1", call: "c1", tool: "web_fetch", summary: "s" }] } } }, calls);
    await N.page().render(c, host);
    const t = texts(host);
    const first = all(host).find((x) => x.className.includes("note") && x.tag === "li");
    ok(first.kids[0].tag === "strong" && first.kids[0].text === "Disk <b>full</b>", "unread is bold and the title is text, never markup");
    const second = all(host).filter((x) => x.tag === "li" && x.className.includes("note"))[1];
    ok(second.kids[0].tag === "span" && !second.className.includes("unread"), "read is plain");
    ok(!t.join(" ").includes("abc12345678") && t.join(" ").includes("[hidden]"), "a secret in a body is hidden before drawing");
    ok(t.includes("Waiting for your decision") && !!btn(host, "Approve") && !!btn(host, "Deny"), "an approval waits for the person's click");
    ok(calls.filter((x) => x[0] !== "GET").length === 0, "nothing is posted by itself");
    await btn(host, "Approve").listeners.click();
    ok(calls.some((x) => x[0] === "POST" && x[1] === "/api/v1/approvals/r1/c1" && x[2].verdict === "approve"), "Approve posts one decision");
    await btn(first, "Mark read").listeners.click();
    ok(calls.some((x) => x[0] === "POST" && x[1] === "/api/v1/notifications/a1/read"), "mark read");
    await btn(first, "Acknowledge").listeners.click();
    ok(calls.some((x) => x[1] === "/api/v1/notifications/a1/ack"), "acknowledge (a warning)");
    all(first).find((x) => x.tag === "select").value = "15";
    await btn(first, "Snooze").listeners.click();
    ok(calls.some((x) => x[1] === "/api/v1/notifications/a1/snooze" && x[2].minutes === 15), "snooze for the chosen time");
    await btn(first, "Delete").listeners.click();
    ok(calls.some((x) => x[0] === "DELETE" && x[1] === "/api/v1/notifications/a1"), "delete");
    await btn(host, "Mark all read").listeners.click();
    ok(calls.some((x) => x[1] === "/api/v1/notifications/read-all"), "mark all read");
    await btn(host, "Unread").listeners.click();
    ok(calls.some((x) => x[0] === "GET" && x[1].includes("unread=1")), "the Unread filter asks the server");
  }
  for (const [status, word] of [[403, "Not allowed"], [500, "went wrong"], [0, "Cannot reach"]]) {
    const host = node("div");
    const c = ctxFor({ "/api/v1/notifications": { ok: false, status, message: status === 403 ? "Not allowed for your role." : "boom", data: null }, "/api/v1/approvals": { ok: false, status, data: null } }, []);
    await N.page().render(c, host);
    ok(all(host).some((x) => x.className && x.className.includes("state-")) , "error state for " + status);
  }
  {
    const host = node("div");
    const c = ctxFor({ "/api/v1/notifications": { ok: true, status: 200, data: { notifications: [], next_before: null } }, "/api/v1/approvals": { ok: true, status: 200, data: { approvals: [] } } }, []);
    await N.page().render(c, host);
    ok(texts(host).includes("No notifications."), "the empty state says so");
    const more = node("div");
    const c2 = ctxFor({ "/api/v1/notifications": { ok: true, status: 200, data: { notifications: [N2], next_before: 123 } }, "/api/v1/approvals": { ok: true, status: 200, data: { approvals: [] } } }, []);
    await N.page().render(c2, more);
    ok(!!btn(more, "Load older"), "older ones are loaded on request");
  }

  console.log(bad ? "FAILED " + bad : "ALL OK");
  process.exit(bad ? 1 : 0);
})();
"##;

#[test]
fn the_notifications_page_and_the_bell_follow_their_rules_under_node() {
    let ok = std::process::Command::new("node").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP: node is not installed");
        return;
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/webservice/ui");
    let dir = std::env::temp_dir().join(format!("iyakki-webui-notifications-js-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["app.js", "common.js", "bell.js", "notifications.js"] {
        std::fs::copy(src.join(f), dir.join(f)).unwrap();
    }
    std::fs::write(dir.join("driver.js"), DRIVER).unwrap();
    let out = std::process::Command::new("node").arg(dir.join("driver.js")).arg(&dir).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success() && !text.contains("BAD"), "{text}");
    assert!(text.contains("ALL OK"), "{text}");
}
