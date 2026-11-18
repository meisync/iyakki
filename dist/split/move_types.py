#!/usr/bin/env python3
"""Pass 1 of the UI/agent split (docs/development/UI-BOUNDARY.md, "the scripted split"): move the pure TYPES of an agent module that the UI
names into the leaf crate `iyakki-shared`, leave a `pub use` behind in the agent, and rewrite the UI paths.

    python3 dist/split/move_types.py MODULE [MODULE ...] [--dry]

What counts as movable (checked per top-level item of `crates/iyakki-agent/src/MODULE.rs`):
  * `struct`, `enum`, `const`, `type` items, and every `impl` block whose self type is one of them;
  * every CamelCase / ALL_CAPS name they use resolves to std, an external crate, another movable item of the file, or a module that
    already lives in iyakki-shared; no `crate::` path outside iyakki-shared; no call of a function of the module's own file;
  * the set is a fixpoint: an item that needs something that cannot move cannot move either.
Only the items the UI names, plus what they need, are moved (the closure). Fields and methods of moved items become `pub`.
The agent keeps `pub use iyakki_shared::MODULE::{names};` so nothing else changes. Idempotent for names already moved.
"""
import os, re, subprocess, sys

ROOT = subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip()
AG = ROOT + "/crates/iyakki-agent/src/"
SH = ROOT + "/crates/iyakki-shared/src/"
UI = [ROOT + "/crates/iyakki-apps/src", ROOT + "/crates/iyakki-shell/src"]
EXTERNAL_OK = {"std", "core", "alloc", "serde", "serde_json", "iyakki_sec", "iyakki_schema", "iyakki_clock", "iyakki_core_api", "iyakki_storage",
               "iyakki_netscan", "iyakki_devctl", "iyakki_proxy", "iyakki_wasm", "iyakki_filescope", "iyakki_cloudmode", "iyakki_shared", "sha2", "zeroize", "thiserror"}
KW = set("self Self super crate pub fn let mut if else match for while loop in as use mod impl struct enum const static type trait where return Some None Ok Err Option Result Vec String Box Arc Rc Mutex RwLock HashMap HashSet BTreeMap BTreeSet VecDeque Default Clone Copy Debug PartialEq Eq PartialOrd Ord Hash Serialize Deserialize From Into Display Iterator IntoIterator Send Sync Sized ToString PathBuf Path Duration Instant AtomicBool AtomicU32 AtomicU64 AtomicUsize AtomicI64 Ordering Cow Error Write Read Self".split())


def strip(code):
    """Blank out comments and string/char literals so braces and identifiers can be counted."""
    out = []
    i, n = 0, len(code)
    while i < n:
        c = code[i]
        if code.startswith("//", i):
            j = code.find("\n", i)
            j = n if j < 0 else j
            out.append(" " * (j - i))
            i = j
        elif code.startswith("/*", i):
            j = code.find("*/", i)
            j = n if j < 0 else j + 2
            out.append(re.sub(r"[^\n]", " ", code[i:j]))
            i = j
        elif c == '"' or (c == "r" and re.match(r'r#*"', code[i:i + 4])):
            if c == "r":
                m = re.match(r'r(#*)"', code[i:])
                end = '"' + m.group(1)
                j = code.find(end, i + len(m.group(0)))
                j = n if j < 0 else j + len(end)
            else:
                j = i + 1
                while j < n and code[j] != '"':
                    j += 2 if code[j] == "\\" else 1
                j += 1
            out.append(re.sub(r"[^\n]", " ", code[i:j]))
            i = j
        elif c == "'" and re.match(r"'(\\.[^']*|[^\\'])'", code[i:i + 12]):
            m = re.match(r"'(\\.[^']*|[^\\'])'", code[i:])
            out.append(" " * len(m.group(0)))
            i += len(m.group(0))
        else:
            out.append(c)
            i += 1
    return "".join(out)


def items(text):
    """Top-level items as (start, end, head) line indexes: `start` includes the doc and attribute lines above the head line."""
    slines = strip(text).split("\n")
    tl = text.split("\n")
    res = []
    i = 0
    attr_start = None
    while i < len(slines):
        ln = slines[i]
        raw = tl[i].lstrip()
        if raw.startswith("///") or raw.startswith("#[") or raw.startswith("#!["):
            if tl[i][:1].isspace():
                i += 1
                continue
            if attr_start is None:
                attr_start = i
            d = 0
            k = i
            while k < len(slines):
                d += slines[k].count("[") - slines[k].count("]")
                if d <= 0:
                    break
                k += 1
            i = k + 1
            continue
        if not raw.strip():
            attr_start = None
            i += 1
            continue
        if raw.startswith("//") or not ln.strip() or ln[0].isspace() or ln[0] == "}":
            i += 1
            continue
        d = 0
        k = i
        end = None
        while k < len(slines):
            for ch in slines[k]:
                if ch in "{([":
                    d += 1
                elif ch in "})]":
                    d -= 1
            if d == 0 and (slines[k].rstrip().endswith(";") or slines[k].rstrip().endswith("}")):
                end = k
                break
            k += 1
        if end is None:
            break
        res.append((attr_start if attr_start is not None else i, end, ln))
        attr_start = None
        i = end + 1
    return res, tl, slines


def classify(head):
    h = head.strip()
    m = re.match(r"(?:pub(?:\([a-z]+\))?\s+)?(struct|enum|const|static|type|trait|fn|mod|use|impl|macro_rules!|union)\b\s*(?:<[^>]*>\s*)?([A-Za-z_]\w*)?", h)
    if not m:
        return None, None
    return m.group(1), m.group(2)


def impl_self(head_text):
    h = re.sub(r"\s+", " ", head_text)
    m = re.match(r"\s*impl(?:<[^>{]*>)?\s+(?:([\w:<>, &']+?)\s+for\s+)?([A-Za-z_]\w*)", h)
    if not m:
        return None, None
    return m.group(2), m.group(1)


EXCL = os.path.join(os.path.dirname(os.path.abspath(__file__)), "excluded.txt")


def read_excluded():
    """dist/split/excluded.txt: `module Name` lines, items that must stay in the agent (found by the compile loop)."""
    out = {}
    if os.path.exists(EXCL):
        for ln in open(EXCL).read().split("\n"):
            if ln.strip() and not ln.startswith("#"):
                m, n = ln.split()[:2]
                out.setdefault(m, set()).add(n)
    return out


_NAMES = {}


def shared_names(m):
    """Top-level type/const names of a module fragment already in iyakki-shared."""
    if m not in _NAMES:
        fp = SH + m + ".rs"
        names = set()
        if os.path.exists(fp):
            t = open(fp).read()
            its, tl, sl = items(t)
            for hs, e, h in its:
                k, n = classify(h)
                if n and k in ("struct", "enum", "const", "type", "union", "fn", "trait"):
                    names.add(n)
        _NAMES[m] = names
    return _NAMES[m]


def use_ok(pth, shared_complete):
    """Can `pth` (a path as written in a `use`, one name) be resolved from inside iyakki-shared?"""
    parts = pth.split("::")
    if parts[0] != "crate":
        return parts[0] in EXTERNAL_OK
    if len(parts) < 2:
        return False
    if parts[1] in shared_complete:
        return True
    return len(parts) >= 3 and parts[2] in shared_names(parts[1])


def shared_modules():
    """Modules that were complete in iyakki-shared before this run (HEAD), so everything in them can be named from the new fragments."""
    head = subprocess.check_output(["git", "show", "HEAD:crates/iyakki-shared/src/lib.rs"], cwd=ROOT, text=True)
    whole = set(re.findall(r"^pub use iyakki_shared::(\w+);", open(AG + "lib.rs").read(), re.M))
    return set(re.findall(r"pub mod (\w+);", head)) & whole


def shared_present():
    return set(re.findall(r"pub mod (\w+);", open(SH + "lib.rs").read()))


def ui_names(mod):
    names = set()
    for d in UI:
        for r, _, fs in os.walk(d):
            for f in fs:
                if not f.endswith(".rs") or f.endswith("_tests.rs") or f == "tests.rs":
                    continue
                t = open(os.path.join(r, f)).read()
                m = re.search(r"#\[cfg\(test\)\]\s*(pub(\([a-z]+\))? )?mod ", t)
                code = t[:m.start()] if m else t
                for mm in re.finditer(r"iyakki_agent::%s::(\w+)" % mod, code):
                    names.add(mm.group(1))
                for mm in re.finditer(r"iyakki_agent::%s::\{([^}]*)\}" % mod, code):
                    for x in mm.group(1).split(","):
                        x = x.strip().split(" as ")[0].strip()
                        if re.match(r"^\w+$", x):
                            names.add(x)
    return names


def plan(mod, shared):
    path = AG + mod + ".rs"
    if not os.path.exists(path):
        path = AG + mod + "/mod.rs"  # a directory module: its own items (the types in mod.rs) move; its submodules stay
        if not os.path.exists(path):
            return None
    text = open(path).read()
    cut = re.search(r"#\[cfg\(test\)\]\s*(?:#\[path[^\]]*\]\s*)?mod \w+", text)
    body_end = cut.start() if cut else len(text)
    its, tl, slines = items(text[:body_end])
    uses = {}
    for hs, e, head in its:
        k, n = classify(head)
        if k == "use":
            blk = "\n".join(slines[hs:e + 1])
            blk = re.sub(r"\s+", " ", blk)
            m = re.match(r"\s*(?:pub(?:\([a-z]+\))?\s+)?use (.*);", blk)
            if not m:
                continue
            try:
                for path_, name in expand_use(m.group(1)):
                    uses[name] = path_
            except ValueError:
                pass
    cand = {}
    raws = {}
    toplevel = {}
    impls = []
    for idx, (hs, e, head) in enumerate(its):
        k, n = classify(head)
        full = "\n".join(slines[hs:e + 1])
        if k in ("struct", "enum", "const", "type", "union") and n:
            cand[n] = (idx, k, full)
            raws[n] = "\n".join(tl[hs:e + 1])
        elif k == "impl":
            sty, tr = impl_self("\n".join(slines[hs:e + 1]).split("{")[0])
            impls.append((idx, sty, tr, full))
            raws[("impl", idx)] = "\n".join(tl[hs:e + 1])
        if n and k in ("fn", "static", "trait", "struct", "enum", "const", "type", "mod"):
            toplevel[n] = k
    for h in its:
        pass
    fns = {n for n, k in toplevel.items() if k in ("fn", "static", "trait", "mod")}
    macros = set(re.findall(r"macro_rules!\s*(\w+)", text))
    # per-item dependencies
    def deps_of(full, raw=""):
        bad = False
        names = set()
        for m in re.finditer(r"crate::(\w+)(?:::(\w+))?", full):
            if not (m.group(1) in shared or (m.group(2) and m.group(2) in shared_names(m.group(1)))):
                bad = True
        if re.search(r"\bsuper::", full):
            bad = True
        for m in re.finditer(r"(?<![:\w])([A-Z][A-Za-z0-9_]*)\b", full):
            names.add(m.group(1))
        for m in re.finditer(r'default\s*=\s*"(\w+)"|(?:with|serialize_with|deserialize_with)\s*=\s*"(\w+)', raw):
            f = m.group(1) or m.group(2)
            if f in fns:
                bad = True
        for f in fns:
            if re.search(r"(?<![\w:.])%s(?![\w])" % re.escape(f), full) and not re.search(r"\bfn\s+%s\b" % re.escape(f), full):
                bad = True
        for m in macros:
            if re.search(r"\b%s!" % m, full):
                bad = True
        if re.search(r"\b(include_str|include_bytes|include|env|option_env|concat|module_path|file|line)!", full):
            bad = True
        for nm, pth in uses.items():
            root = pth.split("::")[0]
            ok = use_ok(pth, shared)
            if not ok and re.search(r"(?<![.\w])%s\s*(\(|::|!)" % re.escape(nm), full):
                bad = True
        return bad, names
    info = {}
    for n, (idx, k, full) in cand.items():
        info[n] = [full, *deps_of(full, raws.get(n, ""))]
    impl_of = {}
    for idx, sty, tr, full in impls:
        if sty in cand:
            impl_of.setdefault(sty, []).append((tr, full, raws.get(("impl", idx), "")))
    for n in list(cand):
        for tr, full, rawi in impl_of.get(n, []):
            bad, names = deps_of(full, rawi)
            info[n][1] = info[n][1] or bad
            info[n][2] |= names
            if tr:
                trn = re.findall(r"[A-Z]\w*", tr)
                info[n][2] |= set(trn)
    variants = set()
    for n_, (idx_, k_, full_) in cand.items():
        if k_ == "enum":
            variants |= set(re.findall(r"^\s+([A-Z]\w*)", full_, re.M))
    # generic parameters and assoc type names written in the items
    GENERIC_NAMES = {"Output", "Item", "Target", "Error", "Ok", "Self"} & set()
    def resolves(name, movable):
        if name in cand:
            return name in movable
        if name in KW:
            return True
        if name in uses:
            p = uses[name]
            root = p.split("::")[0]
            if root == "crate":
                return use_ok(p, shared)
            return root in EXTERNAL_OK
        if name in toplevel or name in fns:
            return False  # a trait/fn/static of this file
        return name in variants or len(name) <= 2 or name in GENERIC_NAMES  # generics, enum variants of this file
    excluded = read_excluded().get(mod, set())
    movable = {n for n in cand if not info[n][1] and n not in excluded}
    changed = True
    while changed:
        changed = False
        for n in list(movable):
            if any(not resolves(d, movable) for d in info[n][2] if d != n):
                movable.discard(n)
                changed = True
    want = ui_names(mod) & set(cand)
    need = set()
    stack = [w for w in want if w in movable]
    while stack:
        x = stack.pop()
        if x in need:
            continue
        need.add(x)
        for d in info[x][2]:
            if d in cand and d in movable and d not in need:
                stack.append(d)
    # an item whose needed dependency is unmovable was already dropped by the fixpoint
    return dict(text=text, its=its, slines=slines, cand=cand, impls=impls, uses=uses, need=need, want=want, movable=movable, path=path)


def expand_use(s):
    s = s.strip()
    out = []
    def rec(prefix, rest):
        rest = rest.strip()
        if rest.startswith("{") and rest.endswith("}"):
            inner = rest[1:-1]
            depth, cur, parts = 0, "", []
            for ch in inner:
                if ch == "{":
                    depth += 1
                if ch == "}":
                    depth -= 1
                if ch == "," and depth == 0:
                    parts.append(cur)
                    cur = ""
                else:
                    cur += ch
            if cur.strip():
                parts.append(cur)
            for p in parts:
                rec(prefix, p)
            return
        m = re.match(r"^(.*?)::(\{.*\})$", rest)
        if m:
            rec(prefix + m.group(1) + "::", m.group(2))
            return
        if " as " in rest:
            a, b = rest.split(" as ")
            out.append((prefix + a.strip(), b.strip()))
        elif rest == "self":
            out.append((prefix.rstrip(":"), prefix.rstrip(":").split("::")[-1]))
        else:
            out.append((prefix + rest, rest.split("::")[-1]))
    rec("", s)
    return out


def pubify(text):
    """Make what the agent used to reach from the same file reachable from the agent crate: struct fields and inherent methods become `pub`."""
    text = re.sub(r"\bpub\((?:crate|super|self)\)", "pub", text)
    out = []
    mode = None  # "struct", "inherent", "other"
    depth = 0
    for ln in text.split("\n"):
        st = ln.strip()
        if depth == 0 and re.match(r"(struct|enum|const|type|union)\s", ln):
            ln = "pub " + ln
            st = ln.strip()
        if depth == 0 and ln and not ln[0].isspace():
            if re.match(r"(pub\s+)?struct\s", st):
                mode = "struct"
            elif re.match(r"impl\b", st) and not re.search(r"\bfor\b", st.split("{")[0]):
                mode = "inherent"
            else:
                mode = "other"
        if depth == 1 and mode == "struct" and re.match(r"^\s+[a-z_]\w*\s*:", ln) and not st.startswith(("pub ", "//", "#")):
            ln = re.sub(r"^(\s+)", r"\1pub ", ln, count=1)
        elif depth == 1 and mode == "inherent" and re.match(r"^\s+(const\s+)?fn\s", ln):
            ln = re.sub(r"^(\s+)", r"\1pub ", ln, count=1)
        depth += strip(ln).count("{") - strip(ln).count("}")
        out.append(ln)
    return "\n".join(out)


def apply(mod, p, dry):
    shared = shared_modules()
    need = p["need"]
    if not need:
        return 0, []
    its, slines, cand, impls = p["its"], p["slines"], p["cand"], p["impls"]
    text_lines = p["text"].split("\n")
    take = set()
    frags = []
    for n in sorted(need, key=lambda x: cand[x][0]):
        take.add(cand[n][0])
    for idx, sty, tr, full in impls:
        if sty in need:
            take.add(idx)
    use_idx = [i for i, (hs, e, h) in enumerate(its) if classify(h)[0] == "use"]
    body = []
    for i in sorted(take):
        hs, e, head = its[i]
        body.append("\n".join(text_lines[hs:e + 1]))
    frag = pubify("\n\n".join(body))
    # keep the uses that the fragment mentions
    uses_txt = []
    seen = set()
    for i in use_idx:
        hs, e, head = its[i]
        blk = "\n".join(text_lines[hs:e + 1])
        origin = re.sub(r"^\s*(pub(\([a-z]+\))?\s+)?use\s+", "", blk.strip())
        if "*" in origin:
            continue
        try:
            pairs = expand_use(re.sub(r"\s+", " ", origin).rstrip(";"))
        except ValueError:
            continue
        for pth, nm in pairs:
            if nm in ("self", "*") or not re.search(r"(?<![\w.])%s\b" % re.escape(nm), frag) or not use_ok(pth, shared):
                continue
            if pth.startswith("iyakki_shared::"):
                pth = "crate::" + pth[len("iyakki_shared::"):]
            if pth.startswith("crate::%s::" % mod):
                continue
            last = pth.split("::")[-1]
            stmt = "use %s;" % pth if last == nm else "use %s as %s;" % (pth, nm)
            if stmt not in seen:
                seen.add(stmt)
                uses_txt.append(stmt)
    out = "//! Types moved out of `iyakki_agent::%s` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.\n\n" % mod + "\n".join(uses_txt) + "\n\n" + frag + "\n"
    names_sorted = sorted(need)
    if dry:
        print(mod, "move", names_sorted)
        return len(need), names_sorted
    # write the shared module
    sp = SH + mod + ".rs"
    appended = os.path.exists(sp)
    if appended:
        have = open(sp).read()
        its_s, tl_s, sl_s = items(have)
        old_names = set()
        for hs_, e_, h_ in its_s:
            k_, n_ = classify(h_)
            if n_ and k_ in ("struct", "enum", "const", "type", "union"):
                old_names.add(n_)
        names_sorted = sorted(set(names_sorted) - old_names)
        if not names_sorted:
            return 0, []
        # rebuild the fragment for the new names only
        take = set()
        for n in names_sorted:
            take.add(cand[n][0])
        for idx, sty, tr, full in impls:
            if sty in names_sorted:
                take.add(idx)
        body = []
        for i in sorted(take):
            hs, e, head = its[i]
            body.append("\n".join(text_lines[hs:e + 1]))
        frag = pubify("\n\n".join(body))
        newuses = [u for u in uses_txt if u not in have]
        out = "\n" + "\n".join(newuses) + "\n\n" + frag + "\n"
        open(sp, "a").write(out)
    else:
        open(sp, "w").write(out)
        lib = open(SH + "lib.rs").read()
        open(SH + "lib.rs", "w").write(lib + "pub mod %s;\n" % mod)
    record_run(mod, names_sorted)
    # agent: remove the items, add the re-export after the last `use`
    drop = []
    for i in sorted(take):
        hs, e, head = its[i]
        drop.append((hs, e))
    new = []
    dropped = set()
    for hs, e in drop:
        dropped.update(range(hs, e + 1))
    for ln_no, ln in enumerate(text_lines):
        if ln_no in dropped:
            continue
        new.append(ln)
    t = "\n".join(new)
    reexp = "pub use iyakki_shared::%s::{%s};\n" % (mod, ", ".join(names_sorted))
    m = None
    for m in re.finditer(r"^use [^\n]*;\n", t, re.M):
        pass
    if m:
        t = t[:m.end()] + reexp + t[m.end():]
    else:
        k = 0
        for mm in re.finditer(r"^(//![^\n]*\n|\n)", t):
            pass
        lines_ = t.split("\n")
        while k < len(lines_) and (lines_[k].startswith("//!") or not lines_[k].strip()):
            k += 1
        t = "\n".join(lines_[:k]) + ("\n" if k else "") + reexp + "\n".join(lines_[k:])
    open(p["path"], "w").write(t)
    # UI rewrite
    for d in UI:
        for r, _, fs in os.walk(d):
            for f in fs:
                if not f.endswith(".rs"):
                    continue
                fp = os.path.join(r, f)
                s = open(fp).read()
                o = s
                for nm in names_sorted:
                    s = re.sub(r"\biyakki_agent::%s::%s\b" % (mod, nm), "iyakki_shared::%s::%s" % (mod, nm), s)
                s = rewrite_groups(s, mod, set(names_sorted))
                if s != o:
                    open(fp, "w").write(s)
    return len(need), names_sorted


def rewrite_groups(s, mod, names):
    def rep(m):
        items_ = [x.strip() for x in m.group(2).split(",") if x.strip()]
        moved = [x for x in items_ if x.split(" as ")[0].strip() in names]
        keep = [x for x in items_ if x not in moved]
        if not moved:
            return m.group(0)
        res = m.group(1) + "use iyakki_shared::%s::{%s};" % (mod, ", ".join(moved))
        if keep:
            res += "\n" + m.group(1) + "use iyakki_agent::%s::{%s};" % (mod, ", ".join(keep))
        return res
    return re.sub(r"^([ \t]*)use iyakki_agent::%s::\{([^}]*)\};" % re.escape(mod), rep, s, flags=re.M)


LAST = os.path.join(os.path.dirname(os.path.abspath(__file__)), ".lastrun.json")


def record_run(mod, names):
    import json
    d = json.load(open(LAST)) if os.path.exists(LAST) else {}
    d[mod] = sorted(set(d.get(mod, [])) | set(names))
    json.dump(d, open(LAST, "w"))


def undo(mod):
    """Take what this run moved of a module back out (used when the shared crate does not compile with it)."""
    import json
    d = json.load(open(LAST)) if os.path.exists(LAST) else {}
    names = set(d.pop(mod, []))
    json.dump(d, open(LAST, "w"))
    sp = SH + mod + ".rs"
    tracked = subprocess.call(["git", "ls-files", "--error-unmatch", sp], cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) == 0
    if tracked:
        subprocess.call(["git", "checkout", "--", sp], cwd=ROOT)
    else:
        if os.path.exists(sp):
            os.remove(sp)
        lib = open(SH + "lib.rs").read().replace("pub mod %s;\n" % mod, "")
        open(SH + "lib.rs", "w").write(lib)
    subprocess.call(["git", "checkout", "--", AG + mod + ".rs"], cwd=ROOT)
    for d_ in UI:
        for r, _, fs in os.walk(d_):
            for f in fs:
                if f.endswith(".rs"):
                    fp = os.path.join(r, f)
                    s = open(fp).read()
                    t = s
                    if not tracked:
                        t = re.sub(r"\biyakki_shared::%s\b" % re.escape(mod), "iyakki_agent::%s" % mod, t)
                    else:
                        for nm in names:
                            t = re.sub(r"\biyakki_shared::%s::%s\b" % (re.escape(mod), nm), "iyakki_agent::%s::%s" % (mod, nm), t)
                        t = unrewrite_groups(t, mod, names)
                    if t != s:
                        open(fp, "w").write(t)


def unrewrite_groups(s, mod, names):
    def rep(m):
        items_ = [x.strip() for x in m.group(2).split(",") if x.strip()]
        back = [x for x in items_ if x.split(" as ")[0].strip() in names]
        keep = [x for x in items_ if x not in back]
        if not back:
            return m.group(0)
        res = m.group(1) + "use iyakki_agent::%s::{%s};" % (mod, ", ".join(back))
        if keep:
            res += "\n" + m.group(1) + "use iyakki_shared::%s::{%s};" % (mod, ", ".join(keep))
        return res
    return re.sub(r"^([ \t]*)use iyakki_shared::%s::\{([^}]*)\};" % re.escape(mod), rep, s, flags=re.M)


if __name__ == "__main__" and "--undo" in sys.argv:
    for m in [a for a in sys.argv[1:] if not a.startswith("--")]:
        undo(m)
    sys.exit(0)

if __name__ == "__main__":
    if "--keep-last" not in sys.argv and os.path.exists(LAST):
        os.remove(LAST)
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    dry = "--dry" in sys.argv
    shared = shared_modules()
    total = 0
    for mod in args:
        if mod in shared:
            print(mod, "already in iyakki-shared")
            continue
        p = plan(mod, shared)
        if not p:
            print(mod, "no flat file")
            continue
        n, names = apply(mod, p, dry)
        print(mod, "moved" if not dry else "would move", n, names[:12], "| wanted", len(p["want"]), "movable", len(p["movable"]))
        total += n
    print("items", total)
