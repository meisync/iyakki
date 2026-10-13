# Trying IYAKKI on this computer first

Everything below runs on a Linux PC with QEMU and needs no USB stick. Use it to finish and check features before
you take them to real hardware (an Apple Silicon Mac, an older Mac, an AMD PC).

| You want to… | Run | Notes |
|---|---|---|
| Click through the shell fastest | `./run.sh window` | A desktop window with the real shell (login, tiles, Settings, Browser). Real network, real files in `~/.local/share/iyakki-dev`. Reset: `rm -rf ~/.local/share/iyakki-dev`. |
| Try disks without risk | `./run.sh disk add 8G` (also `list`, `rm vda`) | The window shows **virtual disks only** (sparse files in `~/.local/share/iyakki-dev/disks`), never this computer's drives; Settings → Storage makes and deletes them too, and "make data disk" is rehearsed on them. `./run.sh window --real-disks` shows the real ones. |
| Open the desktop in a browser | `cargo run -p iyakki-remote --example demo -- 127.0.0.1:18080` | A throw-away server with an account `root` / `rootpass12`; the real thing is `iyakki web` (see the README). |
| Measure boot time | `QEMU="sudo qemu-system-x86_64" python3 dist/buildroot/boot-time.py` | Power-on to the sign-in screen in QEMU (3 runs, medians); the first baseline is in FEATURES.md F-BOOT-03. |
| Try the program on other distributions | `dist/test-bases.sh` | Alpine, Debian, Ubuntu, Fedora and Arch containers: starts, runs a guest, serves the browser page, signs in. |
| Prepare a release | `./run.sh release 0.1.0` | Copies the built images to `target/release-0.1.0/`, writes `SHA256SUMS`, signs it, writes the download page. |
| Boot the real system (kernel, init, boot screen) | `./run.sh vm-direct` | Skips GRUB. Add `--ssh` to log in with `ssh -i ~/.ssh/afos_diag -p 2222 root@127.0.0.1`. |
| Boot it like the USB stick (GRUB, data partition) | `./run.sh vm-disk` | A 4 GB virtual disk made by the same writer as the stick. The account you create is kept between runs. `--uefi` for UEFI. `./run.sh vm-reset` starts over. |
| Boot the ARM64 build (Apple Silicon VM path) | `./run.sh vm-arm` | Emulated, so slow; first run `./run.sh build-arm` (long, once). |
| Look at every screen as pictures | `./run.sh screens` | PNG files in `/tmp/iyakki-screens`. |
| Check the boot screen and logo | `sudo python3 dist/buildroot/qemu-splash-test.py` | Saves screenshots; `iyakki.splash=6` on the kernel line keeps the splash up for 6 s. |
| Run every automated check | `./run.sh test`, then `dist/buildroot/qemu-test.sh`, `qemu-install-test.sh`, `qemu-persist-test.sh` | Use `QEMU="sudo qemu-system-x86_64"` if QEMU complains about io_uring. |

The VM has internet through QEMU's NAT, so BROWSER can open real sites. The boot log is in `/tmp/iyakki-vm-serial.log`.

## A checklist for a local pass

1. **Boot screen**: logo, galaxy, progress bar, `L` shows the log. Then the sign-in screen with six bars under the name.
2. **First account**: create it, restart (`vm-disk`), sign in again: the account must still be there.
3. **Tiles**: nine tiles in two rows, no numbers, every label fits, arrow keys (also Up/Down) and the mouse work.
4. **Settings**: Network, Bluetooth, Display, Sound show what the VM has (a missing adapter says "not found"); Enter on an Ethernet row turns it off and on.
5. **Browser**: open a page, follow a link (click and Tab+Enter), back/forward, `g` to type an address. Press `j` to allow JavaScript for a site, `c` for the notes.
6. **Chat/Edit/CLI**: as before.

## Bare-metal test checklist

For the first real-machine pass (a USB stick boot on the Mac mini or another computer). Boot the **web** image
(`target/buildroot-web/images/iyakki-x86_64.iso`, or the install image) to try everything; the default image does the same
minus the video. Sound needs the speaker set up in Settings -> Voice, and for the web engine's own audio the mixer may start
muted (Settings -> Sound, or `amixer sset Master 80% unmute` over SSH).

1. **Sign in**: create the first account, sign in. If the machine has no keyboard layout issue the tiles appear.
2. **Samples**: open MEDIA. **Tracks** must list *IYAKKI Sample Beats* and *IYAKKI Sample Melody*, **Videos** *IYAKKI Sample
   Video*; **Folders** ends with *IYAKKI samples (read-only)* and `d` on it refuses. Press `t` on a sample: its type is
   *kuthu* / *melody* and the tag *sample*.
3. **Play the MP3**: Enter on *IYAKKI Sample Melody*. The now-playing card lights up, the clock runs and sound comes out of the
   speaker (if not, Settings -> Voice -> sound command; the status line says why). `s` stops, `.` and `,` step the queue.
4. **Search**: `/` then `kutthu song` -> only the beats sample; `melody` -> the melody sample. Press `4` to rate one (stars show
   in the row); `t` and type `my test` as a tag; Types page lists it.
5. **Download queue**: put a small media file on the other computer (`python3 -m http.server 8000` in a folder with an MP3 or the
   sample WebM), then in MEDIA press `D`, enter `http://<that-computer>:8000/song.mp3 http://<that-computer>:8000/clip.webm`
   (two addresses separated by a space), type `action, my test` as the category (click a chip too). The Folder line says
   `Videos (auto)` once the address ends in `.webm`. Save: the Downloads page shows both, the header chip says
   `Downloads: 2 running, 0 queued` and flashes `Download succeeded`. Press `x` on a running one (Cancelled), `r` (it runs
   again), Enter on a finished one (plays it), `f` (shows it in the library). A wrong address (`.../missing.mp3`) must end as
   *Failed* with the reason. The account needs that host in its allowed sites and write access to Music/Videos.
6. **Video** (web image only): Videos page -> Enter on *IYAKKI Sample Video*. The page area becomes the video (a colour-bar
   pattern with moving shapes and a two-note tone); Space pauses and plays, Left/Right seek, Esc stops. On the default image the
   status line must say *Video needs the web image or an installed player (mpv, VLC, ffplay)*.
7. **Chat** (needs the Agent switched on and a model): ask *play a kutthu song* -> the beats sample plays (MP3 in IYAKKI), the
   MEDIA app shows it; *download http://<computer>:8000/song.mp3* -> the answer is `queued as #N`, ask *did the download
   finish?* and it lists the queue.
8. **Restart**: restart during a download, open MEDIA -> Downloads: the entry says *Failed: Interrupted ... Retry* and `r` fetches it.

Write down for each step what you saw; the most useful findings are no sound (which sound command), a video that stays black
(say what the status line shows), and a download that fails with a reason you do not
understand.

## What a VM cannot tell you

* How the keyboard, mouse, touch screen and display feel on real hardware, and how fast it boots from a real stick.
* WiFi, Bluetooth, sound and GPU drivers (the VM has virtual ones).
* Secure Boot, and firmware quirks (an old Mac's boot menu, the Apple Silicon hypervisor's own screen).

Those are what the real-machine tests are for. For an Apple Silicon Mac, the ARM64 image (`iyakki-aarch64.img`) boots in
UTM or `qemu-system-aarch64 -M virt -accel hvf -cpu host -bios <AAVMF/edk2 firmware> -drive file=iyakki-aarch64.img,...`
as in `dist/buildroot/qemu-uefi-aarch64-test.sh`.
