# Boot-test image: QEMU + OVMF + Python 3 (standard library only). Rebuild only when this file changes.
# Base: Debian 12 slim (public, DFSG packages). Everything below is fetched from the Debian archive at build time
# and is a TEST TOOL inside the CI container; none of it is shipped in an IYAKKI image or release.
#   qemu-system-x86 (GPL-2.0 and others)  the emulator, used as a separate program
#   qemu-utils      (GPL-2.0)             qemu-img: a throw-away overlay so the image under test is never written
#   ovmf            (BSD-2-Clause-Patent) UEFI firmware (the .fd files under /usr/share/OVMF)
#   python3         (PSF)                  runs boot-test.py; no third-party Python packages
FROM debian:bookworm-slim
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      qemu-system-x86 qemu-utils ovmf python3 \
 && rm -rf /var/lib/apt/lists/*
COPY boot-test.py /usr/local/bin/boot-test.py
ENTRYPOINT ["python3", "/usr/local/bin/boot-test.py"]
