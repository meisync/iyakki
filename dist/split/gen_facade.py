#!/usr/bin/env python3
"""Pass 2 of the UI/agent split (docs/development/UI-BOUNDARY.md, section 11): a generated facade for the FUNCTIONS the UI calls.

    python3 dist/split/gen_facade.py [--dry] [--rewrite]

1. Scans the UI (iyakki-apps, iyakki-shell, non-test code) for `iyakki_agent::MODULE::function(` calls.
2. Finds `pub fn function` in `crates/iyakki-agent/src/MODULE.rs`, parses its signature and keeps it when every type in it is std, a prelude type,
   or a type that lives in iyakki-shared, and the result has a sensible default (so a core without the facet answers "unavailable").
3. Writes `crates/iyakki-agent-api/src/generated.rs`: one trait per domain (`AgentSettings`, `AgentVault`, ...), each method `MODULE_function` with a
   default body, an `AgentApi` super trait, and `FakeAgentApi` (every call recorded, defaults answered).
4. Writes `crates/iyakki-agent/src/agentapi_gen.rs`: `AgentApiImpl`, the delegating implementation.
5. With `--rewrite`: calls on a line that mentions `cx` become `cx.agent.MODULE_function(`. The caller then compiles and runs
   `dist/split/revert_failed.py` to put back every line that does not compile.
"""
import json, os, re, subprocess, sys

ROOT = subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip()
AG = ROOT + "/crates/iyakki-agent/src/"
SH = ROOT + "/crates/iyakki-shared/src/"
API = ROOT + "/crates/iyakki-agent-api/"
UI = [ROOT + "/crates/iyakki-apps/src", ROOT + "/crates/iyakki-shell/src"]
BACKUP = os.path.join(os.path.dirname(os.path.abspath(__file__)), ".facade-backup.json")

PRELUDE = set("String Vec Option Result Box Self bool str u8 u16 u32 u64 u128 usize i8 i16 i32 i64 i128 isize f32 f64 char Some None Ok Err".split())
STD_OK = {"std", "core", "alloc"}


def domain_map():
    src = open(ROOT + "/crates/iyakki-core/tests/ui_agent_uses.rs").read()
    body = src[src.index("fn domain_of"):]
    body = body[:body.index("\n}\n")]
    out = {}
    for m in re.finditer(r'((?:"\w+"\s*\|\s*)*"\w+")\s*=>\s*"(\w+)"', body):
        for mod in re.findall(r'"(\w+)"', m.group(1)):
            out[mod] = m.group(2)
    return out


def ui_files():
    for d in UI:
        for r, _, fs in os.walk(d):
            for f in fs:
                if f.endswith(".rs") and not f.endswith("_tests.rs") and f != "tests.rs":
                    yield os.path.join(r, f)


def non_test(t):
    m = re.search(r"#\[cfg\(test\)\]\s*(pub(\([a-z]+\))? )?mod ", t)
    return t[:m.start()] if m else t


def used_calls():
    cnt = {}
    for f in ui_files():
        t = non_test(open(f).read())
        for ln in t.split("\n"):
            if ln.strip().startswith("//"):
                continue
            for m in re.finditer(r"iyakki_agent::(\w+)::([a-z_]\w*)\(", ln):
                cnt[(m.group(1), m.group(2))] = cnt.get((m.group(1), m.group(2)), 0) + 1
    return cnt


def shared_defs():
    """ident -> set of shared modules that define it at top level."""
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import move_types as mt
    defs = {}
    for fn in os.listdir(SH):
        if fn.endswith(".rs") and fn != "lib.rs":
            mod = fn[:-3]
            for n in mt.shared_names(mod):
                defs.setdefault(n, set()).add(mod)
    return defs


def split_top(s, sep=","):
    out, d, cur = [], 0, ""
    for ch in s:
        if ch in "<([{":
            d += 1
        elif ch in ">)]}":
            d -= 1 if not (ch == ">" and cur.endswith("-")) else 0
        if ch == sep and d == 0:
            out.append(cur)
            cur = ""
        else:
            cur += ch
    if cur.strip():
        out.append(cur)
    return out


def parse_fn(text, name):
    m = re.search(r"^pub fn %s\b" % re.escape(name), text, re.M)
    if not m:
        return None
    i = m.end()
    if text[i:i + 1] == "<":
        return None  # generic
    if text[i] != "(":
        return None
    d = 0
    j = i
    while j < len(text):
        if text[j] == "(":
            d += 1
        elif text[j] == ")":
            d -= 1
            if d == 0:
                break
        j += 1
    params = text[i + 1:j]
    rest = text[j + 1:]
    k = re.search(r"\{|;|\bwhere\b", rest)
    ret = rest[:k.start()].strip() if k else ""
    if k and rest[k.start():].startswith("where"):
        return None
    if ret.startswith("->"):
        ret = ret[2:].strip()
    else:
        ret = ""
    ps = []
    for p in split_top(params):
        p = p.strip()
        if not p:
            continue
        if "self" in p.split(":")[0]:
            return None
        mm = re.match(r"^(mut\s+)?(\w+)\s*:\s*(.+)$", p, re.S)
        if not mm:
            return None
        ps.append((mm.group(2), re.sub(r"\s+", " ", mm.group(3).strip()), bool(mm.group(1))))
    return ps, re.sub(r"\s+", " ", ret)


def default_of(ret):
    r = ret.replace(" ", "")
    if r == "":
        return "()"
    if r == "String":
        return "String::new()"
    if r == "bool":
        return "false"
    if r.startswith("Option<"):
        return "None"
    if r.startswith("Vec<"):
        return "Vec::new()"
    if re.match(r"Result<.*,String>$", r):
        return 'Err("unavailable: this core does not serve this call".into())'
    if r in ("u8", "u16", "u32", "u64", "usize", "i32", "i64", "isize"):
        return "0"
    if r in ("f32", "f64"):
        return "0.0"
    if r in ("std::path::PathBuf", "PathBuf"):
        return "std::path::PathBuf::new()"
    if r.startswith("std::collections::HashMap<") or r.startswith("std::collections::BTreeMap<"):
        return "Default::default()"
    if r.startswith("(") and r.endswith(")"):
        return "Default::default()"
    return None


def qualify(tyx, modname, usemap, defs, shared_here):
    """Rewrite the type idents of a signature to paths visible from iyakki-agent-api; None when one cannot be."""
    bad = False

    def rep(m):
        nonlocal bad
        n = m.group(0)
        if n in PRELUDE:
            return n
        o = usemap.get(n)
        if o:
            root = o.split("::")[0]
            if root in STD_OK:
                return o
            if root == "iyakki_shared":
                return o
            if root == "crate":
                parts = o.split("::")
                if len(parts) >= 3 and n in defs and parts[1] in defs[n]:
                    return "iyakki_shared::%s::%s" % (parts[1], n)
                if len(parts) >= 3 and parts[1] in defs.get(n, ()):
                    return "iyakki_shared::%s::%s" % (parts[1], n)
            if root in ("serde_json",):
                return o
            bad = True
            return n
        if n in shared_here:
            return "iyakki_shared::%s::%s" % (modname, n)
        bad = True
        return n

    out = re.sub(r"(?<![\w:'])[A-Z][A-Za-z0-9_]*\b", rep, tyx)
    # paths written in full: only std, iyakki_shared, iyakki_sec and serde_json may start one
    for m in re.finditer(r"(?<![\w:])([a-z_][a-z0-9_]*)::", out):
        if m.group(1) not in ("std", "core", "alloc", "iyakki_shared", "iyakki_sec", "serde_json"):
            bad = True
    if "crate::" in out or "super::" in out:
        bad = True
    return None if bad else out


def use_map(text):
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import move_types as mt
    um = {}
    for ln in re.finditer(r"^(?:pub(?:\([a-z]+\))?\s+)?use\s+([^;]+);", text, re.M):
        try:
            for p, n in mt.expand_use(re.sub(r"\s+", " ", ln.group(1))):
                um[n] = p
        except ValueError:
            pass
    return um


def camel(s):
    return "".join(w.capitalize() for w in s.split("_"))


def gen(dry, rewrite):
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import move_types as mt
    calls = used_calls()
    KEYS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "facade-keys.json")
    for mod_, fn_ in (json.load(open(KEYS)) if os.path.exists(KEYS) else []):
        calls.setdefault((mod_, fn_), 0)  # what an earlier run facaded stays in the facade
    defs = shared_defs()
    dom = domain_map()
    keep = {}
    skipped = {}
    for (mod, fn), n in sorted(calls.items()):
        path = AG + mod + ".rs"
        if not os.path.exists(path):
            skipped[(mod, fn)] = "no flat file"
            continue
        text = open(path).read()
        sig = parse_fn(non_test(text), fn)
        if not sig:
            skipped[(mod, fn)] = "no simple pub fn"
            continue
        ps, ret = sig
        um = use_map(text)
        shared_here = mt.shared_names(mod)
        qp = []
        ok = True
        for nm, ty, m_ in ps:
            q = qualify(ty, mod, um, defs, shared_here)
            if q is None or "impl " in q or "dyn " in q.replace("&dyn Fn", "").replace("&mut dyn Fn", "") or "'" in q.replace("'static", ""):
                ok = False
                break
            qp.append((nm, q, m_))
        qr = qualify(ret, mod, um, defs, shared_here) if ret else ""
        if not ok or qr is None or "impl " in (qr or "") or "&" in (qr or ""):
            skipped[(mod, fn)] = "type outside std/shared"
            continue
        d = default_of(qr)
        if d is None:
            skipped[(mod, fn)] = "no default for " + qr
            continue
        keep[(mod, fn)] = (qp, qr, d, n)
    print("functions used:", len(calls), "facaded:", len(keep), "uses covered:", sum(v[3] for v in keep.values()), "of", sum(calls.values()))
    if dry:
        why = {}
        for k, w in skipped.items():
            why[w.split(" for ")[0]] = why.get(w.split(" for ")[0], 0) + calls[k]
        print("skipped by reason (uses):", why)
        return
    json.dump(sorted(keep), open(KEYS, "w"))
    bydom = {}
    for (mod, fn), v in keep.items():
        bydom.setdefault(dom.get(mod, "other"), []).append((mod, fn, v))
    os.makedirs(API + "src", exist_ok=True)
    g = ["//! GENERATED by dist/split/gen_facade.py (F-SPLIT-03, pass 2). Do not edit: change the script or the agent function and run it again.", "#![allow(clippy::all)]", ""]
    traits = []
    for d, lst in sorted(bydom.items()):
        tn = "Agent" + camel(d)
        traits.append(tn)
        g.append("/// The `%s` functions of the agent that the UI calls (generated). Every method defaults to an \"unavailable\" answer." % d)
        g.append("pub trait %s: Send + Sync {" % tn)
        for mod, fn, (qp, qr, dflt, n) in sorted(lst):
            args = ", ".join("%s%s: %s" % ("mut " if m_ else "", nm, ty) for nm, ty, m_ in qp)
            g.append("    fn %s_%s(&self%s)%s {" % (mod, fn, (", " + args) if args else "", (" -> " + qr) if qr else ""))
            g.append("        %s" % ("let _ = (%s,);\n        " % ", ".join(nm for nm, _, _ in qp) if qp else "") + dflt)
            g.append("    }")
        g.append("}")
        g.append("")
    g.append("/// Everything the UI may ask of the agent's functions, in one handle (`cx.agent`).")
    g.append("pub trait AgentApi: %s + crate::facets::AgentFacets + crate::facets_b3::AgentFacetsB3 {}" % " + ".join(traits))
    g.append("impl<T: %s + crate::facets::AgentFacets + crate::facets_b3::AgentFacetsB3> AgentApi for T {}" % " + ".join(traits))
    g.append("")
    g.append("/// The fake: every call is recorded by name and answered with the default.")
    g.append("#[derive(Default)]")
    g.append("pub struct FakeAgentApi {")
    g.append("    pub calls: std::sync::Mutex<Vec<String>>,")
    g.append("}")
    g.append("")
    g.append("impl FakeAgentApi {")
    g.append("    pub fn calls(&self) -> Vec<String> {")
    g.append("        self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()")
    g.append("    }")
    g.append("    fn note(&self, name: &str) {")
    g.append("        self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(name.to_string());")
    g.append("    }")
    g.append("}")
    g.append("")
    for d, lst in sorted(bydom.items()):
        tn = "Agent" + camel(d)
        g.append("impl %s for FakeAgentApi {" % tn)
        for mod, fn, (qp, qr, dflt, n) in sorted(lst):
            args = ", ".join("_%s: %s" % (nm, ty) for nm, ty, m_ in qp)
            g.append("    fn %s_%s(&self%s)%s {" % (mod, fn, (", " + args) if args else "", (" -> " + qr) if qr else ""))
            g.append('        self.note("%s::%s");' % (mod, fn))
            g.append("        %s" % dflt)
            g.append("    }")
        g.append("}")
        g.append("")
    open(API + "src/generated.rs", "w").write("\n".join(g))
    # the agent side
    a = ["//! GENERATED by dist/split/gen_facade.py: the delegating implementation of `iyakki_agent_api::AgentApi` (F-SPLIT-03, pass 2).", "#![allow(clippy::all)]", "", "pub struct AgentApiImpl;", ""]
    for d, lst in sorted(bydom.items()):
        tn = "Agent" + camel(d)
        a.append("impl iyakki_agent_api::%s for AgentApiImpl {" % tn)
        for mod, fn, (qp, qr, dflt, n) in sorted(lst):
            args = ", ".join("%s%s: %s" % ("mut " if m_ else "", nm, ty) for nm, ty, m_ in qp)
            a.append("    fn %s_%s(&self%s)%s {" % (mod, fn, (", " + args) if args else "", (" -> " + qr) if qr else ""))
            a.append("        crate::%s::%s(%s)" % (mod, fn, ", ".join(nm for nm, _, _ in qp)))
            a.append("    }")
        a.append("}")
        a.append("")
    open(AG + "agentapi_gen.rs", "w").write("\n".join(a))
    if not os.path.exists(API + "Cargo.toml"):
        open(API + "Cargo.toml", "w").write('''[package]
name = "iyakki-agent-api"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "The generated facade of the agent's functions the UI calls (F-SPLIT-03 pass 2): traits per domain, a recording fake. Pure types only."

[dependencies]
iyakki-shared = { workspace = true }
serde_json = "1"
iyakki-sec = { workspace = true }
''')
        open(API + "src/lib.rs", "w").write("//! The generated facade (see dist/split/gen_facade.py). Hand-written traits for stateful handles go next to `generated`.\n\npub mod generated;\npub use generated::*;\n")
    if rewrite:
        do_rewrite(keep)


def fn_has_cx(lines, i):
    """Does the nearest enclosing `fn` above line i take a `cx: &AppCx`?"""
    j = i
    while j >= 0 and i - j < 400:
        if re.match(r"^\s*(pub(\([a-z]+\))?\s+)?(const\s+)?(async\s+)?fn\s", lines[j]):
            sig = ""
            k = j
            while k < len(lines) and k < j + 12:
                sig += lines[k]
                if "{" in lines[k] or ";" in lines[k]:
                    break
                k += 1
            return bool(re.search(r"\bcx\s*:\s*&", sig))
        j -= 1
    return False


GLOBAL = "--global" in sys.argv


def do_rewrite(keep):
    backup = json.load(open(BACKUP)) if os.path.exists(BACKUP) else {}
    changed = 0
    for f in ui_files():
        t = open(f).read()
        cut = re.search(r"#\[cfg\(test\)\]\s*(pub(\([a-z]+\))? )?mod ", t)
        head, tail = (t[:cut.start()], t[cut.start():]) if cut else (t, "")
        lines = head.split("\n")
        imports = {}
        for mm in re.finditer(r"^use iyakki_agent::(\w+)(?: as (\w+))?;", head, re.M):
            imports[mm.group(2) or mm.group(1)] = mm.group(1)
        out = []
        for i, ln in enumerate(lines):
            new = ln
            if not ln.strip().startswith("//") and imports and (re.search(r"\bcx\b", ln) or fn_has_cx(lines, i)):
                def rep2(m):
                    k = (imports[m.group(1)], m.group(2))
                    return "cx.agent.%s_%s(" % k if k in keep else m.group(0)
                new = re.sub(r"(?<![\w:.])(%s)::([a-z_]\w*)\(" % "|".join(map(re.escape, imports)), rep2, new)
            if not ln.strip().startswith("//") and (re.search(r"\bcx\b", ln) or fn_has_cx(lines, i)) and "iyakki_agent::" in ln:
                def rep(m):
                    k = (m.group(1), m.group(2))
                    return "cx.agent.%s_%s(" % k if k in keep else m.group(0)
                new = re.sub(r"iyakki_agent::(\w+)::([a-z_]\w*)\(", rep, new)
            if GLOBAL and not ln.strip().startswith("//") and "iyakki_agent::" in new or (GLOBAL and imports and not ln.strip().startswith("//")):
                g = "crate::agent()" if "/iyakki-apps/" in f else "iyakki_apps::agent()"
                def rep3(m):
                    k = (m.group(1), m.group(2))
                    return "%s.%s_%s(" % (g, k[0], k[1]) if k in keep else m.group(0)
                new = re.sub(r"iyakki_agent::(\w+)::([a-z_]\w*)\(", rep3, new)
                if imports:
                    def rep4(m):
                        k = (imports[m.group(1)], m.group(2))
                        return "%s.%s_%s(" % (g, k[0], k[1]) if k in keep else m.group(0)
                    new = re.sub(r"(?<![\w:.])(%s)::([a-z_]\w*)\(" % "|".join(map(re.escape, imports)), rep4, new)
            if new != ln:
                backup.setdefault(f, {})[str(i + 1)] = ln
                changed += 1
            out.append(new)
        text = "\n".join(out)
        for alias, modname in imports.items():
            if not re.search(r"(?<![\w:.])%s::" % re.escape(alias), text + tail) and not re.search(r"\b%s\b" % re.escape(alias), re.sub(r"^use iyakki_agent::[^\n]*\n", "", text + tail, flags=re.M)):
                text = re.sub(r"^use iyakki_agent::%s(?: as %s)?;\n" % (re.escape(modname), re.escape(alias)), "", text, flags=re.M)
        if text != head:
            open(f, "w").write(text + tail)
    json.dump(backup, open(BACKUP, "w"))
    print("rewrote", changed, "lines")


if __name__ == "__main__":
    gen("--dry" in sys.argv, "--rewrite" in sys.argv)
