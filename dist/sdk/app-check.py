#!/usr/bin/env python3
"""app-check: run ONLY the checks an IYAKKI app author can hit, fast, and print a short numbered fix list.

    python3 dist/sdk/app-check.py PATH [--no-tests] [--cargo "CMD"] [--contract] [--update-golden] [--root CHECKOUT] [--selftest]
    ./run.sh app-check PATH            (the same)

PATH is the folder of an app crate (it holds Cargo.toml, app.toml, src/). Steps:
  1. static checks of the source and the manifests (milliseconds, no build): other IYAKKI dependencies, machine access, colour literals,
     styling numbers, unnamed controls, a tool that changes data but does not ask, app.toml;
  2. the app's own tests (`cargo test`): names, bounds, touch size at three sizes and text 3x, the golden files, the source rules;
  3. with --contract (inside the repository only): the contract tests an app can trip (boundary, dependency_ratchet, ui_agent_uses).
Every problem prints as `N. file:line  [rule]  what is wrong`, then `Fix:` and, where useful, `Before:` and `After:`. Exit code 0 means
no problem, 1 means problems. --cargo gives a command that runs cargo (a wrapper for a remote build host); default is `cargo`. With --cargo and an app inside a
checkout, cargo runs from the checkout root with `--manifest-path <app>/Cargo.toml`, so only the app's tests run, never the workspace's.
--root PATH (or --sdk-checkout PATH) names the IYAKKI checkout cargo runs from; without it the nearest parent that holds crates/iyakki-ui-sdk is used
(a project folder's own .git does not count). --update-golden brings the golden files back when cargo ran elsewhere (the test prints its frames; they are written to tests/golden).
Keep the rules in step with crates/iyakki-ui-sdk/src/lint.rs (the Rust lint is the authority; --selftest checks this port against samples).
"""
import os
import re
import subprocess
import sys
import time

OK_DEPS = ("iyakki-ui-sdk", "iyakki-sdk-tile")
MACHINE = ["std::fs", "std::net", "std::process", "iyakki_uikit", "iyakki_agent", "iyakki_shell", '"/proc', '"/sys', '"/dev']
CHANGING = re.compile(r'"(add|set|delete|remove|write|run|create|save|clear|reset|install|send|open|play)_')


class Problem:
    def __init__(self, rule, where, what, fix, before=None, after=None):
        self.rule, self.where, self.what, self.fix, self.before, self.after = rule, where, what, fix, before, after

    def text(self, n):
        out = [f"{n}. {self.where}  [{self.rule}]  {self.what}", f"   Fix: {self.fix}"]
        if self.before:
            out.append(f"   Before: {self.before}")
        if self.after:
            out.append(f"   After:  {self.after}")
        return "\n".join(out)


def strip_code(line):
    """The line without strings and trailing // comments (for scanning numbers)."""
    out, i, q = [], 0, False
    while i < len(line):
        c = line[i]
        if q:
            if c == "\\":
                i += 2
                continue
            if c == '"':
                q = False
            i += 1
            continue
        if c == '"':
            q = True
        elif c == "/" and line[i:i + 2] == "//":
            break
        else:
            out.append(c)
        i += 1
    return "".join(out)


def style_literals(src):
    """(line number, text) of decimal numbers of 3.0 or more in drawing code: the port of iyakki_ui_sdk::lint::style_literals."""
    out = []
    for i, raw in enumerate(src.split("\n"), 1):
        t = raw.lstrip()
        if t.startswith("#[cfg(test)]"):
            break
        if t.startswith("//") or t.startswith("const ") or t.startswith("pub const "):
            continue
        code = strip_code(raw).replace("em(", "em_(")
        for m in re.finditer(r"(?<![A-Za-z0-9_.])(\d+)\.\d", code):
            start = m.start(1)
            in_em = start >= 4 and code[start - 4:start] == "em_("
            if int(m.group(1)) >= 3 and not in_em:
                out.append((i, raw.strip()))
                break
    return out


UNNAMED = re.compile(r'\bHit\s*\{[^{}]*?\bname\s*:\s*(?:String::new\(\)|""\.into\(\)|""\.to_string\(\)|String::from\(""\)|"")', re.S)


def unnamed_controls(src):
    """Line numbers of a `Hit { .. name: <empty> .. }` (anchored on the Hit construction, so a field called `tpl_name: String::new()` is not one)."""
    return [src[:m.start()].count("\n") + 1 for m in UNNAMED.finditer(src)]


def read(path):
    try:
        return open(path, encoding="utf-8").read()
    except OSError:
        return None


def rs_files(root):
    for dp, dn, fs in os.walk(os.path.join(root, "src")):
        for f in sorted(fs):
            if f.endswith(".rs") and not f.endswith("_tests.rs") and f != "tests.rs":
                yield os.path.join(dp, f)


def events_unhandled(src):
    """Port of lint::events_unhandled: an app that registers for events handles `missed` and looks at `late`."""
    body = src.split("#[cfg(test)]")[0]
    code = "\n".join(l for l in body.split("\n") if not l.strip().startswith("//"))
    if not ("EventRouter" in code or "EventsPort" in code) or not any(k in code for k in (".on(", ".on_prefix(", ".register(")):
        return []
    out = []
    if "on_missed(" not in code and "is_missed()" not in code:
        out.append("missed")
    if ".late" not in code:
        out.append("late")
    if "on_permissions(" not in code:
        out.append("permissions")
    return out


def static_checks(root):
    P = []
    rel = lambda p: os.path.relpath(p, root)
    cargo = read(os.path.join(root, "Cargo.toml"))
    if cargo is None:
        return [Problem("layout", "Cargo.toml", "there is no Cargo.toml in this folder", "pass the folder of the app crate (the one `iyakki new-app` made)")]
    in_deps = False
    for n, l in enumerate(cargo.split("\n"), 1):
        if l.startswith("["):
            in_deps = l.strip() == "[dependencies]"
        elif in_deps and re.match(r"\s*iyakki-", l) and not l.strip().startswith(OK_DEPS):
            name = l.split("=")[0].strip()
            P.append(Problem("manifest-dependency", f"Cargo.toml:{n}", f"the app depends on `{name}`", "an app depends on iyakki-ui-sdk (and iyakki-sdk-tile for a tile) and no other IYAKKI crate; reach the machine through a port. If you need a new port, stop and ask the owner.", f"{l.strip()}", "(delete the line)"))
    manifest = read(os.path.join(root, "app.toml"))
    lib = read(os.path.join(root, "src", "lib.rs")) or ""
    if manifest is None:
        P.append(Problem("manifest", "app.toml", "there is no app.toml", "add app.toml with id, title, version, ui_sdk = 1, display_list = 1, min_width, min_height, ports"))
    else:
        mid = re.search(r'^id\s*=\s*"([^"]+)"', manifest, re.M)
        cid = re.search(r'pub const ID: &str = "([^"]+)"', lib)
        if not mid:
            P.append(Problem("manifest", "app.toml", "no `id = \"...\"`", 'add `id = "your-app"`'))
        elif cid and cid.group(1) != mid.group(1):
            P.append(Problem("manifest", "app.toml", f"id `{mid.group(1)}` differs from ID `{cid.group(1)}` in src/lib.rs", "make both the same text"))
        if "ports" not in manifest:
            P.append(Problem("manifest", "app.toml", "no `ports = [...]` line", 'list the ports the app uses, for example `ports = ["storage"]` (an empty list means none)'))
    for path in rs_files(root):
        src = read(path) or ""
        lines = src.split("\n")
        body = src.split("#[cfg(test)]")[0]
        for n, l in enumerate(lines[:len(body.split("\n"))], 1):
            t = l.strip()
            if t.startswith("//"):
                continue
            for bad in MACHINE:
                if bad in l and not (bad == "iyakki_uikit" and "use iyakki_ui_sdk" in l):
                    fixes = {"std::fs": "use a FilesPort (the person's home folder) or the StoragePort", "std::net": "use a BrowserPort or the assistant; an app opens no socket", "std::process": "use a TerminalPort (under the person's policy and approval)"}
                    P.append(Problem("machine-access", f"{rel(path)}:{n}", f"source names `{bad}`", fixes.get(bad, "the app never reaches Linux, a device or another IYAKKI layer itself; ask the core through a port the host hands in"), t, "self.files.read_text(path, MAX)  // through a port"))
            if re.search(r"\brgba?\(", l) or re.search(r"\[\s*\d{1,3}\s*,\s*\d{1,3}\s*,\s*\d{1,3}\s*(,\s*\d{1,3}\s*)?\]", strip_code(l)):
                P.append(Problem("colour-literal", f"{rel(path)}:{n}", "a colour written in the source", "take colours from the Look: `let th = cx.colors();` then `th.text`, `th.muted`, `th.accent`, `th.card`, `th.border`; an own accent goes through `cx.accent(wanted)`", t, "c.text(x, y, size, th.text, \"Hello\", TextStyle::Regular)"))
        for n in unnamed_controls(src):
            P.append(Problem("unnamed-control", f"{rel(path)}:{n}", "a Hit with an empty name", "every control has a name: the visible label (the screen reader says it and the tests find the control by it)", lines[n - 1].strip(), 'Hit { action, name: "Save".into(), role: Role::Button }'))
        if path.endswith(("lib.rs", "ports.rs")) or "/src/" in path:
            for n, t in style_literals(src):
                P.append(Problem("styling-number", f"{rel(path)}:{n}", "a number of 3.0 or more in drawing code", "use the skin's steps: text `m.size(TypeRole::Body)`, gap `m.gap(Space::Md)`, radius `m.radius(Radius::Md)`, line `m.line(Line::Normal)`, target `m.min_target()`. A count or divisor (three tabs, `/ 3.0`) is named in a const: `const TABS: f32 = 3.0;` (the lint lets const lines through)", t, "c.text(x, y, m.size(TypeRole::Body), th.text, s, style); let gap = m.gap(Space::Md);"))
        for m in re.finditer(r'ToolSpec::new\(\s*("[A-Za-z_]+")[^;]*?,\s*(true|false)\s*\)', src, re.S):
            if m.group(2) == "false" and CHANGING.match(m.group(1)):
                n = src[:m.start()].count("\n") + 1
                P.append(Problem("tool-should-ask", f"{rel(path)}:{n}", f"tool {m.group(1)} sounds like it changes something but is declared with ask = false", "a tool that changes data or the machine asks the person every time: `ask = true` (a card is drawn and the core logs it); only a pure read is `false`", f"ToolSpec::new({m.group(1)}, ..., false)", f"ToolSpec::new({m.group(1)}, ..., true)"))
    all_src = "\n".join(read(p) or "" for p in rs_files(root))
    lack = events_unhandled(all_src)
    if "missed" in lack:
        P.append(Problem("events-missed", "src/", "the app registers for events but never handles the `missed` marker", "add `.on_missed(|e, acts| ...)` to the EventRouter: events were dropped (queue full, too old, too noisy), so re-read the state the app mirrors", 'EventRouter::new().on("connectivity.changed", h)', 'EventRouter::new().on("connectivity.changed", h).on_missed(|_, _| reload())'))
    if "late" in lack:
        P.append(Problem("events-late", "src/", "the app registers for events but never looks at `late`", "an event that arrives `late` happened while the app was not running: check `e.late` before you ring, animate or start something", "|e, acts| acts.notify(\"Offline\")", "|e, acts| if !e.late { acts.notify(\"Offline\") }"))
    if "permissions" in lack:
        P.append(Problem("events-permissions", "src/", "the app registers for events but never handles `app.permissions`", "add `.on_permissions(|change, e, acts| ...)` to the EventRouter: the person or a policy can take a permission away at any time, and the app must stop using it and read `port.capabilities()` again", 'EventRouter::new().on("x", h)', 'EventRouter::new().on("x", h).on_permissions(|c, _, _| refresh(c))'))
    has_tests = os.path.isdir(os.path.join(root, "tests")) or any(f.endswith("tests.rs") for dp, _, fs in os.walk(os.path.join(root, "src")) for f in fs)
    if not has_tests:
        P.append(Problem("tests", "tests/", "the app has no tests folder", "copy tests/golden.rs from an app made by `iyakki new-app`: names, bounds, touch size, input, source rules"))
    return P


FAIL_HELP = [
    ("frame changed", "golden", "the drawing changed", "if you meant it: delete tests/golden and run the tests once, read the diff, commit; if not, undo the change that moved things"),
    ("a control with no name", "unnamed-control", "a control has no name", "give the widget its visible label, or a non-empty Hit.name"),
    ("leaves the pane", "bounds", "something is outside the pane at one of the three sizes or at 3x text", "lay out from c.size() and m.gutter(); shorten with fit(); do not use pixel constants"),
    ("too small to touch", "target-size", "a target is under 44 units on touch", "use the widgets or make the rectangle m.min_target() high"),
    ("styling numbers", "styling-number", "a number of 3.0 or more in drawing code", "use m.size / m.gap / m.radius / m.line (see themes-skins-fonts.md)"),
    ("contains", "source-rules", "forbidden text in the source", "see the WHAT/FIX lines above"),
    ("a dependency besides the SDK", "manifest-dependency", "another IYAKKI crate in [dependencies]", "remove it"),
]


def parse_test_output(text):
    """Problems from `cargo test` output: compile errors, and failed tests with the WHAT/FIX the test printed."""
    P = []
    m = re.search(r"^error(\[E\d+\])?: .*$", text, re.M)
    if m and "could not compile" in text:
        loc = re.search(r"-->\s+([^\s:]+:\d+)", text[m.start():])
        P.append(Problem("compile", loc.group(1) if loc else "cargo", m.group(0)[:200], "fix the compiler error first (run `cargo build` for the full text); the other checks need a build"))
        return P
    for m in re.finditer(r"---- (\S+) stdout ----\n(.*?)(?=\n---- |\n\nfailures:|\Z)", text, re.S):
        name, body = m.group(1), m.group(2)
        msg = re.sub(r"^thread '.*?' .*? panicked at (\S+):\n", "", body, flags=re.M).strip()
        at = re.search(r"panicked at ([^\s:]+:\d+)", body)
        rule = next((h for h in FAIL_HELP if h[0] in body), None)
        if at and "starter.rs" in at.group(1):
            P.append(Problem("starter-test", at.group(1), f"test `{name}` of the STARTER counter fails because the app no longer behaves like the starter", "tests/starter.rs describes the app `iyakki new-app` made, not yours: rewrite it for what your app does (tap its controls through `Host`, check its texts) or delete the file; keep the checks in tests/golden.rs"))
            continue
        what = next((l for l in msg.split("\n") if l.startswith("WHAT:")), None)
        fix = next((l for l in msg.split("\n") if l.startswith("FIX:")), None)
        if what or fix:
            P.append(Problem(rule[1] if rule else "test", at.group(1) if at else name, (what or "").replace("WHAT: ", "") or f"test {name} failed", (fix or "").replace("FIX: ", "") or (rule[3] if rule else "read the message")))
        else:
            first = msg.split("\n")[0][:200] if msg else "failed"
            P.append(Problem(rule[1] if rule else "test", at.group(1) if at else name, f"test `{name}` failed: {first}", rule[3] if rule else "read the failing test; it names what it expects"))
    return P


def find_top(root, explicit=None):
    """The IYAKKI checkout for a wrapper run: --root/--sdk-checkout when given, else the nearest parent that contains crates/iyakki-ui-sdk (a .git folder
    alone does not count: a project folder with its own repository is not the checkout). None: a standalone project."""
    if explicit:
        return os.path.abspath(explicit)
    top = os.path.dirname(root)
    while top != os.path.dirname(top):
        if os.path.isfile(os.path.join(top, "crates", "iyakki-ui-sdk", "Cargo.toml")):
            return top
        top = os.path.dirname(top)
    return None


def cargo_call(root, cargo_cmd, wrapper, args, explicit=None):
    """(argv, cwd) that runs `cargo ARGS` for the app only. With --cargo (a wrapper that copies a checkout to a build host) and an app inside a
    checkout: run from the checkout root with --manifest-path of the app, so only the app's tests run (never the whole workspace). A standalone
    app (or no wrapper) runs in the app's own folder."""
    top = find_top(root, explicit)
    if wrapper and top:
        return cargo_cmd.split() + args + ["--manifest-path", os.path.relpath(os.path.join(root, "Cargo.toml"), top)], top
    return cargo_cmd.split() + args, root


DUMP_BLOCK = """    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    if dir.join(".dump").exists() {
        println!("GOLDEN-BEGIN {name}\\n{got}\\nGOLDEN-END {name}");
        continue;
    }"""


def update_golden(root, cargo_cmd, wrapper, explicit=None):
    """Make the golden test print its frames (marker file tests/golden/.dump, which travels with the sources) and write them here."""
    gdir = os.path.join(root, "tests", "golden")
    os.makedirs(gdir, exist_ok=True)
    marker = os.path.join(gdir, ".dump")
    open(marker, "w").close()
    try:
        argv, cwd = cargo_call(root, cargo_cmd, wrapper, ["test", "--test", "golden", "--", "--nocapture"], explicit)
        p = subprocess.run(argv, cwd=cwd, capture_output=True, text=True)
    finally:
        os.remove(marker)
    got = re.findall(r"GOLDEN-BEGIN (\S+)\n(.*?)\nGOLDEN-END \1", p.stdout, re.S)
    if not got:
        src = read(os.path.join(root, "tests", "golden.rs")) or ""
        if "GOLDEN-BEGIN" not in src:
            print("app-check --update-golden: tests/golden.rs has no dump mode (it was rewritten). Put this at the top of the loop that compares each frame (`name` and `got` are the size name and the frame text):\n")
            print(DUMP_BLOCK)
            return 1
        print("app-check --update-golden: the golden test printed no frames; run `cargo test --test golden` and read its error")
        print((p.stdout + p.stderr)[-600:])
        return 1
    for name, text in got:
        open(os.path.join(gdir, name + ".txt"), "w", encoding="utf-8").write(text)
        print(f"wrote tests/golden/{name}.txt")
    print("Read the diff (git diff tests/golden) before you commit it.")
    return 0


def run_tests(root, cargo_cmd, contract, wrapper=False, explicit=None):
    argv, cwd = cargo_call(root, cargo_cmd, wrapper, ["test", "--quiet"], explicit)
    p = subprocess.run(argv, cwd=cwd, capture_output=True, text=True)
    text = p.stdout + "\n" + p.stderr
    probs = parse_test_output(text) if p.returncode != 0 else []
    if p.returncode != 0 and not probs:
        probs.append(Problem("cargo", "cargo test", "cargo test failed with no test failure to show", "run `cargo test` yourself and read the first error: " + (text.strip().split("\n")[-1][:160])))
    if contract:
        top = root
        while top != "/" and not os.path.isfile(os.path.join(top, "crates", "iyakki-core", "Cargo.toml")):
            top = os.path.dirname(top)
        if top == "/":
            probs.append(Problem("contract", "--contract", "the app is not inside the IYAKKI repository", "run --contract from an app crate under crates/"))
        else:
            args = ["test", "--quiet", "-p", "iyakki-core", "--test", "boundary", "--test", "dependency_ratchet", "--test", "ui_agent_uses"]
            q = subprocess.run(cargo_cmd.split() + args, cwd=top, capture_output=True, text=True)
            if q.returncode != 0:
                t = q.stdout + "\n" + q.stderr
                for m in re.finditer(r"(WHAT:.*?)(?=\n\n|\Z)", t, re.S):
                    block = m.group(1)
                    probs.append(Problem("contract", "contract tests", block.split("\n")[0].replace("WHAT: ", "")[:200], (re.search(r"FIX:(.*)", block) or [None, "read the test output"])[1].strip()[:400]))
                if not any(p.rule == "contract" for p in probs):
                    probs.append(Problem("contract", "contract tests", "a contract test failed", "run `cargo test -p iyakki-core --test boundary --test dependency_ratchet --test ui_agent_uses` and read the first failure"))
    return probs


def selftest():
    bad = style_literals("fn v() {\n  c.text(x, y, 14.0, th.text, \"a 5.5 b\", s);\n  let g = 8.0; // ok? no\n  let h = m.em(3.5);\n  let z = 2.5;\n}\nconst K: f32 = 9.0;\n#[cfg(test)]\nmod t { let a = 99.9; }\n")
    assert [n for n, _ in bad] == [2, 3], bad
    here = os.path.dirname(os.path.abspath(__file__))
    rec = os.path.join(here, "..", "..", "docs", "sdk", "recipes")
    if os.path.isdir(rec):
        for f in sorted(os.listdir(rec)):
            if f.endswith(".rs"):
                assert not style_literals(read(os.path.join(rec, f))), f
    assert events_unhandled('EventRouter::new().on("x", |e, a| {});') == ["missed", "late", "permissions"]
    assert events_unhandled('EventRouter::new().on("x", |e, a| { if e.late {} }).on_missed(|_, _| {}).on_permissions(|c, e, a| {});') == []
    assert events_unhandled("fn a() {}") == []
    assert unnamed_controls("let h = Hit { action: 1, name: String::new(), role: Role::Button };") == [1]
    assert unnamed_controls("c.hit(r, Hit {\n action: a,\n name: \"\".into(),\n role });") == [1]
    assert not unnamed_controls("struct T { tpl_name: String::new(), name2: String::new() }\nlet h = Hit { action: 1, name: label.to_string(), role: Role::Button, extra: String::new() };")
    out = parse_test_output("---- x::t stdout ----\n\nthread 'x::t' (1) panicked at tests/golden.rs:12:5:\nWHAT: a control has no name.\nFIX: give it a label.\n\n\nfailures:\n")
    assert len(out) == 1 and "no name" in out[0].what and out[0].fix == "give it a label.", out[0].text(1)
    comp = parse_test_output("error[E0425]: cannot find value `x` in this scope\n --> src/lib.rs:5:9\nerror: could not compile `a`\n")
    assert comp and comp[0].where == "src/lib.rs:5", comp[0].where
    import tempfile
    with tempfile.TemporaryDirectory() as d:
        os.makedirs(os.path.join(d, "co", "crates", "iyakki-ui-sdk"))
        open(os.path.join(d, "co", "crates", "iyakki-ui-sdk", "Cargo.toml"), "w").close()
        app = os.path.join(d, "co", "crates", "iyakki-app-x")
        os.makedirs(app)
        argv, cwd = cargo_call(app, "wrap DIR", True, ["test"])
        assert cwd == os.path.join(d, "co") and argv == ["wrap", "DIR", "test", "--manifest-path", "crates/iyakki-app-x/Cargo.toml"], (argv, cwd)
        argv, cwd = cargo_call(app, "cargo", False, ["test"])
        assert cwd == app and argv == ["cargo", "test"]
        proj = os.path.join(d, "billing", "iyakki-app-b")
        os.makedirs(os.path.join(d, "billing", ".git"))
        os.makedirs(proj)
        assert find_top(proj) is None, "a project's own .git is not the checkout"
        assert find_top(proj, os.path.join(d, "co")) == os.path.join(d, "co")
        argv, cwd = cargo_call(proj, "wrap", True, ["test"], os.path.join(d, "co"))
        assert cwd == os.path.join(d, "co") and argv[-1] == "../billing/iyakki-app-b/Cargo.toml", argv
        lone = os.path.join(d, "lone")
        os.makedirs(lone)
        assert cargo_call(lone, "wrap", True, ["test"])[1] == lone
    print("app-check selftest: ok")
    return 0


def main(argv):
    if "--selftest" in argv:
        return selftest()
    args = [a for a in argv if not a.startswith("--")]
    flags = {}
    for i, a in enumerate(argv):
        if a in ("--cargo", "--root", "--sdk-checkout") and i + 1 < len(argv):
            flags["cargo" if a == "--cargo" else "root"] = argv[i + 1]
            args = [x for x in args if x != argv[i + 1]]
    if not args:
        print(__doc__)
        return 2
    root = os.path.abspath(args[0])
    if "--update-golden" in argv:
        return update_golden(root, flags.get("cargo", "cargo"), "cargo" in flags, flags.get("root"))
    t0 = time.time()
    probs = static_checks(root)
    t_static = time.time() - t0
    if "--no-tests" not in argv and not any(p.rule == "layout" for p in probs):
        probs += run_tests(root, flags.get("cargo", "cargo"), "--contract" in argv, "cargo" in flags, flags.get("root"))
    # one line per (rule, where)
    seen, uniq = set(), []
    for p in probs:
        k = (p.rule, p.where, p.what)
        if k not in seen:
            seen.add(k)
            uniq.append(p)
    if not uniq:
        print(f"app-check: no problems ({time.time() - t0:.1f}s; static part {t_static * 1000:.0f} ms)")
        return 0
    print(f"app-check: {len(uniq)} problem{'s' if len(uniq) != 1 else ''} ({time.time() - t0:.1f}s)\n")
    for n, p in enumerate(uniq, 1):
        print(p.text(n))
    print("\nEscalate (do not guess): a new core facet or port, an approval card's wording, a privacy rule, a contract test failing in another crate.")
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
