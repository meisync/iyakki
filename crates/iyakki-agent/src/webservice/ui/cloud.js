/*
  SPDX-License-Identifier: Apache-2.0
  Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

  Page "Cloud access": the roles manager of a cloud tenant, the web twin of Settings -> Cloud access. It shows the signed-in
  account's tenant role, the tenant members (to owners and administrators), the namespaces the account may see and, for the ones it
  is in, their members. Every change is a call to /api/v1/tenant, /api/v1/tenant/members or /api/v1/namespaces; the server decides
  and audits, and its refusals are shown as they are. Removing a person asks first. Keys are rotated by the service; this page
  only says what is pending.
*/
"use strict";
(function (root) {
  const TENANT_ROLES = ["member", "admin", "owner"];
  const NS_ROLES = ["viewer", "editor", "admin"];

  // ---- what the page offers (hiding only: the server refuses what the role does not allow)
  function isTenantAdmin(view) {
    return !!view && (view.tenant_role === "admin" || view.tenant_role === "owner");
  }
  // an administrator may hand out member and admin; only an owner may make another owner
  function tenantRoleChoices(view) {
    if (!isTenantAdmin(view)) return [];
    return view.tenant_role === "owner" ? TENANT_ROLES.slice() : ["member", "admin"];
  }
  function canManageNamespaces(view) {
    return isTenantAdmin(view);
  }
  function canManageNsMembers(ns) {
    return !!ns && ns.joined === true && ns.role === "admin";
  }
  // a tenant administrator may join a namespace they are not in (recorded); the native page joins as editor
  function canJoin(view, ns) {
    return isTenantAdmin(view) && !!ns && ns.joined === false;
  }
  function validName(s) {
    const t = String(s || "").trim();
    return t.length > 0 && t.length <= 64 && !/[\s/\\]/.test(t);
  }
  // the rotation report of a removal, as one sentence
  function rotationText(data) {
    const r = data && data.rotation;
    if (!r) return "";
    const pend = (r.pending || []).length;
    const done = (r.rotated || []).length;
    if (pend) return " Keys: " + done + " rotated, " + pend + " pending (an administrator of the service finishes them).";
    return done ? " Keys rotated: " + done + "." : "";
  }

  function page() {
    return {
      id: "cloud",
      title: "Cloud access",
      available: (me) => !!me && me.profile === "cloud",
      render: renderPage,
    };
  }

  async function renderPage(ctx, host) {
    const { api, el, say } = ctx;
    const body = el("div");
    host.appendChild(body);
    let data = null; // {view, members: {ns: [...]}} or {state: "..."}
    let sure = null; // {kind, ns, name} while a removal waits for its answer
    let sig = "";

    async function load(force) {
      const t = await api.get("/api/v1/tenant");
      if (!t.ok) {
        const next = { state: t.status === 403 ? (ctx.me.admin ? "bootstrap" : "outsider") : "error", message: t.message };
        return commit(next, force);
      }
      const members = {};
      for (const ns of t.data.namespaces) {
        if (!ns.joined) continue;
        const m = await api.get("/api/v1/namespaces/" + encodeURIComponent(ns.name) + "/members");
        members[ns.name] = m.ok ? m.data.members : [];
      }
      return commit({ view: t.data, members }, force);
    }

    function commit(next, force) {
      const s = JSON.stringify(next);
      if (!force && s === sig) return;
      // never wipe what the person is typing: a polled refresh waits until focus leaves the page's fields
      const a = ctx.doc.activeElement;
      if (!force && a && body.contains(a) && /^(INPUT|SELECT)$/.test(a.tagName)) return;
      sig = s;
      data = next;
      draw();
    }

    async function act(path, payload, okText) {
      const r = await api.post(path, payload);
      if (r.ok) say(okText + rotationText(r.data), "ok");
      else say(r.message, "err");
      sure = null;
      await load(true);
    }

    const roleSelect = (id, choices, value) => el("select", { id, "aria-label": "Role" }, choices.map((c) => el("option", { value: c, selected: c === value, text: c })));

    function confirmRow(text, yes) {
      return el("div", { class: "inline", role: "alertdialog", "aria-label": text }, [
        el("span", { class: "warn", text }),
        el("button", { type: "button", class: "danger", text: "Yes, remove", onclick: yes }),
        el("button", { type: "button", text: "Cancel", onclick: () => { sure = null; draw(); } }),
      ]);
    }

    function memberRow(name, role, choices, canEdit, onSave, onRemove, removing, removeText) {
      const sel = roleSelect("", choices.includes(role) ? choices : [role].concat(choices), role);
      sel.disabled = !canEdit;
      const row = el("div", { class: "row" }, [
        el("span", { class: "name", text: name }),
        sel,
      ]);
      if (canEdit) {
        row.appendChild(el("button", { type: "button", text: "Save role", onclick: () => onSave(sel.value) }));
        row.appendChild(el("button", { type: "button", class: "danger", text: "Remove", "aria-label": "Remove " + name, onclick: onRemove }));
      }
      if (removing) row.appendChild(confirmRow(removeText, () => removing()));
      return row;
    }

    function addForm(label, choices, defaultRole, onAdd) {
      const nm = el("input", { type: "text", "aria-label": label + " name", placeholder: "Account name", autocapitalize: "none", spellcheck: "false" });
      const sel = roleSelect("", choices, defaultRole);
      return el("form", { class: "inline", onsubmit: (ev) => { ev.preventDefault(); if (!validName(nm.value)) { say("Type the account's name.", "err"); return; } onAdd(nm.value.trim(), sel.value); } }, [
        nm, sel, el("button", { type: "submit", class: "primary", text: label }),
      ]);
    }

    function draw() {
      body.textContent = "";
      if (!data) return;
      if (data.state === "bootstrap") {
        body.appendChild(el("div", { class: "card" }, [
          el("p", { text: "This tenant has no owner yet. As an administrator of the service you can become its first owner. This works once and is recorded." }),
          el("button", { type: "button", class: "primary", text: "Make me the owner", onclick: () => act("/api/v1/tenant/bootstrap", {}, "You are the tenant's owner.") }),
        ]));
        return;
      }
      if (data.state === "outsider") {
        body.appendChild(el("p", { class: "card", text: "You are not a member of this tenant. Ask an owner or an administrator to add you." }));
        return;
      }
      if (data.state === "error") {
        body.appendChild(el("p", { class: "card err", text: data.message }));
        return;
      }
      const v = data.view;
      body.appendChild(el("p", {}, [ "Signed in as ", el("strong", { text: v.user }), ". Your role in the tenant: ", el("span", { class: "badge", text: v.tenant_role || "none" }), "." ]));

      if (isTenantAdmin(v)) {
        const card = el("section", { class: "card", "aria-labelledby": "h-members" }, [ el("h2", { id: "h-members", text: "Tenant members" }) ]);
        const choices = tenantRoleChoices(v);
        for (const m of v.members) {
          const removing = sure && sure.kind === "tenant" && sure.name === m.name ? () => act("/api/v1/tenant/members", { name: m.name, role: "remove" }, m.name + " is out of the tenant.") : null;
          card.appendChild(memberRow(m.name, m.role, choices, true,
            (role) => act("/api/v1/tenant/members", { name: m.name, role }, m.name + " is now " + role + "."),
            () => { sure = { kind: "tenant", name: m.name }; draw(); },
            removing, "Remove " + m.name + " from the tenant and from every namespace?"));
        }
        card.appendChild(addForm("Add person", choices, "member", (name, role) => act("/api/v1/tenant/members", { name, role }, name + " is a tenant " + role + ".")));
        body.appendChild(card);
      }

      const nsCard = el("section", { class: "card", "aria-labelledby": "h-ns" }, [ el("h2", { id: "h-ns", text: "Namespaces" }) ]);
      if (!v.namespaces.length) nsCard.appendChild(el("p", { class: "muted", text: "No namespaces yet." }));
      for (const ns of v.namespaces) {
        const box = el("div", { class: "card" }, [
          el("div", { class: "inline" }, [
            el("h3", { text: ns.name }),
            el("span", { class: "badge", text: ns.joined ? "you: " + ns.role : "not joined" }),
          ]),
        ]);
        if (canJoin(v, ns)) {
          box.appendChild(el("p", { class: "muted", text: "You see the name only. Joining adds you as an editor and is recorded." }));
          box.appendChild(el("button", { type: "button", text: "Join", onclick: () => act("/api/v1/namespaces/" + encodeURIComponent(ns.name) + "/members", { name: v.user, role: "editor" }, "You joined " + ns.name + ".") }));
        }
        if (ns.joined) {
          const manage = canManageNsMembers(ns);
          for (const m of data.members[ns.name] || []) {
            const removing = sure && sure.kind === "ns" && sure.ns === ns.name && sure.name === m.name ? () => act("/api/v1/namespaces/" + encodeURIComponent(ns.name) + "/members", { name: m.name, role: "remove" }, m.name + " left " + ns.name + ".") : null;
            box.appendChild(memberRow(m.name, m.role, NS_ROLES, manage,
              (role) => act("/api/v1/namespaces/" + encodeURIComponent(ns.name) + "/members", { name: m.name, role }, m.name + " is now " + role + " in " + ns.name + "."),
              () => { sure = { kind: "ns", ns: ns.name, name: m.name }; draw(); },
              removing, "Remove " + m.name + " from " + ns.name + "?"));
          }
          if (manage) box.appendChild(addForm("Add to " + ns.name, NS_ROLES, "viewer", (name, role) => act("/api/v1/namespaces/" + encodeURIComponent(ns.name) + "/members", { name, role }, name + " is " + role + " in " + ns.name + ".")));
        }
        if (canManageNamespaces(v)) {
          if (sure && sure.kind === "delns" && sure.ns === ns.name) {
            box.appendChild(confirmRow("Delete the namespace " + ns.name + " and its member list?", () => act("/api/v1/namespaces", { name: ns.name, action: "delete" }, ns.name + " is deleted.")));
          } else {
            box.appendChild(el("div", { class: "inline" }, [ el("button", { type: "button", class: "danger", text: "Delete namespace", "aria-label": "Delete namespace " + ns.name, onclick: () => { sure = { kind: "delns", ns: ns.name }; draw(); } }) ]));
          }
        }
        nsCard.appendChild(box);
      }
      if (canManageNamespaces(v)) {
        const nm = el("input", { type: "text", "aria-label": "New namespace name", placeholder: "New namespace", autocapitalize: "none", spellcheck: "false" });
        nsCard.appendChild(el("form", { class: "inline", onsubmit: (ev) => { ev.preventDefault(); if (!validName(nm.value)) { say("Type a name for the namespace.", "err"); return; } act("/api/v1/namespaces", { name: nm.value.trim(), action: "create" }, nm.value.trim() + " is created."); } }, [
          nm, el("button", { type: "submit", class: "primary", text: "New namespace" }),
        ]));
      }
      body.appendChild(nsCard);
    }

    await load(true);
    // TODO(web-ui): polling stands in for the events WebSocket the API does not have yet (see app.js, poll).
    ctx.poll(() => load(false), 5000);
  }

  const exported = { page, isTenantAdmin, tenantRoleChoices, canManageNamespaces, canManageNsMembers, canJoin, validName, rotationText };
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
  else (root.IyakkiPages = root.IyakkiPages || []).push(page());
})(typeof window !== "undefined" ? window : globalThis);
