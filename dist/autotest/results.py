"""The result record of one scenario for one row on one target (docs/development/AUTOTEST.md).

Pure code, standard library only. The runner writes these, the evidence enforcer (evidence.py) and the checklist read them.
"""
import fcntl
import hashlib
import json
import os
import re
import time

STATUSES = ("PASS", "FAIL", "SKIP", "BLOCKED", "NOT-APPLICABLE", "NEEDS-HUMAN", "NEW-GOLDEN")
TARGETS = ("qemu", "browser", "real")
ROW_RE = re.compile(r"^F-[A-Z0-9]+-\d+[a-z]?$")
FIELDS = ("row", "level", "scenario", "commit", "image", "image_commit_source", "run_id", "target", "device", "status", "rung_claim", "evidence", "covered",
          "not_covered", "clauses_attested", "needs_human", "ran_at", "flaky", "reason")


def feature_sha(feature_text):
    """A short hash of the row's Feature cell: a scenario names the text it was written against."""
    return hashlib.sha256(re.sub(r"\s+", " ", feature_text.strip()).encode("utf-8")).hexdigest()[:16]


def level_of(target, device=""):
    """The evidence level (never merged with another): qemu, browser, or machine-<device name> for a paired machine. unit and human are their own
    kinds (unit tests are cited in FEATURES.md; human files are human/*.json)."""
    if target == "real":
        return "machine-" + (re.sub(r"[^A-Za-z0-9_.-]+", "-", device) or "unknown")
    return target


def new_result(row, scenario, target, status, **kw):
    if status not in STATUSES:
        raise ValueError("unknown status %r" % status)
    if target not in TARGETS:
        raise ValueError("unknown target %r" % target)
    if not ROW_RE.match(row):
        raise ValueError("not a row id: %r" % row)
    lvl = kw.pop("level", None) or level_of(target, kw.get("device", ""))
    r = {"row": row, "level": lvl, "scenario": scenario, "commit": "", "image": "", "image_commit_source": "unknown", "run_id": "", "target": target, "device": "",
         "status": status, "rung_claim": 70 if status == "PASS" else 0, "evidence": [], "covered": [], "not_covered": [], "clauses_attested": False,
         "needs_human": [], "ran_at": time.strftime("%Y-%m-%dT%H:%M:%S"), "flaky": False, "reason": ""}
    r.update(kw)
    return r


def validate(r):
    """A list of problems with a result record (empty when it is well formed)."""
    bad = []
    for f in FIELDS:
        if f not in r:
            bad.append("missing field %s" % f)
    if bad:
        return bad
    if r["level"] != level_of(r["target"], r["device"]):
        bad.append("level %r does not fit target %r and device %r" % (r["level"], r["target"], r["device"]))
    if r["status"] not in STATUSES:
        bad.append("unknown status %r" % r["status"])
    if r["target"] not in TARGETS:
        bad.append("unknown target %r" % r["target"])
    if not ROW_RE.match(str(r["row"])):
        bad.append("not a row id")
    for f in ("evidence", "covered", "not_covered", "needs_human"):
        if not isinstance(r[f], list):
            bad.append("%s is not a list" % f)
    if r["status"] == "PASS" and not r["evidence"]:
        bad.append("a PASS names no evidence file")
    return bad


def write_result(out_root, r):
    """OUT/<ROW>/<target>/result.json (the evidence files of the scenario live beside it)."""
    bad = validate(r)
    if bad:
        raise ValueError("bad result for %s: %s" % (r.get("row"), "; ".join(bad)))
    d = os.path.join(out_root, r["row"], r["level"])
    os.makedirs(d, exist_ok=True)
    p = os.path.join(d, "result.json")
    with open(p, "w", encoding="utf-8") as f:
        json.dump(r, f, indent=1, sort_keys=True)
        f.write("\n")
    return p


def load_results(root):
    """Every result.json under root (a run folder or a folder of run folders), parsed; unreadable files are skipped and named."""
    out, skipped = [], []
    for dp, _dn, fns in os.walk(root):
        if "result.json" in fns:
            p = os.path.join(dp, "result.json")
            try:
                with open(p, encoding="utf-8") as fh:
                    r = json.load(fh)
                if isinstance(r, dict) and "row" in r:
                    r["_dir"] = dp
                    out.append(r)
            except (OSError, ValueError):
                skipped.append(p)
    return out, skipped


def gate_lock_held(path=None):
    """True while the release gate holds its lock (it takes an flock on this file for the whole run). The file stays on disk after a run, so its existence says nothing: only a
    lock that cannot be taken says the gate is running (Q1, 2026-11-21: the two update scripts refused every run on the existing, unlocked file)."""
    path = path or os.environ.get("IYAKKI_GREEN_LOCK") or os.path.join(os.path.expanduser("~"), "iyakki-work", ".release-green.lock")
    if not os.path.exists(path):
        return False
    with open(path, "a") as fh:
        try:
            fcntl.flock(fh, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            return True
        fcntl.flock(fh, fcntl.LOCK_UN)
    return False
