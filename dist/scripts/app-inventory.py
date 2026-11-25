#!/usr/bin/env python3
"""Inventory of the built-in apps of the shell for the move onto the app standard (docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10).

One row per app that `crates/iyakki-shell/src/shell.rs` can open (the `PApp::X => Box::new(...)` lines): its files, how often the non-test code
names the shell context and the services, the lines the kernel ratchet allows for it, whether chat has tools for it and whether Settings has a page
of that name, the name of the port trait it would get, a size class and an order.

    python3 dist/scripts/app-inventory.py                 # prints the table (Markdown)
    python3 dist/scripts/app-inventory.py --write         # writes docs/development/APP-MIGRATION-INVENTORY.md
    python3 dist/scripts/app-inventory.py --check         # exits 1 when the committed file is out of date

Counting rules (the same cut as crates/iyakki-core/tests/dependency_ratchet.rs): only non-test code (the file up to its first `#[cfg(test)] mod`),
`*_tests.rs`, `tests.rs` and `*_port.rs` files are left out (the port is the one place that may name the services), comment lines are left out. Uses are counted as occurrences, not lines. Python 3, standard library only.
"""
import os
import re
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
APPS = os.path.join(ROOT, "crates", "iyakki-apps", "src", "apps")
SHELL = os.path.join(ROOT, "crates", "iyakki-shell", "src", "shell.rs")
ALLOW = os.path.join(ROOT, "crates", "iyakki-core", "tests", "data", "shell_kernel_access.allow")
MIGRATED = os.path.join(ROOT, "crates", "iyakki-core", "tests", "data", "migrated_tiles.list")
AGENT = os.path.join(ROOT, "crates", "iyakki-agent", "src")
SETTINGS = os.path.join(APPS, "settings.rs")
OUT = os.path.join(ROOT, "docs", "development", "APP-MIGRATION-INVENTORY.md")
TILE_REEXPORT = re.compile(r"^\s*pub use iyakki_tile_[a-z0-9_]+::|^use iyakki_tile_[a-z0-9_]+ as tile;", re.M)

CONTEXT = [("services", [r"\.services\b", r"\bServices\b"]), ("host", [r"\bcx\.host\b"]), ("web", [r"\bcx\.web\b"]), ("chat", [r"\bcx\.chat\b"]), ("store", [r"\bcx\.store\b"]), ("session/audit", [r"\bcx\.session\b", r"\bcx\.audit\b"])]


def read(p):
    try:
        with open(p, encoding="utf-8", errors="replace") as f:
            return f.read()
    except OSError:
        return ""


def non_test_code(text):
    """Everything before the first `#[cfg(test)]` that is followed by a `mod` item (a single test-only function does not end the scan)."""
    at = 0
    while True:
        i = text.find("#[cfg(test)]", at)
        if i < 0:
            return text
        after = text[i + len("#[cfg(test)]"):]
        nxt = next((l.strip() for l in after.splitlines() if l.strip() and not l.strip().startswith("#[") and not l.strip().startswith("//")), "")
        if re.match(r"(pub(\([a-z]+\))? )?mod ", nxt):
            return text[:i]
        at = i + 1


def code_lines(text):
    return [l for l in non_test_code(text).splitlines() if not l.strip().startswith("//")]


def rust_files(d):
    out = []
    for dp, _, fs in os.walk(d):
        for f in fs:
            if f.endswith(".rs") and not f.endswith("_tests.rs") and f != "tests.rs" and not f.endswith("_port.rs"):
                out.append(os.path.join(dp, f))
    return sorted(out)


def registry():
    """(PApp name, class name) for every app the shell opens."""
    text = read(SHELL)
    seen, out = set(), []
    for m in re.finditer(r"PApp::(\w+)\s*=>\s*Box::new\(([^\n]*)", text):
        cls = re.search(r"\b([A-Z]\w*App)\b(?:::new)?", m.group(2))
        if cls and m.group(1) not in seen:
            seen.add(m.group(1))
            out.append((m.group(1), cls.group(1)))
    return out


def snake(name):
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def allow_counts():
    m = {}
    for l in read(ALLOW).splitlines():
        l = l.strip()
        if l and not l.startswith("#"):
            p, n = l.rsplit(" ", 1)
            m[p] = int(n)
    return m


def migrated():
    m = {}
    for l in read(MIGRATED).splitlines():
        l = l.strip()
        if l and not l.startswith("#"):
            p, n = l.rsplit(" ", 1)
            m[p] = int(n)
    return m


def tab_names():
    text = read(SETTINGS)
    m = re.search(r"pub enum Tab \{(.*?)\n\}", text, re.S)
    return set(re.findall(r"^\s*(\w+),?\s*$", m.group(1), re.M)) if m else set()


def agent_text():
    t = []
    for f in rust_files(AGENT):
        t.append(non_test_code(read(f)))
    return "\n".join(t)


def home_file(cls, files):
    """The file that defines the app's struct."""
    for f in files:
        if re.search(r"\bpub struct %s\b" % cls, read(f)):
            return f
    return None


def main():
    allfiles = rust_files(APPS)
    allow, mig, tabs, agent = allow_counts(), migrated(), tab_names(), agent_text()
    rows = []
    for papp, cls in registry():
        home = home_file(cls, allfiles)
        if home is None:
            # the app lives in an extracted crate; the shell holds an adapter or nothing
            crate = os.path.join(ROOT, "crates", "iyakki-tile-" + snake(papp).replace("_", "-"))
            rows.append(dict(papp=papp, cls=cls, files=[], stem=snake(papp), extracted=os.path.isdir(crate)))
            continue
        stem = os.path.splitext(os.path.basename(home))[0]
        d = os.path.dirname(home)
        files = [f for f in allfiles if os.path.splitext(os.path.basename(f))[0] == stem or os.path.splitext(os.path.basename(f))[0].startswith(stem + "_") or os.path.relpath(f, APPS).startswith(stem + os.sep)]
        if d != APPS:
            files = [f for f in allfiles if os.path.dirname(f) == d]
        rows.append(dict(papp=papp, cls=cls, files=files, stem=stem, extracted=False))
    # a file that belongs to two apps (a prefix of another app) goes to the longer name
    stems = sorted({r["stem"] for r in rows}, key=len, reverse=True)
    for r in rows:
        r["files"] = [f for f in r["files"] if not any(s != r["stem"] and len(s) > len(r["stem"]) and os.path.splitext(os.path.basename(f))[0].startswith(s) for s in stems)]
    for r in rows:
        appcx, ctx, lines, ratchet, agentp = 0, {k: 0 for k, _ in CONTEXT}, 0, 0, 0
        for f in r["files"]:
            code = code_lines(read(f))
            lines += len(code)
            body = "\n".join(code)
            appcx += len(re.findall(r"\bAppCx\b", body))
            agentp += len(re.findall(r"\biyakki_agent::", body))
            for k, pats in CONTEXT:
                ctx[k] += sum(len(re.findall(p, body)) for p in pats)
            ratchet += allow.get(os.path.relpath(f, ROOT).replace(os.sep, "/"), 0)
        r.update(appcx=appcx, ctx=ctx, lines=lines, ratchet=ratchet, uses=sum(ctx.values()), agent=agentp)
        # an app whose tile crate is extracted: the shell file is a thin adapter (`App as TileApp`) or re-exports the crate (`pub use iyakki_tile_x::...`)
        r["adapter"] = any(re.search(r"App as TileApp", read(f)) or TILE_REEXPORT.search(read(f)) for f in r["files"])
        pas = r["cls"][:-3]
        r["port"] = pas + "Port"
        r["tools"] = bool(re.search(r'"(%s|%s)_[a-z_]+"' % (re.escape(r["stem"].split("_")[0]), re.escape(snake(r["papp"]))), agent))
        r["settings"] = r["papp"] in tabs or pas in tabs
        r["migrated"] = any(os.path.relpath(f, ROOT).replace(os.sep, "/") in mig for f in r["files"])
        if r["migrated"]:
            r["effort"], r["state"] = "-", "migrated (port)"
        elif r["extracted"] or not r["files"] or r["adapter"]:
            r["effort"], r["state"] = "-", "extracted crate (adapter in the shell)"
        else:
            size = r["uses"] + r["agent"] // 2
            e = "S" if size <= 15 else "M" if size <= 60 else "L"
            if r["lines"] > 6000 and e != "L":
                e = {"S": "M", "M": "L"}[e]
            r["effort"], r["state"] = e, "to do"
    todo = [r for r in rows if r["state"] == "to do"]
    todo.sort(key=lambda r: ({"S": 0, "M": 1, "L": 2}[r["effort"]], r["uses"] + r["agent"] // 2 + r["ratchet"] * 5, r["lines"]))
    for i, r in enumerate(todo, 1):
        r["order"] = i
    out = []
    out.append("# Built-in apps: the inventory for the move onto the app standard\n")
    out.append("Written by `python3 dist/scripts/app-inventory.py --write`; do not edit by hand, run the script again (`--check` says when the file is out of date).")
    out.append("The standard and the way to move one app is [APP-DEVELOPMENT-PROCEDURE.md](APP-DEVELOPMENT-PROCEDURE.md) section 10 (worked example: section 12, RADIO).\n")
    out.append("How to read it: **uses** counts, in the non-test code of the app's files, the places that name the services or the shell context (`.services`/`Services`, `cx.host`, `cx.web`, `cx.chat`, `cx.store`, `cx.session`/`cx.audit`); `AppCx` counts the mentions of the context type itself (a migrated tile keeps only the entry points of the `App` trait). **agent paths** counts the `iyakki_agent::` paths (the data types and helpers the app takes from the agent, which an extracted crate cannot); **ratchet** is the sum of the lines the app's files hold in `shell_kernel_access.allow` (direct kernel and hardware access). **chat** says whether the assistant has tools whose names start with the app's name; **settings** whether a Settings page has the app's name. Effort: S up to 15, M up to 60, L above, counting uses plus half the agent paths, one class up for an app of more than 6000 lines. The order is the suggested order for alpha.4: small and quiet first, so that every change is one tile and the procedure is corrected on the easy ones before the hard ones.\n")
    out.append("| Order | App | Files | Lines | AppCx | services | host | web | chat | store | session/audit | uses | agent paths | ratchet | chat tools | settings page | port to write | effort | state |")
    out.append("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    ordered = sorted(rows, key=lambda r: (r.get("order", 10_000), r["papp"]))
    for r in ordered:
        files = ", ".join(sorted({os.path.relpath(f, APPS).replace(os.sep, "/") for f in r["files"]})[:3])
        if len(r["files"]) > 3:
            files += f" and {len(r['files']) - 3} more"
        c = r.get("ctx", {})
        out.append("| %s | %s (`%s`) | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | `%s` | %s | %s |" % (
            r.get("order", "done" if r["state"] != "to do" else "-"), r["papp"], r["cls"], files or "(an extracted crate)", r.get("lines", "-"), r.get("appcx", "-"),
            c.get("services", "-"), c.get("host", "-"), c.get("web", "-"), c.get("chat", "-"), c.get("store", "-"), c.get("session/audit", "-"), r.get("uses", "-"), r.get("agent", "-"), r.get("ratchet", "-"),
            "yes" if r.get("tools") else "no", "yes" if r.get("settings") else "no", r["port"], r["effort"], r["state"]))
    tot = lambda k: sum(r.get(k, 0) for r in rows)
    out.append("")
    out.append("Totals: %d apps, %d migrated or extracted, %d to do (S %d, M %d, L %d); %d uses of the services and the context in the apps still to do." % (
        len(rows), len(rows) - len(todo), len(todo), sum(1 for r in todo if r["effort"] == "S"), sum(1 for r in todo if r["effort"] == "M"), sum(1 for r in todo if r["effort"] == "L"), sum(r["uses"] for r in todo)))
    out.append("")
    out.append("Notes: the count is by occurrence and the file list is by name prefix, so a helper module shared by two apps is counted for the app whose name is the longer prefix; read the numbers as a size class, not as an exact work estimate. An app whose crate was already extracted (`iyakki-tile-*`) shows `an extracted crate`: its source traits are in `iyakki-tile-api` already.")
    text = "\n".join(out) + "\n"
    if "--write" in sys.argv:
        with open(OUT, "w", encoding="utf-8") as f:
            f.write(text)
        print("wrote", os.path.relpath(OUT, ROOT))
    elif "--check" in sys.argv:
        if read(OUT) != text:
            print("docs/development/APP-MIGRATION-INVENTORY.md is out of date: run python3 dist/scripts/app-inventory.py --write")
            sys.exit(1)
        print("up to date")
    else:
        sys.stdout.write(text)


if __name__ == "__main__":
    main()
