"""The shared contract of the human manual-test checklist (docs/development/AUTOTEST.md, section "Human checklist contract").

Two JSON shapes, both versioned (`schema: 1`):
  * a checklist file, dist/autotest/checklists/<ROW>.json: what a person is asked to do for one row (steps, expected result, clause ids, screenshots,
    machine kinds);
  * a human evidence file, human/<ROW>.<clause>.<machine>.json: the person's verdict. The in-OS Doctor tab and the offline HTML page both write exactly this.
Pure code, standard library only. The Rust side (Doctor tab) validates with the same rules; the field lists below are the contract.
"""
import json
import os
import re

SCHEMA = 1
VERDICTS = ("pass", "fail", "cannot-test")
ROW_RE = re.compile(r"^F-[A-Z0-9]+-\d+$")
ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_-]{0,47}$")          # a clause id and a machine id
MACHINE_KINDS = ("any", "qemu", "mac-mini-2014", "mac-mini-2018", "macbook-pro", "lenovo", "hp", "t7-ssd", "other")
MAX_TEXT = 2000


def _txt(x, what, bad, required=True):
    if not isinstance(x, str) or (required and not x.strip()):
        bad.append("%s must be a non-empty text" % what)
    elif len(x) > MAX_TEXT:
        bad.append("%s is longer than %d" % (what, MAX_TEXT))


def _rel(p, what, bad):
    if not isinstance(p, str) or not p or os.path.isabs(p) or ".." in p.split("/") or not re.match(r"^[A-Za-z0-9._/-]+$", p):
        bad.append("%s must be a relative path of plain characters, not %r" % (what, p))


def validate_checklist(c):
    """A list of problems with a checklist file (empty when it is well formed)."""
    bad = []
    if not isinstance(c, dict):
        return ["not an object"]
    if c.get("schema") != SCHEMA:
        bad.append("schema must be %d" % SCHEMA)
    if not ROW_RE.match(str(c.get("row", ""))):
        bad.append("row is not a row id")
    if not re.match(r"^[0-9a-f]{16}$", str(c.get("feature_sha", ""))):
        bad.append("feature_sha must be the 16 hex digits of the Feature text hash")
    _txt(c.get("title"), "title", bad)
    clauses = c.get("clauses")
    if not isinstance(clauses, list) or not clauses:
        return bad + ["clauses must be a non-empty list"]
    seen = set()
    for i, cl in enumerate(clauses):
        w = "clauses[%d]" % i
        if not isinstance(cl, dict):
            bad.append(w + " is not an object")
            continue
        cid = cl.get("id", "")
        if not ID_RE.match(str(cid)):
            bad.append("%s.id is not a plain id" % w)
        elif cid in seen:
            bad.append("%s.id %r is used twice" % (w, cid))
        seen.add(cid)
        _txt(cl.get("text"), w + ".text", bad)
        ms = cl.get("machines")
        if not isinstance(ms, list) or not ms or any(m not in MACHINE_KINDS for m in ms):
            bad.append("%s.machines must be a non-empty list of %s" % (w, ", ".join(MACHINE_KINDS)))
        steps = cl.get("steps")
        if not isinstance(steps, list) or not steps:
            bad.append(w + ".steps must be a non-empty list")
            continue
        for j, s in enumerate(steps):
            sw = "%s.steps[%d]" % (w, j)
            if not isinstance(s, dict):
                bad.append(sw + " is not an object")
                continue
            _txt(s.get("do"), sw + ".do", bad)
            _txt(s.get("expect"), sw + ".expect", bad)
            if "screenshot" in s:
                _rel(s["screenshot"], sw + ".screenshot", bad)
            if "runner_did" in s:
                _txt(s["runner_did"], sw + ".runner_did", bad, required=False)
    return bad


def validate_human(h):
    """A list of problems with a human evidence file."""
    bad = []
    if not isinstance(h, dict):
        return ["not an object"]
    if h.get("schema") != SCHEMA:
        bad.append("schema must be %d" % SCHEMA)
    if not ROW_RE.match(str(h.get("row", ""))):
        bad.append("row is not a row id")
    if not ID_RE.match(str(h.get("clause", ""))):
        bad.append("clause is not a plain id")
    if not ID_RE.match(str(h.get("machine", ""))):
        bad.append("machine is not a plain machine id")
    if not re.match(r"^[0-9a-f]{7,40}$", str(h.get("image_commit", ""))):
        bad.append("image_commit must be the commit of the image under test (7 to 40 hex digits)")
    _txt(h.get("by"), "by", bad)
    if h.get("verdict") not in VERDICTS:
        bad.append("verdict must be one of %s" % ", ".join(VERDICTS))
    if not re.match(r"^\d{4}-\d\d-\d\d(T\d\d:\d\d:\d\d)?$", str(h.get("date", ""))):
        bad.append("date must be YYYY-MM-DD or YYYY-MM-DDTHH:MM:SS")
    _txt(h.get("note", ""), "note", bad, required=False)
    if "photo" in h:
        _rel(h["photo"], "photo", bad)
    extra = set(h) - {"schema", "row", "clause", "machine", "image_commit", "by", "verdict", "date", "note", "photo"}
    if extra:
        bad.append("unknown field(s): %s" % ", ".join(sorted(extra)))
    return bad


def human_filename(row, clause, machine):
    return "%s.%s.%s.json" % (row, clause, machine)


def write_human(human_dir, h):
    """Validate and write human/<ROW>.<clause>.<machine>.json; refuses a bad record. Returns the path."""
    bad = validate_human(h)
    if bad:
        raise ValueError("; ".join(bad))
    os.makedirs(human_dir, exist_ok=True)
    p = os.path.join(human_dir, human_filename(h["row"], h["clause"], h["machine"]))
    with open(p, "w", encoding="utf-8") as fh:
        json.dump(h, fh, indent=1, sort_keys=True)
        fh.write("\n")
    return p


def load_checklists(folder):
    """[(path, checklist, problems)] for every *.json of the folder."""
    out = []
    if os.path.isdir(folder):
        for fn in sorted(os.listdir(folder)):
            if fn.endswith(".json"):
                p = os.path.join(folder, fn)
                try:
                    with open(p, encoding="utf-8") as fh:
                        c = json.load(fh)
                    out.append((p, c, validate_checklist(c)))
                except (OSError, ValueError) as e:
                    out.append((p, None, ["unreadable: %s" % e]))
    return out
