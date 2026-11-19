#!/usr/bin/env python3
"""Layered tests and green stamps (alpha.5 item 0b).

  dist/scripts/testlayers.py check                 every workspace crate has a layer (dist/scripts/crate-layers.txt, no catch-all); exit 1 if not
  dist/scripts/testlayers.py crates GROUP [--p]    the crates of a test group, one per line (--p: as `-p a -p b` for cargo)
  dist/scripts/testlayers.py hash GROUP            the input hash of a group (exit 1 and a reason when the inputs are not committed)
  dist/scripts/testlayers.py verify GROUP          exit 0 when a green stamp matches the current inputs (prints "stamp <commit>"), 1 otherwise
  dist/scripts/testlayers.py stamp GROUP --expect HASH   write the stamp; refused when the inputs changed since HASH or are not committed
  dist/scripts/testlayers.py status                a table of both groups; exit 1 unless every group has a valid stamp (release-check --status)

Groups: `core` = the layers base, core, agent and tools; `apps` = the layers apps and ui (the tile crates, iyakki-apps, the shell, the UI SDK). The layer of a crate is the first
matching glob of crate-layers.txt. The two groups together are exactly the workspace: every crate is in one of them.

The input hash of a group is the SHA-256 of the git index entries (mode, blob hash, path: git's own content hashes, never mtimes) of
  - the source trees of the group's crates and of every crate of the groups below it (apps sit on top of core),
  - Cargo.toml, Cargo.lock, the toolchain and cargo config files, and crate-layers.txt itself,
  - for `core` only: everything else under version control except site/, .github/ and .impeccable/ (the contract tests read FEATURES.md,
    BOUNDARY.md, docs, dist scripts, tests/ ...). The `apps` group does not look at those: its tests are about the apps.
The hash is refused (None) when any of those paths has an uncommitted change or an untracked file, so a stamp always names a commit.
Strict mode (IYAKKI_STAMP_STRICT=1, used by the release tag check): the hash also covers every workspace crate that a crate of the group depends on,
transitively (normal, build and dev dependencies read from the Cargo.toml files). `iyakki-core` links the shell and the apps, so with strict on an edit in
the apps crate invalidates the core stamp too: no dependent is skipped on the strength of an older green run. Every stamp holds both hashes; the loop and the
gate use the plain one (a layer is rerun when it or a layer below it changed, as the plan says), the release uses the strict one.
A stamp is a small JSON file: target/alpha-check/stamps/<group> (IYAKKI_STAMP_DIR overrides). It is written only by `stamp`, which the layer
runner (dist/release/test-layer.sh) calls after a complete, passing run of the group with no test filter.
"""
import fnmatch
import hashlib
import json
import os
import subprocess
import sys
import time
import tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
GROUPS = {"core": ("base", "core", "agent", "tools"), "apps": ("apps", "ui")}
ORDER = ["core", "apps"]  # lowest first: a group sits on top of those before it
ROOT_FILES = ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "rust-toolchain", ".cargo", "deny.toml", "dist/scripts/crate-layers.txt"]
NOT_INPUTS = ("site", ".github", ".impeccable")


class LayerError(Exception):
    pass


def git(root, *args):
    return subprocess.check_output(["git", "-C", root] + list(args), text=True)


def load_rules(path):
    rules = []
    for line in open(path):
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        glob, layer = line.rsplit(None, 1)
        rules.append((layer.strip(), glob.strip()))
    return rules


def layer_of(name, rules, default=None):
    for layer, glob in rules:
        if fnmatch.fnmatch(name, glob):
            return layer
    return default


def workspace_crates(root):
    """name -> directory (relative to root) of every workspace member, read from Cargo.toml (no cargo needed)."""
    members = tomllib.load(open(os.path.join(root, "Cargo.toml"), "rb")).get("workspace", {}).get("members", [])
    out = {}
    for m in members:
        with open(os.path.join(root, m, "Cargo.toml"), "rb") as f:
            out[tomllib.load(f)["package"]["name"]] = m
    return out


def split_groups(crates, rules):
    """{group: [crate names]}; raises LayerError naming the crates that have no layer or a layer that is in no group."""
    layer_group = {ly: g for g, lys in GROUPS.items() for ly in lys}
    res = {g: [] for g in GROUPS}
    bad = []
    for n in sorted(crates):
        ly = layer_of(n, rules)
        if ly is None or ly not in layer_group:
            bad.append(f"{n} ({'no layer' if ly is None else 'unknown layer ' + ly})")
        else:
            res[layer_group[ly]].append(n)
    if bad:
        raise LayerError("workspace crate(s) without a usable layer in crate-layers.txt: " + ", ".join(bad))
    return res


def workspace_deps(root, crates):
    """crate name -> the workspace crates it depends on (normal, build, dev and target-specific dependencies), read from the Cargo.toml files."""
    out = {}
    for n, d in crates.items():
        with open(os.path.join(root, d, "Cargo.toml"), "rb") as f:
            t = tomllib.load(f)
        tables = [t.get(k, {}) for k in ("dependencies", "dev-dependencies", "build-dependencies")]
        for tgt in t.get("target", {}).values():
            tables += [tgt.get(k, {}) for k in ("dependencies", "dev-dependencies", "build-dependencies")]
        names = set()
        for tb in tables:
            for k, v in tb.items():
                names.add(v["package"] if isinstance(v, dict) and "package" in v else k)
        out[n] = sorted(x for x in names if x in crates and x != n)
    return out


def closure(names, deps):
    seen, todo = set(), list(names)
    while todo:
        c = todo.pop()
        if c not in seen:
            seen.add(c)
            todo += deps.get(c, [])
    return seen


def input_crates(root, group, crates, groups, strict=False):
    """The crates whose source decides the group's tests: the group and the groups below it; strict: and everything any of them depends on."""
    base = {n for g in ORDER[: ORDER.index(group) + 1] for n in groups[g]}
    return sorted(closure(base, workspace_deps(root, crates)) if strict else base)


def input_paths(root, group, crates, groups, strict=False):
    """The tracked paths whose content decides the group's tests."""
    paths = [crates[n] for n in input_crates(root, group, crates, groups, strict)]
    paths += [p for p in ROOT_FILES if os.path.exists(os.path.join(root, p))]
    if group == "core":
        for f in git(root, "ls-files", "-z").split("\0"):
            top = f.split("/")[0]
            if f and top not in NOT_INPUTS and top != "crates":
                paths.append(top)  # a top-level folder or file: git takes it as a pathspec
    return sorted(set(paths))


def dirty(root, paths):
    """Paths with an uncommitted change or an untracked file."""
    out = []
    for i in range(0, len(paths), 400):
        chunk = paths[i : i + 400]
        s = git(root, "status", "--porcelain", "--untracked-files=all", "--", *chunk)
        out += [l[3:] for l in s.splitlines() if l]
    return out


def strict_mode():
    return os.environ.get("IYAKKI_STAMP_STRICT") == "1"


def input_hash(root, group, crates, rules, strict=None):
    """(hash, None) or (None, reason). strict: None = the IYAKKI_STAMP_STRICT setting."""
    strict = strict_mode() if strict is None else strict
    groups = split_groups(crates, rules)
    paths = input_paths(root, group, crates, groups, strict)
    d = dirty(root, paths)
    if d:
        return None, f"{len(d)} input file(s) have uncommitted changes, e.g. {d[0]}"
    h = hashlib.sha256()
    h.update(("group " + group + "\ncrates " + " ".join(input_crates(root, group, crates, groups, strict)) + "\n").encode())
    for i in range(0, len(paths), 400):
        h.update(git(root, "ls-files", "-s", "-z", "--", *paths[i : i + 400]).encode())
    return h.hexdigest(), None


def stamp_dir(root):
    return os.environ.get("IYAKKI_STAMP_DIR") or os.path.join(root, "target", "alpha-check", "stamps")


def read_stamp(root, group):
    try:
        return json.load(open(os.path.join(stamp_dir(root), group)))
    except (OSError, ValueError):
        return None


def verify(root, group, crates, rules):
    """(True, stamp) when a stamp matches the current inputs, else (False, reason)."""
    st = read_stamp(root, group)
    if not st:
        return False, "no stamp"
    h, why = input_hash(root, group, crates, rules)
    if h is None:
        return False, why
    if st.get("strict_hash" if strict_mode() else "hash") != h:
        return False, f"inputs changed since the green run at {st.get('commit', '?')[:12]}"
    return True, st


def write_stamp(root, group, crates, rules, expect):
    """Write the stamp for the tree the tests ran on. `expect` is the hash taken before the run (in the current mode); both hashes are stored."""
    now, why = input_hash(root, group, crates, rules)
    if now is None:
        raise LayerError("no stamp written: " + why)
    if now != expect:
        raise LayerError("no stamp written: the inputs changed while the tests ran")
    plain, why = input_hash(root, group, crates, rules, False)
    strict, _ = input_hash(root, group, crates, rules, True)  # None when a crate this group depends on is uncommitted: the release check then reruns the layer
    if plain is None:
        raise LayerError("no stamp written: " + str(why))
    commit = git(root, "rev-parse", "HEAD").strip()
    d = stamp_dir(root)
    os.makedirs(d, exist_ok=True)
    tmp = os.path.join(d, group + ".tmp")
    with open(tmp, "w") as f:
        json.dump({"group": group, "hash": plain, "strict_hash": strict, "commit": commit, "date": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "crates": sum(len(v) for v in split_groups(crates, rules).values())}, f)
    os.replace(tmp, os.path.join(d, group))
    return plain, commit


def main(argv):
    if not argv:
        print(__doc__)
        return 2
    root = os.environ.get("IYAKKI_ROOT") or git(os.getcwd(), "rev-parse", "--show-toplevel").strip()
    rules = load_rules(os.path.join(root, "dist/scripts/crate-layers.txt"))
    crates = workspace_crates(root)
    cmd, a = argv[0], argv[1:]
    try:
        if cmd == "check":
            g = split_groups(crates, rules)
            stale = [gl for _, gl in rules if not any(c in (gl,) or fnmatch.fnmatch(c, gl) for c in crates)]
            if stale:
                raise LayerError("crate-layers.txt names crate(s) that are not in the workspace (remove the line): " + ", ".join(stale))
            print("layers ok: " + ", ".join(f"{k} {len(v)} crates" for k, v in g.items()) + f" ({len(crates)} in the workspace)")
            return 0
        if cmd == "crates":
            names = split_groups(crates, rules)[a[0]]
            print(" ".join(f"-p {n}" for n in names) if "--p" in a else "\n".join(names))
            return 0
        if cmd == "hash":
            h, why = input_hash(root, a[0], crates, rules)
            if h is None:
                print(why, file=sys.stderr)
                return 1
            print(h)
            return 0
        if cmd == "verify":
            ok, x = verify(root, a[0], crates, rules)
            print(f"stamp {x['commit'][:12]}" if ok else x)
            return 0 if ok else 1
        if cmd == "stamp":
            h, c = write_stamp(root, a[0], crates, rules, a[a.index("--expect") + 1])
            print(f"stamp written: {a[0]} {c[:12]} {h[:16]}")
            return 0
        if cmd == "status":
            bad = 0
            for g in ORDER:
                ok, x = verify(root, g, crates, rules)
                print(f"{g:5} {'GREEN stamp ' + x['commit'][:12] + ' ' + x['date'] if ok else 'NOT GREEN: ' + x}")
                bad += 0 if ok else 1
            return 1 if bad else 0
    except (LayerError, IndexError, ValueError, KeyError) as e:
        print(f"testlayers: {e}", file=sys.stderr)
        return 1
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
