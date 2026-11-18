//! The Chat page (`src/webservice/ui/chat.js`, F-WEB-52) under node: the model and tool lists, the body a message starts a run with,
//! the conversation drawn from a run's events, Stop, and that a question for the person is answered only by their own click (one
//! verdict, once). Same method as `webui_schedules_js.rs`: skipped (passes, saying so) when `node` is not installed. NOT a real browser.

const DRIVER: &str = r##"
const path = require("path");
const dir = process.argv[2];
const App = require(path.join(dir, "app.js"));
const Chat = require(path.join(dir, "chat.js"));
let bad = 0;
const ok = (c, m) => { console.log((c ? "OK  " : "BAD ") + m); if (!c) bad++; };
const j = JSON.stringify;

function node(tag) {
  return { tag, attrs: {}, kids: [], _t: "", className: "", listeners: {}, value: "", checked: false,
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
  return { el, doc, me: { profile: "agent-api" }, say: (t, k) => { calls.say = [t, k]; }, poll: (f, ms, opts) => { calls.polls.push({ f, ms, opts }); return () => {}; }, messageFor: App.messageFor,
    api: { get: async (p) => { calls.push(["GET", p]); const a = answers["GET " + p.split("?")[0]]; return typeof a === "function" ? a(p) : (a || { ok: false, status: 404, message: "Not found", data: null }); },
           post: async (p, b) => { calls.push(["POST", p, b]); const a = answers["POST " + p]; return a || { ok: true, status: 200, data: {} }; } } };
}
const newCalls = () => { const c = []; c.polls = []; return c; };

(async () => {
  ok(j(Chat.modelChoices([{ provider: "a", model: "m1" }, { provider: "a", model: "m1" }, { provider: "", model: "x" }, null, { provider: "b", model: "m2" }])) === j(["a / m1", "b / m2"]), "model choices: one per provider and model, damaged entries left out");
  ok(j(Chat.startBody("  hi ", "", [], [])) === j({ prompt: "hi" }), "no model chosen and no narrowing: only the prompt");
  ok(j(Chat.startBody("hi", "a / m1", ["x", "y"], ["x", "y"])) === j({ prompt: "hi", provider: "a", model: "m1" }), "a model is sent as provider and model, all tools means no list");
  ok(j(Chat.startBody("hi", "a / m1", ["x"], ["x", "y"])) === j({ prompt: "hi", provider: "a", model: "m1", tools: ["x"] }), "a narrowed tool list is sent");
  const conv = Chat.conversation("do it", [{ kind: "model", text: "thinking\nCalls: web_fetch" }, { kind: "approval", text: "Waiting for you: fetch" }, { kind: "result", text: "web_fetch: ok" }, { kind: "final", text: "done" }, { kind: "usage", text: "Used 3 tokens" }]);
  ok(conv.map((i) => i.who).join() === "you,assistant,system,tool,assistant" && conv[0].text === "do it", "the conversation: you, assistant, notes, tool results; usage is not a message");

  {
    const h = Chat.history([{ id: "a", prompt: "old one\nsecond line", status: "completed", started: 5 }, { id: "b", prompt: "new token=abcdef123456 one", status: "running", started: 9 }, { id: "", prompt: "no id" }, null, { id: "c".repeat(100), prompt: "too long id" }]);
    ok(h.length === 2 && h[0].id === "b" && h[1].id === "a" && h[1].title === "old one", "history: newest first, a damaged entry and an odd id left out, the title is the first line");
    ok(!h[0].title.includes("abcdef123456"), "history: a secret in a title is hidden");
    ok(Chat.history(Array.from({ length: 30 }, (_, i) => ({ id: "r" + i, prompt: "p", started: i }))).length === 20, "history: at most twenty");
    const b = Chat.blocks("Intro with **bold** and `code`.\n\n- one\n- two\n\n```\nlet x = 1;\n<b>not html</b>\n```\nAfter.");
    ok(b.map((x) => x.t).join() === "p,ul,code,p", "blocks: paragraph, list, code, paragraph");
    ok(b[0].parts.some((p) => p.b && p.text === "bold") && b[0].parts.some((p) => p.code && p.text === "code"), "blocks: bold and inline code are marked");
    ok(b[1].items.length === 2 && b[2].text.includes("<b>not html</b>"), "blocks: list items, and code keeps its text as text (it is drawn as a text node)");
    ok(Chat.blocks("```\nunfinished").length === 1 && Chat.blocks("").length === 0, "blocks: an unfinished fence and an empty text are safe");
  }
  const base = {
    "GET /api/v1/models": { ok: true, status: 200, data: { models: [{ provider: "local", model: "small" }, { provider: "cloud", model: "big" }] } },
    "GET /api/v1/tools": { ok: true, status: 200, data: { tools: ["web_fetch", "now"] } },
    "GET /api/v1/chat/runs": { ok: true, status: 200, data: { runs: [{ id: "old1", prompt: "an earlier question", status: "completed", started: 3 }] } },
  };
  {
    const calls = newCalls(), host = node("div");
    await Chat.page().render(ctxFor(Object.assign({}, base, { "GET /api/v1/models": { ok: false, status: 403, message: "Not allowed: no access" } }), calls), host);
    ok(texts(host).includes("Not allowed: no access") && all(host).some((x) => x.attrs && x.attrs.role === "alert"), "no chat permission: the reason is shown, not a blank page");
  }
  {
    const calls = newCalls(), host = node("div");
    let events = 0;
    const answers = Object.assign({}, base, {
      "POST /api/v1/chat/runs": { ok: true, status: 200, data: { id: "run1" } },
      "GET /api/v1/chat/runs/run1/events": () => {
        events++;
        if (events === 1) return { ok: true, status: 200, data: { id: "run1", status: "running", events: [{ type: "model", text: "I will fetch it.", calls: [{ name: "web_fetch" }] }, { type: "approval", name: "web_fetch", summary: "fetch https://x.example" }], next: 2, done: false } };
        return { ok: true, status: 200, data: { id: "run1", status: "completed", events: [{ type: "tool_result", name: "web_fetch", ok: true, text: "token=abcdef123456 page" }, { type: "final", text: "All done." }], next: 4, done: true } };
      },
      "GET /api/v1/approvals": { ok: true, status: 200, data: { approvals: [{ run: "run1", call: "c9", tool: "web_fetch", summary: "fetch https://x.example", risk: "medium" }, { run: "other", call: "c1", tool: "x", summary: "not mine" }] } },
      "POST /api/v1/approvals/run1/c9": { ok: true, status: 200, data: {} },
    });
    await Chat.page().render(ctxFor(answers, calls), host);
    ok(calls.polls.length === 1 && calls.polls[0].opts && calls.polls[0].opts.push === true, "the page asks to be refreshed by the event stream");
    ok(buttons(host, "Open").length === 1 && texts(host).includes("Earlier conversations (1)") && texts(host).includes("an earlier question"), "the earlier conversations are listed with an Open button");
    ok(all(host).filter((x) => x.tag === "option").length === 3 && texts(host).includes("local / small") && texts(host).includes("Tools this conversation may use (2 of 2)"), "the model list (with the default) and the tool list are offered");
    const ta = all(host).find((x) => x.tag === "textarea"), form = all(host).find((x) => x.tag === "form");
    ta.value = "fetch the page";
    const sel = all(host).find((x) => x.tag === "select");
    await sel.listeners.change({ target: { value: "cloud / big" } });
    await form.listeners.submit({ preventDefault() {} });
    const post = calls.find((c) => c[0] === "POST" && c[1] === "/api/v1/chat/runs");
    ok(post && j(post[2]) === j({ prompt: "fetch the page", provider: "cloud", model: "big" }), "Send starts a run with the prompt and the chosen model");
    const t = texts(host);
    ok(t.includes("You") && t.includes("fetch the page") && t.includes("I will fetch it.") && t.includes("Calls: web_fetch"), "the person's message and the assistant's text are in the conversation");
    ok(buttons(host, "Approve").length === 1 && buttons(host, "Deny").length === 1 && !t.includes("not mine"), "one question for this run, with Approve and Deny; another run's question is not shown here");
    ok(!calls.some((c) => c[0] === "POST" && c[1].includes("/approvals/")), "nothing was answered by itself");
    await buttons(host, "Approve")[0].listeners.click();
    const verdicts = calls.filter((c) => c[0] === "POST" && c[1].startsWith("/api/v1/approvals/"));
    ok(verdicts.length === 1 && verdicts[0][1] === "/api/v1/approvals/run1/c9" && j(verdicts[0][2]) === j({ verdict: "approve" }), "a click sends exactly one verdict, approve");
    ok(!texts(host).includes("abcdef123456") && texts(host).includes("All done.") && texts(host).includes("Done: completed"), "a secret in a tool result is hidden, the final answer and the end are shown");
    ok(buttons(host, "Approve").length === 0 && all(host).some((x) => x.tag === "button" && x._t === "Stop" && x.disabled === true), "after the end: no question left and Stop is off");
  }
  {
    const calls = newCalls(), host = node("div");
    const answers = Object.assign({}, base, {
      "POST /api/v1/chat/runs": { ok: true, status: 200, data: { id: "run2" } },
      "GET /api/v1/chat/runs/run2/events": { ok: true, status: 200, data: { id: "run2", status: "running", events: [], next: 0, done: false } },
      "GET /api/v1/approvals": { ok: true, status: 200, data: { approvals: [] } },
    });
    await Chat.page().render(ctxFor(answers, calls), host);
    const ta = all(host).find((x) => x.tag === "textarea"), form = all(host).find((x) => x.tag === "form");
    ta.value = "   ";
    await form.listeners.submit({ preventDefault() {} });
    ok(!calls.some((c) => c[0] === "POST") && calls.say && calls.say[1] === "err", "an empty message is not sent");
    ta.value = "long task";
    await form.listeners.submit({ preventDefault() {} });
    await buttons(host, "Stop")[0].listeners.click();
    ok(calls.some((c) => c[0] === "POST" && c[1] === "/api/v1/chat/runs/run2/stop"), "Stop posts to the run's stop route");
  }
  {
    const calls = newCalls(), host = node("div");
    const answers = Object.assign({}, base, {
      "GET /api/v1/chat/runs/old1/events": { ok: true, status: 200, data: { id: "old1", status: "completed", events: [{ type: "final", text: "Here is **the answer**:\n- one\n- two" }], next: 1, done: true } },
      "GET /api/v1/approvals": { ok: true, status: 200, data: { approvals: [] } },
    });
    await Chat.page().render(ctxFor(answers, calls), host);
    await buttons(host, "Open")[0].listeners.click();
    const t = texts(host);
    ok(calls.some((c) => c[0] === "GET" && c[1].startsWith("/api/v1/chat/runs/old1/events?after=0")), "Open reads the earlier run from its first event");
    ok(t.includes("an earlier question") && t.includes("Here is") && t.includes("the answer") && t.includes("Done: completed"), "the earlier conversation is shown with its answer");
    ok(all(host).some((x) => x.tag === "strong" && x._t === "the answer") && all(host).filter((x) => x.tag === "li").length === 2, "the answer is drawn formatted: bold and a list, as elements");
    ok(!all(host).some((x) => x.tag === "script") && !all(host).some((x) => x.innerHTML !== undefined), "no HTML is ever set");
  }
  console.log(bad ? "FAILED " + bad : "ALL OK");
  process.exit(bad ? 1 : 0);
})();
"##;

#[test]
fn the_chat_page_starts_follows_stops_and_leaves_every_answer_to_the_persons_click_under_node() {
    let ok = std::process::Command::new("node").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP: node is not installed");
        return;
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/webservice/ui");
    let dir = std::env::temp_dir().join(format!("iyakki-webui-chat-js-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["app.js", "common.js", "runs.js", "chat.js"] {
        std::fs::copy(src.join(f), dir.join(f)).unwrap();
    }
    std::fs::write(dir.join("driver.js"), DRIVER).unwrap();
    let out = std::process::Command::new("node").arg(dir.join("driver.js")).arg(&dir).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success() && !text.contains("BAD"), "{text}");
    assert!(text.contains("ALL OK"), "{text}");
}
