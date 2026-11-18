#!/usr/bin/env python3
"""Test only what a change can break (alpha.4 item 1: layer-aware change detection).

  dist/scripts/test-affected.py [--base REV] [--list] [--run] [--quick] [--layers-file FILE] [-- extra cargo test args]

  --quick   the loop while you work: only the changed crates' own tests (an edit inside crates/iyakki-apps/src/apps/<app> runs only that app's
            tests in the shell crate, not the 3,500 others); dependents are left to the full run before you push.

What it does, in order:
  1. The changed files: `git diff --name-only REV` (default REV: HEAD~1) plus uncommitted and untracked files.
  2. Each file belongs to a workspace crate (crates/<name>/...), or is a *root* file (Cargo.toml, Cargo.lock: everything is affected),
     or a *contract* file (FEATURES.md, BOUNDARY.md, the ratchet allow lists: the contract tests run), or none of these (docs, scripts: nothing to test).
  3. The affected crates are the changed crates plus every workspace crate that depends on them (normal, build and dev dependencies, transitively).
  4. Each crate has a layer (dist/scripts/crate-layers.txt: base, core, agent, ui, apps, tools; the first matching glob wins). A change in base or core also
     runs the contract tests (boundary, dependency_ratchet, kernel_single_reader, feature_matrix) because a layer boundary can have moved.
  5. It prints the plan. `--list` stops there; `--run` runs `cargo test --locked -p ...` for the affected crates, one cargo call, then the contract tests.

The layered gate runs the same layers: `./run.sh test-core`, `./run.sh test-apps` (green stamps, dist/scripts/testlayers.py).
The full gate (`./run.sh alpha-check`) stays the release check: this script is for the loop while you work. It never skips a crate that
depends on a changed one, so it is as safe as the dependency graph is true; a test that reads a file of another crate (a scan of sources, an include_str!)
is the exception: add such a crate to ALWAYS_WITH below.
"""
import fnmatch
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import testlayers  # noqa: E402

ROOT = subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip()
HERE = os.path.dirname(os.path.abspath(__file__))

ROOT_FILES = {"Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "rust-toolchain", ".cargo/config.toml", "deny.toml"}
CONTRACT_FILES = {"FEATURES.md", "BOUNDARY.md", "KNOWN-ISSUES.md", "dist/release/known-flaky.txt"}
CONTRACT_PREFIXES = ("crates/iyakki-core/tests/", "docs/development/core-surface", "docs/development/c-abi-symbols")
CONTRACT_TESTS = ["boundary", "dependency_ratchet", "kernel_single_reader", "feature_matrix"]
# crate -> crates whose tests read its sources (a source scan), so they run when it changes even without a Cargo dependency
ALWAYS_WITH = {"iyakki-shell": ["iyakki-core"], "iyakki-agent": ["iyakki-core"]}


def load_layers(path):
    return testlayers.load_rules(path)


def layer_of(name, rules):
    # one list of layers for the loop and for the gate (testlayers.py checks that no crate lacks a layer); "base" only for a crate a fake rules file leaves out
    return testlayers.layer_of(name, rules, "base")


def workspace():
    meta = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--offline", "--format-version", "1"], cwd=ROOT, text=True))
    ids = set(meta["workspace_members"])
    pk = {p["id"]: p for p in meta["packages"]}
    by_dir = {}
    names = set()
    for i in ids:
        p = pk[i]
        names.add(p["name"])
        by_dir[os.path.relpath(os.path.dirname(p["manifest_path"]), meta["workspace_root"])] = p["name"]  # relative to cargo's own root: also right when cargo ran on a build host
    deps = {n: set() for n in names}  # crate -> workspace crates it depends on
    for i in ids:
        p = pk[i]
        for d in p["dependencies"]:
            if d["name"] in names:
                deps[p["name"]].add(d["name"])
    return names, by_dir, deps


def changed_files(base):
    out = set()
    for cmd in (["git", "diff", "--name-only", base], ["git", "diff", "--name-only", "--cached"], ["git", "ls-files", "--others", "--exclude-standard"]):
        try:
            out.update(x for x in subprocess.check_output(cmd, cwd=ROOT, text=True).splitlines() if x)
        except subprocess.CalledProcessError:
            pass
    return sorted(out)


def classify(files, by_dir):
    crates, root, contract, other = set(), False, False, []
    for f in files:
        if f in ROOT_FILES:
            root = True
            continue
        if f in CONTRACT_FILES or f.startswith(CONTRACT_PREFIXES):
            contract = True
        hit = None
        for d, n in by_dir.items():
            if f == d or f.startswith(d + "/"):
                hit = n
                break
        if hit:
            crates.add(hit)
        elif f not in CONTRACT_FILES and not f.startswith(CONTRACT_PREFIXES):
            other.append(f)
    return crates, root, contract, other


def app_filters(files):
    """Test-name filters for the apps crate: an edit under crates/iyakki-apps/src/apps/ names the app module. None: some other file of the apps crate changed (run all)."""
    mods, other = set(), False
    for f in files:
        if not f.startswith("crates/iyakki-apps/src/"):
            continue
        rest = f[len("crates/iyakki-apps/src/"):]
        if rest.startswith("apps/"):
            first = rest[len("apps/"):].split("/")[0]
            stem = first[:-3] if first.endswith(".rs") else first
            for suf in ("_tests", "_test"):
                if stem.endswith(suf):
                    stem = stem[: -len(suf)]
            mods.add(stem)
        else:
            other = True
    return (None if other else sorted(mods))


def plan(files, names, by_dir, deps, rules):
    crates, root, contract, other = classify(files, by_dir)
    if root:
        affected = set(names)
    else:
        affected = set(crates)
        rev = {n: set() for n in names}
        for n, ds in deps.items():
            for d in ds:
                rev[d].add(n)
        todo = list(crates)
        while todo:
            c = todo.pop()
            for dep in rev.get(c, ()):
                if dep not in affected:
                    affected.add(dep)
                    todo.append(dep)
        for c in list(crates):
            for extra in ALWAYS_WITH.get(c, ()):
                if extra in names:
                    affected.add(extra)
    layers = {n: layer_of(n, rules) for n in affected}
    boundary_moved = root or contract or any(layers[n] in ("base", "core", "agent") for n in crates)
    return {"files": files, "changed": sorted(crates), "shell_filters": app_filters(files), "root": root, "affected": sorted(affected), "layers": layers,
            "contract": boundary_moved, "other": other}


def main(argv):
    base, run, only_list, quick, layers_file, extra = "HEAD~1", False, False, False, os.path.join(HERE, "crate-layers.txt"), []
    a = list(argv)
    while a:
        x = a.pop(0)
        if x == "--base":
            base = a.pop(0)
        elif x == "--run":
            run = True
        elif x == "--list":
            only_list = True
        elif x == "--quick":
            quick = True
        elif x == "--layers-file":
            layers_file = a.pop(0)
        elif x == "--":
            extra = a
            break
        else:
            print(__doc__)
            return 2
    names, by_dir, deps = workspace()
    files = changed_files(base)
    p = plan(files, names, by_dir, deps, load_layers(layers_file))
    by_layer = {}
    for n in p["affected"]:
        by_layer.setdefault(p["layers"][n], []).append(n)
    print(f"changed files: {len(files)}   changed crates: {len(p['changed'])}   affected crates: {len(p['affected'])} of {len(names)}"
          + ("   (a root file changed: everything)" if p["root"] else ""))
    for layer in ("base", "core", "agent", "ui", "apps", "tools"):
        if layer in by_layer:
            print(f"  {layer:5} {' '.join(by_layer[layer])}")
    if p["other"]:
        print(f"  not code ({len(p['other'])} files: docs, scripts...): nothing to test for them")
    print("contract tests (boundary, dependency_ratchet, kernel_single_reader, feature_matrix): " + ("YES" if p["contract"] else "no"))
    if only_list or not p["affected"]:
        return 0
    cmds = []
    if quick:
        plain = [n for n in p["changed"] if not (n == "iyakki-apps" and p["shell_filters"])]
        if plain:
            cmds.append(["cargo", "test", "--locked", "--no-fail-fast"] + [x for n in plain for x in ("-p", n)] + extra)
        if "iyakki-apps" in p["changed"] and p["shell_filters"]:
            cmds.append(["cargo", "test", "--locked", "-p", "iyakki-apps", "--lib", "--"] + ["apps::" + m for m in p["shell_filters"]])
    else:
        cmds.append(["cargo", "test", "--locked", "--no-fail-fast"] + [x for n in p["affected"] for x in ("-p", n)] + extra)  # every crate runs: one red crate must not hide the rest
    for c in cmds:
        print("\n" + " ".join(c))
    if not run:
        print("(add --run to run it)")
        return 0
    rc = 0
    for c in cmds:
        rc = subprocess.call(c, cwd=ROOT) or rc
    if rc == 0 and p["contract"]:
        for t in CONTRACT_TESTS:
            c = ["cargo", "test", "--locked", "-p", "iyakki-core", "--test", t]
            print("\n" + " ".join(c))
            rc = subprocess.call(c, cwd=ROOT) or rc
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
