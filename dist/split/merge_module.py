#!/usr/bin/env python3
"""Pass 3 of the UI/agent split: merge a WHOLE agent module into the module of the same name that already lives in iyakki-shared
(the types moved there by move_types.py), so the UI can name `iyakki_shared::M` for everything in it.

    python3 dist/split/merge_module.py M [M ...]

The agent file is appended to the shared file (its `pub use iyakki_shared::M::...;` lines and duplicate `use` lines dropped, `crate::clock`
becomes `iyakki_clock`, `pub(crate)` becomes `pub`), the agent keeps `pub use iyakki_shared::M;`, and UI paths `iyakki_agent::M` become
`iyakki_shared::M`. The caller compiles and takes a module back with `git checkout -- crates` + `git clean` of nothing else (use a commit first).
"""
import os, re, subprocess, sys
ROOT = subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip()
AG, SH = ROOT + "/crates/iyakki-agent/src/", ROOT + "/crates/iyakki-shared/src/"
UI = [ROOT + "/crates/iyakki-apps/src", ROOT + "/crates/iyakki-shell/src"]

def imported(line):
    """The local names a one-line `use` brings in."""
    line = re.sub(r"^(pub(\([a-z]+\))? )?use ", "", line.strip()).rstrip(";")
    names = set()
    m = re.match(r"([\w:]+)::\{(.*)\}$", line)
    if m:
        for it in m.group(2).split(","):
            it = it.strip()
            if it and it != "*":
                names.add(m.group(1).split("::")[-1] if it == "self" else it.split(" as ")[-1].strip())
    elif not line.endswith("*"):
        names.add(line.split(" as ")[-1].split("::")[-1].strip())
    return names


def merge(m):
    a, s = AG + m + ".rs", SH + m + ".rs"
    A = open(a).read()
    S = open(s).read() if os.path.exists(s) else None
    if os.path.isdir(AG + m):
        os.rename(AG + m, SH + m)
    A = re.sub(r"^pub use iyakki_shared::%s::(\{[^}]*\}|\w+|\*);\n" % m, "", A, flags=re.M)
    A = A.replace("crate::clock::", "iyakki_clock::").replace("pub(crate)", "pub")
    have = set(l.strip() for l in (S or "").split("\n") if l.startswith("use "))
    seen = set()
    for l in (S or "").split("\n"):
        if l.startswith("use ") or l.startswith("pub use "):
            seen |= imported(l)
    out = []
    for l in A.split("\n"):
        if l.startswith("use ") and l.rstrip().endswith(";"):
            names = imported(l)
            if names and names <= seen:
                continue
            m2 = re.match(r"use ([\w:]+)::\{([^}]*)\};", l.strip())
            if m2 and names & seen:
                keep = [i.strip() for i in m2.group(2).split(",") if i.strip() and (i.strip().split(" as ")[-1].strip() not in seen)]
                l = ("use %s::{%s};" % (m2.group(1), ", ".join(keep))) if keep else ""
            elif names & seen:
                l = ""
        out.append(l)
    A = "\n".join(out)
    # inner doc lines of the agent file (//!) stay as plain comments
    A = re.sub(r"^//!", "//", A, flags=re.M)
    for tf in re.findall(r'#\[path = "([^"]+)"\]', A):
        if os.path.exists(AG + tf):
            os.rename(AG + tf, SH + tf)
    if S is None:
        open(s, "w").write(A)
        t = open(SH + "lib.rs").read().rstrip("\n") + "\npub mod %s;\n" % m
        open(SH + "lib.rs", "w").write(t)
    else:
        open(s, "w").write(S.rstrip("\n") + "\n\n// ---- merged from iyakki-agent (F-SPLIT-03) ----\n" + A)
    os.remove(a)
    p = ROOT + "/crates/iyakki-agent/src/lib.rs"
    t = open(p).read()
    t2 = re.sub(r"pub mod %s;" % m, "pub use iyakki_shared::%s;" % m, t, count=1)
    if t2 == t:
        print("lib.rs has no `pub mod %s;`" % m)
    open(p, "w").write(t2)
    for d in UI:
        for r, _, fs in os.walk(d):
            for f in fs:
                if f.endswith(".rs"):
                    q = os.path.join(r, f)
                    x = open(q).read()
                    y = re.sub(r"\biyakki_agent::%s\b" % m, "iyakki_shared::%s" % m, x)
                    if y != x:
                        open(q, "w").write(y)

if __name__ == "__main__":
    for m in sys.argv[1:]:
        merge(m)
        print("merged", m)
