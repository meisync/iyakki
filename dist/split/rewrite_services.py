#!/usr/bin/env python3
"""Pass 6 of the UI/agent split: rewrite one read of the agent's `Services` in the UI to a facet or accessor, one member at a time.

    python3 dist/split/rewrite_services.py MEMBER REPLACEMENT [--recv cx] [--tests]

`RECV.services.MEMBER` becomes `RECV.REPLACEMENT` for the receivers `cx`, `self.cx`, `w.cx` (any `<word>.cx` or `cx`) in the UI crates
(iyakki-apps, iyakki-shell). Examples:

    rewrite_services.py dir 'data_dir()'            cx.services.dir()      -> cx.data_dir()      (a `()` after MEMBER is eaten when REPLACEMENT ends in `)`)
    rewrite_services.py inbox 'inbox()'             cx.services.inbox.add  -> cx.inbox().add

A line that no longer compiles is put back with `dist/split/revert_failed.py` (compile first, run it with the compiler output). `--tests` includes
`*_tests.rs` files. Idempotent: running it twice changes nothing.
"""
import os, re, subprocess, sys
ROOT = subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip()
UI = [ROOT + "/crates/iyakki-apps/src", ROOT + "/crates/iyakki-shell/src"]

def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    member, repl = args[0], args[1]
    tests = "--tests" in sys.argv
    eat_call = repl.endswith(")")
    pat = re.compile(r"((?:\b\w+\.)?\bcx)\.services\.%s\b%s" % (re.escape(member), r"(?:\(\))?" if eat_call else ""))
    n = 0
    for d in UI:
        for r, _, fs in os.walk(d):
            for f in fs:
                if not f.endswith(".rs"):
                    continue
                if not tests and (f.endswith("_tests.rs") or f == "tests.rs" or "/tests/" in r + "/"):
                    continue
                p = os.path.join(r, f)
                s = open(p).read()
                t = pat.sub(lambda m: "%s.%s" % (m.group(1), repl), s)
                if t != s:
                    open(p, "w").write(t)
                    n += 1
    print("rewrote %d files" % n)

main()
