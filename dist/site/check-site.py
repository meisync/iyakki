#!/usr/bin/env python3
"""Checks the generated site against the repository. Run after dist/site/build-site.py:

    python3 dist/site/check-site.py

1. site/progress.html counts (status, Impl/Test averages, Manual, per area) and the home page summary equal FEATURES.md (parsed again here, by a different method: the summary line and awk-style column reads).
2. Every page has a unique title, a meta description, a canonical URL on the base, Open Graph and Twitter tags; the home page has JSON-LD.
3. No script tag besides JSON-LD, no external resource (src/href to another site is only a link to the repository), alt text on every image.
4. The public-hygiene patterns of dist/release/export-public.sh (home paths, private keys, tokens) and e-mail addresses find nothing in site/.
5. site/ is not in dist/release/public-exclude.txt.
6. Every file the sitemap names exists.
"""
import os, re, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
SITE = os.path.join(ROOT, "site")
fails = []


def ok(cond, msg):
    if not cond:
        fails.append(msg)
        print("FAIL:", msg)


def rd(p):
    with open(p, encoding="utf-8") as f:
        return f.read()


# 1. counts
feat = rd(os.path.join(ROOT, "FEATURES.md"))
m = re.search(r"<!-- summary:start -->done (\d+) · partial (\d+) \(impl open (\d+) · unproven (\d+)\) · planned (\d+) · blocked (\d+) · deprecated (\d+) · total (\d+)<!-- summary:end -->", feat)
g = list(map(int, m.groups()))
summary = dict(zip(["done", "partial", "impl_open", "unproven", "planned", "blocked", "deprecated", "total"], g))
rows = [l for l in feat.splitlines() if l.startswith("| F-")]
by = {}
for l in rows:
    st = l.split("|")[4].strip()
    by[st] = by.get(st, 0) + 1
for s in ["done", "partial", "planned", "blocked", "deprecated"]:
    ok(by.get(s, 0) == summary[s], "FEATURES.md summary line disagrees with its rows for %s: %s vs %d (fix FEATURES.md)" % (s, summary[s], by.get(s, 0)))
ok(len(rows) == summary["total"], "FEATURES.md total %d vs %d rows" % (summary["total"], len(rows)))
prog = rd(os.path.join(SITE, "progress.html"))
for s in ["done", "partial", "planned", "blocked", "deprecated", "total"]:
    mm = re.search(r'<b data-status="%s">(\d+)</b>' % s, prog)
    want = len(rows) if s == "total" else by.get(s, 0)
    ok(mm and int(mm.group(1)) == want, "progress.html %s = %s, FEATURES.md = %d (run dist/site/build-site.py)" % (s, mm and mm.group(1), want))
now = re.search(r'<tr data-now="1">(.*?)</tr>', prog, re.S)
ok(bool(now) and [int(x) for x in re.findall(r'<td class="n">(\d+)</td>', now.group(1))] == [by.get(s, 0) for s in ["done", "partial", "planned", "blocked", "deprecated"]] + [len(rows)],
   "the Now row of the snapshot table differs from FEATURES.md")
ph = {}
for l in rows:
    c = [x.strip() for x in l.split("|")]
    ph.setdefault(c[3], [0] * 5)[["done", "partial", "planned", "blocked", "deprecated"].index(c[4])] += 1
for k, v in ph.items():
    mm = re.search(r'<tr data-phase="%s">(.*?)</tr>' % k, prog, re.S)
    got = [int(x) for x in re.findall(r'<td class="n">(\d+)</td>', mm.group(1))] if mm else None
    ok(got == v + [sum(v)], "phase %s row differs: page %s, FEATURES.md %s" % (k, got, v + [sum(v)]))

# 1b. Impl/Test and Manual numbers, read again here with plain column reads (not the build script's functions)
live = []
for l in rows:
    c = [x.strip() for x in l.split("|")]
    if c[4] == "deprecated":
        continue
    a, b = c[6].split("/")
    live.append((c[1].split("-")[1], int(a), int(b), c[7], c[4]))
n = len(live)
half = lambda xs: (2 * sum(xs) + n) // (2 * n) if xs is not None else 0
want = {"impl": half([r[1] for r in live]), "test": half([r[2] for r in live]), "full-impl": sum(r[1] == 100 for r in live), "full-test": sum(r[2] == 100 for r in live),
        "passed": sum(r[3].startswith("passed") for r in live), "needed": sum(r[3] == "needed" for r in live), "failed": sum(r[3].startswith("failed") for r in live)}
for k, v in want.items():
    mm = re.search(r'data-metric="%s">(\d+)' % k, prog)
    ok(mm and int(mm.group(1)) == v, "progress.html metric %s = %s, FEATURES.md = %d (run dist/site/build-site.py)" % (k, mm and mm.group(1), v))
part = [[x.strip() for x in l.split("|")] for l in rows if l.split("|")[4].strip() == "partial"]
ok(sum(1 for c in part if int(c[6].split("/")[0]) < 100) == summary["impl_open"], "FEATURES.md impl open count disagrees with its rows")
ok(sum(1 for c in part if int(c[6].split("/")[0]) == 100 and (int(c[6].split("/")[1]) < 100 or c[7] == "needed" or c[7].startswith("failed"))) == summary["unproven"], "FEATURES.md unproven count disagrees with its rows")
ok(summary["impl_open"] + summary["unproven"] <= summary["partial"], "impl open + unproven exceed partial")
areas = {}
for r in live:
    areas.setdefault(r[0], []).append(r)
for k, v in areas.items():
    mm = re.search(r'<tr data-area="%s">(.*?)</tr>' % k, prog, re.S)
    got = re.findall(r'<td class="n">(\d+)', mm.group(1)) if mm else None
    exp = [str(len(v)), str(sum(r[4] == "planned" for r in v)), str((2 * sum(r[1] for r in v) + len(v)) // (2 * len(v))), str((2 * sum(r[2] for r in v) + len(v)) // (2 * len(v))), str(sum(r[3] == "needed" for r in v)), str(sum(r[3].startswith("passed") for r in v))]
    ok(got == exp, "area %s row differs: page %s, FEATURES.md %s" % (k, got, exp))
ok(len(re.findall(r'<tr data-area=', prog)) == len(areas), "progress.html lists a different set of areas")
ok("<progress" not in prog and "<script" not in prog.replace('application/ld+json', ''), "progress page must stay free of script")
idx = rd(os.path.join(SITE, "index.html"))
ok(("on average %d%% is written and %d%% is proven" % (want["impl"], want["test"])) in idx, "the home page summary differs from FEATURES.md")

# 2/3. page hygiene
pages = ["index.html", "progress.html", "privacy.html", "404.html"]
base = re.search(r'^BASE = "(.*)"', rd(os.path.join(ROOT, "dist/site/build-site.py")), re.M).group(1)
titles = []
for p in pages:
    t = rd(os.path.join(SITE, p))
    tt = re.search(r"<title>(.*?)</title>", t).group(1)
    titles.append(tt)
    ok(re.search(r'<meta name="description" content="[^"]{40,}"', t), p + ": description")
    ok('<link rel="canonical" href="%s' % base in t, p + ": canonical on the base")
    for tag in ['og:title', 'og:description', 'og:image"', 'twitter:card', 'twitter:image"']:
        ok(tag in t, "%s: %s" % (p, tag))
    ok(re.search(r'<html lang="en">', t) and "prefers-color-scheme:dark" in t and 'name="viewport"' in t, p + ": lang, dark mode, viewport")
    scripts = re.findall(r"<script[^>]*>", t)
    ok(all('application/ld+json' in s for s in scripts), p + ": a script that is not JSON-LD")
    for img in re.findall(r"<img[^>]*>", t):
        ok(re.search(r'alt="[^"]+"', img), p + ": image without alt: " + img)
    for u in re.findall(r'(?:src|href)="(https?://[^"]+)"', t):
        ok(u.startswith(base) or u.startswith("https://github.com/meisync/iyakki"), p + ": link outside the project: " + u)
    ok("<link rel=\"stylesheet\"" not in t and "@import" not in t, p + ": external stylesheet")
ok(len(set(titles)) == len(titles), "titles are not unique")
ok('"@type": "SoftwareApplication"' in rd(os.path.join(SITE, "index.html")) and '"Apache' not in "" and "apache.org/licenses/LICENSE-2.0" in rd(os.path.join(SITE, "index.html")), "JSON-LD on the home page")

# 4. hygiene
pat = re.compile(r"/home/[a-z][a-z0-9_-]+/|BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY|sk-ant-[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9]{30,}|AKIA[0-9A-Z]{16}"
                 r"|[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[a-z]{2,}|\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b|\.local\b")
for d, _, fs in os.walk(SITE):
    for f in fs:
        if f.endswith((".png", ".ico")):
            continue
        t = rd(os.path.join(d, f))
        for mm in pat.finditer(t):
            ok(False, "hygiene pattern in %s: %s" % (os.path.relpath(os.path.join(d, f), ROOT), mm.group(0)))

# 5. exported
excl = [l.strip() for l in rd(os.path.join(ROOT, "dist/release/public-exclude.txt")).splitlines() if l.strip() and not l.startswith("#")]
ok(not any(e.startswith("site") or e.startswith("dist/site") or e.startswith(".github") for e in excl), "site/, dist/site or .github is in public-exclude.txt")

# 6. sitemap
for loc in re.findall(r"<loc>(.*?)</loc>", rd(os.path.join(SITE, "sitemap.xml"))):
    rel = loc[len(base):] or "index.html"
    ok(loc.startswith(base) and os.path.exists(os.path.join(SITE, rel)), "sitemap names a missing file: " + loc)
ok(os.path.exists(os.path.join(SITE, "img/social-card.png")) and os.path.exists(os.path.join(SITE, "robots.txt")), "social card or robots.txt missing")

if fails:
    print("%d check(s) failed" % len(fails))
    sys.exit(1)
print("site checks passed: %d rows, %s" % (len(rows), ", ".join("%s %d" % (k, by[k]) for k in sorted(by))))
