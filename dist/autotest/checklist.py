#!/usr/bin/env python3
"""The owner's checklist, offline form (the contract is in docs/development/AUTOTEST.md and dist/autotest/humanschema.py).

  ./run.sh autotest --checklist --results OUT           writes OUT/checklist/<machine>.html (one page per machine) and index.html
  python3 dist/autotest/checklist.py --serve OUT [--port N]   loopback server: the same pages, and a Pass/Fail click writes OUT/human/<ROW>.<clause>.<machine>.json

One self-contained HTML file per machine: pre-filled steps from dist/autotest/checklists/*.json, the runner's screenshots embedded, Pass / Fail / Cannot test
buttons and a note field. Opened as a file the answer is saved as a download with the right name; served, it is written into OUT/human/. No cloud, no
password or key is asked for, nothing leaves the computer.
"""
import argparse
import base64
import html
import http.server
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import humanschema as H  # noqa: E402
import results as R  # noqa: E402

MAX_IMG = 400 * 1024
MAX_BODY = 64 * 1024


def machines_in(results, lists):
    found = {r["level"][len("machine-"):] for r in results if r.get("level", "").startswith("machine-")}
    for _p, c, bad in lists:
        if c and not bad:
            for cl in c["clauses"]:
                found.update(m for m in cl["machines"] if m not in ("any", "qemu"))
    return sorted(found)


def screenshots_for(results, row, limit=3):
    """data: URIs of the first PNGs of the row's evidence folders (any level), small ones only."""
    out = []
    for r in sorted((r for r in results if r["row"] == row), key=lambda r: r.get("ran_at", ""), reverse=True):
        for p in r.get("evidence", []):
            if p.endswith(".png"):
                fp = os.path.join(r["_dir"], p)
                try:
                    if os.path.getsize(fp) <= MAX_IMG:
                        with open(fp, "rb") as fh:
                            out.append((os.path.basename(p), "data:image/png;base64," + base64.b64encode(fh.read()).decode("ascii")))
                except OSError:
                    pass
            if len(out) >= limit:
                return out
    return out


def render_page(machine, lists, results, image_commit):
    e = html.escape
    secs = []
    for _p, c, bad in lists:
        if not c or bad:
            continue
        cls = [cl for cl in c["clauses"] if "any" in cl["machines"] or machine in cl["machines"]]
        if not cls:
            continue
        shots = screenshots_for(results, c["row"])
        body = ['<section class="row"><h2>%s <small>%s</small></h2>' % (e(c["row"]), e(c["title"]))]
        for n, uri in shots:
            body.append('<figure><img src="%s" alt="%s"><figcaption>runner: %s</figcaption></figure>' % (uri, e(n), e(n)))
        for cl in cls:
            body.append('<div class="clause" data-row="%s" data-clause="%s"><h3>%s</h3><ol>' % (e(c["row"]), e(cl["id"]), e(cl["text"])))
            for s in cl["steps"]:
                body.append("<li><b>Do:</b> %s<br><b>Expect:</b> %s%s</li>" % (e(s["do"]), e(s["expect"]), ("<br><i>The runner already did: %s</i>" % e(s["runner_did"])) if s.get("runner_did") else ""))
            body.append('</ol><input class="note" placeholder="note (optional)" maxlength="500"> '
                        '<button data-v="pass">Pass</button> <button data-v="fail">Fail</button> <button data-v="cannot-test">Cannot test</button> <span class="saved"></span></div>')
        body.append("</section>")
        secs.append("".join(body))
    if not secs:
        secs = ["<p>Nothing to check on this machine.</p>"]
    return PAGE % {"machine": e(machine), "commit": e(image_commit or ""), "sections": "\n".join(secs), "machine_js": json.dumps(machine), "commit_js": json.dumps(image_commit or "")}


PAGE = """<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Checklist %(machine)s</title><style>
:root{--bg:#fff;--fg:#1a1a1a;--card:#f3f3f3;--ok:#0a6b2d;--bad:#a11}@media(prefers-color-scheme:dark){:root{--bg:#181818;--fg:#eee;--card:#262626;--ok:#6c6;--bad:#f77}}
body{background:var(--bg);color:var(--fg);font:16px system-ui,sans-serif;margin:0 auto;max-width:60rem;padding:0 16px}
.clause{background:var(--card);border-radius:8px;padding:12px;margin:12px 0}img{max-width:100%%;height:auto}button{font:inherit;padding:6px 14px;margin:4px}
.saved{font-weight:600}.note{width:min(24rem,100%%)}</style></head><body>
<h1>Checklist: %(machine)s</h1><p>Image commit <input id="commit" value="%(commit)s" size="12"> Your name or id <input id="by" value="owner" size="12"> (nothing else is collected)</p>
%(sections)s
<script>
const MACHINE=%(machine_js)s;
function save(div,v){const rec={schema:1,row:div.dataset.row,clause:div.dataset.clause,machine:MACHINE,image_commit:document.getElementById('commit').value.trim(),
by:document.getElementById('by').value.trim()||'owner',verdict:v,date:new Date().toISOString().slice(0,19),note:div.querySelector('.note').value};
const name=rec.row+'.'+rec.clause+'.'+rec.machine+'.json',out=div.querySelector('.saved');
function dl(){const a=document.createElement('a');a.href=URL.createObjectURL(new Blob([JSON.stringify(rec,null,1)],{type:'application/json'}));a.download=name;a.click();out.textContent='downloaded '+name+' (put it in human/)';}
if(location.protocol==='http:'){fetch('/save',{method:'POST',body:JSON.stringify(rec)}).then(r=>r.ok?(out.textContent='saved '+v):r.text().then(t=>out.textContent='not saved: '+t)).catch(dl);}else{dl();}}
document.querySelectorAll('.clause button').forEach(b=>b.onclick=()=>save(b.closest('.clause'),b.dataset.v));
</script></body></html>
"""


def write_pages(results_dir, out_dir, checklists_dir=os.path.join(HERE, "checklists")):
    """Writes one page per machine and an index; returns the paths."""
    results, _ = R.load_results(results_dir)
    lists = H.load_checklists(checklists_dir)
    commit = ""
    try:
        with open(os.path.join(results_dir, "run.json"), encoding="utf-8") as fh:
            commit = json.load(fh).get("commit", "")
    except (OSError, ValueError):
        pass
    os.makedirs(out_dir, exist_ok=True)
    paths = []
    ms = machines_in(results, lists) or ["any"]
    for m in ms:
        p = os.path.join(out_dir, "%s.html" % m)
        with open(p, "w", encoding="utf-8") as fh:
            fh.write(render_page(m, lists, results, commit))
        paths.append(p)
    with open(os.path.join(out_dir, "index.html"), "w", encoding="utf-8") as fh:
        fh.write("<!doctype html><meta charset=utf-8><title>Checklists</title><ul>%s</ul>" % "".join('<li><a href="%s.html">%s</a></li>' % (html.escape(m), html.escape(m)) for m in ms))
    paths.append(os.path.join(out_dir, "index.html"))
    return paths


def make_handler(root):
    out_dir = os.path.join(root, "checklist")
    human = os.path.join(root, "human")

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def _send(self, code, body, ctype="text/plain; charset=utf-8"):
            b = body.encode("utf-8") if isinstance(body, str) else body
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(b)))
            self.end_headers()
            self.wfile.write(b)

        def do_GET(self):
            name = self.path.split("?")[0].strip("/") or "index.html"
            if not name.endswith(".html") or "/" in name or ".." in name:
                return self._send(404, "not found")
            try:
                with open(os.path.join(out_dir, name), "rb") as fh:
                    self._send(200, fh.read(), "text/html; charset=utf-8")
            except OSError:
                self._send(404, "not found")

        def do_POST(self):
            if self.path != "/save":
                return self._send(404, "not found")
            n = int(self.headers.get("Content-Length") or 0)
            if n <= 0 or n > MAX_BODY:
                return self._send(413, "body size")
            try:
                rec = json.loads(self.rfile.read(n).decode("utf-8"))
                p = H.write_human(human, rec)
            except (ValueError, UnicodeDecodeError) as e:
                return self._send(400, str(e))
            self._send(200, os.path.basename(p))

    return Handler


def serve(root, port=0):
    """A loopback-only server (returns it; the caller runs serve_forever). Port 0 lets the system pick; the bound socket is kept."""
    return http.server.HTTPServer(("127.0.0.1", port), make_handler(root))


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[1])
    ap.add_argument("--serve", metavar="OUT")
    ap.add_argument("--write", metavar="OUT")
    ap.add_argument("--port", type=int, default=0)
    a = ap.parse_args(argv)
    if a.write:
        print("\n".join(write_pages(a.write, os.path.join(a.write, "checklist"))))
        return 0
    if not a.serve:
        ap.error("give --serve OUT or --write OUT")
    write_pages(a.serve, os.path.join(a.serve, "checklist"))
    s = serve(a.serve, a.port)
    print("checklist at http://127.0.0.1:%d/ (Ctrl+C stops); answers go to %s" % (s.server_address[1], os.path.join(a.serve, "human")))
    try:
        s.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
