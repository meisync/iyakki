#!/usr/bin/env python3
"""Pass 5 of the UI/agent split: move NAMED top-level items (struct, enum, type, const) of an agent file, with their `impl` blocks, into a shared module.

    python3 dist/split/move_items.py agent/store/lifecycle.rs store_lifecycle Name1,Name2 [--ui iyakki_agent::store::lifecycle]

The shared module `iyakki-shared/src/<shared>.rs` is appended to (or created); the agent file gets `pub use iyakki_shared::<shared>::{names};`.
`use` lines the fragment needs are copied (`super::X` and `crate::store::X` become `crate::store_X`, `crate::clock::` `iyakki_clock::`).
With --ui, UI paths `<ui>::Name` become `iyakki_shared::<shared>::Name`. The caller compiles and fixes what is agent-only.
"""
import os, re, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import move_types as mt
ROOT, AG, SH, UI = mt.ROOT, mt.AG, mt.SH, mt.UI

def main():
    rel, shared, names = sys.argv[1], sys.argv[2], set(sys.argv[3].split(","))
    ui = sys.argv[sys.argv.index("--ui") + 1] if "--ui" in sys.argv else None
    path = AG + rel.replace("agent/", "", 1) if rel.startswith("agent/") else AG + rel
    text = open(path).read()
    its, tl, sl = mt.items(text)
    take, uses = [], []
    for i, (hs, e, head) in enumerate(its):
        k, n = mt.classify(head)
        if k in ("struct", "enum", "type", "const", "static", "fn") and n in names:
            take.append(i)
        elif k == "impl":
            sty, tr = mt.impl_self("\n".join(tl[hs:e + 1]).split("{")[0] if False else head)
            if sty in names:
                take.append(i)
        elif k == "use":
            uses.append(i)
    if not take:
        print("nothing to move"); return
    frag = mt.pubify("\n\n".join("\n".join(tl[its[i][0]:its[i][1] + 1]) for i in take))
    ustmts = []
    for i in uses:
        blk = "\n".join(tl[its[i][0]:its[i][1] + 1])
        if "*" in blk or "pub use" in blk:
            continue
        origin = re.sub(r"^\s*(pub(\([a-z]+\))?\s+)?use\s+", "", blk.strip()).rstrip(";")
        origin = re.sub(r"\s+", " ", origin)
        pairs = []
        try:
            pairs = mt.expand_use(origin)
        except ValueError:
            continue
        for pth, nm in pairs:
            if nm == "self" or not re.search(r"(?<![\w.])%s\b" % re.escape(nm), frag):
                continue
            pth = pth.replace("iyakki_shared::", "crate::")
            pth = re.sub(r"^(?:crate::store|super)::([a-z_0-9]+)", r"crate::store_\1", pth)
            pth = pth.replace("crate::clock::", "iyakki_clock::")
            last = pth.split("::")[-1]
            ustmts.append("use %s;" % pth if last == nm else "use %s as %s;" % (pth, nm))
    frag = re.sub(r"\bsuper::([a-z_0-9]+)\b", r"crate::store_\1", frag).replace("crate::clock::", "iyakki_clock::")
    sp = SH + shared + ".rs"
    new = "\n".join(sorted(set(ustmts))) + "\n\n" + frag + "\n"
    if os.path.exists(sp):
        cur = open(sp).read()
        have = set(l.strip() for l in cur.split("\n") if l.startswith("use "))
        new = "\n".join(l for l in new.split("\n") if l.strip() not in have)
        open(sp, "w").write(cur.rstrip("\n") + "\n\n// ---- moved from iyakki-agent (F-SPLIT-03) ----\n" + new)
    else:
        open(sp, "w").write(new)
        open(SH + "lib.rs", "a").write("pub mod %s;\n" % shared)
    out = [l for j, l in enumerate(tl)]
    drop = set()
    for i in take:
        hs, e, _ = its[i]
        drop.update(range(hs, e + 1))
    rest = [l for j, l in enumerate(tl) if j not in drop]
    reexp = "pub use iyakki_shared::%s::{%s};\n" % (shared, ", ".join(sorted(n for n in names if any(mt.classify(its[i][2])[1] == n for i in take))))
    first_use = next((j for j, l in enumerate(rest) if l.startswith("use ")), 0)
    rest.insert(first_use, reexp.rstrip("\n"))
    open(path, "w").write("\n".join(rest))
    if ui:
        for d in UI:
            for r, _, fs in os.walk(d):
                for f in fs:
                    if f.endswith(".rs"):
                        q = os.path.join(r, f); t = open(q).read(); o = t
                        for n in names:
                            t = re.sub(r"\b%s::%s\b" % (re.escape(ui), n), "iyakki_shared::%s::%s" % (shared, n), t)
                        if t != o:
                            open(q, "w").write(t)
    print("moved", sorted(names), "->", shared)

main()
