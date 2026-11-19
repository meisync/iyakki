#!/usr/bin/env python3
"""Scans all test code for the patterns that make tests flaky and writes docs/development/FLAKY-AUDIT.md.

  python3 dist/scripts/flaky-patterns.py            rewrite docs/development/FLAKY-AUDIT.md
  python3 dist/scripts/flaky-patterns.py --counts   print `crate category risky` lines (what the ratchet test compares)
  python3 dist/scripts/flaky-patterns.py --write-ratchet   lower docs/development/flaky-ratchet.txt to today's counts
  python3 dist/scripts/flaky-patterns.py --check    exit 1 when FLAKY-AUDIT.md is not up to date

Test code = whole files named `*_tests.rs`, `tests.rs`, `*_test.rs`, anything under a `tests/` folder, and in every other file the part
from the first `#[cfg(test)]` on. A hit is `ok` when the file shows the guard that makes the pattern safe (a wall-clock deadline near a
sleep, a testkit lock or an in-process lock static near set_var, ...) or the line carries the marker `// flaky-ok: why`; otherwise `risky`.
The crate `iyakki-testkit` (the cure) is not scanned. Owner order 2026-11-18; see docs/development/FLAKY-TESTS.md.
"""
import os, re, subprocess, sys, collections

ROOT = subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip() if os.path.exists(".git") or True else "."
CATS = {
    "sleep": "sleep used for synchronisation (thread::sleep / sleep(Duration) outside a condition-polling `while`/`loop` and with no wall-clock deadline around it)",
    "elapsed": "an upper bound on elapsed time is asserted",
    "port": "an ephemeral port is bound, freed and reused (bind :0 + local_addr, or a fixed literal port)",
    "env": "set_var / remove_var without a lock",
    "cwd": "set_current_dir in a test",
    "static": "shared mutable static / OnceLock between tests (not a lock or an id counter)",
    "tmpname": "a fixed name under temp_dir()",
    "stubsleep": "a stub child that lives for a fixed time (`sleep N`)",
    "attempts": "a polling loop counted in attempts instead of wall-clock time",
    "cluster": "a test that runs kubectl/k3s without a lock or skip guard",
}

def is_test_file(rel):
    b = os.path.basename(rel)
    return (b.endswith("_tests.rs") or b.endswith("_test.rs") or b in ("tests.rs", "test.rs", "testdir.rs") or "/tests/" in rel or "/benches/" in rel)

def test_region(rel, lines):
    if is_test_file(rel):
        return 0
    for i, l in enumerate(lines):
        if l.strip().startswith("#[cfg(test)]") and any(re.match(r"\s*(pub(\([a-z]+\))? )?mod \w+", x) for x in lines[i + 1: i + 3]):
            return i
    return None

DEADLINE = re.compile(r"Instant::now|elapsed\(\)|deadline|SystemTime::now|wait_until|wait_for|testkit::|timeout")
LOCKISH = re.compile(r"Mutex<\(\)>|iyakki_testkit|testkit::|EnvGuard|\block\(names::|_LOCK\b|\bserial\b", re.I)

def scan_file(rel, text):
    lines = text.split("\n")
    start = test_region(rel, lines)
    if start is None:
        return []
    body = "\n".join(lines[start:])
    guarded_file = bool(LOCKISH.search(body))
    hits = []
    for i in range(start, len(lines)):
        l = lines[i]
        s = l.strip()
        if s.startswith("//"):
            continue
        marker = "flaky-ok" in l
        near = "\n".join(lines[max(start, i - 8): i + 8])
        def add(cat, ok, what=None):
            hits.append((cat, "ok" if (ok or marker) else "risky", i + 1, s[:140]))
        if re.search(r"\b(thread::)?sleep\(", l) and "fn " not in s and "Command" not in l:
            back = lines[max(start, i - 6): i]
            in_cond_loop = any(re.match(r"\s*(\'\w+: )?(while\b|loop\s*\{)", b) for b in back) and not any(re.match(r"\s*for\b", b) for b in back[-3:])
            add("sleep", bool(DEADLINE.search(near)) or in_cond_loop)
        if re.search(r"elapsed\(\)\s*<", l) and "assert" in l:
            # an upper bound of 10 s or more is a hang guard, not a speed claim
            d = re.search(r"from_(millis|secs)\((\d+)\)", l)
            ms = (int(d.group(2)) * (1000 if d.group(1) == "secs" else 1)) if d else 0
            add("elapsed", ms >= 10_000)
        if re.search(r'bind\("[0-9.]*:0"\)|bind\(\("[0-9.]*", 0\)\)', l):
            tail = "\n".join(lines[i: i + 5])
            if re.search(r"local_addr\(\)", tail) and (re.search(r"local_addr\(\)[^;]*\.port\(\)", l) or re.search(r"drop\(", tail)):
                add("port", False)
        if re.search(r"free_port\(\)", l) and "fn free_port" not in l:
            add("port", free_port_is_reserved())
        if re.search(r"set_var\(|remove_var\(", l):
            add("env", guarded_file)
        if "set_current_dir(" in l:
            add("cwd", guarded_file)
        m = re.match(r"\s*(pub(\([a-z]+\))? )?static (mut )?([A-Z0-9_]+): ([^=]*)=", l)
        if m:
            name, ty = m.group(4), m.group(5)
            ok = "Mutex<()>" in ty or re.search(r"^N$|^PORT$|COUNTER|NEXT|SEQ|ID", name) is not None or "Atomic" in ty and re.search(r"COUNT|NEXT|SEQ", name) is not None
            add("static", ok)
        if re.search(r'temp_dir\(\)\s*\.join\("[^"{}]*"\)', l):
            add("tmpname", False)
        if re.search(r'Command::new\("sleep"\)|"sleep [0-9.]+|\bsleep [0-9]+[;"\' ]', l) and "thread::sleep" not in l:
            n = re.search(r'sleep[ "(,]+(\d+)', "\n".join(lines[i: i + 3])) or re.search(r'\.arg\("(\d+)"\)', "\n".join(lines[i: i + 3]))
            add("stubsleep", bool(n) and int(n.group(1)) >= 30)
        mm = re.search(r"for _ in 0\.\.=?(\d+)", l)
        if mm and int(mm.group(1)) >= 20 and re.search(r"sleep\(|yield_now|recv_timeout|try_recv", "\n".join(lines[i: i + 6])):
            add("attempts", bool(DEADLINE.search("\n".join(lines[i: i + 8]))))
        if re.search(r'Command::new\("(kubectl|k3s)"\)', l):
            add("cluster", bool(re.search(r"lock\(|KUBE_LOCK|which|skip|available|tempdir|unique\(|fake|stub", near, re.I)))
    return hits

_reserved = None
def free_port_is_reserved():
    global _reserved
    if _reserved is None:
        ok = True
        for p in ("crates/iyakki-agent/src/testdir.rs", "crates/iyakki-sshfs/src/testdir.rs"):
            fp = os.path.join(ROOT, p)
            if os.path.exists(fp) and "fn claim_port" not in open(fp).read():
                ok = False
        _reserved = ok
    return _reserved

def crate_of(rel):
    parts = rel.split("/")
    return parts[1] if parts[0] == "crates" and len(parts) > 1 else parts[0]

def collect():
    files = subprocess.check_output(["git", "ls-files", "*.rs"], cwd=ROOT, text=True).split("\n")
    res = collections.defaultdict(list)
    for rel in files:
        if not rel or not rel.startswith("crates/") or rel.startswith("crates/iyakki-testkit/"):
            continue
        try:
            text = open(os.path.join(ROOT, rel), errors="replace").read()
        except OSError:
            continue
        for cat, verdict, ln, snip in scan_file(rel, text):
            res[crate_of(rel)].append((cat, verdict, rel, ln, snip))
    return res

def main():
    res = collect()
    if "--write-ratchet" in sys.argv:
        lines = ["# Risky flaky-test patterns allowed per crate (the ratchet test flaky_pattern_ratchet): `crate category count`. Numbers only shrink: after a fix run",
                 "# python3 dist/scripts/flaky-patterns.py --write-ratchet. See docs/development/FLAKY-TESTS.md."]
        for c in sorted(res):
            cnt = collections.Counter(h[0] for h in res[c] if h[1] == "risky")
            for cat in CATS:
                if cnt[cat]:
                    lines.append(f"{c} {cat} {cnt[cat]}")
        open(os.path.join(ROOT, "docs/development/flaky-ratchet.txt"), "w").write("\n".join(lines) + "\n")
        print("wrote docs/development/flaky-ratchet.txt")
        return
    if "--counts" in sys.argv:
        for c in sorted(res):
            cnt = collections.Counter(h[0] for h in res[c] if h[1] == "risky")
            for cat in CATS:
                if cnt[cat]:
                    print(c, cat, cnt[cat])
        return
    out = ["# Flaky-test audit (generated)", "",
           "**For:** whoever fixes flaky tests.", "**You will get:** the risky patterns per crate and the list of hits. Generated by `python3 dist/scripts/flaky-patterns.py`; do not edit by hand. The cure is in [FLAKY-TESTS.md](FLAKY-TESTS.md); `flaky_pattern_ratchet` (iyakki-core tests) lets the risky counts only shrink (`flaky-ratchet.txt`).", "",
           "## Patterns", ""]
    for k, v in CATS.items():
        out.append(f"* `{k}`: {v}")
    out += ["", "A hit is `ok` when the file shows the guard (a wall-clock deadline near a sleep, a lock near set_var, `free_port` of the testdir helpers, which claims each number with an flock like testkit::port) or the line says `// flaky-ok: why`.", "", "## Counts per crate (risky / ok)", "", "| Crate | " + " | ".join(CATS) + " | risky total |", "|---|" + "---|" * (len(CATS) + 1)]
    tot = collections.Counter(); totok = collections.Counter()
    for c in sorted(res):
        r = collections.Counter(h[0] for h in res[c] if h[1] == "risky"); o = collections.Counter(h[0] for h in res[c] if h[1] == "ok")
        if not sum(r.values()) and not sum(o.values()):
            continue
        tot.update(r); totok.update(o)
        out.append(f"| {c} | " + " | ".join(f"{r[k]} / {o[k]}" for k in CATS) + f" | {sum(r.values())} |")
    out.append("| **all** | " + " | ".join(f"{tot[k]} / {totok[k]}" for k in CATS) + f" | {sum(tot.values())} |")
    out += ["", "## Risky hits", ""]
    for c in sorted(res):
        rs = [h for h in res[c] if h[1] == "risky"]
        if not rs:
            continue
        out += [f"### {c} ({len(rs)})", ""]
        for cat, _, rel, ln, snip in sorted(rs, key=lambda h: (h[2], h[3])):
            out.append(f"* `{cat}` {rel}:{ln}: `{snip.replace('`', chr(39))}`")
        out.append("")
    text = "\n".join(out) + "\n"
    path = os.path.join(ROOT, "docs/development/FLAKY-AUDIT.md")
    if "--check" in sys.argv:
        sys.exit(0 if os.path.exists(path) and open(path).read() == text else 1)
    open(path, "w").write(text)
    print("wrote", path, "risky:", sum(tot.values()), "ok:", sum(totok.values()))

main()
