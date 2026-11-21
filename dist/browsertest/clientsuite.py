# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""F-CLIENT rows in real Chrome, as far as no hardware is needed: the committed WebAssembly module of the client engine and its glue
(`crates/iyakki-client-engine/js`), the unmodified node test files `glue.test.mjs` and `media.test.mjs` run in the browser through an
import map, and extra checks that only a browser can give (the module instantiates with no imports under the page's CSP, WebCrypto
Ed25519 non-extractable keys kept in IndexedDB, the real key store code)."""
import json
import os

import harness
import nodeshims


def suite(rep, chrome, root):
    eng = os.path.join(root, "crates/iyakki-client-engine")
    if not os.path.isfile(os.path.join(eng, "assets/iyakki-client-engine.wasm")):
        rep.check("the committed client engine module exists", False)
        return
    extra = dict(nodeshims.EXTRA)  # Static keeps this very dict, so pages can be added to it later

    srv = harness.Static({"/engine/": eng}, extra)
    page = chrome.new_page(1000, 700)
    try:
        # each test file runs on its own page (the tests change globals such as `navigator`)
        for fname, preload in (("glue.test.mjs", ["/engine/assets/iyakki-client-engine.wasm", "/engine/tests/golden/sig.json", "/engine/tests/golden/frame.json", "/engine/tests/golden/frame.bin"]),
                               ("media.test.mjs", [])):
            html = nodeshims.runner_html("/engine/js/test/" + fname, preload)
            # a page served from the same origin as the shims
            path = "/run-" + fname + ".html"
            extra[path] = ("text/html", html.encode())
            page.clear_errors()
            page.goto(srv.base + path)
            res = page.evaluate("window.__ready.then(function(r){return r;}, function(e){return [{name: 'setup', ok: false, error: String(e && e.stack || e)}];})", timeout=120)
            bad = [r for r in res if not r["ok"]]
            for r in res:
                rep.check("%s: %s" % (fname, r["name"]), r["ok"], r.get("error", ""))
            rep.check("%s: %d tests ran in Chrome, %d failed" % (fname, len(res), len(bad)), len(res) >= 5 and not bad)
            errs = page.errors()
            rep.check("%s: no console errors" % fname, errs == [], errs)
            with open(rep.shot("client-" + fname + ".json"), "w") as f:
                json.dump(res, f, indent=1)
    finally:
        page.close()
        srv.stop()


def _sha256_canvas(page):
    import hashlib
    import base64
    r = page.evaluate("(function(){var c=document.getElementById('screen');var d=c.getContext('2d').getImageData(0,0,c.width,c.height).data;var b=new Uint8Array(d.buffer,d.byteOffset,d.length);var ch=[];for(var i=0;i<b.length;i+=4096)ch.push(String.fromCharCode.apply(null,b.subarray(i,i+4096)));return {w:c.width,h:c.height,b64:btoa(ch.join(''))};})()")
    return r["w"], r["h"], hashlib.sha256(base64.b64decode(r["b64"])).hexdigest()


def _wait_stage(page, timeout=10):
    page.wait_for("!document.getElementById('stage').hidden", timeout, "the picture stage")


def page_suite(rep, chrome, root):
    """The real client page (`/connect`) with its real Content-Security-Policy against a scripted fake machine, driven with a real mouse,
    keyboard and touch. Hardware-free: no sound, camera, microphone or clipboard of a real device (those need a person)."""
    import fakehost
    import pagesuite
    eng = os.path.join(root, "crates/iyakki-client-engine")
    meta = json.load(open(os.path.join(eng, "tests/golden/frame.json")))
    frame = open(os.path.join(eng, "tests/golden/frame.bin"), "rb").read()
    host = fakehost.FakeHost(root, frame)
    page = chrome.new_page(1000, 700, 1)
    try:
        host_url = host.base + "/connect"
        # ---- a new link
        page.goto(host_url + "#tok-1")
        page.wait_for("!document.getElementById('enrol').hidden", 10, "the enrol form")
        rep.check("the one-time token is taken out of the address bar at once", page.evaluate("location.hash") == "" and "tok-1" not in page.evaluate("location.href"))
        rep.check("the enrol form offers a device name and says what kind of key it will keep", page.evaluate("document.getElementById('devname').value").startswith("Chrome") and len(page.evaluate("document.getElementById('keyinfo').textContent")) > 20, page.evaluate("document.getElementById('keyinfo').textContent"))
        rep.check("the page labels itself experimental", page.evaluate("document.querySelector('.badge').textContent") == "EXPERIMENTAL")
        a11y = [m for m in page.evaluate(pagesuite.A11Y_JS)]
        rep.check("enrol form: accessibility basics", a11y == [], a11y)
        page.screenshot(rep.shot("client-enrol-desktop.png"))
        page.click("#enrol button[type=submit]")
        enrol = host.wait_for("enrol")[0]
        rep.check("enrolment sends the token and a 64-hex-digit public key under the name given", enrol["token"] == "tok-1" and len(enrol["public"]) == 64 and all(c in "0123456789abcdef" for c in enrol["public"]) and enrol["name"].startswith("Chrome"), enrol)
        auth = host.wait_for("auth")[0]
        rep.check("the device proves itself with an IYAKKI-SIG header made by the browser's Ed25519 key", auth["authorization"].startswith("IYAKKI-SIG "), auth["authorization"][:40])
        page.wait_for("!document.getElementById('signin').hidden", 10, "sign-in form")
        rep.check("the page says the device is known and asks for the person's name and password", "known as Test browser" in page.evaluate("document.getElementById('status').textContent"), page.evaluate("document.getElementById('status').textContent"))
        a11y = page.evaluate(pagesuite.A11Y_JS)
        rep.check("sign-in form: accessibility basics", a11y == [], a11y)
        for vp, (w, h, dpr, mob) in pagesuite.VIEWPORTS.items():
            page.set_viewport(w, h, dpr, mob)
            for scheme in ("light", "dark"):
                page.set_color_scheme(scheme)
                lay = page.evaluate(pagesuite.LAYOUT_JS)
                con = page.evaluate(pagesuite.CONTRAST_JS)
                page.screenshot(rep.shot("client-signin-%s-%s.png" % (vp, scheme)), full=True)
                rep.check("client sign-in %s %s: no sideways scrolling, nothing off screen" % (vp, scheme), lay["overflow"] <= 1 and lay["clipped"] == [], lay)
                rep.check("client sign-in %s %s: text contrast" % (vp, scheme), con["bad"] == [], con["bad"][:4])
        page.set_viewport(1000, 700, 1)
        page.set_color_scheme("light")
        # wrong password
        page.click("#user")
        page.type_text("ann")
        page.click("#pass")
        page.type_text("bad-pw-zq")
        page.click("#signin button[type=submit]")
        page.wait_for("/Wrong name or password/.test(document.getElementById('status').textContent)", 10, "refusal text")
        txt = page.evaluate("document.getElementById('status').textContent")
        rep.check("a wrong password is refused in words and the form stays", not page.evaluate("document.getElementById('signin').hidden") and page.evaluate("document.getElementById('stage').hidden") and "bad-pw-zq" not in page.evaluate("document.body.innerText") and page.evaluate("document.getElementById('pass').value") == "", txt)
        rep.info("refusal text", txt)
        # right password
        page.click("#pass")
        page.type_text("right-pw")
        page.click("#signin button[type=submit]")
        _wait_stage(page)
        host.wait_for("picture_open")
        page.wait_for("document.getElementById('screen').width===%d" % meta["w"], 10, "the picture size")
        page.evaluate("new Promise(function(r){setTimeout(r,200)})")
        w, h, digest = _sha256_canvas(page)
        rep.check("the picture drawn on the canvas is the golden frame (sha256 %s..., %dx%d)" % (digest[:12], w, h), digest == meta["rgba_sha256"] and (w, h) == (meta["w"], meta["h"]), digest)
        page.screenshot(rep.shot("client-picture-desktop.png"))
        rep.check("the page acknowledged the picture message", len(host.sent("ack")) >= 1)
        rep.check("the page told the machine its window size", len(host.sent("input")) >= 1 and any(json.loads(m["msg"])["t"] == "size" for m in host.sent("input")))
        sd = page.evaluate("""(async function(){var db=await new Promise(function(res,rej){var r=indexedDB.open('iyakki-client',1);r.onsuccess=function(){res(r.result)};r.onerror=function(){rej(r.error)}});
            var rec=await new Promise(function(res){var q=db.transaction('kv').objectStore('kv').get('device');q.onsuccess=function(){res(q.result)}});
            db.close(); return rec?{mode:rec.mode,device:rec.device,name:rec.name,hasSeed:!!rec.seed,extractable:rec.priv&&rec.priv.extractable,algo:rec.priv&&rec.priv.algorithm&&rec.priv.algorithm.name}:null;})()""")
        rep.check("the device key is a non-extractable WebCrypto Ed25519 key in IndexedDB with the device id the machine gave", sd and sd["mode"] == "webcrypto" and sd["extractable"] is False and sd["algo"] == "Ed25519" and sd["device"] == "dev-1" and not sd["hasSeed"], sd)

        # ---- real input
        n0 = len(host.sent("input"))
        box = page.evaluate("(function(){var r=document.getElementById('screen').getBoundingClientRect();return [r.x,r.y,r.width,r.height];})()")
        page.click_at(box[0] + box[2] / 2, box[1] + box[3] / 2)
        page.evaluate("new Promise(function(r){setTimeout(r,300)})")
        msgs = [json.loads(m["msg"]) for m in host.sent("input")[n0:]]
        types = [m["t"] for m in msgs]
        rep.check("a real mouse click in the middle of the picture becomes move/down/up at the machine's middle pixel", "down" in types and "up" in types and all(abs(m["x"] - meta["w"] / 2) <= 1 and abs(m["y"] - meta["h"] / 2) <= 1 for m in msgs if m["t"] in ("down", "up")), msgs)
        n0 = len(host.sent("input"))
        page.press("a", "KeyA", 65, text="a")
        page.evaluate("new Promise(function(r){setTimeout(r,200)})")
        msgs = [json.loads(m["msg"]) for m in host.sent("input")[n0:]]
        rep.check("a real key press becomes a key message with key and code", any(m["t"] == "key" and m.get("key") == "a" and m.get("code") == "KeyA" for m in msgs), msgs)
        n0 = len(host.sent("input"))
        page.call("Input.dispatchMouseEvent", {"type": "mouseWheel", "x": box[0] + 10, "y": box[1] + 10, "deltaX": 0, "deltaY": 120})
        page.evaluate("new Promise(function(r){setTimeout(r,200)})")
        msgs = [json.loads(m["msg"]) for m in host.sent("input")[n0:]]
        rep.check("a real wheel turn becomes a wheel message (down is negative)", any(m["t"] == "wheel" and m["d"] < 0 for m in msgs), msgs)
        # the on-screen keyboard of a phone: text typed into the hidden box
        n0 = len(host.sent("input"))
        page.click("#kbd")
        page.type_text("hi")
        page.evaluate("new Promise(function(r){setTimeout(r,200)})")
        msgs = [json.loads(m["msg"]) for m in host.sent("input")[n0:]]
        rep.check("typing on the page's keyboard box sends the characters as key messages", [m.get("key") for m in msgs if m["t"] == "key"] == ["h", "i"], msgs)
        # touch: a tap is a click
        page.call("Emulation.setTouchEmulationEnabled", {"enabled": True, "maxTouchPoints": 2})
        n0 = len(host.sent("input"))
        pt = {"x": box[0] + box[2] / 4, "y": box[1] + box[3] / 4, "id": 1}
        page.call("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [pt]})
        page.call("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
        page.evaluate("new Promise(function(r){setTimeout(r,500)})")
        msgs = [json.loads(m["msg"]) for m in host.sent("input")[n0:]]
        types = [m["t"] for m in msgs]
        rep.check("a real touch tap becomes a click (down and up) at the tapped place", "down" in types and "up" in types, msgs)
        page.call("Emulation.setTouchEmulationEnabled", {"enabled": False})
        # phone size: the stage fits the screen
        page.set_viewport(390, 844, 2, True)
        page.evaluate("new Promise(function(r){setTimeout(r,500)})")
        lay = page.evaluate(pagesuite.LAYOUT_JS)
        page.screenshot(rep.shot("client-picture-phone.png"))
        rep.check("picture at phone width: no sideways scrolling and the buttons of the bar are on screen", lay["overflow"] <= 1 and lay["clipped"] == [], lay)
        sizes = [json.loads(m["msg"]) for m in host.sent("input") if json.loads(m["msg"])["t"] == "size"]
        rep.check("a window resize is reported to the machine (last size %s)" % (sizes[-1:],), sizes and sizes[-1]["w"] == 390 and sizes[-1]["h"] == 844, sizes[-3:])
        page.set_viewport(1000, 700, 1)

        # ---- the connection drops: the page reconnects by itself, with the saved device and the kept sign-in
        c0 = host.connections
        host.sock.close()
        t0 = __import__("time").time()
        page.wait_for("true", 1)
        deadline = t0 + 15
        while host.connections == c0 and __import__("time").time() < deadline:
            __import__("time").sleep(0.1)
        rep.check("after a dropped connection the page reconnects by itself (within the first back-off)", host.connections == c0 + 1, host.connections)
        host.wait_for("auth", 2)
        host.wait_for("sign_in", 2)
        host.wait_for("picture_open", 2)
        rep.check("the reconnect does not enrol again: it proves the saved device and signs in with the kept credentials", len(host.sent("enrol")) == 1 and len(host.sent("auth")) >= 2)
        _wait_stage(page)

        # ---- reload without a link: the saved device is used and the person is asked to sign in
        page.goto(host_url)
        page.wait_for("!document.getElementById('signin').hidden", 10, "the sign-in form after a reload")
        rep.check("a reload without a link uses the saved device (no enrol) and asks for the password", len(host.sent("enrol")) == 1)
        page.click("#user")
        page.type_text("ann")
        page.click("#pass")
        page.type_text("right-pw")
        page.evaluate("document.getElementById('keep').checked=false")
        page.click("#signin button[type=submit]")
        _wait_stage(page)

        # ---- a revoked device is told so, and forgets its key (a reload has no link to use any more)
        host.sock.send(json.dumps({"t": "bye", "reason": "revoked"}))
        page.wait_for("!document.getElementById('problem').hidden", 10, "the problem box")
        pt = page.evaluate("document.getElementById('ptitle').textContent + ' | ' + document.getElementById('pmsg').textContent")
        rep.check("a revoked device is told so in words", "revoked" in pt.lower(), pt)
        page.screenshot(rep.shot("client-revoked.png"))
        page.goto(host_url)
        page.wait_for("!document.getElementById('problem').hidden", 10, "No link after the key was forgotten")
        rep.check("after a revoke the browser no longer holds the device: a reload says there is no link", "No link" in page.evaluate("document.getElementById('ptitle').textContent"))

        # ---- add the device again with a new link, then Disconnect
        page.goto(host_url + "#tok-1")
        page.wait_for("!document.getElementById('enrol').hidden", 10, "the enrol form")
        page.click("#enrol button[type=submit]")
        page.wait_for("!document.getElementById('signin').hidden", 10, "sign-in form")
        page.click("#user")
        page.type_text("ann")
        page.click("#pass")
        page.type_text("right-pw")
        page.click("#signin button[type=submit]")
        _wait_stage(page)
        page.click("#bye")
        host.wait_for("bye")
        page.wait_for("!document.getElementById('problem').hidden", 10, "problem box after Disconnect")
        rep.check("the Disconnect button says goodbye to the machine and says the device stays added", "stays added" in page.evaluate("document.getElementById('pmsg').textContent"))

        # ---- a link that does not work (token unknown): refused in words, the secret is not echoed
        page.goto(host_url + "#bad-token-xyz")
        page.wait_for("!document.getElementById('enrol').hidden", 10, "the enrol form")
        page.click("#enrol button[type=submit]")
        page.wait_for("document.getElementById('status').textContent.length>0 && !/Adding|Connecting|Ready/.test(document.getElementById('status').textContent) || !document.getElementById('problem').hidden", 10, "refusal of the link")
        shown = page.evaluate("document.body.innerText")
        rep.check("an unknown link is refused in words and the token is not shown anywhere", "bad-token-xyz" not in shown and ("link" in shown.lower() or "token" in shown.lower()), shown[:300])
        # ---- no console errors, no CSP violations in the whole run
        errs = page.errors()
        rep.check("no console error, no CSP violation, no failed request in the whole client page run", errs == [], errs)
        csp = [c for c in page.console if "Content Security Policy" in c["text"] or "Refused to" in c["text"]]
        rep.check("the page's own Content-Security-Policy refused nothing it needs", csp == [], csp)
    finally:
        page.close()
        host.stop()


def app_suite(rep, chrome, root):
    """The Apps button of the real client page against a fake machine that agrees to the display-list mode: the menu, an app opened in the window
    form and in the phone form (drawn by the engine module from the display list text), a click on its button becomes input lines, Close ends it."""
    import fakehost
    eng = os.path.join(root, "crates/iyakki-client-engine")
    meta = json.load(open(os.path.join(eng, "tests/golden/frame.json")))
    frame = open(os.path.join(eng, "tests/golden/frame.bin"), "rb").read()
    host = fakehost.FakeHost(root, frame, caps=("picture", "input", "events", "calls", "display.list"))
    page = chrome.new_page(1000, 700, 1)
    try:
        page.goto(host.base + "/connect#tok-1")
        page.wait_for("!document.getElementById('enrol').hidden", 10, "the enrol form")
        page.click("#enrol button[type=submit]")
        page.wait_for("!document.getElementById('signin').hidden", 10, "sign-in form")
        page.click("#user")
        page.type_text("ann")
        page.click("#pass")
        page.type_text("right-pw")
        page.click("#signin button[type=submit]")
        _wait_stage(page)
        page.wait_for("!document.getElementById('apps').hidden", 10, "the Apps button")
        rep.check("the Apps button is shown because the machine agreed to the display-list mode", True)
        page.screenshot(rep.shot("client-app-01-picture.png"))
        page.click("#apps")
        page.wait_for("!document.getElementById('apps-menu').hidden", 5, "the Apps menu")
        names = page.evaluate("Array.from(document.querySelectorAll('#apps-list button')).map(function(b){return b.textContent})")
        rep.check("the Apps menu lists the apps this page can show", names == ["SAMPLE", "Markdown viewer"], names)
        page.screenshot(rep.shot("client-app-02-menu.png"))
        page.evaluate("document.querySelector('#apps-list button').click()")
        page.wait_for("!document.getElementById('app').hidden", 10, "the app pane")
        page.evaluate("new Promise(function(r){setTimeout(r,500)})")
        rep.check("the machine was asked to open the app once", host.wait_for("app_open")[0]["app"] == "sample")
        form = page.evaluate("document.getElementById('stage').dataset.appform")
        rep.check("a wide window shows the window form", form == "window", form)
        rep.check("the machine's picture is hidden while the app is shown", page.evaluate("document.getElementById('screen').hidden"))
        a11y = page.evaluate("Array.from(document.querySelectorAll('#app-aria *')).map(function(e){return (e.getAttribute('role')||e.tagName)+':'+(e.getAttribute('aria-label')||e.textContent)})")
        rep.check("the app's accessibility tree has its button by name", any("Press" in a for a in a11y), a11y)
        page.screenshot(rep.shot("client-app-03-window-form.png"))
        box = page.evaluate("(function(){var r=document.getElementById('app').getBoundingClientRect();return [r.x,r.y,r.width,r.height];})()")
        k = box[2] / 400.0
        n0 = len(host.sent("app_input"))
        page.click_at(box[0] + (16 + 60) * k, box[1] + (100 + 20) * k)
        page.wait_for("true", 1)
        host.wait_for("app_input", n0 + 1)
        lines = host.sent("app_input")[-1].get("lines", "")
        rep.check("a click on the button becomes input lines for the machine", len(lines) > 0, lines)
        page.evaluate("new Promise(function(r){setTimeout(r,500)})")
        page.screenshot(rep.shot("client-app-04-after-press.png"))
        page.set_viewport(390, 844, 3, True)
        page.evaluate("new Promise(function(r){setTimeout(r,500)})")
        form = page.evaluate("document.getElementById('stage').dataset.appform")
        rep.check("a narrow window turns into the phone form", form == "phone", form)
        page.screenshot(rep.shot("client-app-05-phone-form.png"))
        page.set_viewport(1000, 700, 1)
        page.evaluate("new Promise(function(r){setTimeout(r,300)})")
        page.click("#app-close")
        page.wait_for("document.getElementById('app').hidden && !document.getElementById('screen').hidden", 5, "back to the picture")
        host.wait_for("app_close")
        rep.check("Close tells the machine and shows the machine's screen again", True)
        page.screenshot(rep.shot("client-app-06-closed.png"))
        errs = page.errors()
        rep.check("no console error in the app run", errs == [], errs)
    finally:
        page.close()
        host.stop()


def media_suite(rep, chrome, root):
    """The media half of the client page against a fake machine that offers sound, clipboard and a camera list: the Devices panel, a real
    AudioContext and AudioWorklet playing a 440 Hz tone (headless: nothing is audible, the page's own counters say what arrived), the
    clipboard buttons, the camera picker. The person's own camera and microphone need a real device and a person, so they are not run."""
    import fakehost
    import pagesuite
    eng = os.path.join(root, "crates/iyakki-client-engine")
    frame = open(os.path.join(eng, "tests/golden/frame.bin"), "rb").read()
    host = fakehost.FakeHost(root, frame, caps=("picture", "input", "events", "calls", "audio.out", "clipboard.text", "camera.frames"))
    page = chrome.new_page(1000, 700, 1)
    try:
        page.goto(host.base + "/connect#tok-1")
        page.wait_for("!document.getElementById('enrol').hidden", 10, "the enrol form")
        page.click("#enrol button[type=submit]")
        page.wait_for("!document.getElementById('signin').hidden", 10, "sign-in form")
        page.click("#user")
        page.type_text("ann")
        page.click("#pass")
        page.type_text("right-pw")
        page.click("#signin button[type=submit]")
        _wait_stage(page)
        page.wait_for("!document.getElementById('devices').hidden", 10, "the Devices button")
        rep.check("only the buttons this machine can serve are offered: Devices is shown, the person's own camera and microphone are not", page.evaluate("document.getElementById('offer-row').hidden"))
        page.click("#devices")
        page.wait_for("!document.getElementById('panel').hidden", 5, "the panel")
        rep.check("the Devices panel shows the sound row, the clipboard row and the camera row", not page.evaluate("document.getElementById('sound-row').hidden") and not page.evaluate("document.getElementById('clip-row').hidden") and not page.evaluate("document.getElementById('cam-row').hidden"))
        page.screenshot(rep.shot("client-devices-panel.png"))
        a11y = [m for m in page.evaluate(pagesuite.A11Y_JS) if "visible h1" not in m]
        rep.check("the Devices panel: accessibility basics", a11y == [], a11y)
        # sound
        page.click("#sound-btn")
        host.wait_for("audio_open")
        page.wait_for("/Playing here/.test(document.getElementById('sound-state').textContent)", 10, "the sound state")
        rep.check("Hear the machine's sound here sends audio_open with side client and the state says the machine stays quiet", host.sent("audio_open")[0].get("side") == "client" and "the machine is quiet" in page.evaluate("document.getElementById('sound-state').textContent"), page.evaluate("document.getElementById('sound-state').textContent"))
        page.wait_for("/Queued \\d+ ms, \\d+ lost, \\d+ gaps/.test(document.getElementById('sound-state').textContent)", 10, "the worklet's report")
        rep.info("sound state after the tone", page.evaluate("document.getElementById('sound-state').textContent"))
        st = page.evaluate("document.getElementById('sound-state').textContent")
        rep.check("a real AudioContext and AudioWorklet took the tone from the machine (the page's own report: no chunk lost)", "0 lost" in st, st)
        # clipboard
        page.click("#clip-get")
        got = host.wait_for("clipboard_get")
        rep.check("Get the machine's clipboard sends one confirmed request", got[0].get("confirmed") is True)
        page.wait_for("document.getElementById('clip-text').value==='text from the machine' || /clipboard|copied/i.test(document.getElementById('toast').textContent)", 10, "the clipboard text or a toast")
        rep.check("text from the machine reaches the page (in the clipboard, or in the box when the browser refuses)", True, page.evaluate("document.getElementById('clip-text').value + ' | ' + document.getElementById('toast').textContent"))
        # camera list
        # KI-096: the committed WebAssembly module is older than the source (no `camera_list` command), so the page never asks for the list
        try:
            host.wait_for("camera_list", timeout=3)
            asked = True
        except TimeoutError:
            asked = False
        rep.check("opening the Devices panel asks the machine for its cameras", asked, "the engine module refuses {c:'camera_list'}", known="KI-096")
        if asked:
            page.wait_for("!document.getElementById('cam-pick').hidden", 10, "the camera picker")
            rep.check("the camera list shows names, not device paths", page.evaluate("document.getElementById('cam-pick').textContent").strip() == "Desk camera")
        # the connection ends: nothing keeps playing
        host.sock.close()
        page.wait_for("document.getElementById('live').textContent==='Reconnecting'", 10, "the reconnect label")
        rep.check("when the connection ends the sound state is cleared", page.evaluate("document.getElementById('sound-state').textContent") == "")
        errs = page.errors()
        rep.check("no console error in the media run", errs == [], errs)
    finally:
        page.close()
        host.stop()
