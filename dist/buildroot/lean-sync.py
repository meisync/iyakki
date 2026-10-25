#!/usr/bin/env python3
"""Tree copy engine of the lean boot experiment (docs/BOOT-SPEED.md). Called by lean-install.sh and lean-image.sh; it never picks a drive
itself (lean-install.sh refuses anything but a labelled IYAKKI stick before it calls this).

  lean-sync.py manifest TREE            print the manifest (one tab separated line per entry) of an extracted root; run it inside
                                        fakeroot so owners and modes are the image's, not the builder's
  lean-sync.py sync TREE MANIFEST ROOTS ID [--no-chown]
                                        make ROOTS/ID/tree equal to TREE: written as ROOTS/.ID.tmp, only files that differ are copied (the
                                        rest are hard links to the files of the tree that is `current` now, which stays untouched), then
                                        verified, renamed to ROOTS/ID, `previous` and last `current` are switched (atomic renames)
  lean-sync.py verify DIR MANIFEST      fresh-read check of DIR against the manifest (every byte read again from the drive)
  lean-sync.py grubmerge GRUB.CFG BLOCK  put the lean menu block into a grub.cfg (replace it when present, else in front of the normal entry)
  lean-sync.py grubstrip GRUB.CFG        take the lean block out again
  lean-sync.py prune ROOTS              remove trees this tool made that are neither `current` nor `previous`

Manifest line: type(d f l c b) mode(octal) uid gid size sha256|- major:minor|- path link-target (for f: =PATH when the name is a hard link of PATH). Nothing outside ROOTS is ever written.
"""
import hashlib, os, shutil, stat, sys

MARK = ".lean-complete"
OK_ID = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-")


def die(msg):
    sys.stderr.write("lean-sync: %s\n" % msg)
    sys.exit(1)


def sha_file(path, drop=False):
    h = hashlib.sha256()
    fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    try:
        if drop:
            try:
                os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
            except (AttributeError, OSError):
                pass
        while True:
            b = os.read(fd, 1 << 20)
            if not b:
                break
            h.update(b)
        if drop:
            try:
                os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
            except (AttributeError, OSError):
                pass
    finally:
        os.close(fd)
    return h.hexdigest()


def make_manifest(tree):
    out = []
    for root, dirs, files in os.walk(tree):
        dirs.sort()
        for name in sorted(dirs + files):
            p = os.path.join(root, name)
            rel = os.path.relpath(p, tree)
            if "\t" in rel or "\n" in rel:
                die("unsupported file name %r" % rel)
            st = os.lstat(p)
            m = stat.S_IMODE(st.st_mode)
            if stat.S_ISDIR(st.st_mode):
                out.append(("d", m, st.st_uid, st.st_gid, 0, "-", "-", rel, "-"))
            elif stat.S_ISLNK(st.st_mode):
                out.append(("l", m, st.st_uid, st.st_gid, 0, "-", "-", rel, os.readlink(p)))
            elif stat.S_ISREG(st.st_mode):
                out.append(("f", m, st.st_uid, st.st_gid, st.st_size, sha_file(p), "-", rel, "-"))
            elif stat.S_ISCHR(st.st_mode) or stat.S_ISBLK(st.st_mode):
                t = "c" if stat.S_ISCHR(st.st_mode) else "b"
                out.append((t, m, st.st_uid, st.st_gid, 0, "-", "%d:%d" % (os.major(st.st_rdev), os.minor(st.st_rdev)), rel, "-"))
            else:
                sys.stderr.write("lean-sync: skipping %s (not a file, folder, link or device)\n" % rel)
    out.sort(key=lambda e: e[7])
    # hard links (git is one file with 141 names, 600 MB if copied apart): the first name in path order is the file, the others say "=FIRST"
    seen, rows = {}, []
    for e in out:
        if e[0] == "f":
            st = os.lstat(os.path.join(tree, e[7]))
            if st.st_nlink > 1:
                key = (st.st_dev, st.st_ino)
                if key in seen:
                    e = e[:8] + ("=" + seen[key],)
                else:
                    seen[key] = e[7]
        rows.append(e)
    out = rows
    return "".join("\t".join(str(x) if i != 1 else "%o" % x for i, x in enumerate(e)) + "\n" for e in out)


def read_manifest(path):
    rows = []
    for line in open(path, encoding="utf-8").read().split("\n"):
        if not line:
            continue
        f = line.split("\t")
        if len(f) != 9:
            die("bad manifest line %r" % line)
        rows.append((f[0], int(f[1], 8), int(f[2]), int(f[3]), int(f[4]), f[5], f[6], f[7], f[8]))
    return rows


def fsync_dir(d):
    fd = os.open(d, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def write_atomic(path, data):
    tmp = path + ".tmp"
    with open(tmp, "wb") as f:
        f.write(data)
        f.flush()
        os.fsync(f.fileno())
    os.rename(tmp, path)
    fsync_dir(os.path.dirname(path))


def same_meta(st, mode, uid, gid, chown):
    return stat.S_IMODE(st.st_mode) == mode and (not chown or (st.st_uid == uid and st.st_gid == gid))


def copy_file(src, dst, mode, uid, gid, chown):
    tmp = dst + ".leantmp"
    if os.path.lexists(tmp):
        os.unlink(tmp)
    with open(src, "rb") as a, open(tmp, "wb") as b:
        shutil.copyfileobj(a, b, 1 << 20)
        b.flush()
        os.fsync(b.fileno())
    if chown:
        os.chown(tmp, uid, gid)
    os.chmod(tmp, mode)
    os.rename(tmp, dst)


def build(tree, rows, dest, seed, chown):
    """Make DEST (a folder that may hold a half-done earlier try, or hard links of SEED) equal to the manifest. Returns (copied, kept, bytes)."""
    if not os.path.isdir(dest):
        if seed and os.path.isdir(seed):
            # hard links of the previous tree: unchanged files cost no space and no copying; the previous tree is never modified
            shutil.copytree(seed, dest, symlinks=True, copy_function=lambda s, d, **kw: os.link(s, d), dirs_exist_ok=False)
        else:
            os.makedirs(dest)
    copied = kept = nbytes = 0
    want = set()
    for t, mode, uid, gid, size, sha, dev, rel, link in rows:
        want.add(rel)
        p = os.path.join(dest, rel)
        src = os.path.join(tree, rel)
        try:
            st = os.lstat(p)
        except FileNotFoundError:
            st = None
        if t == "d":
            if st is not None and not stat.S_ISDIR(st.st_mode):
                os.unlink(p)
                st = None
            if st is None:
                os.mkdir(p)
            # a folder is never shared between the trees (copytree made new ones), so its mode can be set in place
            os.chmod(p, mode)
            if chown:
                os.chown(p, uid, gid)
        elif t == "l":
            if st is not None and not (stat.S_ISLNK(st.st_mode) and os.readlink(p) == link):
                shutil.rmtree(p) if stat.S_ISDIR(st.st_mode) else os.unlink(p)
                st = None
            if st is None:
                os.symlink(link, p)
                if chown:
                    os.lchown(p, uid, gid)
                copied += 1
            else:
                kept += 1
        elif t == "f" and link.startswith("="):
            first = os.path.join(dest, link[1:])
            if st is not None and stat.S_ISREG(st.st_mode) and os.path.samestat(st, os.lstat(first)):
                kept += 1
                continue
            if st is not None:
                shutil.rmtree(p) if stat.S_ISDIR(st.st_mode) else os.unlink(p)
            os.link(first, p)
            copied += 1
        elif t == "f":
            if st is not None and stat.S_ISDIR(st.st_mode):
                shutil.rmtree(p)
                st = None
            if st is not None and stat.S_ISREG(st.st_mode) and st.st_size == size and same_meta(st, mode, uid, gid, chown) and sha_file(p) == sha:
                kept += 1
                continue
            if st is not None and not stat.S_ISREG(st.st_mode):
                os.unlink(p)
            copy_file(src, p, mode, uid, gid, chown)
            copied += 1
            nbytes += size
        elif t in "cb":
            if not chown:
                sys.stderr.write("lean-sync: not root, device node %s skipped\n" % rel)
                continue
            major, minor = (int(x) for x in dev.split(":"))
            if st is not None:
                if (stat.S_ISCHR(st.st_mode) if t == "c" else stat.S_ISBLK(st.st_mode)) and st.st_rdev == os.makedev(major, minor):
                    kept += 1
                    continue
                os.unlink(p)
            os.mknod(p, mode | (stat.S_IFCHR if t == "c" else stat.S_IFBLK), os.makedev(major, minor))
            os.chown(p, uid, gid)
            copied += 1
    # what the manifest does not name goes (bottom up)
    for root, dirs, files in os.walk(dest, topdown=False):
        for name in files + dirs:
            p = os.path.join(root, name)
            rel = os.path.relpath(p, dest)
            if rel not in want:
                shutil.rmtree(p) if os.path.isdir(p) and not os.path.islink(p) else os.unlink(p)
    return copied, kept, nbytes


def verify(dirpath, rows, drop=True, chown=True):
    """Fresh read of everything: a list of problems (empty = identical)."""
    if drop and os.geteuid() == 0:
        try:
            os.sync()
            open("/proc/sys/vm/drop_caches", "w").write("3\n")
        except OSError:
            pass
    bad, seen = [], set()
    for t, mode, uid, gid, size, sha, dev, rel, link in rows:
        seen.add(rel)
        p = os.path.join(dirpath, rel)
        try:
            st = os.lstat(p)
        except OSError:
            if t in "cb" and not chown:
                continue
            bad.append("missing " + rel)
            continue
        if t == "d" and not stat.S_ISDIR(st.st_mode):
            bad.append("not a folder " + rel)
        elif t == "l" and not (stat.S_ISLNK(st.st_mode) and os.readlink(p) == link):
            bad.append("wrong link " + rel)
        elif t == "f" and link.startswith("="):
            if not stat.S_ISREG(st.st_mode) or not os.path.samestat(st, os.lstat(os.path.join(dirpath, link[1:]))):
                bad.append("not a hard link of %s: %s" % (link[1:], rel))
        elif t == "f":
            if not stat.S_ISREG(st.st_mode) or st.st_size != size:
                bad.append("wrong size " + rel)
            elif sha_file(p, drop=True) != sha:
                bad.append("checksum " + rel)
        if t in "cb" and not chown:
            continue
        if t != "l" and not same_meta(st, mode, uid, gid, chown) and t != "c" and t != "b":
            bad.append("mode/owner " + rel)
    for root, dirs, files in os.walk(dirpath):
        for name in files + dirs:
            rel = os.path.relpath(os.path.join(root, name), dirpath)
            if rel not in seen:
                bad.append("unexpected " + rel)
    return bad


def read_pointer(roots, name):
    try:
        t = open(os.path.join(roots, name)).read().split()
        return t[0] if t and all(c in OK_ID for c in t[0]) and ".." not in t[0] else None
    except OSError:
        return None


def prune(roots):
    keep = {read_pointer(roots, "current"), read_pointer(roots, "previous")}
    for n in sorted(os.listdir(roots)):
        p = os.path.join(roots, n)
        if not os.path.isdir(p) or n in keep:
            continue
        if n.startswith(".") and n.endswith(".tmp") or os.path.exists(os.path.join(p, MARK)):
            shutil.rmtree(p)
            print("lean-sync: removed old %s" % n)


def sync(tree, manifest, roots, idv, chown):
    if not idv or len(idv) > 64 or idv == "." or ".." in idv or not set(idv) <= OK_ID:
        die("bad root id %r" % idv)
    rows = read_manifest(manifest)
    mhash = hashlib.sha256(open(manifest, "rb").read()).hexdigest()
    os.makedirs(roots, exist_ok=True)
    final, tmp = os.path.join(roots, idv), os.path.join(roots, "." + idv + ".tmp")
    cur, prev = read_pointer(roots, "current"), read_pointer(roots, "previous")
    if os.path.isdir(final):
        marker = os.path.join(final, MARK)
        if os.path.isfile(marker) and open(marker).read().strip() == mhash:
            print("lean-sync: %s is already there with this content; checking it again" % idv)
        else:
            die("%s exists with other content (a different build under the same id): rebuild with a new IYAKKI_ROOT_ID" % idv)
    else:
        seed = os.path.join(roots, cur, "tree") if cur and cur != idv else None
        os.makedirs(tmp, exist_ok=True)
        copied, kept, nbytes = build(tree, rows, os.path.join(tmp, "tree"), seed, chown)
        print("lean-sync: %d files written (%.1f MB), %d unchanged%s" % (copied, nbytes / 1048576, kept, " (hard-linked from %s)" % cur if seed else ""))
        os.sync()
        bad = verify(os.path.join(tmp, "tree"), rows, chown=chown)
        if bad:
            die("the new tree does not match before the switch (%d problems, first: %s); nothing was switched" % (len(bad), bad[0]))
        write_atomic(os.path.join(tmp, MARK), (mhash + "\n").encode())
        fsync_dir(tmp)
        os.rename(tmp, final)
        fsync_dir(roots)
    bad = verify(os.path.join(final, "tree"), rows, chown=chown)
    if bad:
        die("fresh read of %s does not match (%d problems, first: %s); `current` was not switched" % (final, len(bad), bad[0]))
    print("lean-sync: fresh read of %d entries matches" % len(rows))
    if cur != idv:
        if cur:
            write_atomic(os.path.join(roots, "previous"), (cur + "\n").encode())
        write_atomic(os.path.join(roots, "current"), (idv + "\n").encode())
        print("lean-sync: current -> %s, previous -> %s" % (idv, cur or "none"))
    else:
        print("lean-sync: current is already %s" % idv)
    prune(roots)


B1, B2 = "# BEGIN lean-boot", "# END lean-boot"


def grub_strip(cfg_text):
    """The grub.cfg without the lean block (and the one blank line that follows it), or unchanged when it has none."""
    if B1 not in cfg_text or B2 not in cfg_text:
        return cfg_text
    a = cfg_text.index(B1)
    b = cfg_text.index("\n", cfg_text.index(B2, a)) + 1
    if cfg_text[b:b + 1] == "\n":
        b += 1
    return cfg_text[:a] + cfg_text[b:]


def grub_merge(cfg_text, block):
    """The stick's grub.cfg with the lean block in: replaced when it is there, else right in front of the normal entry. Nothing else changes."""
    if not block.startswith(B1) or B2 not in block:
        die("the lean block lacks its markers")
    block = block.rstrip("\n") + "\n\n"
    base = grub_strip(cfg_text)
    if base != cfg_text:
        a = cfg_text.index(B1)           # same place as before
        return base[:a] + block + base[a:]
    i = base.find('menuentry "IYAKKI" {')
    if i < 0:
        die("no normal entry named IYAKKI in grub.cfg")
    # in front of the normal entry and the comment lines that describe it: the fast start comes first and is the default
    lines = base[:i].split("\n")           # the last item is the (empty) start of the entry's own line
    k = len(lines) - 1
    while k > 0 and lines[k - 1].startswith("#"):
        k -= 1
    at = len("\n".join(lines[:k])) + (1 if k else 0)
    return base[:at] + block + base[at:]


def main(a):
    if len(a) >= 3 and a[1] == "manifest":
        sys.stdout.write(make_manifest(a[2]))
    elif len(a) >= 6 and a[1] == "sync":
        sync(a[2], a[3], a[4], a[5], "--no-chown" not in a[6:])
    elif len(a) >= 4 and a[1] == "verify":
        bad = verify(a[2], read_manifest(a[3]), chown="--no-chown" not in a[4:])
        for b in bad[:20]:
            print("lean-sync: " + b)
        sys.exit(1 if bad else 0)
    elif len(a) == 4 and a[1] == "grubmerge":
        new = grub_merge(open(a[2]).read(), open(a[3]).read())
        if new != open(a[2]).read():
            write_atomic(a[2], new.encode())
            print("lean-sync: lean entries put into " + a[2])
        else:
            print("lean-sync: grub.cfg already has the lean entries")
    elif len(a) == 3 and a[1] == "grubstrip":
        new = grub_strip(open(a[2]).read())
        write_atomic(a[2], new.encode())
    elif len(a) == 3 and a[1] == "prune":
        prune(a[2])
    else:
        sys.stderr.write(__doc__)
        sys.exit(2)


if __name__ == "__main__":
    main(sys.argv)
