#!/usr/bin/env python3
"""Re-point FEATURES.md test citations after tests moved from one crate to another (COMPILE-SPLIT-PILOT-2.md).

usage: compile-split-rename.py OLD_CRATE NEW_CRATE [--apply] [--tsv FILE]

A cited test `OLD_CRATE::<path>::<name>` whose `fn <name>(` is gone from OLD_CRATE (src and tests) and defined in NEW_CRATE becomes
`NEW_CRATE::<module path in NEW_CRATE>::<name>`. The module path comes from the file (`src/a/b.rs` -> `a::b`, `src/x_tests.rs` ->
`x_tests`, a `mod tests` below it -> `::tests`; `lib.rs` and `mod.rs` add nothing). Without --apply it only prints; --tsv writes the old/new
table that the earlier steps kept next to the docs. Names that exist in both crates are left alone (they are not moved, they are copies).
"""
import re, sys
from pathlib import Path

root = Path(__file__).resolve().parents[2]


def fn_index(crate):
    """name -> list of module paths where `fn name(` is defined in the crate's src/ and tests/."""
    idx = {}
    base = root / "crates" / crate
    for sub in ("src", "tests"):
        for f in sorted((base / sub).rglob("*.rs")):
            rel = f.relative_to(base / sub).with_suffix("")
            parts = [p for p in rel.parts if p not in ("lib", "mod", "main")]
            text = f.read_text(errors="replace")
            # `x_tests.rs` pulled in by `#[path = "x_tests.rs"] mod tests;` in x.rs is the module `x::tests`
            if sub == "src" and rel.name.endswith("_tests"):
                parent = f.with_name(rel.name[: -len("_tests")] + ".rs")
                if parent.exists() and f'#[path = "{f.name}"]' in parent.read_text(errors="replace"):
                    parts = parts[:-1] + [rel.name[: -len("_tests")], "tests"]
            m = re.search(r"^\s*mod tests\b", text, re.M)
            tests_at = m.start() if m else None
            for fm in re.finditer(r"\bfn ([A-Za-z0-9_]+)\(", text):
                mod = list(parts)
                if sub == "src" and tests_at is not None and fm.start() > tests_at and not str(rel).endswith("_tests"):
                    mod.append("tests")
                idx.setdefault(fm.group(1), []).append("::".join(mod))
    return idx


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    old, new = args[0], args[1]
    apply = "--apply" in sys.argv
    tsv = sys.argv[sys.argv.index("--tsv") + 1] if "--tsv" in sys.argv else None
    old_idx, new_idx = fn_index(old), fn_index(new)
    text = (root / "FEATURES.md").read_text()
    rows, missing = [], []

    def fix(m):
        cited = m.group(0)
        name = cited.rsplit("::", 1)[1]
        if name in old_idx:
            return cited
        if name not in new_idx:
            return cited  # prose that names a module, not a test
        mods = new_idx[name]
        mod = mods[0]
        result = f"{new}::{mod}::{name}" if mod else f"{new}::{name}"
        rows.append((cited, result))
        return result

    out = re.sub(re.escape(old) + r"::[A-Za-z0-9_:]*[A-Za-z0-9_]", fix, text)
    for o, n in rows:
        print(f"{o}\t{n}")
    for c in missing:
        print("NOT FOUND ANYWHERE:", c, file=sys.stderr)
    if tsv:
        Path(tsv).write_text("old test path\tnew test path\n" + "".join(f"{o}\t{n}\n" for o, n in rows))
    if apply:
        (root / "FEATURES.md").write_text(out)
    print(f"{len(rows)} citations {'rewritten' if apply else 'would be rewritten'}", file=sys.stderr)


main()
