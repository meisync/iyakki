"""The evidence-rule enforcer: raises the Test column of FEATURES.md from machine-made results, and only by the rules of
docs/development/AUTOTEST.md (plan section 3). Used by `python3 dist/features/migrate-impl-test.py --from-autotest DIR [--apply]`.

Pure code: the clock, the repository root and the folder of results are arguments; nothing is lowered, nothing is guessed.

Rung 70 (Test below 70 -> 70) needs, for the row:
  * the two most recent results of the qemu or browser targets, from two different runs, both PASS, both on the image commit (known, not guessed);
  * each attested (the scenario's feature-sha is the hash of the Feature cell as it is now), `covered` non-empty, `not_covered` empty;
  * evidence files that exist and are not empty, a scenario file that exists in the repository, no flaky flag (the same image commit never
    shows a PASS and a FAIL), results not older than 14 days; Impl 100.
Rung 100 is never set from QEMU evidence unless `qemu_counts_as_real` is switched on (off by default; owner decision pending): then a software-only
row (Manual none, Test 70, no needs-hw or human clause, screen evidence) is set to 100. A `real` target PASS on a machine that matches, all clauses covered,
each human clause answered pass by a human/<ROW>.<clause>.json for that machine, sets 100 and Manual `passed DATE MACHINE`.
Never raised on SKIP, BLOCKED, NOT-APPLICABLE, NEEDS-HUMAN, NEW-GOLDEN, FAIL, stale or unattested results.
"""
import datetime
import hashlib
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import humanschema as H  # noqa: E402
import results as R  # noqa: E402

STALE_DAYS = 14
# Who may answer a human clause: any role of humanschema.BY_ROLES (owner, admin, person) counts as a human pass. The owner looks again at a
# sample of the passes given by the others: SAMPLE_PERCENT of them, chosen by a hash of (row, clause, machine, image commit) so the choice is the
# same on every run and cannot be shopped for. The sample is recorded (in the report line and in owner-review.json), it never blocks a raise.
SAMPLE_PERCENT = 10
RUNGS = (0, 30, 50, 70, 100)


def parse_rows(text):
    """{id: (line index, cells)} for every `| F-` line with nine cells."""
    rows, lines = {}, text.split("\n")
    for i, line in enumerate(lines):
        if line.startswith("| F-"):
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            if len(cells) == 9:
                rows[cells[0]] = (i, cells)
    return rows, lines


def _age_days(ran_at, now):
    try:
        t = datetime.datetime.strptime(ran_at[:19], "%Y-%m-%dT%H:%M:%S")
    except ValueError:
        return 10 ** 6
    return (now - t).total_seconds() / 86400.0


def _evidence_ok(r):
    ev = r.get("evidence") or []
    if not ev:
        return "no evidence files named"
    for p in ev:
        if os.path.isabs(p) or ".." in p.split("/"):
            return "evidence path %s leaves the result folder" % p
        full = os.path.join(r.get("_dir", ""), p)
        if not os.path.isfile(full) or os.path.getsize(full) == 0:
            return "evidence file %s is missing or empty" % p
    return None


def _result_problem(r, fsha, now, repo_root):
    """Why one result cannot count (None when it can)."""
    if r.get("status") != "PASS":
        return "status is %s, not PASS" % r.get("status")
    if R.validate(r):
        return "malformed result: " + "; ".join(R.validate(r))
    if not r.get("clauses_attested") or r.get("feature_sha") != fsha:
        return "the scenario's clauses were written for another Feature text (feature-sha differs): stale clause list"
    if not r.get("covered"):
        return "no clause is named as covered"
    if r.get("not_covered"):
        return "not covered: %s" % "; ".join(r["not_covered"][:3])
    if r.get("flaky"):
        return "flagged flaky"
    if _age_days(r.get("ran_at", ""), now) > STALE_DAYS:
        return "older than %d days: evidence expired" % STALE_DAYS
    bad = _evidence_ok(r)
    if bad:
        return bad
    sf = r.get("scenario_file", "")
    if not sf or not os.path.isfile(os.path.join(repo_root, sf)):
        return "the scenario file %r is not in the repository" % sf
    return None


def _provided_by_program(repo_root, scenario_file):
    """True when the scenario file carries the header `provided-by: program` (before its first step)."""
    try:
        with open(os.path.join(repo_root, scenario_file), encoding="utf-8") as fh:
            return any(re.match(r"^provided-by:\s*program\s*$", ln.split("#", 1)[0].strip()) for ln in fh)
    except OSError:
        return False


def _is_flaky(group):
    """The same image commit shows both a PASS and a FAIL."""
    by = {}
    for r in group:
        by.setdefault(r.get("commit"), set()).add(r.get("status"))
    return any("PASS" in s and "FAIL" in s for s in by.values())


def human_path(human_dir, row, clause, machine):
    safe = lambda x: re.sub(r"[^A-Za-z0-9_.-]+", "-", x)  # noqa: E731
    return os.path.join(human_dir, "%s.%s.%s.json" % (row, safe(clause), safe(machine)))


def _human_file(human_dir, row, clause, machine):
    """The human evidence file of (row, clause, machine): in human/, or in human/<machine>/ where an import puts another machine's answers.
    -> (record, problem). A record that breaks the schema (an unknown field, a bad role) is a problem, never a pass."""
    name = os.path.basename(human_path(human_dir, row, clause, machine))
    safe_machine = re.sub(r"[^A-Za-z0-9_.-]+", "-", machine)
    for p in (os.path.join(human_dir, name), os.path.join(human_dir, safe_machine, name)):
        if os.path.isfile(p):
            try:
                with open(p, encoding="utf-8") as fh:
                    h = json.load(fh)
            except (OSError, ValueError):
                return None, "human result for clause %r cannot be read" % clause
            bad = H.validate_human(h)
            if bad:
                return None, "human result for clause %r breaks the schema: %s" % (clause, bad[0])
            return h, ""
    return None, "no human result for clause %r on %s" % (clause, machine)


def sampled_for_owner(h):
    """True when the owner must look again at this pass: a pass of a team member or another person (never the owner's own) that the hash picks."""
    if h.get("by") == "owner":
        return False
    key = "|".join(str(h.get(k, "")) for k in ("row", "clause", "machine", "image_commit"))
    return int(hashlib.sha256(key.encode("utf-8")).hexdigest(), 16) % 100 < SAMPLE_PERCENT


def _human_ok(human_dir, row, clauses, machine, image_commit=None, sampled=None):
    """(True, "") when every clause has a verdict `pass` file for this machine (schema: docs/development/AUTOTEST.md, human evidence). The role in
    `by` may be owner, admin or person; the clauses the sampling rule picks are appended to `sampled` (a list), if one is given."""
    for c in clauses:
        h, why = _human_file(human_dir, row, c, machine)
        if h is None:
            return False, why
        if h.get("row") != row or h.get("clause") != c or h.get("machine") != machine or h.get("verdict") != "pass":
            return False, "human result for clause %r is not a pass on %s" % (c, machine)
        if image_commit and h.get("image_commit") != image_commit:
            return False, "human result for clause %r is for another image commit" % c
        if sampled is not None and sampled_for_owner(h):
            sampled.append("%s (by %s)" % (c, h.get("by")))
    return True, ""


def review_queue(human_dir):
    """Every pass of a team member or another person the sampling rule hands to the owner, in human/ and human/<machine>/: a list of
    {row, clause, machine, by, image_commit, date}. Pure reading."""
    out = []
    if not os.path.isdir(human_dir):
        return out
    paths = []
    for fn in sorted(os.listdir(human_dir)):
        full = os.path.join(human_dir, fn)
        if os.path.isdir(full):
            paths += [os.path.join(full, x) for x in sorted(os.listdir(full))]
        else:
            paths.append(full)
    for p in paths:
        if not p.endswith(".json"):
            continue
        try:
            with open(p, encoding="utf-8") as fh:
                h = json.load(fh)
        except (OSError, ValueError):
            continue
        if not H.validate_human(h) and h.get("verdict") == "pass" and sampled_for_owner(h):
            out.append({k: h[k] for k in ("row", "clause", "machine", "by", "image_commit", "date")})
    return out


def decide(cells, group, fsha, now, repo_root, image_commit=None, qemu_counts_as_real=False, human_dir=None):
    """-> (new Test or None, new Manual or None, reason). group = every result of this row. Never lowers."""
    impl, test = [int(x) for x in cells[5].split("/")]
    manual = cells[6]
    if test == 100:
        return None, None, "already 100"
    if not group:
        return None, None, "no result"
    if impl < 100:
        return None, None, "Impl is %d: code is still missing" % impl
    soft = sorted([r for r in group if r.get("target") in ("qemu", "browser")], key=lambda r: r.get("ran_at", ""), reverse=True)
    real = sorted([r for r in group if r.get("target") == "real"], key=lambda r: r.get("ran_at", ""), reverse=True)
    real_why = ""
    sampled = []
    # a real machine, rung 100
    if real and test >= 70 and (manual == "needed" or manual == "none"):
        r = real[0]
        p = _result_problem(r, fsha, now, repo_root)
        machine = r.get("device") or ""
        if p is None and machine and r.get("device_ok") is True and not _is_flaky(real):
            if image_commit and r.get("commit") != image_commit:
                p = "result is for commit %s, the image is %s" % (r.get("commit"), image_commit)
            elif r.get("needs_human"):
                ok, why = _human_ok(human_dir or "", cells[0], r["needs_human"], machine, image_commit, sampled)
                if not ok:
                    p = why
            if p is None:
                note = "" if not sampled else "; owner review sampled (%d percent rule): %s" % (SAMPLE_PERCENT, ", ".join(sampled))
                return 100, "passed %s %s" % (r["ran_at"][:10], machine), "real-machine PASS on %s, all clauses covered%s" % (machine, note)
        real_why = p or ("the machine did not match (device_ok is not true)" if not r.get("device_ok") else "")
    if not soft:
        return None, None, "no qemu or browser result" + ("" if not real else " (the real-machine result did not qualify: %s)" % real_why)
    # the software rungs
    latest = soft[:2]
    if latest[0].get("status") != "PASS":
        return None, None, "the latest %s result is %s (%s)" % (latest[0].get("target"), latest[0].get("status"), latest[0].get("reason", "")[:80])
    if len(latest) < 2:
        return None, None, "one PASS so far: two consecutive PASS runs are needed"
    if _is_flaky(soft):
        return None, None, "flaky: the same image commit shows a PASS and a FAIL"
    for r in latest:
        p = _result_problem(r, fsha, now, repo_root)
        if p:
            return None, None, p
        if r.get("image_commit_source") == "unknown" or not r.get("commit"):
            return None, None, "the image commit is not known"
    if latest[0].get("run_id") == latest[1].get("run_id"):
        return None, None, "both PASS results come from one run: two different runs are needed"
    if latest[0]["commit"] != latest[1]["commit"] or (image_commit and latest[0]["commit"] != image_commit):
        return None, None, "the PASS runs are not both on the image commit%s" % (" %s" % image_commit if image_commit else "")
    r = latest[0]
    layered = ""
    for x in latest:
        base = x.get("base_commit") or ""
        if base and base != x["commit"]:
            if not _provided_by_program(repo_root, x.get("scenario_file", "")):
                return None, None, "the image is the program %s over the older base %s and the scenario does not say `provided-by: program`: a clause may come from the base, run it on one image" % (x["commit"][:8], base[:8])
            layered = "; program %s over base %s (provided-by: program)" % (x["commit"][:8], base[:8])
    if test < 70:
        return 70, None, "two consecutive PASS runs on %s (%s), every clause covered%s" % (r["commit"][:8], r["target"], layered)
    if qemu_counts_as_real and test == 70 and manual == "none" and not r.get("needs_human") and r.get("screen_evidence"):
        return 100, None, "QEMU counts as real (switch on): software-only row, screen evidence, two PASS runs"
    return None, None, "already %d; 100 needs a real machine%s" % (test, "" if not qemu_counts_as_real else " or screen evidence")


def apply_to_text(text, results_dir, now=None, repo_root=None, image_commit=None, qemu_counts_as_real=False, human_dir=None):
    """-> (new text, report). report = [(row, old Test, new Test, why)] for rows raised, plus refusals as (row, test, None, why)."""
    now = now or datetime.datetime.now()
    repo_root = repo_root or os.getcwd()
    rows, lines = parse_rows(text)
    allres, skipped = R.load_results(results_dir)
    by = {}
    for r in allres:
        by.setdefault(r.get("row"), []).append(r)
    report = []
    for rid, group in sorted(by.items()):
        if rid not in rows:
            report.append((rid, None, None, "no such row in FEATURES.md"))
            continue
        i, cells = rows[rid]
        fsha = R.feature_sha(cells[1])
        new_t, new_m, why = decide(cells, group, fsha, now, repo_root, image_commit, qemu_counts_as_real, human_dir or os.path.join(results_dir, "human"))
        impl, test = [int(x) for x in cells[5].split("/")]
        if new_t is None or new_t <= test:
            report.append((rid, test, None, why))
            continue
        cells[5] = "%d/%d" % (impl, new_t)
        if new_m:
            cells[6] = new_m
        sf = max(group, key=lambda r: r.get("ran_at", "")).get("scenario_file", "")
        if sf and ("script:" + sf) not in cells[7]:
            cells[7] = (cells[7] + "<br>" if cells[7] not in ("", "—") else "") + "`script:%s`" % sf
        if new_t == 100 and (cells[6] == "none" or cells[6].startswith("passed")):
            cells[3] = "done"  # feature_matrix: a 100/100 row with no open manual test is `done`
        lines[i] = "| " + " | ".join(cells) + " |"
        report.append((rid, test, new_t, why))
    return "\n".join(lines), report


def main(argv, features_path, root):
    import argparse
    ap = argparse.ArgumentParser(prog="migrate-impl-test.py --from-autotest")
    ap.add_argument("dir")
    ap.add_argument("--apply", action="store_true", help="write FEATURES.md (default: print what would change)")
    ap.add_argument("--image-commit", help="the commit the image under test was built from")
    ap.add_argument("--qemu-counts-as-real", action="store_true", help="OFF by default (owner decision pending): let screen-evidenced QEMU PASS runs set 100 for software-only rows")
    ap.add_argument("--now", help="ISO date-time for the staleness rule (tests)")
    ap.add_argument("--features", default=features_path)
    a = ap.parse_args(argv)
    now = datetime.datetime.strptime(a.now, "%Y-%m-%dT%H:%M:%S") if a.now else datetime.datetime.now()
    text = open(a.features, encoding="utf-8").read()
    new, report = apply_to_text(text, a.dir, now, root, a.image_commit, a.qemu_counts_as_real)
    for rid, old, nt, why in report:
        print("%-12s %s %s" % (rid, "RAISE %s -> %s" % (old, nt) if nt is not None else "keep  %s" % old, why))
    n = sum(1 for r in report if r[2] is not None)
    print("%d row(s) %s, %d kept" % (n, "raised" if a.apply else "would be raised (dry run; --apply writes FEATURES.md)", len(report) - n))
    queue = review_queue(os.path.join(a.dir, "human"))
    if queue:
        print("owner review sample (%d percent of the passes of team members and others): %d clause(s)" % (SAMPLE_PERCENT, len(queue)))
    if a.apply:
        with open(os.path.join(a.dir, "owner-review.json"), "w", encoding="utf-8") as fh:
            json.dump({"schema": 1, "sample_percent": SAMPLE_PERCENT, "to_review": queue}, fh, indent=1, sort_keys=True)
            fh.write("\n")
    if a.apply and n:
        open(a.features, "w", encoding="utf-8").write(new)
    return 0
