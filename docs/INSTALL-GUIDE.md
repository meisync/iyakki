# IYAKKI install guide

**For:** a person who has never seen IYAKKI and wants to put it on a computer, a board or a virtual machine.
**You will get:** the right image for your machine, a written stick or card, a first start, your first account, and three useful things to do next.

> **Preview for developers and testers.** IYAKKI is not finished. The writer **erases the drive you choose**. Use a spare USB stick or SSD and, if you can, a spare computer. Every claim below says what was tried and where; the list of machines is in [`TESTED-HARDWARE.md`](TESTED-HARDWARE.md). A machine that is not listed there has **not been tried**. Parts of this guide that nobody has walked through on a real machine are marked **Not tried**.

Contents: [1 Which image for which machine](#1-which-image-for-which-machine) · [2 Check your download](#2-check-your-download) · [3 Write the stick or card](#3-write-the-stick-or-card) · [4 First start](#4-first-start) · [5 First-run screen and signing in](#5-first-run-screen-and-signing-in) · [6 Your first useful things](#6-your-first-useful-things) · [7 If something goes wrong](#7-if-something-goes-wrong)

## 1. Which image for which machine

| Your machine | Use | Where it was tried | Read |
|---|---|---|---|
| A 64-bit PC (Intel or AMD, UEFI or old BIOS) | `iyakki-install-x86_64.img` on a USB stick or SSD of 8 GB or more | Booted by the project's owner on an HP laptop, a Lenovo laptop and one AMD PC; many more checks in QEMU | [`TESTED-HARDWARE.md`](TESTED-HARDWARE.md) |
| An Intel Mac (Mac mini 2014 and 2018, MacBook Pro 2017 and 2020) | the same install image on a USB SSD, then hold the Option key at power-on | Booted on all four of those Macs. Wi-Fi and Bluetooth need Apple's firmware, installed by you ([`MAC-WIFI-BLUETOOTH-SETUP.md`](MAC-WIFI-BLUETOOTH-SETUP.md)); the 2013 to 2015 Macs with a Broadcom BCM4360 card have no Wi-Fi driver, so use a cable | [`MAC-HARDWARE.md`](MAC-HARDWARE.md) |
| An Apple Silicon Mac (M1 and later) | **No bare-metal image.** Run it in a small virtual machine instead | The recipe brought up a working cluster node once, in 2026-11, on one Apple Silicon Mac; the rest of it is untried | [`APPLE-SILICON.md`](APPLE-SILICON.md) |
| Raspberry Pi 5 | `iyakki-rpi5.img` on a microSD card of 16 GB or more, plus a 3.3 V serial adapter | **Not tried on a board.** The files booted in QEMU only | [`BOARD-RPI5-FIRST-BOOT.md`](BOARD-RPI5-FIRST-BOOT.md) |
| AMD Kria KV260 | `iyakki-kv260.img` on a microSD card of 8 GB or more, plus a micro-USB cable for the serial console | **Not tried on a board.** The userspace booted in QEMU only | [`BOARD-KV260-FIRST-BOOT.md`](BOARD-KV260-FIRST-BOOT.md) |
| A Linux desktop, to look around without touching a disk | The **window build**: `./run.sh window` from a source checkout, or the `iyakki-desktop` tarball of a release with its `run-window.sh` | Runs on the project's Linux machines every day. A Mac or Windows window build is not offered | [`INSTALL.md`](INSTALL.md#1-window-mode-linux) |
| Any computer with QEMU | `iyakki-install-x86_64.img` as a USB disk in a virtual machine | The sign-in screen came up about 40 seconds after QEMU started (2026-11-18, one Linux PC) | [`QUICKSTART.md`](QUICKSTART.md#2b-or-a-virtual-machine) |
| VirtualBox, VMware, Hyper-V | not tried | none | [`HARDWARE.md`](HARDWARE.md) |

Two x86_64 images exist: the **install image** (`iyakki-install-x86_64.img`, keeps your account and files when you turn the machine off; **choose this one**) and the **live ISO** (`iyakki-x86_64.iso`, runs from memory and forgets everything at power-off unless you add a data disk). Bigger variants with the full web engine (`web`, `macweb`) are built from source and take hours; they are not part of the first try ([`INSTALL.md`](INSTALL.md#5-variants-web-mac-arm64)).

IYAKKI ships **no AI model**. Chat needs one that you connect in Settings (step 6).

## 2. Check your download

Put the install image, `SHA256SUMS` and the signature files from the release page in one folder. Then:

```
sha256sum -c SHA256SUMS --ignore-missing          # Linux; on a Mac: shasum -a 256 -c SHA256SUMS --ignore-missing
iyakki release verify DIR --pubkey HEX            # DIR is that folder, HEX is the public key printed on the release page
```

**You should see** `OK` next to each file you downloaded, and a good signature from the second command. If either fails, download the file again and do not write it. The `iyakki` program comes from a source build or from a running IYAKKI.

**Not tried:** `iyakki release verify` on a file downloaded from a public release page (the checks ran on the project's own release folders).

## 3. Write the stick or card

### A PC or an Intel Mac: the stick writer

On Linux or macOS, build the writer once from the source tree (a release may also carry it) and list the drives. Listing needs no administrator rights and changes nothing:

```
cargo build --release -p iyakki-usb
target/release/iyakki-usb --list
```

**You should see** your USB drives, each marked as writable or refused. The writer lists only USB drives unless you add `--all`, and always refuses the disk your computer runs from, mounted disks, write-protected disks and the disk that holds the image file.

Write the stick (**this erases the whole stick**):

```
sudo target/release/iyakki-usb --install iyakki-install-x86_64.img
```

A menu lets you choose the drive with the arrow keys. A red **THIS WILL ERASE THE WHOLE DRIVE** box shows the drive, its model and its size. You must **type the drive name** (for example `sdb`, or `disk4` on a Mac) and press Enter; Enter alone cancels. When it finishes it reads the stick back and says it is verified.

* `--table gpt` or `--table mbr` picks the partition table; the default (`auto`) is right for most sticks.
* On Windows there is no tested writer. Rufus in DD Image mode or balenaEtcher should work but nobody on the project tried them.
* No tool at hand: `sudo dd if=iyakki-install-x86_64.img of=/dev/sdX bs=4M conv=fsync status=progress`, after you check the device name with `lsblk`. A wrong name destroys that disk.

**Tried:** the writer wrote the project's sticks and SSDs for the four Macs, the HP and the Lenovo (2026-10 and 2026-11); its macOS paths were not run on a Mac. **Not tried:** a stick written from an image downloaded from a public release.

### Installing to a computer's own disk

After you have booted the stick once, pick **IYAKKI - Install to a disk** in the boot menu (or run `iyakki install`, or `sudo iyakki-usb --plan IMAGE` from another Linux computer). You choose the disk and one of four ways: free space only (touches nothing else), chosen partitions, custom sizes, or the whole disk. Every plan is shown as a table first and needs your confirmation before it erases anything. It never offers the stick you booted from. Details and the safety rules: [`INSTALLER.md`](INSTALLER.md).

**Tried:** an internal-disk install on a MacBook Pro 2020 and a Mac mini 2018 (owner). **Not tried:** dual boot next to Windows on a real PC (disk images only).

### Raspberry Pi 5 and KV260: the card

Both write a microSD card with the same safety rules and have their own first-boot page with the exact commands, the serial console settings and what you should see on each line: [`BOARD-RPI5-FIRST-BOOT.md`](BOARD-RPI5-FIRST-BOOT.md) and [`BOARD-KV260-FIRST-BOOT.md`](BOARD-KV260-FIRST-BOOT.md). A Pi has no boot menu; the recovery guide says how to pick another start there.

### A virtual machine

On Linux with QEMU and the `ovmf` firmware package, the exact command is in [`QUICKSTART.md`](QUICKSTART.md#2b-or-a-virtual-machine). Click the window to give it the mouse; **Ctrl+Alt+G** gives it back.

## 4. First start

1. **Secure Boot.** Most computers ship with Secure Boot on, and IYAKKI images are signed with the project's own key, not one your firmware knows. For a first try, switch Secure Boot **off** in the firmware settings. To keep it on, enrol the project's key once (a blue screen called MokManager, steps in [`QUICKSTART.md`](QUICKSTART.md#3-boot-the-stick-secure-boot) and [`SECURE-BOOT.md`](SECURE-BOOT.md)). Apple T2 Macs also need the Mac's own security setting changed ([`MAC-HARDWARE.md`](MAC-HARDWARE.md)). *Tried:* Secure Boot on with the key enrolled booted on the HP and the Lenovo laptops. *Not tried:* a first-time enrolment from a public release by someone new to the project.
2. **Open the boot menu** with the stick in: F12 (Dell, Lenovo), F9 (HP), Esc or F8 (Asus), F10, or hold **Option** on an Intel Mac. Pick the stick (on a PC, usually the entry that starts with "UEFI").
3. **The IYAKKI logo** appears. With no key pressed, the normal entry starts by itself after 3 seconds. Press any key to see the menu below the logo.
4. **The first start of a new stick** formats the data part of the stick, which takes a few seconds. The screen then shows the welcome screen (next section).

**If the screen stays black** or something does not start, do not keep power-cycling: read [`RECOVERY-GUIDE.md`](RECOVERY-GUIDE.md). The menu entries *safe graphics*, *hardware check* and *show boot messages* are the first things to try.

Real timing: on the Mac mini 2018 the first IYAKKI picture comes several seconds after the kernel starts because of the Apple firmware's USB wait (written down, not yet re-timed with the current loader). In QEMU the sign-in screen is up in under a minute including the firmware.

## 5. First-run screen and signing in

On a stick with no accounts yet the first screen says **Welcome, create the administrator account**.

1. Type a name: lowercase letters, digits, `-` and `_`.
2. Type a password of at least 8 characters, twice. Tab moves between the fields; Enter or the **Create account** button continues.
3. The sign-in screen follows. Sign in with the same name and password.
4. A card may ask **Set the time zone from the network?** It does one lookup at a public service. **Use UTC** and **Not now** are the other answers. Settings, Time zone and clock changes it later.

**You should see** the launcher: tiles sorted into groups (Ask and Create, Build, Media, Data, Devices and System) and your name and role in the top bar.

Things to know now:

* **There is no automatic sign-in.** You sign in with your password every time, because your password also opens your vault (where model keys are kept).
* **Nobody can recover a lost password for you.** An administrator can add another administrator from a shell, and a reset of a password also resets that person's vault. Read the [recovery guide](RECOVERY-GUIDE.md#resetting-an-account-or-a-forgotten-password) before you need it.
* Esc or `q` on the launcher signs you out (a card asks first for some keys); inside an app Esc goes back to the launcher.

**Tried:** creating an account, signing in and finding the account again after a restart, in QEMU (2026-11-18) and on the project's real machines. **Not tried:** the whole path from a public release by a new person.

## 6. Your first useful things

1. **Connect a model.** Open **Settings** (the gear in the top bar, or `s`), **Models**, add a provider (an Ollama server on your network or this computer, or a cloud service with your own key; the key goes into your encrypted vault and is never shown to the assistant), press `m` to make it the chat model and `c` to test it. Walkthrough: [`QUICKSTART.md`](QUICKSTART.md#5-your-first-question).
2. **Ask a question.** Open **CHAT** and type "What can you do on this computer?". Press `Ctrl+G` for Agent mode so the assistant may use tools. Every risky action pauses with **Allow once**, **Allow for this session** or **Deny**; saying nothing means deny.
3. **Run DOCTOR.** Open the **DOCTOR** tile (Devices and System) and press `r`. It checks the computer in plain words and gives each problem one fix button ([`DOCTOR.md`](DOCTOR.md)). Run it once on a new machine: it tells you about sound, network, the clock and disk space.

Then the tour in [`QUICKSTART.md`](QUICKSTART.md) and the user guide ([`USER-GUIDE.md`](USER-GUIDE.md)).

## 7. If something goes wrong

| What you see | Go to |
|---|---|
| Black screen, no keyboard, no network, no sound, will not start | [`RECOVERY-GUIDE.md`](RECOVERY-GUIDE.md) and [`TROUBLESHOOTING.md`](TROUBLESHOOTING.md) |
| You wonder whether a feature works yet | [`KNOWN-LIMITS.md`](KNOWN-LIMITS.md) |
| Your machine is not in the tested list | run the **hardware check** boot entry, then report with `iyakki hwreport` (see [`BOOT-TEST.md`](BOOT-TEST.md)) |
| A bug | [`SUPPORT.md`](../SUPPORT.md); please attach a diagnostics file (recovery guide, last section) |
