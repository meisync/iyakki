/*
  SPDX-License-Identifier: Apache-2.0
  Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

  Page "Tenant settings" (cloud profile; tenant and team administrators): the web face of GET/PUT/DELETE /api/v1/tenant/config...
  (plan: docs/development/TENANT-CONFIG-PLAN.md). Items with their effective state, a form generated from the registry the server
  returns, a dry run, the safety warnings with the acknowledgements the server asks for, versions and rollback, export and import
  (config only), the approvals inbox, approver roles and groups, and a test button that shows a status only.
  Secrets: a secret field is a write-only box. It never shows a stored value, only [set], [not set] or [vault locked]; what the
  person types is sent once in `secrets` and the box is cleared. The server decides every action; this page only hides what a role
  cannot do. No outside library; text is always set as text.
*/
"use strict";
(function (root) {
  const C = (typeof require === "function" && typeof module !== "undefined") ? require("./common.js") : root.IyakkiCommon;
  const CFG = "/api/v1/tenant/config";

  // ---- pure helpers (tested under node)

  // The badge of an item: its effective state as the server named it, plus a lock for a managed one.
  const STATES = {
    ready: { text: "ready", cls: "ok", title: "Switched on and every secret is set." },
    secret_missing: { text: "secret missing", cls: "warn", title: "Switched on, but a secret is not in the vault yet. Nothing is used until it is set." },
    vault_locked: { text: "vault locked", cls: "warn", title: "The vault is locked. Nothing is used until it is unlocked." },
    disabled: { text: "disabled", cls: "muted", title: "Switched off here or by a higher level." },
    invalid: { text: "invalid", cls: "err", title: "Switched on but not valid or blocked by a safety rule." },
  };
  function stateBadge(item) {
    const s = STATES[item && item.state] || { text: String((item && item.state) || "unknown"), cls: "muted", title: "" };
    return { text: s.text, cls: s.cls, title: s.title, source: item && item.source === "file" ? "file" : "api", locked: !!(item && item.managed) };
  }

  // What a secret box says about the stored value. The value itself never reaches the page.
  function secretLabel(v) {
    if (v === "[set]") return "[set]";
    if (v === "[vault locked]") return "[vault locked]";
    return "[not set]";
  }

  // "int 0..100" / "bool" / "text(Host) <= 80" / "one of a|b" / "list(Host) <= 5 x 80" / "secret_ref ns"
  function fieldType(f) {
    const t = String((f && f.type) || "");
    if (f && f.secret) return { kind: "secret" };
    if (t === "bool") return { kind: "bool" };
    let m = t.match(/^int (-?\d+)\.\.(-?\d+)$/);
    if (m) return { kind: "int", min: Number(m[1]), max: Number(m[2]) };
    m = t.match(/^one of (.+)$/);
    if (m) return { kind: "enum", options: m[1].split("|") };
    if (t.startsWith("list(")) return { kind: "list" };
    return { kind: "text" };
  }

  // The value the form holds for a field, as text; lists are one entry per line.
  function toInput(f, v) {
    const ty = fieldType(f).kind;
    if (ty === "secret") return "";
    if (ty === "list") return Array.isArray(v) ? v.join("\n") : "";
    if (v === undefined || v === null) return "";
    return String(v);
  }
  // The JSON value from the form's text; an empty field is left out (undefined).
  function fromInput(f, text, checked) {
    const ty = fieldType(f).kind;
    if (ty === "bool") return !!checked;
    const t = String(text === undefined || text === null ? "" : text).trim();
    if (ty === "secret" || t === "") return undefined;
    if (ty === "int") return /^-?\d+$/.test(t) ? Number(t) : t; // a bad number stays text so the server names the field
    if (ty === "list") return t.split(/[\n,]/).map((x) => x.trim()).filter(Boolean);
    return t;
  }

  // The body of a PUT or a dry run. Secrets go only into `secrets`, and only when the person typed something; a secret field
  // that already holds a reference keeps it by sending nothing for it (the server keeps the stored reference).
  function buildBody(fields, form, ack) {
    const values = {}, secrets = {};
    for (const f of fields) {
      const raw = form.values ? form.values[f.name] : undefined;
      if (f.secret) {
        const typed = form.secrets ? form.secrets[f.name] : "";
        if (typed) secrets[f.name] = String(typed);
        continue;
      }
      const v = fromInput(f, raw, raw === true);
      if (v !== undefined) values[f.name] = v;
    }
    const body = { enabled: !!form.enabled, values, secrets, ack_warnings: ackList(form.warnings || [], ack || {}) };
    if (form.namespace) body.namespace = form.namespace;
    if (form.name) body.name = form.name;
    return body;
  }

  // Which warnings must be ticked: every `warn`; a `block` cannot be ticked away; `info` needs nothing.
  function ackNeeded(warnings) {
    return (warnings || []).filter((w) => w && w.severity === "warn").map((w) => w.code);
  }
  // The codes to send: only warnings that are on the current list AND ticked.
  function ackList(warnings, ticked) {
    const need = ackNeeded(warnings);
    return need.filter((c) => ticked && ticked[c] === true);
  }
  function hasBlock(warnings) {
    return (warnings || []).some((w) => w && w.severity === "block");
  }
  // May the Save button be pressed? (the server checks again)
  function canSave(form, warnings, ticked) {
    if (hasBlock(warnings)) return false;
    if (!form.enabled) return true;
    return ackNeeded(warnings).every((c) => ticked && ticked[c] === true);
  }

  const SEVERITY_ORDER = { block: 0, warn: 1, info: 2 };
  function sortWarnings(ws) {
    return (ws || []).slice().sort((a, b) => (SEVERITY_ORDER[a.severity] ?? 3) - (SEVERITY_ORDER[b.severity] ?? 3) || String(a.code).localeCompare(String(b.code)));
  }

  // The team x environment matrix of k3s bindings: rows are teams, columns the environments (the chain, then any others).
  function k3sMatrix(items, chain) {
    const cells = (items || []).filter((i) => i.kind === "k3s.cluster" && i.namespace);
    const envs = (chain && chain.length ? chain.slice() : ["dev", "preprod", "prod"]);
    for (const i of cells) if (!envs.includes(i.name)) envs.push(i.name);
    const teams = [...new Set(cells.map((i) => i.namespace))].sort();
    const rows = teams.map((t) => ({ team: t, cells: envs.map((e) => cells.find((i) => i.namespace === t && i.name === e) || null) }));
    return { envs, rows };
  }

  // The page's gate: tenant owner/admin, or an administrator of at least one team.
  function mayUse(view) {
    if (!view) return false;
    if (view.tenant_role === "admin" || view.tenant_role === "owner") return true;
    return (view.namespaces || []).some((n) => n && n.joined && n.role === "admin");
  }

  // Who each approval row shows, in words.
  function approvalText(a) {
    return (a.requester || "?") + " asks for " + (a.kind || "?") + " " + (a.item || "") + " in team " + (a.team || "?") + (a.escalated ? " (escalated to tenant administrators)" : "");
  }

  function parseNames(text) {
    return String(text || "").split(/[\n,]/).map((x) => x.trim()).filter(Boolean);
  }

  function path(kind, ns, name) {
    const k = encodeURIComponent(kind);
    if (ns && name) return CFG + "/" + k + "/" + encodeURIComponent(ns) + "/" + encodeURIComponent(name);
    if (name && name !== "default") return CFG + "/" + k + "/" + encodeURIComponent(name);
    return CFG + "/" + k;
  }

  // ---- the page

  function page() {
    return { id: "tenantcfg", title: "Tenant settings", available: (me) => !!me && me.profile === "cloud", render };
  }

  async function render(ctx, host) {
    const { api, el, say } = ctx;
    const body = el("div");
    host.appendChild(body);
    const ver = C.latest();
    let st = { state: "loading", msg: "" }, data = null, tview = null, approvals = [], versions = [], edit = null, tab = "items", teamsView = null;

    async function load() {
      const id = ver.next();
      st = { state: "loading", msg: "" }; draw();
      const t = await api.get("/api/v1/tenant");
      if (!ver.isCurrent(id)) return;
      if (!t.ok) { st = { state: C.stateKind(t.status), msg: t.message }; return draw(); }
      tview = t.data;
      if (!mayUse(tview)) { st = { state: "denied", msg: "Tenant settings are for tenant and team administrators." }; return draw(); }
      const r = await api.get(CFG);
      if (!ver.isCurrent(id)) return;
      if (!r.ok) { st = { state: C.stateKind(r.status), msg: r.message }; return draw(); }
      data = r.data;
      const a = await api.get("/api/v1/tenant/approvals");
      approvals = a.ok ? (a.data.approvals || []) : [];
      const v = await api.get(CFG + "/versions");
      versions = v.ok ? (v.data.versions || []) : [];
      st = { state: "ok", msg: "" };
      draw();
    }

    const isTenantAdmin = () => !!tview && (tview.tenant_role === "admin" || tview.tenant_role === "owner");
    const kinds = () => ((data && data.registry && data.registry.kinds) || []);

    function itemRow(it) {
      const b = stateBadge(it);
      const kids = [
        el("span", { class: "name mono", text: it.kind + (it.namespace ? " / " + it.namespace : "") + (it.name !== "default" ? " / " + it.name : "") }),
        el("span", { class: "badge " + b.cls, text: b.text, title: b.title }),
        el("span", { class: "badge", text: "source: " + b.source, title: "Where this item comes from" }),
      ];
      if (b.locked) kids.push(el("span", { class: "badge", text: "🔒 managed", title: "Managed by the operator's file; it cannot be changed here." }));
      if (it.severity && it.severity !== "none") kids.push(el("span", { class: "badge " + (it.severity === "block" ? "err" : it.severity === "warn" ? "warn" : ""), text: it.severity }));
      if (it.can && it.can.write) kids.push(el("button", { type: "button", text: "Edit", onclick: () => { edit = { spec: kinds().find((k) => k.kind === it.kind), item: it, warnings: it.warnings || [], ticked: {}, secrets: {}, result: null }; tab = "edit"; draw(); } }));
      const row = el("div", { class: "row" }, kids);
      const extra = [];
      for (const r of it.reasons || []) extra.push(el("p", { class: "muted", text: C.redact(r) }));
      return el("div", {}, [row, ...extra]);
    }

    function itemsTab() {
      const box = el("div", { class: "card" }, [el("h2", { text: "Items" })]);
      const items = (data.items || []);
      box.appendChild(el("p", { class: "muted", text: "Vault: " + data.vault + (data.file_error ? ". The operator's file is broken, so none of its items apply." : "") }));
      if (!items.length) box.appendChild(el("p", { class: "muted", text: "Nothing is configured yet. Everything is off until an administrator adds it." }));
      for (const it of items) box.appendChild(itemRow(it));
      const out = [box];
      const chain = (items.find((i) => i.kind === "k3s.environments" && i.effective && Array.isArray(i.effective.order)) || {}).effective;
      const mx = k3sMatrix(items, chain && chain.order);
      if (mx.rows.length) {
        const t = el("table", { class: "matrix" }, [
          el("tr", {}, [el("th", { text: "Team" }), ...mx.envs.map((e) => el("th", { text: e }))]),
          ...mx.rows.map((r) => el("tr", {}, [el("th", { text: r.team }), ...r.cells.map((c) => el("td", {}, c ? [el("span", { class: "badge " + stateBadge(c).cls, text: stateBadge(c).text })] : [el("span", { class: "muted", text: "-" })]))])),
        ]);
        out.push(el("div", { class: "card" }, [el("h2", { text: "Kubernetes bindings (team by environment)" }), t]));
      }
      const add = el("div", { class: "card" }, [el("h2", { text: "Add or change an item" })]);
      const sel = el("select", { "aria-label": "Kind" }, kinds().map((k) => el("option", { value: k.kind, text: k.kind })));
      const ns = el("input", { type: "text", "aria-label": "Team (empty: the tenant)", placeholder: "Team (empty: tenant level)", autocapitalize: "none", spellcheck: "false" });
      const nm = el("input", { type: "text", "aria-label": "Name", placeholder: "Name (default for a single value)", autocapitalize: "none", spellcheck: "false" });
      add.appendChild(el("div", { class: "inline" }, [sel, ns, nm, el("button", { type: "button", text: "New", onclick: () => {
        const spec = kinds().find((k) => k.kind === sel.value);
        edit = { spec, item: { kind: spec.kind, namespace: ns.value.trim() || null, name: nm.value.trim() || "default", enabled: false, values: {} }, warnings: [], ticked: {}, secrets: {}, result: null, isNew: true };
        tab = "edit"; draw();
      } })]));
      if (!kinds().length) add.appendChild(el("p", { class: "muted", text: "The list of kinds is shown to tenant administrators. A team administrator edits an item from the list above." }));
      out.push(add);
      return out;
    }

    function editTab() {
      const e = edit;
      if (!e || !e.spec) return [C.stateNode(el, "empty", "Choose an item first.")];
      const fields = e.spec.fields || [];
      const form = { enabled: !!e.item.enabled, values: {}, secrets: e.secrets, namespace: e.item.namespace, name: e.item.name === "default" ? "" : e.item.name, warnings: e.warnings };
      const inputs = {};
      for (const f of fields) form.values[f.name] = toInput(f, e.item.values && e.item.values[f.name]);
      const card = el("div", { class: "card" }, [el("h2", { text: e.item.kind + (e.item.namespace ? " / " + e.item.namespace : "") + (e.item.name !== "default" ? " / " + e.item.name : "") }), el("p", { class: "muted", text: e.spec.summary || "" })]);
      const en = el("input", { type: "checkbox", id: "tc-enabled", checked: form.enabled });
      en.addEventListener("change", () => { form.enabled = en.checked; refresh(); });
      card.appendChild(el("label", {}, [en, " Switched on"]));
      for (const f of fields) {
        const t = fieldType(f);
        const id = "tc-f-" + f.name;
        let input;
        if (t.kind === "secret") {
          const cur = secretLabel(e.item.values && e.item.values[f.name]);
          input = el("input", { type: "password", id, autocomplete: "new-password", autocapitalize: "none", spellcheck: "false", placeholder: "Type a new value to replace it", "aria-label": f.name + " (write only)" });
          input.addEventListener("input", () => { e.secrets[f.name] = input.value; });
          card.appendChild(el("div", { class: "field" }, [el("label", { for: id, text: f.name + " " }), el("span", { class: "badge", text: cur }), input, el("p", { class: "muted", text: f.help || "" })]));
        } else if (t.kind === "bool") {
          input = el("input", { type: "checkbox", id, checked: (e.item.values && e.item.values[f.name]) === true });
          input.addEventListener("change", () => { form.values[f.name] = input.checked; });
          form.values[f.name] = input.checked;
          card.appendChild(el("div", { class: "field" }, [el("label", {}, [input, " " + f.name]), el("p", { class: "muted", text: f.help || "" })]));
        } else if (t.kind === "enum") {
          input = el("select", { id }, [el("option", { value: "", text: "(not set)" }), ...t.options.map((o) => el("option", { value: o, text: o, selected: String(form.values[f.name]) === o }))]);
          input.addEventListener("change", () => { form.values[f.name] = input.value; });
          card.appendChild(el("div", { class: "field" }, [el("label", { for: id, text: f.name + (f.required ? " *" : "") }), input, el("p", { class: "muted", text: f.help || "" })]));
        } else {
          input = t.kind === "list" ? el("textarea", { id, rows: 3, autocapitalize: "none", spellcheck: "false" }) : el("input", { type: "text", id, autocapitalize: "none", spellcheck: "false", inputmode: t.kind === "int" ? "numeric" : false });
          input.value = form.values[f.name];
          input.addEventListener("input", () => { form.values[f.name] = input.value; });
          card.appendChild(el("div", { class: "field" }, [el("label", { for: id, text: f.name + (f.required ? " *" : "") + (t.kind === "list" ? " (one per line)" : "") }), input, el("p", { class: "muted", text: f.help || "" })]));
        }
        inputs[f.name] = input;
      }
      const warnBox = el("div", { class: "card" });
      const buttons = el("div", { class: "inline" });
      function refresh() {
        warnBox.textContent = "";
        const ws = sortWarnings(e.warnings);
        if (ws.length) warnBox.appendChild(el("h3", { text: "Safety warnings" }));
        for (const w of ws) {
          const row = [el("span", { class: "badge " + (w.severity === "block" ? "err" : w.severity === "warn" ? "warn" : ""), text: w.severity }), " ", el("span", { text: C.redact(w.text) }), " ", el("span", { class: "muted mono", text: w.code })];
          if (w.severity === "warn") {
            const cb = el("input", { type: "checkbox", "aria-label": "Acknowledge " + w.code, checked: e.ticked[w.code] === true });
            cb.addEventListener("change", () => { e.ticked[w.code] = cb.checked; refresh(); });
            warnBox.appendChild(el("div", { class: "row" }, [cb, ...row]));
          } else warnBox.appendChild(el("div", { class: "row" }, row));
        }
        if (ws.length && form.enabled && !canSave(form, e.warnings, e.ticked)) warnBox.appendChild(el("p", { class: "muted", text: hasBlock(e.warnings) ? "A blocking rule must be fixed before saving." : "Tick each warning to switch this on." }));
        save.disabled = !canSave(form, e.warnings, e.ticked);
      }
      const dry = el("button", { type: "button", text: "Check (dry run)", onclick: async () => {
        const r = await api.post(path(e.item.kind, null, null) + "/validate", buildBody(fields, form, e.ticked));
        if (!r.ok) { say(r.message, "err"); return; }
        e.warnings = r.data.warnings || [];
        const probs = (r.data.problems || []).map((p) => p.field + ": " + p.message);
        say(probs.length ? "Problems: " + probs.join("; ") : "Valid. It would read: " + ((r.data.would_be || {}).state || "unknown") + ".", probs.length ? "err" : "ok");
        refresh();
      } });
      const save = el("button", { type: "button", class: "primary", text: "Save", onclick: async () => {
        const r = await api.call("PUT", path(e.item.kind, e.item.namespace, e.item.name), buildBody(fields, form, e.ticked));
        for (const k of Object.keys(e.secrets)) e.secrets[k] = ""; // the typed secret is gone either way
        for (const f of fields) if (f.secret && inputs[f.name]) inputs[f.name].value = "";
        if (!r.ok) { say(r.message, "err"); return; }
        say("Saved as version " + r.data.version + (r.data.pending_approval ? ". Waiting for approval." : "."), "ok");
        edit = null; tab = "items"; await load();
      } });
      const del = e.isNew ? null : el("button", { type: "button", text: "Remove", onclick: async () => {
        const r = await api.call("DELETE", path(e.item.kind, e.item.namespace, e.item.name));
        if (!r.ok) { say(r.message, "err"); return; }
        say("Removed. Secrets stay in the vault.", "ok"); edit = null; tab = "items"; await load();
      } });
      const test = (e.spec.kind === "integration.apikey" || e.spec.kind === "models.local") && !e.isNew ? el("button", { type: "button", text: "Test connection", onclick: async () => {
        const r = await api.post(path(e.item.kind, e.item.namespace, e.item.name) + "/test", {});
        if (!r.ok) { say(r.message, "err"); return; }
        say("Test: " + (r.data.ok ? "ok" : "failed") + (r.data.status ? " (HTTP " + r.data.status + ")" : ""), r.data.ok ? "ok" : "err");
      } }) : null;
      for (const b of [dry, save, test, del, el("button", { type: "button", text: "Back", onclick: () => { edit = null; tab = "items"; draw(); } })]) if (b) buttons.appendChild(b);
      card.appendChild(buttons);
      refresh();
      return [card, warnBox];
    }

    function versionsTab() {
      const card = el("div", { class: "card" }, [el("h2", { text: "Versions" })]);
      if (!versions.length) card.appendChild(el("p", { class: "muted", text: "No saved versions yet." }));
      for (const v of versions) {
        card.appendChild(el("div", { class: "row" }, [
          el("span", { class: "name", text: "v" + v.version + " " + C.formatTime(v.at) + " by " + v.by }), el("span", { class: "muted", text: v.summary + " (" + v.items + " items)" }),
          isTenantAdmin() && v.version !== data.version ? el("button", { type: "button", text: "Roll back", onclick: async () => {
            const r = await api.post(CFG + "/rollback", { version: v.version });
            say(r.ok ? "Restored as a new version." : r.message, r.ok ? "ok" : "err"); if (r.ok) await load();
          } }) : null,
        ]));
      }
      const io = el("div", { class: "card" }, [el("h2", { text: "Export and import" }), el("p", { class: "muted", text: "Configuration only. Secrets are never exported; references are." })]);
      const ta = el("textarea", { rows: 6, "aria-label": "Configuration document", spellcheck: "false", autocapitalize: "none" });
      io.appendChild(ta);
      io.appendChild(el("div", { class: "inline" }, [
        el("button", { type: "button", text: "Export", onclick: async () => { const r = await api.get(CFG + "/export"); if (r.ok) ta.value = JSON.stringify(r.data, null, 2); else say(r.message, "err"); } }),
        el("button", { type: "button", text: "Check import", disabled: !isTenantAdmin(), onclick: async () => {
          let doc; try { doc = JSON.parse(ta.value); } catch (x) { say("That is not valid JSON.", "err"); return; }
          const r = await api.post(CFG + "/import", { doc, dry_run: true });
          say(r.ok ? "Valid: " + r.data.would_import + " item(s) would be imported." : r.message, r.ok ? "ok" : "err");
        } }),
        el("button", { type: "button", class: "primary", text: "Import", disabled: !isTenantAdmin(), onclick: async () => {
          let doc; try { doc = JSON.parse(ta.value); } catch (x) { say("That is not valid JSON.", "err"); return; }
          const r = await api.post(CFG + "/import", { doc });
          say(r.ok ? "Imported." : r.message, r.ok ? "ok" : "err"); if (r.ok) await load();
        } }),
      ]));
      return [card, io];
    }

    function approvalsTab() {
      const card = el("div", { class: "card" }, [el("h2", { text: "Approvals waiting for you" })]);
      if (!approvals.length) card.appendChild(el("p", { class: "muted", text: "Nothing is waiting for you." }));
      // approve and deny are always the person's own click: there is no "approve all" and nothing fires on a timer
      for (const a of approvals) {
        const decide = async (yes) => {
          const r = await api.post("/api/v1/tenant/approvals/" + encodeURIComponent(a.id) + (yes ? "/approve" : "/deny"), {});
          say(r.ok ? (yes ? "Approved." : "Denied.") : r.message, r.ok ? "ok" : "err"); await load();
        };
        card.appendChild(el("div", { class: "row" }, [el("span", { text: approvalText(a) }), el("span", { class: "muted", text: "expires " + C.formatTime(a.expires) }),
          el("button", { type: "button", class: "primary", text: "Approve", onclick: () => decide(true) }), el("button", { type: "button", text: "Deny", onclick: () => decide(false) })]));
      }
      return [card];
    }

    async function loadTeam(ns) {
      const [ap, gr] = await Promise.all([api.get("/api/v1/namespaces/" + encodeURIComponent(ns) + "/approvers"), api.get("/api/v1/namespaces/" + encodeURIComponent(ns) + "/groups")]);
      teamsView = { ns, approvers: ap.ok ? ap.data.approvers : null, groups: gr.ok ? gr.data.groups : null, msg: ap.ok ? "" : ap.message };
      draw();
    }
    function teamsTab() {
      const names = isTenantAdmin() ? ["_tenant", ...(tview.namespaces || []).map((n) => n.name)] : (tview.namespaces || []).filter((n) => n.role === "admin").map((n) => n.name);
      const sel = el("select", { "aria-label": "Team" }, names.map((n) => el("option", { value: n, text: n === "_tenant" ? "(tenant-wide groups)" : n, selected: teamsView && teamsView.ns === n })));
      const out = [el("div", { class: "card" }, [el("h2", { text: "Approver roles and groups" }), el("div", { class: "inline" }, [sel, el("button", { type: "button", text: "Open", onclick: () => loadTeam(sel.value) })])])];
      if (!teamsView) return out;
      const ns = teamsView.ns, enc = encodeURIComponent(ns);
      if (teamsView.msg) out.push(C.stateNode(el, "error", teamsView.msg));
      if (teamsView.approvers && ns !== "_tenant") {
        const card = el("div", { class: "card" }, [el("h3", { text: "Approver roles of " + ns }), el("p", { class: "muted", text: "The role only allows approving requests of that scope in this team. Scope: an environment (prod) or a kind (k3s.cluster)." })]);
        for (const [scope, h] of Object.entries(teamsView.approvers)) {
          card.appendChild(el("div", { class: "row" }, [el("span", { class: "name mono", text: scope }), el("span", { text: "people: " + (h.people || []).join(", ") + " groups: " + (h.groups || []).join(", ") }),
            el("button", { type: "button", text: "Revoke all", onclick: async () => { const r = await api.call("DELETE", "/api/v1/namespaces/" + enc + "/approvers/" + encodeURIComponent(scope), { people: h.people || [], groups: h.groups || [] }); say(r.ok ? "Revoked." : r.message, r.ok ? "ok" : "err"); await loadTeam(ns); } })]));
        }
        const sc = el("input", { type: "text", "aria-label": "Scope", placeholder: "Scope, e.g. prod", autocapitalize: "none" });
        const pp = el("input", { type: "text", "aria-label": "People", placeholder: "People (comma separated)", autocapitalize: "none" });
        const gg = el("input", { type: "text", "aria-label": "Groups", placeholder: "Groups (comma separated)", autocapitalize: "none" });
        card.appendChild(el("div", { class: "inline" }, [sc, pp, gg, el("button", { type: "button", text: "Grant", onclick: async () => {
          const r = await api.call("PUT", "/api/v1/namespaces/" + enc + "/approvers/" + encodeURIComponent(sc.value.trim()), { people: parseNames(pp.value), groups: parseNames(gg.value) });
          say(r.ok ? "Granted." : r.message, r.ok ? "ok" : "err"); await loadTeam(ns);
        } })]));
        out.push(card);
      }
      if (teamsView.groups) {
        const card = el("div", { class: "card" }, [el("h3", { text: "Groups of " + (ns === "_tenant" ? "the tenant" : ns) })]);
        for (const [g, members] of Object.entries(teamsView.groups)) {
          card.appendChild(el("div", { class: "row" }, [el("span", { class: "name mono", text: g }), el("span", { text: (members.members || members || []).join ? (members.members || members).join(", ") : "" }),
            el("button", { type: "button", text: "Delete", onclick: async () => { const r = await api.call("DELETE", "/api/v1/namespaces/" + enc + "/groups/" + encodeURIComponent(g)); say(r.ok ? "Deleted." : r.message, r.ok ? "ok" : "err"); await loadTeam(ns); } })]));
        }
        const gn = el("input", { type: "text", "aria-label": "Group name", placeholder: "Group name", autocapitalize: "none" });
        const gm = el("input", { type: "text", "aria-label": "Members", placeholder: "People (comma separated)", autocapitalize: "none" });
        card.appendChild(el("div", { class: "inline" }, [gn, gm, el("button", { type: "button", text: "Save group", onclick: async () => {
          const r = await api.call("PUT", "/api/v1/namespaces/" + enc + "/groups/" + encodeURIComponent(gn.value.trim()), { members: parseNames(gm.value) });
          say(r.ok ? "Saved." : r.message, r.ok ? "ok" : "err"); await loadTeam(ns);
        } })]));
        out.push(card);
      }
      return out;
    }

    function draw() {
      body.textContent = "";
      if (st.state !== "ok") { body.appendChild(C.stateNode(el, st.state, st.msg)); return; }
      const tabs = [["items", "Items"], ["approvals", "Approvals" + (approvals.length ? " (" + approvals.length + ")" : "")], ["versions", "Versions"], ["teams", "Approvers and groups"]];
      body.appendChild(el("div", { class: "card inline" }, tabs.map(([id, label]) => el("button", { type: "button", text: label, "aria-current": tab === id || (tab === "edit" && id === "items") ? "true" : false, onclick: () => { tab = id; edit = null; draw(); } }))));
      const parts = tab === "edit" ? editTab() : tab === "approvals" ? approvalsTab() : tab === "versions" ? versionsTab() : tab === "teams" ? teamsTab() : itemsTab();
      for (const p of parts) body.appendChild(p);
    }

    await load();
  }

  const exported = { page, stateBadge, secretLabel, fieldType, toInput, fromInput, buildBody, ackNeeded, ackList, hasBlock, canSave, sortWarnings, k3sMatrix, mayUse, approvalText, parseNames, path };
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
  else C.registerPage(root, page());
})(typeof window !== "undefined" ? window : globalThis);
