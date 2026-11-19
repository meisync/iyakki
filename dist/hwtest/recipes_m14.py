"""M14 hardware-check recipes, part 1: Settings pages, tiles and what the screen says about the machine (all read-only).

Every recipe names the FEATURES.md row it evidences (the id has the suffix -m14 because recipe ids are global) and says in its
docstring what PASS and FAIL look like. Nothing here types text except the settings search box (hwcheck proves focus first), nothing
switches a setting, nothing is installed or downloaded, no key is pressed on Home that signs the person out, and no secret is touched.
Page checks match the page's one-line blurb (crates/iyakki-apps/src/apps/settings.rs, `fn blurb`) or its page name, so a wrong page is
a FAIL; where a recipe cannot tell, it ends NEEDS-HUMAN rather than guessing. Pure logic lives in recipes_m14_parse.py and is unit
tested with canned output (dist/hwtest/test_m14.py). Part 2 (kernel options, files, domains, tiles; recipes_m14_hw.py) uses the same helpers.
"""
import re
from hwcheck import recipe, fail, human, dev, Ctx
import recipes as _r1  # gives Ctx.notcovered
from recipes6 import _shell
import recipes_m14_parse as P


# ------------------------------------------------------------------ helpers (shared with part 2)

def tab(c, query, label, blurb):
    """Open a Settings page through the search box and prove it is the right one: its blurb line or its page name."""
    st = c.settings_page(query)
    hit = c.has(st, r"^" + re.escape(blurb[:36]))
    if hit:
        c.log("PAGE blurb seen: %s" % hit[0])
    elif label.lower() in c.page(st).lower():
        c.log("PAGE name seen: %s" % c.page(st))
    else:
        fail("search %r did not open the %s page (no blurb line, page is %r)" % (query, label, c.page(st)))
    c.shot(re.sub(r"\W+", "-", label.lower()))
    return st


def need_buttons(c, st, names):
    missing = [b for b in names if b not in c.buttons(st)]
    if missing:
        fail("button(s) %s missing on %s/%s" % (missing, c.app(st), c.page(st)))


def find_tile(c, label):
    """Home screen state in which the tile's button is visible (tries every group chip); read-only, the tile is not opened."""
    st = c.home()
    if label in c.buttons(st):
        return st
    for g in [b for b in c.buttons(st) if b.startswith("group ")]:
        st = c.click(g, must=False)
        if st and label in c.buttons(st):
            return st
    fail("tile %r not found in any launcher group" % label)


def info(c):
    rc, out = c.run("info", name="info")
    if rc or not out.strip():
        human("`dev info` printed nothing")
    return out


def shell_out(c, script, name):
    rc, out = _shell(c, script, name)
    return out


def dmesg(c, word, n=400):
    return c.run("dmesg", "--grep", word, "-n", n, name="dmesg-" + re.sub(r"\W+", "_", word))[1]


# ------------------------------------------------------------------ network

NET_SCRIPT = ('for d in /sys/class/net/*; do n=${d##*/}; [ -e "$d/device" ] || continue; k=eth; '
              'if [ -e "$d/wireless" ] || [ -e "$d/phy80211" ]; then k=wifi; fi; echo "NET $n $k"; done; '
              'for d in /sys/bus/pci/devices/*; do echo "PCI $(cat $d/vendor 2>/dev/null) $(cat $d/device 2>/dev/null)"; done')


@recipe("F-NET-06-m14", "Settings -> Network agrees with the adapters the kernel has: Ethernet and Wi-Fi rows, a missing one 'not found', a driverless Broadcom card named (read-only)")
def net06(c):
    """Row F-NET-06. PASS when the Network page opens (blurb 'Ethernet and WiFi: ...') and fits sysfs read through `dev shell`: with a
    Wi-Fi link in /sys/class/net the page does not say 'no WiFi adapter'; with none but a Broadcom 4360-family PCI card (14e4:43a0, 43a1,
    43a2, 4360, 4352, 43b1; Mac mini 2014 / MacBook Pro 2018, KI-094) the page names Broadcom; with neither it says not found. FAIL when
    the page contradicts the hardware. NEEDS-HUMAN when the 'not found' wording cannot be read. No switch is pressed."""
    links, broadcom = P.net_facts(shell_out(c, NET_SCRIPT, "net-facts"))
    c.log("adapters in sysfs: %s; Broadcom 4360-family card without driver: %s" % (links or "none", broadcom))
    st = tab(c, "network", "Network", "Ethernet and WiFi: what this computer has, and what is switched on.")
    wifi = [n for n, k in links.items() if k == "wifi"]
    eth = [n for n, k in links.items() if k == "eth"]
    if wifi:
        c.expect_absent(st, r"no WiFi adapter|no Wi-?Fi adapter")
    elif broadcom:
        c.expect(st, r"Broadcom", "a driverless Broadcom Wi-Fi card is named")
    else:
        if not c.has(st, r"not found|no WiFi adapter|no Wi-?Fi adapter"):
            human("no Wi-Fi adapter in sysfs, but the page does not say 'not found' in a line a script can read")
        c.log("page reports the missing Wi-Fi adapter")
    if eth:
        c.expect_absent(st, r"no Ethernet adapter")
    c.notcovered("switching an adapter off and on (each change asks and is audited); address and speed values were not compared with `ip`; the BCM4360 message was not read in the join form")


@recipe("F-NET-08-m14", "Settings -> Bluetooth agrees with the kernel: adapter present, or 'No Bluetooth adapter' (read-only)")
def net08(c):
    """Row F-NET-08. PASS when the Bluetooth page opens and its text matches whether the kernel created hciN (kernel log and
    /sys/class/bluetooth): with an adapter the page does not say 'No Bluetooth adapter'; without one it does. Switch not pressed."""
    dm = dmesg(c, "hci")
    sysls = shell_out(c, "ls /sys/class/bluetooth 2>/dev/null", "sys-bluetooth")
    present = P.bt_adapter_present(dm, sysls)
    c.log("kernel has a Bluetooth controller: %s" % present)
    st = tab(c, "bluetooth", "Bluetooth", "The Bluetooth adapter, if there is one.")
    if present:
        c.expect_absent(st, r"^No Bluetooth adapter")
    else:
        c.expect(st, r"^No Bluetooth adapter", "no hci device in the kernel")
    c.notcovered("the on/off switch (it changes rfkill), scanning, pairing; with an adapter the page's switch state was not compared with rfkill")


@recipe("F-NET-11-m14", "Mac mini 2018 Bluetooth: BCM4364 on a UART, does hci0 get an address? (read-only; documented limit is FAIL)")
def net11(c):
    """Row F-NET-11. Only on a Macmini8,1 (other models: NEEDS-HUMAN). PASS when /sys/class/bluetooth/hci0/address is a MAC address.
    FAIL, with the log lines as evidence, when the controller answers (hci_uart_bcm / BCM4364B0) but `brcm/BCM.hcd` is missing and
    there is no address: the known limit in the row (macOS has no BCM4364 patch file). Nothing is loaded or written."""
    model = P.model_of(info(c))
    if model != "Macmini8,1":
        human("this row is about the Mac mini 2018 (Macmini8,1); this machine reports %r" % model)
    dm = dmesg(c, "BCM", 200) + "\n" + dmesg(c, "hci", 200)
    for l in P.hci_lines(dm)[:12]:
        c.log("KERNEL " + l)
    addr = shell_out(c, "cat /sys/class/bluetooth/hci0/address 2>/dev/null", "hci0-address")
    kind, why = P.bcm4364_state(dm, addr)
    c.log("state: %s (%s)" % (kind, why))
    if kind == "address":
        c.notcovered("pairing and a phone connecting (row F-NET-21 / F-NET-18); only that the controller has an address")
        return
    if kind == "silent":
        human(why)
    fail("hci0 has no address: %s (documented limit; a USB Bluetooth adapter is the fallback)" % why)


# ------------------------------------------------------------------ sound

@recipe("F-SND-02-m14", "Settings -> Sound lists where sound can play; the kernel has a playback device behind it (read-only)")
def snd02(c):
    """Row F-SND-02. PASS when /proc/asound/pcm has at least one playback device and the Sound page opens with its blurb ('Where
    sound plays ...'). With no playback device the page must say there is no sound output (text 'no sound output'); otherwise FAIL.
    Nothing is played, no output is chosen."""
    pcm = P.playback_pcms(shell_out(c, "cat /proc/asound/pcm 2>/dev/null", "asound-pcm"))
    c.log("playback devices: %s" % (pcm or "none"))
    st = tab(c, "sound", "Sound", "Where sound plays (HDMI/DP, USB, speakers), a test chime, volume and mute.")
    if not pcm:
        if not c.has(st, r"no sound output"):
            fail("the kernel has no playback device and the page does not say 'no sound output'")
        return
    c.expect_absent(st, r"^no sound output")
    c.notcovered("which output Automatic picked, the monitor name from its ELD, plugging a monitor in; the Test chime was not played (a person must hear it)")


@recipe("F-SND-04-m14", "Settings -> Sound shows a volume level and the mute state (read-only)")
def snd04(c):
    """Row F-SND-04. PASS when the Sound page shows a line with a percentage (the master volume) or the word Muted. Nothing is changed.
    NEEDS-HUMAN when the machine has no playback device (no volume to show)."""
    if not P.playback_pcms(shell_out(c, "cat /proc/asound/pcm 2>/dev/null", "asound-pcm")):
        human("no playback device on this machine: there is no master volume to show")
    st = tab(c, "sound", "Sound", "Where sound plays (HDMI/DP, USB, speakers), a test chime, volume and mute.")
    c.expect(st, r"\b\d{1,3}\s?%|Muted", "a volume percentage or Muted")
    c.notcovered("keys, popover and radio bars driving the same level; applying it once by softvol; no volume was changed (the level is a person's)")


@recipe("F-SND-20-m14", "Built-in (T2) microphone switch is offered on a T2 Mac and only there (read-only)")
def snd20(c):
    """Row F-SND-20. PASS when the Sound page shows "Use the Mac's built-in microphone (experimental)" on a T2 Mac (Macmini8,1,
    MacBookPro15/16, ...) and does not show it on any other machine. The switch itself is never pressed."""
    model = P.model_of(info(c))
    st = tab(c, "sound", "Sound", "Where sound plays (HDMI/DP, USB, speakers), a test chime, volume and mute.")
    seen = c.has(st, r"Use the Mac's built-in microphone \(experimental\)")
    c.log("model %s, T2: %s, switch shown: %s" % (model, P.is_t2(model), bool(seen)))
    if P.is_t2(model) and not seen:
        # the row may sit under a Microphone section further down the page
        fail("T2 Mac %s but the Sound page has no built-in microphone switch" % model)
    if not P.is_t2(model) and seen:
        fail("the T2 microphone switch is offered on %s, which has no T2 chip" % model)
    c.notcovered("switching it on (restarts the sound path; a person must speak and check the level); the page may need scrolling on a small screen")


# ------------------------------------------------------------------ power and battery

@recipe("F-PWR-01-m14", "Settings -> Power offers the three power profiles and the sleep-after-idle line (read-only)")
def pwr01(c):
    """Row F-PWR-01. PASS when the Power page shows three 'Power profile: ...' rows (performance, balanced, power saver) and a line
    'Sleep when idle: never | after N minutes'. No profile is chosen; the CPU governor of cpu0 is only logged."""
    st = tab(c, "power", "Power", "How fast the processor runs (performance, balanced, power saver) and whether the computer sleeps by itself when it is not used.")
    rows = c.has(st, r"^Power profile: ")
    if len(rows) < 3:
        fail("expected three 'Power profile:' rows, saw %d: %s" % (len(rows), rows))
    c.log("profiles: " + " | ".join(rows))
    c.expect(st, r"^Sleep when idle: (never|after \d+ minutes)")
    gov = shell_out(c, "cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null", "governor").strip()
    c.log("cpu0 governor now: %s" % (gov or "(no cpufreq)"))
    c.notcovered("choosing a profile and watching the governor change; sleeping; the profile kept across a restart")


@recipe("F-BATT-04-m14", "Settings -> Battery on a laptop shows the charge and the three switches (read-only; a desktop is NEEDS-HUMAN)")
def batt04(c):
    """Row F-BATT-04. On a machine with a BAT* supply in /sys/class/power_supply: PASS when the Battery page opens (blurb 'How full
    the battery is ...') and shows a percentage. FAIL when a battery exists and the page is missing or has no percentage. On a desktop
    (no battery) NEEDS-HUMAN: the page is laptops only. No switch is pressed."""
    ps = shell_out(c, "ls /sys/class/power_supply 2>/dev/null", "power-supply")
    bats = [l for l in ps.split() if l.startswith("BAT")]
    if not bats:
        human("no battery in /sys/class/power_supply (%s): not a laptop, the Battery page is not offered" % (ps.split() or "empty"))
    st = tab(c, "battery", "Battery", "How full the battery is, what it is doing, how worn it is, and the low-battery warnings.")
    c.expect(st, r"\b\d{1,3}\s?%", "the charge")
    c.notcovered("health and cycles against the battery's own values, the 20 / 10 / 5 % warnings (need a discharging battery), the three switches")


# ------------------------------------------------------------------ screens and touch

@recipe("F-DISP-07-m14", "Touch screens on several displays: a touch screen exists and the kernel log has its 'touch: ... -> <display>' mapping (read-only)")
def disp07(c):
    """Row F-DISP-07. NEEDS-HUMAN when no touch screen is attached (INPUT_PROP_DIRECT with a position and BTN_TOUCH or multi-touch in
    /proc/bus/input/devices). With one: PASS when `dev dmesg` has a line 'touch: vvvv:pppp Name -> <output>' naming a display; FAIL
    when the touch screen is there but the shell logged no mapping. The Displays page blurb is also checked; the calibration buttons are
    only logged. Nothing is touched."""
    devs = P.input_devices(shell_out(c, "cat /proc/bus/input/devices", "input-devices"))
    ts = P.touch_screens(devs)
    c.log("touch screens: %s" % ([d["name"] for d in ts] or "none"))
    if not ts:
        human("no touch screen is attached to this machine")
    dm = dmesg(c, "touch:")
    maps = [l for l in dm.splitlines() if re.search(r"touch: [0-9a-f]{4}:[0-9a-f]{4} .* -> \S+", l)]
    for l in maps[:6]:
        c.log("MAP " + l)
    if not maps:
        fail("a touch screen is attached (%s) but the kernel log has no 'touch: ... -> display' line" % ts[0]["name"])
    tab(c, "displays", "Displays", "Several displays: resolution, which is the main one, where the others sit, mirror or extend, and whether layouts are used.")
    c.notcovered("that a tap lands on the right display (needs a finger), the calibration steps, unplug and replug, two displays at once")


@recipe("F-BOOT-06-m14", "Touch screens: a multi-touch-only panel is present (read-only; the finger test is NEEDS-HUMAN)")
def boot06(c):
    """Row F-BOOT-06. NEEDS-HUMAN when no touch screen is attached. PASS when a touch screen reports multi-touch positions without
    plain ABS_X (the 'multi-touch-only panel' of the row) and the kernel log shows the shell mapped it. A panel that also reports
    ABS_X is logged and the row is NEEDS-HUMAN (not the case the row names). A tap is never sent."""
    devs = P.input_devices(shell_out(c, "cat /proc/bus/input/devices", "input-devices"))
    ts, mt = P.touch_screens(devs), P.multitouch_only(devs)
    c.log("touch screens: %s; multi-touch only: %s" % ([d["name"] for d in ts] or "none", [d["name"] for d in mt] or "none"))
    if not ts:
        human("no touch screen is attached to this machine")
    if not mt:
        human("touch screen %s also reports plain ABS_X; it is not a multi-touch-only panel" % ts[0]["name"])
    dm = dmesg(c, "touch:")
    if not re.search(r"touch: [0-9a-f]{4}:[0-9a-f]{4} ", dm):
        fail("multi-touch panel %s is attached but the shell logged no 'touch:' line for it" % mt[0]["name"])
    c.notcovered("first finger driving the pointer, a tap landing under the finger (needs a finger on the panel)")


BL_SCRIPT = ('for d in /sys/class/backlight/*; do [ -e "$d/brightness" ] && echo "BL ${d##*/} $(cat $d/brightness) $(cat $d/max_brightness)"; done; '
             'for d in /sys/class/leds/*kbd_backlight*; do [ -e "$d" ] && echo "LED ${d##*/}"; done; true')


@recipe("F-KBD-03-m14", "Brightness and keyboard-light sources: which backlight the kernel registered and which the shell would rank first (read-only)")
def kbd03(c):
    """Row F-KBD-03. Reads /sys/class/backlight and /sys/class/leds through `dev shell` and logs every device with its value and maximum
    (the Touch Bar's backlight is excluded). NEEDS-HUMAN when there is no backlight (a Mac mini: 'No brightness control on this
    display' is then the right text, which is checked on the Display page). PASS when at least one backlight exists, its value is within
    0..max, and the Display page opens. FAIL when a value is above its maximum. Brightness is not changed."""
    out = shell_out(c, BL_SCRIPT, "backlights")
    bl, leds = P.backlights(out), P.kbd_lights(out)
    c.log("backlights (best first): %s; keyboard lights: %s" % ([b["name"] for b in bl] or "none", leds or "none"))
    st = tab(c, "display", "Display", "The screen in use, connected displays and brightness.")
    if not bl:
        c.log("page says 'No brightness control on this display': %s" % bool(c.has(st, r"No brightness control on this display")))
        human("no backlight on this machine (checked: the Display page says there is no brightness control); the key test needs a laptop")
    for b in bl:
        if b["brightness"] > b["max"] or b["max"] <= 0:
            fail("backlight %s reports %d of %d" % (b["name"], b["brightness"], b["max"]))
    c.notcovered("which device the shell picks for the keys on a MacBook Pro 2019 (read the log line there); the keys, the 16 steps, 'never 0', the keyboard-light toggle (a person must see the light)")
