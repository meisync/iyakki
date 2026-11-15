"""M14 hardware-check recipes, part 2: kernel options, files on the image, services and tiles (all read-only).
Helpers and rules are those of recipes_m14.py; pure logic is in recipes_m14_parse.py and tested in test_m14.py."""
import re
from hwcheck import recipe, fail, human, dev, Ctx
import recipes as _r1  # gives Ctx.notcovered
from recipes_m14 import tab, need_buttons, find_tile, info, shell_out, dmesg
import recipes_m14_parse as P


# ------------------------------------------------------------------ kernel and files

@recipe("F-BATT-05-m14", "The running kernel has the battery options (ACPI_BATTERY, ACPI_AC, POWER_SUPPLY, ACPI_SBS, PM, ACPI_BUTTON, ACPI_VIDEO) (read-only)")
def batt05(c):
    """Row F-BATT-05. Reads /proc/config.gz through `dev shell`. PASS when all seven options are y or m. FAIL naming each option that
    is n or absent. NEEDS-HUMAN when the kernel does not expose its configuration. On a laptop the BAT0 supply is also listed."""
    pat = "|".join(o[len("CONFIG_"):] for o in P.BATTERY_OPTIONS)
    out = shell_out(c, "zcat /proc/config.gz 2>/dev/null | grep -E '^(# )?CONFIG_(%s)[= ]'" % pat, "kconfig-battery")
    cfg = P.kconfig(out)
    if not cfg:
        # Not a verdict: sysfs shows the battery and AC drivers worked, but it cannot prove ACPI_SBS, ACPI_VIDEO or ACPI_BUTTON
        # (no node says which options were built), so the row stays with a person; the facts go into the evidence.
        facts = shell_out(c, 'for d in /sys/class/power_supply/*; do echo "PS ${d##*/} $(cat $d/type 2>/dev/null)"; done; '
                             '[ -e /sys/power/state ] && echo "PM /sys/power/state $(cat /sys/power/state)"; '
                             'ls /sys/firmware/acpi 2>/dev/null | sed "s/^/ACPI /"; true', "acpi-facts")
        c.log("sysfs facts (not enough to prove the kernel options): " + facts.replace("\n", " | ")[:300])
        human("/proc/config.gz is not readable here (kernel without IKCONFIG_PROC, or zcat missing); sysfs lists the power supplies "
              "but cannot show whether ACPI_SBS, ACPI_VIDEO and ACPI_BUTTON are built, so the row is not decided by a script")
    miss = P.options_missing(cfg, P.BATTERY_OPTIONS)
    c.log("options: " + ", ".join("%s=%s" % (k, cfg.get(k, "absent")) for k in P.BATTERY_OPTIONS))
    if miss:
        fail("kernel options not enabled: %s" % ", ".join("%s (%s)" % m for m in miss))
    c.log("power supplies: " + shell_out(c, "ls /sys/class/power_supply 2>/dev/null", "power-supply").replace("\n", " "))
    c.notcovered("BAT0 appearing on a laptop whose battery driver was missing before (read the power-supply list above on that laptop); the arm64 options")


@recipe("F-DRIVERS-27-m14", "Macs with the Cirrus CS8409 codec: codec bound to the CS8409 driver and the Sound page not asking for it (read-only)")
def drv27(c):
    """Row F-DRIVERS-27. NEEDS-HUMAN unless the model is MacBookPro13,x / 14,x or iMac18,x / 19,x. There: PASS when an ALSA codec file
    names the CS8409 codec, snd_hda_codec_cs8409 is in /proc/modules and the Sound page does NOT say 'need the CS8409 driver (not
    loaded)'. FAIL when the codec is there but the module is not loaded or the page asks for the driver. Never plays a sound; that the
    speakers are audible is NOT covered."""
    model = P.model_of(info(c)) or ""
    if not re.fullmatch(r"MacBookPro1[34],\d|iMac(18|19),\d", model):
        human("this row is about CS8409 Macs without a T2 chip (MacBookPro13/14, iMac 2017-2019); this machine reports %r" % model)
    codec = P.cs8409_codec(shell_out(c, "cat /proc/asound/card*/codec#* 2>/dev/null | head -400", "codec-dump"))
    mods = shell_out(c, "grep -i cs8409 /proc/modules", "modules-cs8409")
    c.log("CS8409 codec seen: %s; module loaded: %s" % (codec, bool(mods.strip())))
    if not codec:
        human("no CS8409 codec on this %s" % model)
    if "snd_hda_codec_cs8409" not in mods.replace("-", "_"):
        fail("CS8409 codec present but snd_hda_codec_cs8409 is not loaded")
    st = tab(c, "sound", "Sound", "Where sound plays (HDMI/DP, USB, speakers), a test chime, volume and mute.")
    c.expect_absent(st, r"need the CS8409 driver \(not loaded\)")
    c.notcovered("any sound from the speakers (the row says nothing is known about sound on this kernel; a person must listen)")


@recipe("F-SND-05-m14", "T2 Mac sound: Apple T2 Audio card exists and aaudio logged (read-only)")
def snd05(c):
    """Row F-SND-05. NEEDS-HUMAN unless the model is a T2 Mac. There: PASS when /proc/asound/cards lists 'Apple T2 Audio' and the
    kernel log has aaudio / apple-bce lines. FAIL when the model is a T2 Mac and either is missing. Nothing is played; whether radio
    and TALES play (the dmix underrun the row describes) is NOT covered."""
    model = P.model_of(info(c))
    if not P.is_t2(model):
        human("not a T2 Mac (%r): the aaudio card does not exist here" % model)
    cards = shell_out(c, "cat /proc/asound/cards 2>/dev/null", "asound-cards")
    lines = P.t2_audio_lines(dmesg(c, "aaudio") + "\n" + dmesg(c, "bce"))
    c.log("cards: %s; aaudio/bce log lines: %d" % (cards.strip().replace("\n", " / ")[:200], len(lines)))
    if "Apple T2 Audio" not in cards:
        fail("T2 Mac %s has no 'Apple T2 Audio' ALSA card" % model)
    if not lines:
        fail("no aaudio / apple-bce line in the kernel log")
    c.notcovered("playback through `iyakki` / plughw; radio and TALES reading; the microphone")


@recipe("F-MEDIA-07-m14", "Sample media is on the image: a melody, a beat (mp3) and a sample WebM video (read-only)")
def media07(c):
    """Row F-MEDIA-07. PASS when /usr/share/iyakki/samples/Music holds 'IYAKKI Sample Melody.mp3' and 'IYAKKI Sample Beats.mp3' and
    Videos holds 'IYAKKI Sample Video.webm', each non-empty. FAIL when one is missing or empty. Nothing is played or decoded."""
    out = shell_out(c, 'for d in Music Videos; do echo "== $d"; for f in /usr/share/iyakki/samples/$d/*; do [ -f "$f" ] && echo "${f##*/}"; done; done; '
                       'for f in /usr/share/iyakki/samples/*/*; do [ -s "$f" ] || echo "EMPTY $f"; done; true', "samples")
    m = P.sample_media(out)
    c.log("samples: %s" % m)
    need = {"Music": ("IYAKKI Sample Melody.mp3", "IYAKKI Sample Beats.mp3"), "Videos": ("IYAKKI Sample Video.webm",)}
    for d, names in need.items():
        for n in names:
            if n not in m.get(d, []):
                fail("sample %s/%s is missing from the image" % (d, n))
    if re.search(r"^EMPTY ", out, re.M):
        fail("an empty sample file: " + re.search(r"^EMPTY .*$", out, re.M).group(0))
    c.notcovered("that they play, the 12 s length, the licence text next to them, and how the Media tile lists them (a person opens MEDIA)")


@recipe("F-LDR-09-m14", "Boot log on the boot volume: EFI/iyakki-boot/last-boot.log exists and has lines (read-only; NEEDS-HUMAN if the boot volume is not mounted)")
def ldr09(c):
    """Row F-LDR-09. Looks for the boot volume at /.stage1/boot (where a booted stick or disk
    mounts it, read-only), /boot, /boot/efi, /efi, /mnt/boot, /mnt/esp. PASS when last-boot.log (or the root
    /last-boot.log) exists with at least 10 lines. FAIL when the volume is mounted but has no log, or the log is shorter. NEEDS-HUMAN
    when no boot volume is mounted (the file is then not reachable by `dev shell`). The file is only counted, never copied out."""
    out = shell_out(c, 'for m in /.stage1/boot /boot /boot/efi /efi /mnt/boot /mnt/esp; do if [ -d "$m/EFI" ]; then '
                       'f="$m/EFI/iyakki-boot/last-boot.log"; [ -f "$f" ] || f="$m/last-boot.log"; '
                       'if [ -f "$f" ]; then echo "FOUND $f $(wc -l < "$f")"; else echo "ESP $m MISSING"; fi; break; fi; done; true', "bootlog-probe")
    kind, where, n = P.bootlog_state(out)
    c.log("%s %s %d" % (kind, where, n))
    if kind == "no-esp":
        human("no boot volume is mounted on this device, so the loader's log cannot be read from here")
    if kind == "missing":
        fail("boot volume %s is mounted but has no last-boot.log" % where)
    if n < 10:
        fail("%s has only %d lines" % (where, n))
    c.notcovered("the four write points, 'log: about to start' ending, a read-only medium, what the log says on the Apple firmware (read it by hand)")


@recipe("F-REL-01-m14", "Version everywhere: `dev info` and the About page of the IYAKKI menu name the same version (read-only)")
def rel01(c):
    """Row F-REL-01. PASS when the version in `dev info` (image line) is an `0.1.0-alpha.N` style number and the About page reached from
    the IYAKKI menu shows the same string. FAIL when they differ. NEEDS-HUMAN when the About entry cannot be found by label."""
    v = P.image_version(info(c))
    if not v:
        human("`dev info` has no recognisable 'image  IYAKKI <version>' line")
    c.log("dev info version: %s" % v)
    c.home()
    st = c.click("IYAKKI menu")
    about = [b for b in c.buttons(st) if re.search(r"(?i)\babout\b", b)]
    if not about:
        human("no About entry among the IYAKKI menu buttons: %s" % c.buttons(st)[:12])
    st = c.click(about[0], wait=2)
    c.shot("about")
    if not [t for t in c.texts(st) if v in t]:
        fail("the About page does not show version %s (texts: %s)" % (v, c.texts(st)[:8]))
    c.log("About page shows %s" % v)
    c.notcovered("crate versions, the ISO / image file names and the site, the Rust version string of `iyakki --version`")


@recipe("F-DEV-10-m14", "`iyakki dev usb` prints its numbered check list (read-only; no authorize, pair or gadget change)")
def dev10(c):
    """Row F-DEV-10. Runs only `iyakki dev usb` (the check, which changes nothing). PASS when the output has the seven numbered
    steps; NEEDS-HUMAN when the numbering is not recognised or the command is refused for lack of the scope. authorize, pair, link and
    gadget are never run."""
    rc, out = c.run("usb", name="usb-check")
    if rc or not out.strip():
        human("`dev usb` returned %d with no output (scope missing?)" % rc)
    steps = P.usb_steps(out)
    c.log("steps seen: %s; status per step: %s" % (steps, P.usb_status(out)))
    if steps == [1, 2, 3, 4, 5, 6, 7]:
        c.notcovered("whether each step's verdict is right for the cable plugged in, authorize, pair, speed, link, gadget (they change the link)")
        return
    human("the check list was not recognised as seven numbered steps (saw %s); read usb-check.txt" % steps)


# ------------------------------------------------------------------ services

@recipe("F-DOMAIN-02-m14", "Domain listener: nothing listens on ports 80 or 443 on a non-loopback address unless approved (read-only)")
def domain02(c):
    """Row F-DOMAIN-02. Reads the listening TCP sockets through `dev shell`. PASS when nothing listens on 80 or 443 at all. When
    something does on a non-loopback address the row's 'only after approval' cannot be judged by a script: NEEDS-HUMAN with the
    sockets in the evidence. The Domains page must open (blurb). Nothing is switched."""
    out = shell_out(c, "netstat -ltn 2>/dev/null | grep LISTEN", "listening")
    socks = P.listeners(out)
    if not socks:
        human("no listening sockets read (netstat unavailable?)")
    c.log("listening: " + ", ".join("%s:%d" % s for s in socks))
    tab(c, "domains", "Domains", "Give services a name such as store.local: a route table, the listener, the firewall opening and the certificates in your vault.")
    bad = P.non_loopback(socks, (80, 443))
    if bad:
        human("port 80/443 listens on %s: check the approval in the audit log" % bad)
    c.notcovered("the listener switched on, 8080 / 8443 on loopback, the approval for 80 and 443, certificates")


@recipe("F-DOMAIN-06-m14", "Settings -> Network -> Domains opens with its blurb and its route-table buttons (read-only)")
def domain06(c):
    """Row F-DOMAIN-06. PASS when the Domains page opens (blurb 'Give services a name such as store.local ...') and has at least one
    button; the button names are logged. Nothing is added, edited or removed."""
    st = tab(c, "domains", "Domains", "Give services a name such as store.local: a route table, the listener, the firewall opening and the certificates in your vault.")
    b = [x for x in c.buttons(st) if x and x not in ("field", "clipboard", "home", "close", "back")]
    c.log("buttons: " + ", ".join(b))
    if not b:
        fail("the Domains page has no buttons at all")
    c.notcovered("add / edit / remove a route (each asks), the listener and certificate sections, the audit lines")


@recipe("F-CLOUD-01-m14", "Cloud mode is off on a normal machine: no iyakki.mode=cloud on the kernel line and the Cloud page opens (read-only)")
def cloud01(c):
    """Row F-CLOUD-01. PASS when /proc/cmdline has no `iyakki.mode=cloud` and the Settings -> Cloud page opens (blurb 'For servers,
    cloud machines ...'). NEEDS-HUMAN when the kernel line forces cloud mode (then it is not the bare-metal default under test). The
    Automatic / On / Off choice is not changed."""
    cmd = shell_out(c, "cat /proc/cmdline", "cmdline")
    if re.search(r"\biyakki\.mode=cloud\b", cmd):
        human("the kernel line carries iyakki.mode=cloud: this device runs in forced cloud mode")
    c.log("cmdline has no iyakki.mode=cloud")
    tab(c, "cloud mode", "Cloud", "For servers, cloud machines and remote-desktop-only computers: hide what needs a local screen, speakers or camera.")
    c.notcovered("the three choices and what each hides (switching changes the home screen); the boot parameter on a server image")


@recipe("F-CLOUD-20-m14", "Settings -> Cloud access opens for an administrator (read-only)")
def cloud20(c):
    """Row F-CLOUD-20. PASS when the Cloud access page opens (blurb 'Who is in the cloud tenant and its namespaces ...'). FAIL when
    the search finds another page. Nobody is added or removed; the audit trail is not read."""
    tab(c, "cloud access", "Cloud access", "Who is in the cloud tenant and its namespaces, with what role: add and remove people and namespaces, and read the audit trail.")
    c.notcovered("adding or removing people / namespaces (each asks), key rotation, the audit trail")


@recipe("F-CLIENT-08-m14", "Settings -> Remote access shows the Connect-a-client state (EXPERIMENTAL; off by default; read-only)")
def client08(c):
    """Row F-CLIENT-08. PASS when the Remote access page opens and has a line 'Client connections: on|off ...'. The state is logged;
    the device permission switches are only reachable when it is on, so nothing is switched on to look."""
    st = tab(c, "remote access", "Remote access", "Open this desktop in a browser on another computer on your network. Off by default; encrypted; signs in with this machine's accounts.")
    m = c.expect(st, r"^Client connections: (on|off)\b")
    c.log("client connections: " + m[0])
    c.notcovered("the five device switches (audio.out, clipboard.client, camera.view, ...) and the chat tools: they need Connect a client on, which this recipe never does")


@recipe("F-K8S-18-m14", "Settings -> Clusters has the Platform services button (read-only; not pressed)")
def k8s18(c):
    """Row F-K8S-18. PASS when the Clusters page opens (blurb 'The clusters the AI IDE can deploy to ...') and its buttons include
    'Platform services'. Nothing is pressed (opening it only lists; deploying asks)."""
    st = tab(c, "clusters", "Clusters", "The clusters the AI IDE can deploy to, this machine's own k3s, and where images go.")
    need_buttons(c, st, ["Platform services"])
    c.notcovered("the list with live state, Deploy / Remove (they ask), the assistant tools")


@recipe("F-VPN-07-m14", "Settings -> VPN opens with its import choices and no tunnel interface is up by default (read-only)")
def vpn07(c):
    """Row F-VPN-07. PASS when the VPN page opens (blurb 'Import a WireGuard or OpenVPN profile ...') and `dev info` shows no wg*
    or tun* network device while the page lists no connected profile. Nothing is imported or connected; no secret is shown."""
    inf = info(c)
    tunnels = re.findall(r"^network\s+((?:wg|tun|tap)\d+)\b", inf, re.M)
    st = tab(c, "vpn", "VPN", "Import a WireGuard or OpenVPN profile you already have, and choose whether all traffic or only what your filters match goes through it.")
    if tunnels and not c.has(st, r"(?i)connected|on\b"):
        human("tunnel device(s) %s exist but the page shows no connected profile" % tunnels)
    c.log("tunnel devices: %s" % (tunnels or "none"))
    c.notcovered("FULL / split modes, the kill switch, a real handshake (needs a profile; secrets are not touched here)")


@recipe("F-SHARE-01-m14", "Settings -> Shared folders opens; the listening sockets are logged as evidence (read-only)")
def share01(c):
    """Row F-SHARE-01. PASS when the Shared folders page opens (its own first line 'Share a folder of yours with other devices on this network ...',
    not the search-result blurb) with its title and the 'New share...' button. Nothing is
    started or shared. The listening sockets are logged for the evidence."""
    st = tab(c, "shared folder", "Shared folders", "Share a folder of yours with other devices on this network: browse and download, or serve a small website.")
    c.expect(st, r"^Shared folders$", "the page title")
    need_buttons(c, st, ["New share\u2026"])
    c.log("listening: " + ", ".join("%s:%d" % s for s in P.listeners(shell_out(c, "netstat -ltn 2>/dev/null | grep LISTEN", "listening"))))
    c.notcovered("starting a share, one port per share, uploads, the token and password, the QR link")


@recipe("F-VOICE-19-m14", "Settings -> System sounds opens (read-only; no cue is played)")
def voice19(c):
    """Row F-VOICE-19. PASS when the System sounds page opens (blurb 'Short quiet sounds for signing in and out ...'). No preview is
    played; whether the notes are pleasant and quiet needs a person."""
    st = tab(c, "system sounds", "System sounds", "Short quiet sounds for signing in and out, new notifications and finished jobs: switches, level and a preview of each.")
    c.log("buttons: " + ", ".join(b for b in c.buttons(st) if b not in ("field", "clipboard")))
    c.notcovered("hearing the two rising / falling notes and the cues, the level, the switches saved across a restart")


# ------------------------------------------------------------------ tiles

@recipe("F-CAM-06-m14", "CAMERA tile is enabled exactly when the kernel has a video device (reads the tile's caption; does not open the camera)")
def cam06(c):
    """Row F-CAM-06. Lists /sys/class/video4linux through `dev shell`, then reads the Home screen's CAMERA tile caption without opening
    it (the camera light must not come on). PASS: video nodes and no 'Connect a camera' caption, or no nodes and the caption present.
    NEEDS-HUMAN: nodes but the tile is greyed (they may be metadata nodes only). The tile is not pressed."""
    out = shell_out(c, 'for d in /sys/class/video4linux/video*; do [ -e "$d/name" ] && echo "V ${d##*/} $(cat $d/name)"; done; true', "video-nodes")
    nodes = P.video_nodes(out)
    c.log("video nodes: %s" % (nodes or "none"))
    st = find_tile(c, "CAMERA")
    greyed = bool(c.has(st, r"Connect a camera"))
    c.log("tile greyed ('Connect a camera'): %s" % greyed)
    if nodes and greyed:
        human("video nodes %s exist but the CAMERA tile is greyed: a metadata-only node does not count; check by hand" % [n for n, _ in nodes])
    if not nodes and not greyed:
        fail("no video device in sysfs, yet the CAMERA tile is not greyed")
    c.notcovered("QUERYCAP on the nodes, plugging a camera in and the two-second rescan, the camera light, opening the tile")


@recipe("F-VOICE-17-m14", "VOICE tile opens with its READ ALOUD, DICTATE and RECORD tabs; nothing is spoken or recorded (read-only)")
def voice17(c):
    """Row F-VOICE-17. PASS when the VOICE tile opens and its buttons include the tabs READ ALOUD, DICTATE and RECORD. Nothing is
    typed, spoken or recorded. NEEDS-HUMAN when the tile is hidden (cloud mode without a sound output, or a guest)."""
    try:
        find_tile(c, "VOICE")
    except Exception:
        human("no VOICE tile on this machine's home screen (hidden without a sound output or for guests)")
    st = c.open_tile("VOICE")
    c.shot("voice")
    need_buttons(c, st, ["READ ALOUD", "DICTATE", "RECORD"])
    c.notcovered("reading aloud, the word highlight, the engines found on the computer, recording (never started by a script)")


@recipe("F-DEVT-05-m14", "DEV TOOLS tile has the QR code tool and shows its description (read-only; no code is made)")
def devt05(c):
    """Row F-DEVT-05. PASS when DEV TOOLS opens, a 'QR code' button exists and clicking it shows the tool's description ('Make a QR
    code from text, a web address, a Wi-Fi network ...'). Nothing is typed, saved or scanned."""
    st = c.open_tile("DEV TOOLS")
    need_buttons(c, st, ["QR code"])
    st = c.click("QR code", wait=1.5)
    c.shot("qr")
    c.expect(st, r"Make a QR code from text", "the tool's description")
    c.notcovered("the eight kinds of code, a Wi-Fi code with WPA, saving, a phone scanning it (needs a phone)")


@recipe("F-CLI-07-m14", "CLI tile -> Connections tab opens (read-only; no host is added or connected)")
def cli07(c):
    """Row F-CLI-07. PASS when the CLI tile opens (administrators with a full terminal) and the Connections tab shows its heading
    'Connections', the ABOUT line 'Saved hosts for ssh ...', the empty-list line 'No saved connections yet. Press N to add one.' and the
    buttons Connect (c), New (n), Edit (e), Remove (x), Trust key (t), Forget key (f), Make key (g). NEEDS-HUMAN when hosts are already
    saved (the empty-list line is then absent and the list rows are not read). Nothing is saved, no ssh session started, no vault key used."""
    c.open_tile("CLI")
    st = c.click("Connections", wait=1.5)
    c.shot("connections")
    c.expect(st, r"^Connections$", "the tab's heading")
    c.expect(st, r"^Saved hosts for ssh\b", "the ABOUT line")
    if not c.has(st, r"^No saved connections yet\. Press N to add one\."):
        human("the empty-list line is not shown: hosts may be saved on this machine; the list rows are not read by this recipe")
    c.log("empty-list line seen")
    need_buttons(c, st, ["Connect (c)", "New (n)", "Edit (e)", "Remove (x)", "Trust key (t)", "Forget key (f)", "Make key (g)"])
    c.notcovered("saving a host, host keys, vault key names, the default command, connecting; the account's network rules")
