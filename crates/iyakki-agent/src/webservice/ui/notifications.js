/*
  SPDX-License-Identifier: Apache-2.0
  Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

  Page "Notifications": the signed-in person's own notifications (never anyone else's) and the approvals waiting for them. Unread ones
  are bold. Each can be marked read or unread, snoozed, acknowledged (which stops its escalation) or deleted. An approval is decided
  here, by the person's own click, through the same endpoints as any other client; nothing a message carries can decide for them.
  The four states every page has (loading, empty, error, not allowed) are drawn from common.js. Loads under node for the tests.
*/
"use strict";
(function (root) {
  const C = typeof require === "function" && typeof module !== "undefined" ? require("./common.js") : root.IyakkiCommon;
  const FILTERS = [{ id: "all", label: "All" }, { id: "unread", label: "Unread" }, { id: "warning", label: "Warnings" }, { id: "snoozed", label: "Snoozed" }];
  const SNOOZES = [{ minutes: 15, label: "15 minutes" }, { minutes: 60, label: "1 hour" }, { minutes: 240, label: "4 hours" }, { minutes: 1440, label: "Tomorrow" }];

  // what the list shows for a filter (the server already hides snoozed notes unless asked)
  function visible(list, filter) {
    const rows = Array.isArray(list) ? list.filter((n) => n && typeof n.id === "string") : [];
    if (filter === "unread") return rows.filter((n) => !n.read && !n.snoozed);
    if (filter === "warning") return rows.filter((n) => n.kind === "warning" || n.kind === "error");
    if (filter === "snoozed") return rows.filter((n) => n.snoozed);
    return rows.filter((n) => !n.snoozed);
  }

  function ago(t, now) {
    const s = Math.max(0, Math.floor(now - Number(t)));
    if (!isFinite(s)) return "";
    if (s < 60) return "just now";
    if (s < 3600) return Math.floor(s / 60) + " min ago";
    if (s < 86400) return Math.floor(s / 3600) + " h ago";
    return Math.floor(s / 86400) + " d ago";
  }

  // an approval row from the unified list: what to say and where each answer goes
  function approvalRow(a) {
    if (!a || typeof a !== "object") return null;
    if (a.kind === "tool_call" && a.run && a.call) {
      return { key: "t:" + a.run + ":" + a.call, kind: "tool_call", text: "A run asks to use " + String(a.tool || "a tool") + ": " + C.redact(a.summary || ""), approve: { path: "/api/v1/approvals/" + a.run + "/" + a.call, body: { verdict: "approve" } }, deny: { path: "/api/v1/approvals/" + a.run + "/" + a.call, body: { verdict: "deny" } } };
    }
    if (a.kind === "tenant" && a.id && a.decide && a.decide.approve && a.decide.deny) {
      const esc = a.escalated ? " (escalated to tenant administrators)" : "";
      return { key: "c:" + a.id, kind: "tenant", text: String(a.requester || "Someone") + " asks to change " + String(a.kind_of_change || "a setting") + " " + String(a.item || "") + " in " + String(a.team || "the tenant") + esc, approve: { path: a.decide.approve, body: {} }, deny: { path: a.decide.deny, body: {} } };
    }
    return null;
  }

  function page() {
    return { id: "notifications", title: "Notifications", available: (me) => !!me, render: renderPage };
  }

  async function renderPage(ctx, host) {
    const { api, el, say } = ctx;
    const body = el("div");
    host.appendChild(body);
    const guard = C.latest();
    let filter = "all", notes = [], next = null, approvals = [], state = "loading", message = "";
    const nowSec = () => Math.floor(Date.now() / 1000);

    async function load(more) {
      const mine = guard.next();
      const q = "/api/v1/notifications?limit=50" + (filter === "snoozed" ? "&snoozed=1" : "") + (filter === "unread" ? "&unread=1" : "") + (more && next ? "&before=" + next : "");
      const [r, ap] = await Promise.all([api.get(q), api.get("/api/v1/approvals")]);
      if (!guard.isCurrent(mine)) return;
      if (!r.ok) { state = C.stateKind(r.status); message = r.message; draw(); return; }
      const got = Array.isArray(r.data.notifications) ? r.data.notifications : [];
      notes = more ? notes.concat(got) : got;
      next = r.data.next_before || null;
      approvals = ap.ok && ap.data && Array.isArray(ap.data.approvals) ? ap.data.approvals.map(approvalRow).filter(Boolean) : [];
      state = "ok";
      draw();
    }

    async function act(path, payload, okText) {
      const r = await api.post(path, payload === undefined ? {} : payload);
      say(r.ok ? okText : r.message, r.ok ? "ok" : "err");
      await load(false);
    }

    async function remove(n) {
      const r = await api.call("DELETE", "/api/v1/notifications/" + n.id);
      say(r.ok ? "Deleted." : r.message, r.ok ? "ok" : "err");
      await load(false);
    }

    function noteNode(n) {
      const base = "/api/v1/notifications/" + n.id;
      const snoozeSel = el("select", { "aria-label": "Snooze for" }, SNOOZES.map((s) => el("option", { value: String(s.minutes), text: s.label })));
      const items = Array.isArray(n.items) && n.items.length > 1 ? el("ul", {}, n.items.slice(-5).map((i) => el("li", { text: C.clip(C.redact(i.text), 200) }))) : null;
      return el("li", { class: "card note" + (n.read ? "" : " unread") + (n.kind === "error" || n.kind === "warning" ? " note-" + n.kind : "") }, [
        el(n.read ? "span" : "strong", { class: "note-title", text: C.clip(C.redact(n.title), 200) }),
        el("span", { class: "muted", text: " " + String(n.kind || "") + " - " + ago(n.time, nowSec()) + (n.source ? " - " + n.source : "") + (n.count > 1 ? " (" + n.count + ")" : "") }),
        n.body ? el("p", { class: "line", text: C.clip(C.redact(n.body), 600) }) : null,
        items,
        el("div", { class: "inline" }, [
          el("button", { type: "button", text: n.read ? "Mark unread" : "Mark read", onclick: () => act(base + (n.read ? "/unread" : "/read"), {}, n.read ? "Marked unread." : "Marked read.") }),
          n.kind === "warning" || n.kind === "error" ? el("button", { type: "button", text: "Acknowledge", onclick: () => act(base + "/ack", {}, "Acknowledged.") }) : null,
          snoozeSel,
          el("button", { type: "button", text: "Snooze", onclick: () => act(base + "/snooze", { minutes: Number(snoozeSel.value) }, "Snoozed.") }),
          el("button", { type: "button", class: "danger", text: "Delete", onclick: () => remove(n) }),
        ]),
      ]);
    }

    function draw() {
      body.textContent = "";
      if (state !== "ok") { body.appendChild(C.stateNode(el, state, message)); return; }
      if (approvals.length) {
        body.appendChild(el("h2", { text: "Waiting for your decision" }));
        for (const a of approvals) {
          body.appendChild(el("div", { class: "card note-approval", role: "group", "aria-label": "Approval" }, [
            el("p", { text: a.text }),
            el("div", { class: "inline" }, [
              el("button", { type: "button", class: "primary", text: "Approve", onclick: () => act(a.approve.path, a.approve.body, "Approved.") }),
              el("button", { type: "button", text: "Deny", onclick: () => act(a.deny.path, a.deny.body, "Denied.") }),
            ]),
          ]));
        }
      }
      body.appendChild(el("div", { class: "inline", role: "toolbar", "aria-label": "Filter" }, FILTERS.map((f) => el("button", { type: "button", text: f.label, "aria-pressed": f.id === filter ? "true" : "false", onclick: () => { filter = f.id; load(false); } })).concat([
        el("button", { type: "button", text: "Mark all read", onclick: () => act("/api/v1/notifications/read-all", {}, "All marked read.") }),
      ])));
      const rows = visible(notes, filter);
      if (!rows.length) { body.appendChild(C.stateNode(el, "empty", filter === "all" ? "No notifications." : "Nothing matches this filter.")); return; }
      body.appendChild(el("ul", { class: "notes" }, rows.map(noteNode)));
      if (next) body.appendChild(el("button", { type: "button", text: "Load older", onclick: () => load(true) }));
    }

    draw();
    await load(false);
    // a push from the event stream (bell.js) reloads at once; the 30 second poll is the fallback
    const bell = root.iyakkiBell;
    if (bell && bell.subscribe) {
      const off = bell.subscribe((ev) => {
        if (ev.type !== "count" && host.parentNode === null) { off(); return; }
        if (ev.type !== "count") load(false);
      });
    }
    ctx.poll(() => load(false), 30000);
  }

  const exported = { page, visible, ago, approvalRow, FILTERS, SNOOZES };
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
  else C.registerPage(root, page());
})(typeof window !== "undefined" ? window : globalThis);
