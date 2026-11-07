/*
  SPDX-License-Identifier: Apache-2.0
  Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

  Page "Remote access": the web twin of Settings -> Remote access, the switch "Serve this desktop to browsers". It reads
  GET /api/v1/webaccess and changes it with POST /api/v1/webaccess ({on}, {port}). The service decides (administrators only,
  ports 1024 to 65535, HTTPS only) and writes the same audit lines as the native page; this page shows what it exposes before
  the switch is turned on, the addresses to open and the certificate fingerprint to compare on the first visit.
*/
"use strict";
(function (root) {
  // The text of the port field: empty means the default (0). Mirrors iyakki_agent::webaccess::parse_port; the server checks again.
  function parsePort(text) {
    const t = String(text === undefined || text === null ? "" : text).trim();
    if (t === "") return { ok: true, port: 0 };
    if (!/^[0-9]{1,5}$/.test(t)) return { ok: false, message: "The port is a number from 1024 to 65535." };
    const p = Number(t);
    if (p > 65535) return { ok: false, message: "The port is a number from 1024 to 65535." };
    if (p < 1024) return { ok: false, message: "Choose a port from 1024 to 65535 (lower ports belong to the system's own services)." };
    return { ok: true, port: p };
  }
  // "AA:BB:..." in lines of eight pairs, as the native page shows it
  function fingerprintLines(fp) {
    if (!fp) return [];
    const parts = String(fp).split(":");
    const out = [];
    for (let i = 0; i < parts.length; i += 8) out.push(parts.slice(i, i + 8).join(":"));
    return out;
  }
  // the word and the colour class for the state
  function stateWord(s) {
    if (s.listening) return { text: s.status, cls: "warn" };
    if (s.error) return { text: s.status, cls: "err" };
    if (s.on) return { text: s.status, cls: "warn" };
    return { text: "Off: nothing listens, no browser can reach this desktop", cls: "on" };
  }

  function page() {
    return {
      id: "remote",
      title: "Remote access",
      // administrators only, on a machine that has a desktop to serve; the server refuses anyone else all the same
      available: (me) => !!me && me.admin && me.profile !== "cloud",
      render: renderPage,
    };
  }

  async function renderPage(ctx, host) {
    const { api, el, say } = ctx;
    const body = el("div");
    host.appendChild(body);
    let st = null;
    let asking = false; // the exposure warning is shown and waits for "Yes, serve"
    let sig = "";

    async function load(force) {
      const r = await api.get("/api/v1/webaccess");
      if (!r.ok) {
        st = { unavailable: true, message: r.status === 404 || r.status === 501 ? "This machine has no desktop to serve to browsers." : r.message };
        sig = "";
        return draw();
      }
      const s = JSON.stringify(r.data);
      const a = ctx.doc.activeElement;
      if (!force && s === sig) return;
      if (!force && a && body.contains(a) && a.tagName === "INPUT") return; // do not wipe a port being typed
      sig = s;
      st = r.data;
      draw();
    }

    async function change(payload, okText) {
      const r = await api.post("/api/v1/webaccess", payload);
      say(r.ok ? okText : r.message, r.ok ? "ok" : "err");
      asking = false;
      await load(true);
    }

    function draw() {
      body.textContent = "";
      if (!st) return;
      if (st.unavailable) {
        body.appendChild(el("p", { class: "card", text: st.message }));
        return;
      }
      const w = stateWord(st);
      body.appendChild(el("p", { class: "muted", text: "Switch this on to open this desktop in a browser on another computer or tablet. Off by default; always encrypted (HTTPS), never plain HTTP." }));
      body.appendChild(el("div", { class: "card" }, [
        el("p", {}, [ "Remote access: ", el("strong", { class: w.cls, text: w.text }) ]),
        el("p", { class: "muted", text: "Port " + st.port + " (HTTPS only)" }),
        st.boot ? el("p", { class: "muted", text: "It was started at boot; the switch leaves that one alone." }) : null,
        asking ? el("div", { role: "alertdialog", "aria-label": "What this exposes" }, [
          el("h2", { text: "What this exposes" }),
          el("p", { class: "warn", text: st.exposes }),
          el("div", { class: "inline" }, [
            el("button", { type: "button", class: "primary", text: "Yes, serve this desktop", onclick: () => change({ on: true }, "Serving this desktop to browsers. Turn it off when you are done.") }),
            el("button", { type: "button", text: "Cancel", onclick: () => { asking = false; draw(); } }),
          ]),
        ]) : el("div", { class: "inline" }, [
          st.on
            ? el("button", { type: "button", class: "danger", text: "Stop serving this desktop", onclick: () => change({ on: false }, "No longer serving this desktop to browsers: the port is closed within a few seconds and open sessions end.") })
            : el("button", { type: "button", class: "primary", text: "Serve this desktop to browsers", onclick: () => { asking = true; draw(); } }),
        ]),
      ]));

      const pf = el("input", { id: "port", type: "text", inputmode: "numeric", "aria-label": "Port for browsers", value: String(st.port), placeholder: "8443 (the default)" });
      body.appendChild(el("form", { class: "card", onsubmit: (ev) => {
        ev.preventDefault();
        const p = parsePort(pf.value);
        if (!p.ok) { say(p.message, "err"); return; }
        change({ port: p.port }, "The port for browsers is " + (p.port || 8443) + "." + (st.on ? " It follows within a few seconds." : " It is used when you switch the service on."));
      } }, [
        el("label", { for: "port", text: "Port for browsers (HTTPS), 1024 to 65535" }),
        el("div", { class: "inline" }, [ pf, el("button", { type: "submit", text: "Change port" }) ]),
      ]));

      if (st.on || st.listening) {
        const card = el("div", { class: "card" }, [ el("h2", { text: "Open one of these in the browser of the other computer" }) ]);
        for (const u of st.urls) card.appendChild(el("p", { class: "mono", text: u }));
        card.appendChild(el("p", { class: "muted", text: "The name ends in .local and works on most computers and phones on the same network; if yours cannot find it, use an address." }));
        card.appendChild(el("h2", { text: "Certificate fingerprint (SHA-256)" }));
        card.appendChild(el("p", { class: "muted", text: "The browser warns once, because the certificate is made by this machine; compare this with what it shows before you continue." }));
        const lines = fingerprintLines(st.fingerprint);
        if (lines.length) for (const l of lines) card.appendChild(el("div", { class: "mono", text: l }));
        else card.appendChild(el("p", { class: "muted", text: "(the certificate is being made: look again in a moment)" }));
        body.appendChild(card);
      }
      body.appendChild(el("p", { class: "muted", text: "Turn it off when you are done. The firewall port is opened for the local networks only while it is on, and closed when it is off." }));
    }

    await load(true);
    // TODO(web-ui): polling stands in for the events WebSocket the API does not have yet (see app.js, poll).
    ctx.poll(() => load(false), 5000);
  }

  const exported = { page, parsePort, fingerprintLines, stateWord };
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
  else (root.IyakkiPages = root.IyakkiPages || []).push(page());
})(typeof window !== "undefined" ? window : globalThis);
