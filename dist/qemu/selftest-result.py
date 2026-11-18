#!/usr/bin/env python3
"""Read the result lines of a guest self-test from a serial log (the format of crates/iyakki-core/src/selfcheck.rs).

    selftest-result.py LOG MODE        MODE is sandboxnet or bootlist

Prints each check, then PASS or FAIL. Exit 0: a RESULT PASS line that agrees with its checks and no failed check. Exit 1: anything else
(a failed check, a RESULT that disagrees, or no RESULT line because the guest did not finish). Exit 2: usage or an unreadable log.
"""
import re, sys

CHECK = re.compile(r"IYAKKI-SELFTEST (\S+) check (\S+) (PASS|FAIL)\s?(.*)$")
RESULT = re.compile(r"IYAKKI-SELFTEST (\S+) RESULT (PASS|FAIL) (\d+)/(\d+)\s*$")


def parse(text, mode):
    checks, result = [], None
    for raw in text.replace("\r", "").split("\n"):
        i = raw.find("IYAKKI-SELFTEST")
        line = raw[i:] if i >= 0 else raw
        m = CHECK.match(line)
        if m and m.group(1) == mode:
            checks.append((m.group(2), m.group(3) == "PASS", m.group(4)))
            continue
        m = RESULT.match(line)
        if m and m.group(1) == mode:
            result = (m.group(2) == "PASS", int(m.group(3)), int(m.group(4)))
    return checks, result


def verdict(checks, result):
    if result is None:
        return "no RESULT line (the guest did not finish)"
    failed = [c[0] for c in checks if not c[1]]
    if failed:
        return "failed: " + ", ".join(failed)
    ok, a, b = result
    if not ok:
        return f"RESULT FAIL {a}/{b}"
    passes = sum(1 for c in checks if c[1])
    if passes != a or len(checks) != b or b == 0:
        return f"RESULT {a}/{b} does not match the {passes} of {len(checks)} checks printed"
    return None


def main(argv):
    if len(argv) != 2:
        print(__doc__.strip().splitlines()[0], file=sys.stderr)
        return 2
    try:
        text = open(argv[0], encoding="utf-8", errors="replace").read()
    except OSError as e:
        print(f"selftest-result: {e}", file=sys.stderr)
        return 2
    checks, result = parse(text, argv[1])
    for name, ok, detail in checks:
        print(f"  {'PASS' if ok else 'FAIL'}  {name}  {detail}")
    why = verdict(checks, result)
    print("PASS" if why is None else f"FAIL: {why}")
    return 0 if why is None else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
