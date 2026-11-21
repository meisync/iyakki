#!/usr/bin/env python3
"""./run.sh autotest: the test-automation runner (docs/development/AUTOTEST.md).

  ./run.sh autotest [--profile qemu|browser|machine NAME] [--rows F-A-1,F-B-2] [--checklist] [--image BUILD_DIR] [--out DIR]
                    [--update-golden] [--list] [--dry-run] [--image-commit SHA] [--client PATH]

Reads dist/autotest/scenarios/*.scn (the dist/devtest text format plus the autotest headers and verbs), runs the ones for the profile, and writes
OUT/<ROW>/<target>/result.json with evidence beside it, OUT/run.json and OUT/results.json. Evidence stays outside the repository
(default ~/iyakki-work/autotest/<date-time>). It reuses dist/devtest (screen driving, safety guard, restore) and dist/qemu (boot and pairing); it
signs in nothing, types no password, answers no approval except the scratch guest's own, and never builds. One run at a time (a lock), and not while the
release gate holds its lock. Exit: 0 nothing failed, 1 a scenario FAILed, 2 nothing could run, 3 lock held, 4 disk above 85 percent.
"""
import argparse
import datetime
import fcntl
import json
import os
import re
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, "..", "devtest"))
sys.path.insert(0, os.path.join(HERE, "..", "qemu"))
sys.path.insert(0, os.path.join(HERE, "..", "hwtest"))
import devui  # noqa: E402
import builtin_scenarios  # noqa: E402
import results as R  # noqa: E402
import verbs  # noqa: E402

SCENARIO_DIR = os.path.join(HERE, "scenarios")
WORK = os.path.join(os.path.expanduser("~"), "iyakki-work")
LOCK = os.environ.get("IYAKKI_AUTOTEST_LOCK", os.path.join(WORK, ".autotest.lock"))
GATE_LOCK = os.environ.get("IYAKKI_GREEN_LOCK", os.path.join(WORK, ".release-green.lock"))
# the same boot profiles as dist/qemu/session-checks.py (that file has a hyphen and keeps them inside main())
PROFILES = {"ram": {}, "data": {"data_disk": True}, "idle": {}, "full": {"data_disk": True, "ram": 6144, "disk_gb": 16},
            "clock": {"extra_qemu": ["-rtc", "base=2024-01-01T00:00:00"], "append": "no-kvmclock"}}
TARGET_OF_PROFILE = {"qemu": "qemu", "browser": "browser", "machine": "real"}


# ---- planning (pure) -----------------------------------------------------------------------------------------------

def load_scenarios(folder=SCENARIO_DIR):
    """The entries of the autotest folder, sorted by file name. Each entry gets rows, targets and the devices the devtest guard needs."""
    builtin_scenarios.load_text_scenarios(folder)
    out = []
    for e in devui.SCENARIOS.values():
        f = e.get("file")
        if not f or os.path.dirname(os.path.abspath(f)) != os.path.abspath(folder):
            continue
        m = e["meta"]
        rows = [x.strip() for x in re.split(r"[,\s]+", m.get("row", "")) if x.strip()]
        if not rows:
            stem = os.path.splitext(os.path.basename(f))[0]
            rows = [stem] if R.ROW_RE.match(stem) else []
        e["rows"] = rows
        e["targets"] = (m.get("targets") or "qemu").split()
        e["devices"] = tuple(d for d, k in (("qemu", "qemu"), ("real", "real")) if k in e["targets"] or (k == "real" and any(t.startswith("hw:") for t in e["targets"])))
        out.append(e)
    return sorted(out, key=lambda e: e["file"])


def problems(entries, known_rows=None):
    """Scenario files that break the format: no row, a bad row id, no clause, an unknown row."""
    bad = []
    for e in entries:
        n = os.path.basename(e["file"])
        if not e["rows"]:
            bad.append("%s: no `row:` header and the file name is not a row id" % n)
        for r in e["rows"]:
            if not R.ROW_RE.match(r):
                bad.append("%s: %r is not a row id" % (n, r))
            elif known_rows is not None and r not in known_rows:
                bad.append("%s: row %s is not in FEATURES.md" % (n, r))
        if not e["meta"].get("clause") and not e["meta"].get("human"):
            bad.append("%s: no `clause:` header (a scenario must say what it covers)" % n)
        for t in e["targets"]:
            if t not in R.TARGETS and not t.startswith("hw:"):
                bad.append("%s: unknown target %r" % (n, t))
    return bad


def select(entries, profile, rows=None):
    """Entries for the profile (and the --rows filter). A row asked for by name but not for this target is NOT-APPLICABLE for the report."""
    target = TARGET_OF_PROFILE[profile]
    want = set(rows) if rows else None
    run, na = [], []
    for e in entries:
        if want is not None and not (want & set(e["rows"])):
            continue
        ok = (target in e["targets"]) or (target == "real" and any(t.startswith("hw:") for t in e["targets"]))
        (run if ok else na).append(e)
    return run, (na if want is not None else [])


def features_index(path=os.path.join(ROOT, "FEATURES.md")):
    """{row id: Feature cell}"""
    idx = {}
    try:
        for line in open(path, encoding="utf-8"):
            if line.startswith("| F-"):
                c = [x.strip() for x in line.strip().strip("|").split("|")]
                idx[c[0]] = c[1]
    except OSError:
        pass
    return idx


def screen_evidence(entry):
    verbs_ = [s.split(" ", 1)[0] for s in entry.get("steps", [])]
    return "shot" in verbs_ and ("expect-text" in verbs_ or "expect-golden" in verbs_)


def map_status(res, new_golden=False):
    """devtest result dict -> (autotest status, reason)."""
    st, why = res.get("status"), res.get("reason", "")
    if st == "pass":
        return ("NEW-GOLDEN", "a golden screen was recorded as a candidate") if new_golden else ("PASS", "")
    if st in ("fail", "error"):
        return "FAIL", why
    if st == "skipped":
        if why.startswith("not run:"):
            return "BLOCKED", why
        if why.startswith("scenario supports"):
            return "NOT-APPLICABLE", why
        return "SKIP", why
    return "FAIL", "unknown devtest status %r" % st


def build_results(entry, target, status, reason, meta, feat_idx, evidence_by_row, device="", device_ok=None, new_golden=False):
    """A record per row, at its own level (qemu, browser, machine-NAME): levels are never merged (docs/development/AUTOTEST.md)."""
    """One result record per row of the scenario."""
    out = []
    covered = list(entry["meta"].get("clause", []))
    not_cov = list(entry["meta"].get("not-covered", []))
    human = list(entry["meta"].get("human", []))
    if target != "real":
        not_cov += ["needs a person: %s" % h for h in human]
    for row in entry["rows"]:
        fsha = R.feature_sha(feat_idx.get(row, ""))
        attested = bool(entry["meta"].get("feature-sha")) and entry["meta"]["feature-sha"] == fsha
        s = status
        if human and not covered and status == "PASS":
            s = "NEEDS-HUMAN"
        r = R.new_result(row, os.path.splitext(os.path.basename(entry["file"]))[0], target, s, commit=meta["commit"], image=meta["image"],
                         image_commit_source=meta["commit_source"], run_id=meta["run_id"], device=device, evidence=evidence_by_row.get(row, []),
                         covered=covered if s == "PASS" else [], not_covered=not_cov, clauses_attested=attested,
                         needs_human=[h.split("|")[0].strip() for h in human], reason=reason,
                         scenario_file=os.path.relpath(entry["file"], ROOT))
        r["feature_sha"] = fsha
        r["screen_evidence"] = screen_evidence(entry)
        if device_ok is not None:
            r["device_ok"] = device_ok
        r["rung_claim"] = 70 if s == "PASS" and not not_cov else 0
        out.append(r)
    return out


# ---- guards ---------------------------------------------------------------------------------------------------------

def disk_ok(path, limit=0.85):
    p = path
    while not os.path.exists(p):
        p = os.path.dirname(p) or "/"
    u = shutil.disk_usage(p)
    return u.used / float(u.total) <= limit


def take_lock(path=LOCK):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    fh = open(path, "w")
    try:
        fcntl.flock(fh, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        fh.close()
        return None
    return fh


def gate_running(path=GATE_LOCK):
    if not os.path.exists(path):
        return False
    with open(path, "w") as fh:
        try:
            fcntl.flock(fh, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            return True
        fcntl.flock(fh, fcntl.LOCK_UN)
    return False


def image_commit(build, given=None):
    """(commit, source): the argument, else the `commit:` line of the gate's RESULT.txt next to the pack, else the repository HEAD marked unknown."""
    if given:
        return given, "argument"
    d = os.path.abspath(build) if build else ""
    for _ in range(4):
        rt = os.path.join(d, "RESULT.txt")
        if d and os.path.isfile(rt):
            m = re.search(r"^commit:\s*([0-9a-f]{7,40})", open(rt).read(), re.M)
            if m:
                return m.group(1), "RESULT.txt"
        d = os.path.dirname(d)
    try:
        head = subprocess.run(["git", "-C", ROOT, "rev-parse", "HEAD"], capture_output=True, text=True, timeout=20).stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        head = ""
    return head, "unknown"


# ---- running ---------------------------------------------------------------------------------------------------------

def _copy_evidence(src_dir, rel, rows, out_root, target, first_row):
    """Evidence of one scenario run lives under OUT/<first row>/<target>/<name>/; the other rows of the scenario get a copy of the folder."""
    files = []
    base = os.path.join(out_root, first_row, target, rel)
    if os.path.isdir(base):
        for dp, _dn, fns in os.walk(base):
            for fn in sorted(fns):
                files.append(os.path.relpath(os.path.join(dp, fn), os.path.join(out_root, first_row, target)))
    by = {first_row: files}
    for row in rows[1:]:
        dst = os.path.join(out_root, row, target, rel)
        if os.path.isdir(base):
            shutil.copytree(base, dst, dirs_exist_ok=True)
        by[row] = list(files)
    return by


def run_entries(entries, dev, target, out_root, meta, feat_idx, ctx_kw, update_golden, device="", device_ok=None, log=print):
    """Run text scenarios on one device handle; returns the list of result records (also written)."""
    allr = []
    for e in entries:
        name = os.path.splitext(os.path.basename(e["file"]))[0]
        first = e["rows"][0]
        verbs.Active.reset(first, update_golden=update_golden, **ctx_kw)
        lvl = R.level_of(target, device)
        scn_root = os.path.join(out_root, first, lvl)
        os.makedirs(scn_root, exist_ok=True)
        entry = dict(e)
        entry["name"] = name
        log("[%s] %s ..." % (target, name))
        if not e["steps"] and e["meta"].get("human"):
            res = {"status": "pass", "reason": "", "checks": [], "shots": []}
        else:
            res = devui.run_scenario(dev, entry, scn_root)
        status, reason = map_status(res, new_golden=bool(verbs.Active.new_goldens))
        ev = _copy_evidence(scn_root, name, e["rows"], out_root, lvl, first)
        # the check list is evidence too
        for row in e["rows"]:
            d = os.path.join(out_root, row, lvl, name)
            os.makedirs(d, exist_ok=True)
            with open(os.path.join(d, "checks.json"), "w") as fh:
                json.dump({"checks": res.get("checks", []), "notes": res.get("notes", []), "reason": reason}, fh, indent=1)
            ev.setdefault(row, [])
            if os.path.join(name, "checks.json") not in ev[row]:
                ev[row].append(os.path.join(name, "checks.json"))
        for rec in build_results(e, target, status, reason, meta, feat_idx, ev, device, device_ok, bool(verbs.Active.new_goldens)):
            R.write_result(out_root, rec)
            allr.append(rec)
        log("[%s] %s: %s %s" % (target, name, status, reason))
    return allr


def run_session_checks(entries, sess, out_root, meta, feat_idx, log=print):
    """The adapter: a scenario with `session-check: NAME` runs that check of dist/qemu/session_checks_defs.py on the booted session."""
    import session_checks_defs as d0
    import session_checks_ui_defs as d1
    allr = []
    for e in entries:
        name = e["meta"]["session-check"]
        fn = getattr(d0, "check_" + name, None) or getattr(d1, "check_" + name, None)
        first = e["rows"][0]
        folder = os.path.join(out_root, first, "qemu", os.path.splitext(os.path.basename(e["file"]))[0])
        os.makedirs(folder, exist_ok=True)
        try:
            if fn is None:
                raise devui.Skip("no function check_%s" % name)
            ev, status, why = fn(sess), "PASS", ""
        except devui.Skip as ex:
            ev, status, why = "skipped: %s" % ex, "SKIP", str(ex)
        except Exception as ex:  # noqa: BLE001
            ev, status, why = "%s: %s" % (type(ex).__name__, ex), "FAIL", str(ex)[:200]
        with open(os.path.join(folder, "session-check-%s.txt" % name), "w") as fh:
            fh.write(ev + "\n")
        rel = os.path.join(os.path.basename(folder), "session-check-%s.txt" % name)
        by = {r: [rel] for r in e["rows"]}
        for r in e["rows"][1:]:
            dst = os.path.join(out_root, r, "qemu", os.path.basename(folder))
            os.makedirs(dst, exist_ok=True)
            shutil.copy(os.path.join(folder, "session-check-%s.txt" % name), dst)
        for rec in build_results(e, "qemu", status, why, meta, feat_idx, by):
            R.write_result(out_root, rec)
            allr.append(rec)
        log("[qemu] session check %s: %s" % (name, status))
    return allr


def run_suites(entries, out_root, meta, feat_idx, log=print):
    """The browser target: `suite: SCRIPT ARGS` runs a dist/browsertest script (exit 0 PASS, 1 FAIL, 2 nothing tested = SKIP)."""
    allr = []
    for e in entries:
        parts = e["meta"].get("suite", "").split()
        first = e["rows"][0]
        folder = os.path.join(out_root, first, "browser")
        os.makedirs(folder, exist_ok=True)
        status, why, logname = "SKIP", "no suite named", "suite.log"
        if parts:
            script = os.path.join(ROOT, "dist", "browsertest", parts[0])
            try:
                p = subprocess.run([sys.executable, script] + parts[1:] + ["--out", os.path.join(folder, "suite-out")], capture_output=True, text=True, timeout=1800)
                status = {0: "PASS", 1: "FAIL"}.get(p.returncode, "SKIP")
                why = "" if status == "PASS" else ("exit %d" % p.returncode)
                with open(os.path.join(folder, logname), "w") as fh:
                    fh.write(p.stdout[-20000:] + "\n" + p.stderr[-5000:])
            except (OSError, subprocess.TimeoutExpired) as ex:
                status, why = "SKIP", "could not run: %s" % ex
                open(os.path.join(folder, logname), "w").write(why)
        by = {r: [logname] for r in e["rows"]}
        for r in e["rows"][1:]:
            os.makedirs(os.path.join(out_root, r, "browser"), exist_ok=True)
            shutil.copy(os.path.join(folder, logname), os.path.join(out_root, r, "browser", logname))
        for rec in build_results(e, "browser", status, why, meta, feat_idx, by):
            R.write_result(out_root, rec)
            allr.append(rec)
        log("[browser] %s: %s" % (os.path.basename(e["file"]), status))
    return allr


def device_gate(dev):
    """For a paired machine: (ok, reason). Refuses a machine that is at the sign-in screen or in use (the existing guard of device-ui)."""
    st = dev.try_state()
    if st is None:
        return False, "not reachable"
    if st.app.lower() not in ("home", "settings"):
        return False, "not run: %r is in front (signed out or in use)" % st.app
    return True, ""


def hw_ok(entry, info):
    """(ok, reason) for the `hw:KEY` targets of an entry against `dev info` text."""
    import hwmatch
    for t in entry["targets"]:
        if t.startswith("hw:"):
            key = t[3:]
            if key not in hwmatch.REQUIREMENTS:
                return False, "unknown hardware key %s" % key
            ok, why = hwmatch.check(key, info)
            if not ok:
                return False, why
    return True, ""


def quarantined(path=os.path.join(HERE, "quarantine.txt")):
    try:
        return {l.split("#")[0].strip() for l in open(path) if l.split("#")[0].strip()}
    except OSError:
        return set()


def apply_quarantine(recs, q):
    """A quarantined scenario never counts as PASS: its result is BLOCKED (the list may only shrink)."""
    for r in recs:
        if r["scenario"] in q and r["status"] == "PASS":
            r["status"], r["rung_claim"], r["reason"] = "BLOCKED", 0, "quarantined as flaky (dist/autotest/quarantine.txt)"
    return recs


def finish(out, meta, recs, log=print):
    with open(os.path.join(out, "run.json"), "w") as fh:
        json.dump(meta, fh, indent=1)
    with open(os.path.join(out, "results.json"), "w") as fh:
        json.dump({"run": meta, "results": [{k: v for k, v in r.items() if k != "_dir"} for r in recs]}, fh, indent=1)
    counts = {}
    for r in recs:
        counts[r["status"]] = counts.get(r["status"], 0) + 1
    log("results: %s" % ", ".join("%s %d" % kv for kv in sorted(counts.items())) if counts else "no results")
    log("evidence: %s" % out)
    return counts


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0], formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--profile", default="qemu", choices=["qemu", "browser", "machine"])
    ap.add_argument("machine", nargs="?", help="with --profile machine: the paired device name (iyakki dev devices)")
    ap.add_argument("--rows", help="comma separated row ids")
    ap.add_argument("--checklist", action="store_true", help="write the owner's checklist pages (from --results DIR, or from this run)")
    ap.add_argument("--results", help="with --checklist alone: read the results of this folder")
    ap.add_argument("--image", metavar="BUILD_DIR", help="the build folder with images/bzImage and rootfs.cpio.zst (qemu profile)")
    ap.add_argument("--image-commit")
    ap.add_argument("--client", help="the iyakki client binary")
    ap.add_argument("--out")
    ap.add_argument("--update-golden", action="store_true")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--dry-run", action="store_true", help="print the plan, boot nothing")
    ap.add_argument("--keep", action="store_true")
    a = ap.parse_args(argv)
    entries = load_scenarios()
    feat_idx = features_index()
    bad = problems(entries, set(feat_idx) if feat_idx else None)
    rows = [r.strip() for r in a.rows.split(",")] if a.rows else None
    if a.list:
        for e in entries:
            print("%-14s %-22s %s %s" % (",".join(e["rows"]), "/".join(e["targets"]), os.path.basename(e["file"]), e["meta"].get("doc", "")[:70]))
        for b in bad:
            print("PROBLEM " + b)
        return 1 if bad else 0
    if a.checklist and not a.profile == "machine" and a.results:
        import checklist
        print("\n".join(checklist.write_pages(a.results, os.path.join(a.results, "checklist"))))
        return 0
    if bad:
        for b in bad:
            print("PROBLEM " + b, file=sys.stderr)
        return 2
    if a.profile == "machine" and not a.machine:
        ap.error("--profile machine needs the device name")
    run, na = select(entries, a.profile, rows)
    if a.dry_run:
        print("profile %s: %d scenario(s) would run, %d not for this target" % (a.profile, len(run), len(na)))
        for e in run:
            print("  run  %-14s %s" % (",".join(e["rows"]), os.path.basename(e["file"])))
        for e in na:
            print("  n/a  %-14s %s" % (",".join(e["rows"]), os.path.basename(e["file"])))
        return 0
    out = os.path.abspath(a.out or os.path.join(WORK, "autotest", time.strftime("%Y%m%d-%H%M%S")))
    if not disk_ok(out):
        print("autotest: the disk is above 85 percent; clean ~/iyakki-work first", file=sys.stderr)
        return 4
    if gate_running():
        print("autotest: the release gate holds its lock; try again when it is done", file=sys.stderr)
        return 3
    lk = take_lock()
    if lk is None:
        print("autotest: another run holds %s" % LOCK, file=sys.stderr)
        return 3
    os.makedirs(out, exist_ok=True)
    commit, src = image_commit(a.image, a.image_commit)
    meta = {"run_id": os.path.basename(out), "commit": commit, "commit_source": src, "image": os.path.abspath(a.image) if a.image else "", "profile": a.profile,
            "started": datetime.datetime.now().strftime("%Y-%m-%dT%H:%M:%S")}
    recs = []
    q = quarantined()
    client = a.client or devui.DEFAULT_CLIENT
    rc = 0
    try:
        for e in na:
            recs += build_results(e, TARGET_OF_PROFILE[a.profile], "NOT-APPLICABLE", "scenario is not written for the %s target" % a.profile, meta, feat_idx, {})
        if a.profile == "qemu":
            if not a.image:
                print("--image BUILD_DIR is needed for the qemu profile", file=sys.stderr)
                return 2
            import session_lib
            import qemu_guest  # noqa: F401
            by_prof = {}
            for e in run:
                by_prof.setdefault(e["meta"].get("profile", "ram"), []).append(e)
            for prof, group in by_prof.items():
                if prof not in PROFILES:
                    print("unknown boot profile %s" % prof, file=sys.stderr)
                    continue
                sess = session_lib.Session(os.path.abspath(a.image), client, out, keep=a.keep, **PROFILES[prof])
                try:
                    sess.boot()
                except Exception as ex:  # noqa: BLE001
                    print("profile %s: the guest could not be booted and paired: %s" % (prof, ex), file=sys.stderr)
                    for e in group:
                        for rec in build_results(e, "qemu", "BLOCKED", "the guest could not be booted and paired: %s" % str(ex)[:120], meta, feat_idx, {}):
                            recs.append(rec)
                    sess.stop()
                    continue
                try:
                    kw = {"sh": sess.sh, "serial": sess.serial}
                    plain = [e for e in group if not e["meta"].get("session-check")]
                    chk = [e for e in group if e["meta"].get("session-check")]
                    recs += run_entries(plain, sess.ui(), "qemu", out, meta, feat_idx, kw, a.update_golden)
                    recs += run_session_checks(chk, sess, out, meta, feat_idx)
                finally:
                    with open(os.path.join(out, "serial-%s.log" % prof), "w") as fh:
                        fh.write(sess.serial())
                    sess.stop()
        elif a.profile == "browser":
            recs += run_suites(run, out, meta, feat_idx)
        else:
            dev = devui.Device(a.machine, run_dir=out, client=client, kind="real")
            ok, why = device_gate(dev)
            info = ""
            if ok:
                try:
                    info = dev.info()
                except devui.DevError:
                    info = ""
            for e in run:
                hok, hwhy = hw_ok(e, info) if ok else (False, why)
                if not ok:
                    recs += build_results(e, "real", "BLOCKED", why, meta, feat_idx, {}, device=a.machine, device_ok=False)
                elif not hok:
                    recs += build_results(e, "real", "NOT-APPLICABLE", hwhy, meta, feat_idx, {}, device=a.machine, device_ok=False)
                else:
                    recs += run_entries([e], dev, "real", out, meta, feat_idx, {}, a.update_golden, device=a.machine, device_ok=True)
        recs = apply_quarantine(recs, q)
        for rec in recs:  # every record is written once more here: a quarantine turns a PASS into BLOCKED, and BLOCKED or NOT-APPLICABLE records have no run
            R.write_result(out, rec)
        counts = finish(out, meta, recs)
        if a.checklist:
            import checklist
            print("\n".join(checklist.write_pages(out, os.path.join(out, "checklist"))))
        if counts.get("FAIL"):
            rc = 1
        elif not recs:
            rc = 2
        return rc
    finally:
        lk.close()


if __name__ == "__main__":
    sys.exit(main())
