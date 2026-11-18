#!/usr/bin/env python3
"""NOTE: first run moved only dash::frame, flows::programs, extagent::relay and none compiled cleanly (inherent impls of the moved types in sibling files, the log crate): kept for reference, not part of the ratchet work.
Pass 1b of the UI/agent split: move whole PURE SUBMODULES of directory modules (`agent/src/M/S.rs`) into iyakki-shared as `M_S`.

    python3 dist/split/move_submods.py M [M ...] [--dry]      (--undo M_S to take one back)

A submodule moves when every `crate::X` in it is a whole module of iyakki-shared (or a sibling that moves too), every `super::X` is a sibling
that moves too, and it names no agent-only item. In the agent, `pub mod S;` in `M/mod.rs` becomes `pub use iyakki_shared::M_S as S;` (the agent's own
paths keep working), its test file (`#[path]`) moves along, and the UI paths `iyakki_agent::M::S::` become `iyakki_shared::M_S::`.
"""
import os, re, subprocess, sys
ROOT = subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip()
AG = ROOT + "/crates/iyakki-agent/src/"
SH = ROOT + "/crates/iyakki-shared/src/"
UI = [ROOT + "/crates/iyakki-apps/src", ROOT + "/crates/iyakki-shell/src"]
EXT_OK = {"std", "core", "alloc", "serde", "serde_json", "iyakki_sec", "iyakki_schema", "iyakki_clock", "iyakki_core_api", "iyakki_storage", "iyakki_netscan",
          "iyakki_devctl", "iyakki_proxy", "iyakki_wasm", "iyakki_filescope", "iyakki_cloudmode", "iyakki_shared", "sha2", "zeroize", "thiserror", "ed25519_dalek", "argon2", "chacha20poly1305"}


def whole_shared():
    return set(re.findall(r"^pub use iyakki_shared::(\w+);", open(AG + "lib.rs").read(), re.M))


def body(text):
    i = text.find("#[cfg(test)]")
    b = text if i < 0 else text[:i]
    return "\n".join(l for l in b.split("\n") if not l.strip().startswith("//"))


def analyse(M, subs, shared):
    info = {}
    for s in subs:
        p = AG + "%s/%s.rs" % (M, s)
        if not os.path.exists(p):
            continue
        t = open(p).read()
        b = body(t)
        info[s] = dict(crate=set(re.findall(r"crate::(\w+)(?:::(\w+))?", b)), sup=set(re.findall(r"super::(\w+)", b)),
                       ext=set(re.findall(r"(?<![\w:])(iyakki_\w+)::", b)), text=t)
    ok = set(info)
    changed = True
    while changed:
        changed = False
        for s in list(ok):
            i = info[s]
            bad = False
            for a, b2 in i["crate"]:
                if a == M and b2 in ok:
                    continue
                if a in shared:
                    continue
                bad = True
            for x in i["sup"]:
                if x not in ok and x != s:
                    bad = True
            if i["ext"] - EXT_OK or re.search(r"super::super|use super::\*", body(i["text"])):
                bad = True
            if bad:
                ok.discard(s)
                changed = True
    return ok, info


def ui_subs(M):
    out = set()
    for d in UI:
        for r, _, fs in os.walk(d):
            for f in fs:
                if f.endswith(".rs") and not f.endswith("_tests.rs") and f != "tests.rs":
                    t = open(os.path.join(r, f)).read()
                    m = re.search(r"#\[cfg\(test\)\]\s*(pub(\([a-z]+\))? )?mod ", t)
                    for mm in re.finditer(r"iyakki_agent::%s::(\w+)::" % M, t[:m.start()] if m else t):
                        out.add(mm.group(1))
                    for mm in re.finditer(r"iyakki_agent::%s::(\w+)\b" % M, t[:m.start()] if m else t):
                        out.add(mm.group(1))
    return out


def move(M, s, info, dry):
    name = "%s_%s" % (M, s)
    t = info[s]["text"]
    files = [s + ".rs"] + re.findall(r'#\[path = "([^"]+)"\]', t)
    if dry:
        print("would move", M, s)
        return
    for f in files:
        subprocess.check_call(["git", "mv", AG + M + "/" + f, SH + (name + ".rs" if f == s + ".rs" else M + "_" + f)])
    txt = open(SH + name + ".rs").read()
    for sib in set(re.findall(r"super::(\w+)", txt)):
        txt = re.sub(r"\bsuper::%s\b" % sib, "crate::%s_%s" % (M, sib), txt)
    txt = re.sub(r"crate::%s::(\w+)" % re.escape(M), lambda m: "crate::%s_%s" % (M, m.group(1)), txt)
    # path attributes of the test files follow
    for f in files[1:]:
        txt = txt.replace('#[path = "%s"]' % f, '#[path = "%s_%s"]' % (M, f))
    open(SH + name + ".rs", "w").write(txt)
    lib = open(SH + "lib.rs").read() + "pub mod %s;\n" % name
    open(SH + "lib.rs", "w").write(lib)
    mp = AG + M + "/mod.rs"
    mt = open(mp).read()
    mt = re.sub(r"^(\s*)pub mod %s;" % re.escape(s), r"\1pub use iyakki_shared::%s as %s;" % (name, s), mt, flags=re.M)
    open(mp, "w").write(mt)
    for d in UI:
        for r, _, fs in os.walk(d):
            for f in fs:
                if f.endswith(".rs"):
                    fp = os.path.join(r, f)
                    x = open(fp).read()
                    y = re.sub(r"\biyakki_agent::%s::%s\b" % (re.escape(M), re.escape(s)), "iyakki_shared::%s" % name, x)
                    if y != x:
                        open(fp, "w").write(y)


def undo(name):
    M, s = name.split("_", 1)
    subprocess.call(["git", "checkout", "--", AG + M + "/mod.rs"], cwd=ROOT)
    for f in os.listdir(SH):
        if f == name + ".rs" or f.startswith(M + "_" + s + "_"):
            orig = f.replace(M + "_", "", 1) if f != name + ".rs" else s + ".rs"
            subprocess.call(["git", "mv", SH + f, AG + M + "/" + orig])
    lib = open(SH + "lib.rs").read().replace("pub mod %s;\n" % name, "")
    open(SH + "lib.rs", "w").write(lib)
    for d in UI:
        for r, _, fs in os.walk(d):
            for f in fs:
                if f.endswith(".rs"):
                    fp = os.path.join(r, f)
                    x = open(fp).read()
                    y = re.sub(r"\biyakki_shared::%s\b" % re.escape(name), "iyakki_agent::%s::%s" % (M, s), x)
                    if y != x:
                        open(fp, "w").write(y)


if __name__ == "__main__":
    if "--undo" in sys.argv:
        for n in [a for a in sys.argv[1:] if not a.startswith("--")]:
            undo(n)
        sys.exit(0)
    dry = "--dry" in sys.argv
    shared = whole_shared()
    for M in [a for a in sys.argv[1:] if not a.startswith("--")]:
        mp = AG + M + "/mod.rs"
        subs = re.findall(r"^pub mod (\w+);", open(mp).read(), re.M)
        ok, info = analyse(M, subs, shared)
        want = ui_subs(M)
        take = {s for s in ok if s in want}
        # a taken submodule needs its movable siblings it names
        stack = list(take)
        while stack:
            s = stack.pop()
            for a, b2 in info[s]["crate"]:
                if a == M and b2 in ok and b2 not in take:
                    take.add(b2)
                    stack.append(b2)
            for x in info[s]["sup"]:
                if x in ok and x not in take:
                    take.add(x)
                    stack.append(x)
        for s in sorted(take):
            move(M, s, info, dry)
            print(M, s, "moved" if not dry else "")
