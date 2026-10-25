# Troubleshooting

**For:** anyone whose IYAKKI is not doing what the guides say.
**You will get:** the problems we actually hit, each with symptom, cause and fix, and what to send when you report one.

Quick map: [Window mode](#window-mode) · [Models and chat](#models-and-chat) · [Kubernetes and deploy](#kubernetes-and-deploy) ·
[Browser](#browser) · [Sound, voice, TTS](#sound-voice-and-speech) · [Hardware, boot, network](#hardware-boot-and-network) ·
[Drive lost](#the-drive-was-disconnected-io-errors-everywhere) · [Accounts and audit](#accounts-vault-and-audit) · [Building and testing](#building-and-testing) · [What to send](#what-to-send-with-a-report)

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

**Claude Code asks me to sign in again after a restart.** The login is kept by Claude Code in your home folder (`/home/<you>/.claude`) on the data volume and survives restarts and updates. The chip used to say "sign in" because the status had been looked up before you signed in to IYAKKI; it is now looked up again at your sign-in, when you open the model list or the chat and it says "sign in", and after Login. If it still says "sign in": wait a few seconds and open the model list again (the first start of Claude Code after boot can take long); then look in the system log for the line `agent claude-code: looks signed out (HOME=..., exit ..., login files: [... exists|missing], output: ...)`. `missing` means the login really is not there (sign in once more); `exists` with exit 1 means Claude Code itself refused it (the output says why; a network error is shown as "cannot be checked", not "sign in"). Tokens are never written to the log. GitHub Copilot works the same way (`~/.copilot/config.json`, or a token kept in your vault, which stays unreadable until you have signed in to IYAKKI).

**The chat says "No chat model selected" but I chose Claude Code.** Fixed: the chat now shows `agent: Claude Code · <model>` in its footer. If the agent is not ready the empty chat says why (`Claude Code: sign in` with a Login button, or `not installed` with the install command).

**Approvals never come up.** Open the INBOX (`F9`), Approvals page: every waiting call is there. Not answering means deny.

## Kubernetes and deploy

**The K8S tile or the IDE's K8s panel sometimes says "kubectl failed" or "The cluster is not reachable", and Refresh fixes it.** The
cluster was busy or restarting for a moment (k3s shares the computer with its pods, the browser and the shell). Since alpha.2 these
failures are retried by themselves (three quick retries, then 2, 4, 8, 15 s), the last list stays on screen with a dim "Updating..." line,
and the banner appears only after about 25 s or for a failure that cannot pass (forbidden, no kubeconfig, a bad context). When it does
show, it says what kubectl said, when it last worked and how often it retries. To report one: tile -> **Show log** -> **Copy** and send
the text (it holds no kubeconfig path or credentials). Details: [`K8S-TILE.md`](K8S-TILE.md#when-the-cluster-does-not-answer-for-a-moment).

**Two local clusters fight for ports ("address already in use", the second one fails or the first one stops answering).** Each extra local cluster gets its own API port (6543, 6643, ...), kubelet ports, node name and address ranges, so two IYAKKI clusters never share one; the clash comes from something else on the machine: a hand-started k3s, a system k3s (`/etc/rancher/k3s`), another program on 6443 or on one of the ports in the cluster's plan (Settings, Clusters, Set up shows them, or `ss -ltnp | grep -E '6443|6543|102[5-9]0'`). Stop the other program, or stop one of the two clusters (a second local k3s also needs about 2.5 GB of free memory; below that it is refused with the numbers). Two k3s servers share the host's flannel device and `cni0` bridge, so pod networking of one can disturb the other; run only one at a time if pods lose connectivity. The log of each is `<data>/k3s-dev-<id>/k3s.log` (`k3s-dev/` for This machine).

**The cluster switcher is not shown.** It appears only when the account has more than one cluster (Settings, Clusters). A guest never sees it; a user sees only the clusters marked *visible to users*.

**k3s stops by itself after a network change.** Moving the stick to another network (or Wi-Fi to Ethernet) changes the node's address; k3s may end itself then (its log shows `NodeIPs changed for the node`). IYAKKI sees that the address changed, starts k3s again at once and counts no failure. If the dev cluster shows *failed* with "IYAKKI stopped watching k3s", the reason is in the audit (`devcluster.supervisor-stopped`) and `k3s.log` ends with `iyakki: k3s exited (...)` telling the exit status or signal; Restart in Settings → Dev cluster starts it again.

**The dev cluster says "waiting for the network…" or "no network yet".** k3s will not start without a network interface that has an IPv4 default route (`ip route` shows a `default via …` line; `/proc/net/route` is what IYAKKI reads). After a boot the DHCP client may need a minute. IYAKKI waits, shows this in SYSTEM and Settings → Dev cluster, and starts k3s by itself once a route appears; it does not count the wait as a failure. If it never appears, check the cable or Wi-Fi and the router's DHCP; Stop cancels the wait.

**k3s sometimes runs, sometimes not: wrong date at boot.** Some computers have no usable hardware clock (a MacBook Pro 2016/17 reads 2024-01-01 at every start). The time server corrects the clock only about 30 seconds after boot (Wi-Fi and DNS must be up first), but k3s used to start as soon as a default route existed, with the fake date: its certificates (made later) were "not yet valid" and `k3s.log` showed `Failed to validate connection to cluster ... CA cert validation failed`; then the clock jumped by years and k3s survived or not. Now: (1) the clock never starts earlier than the saved time, the image's build time or a compiled-in date (`clockfloor.rs`; the time is saved in `/var/lib/iyakki/clock-floor` at shutdown, every 5 minutes and when the network time was set); (2) k3s waits for the clock: the floor applied AND (a time server answered OR the machine has been up 120 s, so it still starts offline); the status line says "waiting for the clock to be set"; (3) if the clock jumps by more than 3 minutes while k3s runs, k3s is restarted once (audit `devcluster.restart-clock-step`, no failure counted), and a log line about `CA cert validation failed` or `not yet valid` in the first 2 minutes restarts it once (`devcluster.restart-clock-cert`). To check: `date` right after boot (it must not show 2024), `dmesg | grep -i clock` (the `clock floor` lines and `Clock set from ...`), the audit log for `devcluster.waiting-for-clock` and `devcluster.restart-clock-*`, and the start of `/var/lib/iyakki/k3s-dev/k3s.log`. Only fakes have tested this so far (F-CLOCK-01, F-K3S-27). The first boot of a new image has no saved time: it uses the build time. A clock that is still wrong after the time server answered shows in Settings, Time.

**`cannot find filesystem info for device /dev/sda3` (cadvisor, eviction manager) fills `k3s.log`.** The kubelet's cadvisor looks the data partition up through `/dev/block` and `/sys/dev/block` and, because the system runs from a RAM file system with the data partition bind-mounted, cannot match the device of `/var/lib/iyakki/k3s-dev` to a block device. It is noise: the node still becomes Ready and pods run; only disk-pressure eviction by the kubelet does not see this disk (IYAKKI's own disk alert at 85 % still works). Left alone on purpose: the usual remedies (a kubelet `--root-dir` on another mount, `--feature-gates` or `evictionHard` changes) alter how k3s sees its storage and could not be tried on the device. To keep the log readable, look at it with `grep -v cadvisor /var/lib/iyakki/k3s-dev/k3s.log`. Reported to the owner as an open point in `docs/K3S-DEV.md`.

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

**No hand pointer over links (bare metal).** Fixed after alpha.1: on the framebuffer the pointer was always the arrow. It now follows what is under it (hand over links and buttons, I-beam over fields, the web engine's own shapes in the full browser; none while a page hides it). Needs an image built after this change and written to the stick; the shapes are drawn in software, so a page that changes the pointer under a still mouse updates within about 33 ms. Until seen on a real screen this is tested with fakes only.

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
* **The radio says "io error" or the chime is barely audible (MacBook, analog card).** "io error" means the player opened the raw card
  because no output was chosen yet (the card appeared after the shell started) or because the card refused the format (a
  CS8409 card takes only 32-bit/24-bit at 44.1 kHz). Settings -> Sound should now show the speakers and a toast *Sound output ready*
  appears within a few seconds of the card coming up; `cat /etc/asound.conf` must start with `# Written by IYAKKI`. Right after boot
  the page says *waiting for the sound card (the built-in speakers of this Mac start up slowly)* for up to 30 s: press `r` to look again.
  If the slider changes nothing, the card has a hardware volume control that does nothing: Settings -> Sound -> Advanced (`A`) ->
  *Does the hardware volume work?*, or set *Volume control* to Software. If it is too quiet at 100 %, try *Boost* (it may distort).
* **Settings -> Sound says "sound card is in use by <program>".** A program (often a player started before IYAKKI wrote `/etc/asound.conf`)
  holds the card, so the shared device behind the volume cannot open. The file is already written; stop that program and press `r`
  (Look again), the volume control is then created. The volume keys say *Volume control not ready* until then.
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
* **YouTube plays noise or no sound (MacBookPro16,2 and other T2 Macs).** Cause (measured): the T2 sound device has one-frame periods and
  ALSA's `plug` layer plays 24, 32, 16 and 12 kHz audio at the wrong speed and cannot take 32-bit float at 44.1 kHz. The Mac image's
  web engine now plays through `iyakkiweb` (`grep iyakkiweb /etc/asound.conf`; its processes have `ALSA_CONFIG_PATH` set:
  `tr '\0' '\n' < /proc/$(pidof -s WPEWebProcess)/environ | grep ALSA`). Safe checks over SSH (never `arecord` on card 0: it can crash the Mac):
  `cat /proc/asound/card0/pcm0p/sub0/status` while a video plays (state RUNNING), `.../hw_params` (S24_LE, 4, 48000). If the page was paused
  and the radio says busy, pause/close the tab or wait: idle web pages are asked to release the device. A tone check that is safe:
  `aplay -D default /usr/share/sounds/alsa/Front_Center.wav` (or any 44.1/48 kHz wav), and for the engine's chain
  `printf '</usr/share/alsa/alsa.conf>\npcm.!default iyakkiweb\n' > /tmp/w.conf; ALSA_CONFIG_PATH=/tmp/w.conf aplay /path/file.wav`.
* **Do not record from the T2 loopback or microphone: kernel crash.** Opening the capture of the T2 sound card (`Bridge Loopback`, `Digital Mic`,
  `Codec Input`) while something plays hung and reset a MacBookPro16,2. The Voice tile hides them and the driver refuses them unless
  `echo 1 > /sys/module/apple_bce/parameters/capture` (at your own risk; patch 0003, needs the new image).
* **The speaker keeps repeating a chunk of the last sound while the Mac restarts or powers off** (T2 Macs). The T2 plays its audio buffer in a
  loop until told to stop. Before the final disk flush IYAKKI now sets the volume to 0, ends every program that still plays (waiting at most
  1.5 s), and writes a moment of silence; the apple-bce patch 0004 also stops the stream and zeroes the buffer when the machine shuts down.
  Needs the new image; the log line `aaudio: silenced output of ... on shutdown` in `dmesg` shows the driver part ran.
* **No sound in the browser (YouTube) while the radio or a media file plays, or the reverse** (`Device or resource busy`):
  the output is shared through a `dmix` in `/etc/asound.conf` (`type dmix` under `pcm.iyakkimix`). If the file instead has
  `slave.pcm "plughw:C,D"`, the dmix could not be opened on that device and IYAKKI fell back to the exclusive one (look for
  `dmix on card C device D could not be opened` in the log); `fuser -v /dev/snd/pcmC*D*p` shows who holds the device. A
  program holding the raw device from before the file was written also blocks it: stop it or restart IYAKKI.
* **The browser is silent on YouTube for a whole session; closing it and opening it again helps (T2 Macs).** Seen with the speaker
  `closed` in `/proc/asound/card0/pcm0p/sub0/status`, so nobody held it: the web engine's sound sink never opened the card. IYAKKI now
  watches for it: when the page plays audio (the helper's `AudioPlaying`) and no stream of the engine runs after 3 s, the browser bar
  says "Sound was not connected; reconnecting..." and the playing media is paused and played again (twice at most); if that fails
  it says "Sound is still not connected: reload the tab" once. The agent no longer opens the speaker itself (the volume control's
  "prime") while any stream is open or the page plays; it sets the volume on an existing control without opening anything. Evidence
  to gather when it happens again: `cat /proc/asound/card0/pcm0p/sub0/status` and `hw_params`; the agent log lines starting
  `sound doctor:` (they include the first GStreamer/ALSA warning of the engine, if any) and `sound: not opening iyakki`.
* Over SSH: `cat /proc/asound/cards`, `aplay -l`, `amixer -c 0 contents | grep -A2 Jack` (which jacks are on),
  `cat /proc/asound/card0/eld#*` (monitor_present, monitor_name), and `aplay -D plughw:0,3 some.wav` to try one device by hand.
  ALSA's `default` is card 0 device 0, which HDMI-only machines do not have; IYAKKI makes `default` (and `iyakki`) the output in
  use in `/etc/asound.conf`, and passes `-D iyakki` (or `-D plughw:C,D` where it cannot write the file) to `aplay` itself.

A VM has only a virtual sound card. RADIO plays MP3 streams only. Settings -> Voice -> `p` is the speaker test of the voice setup.

**TTS missing: TALES / read-aloud does not speak.** No text-to-speech command is configured (or the program is not installed).
Settings -> Voice -> `v` set the TTS command, `y` test. Without an engine the story is still written and saved. Dictation (`Ctrl+R`)
needs a speech-to-text command (`s`). MEDIA needs mpv, VLC, ffplay or mplayer for formats other than MP3.

## Hardware, boot and network

### How do I power off or restart from the login screen?

Click **Power** at the right end of the bar at the top of the login (or first-run setup) screen, or press **Ctrl+P** (or the keyboard's
power key), then choose **Power off** or **Restart** (Enter or y confirms the highlighted choice, Esc cancels). No password is
asked on the machine's own screen. If the card says *This system cannot power off from here*, IYAKKI is running in a window on
another computer (close the window) or without the rights to power off. In the browser (remote desktop) the login page has
no power button on purpose: sign in first (the account needs the power permission) or use the machine itself.

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

### Content beyond the screen (Back, clock or icons cannot be seen)

*Symptom:* the right or bottom part of the desktop is missing, so the top-right **Back** button, the clock and icons cannot be
clicked. Seen on a MacBookPro14,1 (framebuffer 2880x1800, panel 2560x1600).

1. `dmesg | grep display:` should say `display: buffer 2880x1800, visible 2560x1600 (eDP-1 preferred mode)`. The visible size is
   the connected screen's preferred mode from `/sys/class/drm/card*-*/modes`. If the line shows `visible` equal to the buffer, look at
   `cat /sys/class/drm/card*-*/status` and `.../modes` (nothing readable keeps the buffer size).
2. **Keyboard way out:** Esc goes back in an app; F1 lists the keys; F10 opens the IYAKKI menu; F12 takes a screenshot.
3. With an external monitor too, the same buffer is shown on both: the built-in panel's size is used, so a larger external screen shows
   the desktop in its top-left corner.
4. **Send:** `dmesg | grep -E "display:|splash:"` and `cat /sys/class/drm/card*-*/{status,modes}`.

### "AI IDE stopped because of an internal error. Details are in the system log."

*Symptom:* a red message at the bottom, the app closes and you are back at the launcher; or the chat box of the AI IDE (or any
app) seems not to react to clicks because the app closes the moment the pointer reaches it. Seen on a MacBookPro14,1 (2560x1600):
`PANIC in thread 'iyakki-shell': ... min > max, or either was NaN. min = 1908.4784, max = 1908.4783` (a layout calculation whose
lower bound was 0.0001 above its upper bound). Every such calculation now uses a helper that cannot panic, but any other bug
of this kind shows the same message. The shell catches the panic, closes only that app and keeps running.

1. **Where the log is:** the kernel log. Open a terminal tile (CLI) and run `dmesg | grep -E "PANIC|iyakki"`, or from a paired
   computer `iyakki dev dmesg --grep PANIC` and `iyakki dev logs --system` ([`DEVELOPER-MODE.md`](DEVELOPER-MODE.md)). The boot log is
   `/var/log/iyakki-boot.log`.
2. A panic prints `PANIC in thread '...': panicked at <file>:<line>: <message>`, followed (for the first five panics of a minute)
   by `PANIC at #N <function> at <file>:<line>` lines: the frames of IYAKKI's own code, innermost first. The first of them is the call site.
3. **Send:** the output of `dmesg | grep PANIC`, the app, the screen size (`dmesg | grep display:`), and what you did in the last
   seconds (for example "dragged the divider of the chat", "selected text with the trackpad"). Opening the app again is safe.

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

### MacBook: no keyboard or trackpad in the image (2015-2017, SPI keyboard), or no pointer from the trackpad

The built-in keyboard and trackpad of these MacBooks are on an SPI bus; they need `intel_lpss_pci`, `spi_pxa2xx_platform` and `applespi`
(`lsmod`, `ls /sys/bus/spi/devices`). The boot loads them (repeated coldplug and a by-name list for the listed models). By hand:
`modprobe spi_pxa2xx_platform applespi`. The log lines `pxa2xx-spi ...: no DMA channels available, using PIO` and a few `applespi ...
Received corrupted packet (invalid message length 8 - num-fingers 0 ...)` at start are expected on the 14,1 (short non-touch messages the
driver drops). Keyboard but no pointer: the trackpad needs the Mac image's touchpad support; check `grep -A8 "Apple SPI Touchpad"
/proc/bus/input/devices` and Settings -> Keyboard -> Trackpad. Details and the test sequence: `docs/MAC-HARDWARE.md`.

### Mac firmware install says "Not done" (or "Retry downloads again and fails")

Open IYAKKI menu, Mac drivers (or Store, Setup, the Mac Wi-Fi and Bluetooth firmware card) and read the last result and the log. What each ending means:

* **"Apple's macOS versions for this Mac do not contain ... What works: Wi-Fi works with the generic firmware ... and Bluetooth through a USB Bluetooth adapter"**: the job opened every macOS image it may try for this Mac (at most three, named in the log with "nothing for this Mac") and none had the files. Nothing is broken: Wi-Fi keeps the generic firmware; for Bluetooth use a USB adapter. The images are kept so that **Retry (no new download)** costs nothing; Delete download frees the space (0.5 to 1 GB per image).
* **On a 2017 MacBook Pro (BCM4350)** this is expected for Wi-Fi: no macOS recovery image has extra Wi-Fi files for that chip. The Bluetooth patch is in the macOS 11 image only, which the job now asks for first; if the log says `BCM.hcd was made from BCM4350-Updater.hex` and Bluetooth still has no address, restart IYAKKI once from the menu and send the `dmesg` lines about `hci0`.
* **"Bluetooth firmware is installed, but the controller still has no address"**: restart once from the menu (nothing restarts by itself), then check Settings, Bluetooth.
* **Retry downloaded the image again** (old versions only): they deleted the first image before trying another. The current job keeps every image until the files are installed, so the log of a retry reads `using the image kept from an earlier try (... no new download)`. If you still see a second download, the first one did not finish: it then says `continuing a partial file`.
* **A download that stops** (connection, power): press Retry; a partial image resumes from the bytes on disk. A damaged image (it does not match Apple's chunk list) is deleted and fetched again, and the log says so.
* **"the data volume has N MB free"**: an image needs about 2.5 GB of free space during the download. Press Delete download for old images or free space, then Retry.
* **"7-Zip could not be installed"**: the reader of the image is fetched from its project at your click; with no network put the official `7zz` into the folder named in the message and press Retry (the images are kept).

Everything the job did is in its log (the installer list beside the clock, View log) and in `/var/lib/iyakki/mac-firmware/last.json`; each request names its host (`osrecovery.apple.com`, `oscdn.apple.com`) and its byte counts. Background: [`MAC-WIFI-BLUETOOTH-SETUP.md`](MAC-WIFI-BLUETOOTH-SETUP.md).

### T2 MacBook (MacBookPro16,2 and similar): no keyboard or trackpad, radio and TALES silent, adapter not found until a restart

* **No keyboard or trackpad:** `lsmod | grep -E 'hid_apple|bcm5974'`. By hand: `modprobe hid-apple` (keyboard), `modprobe bcm5974` (trackpad). The drivers load by their hardware alias since the boot scan reaches the T2's USB devices; older builds missed them. `dmesg | grep 'bad trackpad package'` lines are harmless debug output (`echo 0 > /sys/module/bcm5974/parameters/debug`; new builds set it).
* **Radio stops and says "the sound player stopped" while the Settings sound test is fine:** the test plays straight to the card, the radio and TALES go through the volume. Run `aplay -D iyakki some.wav`; `Unable to install hw params` with a one-frame period means the old volume chain; new builds put the volume on `hw` for this card. `cat /etc/asound.conf` shows `slave.pcm "hw:0,0"` then. The radio message now quotes what `aplay` said.
* **A USB Ethernet or Wi-Fi adapter plugged in after the start is not found:** `dmesg | tail` and `ip -br link`. New builds load its driver and set it up when it appears. By hand: `modprobe r8153_ecm; ip link set eth0 up; udhcpc -b -i eth0`.
* **Touch Bar dark, no `Touch Bar Display` in `/proc/bus/input/devices`:** `cat /sys/bus/usb/devices/*/bConfigurationValue` for the device with `idProduct` 8302 is empty. The boot retries; if every write times out, restart the Mac. Touch ID and the Touch Bar's own drawing are not available on Linux.
* **Wi-Fi on a T2 MacBook** needs Apple's files (Mac drivers page); `dmesg | grep brcmfmac` names the file it wants. Details: [`MAC-HARDWARE.md`](MAC-HARDWARE.md) "MacBookPro16,2".

### MacBookPro13,x/14,x: no sound although the CS8409 driver is loaded

`dmesg | grep "cs8409:"` shows which way the driver went and what it found (DACs, PCMs); send those lines with `aplay -l` and
`head -8 /proc/asound/card0/codec#0`. Test sequence and how to read the lines: `docs/MAC-HARDWARE.md` (sound through the CS8409 driver).

### Two computers, one cable: "Connect over USB-C" stays red

Open Settings -> Developer mode -> Connect over USB-C (or run `iyakki dev usb`); the first red step says what to do. The usual causes, in order:

* **Step 1 or 2 red with a plain USB-C cable.** Two PCs cannot talk over a plain USB-C or USB 3 cable (both ports are hosts). Use a Thunderbolt 3/4 or USB4 cable between
  two Thunderbolt/USB4 ports, or a host-to-host USB bridge cable (Prolific PL-25A1/PL-27A1 type). A charge-only cable shows nothing.
* **Step 2 red with the right cable.** Plug it into a Thunderbolt/USB4 port on both computers (some PCs need Thunderbolt switched on in the firmware setup), wait a few
  seconds, press Check again. `ls /sys/bus/thunderbolt/devices` should list a second device. A dock waiting at security level `user` needs Authorize.
* **Step 3 red, "no network interface".** The other computer is there but `thunderbolt_net` is missing: the kernel was built without `CONFIG_USB4_NET` (an image made before
  this feature), or the module did not load: `modprobe thunderbolt_net`.
* **Step 4 red.** The other computer must run IYAKKI with the link up; both ends send a beacon every 3 seconds on the cable only. A system without IYAKKI is not found; give its
  address to `iyakki dev pair <address>`.
* **Step 5 red.** Developer mode is off on the other computer (Settings -> Developer mode, `e`), or its firewall is from before the direct-link zone (port 7979 blocked on the cable).
* **Step 6 red.** On the other computer press `p` (Pair a computer) to show a code and the fingerprint, then here **Pair**, type the code, Enter. Codes last five minutes.
* A plain wired adapter or tethering phone that suddenly has a 169.254 address and a strict firewall: an adapter that got no DHCP answer for 20 seconds is treated as a direct link
  (developer mode, beacon and ping only). It goes back to normal when DHCP answers.
* Copy diagnostics (the button, or `iyakki dev usb`) and send that text; it has no keys and no codes.

### Publish from the IDE fails: the private store is deployed but not added

**Symptom.** The Publish form offers only "Deploy a new private store to k3s…" and "Export package file", or the assistant says to go to Settings;
the audit log has no publish line. The store itself is fine: `kubectl get pods -n iyakki-private-store` shows `private-store` Running and
`curl http://localhost:30300/catalog.json` answers.

**Cause.** The deploy recorded the store and kept its admin token in the vault, but this device's STORE never got it as a catalog with its key
pinned (the older form only listed stores that were already added).

**Fix.** Press Publish: the store is the first row, "Private store on this device (not added yet)". Choose it, compare the fingerprint on the
card with `kubectl logs -n iyakki-private-store deploy/private-store` (the line ends with `fingerprint ...`), press **Add it and publish**. On an
older image: STORE, Settings, Private store, **Add this store** (address `http://localhost:30300`), compare, confirm, then `h` to choose it as the
publishing target. Only an administrator can add a store.

**If the form says the store is not answering.** The pod is not ready or the NodePort is closed: wait a minute after a deploy, or open the K8S
tile and check pod `private-store` (namespace `iyakki-private-store`); `kubectl get svc -n iyakki-private-store` shows the NodePort. Nothing was
changed and no key or version was written.

### The computer crashed or froze: where is the log

**Symptom.** The screen went blank or stopped moving, or the computer restarted by itself or had to be powered off with the button, and you want to know why.

**Where the log is.** From the moment the data volume is mounted, IYAKKI copies the kernel's log to the stick, in both the normal and the lean boot:
`/var/lib/iyakki/log/kmsg-<boot id>.log` (this boot), `kmsg-previous-1.log` to `-3.log` (the last three boots), and `kmsg-crash-<boot id>.log` (a boot that ended
unexpectedly, or whose log shows a panic or oops: kept until you delete it). Each line has the date and time when the clock was right, then the kernel's own time in seconds, the level (`<3>` is an error) and the message.
It is written at most 2 seconds late, and at once for errors and words such as BUG, Oops, panic, Call Trace, hung task, soft lockup and out of memory, so the file ends within about 2 seconds
of where the computer stopped. The whole folder stays under 64 MB (8 MB per boot); the oldest ordinary files go first, crash files last.

**How to read it.**
* On the computer: Settings -> Overview shows "The last session ended unexpectedly (date)" after such a crash; **Fix it** opens Settings -> Last session log (the last 200 lines; Up/Down, PageUp/PageDown, Home/End scroll;
  `p` switches between the crashed session, the boot before and this boot; **I have seen it** clears the note, the file stays).
* In a terminal: `iyakki log kernel --crash` (the newest crash file), `iyakki log kernel --previous`, `iyakki log kernel` (this boot), `iyakki log sessions` (every boot with clean or unclean).
* On another computer: plug the stick in and open the `IYAKKI-DATA` volume, folder `log`. The audit log has the line `system.unclean-shutdown` with the last kernel line and how long the session had been up.

**What it cannot tell you.** A kernel that dies cannot write its own last words: if the computer froze without printing anything (a hard hang), the log simply stops, and the last lines are the best clue. A session also counts as
"unexpected" when the power was cut by hand, because the computer cannot tell the two apart. Nothing is written while the stick is lost or in error (docs/STORAGE-LOSS.md): those lines are dropped, and a note says how many.

**What the boot options do.** Every boot entry has `panic=20`: after a kernel panic the computer restarts by itself 20 seconds later (a blank screen that stays blank after a panic helps nobody on a computer that starts from a stick;
the 20 seconds leave time to read the message on a screen that shows it). `oops=panic` is deliberately not set (an oops in one driver, for example sound, should not end the whole session, and the log keeps it), and neither are the soft-lockup and hung-task panics (a slow stick would turn into a restart).
`printk.devkmsg` is switched to `on` early in boot so that IYAKKI's own boot lines are not dropped.

**For the first-stage failures of the lean boot** (before the system starts) see docs/BOOT-SPEED.md: `boot/stage1.log` on the boot volume and `stage1-kmsg.log` on the data volume.

### The drive was disconnected: I/O errors everywhere

**Symptom.** A red card "The IYAKKI drive was disconnected for a moment, so saving is not possible right now" (or "is not answering", "is read-only"); the camera cannot save, the chat says
the work folder is missing, Claude Code or the AI IDE "failed to load" or says "io error"; over SSH `ls /home/<name>` prints `Input/output error` and `touch` fails. The system itself keeps
running because it lives in RAM.

**What to look for.** `dmesg` shows, within a second of each other:

```
usb 2-2: USB disconnect, device number 2
usb 2-2: cmd cmplt err -108
uas_zap_pending
I/O error, dev sda, sector ... op 0x1:(WRITE)
EXT4-fs (sda3): failed to convert unwritten extents to written extents -- potential data loss!
usb 2-2: new SuperSpeed Plus Gen 2x1 USB device number 3 using xhci_hcd     (the drive is back, as sdb)
```

The drive came back as a new device (`sdb` for the dead `sda`) and the data folder was still mounted on the dead one. IYAKKI now notices this within a few seconds
and, when it is the same filesystem (same UUID, label IYAKKI-DATA), reconnects it by itself after 3 steady seconds. `iyakki storage status` says where it stands; `iyakki storage reattach` or the card's
**Reconnect now** does it at once. Details, and what can and cannot be saved: [STORAGE-LOSS.md](STORAGE-LOSS.md).

**Why it happens (not known for sure).** A loose or worn USB-C cable, a port or hub that browned out, or a drive and the UAS driver misbehaving together. Work through them in this order:

1. Use the cable that came with the drive (a USB 3.2 Gen 2, 10 Gbit/s cable), pushed in fully; avoid adapters and long cables.
2. Plug the drive straight into the computer, not into a hub or dock; on a Mac try the other side's ports. Settings -> Storage says "runs at SuperSpeed+ 10 Gbit/s on USB port 2-2": a port like `2-1.3` is behind a hub, and `High-speed 480 Mbit/s` means the link fell back to USB 2 (cable).
3. The `storage:` lines in the log give the port, hub and speed at the moment of the loss; a loss that happens again on one port and not on another points at the port or cable.
4. Still happening: Settings -> Storage -> key `u` (Compatible USB mode, no UAS) or the boot entry "IYAKKI - compatible USB mode (no UAS)". Slower, steadier; it needs a restart.

**What is lost.** Anything that was being written at the moment of the loss (the last seconds of a chat, a photo being saved). The file system itself is repaired by the journal when the drive is mounted again; IYAKKI never runs fsck by itself. If
the card says the drive could not be reconnected, press **Restart**; **Check the drive** shows a read-only check (`e2fsck -n`) or the end of the kernel log.

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
