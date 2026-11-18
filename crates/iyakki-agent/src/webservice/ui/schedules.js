/*
  SPDX-License-Identifier: Apache-2.0
  Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

  Page "Schedules" (cloud pods): the timed triggers of the signed-in person's own flows (GET /api/v1/schedules), a form to create or
  change one (POST /api/v1/schedules) with the next fire times shown as the cron expression is typed (GET /api/v1/schedules/preview,
  computed by the service in the tenant's time zone), pause and resume (POST /api/v1/schedules/{flow}/pause|resume), delete
  (DELETE /api/v1/schedules/{flow}/{trigger}), "Run now" (the existing POST /api/v1/flows/{id}/run) and the counters of the tenant
  scheduler (GET /api/v1/schedules/status). A schedule runs with nobody signed in, as the person who owns it (or the team identity,
  for team admins); this page says so, and says why a schedule was refused or paused. No outside library; text is always set as text.
*/
"use strict";
(function (root) {
  const C = (typeof require === "function" && typeof module !== "undefined") ? require("./common.js") : root.IyakkiCommon;
  const PREVIEW_COUNT = 5;
  const MIN_INTERVAL_S = 60;

  // ---- pure helpers (tested under node)
  function checkCron(expr) {
    const f = String(expr || "").trim().split(/\s+/).filter(Boolean);
    if (/^@(yearly|annually|monthly|weekly|daily|midnight|hourly)$/.test(f[0] || "") && f.length === 1) return "";
    if (f.length !== 5) return "A cron expression has five parts: minute hour day month weekday (for example 0 9 * * 1-5).";
    if (!f.every((p) => /^[A-Za-z0-9*?,\/\-]+$/.test(p))) return "Use only digits, names, * ? , / and -.";
    return "";
  }
  function checkInterval(text) {
    const n = Number(String(text || "").trim());
    if (!isFinite(n) || n !== Math.floor(n)) return "Give the interval in whole seconds.";
    if (n < MIN_INTERVAL_S) return "An interval is at least " + MIN_INTERVAL_S + " seconds.";
    return "";
  }
  function checkOnce(ms, nowMs) {
    const n = Number(ms);
    if (!isFinite(n) || n <= 0) return "Pick a date and time.";
    if (n <= nowMs) return "A one-time schedule needs a time in the future.";
    return "";
  }
  // the trigger JSON the service takes, from the form's fields; the id is kept when a schedule is being changed
  function buildTrigger(f) {
    const t = { kind: f.kind, enabled: f.enabled !== false, missed: f.missed || "run_once", overlap: f.overlap || "skip" };
    if (f.id) t.id = f.id;
    if (f.kind === "cron") { t.expr = String(f.expr || "").trim(); t.tz = String(f.tz || "").trim() || "local"; }
    else if (f.kind === "interval") t.every_s = Number(f.every_s);
    else if (f.kind === "once") t.at_ms = Number(f.at_ms);
    return t;
  }
  function saveBody(f) {
    const b = { flow: f.flow, team: f.team, trigger: buildTrigger(f) };
    if (f.environment) b.environment = f.environment;
    if (f.run_as) b.run_as = f.run_as;
    return b;
  }
  function checkForm(f, nowMs) {
    if (!f.flow) return "Choose a flow.";
    if (!f.team) return "Choose the team this schedule belongs to.";
    if (f.kind === "cron") return checkCron(f.expr);
    if (f.kind === "interval") return checkInterval(f.every_s);
    if (f.kind === "once") return checkOnce(f.at_ms, nowMs);
    return "Choose cron, interval or once.";
  }
  function describeTrigger(t) {
    if (!t) return "";
    if (t.kind === "cron") return "cron " + C.clip(t.expr || "", 60) + (t.tz && t.tz !== "local" ? " (" + C.clip(t.tz, 30) + ")" : "");
    if (t.kind === "interval") {
      const s = Number(t.every_s) || 0;
      return "every " + (s % 3600 === 0 ? s / 3600 + " h" : s % 60 === 0 ? s / 60 + " min" : s + " s");
    }
    if (t.kind === "once") return "once at " + C.formatTime(t.at_ms);
    return String(t.kind || "");
  }
  function nextText(list) {
    const a = (list || []).filter((x) => Number(x) > 0).slice(0, PREVIEW_COUNT);
    return a.length ? a.map((x) => C.formatTime(x)).join(", ") : "no later time";
  }
  // the state of a row: words, colour class
  function rowState(s) {
    if (s && s.paused_by_service) return { text: "paused by the service", cls: "err", why: C.clip(C.redact(s.paused_by_service), 300) };
    if (s && s.owner_check) return { text: "owner lost access", cls: "err", why: C.clip(C.redact(s.owner_check), 300) };
    if (s && s.paused) return { text: "paused", cls: "warn", why: "" };
    if (s && s.trigger && s.trigger.enabled === false) return { text: "switched off", cls: "muted", why: "" };
    if (s && s.active === false) return { text: "not published yet", cls: "warn", why: "Publish the flow in FLOWS to start this schedule." };
    return { text: "active", cls: "on", why: "" };
  }
  // what a refusal of the save says, including the reasons a flow cannot run unattended
  function refusalText(res) {
    const e = res && res.data && res.data.error;
    if (e && Array.isArray(e.problems) && e.problems.length) return C.clip(C.redact(e.message + ": " + e.problems.join(" ")), 600);
    return (res && res.message) || "That did not work.";
  }
  function statusLine(st) {
    if (!st) return "";
    const r = st.runs || {};
    const lead = st.leader ? "This pod runs the schedules" : "Another pod runs the schedules";
    return lead + ". Runs: " + (r.running | 0) + " going, " + (r.queued | 0) + " waiting, " + (r.blocked | 0) + " blocked. Held back by the quota: " + (st.quota_refused | 0) + ". Approvals waiting: " + (st.approvals_waiting | 0) + ".";
  }
  function pausePath(flow, paused) {
    return "/api/v1/schedules/" + encodeURIComponent(flow) + "/" + (paused ? "pause" : "resume");
  }
  function deletePath(flow, trigger) {
    return "/api/v1/schedules/" + encodeURIComponent(flow) + "/" + encodeURIComponent(trigger);
  }
  function previewPath(expr, tz) {
    return "/api/v1/schedules/preview?expr=" + encodeURIComponent(String(expr || "").trim()) + (String(tz || "").trim() ? "&tz=" + encodeURIComponent(String(tz).trim()) : "") + "&count=" + PREVIEW_COUNT;
  }
  function localToMs(text) {
    const t = Date.parse(String(text || ""));
    return isNaN(t) ? 0 : t;
  }

  function page() {
    return { id: "schedules", title: "Schedules", available: (me) => !!me && me.profile === "cloud", render };
  }

  async function render(ctx, host) {
    const { api, el, say } = ctx;
    const body = el("div");
    host.appendChild(body);
    let list = [], state = "loading", msg = "", limit = 0, flows = [], teams = [], status = null, editing = null, busy = false, preview = "", previewErr = "";
    let form = { kind: "cron", expr: "0 9 * * *", tz: "", every_s: "3600", at_ms: 0, missed: "run_once", overlap: "skip", flow: "", team: "", environment: "dev", run_as: "owner", enabled: true };
    const seq = C.latest();

    async function load() {
      state = "loading"; draw();
      const mine = seq.next();
      const [r, f, t, s] = await Promise.all([api.get("/api/v1/schedules"), api.get("/api/v1/flows"), api.get("/api/v1/tenant"), api.get("/api/v1/schedules/status")]);
      if (!seq.isCurrent(mine)) return;
      if (r.ok) { list = r.data.schedules || []; limit = r.data.limit | 0; state = "ok"; } else { list = []; state = C.stateKind(r.status); msg = r.message; }
      flows = f.ok ? (f.data.flows || []) : [];
      teams = t.ok ? ((t.data.namespaces || []).map((n) => n.name || n).filter(Boolean)) : [];
      status = s.ok ? s.data : null;
      if (!form.flow && flows.length) form.flow = flows[0].id;
      if (!form.team && teams.length) form.team = teams[0];
      draw();
    }
    async function act(label, fn) {
      if (busy) return;
      busy = true; draw();
      const r = await fn();
      busy = false;
      if (r.ok) { say(label, "ok"); editing = null; await load(); } else { say(refusalText(r), "err"); draw(); }
    }
    let previewTimer = null;
    function askPreview() {
      if (form.kind !== "cron") { preview = ""; previewErr = ""; draw(); return; }
      const bad = checkCron(form.expr);
      if (bad) { preview = ""; previewErr = bad; draw(); return; }
      previewErr = "";
      const mine = seq.next();
      api.get(previewPath(form.expr, form.tz)).then((r) => {
        if (!seq.isCurrent(mine)) return;
        if (r.ok) { preview = "Next: " + nextText(r.data.next) + " (" + C.clip(r.data.zone || "", 40) + ")"; previewErr = ""; } else { preview = ""; previewErr = r.message; }
        draw();
      });
    }

    function field(label, input) {
      return el("label", { class: "field" }, [el("span", { text: label }), input]);
    }
    function formBox() {
      const sel = (opts, cur, on, aria) => el("select", { "aria-label": aria, onchange: (ev) => on(ev.target.value) }, opts.map((o) => el("option", { value: o[0], text: o[1], selected: o[0] === cur })));
      const kids = [el("h3", { text: editing ? "Change schedule" : "New schedule" })];
      kids.push(field("Flow", sel(flows.map((f) => [f.id, f.name || f.id]), form.flow, (v) => { form.flow = v; }, "Flow")));
      kids.push(field("Team", sel(teams.map((t) => [t, t]), form.team, (v) => { form.team = v; }, "Team")));
      kids.push(field("Kind", sel([["cron", "Cron expression"], ["interval", "Every ... seconds"], ["once", "Once"]], form.kind, (v) => { form.kind = v; askPreview(); draw(); }, "Kind of schedule")));
      if (form.kind === "cron") {
        kids.push(field("Cron (minute hour day month weekday)", el("input", { type: "text", value: form.expr, class: "mono", "aria-label": "Cron expression", oninput: (ev) => { form.expr = ev.target.value; clearTimeout(previewTimer); previewTimer = setTimeout(askPreview, 300); } })));
        kids.push(field("Time zone (empty: the tenant's)", el("input", { type: "text", value: form.tz, placeholder: "local", "aria-label": "Time zone", oninput: (ev) => { form.tz = ev.target.value; clearTimeout(previewTimer); previewTimer = setTimeout(askPreview, 300); } })));
        if (preview) kids.push(el("p", { class: "muted", role: "status", text: preview }));
        if (previewErr) kids.push(el("p", { class: "err", role: "alert", text: previewErr }));
      } else if (form.kind === "interval") {
        kids.push(field("Seconds between runs", el("input", { type: "text", inputmode: "numeric", value: String(form.every_s), "aria-label": "Seconds between runs", oninput: (ev) => { form.every_s = ev.target.value; } })));
      } else {
        kids.push(field("Date and time", el("input", { type: "datetime-local", "aria-label": "Date and time", onchange: (ev) => { form.at_ms = localToMs(ev.target.value); } })));
      }
      kids.push(field("If a run was missed", sel([["run_once", "Run once when back"], ["skip", "Skip it"]], form.missed, (v) => { form.missed = v; }, "Missed run policy")));
      kids.push(field("If the last run is still going", sel([["skip", "Skip this one"], ["queue", "Queue it"], ["allow", "Run both"]], form.overlap, (v) => { form.overlap = v; }, "Overlap policy")));
      kids.push(field("Run as", sel([["owner", "Me (my role is checked every time)"], ["team-service", "The team (team admins only)"]], form.run_as, (v) => { form.run_as = v; }, "Run as")));
      kids.push(el("p", { class: "muted", text: "It runs with nobody signed in. It can use only team secrets a team admin marked for scheduled runs, and never a personal Claude or Copilot sign-in." }));
      const bad = checkForm(form, Date.now());
      kids.push(el("div", { class: "inline" }, [
        el("button", { type: "button", class: "primary", text: editing ? "Save changes" : "Create schedule", disabled: busy || !!bad, title: bad, onclick: () => act("Saved the schedule.", () => api.post("/api/v1/schedules", saveBody(form))) }),
        editing ? el("button", { type: "button", text: "Cancel", onclick: () => { editing = null; form.id = ""; draw(); } }) : null,
      ]));
      return el("div", { class: "card", role: "region", "aria-label": "Schedule form" }, kids.filter(Boolean));
    }

    function rowFor(s) {
      const st = rowState(s);
      const name = s.name || s.flow;
      const kids = [
        el("span", { class: "name", text: C.clip(name, 60) }),
        el("span", { class: "mono muted", text: describeTrigger(s.trigger) }),
        el("span", { class: "badge " + st.cls, text: st.text }),
      ];
      const box = el("div", { class: "row" }, kids);
      const extra = [];
      if (st.why) extra.push(el("p", { class: st.cls, text: st.why }));
      extra.push(el("p", { class: "muted", text: "Next: " + nextText(s.next) + (s.team ? ". Team " + C.clip(s.team, 40) + (s.run_as === "team-service" ? ", runs as the team" : ", runs as " + C.clip(s.owner || "you", 40)) : ". Not yet attached to a team: save it again to attach one.") }));
      extra.push(el("div", { class: "inline" }, [
        el("button", { type: "button", text: s.paused ? "Resume" : "Pause", disabled: busy, "aria-label": (s.paused ? "Resume " : "Pause ") + name, onclick: () => act(s.paused ? "Resumed." : "Paused.", () => api.post(pausePath(s.flow, !s.paused))) }),
        el("button", { type: "button", text: "Run now", disabled: busy, "aria-label": "Run " + name + " now", onclick: () => act("Started a run.", () => api.post("/api/v1/flows/" + encodeURIComponent(s.flow) + "/run", s.active === false ? { draft: true } : {})) }),
        el("button", { type: "button", text: "Change", "aria-label": "Change " + name, onclick: () => { editing = s; const t = s.trigger || {}; form = Object.assign(form, { id: t.id, flow: s.flow, team: s.team || form.team, kind: t.kind || "cron", expr: t.expr || form.expr, tz: t.tz && t.tz !== "local" ? t.tz : "", every_s: String(t.every_s || 3600), at_ms: t.at_ms || 0, missed: (t.missed && typeof t.missed === "string" ? t.missed : "run_once"), overlap: t.overlap || "skip", run_as: s.run_as || "owner", enabled: t.enabled !== false }); askPreview(); draw(); } }),
        el("button", { type: "button", class: "danger", text: "Delete", disabled: busy, "aria-label": "Delete the schedule of " + name, onclick: () => act("Deleted.", () => api.call("DELETE", deletePath(s.flow, (s.trigger || {}).id))) }),
        el("a", { href: "#/flows", text: "History" }),
      ]));
      return [box].concat(extra);
    }

    function draw() {
      body.textContent = "";
      if (status) body.appendChild(el("p", { class: "muted", role: "status", text: statusLine(status) }));
      const card = el("div", { class: "card" }, [el("h2", { text: "Schedules" })]);
      body.appendChild(card);
      if (state === "loading") card.appendChild(C.stateNode(el, "loading"));
      else if (state !== "ok") card.appendChild(C.stateNode(el, state, msg));
      else {
        if (!list.length) card.appendChild(el("p", { class: "muted", text: "No schedules yet. Create one below." }));
        for (const s of list) for (const n of rowFor(s)) card.appendChild(n);
        if (limit) card.appendChild(el("p", { class: "muted", text: list.length + " of " + limit + " schedules used in this tenant." }));
        body.appendChild(formBox());
      }
      body.appendChild(el("p", { class: "muted" }, ["Questions that wait for an approval are answered on the page ", el("a", { href: "#/runs", text: "Runs" }), "; runs and their history are on ", el("a", { href: "#/flows", text: "Flows" }), "."]));
    }
    await load();
  }

  const exported = { page, checkCron, checkInterval, checkOnce, buildTrigger, saveBody, checkForm, describeTrigger, nextText, rowState, refusalText, statusLine, pausePath, deletePath, previewPath, localToMs, PREVIEW_COUNT, MIN_INTERVAL_S };
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
  else C.registerPage(root, page());
})(typeof window !== "undefined" ? window : globalThis);
