# IYAKKI recovery guide

**For:** a person whose IYAKKI does not start, shows the wrong thing, or whose account or files are locked.
**You will get:** the boot menu explained, what to try in what order, and the exact steps for the seed root, the previous version, a lost password, a lost encrypted-home key, a reinstall that keeps your files, and a diagnostics file to send.

> **Preview for developers and testers.** Each section says what was tried and where. **Not tried** means the steps come from the project's own design and tests (fake machines, QEMU, folders), never from a real machine. Where a step erases something, it says so before you do it.

Contents: [Start here](#start-here) · [The boot menu](#the-boot-menu) · [Hardware check](#hardware-check) · [Safe graphics](#safe-graphics-black-screen) · [Boot messages](#reading-the-boot-messages) · [Remote diagnostics](#remote-diagnostics) · [Going back to the previous version](#going-back-to-the-previous-version) · [The seed root](#the-seed-root-last-resort-start) · [Raspberry Pi and KV260](#raspberry-pi-5-and-kv260) · [Resetting an account or a forgotten password](#resetting-an-account-or-a-forgotten-password) · [Encrypted home: the recovery key](#encrypted-home-the-recovery-key) · [Reinstalling and keeping your data](#reinstalling-and-keeping-your-data) · [Collecting a diagnostics bundle](#collecting-a-diagnostics-bundle)

## Start here

| What you see | Do this first |
|---|---|
| Nothing happens, the stick is ignored, or a grey "Security Violation" box | Secure Boot is on. Switch it off in the firmware settings, or enrol the project's key ([`INSTALL-GUIDE.md`](INSTALL-GUIDE.md#4-first-start), [`SECURE-BOOT.md`](SECURE-BOOT.md)) |
| The logo, then a black screen | [Safe graphics](#safe-graphics-black-screen) |
| Stops somewhere before the sign-in screen | [Reading the boot messages](#reading-the-boot-messages), then the trouble entries |
| Keyboard, mouse or touch does nothing | [Hardware check](#hardware-check) |
| It started, but the last update broke something | [Going back to the previous version](#going-back-to-the-previous-version) |
| It will not start at all after an update | [The seed root](#the-seed-root-last-resort-start) |
| You forgot your password | [Resetting an account](#resetting-an-account-or-a-forgotten-password) |
| Your encrypted home will not open | [Encrypted home](#encrypted-home-the-recovery-key) |
| The screen works, something else does not | the **DOCTOR** tile (press `r`), then [`TROUBLESHOOTING.md`](TROUBLESHOOTING.md) |

## The boot menu

IYAKKI starts through its own boot loader on UEFI computers (and through GRUB on legacy-BIOS computers and the live ISO). With no key pressed the normal entry starts by itself after about 3 seconds, and the screen shows only the logo. **Press any key** (or tap the screen) and the menu appears below the logo. The first level lists **IYAKKI**, **IYAKKI (previous version)** (only after an update kept one), **Install to a disk** (only on install media) and **More options**; Enter on More options opens the second level. Arrow keys, Page Up and Down, Home, End, Enter and Esc work; a mouse or a tap chooses a row.

The entries of the second level, and what each is for (the list is the project's own table, `dist/buildroot/boot-loader-stage.sh`, and is checked against the GRUB menu by a test; the exact set depends on the image):

| Entry | Use it when |
|---|---|
| Safe graphics (no GPU drivers) | the screen is black after the logo: simple framebuffer, no GPU driver |
| Screen test (no simple framebuffer) | an experiment for the black second between the logo and the sign-in form: it starts the normal system without the simple framebuffer so the GPU driver is the only one; compare it with the plain entry |
| Remote diagnostics (SSH, keys only) | you want to look at a stuck machine from another computer |
| Compatible USB mode (no UAS) | the USB SSD or stick drops off the bus after it started (seen once with a Samsung T7 on a USB-C Mac); it uses the plain USB storage driver, slower but steadier. **Settings, Storage, Compatible USB mode** writes the same setting for the drive IYAKKI runs from |
| Trouble 1 to 4 | it stops early: messages on the firmware screen, then PCI and power saving off, one CPU, ACPI limited. Try them in order |
| Trouble 5 | safe graphics with every message on the screen |
| Show boot messages | you want to read what the kernel says |
| Hardware check (keyboard, mouse, touch, display) | first run on a new machine; input does not work |
| Boot self-test (verbose, powers off) | you want the self-test, then the computer powers off |
| NVIDIA driver (nouveau off) | an NVIDIA card gives a black screen or a frozen picture |
| Also in a browser (HTTPS on port 8443) | you want the desktop in a browser on your network as well |
| Touch screen entries (probe, tests, full probe) | a touch monitor is dead after a cold boot; they are experiments that change one thing each, see [`KNOWN-LIMITS.md`](KNOWN-LIMITS.md) |
| Full start (slower) | the fast start of a lean install does not work |
| Previous version of this boot loader | the newest boot loader does not start (shown only when there is a kept copy) |
| Recovery (GRUB menu) | the loader menu does not work: GRUB is kept as a second way in; its menu has the same entries |
| UEFI firmware setup | you need the firmware settings (shown only when the firmware can do it) |

**Tried:** the loader menu and the GRUB menu were checked by tests (every entry of one has a twin in the other). The loader booted the Mac mini 2018, the MacBook Pro 14,1, the HP and the Lenovo (owner, 2026-11). "Trouble 1" was written for the HP laptop that stopped on the logo, and its messages showed the missing kernel option (x2APIC), which was then fixed. **Not tried:** trouble 2 to 4 and the NVIDIA entry on a machine that needed them.

## Hardware check

1. Pick **Hardware check (keyboard, mouse, touch, display)** in the menu.
2. **You should see** a report: attention lines first (no keyboard, no mouse, no display, only a touchpad), then the display with its monitor names and native sizes, the input devices (`[keyboard]`, `[mouse]`, `[touchscreen]`, `[touchpad]`), network, storage, audio and USB.
3. A live tester follows: type, move the mouse, tap the eight squares at the corners and edges; each turns green. A touch panel that is offset or covers only part of the screen shows here.
4. Keys: `Enter` continue to sign-in, `S` save the report to `/var/lib/iyakki/hwreport.txt` (kept only if the machine has a data volume; otherwise photograph the screen), `R` refresh, `C` clear.
5. On a running system, or over a serial or SSH shell, the same report is `iyakki hwreport`.

No keyboard at all: try another USB port and avoid hubs. Details: [`BOOT-TEST.md`](BOOT-TEST.md). **Tried:** on the Mac mini 2018, the MacBooks, the HP and the Lenovo.

## Safe graphics (black screen)

1. Restart, press a key at the logo, **More options**, **Safe graphics (no GPU drivers)**.
2. **You should see** the IYAKKI screen drawn by the simple framebuffer; it is slower and may not use the monitor's best size.
3. On a Mac, unplug and plug the HDMI cable again **before** power-on.
4. If safe graphics works and the normal entry does not, the machine's GPU driver is the cause. Write down the machine and GPU and report it ([`TESTED-HARDWARE.md`](TESTED-HARDWARE.md#add-your-machine)). With an NVIDIA card also try **NVIDIA driver (nouveau off)**.

**Tried:** safe graphics boots in QEMU; no note records that a real machine needed it. It is a fall-back, not a fix: a 4K monitor on a Mac mini 2018 once listed only 1080p on the first USB-C plug and another cable gave 4K; the cause is not proven.

## Reading the boot messages

Pick **Show boot messages**. The screen then shows the kernel's text instead of the logo; if it stops, photograph the last lines. In a virtual machine the log is in the file named on the screen where QEMU writes its serial output. On an installed system `dmesg` (from the Terminal tile or SSH) shows the same.

Boot lines of the loader and stage 1 start with `stage1:`; a message such as `no IYAKKI-BOOT or IYAKKI-DATA volume appeared` means the stick or disk driver did not come up (try **Compatible USB mode**).

## Remote diagnostics

Pick **Remote diagnostics (SSH, keys only)** (it starts the system with `iyakki.ssh=1`). It opens an SSH server on port 22 that accepts **keys only, never passwords**, and shows the machine's address on the sign-in screen. It works only on an image that was built with **your own public key** inside (`IYAKKI_SSH_AUTHORIZED_KEYS=<file>` when building, see [`ADMIN-GUIDE.md`](ADMIN-GUIDE.md#logs-and-diagnostics) and [`CLI-SSH.md`](CLI-SSH.md)); a downloaded image has none, so for a downloaded image use the hardware check, the boot messages and the diagnostics file instead.

Once connected: `iyakki doctor --detail` (checks in plain words), `iyakki hwreport` (hardware), `dmesg`. **Tried:** on the Mac mini 2018, in the project's own early test loop, with an image built with the owner's key. **Not tried:** a downloaded image (it has no key).

## Going back to the previous version

Every update keeps the version it replaced.

* **From the running system:** **Settings, System update, Roll back**. It asks first. Restart when asked.
* **From the boot menu:** pick **IYAKKI (previous version)** on the first level. The loader also starts it **by itself** when the new version fails to start several times in a row (it counts the tries).
* Your accounts, files and settings are **not** touched either way.
* The row exists only after the first update. A freshly written stick has none, and there is nothing to go back to.
* **Roll back is refused while an update is unfinished.**

Details: [`UPDATES.md`](UPDATES.md), [`BOOT-LOADER.md`](BOOT-LOADER.md). **Tried:** the kept-version swap and the automatic fall-back with a folder as the boot partition and in QEMU; updates through developer mode on the Lenovo and the Mac mini 2018 (alpha.6). **Not tried:** the automatic fall-back after a real failed boot on a real machine.

## The seed root: last-resort start

Every image carries a first root system (the **seed root**, `boot/root.sfs` on the boot partition) next to any newer root that updates put on the data volume. If an update is broken so badly that even the previous version does not start, start the seed root: it ignores everything the data volume holds for the system, **and keeps your accounts and files**.

1. At the menu, open **Recovery (GRUB menu)** (UEFI), or use the GRUB menu of a BIOS or ISO start.
2. Put the cursor on **IYAKKI** and press `e`.
3. At the end of the line that begins with `linux`, add a space and `iyakki.root=boot`.
4. Press `Ctrl+X` (or `F10`) to start.

**You should see** IYAKKI start from the seed version. Use **Settings, System update** from there to install a good version, or write the stick again ([next sections](#reinstalling-and-keeping-your-data)).

A second option for a stuck early start: `iyakki.stage1=shell` instead stops in a small shell on the screen, for someone who knows what to look at. On a Raspberry Pi there is no menu: see the next section.

**Tried:** the option is read by stage 1 and covered by its unit tests; the seed start was used on the HP laptop stick, where the default entry was given `iyakki.root=boot` so that the root next to the kernel was used (project notes, 2026-11-07). **Not tried:** the GRUB steps above, typed by a newcomer on a real machine.

## Raspberry Pi 5 and KV260

A Pi has no boot menu. With the card in a computer, copy one of the `cmdline-*.txt` files on the card over `cmdline.txt` (`cmdline-hwcheck.txt`, `cmdline-safegraphics.txt`, `cmdline-selftest.txt`, `cmdline-ssh.txt`, `cmdline-web.txt`, or `cmdline-seed.txt`, which boots the seed root and ignores updates). Copy `cmdline-normal.txt` back to go back to the normal start ([`BOARD-RPI5-FIRST-BOOT.md`](BOARD-RPI5-FIRST-BOOT.md)). The KV260 shows a small menu on its serial console with fall-back entries ([`BOARD-KV260-FIRST-BOOT.md`](BOARD-KV260-FIRST-BOOT.md)). **Not tried on either board.**

## Resetting an account or a forgotten password

There is **no way to recover your own password without the old one**, and no recovery key for accounts. An administrator can reset another person's password. If the lost password belonged to the only administrator, use the rescue steps.

**Another administrator exists:**

1. Sign in as that administrator and open the **USERS** tile.
2. Select the person and press `p` (reset password). Set a new password.
3. **A password reset also resets that person's personal vault.** Its contents (their saved keys) are lost; the system vault is unaffected if another administrator still has a key slot. If the person has an **encrypted home**, it stays closed until the recovery key is used ([next section](#encrypted-home-the-recovery-key)).

**The only administrator forgot the password:** you need a shell on the machine: the remote diagnostics entry, a window-mode terminal, or the data volume mounted on another Linux computer. Then:

```
echo 'a new long password' | iyakki user add rescue --role admin --password-stdin
```

(with `IYAKKI_USERS=/path/to/users.json` when the file is not at the default `/var/lib/iyakki/users.json`; the command refuses an existing name and never takes a password as an argument). Sign in as `rescue`, open USERS, select the old administrator, press `p`, then delete `rescue` with `d` if you do not want it. A fresh **live ISO** with nothing saved simply asks for a new administrator.

**Tried:** `iyakki user add` and the USERS keys are covered by unit tests; the rescue recipe was checked against the command. **Not tried:** the whole rescue from a real machine's data volume.

## Encrypted home: the recovery key

If you turned on **Settings, Encrypted home**, your private folder is stored in an encrypted container and opens at sign-in with your sign-in password. When you made it, the page showed a **recovery key** once (fourteen groups of four characters, with check letters) and asked you to type it back to prove you had saved it. **Nobody, including the project, can open the folder without the password or the recovery key.**

When the home does not open (the password was reset by an administrator, or you forgot it):

1. Sign in. Sign-in still works and you get a plain notice that your private folder could not be opened; you never land in an empty home that looks real.
2. Open **Settings, Encrypted home**. The page says the folder is sealed.
3. Choose **Open with recovery key**, type the key (spaces and lower case are fine; a mistyped character is caught by the check letters and the message says so), and set a current password when asked.
4. Make a **new recovery key** afterwards (Settings, Encrypted home): only one key exists at a time and the old one stops working. Save the new one where only you can read it.

The assistant can show the state and plan a move for free; making the container, locking, unlocking, a new key and moving files always ask you, and no tool takes a password or a key as an argument.

**Tried:** all of it with a recording fake kernel and test folders; on a real kernel, the encrypted home was run on a Mac mini 2018 on 2026-11-10 and three bugs it showed were fixed (owner's run). **Not tried:** the recovery-key path on a real machine after the fixes. A damaged container header is repaired from its second copy; losing both copies loses the home.

## Reinstalling and keeping your data

Be exact about what exists. **No installer mode keeps an existing IYAKKI data volume:** the stick writer and the installer write a new data partition, and a whole-disk install erases everything on the disk. Three real ways to keep your files:

1. **Update instead of reinstalling.** **Settings, System update** installs a newer version from a folder or a stick and does not touch your files, accounts or settings ([`UPDATES.md`](UPDATES.md)). It is the way to move forward.
2. **Back up first, restore after.** **Settings, Account and people, Backup and restore** writes one file with your settings, bookmarks, chats, notes, flows, recordings and your vault (the vault stays encrypted; the file itself is not encrypted unless you give a passphrase of 8 or more characters). Keep it on another stick. After the new install, sign in and restore from the same page. It does not include VPN or SSH keys, tokens, other people's data, models or the store ([`BACKUP.md`](BACKUP.md)).
3. **Copy the data volume by hand.** The data partition has the label `IYAKKI-DATA` (ext4). Mount it on another Linux computer and copy it; your accounts are in `users.json`, vaults in `vault/`, files in `home/`. Everything except the vaults is **not encrypted**, so keep the copy private ([`INSTALL.md`](INSTALL.md#7-upgrade-back-up-uninstall-recover)).

Then write the new image with the stick writer ([`INSTALL-GUIDE.md`](INSTALL-GUIDE.md#3-write-the-stick-or-card)). **Tried:** the backup and restore code with test folders (restore is fully tested); a restore on a real machine is not tried. The hand copy is plain ext4 files.

## Collecting a diagnostics bundle

1. Open the **DOCTOR** tile and press `s` (or **Settings, Overview, Share diagnostics**).
2. A window shows the **whole** file, line by line, before anything is written; what you see is exactly what is saved. Passwords, tokens, cookies, key blocks, e-mail addresses, network addresses and your account name in paths are removed; very long hexadecimal strings are removed too.
3. `s` or Enter saves `iyakki-diagnostics-YYYYMMDD-HHMMSS.txt` in your home folder. Keys `1` to `5` save it on a USB stick that the DISKS tile opened. A file is never replaced. Nothing is uploaded: you attach it to your report yourself, after reading it.
4. With no screen: `iyakki doctor --detail` prints the same checks as text, and `iyakki hwreport` prints the hardware.

The file holds the version, services, the DOCTOR findings, the newest check-up, switches that are on, hardware, the web engine log and the end of the boot log; at most 512 KB. Report problems as described in [`SUPPORT.md`](../SUPPORT.md); security problems go through [`../SECURITY.md`](../SECURITY.md), not a public report. More: [`DOCTOR.md`](DOCTOR.md).

**Tried:** the bundle and its redaction by tests (the text on screen and the file are compared byte for byte). **Not tried:** a diagnostics file saved and read on a real stick in a first-time person's hands.
