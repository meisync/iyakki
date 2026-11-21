#!/usr/bin/env python3
"""The nightly schedule on one lock, and the morning triage (docs/development/AUTOTEST.md). Nothing here enables a cron job or a timer.

  python3 dist/autotest/nightly.py plan                       the timeline
  python3 dist/autotest/nightly.py run gate|qemu|machines|triage [--image BUILD_DIR] [--results-root DIR]
  python3 dist/autotest/nightly.py triage [--results-root DIR] [--drafts-dir DIR]
  python3 dist/autotest/nightly.py install [--dir DIR]        writes systemd unit text to DIR (or prints it); YOU enable it, if you want it

Timeline (local time; each step waits for the previous step's DONE marker, one lock for all): 01:00 gate and image (AMD host), 02:30 autotest qemu then
browser, 04:00 paired machines one at a time (a machine at its sign-in screen is BLOCKED, not retried), 06:30 triage.
Triage never edits FEATURES.md and never lowers Test: two FAILs in a row draft a KNOWN-ISSUES entry file (no invented KI number); a PASS and a FAIL on one
commit goes to the FLAKY-TESTS suggestions; a level with no PASS for 14 days is reported as expired evidence.
"""
import argparse
import datetime
import fcntl
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
import results as R  # noqa: E402

WORK = os.path.join(os.path.expanduser("~"), "iyakki-work")
RESULTS_ROOT = os.path.join(WORK, "autotest")
LOCK = os.environ.get("IYAKKI_AUTOTEST_NIGHTLY_LOCK", os.path.join(WORK, ".autotest-nightly.lock"))
STALE_DAYS = 14
TIMELINE = [("gate", "01:00", "AMD host: ./run.sh release-green --image (the gate and the signed lean image)", None),
            ("qemu", "02:30", "AMD host: autotest --profile qemu, then --profile browser, on that image", "gate"),
            ("machines", "04:00", "each paired machine, one at a time: autotest --profile machine NAME (sign-in screen => BLOCKED)", "qemu"),
            ("triage", "06:30", "triage: KI drafts, flake suggestions, expired evidence", "machines")]


def plan_text():
    return "\n".join("%s  %-9s %s%s" % (t, n, what, ("  (after %s)" % after) if after else "") for n, t, what, after in TIMELINE)


def wait_for(path, deadline, clock=time.time, sleep=time.sleep, step=30):
    """True when `path` exists before the deadline (injectable clock and sleep)."""
    while True:
        if os.path.exists(path):
            return True
        if clock() >= deadline:
            return False
        sleep(step)


def take_lock(path=LOCK):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    fh = open(path, "w")
    try:
        fcntl.flock(fh, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        fh.close()
        return None
    return fh


def _parse(ts):
    try:
        return datetime.datetime.strptime(ts[:19], "%Y-%m-%dT%H:%M:%S")
    except ValueError:
        return datetime.datetime.min


def triage(results, now):
    """-> {"drafts": [...], "flakes": [...], "expired": [...]} from a list of result records (any levels, any runs). Pure."""
    by = {}
    for r in results:
        by.setdefault((r["row"], r["level"]), []).append(r)
    drafts, flakes, expired = [], [], []
    for (row, level), group in sorted(by.items()):
        group.sort(key=lambda r: r.get("ran_at", ""))
        real = [r for r in group if r["status"] in ("PASS", "FAIL")]
        if len(real) >= 2 and real[-1]["status"] == "FAIL" and real[-2]["status"] == "FAIL" and real[-1].get("run_id") != real[-2].get("run_id"):
            good = [r for r in real if r["status"] == "PASS"]
            drafts.append({"row": row, "level": level, "scenario": real[-1]["scenario"], "evidence": real[-1].get("_dir", ""),
                           "last_good_commit": good[-1]["commit"] if good else "", "reason": real[-1].get("reason", "")})
        commits = {}
        for r in real:
            commits.setdefault(r.get("commit"), set()).add(r["status"])
        for c, st in sorted(commits.items(), key=lambda kv: str(kv[0])):
            if st == {"PASS", "FAIL"}:
                flakes.append({"row": row, "level": level, "scenario": group[-1]["scenario"], "commit": c})
        passes = [r for r in group if r["status"] == "PASS"]
        if passes and (now - _parse(passes[-1]["ran_at"])).days > STALE_DAYS:
            expired.append({"row": row, "level": level, "last_pass": passes[-1]["ran_at"][:10]})
    return {"drafts": drafts, "flakes": flakes, "expired": expired}


def write_triage(t, drafts_dir, out_dir, date):
    """KI-DRAFT files, a flake-suggestion file and the morning report. Returns the paths written."""
    paths = []
    os.makedirs(drafts_dir, exist_ok=True)
    for d in t["drafts"]:
        p = os.path.join(drafts_dir, "KI-DRAFT-%s-%s.md" % (d["row"], d["level"]))
        with open(p, "w", encoding="utf-8") as fh:
            fh.write("# KNOWN-ISSUES draft: %s at level %s\n\nTwo runs in a row FAILed (drafted %s; the coordinator turns this into a KNOWN-ISSUES entry and gives it the next number).\n\n"
                     "* Scenario: `%s`\n* Evidence: %s\n* Last good commit: %s\n* Last reason: %s\n" % (d["row"], d["level"], date, d["scenario"], d["evidence"], d["last_good_commit"] or "none seen", d["reason"]))
        paths.append(p)
    os.makedirs(out_dir, exist_ok=True)
    p = os.path.join(out_dir, "flaky-suggestions.txt")
    with open(p, "w", encoding="utf-8") as fh:
        for f in t["flakes"]:
            fh.write("%s | %s | scenario %s | passes and fails on commit %s | suggestion: quarantine it (dist/autotest/quarantine.txt) and fix the wait\n" % (f["row"], f["level"], f["scenario"], f["commit"]))
    paths.append(p)
    p = os.path.join(out_dir, "morning.txt")
    with open(p, "w", encoding="utf-8") as fh:
        fh.write("triage %s: %d draft(s), %d flake(s), %d expired level(s)\n" % (date, len(t["drafts"]), len(t["flakes"]), len(t["expired"])))
        for e in t["expired"]:
            fh.write("evidence expired: %s at %s (last PASS %s); Test is not lowered, the row is marked for review\n" % (e["row"], e["level"], e["last_pass"]))
    paths.append(p)
    return paths


def unit_text(step, host_note=""):
    """systemd unit text for one step (not installed, not enabled)."""
    time_ = {n: t for n, t, _w, _a in TIMELINE}[step]
    svc = ("[Unit]\nDescription=IYAKKI nightly autotest: %s\n\n[Service]\nType=oneshot\nWorkingDirectory=%s\nExecStart=/usr/bin/python3 dist/autotest/nightly.py run %s\n" % (step, ROOT, step))
    tim = ("[Unit]\nDescription=IYAKKI nightly autotest %s at %s\n\n[Timer]\nOnCalendar=*-*-* %s:00\nPersistent=false\n\n[Install]\nWantedBy=timers.target\n" % (step, time_, time_))
    return svc, tim


def run_step(step, image, root, drafts_dir, log=print, runner=None):
    runner = runner or (lambda cmd: subprocess.run(cmd, cwd=ROOT).returncode)
    day = time.strftime("%Y%m%d")
    marks = os.path.join(root, "nightly", day)
    os.makedirs(marks, exist_ok=True)
    after = {n: a for n, _t, _w, a in TIMELINE}[step]
    if after and not wait_for(os.path.join(marks, after + ".DONE"), time.time() + 3 * 3600):
        log("nightly %s: step %s never finished; not running" % (step, after))
        return 2
    lk = take_lock()
    if lk is None:
        log("nightly: another step holds the lock")
        return 3
    try:
        rc = 0
        if step == "gate":
            rc = runner(["./run.sh", "release-green", "--image"])
        elif step == "qemu":
            out = os.path.join(root, "%s-nightly" % day)
            rc = runner([sys.executable, os.path.join(HERE, "run.py"), "--profile", "qemu", "--image", image or "", "--out", out + "-qemu"])
            rc = max(rc, runner([sys.executable, os.path.join(HERE, "run.py"), "--profile", "browser", "--out", out + "-browser"]))
        elif step == "machines":
            names = ["lenovo-t7", "macmini-disk"]
            mf = os.path.join(root, "machines.txt")
            if os.path.isfile(mf):
                names = [l.strip() for l in open(mf) if l.strip() and not l.startswith("#")]
            for n in names:   # one at a time; a machine at its sign-in screen comes back BLOCKED from run.py
                rc = max(rc, runner([sys.executable, os.path.join(HERE, "run.py"), "--profile", "machine", n, "--out", os.path.join(root, "%s-%s" % (day, n))]))
        elif step == "triage":
            res, _ = R.load_results(root)
            t = triage(res, datetime.datetime.now())
            for p in write_triage(t, drafts_dir, os.path.join(root, "triage", day), day):
                log("wrote " + p)
        open(os.path.join(marks, step + ".DONE"), "w").write("rc %d\n" % rc)
        return rc
    finally:
        lk.close()


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[1])
    ap.add_argument("cmd", choices=["plan", "run", "triage", "install"])
    ap.add_argument("step", nargs="?", choices=[t[0] for t in TIMELINE])
    ap.add_argument("--image")
    ap.add_argument("--results-root", default=RESULTS_ROOT)
    ap.add_argument("--drafts-dir")
    ap.add_argument("--dir")
    a = ap.parse_args(argv)
    drafts = a.drafts_dir or os.path.join(a.results_root, "triage", "known-issues-drafts")
    if a.cmd == "plan":
        print(plan_text())
        return 0
    if a.cmd == "run":
        if not a.step:
            ap.error("run needs a step")
        return run_step(a.step, a.image, a.results_root, drafts)
    if a.cmd == "triage":
        return run_step("triage", None, a.results_root, drafts)
    for n, _t, _w, _a in TIMELINE:
        svc, tim = unit_text(n)
        if a.dir:
            os.makedirs(a.dir, exist_ok=True)
            open(os.path.join(a.dir, "iyakki-autotest-%s.service" % n), "w").write(svc)
            open(os.path.join(a.dir, "iyakki-autotest-%s.timer" % n), "w").write(tim)
        else:
            print("# iyakki-autotest-%s.service\n%s\n# iyakki-autotest-%s.timer\n%s" % (n, svc, n, tim))
    print("# Nothing was enabled. To use it: copy the files to ~/.config/systemd/user/ and run systemctl --user enable --now iyakki-autotest-<step>.timer")
    return 0


if __name__ == "__main__":
    sys.exit(main())
