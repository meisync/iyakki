# Troubleshooting

**For:** anyone whose IYAKKI is not doing what the guides say.
**You will get:** the problems we actually hit, each with symptom, cause and fix, and what to send when you report one.

Quick map: [Window mode](#window-mode) · [Models and chat](#models-and-chat) · [Kubernetes and deploy](#kubernetes-and-deploy) ·
[Browser](#browser) · [Sound, voice, TTS](#sound-voice-and-speech) · [Hardware, boot, network](#hardware-boot-and-network) ·
[Accounts and audit](#accounts-vault-and-audit) · [Building and testing](#building-and-testing) · [What to send](#what-to-send-with-a-report)

## Window mode

**The window does not open (Wayland or X11).** The window runs on both. From a terminal run `./run.sh window` and read the last
lines. On a Wayland-only session without XWayland, or odd GPU drivers, try `WINIT_UNIX_BACKEND=x11` or `=wayland` in front of the
command (standard winit variable; unverified for this build). A build error on a fresh machine usually means a missing desktop
library: install the Wayland/X11, xkbcommon and OpenGL development packages for your distribution.

**"IYAKKI is already running on this data folder (process N). Two windows share accounts and settings; continue anyway?"**
You started a second window on the same data folder. **Quit** unless you mean it; two windows are safe (changes are re-read before
each save) but confusing. A stale warning after a crash clears itself: the lock is ignored when its process is gone.

**"This workspace is open in another IYAKKI window"** (AI IDE). The workspace lock is held by another window, person or computer.
Choose **Take over** (the other window turns read-only), **Open read-only**, or Cancel. A crashed window frees the lock at once;
a vanished computer frees it after 90 seconds.

**The window crashed or froze.** A panic is caught and logged to `<data folder>/crash.log` (`~/.local/share/iyakki-dev/crash.log`)
and the window carries on. See [What to send](#what-to-send-with-a-report).

**Start from scratch:** `rm -rf ~/.local/share/iyakki-dev` (deletes accounts, chats, vaults).

## Models and chat

**"No model" / the chat says it is not configured.** Settings -> Models: add a provider, set it as the chat model (`m`), `c` to
test. Ollama on this machine is found at sign-in; on another machine use NET SCAN -> `a`. See [`QUICKSTART.md`](QUICKSTART.md#2-connect-a-model).

**The model does not respond.** Check, in order: the provider's address reachable from this machine (the Settings -> Models test
`t`), the model name exists on the server (`ollama list`), a local model may be loading into memory (first answer takes much longer),
cloud key missing or wrong (`k`), the account's allowed network hosts (an administrator limited `net_hosts` in USERS -> `h`), proxies.
Every failure ends in a visible message; the chat never stops silently.

**A thinking model (qwen3, deepseek-r1) shows "Thinking..." for a long time.** The thinking block counts the seconds and can be
opened. A reply that is only thinking is retried once with thinking off, and a model silent for three minutes ends the run with a
message. In agent mode IYAKKI turns thinking off for tool turns to be fast; if answers get worse, start with `IYAKKI_THINK=1`
([`ASSISTANT-ROBUSTNESS.md`](ASSISTANT-ROBUSTNESS.md#thinking-models)).

**"The assistant says it cannot use tools / cannot access websites."** Agent mode must be on (`Ctrl+G`) and your role must allow it.
A wrong refusal is retried once automatically. Models without tool calling use a plain-text protocol and are less reliable.

**Approvals never come up.** Open the INBOX (`F9`), Approvals page: every waiting call is there. Not answering means deny.

## Kubernetes and deploy

**kubeconfig confusion: the IDE talks to the wrong cluster, or `$KUBECONFIG` is ignored.** By design a workspace uses its own
cluster: the one chosen for the workspace, else your earlier choice, else this machine's k3s, then `$KUBECONFIG`, then
`~/.kube/config`; children do not inherit your `KUBECONFIG`, and `kubectl` run by the assistant or the IDE terminal is pinned to the
workspace's cluster (the terminal prints `kubectl -> <server> (workspace X)`). Fix: AI IDE -> K8s -> **Cluster...** (`c`), pick the
cluster, or use **Disconnect this workspace**. A remote cluster that only came from `$KUBECONFIG` is read-only for the assistant
until you choose it. Settings -> Dev cluster lists every profile. Details: [`K3S-DEV.md`](K3S-DEV.md#which-cluster-a-workspace-uses-no-leaks-from-kubeconfig).

**"kubeconfig exists but cannot be read" (root-only k3s.yaml).** IYAKKI never runs `sudo`. Copy it once:
`sudo cp /etc/rancher/k3s/k3s.yaml ~/.kube/k3s.yaml && sudo chown $USER ~/.kube/k3s.yaml`, then press Cluster... and choose it.

**Deploy is greyed out.** Reasons shown on hover: no cluster answers, the workspace is read-only, your account may not run programs,
or the cluster looks like production (a name containing `prod`; the assistant only reads there).

**Docker or podman missing.** The Build step needs `podman` (or `docker`). Install one (`sudo apt install podman`). The pre-flight
also checks the Dockerfile: "COPY package*.json matches no file" means the Dockerfile and the app files are not in one folder.

**Registry pull errors (HTTP vs HTTPS).** `ImagePullBackOff` with "server gave HTTP response to HTTPS client": the cluster tries
HTTPS on a plain-HTTP registry. Fixes, in order of ease: use the in-cluster registry (K8s -> `g` Registry... -> option A, no root,
pods pull `localhost:30500/...`); or on a single local node write `/etc/rancher/k3s/registries.yaml` with the `http://` mirror
(the dialog and the pod diagnosis print the exact text and `sudo` commands for you to run, then restart `k3s` or `k3s-agent`); for
Docker pushing to a plain-HTTP registry add it to `insecure-registries` in `/etc/docker/daemon.json` and restart docker. IYAKKI never
edits `/etc`. `x509` errors mean an untrusted certificate (`ca_file`), "no basic auth credentials" means a login is needed.

**Pods in `CreateContainerConfigError` ("runAsNonRoot ... non-numeric user").** Add a numeric `USER 10001` after installing in the
Dockerfile, rebuild with a new tag, redeploy. The diagnosis says so.

**"This machine already manages its own cluster" / no Set up button.** IYAKKI never starts a second cluster; it found k3s, rke2 or a
running kubelet. In a window it never installs one. See [`K3S-DEV.md`](K3S-DEV.md).

## Browser

**A page looks empty or broken.** The built-in reader handles articles and simple pages; web apps need the full engine: press `F`
in BROWSER (needs the helper: WebKitGTK `python3-gi gir1.2-webkit2-4.1 gir1.2-gtk-3.0` on a desktop, or the `web` image). `j` allows
JavaScript for the site; `c` shows what was blocked. Video needs `xvfb` on a desktop or the `web` image; sites that only offer
H.264 will not play video unless the image was built with `--h264`. If the helper is missing, `F` says so.

**Text shows as boxes.** Chinese, Japanese, Korean and emoji are not drawn by the built-in engine (Indic, Arabic and Hebrew are).

**The assistant "cannot use my browser".** The browser agent is off or read-only for your account (USERS -> `w`), or you have not
approved the site; passwords and codes are never typed by the model.

## Sound, voice and speech

**No sound.** Open Settings -> Sound and read the first row: it names the output IYAKKI plays on and why, or why there is none.
* *no sound output: connect a monitor with speakers over HDMI/DP or a USB audio device; built-in speakers on T2 Macs need the apple-bce
  driver of the Mac image*: the machine has only HDMI/DP outputs (Mac mini 2018 and other T2 Macs: devices 3, 7 and 8 of `HDA Intel PCH`,
  no device 0) and nothing with speakers is plugged in. Plug the monitor in (it is used within a few seconds) or add a USB audio device.
  On a T2 Mac, the Mac image adds card `Apple T2 Audio` (built-in speaker, headphone jack); if `cat /proc/asound/cards` lacks it,
  `dmesg | grep -iE 'apple-bce|aaudio'` says why (docs/MAC-HARDWARE.md section 6).
* An output is shown but you hear nothing: press `t` (Test sound) on each output row; choose the one that sounds with `Enter` or
  `o`. Check the monitor's own volume and input; the row says when the monitor reports no speakers or when the digital output is
  switched off in the mixer.
* **IYAKKI volume** muted or low: `m`, `+`. Each output keeps its own volume. When the output has no hardware volume (HDMI on
  Intel, the T2 Mac's speaker and headphones) IYAKKI writes `/etc/asound.conf` with an ALSA softvol device made `default`, so
  the slider moves every program's sound (media players too); its control is `IYAKKI` on the output's card. Cards with a
  hardware mixer also have their own row (the `web` image's mixer starts muted; over SSH `amixer sset Master 80% unmute`).
* **The volume slider changes nothing.** Over SSH: `cat /etc/asound.conf` (first line `# Written by IYAKKI` and the output in
  use, e.g. `plughw:1,0`; a file without that line is someone else's and IYAKKI leaves it, and keeps scaling only its own
  sound), `amixer -c 1 sget IYAKKI` (`Unable to find simple control` until the device was opened once: play something, or
  `aplay -D iyakki -t raw -f S16_LE -r 48000 -c 2 /dev/zero` for a moment), `amixer -c 1 sset IYAKKI 30%` to try by hand. A
  program that opens `plughw:C,D` itself bypasses the software volume and, while it plays, blocks the shared output.
* **No sound in the browser (YouTube) while the radio or a media file plays, or the reverse** (`Device or resource busy`):
  the output is shared through a `dmix` in `/etc/asound.conf` (`type dmix` under `pcm.iyakkimix`). If the file instead has
  `slave.pcm "plughw:C,D"`, the dmix could not be opened on that device and IYAKKI fell back to the exclusive one (look for
  `dmix on card C device D could not be opened` in the log); `fuser -v /dev/snd/pcmC*D*p` shows who holds the device. A
  program holding the raw device from before the file was written also blocks it: stop it or restart IYAKKI.
* Over SSH: `cat /proc/asound/cards`, `aplay -l`, `amixer -c 0 contents | grep -A2 Jack` (which jacks are on),
  `cat /proc/asound/card0/eld#*` (monitor_present, monitor_name), and `aplay -D plughw:0,3 some.wav` to try one device by hand.
  ALSA's `default` is card 0 device 0, which HDMI-only machines do not have; IYAKKI makes `default` (and `iyakki`) the output in
  use in `/etc/asound.conf`, and passes `-D iyakki` (or `-D plughw:C,D` where it cannot write the file) to `aplay` itself.

A VM has only a virtual sound card. RADIO plays MP3 streams only. Settings -> Voice -> `p` is the speaker test of the voice setup.

**TTS missing: TALES / read-aloud does not speak.** No text-to-speech command is configured (or the program is not installed).
Settings -> Voice -> `v` set the TTS command, `y` test. Without an engine the story is still written and saved. Dictation (`Ctrl+R`)
needs a speech-to-text command (`s`). MEDIA needs mpv, VLC, ffplay or mplayer for formats other than MP3.

## Hardware, boot and network

Boot problems, keys, UEFI, black screen: [`INSTALL.md`](INSTALL.md#8-boot-problems). Run the hardware-check boot entry first on any
new machine.

### No boot logo

*Symptom:* the stick boots and the desktop appears, but you never saw the IYAKKI logo and progress bar (on an Intel Mac mini
it was reported from the stick over Ethernet). Everything below is about the first seconds only.

1. **Look at the menu first.** The GRUB menu is now drawn over the IYAKKI logo. If you see a plain text menu, the firmware gave
   GRUB no graphics mode (or the image has no GRUB font); the boot still works, the logo then first appears when the kernel's
   screen does.
2. **Keep the boot screen up long enough to see it.** Edit the boot entry (press `e` at the menu) and add `iyakki.splash=8`: the
   boot screen then stays at least 8 seconds (at most 30). On a fast machine the desktop used to replace it within a second.
3. **Send us the story.** On the desktop open the Terminal tile, or SSH in (`iyakki.ssh=1`), and run
   `dmesg | grep -i splash`, plus `cat /var/log/iyakki-boot.log` and `ls /sys/class/graphics /sys/class/drm`. The lines say when the
   framebuffer appeared (`fb0 appeared 1840 ms after the boot screen was requested`), its geometry and format
   (`drawing on /dev/fb0: simpledrm 2560x1440 32 bpp stride 10240 (R16/8 G8/8 B0/8)`), whether the screen changed under it
   (`screen changed ... following it`, which is the GPU driver taking over from the firmware's screen), and why it stopped
   (`stopped after 214 frames in 10.7 s (boot finished)` or `boot finished after 900 ms before any framebuffer appeared`).
4. **Try the other entries.** *safe graphics* keeps the firmware's simple framebuffer and skips the GPU drivers: if the logo
   shows there, the GPU driver (i915 on a Mac mini 2014) switching the screen is the cause. *show boot messages* shows kernel text.
5. **Booted in the old BIOS way?** An Intel Mac lists the stick twice in the Option menu ("EFI Boot" and a "Windows" style entry).
   Pick **EFI Boot**: through the legacy entry there is no framebuffer until the GPU driver loads.

Why it can be missing on real hardware while QEMU shows it (ranked by how likely each is; the investigation is in
[`MAC-HARDWARE.md`](MAC-HARDWARE.md#boot-screen-and-no-boot-logo-investigation)):
the GPU driver replaces the firmware screen and the splash is already over; GRUB gave the kernel a text mode (BIOS) so no screen
exists early; the desktop replaced the splash within a second; an odd stride or pixel format; `quiet` leaves the screen black until
the splash starts.

### Flicker or "no signal" for many seconds at start

*Symptom:* GRUB's logo shows, then the monitor says "no signal" (sometimes several times) before the sign-in screen. Seen on a Mac
mini 2018 on HDMI. Each time the GPU driver turns the screen off and on, a monitor may need seconds to lock on again.

1. **Try *IYAKKI - safe graphics*.** It keeps the firmware's screen (`nomodeset`, no GPU driver): no mode change at all. If that
   is calm, the GPU driver's mode changes are the cause.
2. **Mac mini 2018 / Coffee Lake:** use the Mac image built from a tree with the CDCLK kernel patch (see
   [`MAC-HARDWARE.md`](MAC-HARDWARE.md#mac-mini-2018-macmini81-blank-screen-and-flicker-at-start)).
3. **Monitor never shows up until you re-plug it:** IYAKKI probes the connectors again for 20 s and on every display event;
   `dmesg | grep display:` says what it found. If it gives up, add `video=DP-3:e` (your connector name from
   `ls /sys/class/drm`) to the boot entry with `e` at the menu.
4. **Send:** `dmesg | grep -iE "splash|display:|i915|drm"`, and for a deep look boot once with `drm.debug=0x106` added to the entry.

### USB webcam: no camera, or the camera status stays red

*Symptom:* a USB webcam is plugged in but no `/dev/video*` appears (Devices shows no camera). Mac minis have no built-in camera,
so this is the usual camera there.

1. **Look at the log:** `dmesg | grep -E "camera:|uvcvideo|UVC"`. IYAKKI applies `uvcvideo` quirks by itself for cameras it knows
   and, for others, retries once with quirk `0x100` and then `0x2` when the kernel reports `Failed to query (129) UVC probe
   control : -32` or `Failed to set UVC probe control : -110`. A line like `camera: 1d6c:0103 needed uvcvideo quirk 0x100` means
   it worked.
2. **`still has no video device after uvcvideo quirks ...` or `can't set config #1, error -110`:** the camera has stopped
   answering. Unplug it and plug it in again (or reboot with it plugged in: IYAKKI loads `uvcvideo` and `snd-usb-audio` itself, after a
   known camera's microphone and video are held back, so its first probe has the quirk).
3. **By hand** (as root, `1-8` being the camera's USB port from `dmesg`): `cat /sys/module/uvcvideo/parameters/quirks` (note
   it), `echo 0x100 > /sys/module/uvcvideo/parameters/quirks`, `echo 1-8:1.0 > /sys/bus/usb/drivers/uvcvideo/bind`, then write
   the noted value back. If that works, send us the camera's `idVendor:idProduct` (`lsusb`) so it can go in the known table.
   Details: [`MAC-HARDWARE.md`](MAC-HARDWARE.md#mac-mini-2018-usb-webcam).

**Wi-Fi on Macs.** The Mac mini 2014's built-in BCM4360 has no open driver and no image we ship contains Broadcom's: use Ethernet, a USB Wi-Fi
adapter with an open driver (MediaTek MT7612U or MT7921AU or MT7601U, Atheros AR9271), or build Broadcom's non-free `wl` yourself
on a Linux PC (`sh dist/drivers/build-wl-pack.sh --accept-broadcom-licence`) and install the signed pack from a USB stick (STORE ->
Drivers -> Broadcom BCM4360 Wi-Fi; steps in [`MAC-HARDWARE.md`](MAC-HARDWARE.md) section 4). Other Broadcom chips need a firmware file you supply
(STORE -> Drivers) or the `hw`/`mac` image. Apple silicon cannot boot a USB stick; use a VM ([`MAC-HARDWARE.md`](MAC-HARDWARE.md)).

**No Wi-Fi on a PC.** The default image carries no Wi-Fi firmware. Use Ethernet, or a USB tether, or a build with `hw`. Wi-Fi joining
(Settings -> Network -> `w`) has never been tested against a real access point.

**DHCP reservation or router rule does not match / the router shows a strange address.** Reservations, parental controls and
"known device" lists are keyed on the MAC address. IYAKKI keeps the hardware address on Ethernet by default (mode **Off**), so a
reservation made for the Mac mini's real address works; Wi-Fi is **Per network** by default, so the router sees a different,
made-up address (the second character of its first pair is 2, 6, a or e, for example `a2:...`) that is the same each time but not the
one printed on the adapter. For a reservation on Wi-Fi either reserve the address shown in Settings -> Network (it is stable for
that network) or set Wi-Fi to **Off** (`m`, or `iyakki net mac off --wifi`) and reconnect. **Random each connection** can never be
reserved. Changing the mode applies on the next connection; `a` applies it now. If a change is refused ("not supported by this
driver") that adapter always shows its hardware address.

**Data is gone after a reboot.** You booted the live ISO or `iyakki.data=ram`. Install to the stick (`iyakki-usb --install`) or
make a data disk in Settings -> Storage (it asks you to type the disk name and erases it).

**Bluetooth shows "no answer" (Mac mini 2018 and other T2 Macs).** The kernel made `hci0` but the controller did not respond, so
there is no address and nothing can scan or pair. Check `ls /sys/class/bluetooth/hci0/address`, `dmesg | grep -iE
'bluetooth|hci_uart'` (`Reset failed (-110)` or `command 0xfc18 tx timeout` is this case) and `grep '^0:' /proc/tty/driver/serial`.
It is not a missing file: the BCM4364 starts from its ROM. Known open problem, see docs/MAC-HARDWARE.md, Bluetooth. A USB Bluetooth
adapter works as a stop-gap (`btusb` is in every image). Bluetooth on a BCM4377 Mac (MacBookPro15,4, 16,3, Air 9,1) does need
`brcmbt*.bin` and `.ptb` from macOS Monterey or later (the macOS script copies them).

## Accounts, vault and audit

**Forgot the admin password:** [`ADMIN-GUIDE.md`](ADMIN-GUIDE.md#forgotten-admin-password).
**"Your personal vault could not be opened":** your password changed outside IYAKKI (an admin reset it, which resets the vault).
Settings -> Vault explains; secrets stored before are lost, store them again.
**The audit log says BROKEN at entry N but nobody touched it:** the old race (two events in the same second sharing a `prev`). It is fixed for a single
running IYAKKI, but a chain that already contains it stays BROKEN, and two IYAKKI processes on one data folder can still collide. Look at lines N-1
and N of `audit.log`: the same `prev` and times a second apart means the race; anything else is treated as tampering. To get a clean chain, stop
IYAKKI, move `audit.log` aside (keep it) and start again. Steps: [`ADMIN-GUIDE.md`](ADMIN-GUIDE.md#what-a-broken-report-means-now).
**A tool keeps asking even after "Allow for this session":** it is on the always-ask list (vault, remote cluster, destructive, drive writes, store
publish, platform deploy) or its scope differs (another cluster, host, folder or subcommand); INBOX approvals and parallel tasks have no session grants.
**A long command seems to vanish with the chat:** it should not, jobs are detached: open the Jobs strip or RUNS -> Jobs ([`JOBS.md`](JOBS.md)).
**TALES does not read aloud:** Settings -> Voice shows which engines were found and why not; in the window use the browser voice helper ([`VOICE.md`](VOICE.md)).
**Sign-in is delayed:** three wrong tries slow the next attempts (up to 60 s per attempt for that name).

## Building and testing

**`ENOSPC: no space left on device` although `df` shows free space.** Run `df -i /tmp`. If it is 100% the `/tmp` tmpfs has run out of
**inodes**: a tmpfs has a fixed inode limit and `cargo test` leaks many small temp folders (`/tmp/iyakki-*`). Fixes:
`sudo mount -o remount,nr_inodes=4194304 /tmp` (not persistent across reboots), delete the leftovers with
`rm -rf /tmp/iyakki-*`, and run tests with their own temp folder: `TMPDIR=$HOME/.cache/iyakki-tmp cargo test --workspace`
(create the folder first). The same error also shows up as tools failing to write their own output files.

**`./run.sh window` first build takes minutes.** Normal; later runs are fast. `./run.sh build` of the ISO takes 30-40 minutes and
downloads about 1 GB; the `web` image needs about 30 GB disk and 16+ GB RAM.

## What to send with a report

1. IYAKKI version (`iyakki --version`, or `iyakki-desktop --version`), how you run it (window, USB, VM, container), OS and hardware.
2. What you did, what you expected, what happened (a screenshot with `F12` helps).
3. Window mode: the last entries of `~/.local/share/iyakki-dev/crash.log` and the terminal output of `./run.sh window`. Installed
   system: the boot messages (the "show boot messages" entry), `iyakki hwreport`, `iyakki storage`.
4. **Never send** `users.json`, vault files, API keys or an unredacted `audit.log`.

Where to send it: [`../CONTRIBUTING.md`](../CONTRIBUTING.md#reporting-issues). Security problems: [`../SECURITY.md`](../SECURITY.md).
