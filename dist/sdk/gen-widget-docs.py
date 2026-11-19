#!/usr/bin/env python3
"""Generate the SDK reference pages from the code, and keep the docs' code snippets equal to the files the tests compile.

    python3 dist/sdk/gen-widget-docs.py            write docs/sdk/reference/*.md and refresh the snippet blocks in docs/sdk/*.md
    python3 dist/sdk/gen-widget-docs.py --check    change nothing; exit 1 and name every stale file (the test
                                                   crates/iyakki-ui-sdk/tests/docs_fresh.rs runs this)

What it reads: the public API and the `///` and `//!` comments of the modules of crates/iyakki-ui-sdk/src (the list is PAGES below; a module
that is in no page makes the check fail, so a new component cannot be forgotten). What it does not do: invent text. If an item has no doc
comment the reference shows none; write the comment in the code, not in the page.

Snippets: a page in docs/sdk/ holds
    <!-- snippet: embed_files.rs -->
    ```rust ... ```
    <!-- /snippet -->
and the block between the markers is replaced by the file docs/sdk/recipes/embed_files.rs (the file the doc-test compiles). So an example
in the guide is always an example that builds.
"""
import os
import re
import sys

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
SRC = os.path.join(ROOT, "crates", "iyakki-ui-sdk", "src")
OUT = os.path.join(ROOT, "docs", "sdk", "reference")
GUIDE = os.path.join(ROOT, "docs", "sdk")
SNIP = os.path.join(GUIDE, "recipes")

# page file, title, one line of what it is, modules
PAGES = [
    ("app-and-host", "The app, its context and the host", "What an app implements (`UiApp`), what it is handed on every call (`UiCx`), the driver (`Host`), input events and the storage port.",
     ["app.rs", "cx.rs", "host.rs", "event.rs", "gesture.rs", "ports.rs"]),
    ("drawing", "Drawing: Canvas, geometry and the display list", "The one drawing trait, the shapes it uses, the recording canvas and the wire format.",
     ["canvas.rs", "geom.rs", "list.rs", "path.rs", "raster.rs", "wire.rs"]),
    ("widgets-and-style", "Widgets, size classes, tokens and fonts", "The small widgets, the size classes and metrics, the design tokens, the font roles and the style-literal lint.",
     ["widgets.rs", "size.rs", "tokens.rs", "typeface.rs", "lint.rs"]),
    ("desktop", "Desktop parts: dock, wallpaper, screens, panes", "What the shell gives an app about its place: arrangement, dock port, wallpaper port, status.",
     ["desktop.rs"]),
    ("chat", "ChatView: the assistant in your layout", "The assistant as a component, its port, tools, context and the scripted test port.",
     ["chatview.rs", "assistant.rs", "toolargs.rs"]),
    ("browser", "BrowserView: a web page in your layout", "The embedded browser component and its port.",
     ["browser.rs"]),
    ("files", "FilesView: the person's files", "The file browser component and its port.",
     ["filesview.rs", "files.rs"]),
    ("terminal", "TerminalView: the command line", "The terminal component and its port.",
     ["terminalview.rs", "terminal.rs"]),
    ("editor", "EditorView: a text editor", "The editor component (the engine is `iyakki-edit`, re-used through the SDK).",
     ["editorview.rs"]),
    ("media", "AudioView, MediaView and the media port", "The sound player component and the media port (video is `MediaView`, planned).",
     ["audioview.rs", "mediaview.rs", "media.rs"]),
    ("barcode", "Barcodes: Code 128, EAN-13, UPC-A", "In-house encoders (no table to recall) and a view that draws them as dark bars on light paper.",
     ["barcode.rs", "barcodeview.rs"]),
    ("camera", "CameraView: a camera in your layout", "The camera component and its port.",
     ["cameraview.rs", "cameras.rs"]),
    ("events", "Events: hear what happens, online and offline", "The events port, the router with its handlers and actions, and the in-memory port.",
     ["events.rs"]),
]
# modules that are not API: tests
SKIP = re.compile(r"(_tests|^tests)\.rs$|^lib\.rs$")

ITEM = re.compile(r"^pub\s+(?:unsafe\s+)?(struct|enum|trait|fn|const|static|type)\s+([A-Za-z_][A-Za-z0-9_]*)")


def code_of(line):
    """The line without a trailing // comment and without the text of strings (for brace counting)."""
    out, i, n, q = [], 0, len(line), False
    while i < n:
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
        elif c == "'" and i + 2 < n and (line[i + 2] == "'" or (line[i + 1] == "\\" and i + 3 < n and line[i + 3] == "'")):
            i += 3 if line[i + 1] != "\\" else 4
            continue
        elif c == "/" and i + 1 < n and line[i + 1] == "/":
            break
        else:
            out.append(c)
        i += 1
    return "".join(out)


def sig_from(lines, i):
    """The declaration starting at line i, up to its `{` or `;`, on one line, and the index of its last line."""
    parts, j = [], i
    while j < len(lines):
        c = code_of(lines[j]).strip()
        parts.append(c)
        joined = " ".join(parts)
        if "{" in c or c.endswith(";"):
            break
        j += 1
    s = " ".join(parts)
    cut = len(s)
    for ch in ("{",):
        k = s.find(ch)
        if k != -1:
            cut = min(cut, k)
    s = s[:cut].strip().rstrip(";").strip()
    s = re.sub(r"\s+", " ", s)
    s = re.sub(r"\(\s+", "(", s)
    s = re.sub(r",\s*\)", ")", s)
    return s, j


def first_para(docs):
    out = []
    for d in docs:
        if not d.strip():
            if out:
                break
            continue
        out.append(d.strip())
    return " ".join(out)


class Item:
    def __init__(self, kind, name, sig, docs):
        self.kind, self.name, self.sig, self.docs = kind, name, sig, docs
        self.members = []  # (sig, docs)
        self.methods = []  # (sig, docs)


def parse(path):
    lines = open(path, encoding="utf-8").read().split("\n")
    mod_docs, items = [], []
    i, depth, docs, skip_until = 0, 0, [], None
    # the module doc
    while i < len(lines) and lines[i].startswith("//!"):
        mod_docs.append(lines[i][3:].strip() if len(lines[i]) > 3 else "")
        i += 1
    cur, cur_depth, cur_impl = None, None, None
    cfg_test = False
    while i < len(lines):
        raw = lines[i]
        t = raw.strip()
        if depth == 0 and t.startswith("#[cfg(test)]"):
            # everything from the test module on is not API
            break
        if t.startswith("///"):
            docs.append(t[3:][1:] if t.startswith("/// ") else t[3:])
            i += 1
            continue
        if t.startswith("#[") or t.startswith("//"):
            i += 1
            continue
        if t == "":
            docs = []
            i += 1
            continue
        c = code_of(raw)
        opened, closed = c.count("{"), c.count("}")
        if depth == 0:
            m = ITEM.match(t)
            if m:
                kind, name = m.group(1), m.group(2)
                sig, last = sig_from(lines, i)
                if kind in ("const", "static", "type"):
                    full = " ".join(code_of(lines[k]).strip() for k in range(i, last + 1))
                    sig = re.sub(r"\s+", " ", full).rstrip(";").strip() if "=" in full else sig
                it = Item(kind, name, sig, docs)
                items.append(it)
                cur = it if kind in ("struct", "enum", "trait") else None
                cur_impl = None
                # consume the declaration lines, tracking braces
                for k in range(i, last + 1):
                    ck = code_of(lines[k])
                    depth += ck.count("{") - ck.count("}")
                cur_depth = depth
                i = last + 1
                docs = []
                continue
            im = re.match(r"^impl(?:<[^>]*>)?\s+([A-Za-z_][A-Za-z0-9_<>:, ']*?)\s*(?:where\b.*)?\{?$", t)
            if im and " for " not in t:
                nm = re.match(r"([A-Za-z_][A-Za-z0-9_]*)", im.group(1))
                target = nm.group(1) if nm else None
                cur_impl = next((x for x in items if x.name == target), None)
                depth += opened - closed
                docs = []
                i += 1
                continue
        elif depth == 1:
            # members of the struct, enum or trait, or methods of an inherent impl
            if cur is not None and cur_impl is None and cur.kind in ("struct", "enum"):
                if cur.kind == "struct":
                    mm = re.match(r"^pub\s+([a-z_][a-z0-9_]*)\s*:\s*(.*?),?$", code_of(raw).strip())
                    if mm:
                        cur.members.append((f"pub {mm.group(1)}: {mm.group(2).rstrip(',')}", docs))
                else:
                    mm = re.match(r"^([A-Z][A-Za-z0-9_]*)\b", t)
                    if mm:
                        cur.members.append((code_of(raw).strip().rstrip(","), docs))
            elif cur is not None and cur.kind == "trait" and cur_impl is None:
                mm = re.match(r"^fn\s+([a-z_][a-z0-9_]*)", t)
                if mm:
                    sig, last = sig_from(lines, i)
                    has_body = "{" in " ".join(code_of(lines[k]) for k in range(i, last + 1))
                    cur.members.append((sig + ("  // has a default" if has_body else ""), docs))
            if cur_impl is not None:
                mm = re.match(r"^pub\s+(?:const\s+)?fn\s+([a-z_][a-z0-9_]*)", t)
                if mm:
                    sig, last = sig_from(lines, i)
                    cur_impl.methods.append((sig, docs))
                    for k in range(i, last + 1):
                        ck = code_of(lines[k])
                        depth += ck.count("{") - ck.count("}")
                    i = last + 1
                    docs = []
                    continue
        depth += opened - closed
        if depth < 0:
            depth = 0
        if depth == 0:
            cur_impl = None if cur_impl is not None and closed else cur_impl
            if closed and cur is not None:
                cur = None
        docs = []
        i += 1
    return mod_docs, items


def fence(s):
    return "```rust\n" + s + "\n```"


def render_module(fname, mod_docs, items):
    out = [f"## `{fname[:-3]}`  (`crates/iyakki-ui-sdk/src/{fname}`)", ""]
    md = "\n".join(mod_docs).strip()
    if md:
        out += [md, ""]
    for it in items:
        out.append(f"### {it.kind} `{it.name}`")
        out.append("")
        d = "\n".join(it.docs).strip()
        if d:
            out += [d, ""]
        out += [fence(it.sig), ""]
        if it.members:
            lab = {"struct": "Public fields", "enum": "Variants", "trait": "Methods"}[it.kind]
            out.append(f"{lab}:")
            out.append("")
            for sig, docs in it.members:
                p = first_para(docs)
                out.append(f"- `{sig}`" + (f": {p}" if p else ""))
            out.append("")
        if it.methods:
            out.append("Methods:")
            out.append("")
            for sig, docs in it.methods:
                p = first_para(docs)
                out.append(f"- `{sig}`" + (f": {p}" if p else ""))
            out.append("")
    return "\n".join(out)


def build():
    """{path: text} of every generated file, and the list of problems."""
    files, problems = {}, []
    listed = set()
    index = ["# SDK reference (generated)", "",
             "These pages are written by `python3 dist/sdk/gen-widget-docs.py` from the public API and the doc comments of `crates/iyakki-ui-sdk/src`. Do not edit them: change the comment in the code and run the script. The test `crates/iyakki-ui-sdk/tests/docs_fresh.rs` fails when they are stale.", "",
             "| Page | What is in it |", "|---|---|"]
    for slug, title, blurb, mods in PAGES:
        have = [m for m in mods if os.path.exists(os.path.join(SRC, m))]
        for m in mods:
            listed.add(m)
        if not have:
            continue
        body = [f"# {title}", "", "<!-- Generated by dist/sdk/gen-widget-docs.py from the code. Do not edit by hand. -->", "", blurb, ""]
        for m in have:
            mod_docs, items = parse(os.path.join(SRC, m))
            body.append(render_module(m, mod_docs, items))
        files[os.path.join(OUT, slug + ".md")] = "\n".join(body).rstrip() + "\n"
        index.append(f"| [{title}]({slug}.md) | {blurb} |")
    files[os.path.join(OUT, "README.md")] = "\n".join(index) + "\n"
    for f in sorted(os.listdir(SRC)):
        if f.endswith(".rs") and not SKIP.search(f) and f not in listed:
            problems.append(f"crates/iyakki-ui-sdk/src/{f} is in no page of dist/sdk/gen-widget-docs.py (add it to PAGES)")
    return files, problems


SNIP_RE = re.compile(r"(<!-- snippet: ([A-Za-z0-9_.\-]+) -->\n)(.*?)(<!-- /snippet -->)", re.S)


def sync_snippets(path, text, problems):
    def rep(m):
        name = m.group(2)
        sp = os.path.join(SNIP, name)
        if not os.path.exists(sp):
            problems.append(f"{os.path.relpath(path, ROOT)}: snippet {name} does not exist in docs/sdk/recipes")
            return m.group(0)
        code = open(sp, encoding="utf-8").read().rstrip("\n")
        return m.group(1) + "```rust\n" + code + "\n```\n" + m.group(4)
    return SNIP_RE.sub(rep, text)


def main():
    check = "--check" in sys.argv
    files, problems = build()
    stale = []
    for p, text in files.items():
        cur = open(p, encoding="utf-8").read() if os.path.exists(p) else None
        if cur != text:
            stale.append(p)
            if not check:
                os.makedirs(os.path.dirname(p), exist_ok=True)
                open(p, "w", encoding="utf-8").write(text)
    # stale reference files of pages that no longer exist
    if os.path.isdir(OUT):
        for f in os.listdir(OUT):
            p = os.path.join(OUT, f)
            if f.endswith(".md") and p not in files:
                stale.append(p)
                if not check:
                    os.remove(p)
    # the snippet blocks of the guide pages
    for dp, _, fs in os.walk(GUIDE):
        if os.path.abspath(dp).startswith(os.path.abspath(OUT)):
            continue
        for f in fs:
            if not f.endswith(".md"):
                continue
            p = os.path.join(dp, f)
            text = open(p, encoding="utf-8").read()
            new = sync_snippets(p, text, problems)
            if new != text:
                stale.append(p)
                if not check:
                    open(p, "w", encoding="utf-8").write(new)
    # snippets nobody shows are fine (tests compile them); a page that names a missing one is reported above
    for p in problems:
        print("problem:", p)
    if check:
        for p in stale:
            print("stale:", os.path.relpath(p, ROOT))
        if stale or problems:
            print("run: python3 dist/sdk/gen-widget-docs.py")
            return 1
        print("docs/sdk is up to date")
        return 0
    for p in stale:
        print("wrote:", os.path.relpath(p, ROOT))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
