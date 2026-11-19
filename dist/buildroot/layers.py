#!/usr/bin/env python3
"""Split a root tree into squashfs layers by the rules of layers.txt (docs/development/LAYERED-ROOT-DESIGN.md).

    layers.py split ROOTDIR OUTDIR [--rules FILE] [--level N] [--release R] [--volume boot|data]
    layers.py assign ROOTDIR [--rules FILE]        print `layer path` for every non-folder (debugging / tests)

split writes OUTDIR/<name>-<first 12 hex of the SHA-256>.sfs for each layer and OUTDIR/layers.json (the layer manifest the first stage and the
update code read; schema 1). Builds are reproducible: same files give the same bytes (so an unchanged layer has an unchanged hash and a delta pack
carries nothing for it). Run it inside fakeroot when the tree has real owners. Needs mksquashfs.
"""
import fnmatch, hashlib, json, os, re, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))


def glob_re(g):
    out, i = "", 0
    while i < len(g):
        if g.startswith("**", i):
            out += ".*"; i += 2
        elif g[i] == "*":
            out += "[^/]*"; i += 1
        elif g[i] == "?":
            out += "[^/]"; i += 1
        else:
            out += re.escape(g[i]); i += 1
    return re.compile("^" + out + "$")


def parse_rules(path):
    layers, rules = [], []
    for n, line in enumerate(open(path), 1):
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        w = line.split()
        if w[0] == "layer":
            l = {"name": w[1], "provides": {}, "requires": {}, "optional": False, "store": False}
            for x in w[2:]:
                if x == "provides" or x == "requires":
                    cur = x
                elif x == "optional":
                    l["optional"] = True
                elif x == "store":
                    l["store"] = True   # data blocks stored uncompressed: bigger on disk, but a delta between two versions of a program is then small
                elif "=" in x:
                    k, v = x.split("=", 1)
                    l[cur][k] = v
                else:
                    raise SystemExit(f"{path}:{n}: cannot read {x!r}")
            layers.append(l)
        elif w[0] == "rule" and len(w) == 3:
            rules.append((w[1], glob_re(w[2])))
        else:
            raise SystemExit(f"{path}:{n}: cannot read {line!r}")
    names = [l["name"] for l in layers]
    if not layers or len(set(names)) != len(names):
        raise SystemExit(f"{path}: no layers, or a layer twice")
    for r, _ in rules:
        if r not in names:
            raise SystemExit(f"{path}: rule for unknown layer {r}")
    return layers, rules


def walk(root):
    """(relative path, is_dir) for everything under root, folders after their content is listed by the caller."""
    for d, dirs, files in os.walk(root, followlinks=False):
        dirs.sort()
        rel = os.path.relpath(d, root)
        rel = "" if rel == "." else rel
        # symlinks to folders appear in `dirs` of os.walk but are not walked into: treat them as non-folders
        real_dirs = [x for x in dirs if not os.path.islink(os.path.join(d, x))]
        links = [x for x in dirs if os.path.islink(os.path.join(d, x))]
        yield rel, real_dirs, sorted(files + links)


def assign(root, rules, layers):
    """{relative path of a non-folder: layer}"""
    first = layers[-1]["name"]
    out = {}
    for rel, _dirs, files in walk(root):
        for f in files:
            p = f"{rel}/{f}" if rel else f
            for name, rx in rules:
                if rx.match(p):
                    out[p] = name
                    break
            else:
                raise SystemExit(f"no rule matches {p} (end layers.txt with `rule <layer> **`)")
    return out


def excludes(root, owner, name, bottom=False):
    """Paths to leave out of layer `name`: other layers' files, and folders with nothing of this layer below them."""
    ex = []

    def has(rel_dir):
        return has_cache[rel_dir]

    # which folders contain something of this layer (computed bottom-up)
    has_cache = {}
    anyfile = {}
    dirs_in_order = []
    for rel, dirs, files in walk(root):
        dirs_in_order.append((rel, dirs, files))
    for rel, dirs, files in reversed(dirs_in_order):
        mine = any(owner[f"{rel}/{f}" if rel else f] == name for f in files)
        mine = mine or any(has_cache.get(f"{rel}/{d}" if rel else d, False) for d in dirs)
        anyfile[rel] = bool(files) or any(anyfile.get(f"{rel}/{d}" if rel else d, False) for d in dirs)
        # a folder with no file anywhere below it (/proc, /sys, /mnt, /tmp: mount points and empty folders) belongs to the bottom layer
        has_cache[rel] = mine or (bottom and not anyfile[rel])
    for rel, dirs, files in dirs_in_order:
        if rel and not has_cache[rel]:
            # the parent excludes this whole folder (skip: handled when the parent lists it)
            pass
        for d in dirs:
            p = f"{rel}/{d}" if rel else d
            if not has_cache[p]:
                ex.append(p)
        if rel == "" or has_cache[rel]:
            for f in files:
                p = f"{rel}/{f}" if rel else f
                if owner[p] != name:
                    ex.append(p)
    return ex


def split(root, out, rules_file, level, release, volume):
    layers, rules = parse_rules(rules_file)
    owner = assign(root, rules, layers)
    os.makedirs(out, exist_ok=True)
    tmp = os.path.join(out, ".layers-work")
    os.makedirs(tmp, exist_ok=True)
    entries = []
    for l in layers:
        name = l["name"]
        if not any(v == name for v in owner.values()):
            if l["optional"]:
                continue
            raise SystemExit(f"layer {name} would be empty: no file matches its rules")
        exf = os.path.join(tmp, f"{name}.exclude")
        with open(exf, "w", encoding="utf-8", errors="surrogateescape") as f:
            f.write("".join(p + "\n" for p in excludes(root, owner, name, bottom=(l is layers[0]))))
        sfs = os.path.join(tmp, f"{name}.sfs")
        subprocess.run(["mksquashfs", root, sfs, "-ef", exf, "-comp", "zstd", "-Xcompression-level", str(level), "-b", "256K", "-noappend",
                        "-quiet", "-no-progress", "-all-time", "0", "-mkfs-time", "0"] + (["-noD"] if l["store"] else []), check=True, stdout=subprocess.DEVNULL)
        h = hashlib.sha256()
        with open(sfs, "rb") as f:
            for blk in iter(lambda: f.read(1 << 20), b""):
                h.update(blk)
        sha = h.hexdigest()
        fname = f"{name}-{sha[:12]}.sfs"
        os.replace(sfs, os.path.join(out, fname))
        os.remove(exf)
        d = "boot/layers" if volume == "boot" else "iyakki/layers"
        e = {"name": name, "volume": volume, "path": f"{d}/{fname}", "sha256": sha, "size": os.path.getsize(os.path.join(out, fname)),
             "provides": l["provides"], "requires": l["requires"]}
        if l["optional"]:
            e["optional"] = True
        entries.append(e)
        print(f"layers: {name:5s} {e['size'] / 1048576:7.1f} MB  {sha[:12]}  ({sum(1 for v in owner.values() if v == name)} files)", file=sys.stderr)
    try:
        os.rmdir(tmp)
    except OSError:
        pass
    with open(os.path.join(out, "layers.json"), "w") as f:
        json.dump({"schema": 1, "release": release, "layers": entries}, f, indent=2)
        f.write("\n")


def main(a):
    if len(a) < 2 or a[0] not in ("split", "assign"):
        sys.exit(__doc__)
    def opt(n, d):
        if n in a:
            i = a.index(n); v = a[i + 1]; del a[i:i + 2]; return v
        return d
    rules = opt("--rules", os.path.join(HERE, "layers.txt")); level = int(opt("--level", "3")); release = opt("--release", ""); volume = opt("--volume", "boot")
    if a[0] == "assign":
        layers, r = parse_rules(rules)
        for p, l in sorted(assign(a[1], r, layers).items()):
            print(l, p)
        return
    if len(a) < 3:
        sys.exit(__doc__)
    split(a[1], a[2], rules, level, release, volume)


if __name__ == "__main__":
    main(sys.argv[1:])
