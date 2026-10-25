# Lock screen, encrypted data partition and encrypted vault partition

**Status: DESIGN FOR THE OWNER TO APPROVE (2026-10-24, owner request). Nothing here is built.** No code, script or image was changed
to write it. Tags: **EXISTS** (read in code or docs; path given), **GAP** (missing today), **PROPOSED** (this design). Numbers marked
"expected" are estimates, not measurements. Licence remarks are the owner's working rule, not legal advice.

Three requests: (1) a lock screen, (2) an option to encrypt the data partition, (3) an option for a separate encrypted vault partition.
They are related (the lock screen is what keeps a sealed vault sealed while the machine is on) but can ship separately.

## 0. What exists today

| Fact | Where | Tag |
|---|---|---|
| No lock screen. The saver wakes straight into the session; signing out is the only protection | `docs/SCREEN-SAVER.md` "Not built", `docs/development/TOTP-2FA.md` section 2 | GAP |
| Screens are an enum: HwCheck, Setup, Login, Launcher, App, PowerMenu; the session is `Option<Session>`; `sign_out()` stops flows, shared folders, services, the assistant's browser tab, then shows Login | `crates/iyakki-shell/src/shell.rs` (`enum Screen` line 203, `session` line 317, `sign_out` line 898) | EXISTS |
| Idle machine with stages Awake, Dim, Saver, Off; inhibitors; the first input only wakes and is not delivered to the app | `crates/iyakki-shell/src/saver.rs` (`Stage`, `IdleMachine`), `shell_saver.rs` | EXISTS |
| Sign-in password check: Argon2id PHC in `users.json`; throttle in memory only | `crates/iyakki-shell/src/accounts.rs` (`authenticate`, `throttle_left`) | EXISTS |
| Remote (browser) screens are shells too (`set_remote`) and have their own sign-in on port 8443 | `shell.rs` line 608, `crates/iyakki-remote` | EXISTS |
| Vault: random 256-bit data key, XChaCha20-Poly1305, key wrapped per slot with Argon2id of that slot's password; files `vault/system.afv`, `vault/users/<user>.afv` | `crates/iyakki-agent/src/vault.rs` (crates `argon2` 0.5, `chacha20poly1305` 0.10, `zeroize`) | EXISTS |
| The data volume is NOT encrypted ("vaults are encrypted, the volume is not") | `SECURITY.md` ("What the alpha does not promise"), `KNOWN-ISSUES.md` | EXISTS (a stated limit) |
| Data volume found by label `IYAKKI-DATA` (ext4); a "pending" partition (marker `IYAKKI-FORMAT-ME-v1`) is formatted at first start with `mke2fs`, then mounted at `/var/lib/iyakki` with mode 0700 | `crates/iyakki-storage/src/lib.rs` (`find_data` 200, `find_pending` 233, `format_pending` 303, `mount_data` 394, `classify` 110); `crates/iyakki-core/src/boot.rs` (`mount_data_volume` 749) | EXISTS |
| The lean first stage (about 1 MB, no process spawned) finds both volumes by label before the system exists, mounts the data volume, reads the root tree from it, and leaves it mounted; PID 1 notices (`dir_is_mount_point`) | `crates/iyakki-stage1` (`DATA_LABEL` in `lib.rs`; label scan in `main.rs` ~line 249), `docs/BOOT-SPEED.md` lines 73-80, `docs/development/SELF-PREPARING-BOOT.md` | EXISTS (experimental) |
| The self-preparing boot's tree, marker and attempts file live on the data volume: `iyakki/root/<tree>/tree/`, `.lean-complete`, `fast.attempts` | `docs/development/SELF-PREPARING-BOOT.md` sections 5-8 | PROPOSED design (not built) |
| System update stages on the data volume (`boot-next/`, `boot-prev/`) and replaces files on the FAT boot partition | `docs/DEVELOPER-MODE.md` section 7, `docs/development/REMOTE-PATCH.md` | EXISTS |
| DISKS lists LUKS and BitLocker as "Encrypted: unlock needs the key"; unlocking is not built | `docs/DISKS.md` file-system table and "Not built yet" | GAP |
| Kernel: `CONFIG_CRYPTO_AES`, `CCM`, `GCM`, `CMAC`, `CTR`, `SHA256`, `ECB` are on (for Wi-Fi). **No** `CONFIG_BLK_DEV_DM`, `CONFIG_DM_CRYPT`, `CONFIG_CRYPTO_XTS`, `CONFIG_CRYPTO_AES_NI_INTEL`, `CONFIG_CRYPTO_USER_API_*` or `CONFIG_CRYPTO_ARGON2` lines in either fragment or the defconfigs (grep over `dist/buildroot/board` and `configs`) | `dist/buildroot/board/iyakki/linux.fragment` lines 243-251, `board/iyakki-aarch64/linux.fragment` lines 251-258 | GAP (kernel rebuild needed) |
| Userland has no `cryptsetup`, `libgcrypt`, `lvm2`/`libdevmapper` | `dist/buildroot/configs/iyakki_x86_64_defconfig` package list | GAP |
| `CONFIG_PM=y`, `CONFIG_ACPI=y`, `CONFIG_ACPI_BUTTON=y`; no suspend or hibernate in IYAKKI; MAC-HARDWARE advises "do not count on suspend (lid close)" | `linux.fragment` lines 315-319; `docs/BATTERY.md` lines 73 and 97; `docs/MAC-HARDWARE.md` line 794 | EXISTS (no sleep) |
| Swap: zram is RAM; an optional swap file lives at `/var/lib/iyakki/swapfile` (config `swapfile`) | `crates/iyakki-zram/src/config.rs`, `stats.rs` | EXISTS |
| Secret-bearing files on the data volume: `users.json`, `audit.log`, `vault/*.afv`, `ssh/` (keys), `tls/` (self-signed cert and key), `/var/lib/iyakki/mac-secret`, `devmode/keys.json`, `web-profile` (browser cookies) | `accounts.rs`, `vault.rs`, `crates/iyakki-agent/src/ssh.rs`, `docs/DEVELOPER-MODE.md` section 8, `crates/iyakki-devctl/src/mac.rs`, `crates/iyakki-webview/src/proto.rs` | EXISTS |

---

## A. Lock screen

### A1. What it is for

| Use | Behaviour |
|---|---|
| Leave the desk | Manual lock: key chord (proposed `Super+L`, to be checked against `docs/KEYBOARD-AND-FN-KEYS.md`), a Lock entry in the power card (`Screen::PowerMenu`), a lock icon in the top bar, a Touch Bar button |
| Idle | New saver choice "When the screen is idle: ... then lock" with its own delay (Never, 1, 5, 10, 15, 30 min, **after** the saver starts or at the same time). Default **off** (matches the owner's "options off by default" habit for security prompts) |
| Lid close | Closing the lid cannot suspend (no suspend exists), so: lock and blank the screen on `SW_LID` (kernel input event; `ACPI_BUTTON=y`). Open lid: lock screen shown |
| Wake from saver or screen-off | If locking is on, the first input wakes to the lock screen, not to the desktop |
| Sign-out | Unchanged. Lock is lighter: nothing stops, nobody is signed out |

### A2. Design (PROPOSED)

New `Screen::Locked(LockState)` kept **beside** `session` (the session stays in memory; the lock only gates input and drawing).
The saver's `Stage` machine stays as is; a `locked: bool` on the shell decides whether leaving Saver/Off returns to `Launcher`/`App` or to `Locked`.

| Item | Decision |
|---|---|
| Shows | person's name, clock and date, a count of unseen notifications (a number only: no titles, no message text, no app names), battery chip, a password box, an Emergency/Power button. Nothing from the session: no window thumbnails, no toasts, no chat text. Drawn from a fresh buffer so previous pixels are never visible |
| Window contents | The app tree is not drawn at all while locked (it is not hidden under a layer: the draw path simply does not run), so a bug in an overlay cannot leak it |
| Unlock | Password, checked by `UserStore::authenticate` for **that session's user only** (a different name is refused: no way to switch user here). Same Argon2 cost as sign-in |
| Authenticator code | `docs/development/TOTP-2FA.md` section 5 currently says unlock asks the password only (the code was proven at sign-in). That is the default here; an optional per-account "Ask for a code when unlocking" (phase 2) uses the same half-session code path. Recovery codes work as at sign-in |
| Security key | NOT PLANNED here; needs HID/CTAP2 (TOTP-2FA.md section 11) |
| Failed attempts | Use TOTP-2FA.md numbers: 3 free tries, then 1, 2, 4 ... 60 s waits; the counter is persisted in `users.json` (closes the in-memory-only GAP). After 10 failures in a row the screen offers "Sign out" and says unlocking needs the password; it never wipes data. Audit events `lock`, `unlock.ok`, `unlock.fail`, `unlock.lockout` (names only, never a password) |
| Forgot the password while locked | Power button, hold: shutdown, then the existing rescue route (`docs/ADMIN-GUIDE.md` "Forgotten admin password"). The session's unsaved work in RAM is lost; this is stated on the screen |
| Multi-user | **One session at a time, as today** (`session: Option<Session>`). No fast user switching in v1. Another person uses **Sign out** (visible on the lock screen; asks "Unsaved work in apps may be lost" only if an app reports dirty state) |
| Accessibility | Large-text and high-contrast follow the person's settings (read from their own file, since the lock is the person's); a screen reader is not part of IYAKKI today (GAP) so the screen also has a spoken "Locked. Type your password" cue through the existing system sounds only if enabled; the on-screen keyboard is offered on touch screens |
| External displays | Every display shows the lock look (clock and name on the primary, black with a small logo on others) via the KMS path. The saver's single-framebuffer limit applies today (`docs/SCREEN-SAVER.md` "Several displays"): until that follow-up exists, locking on a multi-display machine is **refused with a message** rather than leaving a display unlocked |

### A3. What keeps running while locked

| Thing | Rule |
|---|---|
| Assistant tasks, flows, jobs, downloads, builds | Keep running (`docs/JOBS.md`, `docs/PARALLEL-TASKS.md`). Locking is not signing out, so `stop_for_logout` is not called |
| Sensitive popups (approval cards, step-up code asks, sign-in of AI tools, store purchase, power card) | **Wait.** They queue; the lock screen shows only "1 item needs you" (count). They appear after unlock. Exception: critical battery shutdown and the drive-loss card's Restart act without unlock (they never reveal content) |
| Approval cards for developer mode (`shell`, `reboot`, `poweroff`) | Not approvable while locked; a paired computer gets "device is locked, ask the person" (new reason code). This also stops someone at the keyboard approving a remote shell for themselves |
| Alarms | Ring and can be snoozed or stopped from the lock screen without unlocking; the alarm label is hidden unless the person turns "show alarm names when locked" on |
| Media (radio, player, tales) | Keeps playing (`saver.rs` already holds the screen on; here the **lock** is independent of that hold). Allowed on the lock screen: play/pause, next, volume, mute. No track titles unless "show media title when locked" is on (default off) |
| Touch Bar | Shows only function and media/volume/brightness keys while locked; no app strips, no context text (`docs/TOUCH-BAR.md`, `crates/iyakki-shell/src/touchbar_host.rs`) |
| Clipboard | Cleared on lock (option, default on) so a password manager copy does not sit there |
| Drives | Stay mounted (nothing is forced; `docs/DISKS.md`) |
| Vault | Personal vault stays open in v1 (the sealed-vault variant is section C) |
| Assistant sensitive tools (vault read, shell) | The assistant's own sensitive-tool prompts wait for unlock like popups |

### A4. Remote and other doors

| Door | Locked or not | Reason |
|---|---|---|
| Dev mode (`iyakki dev`, port 7979) | Pairing and approvals refused while locked; already-approved scopes (install, logs) keep working, since the paired key is its own credential | developer mode has its own pairing, scopes and expiry (`docs/DEVELOPER-MODE.md` sections 6, 8); making it stop at every lock would break builds |
| SSH diagnostics (`iyakki.ssh=1`, key-only) | Not affected | separate key policy, out of scope (TOTP-2FA open decision 10) |
| Web remote screen (8443) | Has its own sign-in and session; **locking the physical screen does not lock the web one** and vice versa. A "Lock everywhere" button in the power card locks both; the web page shows its own lock card | different screen, different person possibly |
| Remote screen idle | The web screen already runs the saver at 4 fps (`saver.rs` `fps`); gets its own lock timer |
| Physical keyboard while locked | Function keys that are harmless (volume, brightness) work; no app shortcuts reach apps |

### A5. Power events

IYAKKI has no suspend or hibernate (`docs/BATTERY.md` line 97; no `/sys/power` use found in `crates`). So "lid close" means: screen off + lock, machine
keeps running (and keeps heating in a bag: say so in the docs). Real suspend is a separate, larger feature (kernel `CONFIG_SUSPEND`, driver resume on the Macs: `applespi`,
`brcmfmac`, `apple-bce` are known resume trouble). When it exists, lock **before** suspending and drop an encrypted vault's key (section C) before sleeping.

### A6. Tests and effort

| Test | Content |
|---|---|
| Unit (fake clock, as `saver.rs` tests) | lock on idle; lock on lid event; wake goes to Locked; correct password returns to the exact previous screen; wrong password never changes screen; other user's name refused |
| Leak tests | draw a marker app, lock, assert the framebuffer buffer holds none of the marker pixels or text; toasts and cards queued while locked are not drawn |
| Popups | an approval card created while locked is not delivered and appears after unlock; battery-critical still acts |
| Throttle | persists across a reopen of `UserStore` |
| Remote | web screen unaffected by a physical lock, "Lock everywhere" locks both |
| Device test | MacBook lid close/open, Touch Bar, external display, media keeps playing |

Effort: **M** (shell state, draw path, settings card, `users.json` throttle fields, lid event reader in `iyakki-core` or the shell, tests). Risks: a missed draw
path leaks content (hence the leak tests); a wrong lid event on some Macs (SPI keyboards, `applespi`) gives lock loops (needs a "ignore lid" switch);
being locked out of your own machine by a stuck keyboard (on-screen keyboard and the power-button route).

---

## B. Encrypted data partition (option)

### B1. Threat model

| Protects against | Does NOT protect against |
|---|---|
| A stolen or lost drive, USB SSD or laptop that is **powered off**: accounts, documents, vault files, chat history, k3s data, browser profile, audit log are unreadable without the passphrase | A running, unlocked machine (keys are in kernel memory); cold-boot or DMA attacks on RAM |
| A drive sent for repair or recycled | The unencrypted **boot partition** (FAT: kernel, initrd, `grub.cfg`, `root.sfs`, install id): an attacker can read or replace it. Secure Boot (`docs/SECURE-BOOT.md`) stops a replaced kernel only if enabled; it is optional and off by default. An "evil maid" can put a fake passphrase prompt on the boot partition. State this plainly |
| Offline attack on the password hashes in `users.json` (they sit inside the encrypted volume) | Firmware or hardware implants, a malicious administrator, weak passphrases |
| | Anything on the boot partition, so **it must never hold a secret** (B9) |

Today's honest position is `SECURITY.md`: physical access is out of the model. This option moves it in for powered-off theft.

### B2. Options for the crypto stack

| Choice | What | Licence (working rule) | Verdict |
|---|---|---|---|
| **Kernel dm-crypt, `aes-xts-plain64`, 512-bit key (AES-256-XTS)** | The kernel does the sector crypto (AES-NI on x86, ARMv8 CE on ARM) | Linux kernel GPL-2.0, already our kernel | **Use.** Never hand-roll sector crypto |
| LUKS2 format with `cryptsetup` userland | Standard, recoverable on any Linux, `luksAddKey`, `luksChangeKey`, header backup, argon2id | cryptsetup tool GPL-2.0+ (library LGPL-2.1+); needs libdevmapper (LGPL-2.1), json-c (MIT), libuuid, popt, and OpenSSL or libgcrypt (the project stance: see below) | Strong default for **formatting and key management in the full image/installer**: GPL is the same class as BusyBox, e2fsprogs (already shipped); offer the source; no linking into IYAKKI's own binaries (run it as a process) |
| In-house **LUKS2 reader + dm table setup by ioctl** | Parse the 4 KiB binary header and JSON area, derive the slot key with Argon2id, undo the anti-forensic splitter (AF, SHA-256), AES-XTS-decrypt the key material, verify the digest (PBKDF2-SHA256), then `DM_DEV_CREATE` + `DM_TABLE_LOAD` with a `crypt` target on `/dev/mapper/control` | Our code; uses `argon2` (MIT/Apache-2.0, already in `vault.rs`), `sha2` (already) | Feasible **read-only** (about 800-1200 lines Rust, with LUKS test vectors from a `cryptsetup`-made image); recommended **for stage1** (unlock only) so the 1 MB first stage does not need cryptsetup and its libraries. Writing/formatting stays with cryptsetup |
| Own on-disk format (not LUKS), reuse the vault's slot design | Same slot wrapping as `vault.rs` (Argon2id + XChaCha20-Poly1305) around a volume key given to dm-crypt | Ours | Possible and licence-clean; **not recommended**: nobody else's tool can recover it, and a new header format is where weak-crypto bugs live. Keep for the small vault container only if LUKS is rejected |
| PBKDF2 instead of Argon2id | Needs no memory | | **Reject** (GPU-cheap). If a machine cannot afford the memory, lower Argon2id memory (cap, floor 64 MiB), never switch to PBKDF2 |
| fscrypt, eCryptfs, per-file crypto | | | Reject: leaks sizes, names, metadata; does not cover k3s and swap file |

**Must NOT be hand-rolled:** AES or XTS (use kernel dm-crypt), the KDF (use the `argon2` crate or cryptsetup's), random numbers (kernel `getrandom`; `iyakki_sec::random_fill` already wraps it), any new header format with new integrity rules, comparing derived keys without constant time. **Honest in-house parts:** parsing a LUKS2 header, the AF merge and the device-mapper ioctl. Each has public test vectors, and a bug there fails closed (a wrong key does not decrypt) rather than silently weakening anything.

Project stance note: the vault already depends on RustCrypto crates (`argon2`, `chacha20poly1305`, `zeroize`; `crates/iyakki-agent/Cargo.toml` lines 33-35), so "own code first" is already applied with the line drawn at primitives. That is the line proposed here.

**Kernel changes (PROPOSED):** `CONFIG_MD=y`, `CONFIG_BLK_DEV_DM=y`, `CONFIG_DM_CRYPT=y`, `CONFIG_CRYPTO_XTS=y`, `CONFIG_CRYPTO_AES_NI_INTEL=y` (x86), `CONFIG_CRYPTO_AES_ARM64_CE_BLK=y` (aarch64 fragment), `CONFIG_CRYPTO_SHA256`
(exists), `CONFIG_CRYPTO_USER_API_SKCIPHER/HASH` only if cryptsetup is built against the kernel crypto API (not needed for the in-house path), and `CONFIG_DM_UEVENT` optional. Built **into** the kernel (not modules) because the
first stage has no module loader and runs before `/lib/modules` exists. Size cost about 150-300 KB (expected). `docs/DISKS.md`'s note stands: this is a kernel configuration change made when an image is built.

### B3. Cipher, key derivation, slots

| Item | Choice |
|---|---|
| Cipher | `aes-xts-plain64`, 512-bit key (two AES-256 keys); sector size 4096 where the drive reports 4096, else 512; LUKS2 `--sector-size 4096` on NVMe/SSD helps speed (expected) |
| KDF | Argon2id; cryptsetup benchmark picks memory up to 1 GiB, time about 2 s on the machine that formats. Stage1 must have that much free RAM: Macs and PCs have 4-16 GB (the unlock runs before anything else uses RAM). Floor 256 MiB on machines with under 2 GB RAM (cap in the formatter, written into the slot) |
| Slots (max 8 in LUKS2) | 0: passphrase; 1: **recovery key** (random 256-bit, shown as 8 groups of 4 base32 characters, once); 2+: spare. No TPM, no hardware token in v1 (a security key later) |
| Same password as first account? | **Offer it as the default** ("Use my account password to unlock the drive"): the installer asks one password. Cost: changing the account password later must also change the slot (the shell holds the old password at that moment, so it can run `luksChangeKey`; an admin reset of someone else's password does **not** touch the disk slot). Alternative "a separate disk password" is a second field. The owner decides (decision 3) |
| Passphrase quality | Minimum length 8 and a warning below 12; no complexity theatre. Argon2id makes offline attack cost memory-bound but a short password is still short |

### B4. Where the passphrase is asked

The key is needed before the data volume is mounted. Two boot paths exist today:

| Path | Where the prompt goes | Notes |
|---|---|---|
| **Full boot** (`boot/initrd`, 173 MB; PID 1 `iyakki-core` runs `mount_data_volume`, `boot.rs` 749, after `start_boot_screen` per the test at boot.rs ~1135) | The boot screen. PROPOSED: `mount_data_volume` classifies the candidate (`classify` learns LUKS magic `LUKS\xba\xbe` at offset 0 and returns `Content::Locked`), shows a passphrase card on the splash (`iyakki_shell::splash`, shell-side drawing with the existing text input widgets and the on-screen keyboard), unlocks, then continues exactly as today. Input devices exist by then (drivers load before; USB, and on the SPI MacBooks `applespi` as a module) | GAP: today a boot screen has no input; the shell loop is not running yet. Needs a small input loop in `iyakki-core` or an early minimal shell mode |
| **Lean boot** (`initrd-lean`, stage1) | stage1 prints on `/dev/tty0` (it already does, `main.rs` ~78) and reads the keyboard from the **console tty**. Needs a keyboard that works in stage1: USB keyboards need `CONFIG_HID`/USB HID built in (`CONFIG_HID=y` and the xHCI options are, `linux.fragment` 58-65; `CONFIG_USB_HID` to be verified); the **SPI keyboards of the 2015-2017 MacBooks need `applespi`, which is a module (`linux-mac.fragment` line 112) and stage1 loads no modules**. So on those Macs stage1 cannot ask: it falls back to the full boot (the safe route already in place: write the failed marker, restart into the normal entry; or better, an explicit "locked: use the full start" marker) | GAP; this is the strongest reason to build `applespi` and the SPI chain into the kernel if lean boot plus encryption is wanted there |
| T2 MacBooks | Internal keyboard and Touch Bar sit behind the T2 (`apple-bce`, a module, `docs/MAC-HARDWARE.md` line 31). Same fallback as above; **a USB keyboard works** | stage1 cannot use the T2 keyboard |
| Timeout | None by default (a locked drive waits). Option `iyakki.unlock=timeout:N` for unattended hardware: after N seconds start **without** the data volume, from RAM (`iyakki.data=ram` behaviour), with a clear message; never loops forever without a screen. Three wrong tries: back to the prompt after a growing delay |
| Remote unlock | NOT in v1 (needs network before the volume; dropbear in initrd is the usual trick, and puts a key-handling service in the unencrypted initrd) |
| Boot screen on external keyboard timing | USB keyboards appear after the drivers; stage1 already waits up to 30 s for devices; the prompt starts when `/dev/tty0` input exists |

The on-screen keyboard (touch, no physical keyboard) is proposed for the full-boot card only.

### B5. Installer and first boot

| Step | Design |
|---|---|
| Option | A checkbox **Encrypt my data** on the installer's Mode/Plan page (`crates/iyakki-usb/src/wizard.rs`, `planner.rs`; plan field `encrypt: Option<EncryptRequest>`, included in the digest), off by default; also `--encrypt` on `iyakki-usb --plan` |
| Why the format must happen at first boot, not in the installer | `docs/INSTALLER.md`: the installer never formats DATA; it writes the pending marker (first MiB: "format me" plus settings) and the first start runs `format_pending`. Same here: the marker block gains `encrypt=luks2`; the **passphrase never goes into the marker or the plan file** (both are readable). It is asked on the **first start**, at the same boot-screen card that asks it later, together with the first account setup |
| First start | `format_pending` (storage) runs `cryptsetup luksFormat` (or the in-house writer, later) then opens it and `mke2fs` on `/dev/mapper/iyakki-data`, verify, shows the **recovery key once** with Copy and Print (the same pattern as TOTP recovery codes, `docs/development/TOTP-2FA.md` section 4) and a required "I saved it" tick |
| Install id | The per-install identity design (`docs/INSTALLER.md` "Per-install identity", unbuilt) derives the filesystem UUID from the id. With LUKS, the UUID is the **LUKS UUID** (set at `luksFormat --uuid`), and the id file `iyakki/install-id` is inside the encrypted volume, so the finder must match on the LUKS UUID (visible without a key). Conflict: see section D |
| Finder | `find_data` learns "a LUKS2 container whose UUID matches" as `Locked` data. `DISKS`, the partition manager and the safety rules treat it as SYSTEM (already: "held by ... an encrypted volume" refuses) |

### B6. Day-to-day operations

| Operation | How |
|---|---|
| Change password | Settings -> Storage -> Disk encryption -> Change: old passphrase, new twice; `luksChangeKey` (adds the new slot before removing the old, so power loss leaves one valid slot) |
| Add/remove a slot | Add: needs an existing passphrase; Remove: refuses to remove the **last** slot; refuses to remove the recovery slot unless a fresh recovery key is made |
| Header backup | `luksHeaderBackup` to `/var/lib/iyakki/.luks-header` **is useless there** (inside the volume); store the backup on the **boot partition**: the LUKS header holds only wrapped keys and is safe against a weak passphrase only as much as the passphrase is strong, so storing it on the unencrypted boot partition **reduces** protection against a thief who also has the passphrase-guess budget. Recommendation: do NOT keep a header copy on the boot partition; offer "Save a header backup" to a **removable drive the person chooses**, and make the recovery key the main safety net. Power loss during a header update is covered by LUKS2's redundant metadata copies (two areas, written alternately) |
| Encrypt later (convert an existing unencrypted volume) | **Recommend: not in place.** `cryptsetup reencrypt --encrypt` can do it (it needs 32 MiB free at the start of the partition, takes hours, is resumable) but a crash or a failing USB (see storage loss) is a data-loss scenario. Offer instead: Settings -> Storage -> "Move to an encrypted drive" = back up (copy tree) to a second drive, reformat encrypted, restore; or **re-install with the box ticked and import** (`docs/development/IMPORT-FROM-DRIVE.md`). In-place encryption: later, only with a verified backup |
| Disable encryption | Same, by migration; never "in place" |
| Forgotten passphrase | The recovery key. Without both, the data is gone: say so at setup in one sentence. No escrow, no backdoor |

### B7. Performance and wear

| Item | Expected |
|---|---|
| AES-NI x86 (the Macs 2015-2018 have it; AMD Zen has it) | aes-xts about 2-4 GB/s per core: faster than a USB 3 SSD (up to ~1 GB/s) so the overhead is small on a USB or SATA drive, a few percent on NVMe |
| ARM with Crypto Extensions | similar order; the aarch64 fragment needs the `ARM64_CE` options |
| CPU without AES-NI (old Atom/Core 2) | generic AES about 100-200 MB/s per core (expected): noticeable on NVMe, fine on USB 2/3. `cryptsetup benchmark` result is shown in Settings. Adiantum is an alternative for such CPUs but is **not proposed** (needs more kernel options and is a different on-disk format) |
| Boot time | Unlock adds the Argon2id time (about 1-2 s) plus the typing. The lean boot's budget (`docs/BOOT-SPEED.md` goal: sign-in in about 8 s) no longer holds on encrypted machines: state it |
| TRIM/discard | Passing discard through dm-crypt reveals **which blocks are free** and the filesystem type pattern. Default **off** (`allow_discards` not set); the weekly `fstrim` becomes an opt-in "Allow TRIM (faster SSD, reveals free space map)". USB sticks and cheap SSDs without TRIM wear faster without it: explain in the card |
| Queue flags | `no_read_workqueue` and `no_write_workqueue` (kernel 5.9+) are the usual speed options on SSDs; kernel is 6.12 (`BR2_PACKAGE_HOST_LINUX_HEADERS_CUSTOM_6_12`) so they exist; measure before turning on |

### B8. Interplay with what is on the volume

| Topic | Rule |
|---|---|
| k3s data and containers | Live on the volume (`docs/K3S-DEV.md`); encrypted with it, no change. Heavy small-IO load is where CPU without AES-NI hurts |
| Storage-loss handling (`docs/STORAGE-LOSS.md`) | The watcher mounts a **new device node by the same UUID** and the label. With dm-crypt the mounted device is `/dev/mapper/iyakki-data`; a USB re-attach brings a **new underlying node while the old mapping is dead**. PROPOSED: the watcher keeps the **volume key in the kernel keyring (session key, `keyctl`) or in locked memory of PID 1** so a reattach can recreate the mapping without asking again; removing the mapping and the key from memory on lock/shutdown. Step 2 of "Reconnecting" becomes: open the LUKS container on the new node with the held key, then mount. If the key is not held (shell restarted? it is PID 1's memory, so it survives) a card asks for the passphrase. UUID rule: compare the **LUKS UUID** (visible without a key) for the reattach decision |
| Self-preparing boot tree | `docs/development/SELF-PREPARING-BOOT.md`: the tree `iyakki/root/<tree>/tree/` and `.lean-complete` live on the data volume. On an encrypted volume **stage1 must unlock before reading `iyakki/root/current`** (B4). If it cannot (no keyboard in stage1) it must use `boot/root.sfs` from the boot volume (the existing seed route, which needs no data volume) or fall to the full boot. The "fast start ready" marker therefore also moves **inside** the volume or is made per-volume; a marker on the FAT boot partition must say only "tree exists", never names or contents |
| Update and patch flows | `boot-next/`, `boot-prev/` (staged copies of kernel/initrd) are on the data volume and encrypted; GRUB's "previous version" entry reads `boot-prev/<version>/bzImage` with GRUB's `ext2` module and **cannot read an encrypted volume** (GRUB has `luks2` support only in newer builds and with limits; `grub-efi-builtin.cfg` has no cryptodisk). Therefore: with encryption on, `boot-prev/` must stay on the **boot partition** (it has room only on the 512 MB PC image, not the 256 MB one) or the "previous version" entry is dropped for encrypted installs. This is a conflict with `docs/DEVELOPER-MODE.md` section 7 step 3: decision 6 |
| What must never be on the boot partition | passphrases, recovery keys, header backups (above), device keys, `net-mac.conf`/`mac-secret`, TLS keys, logs with usernames. Existing boot-partition files today (kernel, initrd, `grub.cfg`, `iyakki-release`, `iyakki-usb-compat.cfg`, `stage1.log`, `iyakki-lean-failed`) are non-secret; `stage1.log` must not print the passphrase or a derived key (test) |
| Crash dumps, logs | `/var/lib/iyakki/log`, `boot-lean.log`, `audit.log` are on the volume: encrypted. The kernel log copy while locked/unmounted stays in RAM only |
| Swap and hibernation | zram is RAM (no issue). A swap file (`/var/lib/iyakki/swapfile`) is on the encrypted volume: fine. A swap **partition** or swap on the boot partition: never created. No hibernation exists, which is the one case that would write RAM (and keys) to disk |
| Dev mode and the shell | `devmode/keys.json` and TLS keys are inside the volume (encrypted at rest) |
| Live and RAM-only sessions | Unchanged (`iyakki.data=ram`); nothing to encrypt |

### B9. Hardware: T2 Macs and the external stick

| Case | What it means |
|---|---|
| **IYAKKI on a USB SSD** (the owner's Mac mini and stick loop) | The drive has no hardware encryption: dm-crypt is the only protection. This is the case that benefits most |
| **Internal SSD of a T2 Mac** (Mac mini 2018, MacBookPro15,x/16,x) | The SSD controller is inside the T2 chip, which encrypts everything with a hardware key tied to the T2 (macOS FileVault adds a user-password layer on top). From Linux (via the t2linux NVMe/BCE path) the T2 decrypts transparently once the Mac boots, **so the drive is protected against someone taking the chips, but not against someone booting another OS on that Mac** (the T2's Startup Security Utility may block external boot; that is a firmware setting, not IYAKKI). General knowledge of the T2, not verified in this repo; the repo only notes that reads behind the T2 can stall (`crates/iyakki-stage1/src/main.rs` ~line 188/221). dm-crypt **adds** a layer keyed to the user's passphrase: double encryption, a few percent more CPU, no weakening. On a T2 Mac with external boot disabled by the owner, the extra layer is optional; on a Mac set to allow external boot (the IYAKKI use case) it is worthwhile |
| Pre-T2 Macs (MacBookPro14,1) and PCs | No hardware encryption: dm-crypt is the protection. OPAL/self-encrypting drives are not used (vendor-dependent, hard to audit) |
| AMD PCs | AES-NI present; same kernel options |
| Secure Boot | Not required, not provided by encryption; the boot-partition caveat in B1 stays. Measured boot / TPM sealing is a later project (the Macs with T2 have no usable TPM for Linux here) |

### B10. Tests

| Test | Content |
|---|---|
| Format/open | QEMU with a small virtual disk (as `dist/buildroot/qemu-persist-test.sh`, `qemu-install-test.sh`): first start with `encrypt=luks2`, passphrase from a test file, recovery key printed, ext4 mounted, reboot, unlock, data still there |
| Wrong password | Rejected, delay grows, no mount, boot continues to the prompt or RAM mode by timeout |
| Header | Known-answer test: a LUKS2 image made by `cryptsetup` on a build host with a fixed salt is opened by the in-house reader (all slots: passphrase, recovery); a corrupted first metadata area falls back to the second; truncated header refuses |
| Argon2 | KAT vectors for `argon2` crate against RFC 9106 test vectors |
| Power loss | Kill QEMU during `luksChangeKey` and during first-start format (the pending marker must remain until success); reopen with old or new passphrase, never neither |
| Lost header | Without a header backup the data is unrecoverable: the test asserts the UI says so |
| Reattach | Fake USB disconnect (the existing storage-loss harness) with an encrypted volume: no passphrase prompt when the key is held |
| Stage1 | `crates/iyakki-stage1` unit tests for the new decisions in `lib.rs` (LUKS magic vs ext4 label; refuse to guess); QEMU lean boot with USB keyboard events typed through the QEMU monitor (`sendkey`) |
| No secrets on boot | Grep `boot/stage1.log` and the FAT image for the passphrase and the key bytes |

**Effort B:** **L**. Kernel (S), cryptsetup in the image and the licence/source offer (S), in-house LUKS2 reader + dm ioctl in `iyakki-storage` (M), stage1 prompt and fallback (M), full-boot prompt on the splash (M), installer checkbox and first-start formatting (M), Settings page (M), storage-loss and update-path changes (M), tests (M).

---

## C. Separate encrypted vault partition (option)

### C1. Purpose and what moves

A small encrypted store for **secrets only**, separate from the data volume, so the data volume can stay unencrypted (fast, simple, k3s-friendly) while secrets stay sealed. Size 64-512 MiB (default 128 MiB; the vault files, keys and certificates are KBs to a few MB).

| Move into it | From | Notes |
|---|---|---|
| `vault/system.afv`, `vault/users/*.afv` | `/var/lib/iyakki/vault/` | **EXISTS** (vault.rs) |
| SSH keys (`ssh/`), git keys | `crates/iyakki-agent/src/ssh.rs`, `gitkeys.rs` | including the dropbear/device host key for diagnostics (name to be confirmed) |
| Store publisher keys (`publisher-keys/`) | store module | signing keys |
| `mac-secret` (the per-install secret that derives the Wi-Fi/Ethernet MAC) | `crates/iyakki-devctl/src/mac.rs`, `/var/lib/iyakki/mac-secret` | hides a stable hardware identifier |
| `tls/` (web front end and dev mode certificate and key) | `docs/DEVELOPER-MODE.md` section 8 | |
| `devmode/keys.json`, TOTP seeds (`system.totp` lives in the personal vault already) | | |
| Wi-Fi passwords (wpa_supplicant config) | | |
| `users.json` (password hashes) | `accounts.rs` | **Stay in the data volume** in v1: the sign-in screen needs it **before** anybody can unlock the vault partition. See C3 |

How: a mount at `/var/lib/iyakki/vault-store` (**PROPOSED**) with bind mounts (or symlinks) from the old paths, so existing code keeps its paths (`vault/`, `ssh/`, `tls/`). Code that writes these paths when the partition is sealed must fail with a clear "sealed" error, not create the folder on the data volume (a guard: mount the target directory **read-only empty** over the old path while sealed, so a stray write fails instead of leaking).

### C2. Container shapes

| Shape | Pros | Cons |
|---|---|---|
| **A third partition `IYAKKI-VAULT`** (GPT, LUKS2) | Visibly separate, survives reformatting the data volume | The installer must create it (the planner has two roles today, BOOT and DATA; a third role is an extra table entry and extra free-space need); impossible to add later on a disk with no free space; no resize tool (`docs/INSTALLER.md` rule 7) |
| **A LUKS2 container file `iyakki/vault.img` on the data volume**, loop-mounted | Can be added to existing installs, any size, no partition table change; `loop_attach` already exists in stage1 | File size and name visible on the unencrypted volume; deleted with the volume |
| Recommendation | **File container first** (works today, simplest, solves "add later"); partition as the installer option once the first is proven. The code treats both as "a block device with LUKS2" so the unlock path is the same |

### C3. When it is unlocked

| Option | Behaviour | Verdict |
|---|---|---|
| At boot (with B) | One prompt | Meaningless unless the data volume is encrypted too |
| **At sign-in with the account password** (PROPOSED) | `authenticate` succeeds -> the same password opens the vault container (key slot per **account**, as `.afv` already has slots per user) -> bind mounts appear -> services that need secrets start. Sign-out and **lock** (option "seal the vault when locked", default off) remove the mounts and drop the key | A powered-off drive reveals no secrets even with an unencrypted data volume; a locked screen keeps secrets sealed when the option is on. Needs a **privileged helper** in `iyakki-core` (the shell asks PID 1 to open/close; the shell does not get root; sanctioned by the BOUNDARY rules, to be checked in `BOUNDARY.md`) |
| Boot-time services that need secrets (dev mode TLS, SSH diagnostics, the web front end's certificate, `mac-secret` for the network address) | Cannot wait for a sign-in | **Conflict.** Split: keep a **system slot** (the machine key) for the system secrets so the machine boots unattended, and the **user slots** for personal secrets; the system slot's key is then on the unencrypted drive, so system secrets are not really protected from a thief. Honest answer: the vault partition protects **personal** secrets in the first release; system secrets (TLS key, SSH host key, `mac-secret`) get protection only with option B (boot-time passphrase). Decision 8 |

### C4. What the existing vault already gives, and what the partition adds

| Question | Answer |
|---|---|
| What the `.afv` already gives | Contents of each vault: XChaCha20-Poly1305 under a random key wrapped per slot with Argon2id (`vault.rs`). A thief with the drive cannot read the secrets without that password, and each user's vault is separate. This is most of the benefit **for the secrets inside the vault** |
| What it does not cover | File **names and sizes** (`vault/users/<name>.afv` reveals account names and how many secrets roughly); everything that is **not** stored in the vault: `ssh/` private keys, `tls/`, `mac-secret`, publisher keys, `devmode/keys.json`, wpa_supplicant config; the SSH host key; `users.json` hashes (Argon2id, attackable offline but memory-hard); the audit log and browser profile; swap/log leaks of secrets |
| What the vault partition adds | Those files and metadata under one disk-level key; one tidy place to put new secrets; no per-secret migration into `.afv` |
| Honest conclusion | A **cheaper first step** is to move `ssh/`, `publisher-keys/`, `tls/`, `mac-secret`, `devmode/keys.json` **into the existing vault format** (`system.afv`), which needs no kernel change and no partition. What a vault *partition* adds is protection for metadata and for whatever the vault format cannot hold (files used by other programs like dropbear that read key files from the filesystem). Since dropbear reads key files by path, those need a mount anyway. So the partition is justified mainly by **programs that need files**, not by crypto strength |

### C5. Combination matrix

| Data volume | Vault partition | Result | Use |
|---|---|---|---|
| Plain | Plain | Today | default |
| Plain | **Encrypted (C)** | Documents readable by a thief, secrets sealed until sign-in | light-weight protection, fast boot, no prompt at boot |
| **Encrypted (B)** | Plain (secrets in `.afv` only) | Everything sealed at rest; one prompt at boot | normal "encrypt my data" user |
| Encrypted (B) | Encrypted (C) | Secrets sealed even after the machine is booted and the volume unlocked: they open at sign-in and can be sealed on lock | strongest; two layers; one prompt at boot, one password at sign-in (the same password: nothing extra to type if the slot uses the account password) |

### C6. Installer, Settings and failures

| Item | Design |
|---|---|
| Installer | Checkbox **Keep my secrets in a separate encrypted vault** (off by default); with "Encrypt my data" ticked it is greyed out as "already covered" but selectable ("two layers") |
| Settings | Storage -> Secret vault: state (sealed/open), size, "Seal now", "Change size" (grow only: container file), slots (account passwords), recovery key (made once, like B) |
| Lost key | Lost password and recovery key = lost secrets (SSH keys, publisher keys: the publisher must re-key; Wi-Fi passwords retyped; TOTP must be re-enrolled via the recovery path of TOTP-2FA.md) |
| Admin resets another's password | Their slot is removed (existing rule: personal vault reset); system slot unaffected |
| Container damaged | LUKS2 metadata copies; a read-only "vault.img.bad" copy kept; the machine still boots (secrets sealed -> services needing them show a clear error) |
| Account removal | The slot is deleted |
| Effort | **M** for the file container and sign-in unlock; **+S** for the partition role in the installer |

---

## D. Phased plan, interplay and risks

| Phase | Work | Effort | Depends on |
|---|---|---|---|
| 1 | **Lock screen** (A): manual lock, idle lock, lid lock, throttle persisted, leak tests | M | none; TOTP phases 1-4 only for the optional code at unlock |
| 2 | Kernel options (`DM_CRYPT`, `XTS`, AES-NI/CE) in a QEMU-tested build; cryptsetup in the **full image**; Settings -> Storage "Secret vault" file container unlocked at sign-in (C) | M | phase 1 (seal on lock); privileged helper in `iyakki-core` |
| 3 | Move `ssh/`, `tls/`, `publisher-keys/`, `mac-secret` behind the vault container, with sealed-write guard | M | phase 2 |
| 4 | **Encrypted data volume, full boot only**: `Content::Locked`, boot-screen passphrase card, first-start format, recovery key, change password, installer checkbox | L | phase 2; installer per-install identity decision |
| 5 | In-house LUKS2 reader + dm ioctl; stage1 unlock; keyboards in stage1 (SPI/T2 fallback); storage-loss reattach with the held key | L | phase 4; self-preparing boot decisions |
| 6 | Partition-based vault role in the installer; encrypt-later by migration; optional TRIM, queue flags | M | phase 4 |

Code places that change: `crates/iyakki-shell/src/shell.rs`, `saver.rs`, `shell_saver.rs`, `apps/settings*.rs` (lock, cards), `accounts.rs` (persisted throttle); `crates/iyakki-core/src/boot.rs` and a new helper module (unlock, seal); `crates/iyakki-storage/src/lib.rs` (`classify`, `find_data`, `format_pending`, `mount_data`), `loss.rs` (reattach); `crates/iyakki-stage1` (`lib.rs`, `main.rs`); `crates/iyakki-usb` (`planner.rs`, `wizard.rs`, `apply.rs`: marker `encrypt=`); `crates/iyakki-agent/src/vault.rs` (mount-aware paths), `ssh.rs`, `store/keys.rs`; `dist/buildroot` kernel fragments, defconfigs and a `cryptsetup` package; docs (`SECURITY.md`, `KNOWN-ISSUES.md`, `FEATURES.md` rows, to be added by the owner's rule when built).

### Conflicts and order with other design documents

| Document | Conflict or interplay | Proposed order |
|---|---|---|
| `docs/development/TOTP-2FA.md` | Its phase 5 is this lock screen; TOTP step-up windows and the lock should share one "recently proven" state | Lock screen first, then TOTP phase 5 becomes "done" |
| `docs/INSTALLER.md` "Per-install identity" | Finder, UUID and `install-id` file design assumes a readable ext4; encrypted volumes need the LUKS UUID as the identity | Decide the identity first, encode LUKS UUID in it |
| `docs/development/SELF-PREPARING-BOOT.md` | Tree and marker on the data volume: stage1 unlocks first; fast-start prep job needs the volume open (it runs inside the system, so fine) | Encryption full-boot first, lean second |
| `docs/BOOT-SPEED.md` | Unlock time and prompt break the 8 s goal; SPI/T2 keyboards cannot prompt in stage1 | State "encrypted installs use the normal start until phase 5" |
| `docs/DEVELOPER-MODE.md` section 7 and `docs/development/REMOTE-PATCH.md` | `boot-prev/` on the data volume vs GRUB reading it | Decide in decision 6 |
| `docs/STORAGE-LOSS.md` | Reattach needs the key kept | In phase 5 |
| `docs/DISKS.md` | Add "IYAKKI encrypted volume" as SYSTEM | With phase 4 |
| `docs/SECURE-BOOT.md` | Encryption without Secure Boot has the evil-maid hole; the two should be offered together | Suggest in the UI |
| `docs/BATTERY.md` | No suspend: lid = lock + screen off | Phase 1 |

### Risks

| Risk | Mitigation |
|---|---|
| Lockout from own data (forgotten passphrase) | recovery key, plain wording, no silent enabling |
| Unreadable prompt in stage1 on SPI/T2 keyboards | fallback to full start; build those drivers in if wanted |
| In-house LUKS2 reader bug | read-only, KATs from real `cryptsetup` images, fails closed, cryptsetup remains for writing |
| GPL tooling in the image | the same class as BusyBox/e2fsprogs; source offer and notices in `LICENSING.md`, `NOTICE`; run as a separate process |
| Kernel size and cost | measure: expected 150-300 KB |
| Boot partition still unprotected | state it in UI; recommend Secure Boot |
| Data loss on in-place conversion | not offered; migration only |
| A leak of content through the lock screen | draw nothing from the session; leak tests |

### QEMU test plan (summary)

Existing harnesses to extend: `dist/buildroot/qemu-persist-test.sh`, `qemu-install-test.sh`, `qemu-lean-test.py`, `qemu-ui-test.py`, `qemu-boot-speed.py`. New cases: encrypted first start (keys typed with `sendkey`), wrong password, reboot unlock, lean boot with USB keyboard, SPI keyboard fallback, fake disconnect with held key, power loss during format and during `luksChangeKey`, lock screen draw leak check on the framebuffer, vault container sealed/unsealed with sign-in and lock.

## OPEN DECISIONS (owner)

1. Approve the design and the phase order (lock screen first, vault container second, encrypted volume third)?
2. Lock screen: default off or on after the saver (proposed: off)? Is one session at a time acceptable (no fast user switching)?
3. Disk password = first account password by default (convenience, proposed), or separate?
4. Crypto stack: cryptsetup (GPL-2.0+, in the full image, source offered) for formatting plus an in-house read-only LUKS2 reader in stage1 (proposed), or cryptsetup everywhere, or an own on-disk format?
5. Kernel: accept built-in `DM_CRYPT`, `XTS`, AES-NI/ARM CE (about 150-300 KB, expected)?
6. Encrypted install and the "previous version" boot entry: keep `boot-prev/` on the boot partition (needs the 512 MB boot volume), or drop that entry for encrypted installs?
7. Encrypt later: migration only (proposed) or also in-place conversion with a verified backup?
8. Vault: file container first (proposed) or partition first? And are system secrets (TLS key, SSH host key, `mac-secret`) accepted as unprotected until the data volume is encrypted?
9. Header backup: removable drive only (proposed), never on the boot partition?
10. TRIM: off by default, opt-in (proposed)?
11. Lean-boot encrypted installs: accept falling back to the full start where stage1 has no keyboard, or build the SPI/T2 keyboard drivers into the kernel?
12. Offer an unlock timeout that starts without the data volume (RAM mode) for unattended machines?
13. Lid close without suspend: lock plus screen off (proposed); is real suspend wanted as its own project?
