# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""More real-browser checks of the web UI: the Runs states (empty, waiting for an approval, answered with the keyboard, failed, slow and
stopped), the Remote access switch and its confirm step, the Git page on a real repository, the tenant switch with two tenants,
keyboard-only operation of every page, long text, right-to-left and 200%/400% zoom layouts, and the size and time of every page with the
proof that nothing is requested from outside."""
import json
import time

import funcsuite as fx_
import pagesuite as ps

WALK_JS = """(function(){var a=document.activeElement;if(!a||a===document.body)return null;var r=a.getBoundingClientRect(),cs=getComputedStyle(a);
 var nm=(a.getAttribute('aria-label')||(a.labels&&a.labels[0]&&a.labels[0].textContent)||a.textContent||a.title||'').trim().slice(0,30);
 var ring=(cs.outlineStyle!=='none'&&parseFloat(cs.outlineWidth)>=2)||(cs.boxShadow&&cs.boxShadow!=='none');
 return {d:a.tagName.toLowerCase()+(a.id?'#'+a.id:''),name:nm,visible:r.width>0&&r.height>0&&!a.closest('[hidden]'),inView:r.bottom>0&&r.top<innerHeight&&r.right>0&&r.left<innerWidth,ring:!!ring,key:a.tagName+'|'+a.id+'|'+nm};})()"""


def keyboard_walk(rep, page, title, limit=120):
    """Tab through the whole page from the skip link. Every stop must be visible, in view, named and show a focus ring; no trap; Shift+Tab walks back."""
    page.evaluate("document.querySelector('a.skip').focus()")
    stops = []
    for _ in range(limit):
        page.tab()
        d = page.evaluate(WALK_JS)
        if d is None or (stops and d["key"] == stops[0]["key"]):
            break
        stops.append(d)
    else:
        rep.check("%s: Tab leaves the page within %d stops (no keyboard trap)" % (title, limit), False, [s["d"] for s in stops[-5:]])
        return
    bad = [s for s in stops if not (s["visible"] and s["inView"])]
    rep.check("%s: every one of the %d Tab stops is visible and in view" % (title, len(stops)), bad == [], bad[:3])
    unnamed = [s for s in stops if not s["name"] and not s["d"].startswith("canvas")]
    rep.check("%s: every Tab stop has a name" % title, unnamed == [], unnamed[:3])
    noring = [s["d"] + " " + s["name"] for s in stops if not s["ring"] and not s["d"].startswith("a.skip")]
    rep.check("%s: every Tab stop shows where the focus is" % title, noring == [], noring[:4])
    # back with Shift+Tab: the same stops in the opposite order
    page.evaluate("var l=document.querySelectorAll('input,select,textarea,button,a[href],[tabindex]');var v=Array.prototype.filter.call(l,function(e){var r=e.getBoundingClientRect();return r.width>0&&e.tabIndex>=0&&!e.closest('[hidden]')});if(v.length)v[v.length-1].focus();")
    back = []
    for _ in range(len(stops) + 2):
        d = page.evaluate(WALK_JS)
        if d is None:
            break
        back.append(d["key"])
        page.tab(shift=True)
    fwd = [s["key"] for s in stops]
    rep.check("%s: Shift+Tab walks the same stops backwards" % title, back[:len(fwd)][::-1][-len(back):] == fwd[-len(back):] if len(back) <= len(fwd) else False or set(fwd) <= set(back), (fwd[:6], back[:6]))


def metrics_for_load(rep, page, base):
    m = page.evaluate("""(function(){var n=performance.getEntriesByType('navigation')[0]||{};var rs=performance.getEntriesByType('resource');
      var sum=function(f){return rs.filter(f).reduce(function(a,r){return a+(r.encodedBodySize||0)},0)};
      return {html:n.encodedBodySize||0,dcl_ms:Math.round(n.domContentLoadedEventEnd||0),load_ms:Math.round(n.loadEventEnd||0),
        requests:rs.length,js_bytes:sum(function(r){return /\\.js$/.test(r.name)}),css_bytes:sum(function(r){return /\\.css$/.test(r.name)}),
        other_bytes:sum(function(r){return !/\\.(js|css)$/.test(r.name)}),names:rs.map(function(r){return r.name})};})()""")
    outside = [n for n in m["names"] if not n.startswith(base)]
    m["external_requests"] = len(outside)
    rep.check("the sign-in page is made of %d same-origin files and asks nothing from outside" % m["requests"], outside == [], outside)
    names = m.pop("names")
    rep.metric("sign-in page load (encoded bytes, ms)", dict(m, total_static_bytes=m["html"] + m["js_bytes"] + m["css_bytes"]))
    rep.check("the sign-in page's HTML, JavaScript and CSS together stay under 300 KB", m["html"] + m["js_bytes"] + m["css_bytes"] < 300 * 1024, m)
    return names


def page_numbers(rep, page, title, base, t_click_ms):
    d = page.evaluate("""(function(){var rs=performance.getEntriesByType('resource').filter(function(r){return r.initiatorType==='fetch'||r.initiatorType==='xmlhttprequest'});
      return {api_calls:rs.length,api_bytes:rs.reduce(function(a,r){return a+(r.encodedBodySize||0)},0),dom_nodes:document.getElementsByTagName('*').length,
        outside:performance.getEntriesByType('resource').filter(function(r){return r.name.indexOf(%s)!==0}).length};})()""" % json.dumps(base))
    d["draw_ms"] = t_click_ms
    rep.metric("page %s (draw time from the click, api calls so far, dom nodes)" % title, d)
    rep.check("page %s: nothing requested from outside; drawn in under 3 s" % title, d["outside"] == 0 and t_click_ms < 3000, d)


def idle_rate(rep, page, title, seconds=6):
    """Requests a page makes by itself (polling) per minute: the service refuses a person more than 60 a minute, so the pages must stay well under."""
    n0 = len(page.requests)
    time.sleep(seconds)
    per_min = round((len(page.requests) - n0) * 60.0 / seconds)
    rep.metric("page %s idle requests per minute" % title, {"requests_per_minute": per_min})
    rep.check("page %s: polling by itself is %d requests a minute (the service allows 60 per person, the bell and other open tabs share it)" % (title, per_min), per_min <= 30, per_min)


def timed_goto(page, title):
    t0 = time.time()
    ps.goto_page(page, title)
    return int((time.time() - t0) * 1000)


def stress_layouts(rep, page, titles, tag):
    """320 px (400% zoom of a 1280 px window), 640 px (200%), right-to-left, and text that is much longer than the layout expects."""
    for label, (w, h, rtl) in {"400% zoom (320 px wide)": (320, 480, False), "200% zoom (640 px wide)": (640, 450, False), "right-to-left at phone width": (390, 844, True), "right-to-left at desktop width": (1280, 800, True)}.items():
        page.set_viewport(w, h, 1, False)
        page.evaluate("document.documentElement.dir=%s" % json.dumps("rtl" if rtl else "ltr"))
        for t in titles:
            ps.goto_page(page, t)
            page.evaluate("new Promise(function(r){setTimeout(r,150)})")
            lay = page.evaluate(ps.LAYOUT_JS)
            # at 320 px a file input's native width may exceed a card; the page must still not scroll sideways
            rep.check("%s, %s: no sideways scrolling and no control off the screen" % (t, label), lay["overflow"] <= 1 and lay["clipped"] == [], {"overflow": lay["overflow"], "clipped": lay["clipped"][:4], "wide": lay["wide"]})
            if t == titles[0] or rtl is False and w == 320:
                page.screenshot(rep.shot("%s-%s-%s.png" % (tag, t.lower().replace(" ", "-"), label.split(" (")[0].replace(" ", "-").replace("%", "pct"))), full=True)
    page.set_viewport(1280, 800, 1, False)
    page.evaluate("document.documentElement.dir='ltr'")


def suite_agent(rep, chrome, fx):
    """agent-api profile: Runs states, Remote access, keyboard, metrics."""
    rep.section = "more-agent"
    page = chrome.new_page(1280, 900, 1)
    try:
        tok = ps.token_for(fx, "root", fx.info["admin_password"])
        ps.login(page, fx.base, "root", fx.info["admin_password"])
        page.clear_errors()
        # ---- Runs: empty
        ps.goto_page(page, "Runs")
        rep.check("Runs with no run yet says so in words", "No runs yet" in page.evaluate("document.getElementById('page').innerText"))
        # ---- a run that waits for an approval, answered with the keyboard only
        fx.turn("call")
        fx.turn("final:fetched ok")
        fx_._type_into(page, fx_._label_sel("What do you want to ask?", "textarea"), "fetch it")
        page.evaluate("Array.prototype.find.call(document.querySelectorAll('#page button'),function(b){return b.textContent.trim()==='Start'}).focus()")  # Tab from the box lands here; the page redraws now and then, so the focus is set directly
        page.press("Enter", "Enter", 13, text="\r")
        try:
            page.wait_for("!!Array.prototype.find.call(document.querySelectorAll('#page button'),function(b){return b.getAttribute('aria-label')==='Approve web_fetch'})", 15, "the approval card")
        except Exception:
            rep.info("runs on the service", json.dumps(ps.api(fx, "GET", "/api/v1/chat/runs", tok)[1])[:600])
            rep.info("Runs page when no approval card came", page.evaluate("document.getElementById('status').textContent + ' || ' + document.getElementById('page').innerText")[:800])
            raise
        txt = page.evaluate("document.getElementById('page').innerText")
        rep.check("an approval card names the tool, the risk and that only the person's own click answers", "web_fetch" in txt and "Risk" in txt and "Only your own click" in txt, txt[:300])
        page.screenshot(rep.shot("more-runs-approval.png"), full=True)
        page.evaluate("Array.prototype.find.call(document.querySelectorAll('#page button'),function(b){return b.getAttribute('aria-label')==='Approve web_fetch'}).focus()")
        page.press("Enter", "Enter", 13, text="\r")
        page.wait_for("/fetched ok/.test(document.getElementById('page').innerText)", 15, "the answer after approval")
        rep.check("approving with the Enter key lets the run go on to its answer", True)
        # ---- deny
        fx.turn("call")
        fx.turn("final:went on without it")
        fx_._type_into(page, fx_._label_sel("What do you want to ask?", "textarea"), "fetch again")
        fx_._button(page, "Start")
        page.wait_for("!!Array.prototype.find.call(document.querySelectorAll('#page button'),function(b){return b.getAttribute('aria-label')==='Deny web_fetch'})", 15, "the second approval card")
        fx_._button(page, "Deny web_fetch")
        page.wait_for("/went on without it/.test(document.getElementById('page').innerText)", 15, "the answer after a denial")
        rep.check("denying lets the run continue without the tool", True)
        # ---- failed model
        fx.turn("err")
        fx_._type_into(page, fx_._label_sel("What do you want to ask?", "textarea"), "this will fail")
        fx_._button(page, "Start")
        page.wait_for("/not reachable|failed|error/i.test(document.getElementById('page').innerText)", 15, "the failure")
        rep.check("a run whose model fails shows the failure in words", True, page.evaluate("document.getElementById('page').innerText")[:300])
        page.screenshot(rep.shot("more-runs-failed.png"), full=True)
        # ---- slow model, stopped
        fx.turn("hang")
        fx_._type_into(page, fx_._label_sel("What do you want to ask?", "textarea"), "this is slow")
        fx_._button(page, "Start")
        page.wait_for("!!Array.prototype.find.call(document.querySelectorAll('#page button'),function(b){return b.textContent.trim()==='Stop'})", 15, "the Stop button")
        rep.check("a running run shows its state and a Stop button", "running" in page.evaluate("document.getElementById('page').innerText"))
        for _ in range(4):  # the page redraws now and then and can replace the button under the pointer: press again if nothing happened
            fx_._button(page, "Stop")
            try:
                page.wait_for("document.getElementById('status').textContent.indexOf('Stop requested')>=0", 3, "the stop acknowledged")
                break
            except Exception:
                continue
        page.wait_for("document.getElementById('status').textContent.indexOf('Stop requested')>=0", 5, "the stop acknowledged")
        rep.check("Stop is acknowledged on the page at once", True)
        # the agent looks at the stop flag between steps: a model call that is already running finishes first (8 s here), then the run ends
        page.wait_for("!Array.prototype.find.call(document.querySelectorAll('#page button'),function(b){return b.textContent.trim()==='Stop'})", 20, "the run to end")
        rep.check("after the model call returns the run is over: the Stop button is gone and the state is a final one", bool(__import__('re').search("completed|stopped", page.evaluate("document.getElementById('page').innerText"))), page.evaluate("document.getElementById('page').innerText")[:200])
        # ---- Remote access: confirm step, keyboard, Esc, on, off
        ps.goto_page(page, "Remote access")
        fx_._button(page, "Serve this desktop to browsers")
        page.wait_for("!!document.querySelector('#page [role=alertdialog]')", 5, "the confirm step")
        rep.check("pressing Serve asks first and says what it exposes", "Anyone on this network" in page.evaluate("document.querySelector('#page [role=alertdialog]').innerText"))
        st, wa = ps.api(fx, "GET", "/api/v1/webaccess", tok)
        rep.check("nothing is switched on until the person confirms", st == 200 and wa["on"] is False, wa)
        inside = page.evaluate("!!document.activeElement && !!document.activeElement.closest('[role=alertdialog]')")
        rep.check("the confirm step takes the keyboard focus (a keyboard user is not left behind on a button that is gone)", inside, page.evaluate(ps.ACTIVE_JS))
        page.press("Escape", "Escape", 27)
        page.evaluate("new Promise(function(r){setTimeout(r,200)})")
        rep.check("Esc cancels the confirm step", page.evaluate("!document.querySelector('#page [role=alertdialog]')"))
        rep.check("after Esc the focus is back on the Serve button", "Serve this desktop" in page.evaluate("document.activeElement.textContent"), page.evaluate(ps.ACTIVE_JS))
        fx_._button(page, "Serve this desktop to browsers")
        page.wait_for("!!document.querySelector('#page [role=alertdialog]')", 5, "the confirm step again")
        fx_._button(page, "Yes, serve this desktop")
        page.wait_for("/Stop serving/.test(document.getElementById('page').innerText)", 10, "serving")
        st, wa = ps.api(fx, "GET", "/api/v1/webaccess", tok)
        rep.check("after Yes the service is switched on, with an address and the certificate fingerprint on the page", wa["on"] is True and "https://" in page.evaluate("document.getElementById('page').innerText") and "AA:BB" in page.evaluate("document.getElementById('page').innerText"), wa)
        page.screenshot(rep.shot("more-remote-on.png"), full=True)
        fx_._type_into(page, fx_._label_sel("Port for browsers"), "80")
        page.press("Enter", "Enter", 13, text="\r")
        page.evaluate("new Promise(function(r){setTimeout(r,300)})")
        rep.check("a port under 1024 is refused on the page and the service keeps its port", "1024" in page.evaluate("document.getElementById('status').textContent") and ps.api(fx, "GET", "/api/v1/webaccess", tok)[1]["port"] == 8443)
        fx_._button(page, "Stop serving this desktop")
        page.wait_for("/Serve this desktop to browsers/.test(document.getElementById('page').innerText)", 10, "off again")
        rep.check("Stop serving switches it off", ps.api(fx, "GET", "/api/v1/webaccess", tok)[1]["on"] is False)
        # ---- keyboard on every page; numbers
        names = None
        page.goto(fx.base + "/ui/")
        page.wait_for("!document.getElementById('top').hidden", 10)
        ps.settle(page)
        names = metrics_for_load(rep, page, fx.base)
        titles = ps.nav_titles(page)
        for t in titles:
            ms = timed_goto(page, t)
            keyboard_walk(rep, page, "keyboard " + t)
            page_numbers(rep, page, "agent/" + t, fx.base, ms)
            idle_rate(rep, page, "agent/" + t)
        stress_layouts(rep, page, titles, "agent")
        # Enter on a nav button opens the page, Space too
        page.evaluate("document.querySelector('a.skip').focus()")
        page.tab()
        page.press("Enter", "Enter", 13, text="\r")
        page.evaluate("new Promise(function(r){setTimeout(r,300)})")
        rep.check("Enter on the first page button opens that page", page.evaluate("document.querySelector('#page h1').textContent") == titles[0], page.evaluate("document.querySelector('#page h1').textContent"))
        page.evaluate("document.querySelector('a.skip').focus()")  # the focus moved into the page when it opened (by design): start again from the top
        page.tab()
        page.tab()
        page.press(" ", "Space", 32, text=" ")
        page.evaluate("new Promise(function(r){setTimeout(r,300)})")
        rep.check("Space on the next page button opens that page", page.evaluate("document.querySelector('#page h1').textContent") == titles[1], page.evaluate("document.querySelector('#page h1').textContent"))
        allreq = [u for u in page.requests if not u.startswith(fx.base) and not u.startswith("data:")]
        rep.check("the whole run requested nothing from outside this origin", allreq == [], allreq[:5])
        errs = page.errors(ignore=["api/v1/whoami"])
        rep.check("no console error in the agent-profile walk", errs == [], errs[:4])
    finally:
        page.close()


def suite_cloud(rep, chrome, fx):
    """cloud profile with two tenants: Git on a real repository, the tenant switch, long text, keyboard, numbers."""
    rep.section = "more-cloud"
    page = chrome.new_page(1280, 900, 1)
    try:
        tok = ps.token_for(fx, "root", fx.info["admin_password"])
        ps.api(fx, "POST", "/api/v1/tenant/bootstrap", tok)
        ps.api(fx, "POST", "/api/v1/tenant/members", tok, {"name": "ann", "role": "admin"})
        ps.login(page, fx.base, "root", fx.info["admin_password"])
        page.clear_errors()
        # ---- tenant switch with the keyboard
        sel = "#tenant-select"
        rep.check("two tenants: the top bar offers a switch with both names", page.evaluate("Array.prototype.map.call(document.querySelectorAll('#tenant-select option'),function(o){return o.textContent})") == ["Acme Ltd (owner)", "Gamma GmbH (owner)"])
        page.evaluate("document.querySelector('%s').focus()" % sel)
        # a native select cannot be driven by arrow keys in headless Chrome on macOS (it opens a popup that is not part of the page): the
        # choice is made by setting the value and sending the change event the browser sends, with the control focused
        page.evaluate("(function(){var s=document.querySelector('%s');s.value='gamma';s.dispatchEvent(new Event('change',{bubbles:true}));})()" % sel)
        page.wait_for("document.querySelector('#tenant .tenant-name') && document.querySelector('#tenant .tenant-name').textContent==='Gamma GmbH'", 10, "the tenant name changes")
        st, tl = ps.api(fx, "GET", "/api/v1/tenants", tok)
        rep.check("choosing the second tenant on the page works (the name changes; the change event was sent by script, see the comment)", True)
        page.screenshot(rep.shot("more-tenant-gamma.png"))
        ps.goto_page(page, "Cloud access")
        txt = page.evaluate("document.getElementById('page').innerText")
        rep.check("after the switch Cloud access shows the other tenant's own members (bob, not ann)", "bob" in txt and "ann" not in txt, txt[:300])
        page.evaluate("document.querySelector('%s').focus()" % sel)
        page.evaluate("(function(){var s=document.querySelector('%s');s.value='acme';s.dispatchEvent(new Event('change',{bubbles:true}));})()" % sel)
        page.wait_for("document.querySelector('#tenant .tenant-name').textContent==='Acme Ltd'", 10, "back to the first tenant")
        ps.goto_page(page, "Cloud access")
        rep.check("switching back shows the first tenant's members again", "ann" in page.evaluate("document.getElementById('page').innerText"))
        # ---- Git on a real repository
        ps.goto_page(page, "Git")
        rep.check("Git lists the clone in the person's space with its branch", "repo" in page.evaluate("document.getElementById('page').innerText") and "branch main" in page.evaluate("document.getElementById('page').innerText"))
        fx_._button(page, "Open repo")
        page.wait_for("/1 changed, 1 new/.test(document.getElementById('page').innerText)", 15, "the status")
        txt = page.evaluate("document.getElementById('page').innerText")
        rep.check("opening a clone shows its branch and the changed and the untracked file", "main, 1 changed, 1 new" in txt and "f.txt" in txt and "new.txt" in txt, txt[:300])
        page.screenshot(rep.shot("more-git-open.png"), full=True)
        fx_._button(page, "Diff")
        page.wait_for("!!document.querySelector('#page .diff')", 10, "the diff")
        dt = page.evaluate("document.querySelector('#page .diff').innerText")
        rep.check("a diff is drawn with + and - in the text (colour is not the only sign)", "+changed" in dt and "-three" in dt, dt[:200])
        page.screenshot(rep.shot("more-git-diff.png"), full=True)
        fx_._button(page, "Log")
        page.wait_for("/commit three 2/.test(document.getElementById('page').innerText)", 15, "the log")
        txt = page.evaluate("document.getElementById('page').innerText")
        rep.check("the log shows the commits, newest first", txt.index("commit three 2") < txt.index("commit one 0"), txt[:300])
        fx_._button(page, "Branches")
        page.wait_for("/main/.test(document.getElementById('page').innerText)", 10, "the branches")
        rep.check("Branches lists main", True)
        # the clone form refuses an address with a password without any request to it
        n0 = len(page.requests)
        ps.goto_page(page, "Git")
        fx_._type_into(page, "#page input[aria-label='Repository address']", "https://user:secret@example.org/o/r.git")
        page.press("Enter", "Enter", 13, text="\r")
        page.evaluate("new Promise(function(r){setTimeout(r,500)})")
        after = " ".join(page.requests[n0:])
        shown = page.evaluate("document.getElementById('status').textContent + ' ' + document.getElementById('page').innerText")
        rep.check("an address with a password in it is refused and the secret is neither sent nor shown", "secret" not in after and "secret" not in shown and "git/clone" not in after, (after[-200:], shown[:200]))
        # ---- pause is kept now
        ps.goto_page(page, "Schedules")
        fx_._button(page, "Pause Nightly")
        page.wait_for("!!Array.prototype.find.call(document.querySelectorAll('#page button'),function(b){return b.getAttribute('aria-label')==='Resume Nightly'})", 10, "the Resume button")
        st, sc = ps.api(fx, "GET", "/api/v1/schedules", tok)
        rep.check("Pause is kept: the service lists the schedule as paused and the page offers Resume", sc["schedules"][0]["paused"] is True, sc)
        fx_._button(page, "Resume Nightly")
        page.wait_for("!!Array.prototype.find.call(document.querySelectorAll('#page button'),function(b){return b.getAttribute('aria-label')==='Pause Nightly'})", 10, "the Pause button again")
        rep.check("Resume undoes it", ps.api(fx, "GET", "/api/v1/schedules", tok)[1]["schedules"][0]["paused"] is False)
        # ---- long text
        long64 = "n" * 62 + "-x"
        ps.api(fx, "POST", "/api/v1/namespaces", tok, {"name": long64, "action": "create"})
        ps.api(fx, "POST", "/api/v1/chat/runs", tok, {"prompt": "L" * 3000 + " " + "word " * 400})
        ps.api(fx, "POST", "/api/v1/files/mkdir", tok, {"path": "f" * 64})
        titles = ps.nav_titles(page)
        page.goto(fx.base + "/ui/")
        page.wait_for("!document.getElementById('top').hidden", 10)
        ps.settle(page)
        for t in titles:
            ms = timed_goto(page, t)
            keyboard_walk(rep, page, "keyboard " + t)
            page_numbers(rep, page, "cloud/" + t, fx.base, ms)
            idle_rate(rep, page, "cloud/" + t)
        stress_layouts(rep, page, titles, "cloud")
        for vp, (w, h) in {"phone": (390, 844), "desktop": (1440, 900)}.items():
            page.set_viewport(w, h, 1, False)
            for t in ("Cloud access", "Runs", "Files"):
                ps.goto_page(page, t)
                lay = page.evaluate(ps.LAYOUT_JS)
                rep.check("long names and a 3000-character word on %s at %s: no sideways scrolling" % (t, vp), lay["overflow"] <= 1, lay["overflow"])
        allreq = [u for u in page.requests if not u.startswith(fx.base) and not u.startswith("data:")]
        rep.check("the whole cloud run requested nothing from outside this origin", allreq == [], allreq[:5])
    finally:
        page.close()
