# System update (Update centre)

Settings -> System update (administrators). It installs a newer IYAKKI from a **folder or a USB stick**, keeps the version you have, and
**rolls back** to it. It is the developer-mode update (`iyakki dev update`, docs/DEVELOPER-MODE.md section on system updates) with a page
in front of it, so a person does not need a developer computer. Status of each part: FEATURES.md rows F-UPD-05 to F-UPD-07.

## What a person does

1. Put the update on a stick (or in a folder). An update is a folder that holds `update.json` and a `boot/` (and maybe `EFI/`) folder. Nobody
   has to unpack anything: the folder is what `iyakki dev update-pack IMAGE.img -o DIR` makes.
2. Settings -> System update. The page shows **Installed** (the version whose files are on the boot partition), **Running now** when that differs
   (an update waits for a restart), and **Kept before it** (the previous version).
3. The page lists the updates it finds on mounted drives (`/media`, `/run/media`, `/mnt`); **Choose folder** (key `f`, with Browse) adds
   a folder, **Look again** (`s`) reads the drives again. Up/Down chooses one.
4. Under the list the page says in plain words what will change: the version, how many boot files and megabytes are replaced, that the
   person's files, accounts and settings are not touched, and that the current version is kept. Lines that begin **Needs your yes** are the
   reasons it asks: another image variant (for example `mac` on a `pc` device), an older version, a pack with no signature.
5. **Install** (`i`) asks again in one sentence and shows those reasons; **Yes, do it** (`y`) writes, **No** (`n`) leaves everything as it was.
   The page warns not to remove the drive or switch the computer off while it writes (a few minutes on a slow stick).
6. When it is done the page says **Restart when you are ready**. Nothing restarts by itself and there is no restart button on this page.
7. **Roll back** (`r`) asks first, then puts the kept version back on the boot partition (and keeps the one you leave, so you can go forward
   again). Restart afterwards. Your data is not touched either way.

## What is checked, in order

* The pack's `update.json` is valid (format, plain `boot/...` or `EFI/...` paths, sizes, SHA-256 per file, the files every update needs).
* **Signature.** `update.json.sig` is a detached ed25519 signature over the exact bytes of `update.json`, checked with the key that already
  signs the downloads manifest (`legal/downloads-manifest.pub`, `iyakki-agent::dlmanifest`). A wrong signature is refused, always. No signature
  means **Needs your yes**. The owner signs with `iyakki dev update-pack IMAGE.img -o DIR --sign KEYFILE` (the key file holds 64 hex characters
  and stays on the owner's computer). The per-file SHA-256 values in the signed `update.json` then cover every payload file.
* The same refusals as the developer path (`devmode/update.rs`): a different **variant** or an **older version** (both can be lifted by the
  person's yes); a changed **Secure Boot** chain, a RAM-only data volume (the previous version could not be kept), too little room on the
  data partition, or new files that do not fit on the boot partition (these cannot be lifted).
* **Before anything is written** every payload file on the drive is read and compared with its SHA-256. A damaged or missing file stops the
  install with nothing written, not even staged. If the pack changed on the drive between the preview and the click, it is refused.
* Writes go only to the **device's own boot partition** (found on the same disk as the data volume, never another drive) and to the engine's own
  folders on the data volume (`boot-next/`, `boot-prev/`, `update-trial.json`). The order, the marker for an interrupted update, the read-back
  of every file and the menu entry **IYAKKI (previous version)** are the developer-mode engine's, unchanged.

## After the restart

`update-trial.json` remembers what was installed and which start it was written in. When Settings is opened after a restart:

* the running version is the new one: the update is marked **good** and the page says so;
* the running version is still the old one: it is recorded as a **fall-back** (the person chose "IYAKKI (previous version)" in the boot menu, or
  the new version did not start) and the page and the Overview say so. The kept version still boots from the menu.

An update that was cut (power loss) shows on the page and the Overview with the usual sentence: choose "IYAKKI (previous version)" in the boot
menu, then install again. Roll back is refused while an update is unfinished.

## What is not built or not proven

* **No download from the internet.** There is no update index yet (open decision D6 in docs/development/IMPROVEMENTS-PLAN.md: where it is
  hosted, who signs). Folder and USB stick only. No USB-C cable path on this page (that is `iyakki dev update` from a laptop).
* **The fall-back is not automatic.** GRUB does not count boots yet (F-UPD-02 stays planned). The check after a start runs when Settings is
  opened, not at boot.
* A power cut during **Roll back** leaves the boot menu entry "IYAKKI (previous version)" pointing at the version being restored (tested with
  a fake power cut in the engine), but this was never run on a real boot partition.
* Everything above was tested with fake packs and a folder standing in for the boot partition. The real mount of `IYAKKI-BOOT`, the speed of the
  in-place initramfs write on a stick and GRUB reading `boot-prev` from the data partition are the open points of docs/DEVELOPER-MODE.md.

## How the owner can try it (nothing here was done by the assistant)

On a test stick or the Mac mini with the disk install, not the only copy of anything:

1. On the build computer make a pack from the newer image and sign it:
   `iyakki dev update-pack iyakki-pc.img -o /tmp/pack --sign ~/keys/release.key` (a second, unsigned pack tests the "no signature" yes).
2. Copy `/tmp/pack` to a USB stick, plug it into the IYAKKI computer, sign in as an administrator.
3. Settings -> System update. Check that the stick's pack appears as **signed**, read the preview, press `i`, then `y`.
4. Do not remove the stick until the page says Restart. Restart from the power menu. Check the version in System -> About.
5. Open Settings -> System update again: the page should say the new version is marked good.
6. Press `r`, `y`, restart: the old version should run. Also try the boot menu entry "IYAKKI (previous version)" once.
7. Optional: try an older or other-variant pack to see the "Needs your yes" lines, and cut the power during step 3 on a spare stick to see the
   unfinished-update sentence.
