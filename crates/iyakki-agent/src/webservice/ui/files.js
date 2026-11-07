/*
  SPDX-License-Identifier: Apache-2.0
  Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

  Page "Files" (cloud pods): browse a folder of the signed-in person's own space or of a namespace, open a text file, download any file,
  upload with a progress bar, make a folder, delete with a question first. Calls: GET /api/v1/files, /files/content, /files/download,
  PUT /files/upload, POST /files/write, /files/delete, /files/mkdir (a real folder) and /files/move (rename, same space only; asks
  again with overwrite:true after a 409, never silently). The server checks the role on every call (viewers read, editors write); this
  page hides the buttons a viewer cannot use. Folders made earlier with a ".keep" file still work; ".keep" is not listed.
  The upload uses XMLHttpRequest because fetch cannot report upload progress; it goes to this origin only, with the same bearer token.
*/
"use strict";
(function (root) {
  const C = (typeof require === "function" && typeof module !== "undefined") ? require("./common.js") : root.IyakkiCommon;
  const PAGE_SIZE = 100;
  const MAX_UPLOAD = 128 * 1024 * 1024; // cloudfiles::MAX_BINARY
  const TEXT_LIMIT = 512 * 1024; // cloudfiles::MAX_FILE
  const MARKER = ".keep";

  // ---- pure helpers (tested under node)
  function validName(s) {
    const t = String(s === undefined || s === null ? "" : s);
    return t.length > 0 && t.length <= 200 && t !== "." && t !== ".." && !/[\/\\\u0000-\u001f\u007f]/.test(t) && t.trim() === t;
  }
  function splitPath(p) {
    return String(p || "").split("/").filter((x) => x.length > 0);
  }
  function joinPath(dir, name) {
    return splitPath(dir).concat([name]).join("/");
  }
  function parentPath(p) {
    const parts = splitPath(p);
    parts.pop();
    return parts.join("/");
  }
  // [{label, path}] from the root to the folder; the root is called by the space's name
  function breadcrumbs(path, rootLabel) {
    const out = [{ label: rootLabel || "Files", path: "" }];
    let acc = "";
    for (const part of splitPath(path)) {
      acc = acc ? acc + "/" + part : part;
      out.push({ label: part, path: acc });
    }
    return out;
  }
  // the rename of one entry: "" when fine, else the words for the person. `existing` are the names in the same folder.
  function renameCheck(oldName, newName, existing) {
    if (!validName(newName)) return "Type a name without slash or control characters (at most 200 characters, no space at the ends).";
    if (newName === MARKER) return "That name is reserved.";
    if (newName === oldName) return "That is the name it has already.";
    if ((existing || []).some((n) => n === newName)) return newName + " exists already in this folder.";
    return "";
  }
  // the path a move to `dest` ("" = the open folder; "a/b" = a folder below it, or ".." parts are not accepted) gives; "" when refused
  function moveTarget(dir, destFolder, name) {
    const parts = splitPath(destFolder);
    if (parts.some((p) => p === ".." || p === "." || !validName(p))) return "";
    return joinPath(parts.join("/") ? parts.join("/") : dir, name);
  }
  // folders first, then files, by name without regard to case; the folder-marker file is not shown; a filter is a plain substring
  function arrange(entries, filter) {
    const f = String(filter || "").trim().toLowerCase();
    return (entries || [])
      .filter((e) => e && typeof e.name === "string" && e.name !== MARKER && (!f || e.name.toLowerCase().includes(f)))
      .slice()
      .sort((a, b) => ((a.kind === "dir") === (b.kind === "dir") ? a.name.toLowerCase().localeCompare(b.name.toLowerCase()) || (a.name < b.name ? -1 : a.name > b.name ? 1 : 0) : a.kind === "dir" ? -1 : 1));
  }
  // {items, page, pages, from, to, total}; a page number out of range is pulled in
  function pageSlice(list, page, size) {
    const n = size || PAGE_SIZE;
    const total = list.length;
    const pages = Math.max(1, Math.ceil(total / n));
    const p = Math.min(Math.max(0, page | 0), pages - 1);
    const items = list.slice(p * n, p * n + n);
    return { items, page: p, pages, from: total ? p * n + 1 : 0, to: p * n + items.length, total };
  }
  // the spaces: the person's own, and for each namespace its folder for the person and its shared one
  function spaces(tenant) {
    const out = [{ id: "personal", label: "My files", scope: null, namespace: null, role: "owner" }];
    for (const ns of (tenant && tenant.namespaces) || []) {
      out.push({ id: "person:" + ns.name, label: ns.name + " - mine", scope: "person", namespace: ns.name, role: ns.role });
      out.push({ id: "shared:" + ns.name, label: ns.name + " - shared", scope: "shared", namespace: ns.name, role: ns.role });
    }
    return out;
  }
  // hiding only: a viewer (or someone who is not in the namespace) is not offered writing; the server refuses all the same
  function canWrite(space) {
    return !!space && (space.scope === null || space.role === "editor" || space.role === "admin");
  }
  function query(space, path, extra) {
    const parts = [];
    if (space && space.scope) parts.push("scope=" + encodeURIComponent(space.scope), "namespace=" + encodeURIComponent(space.namespace));
    if (path) parts.push("path=" + encodeURIComponent(path));
    for (const k of Object.keys(extra || {})) parts.push(k + "=" + encodeURIComponent(extra[k]));
    return parts.length ? "?" + parts.join("&") : "";
  }
  function uploadCheck(file) {
    if (!file || !validName(file.name)) return "That file name cannot be used here (no slash, no control characters).";
    if (file.size > MAX_UPLOAD) return file.name + " is larger than " + C.formatSize(MAX_UPLOAD) + ".";
    return "";
  }
  function progressText(done, total) {
    if (!total) return "Starting ...";
    return C.formatSize(done) + " of " + C.formatSize(total) + " (" + Math.min(100, Math.floor((done * 100) / total)) + "%)";
  }
  // a text file the page may show: the API refuses bigger ones, so do not ask
  function canPreview(entry) {
    return !!entry && entry.kind === "file" && entry.size <= TEXT_LIMIT;
  }

  // ---- the browser part
  function page() {
    return { id: "files", title: "Files", available: (me) => !!me && me.profile === "cloud", render };
  }

  async function render(ctx, host) {
    const { api, el, say, doc } = ctx;
    const body = el("div");
    host.appendChild(body);
    const ver = C.latest();
    let tenant = null, list = [], sp = null, path = "", pageNo = 0, filter = "", sure = null, busy = false, editing = null, replace = null;
    let view = null; // {name, text} of an opened file
    let upload = null; // {name, done, total, xhr}

    body.appendChild(C.stateNode(el, "loading"));
    const t = await api.get("/api/v1/tenant");
    if (!t.ok) {
      body.textContent = "";
      body.appendChild(C.stateNode(el, C.stateKind(t.status), t.message));
      return;
    }
    tenant = t.data;
    const all = spaces(tenant);
    sp = all[0];

    async function load() {
      const id = ver.next();
      draw("loading");
      const r = await api.get("/api/v1/files" + query(sp, path));
      if (!ver.isCurrent(id)) return;
      if (!r.ok) { list = []; draw(C.stateKind(r.status), r.message); return; }
      list = (r.data.entries || []);
      draw("ok");
    }

    function go(newPath) { path = newPath; pageNo = 0; filter = ""; sure = null; editing = null; replace = null; view = null; load(); }

    async function act(p, b, okText) {
      if (busy) return;
      busy = true;
      const r = await api.post(p, b);
      busy = false;
      if (r.ok) { say(okText, "ok"); sure = null; editing = null; replace = null; await load(); } else { say(r.message, "err"); draw("ok"); }
    }

    // rename inside the open folder; a 409 (the name exists) asks first and only then repeats the call with overwrite:true
    async function rename(e, to, overwrite) {
      if (busy) return;
      const bad = renameCheck(e.name, to, overwrite ? [] : list.map((x) => x.name));
      if (bad) { say(bad, "err"); return; }
      busy = true;
      const r = await api.post("/api/v1/files/move", spaceBody({ from: joinPath(path, e.name), to: joinPath(path, to), overwrite: !!overwrite }));
      busy = false;
      if (r.ok) { say(e.name + " is now " + to + ".", "ok"); editing = null; replace = null; await load(); return; }
      if (r.status === 409 && e.kind === "file" && !overwrite) { replace = { entry: e, to }; draw("ok"); return; }
      say(r.message, "err");
      draw("ok");
    }

    function spaceBody(extra) {
      const b = {};
      if (sp.scope) { b.scope = sp.scope; b.namespace = sp.namespace; }
      return Object.assign(b, extra);
    }

    async function open(entry) {
      const full = joinPath(path, entry.name);
      say("");
      const r = await api.get("/api/v1/files/content" + query(sp, full));
      if (!r.ok) { say(r.message, "err"); return; }
      view = { name: entry.name, text: String(r.data.content || "") };
      draw("ok");
    }

    async function download(entry) {
      const full = joinPath(path, entry.name);
      say("Downloading " + entry.name + " ...");
      const res = await api.raw("GET", "/api/v1/files/download" + query(sp, full));
      if (!res.ok) {
        let data = null;
        try { data = await res.json(); } catch (e) { data = null; }
        say(res.status === 0 ? "Cannot reach the service. Check the network and try again." : ctx.messageFor ? ctx.messageFor(res.status, data) : "The download was refused (" + res.status + ").", "err");
        return;
      }
      const blob = await res.blob();
      const url = root.URL.createObjectURL(blob);
      const a = el("a", { href: url, download: entry.name, hidden: true });
      doc.body.appendChild(a);
      a.click();
      a.remove();
      root.setTimeout(() => root.URL.revokeObjectURL(url), 10000);
      say("Saved " + entry.name + " (" + C.formatSize(blob.size) + ").", "ok");
    }

    function startUpload(file) {
      const bad = uploadCheck(file);
      if (bad) { say(bad, "err"); return; }
      if (upload) { say("An upload is already running.", "err"); return; }
      const full = joinPath(path, file.name);
      const xhr = new root.XMLHttpRequest();
      upload = { name: file.name, done: 0, total: file.size, xhr };
      xhr.open("PUT", "/api/v1/files/upload" + query(sp, full));
      for (const [k, v] of Object.entries(api.authHeaders())) xhr.setRequestHeader(k, v);
      xhr.setRequestHeader("Accept", "application/json");
      xhr.upload.onprogress = (ev) => { if (upload) { upload.done = ev.loaded; upload.total = ev.total || file.size; drawUpload(); } };
      xhr.onload = () => {
        const ok = xhr.status >= 200 && xhr.status < 300;
        let data = null;
        try { data = JSON.parse(xhr.responseText); } catch (e) { data = null; }
        upload = null;
        if (xhr.status === 401) { api.expire(); return; }
        if (ok) { say("Uploaded " + file.name + ".", "ok"); load(); } else { say(ctx.messageFor ? ctx.messageFor(xhr.status, data) : "The upload was refused (" + xhr.status + ").", "err"); draw("ok"); }
      };
      xhr.onerror = () => { upload = null; say("The upload stopped: cannot reach the service.", "err"); draw("ok"); };
      xhr.onabort = () => { upload = null; say("Upload cancelled.", ""); draw("ok"); };
      xhr.send(file);
      drawUpload();
    }

    const upBox = el("div", { class: "card", hidden: true });
    function drawUpload() {
      upBox.textContent = "";
      upBox.hidden = !upload;
      if (!upload) return;
      const bar = el("progress", { max: String(upload.total || 1), value: String(upload.done), "aria-label": "Upload progress" });
      upBox.appendChild(el("p", { text: "Uploading " + upload.name }));
      upBox.appendChild(bar);
      upBox.appendChild(el("p", { class: "muted", role: "status", text: progressText(upload.done, upload.total) }));
      upBox.appendChild(el("button", { type: "button", text: "Cancel upload", onclick: () => { if (upload) upload.xhr.abort(); } }));
    }

    function toolbar() {
      const write = canWrite(sp);
      const space = el("select", { "aria-label": "Space", onchange: (ev) => { sp = all.find((s) => s.id === ev.target.value) || all[0]; go(""); } },
        all.map((s) => el("option", { value: s.id, text: s.label, selected: s.id === sp.id })));
      const fileIn = el("input", { type: "file", "aria-label": "Choose a file to upload", onchange: (ev) => { const f = ev.target.files && ev.target.files[0]; ev.target.value = ""; if (f) startUpload(f); } });
      const nm = el("input", { type: "text", "aria-label": "New folder name", placeholder: "New folder", autocapitalize: "none", spellcheck: "false" });
      const kids = [
        el("div", { class: "inline" }, [space]),
        el("nav", { "aria-label": "Folder path", class: "inline crumbs" }, breadcrumbs(path, sp.label).map((c, i, a) =>
          el("button", { type: "button", text: c.label, "aria-current": i === a.length - 1 ? "location" : false, onclick: () => go(c.path) }))),
      ];
      if (write) {
        kids.push(el("div", { class: "inline" }, [el("label", { class: "filebtn" }, ["Upload a file", fileIn])]));
        kids.push(el("form", { class: "inline", onsubmit: (ev) => {
          ev.preventDefault();
          if (!validName(nm.value)) { say("Type a folder name (no slash).", "err"); return; }
          act("/api/v1/files/mkdir", spaceBody({ path: joinPath(path, nm.value) }), "Folder " + nm.value + " is made.");
        } }, [nm, el("button", { type: "submit", text: "New folder" })]));
      } else {
        kids.push(el("p", { class: "muted", text: "You can read here but not change anything (your role is " + (sp.role || "viewer") + ")." }));
      }
      return kids;
    }

    function row(e) {
      const full = joinPath(path, e.name);
      const write = canWrite(sp);
      const isDir = e.kind === "dir";
      const name = isDir
        ? el("button", { type: "button", class: "name linkish", text: e.name + "/", "aria-label": "Open folder " + e.name, onclick: () => go(full) })
        : el("span", { class: "name", text: e.name + (e.kind === "link" ? " (link)" : "") });
      const acts = [];
      if (e.kind === "file") {
        acts.push(el("button", { type: "button", text: "Download", "aria-label": "Download " + e.name, onclick: () => download(e) }));
        if (canPreview(e)) acts.push(el("button", { type: "button", text: "View", "aria-label": "View " + e.name, onclick: () => open(e) }));
      }
      if (write) {
        acts.push(el("button", { type: "button", text: "Rename", "aria-label": "Rename " + e.name, onclick: () => { editing = e.name; sure = null; replace = null; draw("ok"); } }));
        acts.push(el("button", { type: "button", class: "danger", text: "Delete", "aria-label": "Delete " + e.name, onclick: () => { sure = e.name; draw("ok"); } }));
      }
      if (editing === e.name) {
        const inp = el("input", { type: "text", value: e.name, "aria-label": "New name for " + e.name, autocapitalize: "none", spellcheck: "false", maxlength: "200" });
        const form = el("form", { class: "card inline", onsubmit: (ev) => { ev.preventDefault(); rename(e, inp.value.trim() === inp.value ? inp.value : inp.value, false); } }, [
          inp,
          el("button", { type: "submit", text: "Rename" }),
          el("button", { type: "button", text: "Cancel", onclick: () => { editing = null; draw("ok"); } }),
        ]);
        root.setTimeout(() => { try { inp.focus(); inp.select(); } catch (x) { /* not in a browser */ } }, 0);
        return form;
      }
      if (replace && replace.entry.name === e.name) {
        return el("div", { class: "card confirm", role: "alertdialog", "aria-label": "Confirm replace" }, [
          el("p", { class: "warn", text: replace.to + " exists already. Replace it with " + e.name + "? The old one is lost." }),
          el("div", { class: "inline" }, [
            el("button", { type: "button", class: "danger", text: "Yes, replace", onclick: () => rename(e, replace.to, true) }),
            el("button", { type: "button", text: "Keep both", onclick: () => { replace = null; draw("ok"); } }),
          ]),
        ]);
      }
      const r = el("div", { class: "row" }, [name, el("span", { class: "muted", text: isDir ? "folder" : C.formatSize(e.size) })].concat(acts));
      if (sure === e.name) {
        const msg = isDir ? "Delete the folder " + e.name + " and everything in it? This cannot be undone." : "Delete " + e.name + "? This cannot be undone.";
        return el("div", { class: "card confirm", role: "alertdialog", "aria-label": "Confirm delete" }, [
          el("p", { class: "warn", text: msg }),
          el("div", { class: "inline" }, [
            el("button", { type: "button", class: "danger", text: "Yes, delete", onclick: () => act("/api/v1/files/delete", spaceBody({ path: full, recursive: isDir }), e.name + " is deleted.") }),
            el("button", { type: "button", text: "Keep it", onclick: () => { sure = null; draw("ok"); } }),
          ]),
        ]);
      }
      return r;
    }

    function draw(state, message) {
      body.textContent = "";
      body.appendChild(el("div", { class: "card" }, toolbar()));
      body.appendChild(upBox);
      drawUpload();
      if (view) {
        body.appendChild(el("div", { class: "card" }, [
          el("h2", { text: view.name }),
          el("pre", { class: "mono viewer", tabindex: "0", text: C.redact(view.text) }),
          el("button", { type: "button", text: "Close", onclick: () => { view = null; draw("ok"); } }),
        ]));
      }
      if (state !== "ok") { body.appendChild(C.stateNode(el, state, message)); return; }
      const shown = arrange(list, filter);
      const pg = pageSlice(shown, pageNo, PAGE_SIZE);
      const card = el("div", { class: "card" });
      if (list.length > 20) {
        card.appendChild(el("input", { type: "search", "aria-label": "Filter this folder by name", placeholder: "Filter by name", value: filter, onchange: (ev) => { filter = ev.target.value; pageNo = 0; draw("ok"); } }));
      }
      if (!shown.length) {
        card.appendChild(el("p", { class: "muted", text: filter ? "No name matches the filter." : "This folder is empty." }));
      }
      for (const e of pg.items) card.appendChild(row(e));
      if (pg.pages > 1) {
        card.appendChild(el("div", { class: "inline" }, [
          el("button", { type: "button", text: "Previous", disabled: pg.page === 0, onclick: () => { pageNo = pg.page - 1; draw("ok"); } }),
          el("span", { class: "muted", role: "status", text: pg.from + " to " + pg.to + " of " + pg.total }),
          el("button", { type: "button", text: "Next", disabled: pg.page >= pg.pages - 1, onclick: () => { pageNo = pg.page + 1; draw("ok"); } }),
        ]));
      }
      body.appendChild(card);
    }

    await load();
  }

  const exported = { page, validName, splitPath, joinPath, parentPath, breadcrumbs, arrange, renameCheck, moveTarget, pageSlice, spaces, canWrite, query, uploadCheck, progressText, canPreview, PAGE_SIZE, MAX_UPLOAD, MARKER };
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
  else {
    root.IyakkiFiles = exported; // git.js shares the list of spaces
    C.registerPage(root, page());
  }
})(typeof window !== "undefined" ? window : globalThis);
