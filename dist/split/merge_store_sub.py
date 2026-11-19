#!/usr/bin/env python3
"""Pass 4 of the UI/agent split: merge a SUBMODULE of the agent's `store` (crates/iyakki-agent/src/store/X.rs) into iyakki-shared as `store_X`
(appended when `store_X.rs` exists already). `super::Y` becomes `crate::store_Y`, `crate::store::Y` the same, `crate::clock::` becomes
`iyakki_clock::`. In `store/mod.rs`, `pub mod X;` becomes `pub use iyakki_shared::store_X as X;`; UI paths `iyakki_agent::store::X` become
`iyakki_shared::store_X`. The caller compiles and puts a module back (git checkout + clean) when it needs agent-only items.

    python3 dist/split/merge_store_sub.py X [X ...]
"""
import os, re, subprocess, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import merge_module as mm
ROOT, AG, SH, UI = mm.ROOT, mm.AG, mm.SH, mm.UI

def merge(x):
    a, s = AG + "store/%s.rs" % x, SH + "store_%s.rs" % x
    A = open(a).read()
    S = open(s).read() if os.path.exists(s) else None
    A = re.sub(r"^pub use iyakki_shared::store_%s::(\{[^}]*\}|\w+|\*);\n" % x, "", A, flags=re.M)
    A = A.replace("crate::clock::", "iyakki_clock::").replace("pub(crate)", "pub")
    A = re.sub(r"\bsuper::super::", "crate::store_", A)
    A = re.sub(r"\bsuper::([a-z_0-9]+)\b", r"crate::store_\1", A)
    A = re.sub(r"\bcrate::store::([a-z_0-9]+)\b", r"crate::store_\1", A)
    A = A.replace("iyakki_shared::", "crate::").replace("../../../../", "../../../")
    seen = set()
    for l in (S or "").split("\n"):
        if l.startswith("use ") or l.startswith("pub use "):
            seen |= mm.imported(l)
    out = []
    for l in A.split("\n"):
        if l.startswith("use ") and l.rstrip().endswith(";"):
            names = mm.imported(l)
            if names and names <= seen:
                continue
            m2 = re.match(r"use ([\w:]+)::\{([^}]*)\};", l.strip())
            if m2 and names & seen:
                keep = [i.strip() for i in m2.group(2).split(",") if i.strip() and (i.strip().split(" as ")[-1].strip() not in seen)]
                l = ("use %s::{%s};" % (m2.group(1), ", ".join(keep))) if keep else ""
            elif names & seen:
                l = ""
        out.append(l)
    A = re.sub(r"^//!", "//", "\n".join(out), flags=re.M)
    for tf in re.findall(r'#\[path = "([^"]+)"\]', A):
        if os.path.exists(AG + "store/" + tf):
            os.rename(AG + "store/" + tf, SH + "store_" + tf)
            A = A.replace('#[path = "%s"]' % tf, '#[path = "store_%s"]' % tf)
    if S is None:
        open(s, "w").write(A)
        open(SH + "lib.rs", "a").write("pub mod store_%s;\n" % x)
    else:
        open(s, "w").write(S.rstrip("\n") + "\n\n// ---- merged from iyakki-agent (F-SPLIT-03) ----\n" + A)
    os.remove(a)
    p = AG + "store/mod.rs"
    t = open(p).read()
    t = re.sub(r"pub mod %s;" % x, "pub use iyakki_shared::store_%s as %s;" % (x, x), t, count=1)
    open(p, "w").write(t)
    for d in UI:
        for r, _, fs in os.walk(d):
            for f in fs:
                if f.endswith(".rs"):
                    q = os.path.join(r, f)
                    y0 = open(q).read()
                    y = re.sub(r"\biyakki_agent::store::%s\b" % x, "iyakki_shared::store_%s" % x, y0)
                    if y != y0:
                        open(q, "w").write(y)

if __name__ == "__main__":
    for x in sys.argv[1:]:
        merge(x)
        print("merged store/" + x)
