#!/usr/bin/env python3
"""Compile-revert loop for gen_facade.py --rewrite: compile the workspace, put back every rewritten line that has a compile error, repeat.

    RCARGO="path/to/rcargo $PWD" python3 dist/split/revert_failed.py
"""
import json, os, re, subprocess, sys
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip()
RC = os.environ.get("RCARGO", "cargo").split()
BK = HERE + "/.facade-backup.json"
for rnd in range(12):
    p = subprocess.run(RC + ["check", "--workspace", "--tests", "--message-format=short"], cwd=ROOT, capture_output=True, text=True)
    out = p.stdout + p.stderr
    if p.returncode == 0:
        print("compiles after", rnd, "rounds")
        break
    backup = json.load(open(BK))
    errs = set(re.findall(r"^(crates/[^:]+\.rs):(\d+):\d+: error", out, re.M))
    fixed = 0
    for f, ln in errs:
        fp = os.path.join(ROOT, f)
        cand = [ln] if ln in backup.get(fp, {}) else [str(x) for x in range(int(ln) - 40, int(ln) + 60) if str(x) in backup.get(fp, {})]
        for l2 in cand:
            orig = backup[fp].pop(l2)
            lines = open(fp).read().split("\n")
            lines[int(l2) - 1] = orig
            open(fp, "w").write("\n".join(lines))
            fixed += 1
    json.dump(backup, open(BK, "w"))
    print("round", rnd, "errors", len(errs), "reverted", fixed)
    if not fixed:
        print(out[-3000:])
        sys.exit("errors that are not on rewritten lines")
