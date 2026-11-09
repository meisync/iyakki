/*
  SPDX-License-Identifier: Apache-2.0
  Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

  Page "Tiles": a WASM tile that runs on the machine that serves this page, drawn here. The page sends input events
  (POST /api/v1/tiles/<id>/events, JSON) and draws the display list the service answers with (tilerender.js). The tile, its state
  and its permissions stay on the server: this page only draws and forwards input, and holds no authority of its own.

  Input: Pointer Events (mouse, touch, pen), the keyboard, text through a hidden text box (so input methods and composition work: the
  text is sent when the composition ends), the wheel (as the ABI's up/down arrow keys; version 1 has no wheel event). Moves are sent only
  while a button or finger is down, and only the newest one. One request is in flight at a time; what happens meanwhile is queued
  and sent together. A lost connection is retried with a growing pause; a refused or crashed tile shows the service's plain sentence
  and a Reload button (the state is lost, as with a hot reload on the device). Loads under node for the tests (module.exports).
*/
"use strict";
(function (root) {
  const MAX_DIM = 4096;
  const RETRY = [1000, 2000, 4000, 8000, 15000];
  const MOUSE_BUTTONS = 255;
  const KEYS = { Backspace: 8, Tab: 9, Enter: 13, Escape: 27, ArrowLeft: 0x100, ArrowRight: 0x101, ArrowUp: 0x102, ArrowDown: 0x103, Home: 0x104, End: 0x105, PageUp: 0x106, PageDown: 0x107, Delete: 0x108 };

  function retryDelay(n) { return RETRY[Math.max(0, Math.min(RETRY.length - 1, n | 0))]; }

  // The ABI key code of a keyboard event: named keys from the table, a printable ASCII key by its character, else null.
  function keyCode(key) {
    if (Object.prototype.hasOwnProperty.call(KEYS, key)) return KEYS[key];
    if (typeof key === "string" && key.length === 1 && key.charCodeAt(0) >= 32 && key.charCodeAt(0) < 127) return key.charCodeAt(0);
    if (/^F([1-9]|1[0-2])$/.test(String(key))) return 0x110 + Number(String(key).slice(1));
    return null;
  }
  const modBits = (e) => (e.shiftKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.altKey ? 4 : 0) | (e.metaKey ? 8 : 0);

  // A point of the page -> a whole-number point of the tile's surface (logical pixels), clamped to a sane range.
  function toSurface(clientX, clientY, rect, w, h) {
    const sx = rect.width > 0 ? w / rect.width : 1, sy = rect.height > 0 ? h / rect.height : 1;
    const c = (v) => Math.max(-32768, Math.min(32768, Math.round(v)));
    return { x: c((clientX - rect.left) * sx), y: c((clientY - rect.top) * sy) };
  }

  // Whole wheel steps -> arrow key presses (down for positive deltas). The remainder carries to the next wheel event.
  function wheelKeys(acc, deltaY) {
    let total = (acc || 0) + (Number(deltaY) || 0);
    const events = [];
    while (Math.abs(total) >= 100 && events.length < 8) {
      const code = total > 0 ? 0x103 : 0x102;
      events.push({ type: "key", code, mods: 0, down: true }, { type: "key", code, mods: 0, down: false });
      total += total > 0 ? -100 : 100;
    }
    return { events, rest: Math.abs(total) >= 100 ? 0 : total };
  }

  // The events waiting to be sent. Pointer moves keep only the newest one; everything else keeps its order.
  function newQueue() {
    const q = [];
    return {
      push(e) {
        if (e.type === "pointer_move" && q.length && q[q.length - 1].type === "pointer_move") { q[q.length - 1] = e; return; }
        if (e.type === "resize") {
          const i = q.findIndex((x) => x.type === "resize");
          if (i >= 0) q.splice(i, 1);
        }
        if (q.length < 100) q.push(e);
      },
      take() { return q.splice(0, q.length); },
      // events that could not be delivered go back in front of what came meanwhile, in their order
      front(list) { q.unshift(...list.slice(0, 100)); if (q.length > 100) q.length = 100; },
      size: () => q.length,
    };
  }

  function themeFor(env) {
    const dark = !!(env.matchMedia && env.matchMedia("(prefers-color-scheme: dark)").matches);
    return { dark, bg: dark ? "#181818" : "#f5f5f5" };
  }

  // The controller of one open tile: queue, request loop, drawing, messages. `env`: {api, canvas, tile, say, setTimeout, clearTimeout,
  // devicePixelRatio, matchMedia, onState}. `onState(state, text)` is told "starting", "live", "reconnecting" or "stopped".
  function openTile(env) {
    const R = env.renderer || root.IyakkiTileRender;
    const queue = newQueue();
    const theme = themeFor(env);
    const state = { size: { w: Math.min(MAX_DIM, Math.max(1, env.width | 0)), h: Math.min(MAX_DIM, Math.max(1, env.height | 0)) }, inflight: false, started: false, stopped: false, failures: 0, reset: false, timer: null, frame: null };
    const dpr = () => Math.max(1, Math.min(4, Number(env.devicePixelRatio) || 1));
    const tell = (s, t) => { if (env.onState) env.onState(s, t || ""); };

    function schedule(ms) {
      if (state.timer !== null || state.stopped) return;
      state.timer = env.setTimeout(() => { state.timer = null; pump(); }, ms);
    }

    async function pump() {
      if (state.inflight || state.stopped) return;
      const events = queue.take();
      if (state.started && !events.length && !state.retrying && !state.reset) return;
      state.inflight = true;
      // the size and theme go with every request: the service uses them only when it has to (re)start the tile (first call, Reload,
      // or after it dropped an idle one)
      const body = { events, width: state.size.w, height: state.size.h, scale_pct: Math.round(dpr() * 100), dark: theme.dark };
      if (state.reset) body.reset = true;
      let r;
      try { r = await env.api.post("/api/v1/tiles/" + encodeURIComponent(env.tile) + "/events", body); } catch (e) { r = { ok: false, status: 0, message: "" }; }
      state.inflight = false;
      if (state.stopped) return;
      if (r.ok && r.data && r.data.frame) {
        state.started = true; state.reset = false; state.retrying = false; state.failures = 0;
        state.frame = r.data.frame;
        const sz = R.draw(env.canvas, r.data.frame, dpr(), R.parseColor(theme.bg));
        state.size = { w: sz.w, h: sz.h };
        tell("live");
        if (queue.size()) schedule(0);
        return;
      }
      // the events that were taken are not lost on a broken connection: put them back in front
      if (r.status === 0 || r.status === 429 || r.status === 503) {
        queue.front(events);
        state.failures += 1;
        state.retrying = true;
        tell("reconnecting", r.status === 429 ? "Too many requests. Waiting a moment." : "The connection to the service was lost. Trying again.");
        schedule(r.status === 429 ? 2000 : retryDelay(state.failures - 1));
        return;
      }
      if (r.status === 401) { state.stopped = true; return; } // the shell signs the person out
      state.stopped = true;
      tell("stopped", r.message || "This tile stopped.");
    }

    return {
      send(e) { if (!state.stopped) { queue.push(e); schedule(0); } },
      start() { state.stopped = false; tell("starting"); schedule(0); },
      // a new surface size, in logical pixels
      resize(w, h) {
        w = Math.min(MAX_DIM, Math.max(1, Math.round(w))); h = Math.min(MAX_DIM, Math.max(1, Math.round(h)));
        if (state.started) this.send({ type: "resize", w, h, scale_pct: Math.round(dpr() * 100) });
        else state.size = { w, h };
      },
      // Reload after a stop: the service throws the state away and starts the tile again
      reload() {
        if (state.timer !== null) { env.clearTimeout(state.timer); state.timer = null; }
        state.stopped = false; state.reset = true; state.failures = 0; state.retrying = false;
        tell("starting");
        schedule(0);
      },
      stop() { state.stopped = true; if (state.timer !== null) { env.clearTimeout(state.timer); state.timer = null; } },
      size: () => state.size,
      frame: () => state.frame,
      queued: () => queue.size(),
      theme: () => theme,
    };
  }

  function page() {
    return { id: "tiles", title: "Tiles", available: (me) => !!me, render: renderPage };
  }

  async function renderPage(ctx, host) {
    const { api, el, say, doc } = ctx;
    const body = el("div");
    host.appendChild(body);
    const r = await api.get("/api/v1/tiles");
    if (!r.ok) { body.appendChild(el("p", { class: "card", text: r.message })); return; }
    const tiles = (r.data && r.data.tiles) || [];
    body.appendChild(el("p", { class: "muted", text: "These apps run on the machine you signed in to and are drawn here. What an app may do is decided there, by your permissions: this page adds none." }));
    if (!tiles.length) { body.appendChild(el("p", { class: "card", text: "No tiles are installed on this machine." })); return; }
    const tabs = el("div", { class: "inline" });
    const stage = el("div", { class: "card tile-stage" });
    body.appendChild(tabs);
    body.appendChild(stage);
    let live = null;

    function open(tile) {
      if (live) live.stop(); // (the previous tile's window listener ends with the page; a new tile replaces the stage)
      stage.textContent = "";
      const win = doc.defaultView || root;
      const canvas = el("canvas", { class: "tile-canvas tile-off", tabindex: "0", role: "application", "aria-label": tile.title });
      let off = true, hot = false;
      const paintClass = () => { canvas.className = "tile-canvas" + (off ? " tile-off" : "") + (hot ? " tile-hot" : ""); };
      const note = el("p", { class: "muted", role: "status", "aria-live": "polite" });
      const reloadBtn = el("button", { type: "button", text: "Reload", hidden: true });
      const input = el("textarea", { class: "tile-input", "aria-label": "Text for " + tile.title, autocomplete: "off", autocapitalize: "none", spellcheck: "false", rows: "1" });
      stage.appendChild(canvas);
      stage.appendChild(input);
      stage.appendChild(note);
      stage.appendChild(reloadBtn);
      const width = Math.min(1200, Math.max(200, Math.floor((stage.clientWidth || tile.width) - 24)));
      const t = openTile({
        api, canvas, tile: tile.id, width: Math.min(tile.width || 320, width), height: tile.height || 200,
        setTimeout: win.setTimeout.bind(win), clearTimeout: win.clearTimeout.bind(win), devicePixelRatio: win.devicePixelRatio, matchMedia: win.matchMedia ? win.matchMedia.bind(win) : null,
        onState(s, text) {
          note.textContent = s === "live" || s === "starting" ? "" : text;
          reloadBtn.hidden = s !== "stopped";
          off = s !== "live";
          paintClass();
        },
      });
      live = t;
      reloadBtn.addEventListener("click", () => t.reload());

      let down = false, composing = false, acc = 0;
      const at = (e) => toSurface(e.clientX, e.clientY, canvas.getBoundingClientRect(), t.size().w, t.size().h);
      canvas.addEventListener("pointerdown", (e) => {
        input.focus({ preventScroll: true });
        down = true;
        if (canvas.setPointerCapture) { try { canvas.setPointerCapture(e.pointerId); } catch (x) { /* a touch that already ended */ } }
        t.send(Object.assign({ type: "pointer_down", button: Math.min(MOUSE_BUTTONS, e.button | 0) }, at(e)));
        e.preventDefault();
      });
      canvas.addEventListener("pointerup", (e) => {
        if (!down) return;
        down = false;
        t.send(Object.assign({ type: "pointer_up", button: Math.min(MOUSE_BUTTONS, e.button | 0) }, at(e)));
      });
      canvas.addEventListener("pointercancel", () => { down = false; });
      canvas.addEventListener("pointermove", (e) => {
        if (down) t.send(Object.assign({ type: "pointer_move" }, at(e)));
        else if (t.frame()) {
          const over = root.IyakkiTileRender.hitAt(t.frame().ops, at(e).x, at(e).y) !== null;
          if (over !== hot) { hot = over; paintClass(); }
        }
      });
      canvas.addEventListener("wheel", (e) => {
        const w = wheelKeys(acc, e.deltaMode === 1 ? e.deltaY * 40 : e.deltaY);
        acc = w.rest;
        for (const k of w.events) t.send(k);
        if (w.events.length) e.preventDefault();
      }, { passive: false });
      const keyEvent = (down_) => (e) => {
        if (composing || e.isComposing || e.keyCode === 229) return;
        const code = keyCode(e.key);
        if (code === null) return;
        t.send({ type: "key", code, mods: modBits(e), down: down_ });
        // keys the tile uses are not also page shortcuts (Tab stays with the page so the keyboard can leave the tile)
        if (e.key !== "Tab" && (code < 32 || code >= 0x100)) e.preventDefault();
      };
      input.addEventListener("keydown", keyEvent(true));
      input.addEventListener("keyup", keyEvent(false));
      canvas.addEventListener("keydown", () => input.focus({ preventScroll: true }));
      input.addEventListener("compositionstart", () => { composing = true; });
      input.addEventListener("compositionend", (e) => {
        composing = false;
        if (e.data) t.send({ type: "text", text: String(e.data).slice(0, 256) });
        input.value = "";
      });
      input.addEventListener("input", (e) => {
        if (composing || e.isComposing) return;
        if (input.value) t.send({ type: "text", text: input.value.slice(0, 256) });
        input.value = "";
      });
      const onResize = () => {
        const w = Math.min(1200, Math.max(200, Math.floor(stage.clientWidth - 24)));
        if (stage.clientWidth && Math.abs(w - t.size().w) > 4) t.resize(w, t.size().h);
      };
      win.addEventListener("resize", onResize);
      ctx.onLeave(() => { t.stop(); win.removeEventListener("resize", onResize); });
      t.start();
    }

    for (const tile of tiles) {
      tabs.appendChild(el("button", { type: "button", text: tile.title, onclick: () => open(tile) }));
    }
    open(tiles[0]);
    say("");
  }

  const exported = { page, openTile, keyCode, modBits, toSurface, wheelKeys, newQueue, retryDelay, themeFor, KEYS };
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
  else (root.IyakkiPages = root.IyakkiPages || []).push(page());
})(typeof window !== "undefined" ? window : globalThis);
