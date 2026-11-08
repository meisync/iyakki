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

## Checking for updates (F-UPD-04)

System update shows a line for how often to check (every day, only on request, off), when it last looked and what it found. Check for updates
looks at every store source and, when an administrator set one with Update list address, a small JSON file `{"version": "...", "variant":
"...", "notes": "..."}` that announces the newest IYAKKI. There is no built-in address. Instead of typing one, **Use a store** (key `t`)
sets the list to the signed announcement of a private store that was added with its key checked: `<store>/v1/system.json` (see
STORE.md, section 13: `{"version","variant","notes","channel"}`, signed with the store's catalog key; the device accepts it only with
the pinned key and refuses an unsigned or tampered one; no file means nothing is announced). It only reports: nothing is downloaded or
installed. A system update is installed from a folder or stick on this page; an app update in the STORE, which shows new permissions first
(the check marks such updates). Packages that are in no catalog (a developer's own) are never listed.

**When it looks.** The default is every day: a real background timer that runs while an administrator is signed in, with a stable delay of
up to an hour per device, and once at sign-in when the last look is older than 24 hours. It asks only what was added on this computer
(stores, folders with a web address, the update list address); the built-in offline catalog is never contacted and with nothing added
nothing is fetched. It can be set to "only on request" or "off" in System update (How often), Store -> Settings (`o`), `iyakki dev config
set store.daily_check false` (true/false; "false" never turns off into on) or by chat (`updates_settings_status`, `updates_settings_set`,
which asks). This computer cannot tell a metered connection yet, so the daily check also runs on one.

**What notifies.** When a check finds something new the device leaves ONE inbox notice for the administrator who was signed in, such as
"Updates available: 3 apps, system 1107h" (counts for apps and skills, driver packs and the system, with up to three names in its text).
The notice is replaced in place when the set changes (same entry, never stacked day after day): a new version of anything updates the text
and marks it unread again; if only fewer updates remain (something was installed) the text follows and the reading state stays; when
nothing is pending any more it is removed. Dismissing it is respected: the same set is not announced again. A source that cannot be
reached does not remove the notice or make an old update look new. Clicking it opens the STORE on Installed with the Updates filter (which
adds a line for a waiting system update) or, when only the system is waiting, Settings -> System update. The same count shows on the STORE
tile, in a chip in the top bar, in the Overview and on the System update line, and drops at once when an update is installed. The notice
carries titles and versions only, and nothing is ever installed automatically. Notices from "store" can be quietened or turned off in
Settings -> Notifications like any source. `updatenotice::announce_system_update(version, notes)` is the hook for the planned component
manifest: it records "version X is out" so the badge, the System update page and the notice show it until the running version catches up.
The notice goes to the person who was signed in at the check, not to every administrator. Not built: Update all in one click for the
system, and keeping the previous package version for rollback.

## System components from a store (apply path)

A private store the person added with its key confirmed can offer separate system parts: kernel, boot, base, core and the kernel layer
(docs/development/COMPONENT-UPDATES-PLAN.md). The way they are taken is the same machinery as above, not a new one:

* **See it:** the assistant's `components_status`, or the line "Components from <store>: N newer" in Settings -> System update (kept from
  the last "Check for updates"). Both only read the store's signed list.
* **Approve it:** the assistant's `components_apply` (a hard ask, never covered by a session grant) or the button **Review and apply** in
  Settings -> System update. Both show the same review: what changes, how big (only the difference or the whole file), whether a restart is
  needed, and what is kept to go back to. Approving that review (the card, or `y` on the page) is the approval; it is bound to that exact plan,
  so a list that changed after it was shown is not installed.
* **What happens:** each file is downloaded (resuming) and checked against the SHA-256 in the store's signed list; a difference is applied to
  the installed file as a stream (a few hundred KB of buffers, not the file size). **Kernel, boot and base** become an update payload and go
  through this page's engine: previous version copied to the data volume and kept in the boot menu as "IYAKKI (previous version)", UPDATING
  marker, atomic replace, boot counter, trial record. Every written file is then read back against the signed hash; if one differs, the previous
  version is put back (the same as **Roll back**) and nothing is recorded. **Core** (and the kernel layer) become a patch pack, a difference
  against the running program, stored for the next start; the running program is not replaced, and the patch start counter turns the layer off
  by itself if starts keep failing.
* **What it refuses:** no approval, free space short (work folder, boot partition, data volume), a boot partition that is not this install's
  (found on the data volume's own disk, with an IYAKKI boot menu), a `requires` range that would break (it names the part to update first), an
  update already written that waits for a restart or an unfinished one, and a release label that is not the running one unless the base is part
  of the same update (the base carries the label the first start is checked against).
* **What it records:** `components-installed.json` in the data folder gets the new versions and hashes only after the read-back succeeded, and
  marks them as waiting for the restart. At every start (and whenever the page or the assistant reads it) the record is checked against facts: the
  running kernel release, the `VERSION` of `/etc/iyakki-release` and the version the running program was built as. A record that disagrees is
  replaced by the fact, so a rollback or a fall-back start cannot leave it lying.
* **The label:** the store signs the IYAKKI version and image variant its parts belong to into the list (`release`); the payload uses it. A
  partial update (say only a kernel) must carry the label of the install it goes onto.
* The computer never restarts by itself.

Not proven: any of this on a real boot partition, a real 400 MB base image, the real mount, a real store, or a restart into the result (see
docs/development/COMPONENT-UPDATES-PLAN.md, "Built so far").

## What is not built or not proven

* **No download from the internet.** There is no update index yet (open decision D6 in docs/development/IMPROVEMENTS-PLAN.md: where it is
  hosted, who signs). Folder and USB stick only. No USB-C cable path on this page (that is `iyakki dev update` from a laptop).
* **The automatic fall-back is built and tested with a folder as the boot partition only** (F-UPD-02). An install or a rollback adds a small
  guard to the boot menu (`grub.cfg`) and sets `iyakki_trial=1` in `boot/grub/grubenv`. At the next start the guard changes it to 2 and boots the
  new set; the running system sets it to 0 once the data volume is mounted and the shell has drawn its first picture. If the next start finds a 2
  (the new set never got that far) the menu makes "IYAKKI (previous version)" the default for that start. The guard is only written together with
  that entry, and nothing is armed when the menu has no guard or no such entry. Not proven: that the image's GRUB has the `loadenv` module and
  reads `grubenv` from the boot partition (BIOS and UEFI, Secure Boot), and the whole loop on a real machine. A start that is restarted by hand
  before the first picture also counts as a failed try.
* The check after a start still runs when Settings is opened (it writes the page's note); the boot counter above is what acts at boot.
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
