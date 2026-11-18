# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""Page-level tests of the web UI (`ui/*.js`) served by the real web service, run in real Chrome: loading without console errors,
sign-in flow, every page renders, accessibility basics, phone/tablet/desktop widths, light and dark."""
import json

import cdp

VIEWPORTS = {"phone": (390, 844, 2, True), "tablet": (820, 1180, 2, True), "desktop": (1440, 900, 1, False)}

# Accessibility problems visible in the DOM. Returns a list of strings.
A11Y_JS = r"""(function(){
  var out = [];
  function vis(e){ var r = e.getBoundingClientRect(); var cs = getComputedStyle(e); return r.width > 0 && r.height > 0 && cs.visibility !== 'hidden' && cs.display !== 'none' && !e.closest('[hidden]'); }
  function name(e){
    var a = e.getAttribute('aria-label'); if (a && a.trim()) return a.trim();
    var lb = e.getAttribute('aria-labelledby');
    if (lb) { var t = lb.split(/\s+/).map(function(i){ var n = document.getElementById(i); return n ? n.textContent : ''; }).join(' ').trim(); if (t) return t; }
    if (e.labels && e.labels.length) { var l = Array.prototype.map.call(e.labels, function(x){ return x.textContent; }).join(' ').trim(); if (l) return l; }
    if (e.tagName === 'BUTTON' || e.tagName === 'A') { var t2 = e.textContent.trim(); if (t2) return t2; }
    if (e.title) return e.title;
    if (e.tagName === 'INPUT' && (e.type === 'submit' || e.type === 'button') && e.value) return e.value;
    return '';
  }
  if (!document.documentElement.lang) out.push('html has no lang');
  if (!document.title.trim()) out.push('page has no title');
  var m = document.querySelectorAll('main'); if (m.length !== 1) out.push('expected one main landmark, found ' + m.length);
  var hs = Array.prototype.filter.call(document.querySelectorAll('h1'), vis); if (hs.length !== 1) out.push('expected one visible h1, found ' + hs.length);
  var last = 0;
  Array.prototype.filter.call(document.querySelectorAll('h1,h2,h3,h4,h5,h6'), vis).forEach(function(h){ var n = +h.tagName[1]; if (last && n > last + 1) out.push('heading level jumps from h' + last + ' to h' + n + ': ' + h.textContent.trim().slice(0, 40)); last = n; });
  var ids = {}; document.querySelectorAll('[id]').forEach(function(e){ if (ids[e.id]) out.push('duplicate id "' + e.id + '" on <' + e.tagName.toLowerCase() + '> ' + e.outerHTML.slice(0, 60)); ids[e.id] = 1; });
  document.querySelectorAll('input,select,textarea').forEach(function(e){
    if (e.type === 'hidden' || !vis(e) && !e.closest('label')) return;
    if (!name(e)) out.push('field without a label: <' + e.tagName.toLowerCase() + ' ' + (e.id || e.name || e.type) + '>');
  });
  document.querySelectorAll('button,[role=button],a[href]').forEach(function(e){ if (vis(e) && !name(e)) out.push('control without a name: ' + e.outerHTML.slice(0, 80)); });
  document.querySelectorAll('img').forEach(function(e){ if (!e.hasAttribute('alt')) out.push('img without alt'); });
  document.querySelectorAll('canvas').forEach(function(e){ if (vis(e) && !e.getAttribute('aria-label') && !e.getAttribute('aria-labelledby') && !e.getAttribute('role')) out.push('canvas without a name or role'); });
  document.querySelectorAll('[tabindex]').forEach(function(e){ if (+e.getAttribute('tabindex') > 0) out.push('positive tabindex on ' + e.tagName); });
  document.querySelectorAll('table').forEach(function(t){ if (vis(t) && !t.querySelector('th')) out.push('table without header cells'); });
  document.querySelectorAll('label[for]').forEach(function(l){ if (!document.getElementById(l.getAttribute('for'))) out.push('label for a missing id ' + l.getAttribute('for')); });
  return out;
})()"""

# Contrast of every visible text element against its effective background; returns [{text, ratio, fg, bg, size}] for those under 4.5 (3 for large text).
CONTRAST_JS = r"""(function(){
  function parse(c){ var m = c.match(/rgba?\(([^)]+)\)/); if (!m) { var s = c.match(/color\(srgb ([^)]+)\)/); if (s) { var q = s[1].split(/[\s/]+/).map(Number); return [q[0]*255, q[1]*255, q[2]*255, q.length > 3 ? q[3] : 1]; } return null; } var p = m[1].split(/[,\s/]+/).filter(Boolean).map(Number); return [p[0], p[1], p[2], p.length > 3 ? p[3] : 1]; }
  function lum(c){ var f = c.slice(0,3).map(function(v){ v /= 255; return v <= 0.03928 ? v/12.92 : Math.pow((v+0.055)/1.055, 2.4); }); return 0.2126*f[0] + 0.7152*f[1] + 0.0722*f[2]; }
  function over(top, bottom){ var a = top[3]; return [top[0]*a + bottom[0]*(1-a), top[1]*a + bottom[1]*(1-a), top[2]*a + bottom[2]*(1-a), 1]; }
  function bgOf(e){ var layers = []; for (var n = e; n; n = n.parentElement) { var c = parse(getComputedStyle(n).backgroundColor); if (c && c[3] > 0) { layers.push(c); if (c[3] >= 1) break; } }
    var base = parse(getComputedStyle(document.documentElement).backgroundColor); if (!base || base[3] < 1) base = [255,255,255,1];
    var cur = base; for (var i = layers.length - 1; i >= 0; i--) cur = over(layers[i], cur); return cur; }
  var bad = [], seen = 0;
  var walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  var done = new Set();
  while (walker.nextNode()) {
    var t = walker.currentNode; if (!t.nodeValue.trim()) continue;
    var e = t.parentElement; if (!e || done.has(e)) continue; done.add(e);
    var cs = getComputedStyle(e), r = e.getBoundingClientRect();
    if (r.width === 0 || r.height === 0 || cs.visibility === 'hidden' || e.closest('[hidden],noscript,script,style')) continue;
    var fg = parse(cs.color), bg = bgOf(e); if (!fg) continue;
    fg = over(fg, bg);
    var l1 = lum(fg), l2 = lum(bg), ratio = (Math.max(l1,l2)+0.05)/(Math.min(l1,l2)+0.05);
    var size = parseFloat(cs.fontSize), bold = parseInt(cs.fontWeight, 10) >= 700, large = size >= 24 || (size >= 18.66 && bold);
    seen++;
    var disabled = e.closest('button:disabled,input:disabled,select:disabled');
    if (!disabled && ratio < (large ? 3 : 4.5)) bad.push({text: t.nodeValue.trim().slice(0, 40), ratio: Math.round(ratio*100)/100, fg: cs.color, bg: 'rgb(' + bg.slice(0,3).map(Math.round).join(',') + ')', size: size, tag: e.tagName.toLowerCase() + (e.className ? '.' + e.className : '')});
  }
  return {seen: seen, bad: bad};
})()"""

LAYOUT_JS = r"""(function(){
  var de = document.documentElement, out = {overflow: de.scrollWidth - de.clientWidth, wide: Array.prototype.filter.call(document.querySelectorAll('main *'), function(e){var r=e.getBoundingClientRect();return r.width>0&&r.right>de.clientWidth+1&&!e.closest('[hidden]')&&!e.closest('.diff,pre,table,.tile-stage');}).slice(0,3).map(function(e){return e.tagName.toLowerCase()+'.'+e.className+' "'+(e.textContent||'').trim().slice(0,24)+'" right='+Math.round(e.getBoundingClientRect().right);}), small: [], tiny: [], clipped: [], links: []};
  document.querySelectorAll('button,input,select,textarea,a[href],[role=button]').forEach(function(e){
    var r = e.getBoundingClientRect(), cs = getComputedStyle(e);
    if (r.width === 0 || r.height === 0 || cs.visibility === 'hidden' || cs.opacity === '0' || e.closest('[hidden]') || e.type === 'hidden') return;
    if (e.closest('.skip') || e.classList.contains('tile-input') || (e.type === 'file' && e.closest('label'))) return; // a file input sits inside its label, and the label is the target
    if (e.tagName === 'A' && e.closest('p,li,span,.muted,.line')) { out.links.push(e.textContent.trim().slice(0, 20) + ' ' + Math.round(r.width) + 'x' + Math.round(r.height)); return; }
    if (r.width < 24 - 0.5 || r.height < 24 - 0.5) out.tiny.push((e.tagName.toLowerCase() + '#' + (e.id || '') + ' "' + (e.textContent || e.value || e.type || '').trim().slice(0, 20) + '" ' + Math.round(r.width) + 'x' + Math.round(r.height)));
    if (r.width < 44 - 0.5 || r.height < 44 - 0.5) out.small.push((e.tagName.toLowerCase() + '#' + (e.id || '') + ' "' + (e.textContent || e.value || e.type || '').trim().slice(0, 20) + '" ' + Math.round(r.width) + 'x' + Math.round(r.height)));
    if (r.right > de.clientWidth + 1 || r.left < -1) out.clipped.push(e.tagName.toLowerCase() + '#' + (e.id || '') + ' "' + (e.textContent || '').trim().slice(0, 20) + '" right=' + Math.round(r.right));
  });
  return out;
})()"""

# the order a person tabs through: the real answer from the browser, one Tab at a time
ACTIVE_JS = "(function(){var a=document.activeElement;if(!a||a===document.body)return 'body';return a.tagName.toLowerCase()+(a.id?'#'+a.id:'')+(a.className&&typeof a.className==='string'?'.'+a.className.split(' ')[0]:'')+(a.textContent&&a.tagName==='BUTTON'?'['+a.textContent.trim().slice(0,16)+']':'');})()"

FOCUS_STYLE_JS = "(function(){var a=document.activeElement;if(!a)return null;var cs=getComputedStyle(a);return {style:cs.outlineStyle,width:parseFloat(cs.outlineWidth)};})()"


def api(fx, method, path, token=None, body=None):
    """A direct call to the service (for seeding data and for checking what the page showed). Returns (status, json)."""
    import ssl
    import urllib.error
    import urllib.request
    ctx = ssl.create_default_context()
    ctx.check_hostname = False
    ctx.verify_mode = ssl.CERT_NONE
    data = json.dumps(body).encode() if body is not None else None
    rq = urllib.request.Request(fx.base + path, data=data, method=method)
    if data is not None:
        rq.add_header("Content-Type", "application/json")
    if token:
        rq.add_header("Authorization", "Bearer " + token)
    try:
        r = urllib.request.urlopen(rq, context=ctx, timeout=20)
        raw = r.read()
        return r.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as e:
        raw = e.read()
        try:
            return e.code, json.loads(raw)
        except ValueError:
            return e.code, None


def token_for(fx, name, pw):
    st, v = api(fx, "POST", "/api/v1/login", body={"name": name, "password": pw})
    assert st == 200, (st, v)
    return v["token"]


def login(page, base, user, pw, expect_ok=True):
    page.goto(base + "/ui/")
    page.wait_for("!document.getElementById('signin').hidden", 10, "sign-in form")
    page.click("#user")
    page.type_text(user)
    page.click("#pass")
    page.type_text(pw)
    page.click("#signin-go")
    if expect_ok:
        page.wait_for("!document.getElementById('top').hidden", 10, "signed-in top bar")
        settle(page)


def settle(page, timeout=10):
    """Wait until the page area has stopped saying Loading and has a heading."""
    page.wait_for("(function(){var p=document.getElementById('page');return !p.hidden && !!p.querySelector('h1') && !/Loading/.test(p.textContent);})()", timeout, "page drawn")
    page.evaluate("new Promise(function(r){setTimeout(r,150)})")


def nav_titles(page):
    page.wait_for("document.querySelectorAll('#nav button').length>0", 10, "navigation drawn")
    return page.evaluate("Array.prototype.map.call(document.querySelectorAll('#nav button'), function(b){return b.textContent;})")


def goto_page(page, title):
    page.click("#nav button[aria-current], #nav button") if False else None
    page.evaluate("(function(){var b=Array.prototype.find.call(document.querySelectorAll('#nav button'),function(x){return x.textContent===%s});b.click();})()" % json.dumps(title))
    page.wait_for("document.querySelector('#page h1') && document.querySelector('#page h1').textContent===%s" % json.dumps(title), 10, "page " + title)
    settle(page)


def suite_pages(rep, chrome, fx, tag):
    """One fixture profile: sign-in, every page, a11y, layouts, themes."""
    base = fx.base
    prof = fx.info["profile"]
    rep.section = "pages-" + tag
    page = chrome.new_page(1280, 800, 1, False)
    try:
        # ---- the sign-in page itself
        page.goto(base + "/ui/")
        page.wait_for("!document.getElementById('signin').hidden", 10, "sign-in form")
        # the probe "am I already signed in here" answers 401 on purpose; the browser logs every failed request, nothing to do about it
        errs = page.errors(ignore=["api/v1/whoami"])
        rep.check("sign-in page: no console errors and no failed requests (the 401 of the whoami probe is expected)", errs == [], errs)
        page.clear_errors()
        rep.check("sign-in page: title and heading", page.evaluate("document.title") == "IYAKKI" and page.evaluate("document.querySelector('#signin h1').textContent") == "Sign in")
        page.screenshot(rep.shot("%s-signin-desktop-light.png" % tag))
        # the name field has the focus when the form appears; Tab goes to the password, then the button, then out of the page and round to the skip link
        order = [page.evaluate(ACTIVE_JS)]
        for _ in range(4):
            page.tab()
            order.append(page.evaluate(ACTIVE_JS))
        rep.check("sign-in page: focus starts in the name field; Tab goes password, Sign in, (browser), skip link, name", order[:3] == ["input#user", "input#pass", "button#signin-go.primary[Sign in]"] and order[4] == "a.skip" or order == ["input#user", "input#pass", "button#signin-go.primary[Sign in]", "body", "a.skip"], order)
        page.evaluate("document.getElementById('pass').focus()")
        page.tab(shift=True)
        page.tab(shift=True)
        rep.check("sign-in page: Shift+Tab goes back to the skip link before the name field", page.evaluate(ACTIVE_JS) == "a.skip", page.evaluate(ACTIVE_JS))
        fs = page.evaluate(FOCUS_STYLE_JS)
        rep.check("a focused control shows an outline (keyboard users can see where they are)", fs and fs["style"] != "none" and fs["width"] >= 2, fs)
        a11y = page.evaluate(A11Y_JS)
        # the hidden top bar and page area have no heading yet, that is expected: only h1 count of visible matters
        rep.check("sign-in page: accessibility basics", a11y == [], a11y)
        # wrong password
        page.click("#user")
        page.type_text("root")
        page.click("#pass")
        page.type_text("wrong-password")
        page.click("#signin-go")
        page.wait_for("document.getElementById('status').textContent.length>0", 10, "error text")
        rep.check("a wrong password says so and stays on the form", page.evaluate("document.getElementById('status').textContent") == "The name or the password is not right." and page.evaluate("!document.getElementById('signin').hidden") and page.evaluate("document.getElementById('pass').value") == "")
        rep.check("the error is announced (status is a polite live region)", page.evaluate("document.getElementById('status').getAttribute('aria-live')") == "polite")
        page.clear_errors()  # the 401 of the wrong password is the expected failed request
        # ---- the right one
        page.click("#user")
        page.evaluate("document.getElementById('user').select()")
        page.type_text("root")
        page.click("#pass")
        page.type_text(fx.info["admin_password"])
        page.press("Enter", "Enter", 13, text="\r")
        page.wait_for("!document.getElementById('top').hidden", 10, "signed-in")
        settle(page)
        rep.check("sign in by the Enter key works", page.evaluate("document.getElementById('who').textContent") == "root (admin)", page.evaluate("document.getElementById('who').textContent"))
        store = page.evaluate("({s: Object.keys(sessionStorage), l: Object.keys(localStorage), c: document.cookie})")
        rep.check("the token is in this tab's session storage only (no localStorage, no cookie)", len(store["s"]) == 1 and store["l"] == [] and store["c"] == "", store)
        page.goto(base + "/ui/")
        page.wait_for("!document.getElementById('top').hidden", 10, "session survives a reload")
        rep.check("a reload keeps the session of the tab", True)
        page.clear_errors()
        titles = nav_titles(page)
        if prof == "cloud":
            # a fresh pod: the tenant has no lists yet, so the tenant pages say "not allowed" in words (and the browser logs the 403s)
            goto_page(page, "Cloud access")
            txt = page.evaluate("document.getElementById('page').innerText")
            rep.check("a pod with no tenant yet: Cloud access says so in words, not a blank page", len(txt) > len("Cloud access") + 10, txt[:200])
            page.screenshot(rep.shot("cloud-access-before-bootstrap.png"))
            rep.check("a pod with no tenant yet: an administrator is offered to become its first owner", "Make me the owner" in txt, txt[:200])
            page.clear_errors()
            tok = token_for(fx, "root", fx.info["admin_password"])
            rep.check("seed: tenant bootstrap", api(fx, "POST", "/api/v1/tenant/bootstrap", tok)[0] == 200)
            rep.check("seed: members", api(fx, "POST", "/api/v1/tenant/members", tok, {"name": "ann", "role": "admin"})[0] == 200 and api(fx, "POST", "/api/v1/tenant/members", tok, {"name": "bob", "role": "member"})[0] == 200)
            rep.check("seed: namespace", api(fx, "POST", "/api/v1/namespaces", tok, {"name": "web", "action": "create"})[0] == 200)
            page.goto(fx.base + "/ui/")
            titles = nav_titles(page)
            settle(page)
            page.clear_errors()
        rep.info("pages offered to the administrator", titles)
        want = {"cloud": ["Files", "Git", "Flows", "Runs", "Chat", "Schedules", "Cloud access", "Tenant settings", "Notifications", "Tiles"], "agent-api": ["Runs", "Chat", "Remote access", "Notifications", "Tiles"]}[prof]
        rep.check("the navigation offers the pages this profile has", sorted(titles) == sorted(want), titles)

        # ---- every page: renders, no errors, a11y, in three sizes and two themes
        for title in titles:
            goto_page(page, title)
            errs = page.errors()
            rep.check("page %s: draws with no console error and no failed request" % title, errs == [], errs)
            page.clear_errors()
            h1 = page.evaluate("document.querySelector('#page h1').textContent")
            rep.check("page %s: heading" % title, h1 == title)
            a11y = page.evaluate(A11Y_JS)
            rep.check("page %s: accessibility basics" % title, a11y == [], a11y)
            rep.check("page %s: the nav button of the page is marked current" % title, page.evaluate("(function(){var c=document.querySelector('#nav [aria-current=page]');return c&&c.textContent;})()") == title)
            slug = title.lower().replace(" ", "-")
            for vp, (w, h, dpr, mob) in VIEWPORTS.items():
                for scheme in ("light", "dark"):
                    page.set_viewport(w, h, dpr, mob)
                    page.set_color_scheme(scheme)
                    page.evaluate("new Promise(function(r){setTimeout(r,120)})")
                    lay = page.evaluate(LAYOUT_JS)
                    con = page.evaluate(CONTRAST_JS)
                    page.screenshot(rep.shot("%s-%s-%s-%s.png" % (tag, slug, vp, scheme)), full=True)
                    rep.check("page %s %s %s: no sideways scrolling" % (title, vp, scheme), lay["overflow"] <= 1, "overflow %s px" % lay["overflow"])
                    rep.check("page %s %s %s: no control pushed off the screen" % (title, vp, scheme), lay["clipped"] == [], lay["clipped"][:5])
                    if scheme == "light":
                        rep.check("page %s %s: controls are at least 24x24 (WCAG 2.2 AA target size)" % (title, vp), lay["tiny"] == [], lay["tiny"][:6])
                        rep.check("page %s %s: controls are at least 44x44 (links inside a sentence excepted)" % (title, vp), lay["small"] == [], lay["small"][:6])
                    rep.check("page %s %s %s: text contrast at least 4.5:1 (3:1 for large text)" % (title, vp, scheme), con["bad"] == [], con["bad"][:5])
                    bg = page.evaluate("getComputedStyle(document.body).backgroundColor")
                    dark = sum(int(x) for x in bg[bg.index("(") + 1:bg.index(")")].split(",")[:3]) < 3 * 128
                    rep.check("page %s %s: the %s theme is applied (body background %s)" % (title, vp, scheme, bg), dark == (scheme == "dark"))
            page.set_viewport(1280, 800, 1, False)
            page.set_color_scheme("light")

        # ---- keyboard on the signed-in shell: the header comes before the page in the document, and Tab from the skip link walks the page buttons
        page.set_viewport(1280, 800, 1, False)
        foc = page.evaluate("(function(){var sel='a[href],button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),[tabindex]';var all=Array.prototype.filter.call(document.querySelectorAll(sel),function(e){var r=e.getBoundingClientRect();return r.width>0&&r.height>0&&!e.closest('[hidden]')&&e.tabIndex>=0;});var m=document.getElementById('main');return all.map(function(e){return (m.contains(e)?'main:':'top:')+e.tagName.toLowerCase();});})()")
        first_main = next((i for i, v in enumerate(foc) if v.startswith("main:")), len(foc))
        rep.check("signed-in shell: every control of the top bar comes before the page in tab order", all(v.startswith("top:") for v in foc[:first_main]) and not any(v.startswith("top:") for v in foc[first_main:]), foc[:25])
        page.evaluate("document.querySelector('a.skip').focus()")
        order = []
        for _ in range(len(titles)):
            page.tab()
            order.append(page.evaluate(ACTIVE_JS))
        want_order = ["button[%s]" % t[:16] for t in titles]
        rep.check("signed-in shell: Tab from the skip link visits the page buttons in the order shown", order == want_order, order)
        before = page.evaluate("document.querySelector('#page h1').textContent")
        page.evaluate("document.querySelector('a.skip').focus()")
        page.press("Enter", "Enter", 13, text="\r")
        page.evaluate("new Promise(function(r){setTimeout(r,400)})")
        after = page.evaluate("document.querySelector('#page h1') && document.querySelector('#page h1').textContent")
        rep.check("the skip link keeps the person on the page they are on (%s)" % before, after == before, "page is now %s, hash %s" % (after, page.evaluate("location.hash")))
        rep.check("the skip link puts the focus in the main area", page.evaluate("document.activeElement.id") == "main" or page.evaluate("!!document.activeElement.closest('main')"), page.evaluate(ACTIVE_JS))

        # ---- sign out
        page.click("#signout")
        page.wait_for("!document.getElementById('signin').hidden", 10, "back to the form")
        rep.check("sign out shows the form again and removes the token", page.evaluate("Object.keys(sessionStorage).length") == 0 and page.evaluate("document.getElementById('top').hidden"))
        page.goto(base + "/ui/")
        page.wait_for("!document.getElementById('signin').hidden", 10, "form after reload")
        rep.check("after sign out a reload asks for the password", True)
        errs = page.errors(ignore=["api/v1/whoami", "api/v1/approvals", "api/v1/notifications"])
        rep.check("sign-out and the reload cause no errors except 401 answers of calls made just before the token went", errs == [], errs)
    finally:
        page.close()
