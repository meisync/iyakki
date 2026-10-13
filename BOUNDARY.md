# Where IYAKKI ends and the base OS begins

**IYAKKI** is the code in `crates/` plus the files we add under `dist/buildroot/` (config, overlay, scripts).
**The base OS** is everything we do not write: the Linux kernel, the Buildroot-built userland (BusyBox, kmod,
iproute2, wpa_supplicant, alsa-utils, e2fsprogs, CA certificates), the bootloader (GRUB, later shim) and
firmware. Keeping the line sharp is what lets us change either side without breaking the other, keeps our code
Apache-2.0, and keeps the attack surface small.

## Rules

1. **One direction.** IYAKKI depends on the base OS through the interfaces listed here. The base OS knows
   nothing about IYAKKI internals: it starts `iyakki` as PID 1, passes kernel command-line options, and our files
   live in `/etc/iyakki` and `/var/lib/iyakki`.
2. **Only four kinds of interface:** (A) the kernel ABI (syscalls, `/proc`, `/sys`, `/dev`, ioctls); (B) named
   base programs started as separate processes with fixed arguments and **never through a shell**; (C) kernel
   command-line options; (D) files we own. Anything else is a boundary violation.
3. **No patches to upstream.** The kernel is a stock version plus a config fragment
   (`dist/buildroot/board/iyakki/linux.fragment`). Buildroot is used unmodified through an external tree
   (`dist/buildroot`). We do not fork or patch base programs. If the base lacks something, first change
   configuration, then add a separate optional package; patching comes last, and not silently.
4. **GPL stays at arm's length.** Base programs are only executed, never linked, so IYAKKI remains Apache-2.0
   (see `LICENSING.md`).
5. **Every touchpoint is listed below and enforced.** `crates/iyakki-core/tests/boundary.rs` fails when non-test
   code outside a listed file starts a process or makes a system-level libc call, and when a listed file no
   longer needs its row. To touch the base OS from a new place: add the row (with the reason) in the same change.
6. **Containers.** `iyakki` also runs in Docker and K3s without the base OS pieces. Code that needs a real boot
   (mounting, driver loading, rebooting) must only run when IYAKKI is PID 1 on a real boot (`boot.rs`), never in a container.
7. **Prefer the kernel ABI to programs.** Each program in table B is a candidate to replace with direct syscalls
   when that is simple and safe (noted per row).

8. **The base OS is replaceable.** Because the interface is this short and listed, a person can swap the base OS for
   another one: a different Linux distribution (Alpine, Debian, Ubuntu, a Yocto or Buildroot image of their own), a
   different kernel build, or a container host. What the replacement must offer is exactly tables A to D: the Linux
   kernel ABI we call, the named ordinary programs (all common on any distribution), the boot options if IYAKKI is PID 1,
   and a place for `/etc/iyakki` and `/var/lib/iyakki`. A kernel that is not Linux would need the files in table A
   ported; that table is the porting list. Nothing outside these tables depends on the base OS, and the test above keeps
   it that way. Only the Buildroot image and a Linux desktop have been tried so far (`F-PKG-14` tracks the others).

## A. Kernel ABI (system-level calls)

| File | Calls | Purpose |
|---|---|---|
| `crates/iyakki-core/src/boot.rs` | `mount`, `sync`, `reboot`, `sethostname` | Early boot: pseudo filesystems, hostname, safe power-off and reboot. |
| `crates/iyakki-core/src/lib.rs` | `sigaction`, `waitpid` | Shutdown signals; reaping orphaned children as PID 1. |
| `crates/iyakki-storage/src/lib.rs` | `mount`, `umount`, `statvfs`, `sync` | Mount the IYAKKI-DATA volume, free-space figures. |
| `crates/iyakki-shell/src/fb.rs` | `ioctl`, `mmap`, `munmap` | Framebuffer (`/dev/fb0`) geometry and pixel output. |
| `crates/iyakki-shell/src/input.rs` | `ioctl`, `poll` | Keyboard, mouse and touch devices (`/dev/input/event*`). |
| `crates/iyakki-shell/src/backend.rs` | `ioctl` | Switch the text console to graphics mode while the shell runs. |
| `crates/iyakki-devctl/src/lib.rs` | `ioctl`, `socket`, `close`, `getifaddrs`, `freeifaddrs` | Settings: bring a network link up or down, list IPv4 addresses. Radios, brightness and detection use plain `/sys` files. |
| `crates/iyakki-sec/src/lib.rs` | `getrandom` | Operating-system entropy. |
| `crates/iyakki-agent/src/voice.rs` | `kill` | Stop a recording/playback process and its children. |
| `crates/iyakki-agent/src/cluster.rs` | `mount`, `chroot`, `unshare`, `kill` | Give the container cluster (k3s) a mount namespace whose root is a tmpfs when the system runs from RAM, so containers can start; stop it. |
| `crates/iyakki-agent/src/webengine.rs` | `kill` | Optional full web engine, legacy hand-over (images that have `cage` and `cog` but no embedded helper): stops the compositor and everything it started (its process group): SIGTERM, then SIGKILL after 3 s, when the person leaves it or the system shuts down. |
| `crates/iyakki-agent/src/sysinfo.rs` | `kill` | The SYSTEM tile and `iyakki top` stop (SIGTERM) or kill a process the administrator picks; process 1 and the shell itself are refused. It also reads `/proc` and `/sys` (processor, memory, processes, network, heat). |
| `crates/iyakki-agent/src/ollama.rs` | `kill` | Stops the Ollama server that IYAKKI started (SIGTERM to its process group, SIGKILL after ten seconds). |

Read-only use of `/dev/kmsg` (the boot screen's loading log), `/dev/urandom` (random bytes for page scripts), `/proc` and `/sys` (no system-level calls) is also part of this interface: `iyakki-hwinfo`
(hardware report), `iyakki-core/src/net.rs` (interface list) and `iyakki-storage` (disk list).

## B. Base programs (started as separate processes)

| File | Program (Buildroot package) | Why | Replace with |
|---|---|---|---|
| `crates/iyakki-core/src/boot.rs` | `modprobe` (kmod), `mdev` (BusyBox) | Load drivers by hardware alias; create device nodes. | `finit_module(2)` and our own device broker (F-BOOT-04). |
| `crates/iyakki-devctl/src/lib.rs` | `amixer` (alsa-utils), `udhcpc` (BusyBox) | Settings: sound volume and mute; ask for an address after a link is switched on. | ALSA control ioctls; our own DHCP client. |
| `crates/iyakki-core/src/boot.rs` | `dropbear`, `ip` | Remote diagnostics over SSH, only when the boot entry sets `iyakki.ssh=1`: keys only, no passwords. | keep; SSH is not ours to write. |
| `crates/iyakki-core/src/net.rs` | `ip` (iproute2), `wpa_supplicant` | Bring up Ethernet and WiFi from `/etc/iyakki/net.conf`. | netlink for `ip`; keep `wpa_supplicant` (WPA is not ours to write). |
| `crates/iyakki-core/src/host.rs` | `ip` | Report network status. | netlink. |
| `crates/iyakki-core/src/main.rs` | `ip`, `arecord` (alsa-utils) | Boot self-test output only. | none needed (diagnostic). |
| `crates/iyakki-storage/src/lib.rs` | `mke2fs` (e2fsprogs) | Create the IYAKKI-DATA ext4 volume. | keep; filesystem creation is not ours to write. |
| `crates/iyakki-agent/src/voice.rs` | user-configured commands, default `arecord` / `aplay` | Microphone and speaker; speech engines such as whisper-cli, piper. | any command the admin chooses; no shell is involved. |
| `crates/iyakki-agent/src/exec.rs` | `ssh`, `aws`, `gcloud` (only these) | Lets the assistant run them for the person, after approval: no shell, cut-down environment, time and output limits. | any program the person already uses; the list is fixed in code. |
| `crates/iyakki-agent/src/clis.rs` | the installed tool's `--version`, `python3` (Azure CLI only) | Reports a freshly installed command-line tool's version; makes the Python environment the Azure CLI is installed into. | nothing to replace (tools are the vendors' programs); downloads and unpacking are done by our own code. |
| `crates/iyakki-agent/src/cluster.rs` | `k3s` (server, kubectl), `kill` | Starts the container cluster as a background process of IYAKKI, runs code in Jobs through its kubectl, and stops it with SIGTERM (then SIGKILL after 30 s). | k3s itself; no replacement planned. |
| `crates/iyakki-agent/src/ollama.rs` | `ollama serve` | Starts the local Ollama server (installed from the CLI tile) as a background process listening on 127.0.0.1 only, with its models in IYAKKI's data folder. | Ollama itself; no replacement planned. |
| `crates/iyakki-devctl/src/wifi.rs` | `iw`, `ip`, `wpa_cli`, `wpa_supplicant`, `udhcpc` | Settings → Network: scan for WiFi networks, join one (writes the supplicant's configuration into `/run`, starts or re-reads the supplicant, asks for an address). | the same ordinary programs the boot-time network setup already uses. |
| `crates/iyakki-agent/src/webengine.rs` | `cage`, `cog` | Optional full web engine, legacy hand-over (only used when the image has them but no embedded helper, `docs/FULL-BROWSER.md`): `cage -s -- cog --platform=wl <url>` runs WPE WebKit in a kiosk Wayland compositor as separate processes while the shell lets go of the screen and keyboard; profile in `/var/lib/iyakki/web-profile/<user>`. Refused for hosts the account may not open. | the vendors' programs; no replacement planned. |
| `crates/iyakki-agent/src/webengine.rs` | `udevd`, `udevadm` (eudev) | Optional full web engine: the compositor's input library reads device data from udev, so `udevd` is started (once) when the full browser starts and every present device is described to it. | none; udev data is how libinput finds keyboards and mice. |
| `crates/iyakki-webview/src/client.rs` | the web engine's helper program: `/usr/libexec/iyakki/webview-helper` (WPE WebKit, `web` image variant), or on a development desktop `dist/webview/host-helper.py` (WebKitGTK; it starts `Xvfb` itself for video) | The browser's full engine (`docs/FULL-BROWSER.md`, `docs/WEBVIEW-PROTOCOL.md`): one helper per tab renders the page offscreen, the shell draws its frames in the page area. Started without a shell as `helper --socket-fd 0 --shm FILE` (the socket is its standard input; pixels travel through a file in `/dev/shm`); an address is only ever sent as a protocol message, never as an argument. Stopped with a Quit message, then killed after 2 s. | the vendors' engines; no replacement planned. |
| `crates/iyakki-agent/src/mcp.rs` | user-configured MCP servers | External tool servers the user adds. | n/a (user's choice, permission-gated). |

## C. Kernel command-line options we read

`iyakki.selftest` (`1`, `persist1`, `persist2`), `iyakki.shell` (`0` disables the shell), `iyakki.hwcheck` (`1` starts
the hardware check), `iyakki.ssh` (`1` starts the SSH server for diagnostics; key login only), `iyakki.web` (`1`, a port, `host:port` or `http:port` serves the desktop to browsers; also `IYAKKI_WEB` in a container), `iyakki.splash` (`N` keeps the boot screen up for at least N seconds), `iyakki.data` (`ram` ignores the data volume), `iyakki.webtest` (test only, `qemu-web-test.sh`: an address to open in the full web engine at boot; use with `iyakki.shell=0`), `iyakki.disk` (self-test disk), `iyakki.env`
(force environment detection). Boot entries live in `dist/buildroot/board/iyakki/grub.cfg`.

## D. Files and directories IYAKKI owns

`/etc/iyakki/` (`net.conf`, `ai.conf`, `mcp.json`, read-only config shipped in the image overlay),
`/var/lib/iyakki/` (accounts, the devices seen on the network (`devices.json`), settings, vaults, knowledge base, chats, runs, skills, audit log, the browser front end's `tls/` certificate, `ollama/` models and log, `wifi.json` (the WiFi networks joined from Settings); this is the only
place IYAKKI writes; it is the IYAKKI-DATA volume when one exists), `/run/iyakki*` (scratch). IYAKKI never edits other
base files.

## E. Host tools (run on the user's own computer, not part of the IYAKKI image)

`iyakki-usb` writes an image to a USB stick. It is a separate program with the same discipline: the base-OS
interface is small and listed here.

| File | Calls | Purpose |
|---|---|---|
| `crates/iyakki-usb/src/drives.rs` | `ioctl` | Ask the kernel to re-read a stick's partition table after writing (Linux). |
| `crates/iyakki-usb/src/term.rs` | `poll`, `ioctl` | Single key presses without echo for the arrow-key menu; terminal width. |
| `crates/iyakki-usb/src/macos.rs` | `diskutil`, `mount` (spawned) | List external disks and unmount them on macOS. |
