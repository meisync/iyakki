/*
  SPDX-License-Identifier: Apache-2.0
  Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

  The web UI shell: sign-in against the service's own sessions (POST /api/v1/login, a bearer token kept for this tab only), a top
  bar, navigation and the page registry. Pages (cloud.js, remote.js) register themselves in IyakkiPages and are drawn into #page.
  Every action is a call to the same /api/v1 routes as any other client; the server checks everything and this code only hides.
  No outside library, no inline script, no request to any other address (the Content-Security-Policy forbids it).
  Loads under node for the tests (module.exports) with stand-ins passed in `env`.
*/
"use strict";
(function (root) {
  const TOKEN_KEY = "iyakki.token";
  const POLL_MS = 5000;

  // ---- small helpers (text is always set as text, never parsed as markup)
  function make(doc) {
    return function el(tag, attrs, kids) {
      const n = doc.createElement(tag);
      for (const k of Object.keys(attrs || {})) {
        const v = attrs[k];
        if (v === false || v === null || v === undefined) continue;
        if (k === "text") n.textContent = v;
        else if (k === "class") n.className = v;
        else if (k.startsWith("on") && typeof v === "function") n.addEventListener(k.slice(2), v);
        else n.setAttribute(k, v === true ? "" : String(v));
      }
      for (const c of [].concat(kids || [])) {
        if (c === null || c === undefined || c === false) continue;
        n.appendChild(typeof c === "string" ? doc.createTextNode(c) : c);
      }
      return n;
    };
  }

  // The service answers {"error":{"code","message"}}; its message is for people. A 403 reads the same in every page.
  function messageFor(status, body) {
    const m = body && body.error && body.error.message;
    if (status === 0) return "Cannot reach the service. Check the network and try again.";
    if (status === 401) return "Sign in again.";
    if (status === 403) return m ? "Not allowed: " + m : "Not allowed for your role.";
    if (status === 429) return "Too many requests. Wait a moment.";
    if (status >= 500) return "The service had a problem (" + status + ").";
    return m || "The request was refused (" + status + ").";
  }

  // `#/cloud` -> "cloud"; an unknown or unavailable page falls back to the first one that is available.
  function pageFromHash(hash, available) {
    const id = String(hash || "").replace(/^#\/?/, "");
    return available.find((p) => p.id === id) || available[0] || null;
  }

  // ---- the API client: one place that adds the token and turns every answer into {ok, status, data, message}
  function createApi(env) {
    const storage = env.storage;
    let onSignedOut = function () {};
    // true when the browser is already signed in to the desktop on this origin: the server bridges its cookie session to a token, so
    // these calls carry the (same-origin, HttpOnly) cookie and no token of ours
    let viaSession = false;
    const credentials = () => (viaSession ? "same-origin" : "omit");
    async function call(method, path, body) {
      const headers = { Accept: "application/json" };
      const token = storage.getItem(TOKEN_KEY);
      if (token) headers.Authorization = "Bearer " + token;
      const init = { method, headers, cache: "no-store", credentials: credentials(), referrerPolicy: "no-referrer" };
      if (body !== undefined) {
        headers["Content-Type"] = "application/json";
        init.body = JSON.stringify(body);
      }
      let res;
      try {
        res = await env.fetch(path, init);
      } catch (e) {
        return { ok: false, status: 0, data: null, message: messageFor(0, null) };
      }
      let data = null;
      try { data = await res.json(); } catch (e) { data = null; }
      if (res.status === 401 && path !== "/api/v1/login") {
        storage.removeItem(TOKEN_KEY);
        viaSession = false;
        onSignedOut();
      }
      return { ok: res.status >= 200 && res.status < 300, status: res.status, data, message: res.status >= 200 && res.status < 300 ? "" : messageFor(res.status, data) };
    }
    return {
      call,
      get: (p) => call("GET", p),
      post: (p, b) => call("POST", p, b === undefined ? {} : b),
      hasToken: () => !!storage.getItem(TOKEN_KEY),
      // for the pages that must handle bytes themselves (a download, an upload with progress): the same headers and credentials
      authHeaders() {
        const token = storage.getItem(TOKEN_KEY);
        return token ? { Authorization: "Bearer " + token } : {};
      },
      async raw(method, path) {
        const init = { method, headers: Object.assign({}, this.authHeaders()), cache: "no-store", credentials: credentials(), referrerPolicy: "no-referrer" };
        try {
          const res = await env.fetch(path, init);
          if (res.status === 401) { storage.removeItem(TOKEN_KEY); viaSession = false; onSignedOut(); }
          return res;
        } catch (e) {
          return { ok: false, status: 0, json: async () => null, blob: async () => null };
        }
      },
      expire() { storage.removeItem(TOKEN_KEY); viaSession = false; onSignedOut(); },
      inSession: () => viaSession,
      // Already signed in to the desktop in this browser (cookie session on this origin)? Then `whoami` answers and no form is needed.
      async trySession() {
        let res;
        try {
          res = await env.fetch("/api/v1/whoami", { method: "GET", headers: { Accept: "application/json" }, cache: "no-store", credentials: "same-origin", referrerPolicy: "no-referrer" });
        } catch (e) { return false; }
        if (res.status !== 200) return false;
        viaSession = true;
        return true;
      },
      setSignedOut: (f) => { onSignedOut = f; },
      async login(name, password) {
        const r = await call("POST", "/api/v1/login", { name, password });
        if (r.ok && r.data && r.data.token) { storage.setItem(TOKEN_KEY, r.data.token); viaSession = false; }
        if (r.status === 401) r.message = "The name or the password is not right.";
        return r;
      },
      async logout() {
        await call("POST", "/api/v1/logout", {});
        storage.removeItem(TOKEN_KEY);
        viaSession = false;
      },
    };
  }

  // ---- the browser part
  function start(env) {
    const doc = env.document;
    const el = make(doc);
    const api = createApi(env);
    const $ = (id) => doc.getElementById(id);
    let me = null; // {user, admin, role, profile}
    let current = null;
    let stoppers = [];

    function say(text, kind) {
      const s = $("status");
      s.textContent = text || "";
      s.className = kind || "";
    }

    // TODO(web-ui): the API has no events WebSocket yet for these pages. Until it has, a page polls with this helper (every few
    // seconds, only while the tab is visible, stopped when the page changes). Replace the callers with the event stream when
    // the one WebSocket of docs/development/WEB-UI-PLAN.md section 3.2 exists; do not add a second protocol here.
    function poll(fn, ms) {
      const t = env.setInterval(() => { if (!doc.hidden) fn(); }, ms || POLL_MS);
      const stop = () => env.clearInterval(t);
      stoppers.push(stop);
      return stop;
    }

    // run `fn` when the page is left (a page that holds a live connection or a listener on the window)
    function onLeave(fn) { stoppers.push(fn); }

    function ctx() {
      return { api, el, say, poll, onLeave, me, doc, messageFor };
    }

    function pagesAvailable() {
      return (env.pages || []).filter((p) => !p.available || p.available(me));
    }

    function drawNav(pages, active) {
      const nav = $("nav");
      nav.textContent = "";
      for (const p of pages) {
        nav.appendChild(el("button", { type: "button", text: p.title, "aria-current": p.id === (active && active.id) ? "page" : false, onclick: () => { env.location.hash = "#/" + p.id; } }));
      }
    }

    async function show() {
      for (const s of stoppers) s();
      stoppers = [];
      say("");
      const pages = pagesAvailable();
      const page = pageFromHash(env.location.hash, pages);
      drawNav(pages, page);
      const host = $("page");
      host.textContent = "";
      host.hidden = false;
      current = page;
      if (!page) {
        host.appendChild(el("p", { class: "card", text: "Nothing to show for this account on this machine." }));
        return;
      }
      host.appendChild(el("h1", { text: page.title }));
      try {
        await page.render(ctx(), host);
      } catch (e) {
        say("This page could not be drawn.", "err");
      }
      $("main").focus();
    }

    function signedOut() {
      for (const s of stoppers) s();
      stoppers = [];
      me = null;
      if (env.tenant) env.tenant.clear($("tenant"), env.storage);
      if (env.bell) env.bell.stop();
      $("top").hidden = true;
      $("page").hidden = true;
      $("signin").hidden = false;
      $("pass").value = "";
      $("user").focus();
    }

    async function enter() {
      const who = await api.get("/api/v1/whoami");
      if (!who.ok) { signedOut(); return; }
      const ver = await api.get("/api/v1/version");
      me = { user: who.data.user, admin: !!who.data.admin, role: who.data.role, profile: ver.ok ? ver.data.profile : who.data.profile };
      $("signin").hidden = true;
      $("top").hidden = false;
      $("who").textContent = me.user + " (" + me.role + ")";
      // the tenant control (tenant.js): the name and role from the API, a switch when the account has several tenants
      if (env.tenant) await env.tenant.mount({ api, el, slot: $("tenant"), say, storage: env.storage, location: env.location, onSwitched: () => show() });
      // the bell (bell.js): counts from the server, kept fresh over the event stream, polling when there is none
      if (env.bell) env.bell.mount({ api, el, slot: $("bell"), location: env.location, WebSocket: env.WebSocket, setTimeout: env.setTimeout, clearTimeout: env.clearTimeout, setInterval: env.setInterval, clearInterval: env.clearInterval, go: (id) => { env.location.hash = "#/" + id; } });
      await show();
    }

    api.setSignedOut(signedOut);
    $("signin-form").addEventListener("submit", async (ev) => {
      ev.preventDefault();
      const go = $("signin-go");
      go.disabled = true;
      say("");
      const r = await api.login($("user").value.trim(), $("pass").value);
      go.disabled = false;
      $("pass").value = "";
      if (r.ok) await enter();
      else say(r.message, "err");
    });
    $("signout").addEventListener("click", async () => { await api.logout(); signedOut(); say("Signed out.", "ok"); });
    env.window.addEventListener("hashchange", () => { if (me) show(); });
    // a token of this tab first; else the desktop's own sign-in if this browser has one; else the form
    if (api.hasToken()) enter(); else api.trySession().then((yes) => (yes ? enter() : signedOut()));
    return { api, show: () => current };
  }

  const exported = { make, messageFor, pageFromHash, createApi, start, TOKEN_KEY, POLL_MS };
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
  else {
    root.IyakkiUI = exported;
    // the browser: start once the page is there; the scripts are at the end of <body>
    start({
      document: root.document,
      window: root,
      location: root.location,
      storage: root.sessionStorage,
      fetch: root.fetch.bind(root),
      setInterval: root.setInterval.bind(root),
      clearInterval: root.clearInterval.bind(root),
      pages: root.IyakkiPages || [],
      tenant: root.iyakkiTenant,
      bell: root.iyakkiBell,
      WebSocket: root.WebSocket,
      setTimeout: root.setTimeout.bind(root),
      clearTimeout: root.clearTimeout.bind(root),
    });
  }
})(typeof window !== "undefined" ? window : globalThis);
