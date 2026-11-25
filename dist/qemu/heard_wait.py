"""Wait for a NEW line of SAMPLE's Heard log, by looking again until a bounded number of looks is used up.

A check that plugs a device and then reads the log must not be fooled by a line that was already there (an earlier plug, a boot-time
device) and must not depend on how long one look takes: the guard is a number of looks, not a time. Pure code: the read function, the
pause and the counting are arguments, so the unit test runs without a guest (dist/autotest/tests/test_heard_wait.py).
"""
import re

LOOKS = 20  # looks at the screen, one pause apiece (the core looks at devices once a second)


def count(text, pattern):
    return len(re.findall(pattern, text))


def wait_new(read, pattern, before, looks=LOOKS, pause=None):
    """Look again until the log shows more lines matching `pattern` than `before`; returns the newest matching line (a string).
    Raises AssertionError with the tail of what was seen when `looks` looks did not show it."""
    t = ""
    for i in range(looks):
        t = read()
        if count(t, pattern) > before:
            return list(re.finditer(pattern, t))[-1].group(0)
        if pause and i + 1 < looks:
            pause()
    raise AssertionError("SAMPLE's Heard log did not show a new line matching %r in %d looks: %s" % (pattern, looks, t[-400:].replace("\n", " | ")))
