#!/usr/bin/env python3
"""rewrite_services2.py RECV MEMBER REPLACEMENT [--tests]

Like rewrite_services.py but for any receiver regex (`self\\.0`, `self\\.env`, `env`, `cx2` ...) and only for non-test code: the part of a file
before its first column-0 `#[cfg(test)]` (and never a `*_tests.rs` file) unless `--tests` is given. `RECV.services.MEMBER` becomes `RECV.REPLACEMENT`;
a `()` after MEMBER is eaten when REPLACEMENT ends in `)`.

    rewrite_services2.py 'self\\.0' radio 'radio()'        radio_port.rs: self.0.services.radio.play(..) -> self.0.radio().play(..)
    rewrite_services2.py 'self\\.env' k3sdev 'k3sdev()'    shell: self.env.services.k3sdev.set_audit(..) -> self.env.k3sdev().set_audit(..)
"""
import os, re, subprocess, sys
ROOT = subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip()
UI = [ROOT + "/crates/iyakki-apps/src", ROOT + "/crates/iyakki-shell/src"]
args = [a for a in sys.argv[1:] if not a.startswith("--")]
recv, member, repl = args
tests = "--tests" in sys.argv
eat = repl.endswith(")")
pat = re.compile(r"\b(%s)\.services\.%s\b%s" % (recv, re.escape(member), r"(?:\(\))?" if eat else ""))
n = 0
for d in UI:
    for r, _, fs in os.walk(d):
        for f in fs:
            if not f.endswith(".rs"):
                continue
            if not tests and (f.endswith("_tests.rs") or f == "tests.rs" or "/tests/" in r + "/"):
                continue
            p = os.path.join(r, f)
            lines = open(p).read().split("\n")
            end = len(lines)
            if not tests:
                for i, l in enumerate(lines):
                    if l.startswith("#[cfg(test)]"):
                        end = i
                        break
            changed = False
            for i in range(end):
                t = pat.sub(lambda m: "%s.%s" % (m.group(1), repl), lines[i])
                if t != lines[i]:
                    lines[i] = t
                    changed = True
            if changed:
                open(p, "w").write("\n".join(lines))
                n += 1
print("rewrote %d files" % n)
