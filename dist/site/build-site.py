#!/usr/bin/env python3
"""Builds the static demo site in site/ (GitHub Pages). One command, offline; needs Python 3 with Pillow and numpy (the logo code needs them):

    python3 dist/site/build-site.py            # writes site/*.html, sitemap.xml, robots.txt, img/*.png

Facts come from the repository: FEATURES.md (counts are parsed, never typed), KNOWN-ISSUES.md, the newest RELEASE-NOTES-*.md,
docs/ALPHA-SCOPE.md and docs/BETA-SCOPE.md (two earlier snapshots), SECURITY.md. Nothing is fetched; the pages carry one small inline
stylesheet, no script, no font, no tracker. The social card is drawn with the repository's own logo code (dist/brand/draw-logo.py).
dist/site/check-site.py proves the page counts equal FEATURES.md.
"""
import datetime, html, importlib.util, json, os, re

# ---- the ONE place that names the public address (change here when the real Pages address is known) ----
BASE = "https://meisync.github.io/iyakki/"
REPO = "https://github.com/meisync/iyakki"

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
SITE = os.path.join(ROOT, "site")
STATUSES = ["done", "partial", "planned", "blocked", "deprecated"]


def read(p):
    with open(os.path.join(ROOT, p), encoding="utf-8") as f:
        return f.read()


def esc(s):
    return html.escape(s, quote=True)


# ---------------------------------------------------------------- facts
def parse_features():
    rows = []
    for line in read("FEATURES.md").splitlines():
        if not line.startswith("| F-"):
            continue
        c = [x.strip() for x in line.strip().strip("|").split("|")]
        fid, phase, status = c[0], c[2], c[3]
        if status not in STATUSES:
            raise SystemExit("unreadable status in row %s: %s" % (fid, status))
        m = re.fullmatch(r"(\d+)/(\d+)", c[5])
        if not m:
            raise SystemExit("unreadable Impl/Test in row %s: %s" % (fid, c[5]))
        rows.append((fid, phase, status, int(m.group(1)), int(m.group(2)), c[6]))
    return rows


def avg(vals):
    """Rounded half up, in integers (check-site.py does the same by its own code)."""
    return (2 * sum(vals) + len(vals)) // (2 * len(vals)) if vals else 0


def metrics(rows):
    live = [r for r in rows if r[2] != "deprecated"]
    return dict(live=len(live), impl=avg([r[3] for r in live]), test=avg([r[4] for r in live]),
                full_impl=sum(1 for r in live if r[3] == 100), full_test=sum(1 for r in live if r[4] == 100),
                passed=sum(1 for r in live if r[5].startswith("passed")), needed=sum(1 for r in live if r[5] == "needed"),
                failed=sum(1 for r in live if r[5].startswith("failed")))


def phase_names():
    m = re.search(r"\*Phase\*: (.*?)\. \*Status\*", read("FEATURES.md"))
    out = {}
    for part in m.group(1).split(" · "):
        k, v = part.split(" ", 1)
        out[k] = v
    return out


def snapshot(path, label):
    m = re.search(r"Derived from `FEATURES.md` on (\d{4}-\d\d-\d\d) \(done (\d+) · partial (\d+) · planned (\d+) · blocked (\d+) · deprecated (\d+) · total (\d+)\)", read(path))
    d, *n = m.groups()
    return {"label": label, "date": d, "counts": dict(zip(STATUSES, map(int, n[:5]))), "total": int(n[5])}


def latest_release():
    def key(f):
        return [int(x) for x in re.findall(r"\d+", f)]
    notes = sorted((f for f in os.listdir(ROOT) if re.match(r"RELEASE-NOTES-.*\.md$", f)), key=key)
    f = notes[-1]
    ver = re.match(r"RELEASE-NOTES-(.*)\.md", f).group(1)
    m = re.search(r"\*An OS for agents\.\* [^,\n]*, (\d{4}-\d\d-\d\d)", read(f))
    return ver, f, (m.group(1) if m else "")


def known_issues():
    lines = read("KNOWN-ISSUES.md").splitlines()
    ids = set(re.findall(r"KI-\d{3}", "\n".join(lines)))
    top = []
    start = next(i for i, l in enumerate(lines) if l.startswith("## Do not use it for"))
    end = next(i for i, l in enumerate(lines) if i > start and l.startswith("## "))
    for ln in lines[start:end]:
        m = re.match(r"\* (KI-\d{3}) (?:\*\*(.+?)\*\*|([^.]+)\.)", ln)
        if m:
            top.append((m.group(1), (m.group(2) or m.group(3)).rstrip(".")))
    return len(ids), top


# ---------------------------------------------------------------- page frame
CSS = """
:root{--bg:#fff;--fg:#1a1d23;--mut:#4a5260;--card:#f3f5f8;--line:#c9cfd9;--link:#0b5cad;--acc:#0b5cad;--onacc:#fff;
--done:#1b6e3c;--partial:#8a5a00;--planned:#2b4fa8;--blocked:#a3262a;--deprecated:#5b6270}
@media (prefers-color-scheme:dark){:root{--bg:#0f1217;--fg:#e6e9ef;--mut:#a9b2c0;--card:#171c24;--line:#303846;--link:#7fbcff;--acc:#7fbcff;--onacc:#0f1217;
--done:#5fd08a;--partial:#f0b94d;--planned:#8fb0ff;--blocked:#ff8a8d;--deprecated:#a9b2c0}}
*{box-sizing:border-box}
html{-webkit-text-size-adjust:100%}
body{margin:0;background:var(--bg);color:var(--fg);font:1rem/1.6 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}
a{color:var(--link);text-underline-offset:.15em}
a:focus-visible,summary:focus-visible{outline:3px solid var(--acc);outline-offset:2px;border-radius:3px}
.skip{position:absolute;left:-999px;top:0;background:var(--acc);color:var(--onacc);padding:.5rem 1rem;z-index:9}
.skip:focus{left:0}
header.top,main,footer{max-width:62rem;margin:0 auto;padding-left:16px;padding-right:16px}
header.top{display:flex;flex-wrap:wrap;gap:.5rem 1.5rem;align-items:center;justify-content:space-between;padding-top:.9rem;padding-bottom:.9rem;border-bottom:1px solid var(--line)}
.brand{display:flex;align-items:center;gap:.6rem;font-weight:700;color:var(--fg);text-decoration:none;letter-spacing:.04em}
.brand img{width:44px;height:auto}
nav ul{display:flex;flex-wrap:wrap;gap:.25rem 1.1rem;list-style:none;margin:0;padding:0}
nav a{display:inline-block;padding:.35rem .1rem;color:var(--fg)}
nav a[aria-current=page]{font-weight:700;border-bottom:2px solid var(--acc)}
h1{font-size:clamp(1.8rem,5vw,2.8rem);line-height:1.15;margin:1.6rem 0 .6rem}
h2{font-size:1.5rem;margin:2.4rem 0 .6rem;line-height:1.25}
h3{font-size:1.1rem;margin:1.2rem 0 .3rem}
.lead{font-size:1.15rem;color:var(--mut);max-width:44rem}
.note{background:var(--card);border:1px solid var(--line);border-left:5px solid var(--partial);padding:.8rem 1rem;border-radius:6px;margin:1.2rem 0}
.btns{display:flex;flex-wrap:wrap;gap:.7rem;margin:1.2rem 0}
.btn{display:inline-block;padding:.65rem 1.1rem;border-radius:6px;border:2px solid var(--acc);text-decoration:none;font-weight:600;color:var(--acc)}
.btn.primary{background:var(--acc);color:var(--onacc)}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(15rem,100%),1fr));gap:1rem;margin:1rem 0}
.card{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:.9rem 1rem}
.card h3{margin-top:0}
.card ul{margin:.3rem 0 0;padding-left:1.1rem}
.tag{font-size:.8rem;font-weight:700;border:1px solid currentColor;border-radius:4px;padding:0 .35rem;color:var(--partial);white-space:nowrap}
.tw{overflow-x:auto;margin:.8rem 0}
table{border-collapse:collapse;width:100%;min-width:30rem}
caption{text-align:left;color:var(--mut);padding:.3rem 0}
th,td{border-bottom:1px solid var(--line);padding:.45rem .6rem;text-align:left;vertical-align:top}
td.n,th.n{text-align:right;font-variant-numeric:tabular-nums}
th{background:var(--card)}
.bar{display:flex;height:1.4rem;border-radius:4px;overflow:hidden;border:1px solid var(--line);margin:.5rem 0 .3rem}
.bar span{display:block;height:100%}
.s-done{background:var(--done)}.s-partial{background:var(--partial)}.s-planned{background:var(--planned)}.s-blocked{background:var(--blocked)}.s-deprecated{background:var(--deprecated)}
.meter{margin:1rem 0}.meter .top{display:flex;justify-content:space-between;gap:1rem;font-weight:700}.meter .top span{font-variant-numeric:tabular-nums}
.track{height:1.5rem;border-radius:4px;border:1px solid var(--line);background:var(--card);overflow:hidden}
.fill{display:block;height:100%}.fill.fi{background:var(--planned)}.fill.ft{background:var(--done)}
.meter p{margin:.3rem 0 0;color:var(--mut);font-size:.95rem}
dl.legend{display:grid;grid-template-columns:max-content 1fr;gap:.3rem 1rem;margin:.6rem 0}dl.legend dt{font-weight:700}dl.legend dd{margin:0}
@media (max-width:520px){dl.legend{grid-template-columns:1fr}}
.key{display:flex;flex-wrap:wrap;gap:.3rem 1.2rem;list-style:none;padding:0;margin:.3rem 0 1rem}
.key i{display:inline-block;width:.9rem;height:.9rem;border-radius:2px;margin-right:.35rem;vertical-align:-.1rem}
.stats{display:grid;grid-template-columns:repeat(auto-fit,minmax(7rem,1fr));gap:.8rem;margin:1rem 0}
.stat{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:.6rem .9rem}
.stat b{display:block;font-size:1.8rem;line-height:1.2;font-variant-numeric:tabular-nums}
.shots{border:2px dashed var(--line);border-radius:8px;padding:1rem;color:var(--mut)}
details{margin:.8rem 0}summary{cursor:pointer;font-weight:600;padding:.3rem 0}
footer{margin-top:3rem;padding-top:1rem;padding-bottom:2rem;border-top:1px solid var(--line);color:var(--mut);font-size:.92rem}
code{background:var(--card);padding:.05rem .3rem;border-radius:3px;font-size:.92em;word-break:break-word}
@media (max-width:520px){h2{font-size:1.3rem}.brand img{width:36px}}
"""

NAV = [("index.html", "Home"), ("progress.html", "Progress"), ("privacy.html", "Privacy and security")]
ALT = "The IYAKKI emblem, three steel arrow heads with blue circuit light, and the words IYAKKI, An OS for Agents"


def frame(fname, title, desc, body, ver, absolute=False, jsonld=None, noindex=False):
    pre = BASE if absolute else ""
    url = BASE + ("" if fname == "index.html" else fname)
    nav = "".join('<li><a href="%s%s"%s>%s</a></li>' % (pre, f, ' aria-current="page"' if f == fname else "", esc(t)) for f, t in NAV)
    nav += '<li><a href="%s">Source on GitHub</a></li>' % REPO
    ld = '<script type="application/ld+json">%s</script>\n' % json.dumps(jsonld, indent=1) if jsonld else ""
    robots = '<meta name="robots" content="noindex">\n' if noindex else ""
    return """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>%(title)s</title>
<meta name="description" content="%(desc)s">
%(robots)s<link rel="canonical" href="%(url)s">
<link rel="icon" type="image/png" href="%(pre)simg/favicon.png">
<meta name="color-scheme" content="light dark">
<meta property="og:type" content="website">
<meta property="og:site_name" content="IYAKKI">
<meta property="og:title" content="%(title)s">
<meta property="og:description" content="%(desc)s">
<meta property="og:url" content="%(url)s">
<meta property="og:image" content="%(base)simg/social-card.png">
<meta property="og:image:alt" content="%(alt)s">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="%(title)s">
<meta name="twitter:description" content="%(desc)s">
<meta name="twitter:image" content="%(base)simg/social-card.png">
<meta name="twitter:image:alt" content="%(alt)s">
<style>%(css)s</style>
%(ld)s</head>
<body>
<a class="skip" href="#main">Skip to the content</a>
<header class="top">
<a class="brand" href="%(pre)sindex.html"><img src="%(pre)simg/logo.png" alt="IYAKKI emblem" width="44" height="25"><span>IYAKKI</span></a>
<nav aria-label="Main"><ul>%(nav)s</ul></nav>
</header>
<main id="main">
%(body)s
</main>
<footer>
<p>IYAKKI %(ver)s is an alpha preview for developers and testers, not for production. Licence: <a href="%(repo)s/blob/HEAD/LICENSE">Apache-2.0</a>.
This site is static: no script, no cookies, no tracking, nothing loaded from another site.</p>
<p><a href="%(repo)s">Source</a> · <a href="%(repo)s/releases">Releases</a> · <a href="%(repo)s/blob/HEAD/SECURITY.md">Security policy</a> · <a href="%(repo)s/blob/HEAD/SUPPORT.md">Support</a></p>
</footer>
</body>
</html>
""" % dict(title=esc(title), desc=esc(desc), robots=robots, url=url, pre=pre, base=BASE, css=CSS.strip(), ld=ld, nav=nav, body=body,
           ver=esc(ver), repo=REPO, alt=esc(ALT))


EXP = '<span class="tag">experimental</span>'


# ---------------------------------------------------------------- index
def page_index(ver, rel_file, rel_date, counts, total, mt):
    body = """
<h1>IYAKKI: an operating system built for AI agents</h1>
<p class="lead">A small, memory-safe operating system written in Rust and WebAssembly, built around an AI assistant that is allowed to <em>do</em> things:
safely, visibly and only with your permission.</p>
<div class="note"><strong>Alpha (%(ver)s%(date)s).</strong> A preview for developers and testers, <strong>not for production</strong>. The installer erases the whole drive you pick.
Many features are partial (%(partial)d of %(total)d rows in the feature matrix; on average %(impl)d%% is written and %(test)d%% is proven, see the progress page) and data formats are not promised to stay stable.
Read the <a href="%(repo)s/blob/HEAD/KNOWN-ISSUES.md">known issues</a> first.</div>
<div class="btns">
<a class="btn primary" href="%(repo)s/releases">Get the releases</a>
<a class="btn" href="%(repo)s/blob/HEAD/docs/INSTALL.md">Install guide</a>
<a class="btn" href="progress.html">See the progress</a>
</div>

<h2 id="what">What it is</h2>
<p>Today an AI assistant is usually a chat box inside someone else's operating system. It can talk, but anything it does (read a file, call an API, run a command)
is either impossible or happens with whatever power the program around it has. IYAKKI turns that around: the operating system is built for the agent.
The project's aim is to show that an assistant can be useful on a computer (plan your week, read your documents, run <code>kubectl</code> or <code>aws</code>,
try a Python program in a container, play music) without being trusted blindly.</p>

<h2 id="who">Who it is for</h2>
<ul>
<li><strong>Developers and testers</strong> who want to try an assistant that works under a permission model, on their own machine and with the models they choose
(Ollama on the network, or a cloud provider with their own key in an encrypted vault).</li>
<li><strong>People who care about auditability.</strong> The code is Apache-2.0 and the dependencies are permissive; the project keeps a tested boundary between its own code and the base operating system.</li>
<li>Not for production, and not for a machine holding data you cannot lose, while it is alpha.</li>
</ul>

<h2 id="why">Why it is different</h2>
<div class="grid">
<div class="card"><h3>One permission model</h3><p>Each person has a role (administrator, operator, user, guest) and a policy: which apps, folders, network hosts, tools and how many tokens.
The assistant can never do more than the person it works for.</p></div>
<div class="card"><h3>Risky things ask first</h3><p>Reading is free. Writing a file, calling an API, running a program or starting a container pauses for
Allow once, Allow for this session, or Deny. No answer means deny. Every run is recorded in a tamper-evident ledger.</p></div>
<div class="card"><h3>Untrusted code is sandboxed</h3><p>Plugins, tools and model-generated code run in WebAssembly (wasmtime) with a fuel budget, a memory cap and capability tokens,
or in a limited container.</p></div>
<div class="card"><h3>Yours to run</h3><p>It talks only to the models you connect. Nothing is sent anywhere you did not configure.
One program boots from a USB stick, runs in a virtual machine, as a desktop window, or as a container.</p></div>
<div class="card"><h3>Built in-house, permissively licensed</h3><p>The editor, browser engine, archive unpacking, Markdown and more are written by the project,
so the whole stays Apache-2.0 and small enough to audit.</p></div>
</div>

<h2 id="features">Key features, by group</h2>
<p>After signing in you get a launcher of tiles, sorted into groups. Each tile is an app, locked or unlocked by your role.
Status per feature is on the <a href="progress.html">progress page</a>; this is the overview from the README.</p>
<div class="grid">
<div class="card"><h3>Ask and create</h3><ul><li>CHAT with an agent mode: tools, each risky call asks first</li><li>RAG: search your documents with citations</li><li>SKILLS, CALENDAR, TALES (story teller), VOICE, LAB</li></ul></div>
<div class="card"><h3>Build</h3><ul><li>AI IDE: editor, terminal, feature board, Kubernetes view, approved Deploy flow</li><li>GIT, FLOWS (workflows), K8S, EDIT, CLI, MCP, RUNS (history of every agent run)</li></ul></div>
<div class="card"><h3>Media</h3><ul><li>BROWSER: in-house engine, plus an optional full WebKit engine</li><li>RADIO, MEDIA, CAMERA</li></ul></div>
<div class="card"><h3>Data</h3><ul><li>DATA: open large CSV/TSV/JSON-lines tables</li><li>DASH: read-only dashboards over Prometheus, Elasticsearch/OpenSearch, ClickHouse and DuckDB</li><li>USAGE: token use per model</li></ul></div>
<div class="card"><h3>Devices and system</h3><ul><li>SYSTEM, DISKS, NET SCAN, MESH, USERS (roles, limits, audit log), STORE</li><li>Bluetooth written in-house; encrypted home folders; lock screen</li></ul></div>
<div class="card"><h3>Where it runs</h3><ul><li>Desktop window on Linux</li><li>USB stick on a PC or Mac</li><li>QEMU virtual machine</li><li>Browser front end, Docker, K3s</li></ul></div>
</div>
<p>Marked experimental or alpha in the project's own documents: the wake phrase of the My IYAKKI voice session (%(exp)s, off by default), the Connect a client feature and its window program
(%(exp)s), and Linux programs in a sandbox (alpha, untried on a device).</p>

<h2 id="hardware">What has been tried on real hardware</h2>
<p>From the <a href="%(repo)s/blob/HEAD/%(rel)s">release notes</a>: the alpha.2 USB image booted on four real computers (a Mac mini 2018, a MacBook Pro 14,1, an HP laptop and a Lenovo laptop),
and most of what was tried on real hardware was tried on the Mac mini. Bluetooth pairing, phone sound and tether were tried on the MacBook Pro 14,1. Everything else in the changelog is checked by tests and fakes only;
the known issues say what is unproven. Secure Boot is verified in QEMU only on most machines.</p>

<h2 id="try">How to try it</h2>
<ol>
<li>Download the latest alpha from the <a href="%(repo)s/releases">GitHub releases</a> and verify the checksums and signatures that come with it.</li>
<li>Follow the <a href="%(repo)s/blob/HEAD/docs/INSTALL.md">install guide</a>: a desktop window on Linux (<code>./run.sh window</code>, needs the Rust toolchain), a USB stick, or a virtual machine.</li>
<li>Then the <a href="%(repo)s/blob/HEAD/docs/QUICKSTART.md">quickstart</a>. Chat needs a model: Settings, then Models.</li>
</ol>
<p><strong>The USB installer erases the drive you choose.</strong> Do not use it on a machine that holds anything you cannot lose.</p>

<h2 id="screens">Screenshots</h2>
<div class="shots" role="note"><p>Screenshots are added here once they are taken from a real image. This first version of the page has none.</p></div>
<!-- Screenshot slots (see site/img/README): launcher, chat-approval, ide, boot-menu. None is used yet. -->

<h2 id="more">More</h2>
<ul>
<li><a href="%(repo)s/blob/HEAD/docs/ARCHITECTURE.md">Architecture</a> · <a href="%(repo)s/blob/HEAD/docs/INDEX.md">All documentation</a> · <a href="%(repo)s/blob/HEAD/CHANGELOG.md">Changelog</a></li>
<li><a href="progress.html">Progress and roadmap</a> · <a href="privacy.html">Privacy and security</a> · <a href="%(repo)s/blob/HEAD/CONTRIBUTING.md">Contributing</a></li>
</ul>
""" % dict(ver=esc(ver), date=(", " + rel_date) if rel_date else "", partial=counts["partial"], total=total, impl=mt["impl"], test=mt["test"], repo=REPO, rel=rel_file, exp=EXP)
    desc = "IYAKKI is an AI-first desktop operating system written in Rust and WebAssembly: an assistant that can act on your computer under a permission model, with approvals and a tamper-evident ledger. Apache-2.0 alpha."
    ld = {
        "@context": "https://schema.org",
        "@type": "SoftwareApplication",
        "name": "IYAKKI",
        "alternateName": "IYAKKI, An OS for Agents",
        "description": "An operating system for AI agents, written in Rust and WebAssembly. Alpha preview for developers and testers.",
        "applicationCategory": "DeveloperApplication",
        "operatingSystem": "Linux (x86_64); USB boot image; QEMU virtual machine",
        "softwareVersion": ver,
        "license": "https://www.apache.org/licenses/LICENSE-2.0",
        "url": BASE,
        "codeRepository": REPO,
        "downloadUrl": REPO + "/releases",
        "image": BASE + "img/social-card.png",
        "isAccessibleForFree": True,
        "offers": {"@type": "Offer", "price": "0", "priceCurrency": "USD"},
    }
    return frame("index.html", "IYAKKI: an operating system built for AI agents", desc, body, ver, jsonld=ld)


# ---------------------------------------------------------------- progress
def bar(counts, total):
    spans = "".join('<span class="s-%s" style="width:%.3f%%"></span>' % (s, 100.0 * counts[s] / total) for s in STATUSES if counts[s])
    return '<div class="bar" role="img" aria-label="%s">%s</div>' % (esc(", ".join("%s %d" % (s, counts[s]) for s in STATUSES)), spans)


def status_row(label, counts, total, attr=""):
    cells = "".join('<td class="n">%d</td>' % counts[s] for s in STATUSES)
    return '<tr%s><th scope="row">%s</th>%s<td class="n">%d</td></tr>' % (attr, label, cells, total)


def page_progress(ver, rows, rel_file):
    total = len(rows)
    counts = {s: sum(1 for r in rows if r[2] == s) for s in STATUSES}
    names = phase_names()
    by_phase, by_area = {}, {}
    for fid, ph, st, *_ in rows:
        by_phase.setdefault(ph, {s: 0 for s in STATUSES})[st] += 1
        by_area.setdefault(fid.split("-")[1], {s: 0 for s in STATUSES})[st] += 1
    snaps = [snapshot("docs/ALPHA-SCOPE.md", "Derived for alpha.1"), snapshot("docs/BETA-SCOPE.md", "Beta scope")]
    th = "".join('<th scope="col" class="n">%s</th>' % s for s in STATUSES)
    thead = '<tr><th scope="col">%%s</th>%s<th scope="col" class="n">total</th></tr>' % th
    stats = "".join('<div class="stat"><b data-status="%s">%d</b>%s</div>' % (s, counts[s], s) for s in STATUSES) + '<div class="stat"><b data-status="total">%d</b>total</div>' % total
    key = "".join('<li><i class="s-%s"></i>%s</li>' % (s, s) for s in STATUSES)
    phase_rows = "".join(status_row("%s %s" % (esc(k), esc(names.get(k, ""))), by_phase[k], sum(by_phase[k].values()), ' data-phase="%s"' % esc(k))
                         for k in sorted(by_phase, key=lambda k: int(k[1:])))
    area_rows = "".join(status_row("<code>%s</code>" % esc(k), v, sum(v.values())) for k, v in sorted(by_area.items()))
    snap_rows = "".join(status_row("%s, %s" % (esc(s["label"]), s["date"]), s["counts"], s["total"]) for s in snaps)
    snap_rows += status_row("<strong>Now</strong> (FEATURES.md)", counts, total, ' data-now="1"')
    mt = metrics(rows)
    areas = {}
    for r in rows:
        if r[2] != "deprecated":
            areas.setdefault(r[0].split("-")[1], []).append(r)
    area_impl_rows = "".join(
        '<tr data-area="%s"><th scope="row"><code>%s</code></th><td class="n">%d</td><td class="n">%d%%</td><td class="n">%d%%</td><td class="n">%d</td><td class="n">%d</td></tr>' % (
            esc(k), esc(k), len(v), avg([r[3] for r in v]), avg([r[4] for r in v]), sum(1 for r in v if r[5] == "needed"), sum(1 for r in v if r[5].startswith("passed")))
        for k, v in sorted(areas.items()))
    n_ki, top = known_issues()
    ki = "".join("<li><strong>%s</strong> %s.</li>" % (esc(a), esc(b)) for a, b in top)
    body = """
<h1>Progress</h1>
<p class="lead">Generated from the project's feature matrix, <a href="%(repo)s/blob/HEAD/FEATURES.md"><code>FEATURES.md</code></a>: one row per feature, each with a status and the tests that prove it.
The numbers below are read from that file by <code>dist/site/build-site.py</code>; they are not typed by hand.</p>
<div class="note">The matrix has no per-release column, so there is no honest count per alpha for each row. The section on earlier snapshots uses the totals the project's own scope documents quote on the date they were written.
Most rows are <code>partial</code>: some of it works and the notes in the matrix say what is missing. The numbers come from the columns Impl/Test and Manual of the project's working feature matrix as of %(asof)s; the <code>FEATURES.md</code> in this repository catches up with each release. Nothing here is typed by hand.</div>

<h2 id="overall">How far along it is</h2>
<p>Two separate questions for every feature: how much of it is <em>written</em>, and how much of it is <em>proven to work</em>. Averages over the %(live)d features that are not deprecated.</p>
<div class="meter"><div class="top"><span>Implementation</span><span data-metric="impl">%(impl)d%%</span></div>
<div class="track" role="img" aria-label="Implementation, %(impl)d percent"><span class="fill fi" style="width:%(impl)d%%"></span></div>
<p><b data-metric="full-impl">%(full_impl)d</b> of %(live)d features are fully written.</p></div>
<div class="meter"><div class="top"><span>Tested</span><span data-metric="test">%(test)d%%</span></div>
<div class="track" role="img" aria-label="Tested, %(test)d percent"><span class="fill ft" style="width:%(test)d%%"></span></div>
<p><b data-metric="full-test">%(full_test)d</b> of %(live)d features are fully proven.</p></div>
<div class="stats">
<div class="stat"><b data-metric="passed">%(passed)d</b>seen on real hardware (manual test passed)</div>
<div class="stat"><b data-metric="needed">%(needed)d</b>waiting for a manual test</div>
<div class="stat"><b data-metric="failed">%(failed)d</b>manual test failed</div>
</div>

<h2 id="legend">What the numbers mean</h2>
<dl class="legend">
<dt>Impl</dt><dd>How much of the feature's behaviour is written, 0 to 100.</dd>
<dt>Test 0</dt><dd>Nothing is tested yet.</dd>
<dt>Test 30</dt><dd>Only parts are tested, with fakes or unit tests; or it has not been tried for real.</dd>
<dt>Test 50</dt><dd>The behaviour is tested end to end, still on fakes.</dd>
<dt>Test 70</dt><dd>Proven by an integration test, a virtual-machine boot, the browser harness or a script.</dd>
<dt>Test 100</dt><dd>Also seen working on the real hardware or screen it is for, or a manual test is recorded as passed. Features that need no hardware reach 100 by tests alone.</dd>
<dt>Manual none</dt><dd>No manual test is needed.</dd>
<dt>Manual needed</dt><dd>A person, or the test Mac mini or stick, must try it.</dd>
<dt>Manual passed</dt><dd>Tried by hand with the date and the machine recorded.</dd>
<dt>Manual failed</dt><dd>Tried and failed; a known issue (KI-number) says what.</dd>
</dl>

<h2 id="areas">By feature area</h2>
<div class="tw"><table><caption>Averages over the features of each area that are not deprecated. Area codes are the middle part of an ID such as F-CORE-01.</caption>
<thead><tr><th scope="col">Area</th><th scope="col" class="n">Features</th><th scope="col" class="n">Impl</th><th scope="col" class="n">Test</th><th scope="col" class="n">Manual needed</th><th scope="col" class="n">Manual passed</th></tr></thead>
<tbody>%(area_impl_rows)s</tbody></table></div>

<h2 id="now">Status counts now (%(ver)s)</h2>
<p>A feature is <code>done</code> only when it is fully written, fully proven and needs no open manual test; everything else with some work done is <code>partial</code>.</p>
<div class="stats">%(stats)s</div>
%(bar)s
<ul class="key" aria-label="Colour key">%(key)s</ul>

<h2 id="snapshots">Earlier snapshots</h2>
<div class="tw"><table><caption>Totals quoted in docs/ALPHA-SCOPE.md and docs/BETA-SCOPE.md, and now. The matrix grows as features are added.</caption>
<thead>%(thead_s)s</thead><tbody>%(snap_rows)s</tbody></table></div>

<h2 id="phases">Feature groups by phase</h2>
<div class="tw"><table><caption>Rows per phase of the matrix and their status.</caption>
<thead>%(thead_p)s</thead><tbody>%(phase_rows)s</tbody></table></div>
<details><summary>All %(n_areas)d feature areas</summary>
<div class="tw"><table><caption>Rows per area code (the middle part of an ID such as F-CORE-01).</caption>
<thead>%(thead_a)s</thead><tbody>%(area_rows)s</tbody></table></div></details>

<h2 id="issues">Known issues in short</h2>
<p>The honest list is <a href="%(repo)s/blob/HEAD/KNOWN-ISSUES.md">KNOWN-ISSUES.md</a>; it names %(n_ki)d numbered issues (KI-numbers), grouped by hardware, browser and media, assistant, security, install and tests.
Its first section, "Do not use it for", says:</p>
<ul>%(ki)s</ul>
<p>From the <a href="%(repo)s/blob/HEAD/%(rel)s">alpha.2 release notes</a>, known limits include: the wake phrase has no real-voice measurement and should stay off;
the encrypted home folder is a first version (whole-disk encryption and TPM sealing are not built); Secure Boot is verified in QEMU only on most machines.</p>

<h2 id="roadmap">Roadmap</h2>
<p>Taken only from documents that are public in the repository. Plans can change.</p>
<h3>alpha.3 (in progress)</h3>
<ul>
<li>Browser sound as a low-latency live source of the mixer, to fix the lag and the tail after stopping (KI-085; tested with a fake sink, not on hardware).</li>
<li>Driver and firmware downloads fail within seconds with a plain message when there is no network (KI-083; tested, not yet on hardware).</li>
<li>A covered camera is reported as such instead of a bare black picture (KI-082), and the Bluetooth headline stops saying a firmware file is missing when the controller works (KI-084).</li>
<li>Client devices (%(exp)s, off unless switched on): a browser page and the <code>iyakki-connect</code> window program, with sound, clipboard, camera frames and the client's own camera and microphone as sources.
Tried on Linux with headless Chrome and fakes only (KI-088, KI-089).</li>
<li>Bundles with <code>requires</code> ranges and wizards in tile packages (data and checks only), and a developer-mode standing restart grant.</li>
</ul>
<h3>alpha.4 (plan only, nothing built)</h3>
<ul>
<li>Support for multiple languages: a framework so that a person or community can build a language as an installable pack (screen text, formats, fonts, keyboard and voice), with tools to make and test it.
Not machine translation; English complete, a pseudo-locale and one small sample pack. See <a href="%(repo)s/blob/HEAD/docs/development/I18N-PLAN.md">the plan</a>.</li>
</ul>
<h3>Before 1.0</h3>
<p>The v1.0.0 gate and the open decisions are in <a href="%(repo)s/blob/HEAD/docs/project/BACKLOG.md">the backlog</a> and <a href="%(repo)s/blob/HEAD/docs/BETA-SCOPE.md">the beta scope</a>.</p>
""" % dict(asof=datetime.date.today().isoformat(), repo=REPO, ver=esc(ver), area_impl_rows=area_impl_rows, impl=mt["impl"], test=mt["test"], live=mt["live"], full_impl=mt["full_impl"],
           full_test=mt["full_test"], passed=mt["passed"], needed=mt["needed"], failed=mt["failed"], stats=stats, bar=bar(counts, total), key=key, thead_s=thead % "Snapshot", snap_rows=snap_rows,
           thead_p=thead % "Phase", phase_rows=phase_rows, thead_a=thead % "Area", area_rows=area_rows, n_areas=len(by_area), n_ki=n_ki, ki=ki,
           rel=rel_file, exp=EXP)
    desc = "Progress of IYAKKI, the AI-first operating system: feature counts by status generated from the project's feature matrix (%d rows), known issues in short, and the alpha.3 and alpha.4 roadmap." % total
    return frame("progress.html", "IYAKKI progress: feature status and roadmap", desc, body, ver)


# ---------------------------------------------------------------- privacy / 404
def page_privacy(ver):
    body = """
<h1>Privacy and security</h1>
<p class="lead">This site does not track you.</p>
<h2 id="privacy">Privacy of this site</h2>
<ul>
<li>No analytics, no cookies, no scripts, no web fonts, no embedded content, nothing loaded from another site. Every page is plain HTML with one small stylesheet inside it.</li>
<li>The pages are served by GitHub Pages. GitHub, like any web host, sees the requests it serves; its own policies apply to that and this project does not receive that data.</li>
<li>Links to the repository, the releases and the documents leave this site and go to GitHub.</li>
</ul>
<h2 id="software">Privacy of the software</h2>
<p>IYAKKI talks only to the models and services you connect, and secrets live in encrypted vaults. The assistant is told that a secret exists, never its value.
Details: <a href="%(repo)s/blob/HEAD/SECURITY.md">SECURITY.md</a> and <a href="%(repo)s/blob/HEAD/BOUNDARY.md">BOUNDARY.md</a>.</p>
<h2 id="security">Security</h2>
<ul>
<li><strong>Report a vulnerability</strong> privately, with GitHub's private vulnerability reporting on the repository (Security, then Report a vulnerability). Please do not open a public issue for a security problem.
The policy promises an acknowledgement within 7 days. Read <a href="%(repo)s/blob/HEAD/SECURITY.md">SECURITY.md</a> for what to include.</li>
<li>Agents are treated as untrusted: roles, scopes, budgets and approvals; guests run in a WebAssembly sandbox; network access goes through an allow-listed egress proxy.</li>
<li>Releases are signed (<code>SHA256SUMS</code>, <code>release.json</code> and their signatures); verify before you write an image.</li>
</ul>
<div class="note"><strong>What the alpha does not promise.</strong> No security review by anyone outside the project, no penetration test, no Secure Boot on most real machines, no encryption of the whole data volume
(vaults and, where switched on, per-person home folders are encrypted), and formats may change between alphas. The full list is in <a href="%(repo)s/blob/HEAD/SECURITY.md">SECURITY.md</a>.</div>
""" % dict(repo=REPO)
    desc = "Privacy and security of IYAKKI: this site has no tracking, no cookies and no third-party content, and the security policy says how to report a vulnerability privately."
    return frame("privacy.html", "IYAKKI privacy and security", desc, body, ver)


def page_404(ver):
    body = """
<h1>Page not found</h1>
<p class="lead">There is no page at this address.</p>
<ul>
<li><a href="%(b)sindex.html">Home</a></li>
<li><a href="%(b)sprogress.html">Progress</a></li>
<li><a href="%(b)sprivacy.html">Privacy and security</a></li>
<li><a href="%(r)s">The repository</a></li>
</ul>
""" % dict(b=BASE, r=REPO)
    return frame("404.html", "Page not found: IYAKKI", "This page does not exist. Go to the IYAKKI home page, the progress page or the repository.", body, ver, absolute=True, noindex=True)


# ---------------------------------------------------------------- images
def draw_images():
    from PIL import Image, ImageDraw, ImageFont
    spec = importlib.util.spec_from_file_location("draw_logo", os.path.join(ROOT, "dist", "brand", "draw-logo.py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    emblem = m.draw_emblem()                       # transparent
    os.makedirs(os.path.join(SITE, "img"), exist_ok=True)
    emblem.resize((176, 98), Image.LANCZOS).save(os.path.join(SITE, "img", "logo.png"), optimize=True)
    fav = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
    e = emblem.resize((64, 36), Image.LANCZOS)
    fav.paste(e, (0, 14), e)
    fav.save(os.path.join(SITE, "img", "favicon.png"), optimize=True)
    card = Image.new("RGBA", (1200, 630), (30, 32, 36, 255))
    d = ImageDraw.Draw(card)
    em = emblem.resize((520, 290), Image.LANCZOS)
    card.alpha_composite(em, (50, 170))
    fdir = os.path.join(ROOT, "crates", "iyakki-shell", "assets")
    bold = os.path.join(fdir, "DejaVuSans-Bold.ttf")
    reg = os.path.join(fdir, "DejaVuSans.ttf")
    card = card.convert("RGB")
    d = ImageDraw.Draw(card)
    d.text((620, 175), "IYAKKI", font=ImageFont.truetype(bold, 92), fill=(236, 240, 246))
    d.text((622, 290), "An OS for Agents", font=ImageFont.truetype(bold, 40), fill=(127, 188, 255))
    s = ImageFont.truetype(reg, 28)
    d.text((622, 360), "Rust and WebAssembly.", font=s, fill=(190, 198, 210))
    d.text((622, 400), "Permissions, approvals, a ledger.", font=s, fill=(190, 198, 210))
    d.text((622, 480), "Apache-2.0 alpha preview", font=s, fill=(150, 160, 175))
    card.save(os.path.join(SITE, "img", "social-card.png"), optimize=True)


def write(name, text):
    p = os.path.join(SITE, name)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w", encoding="utf-8") as f:
        f.write(text)


def main():
    rows = parse_features()
    ver, rel_file, rel_date = latest_release()
    counts = {s: sum(1 for r in rows if r[2] == s) for s in STATUSES}
    draw_images()
    write("index.html", page_index(ver, rel_file, rel_date, counts, len(rows), metrics(rows)))
    write("progress.html", page_progress(ver, rows, rel_file))
    write("privacy.html", page_privacy(ver))
    write("404.html", page_404(ver))
    write("sitemap.xml", '<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
          + "".join("  <url><loc>%s%s</loc></url>\n" % (BASE, p) for p in ["", "progress.html", "privacy.html"]) + "</urlset>\n")
    write("robots.txt", "User-agent: *\nAllow: /\n\nSitemap: %ssitemap.xml\n" % BASE)
    write(".nojekyll", "")
    # Google Search Console ownership file (HTML-file method); remove this line and the file once the site is verified and the owner says so
    write("google30a427f5c6eda0d9.html", "google-site-verification: google30a427f5c6eda0d9.html")
    write("img/README", """Screenshot slots (none is used yet; screenshots are added last, from a real image).

Name a file exactly like this, 1280 px wide, PNG, and give it alt text where site/index.html (section Screenshots) uses it:

  shot-launcher.png        the launcher with its tiles and group chips
  shot-chat-approval.png   a chat tool call waiting for Allow once / Allow for this session / Deny
  shot-ide.png             the AI IDE workspace
  shot-boot-menu.png       the boot menu of the USB image

Generated by dist/site/build-site.py: logo.png, favicon.png, social-card.png (drawn from dist/brand/draw-logo.py). Do not edit those by hand.
""")
    print("wrote site/ (%d feature rows: %s)" % (len(rows), ", ".join("%s %d" % (s, counts[s]) for s in STATUSES)))


if __name__ == "__main__":
    main()
