#!/usr/bin/env python3
"""IYAKKI boot test: run a disk image in QEMU (UEFI), take screendumps, check the serial log.

Standard library only. Environment (all optional except the image):
  IMAGE        path of the raw disk image (read-only; a throw-away qcow2 overlay takes the writes)
  SECONDS      how long to let the guest run (default 90)
  SHOT_TIMES   comma list of screendump times in seconds (default 3,8,15,30,60,<SECONDS-3>)
  RESULT_DIR   where result.json, serial.log, shot-*.png go (default /results)
  MEM_MB       guest memory (default 2048);  SMP  guest CPUs (default 2)
  REQUIRE_KVM  1 = fail at once when /dev/kvm cannot be opened (default 0: fall back to TCG, slow, recorded in the result)
Exit code 0 = every check passed, 1 = a check failed, 2 = the harness itself failed.
"""
import glob, json, os, re, shutil, socket, struct, subprocess, sys, tempfile, time, zlib

PANIC_RE = re.compile(r"panic|failed printing|Kernel panic")
MUST_HAVE = ["stage1: switching root", "IYAKKI-BOOT supervisor up"]


# ---------------------------------------------------------------- pictures
def read_ppm(path):
    data = open(path, "rb").read()
    m = re.match(rb"P6\s+(\d+)\s+(\d+)\s+(\d+)\s", data)
    if not m:
        raise ValueError("not a P6 PPM: " + path)
    w, h = int(m.group(1)), int(m.group(2))
    return w, h, data[m.end():m.end() + w * h * 3]


def write_png(path, w, h, rgb):
    raw = b"".join(b"\0" + rgb[y * w * 3:(y + 1) * w * 3] for y in range(h))
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b"")
    open(path, "wb").write(png)


def luma(rgb, n):
    r, g, b = rgb[0:n * 3:3], rgb[1:n * 3:3], rgb[2:n * 3:3]
    return bytes((299 * r[i] + 587 * g[i] + 114 * b[i]) // 1000 for i in range(n))


def screen_stats(w, h, rgb):
    """Fraction of pixels that are not (nearly) black, and the number of distinct coarse colours."""
    n = w * h
    lum = luma(rgb, n)
    lit = sum(1 for v in lum if v > 24)
    colours = len({(rgb[i] >> 5, rgb[i + 1] >> 5, rgb[i + 2] >> 5) for i in range(0, n * 3, 3 * 7)})
    return lit / n, colours


def find_tofu(w, h, rgb, min_group=6):
    """Tofu = a row of identical hollow rectangles where letters should be.

    Foreground = pixels far from the most common luma (the background). Connected components (8-way) are
    found with run-based union-find; a component counts as a hollow box when it is glyph-sized, its four
    edges are almost fully set and its inside is almost empty. Real letters (O, D, 0) have rounded corners
    or inner strokes, so they miss the edge/inside tests. Returns (found, details)."""
    n = w * h
    lum = luma(rgb, n)
    hist = [0] * 256
    for v in lum:
        hist[v] += 1
    bg = max(range(256), key=hist.__getitem__)
    fg = bytearray(1 if abs(v - bg) > 60 else 0 for v in lum)
    # runs per row
    parent = []
    runs = []  # (y, x0, x1)
    prev = []  # indices of runs of the previous row
    def find(a):
        while parent[a] != a:
            parent[a] = parent[parent[a]]
            a = parent[a]
        return a
    for y in range(h):
        row = fg[y * w:(y + 1) * w]
        cur, x = [], 0
        while x < w:
            if row[x]:
                x0 = x
                while x < w and row[x]:
                    x += 1
                idx = len(runs)
                runs.append((y, x0, x - 1))
                parent.append(idx)
                for j in prev:
                    _, p0, p1 = runs[j]
                    if p0 <= x and p1 >= x0 - 1:  # 8-connectivity: touching diagonally counts
                        ra, rb = find(idx), find(j)
                        if ra != rb:
                            parent[ra] = rb
                cur.append(idx)
            else:
                x += 1
        prev = cur
    comps = {}
    for i, r in enumerate(runs):
        comps.setdefault(find(i), []).append(r)
    groups = {}
    for rs in comps.values():
        x0 = min(r[1] for r in rs); x1 = max(r[2] for r in rs)
        y0 = rs[0][0]; y1 = rs[-1][0]
        bw, bh = x1 - x0 + 1, y1 - y0 + 1
        if not (5 <= bw <= 64 and 7 <= bh <= 96):
            continue
        mask = [bytearray(bw) for _ in range(bh)]
        for (y, a, b) in rs:
            for x in range(a, b + 1):
                mask[y - y0][x - x0] = 1
        top = sum(mask[0]) / bw
        bottom = sum(mask[-1]) / bw
        left = sum(mask[y][0] for y in range(bh)) / bh
        right = sum(mask[y][-1] for y in range(bh)) / bh
        if min(top, bottom, left, right) < 0.9:
            continue
        ix, iy = max(2, bw // 5), max(2, bh // 5)
        inner = [mask[y][x] for y in range(iy, bh - iy) for x in range(ix, bw - ix)]
        if not inner or sum(inner) / len(inner) > 0.1:
            continue
        groups.setdefault((bw, bh), []).append((x0, y0))
    worst = max(groups.items(), key=lambda kv: len(kv[1]), default=None)
    found = bool(worst and len(worst[1]) >= min_group)
    return found, {"background_luma": bg,
                   "largest_group": ({"size": list(worst[0]), "count": len(worst[1]), "first": list(worst[1][0])}
                                     if worst else None)}


# ---------------------------------------------------------------- qemu
class Monitor:
    def __init__(self, path):
        self.s = socket.socket(socket.AF_UNIX)
        for _ in range(100):
            try:
                self.s.connect(path); break
            except OSError:
                time.sleep(0.1)
        else:
            raise RuntimeError("QEMU monitor socket never appeared")
        self.s.settimeout(5)
        self._read()

    def _read(self):
        buf = b""
        try:
            while not buf.endswith(b"(qemu) "):
                d = self.s.recv(4096)
                if not d:
                    break
                buf += d
        except socket.timeout:
            pass
        return buf.decode("utf8", "replace")

    def cmd(self, c):
        self.s.sendall(c.encode() + b"\n")
        return self._read()


def find_ovmf():
    code = sorted(glob.glob("/usr/share/OVMF/OVMF_CODE*.fd"))
    pref = [c for c in code if not re.search(r"secboot|\.ms\.|snakeoil", c)]
    code = (pref or code)
    if not code:
        raise RuntimeError("no OVMF_CODE*.fd under /usr/share/OVMF")
    c = code[0]
    v = c.replace("OVMF_CODE", "OVMF_VARS")
    if not os.path.exists(v):
        raise RuntimeError("no matching OVMF_VARS for " + c)
    return c, v


def main():
    image = os.environ.get("IMAGE") or (sys.argv[1] if len(sys.argv) > 1 else "")
    secs = int(os.environ.get("SECONDS") or (sys.argv[2] if len(sys.argv) > 2 else 90))
    out = os.environ.get("RESULT_DIR", "/results")
    mem, smp = os.environ.get("MEM_MB", "2048"), os.environ.get("SMP", "2")
    shots = sorted({int(x) for x in os.environ.get("SHOT_TIMES", "").split(",") if x.strip()}
                   or {3, 8, 15, 30, 60, max(secs - 3, 4)})
    shots = [t for t in shots if t < secs] or [max(secs - 1, 1)]
    os.makedirs(out, exist_ok=True)
    res = {"image": os.path.basename(image), "seconds": secs, "started": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
           "checks": {}, "shots": [], "pass": False}

    def done(code):
        res["pass"] = code == 0
        tmp = os.path.join(out, "result.json.tmp")
        json.dump(res, open(tmp, "w"), indent=2)
        os.replace(tmp, os.path.join(out, "result.json"))
        for p in glob.glob(os.path.join(out, "*")):
            try: os.chmod(p, 0o666 if os.path.isfile(p) else 0o777)
            except OSError: pass
        open(os.path.join(out, "DONE"), "w").write("pass\n" if code == 0 else "fail\n")
        print(json.dumps({"pass": res["pass"], "checks": {k: v["pass"] for k, v in res["checks"].items()}}))
        sys.exit(code)

    if not image or not os.path.isfile(image):
        res["error"] = "image not found: %r" % image
        done(2)
    kvm = os.access("/dev/kvm", os.R_OK | os.W_OK)
    res["accel"] = "kvm" if kvm else "tcg"
    if not kvm and os.environ.get("REQUIRE_KVM") == "1":
        res["error"] = "/dev/kvm is not usable in this pod"
        done(2)

    work = tempfile.mkdtemp(prefix="bt-")
    try:
        code_fd, vars_src = find_ovmf()
        vars_fd = os.path.join(work, "vars.fd")
        shutil.copy(vars_src, vars_fd)
        overlay = os.path.join(work, "overlay.qcow2")
        subprocess.run(["qemu-img", "create", "-q", "-f", "qcow2", "-b", image, "-F", "raw", overlay], check=True)
        mon_sock = os.path.join(work, "mon.sock")
        serial = os.path.join(out, "serial.log")
        err = open(os.path.join(out, "qemu.err"), "w")
        cmd = ["qemu-system-x86_64", "-accel", "kvm" if kvm else "tcg"] + (["-cpu", "host"] if kvm else ["-cpu", "max"]) + [
               "-m", mem, "-smp", smp, "-display", "none", "-vga", "std", "-no-reboot",
               "-drive", "if=pflash,format=raw,readonly=on,file=" + code_fd,
               "-drive", "if=pflash,format=raw,file=" + vars_fd,
               "-drive", "file=%s,format=qcow2,if=virtio" % overlay,
               "-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0",
               "-device", "virtio-rng-pci",
               "-serial", "file:" + serial,
               "-monitor", "unix:%s,server,nowait" % mon_sock]
        res["qemu_cmd"] = " ".join(cmd)
        t0 = time.time()
        q = subprocess.Popen(cmd, stdout=err, stderr=err)
        mon = Monitor(mon_sock)
        died = None
        for t in shots + [secs]:
            while time.time() - t0 < t:
                if q.poll() is not None:
                    died = q.returncode; break
                time.sleep(0.2)
            if died is not None or t == secs:
                break
            ppm = os.path.join(work, "s%d.ppm" % t)
            mon.cmd("screendump " + ppm)
            for _ in range(50):
                if os.path.exists(ppm) and os.path.getsize(ppm) > 16:
                    time.sleep(0.2); break
                time.sleep(0.1)
            ent = {"t": t, "actual_t": round(time.time() - t0, 1)}
            try:
                w, h, rgb = read_ppm(ppm)
                png = os.path.join(out, "shot-%03ds.png" % t)
                write_png(png, w, h, rgb)
                lit, colours = screen_stats(w, h, rgb)
                tofu, tinfo = find_tofu(w, h, rgb)
                ent.update(file=os.path.basename(png), size=[w, h], lit_fraction=round(lit, 4), colours=colours,
                           tofu=tofu, tofu_detail=tinfo)
            except Exception as e:  # a missing/odd screendump is a harness problem, recorded, not fatal
                ent["error"] = str(e)
            res["shots"].append(ent)
        res["guest_seconds"] = round(time.time() - t0, 1)
        res["qemu_died_early"] = died
        try:
            mon.cmd("quit")
        except Exception:
            pass
        try:
            q.wait(timeout=10)
        except subprocess.TimeoutExpired:
            q.kill()
    except Exception as e:
        res["error"] = "harness: %s" % e
        done(2)
    finally:
        shutil.rmtree(work, ignore_errors=True)

    log = open(os.path.join(out, "serial.log"), "rb").read().decode("utf8", "replace").replace("\r", "")
    bad = [l.strip() for l in log.split("\n") if PANIC_RE.search(l) and "panic=" not in l]
    c = res["checks"]
    c["no_panic"] = {"pass": not bad, "detail": bad[:5]}
    for m in MUST_HAVE:
        c["serial: " + m] = {"pass": m in log}
    c["qemu_ran_to_the_end"] = {"pass": died is None, "detail": "exit %s" % died if died is not None else ""}
    good = [s for s in res["shots"] if "error" not in s]
    c["screendumps_taken"] = {"pass": len(good) == len(res["shots"]) and len(good) > 0}
    c["no_tofu"] = {"pass": bool(good) and not any(s["tofu"] for s in good),
                    "detail": [{"t": s["t"], **(s["tofu_detail"]["largest_group"] or {})} for s in good if s["tofu"]]}
    last = good[-1] if good else None
    c["final_screen_not_blank"] = {"pass": bool(last) and last["lit_fraction"] > 0.005,
                                   "detail": last and last["lit_fraction"]}
    res["note"] = ("tofu check is a pixel heuristic (rows of identical hollow boxes); the PNGs are kept for human review. "
                   "serial.log is the guest console.")
    done(0 if all(v["pass"] for v in c.values()) else 1)


if __name__ == "__main__":
    main()
