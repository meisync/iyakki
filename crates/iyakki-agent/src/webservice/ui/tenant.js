/*
  SPDX-License-Identifier: Apache-2.0
  Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

  The tenant control of the top bar: after sign-in it shows the tenant the person is working in (its name and a small badge with their
  role there) and, when their account belongs to several, a drop-down to switch without signing in again. The names, ids and roles come
  only from GET /api/v1/tenants, never from the address; the server checks every switch (POST /api/v1/tenant/switch) and this code only
  shows what it is told. A switch clears what the pages cached and draws the page again for the new tenant.
  Pages that keep data of their own call `iyakkiTenant.onChange(fn)` to drop it when the tenant changes.
  Loads under node for the tests (module.exports).
*/
"use strict";
(function (root) {
  const CACHE_PREFIX = "iyakki.cache.";
  const listeners = [];

  // The control's shape from the server's answer: what to draw, and nothing the server did not say.
  function shape(data) {
    const list = (data && Array.isArray(data.tenants) ? data.tenants : [])
      .filter((t) => t && typeof t.id === "string" && typeof t.name === "string" && t.role)
      .map((t) => ({ id: t.id, name: t.name, role: String(t.role), current: !!t.current }));
    const current = list.find((t) => t.current) || list.find((t) => data && t.id === data.current) || list[0] || null;
    return { current, list, canSwitch: list.length > 1 && !!(data && data.can_switch) };
  }

  // What the badge says: the role as the server named it.
  function badgeText(t) {
    return t ? t.role : "";
  }

  // The words for a switch that did not work.
  function failText(r) {
    if (r.status === 404) return "That tenant is not available to you any more.";
    if (r.status === 429) return "Too many switches. Wait a moment.";
    return r.message || "The switch did not work.";
  }

  // Forget what the pages kept for the tenant that was left (tab storage under our own prefix only; never the token).
  function clearCaches(storage) {
    if (!storage || typeof storage.length !== "number" || typeof storage.key !== "function") return 0;
    const names = [];
    for (let i = 0; i < storage.length; i++) {
      const k = storage.key(i);
      if (k && k.startsWith(CACHE_PREFIX)) names.push(k);
    }
    for (const k of names) storage.removeItem(k);
    return names.length;
  }

  function fire(tenant) {
    for (const f of listeners.slice()) {
      try { f(tenant); } catch (e) { /* one page's cache must not stop the switch */ }
    }
  }

  // Draw into `o.slot`. o = {api, el, slot, say, storage, location, onSwitched(tenant)}.
  async function mount(o) {
    const r = await o.api.get("/api/v1/tenants");
    o.slot.textContent = "";
    if (!r.ok) return null; // no control is better than a wrong one
    const s = shape(r.data);
    if (!s.current) return null;
    const el = o.el;
    const kids = [el("span", { class: "tenant-name", text: s.current.name }), el("span", { class: "badge", text: badgeText(s.current), title: "Your role in this tenant" })];
    if (s.canSwitch) {
      const sel = el("select", { id: "tenant-select", "aria-label": "Switch tenant" }, s.list.map((t) => el("option", { value: t.id, text: t.name + " (" + badgeText(t) + ")", selected: t.id === s.current.id })));
      sel.value = s.current.id;
      sel.addEventListener("change", async () => {
        const want = sel.value;
        if (want === s.current.id) return;
        sel.disabled = true;
        const res = await o.api.post("/api/v1/tenant/switch", { tenant: want });
        if (!res.ok) {
          if (o.say) o.say(failText(res), "err");
          await mount(o); // the list may have changed (a removal): draw what is true now
          return;
        }
        clearCaches(o.storage);
        fire(res.data && res.data.tenant);
        const url = res.data && res.data.url;
        if (typeof url === "string" && /^https:\/\//.test(url) && o.location) {
          // the tenant lives at another address: a different server holds a different session, so the person signs in there
          o.location.assign(url);
          return;
        }
        await mount(o);
        if (o.say) o.say("Now working in " + (res.data && res.data.tenant ? res.data.tenant.name : "the new tenant") + ".", "ok");
        if (o.onSwitched) await o.onSwitched(res.data && res.data.tenant);
      });
      kids.push(sel);
    }
    for (const k of kids) o.slot.appendChild(k);
    return s.current;
  }

  function clear(slot, storage) {
    if (slot) slot.textContent = "";
    clearCaches(storage);
  }

  const api = { mount, clear, shape, badgeText, failText, clearCaches, onChange: (f) => { if (typeof f === "function") listeners.push(f); }, CACHE_PREFIX };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.iyakkiTenant = api;
})(typeof window !== "undefined" ? window : globalThis);
