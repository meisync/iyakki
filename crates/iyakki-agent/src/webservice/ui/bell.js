/*
  SPDX-License-Identifier: Apache-2.0
  Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

  The bell in the top bar: the number of unread notifications and approvals waiting, red while an approval waits. It reads
  GET /api/v1/notifications/count (the server counts from the stored inbox) and keeps itself fresh over the event stream
  (/api/v1/events: a WebSocket that signs in with its first message; the token is never in the address). When the stream is not
  there (a cookie-only sign-in, a proxy that blocks it, an error) it polls every 30 seconds instead and tries the stream again with a
  growing pause. The page's other parts can listen with `subscribe(fn)`. No outside library; text is set as text.
  Loads under node for the tests (module.exports) with stand-ins in `env`.
*/
"use strict";
(function (root) {
  const POLL_MS = 30000;
  const listeners = [];
  let session = null;

  // "99+" for a lot, "" for none
  function badgeText(n) {
    const v = Number(n);
    if (!isFinite(v) || v <= 0) return "";
    return v > 99 ? "99+" : String(Math.floor(v));
  }

  // what a screen reader says
  function label(c) {
    const unread = (c && c.unread) | 0, appr = (c && c.approvals) | 0;
    const parts = [];
    parts.push(unread === 1 ? "1 unread notification" : unread + " unread notifications");
    if (appr) parts.push(appr === 1 ? "1 approval waiting" : appr + " approvals waiting");
    return "Notifications: " + parts.join(", ");
  }

  // the address of the stream on this origin; the token is not part of it
  function socketUrl(loc) {
    return (loc.protocol === "https:" ? "wss:" : "ws:") + "//" + loc.host + "/api/v1/events";
  }

  // seconds to wait before the n-th retry: 5, 10, 20, 40, then 60
  function retryDelay(attempt) {
    return Math.min(60, 5 * Math.pow(2, Math.max(0, attempt)));
  }

  // a message of the stream as {type, data}, or null when it is not one of ours
  function parseMessage(text) {
    let v;
    try { v = JSON.parse(text); } catch (e) { return null; }
    if (!v || typeof v.type !== "string") return null;
    return { type: v.type, data: v.data || {} };
  }

  // the bearer token of the sign-in, for the first message of the stream (null for a cookie-only sign-in)
  function tokenOf(api) {
    const h = api && api.authHeaders ? api.authHeaders() : {};
    const a = h.Authorization || "";
    return a.startsWith("Bearer ") ? a.slice(7) : null;
  }

  function subscribe(fn) {
    listeners.push(fn);
    return () => { const i = listeners.indexOf(fn); if (i >= 0) listeners.splice(i, 1); };
  }
  function fire(ev) {
    for (const f of listeners.slice()) { try { f(ev); } catch (e) { /* a listener must not stop the bell */ } }
  }

  function stop() {
    if (session) session.stop();
    session = null;
  }

  // Draw the bell into `slot` and keep it fresh. env: {api, el, slot, location, WebSocket, setTimeout, clearTimeout, setInterval, clearInterval, go}
  function mount(env) {
    stop();
    const { api, el, slot } = env;
    let counts = { unread: 0, approvals: 0, red: false };
    let live = false, socket = null, attempt = 0, timer = null, poller = null, stopped = false, pending = false;
    const Sock = env.WebSocket;

    const badge = el("span", { class: "bell-badge", "aria-hidden": "true", text: "" });
    const btn = el("button", { type: "button", class: "bell", "aria-label": label(counts), onclick: () => { if (env.go) env.go("notifications"); } }, [el("span", { class: "bell-icon", "aria-hidden": "true", text: "Bell" }), badge]);
    slot.textContent = "";
    slot.appendChild(btn);

    function draw() {
      const shown = (counts.unread | 0) + (counts.approvals | 0) + (counts.overdue | 0);
      badge.textContent = badgeText(shown);
      btn.setAttribute("aria-label", label(counts));
      btn.className = "bell" + (counts.red ? " bell-red" : "") + (shown ? " bell-on" : "");
    }

    async function refresh() {
      if (stopped || pending) return;
      pending = true;
      try {
        const r = await api.get("/api/v1/notifications/count");
        if (r.ok && r.data) { counts = r.data; draw(); fire({ type: "count", data: counts }); }
      } finally { pending = false; }
    }

    function startPolling() {
      if (poller || stopped) return;
      poller = env.setInterval(() => { if (!live) refresh(); }, POLL_MS);
    }

    function connect() {
      if (stopped || !Sock) { startPolling(); return; }
      const token = tokenOf(api);
      if (!token) { startPolling(); return; } // a cookie-only sign-in cannot sign in a socket: poll
      let s;
      try { s = new Sock(socketUrl(env.location)); } catch (e) { startPolling(); return; }
      socket = s;
      s.onopen = () => { s.send(JSON.stringify({ type: "auth", token })); };
      s.onmessage = (m) => {
        const msg = parseMessage(m.data);
        if (!msg) return;
        if (msg.type === "ready") { live = true; attempt = 0; refresh(); return; }
        if (msg.type === "notification" || msg.type === "count" || msg.type === "approval") { refresh(); fire(msg); }
      };
      s.onclose = () => {
        live = false;
        socket = null;
        if (stopped) return;
        startPolling();
        timer = env.setTimeout(connect, retryDelay(attempt++) * 1000);
      };
      s.onerror = () => { /* onclose follows */ };
    }

    session = {
      stop() {
        stopped = true;
        if (timer) env.clearTimeout(timer);
        if (poller) env.clearInterval(poller);
        if (socket) { try { socket.close(); } catch (e) { /* already closed */ } }
      },
      refresh,
      connected: () => live,
    };
    draw();
    refresh();
    connect();
    return session;
  }

  const exported = { mount, stop, subscribe, badgeText, label, socketUrl, retryDelay, parseMessage, tokenOf, POLL_MS };
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
  else root.iyakkiBell = exported;
})(typeof window !== "undefined" ? window : globalThis);
