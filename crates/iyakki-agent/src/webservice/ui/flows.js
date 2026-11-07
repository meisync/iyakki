/*
  SPDX-License-Identifier: Apache-2.0
  Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

  Page "Flows" (cloud pods): the flows the signed-in person may see (GET /api/v1/flows) and, on request, one flow's definition
  (GET /api/v1/flows/{id}, shown read-only with anything that looks like a secret hidden). A Run button starts the flow
  (POST /api/v1/flows/{id}/run; the published version, or the draft when nothing is published) and a list shows its runs, newest
  first, paged (GET /api/v1/flows/{id}/runs). A run that waits for a person says what it asks and where to answer: only the person's
  own click on the page "Runs" (or the desktop) approves or denies; this page has no such button. Editing a flow is not offered.
*/
"use strict";
(function (root) {
  const C = (typeof require === "function" && typeof module !== "undefined") ? require("./common.js") : root.IyakkiCommon;

  // ---- pure helpers (tested under node)
  function flowBadges(f) {
    const out = [];
    if (f && f.production) out.push("production");
    if (f && f.published) out.push("published");
    if (f && f.triggers > 0) out.push(f.triggers + (f.triggers === 1 ? " trigger" : " triggers"));
    return out;
  }
  function describeFlow(f) {
    const n = f && typeof f.nodes === "number" ? f.nodes : 0;
    return n + (n === 1 ? " step" : " steps");
  }
  function sortFlows(list, filter) {
    const q = String(filter || "").trim().toLowerCase();
    return (list || []).filter((f) => f && (!q || String(f.name || f.id).toLowerCase().includes(q))).slice().sort((a, b) => String(a.name || a.id).toLowerCase().localeCompare(String(b.name || b.id).toLowerCase()));
  }
  // a flow's definition as safe text (secrets hidden, long ones cut)
  function definitionText(def) {
    let t;
    try { t = JSON.stringify(def, null, 2); } catch (e) { t = ""; }
    return C.clip(C.redact(t), 20000);
  }
  const RUNS_PER = 10;
  // words and colour class for a run's state
  function runState(r) {
    const st = String((r && r.status) || "");
    const text = { succeeded: "done", failed: "failed", blocked: "waiting for you", running: "running", queued: "queued", scheduled: "scheduled", cancelled: "cancelled", retrying: "retrying", healing: "repairing" }[st] || st || "unknown";
    return { text, cls: st === "failed" ? "err" : st === "blocked" ? "warn" : "muted", active: ["running", "queued", "retrying", "healing", "blocked", "scheduled"].includes(st) };
  }
  // one line about a run (ids and times only; errors and questions are redacted and cut)
  function runLine(r) {
    const bits = [r.id, runState(r).text];
    if (r.total > 0) bits.push((r.done | 0) + "/" + r.total + " steps");
    if (r.created) bits.push(C.formatTime(r.created));
    return bits.join(", ");
  }
  function runNote(r) {
    if (r && r.blocked) return "It asks: " + C.clip(C.redact(r.blocked), 300) + " Answer it yourself on the page Runs or on the desktop.";
    if (r && r.error) return "Error: " + C.clip(C.redact(r.error), 300);
    return "";
  }
  function runsQuery(page) {
    return "?per=" + RUNS_PER + "&page=" + Math.max(0, page | 0);
  }
  // the body of the run call: {} (published) or {draft:true}
  function runBody(draft) {
    return draft ? { draft: true } : {};
  }

  function page() {
    return { id: "flows", title: "Flows", available: (me) => !!me && me.profile === "cloud", render };
  }

  async function render(ctx, host) {
    const { api, el, say } = ctx;
    const body = el("div");
    host.appendChild(body);
    let flows = [], state = "loading", msg = "", filter = "", open = null, def = null, defState = "", runs = null, runsPage = 0, starting = false;

    async function load() {
      state = "loading"; draw();
      const r = await api.get("/api/v1/flows");
      if (r.ok) { flows = r.data.flows || []; state = "ok"; } else { flows = []; state = C.stateKind(r.status); msg = r.message; }
      draw();
    }
    async function show(f) {
      open = f.id; def = null; defState = "loading"; draw();
      const r = await api.get("/api/v1/flows/" + encodeURIComponent(f.id));
      if (open !== f.id) return;
      if (r.ok) { def = r.data; defState = "ok"; } else { defState = C.stateKind(r.status); say(r.message, "err"); }
      draw();
    }

    async function loadRuns(f, p) {
      runsPage = Math.max(0, p);
      runs = { state: "loading", msg: "", data: null }; draw();
      const r = await api.get("/api/v1/flows/" + encodeURIComponent(f.id) + "/runs" + runsQuery(runsPage));
      if (open !== f.id) return;
      runs = r.ok ? { state: "ok", msg: "", data: r.data } : { state: C.stateKind(r.status), msg: r.message, data: null };
      draw();
    }
    async function run(f) {
      if (starting) return;
      starting = true; say("Starting " + (f.name || f.id) + " ...");
      const r = await api.post("/api/v1/flows/" + encodeURIComponent(f.id) + "/run", runBody(!f.published));
      starting = false;
      if (r.ok) { say("Started run " + r.data.run + ".", "ok"); if (open !== f.id) await show(f); else await loadRuns(f, 0); } else { say(r.message, "err"); draw(); }
    }
    async function show(f) {
      open = f.id; def = null; defState = "loading"; runs = null; draw();
      const pending = loadRuns(f, 0);
      const r = await api.get("/api/v1/flows/" + encodeURIComponent(f.id));
      if (open !== f.id) return;
      if (r.ok) { def = r.data; defState = "ok"; } else { defState = C.stateKind(r.status); say(r.message, "err"); }
      draw();
      await pending;
    }

    function runsBox(f) {
      const box = el("div", { class: "card", role: "region", "aria-label": "Runs of " + (f.name || f.id) });
      box.appendChild(el("h3", { text: "Runs" }));
      if (!runs || runs.state === "loading") { box.appendChild(C.stateNode(el, "loading")); return box; }
      if (runs.state !== "ok") { box.appendChild(C.stateNode(el, runs.state, runs.msg)); return box; }
      const list = runs.data.runs || [];
      if (!list.length) box.appendChild(el("p", { class: "muted", text: runsPage ? "No more runs." : "This flow has not run yet." }));
      for (const r of list) {
        const note = runNote(r);
        box.appendChild(el("div", { class: "row" }, [el("span", { class: "name mono", text: runLine(r) }), el("span", { class: "badge " + runState(r).cls, text: runState(r).text })]));
        if (note) box.appendChild(el("p", { class: runState(r).cls, text: note }));
      }
      box.appendChild(el("div", { class: "inline" }, [
        el("button", { type: "button", text: "Newer", disabled: runsPage === 0, onclick: () => loadRuns(f, runsPage - 1) }),
        el("span", { class: "muted", role: "status", text: "Page " + (runsPage + 1) }),
        el("button", { type: "button", text: "Older", disabled: !runs.data.more, onclick: () => loadRuns(f, runsPage + 1) }),
        el("button", { type: "button", text: "Refresh", onclick: () => loadRuns(f, runsPage) }),
      ]));
      box.appendChild(el("p", { class: "muted" }, ["Questions that wait for you are answered on the page ", el("a", { href: "#/runs", text: "Runs" }), "."]));
      return box;
    }

    function draw() {
      body.textContent = "";
      const card = el("div", { class: "card" });
      body.appendChild(card);
      if (state !== "ok") { card.appendChild(C.stateNode(el, state, msg)); }
      else {
        if (flows.length > 10) card.appendChild(el("input", { type: "search", "aria-label": "Filter flows by name", placeholder: "Filter by name", value: filter, onchange: (ev) => { filter = ev.target.value; draw(); } }));
        const shown = sortFlows(flows, filter);
        if (!shown.length) card.appendChild(el("p", { class: "muted", text: flows.length ? "No flow matches the filter." : "There are no flows yet." }));
        for (const f of shown) {
          const badges = flowBadges(f).map((b) => el("span", { class: "badge", text: b }));
          card.appendChild(el("div", { class: "row" }, [
            el("span", { class: "name", text: f.name || f.id }),
            el("span", { class: "muted", text: describeFlow(f) }),
          ].concat(badges, [el("button", { type: "button", class: "primary", text: "Run", disabled: starting, "aria-label": "Run " + (f.name || f.id), onclick: () => run(f) }), el("button", { type: "button", text: open === f.id ? "Hide" : "View", "aria-expanded": open === f.id ? "true" : "false", "aria-label": (open === f.id ? "Hide " : "View ") + (f.name || f.id), onclick: () => { if (open === f.id) { open = null; draw(); } else show(f); } })])));
          if (f.description) card.appendChild(el("p", { class: "muted", text: C.clip(f.description, 300) }));
          if (open === f.id) {
            if (defState === "loading") card.appendChild(C.stateNode(el, "loading"));
            else if (defState === "ok") card.appendChild(el("pre", { class: "mono viewer", tabindex: "0", text: definitionText(def) }));
            else card.appendChild(C.stateNode(el, defState, "The definition could not be shown."));
            card.appendChild(runsBox(f));
          }
        }
      }
      body.appendChild(el("p", { class: "muted" }, ["Agent runs, their log and the questions waiting for you are on the page ", el("a", { href: "#/runs", text: "Runs" }), "."]));
    }
    await load();
  }

  const exported = { page, flowBadges, describeFlow, sortFlows, definitionText, runState, runLine, runNote, runsQuery, runBody, RUNS_PER };
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
  else C.registerPage(root, page());
})(typeof window !== "undefined" ? window : globalThis);
