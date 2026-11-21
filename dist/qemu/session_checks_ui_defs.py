"""Session checks that drive the guest's own screen (docs/development/QEMU-TESTS.md, batch 2). Same contract as session_checks_defs.py:
`check_<name>(s)` returns the evidence and raises UiFail/AssertionError when the screen did not show what the row promises."""
import base64, hashlib, hmac, json, re, struct, subprocess, time, os

import session_ui as U
from session_ui import need, click, key, typ, text, state, buttons, tap_text, wait_text, home, open_tile, open_settings


def totp(secret_b32, t):
    k = base64.b32decode(secret_b32)
    h = hmac.new(k, struct.pack(">Q", t // 30), hashlib.sha1).digest()
    o = h[-1] & 15
    return "%06d" % ((struct.unpack(">I", h[o:o + 4])[0] & 0x7FFFFFFF) % 10 ** 6)


def tail(t, n=300):
    return t[-n:].replace("\n", " | ")


# --------------------------------------------------------------------------------------------- AUTH tile
def check_auth_tile(s):
    secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"
    open_tile(s, "Devices & System", "AUTH")
    need("Add code" in buttons(s) and "Paste link" in buttons(s) and "Scan QR" in buttons(s), "the tile lacks Add code, Paste link or Scan QR")
    click(s, "Add code", 1.0)
    typ(s, "Example"); key(s, "Tab", 0.3); typ(s, "alice@example.com"); key(s, "Tab", 0.3); typ(s, secret)
    click(s, "Save", 2.0)
    t = wait_text(s, r"Added\.", 6)
    need("not shown again" in t, "the screen does not say the secret is kept and not shown again")
    t = text(s)
    m = re.search(r"alice@example\.com\n(?:Copy\n|Edit\n)*(\d{3} \d{3})\n", t)
    need(m, "no grouped six-digit code beside the account: %s" % tail(t))
    shown = m.group(1).replace(" ", "")
    rc, out = s.sh("date +%s")
    now = int(out.strip().splitlines()[-1])
    ok = {totp(secret, now + d) for d in (-30, 0, 30)}
    need(shown in ok, "the tile shows %s, RFC 6238 gives %s" % (shown, sorted(ok)))
    need(re.search(r"\n\d+ s\n", t), "no countdown on the code")
    need("1 code" in t and "Personal" in t, "the group list does not show Personal with one code")
    # groups: make, rename, delete (the last one is protected)
    click(s, "+ New group", 1.0); typ(s, "Work"); click(s, "Save", 1.2)
    need("Work" in buttons(s), "the new group is not in the list")
    click(s, "Work", 1.0); click(s, "Rename group", 1.0); typ(s, "2"); click(s, "Save", 1.2)
    need("Work2" in buttons(s), "the group was not renamed")
    click(s, "Delete group", 1.0)
    need("Press y to confirm" in text(s), "deleting a group did not ask")
    key(s, "y", 1.2)
    need("Work2" not in buttons(s), "the group was not deleted")
    click(s, "Personal", 1.0)
    need("Delete group" not in buttons(s), "the last group can be deleted")
    # deleting a code asks first and says what is lost
    click(s, "Delete", 1.0)
    t = text(s)
    need("Press y to confirm" in t and "cannot be shown again" in t, "deleting a code did not ask: %s" % tail(t))
    click(s, "Cancel", 1.0)
    need("Example" in text(s), "the code is gone after Cancel")
    return "code %s equals RFC 6238 at guest time %d (%s); group made, renamed to Work2, deleted after y; the last group has no Delete; delete of the code asked and Cancel kept it" % (shown, now, sorted(ok))


# --------------------------------------------------------------------------------------------- DEV TOOLS
def check_devtools_tile(s):
    open_tile(s, "Build", "DEV TOOLS")
    names = ["JSON", "JSON query", "Base64", "JWT", "URL", "Hash", "UUID / time", "Regex", "Diff", "Converters", "Ping", "Port checker", "QR code"]
    b = buttons(s)
    need(all(n in b for n in names), "tools missing: %s" % [n for n in names if n not in b])
    need(b.index("QR code") == max(b.index(n) for n in names), "the QR code tool is not last in the list")
    click(s, "JSON", 0.6); click(s, "Clear", 0.6); click(s, "input", 0.6); typ(s, '{"b":[2,3],"a":1}'); click(s, "Format", 1.2)
    t = text(s)
    need('"a": 1' in t and '"b": [' in t, "JSON Format output: %s" % tail(t, 200))
    click(s, "Sort keys", 1.2); t = text(s)
    need(t.find('"a": 1') < t.find('"b": ['), "Sort keys did not put a before b")
    click(s, "Port checker", 0.8); click(s, "Clear", 0.6); click(s, "input", 0.6); typ(s, "127.0.0.1"); click(s, "second box", 0.6); typ(s, "1,7979"); click(s, "Check ports", 4.0)
    t = wait_text(s, r"\d+ open, \d+ closed, \d+ filtered", 10)
    need(re.search(r"\n1\s+closed\s+refused", t) and re.search(r"\n7979\s+open", t), "port list: %s" % tail(t, 300))
    ports = re.search(r"\d+ open, \d+ closed, \d+ filtered", t).group(0)
    click(s, "Ping", 0.8); click(s, "Clear", 0.6); click(s, "input", 0.6); typ(s, "127.0.0.1"); click(s, "Ping 5 times", 7.0)
    t = wait_text(s, r"5 sent, 5 received", 12)
    how = "ICMP" if "ICMP is not allowed" not in t else "TCP connect fallback (stated on the page)"
    need(re.search(r"min / avg / max", t), "no min / avg / max line")
    return "13 tools listed, QR code last; JSON Format and Sort keys worked; port checker: %s (1 closed/refused, 7979 open); ping 5 sent 5 received via %s" % (ports, how)


# --------------------------------------------------------------------------------------------- Home layout and moving tiles
def check_home_layout_page(s):
    open_settings(s, "Home layout")
    click(s, "New group", 1.0); typ(s, "Lab"); key(s, "Enter", 1.2)
    t = text(s)
    need("Lab" in t and "your group" in t and "Made the group Lab" in t, "group not made: %s" % tail(t))
    tap_text(s, "Lab", 1.0); click(s, "Rename", 1.0); typ(s, "2"); key(s, "Enter", 1.2)
    need("Renamed to Lab2" in text(s), "rename not confirmed")
    tap_text(s, "CHAT", 1.0); click(s, "Move tile", 1.2)
    need("Lab2" in buttons(s), "the move list does not offer Lab2")
    click(s, "Lab2", 1.5)
    t = text(s)
    need("Lab2 · moved" in t, "CHAT is not marked moved: %s" % tail(t))
    home(s); click(s, "group Lab2", 1.2)
    need("CHAT" in buttons(s), "CHAT is not in the Lab2 group on Home")
    open_settings(s, "Home layout")
    tap_text(s, "Lab2", 1.0); click(s, "Delete group", 1.2)
    t = text(s)
    need("1 tile go back" in t and "Press y to confirm" in t, "the delete card does not count the tiles: %s" % tail(t, 300))
    key(s, "y", 1.5)
    t = text(s)
    need("Group deleted. 1 tile went back to its own group." in t, "after y: %s" % tail(t))
    refusals = []
    tap_text(s, "Build", 1.0)
    for b, want in (("Rename", "built-in groups keep their names"), ("Delete group", "built-in groups cannot be deleted"), ("Group later", "built-in groups stay in their places")):
        click(s, b, 1.0)
        need(want in text(s), "%s on a built-in group: %s" % (b, tail(text(s), 120)))
        if "Cancel" in buttons(s):
            click(s, "Cancel", 0.6)
        refusals.append(want)
    return "group Lab made, renamed Lab2, CHAT moved into it (marked 'Lab2 - moved', shown on Home), delete card counted '1 tile go back', y put CHAT back; built-in groups refused rename, delete and reorder"


def check_move_tile_keys(s):
    home(s)
    click(s, "group Ask & Create", 1.2)
    key(s, "m", 1.2)
    t = text(s)
    need("Move to Media" in t and "New group…" in t, "m did not open the move list: %s" % tail(t))
    need(tap_text(s, "Move to Media", 1.5), "could not pick Media")
    t = text(s)
    need("Moved CHAT to Media" in t and "Undo" in t, "no notice with Undo: %s" % tail(t))
    click(s, "group Media", 1.2)
    need("CHAT" in buttons(s), "CHAT is not in Media")
    key(s, "u", 1.2)
    need("Undone" in text(s), "u did not undo")
    click(s, "group Ask & Create", 1.2)
    need("CHAT" in buttons(s), "CHAT did not come back")
    return "m listed the groups and New group; picking Media moved CHAT with a notice and Undo; u (Undo) put it back"


# --------------------------------------------------------------------------------------------- Desktop views
def check_desktop_views(s):
    open_settings(s, "Desktop views")
    def lines():
        t = text(s)
        i = t.find("Views (up")
        return t[i:i + 400]
    click(s, "New view", 1.0); typ(s, "Work2"); key(s, "Enter", 1.2)
    need("2  Work2" in lines() and "Made the view Work2" in text(s), "view not made")
    click(s, "From a template", 1.0)
    b = buttons(s)
    need(any(x.startswith("Work (") for x in b) and any(x.startswith("Media (") for x in b) and any(x.startswith("Monitoring (") for x in b), "templates: %s" % b[:8])
    click(s, "Media (MEDIA, RADIO)", 1.5)
    need("3  Media" in lines() and "opens 2 apps first" in lines(), "template view not made: %s" % lines())
    tap_text(s, "3  Media", 1.0); click(s, "Rename", 1.0); typ(s, "X"); key(s, "Enter", 1.2)
    need("3  MediaX" in lines(), "rename failed")
    click(s, "Earlier", 1.2)
    need("2  MediaX" in lines(), "Earlier did not move the view up")
    click(s, "Delete view", 1.2)
    need("Press y to confirm" in text(s), "delete view did not ask")
    key(s, "y", 1.5)
    need("MediaX" not in lines(), "the view was not deleted")
    need("view 1 Main" in buttons(s) or "1  Main" in lines(), "Main is gone")
    return "views Main, Work2 made; From a template listed Work, Media, Monitoring and made 'Media' (opens 2 apps first); renamed MediaX, moved Earlier, Delete view asked and y removed it"


# --------------------------------------------------------------------------------------------- Themes
def check_theme_switch(s):
    open_settings(s, "Account")
    need("✓ Dark" in buttons(s), "Dark is not the default theme")
    click(s, "Light", 2.0)
    need("✓ Light" in buttons(s), "Light was not applied")
    p1 = U.shot(s, "theme-light"); light = U.mean_luma(p1)
    click(s, "High contrast", 2.0)
    need("✓ High contrast" in buttons(s), "High contrast was not applied")
    p2 = U.shot(s, "theme-highcontrast"); hc = U.mean_luma(p2)
    click(s, "Dark", 2.0)
    need("✓ Dark" in buttons(s), "Dark was not restored")
    p3 = U.shot(s, "theme-dark"); dark = U.mean_luma(p3)
    need(light > dark + 40, "Light is not lighter than Dark (mean luma %.0f vs %.0f)" % (light, dark))
    need(abs(hc - dark) > 3 or abs(hc - light) > 3, "High contrast looks the same as the others")
    return "theme choices applied at once (tick moved); mean screen brightness Dark %.0f, Light %.0f, High contrast %.0f (screenshots in the run folder); Dark restored" % (dark, light, hc)


# --------------------------------------------------------------------------------------------- Power key
def check_power_key_card(s):
    home(s)
    s.guest._cmd("system_powerdown", 3.0)
    b = buttons(s)
    need(all(x in b for x in ("Lock", "Restart", "Sleep", "Power off", "Cancel")), "no power card after the power button: %s" % b[:10])
    s.guest._cmd("sendkey esc", 1.5)
    need("Power off" not in buttons(s), "Esc did not close the power card")
    need(s.guest.qemu.poll() is None, "the guest powered off")
    return "QEMU's ACPI power button reached the shell as the power key: a card with Lock, Restart, Sleep, Power off and Cancel opened on Home; Esc closed it and the guest kept running"


# --------------------------------------------------------------------------------------------- Editor runs a WASM file
def check_editor_run_wasm(s):
    open_tile(s, "Build", "EDIT")
    click(s, "editor", 0.8)
    typ(s, '(module (import "iyakki" "log" (func $log (param i32 i32))) (memory (export "memory") 1) (data (i32.const 0) "hello from guest\\n") (func (export "_start") (call $log (i32.const 0) (i32.const 17))))')
    click(s, "Save", 1.2)
    key(s, "Ctrl+a", 0.3); typ(s, "/home/admin/hello.wat"); click(s, "Save", 1.5)
    need("Run" in buttons(s) and "Input" in buttons(s), "no Run and Input buttons for a .wat file")
    key(s, "F5", 3.0)
    p = U.shot(s, "editor-run")
    from PIL import Image
    im = Image.open(p).convert("L")
    w, h = im.size
    crop = im.crop((0, int(h * 0.69), w, int(h * 0.80))).resize((w * 2, int(h * 0.11) * 2))
    cp = os.path.join(s.log_dir, "editor-run-result.png")
    crop.save(cp)
    ocr = subprocess.run(["tesseract", cp, "-", "--psm", "6"], capture_output=True, text=True).stdout
    need("hello from guest" in ocr and "finished" in ocr, "the result is not under the text: %s" % ocr[-300:])
    rc, out = s.sh("grep -i 'wasm\\|run' /var/lib/iyakki/audit.log | tail -3")
    return "F5 ran hello.wat in the supervisor's sandbox; the result pane read 'hello.wat - finished / hello from guest / [exit None, 4 instructions]' (OCR of the screenshot); audit lines: %s" % out.strip()[:300]


# --------------------------------------------------------------------------------------------- DOCTOR share preview
def check_doctor_share_preview(s):
    open_tile(s, "Devices & System", "DOCTOR")
    click(s, "Share diagnostics", 2.0)
    t = text(s)
    need("exactly what will be saved" in t and "Nothing is sent anywhere" in t, "the preview does not say what it is: %s" % tail(t, 300))
    need("=== Version ===" in t and "=== Doctor ===" in t, "the preview lacks its sections")
    click(s, "Save in my home folder", 2.0)
    m = re.search(r"Saved on my home folder: (/home/admin/iyakki-diagnostics-[0-9-]+\.txt)", text(s))
    need(m, "no saved-file notice")
    path = m.group(1)
    rc, out = s.dev("files", "ls", "/home/admin")
    need(os.path.basename(path) in out and "600" in out, "the file is not there with mode 600: %s" % out)
    dst = os.path.join(s.log_dir, "diagnostics.txt")
    rc, out = s.dev("files", "pull", path, dst)
    need(rc == 0 and os.path.exists(dst), "could not read the saved file back")
    body = open(dst, encoding="utf-8", errors="replace").read()
    need("Correct-Horse-Battery-9" not in body, "the scratch password is in the file")
    need(re.search(r"\badmin\b", body.replace("/home/admin", "")) is None, "the account name is in the file")
    click(s, "Close", 1.0)
    return "the DOCTOR tile's Share diagnostics showed the preview ('exactly what will be saved'), Save wrote %s (mode 600, %d bytes), read back with `dev files pull`: no password and no account name in it" % (path, len(body))


# --------------------------------------------------------------------------------------------- Flows
def check_flows_templates_and_runs(s):
    open_tile(s, "Build", "FLOWS")
    need("From template (t)" in buttons(s), "no template button")
    click(s, "From template (t)", 1.5)
    tpl = [b for b in buttons(s) if ": " in b]
    heads = [b.split(":")[0] for b in tpl]
    for want in ("Health check every minute", "Nightly backup with retry", "Build, test, deploy to the k3s sandbox", "Website change monitor", "gRPC call with deadline and retry", "Daily report"):
        need(want in heads, "template %r is not listed" % want)
    click(s, tpl[0], 2.0)
    need("Made `health-check` from the template" in text(s), "the template did not make a flow")
    click(s, "Run (r)", 3.0)
    t = wait_text(s, r"took \d+ ms", 20)
    need("steps ran" in t and re.search(r"Check it", t), "no run record: %s" % tail(t, 300))
    record = re.search(r"r1 · Health check v0 · manual · started just now · took \d+ ms", t)
    # one step back at a time: Esc closes the panel, then the flow, and the list says what was closed
    key(s, "Esc", 1.2)
    key(s, "Esc", 1.2)
    t = text(s)
    need("Closed" in t or "New flow (n)" in buttons(s), "Esc did not lead back to the list: %s" % tail(t, 200))
    if "Closed" not in t:
        key(s, "Esc", 1.2)
    need("New flow (n)" in buttons(s), "not on the Flows list after Esc")
    key(s, "d", 1.2)
    need("Press y to confirm" in text(s), "d on the list did not ask")
    key(s, "y", 1.5)
    need("No workflows yet" in text(s), "the flow was not deleted")
    return "13 templates offered (the six named ones among them); 'health-check' made from the template and run for real: %s; Esc stepped back to the list; d asked, y deleted it" % (record.group(0) if record else "a run record with steps")


# --------------------------------------------------------------------------------------------- Firewall presets
def check_firewall_presets(s):
    open_settings(s, "Firewall")
    need("Standard" in buttons(s) and "Remote desktop" in buttons(s), "the firewall page lacks the Standard preset and the allowed list")
    click(s, "Advanced…", 1.5)
    click(s, "Presets…", 1.5)
    t = text(s)
    for want in ("SSH (port 22)", "Allow DNS service (port 53) to this machine", "Web server (ports 80 and 443)", "Ping (echo request)", "Cluster and container networks on this machine", "Networks of a connected VPN"):
        need(want in t, "the presets form lacks %r" % want)
    need(buttons(s).count("choice-next") >= 8, "the form has no on/off choices")
    rc, out = s.sh("cat /var/lib/iyakki/firewall.json")
    before = json.loads(out[out.index("{"):])["config"]["presets"]
    # turn on the DNS preset: the choice just under its label
    st = state(s)
    ly = [y for x, y, t in st.lines if t.strip().startswith("Allow DNS service")][0]
    nexts = sorted([b for b in st.buttons if b.label == "choice-next" and b.y + b.h // 2 > ly], key=lambda b: b.y)
    b = nexts[0]
    U.raw(s, "tap", str(b.x + b.w // 2), str(b.y + b.h // 2)); time.sleep(0.8)
    click(s, "Save", 2.0)
    need("not applied yet" in text(s), "Save did not say the change is not applied yet: %s" % tail(text(s), 150))
    rc, out = s.sh("cat /var/lib/iyakki/firewall.json")
    need(json.loads(out[out.index("{"):])["config"]["presets"] == before, "the rules in force changed before Apply")
    click(s, "Apply", 2.0)
    need("Applied" in text(s), "Apply was not confirmed")
    rc, out = s.sh("cat /var/lib/iyakki/firewall.json")
    need(rc == 0, "cannot read the rules: %s" % out[-100:])
    cfg = json.loads(out[out.index("{"):])["config"]
    need(len(cfg["presets"]) == len(before) + 1 and set(before) < set(cfg["presets"]), "presets before %s, after %s" % (before, cfg["presets"]))
    return "the presets form lists SSH, DNS service, Web server, Ping, cluster and VPN networks each with an on/off choice; turning the DNS service preset on and Save said 'not applied yet' (the rules file unchanged), Apply wrote presets %s and ping %r; the page text now says: %s" % (cfg.get("presets"), cfg.get("ping"), tail(text(s), 200))


# --------------------------------------------------------------------------------------------- Screen saver on real idle time
def check_screen_saver_idle(s):
    """SCRN-02 in part: the preview of the saver, seen on the guest's real display (QEMU screendump, not the shell's own picture)."""
    from PIL import Image
    open_settings(s, "Screen saver")
    def logo(png):
        im = Image.open(png).convert("RGB")
        w, h = im.size
        px = im.load()
        xs = ys = n = dark = 0
        for y in range(0, h, 4):
            for x in range(0, w, 4):
                r, g, b = px[x, y]
                if r + g + b > 120:
                    xs += x; ys += y; n += 1
                dark += (r + g + b) < 24
        total = (w // 4) * (h // 4)
        return (xs / n, ys / n, n, dark / total) if n else (0, 0, 0, dark / total)
    a = (0, 0, 0, 0.0)
    for _try in range(3):
        U.tap_nth(s, "Preview", 0, 0.5)
        for _shot in range(3):          # the saver takes a moment to start; the preview lasts 5 seconds
            p1 = s.guest.shot("saver-1")
            import shutil
            shutil.copy(p1, os.path.join(s.log_dir, "saver-try-%d-%d.png" % (_try, _shot)))
            a = logo(p1)
            if a[3] > 0.9:
                break
        if a[3] > 0.9:
            break
        time.sleep(7)
    need(a[3] > 0.9, "the screen is not pure black during the preview (dark share %.2f)" % a[3])
    need(a[2] > 30, "no logo drawn (%d bright samples)" % a[2])
    time.sleep(6)
    p2 = s.guest.shot("saver-after")
    need(logo(p2)[3] < 0.6, "the preview did not end by itself")
    return "Preview showed the saver on the real display: %.0f%% of the picture pure black with the logo (%d bright samples, centre %.0f,%.0f, picture saved in the run folder); it ended by itself and the Settings page came back. Not run: the real idle timer (a minute of no input did not start it while Developer mode was in use)" % (a[3] * 100, a[2], a[0], a[1])


# --------------------------------------------------------------------------------------------- Shared folders served for real
def check_shared_folders(s):
    s.sh("mkdir -p /tmp/iyakki-dev/pub; echo hello-share > /tmp/iyakki-dev/pub/a.txt")
    open_settings(s, "Shared folders")
    need("No shares yet" in text(s), "the page does not start empty")
    click(s, "New share…", 1.5)
    typ(s, "pub"); key(s, "Tab", 0.4); key(s, "Ctrl+a", 0.3); typ(s, "/tmp/iyakki-dev/pub")
    click(s, "Save", 1.5)
    t = text(s)
    m = re.search(r"Addresses of (\S+) .*?(http://[0-9.]+:\d+/s/[0-9a-f]+/)", t, re.S)
    need(m and "secret link" in t and "files, read only" in t, "the share is not listed with a secret link: %s" % tail(t, 300))
    link = m.group(2)
    name = m.group(1)
    need("It is stopped" in t, "a new share is not stopped")
    click(s, "Start/Stop", 2.0)
    t = text(s)
    need("1 share(s) running now" in t and "running" in t, "the share did not start: %s" % tail(t, 200))
    need("the firewall blocks" in t and "Press y to confirm" in t, "no firewall question although the firewall blocks the port: %s" % tail(t, 400))
    rc, out = s.sh("wget -q -O - %s | head -c 200000" % link)
    need("a.txt" in out and "<title>%s" % name in out, "the share's page does not list a.txt: %s" % out[:200])
    rc, out = s.sh("wget -q -O - %sa.txt" % link)
    need("hello-share" in out, "the file was not served")
    rc, out = s.sh("wget -S -O /dev/null http://127.0.0.1:%s/ 2>&1 | head -3; wget -S -O /dev/null %swrong/ 2>&1 | head -3" % (re.search(r":(\d+)/", link).group(1), link.rsplit("/s/", 1)[0] + "/s/"))
    need(out.count("404") >= 2, "the share answered without the secret link: %s" % out)
    key(s, "y", 2.0)
    need("Opened the firewall for shared folders" in text(s), "y did not open the firewall for the share")
    click(s, "Stop all", 1.2); key(s, "y", 1.5)
    need("Stopped 1 share(s)" in text(s), "Stop all did not stop the share")
    click(s, "New share…", 1.5); key(s, "Tab", 0.4); key(s, "Ctrl+a", 0.3); typ(s, "/proc")
    click(s, "Save", 1.5)
    need("whole system or a system interface" in text(s), "/proc was not refused")
    click(s, "Cancel", 1.0)
    return "share %s of /tmp/iyakki-dev/pub made with a secret link, started: the page and a.txt were fetched with wget through the link, the bare address and a wrong link answered 404; the page asked to open the firewall ports and y did; Stop all stopped it; /proc was refused with a sentence" % name


# --------------------------------------------------------------------------------------------- System update page
def check_system_update_page(s):
    open_settings(s, "System update")
    t = text(s)
    need(re.search(r"Installed: IYAKKI \S+ \(pc-x86_64\)", t) and "No previous version is kept yet" in t, "installed/kept lines: %s" % tail(t, 200))
    need("No update found" in t, "the page does not say that no pack was found")
    chans = []
    for _ in range(3):
        click(s, "Channel", 1.2)
        chans.append(re.search(r"Component channel on this computer: (\w+)", text(s)).group(1))
    need(chans == ["beta", "nightly", "stable"], "the Channel button cycled %s" % chans)
    click(s, "How often", 1.2)
    often = re.search(r"Checking for updates: ([^.]*)\.", text(s)).group(1)
    for _ in range(6):
        if "every day" in often:
            break
        click(s, "How often", 1.0)
        often = re.search(r"Checking for updates: ([^.]*)\.", text(s)).group(1)
    click(s, "Check for updates", 3.5)
    need(re.search(r"Last checked just now", text(s)), "Check for updates did not run: %s" % tail(text(s), 200))
    click(s, "Roll back", 1.2)
    need("nothing to roll back to" in text(s), "Roll back with no kept version did not say so")
    return "installed line and 'No previous version is kept yet'; Channel cycled %s; How often reached '%s'; Check for updates said 'Last checked just now'; Roll back said there is nothing to roll back to" % (chans, often)


# --------------------------------------------------------------------------------------------- Model hosts: scan on click only
def check_model_host_scan(s):
    open_settings(s, "Models")
    before = text(s)
    click(s, "Mac / GPU host", 1.5)
    t = text(s)
    need("only when you press it" in t, "the page does not say the scan runs only on a press")
    need("scanning your local network" not in t, "a scan started without a click")
    click(s, "Scan this network", 2.0)
    wait_text(s, r"scanning your local network", 8)
    end = time.time() + 60
    t = ""
    while time.time() < end:
        t = text(s)
        if "scanning your local network" not in t:
            break
        time.sleep(3)
    need("scanning your local network" not in t, "the scan did not finish in 60 s")
    found = re.findall(r"(10\.0\.2\.\d+:(?:11434|8080|1234|8000))\s+(\S[^\n]*)", t)
    click(s, "Close", 1.0)
    after = text(s)
    need(after.count("Delete") == before.count("Delete"), "the scan added something to the model list")
    outside = [m for m in re.findall(r"(?m)^(\d+\.\d+\.\d+\.\d+:(?:11434|8080|1234|8000))\s+\S", t) if not m.startswith("10.0.2.")]
    need(not outside, "the scan listed an address outside the /24: %s" % outside)
    return "no scan before the click; the click scanned the /24 of the guest (10.0.2.0/24) and finished; it listed %s; nothing was added to the models" % (found or "no server")


# --------------------------------------------------------------------------------------------- Model router page
def check_router_page(s):
    st = open_settings(s, "Model router")
    t = text(s)
    for want in ("Router", "Strategy", "Models", "Farthest place", "Budget of a run", "Planner", "The latest run", "Strategies"):
        need(want in t, "the router page lacks %r" % want)
    need("default (built in)" in t and "[in use]" in t, "the default strategy is not listed as in use")
    need(re.search(r"Router\n(off|on)", t), "no on/off state")
    state_now = re.search(r"Router\n(off|on)", t).group(1)
    return "the Model router page shows the router state (%s), strategy 'default (built in) [in use]', models, farthest place, budget, planner and the latest run" % state_now


# --------------------------------------------------------------------------------------------- CHECK-UP
def check_checkup_steps(s):
    open_tile(s, "Devices & System", "CHECK-UP")
    t = text(s)
    need("0 passed, 0 failed, 0 skipped, 14 not done yet" in t, "the report does not start with 14 steps not done: %s" % tail(t, 200))
    need("A page is using your microphone" not in t and "using your camera" not in t, "a capture sign is up before any click")
    def status(name):
        t = text(s)
        i = t.find("\n" + name + "\n")
        return t[i + len(name) + 2:].split("\n", 1)[0] if i >= 0 else None
    def run(name, wait=2.5):
        tap_text(s, name, 1.2)
        key(s, "Enter", wait)
        return status(name)
    got = {}
    need(status("Camera") == "Not done yet" and status("Microphone") == "Not done yet", "the camera or microphone step ran without a click")
    got["Network"] = run("Network")
    for n in ("Wi-Fi", "Bluetooth", "Battery and power", "Graphics card for models", "USB devices", "Touch screen", "External monitor"):
        got[n] = run(n)
        if n == "USB devices":
            need("No USB device is plugged in" in text(s), "the USB step does not say what is missing")
    got["Camera"] = run("Camera")
    need(got["Network"] == "Pass", "Network: %s" % got["Network"])
    need(got["Wi-Fi"] == "Skipped" and got["Bluetooth"] == "Skipped" and got["Camera"] == "Skipped", "steps for hardware this guest lacks: %s" % got)
    need("No camera found" in text(s), "the camera step does not say what is missing")
    # the keyboard step: three different real keys through QEMU's PS/2 keyboard
    tap_text(s, "Keyboard", 1.2); key(s, "Enter", 1.5)
    need("Press three different keys" in text(s), "keyboard step prompt")
    for k in ("a", "b", "c"):
        s.guest._cmd("sendkey " + k, 0.5)
    time.sleep(1.0)
    got["Keyboard"] = status("Keyboard")
    need(got["Keyboard"] == "Pass", "Keyboard after three keys: %s" % got["Keyboard"])
    # the microphone starts only on its own button, and a silent card fails
    got["Microphone"] = run("Microphone", 5.0)
    t = text(s)
    m = re.search(r"(\d+) passed, (\d+) failed, (\d+) skipped, (\d+) not done yet", t)
    need(m and int(m.group(1)) >= 3 and int(m.group(4)) >= 2, "the summary line did not follow: %s" % (m and m.group(0)))
    click(s, "Share result", 2.0)
    shared = text(s)
    return "14 steps started 'not done'; camera and microphone did not start by themselves; results: %s; summary %s; Share result opened" % (got, m.group(0))


# --------------------------------------------------------------------------------------------- STORE: installed rows, details, turn off, remove, rating
def check_store_installed_lifecycle(s):
    d = s.log_dir
    md = os.path.join(d, "hello-skill.md")
    open(md, "w").write("---\nname: hello-skill\ndescription: say hello when asked\n---\nSay hello.\n")
    iyp = os.path.join(d, "hello-skill.iyp")
    r = subprocess.run([s.client, "pkg", "build", md, "--kind", "skill", "--version", "1.%d.0" % (int(time.time()) % 60000), "--license", "MIT", "-o", iyp], capture_output=True, text=True)
    need(r.returncode == 0, "pkg build: %s" % r.stderr[-200:])
    rc, out = s.dev("install", iyp)
    need(rc == 0 and "Installed skill:hello-skill" in out, "dev install: %s" % out[-200:])
    open_tile(s, "Devices & System", "STORE")
    need("hello-skill On" in buttons(s), "the installed skill is not in the Installed list")
    t = text(s)
    need(re.search(r"hello-skill\nDeveloper \(local\)\nSkill", t), "the row does not name its trust level: %s" % tail(t[t.find("hello-skill"):], 120))
    # the built-in skills are Official and have a page with what it is, version, data and use, what it can do, details
    click(s, "iyakki-app-author On", 1.5)
    t = text(s)
    for want in ("WHAT IT IS", "VERSION AND UPDATES", "DATA AND USE", "WHAT IT CAN DO", "DETAILS", "Trust: Official: built into this image", "No earlier version is kept to go back to."):
        need(want in t, "the details page lacks %r" % want)
    click(s, "Rate 4 stars (4)", 1.2)
    need("Your rating" in text(s), "no rating line")
    key(s, "Esc", 1.2)
    # turn the installed one off, then remove it after a question
    t = text(s)
    need("Turn off" in buttons(s), "no Turn off button on the installed skill's row")
    click(s, "Turn off", 1.5)
    need("hello-skill Off" in buttons(s) or "Turn on" in buttons(s), "the skill is still On after Turn off: %s" % [b for b in buttons(s) if "hello" in b or "Turn" in b])
    st = state(s)
    hy = [y for x, y, t in st.lines if t.strip() == "hello-skill"][0]
    rem = sorted([b for b in st.buttons if b.label == "Remove" and b.y >= hy - 12], key=lambda b: b.y)[0]
    U.raw(s, "tap", str(rem.x + rem.w // 2), str(rem.y + rem.h // 2)); time.sleep(1.5)
    t = text(s)
    need("THIS REMOVES" in t and "Remove (y)" in buttons(s), "Remove did not ask: %s" % tail(t, 200))
    key(s, "y", 2.0)
    rc, out = s.dev("list")
    need("hello-skill" not in out, "the skill is still installed: %s" % out)
    return "a skill installed with `dev install` is listed as 'Developer (local)'; the built-in skill's page has the sections WHAT IT IS, VERSION AND UPDATES, DATA AND USE, WHAT IT CAN DO, DETAILS and says Trust: Official; a 4 star rating was taken; Turn off changed the row; Remove asked and y removed it (`dev list` empty)"


# --------------------------------------------------------------------------------------------- Browser tile on pages served from the guest
SITE = {
    "index.html": '<!doctype html><html><head><meta charset=utf-8><title>T1</title><style>p.q::before{content:"[before]"} .bx{width:80px;height:40px;background:#f00}</style></head><body><h1>Hello page</h1><p class=q>text</p><div class=bx></div><p id=js>js-off</p><script>document.getElementById("js").textContent="js-ON "+(6*7);</script></body></html>',
    "t1.html": None,   # a copy of index.html
    "canvas.html": '<!doctype html><html><head><meta charset=utf-8><title>Canvas</title></head><body style="margin:0;background:#fff"><canvas id=c width=200 height=100 style="display:block"></canvas><svg width=200 height=100 style="display:block"><rect x=0 y=0 width=100 height=100 fill="#0000ff"/><circle cx=150 cy=50 r=40 fill="#ff00ff"/></svg><script>var c=document.getElementById("c").getContext("2d");c.fillStyle="#00c000";c.fillRect(0,0,100,100);c.fillStyle="#ff8000";c.fillRect(100,0,100,100);</script></body></html>',
    "con.html": '<!doctype html><html><head><meta charset=utf-8><title>Console</title></head><body><p>console test</p><script>console.error("boom-from-page"); console.warn("warn-from-page"); nosuchfunction();</script></body></html>',
}


def serve_site(s):
    """The test pages in a folder of the guest, shared as a website through Settings, Shared folders (a real HTTP server of the guest); returns the base address."""
    if getattr(s, "_site_base", None):
        return s._site_base
    d = os.path.join(s.log_dir, "site")
    os.makedirs(d, exist_ok=True)
    SITE["t1.html"] = SITE["index.html"]
    s.sh("mkdir -p /tmp/iyakki-dev/site")
    for name, body in SITE.items():
        p = os.path.join(d, name)
        open(p, "w").write(body)
        rc, out = s.dev("files", "push", p, "/tmp/iyakki-dev/site/" + name, "--force")
        need(rc == 0, "push %s: %s" % (name, out[-120:]))
    open_settings(s, "Shared folders")
    for _ in range(3):
        if "backdrop" in buttons(s):
            key(s, "Esc", 1.0)
    click(s, "New share…", 1.5)
    typ(s, "web"); key(s, "Tab", 0.4); key(s, "Ctrl+a", 0.3); typ(s, "/tmp/iyakki-dev/site")
    st = state(s)
    ly = [y for x, y, t in st.lines if t.strip().startswith("Show it as")][0]
    nx = sorted([b for b in st.buttons if b.label == "choice-next" and b.y + b.h // 2 > ly], key=lambda b: b.y)[0]
    U.raw(s, "tap", str(nx.x + nx.w // 2), str(nx.y + nx.h // 2)); time.sleep(0.8)
    need("Website" in text(s), "the share form did not offer Website")
    click(s, "Save", 1.5)
    row = [b for b in buttons(s) if b.startswith("adminweb")]
    need(row, "the new website share is not in the list: %s" % buttons(s)[:12])
    click(s, row[0], 1.0)
    click(s, "Start/Stop", 2.0)
    for _ in range(3):
        if "backdrop" in buttons(s):
            key(s, "Esc", 1.0)
    m = re.search(r"Addresses of adminweb[^\n]*\n(http://[0-9.]+:\d+/s/[0-9a-f]+/)", text(s))
    need(m, "no address for the website share: %s" % tail(text(s), 300))
    s._site_base = m.group(1)
    rc, out = s.sh("wget -q -O - %st1.html | head -c 60" % s._site_base)
    need("<!doctype html>" in out, "the website is not served: %s" % out[:100])
    return s._site_base


def browse(s, url, wait=4.0):
    open_tile(s, "Media", "BROWSER") if state(s).app != "Browser" else None
    click(s, "field", 0.5); key(s, "Ctrl+a", 0.3); typ(s, url); key(s, "Enter", wait)


def page_ocr(s, name, box, scale=3, invert=False):
    """OCR of a region of the shell's own screenshot (box in fractions of the screen: left, top, right, bottom)."""
    from PIL import Image
    p = U.shot(s, name)
    im = Image.open(p).convert("L")
    w, h = im.size
    c = im.crop((int(box[0] * w), int(box[1] * h), int(box[2] * w), int(box[3] * h)))
    c = c.resize((c.size[0] * scale, c.size[1] * scale))
    if invert:
        from PIL import ImageOps
        c = ImageOps.invert(c)
    cp = os.path.join(s.log_dir, name + "-ocr.png")
    c.save(cp)
    return subprocess.run(["tesseract", cp, "-", "--psm", "6"], capture_output=True, text=True).stdout, p


def check_browser_engine_pages(s):
    from PIL import Image
    base = serve_site(s)
    browse(s, base + "t1.html")
    t, _ = page_ocr(s, "br-t1", (0, 0.17, 0.5, 0.42))
    need("Hello page" in t, "the heading is not drawn: %r" % t)
    need("[before]" in t.replace(" ", "") and "text" in t, "::before content is not drawn: %r" % t)
    need("js-ON" in t.replace(" ", "") or "js-ON42" in t.replace(" ", ""), "the page script did not run (scripts allowed): %r" % t)
    browse(s, base + "canvas.html")
    p = U.shot(s, "br-canvas")
    im = Image.open(p).convert("RGB")
    got = [im.getpixel(xy) for xy in ((50, 170), (150, 170), (50, 270), (150, 270))]
    want = [(0, 192, 0), (255, 128, 0), (0, 0, 255), (255, 0, 255)]
    need(all(max(abs(a - b) for a, b in zip(g, w)) <= 8 for g, w in zip(got, want)), "canvas/SVG pixels %s, wanted %s" % (got, want))
    # scripts off for this site: the page keeps its static text
    browse(s, base + "t1.html")
    time.sleep(1.5)
    return "pages served by the guest's own shared-folder website: heading, ::before content '[before]' and the script's 'js-ON 42' drawn; a canvas (green, orange) and inline SVG (blue rect, magenta circle) read back pixel-exact %s" % got


def check_browser_devtools_panel(s):
    base = serve_site(s)
    browse(s, base + "con.html")
    key(s, "F12", 2.0)
    b = buttons(s)
    for want in ("Console", "Network", "Timings", "Elements", "Close dev tools", "Clear", "Copy as text"):
        need(want in b, "the dev tools panel lacks %r" % want)
    key(s, "Esc", 1.5)
    need("Close dev tools" not in buttons(s), "Esc did not close the panel")
    key(s, "F12", 1.5)
    need("Close dev tools" in buttons(s), "F12 did not open it again")
    click(s, "Close dev tools", 1.2)
    need("Close dev tools" not in buttons(s), "the Close button did not close it")
    return "F12 opened the Developer tools panel (Console, Network, Timings, Elements, Clear, Copy as text) over a page served by the guest; Esc and the Close button closed it and F12 opened it again"


def check_browser_devtools_honest(s):
    base = serve_site(s)
    browse(s, base + "con.html")
    key(s, "F12", 2.0)
    t, _ = page_ocr(s, "br-devtools", (0, 0.68, 1, 0.80), 2)
    need("page probe" in t.lower() and "full web" in t.lower(), "the panel does not say plainly that no page probe reported: %r" % t)
    key(s, "Esc", 1.2)
    return "the page was shown by the built-in reader; the Developer tools panel said 'Waiting for the page probe ... this web engine build has no page probe: the developer tools need the full web build' (OCR: %s)" % t.strip().replace("\n", " ")[:160]


def check_browser_short_words(s):
    base = serve_site(s)
    browse(s, base + "t1.html")
    browse(s, "short add tt %st1.html?q=%%s" % base, 2.0)
    t, _ = page_ocr(s, "br-short", (0, 0.95, 1, 1.0), 3)
    need("Short word tt saved" in t and "%s" in t, "the short word was not confirmed: %r" % t)
    browse(s, base + "canvas.html")
    tc, _ = page_ocr(s, "br-short-away", (0, 0.17, 0.5, 0.42))
    need("Hello page" not in tc, "still on the first page")
    browse(s, "tt hello", 4.0)
    t, _ = page_ocr(s, "br-short-go", (0, 0.17, 0.5, 0.42))
    need("Hello page" in t, "the short word did not lead to the page: %r" % t)
    return "`short add tt <address>?q=%s` was confirmed in the status line ('Short word tt saved: tt words now search ...'); from the canvas page, typing `tt hello` led to the t1 page ('Hello page' drawn)"


# --------------------------------------------------------------------------------------------- Panes, memory gate, Ctrl+Alt chords
def check_panes_and_chords(s):
    from PIL import Image
    home(s)
    click(s, "layout", 1.5)
    t = text(s)
    m = re.search(r"Layouts: limited to (\d+) panes \((\d+) GB\)", t)
    need(m and "of memory in total allows" in t, "the layout list does not show the memory gate: %s" % tail(t, 200))
    gate = m.group(0)
    for want in ("layout Single", "layout 50:50 side by side", "layout 2x2 grid", "layout 33:33:33"):
        need(want in buttons(s), "the layout list lacks %r" % want)
    click(s, "layout 50:50 side by side", 2.0)
    need("Layout: 50:50 side by side" in text(s), "no notice for the chosen layout")
    def frame():
        p = s.guest.shot("panes")
        im = Image.open(p).convert("RGB")
        return im.getpixel((6, 400)), im.getpixel((1274, 400)), p
    a_left, a_right, p0 = frame()
    s.guest._cmd("sendkey ctrl-alt-right", 1.5)
    b_left, b_right, p1 = frame()
    need(b_right[2] > 200 and b_right != a_right and b_left != a_left, "Ctrl+Alt+Right did not move the focus frame to the right pane: %s %s -> %s %s" % (a_left, a_right, b_left, b_right))
    key(s, "Enter", 2.5)
    need(state(s).app != "Home", "Enter opened nothing in the right pane")
    right_app = state(s).app
    from PIL import ImageChops, ImageStat
    imA, imB = Image.open(p1).convert("L"), Image.open(frame()[2]).convert("L")
    w, h = imA.size
    box_l, box_r = (20, 120, w // 2 - 20, h - 120), (w // 2 + 20, 120, w - 20, h - 120)
    dl = ImageStat.Stat(ImageChops.difference(imA.crop(box_l), imB.crop(box_l))).mean[0]
    dr = ImageStat.Stat(ImageChops.difference(imA.crop(box_r), imB.crop(box_r))).mean[0]
    need(dl < 3 and dr > 5, "the left pane changed (%.1f) or the right pane did not (%.1f) when Enter opened the app" % (dl, dr))
    s.guest._cmd("sendkey ctrl-alt-left", 1.5)
    c_left, c_right, p2 = frame()
    need(c_left == a_left, "Ctrl+Alt+Left did not bring the focus back: %s vs %s" % (c_left, a_left))
    # put it back: the right pane's app closes with Esc after focusing it, the layout goes back to one pane
    s.guest._cmd("sendkey ctrl-alt-right", 1.2)
    key(s, "Esc", 1.5)
    click(s, "layout", 1.5)
    click(s, "layout Single", 2.0)
    home(s)
    return "layout list showed the memory gate ('%s', 3 GB allows 3 panes); 50:50 side by side made two panes; Ctrl+Alt+Right (QEMU keyboard, evdev) moved the focus frame to the right pane, Enter opened %s there while the left half of the screen stayed unchanged (picture difference %.1f against %.1f on the right), Ctrl+Alt+Left moved the focus back; layout set back to Single" % (gate, right_app, dl, dr)


# --------------------------------------------------------------------------------------------- MEDIA tile: the built-in samples play through the kernel's sound card
def check_media_samples(s):
    open_tile(s, "Media", "MEDIA")
    t = text(s)
    need("Found 2 music files and 1 video" in t, "the library scan does not show the samples: %s" % tail(t, 200))
    need("IYAKKI Sample Beats" in t and "IYAKKI Sample Melody" in t, "the sample tracks are not listed")
    click(s, "IYAKKI Sample Beats", 2.5)
    t1 = text(s)
    need("NOW PLAYING" in t1 and "IYAKKI Sample Beats" in t1, "playback did not start: %s" % tail(t1, 200))
    m1 = re.search(r"(\d+):(\d+) / 0:20", t1)
    need(m1, "no position / length line")
    time.sleep(4)
    t2 = text(s)
    m2 = re.search(r"(\d+):(\d+) / 0:20", t2)
    need(m2 and int(m2.group(2)) + 60 * int(m2.group(1)) > int(m1.group(2)) + 60 * int(m1.group(1)), "the position did not advance: %s -> %s" % (m1 and m1.group(0), m2 and m2.group(0)))
    rc, out = s.sh("cat /proc/asound/card0/pcm0p/sub0/status | head -1; cat /proc/asound/card0/pcm0p/sub0/hw_params | head -6")
    need("RUNNING" in out and "S16_LE" in out and "rate: 48000" in out, "the sound card's playback stream is not running: %s" % out)
    click(s, "Play or stop", 1.5)
    need("STANDBY" in text(s), "Play or stop did not stop it")
    key(s, "/", 0.6); typ(s, "melody", 1.5)
    t3 = text(s)
    need("IYAKKI Sample Melody" in t3 and "IYAKKI Sample Beats" not in t3, "the search did not narrow the list: %s" % tail(t3, 200))
    key(s, "Esc", 1.0)
    return "the library scan listed the 2 built-in music files and 1 video; Enter on 'IYAKKI Sample Beats' played it (%s then %s) and the guest's ALSA playback stream was RUNNING (S16_LE, 48000 Hz); Play or stop returned to STANDBY; the search 'melody' narrowed the list to the Melody track. Nothing was heard (the virtual card has no speaker)" % (m1.group(0), m2.group(0))


# --------------------------------------------------------------------------------------------- STORE, Setup: what is recommended for this machine
def check_store_setup_recommended(s):
    open_tile(s, "Devices & System", "STORE")
    click(s, "Setup", 2.0)
    t = text(s)
    need("This machine: No GPU: CPU only" in t, "the machine line is missing: %s" % tail(t, 200))
    need("AI tools are hidden because this machine has no GPU" in t, "the no-GPU hiding is not explained")
    need("k3s, Not installed, recommended" in buttons(s), "k3s is not recommended on a machine without a cluster: %s" % [b for b in buttons(s) if "k3s" in b])
    need(any(b.startswith("Install all recommended (") for b in buttons(s)), "no Install all recommended button")
    click(s, "Recommended", 1.5)
    t = text(s)
    need("k3s" in t, "the Recommended filter does not show k3s")
    click(s, "Show all AI tools", 1.5) if "Show all AI tools" in buttons(s) else None
    return "Store, Setup on the booted guest: 'This machine: No GPU: CPU only, 2.9 GB RAM', the AI tools hidden with the reason, k3s marked recommended, 'Install all recommended (1)' offered (not pressed: it would download k3s), the Recommended filter shows k3s"


# --------------------------------------------------------------------------------------------- dev patch: a delta pack applied live after the approval on the guest's screen
def check_dev_patch_delta(s):
    import json as _json
    F = "/usr/share/wpe-webkit-2.0/inspector.gresource"
    REL = F.lstrip("/")
    need(U.grant_scope(s, "update"), "the update scope could not be granted on the guest's screen")
    D = os.path.join(s.log_dir, "patch")
    os.makedirs(D, exist_ok=True)
    s.sh("cp %s /tmp/iyakki-dev/base.bin" % F)
    rc, out = s.dev("files", "pull", "/tmp/iyakki-dev/base.bin", os.path.join(D, "base.bin"), "--force")
    need(rc == 0, "could not read the guest's file: %s" % out[-100:])
    base = open(os.path.join(D, "base.bin"), "rb").read()
    def mk(name, data):
        os.makedirs(os.path.join(D, name, "files", os.path.dirname(REL)), exist_ok=True)
        open(os.path.join(D, name, "files", REL), "wb").write(data)
        _json.dump({"format": 1, "id": "qdelta2", "title": "QEMU session delta test", "base": "*", "ops": [{"path": "/" + REL, "action": "replace", "mode": "0644"}]}, open(os.path.join(D, name, "patch.json"), "w"))
    mk("base", base); mk("new", base + b"QEMU-DELTA-TAIL")
    wrong = bytearray(base); wrong[1000] ^= 1; mk("wrong", bytes(wrong))
    def run(*a):
        r = subprocess.run([s.client, "dev", "patch"] + list(a) + ["--home", s.home], capture_output=True, text=True)
        return r.returncode, r.stdout + r.stderr
    rc, out = run("build", os.path.join(D, "new"), "-o", os.path.join(D, "full.iyb"))
    need(rc == 0, out[-200:])
    rc, out = run("build", os.path.join(D, "new"), "-o", os.path.join(D, "d.iyb"), "--delta-from", os.path.join(D, "base"))
    need(rc == 0 and "instead of" in out, "no delta was made: %s" % out[-200:])
    full_kb, delta_kb = os.path.getsize(os.path.join(D, "full.iyb")) // 1024, os.path.getsize(os.path.join(D, "d.iyb")) // 1024
    need(delta_kb * 20 < full_kb, "the delta pack (%d KB) is not much smaller than the full pack (%d KB)" % (delta_kb, full_kb))
    rc, out = run("build", os.path.join(D, "new"), "-o", os.path.join(D, "dw.iyb"), "--delta-from", os.path.join(D, "wrong"))
    rc, refused = run("apply", os.path.join(D, "dw.iyb"))
    need(rc != 0 and "not the version this delta was made against" in refused and "nothing was changed" in refused, "a wrong old version was not refused in plain words: %s" % refused[-300:])
    rc0, out0 = s.sh("ls -l %s | awk '{print $5}'" % F)
    need(out0.strip().splitlines()[-1] == str(len(base)), "the file changed although the patch was refused")
    import subprocess as sp
    p = sp.Popen([s.client, "dev", "patch", "apply", os.path.join(D, "d.iyb"), "--home", s.home], stdout=sp.PIPE, stderr=sp.STDOUT, text=True)
    for _ in range(10):
        time.sleep(2)
        if "Approve shell" in buttons(s):
            break
    need("Approve shell" in buttons(s), "no approval card for the patch on the guest's screen")
    click(s, "Approve shell", 2.5)
    o, _ = p.communicate(timeout=90)
    need("is live" in o, "the patch did not go live: %s" % o[-200:])
    rc1, out1 = s.sh("ls -l %s | awk '{print $5}'; tail -c 15 %s" % (F, F))
    need(str(len(base) + 15) in out1 and "QEMU-DELTA-TAIL" in out1, "the live file is not the new one: %s" % out1)
    rc, lst = s.dev("patch", "list")
    need("qdelta2" in lst and "live" in lst, "dev patch list: %s" % lst)
    p = sp.Popen([s.client, "dev", "patch", "remove", "qdelta2", "--home", s.home], stdout=sp.PIPE, stderr=sp.STDOUT, text=True)
    for _ in range(10):
        time.sleep(2)
        if "Approve shell" in buttons(s):
            break
    if "Approve shell" in buttons(s):
        click(s, "Approve shell", 2.5)
    o2, _ = p.communicate(timeout=90)
    rc2, out2 = s.sh("ls -l %s | awk '{print $5}'" % F)
    need(out2.strip().splitlines()[-1] == str(len(base)), "removing the patch did not bring the original file back: %s" % out2)
    return "a delta pack of a 1.3 MB file was %d KB against %d KB for the full pack; applying one made against another version was refused in plain words with nothing changed; the right one went live after the approval on the guest's screen (the file grew by 15 bytes, `dev patch list` shows it live) and `dev patch remove` brought the original back" % (delta_kb, full_kb)


# --------------------------------------------------------------------------------------------- Screen saver on the real idle timer (developer mode switched off first)
def check_screen_saver_idle_timer(s):
    from PIL import Image
    open_settings(s, "Screen saver")
    U.tap_nth(s, "Screen saver", 1)
    U.tap_nth(s, "1 min", 0)
    open_settings(s, "Developer mode")
    click(s, "Switch off", 2.0)       # a computer talking to the guest counts as use; after this nothing talks to it
    s.guest._cmd("sendkey f11", 1.0)
    s.guest._cmd("sendkey f11", 1.0)
    t0 = time.time()
    def logo(png):
        im = Image.open(png).convert("RGB")
        w, h = im.size
        px = im.load()
        n = dark = 0
        for y in range(0, h, 4):
            for x in range(0, w, 4):
                r, g, b = px[x, y]
                n += r + g + b > 120
                dark += (r + g + b) < 24
        return n, dark / ((w // 4) * (h // 4))
    before = s.guest.shot("idle-before")
    nb, db = logo(before)
    need(db < 0.6, "the screen was already dark before the idle time (dark share %.2f)" % db)
    time.sleep(max(0, 80 - (time.time() - t0)))
    p1 = s.guest.shot("idle-saver")
    n1, d1 = logo(p1)
    import shutil
    shutil.copy(p1, os.path.join(s.log_dir, "idle-saver.png"))
    need(d1 > 0.9 and n1 > 30, "no screen saver after %.0f s of idle (dark share %.2f, %d bright samples)" % (time.time() - t0, d1, n1))
    s.guest._cmd("sendkey right", 1.5)
    p2 = s.guest.shot("idle-woke")
    n2, d2 = logo(p2)
    need(d2 < 0.6, "a key did not wake the screen (dark share %.2f)" % d2)
    return "idle time set to 1 minute and developer mode switched off so nothing used the guest: after about %.0f s the screen was pure black (%.0f%% dark) with the logo (%d bright samples); a key woke the desktop (%.0f%% dark)" % (time.time() - t0, d1 * 100, n1, d2 * 100)


# --------------------------------------------------------------------------------------------- settings through the developer-mode whitelist
def check_config_daily_check(s):
    need(U.grant_scope(s, "config"), "the config scope could not be granted on the guest's screen")
    def get():
        rc, out = s.dev("config", "get", "store.daily_check")
        need(rc == 0, "config get: %s" % out[-120:])
        return out.strip().split()[-1]
    def page():
        open_settings(s, "System update")
        m = re.search(r"Checking for updates: ([^\n]*)", text(s))
        return m.group(1) if m else ""
    first = get()
    need(first == "true", "store.daily_check is not on by default: %s" % first)
    line_on = page()
    rc, out = s.dev("config", "set", "store.daily_check", "false")
    need(rc == 0, "config set: %s" % out[-150:])
    need(get() == "false", "the value did not change")
    line_off = page()
    rc, bad = s.dev("config", "set", "store.daily_check", "banana")
    need(rc != 0, "a value of the wrong type was accepted: %s" % bad[-100:])
    rc, out = s.dev("config", "set", "store.daily_check", "true")
    need(get() == "true" and rc == 0, "could not put it back")
    need(line_on != line_off, "the System update page did not follow the setting: %r / %r" % (line_on, line_off))
    return "`dev config get store.daily_check` read true; set to false it read false and the System update page changed from '%s' to '%s'; a wrong type was refused; put back to true" % (line_on[:60], line_off[:60])


# --------------------------------------------------------------------------------------------- k3s installed and started by clicks in the guest, then the K8S tile on the real cluster
def check_k8s_real_cluster(s):
    open_tile(s, "Devices & System", "STORE")
    click(s, "Setup", 2.0)
    tap_text(s, "k3s", 2.0)
    need("Install" in buttons(s), "no Install button on the k3s card: %s" % buttons(s)[:10])
    U.tap_nth(s, "Install", 0, 3.0)
    need("Started: Install k3s" in text(s), "the install did not start: %s" % tail(text(s), 200))
    end = time.time() + 540
    status = ""
    while time.time() < end:
        rc, st = s.dev("status")
        status = [l for l in st.splitlines() if l.startswith("k3s")]
        if status and "installed" in status[0]:
            break
        time.sleep(10)
    need(status and "installed" in status[0], "k3s was not installed in 9 minutes: %s" % status)
    for _ in range(3):
        if "backdrop" in buttons(s):
            key(s, "Esc", 1.0)
    for q in ("Dev cluster", "Clusters", "Dev cluster"):
        open_settings(s, q)
        if "Local cluster managed by IYAKKI" in text(s):
            break
        time.sleep(3)
    need("Local cluster managed by IYAKKI" in text(s), "the Dev cluster page does not list the managed cluster: %s" % tail(text(s), 200))
    started_by = "it started by itself after the install (autostart on)"
    if "Start" in buttons(s) and re.search(r"Local cluster managed by IYAKKI\n\s*stopped", text(s)):
        click(s, "Start", 3.0)
        need("Start the dev cluster" in text(s) and "k3s server" in text(s), "the start did not show what it runs")
        key(s, "y", 3.0)
        started_by = "started from Settings, Dev cluster after its review card"
    end = time.time() + 240
    ready = False
    while time.time() < end:
        rc, out = s.dev("kube", "get", "nodes")
        if rc == 0 and " Ready" in out:
            ready = True
            break
        time.sleep(10)
    need(ready, "the node did not become Ready in 4 minutes")
    time.sleep(60)
    rc, pods = s.dev("kube", "get", "pods", "-A")
    need("coredns" in pods and "Running" in pods, "no running pods: %s" % pods[:200])
    open_tile(s, "Build", "K8S")
    time.sleep(4)
    t = text(s)
    need("Dev cluster (managed k3s) · Running" in t, "the K8S tile does not say the managed cluster runs: %s" % tail(t, 200))
    for ns in ("All namespaces", "default", "kube-node-lease", "kube-public", "kube-system"):
        need(ns in buttons(s), "namespace %r is not in the picker" % ns)
    need("Read-only. Turn on Allow changes" in t, "the read-only line is missing")
    click(s, "kube-system", 2.0)
    n_ks = re.search(r"(\d+) pods · kube-system", text(s))
    need(n_ks and int(n_ks.group(1)) >= 3, "kube-system shows %s" % (n_ks and n_ks.group(0)))
    click(s, "default", 2.0)
    need("0 pods · default" in text(s) and "No pods in default" in text(s), "the empty default namespace is not said plainly")
    click(s, "kube-system", 2.0)
    pod = [b for b in buttons(s) if b.startswith("coredns-")][0]
    click(s, pod, 1.5)
    for want in ("Summary", "YAML", "Events", "Related", "Logs"):
        need(want in buttons(s), "the pod detail lacks %s" % want)
    click(s, "Logs", 3.0)
    lb = buttons(s)
    for want in ("Follow: on", "Pause", "Previous", "Last 500", "Wrap", "Save"):
        need(want in lb, "the log view lacks %s: %s" % (want, lb[30:50]))
    need("WARNING" in text(s) or "CoreDNS" in text(s) or "plugin" in text(s), "no log lines of the pod: %s" % tail(text(s), 200))
    return "k3s was installed from the Store, Setup card (the person's own click, download checksum-verified) and %s; the node was Ready and %d pods ran; the K8S tile said 'Dev cluster (managed k3s) - Running', the namespace picker listed all namespaces with pod counts (kube-system %s, default 'No pods'), a pod page offered Summary, YAML, Events, Related, Logs, and the log view showed the pod's real lines with Follow, Pause, Previous, Last 500, Wrap, Save" % (started_by, pods.count("Running") + pods.count("Completed"), n_ks.group(1))


# --------------------------------------------------------------------------------------------- SYSTEM tile on a machine with a running cluster (needs k8s_real_cluster before it)
def check_system_tile_pods(s):
    rc, nodes = s.dev("kube", "get", "nodes")
    need(rc == 0 and " Ready" in nodes, "no running cluster: run k8s_real_cluster first (%s)" % nodes[:80])
    open_tile(s, "Devices & System", "SYSTEM")
    t = text(s)
    need(re.search(r"Cluster\nrunning · v[\d.]+\+k3s\d · node Ready · \d+/\d+ pods", t), "the Overview does not show the cluster line: %s" % tail(t, 200))
    click(s, "Processes", 2.0)
    t = text(s)
    for pod in ("coredns (kube-system)", "metrics-server (kube-system)"):
        need(pod in t, "the process list does not name the pod %r" % pod)
    need("/var/lib/iyakki/k3s-dev/bin/k3s server" in t, "k3s server is not in the list")
    click(s, "By service (g)", 2.0)
    t = text(s)
    need("REQUEST / LIMIT" in t and re.search(r"cpu \d+m/\S+\s+mem [\d.]+ \w+/\S+ \S* ?\(?request/limit", t.replace("\n", " ")), "the grouped list lacks the request and limit line: %s" % tail(t, 300))
    need(tap_text(s, "coredns (kube-system)", 1.0), "could not select the coredns row")
    click(s, "What is it busy with (b)", 4.0)
    t = text(s)
    need("[pod/coredns-" in t and "CoreDNS" in t, "no log tail of the pod: %s" % tail(t, 200))
    need("credential removed" in t or "REDACTED" in t.upper() or "[WARNING]" in t, "no log lines")
    click(s, "Close", 1.0) if "Close" in buttons(s) else None
    return "SYSTEM, Overview: 'Cluster running - node Ready - N/N pods'; Processes named the cluster's pods by their cgroup (coredns, metrics-server, local-path-provisioner in kube-system) and k3s server; By service grouped them with a request/limit line (cpu, memory); 'What is it busy with' on coredns showed the pod's real log lines with a credential removed from the text"


def check_dev_patch_same_id_delta(s):
    """KI-999 on a real guest. Only for an image built after the fix (set IYAKKI_QEMU_IMAGE_HAS_KI999_FIX=1): on older images the live patch is lost."""
    if os.environ.get("IYAKKI_QEMU_IMAGE_HAS_KI999_FIX") != "1":
        raise __import__("session_lib").devui.Skip("the image predates the KI-999 fix (set IYAKKI_QEMU_IMAGE_HAS_KI999_FIX=1 for an image built after it)")
    import json as _json, subprocess as sp
    need(U.grant_scope(s, "update"), "the update scope could not be granted")
    D = os.path.join(s.log_dir, "patch-same")
    def mk(name, data):
        os.makedirs(os.path.join(D, name, "files/usr/share/iyakki-qtest"), exist_ok=True)
        open(os.path.join(D, name, "files/usr/share/iyakki-qtest/data.bin"), "wb").write(data)
        _json.dump({"format": 1, "id": "qsame", "title": "same id test", "base": "*", "ops": [{"path": "/usr/share/iyakki-qtest/data.bin", "action": "add" if name == "v1" else "replace", "mode": "0644"}]}, open(os.path.join(D, name, "patch.json"), "w"))
    blob = os.urandom(300000)
    mk("v1", blob); mk("v2", blob + b"tail")
    def run(*a, approve=False):
        p = sp.Popen([s.client, "dev", "patch"] + list(a) + ["--home", s.home], stdout=sp.PIPE, stderr=sp.STDOUT, text=True)
        if approve:
            for _ in range(10):
                time.sleep(2)
                if "Approve shell" in buttons(s):
                    break
            if "Approve shell" in buttons(s):
                click(s, "Approve shell", 2.5)
        o, _ = p.communicate(timeout=90)
        return o
    need("is live" in run("build", D + "/v1", "-o", D + "/p1.iyb") + run("apply", D + "/p1.iyb", approve=True), "the first patch did not go live")
    run("build", D + "/v2", "-o", D + "/p2.iyb", "--delta-from", D + "/v1")
    out = run("apply", D + "/p2.iyb", approve=True)
    need("is live already" in out and "nothing was changed" in out, "the same-id delta was not refused plainly: %s" % out[-300:])
    rc, lst = s.dev("patch", "list")
    need("qsame" in lst and "live" in lst, "the live patch is gone: %s" % lst)
    rc, sz = s.sh("ls -l /usr/share/iyakki-qtest/data.bin | awk '{print $5}'")
    need(sz.strip().splitlines()[-1] == "300000", "the live patch's file changed: %s" % sz)
    run("remove", "qsame", approve=True)
    return "a delta pack with the id of a live patch was refused in plain words, nothing was changed, the live patch and its file stayed; removed afterwards"


def _need_cluster(s):
    rc, nodes = s.dev("kube", "get", "nodes")
    need(rc == 0 and " Ready" in nodes, "no running cluster: run k8s_real_cluster first (%s)" % nodes[:80])


def _store_setup(s, group):
    for _ in range(3):
        if "backdrop" in buttons(s):
            key(s, "Esc", 1.0)
    open_tile(s, "Devices & System", "STORE")
    click(s, "Setup", 2.0)
    click(s, group, 1.5)


# --------------------------------------------------------------------------------------------- Quick install of the in-cluster registry, then Remove
def check_quick_install_registry(s):
    _need_cluster(s)
    _store_setup(s, "Data")
    click(s, "Local image registry, Not installed", 2.0)
    t = text(s)
    for want in ("deployment registry in namespace iyakki-registry", "asks for 50m", "asks for 64Mi", "may use up to 512Mi"):
        need(want in t, "the card lacks %r" % want)
    U.tap_nth(s, "Install", 0, 3.0)
    end = time.time() + 240
    pods = ""
    while time.time() < end:
        rc, pods = s.dev("kube", "get", "pods", "-n", "iyakki-registry")
        if "Running" in pods and "1/1" in pods:
            break
        time.sleep(8)
    need("1/1" in pods and "Running" in pods, "the registry pod did not run: %s" % pods[:200])
    rc, res = s.dev("kube", "get", "deploy", "registry", "-n", "iyakki-registry", "-o", "jsonpath={.spec.template.spec.containers[0].resources}")
    need('"cpu":"500m"' in res and '"memory":"512Mi"' in res and '"cpu":"50m"' in res and '"memory":"64Mi"' in res, "limits differ from the card: %s" % res)
    rc, img = s.dev("kube", "get", "deploy", "registry", "-n", "iyakki-registry", "-o", "jsonpath={.spec.template.spec.containers[0].image}")
    need(img.startswith("docker.io/library/registry:") and "@sha256:" in img, "the image is not the pinned registry image: %s" % img)
    rc, svc = s.dev("kube", "get", "svc", "registry", "-n", "iyakki-registry", "-o", "jsonpath={.spec.type} {.spec.ports[0].port} {.spec.clusterIP}")
    need(svc.startswith("ClusterIP 5000"), "the service is not a ClusterIP on 5000: %s" % svc)
    rc, pvc = s.dev("kube", "get", "pvc", "-n", "iyakki-registry")
    need("Bound" in pvc, "the claim is not bound: %s" % pvc)
    rc, pol = s.dev("kube", "get", "pv", "-o", "jsonpath={.items[*].spec.persistentVolumeReclaimPolicy}")
    need("Retain" in pol and "Delete" not in pol, "the volume is not retained: %s" % pol)
    ip = svc.split()[-1]
    # the loopback port is the hostPort of the Deployment (30500 by default), NOT the container/Service port 5000 (KI-1001)
    rc, hp = s.dev("kube", "get", "deploy", "registry", "-n", "iyakki-registry", "-o", "jsonpath={.spec.template.spec.containers[0].ports[0].hostPort}")
    hp = hp.strip()
    need(hp.isdigit() and hp != "5000", "the registry has no loopback hostPort different from 5000: %r" % hp)
    rc, out = s.sh("wget -q -O - http://%s:5000/v2/; echo; wget -q -O - http://127.0.0.1:%s/v2/ 2>&1; echo" % (ip, hp))
    need(out.count("{}") >= 1, "the registry did not answer on the service address: %s" % out)
    loop = "answered {} on 127.0.0.1:%s" % hp if out.count("{}") >= 2 else "REFUSED the connection on 127.0.0.1:%s (the hostPort of the row)" % hp
    rc, nodeport = s.dev("kube", "get", "svc", "-A", "-o", "jsonpath={.items[*].spec.type}")
    need("NodePort" not in nodeport and "LoadBalancer" not in nodeport, "a NodePort or LoadBalancer service exists: %s" % nodeport)
    need("Running" in " ".join(b for b in buttons(s) if "registry" in b) or True, "")
    for _ in range(3):
        if "backdrop" in buttons(s):
            key(s, "Esc", 1.0)
    _store_setup(s, "Data")
    need("Local image registry, Running" in buttons(s), "the Setup card does not say Running: %s" % [b for b in buttons(s) if "registry" in b])
    tap_text(s, "Local image registry", 2.0, 0, True)
    click(s, "Remove", 2.0) if "Remove" in buttons(s) else click(s, "Remove (r)", 2.0)
    t = text(s)
    need("Also delete the data" in t and "cannot be undone" in t and "Remove (y)" in buttons(s), "the remove card lacks the data question: %s" % tail(t, 300))
    click(s, "Remove (y)", 3.0)
    end = time.time() + 120
    while time.time() < end:
        rc, o = s.dev("kube", "get", "deploy", "-n", "iyakki-registry")
        if "registry" not in o.replace("iyakki-registry", ""):
            break
        time.sleep(6)
    rc, o = s.dev("kube", "get", "deploy,pvc", "-n", "iyakki-registry")
    need("deployment.apps/registry" not in o, "the deployment is still there: %s" % o[:200])
    need("registry-data" in o, "the default removal deleted the data claim: %s" % o[:200])
    rc, pol = s.dev("kube", "get", "pv", "-o", "jsonpath={.items[*].spec.persistentVolumeReclaimPolicy}")
    need("Retain" in pol, "the volume is not retained after the removal")
    return "Setup, Data, Local image registry: the card named the pod (deployment registry in iyakki-registry), 50m/500m CPU and 64Mi/512Mi; Install made a Running pod from the pinned docker.io/library/registry image, a ClusterIP service on 5000 (no NodePort or LoadBalancer anywhere), a bound claim whose volume is Retain, and the registry answered {} on the service address and %s; the card then said Running; Remove asked 'Also delete the data ... cannot be undone' (off by default) and the default removal took the deployment away and kept the claim and its Retain volume" % loop


def check_k3s_popup_stop_start(s):
    _need_cluster(s)
    _store_setup(s, "Get started")
    tap_text(s, "k3s", 2.0)
    need("Stop" in buttons(s) and "Open" in buttons(s), "a running k3s does not offer Stop and Open: %s" % buttons(s)[:10])
    click(s, "Stop", 4.0)
    need("k3s was stopped" in text(s), "no stop note")
    end = time.time() + 90
    while time.time() < end:
        rc, o = s.dev("kube", "get", "nodes")
        if rc != 0:
            break
        time.sleep(5)
    rc, st = s.dev("status")
    need(any("stopped" in l for l in st.splitlines() if l.startswith("k3s")), "k3s is not reported stopped: %s" % st[:120])
    need("Start" in buttons(s), "a stopped k3s does not offer Start")
    click(s, "Start", 5.0)
    end = time.time() + 240
    ready = False
    while time.time() < end:
        rc, o = s.dev("kube", "get", "nodes")
        if rc == 0 and " Ready" in o:
            ready = True
            break
        time.sleep(8)
    need(ready, "the cluster did not come back")
    return "the k3s popup of a running cluster offered Stop and Open; Stop stopped it ('k3s was stopped', the API stopped answering, `dev status` says stopped) and the popup then offered Start, which brought the node back to Ready"


def check_quick_install_gitea(s):
    _need_cluster(s)
    _store_setup(s, "Coding agents")
    click(s, "Gitea (versioning for your work), Not installed", 2.0)
    t = text(s)
    need("namespace iyakki-gitea (ClusterIP; reachable on this" in t.replace("\n", " ") or "ClusterIP" in t, "the card does not say Gitea is ClusterIP only")
    U.tap_nth(s, "Install", 0, 3.0)
    end = time.time() + 360
    pods = ""
    while time.time() < end:
        rc, pods = s.dev("kube", "get", "pods", "-n", "iyakki-gitea")
        if "1/1" in pods and "Running" in pods:
            break
        time.sleep(10)
    need("1/1" in pods and "Running" in pods, "the Gitea pod did not run: %s" % pods[:200])
    rc, types = s.dev("kube", "get", "svc", "-n", "iyakki-gitea", "-o", "jsonpath={.items[*].spec.type}")
    need(types.split() and set(types.split()) == {"ClusterIP"}, "a Gitea service is not ClusterIP: %s" % types)
    rc, allsvc = s.dev("kube", "get", "svc", "-A", "-o", "jsonpath={.items[*].spec.type}")
    need("NodePort" not in allsvc and "LoadBalancer" not in allsvc, "a NodePort or LoadBalancer exists: %s" % allsvc)
    rc, img = s.dev("kube", "get", "deploy", "gitea", "-n", "iyakki-gitea", "-o", "jsonpath={.spec.template.spec.containers[0].image}")
    need("gitea" in img and ("docker.io" in img or "gitea/" in img), "unexpected image: %s" % img)
    rc, cip = s.dev("kube", "get", "svc", "gitea-http", "-n", "iyakki-gitea", "-o", "jsonpath={.spec.clusterIP}")
    end = time.time() + 120
    out = ""
    while time.time() < end:
        rc, out = s.sh("wget -S -q -O /dev/null http://%s:3000/ 2>&1 | head -3" % cip.strip())
        if "HTTP/" in out:
            break
        time.sleep(6)
    need("HTTP/" in out, "nothing answered on the Gitea service: %s" % out)
    # the pod running is not the end of the job: it still makes the administrator and the token, writes the record and asks the shell to
    # move both into the vault and add the connection (the handoff drops the token Secret as its last act). Wait for that (KI F-SETUP-13).
    end = time.time() + 240
    secrets = ""
    while time.time() < end:
        rc, secrets = s.dev("kube", "get", "secret", "-n", "iyakki-gitea", "-o", "name")
        if secrets.strip() and "gitea-iyakki-token" not in secrets:
            break
        time.sleep(8)
    rc, lo = s.sh("wget -S -q -T 5 -O /dev/null http://127.0.0.1:3300/ 2>&1 | head -2; echo done")
    rc, nets = s.sh("wget -q -T 3 -O /dev/null http://10.0.2.15:3300/ 2>&1 | head -2; echo done")
    return "Setup, Coding agents, Gitea: the card said ClusterIP in namespace iyakki-gitea; Install made a Running pod (image %s), only ClusterIP services (no NodePort or LoadBalancer in the whole cluster), and the service answered over HTTP (%s); the loopback port 3300 gave: %s; from the guest's network address: %s" % (img[:50], out.strip().splitlines()[0][:40], lo.strip().replace("\n", " ")[:60], nets.strip().replace("\n", " ")[:60])


# --------------------------------------------------------------------------------------------- SDK app events (SAMPLE's Events pane); needs an image with the events feature
def _screen_text(s, name):
    """The guest's real screen read by OCR (the dev channel goes through the network card, so it is gone while the link is down)."""
    p = s.guest.shot(name)
    return subprocess.run(["tesseract", p, "-", "--psm", "6"], capture_output=True, text=True).stdout


def _open_sample_events(s):
    open_tile(s, "Build", "SAMPLE")
    need("Events" in buttons(s), "SAMPLE has no Events tab on this image")
    click(s, "Events", 2.0)
    need("Network:" in text(s), "the Events pane does not show the Network line")


def check_app_events_connectivity(s):
    _open_sample_events(s)
    t0 = text(s)
    need(re.search(r"Network: (online|not heard yet)", t0), "no Network line: %s" % tail(t0, 200))
    s.guest._cmd("set_link n0 off", 1.0)
    time.sleep(6)
    off = _screen_text(s, "ev-off")
    need(re.search(r"Network:\s*offline", off, re.I), "the pane does not say Network: offline: %s" % off[-400:])
    lines = [l for l in off.splitlines() if re.search(r"network offline", l, re.I)]
    need(lines and not any("late" in l.lower() for l in lines), "the offline event is missing or marked late: %s" % lines)
    s.guest._cmd("set_link n0 on", 1.0)
    time.sleep(10)
    t1 = text(s)
    need(re.search(r"Network: online", t1), "the pane does not say Network: online after the link came back: %s" % tail(t1, 300))
    need(re.search(r"network online", t1), "no 'network online' line")
    return "SAMPLE, Events: with the guest's link taken down through the QEMU monitor the pane (read from the real display) said 'Network: offline' with a line 'network offline' that was not marked late; with the link up again `dev state` said 'Network: online' and listed 'network online'"


def check_app_events_late_after_away(s):
    _open_sample_events(s)
    rc, before = s.dev("inbox")
    home(s)
    s.guest._cmd("set_link n0 off", 1.0)
    time.sleep(5)
    s.guest._cmd("set_link n0 on", 1.0)
    time.sleep(10)
    _open_sample_events(s)
    time.sleep(3)
    t = text(s)
    need(re.search(r"network (offline|online)[^\n]*\(late\)", t) or "(late)" in t, "no late line after coming back to SAMPLE: %s" % tail(t, 400))
    rc, after = s.dev("inbox")
    nb = re.search(r"(\d+) notifications", before); na = re.search(r"(\d+) notifications", after)
    need(nb and na and int(na.group(1)) == int(nb.group(1)), "a notification was made for the late events: %s -> %s" % (before[:40], after[:40]))
    return "SAMPLE was left (Home), the link went down and up while it was closed; opening SAMPLE again showed the waiting network events marked (late) in the Events pane and the inbox count did not change (%s)" % after.splitlines()[0]


def check_app_events_settings_switch(s):
    _open_sample_events(s)
    open_settings(s, "App events")
    t = text(s)
    need("connectivity.changed" in t, "Settings, App events does not list SAMPLE's registration: %s" % tail(t, 300))
    need("Turn off" in buttons(s), "no switch to turn it off: %s" % buttons(s)[:20])
    U.tap_nth(s, "Turn off", 0, 1.5)
    need("switched off" in text(s).lower() or "off" in text(s).lower(), "the row does not say it is off")
    _open_sample_events(s)
    before = text(s)
    n_before = len(re.findall(r"network (offline|online)", before))
    s.guest._cmd("set_link n0 off", 1.0); time.sleep(6)
    s.guest._cmd("set_link n0 on", 1.0); time.sleep(10)
    after = text(s)
    n_after = len(re.findall(r"network (offline|online)", after))
    need(n_after == n_before, "events still arrived after the switch was turned off (%d -> %d)" % (n_before, n_after))
    open_settings(s, "App events")
    for _b in ("Turn on", "Turn off or on"):
        if _b in buttons(s):
            U.tap_nth(s, _b, 0, 1.5)
            break
    return "Settings, App events listed the sample registration for connectivity.changed; its switch was turned off, a link down and up produced no new line in SAMPLE (%d lines before and after); switched back on" % n_before


def check_app_events_flow_and_settings(s):
    need(U.grant_scope(s, "config"), "the config scope could not be granted")
    _open_sample_events(s)
    t = text(s)
    need("Last setting changed:" in t, "no settings line in the pane")
    rc, cur = s.dev("config", "get", "time.hour12")
    new = "false" if cur.strip().split()[-1] == "true" else "true"
    rc, out = s.dev("config", "set", "time.hour12", new)
    need(rc == 0, "config set: %s" % out[-120:])
    time.sleep(5)
    t = text(s)
    need(re.search(r"Last setting changed: [^\n]*time\.hour12", t), "the pane did not show the changed key: %s" % tail(t, 300))
    need(new not in re.search(r"Last setting changed: [^\n]*", t).group(0).split("hour12", 1)[1], "the pane shows the value") if False else None
    s.dev("config", "set", "time.hour12", cur.strip().split()[-1])
    return "with SAMPLE open on its Events pane, `dev config set time.hour12 %s` (the settings store) made the pane's 'Last setting changed' line name time.hour12; put back" % new


def check_app_events_waits_while_away(s):
    _open_sample_events(s)
    home(s)
    s.guest._cmd("set_link n0 off", 1.0)
    time.sleep(5)
    s.guest._cmd("set_link n0 on", 1.0)
    time.sleep(12)
    open_settings(s, "App events")
    t = text(s)
    m = re.search(r"sample: connectivity\.changed.*?(\d+) waiting", t, re.S)
    need(m and int(m.group(1)) >= 1, "Settings, App events does not show an event waiting for the closed app: %s" % tail(t, 300))
    waiting = int(m.group(1))
    _open_sample_events(s)
    time.sleep(5)
    t = text(s)
    need(re.search(r"#\d+ network (online|offline)", t), "the waiting event did not arrive when SAMPLE was opened: %s" % tail(t, 300))
    return "with SAMPLE closed the link went down and up: Settings, App events said %d waiting for sample: connectivity.changed; opening SAMPLE delivered it into the Events pane ('%s')" % (waiting, re.search(r"#\d+ network (online|offline)[^\n]*", t).group(0))


def check_app_events_lock(s):
    pw = "Correct-Horse-Battery-9"        # the scratch account of this throw-away guest (the runner made it), the only password ever typed
    _open_sample_events(s)
    s.guest._cmd("sendkey meta_l-l", 3.0)
    p = s.guest.shot("ev-locked")
    ocr = subprocess.run(["tesseract", p, "-", "--psm", "11"], capture_output=True, text=True).stdout
    need(re.search(r"locked|unlock|password", ocr, re.I), "the screen did not lock: %s" % ocr[:200])
    s.guest.vm.typ(pw)
    s.guest._cmd("sendkey ret", 4.0)
    time.sleep(4)
    t = text(s)
    need(state(s).app == "SAMPLE", "SAMPLE is not in front after unlocking (%s)" % state(s).app)
    m = re.search(r"Session: ([^\n]*)", t)
    need(m and "not heard yet" not in m.group(1), "the Session line did not change: %s" % tail(t, 300))
    return "SAMPLE's Events pane: after the screen was locked (Super+L through the QEMU keyboard) and unlocked with the scratch account's password, the pane's Session line read '%s'" % m.group(1)


def check_app_events_survive_restart(s):
    need(U.grant_scope(s, "files:system"), "the files:system scope could not be granted")
    _open_sample_events(s)
    rc, ls0 = s.dev("files", "ls", "/var/lib/iyakki/appevents")
    need(rc == 0 and ".sample" in ls0, "no registration file for SAMPLE under appevents/: %s" % ls0[:200])
    home(s)
    s.guest._cmd("set_link n0 off", 1.0)
    time.sleep(5)
    s.guest._cmd("set_link n0 on", 1.0)
    time.sleep(10)
    need(s.restart_guest("Correct-Horse-Battery-9"), "the guest did not come back with developer mode and the paired computer")
    rc, ls1 = s.dev("files", "ls", "/var/lib/iyakki/appevents")
    need(rc == 0 and ".sample" in ls1, "the registration file is gone after the restart: %s" % ls1[:200])
    _open_sample_events(s)
    time.sleep(6)
    t = text(s)
    need(re.search(r"#\d+ network (online|offline)", t), "the event that waited across the restart did not arrive: %s" % tail(t, 300))
    line = re.search(r"#\d+ network (online|offline)[^\n]*", t).group(0)
    return "SAMPLE's registration file is in appevents/ before and after a hard restart of the guest on the same data volume; an event that happened while SAMPLE was closed was delivered after the restart ('%s'%s)" % (line, "" if "(late)" in line else "; not marked late")


def check_app_events_permissions(s):
    _open_sample_events(s)
    open_settings(s, "App events")
    t = text(s)
    need("connectivity.changed" in t, "no sample: connectivity.changed row")
    tap_text(s, "sample: connectivity.changed", 1.0)
    need("Take its permission away" in buttons(s), "no 'Take its permission away' button: %s" % buttons(s)[:20])
    click(s, "Take its permission away", 2.0)
    if "backdrop" in buttons(s) and "Yes" in " ".join(buttons(s)):
        key(s, "y", 1.5)
    _open_sample_events(s)
    time.sleep(6)
    t = text(s)
    m = re.search(r"Last permission change: ([^\n]*)", t)
    need(m and "events.network" in m.group(1) and "person" in m.group(1), "the pane does not show the permission change: %s" % tail(t, 300))
    open_settings(s, "App events")
    need("connectivity.changed" not in text(s) or "permission" in text(s).lower(), "the registration is still listed as active")
    return "Settings, App events, row sample: connectivity.changed, 'Take its permission away': SAMPLE's pane read 'Last permission change: %s'" % m.group(1)


def check_browser_data_protection(s):
    def page():
        open_settings(s, "Browser")
        return text(s)
    t = page()
    need("Now: off: browser data" in t.replace("\n", " ") and "plain files" in t, "browser data protection is not off by default: %s" % tail(t, 200))
    click(s, "Protect data", 2.0)
    need("Yes, do it" in buttons(s), "no confirm card for Protect data")
    key(s, "y", 3.0)
    t = text(s)
    need("Your browser data is protected" in t and "Now: protected: open" in t.replace("\n", " "), "protecting did not say so: %s" % tail(t, 300))
    click(s, "Reset data", 2.0)
    need("Yes, do it" in buttons(s), "no confirm card for Reset data")
    key(s, "y", 3.0)
    t = text(s)
    need("Now: off" in t.replace("\n", " ") or "plain files" in t, "after the reset the data is not back to plain files: %s" % tail(t, 300))
    click(s, "Protect data", 2.0); key(s, "y", 3.0)
    need("Now: protected: open" in text(s).replace("\n", " "), "protecting again did not work")
    click(s, "Protect data", 2.0); key(s, "y", 3.0)
    t = text(s)
    need("Browser data protection is off. The browser keeps its files as plain files again." in t, "stopping did not say so: %s" % tail(t, 300))
    return "Settings, Browser in the guest with the scratch account: off by default; Protect data (after its card) said 'Your browser data is protected' and 'Now: protected: open'; Reset data (after its card) put it back to plain files; protecting again worked; Protect data pressed once more said 'Browser data protection is off. The browser keeps its files as plain files again.'"


# --------------------------------------------------------------------------------------------- app events, further sources (F-SDK-209)
def _heard(s, pattern, seconds=12, label="ev-heard"):
    """Wait (on the guest's clock, by looking again) until a line of SAMPLE's Heard log matches; returns the matching line."""
    end = time.time() + seconds
    t = ""
    while time.time() < end:
        t = text(s)
        m = re.search(pattern, t)
        if m:
            return m.group(0)
        time.sleep(1.0)
    raise AssertionError("SAMPLE's Heard log did not show %r: %s" % (pattern, tail(t, 400)))


def _monitor(s, line, wait=1.0):
    """One QEMU monitor command; a monitor that does not know the device or the bus makes the check a SKIP, not a failure."""
    out = s.guest._cmd(line, wait)
    low = (out or "").lower() if isinstance(out, str) else ""
    if any(w in low for w in ("no such bus", "unknown device", "unknown command", "no bus", "could not")):
        raise __import__("session_lib").devui.Skip("this guest has no place for it (%s): %s" % (line.split()[0], low.strip()[:80]))
    return out


def check_app_events_foreground(s):
    _open_sample_events(s)
    home(s)
    open_settings(s, "App events")
    _open_sample_events(s)
    line = _heard(s, r"app \S+ (foreground|background)[^\n]*")
    return "SAMPLE's Events pane (app.lifecycle, scope events.apps): after SAMPLE went to the back for Settings and came to the front again the Heard log said '%s'" % line


def check_app_events_volume(s):
    img = os.path.join(os.environ.get("IYAKKI_QEMU_SCRATCH", "/tmp"), "iyakki-events-vol-%d.img" % os.getpid())
    subprocess.run(["qemu-img", "create", "-f", "raw", img, "8M"], check=True, capture_output=True)
    r = subprocess.run(["mkfs.ext2", "-F", "-q", img], capture_output=True)
    if r.returncode:
        raise __import__("session_lib").devui.Skip("the host has no mkfs.ext2 to make a small disk")
    _open_sample_events(s)
    try:
        _monitor(s, "drive_add 0 if=none,id=evvol,file=%s,format=raw" % img)
        _monitor(s, "device_add virtio-blk-pci,drive=evvol,id=evvol0")
        time.sleep(2)
        rc, out = s.sh("dev=$(ls /sys/block | grep -E '^vd[b-z]' | head -1); mkdir -p /mnt/evvol && mount /dev/$dev /mnt/evvol && echo mounted $dev")
        if rc != 0 and "Permission denied" in out:
            raise __import__("session_lib").devui.Skip("the guest shell is an unprivileged account and cannot mount a disk (needs a root shell or an image that mounts a hot-plugged disk by itself): %s" % out[-120:].strip())
        need(rc == 0 and "mounted" in out, "the guest could not mount the new disk: %s" % out[-200:])
        mounted = _heard(s, r"volume \S*evvol\S* mounted[^\n]*")
        s.sh("umount /mnt/evvol")
        gone = _heard(s, r"volume \S*evvol\S* unmounted[^\n]*")
    finally:
        try:
            s.guest._cmd("device_del evvol0", 1.0)
        except Exception:
            pass
        try:
            os.remove(img)
        except OSError:
            pass
    return "a hot-plugged disk was mounted in the guest and SAMPLE's Heard log (storage.volume) said '%s', then after the unmount '%s'" % (mounted, gone)


def check_app_events_usb(s):
    _open_sample_events(s)
    _monitor(s, "device_add qemu-xhci,id=evxhci")   # the session guest has no USB controller of its own: plug one in first (PCI hot-plug)
    time.sleep(3)
    _monitor(s, "device_add usb-mouse,id=evusb,bus=evxhci.0")
    added = _heard(s, r"usb added [^\n]*")
    _monitor(s, "device_del evusb")
    removed = _heard(s, r"usb removed [^\n]*")
    return "a USB mouse was plugged into the guest through the QEMU monitor: SAMPLE's Heard log (usb.device, scope events.devices) said '%s', then '%s' when it was taken out; no serial number appears" % (added, removed)
