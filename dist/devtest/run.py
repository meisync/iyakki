#!/usr/bin/env python3
"""Run device UI scenarios against a paired device or a QEMU guest.

  python3 dist/devtest/run.py --device lenovo-t7 --scenarios all --out ~/iyakki-work/devtest-runs/RUN
  python3 dist/devtest/run.py --qemu target/buildroot-pc-push --scenarios home-launcher,settings-pages --out DIR
  python3 dist/devtest/run.py --list

Writes DIR/<device>/<scenario>/NN-label.png, DIR/<device>/results.json and DIR/<device>/report.html.
Exit status: 0 all run scenarios passed (skips are fine), 1 a scenario failed or errored, 2 nothing could run.
Nothing here signs in, types a password, answers an approval or touches power (see docs/development/DEVICE-UI-TESTS.md).
"""
import argparse
import os
import shutil
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import devui  # noqa: E402
import builtin_scenarios  # noqa: E402,F401  (registers the scenarios)


def pick(names):
    reg = devui.SCENARIOS
    if names in ("all", ""):
        return list(reg)
    out = []
    for n in names.split(","):
        n = n.strip()
        if n not in reg:
            sys.exit("unknown scenario %r (known: %s)" % (n, ", ".join(reg)))
        out.append(n)
    return out


def smoke_only(guest, a, notes):
    """No tesseract: the guest could not be paired (the code is read off the screen). Report what the monitor shots prove."""
    out_root = os.path.abspath(os.path.join(a.out, guest.device_name))
    d = os.path.join(out_root, "qemu-boot-smoke")
    os.makedirs(d, exist_ok=True)
    t = devui.Test(devui.Device(guest.device_name, run_dir=out_root, kind="qemu"), "qemu-boot-smoke", d)
    shots = []
    for i, (label, png) in enumerate(guest.smoke, 1):
        dst = os.path.join(d, "%02d-%s.png" % (i, label))
        shutil.copy(png, dst)
        sh = devui.Shot(dst, label, i)
        t.shots.append(sh)
        shots.append(sh)
        t.assert_not_blank(sh, "%s is not blank" % label)
    if len(shots) == 2:
        diff = t.differs(shots[0], shots[1])
        t.check("Home differs from the first-start screen (the scratch account was made)", diff is not None and diff > 3.0, "difference %.1f" % (diff or 0))
    notes.append("QEMU guest booted but not paired: tesseract is missing, so the pairing code could not be read; only the boot smoke shots ran.")
    failed = [c for c in t.checks if not c["ok"]]
    res = {"name": "qemu-boot-smoke", "status": "fail" if failed else "pass", "reason": "; ".join(c["name"] for c in failed), "started": time.strftime("%Y-%m-%d %H:%M:%S"),
           "restore": "none (scratch guest deleted at the end)", "duration": 0, "checks": t.checks, "notes": t.notes,
           "shots": [{"file": os.path.relpath(s.path, out_root), "label": s.label, "index": s.index, "ocr": ""} for s in t.shots]}
    skipped = [{"name": n, "status": "skipped", "reason": "guest not paired (needs tesseract to read the pairing code)", "restore": "", "checks": [], "shots": [], "notes": [], "duration": 0}
               for n in pick(a.scenarios) if "qemu" in devui.SCENARIOS[n]["devices"]]
    result = {"device": guest.device_name, "kind": "qemu", "started": res["started"], "client_version": "(qemu guest, not paired)", "scenarios": [res] + skipped, "notes": notes}
    devui.write_json(result, os.path.join(out_root, "results.json"))
    devui.write_report(result, os.path.join(out_root, "report.html"), out_root)
    print("report: %s" % os.path.join(out_root, "report.html"))
    print("qemu smoke: %s" % res["status"])
    return 1 if failed else 0


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--device", help="a paired device name (iyakki dev devices)")
    ap.add_argument("--qemu", metavar="BUILD_DIR", help="boot this build output folder (images/bzImage, rootfs.cpio.zst, iyakki-install-x86_64.img) in QEMU and run the scenarios that support it")
    ap.add_argument("--scenarios", default="all")
    ap.add_argument("--out", default=os.path.join(os.environ.get("TMPDIR", "/tmp"), "iyakki-devtest-" + time.strftime("%Y%m%d-%H%M%S")))
    ap.add_argument("--client", help="the iyakki client binary (default: $IYAKKI_CLIENT or the release build of this repo)")
    ap.add_argument("--list", action="store_true", help="list the scenarios and exit")
    a = ap.parse_args(argv)
    if a.list:
        for n, s in devui.SCENARIOS.items():
            print("%-18s %-10s restore: %-45s %s" % (n, "/".join(s["devices"]), s["restore"], s["doc"][:80]))
        return 0
    if bool(a.device) == bool(a.qemu):
        ap.error("give exactly one of --device NAME or --qemu BUILD_DIR")
    names = pick(a.scenarios)
    client = a.client or devui.DEFAULT_CLIENT
    if not os.access(client, os.X_OK):
        print("no iyakki client at %s (set --client or IYAKKI_CLIENT)" % client, file=sys.stderr)
        return 2
    notes = []
    if not devui.HAVE_PIL:
        notes.append("Pillow is not installed: image checks are skipped, not passed.")
    if not devui.HAVE_TESSERACT:
        notes.append("tesseract is not installed: OCR checks fall back to the shell's state text or are skipped.")
    guest = None
    try:
        if a.qemu:
            import qemu_guest
            guest = qemu_guest.Guest(os.path.abspath(a.qemu), client)
            guest.boot_and_pair()
            name, kind, home = guest.device_name, "qemu", guest.home
            if not guest.paired:
                return smoke_only(guest, a, notes)
        else:
            name, kind, home = a.device, "real", None
        out_root = os.path.abspath(os.path.join(a.out, name))
        os.makedirs(out_root, exist_ok=True)
        dev = devui.Device(name, run_dir=out_root, client=client, home=home, kind=kind)
        try:
            ver = dev.run(["whoami"], retries=1).strip().replace("\n", " ")[:160]
        except devui.DevError as e:
            print("cannot reach %s: %s" % (name, e), file=sys.stderr)
            return 2
        result = {"device": name, "kind": kind, "started": time.strftime("%Y-%m-%d %H:%M:%S"), "client_version": ver, "scenarios": [], "notes": notes}
        for n in names:
            sc = devui.SCENARIOS[n]
            print("[%s] %s ..." % (name, n), flush=True)
            r = devui.run_scenario(dev, sc, out_root)
            result["scenarios"].append(r)
            print("[%s] %s: %s %s" % (name, n, r["status"].upper(), r.get("reason", "")), flush=True)
        result["finished"] = time.strftime("%Y-%m-%d %H:%M:%S")
        devui.write_json(result, os.path.join(out_root, "results.json"))
        devui.write_report(result, os.path.join(out_root, "report.html"), out_root)
        bad = [s for s in result["scenarios"] if s["status"] in ("fail", "error")]
        ran = [s for s in result["scenarios"] if s["status"] != "skipped"]
        print("report: %s" % os.path.join(out_root, "report.html"))
        print("%d passed, %d failed, %d skipped" % (sum(s["status"] == "pass" for s in result["scenarios"]), len(bad), sum(s["status"] == "skipped" for s in result["scenarios"])))
        if bad:
            return 1
        return 0 if ran else 2
    finally:
        if guest:
            guest.stop()


if __name__ == "__main__":
    sys.exit(main())
