# Hardware firmware: the steps by hand

The Setup item **Hardware firmware** (Store, Setup, Hardware; `crates/iyakki-agent/src/fwextra.rs`) does these steps with a click. This folder keeps
them as scripts so a developer can do the same, or prepare a USB stick on another computer. IYAKKI release images carry no vendor firmware
(licence gate, `docs/FIRMWARE-LICENCES.md`); the person fetches what the computer asks for, from the upstream linux-firmware release.

| Script | What it does |
|---|---|
| `pin.sh TAG COMMIT [TARBALL]` | Writes `dist/firmware/manifest.txt` from kernel.org's real release tarball: the snapshot sha256, and size, sha256 and licence text of every file named in WHENCE. Move the pin by running it with another tag and reviewing the diff. |
| `fetch.sh (--names A,B \| --log FILE) [--dry-run] [--with-nvidia] [--snapshot FILE\|download] [--out DIR]` | Looks the names up in the manifest (links followed, `.ucode` versions collapsed), prints the list with `--dry-run`, otherwise fetches only those files and their licence texts from git.kernel.org at the pinned commit (or from the snapshot tarball), checks size and sha256, and writes `DIR/firmware-extra/`. |

Pinned release: **20261019**, commit `a2dcb924f0897368b0b4f305cbf9808cdda90ce0`, tarball sha256 `f37337e66c1af8ab488972f09ef17efb9b1a2663ed339fc73860936d1af04831` (1021468302 bytes).

## The steps

1. On the computer that lacks the firmware: `dmesg > log.txt` (the lines `Direct firmware load for X failed` name the files). Or, from another computer paired over the cable: `iyakki dev dmesg --device NAME > log.txt`.
2. On a computer with network: `fetch.sh --log log.txt --dry-run`, read the list and the licence texts it names (the manifest lines `file SIZE SHA LICENCE-TEXT PATH`, texts under `LICENSES/` in the release). Some are free to redistribute with notice, some are restricted to the vendor's hardware, NVIDIA's only for NVIDIA processors and open-source systems.
3. `fetch.sh --log log.txt --out stick/` then copy `stick/firmware-extra` to a USB stick; or push over a USB-C cable: `iyakki dev files push stick/firmware-extra '~/firmware-extra' --device NAME` (the `iyakki firmware fetch --for NAME --usb-link` command does steps 1 to 3 in one go).
4. On the device: Store, Setup, Hardware firmware, choose the stick (or "files the other computer sent"), read the licences, accept. The device checks every file again and installs only after that click. Restart only if the screen says so.
5. Undo: the same card, "Remove the extra firmware" (or `iyakki quick-install fw-hardware --remove`).

No Internet at all on the device: the cable can also share the other computer's internet, `iyakki usb share-internet` (see `docs/DEVELOPER-MODE.md` section 10.6).

`crates/iyakki-agent/src/fwextra_cli_tests.rs` (`the_developer_script_and_the_item_choose_the_same_files`) runs `fetch.sh --dry-run` and the item's own list logic on the same names and fails if they differ.
