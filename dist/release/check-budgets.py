#!/usr/bin/env python3
"""Check an image's size and boot-time budgets (F-PKG-05).

    check-budgets.py --image full --images-dir target/buildroot-mac/images [--results results.json] [--budgets dist/release/image-budgets.json] [--strict]

Sizes are read from the files in --images-dir (each budget names a glob; the largest match counts). Timings and memory come from a
results file: {"timings": {"login_wall_s": 9.1}, "memory": {"idle_ram_mib": 410}} (dist/buildroot/boot-time.py writes it when BOOT_TIME_JSON is set).
Exit 0 when nothing is over budget, 1 when something is, 2 when the arguments or the budget file are wrong. A budget with no measurement
here is `unmeasured`; with --strict it also fails. A budget whose ceiling is null is `no budget` and never fails.
"""
import argparse, glob, json, os, sys


def check(budgets, image, images_dir, results):
    spec = budgets["images"].get(image)
    if spec is None:
        raise SystemExit(f"unknown image '{image}' (known: {', '.join(sorted(budgets['images']))})")
    rows = []  # (state, id, text)
    for f in spec.get("files", []):
        hits = [p for p in glob.glob(os.path.join(images_dir, f["glob"])) if os.path.isfile(p)]
        if not hits:
            rows.append(("unmeasured", f["id"], f"no file matching {f['glob']} in {images_dir}"))
            continue
        size = max(os.path.getsize(p) for p in hits)
        mx = f.get("max_bytes")
        if mx is None:
            rows.append(("no budget", f["id"], f"{size} bytes"))
        elif size > mx:
            rows.append(("over", f["id"], f"{size} bytes is over the {mx} budget by {size - mx} bytes ({(size - mx) * 100 / mx:.1f} percent)"))
        else:
            rows.append(("ok", f["id"], f"{size} of {mx} bytes ({size * 100 / mx:.1f} percent)"))
    for kind, section in (("timings", "timings"), ("memory", "memory")):
        for m in spec.get(section, []):
            v = (results.get(kind) or {}).get(m["id"])
            mx = m.get("max")
            if mx is None:
                rows.append(("no budget", m["id"], f"{v if v is not None else 'not measured'}"))
            elif v is None:
                rows.append(("unmeasured", m["id"], "no measurement in the results file"))
            elif v > mx:
                rows.append(("over", m["id"], f"{v} is over the {mx} budget"))
            else:
                rows.append(("ok", m["id"], f"{v} of {mx}"))
    return rows


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--image", required=True)
    ap.add_argument("--images-dir", required=True)
    ap.add_argument("--results")
    ap.add_argument("--budgets", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), "image-budgets.json"))
    ap.add_argument("--strict", action="store_true")
    ap.add_argument("--suggest", action="store_true", help="also print, for each file found, the measured size and a ceiling of measured plus 10 percent (to paste into the budget file after a review)")
    a = ap.parse_args(argv)
    try:
        budgets = json.load(open(a.budgets))
        results = json.load(open(a.results)) if a.results else {}
        rows = check(budgets, a.image, a.images_dir, results)
    except (OSError, ValueError, KeyError) as e:
        print(f"check-budgets: {e}", file=sys.stderr)
        return 2
    for state, id_, text in rows:
        print(f"{state:<10} {id_:<16} {text}")
    if a.suggest:
        print("\nsuggested (measured, measured + 10 percent):")
        for f in budgets["images"][a.image].get("files", []):
            hits = [p for p in glob.glob(os.path.join(a.images_dir, f["glob"])) if os.path.isfile(p)]
            if hits:
                size = max(os.path.getsize(p) for p in hits)
                print(f'  {f["id"]}: "measured_bytes": {size}, "max_bytes": {int(size * 1.1)}')
    bad = [r for r in rows if r[0] == "over" or (a.strict and r[0] == "unmeasured")]
    print(f"{len(rows)} budgets: {sum(r[0] == 'ok' for r in rows)} ok, {sum(r[0] == 'over' for r in rows)} over, {sum(r[0] == 'unmeasured' for r in rows)} unmeasured, {sum(r[0] == 'no budget' for r in rows)} without a ceiling")
    return 1 if bad else 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except SystemExit as e:
        if isinstance(e.code, str):
            print(f"check-budgets: {e.code}", file=sys.stderr)
            sys.exit(2)
        raise
