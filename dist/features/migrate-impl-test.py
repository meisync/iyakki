#!/usr/bin/env python3
"""Migrates FEATURES.md from `Progress` (NN%) to `Impl/Test` (NNN/NNN) plus a `Manual` column. Idempotent: a file that already has the
`Impl/Test` header is left alone. Uses only evidence in the row itself (Status, old Progress, Tests, Notes) and, as a guide for which
verification needs a person, docs/development/partials-triage.json. It never invents a pass: `passed DATE WHERE` is written only when a
sentence of the Notes reports a real run, with a date and a machine named in that same sentence.

Test levels: 0 nothing; 30 fakes/unit tests of parts, or the notes say not tried / unverified; 50 tests listed; 70 a `script:` (QEMU,
browser harness) is listed; 100 notes report a real-hardware run, or a done row with tests whose notes name no open verification.
Second pass (class B, see docs/development/IMPL-TEST-COLUMNS.md) runs on every call, so a migrated file is updated once and then stays put.
Usage: python3 dist/features/migrate-impl-test.py [FEATURES.md]
       python3 dist/features/migrate-impl-test.py --from-autotest DIR [--apply] [--image-commit SHA]   (Test from machine evidence; docs/development/AUTOTEST.md)
"""
import json, os, re, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
if "--from-autotest" in sys.argv:  # machine-made evidence raises Test by the rules of dist/autotest/evidence.py (dry run unless --apply)
    sys.path.insert(0, os.path.join(ROOT, "dist", "autotest"))
    import evidence
    i = sys.argv.index("--from-autotest")
    sys.exit(evidence.main(sys.argv[i + 1:], os.path.join(ROOT, "FEATURES.md"), ROOT))
path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "FEATURES.md")
text = open(path, encoding="utf-8").read()
migrated = "| Impl/Test |" in text

triage = {}
tp = os.path.join(ROOT, "docs/development/partials-triage.json")
if os.path.exists(tp):
    triage = {t["id"]: t for t in json.load(open(tp, encoding="utf-8"))}

NEG = re.compile(r"not (yet )?(been )?(tried|run|verified|seen|measured|tested|proven|checked)|untried|unverified|never (been )?(tried|run|seen|tested|measured)|"
                 r"(fakes?|fake \w+) only|only (with|on|against|over) (a |the )?(fakes?|fake)|not on (a )?(real|hardware|the mac|a device)|"
                 r"hardware (is )?(not|unverified)|awaits? (a |the )?(run|hardware|owner)|no real-(voice|hardware)", re.I)
POS = re.compile(r"(verified|confirmed|checked|passed|seen|works|worked|tried|tested|ran)\b", re.I)
HARD = re.compile(r"hardware|device|screen|speaker|headphone|\bear\b|heard|camera|touch|panel|display|Mac mini|MacBook|HP |Lenovo|owner|real ", re.I)
DATE = re.compile(r"20\d\d-\d\d-\d\d")
WHERE = [("macmini", re.compile(r"Mac mini", re.I)), ("mbp14", re.compile(r"MacBook ?Pro|MacBook|MBP", re.I)),
         ("hp", re.compile(r"\bHP\b")), ("lenovo", re.compile(r"Lenovo|Legion", re.I)), ("qemu", re.compile(r"QEMU"))]
NOTPOS = re.compile(r"\b(not|never|no|un\w+|without|but|until|once|when|if)\b", re.I)


def passed(notes):
    """(date, where) from one sentence that reports a real run, else None."""
    best = None
    for s in re.split(r"(?<=[.;])\s+", notes):
        d = DATE.findall(s)
        if not d or not POS.search(s) or NOTPOS.search(s):
            continue
        for w, rx in WHERE:
            if rx.search(s) and re.search(r"real|hardware|on the|by hand|owner|seen|screen", s, re.I):
                cand = (max(d), w)
                best = max(best, cand) if best else cand
    return best


def convert(cells):
    cells = cells + [""] * (8 - len(cells))
    fid, feat, phase, status, pri, prog, tests, notes = cells[:8]
    impl = int(prog.rstrip("%"))
    has_tests = tests not in ("", "—")
    has_script = "script:" in tests
    neg = bool(NEG.search(notes))
    t = triage.get(fid)
    p = passed(notes)
    if status == "planned":
        test, manual = 0, "none"
    else:
        if not has_tests:
            test = 0
        elif has_script:
            test = 70
        elif neg:
            test = 30
        else:
            test = 50
        manual = "none"
        if t:
            cls = t["class"]
            judge = t.get("macmini") and not t.get("human_only") and not t.get("human_only_why")
            if cls in ("D", "E") or (cls == "B" and not judge):
                manual = "needed"
        elif neg and HARD.search(notes) and status != "deprecated":
            manual = "needed"
        if p and not neg:
            test = 100
        elif p:
            test = max(test, 70)
        if p:
            manual = "passed %s %s" % p
        elif status == "done" and has_tests and not neg and manual == "none":
            test = 100
        elif status == "done" and neg:
            test = min(test, 30)
        if status == "deprecated" and not p:
            manual = "none"
        if manual == "needed" and test == 100:
            test = 70
        if status != "done" and test == 100 and not p:
            test = 70
    # a done row must be fully proven; otherwise it is partial (impl stays 100, so it counts as unproven)
    if status == "done" and (test < 100 or manual == "needed"):
        status = "partial"
    return [fid, feat, phase, status, pri, "%d/%d" % (impl, test), manual, tests, notes]


GAP = re.compile(r"not (yet )?(built|implemented|written|wired|added|supported|done|connected|finished)|isn't (built|wired|written)|\bmissing\b|\bno code\b|\blater\b|\bplanned\b|\bTODO\b|"
                 r"\bstubs?\b|only the first slice|unfinished|to be (built|written|added)|will be (added|built|written)|next slice|follow-up|"
                 r"cannot yet|does not (yet )?(exist|work|handle|support)|nothing (is )?(stopped|enforced)|left to do|\bremaining\b|not (yet )?available|\bno way to\b", re.I)


def second_pass(line):
    """Class B rows (built and unit-tested, never seen on hardware) whose Notes name no missing implementation get Impl 100; their Test and
    Manual stay as assigned. Rows naming a gap (GAP, or the triage flag also_code_gap) and all other classes are left as they are."""
    c = [x.strip() for x in line.strip().strip("|").split("|")]
    t = triage.get(c[0])
    if not t or t["class"] != "B" or c[3] != "partial" or "also_code_gap" in t.get("flags", []) or GAP.search(c[8]):
        return line
    i, tt = map(int, c[5].split("/"))
    if i == 100 or (tt == 100 and c[6] not in ("needed",) and not c[6].startswith("failed")):
        return line  # already 100, or it would be a finished row (done is a person's decision, not this script's)
    c[5] = "100/%d" % tt
    return "| " + " | ".join(c) + " |"


out = []
for line in text.split("\n"):
    if migrated:
        out.append(second_pass(line) if line.startswith("| F-") else line)
        continue
    if line.startswith("| ID | Feature |"):
        line = "| ID | Feature | Phase | Status | Pri | Impl/Test | Manual | Tests | Notes |"
    elif line.startswith("|---|---|---|---|---|---|---|---|"):
        line = "|---|---|---|---|---|---|---|---|---|"
    elif line.startswith("| F-"):
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        c = convert(cells)
        line = second_pass("| " + " | ".join(c) + " |")
    out.append(line)
text = "\n".join(out)


def totals(text):
    rows = [[c.strip() for c in l.strip().strip("|").split("|")] for l in text.split("\n") if l.startswith("| F-")]
    n = {s: 0 for s in ["done", "partial", "planned", "blocked", "deprecated"]}
    io = un = 0
    for r in rows:
        n[r[3]] += 1
        if r[3] == "partial":
            i, t = map(int, r[5].split("/"))
            if i < 100:
                io += 1
            elif t < 100 or r[6] in ("needed",) or r[6].startswith("failed"):
                un += 1
    return "done %d · partial %d (impl open %d · unproven %d) · planned %d · blocked %d · deprecated %d · total %d" % (
        n["done"], n["partial"], io, un, n["planned"], n["blocked"], n["deprecated"], len(rows))


text = re.sub(r"(<!-- summary:start -->).*?(<!-- summary:end -->)", lambda m: m.group(1) + totals(text) + m.group(2), text, flags=re.S)
open(path, "w", encoding="utf-8").write(text)
print(totals(text))
