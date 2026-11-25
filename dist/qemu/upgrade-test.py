#!/usr/bin/env python3
"""Upgrade from an old release pack to the newest by the signed update path, then a rollback, on the REAL packs (F-UPD-29).

    python3 dist/qemu/upgrade-test.py [--old-pack DIR] [--new-pack DIR] [--out DIR]

Needs: two pack folders as `iyakki dev update-pack` writes them (`update.json`, `boot/`, `EFI/`), cargo, python3. Defaults: the old pack is the alpha.7 pack of the green
run `run-20261121-123102-26faa589`, the new one the newest `~/iyakki-work/green/run-*/pack` that is not the old one. Linux build host only, one at a time, never inside
the release gate (the packs are hundreds of MB and the test copies them). The REAL update centre (`devmode::centre`) runs on a folder laid out from the old pack:
it reads the newest pack signed with a throw-away key (the signed path: trust, preview, SHA-256 of every file, previous version kept whole, counter armed, the first start
marks itself good), then the rollback puts the old kernel and initramfs back. The test is the ignored `upgrade_from_the_old_pack_by_the_signed_path_then_rollback_on_real_packs`
of iyakki-agent. Report: OUT/F-UPD-29/qemu/result.json (level `qemu`: real packs and the real engine; a kernel start is covered by update-faults-test.py).
Not covered: the release key (the real pack is signed with the owner's key, this run uses its own), a network transfer, the data volume's roots of a layered install, hardware.
"""
import glob, os, subprocess, sys, time

here = os.path.dirname(os.path.abspath(__file__)); root = os.path.abspath(os.path.join(here, "..", ".."))
sys.path.insert(0, os.path.join(root, "dist", "autotest"))
import results as R  # noqa: E402

G = os.path.join(os.path.expanduser("~"), "iyakki-work", "green")
OLD = "run-20261121-123102-26faa589"


def arg(name, default=None):
    return sys.argv[sys.argv.index(name) + 1] if name in sys.argv else default


def main():
    old = arg("--old-pack", os.path.join(G, OLD, "pack"))
    packs = sorted((p for p in glob.glob(os.path.join(G, "run-*", "pack")) if os.path.abspath(p) != os.path.abspath(old)), key=os.path.getmtime)
    new = arg("--new-pack", packs[-1] if packs else None)
    out = arg("--out", os.path.join(os.path.expanduser("~"), "iyakki-work", "autotest", time.strftime("%Y%m%d-%H%M%S")))
    if not new or not all(os.path.isfile(os.path.join(p, "update.json")) for p in (old, new)):
        sys.exit("needs two pack folders with update.json (--old-pack, --new-pack)")
    if R.gate_lock_held() and not os.environ.get("IYAKKI_FAULT_IGNORE_GATE"):
        sys.exit("the release gate holds its lock: not now")
    env = dict(os.environ, IYAKKI_UPGRADE_OLD_PACK=old, IYAKKI_UPGRADE_NEW_PACK=new)
    r = subprocess.run(["cargo", "test", "--locked", "-q", "-p", "iyakki-agent", "--lib", "--", "--ignored", "--nocapture",
                        "upgrade_from_the_old_pack_by_the_signed_path_then_rollback_on_real_packs"], cwd=root, env=env, capture_output=True, text=True)
    text = r.stdout + r.stderr
    line = next((l for l in text.splitlines() if l.startswith("UPGRADE-RESULT")), "")
    folder = os.path.join(out, "F-UPD-29", "qemu")
    os.makedirs(folder, exist_ok=True)
    open(os.path.join(folder, "upgrade.txt"), "w").write(("old pack: %s\nnew pack: %s\n" % (old, new)) + text[-4000:])
    ok = r.returncode == 0 and bool(line)
    rec = R.new_result("F-UPD-29", "upgrade-test.py", "qemu", "PASS" if ok else "FAIL", evidence=[os.path.join("F-UPD-29", "qemu", "upgrade.txt")],
                       covered=["signed upgrade from the old pack to the newest", "the previous version kept whole", "the counter armed and marked good", "rollback"],
                       not_covered=["the release key", "a network transfer", "hardware"], reason="" if ok else text[-300:])
    R.write_result(out, rec)
    print(line or text[-800:])
    print("%s (upgrade) report: %s" % ("PASS" if ok else "FAIL", out))
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
