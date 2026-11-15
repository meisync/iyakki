/*
  SPDX-License-Identifier: Apache-2.0
  Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

  Page "Chat" (F-WEB-52): one conversation with the assistant, over the same routes as the page Runs. The person types a message
  (POST /api/v1/chat/runs), may choose a model from GET /api/v1/models and narrow the tools of this one run from GET /api/v1/tools,
  and the run is followed through its events (GET /api/v1/chat/runs/{id}/events): their message, the assistant's text, what each
  tool call did, and the final answer. A run can be stopped. A question that waits for a person (a tool that asks first) is shown
  as a card in the conversation and is answered only by that person's own click on Approve or Deny: nothing here answers for them,
  there is no "always" and no key shortcut. What is shown goes through the same redaction and length limits as the page Runs.
  Loads under node for the tests (module.exports).
*/
"use strict";
(function (root) {
  const C = (typeof require === "function" && typeof module !== "undefined") ? require("./common.js") : root.IyakkiCommon;
  const R = (typeof require === "function" && typeof module !== "undefined") ? require("./runs.js") : root.IyakkiRuns;

  // ---- pure helpers (tested under node)
  // "provider / model", the value of one choice in the list
  function modelValue(m) { return m && m.provider && m.model ? m.provider + " / " + m.model : ""; }
  function modelChoices(list) {
    const out = [];
    for (const m of Array.isArray(list) ? list : []) { const v = modelValue(m); if (v && out.indexOf(v) < 0) out.push(v); }
    return out;
  }
  // the one body a run is started with: the prompt, the chosen model (or none: the machine's default) and, only when the person
  // narrowed them, the tools this run may use
  function startBody(prompt, model, tools, allTools) {
    const body = { prompt: String(prompt || "").trim() };
    const at = String(model || "").indexOf(" / ");
    if (at > 0) { body.provider = model.slice(0, at); body.model = model.slice(at + 3); }
    if (Array.isArray(tools) && Array.isArray(allTools) && tools.length < allTools.length) body.tools = tools.slice();
    return body;
  }
  // turn the lines of a run into conversation items: {who: "you"|"assistant"|"tool"|"system", kind, text}
  function conversation(prompt, lines) {
    const items = [{ who: "you", kind: "prompt", text: C.clip(C.redact(prompt), 20000) }];
    for (const l of lines || []) {
      if (l.kind === "model" || l.kind === "final") items.push({ who: "assistant", kind: l.kind, text: l.text });
      else if (l.kind === "result" || l.kind === "failed") items.push({ who: "tool", kind: l.kind, text: l.text });
      else if (l.kind === "approval" || l.kind === "decision" || l.kind === "note" || l.kind === "stopped") items.push({ who: "system", kind: l.kind, text: l.text });
    }
    return items;
  }
  // the earlier conversations of this person: the newest first, at most 20, each with a short redacted title
  function history(list) {
    const out = [];
    for (const r of Array.isArray(list) ? list : []) {
      if (!r || typeof r.id !== "string" || !r.id || r.id.length > 80) continue;
      const first = String(r.prompt || "").split("\n")[0];
      out.push({ id: r.id, title: C.clip(C.redact(first), 80) || "(no title)", status: String(r.status || ""), started: Number(r.started || 0), prompt: String(r.prompt || "") });
    }
    out.sort((a, b) => b.started - a.started);
    return out.slice(0, 20);
  }
  // the answer as a few safe blocks: paragraphs, bullet lists and code. Nothing here is HTML: the blocks are drawn with text nodes only
  function inline(text) {
    const parts = [];
    const re = /(\*\*[^*\n]{1,200}\*\*|`[^`\n]{1,200}`)/g;
    let at = 0, m;
    while ((m = re.exec(text)) !== null) {
      if (m.index > at) parts.push({ text: text.slice(at, m.index) });
      const t = m[0];
      parts.push(t.startsWith("**") ? { b: true, text: t.slice(2, -2) } : { code: true, text: t.slice(1, -1) });
      at = m.index + t.length;
    }
    if (at < text.length) parts.push({ text: text.slice(at) });
    return parts;
  }
  function blocks(text) {
    const out = [];
    const lines = String(text || "").split("\n");
    let para = [], list = null, code = null;
    const flush = () => {
      if (para.length) { out.push({ t: "p", parts: inline(para.join(" ")) }); para = []; }
      if (list) { out.push({ t: "ul", items: list.map(inline) }); list = null; }
    };
    for (const line of lines) {
      if (code) {
        if (/^\s*```/.test(line)) { out.push({ t: "code", text: code.join("\n") }); code = null; } else code.push(line);
        continue;
      }
      if (/^\s*```/.test(line)) { flush(); code = []; continue; }
      const li = /^\s*[-*]\s+(.*)$/.exec(line);
      if (li) { if (para.length) { out.push({ t: "p", parts: inline(para.join(" ")) }); para = []; } (list = list || []).push(li[1]); continue; }
      if (!line.trim()) { flush(); continue; }
      if (list) { out.push({ t: "ul", items: list.map(inline) }); list = null; }
      para.push(line.trim());
    }
    if (code) out.push({ t: "code", text: code.join("\n") });
    flush();
    return out;
  }
  function page() { return { id: "chat", title: "Chat", available: (me) => !!me, render }; }

  async function render(ctx, host) {
    const { api, el, say, poll } = ctx;
    const body = el("div");
    host.appendChild(body);
    let models = [], modelState = "loading", modelMsg = "", toolList = [], toolState = "loading";
    let chosen = "", off = new Set();
    let cur = null; // {id, prompt, lines, next, status, done}
    let approvals = [], starting = false, busy = false;
    const answered = new Set();
    let past = [];

    const ml = await api.get("/api/v1/models");
    if (ml.ok) { models = modelChoices(ml.data.models); modelState = "ok"; } else { modelState = C.stateKind(ml.status); modelMsg = ml.message; say(ml.message, "err"); }
    const tl = await api.get("/api/v1/tools");
    if (tl.ok) { toolList = (tl.data.tools || []).slice().sort(); toolState = "ok"; } else { toolState = C.stateKind(tl.status); }

    async function loadHistory() {
      const r = await api.get("/api/v1/chat/runs");
      past = r.ok ? history(r.data.runs) : [];
    }
    async function open(item) {
      if (cur && !cur.done) { say("A conversation is still running: stop it first.", "err"); return; }
      cur = { id: item.id, prompt: item.prompt, lines: [], next: 0, status: item.status || "running", done: false };
      approvals = [];
      draw();
      await refresh();
    }
    async function follow() {
      if (!cur || cur.done) return;
      const r = await api.get("/api/v1/chat/runs/" + encodeURIComponent(cur.id) + "/events?after=" + cur.next + "&wait=0");
      if (!cur || !r.ok) { if (r && !r.ok && cur) { say(r.message, "err"); cur.done = true; } return; }
      Object.assign(cur, R.applyEvents(cur, r.data));
      const a = await api.get("/api/v1/approvals");
      approvals = a.ok ? (a.data.approvals || []).filter((x) => cur && x.run === cur.id && !answered.has(x.run + "/" + x.call)) : [];
    }
    async function refresh() {
      if (busy) return;
      busy = true;
      try { await follow(); } finally { busy = false; }
      draw();
    }
    async function send(text) {
      if (starting) return;
      const bad = R.checkPrompt(text);
      if (bad) { say(bad, "err"); return; }
      starting = true; draw();
      const allowed = toolList.filter((t) => !off.has(t));
      const r = await api.post("/api/v1/chat/runs", startBody(text, chosen, allowed, toolList));
      starting = false;
      if (r.ok && r.data && r.data.id) { cur = { id: r.data.id, prompt: text.trim(), lines: [], next: 0, status: "running", done: false }; approvals = []; say("", ""); await loadHistory(); drawEarlier(); await refresh(); } else { say(r.message, "err"); draw(); }
    }
    async function stop() {
      if (!cur || cur.done) return;
      const r = await api.post("/api/v1/chat/runs/" + encodeURIComponent(cur.id) + "/stop", {});
      say(r.ok ? "Stop requested." : r.message, r.ok ? "ok" : "err");
      await refresh();
    }
    async function decide(a, word) {
      const key = a.run + "/" + a.call;
      if (answered.has(key)) return;
      answered.add(key);
      approvals = approvals.filter((x) => x !== a);
      draw();
      const r = await api.post(R.approvalPath(a), R.verdictBody(word));
      if (r.ok) say((word === "approve" ? "Approved " : "Denied ") + C.clip(a.tool, 60) + ".", "ok");
      else say(r.status === 409 ? "That question was already answered or timed out." : r.message, "err");
      await refresh();
    }

    function card(a) {
      return el("div", { class: "card ask", role: "group", "aria-label": "A question for you: " + C.clip(a.tool, 60) }, [
        el("h3", { text: "The assistant asks to use " + C.clip(a.tool, 60) }),
        el("p", { text: C.clip(C.redact(a.summary), 500) }),
        a.risk ? el("p", { class: R.riskClass(a.risk), text: "Risk: " + C.clip(a.risk, 40) }) : null,
        el("div", { class: "inline" }, [
          el("button", { type: "button", class: "primary", text: "Approve", "aria-label": "Approve " + C.clip(a.tool, 60), onclick: () => decide(a, "approve") }),
          el("button", { type: "button", class: "danger", text: "Deny", "aria-label": "Deny " + C.clip(a.tool, 60), onclick: () => decide(a, "deny") }),
        ]),
        el("p", { class: "muted", text: "Only your own click answers. This is one answer for this one request." }),
      ]);
    }

    const ta = el("textarea", { "aria-label": "Your message", rows: "3", maxlength: "20000", placeholder: "Ask the assistant ..." });
    const go = el("button", { type: "submit", class: "primary", text: "Send" });
    const stopBtn = el("button", { type: "button", text: "Stop", onclick: () => stop() });
    const form = el("form", { class: "card", onsubmit: (ev) => { ev.preventDefault(); const t = ta.value; ta.value = ""; return send(t); } }, [ta, go, stopBtn]);
    const convo = el("div", { role: "log", "aria-label": "The conversation", "aria-live": "polite" });
    const setup = el("div");
    body.appendChild(setup);
    body.appendChild(convo);
    body.appendChild(form);

    function drawSetup() {
      setup.textContent = "";
      if (modelState !== "ok") { setup.appendChild(C.stateNode(el, modelState, modelMsg || "The models could not be loaded.")); return; }
      const sel = el("select", { "aria-label": "Model", onchange: (ev) => { chosen = ev.target.value; } }, [el("option", { value: "", text: "The machine's default model", selected: chosen === "" })].concat(models.map((m) => el("option", { value: m, text: m, selected: m === chosen }))));
      const box = el("div", { class: "card inline" }, [el("label", { text: "Model " }), sel]);
      if (toolState === "ok" && toolList.length) {
        const det = el("details", {}, [el("summary", { text: "Tools this conversation may use (" + (toolList.length - off.size) + " of " + toolList.length + ")" })]);
        for (const t of toolList) {
          det.appendChild(el("label", { class: "inline" }, [el("input", { type: "checkbox", checked: !off.has(t), "aria-label": t, onchange: (ev) => { if (ev.target.checked) off.delete(t); else off.add(t); drawSetup(); } }), el("span", { class: "mono", text: " " + t })]));
        }
        box.appendChild(det);
      }
      setup.appendChild(box);
    }

    // the answer drawn from its blocks: text nodes only, never HTML
    function formatted(text) {
      const kids = blocks(text).map((b) => {
        const spans = (parts) => parts.map((p) => p.b ? el("strong", { text: p.text }) : p.code ? el("code", { class: "mono", text: p.text }) : el("span", { text: p.text }));
        if (b.t === "code") return el("pre", { class: "mono", text: b.text });
        if (b.t === "ul") return el("ul", {}, b.items.map((it) => el("li", {}, spans(it))));
        return el("p", {}, spans(b.parts));
      });
      return el("div", { class: "answer" }, kids);
    }

    function draw() {
      go.disabled = starting;
      go.textContent = starting ? "Sending ..." : "Send";
      stopBtn.disabled = !cur || cur.done;
      convo.textContent = "";
      if (!cur) { convo.appendChild(el("p", { class: "muted", text: "Say what you want done. Questions that need you appear here and only your click answers them." })); return; }
      for (const it of conversation(cur.prompt, cur.lines)) {
        const who = el("strong", { text: it.who === "you" ? "You" : it.who === "assistant" ? "Assistant" : it.who === "tool" ? "Tool" : "Note" });
        convo.appendChild(el("div", { class: "card msg msg-" + it.who }, [who, it.who === "assistant" ? formatted(it.text) : el("pre", { class: "mono", text: it.text })]));
      }
      if (approvals.length) convo.appendChild(el("div", { role: "region", "aria-label": "Questions waiting for you" }, approvals.map(card)));
      convo.appendChild(el("p", { class: R.statusClass(cur.status), role: "status", text: cur.done ? "Done: " + cur.status : "Working ..." }));
    }

    const earlier = el("div");
    body.insertBefore ? body.insertBefore(earlier, convo) : body.appendChild(earlier);
    function drawEarlier() {
      earlier.textContent = "";
      if (!past.length) return;
      const det = el("details", {}, [el("summary", { text: "Earlier conversations (" + past.length + ")" })]);
      for (const it of past) {
        det.appendChild(el("div", { class: "inline" }, [el("span", { text: it.title + " (" + C.clip(it.status, 20) + ") " }), el("button", { type: "button", text: "Open", "aria-label": "Open the conversation: " + it.title, onclick: () => open(it) })]));
      }
      earlier.appendChild(det);
    }
    await loadHistory();
    drawEarlier();
    drawSetup();
    draw();
    // the event stream refreshes a running conversation at once; the timer is the fallback (and a safety net while the stream is up)
    if (poll) poll(async () => { if (cur && !cur.done) await refresh(); }, 3000, { push: true });
  }

  const exported = { page, modelChoices, modelValue, startBody, conversation, history, blocks, inline };
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
  else C.registerPage(root, page());
})(typeof window !== "undefined" ? window : globalThis);
