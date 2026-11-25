#!/usr/bin/env python3
"""Named checks over ONE QEMU boot of an existing IYAKKI image (docs/development/QEMU-TESTS.md).

    python3 dist/qemu/session-checks.py [BUILD_DIR] [--only NAME[,NAME]] [--list] [--out DIR] [--keep]

BUILD_DIR holds images/bzImage and images/rootfs.cpio.zst (default $IYAKKI_QEMU_BUILD, else target/buildroot-libre). The client is $IYAKKI_CLIENT (a built
`iyakki`, default target/debug/iyakki); the runner never builds anything and touches no real machine (the guest is on 127.0.0.1 and is deleted at the end).
It boots the guest once, makes a scratch account by typing, pairs through developer mode, answers the shell approval on the guest's own screen, then runs every
check of dist/qemu/session-checks.list (or the --only ones) and prints PASS, FAIL or SKIP per row id with the evidence. Evidence and the serial log go to
--out (default ~/iyakki-work/qemu-session-runs/<date-time>; `latest` links to it) as results.json, one NAME.txt per check and serial.log.
Exit status: 0 every selected check passed (a SKIP is not a pass but not a failure), 1 a check failed, 2 the guest could not be booted or paired.
Needs: qemu-system-x86_64 (KVM recommended), python3 with Pillow, tesseract (the pairing code is read off the guest's screen).
"""
import argparse, json, os, sys, time, traceback

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)


def read_list():
    rows, profile = [], "ram"
    for line in open(os.path.join(HERE, "session-checks.list"), encoding="utf-8"):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("profile:"):
            profile = line.split(":", 1)[1].strip()
            continue
        name, ids, what = [x.strip() for x in line.split("|", 2)]
        rows.append((name, [i.strip() for i in ids.split(",")], what, profile))
    return rows


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[1])
    ap.add_argument("build", nargs="?")
    ap.add_argument("--only")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--out")
    ap.add_argument("--keep", action="store_true")
    a = ap.parse_args()
    checks = read_list()
    if a.list:
        for n, ids, what, prof in checks:
            print("%-26s %-5s %-16s %s" % (n, prof, ",".join(ids), what))
        return 0
    if a.only:
        want = a.only.split(",")
        unknown = [w for w in want if w not in [c[0] for c in checks]]
        if unknown:
            print("unknown check(s): %s (see --list)" % ", ".join(unknown)); return 2
        checks = [c for c in checks if c[0] in want]
    build = os.path.abspath(a.build or os.environ.get("IYAKKI_QEMU_BUILD") or os.path.join(ROOT, "target", "buildroot-libre"))
    client = os.environ.get("IYAKKI_CLIENT") or os.path.join(ROOT, "target", "debug", "iyakki")
    if not os.access(client, os.X_OK):
        print("no client at %s (set IYAKKI_CLIENT to a built `iyakki`)" % client); return 2
    base = os.path.join(os.path.expanduser("~"), "iyakki-work", "qemu-session-runs")
    out = os.path.abspath(a.out) if a.out else os.path.join(base, time.strftime("%Y%m%d-%H%M%S"))
    os.makedirs(out, exist_ok=True)
    import session_lib, session_checks_defs as defs0, session_checks_ui_defs as defs1
    import devui
    PROFILES = {"ram": {}, "data": {"data_disk": True}, "idle": {}, "full": {"data_disk": True, "ram": 6144, "disk_gb": 16}, "clock": {"extra_qemu": ["-rtc", "base=2024-01-01T00:00:00"], "append": "no-kvmclock"},
            "usb": {"extra_qemu": ["-device", "qemu-xhci,id=evxhci"]}, "ui_process": {"ui_process": True}}
    results, bad, boot_failed = [], 0, False
    serial = []
    for prof in [p for p in PROFILES if any(c[3] == p for c in checks)]:
        batch = [c for c in checks if c[3] == prof]
        t0 = time.time()
        sess, err = None, None
        for attempt in (1, 2):           # pairing reads a code off the screen by OCR: one more try with a fresh guest
            sess = session_lib.Session(build, client, out, keep=a.keep, **PROFILES[prof])
            try:
                (sess.boot_serial_only if prof == "clock" else sess.boot)()
                err = None
                break
            except Exception as e:  # noqa: BLE001
                err = e
                print("profile %s: boot attempt %d failed: %s" % (prof, attempt, e), flush=True)
                sess.stop()
        if err:
            print("profile %s: the guest could not be booted and paired: %s" % (prof, err))
            boot_failed = True; continue
        print("profile %s: guest up and paired in %.0f s; image %s; shell approval %s" % (prof, time.time() - t0, build, "given" if sess.shell_ok else "NOT given"))
        for name, ids, what, _p in batch:
            fn = getattr(defs0, "check_" + name, None) or getattr(defs1, "check_" + name, None)
            t1 = time.time()
            try:
                if fn is None:
                    raise devui.Skip("no function check_%s" % name)
                ev, status = fn(sess), "PASS"
            except devui.Skip as e:
                ev, status = "skipped: %s" % e, "SKIP"
            except Exception as e:  # noqa: BLE001
                ev, status = "%s: %s\n%s" % (type(e).__name__, e, traceback.format_exc()[-600:]), "FAIL"
            bad += status == "FAIL"
            open(os.path.join(out, name + ".txt"), "w").write(ev + "\n")
            results.append({"name": name, "rows": ids, "profile": prof, "status": status, "seconds": round(time.time() - t1, 1), "evidence": ev})
            for rid in ids:
                print("%s %s %s (%.1f s)" % (status, rid, name, time.time() - t1), flush=True)
            print("    " + ev.replace("\n", "\n    ")[:900])
        serial.append("===== profile %s =====\n%s" % (prof, sess.serial()))
        sess.stop()
    open(os.path.join(out, "serial.log"), "w").write("\n".join(serial))
    json.dump({"build": build, "date": time.strftime("%Y-%m-%d %H:%M:%S"), "results": results}, open(os.path.join(out, "results.json"), "w"), indent=1)
    try:
        link = os.path.join(os.path.dirname(out), "latest")
        if os.path.islink(link) or os.path.exists(link):
            os.remove(link)
        os.symlink(out, link)
    except OSError:
        pass
    print("evidence: %s" % out)
    print("%d passed, %d failed, %d skipped" % (sum(r["status"] == "PASS" for r in results), bad, sum(r["status"] == "SKIP" for r in results)))
    return 2 if boot_failed and not results else (1 if bad else 0)


if __name__ == "__main__":
    sys.exit(main())
