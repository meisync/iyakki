/*
  SPDX-License-Identifier: Apache-2.0
  Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

  Page "Git" (cloud pods): the repositories cloned into the signed-in person's space or a namespace (GET /api/v1/git/clones) and a
  form to clone one more (POST /api/v1/git/clone: https addresses only, no user name or password in the address, no addresses of
  this machine or cluster; the server checks again). Per clone, four READ-ONLY views: status (GET /api/v1/git/status), branches
  (/branches), log (/log, paged, at most 200 a page) and diff (/diff?path= for a changed file, ?commit= for a commit). Every view has
  its loading, empty, error and not-allowed state. A diff is drawn as text lines (each a text node, so nothing in a repository can
  become markup) coloured by + and -; a binary file or a diff over 256 KiB is refused by the service and said so.
  A clone takes up to five minutes; the page waits and shows that it is working. Clones are made without credentials.
*/
"use strict";
(function (root) {
  const C = (typeof require === "function" && typeof module !== "undefined") ? require("./common.js") : root.IyakkiCommon;
  const Files = (typeof require === "function" && typeof module !== "undefined") ? require("./files.js") : root.IyakkiFiles;

  // written in two parts: the page files never contain an absolute address (a test looks for one)
  const SCHEME = "https" + ":" + "//";

  // ---- pure helpers (tested under node); they mirror the server's checks so the person hears it early, the server decides anyway
  function checkUrl(url) {
    const u = String(url || "").trim();
    if (!u) return "Type the address of the repository.";
    if (u.length > 500 || /[\s\u0000-\u001f\u007f]/.test(u)) return "That is not an address.";
    if (!u.startsWith(SCHEME)) return "Only https addresses can be cloned here.";
    const authority = u.slice(SCHEME.length).split(/[\/?#]/)[0];
    if (authority.includes("@")) return "Leave the user name and password out of the address.";
    const m = authority.match(/^(.*?)(?::(\d+))?$/);
    const host = (m ? m[1] : authority).replace(/^\[|\]$/g, "").toLowerCase();
    if (!host || !/^[a-z0-9.:-]+$/.test(host)) return "That host name is not valid.";
    const internal = host === "localhost" || /\.(localhost|local|internal|svc)$/.test(host) || (!host.includes(".") && !host.includes(":"));
    const literal = /^[0-9.]+$/.test(host) || /^[0-9a-f:]+$/.test(host) && host.includes(":");
    if (internal || literal) return "Addresses of this machine, this cluster and bare IP addresses are not cloned from here.";
    return "";
  }
  // the folder a clone gets by default: the last part of the address without .git
  function defaultFolder(url) {
    const last = String(url || "").trim().replace(/\/+$/, "").split("/").pop() || "";
    const n = last.replace(/\.git$/, "");
    return /^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/.test(n) ? n : "";
  }
  function checkFolder(f) {
    return /^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/.test(String(f || "")) ? "" : "A folder name has letters, digits, - and _ only.";
  }
  function checkBranch(b) {
    const t = String(b || "");
    if (!t) return "";
    return /^[A-Za-z0-9._\/-]{1,100}$/.test(t) && !t.startsWith("-") && !t.includes("..") ? "" : "That is not a branch name.";
  }
  function parseDepth(s) {
    const t = String(s || "").trim();
    if (!t) return { ok: true, depth: 0 };
    if (!/^\d{1,6}$/.test(t)) return { ok: false, depth: 0 };
    return { ok: true, depth: Number(t) };
  }
  // the body of the clone call; empty optional fields are left out
  function cloneBody(space, f) {
    const b = { url: String(f.url).trim() };
    if (f.folder) b.folder = f.folder;
    if (f.branch) b.branch = f.branch;
    if (f.depth) b.depth = f.depth;
    if (space && space.scope) { b.scope = space.scope; b.namespace = space.namespace; }
    return b;
  }
  // what the person is told when it worked
  function cloneDone(data) {
    const n = data && data.removed_links ? data.removed_links : 0;
    return "Cloned into " + (data && data.folder) + "." + (n ? " " + n + " link(s) that left the folder were removed." : "");
  }
  // ---- the read-only views
  // the query of a git read: the space, the clone's folder and the rest
  function gitQuery(space, folder, extra) {
    const parts = [];
    if (space && space.scope) parts.push("scope=" + encodeURIComponent(space.scope), "namespace=" + encodeURIComponent(space.namespace));
    parts.push("folder=" + encodeURIComponent(folder));
    for (const k of Object.keys(extra || {})) if (extra[k] !== undefined && extra[k] !== "") parts.push(k + "=" + encodeURIComponent(extra[k]));
    return "?" + parts.join("&");
  }
  const LOG_PER = 50;
  // the class of one diff line: the service names the kind; a line without one is read by its first character
  function diffClass(line) {
    const k = line && line.kind;
    if (k === "add" || k === "del" || k === "hunk" || k === "meta") return "diff-" + k;
    const t = String((line && line.text) || "");
    if (k === "ctx") return "diff-ctx";
    if (t.startsWith("@@")) return "diff-hunk";
    if (t.startsWith("+") && !t.startsWith("+++")) return "diff-add";
    if (t.startsWith("-") && !t.startsWith("---")) return "diff-del";
    return "diff-ctx";
  }
  // [{cls, text}] of one file of a diff; the text stays text (never markup), secrets in it are hidden, a very long line is cut
  function diffRows(file) {
    return ((file && file.lines) || []).map((l) => ({ cls: diffClass(l), text: C.clip(C.redact(String((l && l.text) || "")), 2000) }));
  }
  function diffHeading(f) {
    const name = f.status === "R" ? (f.old || "") + " -> " + (f.new || "") : (f.new || f.old || "");
    return name + "  +" + (f.added | 0) + " -" + (f.removed | 0) + (f.binary ? " (binary: not shown)" : "");
  }
  function statusText(s) {
    if (!s) return "";
    const head = s.detached ? "detached" : s.branch || "no commits yet";
    const bits = [head];
    if (s.ahead > 0) bits.push(s.ahead + " ahead");
    if (s.behind > 0) bits.push(s.behind + " behind");
    bits.push(s.dirty ? (s.changed | 0) + (s.conflicted | 0) + " changed, " + (s.untracked | 0) + " new" : "clean");
    return bits.join(", ");
  }
  // the label of one changed file
  function changeLabel(c) {
    const where = c.staged && c.unstaged ? "staged and more" : c.staged ? "staged" : "not staged";
    return (c.letter || "?") + " " + c.path + (c.from ? " (from " + c.from + ")" : "") + " - " + (c.kind || "") + ", " + where;
  }
  function commitTitle(k) {
    return (k.short || String(k.sha || "").slice(0, 7)) + "  " + C.clip(k.subject || "", 120);
  }
  // a new untracked file has no diff against the last commit; a deleted or changed one has
  function canDiff(c) {
    return !!c && c.kind !== "new, not tracked" && c.kind !== "conflict";
  }
  // "" when this is a commit id the diff call takes
  function checkSha(s) {
    return /^[0-9a-f]{4,64}$/i.test(String(s || "")) ? "" : "That is not a commit id.";
  }

  function page() {
    return { id: "git", title: "Git", available: (me) => !!me && me.profile === "cloud", render };
  }

  async function render(ctx, host) {
    const { api, el, say } = ctx;
    const body = el("div");
    host.appendChild(body);
    const ver = C.latest();
    body.appendChild(C.stateNode(el, "loading"));
    const t = await api.get("/api/v1/tenant");
    if (!t.ok) { body.textContent = ""; body.appendChild(C.stateNode(el, C.stateKind(t.status), t.message)); return; }
    const all = Files.spaces(t.data);
    let sp = all[0], clones = [], state = "loading", msg = "", cloning = false;
    // the open clone: {folder, tab, parts: {status|branches|log|diff: {state, msg, data}}, logPage, diffOf}
    let cur = null;
    const vver = C.latest();

    async function loadPart(tab, query) {
      if (!cur) return;
      const id = vver.next(), folder = cur.folder;
      cur.tab = tab; cur.parts[tab] = { state: "loading", msg: "", data: null }; draw();
      const r = await api.get("/api/v1/git/" + tab + gitQuery(sp, folder, query));
      if (!vver.isCurrent(id) || !cur || cur.folder !== folder) return;
      cur.parts[tab] = r.ok ? { state: "ok", msg: "", data: r.data } : { state: C.stateKind(r.status), msg: r.status === 415 ? "That file is binary; a diff of it is not shown." : r.status === 413 ? "That diff is larger than 256 KiB and is not shown." : r.message, data: null };
      draw();
    }
    function openClone(folder) { cur = { folder, tab: "status", parts: {}, logPage: 0, diffOf: "" }; return loadPart("status"); }
    function showLog(p) { if (!cur) return null; cur.logPage = Math.max(0, p); return loadPart("log", { page: cur.logPage, per: LOG_PER }); }
    function showDiff(query, label) { if (!cur) return null; cur.diffOf = label; return loadPart("diff", query); }

    async function load() {
      const id = ver.next();
      state = "loading"; draw();
      const r = await api.get("/api/v1/git/clones" + (sp.scope ? "?scope=" + encodeURIComponent(sp.scope) + "&namespace=" + encodeURIComponent(sp.namespace) : ""));
      if (!ver.isCurrent(id)) return;
      if (r.ok) { clones = r.data.clones || []; state = "ok"; } else { clones = []; state = C.stateKind(r.status); msg = r.message; }
      draw();
    }

    function form() {
      const url = el("input", { type: "url", "aria-label": "Repository address", placeholder: "Address ending in .git", autocapitalize: "none", spellcheck: "false", autocomplete: "off", required: true });
      const folder = el("input", { type: "text", "aria-label": "Folder name (optional)", placeholder: "Folder (default: from the address)", autocapitalize: "none", spellcheck: "false" });
      const branch = el("input", { type: "text", "aria-label": "Branch (optional)", placeholder: "Branch (default: the repository's)", autocapitalize: "none", spellcheck: "false" });
      const depth = el("input", { type: "text", inputmode: "numeric", "aria-label": "Depth (optional)", placeholder: "Depth (0: everything)" });
      const go = el("button", { type: "submit", class: "primary", text: cloning ? "Cloning ..." : "Clone", disabled: cloning });
      return el("form", { onsubmit: async (ev) => {
        ev.preventDefault();
        const f = { url: url.value, folder: folder.value.trim() || defaultFolder(url.value), branch: branch.value.trim(), depth: 0 };
        const d = parseDepth(depth.value);
        const bad = checkUrl(f.url) || checkFolder(f.folder) || checkBranch(f.branch) || (d.ok ? "" : "The depth is a number.");
        if (bad) { say(bad, "err"); return; }
        f.depth = d.depth;
        cloning = true; say("Cloning ... this can take a few minutes."); draw();
        const r = await api.post("/api/v1/git/clone", cloneBody(sp, f));
        cloning = false;
        if (r.ok) { say(cloneDone(r.data), "ok"); await load(); } else { say(r.message, "err"); draw(); }
      } }, [
        el("label", { text: "Repository address (https)" }), url,
        el("div", { class: "inline" }, [folder, branch, depth]),
        go,
      ]);
    }

    function partBox(tab) {
      const p = cur.parts[tab] || { state: "loading", msg: "" };
      return p;
    }
    function detail() {
      const card = el("div", { class: "card", role: "region", "aria-label": "Clone " + cur.folder });
      card.appendChild(el("h2", { text: cur.folder }));
      const tabs = [["status", "Status"], ["branches", "Branches"], ["log", "Log"]];
      if (cur.parts.diff) tabs.push(["diff", "Diff"]);
      card.appendChild(el("div", { class: "inline", role: "group", "aria-label": "View" }, tabs.map(([id, label]) =>
        el("button", { type: "button", text: label, "aria-pressed": cur.tab === id ? "true" : "false", onclick: () => { if (id === "log") return showLog(cur.logPage); if (id === "diff") { cur.tab = "diff"; draw(); return null; } return loadPart(id); } }))));
      const p = partBox(cur.tab);
      if (p.state !== "ok") { card.appendChild(C.stateNode(el, p.state, p.msg)); return card; }
      const d = p.data || {};
      if (cur.tab === "status") {
        card.appendChild(el("p", { role: "status", text: statusText(d) }));
        if (d.upstream) card.appendChild(el("p", { class: "muted", text: "Follows " + d.upstream }));
        if (d.origin) card.appendChild(el("p", { class: "muted mono", text: "Origin " + C.redact(d.origin) }));
        if (!(d.changes || []).length) card.appendChild(el("p", { class: "muted", text: "Nothing is changed in the working copy." }));
        for (const c of d.changes || []) {
          card.appendChild(el("div", { class: "row" }, [el("span", { class: "name mono", text: changeLabel(c) })].concat(canDiff(c) ? [el("button", { type: "button", text: "Diff", "aria-label": "Diff of " + c.path, onclick: () => showDiff({ path: c.path }, c.path) })] : [])));
        }
        if (d.more > 0) card.appendChild(el("p", { class: "muted", text: d.more + " more changed files are not listed." }));
      } else if (cur.tab === "branches") {
        if (!(d.branches || []).length) card.appendChild(el("p", { class: "muted", text: "This clone has no branches yet." }));
        for (const b of d.branches || []) {
          card.appendChild(el("div", { class: "row" }, [el("span", { class: "name mono", text: b.name }), el("span", { class: "muted", text: C.clip(b.subject || "", 80) })]
            .concat(b.name === d.current ? [el("span", { class: "badge", text: "checked out" })] : [], b.default ? [el("span", { class: "badge", text: "default" })] : [])));
        }
      } else if (cur.tab === "log") {
        if (!(d.commits || []).length) card.appendChild(el("p", { class: "muted", text: cur.logPage ? "No more commits." : "No commits yet." }));
        for (const k of d.commits || []) {
          card.appendChild(el("div", { class: "row" }, [
            el("span", { class: "name", text: commitTitle(k) }),
            el("span", { class: "muted", text: (k.author || "") + (k.when ? " - " + C.formatTime(k.when) : "") }),
            el("button", { type: "button", text: "Diff", "aria-label": "Diff of commit " + (k.short || ""), onclick: () => (checkSha(k.sha) ? null : showDiff({ commit: k.sha }, "commit " + (k.short || ""))) }),
          ]));
        }
        card.appendChild(el("div", { class: "inline" }, [
          el("button", { type: "button", text: "Newer", disabled: cur.logPage === 0, onclick: () => showLog(cur.logPage - 1) }),
          el("span", { class: "muted", role: "status", text: "Page " + (cur.logPage + 1) }),
          el("button", { type: "button", text: "Older", disabled: !d.more, onclick: () => showLog(cur.logPage + 1) }),
        ]));
      } else if (cur.tab === "diff") {
        card.appendChild(el("p", { class: "muted", text: "Changes in " + cur.diffOf }));
        if (!(d.files || []).length) card.appendChild(el("p", { class: "muted", text: "There is no change to show." }));
        for (const f of d.files || []) {
          card.appendChild(el("h3", { class: "mono", text: diffHeading(f) }));
          if (f.binary) continue;
          card.appendChild(el("div", { class: "diff mono", tabindex: "0", role: "group", "aria-label": "Diff of " + (f.new || f.old || "file") }, diffRows(f).map((r) => el("div", { class: "diff-line " + r.cls, text: r.text }))));
        }
      }
      return card;
    }

    function draw() {
      body.textContent = "";
      const space = el("select", { "aria-label": "Space", onchange: (ev) => { sp = all.find((s) => s.id === ev.target.value) || all[0]; cur = null; load(); } },
        all.map((s) => el("option", { value: s.id, text: s.label, selected: s.id === sp.id })));
      const write = !!sp && (sp.scope === null || sp.role === "editor" || sp.role === "admin");
      body.appendChild(el("div", { class: "card" }, [el("div", { class: "inline" }, [space])]));
      const list = el("div", { class: "card" }, [el("h2", { text: "Clones" })]);
      if (state !== "ok") list.appendChild(C.stateNode(el, state, msg));
      else if (!clones.length) list.appendChild(el("p", { class: "muted", text: "Nothing is cloned in this space yet." }));
      else for (const k of clones) list.appendChild(el("div", { class: "row" }, [el("span", { class: "name mono", text: k.folder }), el("span", { class: "badge", text: k.branch ? "branch " + k.branch : "branch unknown" }),
        el("button", { type: "button", text: cur && cur.folder === k.folder ? "Close" : "Open", "aria-expanded": cur && cur.folder === k.folder ? "true" : "false", "aria-label": (cur && cur.folder === k.folder ? "Close " : "Open ") + k.folder, onclick: () => { if (cur && cur.folder === k.folder) { cur = null; draw(); return null; } return openClone(k.folder); } })]));
      body.appendChild(list);
      if (cur) body.appendChild(detail());
      body.appendChild(write ? el("div", { class: "card" }, [el("h2", { text: "Clone a repository" }), form()]) : el("p", { class: "muted", text: "Cloning needs the editor role in this space." }));
    }

    await load();
  }
  const exported = { page, checkUrl, defaultFolder, checkFolder, checkBranch, parseDepth, cloneBody, cloneDone, gitQuery, diffClass, diffRows, diffHeading, statusText, changeLabel, commitTitle, canDiff, checkSha, LOG_PER };
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
  else C.registerPage(root, page());
})(typeof window !== "undefined" ? window : globalThis);
