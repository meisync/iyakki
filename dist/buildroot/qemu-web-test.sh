#!/bin/bash
# Boot the optional "web" image variant in QEMU and open pages with the embedded full web engine (the WPE WebKit helper), saving screenshots.
# Works for the macweb image too: ./qemu-web-test.sh target/buildroot-macweb (QEMU has no Mac hardware: it checks the engine, not the Mac drivers).
# See qemu-web-test.py (this is a thin wrapper):   ./qemu-web-test.sh [build-dir] [out-dir]
#   QEMU="sudo qemu-system-x86_64"  URLS="..."  WAIT=40  SSH_KEY=~/.ssh/iyakki_diag.pub
exec python3 "$(dirname "$0")/qemu-web-test.py" "$@"
