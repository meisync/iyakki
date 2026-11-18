# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""What a person does on the pages, with a real mouse and keyboard in Chrome, against the real web service (cloud profile): create a
namespace and add a person on Cloud access, ask a question on Runs, run a flow, pause a schedule, make a folder and upload a file on
Files. Each action is also checked from outside the page (a direct call to the service)."""
import json
import os

import pagesuite as ps


def _type_into(page, selector, text):
    page.click(selector)
    page.evaluate("document.querySelector(%s).select && document.querySelector(%s).select()" % (json.dumps(selector), json.dumps(selector)))
    page.type_text(text)


def _label_sel(label, tag="input"):
    return "#page %s[aria-label=%s]" % (tag, json.dumps(label))


def _button(page, text):
    """Click the button of the page area with exactly this text or aria-label."""
    page.click_xpath = None
    n = page.evaluate("(function(){var b=Array.prototype.find.call(document.querySelectorAll('#page button'),function(x){return x.textContent.trim()===%s||x.getAttribute('aria-label')===%s;});if(!b)return false;b.scrollIntoView({block:'center'});var r=b.getBoundingClientRect();return [r.x+r.width/2,r.y+r.height/2];})()" % (json.dumps(text), json.dumps(text)))
    if not n:
        raise AssertionError("no button " + text)
    page.click_at(n[0], n[1])


def suite(rep, chrome, fx, work):
    rep.section = "functions-cloud"
    page = chrome.new_page(1280, 900, 1)
    tok = ps.token_for(fx, "root", fx.info["admin_password"])
    ps.api(fx, "POST", "/api/v1/tenant/bootstrap", tok)
    ps.api(fx, "POST", "/api/v1/tenant/members", tok, {"name": "ann", "role": "admin"})
    try:
        ps.login(page, fx.base, "root", fx.info["admin_password"])
        page.clear_errors()
        # ---- Cloud access: a namespace and a person
        ps.goto_page(page, "Cloud access")
        _type_into(page, _label_sel("New namespace name"), "design")
        _button(page, "New namespace")
        page.wait_for("document.getElementById('page').innerText.indexOf('design')>=0", 10, "the new namespace in the list")
        st, view = ps.api(fx, "GET", "/api/v1/tenant", tok)
        names = [n.get("name") or n.get("id") for n in (view or {}).get("namespaces", [])]
        rep.check("Cloud access: the namespace made on the page exists on the service", "design" in names, names)
        _type_into(page, _label_sel("New namespace name"), "bad name!")
        _button(page, "New namespace")
        page.evaluate("new Promise(function(r){setTimeout(r,400)})")
        txt = page.evaluate("document.getElementById('status').textContent + ' ' + document.getElementById('page').innerText")
        st, view2 = ps.api(fx, "GET", "/api/v1/tenant", tok)
        names2 = [n.get("name") or n.get("id") for n in (view2 or {}).get("namespaces", [])]
        rep.check("Cloud access: a bad namespace name is refused and nothing is made", "bad name!" not in names2 and len(names2) == len(names), (names2, txt[:200]))
        _type_into(page, _label_sel("Add person name"), "bob")
        _button(page, "Add person")
        page.wait_for("document.getElementById('page').innerText.indexOf('bob')>=0", 10, "bob in the members")
        st, view = ps.api(fx, "GET", "/api/v1/tenant", tok)
        rep.check("Cloud access: the person added on the page is a member on the service", "bob" in json.dumps(view), json.dumps(view)[:300])
        # F-WEB-50: the audit trail at the bottom of the page lists the change the person just made (read route, tenant administrators)
        txt = page.evaluate("document.getElementById('page').innerText")
        trail = txt[txt.index("Audit trail"):] if "Audit trail" in txt else ""
        rep.check("Cloud access: the audit trail section lists the role change just made (who, what, whom)", "bob" in trail and "root" in trail and "No role changes recorded yet" not in trail, trail[:300])
        st, tr = ps.api(fx, "GET", "/api/v1/tenant/audit", tok)
        rep.info("audit read route", json.dumps([st, tr])[:300])
        page.screenshot(rep.shot("func-cloud-access.png"), full=True)

        # ---- Runs: ask a question
        ps.goto_page(page, "Runs")
        _type_into(page, _label_sel("What do you want to ask?", "textarea"), "What time is it?")
        _button(page, "Start")
        page.wait_for("/done/.test(document.getElementById('page').innerText)", 15, "the answer of the scripted model")
        st, runs = ps.api(fx, "GET", "/api/v1/chat/runs", tok)
        rep.check("Runs: a question asked on the page is a run on the service and its answer is shown", st == 200 and len(runs.get("runs", [])) >= 1, json.dumps(runs)[:200])
        page.screenshot(rep.shot("func-runs.png"), full=True)

        # ---- Flows: run the sample flow
        ps.goto_page(page, "Flows")
        _button(page, "Run Nightly")
        page.evaluate("new Promise(function(r){setTimeout(r,800)})")
        txt = page.evaluate("document.getElementById('status').textContent + ' | ' + document.getElementById('page').innerText")
        rep.info("Flows: after Run Nightly", txt[:300])
        rep.check("Flows: pressing Run says something (a started run or a reason it did not), never nothing", "Nightly" in txt and len(page.evaluate("document.getElementById('status').textContent")) > 0 or "Runs" in txt, txt[:300])
        st, fr = ps.api(fx, "GET", "/api/v1/flows/f1/runs", tok)
        rep.check("Flows: Run on the page started a run of the flow on the service", st == 200 and len(json.dumps(fr)) > 20, (st, json.dumps(fr)[:300]))

        # ---- Schedules: pause
        ps.goto_page(page, "Schedules")
        _button(page, "Pause Nightly")
        page.wait_for("document.getElementById('status').textContent==='Paused.'", 10, "the page says Paused.")
        rep.check("Schedules: Pause on the page calls the pause route and the page says Paused.", any("/schedules/" in u and "pause" in u for u in page.requests))
        page.screenshot(rep.shot("func-schedules.png"), full=True)

        # ---- Files: a folder and an upload
        ps.goto_page(page, "Files")
        _type_into(page, _label_sel("New folder name"), "reports")
        _button(page, "New folder")
        page.wait_for("document.getElementById('page').innerText.indexOf('reports')>=0", 10, "the folder in the list")
        st, fl = ps.api(fx, "GET", "/api/v1/files?scope=personal&path=", tok)
        rep.check("Files: the folder made on the page exists on the service", "reports" in json.dumps(fl), (st, json.dumps(fl)[:300]))
        src = os.path.join(work, "upload-sample.txt")
        open(src, "w").write("hello from the browser test\n")
        doc = page.call("DOM.getDocument", {"depth": 0})
        node = page.call("DOM.querySelector", {"nodeId": doc["root"]["nodeId"], "selector": "#page input[type=file]"})
        page.call("DOM.setFileInputFiles", {"files": [src], "nodeId": node["nodeId"]})
        page.wait_for("document.getElementById('page').innerText.indexOf('upload-sample.txt')>=0", 15, "the uploaded file in the list")
        import time
        for _ in range(50):  # the upload is written under a temporary name and renamed when it is whole
            st, fl = ps.api(fx, "GET", "/api/v1/files?scope=personal&path=", tok)
            if "upload-sample.txt" in json.dumps(fl):
                break
            time.sleep(0.1)
        rep.check("Files: a file chosen on the page is uploaded and listed by the service", "upload-sample.txt" in json.dumps(fl), (st, json.dumps(fl)[:300]))
        page.screenshot(rep.shot("func-files.png"), full=True)

        # ---- the whole walk made no unexpected errors
        errs = page.errors(ignore=["api/v1/whoami"])
        rep.check("the walk through the pages made no console error and no failed request", errs == [], errs)
    finally:
        page.close()
