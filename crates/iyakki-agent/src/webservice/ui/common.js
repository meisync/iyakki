/*
  SPDX-License-Identifier: Apache-2.0
  Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

  Shared by the pages (files.js, git.js, flows.js, runs.js): the page registry, the four states every page has (loading, empty,
  error, not allowed), sizes and times as text, and a redaction of anything that looks like a secret before it is drawn. Loaded
  before the pages and before app.js. No outside library; text is always set as text.
*/
"use strict";
(function (root) {
  // every page file pushes itself into this array; app.js reads it once at start
  function registerPage(r, page) {
    (r.IyakkiPages = r.IyakkiPages || []).push(page);
  }

  function formatSize(n) {
    const v = Number(n);
    if (!isFinite(v) || v < 0) return "";
    if (v < 1024) return v + " B";
    const units = ["KiB", "MiB", "GiB", "TiB"];
    let x = v / 1024, i = 0;
    while (x >= 1024 && i < units.length - 1) { x /= 1024; i++; }
    return (x >= 100 ? x.toFixed(0) : x.toFixed(1)) + " " + units[i];
  }

  // the service gives seconds since 1970 (or milliseconds); anything else is shown as it came
  function formatTime(t) {
    const n = Number(t);
    if (!isFinite(n) || n <= 0) return t === undefined || t === null ? "" : String(t);
    const d = new Date(n > 1e12 ? n : n * 1000);
    return isNaN(d.getTime()) ? String(t) : d.toISOString().replace("T", " ").slice(0, 19) + " UTC";
  }

  function clip(s, max) {
    const t = String(s === undefined || s === null ? "" : s);
    return t.length > max ? t.slice(0, max) + " ... (" + (t.length - max) + " more characters)" : t;
  }

  // which of the page states an answer is: offline, signin, denied, missing, error (or "ok")
  function stateKind(status) {
    if (status >= 200 && status < 300) return "ok";
    if (status === 0) return "offline";
    if (status === 401) return "signin";
    if (status === 403) return "denied";
    if (status === 404) return "missing";
    return "error";
  }

  // words for a state that has no message of its own
  const STATE_TITLES = { loading: "Loading ...", empty: "Nothing here yet.", offline: "Cannot reach the service.", signin: "Sign in again.", denied: "Not allowed for your role.", missing: "Not found.", error: "That did not work.", planned: "Planned." };

  // one box for a state; screen readers hear it (role=status, or role=alert for a failure)
  function stateNode(el, kind, message) {
    const bad = kind === "offline" || kind === "denied" || kind === "error" || kind === "missing" || kind === "signin";
    return el("div", { class: "card state state-" + kind, role: bad ? "alert" : "status" }, [
      el("p", { class: bad ? "err" : "muted", text: message || STATE_TITLES[kind] || "" }),
    ]);
  }

  // Anything that looks like a secret is hidden before it is drawn, copied or logged. This is a safety net: pages do not ask for secrets.
  const SECRET_PATTERNS = [
    [/(authorization\s*[:=]\s*)(bearer\s+)?[^\s"',;]+/gi, "$1[hidden]"],
    [/(bearer\s+)[A-Za-z0-9._~+\/=-]{8,}/gi, "$1[hidden]"],
    [/((?:pass(?:word|wd)?|token|secret|api[_-]?key|private[_-]?key)["']?\s*[:=]\s*["']?)[^\s"',;&]+/gi, "$1[hidden]"],
    [/\b(sk|ghp|gho|ghs|xox[abp])[-_][A-Za-z0-9_-]{12,}/g, "[hidden]"],
    [/(:\/\/[^\/\s:@]+:)[^\/\s@]+@/g, "$1[hidden]@"],
    [/-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?(-----END [A-Z ]*PRIVATE KEY-----|$)/g, "[hidden private key]"],
  ];
  function redact(text) {
    let t = String(text === undefined || text === null ? "" : text);
    for (const [re, to] of SECRET_PATTERNS) t = t.replace(re, to);
    return t;
  }

  // a small guard against two overlapping loads: only the latest answer is drawn
  function latest() {
    let n = 0;
    return { next: () => ++n, isCurrent: (i) => i === n };
  }

  const exported = { registerPage, formatSize, formatTime, clip, stateKind, stateNode, STATE_TITLES, redact, latest };
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
  else root.IyakkiCommon = exported;
})(typeof window !== "undefined" ? window : globalThis);
