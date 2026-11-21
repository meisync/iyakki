"""Pure parsers for the M14 hardware-check recipes (dist/hwtest/recipes_m14*.py).

No device, no hwcheck import: every function takes the text a `iyakki dev` command printed and returns plain data, so the
logic of a recipe can be unit-tested with canned output (dist/hwtest/test_m14.py). Where a format is a guess, the parser
returns None or an empty list and the recipe says NEEDS-HUMAN instead of FAIL: a recipe never fails on its own misreading.
"""
import re


# ---------------------------------------------------------------- `iyakki dev info`

def info_fields(info):
    """`dev info` lines are `key  value` (two or more spaces), indented two spaces on a real device; the first line is the host
    name and has no value. Returns {key: [values]} in order of appearance."""
    out = {}
    for line in info.splitlines():
        m = re.match(r"^\s*([A-Za-z][A-Za-z ]*?)\s{2,}(\S.*)$", line)
        if m:
            out.setdefault(m.group(1).strip().lower(), []).append(m.group(2).strip())
    return out


def model_of(info):
    """The DMI product name (`Macmini8,1`) from the `device` line, or None."""
    v = info_fields(info).get("device")
    if not v:
        return None
    words = v[0].split()
    for w in words:   # Apple: "Apple Inc. Macmini8,1 1.0" (the DMI version follows the product name)
        if re.fullmatch(r"[A-Za-z]+\d+,\d+", w):
            return w
    return words[-1]


def image_version(info):
    """`0.1.0-alpha.3` from `image  IYAKKI 0.1.0-alpha.3 · variant pc-x86_64`, or None."""
    v = info_fields(info).get("image")
    m = re.search(r"IYAKKI (\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)", v[0]) if v else None
    return m.group(1) if m else None


_T2 = (r"MacBookPro15,[1-4]", r"MacBookPro16,[1-4]", r"MacBookAir8,[12]", r"MacBookAir9,1", r"Macmini8,1", r"iMac20,[12]",
       r"iMacPro1,1", r"MacPro7,1")


def is_t2(model):
    """True for the Macs that carry the Apple T2 chip (the family the aaudio / apple-bce rows are about)."""
    return bool(model) and any(re.fullmatch(p, model) for p in _T2)


# ---------------------------------------------------------------- input devices (/proc/bus/input/devices)

def _bit(words, n):
    """Bit n of a kernel bitmap printed as hex words, most significant word first, 64 bits per word."""
    ws = words.split()
    i = n // 64
    if i >= len(ws):
        return False
    return bool((int(ws[len(ws) - 1 - i], 16) >> (n % 64)) & 1)


def input_devices(text):
    """Blocks of /proc/bus/input/devices -> [{name, handlers, direct, abs_x, mt_x, mt_slot, btn_touch}]."""
    devs = []
    for block in re.split(r"\n\s*\n", text.strip()):
        d = {"name": "", "handlers": "", "prop": "0", "abs": "0", "key": "0"}
        seen = False
        for line in block.splitlines():
            m = re.match(r'^N: Name="(.*)"', line)
            if m:
                d["name"] = m.group(1); seen = True
            m = re.match(r"^H: Handlers=(.*)", line)
            if m: d["handlers"] = m.group(1)
            m = re.match(r"^B: (PROP|ABS|KEY)=(.*)", line)
            if m: d[m.group(1).lower()] = m.group(2).strip() or "0"
        if not seen:
            continue
        d["direct"] = _bit(d["prop"], 1)              # INPUT_PROP_DIRECT
        d["abs_x"] = _bit(d["abs"], 0)                # ABS_X
        d["mt_slot"] = _bit(d["abs"], 0x2f)           # ABS_MT_SLOT
        d["mt_x"] = _bit(d["abs"], 0x35)              # ABS_MT_POSITION_X
        d["btn_touch"] = _bit(d["key"], 0x14a)        # BTN_TOUCH
        devs.append(d)
    return devs


def touch_screens(devs):
    """Devices that are touch screens or digitizers: direct input with a position and (a touch button or multi-touch)."""
    return [d for d in devs if d["direct"] and (d["abs_x"] or d["mt_x"]) and (d["btn_touch"] or d["mt_x"])]


def multitouch_only(devs):
    """Touch screens that report only multi-touch positions (no plain ABS_X): the 'multi-touch-only panels' of F-BOOT-06."""
    return [d for d in touch_screens(devs) if d["mt_x"] and not d["abs_x"]]


# ---------------------------------------------------------------- kernel configuration

def kconfig(text):
    """Lines of /proc/config.gz (decompressed) -> {CONFIG_X: 'y'|'m'|value}. `# CONFIG_X is not set` -> 'n'."""
    cfg = {}
    for line in text.splitlines():
        m = re.match(r"^(CONFIG_\w+)=(.*)$", line)
        if m:
            cfg[m.group(1)] = m.group(2).strip('"')
            continue
        m = re.match(r"^# (CONFIG_\w+) is not set", line)
        if m:
            cfg[m.group(1)] = "n"
    return cfg


BATTERY_OPTIONS = ("CONFIG_ACPI_BATTERY", "CONFIG_ACPI_AC", "CONFIG_POWER_SUPPLY", "CONFIG_ACPI_SBS", "CONFIG_PM", "CONFIG_ACPI_BUTTON", "CONFIG_ACPI_VIDEO")


def options_missing(cfg, wanted):
    """Wanted options that are not y (or m): returns [(name, value-or-'absent')]. Empty list = all present."""
    return [(o, cfg.get(o, "absent")) for o in wanted if cfg.get(o) not in ("y", "m")]


# ---------------------------------------------------------------- backlight

_BL_RANK = ("intel_backlight", "amdgpu_bl", "radeon_bl", "nvidia", "apple-panel-bl", "gmux_backlight", "acpi_video")


def backlights(text):
    """Lines `BL name brightness max` -> [{name, brightness, max}], best first by the row's order; the Touch Bar's is dropped."""
    found = []
    for line in text.splitlines():
        m = re.match(r"^BL (\S+) (\d+) (\d+)\s*$", line)
        if m and "touchbar" not in m.group(1).lower() and "tb_" not in m.group(1).lower():
            found.append({"name": m.group(1), "brightness": int(m.group(2)), "max": int(m.group(3))})
    def rank(d):
        for i, p in enumerate(_BL_RANK):
            if d["name"].startswith(p):
                return i
        return len(_BL_RANK)
    return sorted(found, key=rank)


def kbd_lights(text):
    return [m.group(1) for m in re.finditer(r"^LED (\S*kbd_backlight\S*)\s*$", text, re.M)]


# ---------------------------------------------------------------- sound

def playback_pcms(text):
    """/proc/asound/pcm lines `00-00: ALC ... : playback 1 : capture 1` -> [(card, device, name)] with playback."""
    out = []
    for line in text.splitlines():
        m = re.match(r"^(\d+)-(\d+): (.*?)(?: : (.*))?$", line)
        if m and "playback" in line:
            out.append((int(m.group(1)), int(m.group(2)), m.group(3).strip()))
    return out


def cs8409_codec(text):
    """True when a /proc/asound/card*/codec#* dump names the Cirrus CS8409 codec."""
    return bool(re.search(r"^Codec:.*CS8409", text, re.M | re.I))


def t2_audio_lines(dmesg):
    return [l for l in dmesg.splitlines() if re.search(r"aaudio|apple[-_]bce|Apple T2 Audio", l, re.I)]


# ---------------------------------------------------------------- Bluetooth

def hci_lines(dmesg):
    """Kernel lines about the Bluetooth controller (hci, hci_uart_bcm, BCM...), for the log of a recipe."""
    return [l for l in dmesg.splitlines() if re.search(r"hci\d|hci_uart|hci_bcm|BCM\d{4}|Bluetooth:", l)]


def bt_adapter_present(dmesg, sysfs_list=""):
    """True when the kernel created an hci device (`hci0` in the log or in `ls /sys/class/bluetooth`)."""
    return bool(re.search(r"\bhci\d+\b", dmesg) or re.search(r"^hci\d+$", sysfs_list, re.M))


def bcm4364_state(dmesg, address_file):
    """Mac mini 2018: ('address', ..) when hci0 has an address; ('no-patch', ..) when the controller answered but the .hcd
    patch file is missing (the documented limit); ('silent', ..) when nothing about the chip is in the log."""
    if address_file and re.fullmatch(r"([0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2}", address_file.strip()):
        return ("address", address_file.strip())
    if re.search(r"Patch file not found|BCM\.hcd", dmesg):
        return ("no-patch", "controller answered, patch file brcm/BCM.hcd not found")
    if re.search(r"BCM4364|hci_uart_bcm|BCM2E7C", dmesg):
        return ("answered", "controller registered, no address file")
    return ("silent", "no Bluetooth controller line in the kernel log")


# ---------------------------------------------------------------- network

BROADCOM_NO_DRIVER_IDS = ("43a0", "43a1", "43a2", "4360", "4352", "43b1")


def net_facts(text):
    """Shell output with `NET name kind carrier` and `PCI vendor device` lines -> ({name: kind}, broadcom_without_driver)."""
    links, pci = {}, []
    for line in text.splitlines():
        m = re.match(r"^NET (\S+) (eth|wifi)\b", line)
        if m:
            links[m.group(1)] = m.group(2)
        m = re.match(r"^PCI 0x([0-9a-fA-F]{4}) 0x([0-9a-fA-F]{4})", line)
        if m:
            pci.append((m.group(1).lower(), m.group(2).lower()))
    broadcom = any(v == "14e4" and d in BROADCOM_NO_DRIVER_IDS for v, d in pci)
    return links, broadcom


# ---------------------------------------------------------------- sockets and files

def listeners(netstat):
    """`netstat -ltn` -> [(address, port)] of listening TCP sockets."""
    return [(m.group(1), int(m.group(2))) for m in re.finditer(r"^tcp\S*\s+\d+\s+\d+\s+(\S+):(\d+)\s", netstat, re.M)]


def non_loopback(socks, ports):
    return [(a, p) for a, p in socks if p in ports and not (a.startswith("127.") or a == "::1")]


def bootlog_state(text):
    """Shell output of the boot-log probe -> ('found', path, lines) / ('missing', mountpoint, 0) / ('no-esp', '', 0)."""
    m = re.search(r"^FOUND (\S+) (\d+)$", text, re.M)
    if m:
        return ("found", m.group(1), int(m.group(2)))
    m = re.search(r"^ESP (\S+) MISSING$", text, re.M)
    if m:
        return ("missing", m.group(1), 0)
    return ("no-esp", "", 0)


def sample_media(text):
    """`ls -1` of the samples directories (marked `== dir`) -> {dir: [files]}."""
    out, cur = {}, None
    for line in text.splitlines():
        if line.startswith("== "):
            cur = line[3:].strip(); out[cur] = []
        elif cur is not None and line.strip():
            out[cur].append(line.strip())
    return out


def usb_steps(text):
    """`iyakki dev usb` check list -> sorted step numbers 1..7 seen. A step line is `[ ok ] 1  Cable ...`, `[FAIL] 3 ...` or
    `[ .. ] 4 ...` (a status tag, the number, the title); the plain `1. title` / `2) title` forms are also read."""
    return sorted(usb_status(text))


def usb_status(text):
    """Same lines as usb_steps -> {step: 'ok' | 'fail' | 'waiting' | 'other' (a plain numbered line without a tag)}."""
    out = {}
    for m in re.finditer(r"^\s*(?:\[\s*([A-Za-z.]*)\s*\]\s+(\d)\s+|(\d)[.)]\s+)\S", text, re.M):
        n = int(m.group(2) or m.group(3))
        if not 1 <= n <= 7:
            continue
        tag = (m.group(1) or "").lower() if m.group(2) else None
        out[n] = {"ok": "ok", "fail": "fail", "..": "waiting"}.get(tag, "other") if tag is not None else "other"
    return out


def video_nodes(text):
    """`ls /sys/class/video4linux` style lines `V video0 name` -> [(node, name)]."""
    return [(m.group(1), m.group(2).strip()) for m in re.finditer(r"^V (video\d+) (.*)$", text, re.M)]
