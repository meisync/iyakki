"""Pure logic of the drive-loss hardware recipe (F-LOSS-06): reads `iyakki storage status` text and judges a series of samples.
No device, no hwcheck import; tested in test_loss.py with canned output. A format it does not recognise gives None: the recipe then
says NEEDS-HUMAN, never FAIL."""
import re

STATES = ("Healthy", "Errors", "ReadOnly", "Lost", "Reattachable", "Failed")


def drive_state(text):
    """`drive: Healthy (since uptime 120 s), device /dev/sda2, UUID ...` -> ("Healthy", "/dev/sda2"). `drive: Failed(why) ...` keeps
    the word `Failed`. `drive: not watched ...` -> ("NotWatched", ""). Anything else -> None."""
    m = re.search(r"^drive:\s+(\w+)(?:\([^)]*\))?\s*(?:\(since uptime \d+ s\))?(?:,\s*device\s+([^\s,]*))?", text, re.M)
    if not m:
        return None
    word = m.group(1)
    if word == "not":
        return ("NotWatched", "")
    if word not in STATES:
        return None
    return (word, m.group(2) or "")


def judge(samples):
    """samples: [(seconds since the recipe asked for the unplug, state word, device)]. The row promises, in order: Healthy at the start,
    then Lost, then Reattachable, then Healthy again on a NEW device node (the drive comes back as another sdX). Returns
    (verdict, reason, facts) with verdict one of PASS, FAIL, NEEDS-HUMAN."""
    words = [s[1] for s in samples]
    facts = {"first_lost_s": None, "reattachable_s": None, "healthy_again_s": None, "device_before": None, "device_after": None}
    if not samples:
        return ("NEEDS-HUMAN", "no status was read", facts)
    if words[0] == "NotWatched":
        return ("NEEDS-HUMAN", "the drive watcher is not running (data in RAM?): the loss path cannot be tried here", facts)
    if words[0] != "Healthy":
        return ("NEEDS-HUMAN", "the drive was not Healthy when the check started (%s)" % words[0], facts)
    facts["device_before"] = samples[0][2]
    seen = 0   # 0 healthy, 1 lost, 2 reattachable, 3 healthy again
    for t, w, d in samples[1:]:
        if seen == 0 and w in ("Lost", "Errors", "ReadOnly", "Reattachable"):
            seen = 1
            facts["first_lost_s"] = t
        if seen == 1 and w == "Reattachable":
            seen = 2
            facts["reattachable_s"] = t
        if seen == 2 and w == "Healthy":
            seen = 3
            facts["healthy_again_s"] = t
            facts["device_after"] = d
            break
        if w.startswith("Failed") or w == "Failed":
            return ("FAIL", "the watcher gave up (Failed) at %.0f s: the drive was not taken back" % t, facts)
    if seen == 0:
        return ("NEEDS-HUMAN", "the status never left Healthy: the cable may not have been pulled, or the loss was not noticed", facts)
    if seen == 1:
        return ("FAIL", "the drive was lost but never became Reattachable", facts)
    if seen == 2:
        return ("FAIL", "the drive became Reattachable but never Healthy again", facts)
    if facts["first_lost_s"] is not None and facts["first_lost_s"] > 5.0:
        # the row says the loss is noticed within about 3 s of the unplug; the unplug time itself is the person's, so this is only a note
        facts["note"] = "the loss was first seen %.0f s after the check asked for the unplug (the person's own delay is in this)" % facts["first_lost_s"]
    return ("PASS", "Healthy -> Lost -> Reattachable -> Healthy; device %s -> %s" % (facts["device_before"] or "?", facts["device_after"] or "?"), facts)
