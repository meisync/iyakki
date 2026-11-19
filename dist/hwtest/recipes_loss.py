"""F-LOSS-06: the drive-loss path on a real machine (docs/STORAGE-LOSS.md, "Testing on a real machine"). Needs a person's hands: the
recipe reads the drive's status over `iyakki dev`, tells the person (in the evidence log and on stdout) to pull the USB-C cable for a few
seconds, then samples `iyakki storage status` until the drive is Healthy again or the time is up. Nothing is changed on the device and
no key is pressed. Judgement is in recipes_loss_parse.py (tested in test_loss.py). The banner and the "Drive reconnected" card are
read from the screen text when they are there; their absence is only noted (the status is the verdict)."""
import re, time
from hwcheck import recipe, fail, human, dev, Ctx
import recipes as _r1  # gives Ctx.notcovered
import recipes_loss_parse as P

WAIT_FOR_UNPLUG_S = 120   # how long the person has to pull the cable before the recipe gives up
WAIT_AFTER_S = 180        # how long the drive has to come back


def _status(c):
    rc, out = dev("shell", "--", "iyakki", "storage", "status", timeout=45)
    out = "\n".join(l for l in out.splitlines() if not l.startswith("Shell access granted"))
    return P.drive_state(out), out


@recipe("F-LOSS-06", "Pull the USB-C cable of the system drive for a few seconds: lost, reattachable, healthy again on a new device (needs a person; changes nothing)")
def loss06(c):
    """PASS when the status goes Healthy -> Lost -> Reattachable -> Healthy and the device node is read each time. FAIL when it gets
    stuck at Lost or Reattachable, or the watcher says Failed. NEEDS-HUMAN when the status never leaves Healthy (no unplug in time), the
    watcher is not running, or the format is not recognised. NOT covered: the banner wording and timing to the second, the 30 s
    cable-out variant, pressing Reconnect now (a person judges those on the screen)."""
    st, raw = _status(c)
    c.log("status at the start: " + raw.strip().replace("\n", " | "))
    if st is None:
        human("`iyakki storage status` printed something this recipe does not know: " + raw.strip()[:200])
    samples = [(0.0, st[0], st[1])]
    print("F-LOSS-06: pull the USB-C cable of the system drive at the Mac end, count to 3, plug it back in. Waiting up to %d s." % WAIT_FOR_UNPLUG_S)
    c.log("PERSON: pull the USB-C cable for about 3 s and plug it back in now")
    t0 = time.time()
    deadline = t0 + WAIT_FOR_UNPLUG_S
    seen_loss = False
    while time.time() < deadline + WAIT_AFTER_S:
        time.sleep(1.0)
        try:
            cur, raw = _status(c)
        except Exception as e:   # the device may not answer for a moment while the cable is out
            c.log("no answer at %.0f s: %r" % (time.time() - t0, e))
            if not seen_loss:
                samples.append((time.time() - t0, "Lost", ""))
                seen_loss = True
            continue
        if cur is None:
            continue
        samples.append((time.time() - t0, cur[0], cur[1]))
        if cur[0] != "Healthy":
            seen_loss = True
        if seen_loss and cur[0] == "Healthy":
            break
        if not seen_loss and time.time() > deadline:
            break
    open(c.dir + "/samples.txt", "w").write("\n".join("%.1f %s %s" % s for s in samples) + "\n")
    verdict, reason, facts = P.judge(samples)
    c.log("facts: %r" % (facts,))
    if verdict == "FAIL":
        fail(reason)
    if verdict == "NEEDS-HUMAN":
        human(reason)
    _, home = dev("shell", "--", "sh", "-c", "ls /home >/dev/null && echo HOME-OK; mount | grep -c iyakki", timeout=45)
    c.log("after: " + home.strip().replace("\n", " | "))
    if "HOME-OK" not in home:
        fail("the drive is Healthy again but /home cannot be listed")
    c.notcovered("the banner and card wording, the 30 s cable-out variant and Reconnect now: a person looks at the screen")
