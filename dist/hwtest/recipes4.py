"""Batch 4: a USB keyboard on the Mac mini (read-only)."""
import re
from hwcheck import recipe, fail, human, dev, Ctx, not_applicable


@recipe("F-KBD-usb", "a USB keyboard appeared: kernel log, USB notice, Keyboard page (read-only)")
def kbd_usb(c):
    """PASS when the kernel log has an `input: ... USB Keyboard` line with its hid-generic bind, the shell logged `usb: device ...
    appeared: <vid>:<pid> USB Keyboard`, and Settings -> Keyboard still reads Input profile / Function keys / Key repeat / Layout.
    Always ends NEEDS-HUMAN: typing feel, repeat rate accepted by the kernel and key mapping need a person at the keyboard."""
    dm = dev("dmesg", "--grep", "Keyboard", "-n", 600)[1]
    open(c.dir + "/dmesg-keyboard.txt", "w").write(dm)
    m = re.findall(r"input: (.*USB Keyboard) as .*/(\d{4}:[0-9A-F]{4}:[0-9A-F]{4})\.", dm)
    if not m: not_applicable("no USB keyboard was plugged in during this boot (nothing to check)")
    c.log("keyboard: %s" % (m[-1],))
    if not re.search(r"hid-generic .*: input,hidraw\d+: USB HID v[\d.]+ Keyboard", dm): fail("no hid-generic bind line")
    if not re.search(r"iyakki INFO \] usb: device [\d.-]+ appeared: [0-9a-f]{4}:[0-9a-f]{4} USB Keyboard", dm): fail("the shell did not log the USB device")
    st = c.settings_page("keyboard"); c.shot("keyboard-page")
    for pat in (r"^Input profile$", r"^Function keys$", r"^Key repeat$", r"^Layout$", r"^US keyboard"):
        c.expect(st, pat)
    c.expect(st, r"^Apple keyboard driver$"); c.expect(st, r"not loaded \(no Apple keyboard here")
    c.notcovered("the page does not list the USB keyboard by name (its 'detected controls' are the touch screen, brightness and keyboard light); no key from the real keyboard was read by a script")
    human("keyboard %s is seen by the kernel and the shell; repeat rate accepted by the kernel (EVIOCSREP), Ctrl/Alt/Super/Cmd mapping and typing feel need a person typing on it" % (m[-1][0],))
