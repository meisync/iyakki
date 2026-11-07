/*
  SPDX-License-Identifier: Apache-2.0
  Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

  Page "Runs" (the web twin of the chat): ask the assistant something (POST /api/v1/chat/runs), follow a run by polling its events
  (GET /api/v1/chat/runs/{id}/events), stop it (POST .../stop), and answer the questions that wait for a person
  (GET /api/v1/approvals, POST /api/v1/approvals/{run}/{call}). An approval is only ever given by the person's own click on
  "Approve" or "Deny": nothing here answers for them, there is no "always", no auto-answer and no key shortcut that approves, and
  a button cannot be clicked twice. The server accepts only approve or deny. What a run shows is passed through a redaction of
  anything that looks like a secret and cut to a sensible length. Polling stands in for the events WebSocket the API does not
  have yet (see app.js, poll).
*/
"use strict";
(function (root) {
  const C = (typeof require === "function" && typeof module !== "undefined") ? require("./common.js") : root.IyakkiCommon;
  const POLL_MS = 2000;
  const PROMPT_MAX = 20000;
  const SHOW_MAX = 2000;

  // ---- pure helpers (tested under node)
  function isActive(status) {
    return status === "running" || status === "waiting" || status === "started" || status === "queued";
  }
  function statusClass(status) {
    if (isActive(status)) return "warn";
    if (status === "done" || status === "finished" || status === "ok" || status === "completed") return "on";
    if (status === "failed" || status === "error") return "err";
    return "off";
  }
  function checkPrompt(p) {
    const t = String(p || "").trim();
    if (!t) return "Type what you want to ask.";
    if (t.length > PROMPT_MAX) return "That is longer than " + PROMPT_MAX + " characters.";
    return "";
  }
  // one run event as {kind, text}; kinds the page does not draw give null (deltas, thinking, budgets and timings are not shown)
  function eventLine(ev) {
    if (!ev || typeof ev !== "object") return null;
    const t = (s) => C.clip(C.redact(s), SHOW_MAX);
    switch (ev.type) {
      case "model": {
        const calls = (ev.calls || []).map((c) => c.name).filter(Boolean);
        const parts = [];
        if (ev.text) parts.push(t(ev.text));
        if (calls.length) parts.push("Calls: " + calls.join(", "));
        return parts.length ? { kind: "model", text: parts.join("\n") } : null;
      }
      case "approval": return { kind: "approval", text: "Waiting for you: " + t(ev.summary || ev.name) };
      case "decision": return { kind: "decision", text: (ev.approved ? "Approved" : "Denied") + " " + t(ev.name) + " (by " + t(ev.by) + ")" };
      case "tool_result": return { kind: ev.ok ? "result" : "failed", text: t(ev.name) + (ev.ok ? ": " : " failed: ") + t(ev.text) };
      case "final": return { kind: "final", text: t(ev.text) };
      case "failed": return { kind: "failed", text: "The run failed: " + t(ev.error) };
      case "stopped": return { kind: "stopped", text: "Stopped: " + t(ev.reason) };
      case "note": return { kind: "note", text: t(ev.text) };
      case "usage": return { kind: "usage", text: "Used " + (ev.tokens || 0) + " tokens in " + (ev.seconds || 0) + " s" };
      default: return null;
    }
  }
  // fold an answer of the events route into what the page holds: new lines after `next`, the status, whether it is over
  function applyEvents(st, ans) {
    const lines = st.lines.slice();
    for (const ev of ans.events || []) {
      const l = eventLine(ev);
      if (l) lines.push(l);
    }
    const next = Number.isInteger(ans.next) && ans.next >= st.next ? ans.next : st.next;
    return { lines, next, status: ans.status || st.status, done: ans.done === true || (ans.done === undefined && !isActive(ans.status || st.status)) };
  }
  // the one body an approval call ever has; anything but these two words is refused here as well as by the server
  function verdictBody(word) {
    if (word !== "approve" && word !== "deny") throw new Error("the verdict is approve or deny");
    return { verdict: word };
  }
  function approvalPath(a) {
    return "/api/v1/approvals/" + encodeURIComponent(a.run) + "/" + encodeURIComponent(a.call);
  }
  function riskClass(r) {
    const x = String(r || "").toLowerCase();
    return x === "high" || x === "danger" ? "err" : x === "medium" ? "warn" : "muted";
  }

  function page() {
    return { id: "runs", title: "Runs", available: (me) => !!me, render };
  }

  async function render(ctx, host) {
    const { api, el, say, poll } = ctx;
    const body = el("div");
    host.appendChild(body);
    const answered = new Set(); // approvals clicked in this tab (a second click is not possible)
    let runs = [], runsState = "loading", runsMsg = "", approvals = [], apState = "ok";
    let sel = null; // {id, lines, next, status, done}
    let busyPoll = false, starting = false;

    async function loadRuns() {
      const r = await api.get("/api/v1/chat/runs");
      if (r.ok) { runs = (r.data.runs || []).slice().sort((a, b) => Number(b.started || 0) - Number(a.started || 0)); runsState = "ok"; }
      else { runs = []; runsState = C.stateKind(r.status); runsMsg = r.message; }
    }
    async function loadApprovals() {
      const r = await api.get("/api/v1/approvals");
      if (r.ok) { approvals = (r.data.approvals || []).filter((a) => !answered.has(a.run + "/" + a.call)); apState = "ok"; }
      else { approvals = []; apState = C.stateKind(r.status); }
    }
    async function follow() {
      if (!sel || sel.done) return;
      const r = await api.get("/api/v1/chat/runs/" + encodeURIComponent(sel.id) + "/events?after=" + sel.next + "&wait=0");
      if (!sel || !r.ok) { if (r && !r.ok && sel) { say(r.message, "err"); sel.done = true; } return; }
      const keep = sel.id;
      const upd = applyEvents(sel, r.data);
      if (sel && sel.id === keep) Object.assign(sel, upd);
    }
    async function refresh() {
      if (busyPoll) return;
      busyPoll = true;
      try { await Promise.all([loadRuns(), loadApprovals(), follow()]); } finally { busyPoll = false; }
      draw();
    }
    async function select(id) {
      sel = { id, lines: [], next: 0, status: "running", done: false };
      draw();
      await follow();
      draw();
    }
    async function start(prompt) {
      if (starting) return;
      starting = true; draw();
      const r = await api.post("/api/v1/chat/runs", { prompt: prompt.trim() });
      starting = false;
      if (r.ok && r.data && r.data.id) { say("Started.", "ok"); await loadRuns(); await select(r.data.id); } else { say(r.message, "err"); draw(); }
    }
    async function stop() {
      if (!sel) return;
      const r = await api.post("/api/v1/chat/runs/" + encodeURIComponent(sel.id) + "/stop", {});
      say(r.ok ? "Stop requested." : r.message, r.ok ? "ok" : "err");
      await refresh();
    }
    async function decide(a, word) {
      const key = a.run + "/" + a.call;
      if (answered.has(key)) return;
      answered.add(key);
      approvals = approvals.filter((x) => x !== a);
      draw();
      const r = await api.post(approvalPath(a), verdictBody(word));
      if (r.ok) say((word === "approve" ? "Approved " : "Denied ") + C.clip(a.tool, 60) + ".", "ok");
      else say(r.status === 409 ? "That question was already answered or timed out." : r.message, "err");
      await refresh();
    }

    function approvalCard(a) {
      const detail = a.detail ? el("pre", { class: "mono viewer", tabindex: "0", "aria-label": "Details of the request", text: C.clip(C.redact(a.detail), 6000) }) : null;
      return el("div", { class: "card ask", role: "group", "aria-label": "A question for you: " + C.clip(a.tool, 60) }, [
        el("h3", { text: "The assistant asks to use " + C.clip(a.tool, 60) }),
        el("p", { text: C.clip(C.redact(a.summary), 500) }),
        a.risk ? el("p", { class: riskClass(a.risk), text: "Risk: " + C.clip(a.risk, 40) }) : null,
        detail,
        el("div", { class: "inline" }, [
          el("button", { type: "button", class: "primary", text: "Approve", "aria-label": "Approve " + C.clip(a.tool, 60), onclick: () => decide(a, "approve") }),
          el("button", { type: "button", class: "danger", text: "Deny", "aria-label": "Deny " + C.clip(a.tool, 60), onclick: () => decide(a, "deny") }),
        ]),
        el("p", { class: "muted", text: "Only your own click answers. This is one answer for this one request." }),
      ]);
    }

    // The page is three boxes that are redrawn only when what they show has changed, so polling never takes the focus from the
    // question box and never swaps a button under the pointer.
    const apBox = el("div");
    const listBox = el("div");
    const detailBox = el("div");
    const sigs = { ap: "", list: "", detail: "" };
    const ta = el("textarea", { "aria-label": "What do you want to ask?", rows: "3", maxlength: String(PROMPT_MAX), placeholder: "Ask the assistant ..." });
    const go = el("button", { type: "submit", class: "primary", text: "Start" });
    const form = el("form", { class: "card", onsubmit: (ev) => {
      ev.preventDefault();
      const bad = checkPrompt(ta.value);
      if (bad) { say(bad, "err"); return; }
      const p = ta.value; ta.value = ""; start(p);
    } }, [ta, go]);
    body.appendChild(apBox);
    body.appendChild(form);
    body.appendChild(listBox);
    body.appendChild(detailBox);

    function redraw(key, box, sig, build) {
      if (sigs[key] === sig) return;
      sigs[key] = sig;
      box.textContent = "";
      build(box);
    }

    function draw() {
      go.disabled = starting;
      go.textContent = starting ? "Starting ..." : "Start";
      // questions first: they block a run
      redraw("ap", apBox, apState + "|" + approvals.map((a) => a.run + "/" + a.call).join(","), (box) => {
        if (apState === "ok" && approvals.length) {
          box.appendChild(el("div", { role: "region", "aria-label": "Questions waiting for you" }, [el("h2", { text: "Waiting for you (" + approvals.length + ")" })].concat(approvals.map(approvalCard))));
        } else if (apState !== "ok" && apState !== "denied") {
          box.appendChild(C.stateNode(el, apState, "The waiting questions could not be loaded."));
        }
      });
      redraw("list", listBox, runsState + "|" + runsMsg + "|" + (sel ? sel.id : "") + "|" + runs.slice(0, 50).map((r) => r.id + ":" + r.status).join(","), (box) => {
        const list = el("div", { class: "card" }, [el("h2", { text: "Runs" })]);
        if (runsState === "loading") list.appendChild(C.stateNode(el, "loading"));
        else if (runsState !== "ok") list.appendChild(C.stateNode(el, runsState, runsMsg));
        else if (!runs.length) list.appendChild(el("p", { class: "muted", text: "No runs yet. Ask something above." }));
        for (const r of runs.slice(0, 50)) {
          list.appendChild(el("div", { class: "row" }, [
            el("button", { type: "button", class: "name linkish", text: C.clip(C.redact(r.prompt), 80) || r.id, "aria-label": "Open run: " + C.clip(C.redact(r.prompt), 60), "aria-current": sel && sel.id === r.id ? "true" : false, onclick: () => select(r.id) }),
            el("span", { class: "muted", text: C.formatTime(r.started) }),
            el("span", { class: "badge " + statusClass(r.status), text: r.status || "?" }),
          ]));
        }
        box.appendChild(list);
      });
      redraw("detail", detailBox, sel ? sel.id + "|" + sel.status + "|" + sel.done + "|" + sel.lines.length : "", (box) => {
        if (!sel) return;
        const d = el("div", { class: "card", "aria-live": "polite" }, [
          el("div", { class: "inline" }, [el("h2", { text: "Run" }), el("span", { class: "badge " + statusClass(sel.status), text: sel.status }),
            isActive(sel.status) ? el("button", { type: "button", class: "danger", text: "Stop", onclick: stop }) : null,
            el("button", { type: "button", text: "Close", onclick: () => { sel = null; draw(); } })]),
        ]);
        if (!sel.lines.length) d.appendChild(el("p", { class: "muted", text: sel.done ? "This run has nothing to show." : "Waiting for the first answer ..." }));
        for (const l of sel.lines) d.appendChild(el("p", { class: "line line-" + l.kind + (l.kind === "failed" ? " err" : ""), text: l.text }));
        box.appendChild(d);
      });
    }

    draw();
    await refresh();
    poll(refresh, POLL_MS);
  }

  const exported = { page, isActive, statusClass, checkPrompt, eventLine, applyEvents, verdictBody, approvalPath, riskClass, POLL_MS, PROMPT_MAX };
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
  else C.registerPage(root, page());
})(typeof window !== "undefined" ? window : globalThis);
