#!/usr/bin/env python3
"""Run move_types.py over modules, compile iyakki-shared, and for every error blame the moved ITEM that holds the line: it goes to
dist/split/excluded.txt (stays in the agent), its module is taken back and moved again without it. Repeats until iyakki-shared compiles.

    RCARGO="path/to/rcargo $PWD" python3 dist/split/pass1.py MODULE...
"""
import os, re, subprocess, sys
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import move_types as mt
ROOT = mt.ROOT
RCARGO = os.environ.get("RCARGO", "cargo").split()
mods = [a for a in sys.argv[1:] if not a.startswith("--")]
subprocess.check_call([sys.executable, HERE + "/move_types.py"] + mods, cwd=ROOT)


def blame(path, line):
    text = open(path).read()
    its, tl, sl = mt.items(text)
    for hs, e, head in its:
        if hs <= line - 1 <= e:
            k, n = mt.classify(head)
            if k == "use":
                return None
            if k == "impl":
                sty, _ = mt.impl_self("\n".join(sl[hs:e + 1]).split("{")[0])
                return sty
            return n
    return None


for rnd in range(40):
    p = subprocess.run(RCARGO + ["check", "-p", "iyakki-shared", "--tests", "--message-format=short"], cwd=ROOT, capture_output=True, text=True)
    out = p.stdout + p.stderr
    if p.returncode == 0:
        print("shared compiles after", rnd, "rounds")
        break
    hits = sorted(set(re.findall(r"crates/iyakki-shared/src/(\w+)\.rs:(\d+)", out)))
    hits = [(m, int(l)) for m, l in hits if m != "lib"]
    if not hits:
        print(out[-3000:])
        sys.exit("no moved item can be blamed")
    redo = {}
    for m, l in hits:
        n = blame("%s/crates/iyakki-shared/src/%s.rs" % (ROOT, m), l)
        redo.setdefault(m, set()).add(n)
    with open(mt.EXCL, "a") as f:
        for m, ns in redo.items():
            for n in sorted(x for x in ns if x):
                f.write("%s %s\n" % (m, n))
    print("excluding", {m: sorted(x for x in n if x) for m, n in redo.items()})
    subprocess.check_call([sys.executable, HERE + "/move_types.py", "--undo"] + sorted(redo), cwd=ROOT)
    subprocess.check_call([sys.executable, HERE + "/move_types.py", "--keep-last"] + sorted(redo), cwd=ROOT)
