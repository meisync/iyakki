#!/usr/bin/env python3
"""Grade one benchmark task automatically.

    python3 docs/sdk/benchmark/grade.py T1 APP_DIR [--cargo "CMD"] [--attempts N] [--seconds S] [--out FILE.json] [--no-hidden]

APP_DIR is the app crate the model made (it holds Cargo.toml, src/, tests/). The script
  1. runs app-check (static rules and the app's own tests) and counts the problems per rule,
  2. copies the visible acceptance test and (unless --no-hidden) the hidden test into APP_DIR/tests, runs them, removes the hidden copy,
  3. prints and writes a JSON result:
     {task, pass, app_check_problems, rules: {rule: count}, accept: {passed, failed}, hidden: {passed, failed}, attempts, seconds}
PASS means: app-check reports no problem AND every acceptance and hidden test passes. `attempts` and `seconds` are what the runner measured
(number of times the model ran app-check or the tests, and wall time of the whole task); they are recorded, not judged.
--cargo runs cargo through a wrapper (a remote build host), like app-check.
"""
import json
import os
import re
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
CHECK = os.path.join(HERE, "..", "..", "..", "dist", "sdk", "app-check.py")


def opt(argv, name, default=None):
    return argv[argv.index(name) + 1] if name in argv and argv.index(name) + 1 < len(argv) else default


def counts(text):
    ok = failed = 0
    for m in re.finditer(r"test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed", text):
        ok += int(m.group(1))
        failed += int(m.group(2))
    return ok, failed


def main(argv):
    valued = ("--cargo", "--attempts", "--seconds", "--out")
    pos = [a for i, a in enumerate(argv) if not a.startswith("--") and not (i > 0 and argv[i - 1] in valued)]
    if len(pos) < 2:
        print(__doc__)
        return 2
    task, app = pos[0].upper(), os.path.abspath(pos[1])
    cargo = opt(argv, "--cargo", "cargo")
    n = task[1:].lower()
    result = {"task": task, "attempts": int(opt(argv, "--attempts", 0) or 0), "seconds": float(opt(argv, "--seconds", 0) or 0)}
    chk = subprocess.run([sys.executable, CHECK, app, "--cargo", cargo], capture_output=True, text=True)
    rules = {}
    for m in re.finditer(r"^\d+\. \S+\s+\[([a-z-]+)\]", chk.stdout, re.M):
        rules[m.group(1)] = rules.get(m.group(1), 0) + 1
    result["app_check_problems"] = sum(rules.values())
    result["rules"] = rules
    tests = os.path.join(app, "tests")
    os.makedirs(tests, exist_ok=True)
    acc = os.path.join(HERE, "acceptance", f"{task}.rs")
    hid = os.path.join(HERE, "hidden", f"{task}.rs")
    shutil.copy(acc, os.path.join(tests, f"accept_{n}.rs"))
    names = [f"accept_{n}"]
    hidden_copy = None
    if "--no-hidden" not in argv and os.path.exists(hid):
        hidden_copy = os.path.join(tests, f"hidden_{n}.rs")
        shutil.copy(hid, hidden_copy)
        names.append(f"hidden_{n}")
    out = {}
    for nm in names:
        p = subprocess.run(cargo.split() + ["test", "--test", nm], cwd=app, capture_output=True, text=True)
        ok, failed = counts(p.stdout)
        if p.returncode != 0 and failed == 0:
            failed = 1  # did not compile or crashed
        out[nm.split("_")[0]] = {"passed": ok, "failed": failed, "first_failure": next((l for l in (p.stdout + p.stderr).split("\n") if l.startswith("test ") and l.endswith("FAILED")), "") if failed else ""}
    if hidden_copy:
        os.remove(hidden_copy)
    result["accept"] = out["accept"]
    result["hidden"] = out.get("hidden")
    result["pass"] = result["app_check_problems"] == 0 and out["accept"]["failed"] == 0 and (out.get("hidden") is None or out["hidden"]["failed"] == 0) and out["accept"]["passed"] > 0
    text = json.dumps(result, indent=2)
    print(text)
    if opt(argv, "--out"):
        open(opt(argv, "--out"), "w").write(text + "\n")
    return 0 if result["pass"] else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
