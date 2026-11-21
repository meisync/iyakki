"""Which hardware a recipe is written for (pure: no device, no hwcheck import).

A recipe that needs one machine (Mac mini 2014 / 2018, any Apple Mac, a touch screen) asks `check(key, info_text)` with the text
of `iyakki dev info`. A machine that does not match is NOT-APPLICABLE, not FAIL: the row is about hardware this device lacks.
"""
import re

# key -> (pattern on the `device` line of `dev info`, what the recipe is about)
REQUIREMENTS = {
    "macmini2014": (r"\bMacmini7,1\b", "the Mac mini 2014 (Macmini7,1)"),
    "macmini2018": (r"\bMacmini8,1\b", "the Mac mini 2018 (Macmini8,1)"),
    "apple": (r"\bApple\b|\b(Macmini|MacBookPro|MacBookAir|iMac|MacPro)\d+,\d+\b", "an Apple Mac"),
}


def device_line(info):
    for line in info.splitlines():
        m = re.match(r"^\s*device\s{2,}(\S.*)$", line)
        if m:
            return m.group(1).strip()
    return None


def check(key, info):
    """(True, "") when the machine matches, else (False, reason). An unreadable `device` line does NOT match: the reason says so."""
    pat, what = REQUIREMENTS[key]
    dev = device_line(info)
    if dev is None:
        return False, "this recipe is about %s; `dev info` has no device line, so the machine cannot be identified" % what
    if re.search(pat, dev):
        return True, ""
    return False, "this recipe is about %s; this machine reports %r" % (what, dev)


def nvidia_card_present(dmesg):
    """True when the kernel log shows an NVIDIA GPU or its HDMI audio (a driver bound to an NVIDIA PCI function)."""
    return bool(re.search(r"(?i)\bnouveau \S+: NVIDIA |nvidia\S* \d{4}:[0-9a-f:.]+|HDA NVidia", dmesg))


def startup_item_absent(texts, name):
    """True when the Startup list says `name` is 'not on this machine' (the item belongs to hardware that is absent)."""
    names = [t.strip() for t in texts]
    return name in names and "not on this machine" in names[names.index(name) + 1:names.index(name) + 4]
