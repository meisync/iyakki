"""Unit tests of the M14 recipes' logic with canned `iyakki dev` output. No device, no network, nothing run on a machine.

    python3 -I -m unittest discover -s dist/hwtest -p 'test_m14.py'
"""
import os, re, sys, unittest
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import hwcheck
from hwcheck import Ctx, Verdict, RECIPES
import recipes_m14_parse as P
import recipes_m14 as R1
import recipes_m14_hw as R2


def _read(fn):
    with open(fn) as f:
        return f.read()


def state(app="Settings", page="X", texts=(), buttons=()):
    t = "\n".join("%d,%d %s" % (10, 20 + i * 20, x) for i, x in enumerate(texts))
    b = "\n".join(" %d,%d 100x30 %s" % (5, 5, x) for x in buttons)
    return "app: %s\npage: %s\nfocused: nothing\nscreen: 1920x1080\n%s\n\nbuttons\n%s\n" % (app, page, t, b)


class Fake(Ctx):
    """A Ctx with no device: settings_page/open_tile/click return scripted screens."""
    def __init__(self, page_state=None, tile_state=None, home_buttons=("CAMERA",), home_texts=()):
        self.rid, self.dir, self.n, self.touched, self.lines = "T", "/nonexistent", 0, [], []
        self.page_state = page_state or state()
        self.tile_state = tile_state or state(app="Tile")
        self.home_state = state(app="Home", texts=home_texts, buttons=home_buttons)
        self.clicks = []
    def log(self, s): self.lines.append(s)
    def shot(self, name): return name
    def state(self, tag=None): return self.page_state
    def settings_page(self, q): self.q = q; return self.page_state
    def home(self): return self.home_state
    def click(self, label, wait=1.2, must=True):
        self.clicks.append(label)
        return self.tile_state if label not in ("IYAKKI menu",) else state(app="Home", buttons=("About IYAKKI", "Settings"))
    def open_tile(self, label, groups=()): self.opened = label; return self.tile_state
    def run(self, *a, name=None, timeout=60): return self.canned.get(a[0], (0, ""))


def verdict(f, c, **patches):
    """Run recipe function f(c) with module-level helpers replaced; returns (kind, reason)."""
    saved = []
    for mod, name, val in [(R1, "shell_out", None), (R2, "shell_out", None), (R1, "info", None), (R2, "info", None), (R1, "dmesg", None), (R2, "dmesg", None)]:
        pass
    for key, val in patches.items():
        for mod in (R1, R2):
            if hasattr(mod, key):
                saved.append((mod, key, getattr(mod, key))); setattr(mod, key, val)
    try:
        f(c)
        return ("PASS", getattr(c, "not_covered", []))
    except Verdict as e:
        return (e.kind, e.reason)
    finally:
        for mod, key, old in saved: setattr(mod, key, old)


def sh(table):
    """A fake shell_out: returns the canned text of the first key found in the script, else ''."""
    def f(c, script, name):
        for k, v in table.items():
            if k in script or k == name: return v
        return ""
    return f


INFO_MINI = "device  Apple Inc. Macmini8,1\nimage  IYAKKI 0.1.0-alpha.3 · variant pc-x86_64\nnetwork  eth0  1000\n"
INFO_MBP = "device  Apple Inc. MacBookPro14,1\nimage  IYAKKI 0.1.0-alpha.3 · variant pc-x86_64\n"
INFO_LENOVO = """iyakki
  device     LENOVO 82WM Legion Pro 5 16ARX8
  board/bios LNVNB161216 / LPCN25WW
  image      IYAKKI 0.1.0-alpha.2 \u00b7 variant pc-x86_64 \u00b7 built 2026-11-12 \u00b7 from an image
  kernel     6.12.112 \u00b7 78 modules loaded
  mount      /.stage1/boot          vfat  12.2 MB of 486.6 MB free (read-only)
  network    eth0       192.168.0.2
"""
USB_REAL = """[ ok ] 1  Cable and port can carry a network
       A Thunderbolt/USB4 port is present. Security level: user (you approve each new device (the Authorize button below)).
[ ok ] 2  The other computer is connected
       A Thunderbolt device is connected and approved, but it is not a computer joined host to host.
[FAIL] 3  Network interface is up with an address
       No network interface for the cable.
[ .. ] 4  The other IYAKKI is found
       Waiting for step 3.
[ .. ] 5  Developer mode answers
       Waiting for step 4.
[ .. ] 6  This computer is paired with it
       Waiting for step 5.
[ .. ] 7  Speed and delay
       Waiting for step 6.

Last result: 2 of 7 steps passed. Stuck at: Network interface is up with an address
"""
INFO_TUNNEL = INFO_MINI + "network  wg0  10.0.0.2\n"


class Parsers(unittest.TestCase):
    def test_info(self):
        self.assertEqual(P.model_of(INFO_MINI), "Macmini8,1")
        self.assertEqual(P.image_version(INFO_MINI), "0.1.0-alpha.3")
        self.assertIsNone(P.image_version("device  x y\n"))
        self.assertIsNone(P.model_of("nothing"))

    def test_info_real_device_layout(self):
        # indented `key  value` lines, host name on the first line (evidence of 2026-11-12, Lenovo Legion Pro 5)
        self.assertEqual(P.image_version(INFO_LENOVO), "0.1.0-alpha.2")
        self.assertEqual(P.info_fields(INFO_LENOVO)["mount"][0].split()[0], "/.stage1/boot")
        self.assertNotIn("iyakki", P.info_fields(INFO_LENOVO))

    def test_usb_steps_real_output(self):
        self.assertEqual(P.usb_steps(USB_REAL), [1, 2, 3, 4, 5, 6, 7])
        self.assertEqual(P.usb_status(USB_REAL), {1: "ok", 2: "ok", 3: "fail", 4: "waiting", 5: "waiting", 6: "waiting", 7: "waiting"})
        self.assertEqual(P.usb_steps("Last result: 2 of 7 steps passed.\n       Waiting for step 3.\n"), [])

    def test_t2(self):
        for m in ("Macmini8,1", "MacBookPro16,2", "MacBookAir9,1", "iMac20,1"): self.assertTrue(P.is_t2(m), m)
        for m in ("MacBookPro14,1", "Macmini7,1", "HP Pavilion", None): self.assertFalse(P.is_t2(m), m)

    def test_bitmaps(self):
        self.assertTrue(P._bit("3", 0) and P._bit("3", 1) and not P._bit("3", 2))
        self.assertTrue(P._bit("1 0000000000000000", 64) and not P._bit("1 0000000000000000", 0))
        self.assertFalse(P._bit("0", 100))

    TOUCH = ('I: Bus=0003 Vendor=27c0 Product=0859 Version=0111\nN: Name="TouchScreen"\nH: Handlers=mouse1 event5\n'
             'B: PROP=2\nB: EV=b\nB: KEY=400 0 0 0 0 0\nB: ABS=6618000 0\n\n'
             'I: Bus=0011 Vendor=0001 Product=0001 Version=ab41\nN: Name="AT keyboard"\nH: Handlers=sysrq kbd event3\nB: PROP=0\nB: EV=120013\nB: KEY=1 0\n\n'
             'I: Bus=0003 Vendor=1\nN: Name="MT panel"\nH: Handlers=event6\nB: PROP=2\nB: EV=1b\nB: KEY=400 0 0 0 0 0\nB: ABS=260800000000000\n')

    def test_touch(self):
        devs = P.input_devices(self.TOUCH)
        self.assertEqual([d["name"] for d in devs], ["TouchScreen", "AT keyboard", "MT panel"])
        names = [d["name"] for d in P.touch_screens(devs)]
        self.assertNotIn("AT keyboard", names)  # a keyboard is never a touch screen (precise cases: test_multitouch_only)

    def test_multitouch_only(self):
        # ABS bits: 0x35,0x36 (MT position) and 0x2f (slot) set, ABS_X (0) not set
        mt = 'N: Name="P"\nB: PROP=2\nB: KEY=400 0 0 0 0 0\nB: ABS=%x\n' % ((1 << 0x2f) | (1 << 0x35) | (1 << 0x36))
        both = 'N: Name="Q"\nB: PROP=2\nB: KEY=400 0 0 0 0 0\nB: ABS=%x\n' % ((1 << 0) | (1 << 1) | (1 << 0x35))
        mouse = 'N: Name="Mouse"\nB: PROP=0\nB: KEY=1f0000 0 0 0 0\nB: ABS=0\n'
        devs = P.input_devices(mt + "\n" + both + "\n" + mouse)
        self.assertEqual([d["name"] for d in P.touch_screens(devs)], ["P", "Q"])
        self.assertEqual([d["name"] for d in P.multitouch_only(devs)], ["P"])

    def test_kconfig(self):
        cfg = P.kconfig("CONFIG_PM=y\nCONFIG_ACPI_SBS=m\n# CONFIG_ACPI_AC is not set\nCONFIG_X=\"s\"\n")
        self.assertEqual((cfg["CONFIG_PM"], cfg["CONFIG_ACPI_SBS"], cfg["CONFIG_ACPI_AC"], cfg["CONFIG_X"]), ("y", "m", "n", "s"))
        miss = P.options_missing(cfg, ("CONFIG_PM", "CONFIG_ACPI_AC", "CONFIG_POWER_SUPPLY"))
        self.assertEqual(miss, [("CONFIG_ACPI_AC", "n"), ("CONFIG_POWER_SUPPLY", "absent")])

    def test_backlight_rank(self):
        out = "BL acpi_video0 5 10\nBL intel_backlight 100 400\nBL tb_touchbar 1 2\nLED apple::kbd_backlight\nLED input3::capslock\n"
        self.assertEqual([b["name"] for b in P.backlights(out)], ["intel_backlight", "acpi_video0"])
        self.assertEqual(P.kbd_lights(out), ["apple::kbd_backlight"])
        self.assertEqual(P.backlights(""), [])

    def test_sound(self):
        pcm = "00-00: ALC892 Analog : ALC892 Analog : playback 1 : capture 1\n00-03: HDMI 0 : HDMI 0 : playback 1\n01-00: Mic : capture 1\n"
        self.assertEqual([(a, b) for a, b, _ in P.playback_pcms(pcm)], [(0, 0), (0, 3)])
        self.assertTrue(P.cs8409_codec("Codec: Cirrus Logic CS8409\nAddress: 0\n"))
        self.assertFalse(P.cs8409_codec("Codec: Realtek ALC892\n"))
        self.assertEqual(len(P.t2_audio_lines("apple-bce 0000:02:00.1: x\nunrelated\naaudio: probe")), 2)

    def test_bluetooth(self):
        self.assertTrue(P.bt_adapter_present("Bluetooth: hci0: BCM4364B0", ""))
        self.assertTrue(P.bt_adapter_present("", "hci0\n"))
        self.assertFalse(P.bt_adapter_present("Bluetooth: Core ver 2.22", ""))
        self.assertEqual(P.bcm4364_state("", "aa:bb:cc:dd:ee:ff\n")[0], "address")
        self.assertEqual(P.bcm4364_state("BCM: chip id 123\nPatch file not found: brcm/BCM.hcd", "")[0], "no-patch")
        self.assertEqual(P.bcm4364_state("hci_uart_bcm serial0-0: x", "")[0], "answered")
        self.assertEqual(P.bcm4364_state("nothing", "")[0], "silent")

    def test_net(self):
        links, bc = P.net_facts("NET eno1 eth\nNET wlp2s0 wifi\nPCI 0x8086 0x1234\n")
        self.assertEqual(links, {"eno1": "eth", "wlp2s0": "wifi"}); self.assertFalse(bc)
        self.assertTrue(P.net_facts("PCI 0x14e4 0x43a0\n")[1])
        self.assertFalse(P.net_facts("PCI 0x14e4 0x4364\n")[1])

    def test_misc(self):
        socks = P.listeners("tcp  0  0 0.0.0.0:7979  0.0.0.0:* LISTEN\ntcp6 0 0 :::80 :::* LISTEN\ntcp 0 0 127.0.0.1:8080 0.0.0.0:* LISTEN\n")
        self.assertIn(("0.0.0.0", 7979), socks); self.assertIn(("127.0.0.1", 8080), socks)
        self.assertEqual(P.non_loopback([("127.0.0.1", 80), ("0.0.0.0", 443), ("0.0.0.0", 22)], (80, 443)), [("0.0.0.0", 443)])
        self.assertEqual(P.bootlog_state("FOUND /boot/EFI/iyakki-boot/last-boot.log 120\n"), ("found", "/boot/EFI/iyakki-boot/last-boot.log", 120))
        self.assertEqual(P.bootlog_state("ESP /boot MISSING\n")[0], "missing"); self.assertEqual(P.bootlog_state("")[0], "no-esp")
        self.assertEqual(P.sample_media("== Music\na.mp3\nb.mp3\n== Videos\nv.webm\n"), {"Music": ["a.mp3", "b.mp3"], "Videos": ["v.webm"]})
        self.assertEqual(P.usb_steps("1. one\n2) two\n7. seven\n9. nine\nnot a step\n"), [1, 2, 7])
        self.assertEqual(P.video_nodes("V video0 Integrated Camera\nV video1 Integrated Camera\n"), [("video0", "Integrated Camera"), ("video1", "Integrated Camera")])


NET_BLURB = "Ethernet and WiFi: what this computer has, and what is switched on."


class Screens(unittest.TestCase):
    def test_net06(self):
        wifi = Fake(state(page="Network", texts=[NET_BLURB, "Wi-Fi wlp2s0 connected"]))
        self.assertEqual(verdict(R1.net06, wifi, shell_out=sh({"NET": "NET eno1 eth\nNET wlp2s0 wifi\n"}))[0], "PASS")
        bad = Fake(state(page="Network", texts=[NET_BLURB, "This computer has no WiFi adapter"]))
        self.assertEqual(verdict(R1.net06, bad, shell_out=sh({"NET": "NET wlp2s0 wifi\n"}))[0], "FAIL")
        bro = Fake(state(page="Network", texts=[NET_BLURB, "Broadcom BCM43xx has no driver"]))
        self.assertEqual(verdict(R1.net06, bro, shell_out=sh({"NET": "NET eno1 eth\nPCI 0x14e4 0x43a0\n"}))[0], "PASS")
        nobro = Fake(state(page="Network", texts=[NET_BLURB]))
        self.assertEqual(verdict(R1.net06, nobro, shell_out=sh({"NET": "NET eno1 eth\nPCI 0x14e4 0x43a0\n"}))[0], "FAIL")
        none = Fake(state(page="Network", texts=[NET_BLURB, "WiFi adapter not found"]))
        self.assertEqual(verdict(R1.net06, none, shell_out=sh({"NET": ""}))[0], "PASS")
        silent = Fake(state(page="Network", texts=[NET_BLURB]))
        self.assertEqual(verdict(R1.net06, silent, shell_out=sh({"NET": ""}))[0], "NEEDS-HUMAN")

    def test_wrong_page_fails(self):
        c = Fake(state(page="Sound", texts=["Something else"]))
        self.assertEqual(verdict(R1.net06, c, shell_out=sh({"NET": ""}))[0], "FAIL")

    def test_net08(self):
        page = lambda extra: Fake(state(page="Bluetooth", texts=["The Bluetooth adapter, if there is one."] + extra))
        dm = lambda c, w, n=400: "Bluetooth: hci0: x"
        self.assertEqual(verdict(R1.net08, page(["Bluetooth on"]), shell_out=sh({}), dmesg=dm)[0], "PASS")
        self.assertEqual(verdict(R1.net08, page(["No Bluetooth adapter"]), shell_out=sh({}), dmesg=dm)[0], "FAIL")
        none = lambda c, w, n=400: ""
        self.assertEqual(verdict(R1.net08, page(["No Bluetooth adapter"]), shell_out=sh({}), dmesg=none)[0], "PASS")
        self.assertEqual(verdict(R1.net08, page(["Bluetooth on"]), shell_out=sh({}), dmesg=none)[0], "FAIL")

    def test_net11(self):
        c = lambda: Fake()
        dm = lambda c, w, n=400: "hci_uart_bcm serial0-0: x\nPatch file not found: brcm/BCM.hcd"
        self.assertEqual(verdict(R1.net11, c(), info=lambda c: INFO_MBP, dmesg=dm, shell_out=sh({}))[0], "NEEDS-HUMAN")
        self.assertEqual(verdict(R1.net11, c(), info=lambda c: INFO_MINI, dmesg=dm, shell_out=sh({}))[0], "FAIL")
        self.assertEqual(verdict(R1.net11, c(), info=lambda c: INFO_MINI, dmesg=dm, shell_out=sh({"hci0/address": "00:11:22:33:44:55"}))[0], "PASS")
        self.assertEqual(verdict(R1.net11, c(), info=lambda c: INFO_MINI, dmesg=lambda c, w, n=400: "", shell_out=sh({}))[0], "NEEDS-HUMAN")

    SND = "Where sound plays (HDMI/DP, USB, speakers), a test chime, volume and mute."
    PCM = "00-03: HDMI 0 : HDMI 0 : playback 1\n"

    def test_snd02_04(self):
        ok = Fake(state(page="Sound", texts=[self.SND, "Volume 40 %"]))
        self.assertEqual(verdict(R1.snd02, ok, shell_out=sh({"pcm": self.PCM}))[0], "PASS")
        self.assertEqual(verdict(R1.snd04, ok, shell_out=sh({"pcm": self.PCM}))[0], "PASS")
        self.assertEqual(verdict(R1.snd04, ok, shell_out=sh({"pcm": ""}))[0], "NEEDS-HUMAN")
        self.assertEqual(verdict(R1.snd02, ok, shell_out=sh({"pcm": ""}))[0], "FAIL")
        quiet = Fake(state(page="Sound", texts=[self.SND, "no sound output: connect a monitor"]))
        self.assertEqual(verdict(R1.snd02, quiet, shell_out=sh({"pcm": ""}))[0], "PASS")
        self.assertEqual(verdict(R1.snd02, quiet, shell_out=sh({"pcm": self.PCM}))[0], "FAIL")
        self.assertEqual(verdict(R1.snd04, Fake(state(page="Sound", texts=[self.SND])), shell_out=sh({"pcm": self.PCM}))[0], "FAIL")

    def test_snd20(self):
        sw = "Use the Mac's built-in microphone (experimental)"
        withsw = Fake(state(page="Sound", texts=[self.SND, sw])); without = Fake(state(page="Sound", texts=[self.SND]))
        self.assertEqual(verdict(R1.snd20, withsw, info=lambda c: INFO_MINI)[0], "PASS")
        self.assertEqual(verdict(R1.snd20, without, info=lambda c: INFO_MINI)[0], "FAIL")
        self.assertEqual(verdict(R1.snd20, withsw, info=lambda c: INFO_MBP)[0], "FAIL")
        self.assertEqual(verdict(R1.snd20, without, info=lambda c: INFO_MBP)[0], "PASS")

    PWR = "How fast the processor runs (performance, balanced, power saver) and whether the computer sleeps by itself when it is not used."

    def test_pwr01(self):
        rows = [self.PWR, "Power profile: Performance", "Power profile: Balanced", "Power profile: Power saver", "Sleep when idle: never"]
        self.assertEqual(verdict(R1.pwr01, Fake(state(page="Power", texts=rows)), shell_out=sh({}))[0], "PASS")
        self.assertEqual(verdict(R1.pwr01, Fake(state(page="Power", texts=rows[:3] + rows[4:])), shell_out=sh({}))[0], "FAIL")
        self.assertEqual(verdict(R1.pwr01, Fake(state(page="Power", texts=rows[:4])), shell_out=sh({}))[0], "FAIL")

    BAT = "How full the battery is, what it is doing, how worn it is, and the low-battery warnings."

    def test_batt04(self):
        self.assertEqual(verdict(R1.batt04, Fake(), shell_out=sh({"power_supply": "AC\n"}))[0], "NEEDS-HUMAN")
        self.assertEqual(verdict(R1.batt04, Fake(state(page="Battery", texts=[self.BAT, "87 %"])), shell_out=sh({"power_supply": "AC\nBAT0\n"}))[0], "PASS")
        self.assertEqual(verdict(R1.batt04, Fake(state(page="Battery", texts=[self.BAT])), shell_out=sh({"power_supply": "BAT0\n"}))[0], "FAIL")

    def test_touch_rows(self):
        mt = 'N: Name="P"\nB: PROP=2\nB: KEY=400 0 0 0 0 0\nB: ABS=%x\n' % ((1 << 0x2f) | (1 << 0x35) | (1 << 0x36))
        disp = Fake(state(page="Displays", texts=["Several displays: resolution, which is the main one, where the others sit, mirror or extend, and whether layouts are used."]))
        mapped = lambda c, w, n=400: "iyakki INFO ] touch: 27c0:0859 TouchScreen -> DP-4 (auto: only external display)"
        unmapped = lambda c, w, n=400: ""
        self.assertEqual(verdict(R1.disp07, disp, shell_out=sh({"input": mt}), dmesg=mapped)[0], "PASS")
        self.assertEqual(verdict(R1.disp07, disp, shell_out=sh({"input": mt}), dmesg=unmapped)[0], "FAIL")
        self.assertEqual(verdict(R1.disp07, disp, shell_out=sh({"input": 'N: Name="Mouse"\nB: PROP=0\n'}), dmesg=mapped)[0], "NEEDS-HUMAN")
        self.assertEqual(verdict(R1.boot06, disp, shell_out=sh({"input": mt}), dmesg=mapped)[0], "PASS")
        self.assertEqual(verdict(R1.boot06, disp, shell_out=sh({"input": mt}), dmesg=unmapped)[0], "FAIL")
        both = 'N: Name="Q"\nB: PROP=2\nB: KEY=400 0 0 0 0 0\nB: ABS=%x\n' % ((1 << 0) | (1 << 0x35))
        self.assertEqual(verdict(R1.boot06, disp, shell_out=sh({"input": both}), dmesg=mapped)[0], "NEEDS-HUMAN")

    DISPLAY = "The screen in use, connected displays and brightness."

    def test_kbd03(self):
        pg = Fake(state(page="Display", texts=[self.DISPLAY]))
        self.assertEqual(verdict(R1.kbd03, pg, shell_out=sh({"backlight": "BL intel_backlight 100 400\n"}))[0], "PASS")
        self.assertEqual(verdict(R1.kbd03, pg, shell_out=sh({"backlight": "BL intel_backlight 500 400\n"}))[0], "FAIL")
        self.assertEqual(verdict(R1.kbd03, pg, shell_out=sh({"backlight": ""}))[0], "NEEDS-HUMAN")


class Hardware(unittest.TestCase):
    def test_batt05(self):
        allv = "".join("CONFIG_%s=y\n" % o for o in ("ACPI_BATTERY", "ACPI_AC", "POWER_SUPPLY", "ACPI_SBS", "PM", "ACPI_BUTTON", "ACPI_VIDEO"))
        self.assertEqual(verdict(R2.batt05, Fake(), shell_out=sh({"config.gz": allv}))[0], "PASS")
        self.assertEqual(verdict(R2.batt05, Fake(), shell_out=sh({"config.gz": allv.replace("CONFIG_ACPI_AC=y", "# CONFIG_ACPI_AC is not set")}))[0], "FAIL")
        self.assertEqual(verdict(R2.batt05, Fake(), shell_out=sh({"config.gz": ""}))[0], "NEEDS-HUMAN")

    def test_drv27(self):
        pg = lambda extra: Fake(state(page="Sound", texts=["Where sound plays (HDMI/DP, USB, speakers), a test chime, volume and mute."] + extra))
        for_mbp = lambda codec, mods: sh({"codec": codec, "modules": mods})
        cod = "Codec: Cirrus Logic CS8409\n"
        self.assertEqual(verdict(R2.drv27, pg([]), info=lambda c: INFO_MINI, shell_out=sh({}))[0], "NEEDS-HUMAN")
        self.assertEqual(verdict(R2.drv27, pg([]), info=lambda c: INFO_MBP, shell_out=for_mbp(cod, "snd_hda_codec_cs8409 1 0 - Live"))[0], "PASS")
        self.assertEqual(verdict(R2.drv27, pg([]), info=lambda c: INFO_MBP, shell_out=for_mbp(cod, ""))[0], "FAIL")
        self.assertEqual(verdict(R2.drv27, pg(["This Mac's speakers need the CS8409 driver (not loaded)"]), info=lambda c: INFO_MBP, shell_out=for_mbp(cod, "snd_hda_codec_cs8409 1 0 - Live"))[0], "FAIL")
        self.assertEqual(verdict(R2.drv27, pg([]), info=lambda c: INFO_MBP, shell_out=for_mbp("Codec: Realtek\n", ""))[0], "NEEDS-HUMAN")

    def test_snd05(self):
        dm = lambda c, w, n=400: "aaudio: probe ok" if w == "aaudio" else "apple-bce 0000:02:00.1: ready"
        cards = " 0 [T2            ]: AppleT2 - Apple T2 Audio\n"
        self.assertEqual(verdict(R2.snd05, Fake(), info=lambda c: INFO_MINI, dmesg=dm, shell_out=sh({"cards": cards}))[0], "PASS")
        self.assertEqual(verdict(R2.snd05, Fake(), info=lambda c: INFO_MINI, dmesg=dm, shell_out=sh({"cards": ""}))[0], "FAIL")
        self.assertEqual(verdict(R2.snd05, Fake(), info=lambda c: INFO_MINI, dmesg=lambda c, w, n=400: "", shell_out=sh({"cards": cards}))[0], "FAIL")
        self.assertEqual(verdict(R2.snd05, Fake(), info=lambda c: INFO_MBP, dmesg=dm, shell_out=sh({"cards": cards}))[0], "NEEDS-HUMAN")

    def test_media07(self):
        good = "== Music\nIYAKKI Sample Melody.mp3\nIYAKKI Sample Beats.mp3\n== Videos\nIYAKKI Sample Video.webm\n"
        self.assertEqual(verdict(R2.media07, Fake(), shell_out=sh({"samples": good}))[0], "PASS")
        self.assertEqual(verdict(R2.media07, Fake(), shell_out=sh({"samples": good.replace("IYAKKI Sample Beats.mp3\n", "")}))[0], "FAIL")
        self.assertEqual(verdict(R2.media07, Fake(), shell_out=sh({"samples": good + "EMPTY /usr/share/iyakki/samples/Music/x\n"}))[0], "FAIL")
        self.assertEqual(verdict(R2.media07, Fake(), shell_out=sh({"samples": ""}))[0], "FAIL")

    def test_ldr09(self):
        v = lambda out: verdict(R2.ldr09, Fake(), shell_out=sh({"bootlog-probe": out}))[0]
        self.assertEqual(v("FOUND /boot/EFI/iyakki-boot/last-boot.log 80\n"), "PASS")
        self.assertEqual(v("FOUND /boot/EFI/iyakki-boot/last-boot.log 3\n"), "FAIL")
        self.assertEqual(v("ESP /boot MISSING\n"), "FAIL")
        self.assertEqual(v(""), "NEEDS-HUMAN")
        self.assertEqual(v("FOUND /.stage1/boot/EFI/iyakki-boot/last-boot.log 80\n"), "PASS")
        # the probe script itself looks at the stage-1 mount first
        seen = []
        verdict(R2.ldr09, Fake(), shell_out=lambda c, s, n: seen.append(s) or "")
        self.assertIn("/.stage1/boot /boot", seen[0])

    def test_rel01(self):
        about = state(app="Home", texts=["About IYAKKI", "Version 0.1.0-alpha.3"], buttons=("close",))
        self.assertEqual(verdict(R2.rel01, Fake(tile_state=about), info=lambda c: INFO_MINI)[0], "PASS")
        stale = state(app="Home", texts=["About IYAKKI", "Version 0.1.0-alpha.2"], buttons=("close",))
        self.assertEqual(verdict(R2.rel01, Fake(tile_state=stale), info=lambda c: INFO_MINI)[0], "FAIL")
        self.assertEqual(verdict(R2.rel01, Fake(tile_state=about), info=lambda c: "device  x y\n")[0], "NEEDS-HUMAN")
        alpha2 = state(app="Home", texts=["About IYAKKI", "Version 0.1.0-alpha.2"], buttons=("close",))
        self.assertEqual(verdict(R2.rel01, Fake(tile_state=alpha2), info=lambda c: INFO_LENOVO)[0], "PASS")

    def test_dev10(self):
        def go(out, rc=0):
            c = Fake(); c.canned = {"usb": (rc, out)}
            return verdict(R2.dev10, c)[0]
        self.assertEqual(go("\n".join("%d. step %d ok" % (i, i) for i in range(1, 8))), "PASS")
        self.assertEqual(go(USB_REAL), "PASS")
        self.assertEqual(go("1. a\n2. b\n"), "NEEDS-HUMAN")
        self.assertEqual(go("", 1), "NEEDS-HUMAN")

    def test_domain02(self):
        dom = Fake(state(page="Domains", texts=["Give services a name such as store.local: a route table, the listener, the firewall opening and the certificates in your vault."]))
        ln = lambda s: sh({"listening": s})
        self.assertEqual(verdict(R2.domain02, dom, shell_out=ln("tcp 0 0 0.0.0.0:7979 0.0.0.0:* LISTEN\n"))[0], "PASS")
        self.assertEqual(verdict(R2.domain02, dom, shell_out=ln("tcp 0 0 127.0.0.1:80 0.0.0.0:* LISTEN\n"))[0], "PASS")
        self.assertEqual(verdict(R2.domain02, dom, shell_out=ln("tcp 0 0 0.0.0.0:443 0.0.0.0:* LISTEN\n"))[0], "NEEDS-HUMAN")
        self.assertEqual(verdict(R2.domain02, dom, shell_out=ln(""))[0], "NEEDS-HUMAN")

    def test_cloud01(self):
        pg = Fake(state(page="Cloud", texts=["For servers, cloud machines and remote-desktop-only computers: hide what needs a local screen, speakers or camera."]))
        self.assertEqual(verdict(R2.cloud01, pg, shell_out=sh({"cmdline": "BOOT_IMAGE=/vmlinuz quiet"}))[0], "PASS")
        self.assertEqual(verdict(R2.cloud01, pg, shell_out=sh({"cmdline": "quiet iyakki.mode=cloud"}))[0], "NEEDS-HUMAN")

    def test_client08(self):
        blurb = "Open this desktop in a browser on another computer on your network. Off by default; encrypted; signs in with this machine's accounts."
        self.assertEqual(verdict(R2.client08, Fake(state(page="Remote access", texts=[blurb, "Client connections: off, the endpoint answers nothing"])))[0], "PASS")
        self.assertEqual(verdict(R2.client08, Fake(state(page="Remote access", texts=[blurb])))[0], "FAIL")

    def test_k8s18(self):
        blurb = "The clusters the AI IDE can deploy to, this machine's own k3s, and where images go."
        self.assertEqual(verdict(R2.k8s18, Fake(state(page="Clusters", texts=[blurb], buttons=("Platform services", "Test"))))[0], "PASS")
        self.assertEqual(verdict(R2.k8s18, Fake(state(page="Clusters", texts=[blurb], buttons=("Test",))))[0], "FAIL")

    def test_vpn07(self):
        blurb = "Import a WireGuard or OpenVPN profile you already have, and choose whether all traffic or only what your filters match goes through it."
        pg = Fake(state(page="VPN", texts=[blurb]))
        self.assertEqual(verdict(R2.vpn07, pg, info=lambda c: INFO_MINI)[0], "PASS")
        self.assertEqual(verdict(R2.vpn07, pg, info=lambda c: INFO_TUNNEL)[0], "NEEDS-HUMAN")

    def test_cam06(self):
        node = "V video0 Fullhan Camera\n"
        greyed = dict(home_texts=["Connect a camera"]); live = dict(home_texts=["Photos, video"])
        self.assertEqual(verdict(R2.cam06, Fake(**live), shell_out=sh({"video": node}))[0], "PASS")
        self.assertEqual(verdict(R2.cam06, Fake(**greyed), shell_out=sh({"video": ""}))[0], "PASS")
        self.assertEqual(verdict(R2.cam06, Fake(**live), shell_out=sh({"video": ""}))[0], "FAIL")
        self.assertEqual(verdict(R2.cam06, Fake(**greyed), shell_out=sh({"video": node}))[0], "NEEDS-HUMAN")

    def test_tiles(self):
        voice = state(app="Voice", buttons=("READ ALOUD", "DICTATE", "RECORD", "ASSISTANT"))
        self.assertEqual(verdict(R2.voice17, Fake(tile_state=voice, home_buttons=("VOICE",)))[0], "PASS")
        self.assertEqual(verdict(R2.voice17, Fake(tile_state=state(app="Voice", buttons=("READ ALOUD",)), home_buttons=("VOICE",)))[0], "FAIL")
        self.assertEqual(verdict(R2.voice17, Fake(tile_state=voice, home_buttons=("CAMERA",)))[0], "NEEDS-HUMAN")
        qr = state(app="Dev tools", texts=["Make a QR code from text, a web address, a Wi-Fi network"], buttons=("QR code",))
        self.assertEqual(verdict(R2.devt05, Fake(tile_state=qr))[0], "PASS")
        self.assertEqual(verdict(R2.devt05, Fake(tile_state=state(app="Dev tools", buttons=("JSON",))))[0], "FAIL")
        real = ["Connections", "SSH CLIENT", "Dropbear 2026.91 (/usr/bin/ssh)", "ABOUT",
                "Saved hosts for ssh. Host keys are kept by IYAKKI and checked on every connection; private keys stay in your vault. Press N to add a host.",
                "No saved connections yet. Press N to add one."]
        btns = ("Terminal", "CLIs", "Connections", "Connect (c)", "New (n)", "Edit (e)", "Remove (x)", "Trust key (t)", "Forget key (f)", "Make key (g)")
        con = state(app="CLI", texts=real, buttons=btns)
        self.assertEqual(verdict(R2.cli07, Fake(tile_state=con))[0], "PASS")
        self.assertEqual(verdict(R2.cli07, Fake(tile_state=state(app="CLI", texts=real, buttons=btns[:-1])))[0], "FAIL")      # button missing
        self.assertEqual(verdict(R2.cli07, Fake(tile_state=state(app="CLI", texts=real[:-1], buttons=btns)))[0], "NEEDS-HUMAN")  # hosts saved
        self.assertEqual(verdict(R2.cli07, Fake(tile_state=state(app="CLI", texts=real[1:], buttons=btns)))[0], "FAIL")        # no heading
        self.assertEqual(verdict(R2.cli07, Fake(tile_state=state(app="CLI", texts=["$ _"])))[0], "FAIL")

    def test_page_only_rows(self):
        cases = [(R2.domain06, "Give services a name such as store.local: a route table, the listener, the firewall opening and the certificates in your vault.", ("Add route",)),
                 (R2.cloud20, "Who is in the cloud tenant and its namespaces, with what role: add and remove people and namespaces, and read the audit trail.", ()),
                 (R2.share01, "Share a folder of yours with other devices on this network: browse and download, or serve a small website. Nothing is shared until you make a share and start it", ("New share\u2026",)),
                 (R2.voice19, "Short quiet sounds for signing in and out, new notifications and finished jobs: switches, level and a preview of each.", ())]
        for f, blurb, btns in cases:
            self.assertEqual(verdict(f, Fake(state(page="X", texts=[blurb, "Shared folders"], buttons=btns)), shell_out=sh({}))[0], "PASS", f.__name__)
            self.assertEqual(verdict(f, Fake(state(page="Other", texts=["x"], buttons=btns)), shell_out=sh({}))[0], "FAIL", f.__name__)
        self.assertEqual(verdict(R2.domain06, Fake(state(page="Domains", texts=["Give services a name such as store.local"])))[0], "FAIL")  # no buttons


class Registry(unittest.TestCase):
    def test_all_registered_with_suffix_and_notes(self):
        hwcheck.from_recipes()
        mine = {k: v for k, v in RECIPES.items() if k.endswith("-m14")}
        self.assertGreaterEqual(len(mine), 30)
        for k, (title, f) in mine.items():
            self.assertRegex(k, r"^F-[A-Z0-9]+-\d+-m14$")
            self.assertTrue(f.__doc__ and "Row F-" in f.__doc__, k)
            self.assertIn(k.rsplit("-m14", 1)[0], title.join([" ", " "]) + f.__doc__, k)

    def test_ids_do_not_collide(self):
        import glob
        seen = {}
        for fn in glob.glob(os.path.join(os.path.dirname(os.path.abspath(__file__)), "recipes*.py")):
            for rid in re.findall(r'@recipe\("([^"]+)"', _read(fn)):
                self.assertNotIn(rid, seen, "%s in %s and %s" % (rid, fn, seen.get(rid)))
                seen[rid] = fn

    def test_every_recipe_says_what_it_does_not_cover(self):
        for fn in ("recipes_m14.py", "recipes_m14_hw.py"):
            with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), fn)) as f:
                src = f.read()
            for body in re.split(r"\n@recipe\(", src)[1:]:
                self.assertIn("c.notcovered(", body, body[:40])


if __name__ == "__main__":
    unittest.main()
