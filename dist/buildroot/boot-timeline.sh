#!/bin/sh
# Boot timeline of the machine it runs on, from the kernel log. Works over SSH on a booted IYAKKI (BusyBox sh) and on a PC.
#
#   sh boot-timeline.sh            # prints the table
#   ssh admin@HOST 'sh -s' < dist/buildroot/boot-timeline.sh      (boot entry "remote diagnostics", iyakki.ssh=1)
#
# Times are the KERNEL clock (seconds since the kernel started). Everything BEFORE the kernel clock starts (the firmware, GRUB
# reading the kernel and the initial RAM disk from the stick) is invisible here; it is the wall time between choosing the menu
# entry and the first sign of life on the screen: measure it with a stopwatch (docs/BOOT-SPEED.md, "Stopwatch").
# Nothing is written; only /proc and dmesg are read.

line() { dmesg 2>/dev/null | grep -m1 -- "$1"; }
secs() { printf '%s' "$1" | sed -n 's/^[^0-9]*\[ *\([0-9.]*\)\].*/\1/p'; }
row() { # label pattern
  l="$(line "$2")"
  if [ -n "$l" ]; then printf '%-34s %9s s\n' "$1" "$(secs "$l")"; else printf '%-34s %9s\n' "$1" "-"; fi
}

echo "== IYAKKI boot timeline (kernel clock) =="
echo "kernel      $(uname -r)    cmdline: $(cat /proc/cmdline 2>/dev/null)"
printf 'uptime now  %s s\n' "$(cut -d' ' -f1 /proc/uptime)"
echo "-- what GRUB had to read --"
l="$(line 'Freeing initrd memory')"
[ -n "$l" ] && echo "initrd, as unpacked by the kernel: $(printf '%s' "$l" | sed -n 's/.*memory: \([0-9]*\)K.*/\1/p' | awk '{printf "%.1f MB", $1/1024}') (size of the archive GRUB read)"
for f in /boot/bzImage /boot/initrd /mnt/boot/bzImage /mnt/boot/initrd /run/iyakki/boot/initrd /run/iyakki/boot/root.sfs; do
  [ -f "$f" ] && ls -l "$f" | awk '{printf "%s %.1f MB\n", $NF, $5/1048576}'
done
dmesg 2>/dev/null | grep -m1 "stage1: root" | sed 's/^[^]]*\] *//' | sed 's/^/lean boot: /'
echo "-- kernel --"
row "kernel start (first line)"      "Linux version"
row "initramfs unpack started"       "Unpacking initramfs"
row "initramfs unpacked (freed)"     "Freeing initrd memory"
row "PID 1 starts (Run /init)"       "Run /init as init process"
row "stage 1 found the root"         "stage1: root"
echo "-- IYAKKI (critical path to the sign-in screen) --"
row "splash: first drawing"          "splash: "
row "boot summary (to login)"        "boot (to login)"
row "sign-in screen: first picture"  "shell: first picture on screen"
row "supervisor up"                  "supervisor up"
echo "-- after the sign-in screen (must not block it) --"
row "boot summary (after login)"     "boot (after login)"
row "sound card appeared"            "snd_hda\|sound card\|ALSA"
row "network lease"                  "udhcpc: lease"
row "time synchronised (NTP)"        "ntp\|NTP\|time sync\|clock set"
row "k3s start"                      "k3s"
echo "-- IYAKKI's own summary lines --"
dmesg 2>/dev/null | grep "boot (to login)\|boot (after login)\|data: storage search\|data: mount took\|coldplug:\|stage1:" | sed 's/^/  /'
echo "-- gaps of more than 1 s between consecutive kernel log lines --"
dmesg 2>/dev/null | awk '{ if (match($0, /\[ *[0-9.]+\]/)) { t=substr($0, RSTART+1, RLENGTH-2)+0; if (t-prev > 1.0) printf "  +%.1f s before: %s\n", t-prev, substr($0, 1, 110); prev=t } }'
