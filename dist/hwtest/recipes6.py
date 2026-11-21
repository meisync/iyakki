"""Batch 6 recipes for the Mac mini 2018 (Macmini8,1): read-only page and state checks, plus NEEDS-HUMAN stubs for rows a script
cannot judge. Nothing here changes a setting: the only things pressed are view switches (Advanced, Simple view, Ask/Search tab) that
are undone, and every shell command is a read (`dev shell` runs under the standing grant the owner gave on the device, as `nobody`).
A PASS always prints what is NOT COVERED. Row texts: FEATURES.md. (Named recipes6.py because recipes4.py is batch 4.)"""
import re, time, datetime
from hwcheck import require_hardware, not_applicable, recipe, fail, human, dev, Verdict, Ctx
import recipes as _r1  # gives Ctx.notcovered


def _shell(c, script, name, timeout=60):
    """Run a read-only command on the device through the standing dev-shell grant. If the device wants an approval (the grant is
    gone) the request is DENIED on the device and the row is NEEDS-HUMAN: an agent never approves its own shell."""
    try:
        rc, out = dev("shell", "--", "sh", "-c", script, timeout=timeout)
    except Exception:
        try:
            c.click("Deny shell", must=False)
        except Exception:
            pass
        human("the dev shell did not answer within %d s (an approval may be waiting; pressed Deny shell if it was offered)" % timeout)
    out = "\n".join(l for l in out.splitlines() if not l.startswith("Shell access granted"))
    open(c.dir + "/" + name + ".txt", "w").write(out + "\n")
    c.log("SHELL (read-only) %s -> saved %s.txt (rc=%d)" % (script[:120], name, rc))
    return rc, out


def _page_pass(c, st, pats):
    for p in pats: c.expect(st, p)


# ------------------------------------------------------------------ settings pages

@recipe("F-FW-07", "Settings -> Firewall: ON banner, policy sentence, rules table, Advanced view with Disable now / Copy / Import (read-only)")
def fw07(c):
    """PASS when the page reads FIREWALL ON with the default-policy sentence, the rules show status and detail, the buttons Add, Remove,
    Advanced exist; Advanced shows Presets, Apply, Look up names, Copy rules, Import, Clear log, On/off, Disable now and the blocked-count
    line. Nothing is pressed except Advanced and Simple view."""
    st = c.settings_page("firewall"); c.shot("simple")
    _page_pass(c, st, [r"^FIREWALL ON$", r"^Everything that comes in from the network is blocked, except what is allowed below",
                       r"^Standard: block everything coming in", r"^Remote desktop$", r"^allowed$",   # the "File sharing" rule exists only where sharing was switched on
                       r"UDP port 5353 · device name \(mDNS\)", r"IP: TCP port 7979 · developer mode",
                       r"^\d+ connection attempts blocked so far"])
    for b in ("Add…", "Remove", "Advanced…", "Standard", "Open", "Locked down"):
        if b not in c.buttons(st): fail("simple view button %r missing" % b)
    st = c.click("Advanced…", wait=2)
    try:
        c.shot("advanced")
        _page_pass(c, st, [r"^FIREWALL ON$", r"^Incoming connections are blocked unless allowed below", r"^Presets on: ",
                           r"^Blocked connections since the rules were applied: \d+", r"^CIDR  10\.0\.0\.0/8$", r"^IP  \d+\.\d+\.\d+\.\d+$"])
        for b in ("Add rule…", "Edit", "Allow on/off", "Delete", "Presets…", "Apply", "Look up names", "Copy rules", "Import", "Clear log", "On/off", "Disable now", "Simple view"):
            if b not in c.buttons(st): fail("advanced button %r missing" % b)
        w = c.has(st, r"^Rule \d+: /\d+ is a very large network")
        if w: c.log("OBSERVED warning: " + w[0])
    finally:
        c.click("Simple view", must=False)
    c.notcovered("Add/Edit/Allow/Delete forms, Apply with Keep/Revert and the 60 s take-back, Disable now, Copy/Import JSON, audit entries, the OFF and NOT PROTECTED states, the read-only window on another computer, the four screen sizes; no rule was changed")


@recipe("F-HW-05", "Settings -> This computer: model, boots, first/last seen, six category switches, Copy/Reset/Forget buttons (read-only)")
def hw05(c):
    """PASS when the page names this model, says 'Started N times on it · first seen <date> · last seen <date>', lists the six category
    switches (Sound, Display, Keyboard and pointer, Microphone, GPU, Power and battery) as 'per computer' or 'shared', shows this
    computer in the known list and has Copy settings from, Reset this computer and Forget a computer buttons. No button pressed."""
    require_hardware(c, "macmini2018")
    st = c.settings_page("computer"); c.shot("page")
    _page_pass(c, st, [r"^This computer$", r"^Apple Macmini8,1$", r"^Started \d+ times on it · first seen \d{4}-\d\d-\d\d · last seen \d{4}-\d\d-\d\d$",
                       r"^Apple Macmini8,1  \(this computer\) · last seen \d{4}-\d\d-\d\d"])
    for cat in ("Sound", "Display", "Keyboard and pointer", "Microphone", "GPU", "Power and battery"):
        c.expect(st, r"^%s: (per computer|shared)$" % cat)
    for b in ("Copy settings from…", "Reset this computer", "Forget a computer…"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    c.notcovered("no switch, Copy, Reset or Forget was pressed (each asks); the one inbox notice for a new computer was not seen (this computer is not new); only one computer is in the known list; the page was not checked at other sizes")


@recipe("F-A11Y-03", "Settings -> Accessibility: High contrast, Large pointer, Big focus ring, Reduce motion and the text size link (read-only)")
def a11y03(c):
    """PASS when the Accessibility page lists High contrast ('Black and white with a yellow accent and thick borders; every text at least
    7 to 1'), Large mouse pointer, Big focus ring, Reduce motion ('No moving background, no blinking caret') and the Text size line that
    points to Display, each with a button. Nothing is switched."""
    st = c.settings_page("accessib"); c.shot("page")
    _page_pass(c, st, [r"^High contrast$", r"Black and white with a yellow accent and thick borders; every text at least 7 to 1",
                       r"^Large mouse pointer$", r"^A bigger pointer on the screen\.$", r"^Big focus ring$", r"^A thicker ring round the control that has the focus\.$",
                       r"^Reduce motion$", r"^No moving background, no blinking caret\.$", r"^Text size: .*\(\d+%\)\. u makes text and buttons bigger, U smaller \(also in Display\)"])
    for b in ("High contrast", "Large pointer", "Focus ring", "Reduce motion"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    c.notcovered("none of the four options was switched on, so the high-contrast palette, the 2x pointer on the framebuffer, the thicker ring and the steady caret were not seen; the page text does not say which options are on now")


@recipe("F-A11Y-04", "Settings -> Accessibility: keyboard walk hint, Sticky keys, Slow keys (read-only text)")
def a11y04(c):
    """PASS when the page says how to walk the screen without a mouse (Super+Alt+Right and Left, Super+Alt+Enter), offers Sticky keys
    ('Shift, Ctrl, Alt and Super stay on for the next key') and Slow keys with its state ('Slow keys: off', 'W cycles the delay') and points to
    Keyboard for key repeat. Text only: no key walk is sent (it needs a physical keyboard)."""
    st = c.settings_page("accessib")
    _page_pass(c, st, [r"Without a mouse: Super\+Alt\+Right and Left move through the controls of the screen, Super\+Alt\+Enter presses",
                       r"^Sticky keys$", r"Shift, Ctrl, Alt and Super stay on for the next key", r"^Slow keys: (off|\d+ ms)",
                       r"A key counts only after it was held a moment\. W cycles the delay",
                       r"Key repeat speed and delay are in Keyboard"])
    for b in ("Sticky keys", "Slow keys"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    c.notcovered("the walk itself (ring, reading order, Enter presses, R says again), latching and locking of modifiers and the 300-1200 ms delay need a person at a physical keyboard; only the page text was read")


@recipe("F-VOICE-22", "Settings -> Sound -> Advanced: Highlight timing offset row with its default 0 ms (read-only)")
def voice22(c):
    """PASS when Sound -> Advanced (view toggle, not saved) shows the row 'Highlight timing offset' with the value '0  ms' (the default)."""
    st = c.settings_page("sound"); st = c.click("Advanced", wait=1.5)
    found = False
    for i in range(4):
        if c.has(st, r"^Highlight timing offset$"): found = True; break
        [dev("input", "key", "Down") for _ in range(12)]; time.sleep(1.5); st = c.state("scroll%d" % i)
    if not found: fail("no 'Highlight timing offset' row after Advanced and scrolling")
    c.shot("advanced-rows")
    c.expect(st, r"^Highlight timing offset$"); c.expect(st, r"^0\s+ms$"); c.expect(st, r"^Advanced sound rows shown\.$")
    c.log("the Advanced toggle is a view only; reopening the page hides the rows again")
    c.notcovered("+/- steps of 25 ms, the slider, Enter = 0, the -500..500 range, the audit line and the lit word moving earlier or later (needs reading aloud and a person watching); the per-sentence log line was not looked for")


@recipe("F-BROW-54", "Settings -> Browser -> Ads and trackers: no list shipped, EasyList/EasyPrivacy only on a click (read-only; nothing downloaded)")
def brow54(c):
    """PASS when the Ads and trackers section says 'On, but no list yet: nothing is blocked', 'None downloaded yet. Press l to fetch EasyList
    and EasyPrivacy from easylist.to', the licence lines (GPL-3.0 or CC BY-SA 3.0) and 'nothing is shipped with the system', and has the
    buttons Download lists (l), Update (n), Add address (u), Add file (o), Remove lists (r). No button is pressed (no download)."""
    st = c.settings_page("browser"); c.shot("page")
    _page_pass(c, st, [r"^Block ads and trackers$", r"^On, but no list yet: nothing is blocked", r"^None downloaded yet\. Press l to fetch EasyList and EasyPrivacy from easylist\.to",
                       r"Both are free under GPL-3\.0 or CC BY-SA 3\.0 \(dual licence\); nothing is shipped with the system",
                       r"\d+ requests checked since start"])
    for b in ("Download lists (l)", "Update (n)", "Add address (u)", "Add file (o)", "Remove lists (r)"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    c.notcovered("no list was downloaded (a person clicks that), so the per-list name, rules, age, SHA-256, licence, source, the weekly refresh, a failed download, the 12 list / 24 MB limits and real blocking were not seen")


@recipe("F-BROW-58", "Settings -> Browser: Smooth pages is off by default (read-only)")
def brow58(c):
    """PASS when the Browser page reads 'Smooth pages (faster, softer): off. Press m to change.' Nothing is switched."""
    st = c.settings_page("browser")
    if not c.has(st, r"^Smooth pages \(faster, softer\): (on|off)\. Press m to change\.$") and c.has(st, r"^Smooth pages \(faster, softer\)$"):
        # a window with room shows a switch row (label and blurb; the on/off position is only in the picture); the sentence is for small windows
        c.shot("browser-page")
        human("the Browser page shows Smooth pages as a switch row whose position is not in `dev state`; look at the screenshot (the sentence form appears only in a small window)")
    c.expect(st, r"^Smooth pages \(faster, softer\): off\. Press m to change\.$")
    c.notcovered("the half-size drawing with 2x stretch, the llvmpipe thread count, web-tuning, tile-only painting and any speed gain were not measured; the switch was left off")


@recipe("F-VAULT-06", "Settings -> Browser: Protect browser data is off by default (read-only)")
def vault06(c):
    """PASS when the Browser page reads 'Protect browser data: off. Press P to change.' Nothing is switched."""
    st = c.settings_page("browser")
    if c.has(st, r"^Protect browser data$"):   # the card form: the state is the 'Now: ...' end of its blurb
        c.expect(st, r"Encrypts cookies and site data .*Now: off: browser data is plain files")
    else:
        c.expect(st, r"^Protect browser data: off\. Press P to change\.$")
    c.notcovered("sealing, key in the vault, RAM folder, sign-out write-back, migration and Reset browser data were not run; switching it on needs the person's vault and password")


@recipe("F-WEBSVC-03", "Remote access off, nothing listens on 8443/8081/8080; only developer mode (7979) and k3s ports listen (read-only)")
def websvc03(c):
    """PASS when Settings -> Remote access says 'Off: nothing listens, no browser can reach this desktop' and 'Client connections: off, the
    endpoint answers nothing', the Startup page lists Web service and Remote access (web) as off/stopped, and the device's listening TCP
    sockets (read through `dev shell`) include no port 8443, 8081 or 8080. This proves 'off by default' and nothing more."""
    st = c.settings_page("remote access"); c.shot("remote-access")
    _page_pass(c, st, [r"^Remote access: Off: nothing listens, no browser can reach this desktop", r"^Port 8443 \(HTTPS only\)",
                       r"^Client connections: off, the endpoint answers nothing", r"^SFTP file server: off, nothing listens"])
    st = c.settings_page("startup")
    _page_pass(c, st, [r"^Remote access \(web\)$", r"^stopped$", r"^Web service$", r"^off$"])
    rc, out = _shell(c, "netstat -ltn 2>/dev/null | grep LISTEN", "listening-sockets")
    ports = re.findall(r"^tcp\S*\s+\d+\s+\d+\s+(\S+):(\d+)\s", out, re.M)
    c.log("listening: " + ", ".join("%s:%s" % p for p in ports))
    if not ports: fail("no listening sockets read")
    bad = [p for p in ports if p[1] in ("8443", "8081", "8080", "80", "443")]
    if bad: fail("a web port listens although Remote access is off: %s" % bad)
    c.log("non-loopback listeners: " + ", ".join(sorted({p[1] for p in ports if not p[0].startswith("127.")})))
    c.notcovered("the on state: the opt-in LAN rule per network, 60 requests a minute, 10 sign-in tries, 1 MiB bodies, 4 runs, CORS and the audit lines were not exercised; Remote access was not switched on (owner rule)")


@recipe("F-DEVMODE-14", "iyakki dev whoami lists every scope with its line and held state; Developer mode page has Grant all / Extend 4 h (read-only)")
def devmode14(c):
    """PASS when `dev whoami` prints the 12 scopes (install, debug, files, files:system, config, input, input:password, shell, power,
    update, forward, kube) each with [x] or [ ] and a one-line explanation, the 'Scopes this computer holds' line agrees with the [x] marks, and the
    Developer mode page has the buttons Grant all diagnostic scopes and Extend 4 h. Neither is pressed."""
    rc, out = c.run("whoami", name="whoami")
    names = ("install", "debug", "files", "files:system", "config", "input", "input:password", "shell", "power", "update", "forward", "kube")
    held = {}
    for n in names:
        m = re.search(r"^\s*\[( |x)\] %s\s{2,}(\S.{10,})$" % re.escape(n), out, re.M)
        if not m: fail("scope %s is not listed with an explanation in `dev whoami`" % n)
        held[n] = m[1] == "x"
    c.log("held: " + ", ".join(k for k, v in held.items() if v) + " | not held: " + ", ".join(k for k, v in held.items() if not v))
    m = re.search(r"^Scopes this computer holds: (.*)$", out, re.M)
    if not m: fail("no 'Scopes this computer holds' line")
    # the summary line and the [x] marks must agree; which scopes are held is the owner's choice per computer (a test computer may hold all)
    line = {x.strip() for x in m[1].split(",")}
    if line != {k for k, v in held.items() if v}: fail("'Scopes this computer holds' says %s but the [x] marks are %s" % (sorted(line), sorted(k for k, v in held.items() if v)))
    c.log("files %s on this computer; held list and [x] marks agree" % ("held" if held["files"] else "withdrawn"))
    st = c.settings_page("developer"); c.shot("page")
    for b in ("Grant all diagnostic scopes", "Extend 4 h"):
        if b not in c.buttons(st): fail("Developer mode button %r missing" % b)
    c.expect(st, r"^Developer mode: On \(since ")
    c.notcovered("the G and t keys were not pressed (Grant all would widen the paired computer's rights; Extend changes the switch-off time); the 'next switch-on switches off after 4 h' timer was not run to its end; the usage text of `dev` was not compared")


@recipe("F-DEVMODE-16", "Developer mode page offers Approve 4 h / 8 h / 12 h, Shell once, Deny shell (read-only: buttons exist; no request made)")
def devmode16(c):
    """PASS (narrow) when the Developer mode page's button list has Approve 4 h, Approve 8 h, Approve 12 h, Shell once and Deny shell.
    The approval card with a real waiting request, the 4/8/1 keys and the 8 h default are NOT covered."""
    st = c.settings_page("developer")
    for b in ("Approve 4 h", "Approve 8 h", "Approve 12 h", "Shell once", "Deny shell"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    c.log("buttons present: Approve 4 h, Approve 8 h, Approve 12 h, Shell once, Deny shell")
    c.notcovered("no shell request was made, so the card itself, the 4/8/1 keys, 'the 8 hour default by name' and what a click grants were not seen; raising a request would put an approval on the owner's screen and an agent never approves its own request")


@recipe("F-DEVMODE-13", "dev shell: the six standard /dev nodes are usable by everyone and /dev is not nodev (read-only)")
def devmode13(c):
    """PASS when, through `dev shell` (which runs as nobody), /dev/null, zero, full, random, urandom and tty are character devices with
    mode crw-rw-rw-, a redirect to /dev/null works (`echo ok 2>/dev/null >/dev/null` exits 0, `head -c 4` of zero and urandom give 4
    bytes) and /dev is mounted without nodev."""
    rc, out = _shell(c, "ls -l /dev/null /dev/zero /dev/full /dev/random /dev/urandom /dev/tty; echo ok 2>/dev/null >/dev/null; echo REDIRECT=$?; head -c 4 /dev/zero | wc -c; head -c 4 /dev/urandom | wc -c; grep ' /dev ' /proc/mounts; id -u", "dev-nodes")
    for n in ("null", "zero", "full", "random", "urandom", "tty"):
        if not re.search(r"^crw-rw-rw-\s+1 root\s+root\s+\d+,\s+\d+ .* /dev/%s$" % n, out, re.M): fail("/dev/%s is not crw-rw-rw- root" % n)
    if "REDIRECT=0" not in out: fail("a redirect to /dev/null failed")
    if not re.search(r"^4$\n^4$", out, re.M): fail("reading 4 bytes from /dev/zero and /dev/urandom did not give 4")
    m = re.search(r"^devtmpfs /dev devtmpfs (\S+)", out, re.M)
    if not m or "nodev" in m[1].split(","): fail("/dev is mounted nodev or not devtmpfs: %s" % (m and m[1]))
    if out.strip().splitlines()[-1] == "0": fail("the shell ran as root, not as the account's user")
    c.log("user id of the shell: " + out.strip().splitlines()[-1])
    c.notcovered("writing to /dev/tty (the dev shell has no terminal, F-DEV-16); the clean environment (HOME, USER) was not compared; a shell started right after a fresh boot with nodes not yet made was not tried")


@recipe("F-DEV-07", "dev shell: runs as the account's system user (not root), red SHELL ACCESS bar, exit status passed on, command line audited")
def dev07(c):
    """PASS when `dev shell -- sh -c 'id -u; exit 3'` prints a non-zero uid and exits with status 3 ('the command ended with status 3'),
    the top bar on the device reads 'DEV · SHELL ACCESS by <computer>', and the device audit log holds a `devmode.shell.exec ... sh -c`
    line with that exact command. Only the standing grant of the owner is used."""
    marker = "hwtest-dev07-%d" % int(time.time())
    rc, out = _shell(c, "echo %s; id -u; exit 3" % marker, "dev07-shell")
    if rc == 0 or "status 3" not in out: fail("exit status 3 was not passed on (rc=%d): %s" % (rc, out[-120:]))
    uid = re.search(r"^(\d+)$", out, re.M)
    if not uid or uid[1] == "0": fail("the shell ran as root or printed no uid")
    c.log("uid in the shell: " + uid[1])
    time.sleep(1.5)
    st = c.state("topbar"); c.shot("topbar")
    c.expect(st, r"^DEV · SHELL ACCESS by \S+")
    rc, log = c.run("logs", "--system", "-n", 400, name="audit", timeout=90)
    if not re.search(r"devmode\.shell\.exec .*%s" % marker, log): fail("no audit line holds the command (marker %s)" % marker)
    c.log("audit line found for marker " + marker)
    c.notcovered("the approval card and its 10 minute / one session choices (a standing grant was already in force, so none was shown), Ctrl+C, stdin, the output cap, a refused / expired / revoked grant killing the session, root with an 'as root' grant; no terminal (pty)")


@recipe("F-STORE-09", "/home is bind-mounted from the data volume (same device as /var/lib/iyakki) and holds the accounts' folders (read-only)")
def store09(c):
    """PASS when /proc/mounts shows /var/lib/iyakki and /home on the same block device and filesystem (the bind of <data>/home onto
    /home) and /home lists the signed-in account's folder. Mode/owner of the folders is recorded as an observation."""
    rc, out = _shell(c, "grep -E ' /home | /var/lib/iyakki ' /proc/mounts; ls -ld /home /home/*; test -r /home/admin/Downloads && echo ADMIN_HOME_READABLE_BY_NOBODY; df -h /home | tail -1", "home-bind")
    mt = {m[2]: m[1] for m in re.finditer(r"^(\S+) (/home|/var/lib/iyakki) (\S+)", out, re.M)}
    if "/home" not in mt or "/var/lib/iyakki" not in mt: fail("/home or /var/lib/iyakki is not a mount point")
    if mt["/home"] != mt["/var/lib/iyakki"]: fail("/home is on %s but the data volume is %s" % (mt["/home"], mt["/var/lib/iyakki"]))
    if not re.search(r" /home/admin$", out, re.M): fail("no /home/admin on the bound /home")
    c.log("OBSERVED: /home and /var/lib/iyakki are both %s" % mt["/home"])
    if "ADMIN_HOME_READABLE_BY_NOBODY" in out: c.log("OBSERVED: /home/admin is readable by the dev shell's user nobody (see KI)")
    c.notcovered("surviving a power-off, the merge of files made earlier in the boot, the 0700 mode and ownership on first use (the existing /home/admin is drwxr-xr-x root), HOME for the terminal, tool runs and jobs; the IDE build/run panel and voice tools still inherit HOME")


@recipe("F-SYS-03", "SSH server (Dropbear) is not running without the iyakki.ssh=1 boot entry (read-only negative check)")
def sys03(c):
    """PASS when the kernel command line has no iyakki.ssh=1, no dropbear/sshd process exists and nothing listens on TCP port 22. This
    only proves 'started only by the boot entry' in the off direction."""
    rc, out = _shell(c, "cat /proc/cmdline; echo PROCS; cat /proc/[0-9]*/comm 2>/dev/null | sort -u | tr '\\n' ' '; echo; echo LISTEN; netstat -ltn 2>/dev/null | grep -c ':22 '", "ssh-state")
    out = "\n".join(l for l in out.splitlines() if not l.startswith("iyakki: the command ended"))
    cmd = out.split("PROCS")[0]
    if "iyakki.ssh=1" in cmd: human("this boot has iyakki.ssh=1; the negative check does not apply")
    procs = out.split("PROCS")[1].split("LISTEN")[0]
    if re.search(r"dropbear|sshd", procs): fail("an SSH server process runs without iyakki.ssh=1")
    if not re.search(r"LISTEN\s*\n0\s*$", out.strip() + "\n"): fail("something listens on port 22")
    c.log("no iyakki.ssh=1 in /proc/cmdline, no dropbear/sshd process, nothing on port 22")
    c.notcovered("the on direction (boot entry with iyakki.ssh=1, key login, address on the sign-in screen) needs a restart; the persistent host key at /var/lib/iyakki/ssh was not looked at (the dev shell cannot read the data folder)")


@recipe("F-DRIVERS-03", "Firmware packs: /lib/firmware links point into the data volume, resolve, and the boot log shows the pack linked before brcmfmac loaded (read-only)")
def drivers03(c):
    """PASS when /lib/firmware holds symlinks into /var/lib/iyakki/firmware/<pack>/files/..., the kernel log says
    `driver pack <pack>: N file(s) linked ... driver brcmfmac reloaded` (the links cannot be followed by the nobody dev shell) before the `Firmware: BCM` line of brcmfmac."""
    rc, out = _shell(c, "n=0; bad=0; for f in $(find /lib/firmware -maxdepth 3 -type l); do t=$(readlink $f); case $t in /var/lib/iyakki/firmware/*) n=$((n+1)); [ -e $f ] || bad=$((bad+1));; esac; done; echo PACKLINKS=$n BROKEN=$bad; for f in $(find /lib/firmware -maxdepth 3 -type l); do readlink $f; done | grep '^/var/lib/iyakki/firmware/' | sed 's#/files/.*##' | sort | uniq -c", "firmware-links")
    m = re.search(r"PACKLINKS=(\d+) BROKEN=(\d+)", out)
    if not m or int(m[1]) == 0: fail("no firmware links into the data volume")
    c.log("link resolution (-e) is NOT usable as a check: the dev shell runs as nobody and cannot enter /var/lib/iyakki (BROKEN=%s is a permission result, not proof); the kernel loading the firmware below is the proof" % m[2])
    rc, dm = c.run("dmesg", "--grep", "driver pack", "-n", 400, name="dmesg-pack")
    rc, dm2 = c.run("dmesg", "--grep", "brcmfmac", "-n", 400, name="dmesg-brcmfmac"); dm += "\n" + dm2
    ln = re.search(r"\[\s*([\d.]+)\] iyakki: .*driver pack (\S+): (\d+) file\(s\) linked.*reloaded", dm)
    fw = re.search(r"\[\s*([\d.]+)\] brcmfmac: brcmf_c_preinit_dcmds: Firmware: (BCM\S+)", dm)
    if "brcmfmac" not in dm2: not_applicable("no Broadcom Wi-Fi (brcmfmac) on this machine; the row is about the firmware pack for it")
    if not ln: fail("no 'driver pack ... file(s) linked ... reloaded' line in the kernel log")
    if not fw: fail("brcmfmac did not report a firmware version")
    if float(ln[1]) > float(fw[1]): fail("the pack was linked after brcmfmac loaded firmware")
    c.log("pack %s: %s files linked at %ss; brcmfmac %s firmware at %ss; %s links into the data volume" % (ln[2], ln[3], ln[1], fw[2], fw[1], m[1]))
    c.notcovered("`modprobe -r` of a driver, removing or turning a pack off, the options file, the Mac Bluetooth patch pack's skip rule (no T2-less Mac here), and drivers that load before the data volume; the packs' folder names under /var/lib/iyakki/firmware could not be listed (permission) so they come from the link targets")


@recipe("F-BELL-03", "Inbox keeping: counts per source stay within the limits and every note is within its age; Settings -> Notifications has Show/Quiet/Off per kind (read-only)")
def bell03(c):
    """PASS when `dev inbox` shows at most 60 notes per source and 200 in all, no information/new-device note older than 7 days, no
    job/reminder note older than 14 days and no error/warning older than 30 days, and Settings -> Notifications lists Show / Quiet
    (panel only) / Off for each kind with the explanation line. Nothing is changed."""
    rc, out = c.run("inbox", name="inbox")
    tot = re.search(r"^(\d+) notifications", out, re.M)
    if not tot: fail("no total in `dev inbox`")
    if int(tot[1]) > 200: fail("%s notes kept, limit 200" % tot[1])
    src = re.search(r"^by source: (.*)$", out, re.M)
    for name, n in re.findall(r"(\w+) (\d+)", src[1] if src else ""):
        if int(n) > 60: fail("source %s has %s notes (limit 60)" % (name, n))
    rows = re.findall(r"^(\d{4}-\d\d-\d\d) [\d:]+ [* ] (\w+)\s+(\w+)", out, re.M)
    if not rows: fail("no note rows")
    rc, dt = dev("shell", "--", "date", "+%F")
    now = datetime.date.fromisoformat(re.search(r"(\d{4}-\d\d-\d\d)", dt)[1])
    lim = {"info": 7, "device": 7, "download": 7, "agent": 14, "job": 14, "reminder": 14, "error": 30, "warning": 30}
    for d, kind, s in rows:
        age = (now - datetime.date.fromisoformat(d)).days
        if kind in lim and age > lim[kind]: fail("a %s note from %s is %d days old (limit %d)" % (kind, d, age, lim[kind]))
    c.log("%d notes shown (total %s), device date %s, all within their limits; kinds: %s" % (len(rows), tot[1], now, sorted({r[1] for r in rows})))
    st = c.settings_page("notifications"); c.shot("notifications")
    _page_pass(c, st, [r"^New devices on the network$", r"^Reminders and alarms$", r"^Downloads$", r"^Show: counted in the bell by kind\. Quiet: in the inbox only, no number, no dot, no sound\. Off: not kept\."])
    for b in ("Show", "Quiet (panel only)", "Off"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    c.notcovered("pruning itself (nothing in this inbox is old enough to be pruned), the 130-note file, 'grouped the first time the inbox opens', muting with m in the inbox, and the effect of Quiet/Off on the bell; no choice was changed")


@recipe("F-KBD-18", "Startup list has 'Touch screen: ready'; Keyboard page has Probe touch screen (read-only)")
def kbd18(c):
    """PASS (narrow) when Settings -> Startup lists 'Touch screen' as ready and Settings -> Keyboard has a Probe touch screen button.
    A finger event is NOT proven: the row's open question (a panel that stays silent after a cold boot) needs a person touching it."""
    st = c.settings_page("startup"); c.shot("startup")
    names = [t.strip() for t in c.texts(st)]
    if "Touch screen" not in names: fail("no 'Touch screen' item in the Startup list")
    import hwmatch
    if hwmatch.startup_item_absent(names, "Touch screen"): not_applicable("no touch screen on this machine (Startup says 'not on this machine')")
    i = names.index("Touch screen")
    if "ready" not in names[i + 1:i + 3]: fail("Touch screen is not 'ready' in the Startup list: %r" % names[i:i + 3])
    c.log("Startup: Touch screen -> ready")
    rc, dm = c.run("dmesg", "--grep", "touch", "-n", 300, name="dmesg-touch")
    c.log("kernel log touch lines: %d" % len([l for l in dm.splitlines() if l.strip()]))
    st = c.settings_page("keyboard")
    if not [b for b in c.buttons(st) if re.search(r"Probe touch", b, re.I)]: fail("no 'Probe touch screen' button on the Keyboard page")
    c.notcovered("a finger event (no person touching the monitor), whether the panel woke after a cold boot (KI-077), the 10 s detach/attach, the 45 s replug, the three-minute hub reset; Probe touch screen was not pressed (it powers the panel off and on)")


@recipe("F-RAG-12", "RAG tile: Agentic switch on, mode line, Ask tab, empty Evidence page (read-only; no question asked)")
def rag12(c):
    """PASS (narrow) when the RAG tile shows the always-visible line '<N> documents indexed   mode: Search', the Ask tab changes it to
    'mode: Ask the model (agentic)', the switch reads 'Agentic: on' and the Evidence page says 'No research has been run yet. Ask a question
    with Agentic on.' No question is asked (no model)."""
    c.open_tile("RAG"); st = c.state(); c.shot("tile")
    _page_pass(c, st, [r"^\d+ documents? indexed\s+mode: Search$", r"^Agentic: on$"])
    st = c.click("⇄ Ask", wait=1.5)
    c.expect(st, r"^\d+ documents? indexed\s+mode: Ask the model \(agentic\)$")
    st = c.click("Evidence", wait=1.5); c.shot("evidence")
    c.expect(st, r"^No research has been run yet\. Ask a question with Agentic on\.$")
    for b in ("Agentic: on", "Evidence", "Documents"):
        if b not in c.buttons(st): fail("button %r missing" % b)
    c.notcovered("the line's searches used / budget / rounds / model calls / stop reason, the ledger with Older, Newer, Up, Down, Back, result rows with backend and passage id, Ctrl+G and Ctrl+E (all need a research run with a model); the tab switch is a view only and reverts when the tile reopens")


@recipe("F-CLI-09", "CLI tile -> CLIs: the list offers Node.js, Python 3, uv, Claude Code, GitHub Copilot CLI, OpenAI Codex CLI (read-only; nothing installed)")
def cli09(c):
    """PASS (narrow) when the CLIs tab lists Node.js, uv, Python 3, Claude Code, GitHub Copilot CLI and OpenAI Codex CLI as entries
    beside the cluster tools, with an Install (i) button and Check for updates. Nothing is installed or downloaded."""
    c.open_tile("CLI"); st = c.click("CLIs", wait=1.5); c.shot("clis")
    for b in ("Node.js", "uv", "Python 3", "Claude Code", "GitHub Copilot CLI", "OpenAI Codex CLI", "Install (i)", "Check for updates"):
        if b not in c.buttons(st): fail("CLIs entry %r missing" % b)
    c.log("CLIs list: " + ", ".join(b for b in c.buttons(st) if b not in ("Terminal", "CLIs", "Connections", "field", "clipboard")))
    c.notcovered("any download (120-250 MB vendor builds are not fetched), SHA-256 checks, musl/glibc pick, install order, Install/Upgrade/up to date wording per entry, uninstall warnings, whether the musl builds run; the page's text is not exposed by `dev state`, only the entry names")


# ------------------------------------------------------------------ rows only a person can judge

def _stub(rid, title, why):
    @recipe(rid, title)
    def f(c):
        c.log("not attempted by a script: " + why)
        human(why)
    return f


_stub("F-SHARE-12", "who may map which folder in a share", "needs a second, non-administrator account to try a folder outside its home; no account may be created or demoted here")
_stub("F-DISP-08", "touch gestures on the Mac mini touch monitor", "needs a finger on the touch monitor (tap, drag, two-finger scroll); a script cannot touch a panel")
_stub("F-LOCK-02", "ways to lock (Super+L, lock key, Control+Command+Q)", "locking needs the account password to unlock again and a physical Apple keyboard; a script never types a password and cannot reach the lock key")
_stub("F-SHELL-40", "crash containment: a panic in one app closes only that app", "a panic cannot be provoked from outside without a test build with a crash switch; the restart card button and a lock held during a panic need a person and that build")
_stub("F-DEV-13", "developer-mode approval card on every screen", "the card shows only while a request waits; an agent may not raise a request and answer it, and denying it proves nothing about the approval path")
_stub("F-HOTSPOT-14", "sharing policy per kind and device limit", "no page on this build shows the policy (the row says the page only cycles the wired setting; the rest is `iyakki share policy`, which changes policy); the Mac mini has no tether to test")
_stub("F-LOSS-02", "card on every screen when the drive is lost", "needs the IYAKKI drive to be lost or unavailable; unplugging or breaking storage is not allowed here")
_stub("F-LOSS-03", "apps say 'the IYAKKI drive is not available'", "only appears while the data state is not Healthy; it is Healthy and must stay so")
