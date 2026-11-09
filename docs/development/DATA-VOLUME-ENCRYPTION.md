# Data volume encryption (F-STORE-05, queue item Q5)

**Status 2026-11-08: design plus slice 1 (pure code, tested on files and memory). Nothing runs on a real volume.** This page builds on
`docs/development/LOCK-AND-ENCRYPTION.md` section B (2026-10-24), which designed "encrypt the whole data partition with LUKS2 and
`cryptsetup`". Since then the owner fixed rules that change the answer: in-house code first, permissive licences only, GPL tools never
shipped (so no `cryptsetup` on the stick), and the images now do unattended boots (kiosk, cloud, k3s). Where this page differs from
section B, this page wins. Numbers marked "measured" were measured on the build PC (AMD Ryzen 9 7940HS, with AES instructions); "expected"
means an estimate.

## 1. Threat model

| Protected | Not protected |
|---|---|
| A stolen or lost computer, USB SSD or stick that is **powered off or signed out**: the person's files, chat history, browser profile, keys | A **running, unlocked** system: keys are in kernel memory; anyone signed in, any malware running as the person, cold-boot and DMA attacks on RAM |
| A drive sent for repair or thrown away | The **boot partition** (FAT: kernel, initrd, loader, seed root image, install id). It is readable and writable by a thief. It must never hold a secret |
| Offline guessing of the person's password from the drive (the cost is Argon2id, memory-hard) | The **Secure Boot chain**: a thief can replace the unsigned parts of the boot partition (an "evil maid" prompt that records the password) unless Secure Boot is on and the key is enrolled (`docs/SECURE-BOOT.md`). Encryption does not give Secure Boot and Secure Boot does not give encryption |
| | A weak password, a person who writes the recovery key on the lid, a malicious administrator, hardware implants |
| | Anything on the volume that stays **outside** the chosen scope (section 2), for example the root images and update staging under option B |
| | Flash remnants: after a migration the old plaintext blocks may stay in the drive's flash until the drive overwrites or trims them (section 9) |

Today's honest position (`SECURITY.md`): vault files are encrypted, the volume is not. This work moves "powered-off theft" into the model.

## 2. What is encrypted: three options compared

What lives on the data volume (`iyakki/root/<tree>`, `boot-next/`, `boot-prev/`, `users.json`, `audit.log`, `vault/*.afv`, `ssh/`, `tls/`,
`mac-secret`, `devmode/keys.json`, `/home/<name>`, k3s data, logs, the optional swap file). stage1 reads the root tree from this volume
before any account exists (`crates/iyakki-stage1/src/main.rs`, `iyakki/root/current`, volume found by label `IYAKKI-DATA`).

| | A. Whole data partition (dm-crypt under ext4) | B. Per-person home directories, unlocked at sign-in | C. Vault files only (today) |
|---|---|---|---|
| Protects | Everything on the volume except the boot partition | The person's files, browser profile, keys, shell history. System files stay readable | `vault/*.afv` secrets only |
| Does **not** protect | Nothing on the volume (leaks: partition size, that it is encrypted) | Root images (public build anyway), `users.json` (Argon2 hashes only), `audit.log`, logs, k3s data, `mac-secret`, TLS and SSH host keys | Documents, chats, browser cookies, history, anything outside the vault |
| Cost to build | Highest: kernel dm-crypt, stage1 prompt and unlock, full-boot prompt, storage-loss re-open, update path, installer, migration | Medium: container per person (the format built in slice 1), loop + dm-crypt at sign-in, sign-out close, password-change re-wrap | Zero (exists) |
| Unattended boot (kiosk, cloud, k3s need the data volume at boot) | **Breaks**: nobody to type. Needs a TPM-sealed key (not available on the Macs, later) or a key stored beside the data, which gives no protection. Such tiers would simply not enable it | **Works**: the boot never needs the key; system services run from the unencrypted part. Tiers that sign nobody in have nothing to unlock and nothing person-private to protect | Works |
| Root images / update / rollback | stage1 must unlock **before** it can read `iyakki/root/current`. No keyboard in stage1 on SPI/T2 Macs. GRUB (or our loader) cannot read the volume for the "previous version" entry, so `boot-prev/` must move to the boot partition or the entry goes | **Unchanged**: root images and update staging stay plain on the data volume, stage1 and the loader are not touched | Unchanged |
| k3s data and logs | Protected | Not protected (stays system data). A person's own workloads that need private data mount from the home | Not protected |
| Recovery | Recovery key or admin slot; lost header = lost volume | Per person: recovery key or admin slot; one person's loss does not touch the others or the system | Vault recovery as today |
| Performance | All I/O through AES-XTS | Only the person's I/O; system I/O untouched | None |
| Weaknesses | Pre-boot prompt is part of the unprotected boot chain | A thief still reads system files; logs may leak names or file names | Leaves almost everything readable |

**Recommendation: build B first, keep A as a later option for attended single-owner machines, never make A the default.** B protects what a
thief actually wants (the person's files and keys) without touching the boot chain, so it ships without the kernel-driven risks the owner has
met (boot panic, KI-077, SPI keyboards). The unattended tiers keep working untouched. A is added only when stage1 can unlock reliably
(keyboard in stage1, or a USB key, or TPM) and is offered only on attended machines. A *split volume* (a small plain system partition and
a big encrypted private one) is the cleaner form of A, because the root images stay readable; it is a variant of A to evaluate then, not now.

Under B the container is a file `<data>/home-enc/<account>.ivol` in the format of section 5. The shell holds the sign-in password at sign-in
and opens the container with the same password (a separate Argon2id salt, so the sign-in hash on disk is not the key). The result is
mounted over `/home/<account>`; sign-out stops that person's work, unmounts and closes it.

## 3. Key hierarchy

```
password (typed at sign-in) --Argon2id(salt_i, m, t, p)--> KEK_i (256 bit) --AES key wrap--> volume key (512 bit, random, never changes)
recovery key (printed once, 256 bit)     --HMAC-SHA256-----> KEK_r          --AES key wrap--> the same volume key
admin password (optional)                --Argon2id---------> KEK_a          --AES key wrap--> the same volume key
volume key = two AES-256 keys for XTS; a sub-key of it (HMAC) signs the slot table
```

* Up to eight slots. A person's password changes by rewriting one slot; the data is not re-encrypted (built and tested).
* **Recovery key**: 256 random bits, shown once as 14 groups of 4 characters (52 for the key, 4 check characters), with a required "I saved it" step. Typos are caught by the check characters before anything is tried. Only one exists at a time; making a new one replaces the old.
* **Admin slot**: optional second password slot (kind Admin). It lets an administrator reset a forgotten password without data loss, and also lets that administrator read the home. For personal homes this is a privacy decision (question 3 below); the default proposed is **no admin slot**, the recovery key is the way back.
* The sign-in password change (the shell holds the old password then) re-wraps the person's slot. An **administrator resetting** someone's password cannot re-wrap (the old password is unknown): the home stays sealed until the recovery key or an admin slot is used. This must be said in the reset dialog.
* **TPM sealing** (later): a fourth slot kind whose KEK is released by the TPM when the boot measurements match; the Macs with T2 have no usable TPM for Linux. Needs a format version bump. Not in slice 1.
* The volume key is never stored anywhere except wrapped. Rotating the **volume key** itself (re-encrypting the data) is a migration (section 9), not a slot change; "key rotation" in slice 1 means slots.

## 4. Unlock moment

| Moment | Verdict |
|---|---|
| stage1 prompt (before the system exists) | Needed only for option A. stage1 prints to the console tty and has no on-screen keyboard; SPI/T2 keyboards need modules stage1 never loads. Also pulls Argon2 into the roughly 1 MB stage1. **Not chosen now** |
| Full-boot splash prompt | Possible for A on the full boot only. Same unattended problem. **Not chosen now** |
| **At sign-in** | **Chosen** for B. The sign-in screen already has a keyboard (and on-screen keyboard for touch), the password is already typed, and "no auto sign-in" (owner decision 2026-10-28) means the vault already unlocks that way. Unlock failure shows on the sign-in screen with the plain message and never blocks a different person |
| At unlock of a lock screen | Not needed: the lock keeps the session in memory; the home stays mounted |

If a person's password is right for sign-in but the container will not open (damaged header, missing file), sign-in still succeeds into a
minimal session with a banner "Your private folder could not be opened: ..." (never a loop, never silent use of an empty home that the
person might later believe is the real one: an empty temporary home is marked and refused for saving anything important).

## 5. Cipher and format: LUKS2-compatible writer or an own simple format

| | In-house LUKS2 header writer and reader | Own simple format (**recommended**) |
|---|---|---|
| Header | 16 KiB binary + JSON area, two copies, per-keyslot area with the **anti-forensic splitter** (4000 stripes of SHA-256 diffusion), digests via PBKDF2, CRC/SHA-256 checksums | One 4096-byte block, fixed fields, no JSON, no splitter; written twice with a generation number |
| Size of the work | A parser and a writer of roughly a thousand lines, a JSON reader for untrusted input | About 600 lines, built and tested already (slice 1) |
| Proof | Needs known answers made by `cryptsetup`, which is GPL and not on the build PC; no real LUKS volume to compare, so a mistake would fail silently as "wrong password" (the DISKS helper design reached the same conclusion for external volumes) | Known answers exist for every primitive (RFC 3394, RFC 4231, RFC 9106, IEEE 1619, OpenSSL); the header is pinned by a known-header digest test |
| Outside recovery | Any Linux PC with `cryptsetup` opens it. A real plus | Only IYAKKI opens it. Mitigated: recovery key, two header copies, and the payload is plain `aes-xts-plain64` so a converter can add a LUKS2 header later |
| Attack surface | More parser, more JSON, more optional features (hostile-input risk) | Small, strict (all reserved bytes must be zero, costs bounded, anything odd refused) |
| Licence | Fine (our code) | Fine (our code) |

**Recommended: the own format, `IYAKVOL1`.** Cipher `aes-xts-plain64` with a 512-bit key, sector 512 or 4096, with the payload laid out so the
kernel can open the *same bytes* through dm-crypt: the tweak is the 512-byte sector number (with 4096-byte sectors it advances by 8, as dm-crypt does
with `sector_size:4096`; a test pins this). The later dm-crypt table is
`0 <payload sectors> crypt aes-xts-plain64 <key hex> 0 <device> <payload offset / 512> 1 sector_size:4096` (no discards, see below).
A **LUKS2 export** (write a LUKS2 header and keyslot for the existing volume key, leave the payload where it is) is a possible later slice, checked
once on a PC that has `cryptsetup`. Slice 1 uses the `aes` crate's block function (same crate the SSH keys use), Argon2id from the `argon2` crate (as the vault does) and SHA-256 from `sha2`; XTS,
key wrap, HMAC, the header, the recovery key and the container are in this repository. No new dependency was added.

Format facts (see `crates/iyakki-volcrypt/src/header.rs`): magic `IYAKVOL1`, version 1, generation, volume uuid, sector size, payload offset and length, 8 slots of 128 bytes
(kind, Argon2 cost, owner tag, salt, 72-byte wrapped key), a header MAC (HMAC-SHA256 under a key derived from the volume key; catches removed or edited slots by someone without the key),
and a SHA-256 over the block (tells "damaged" from "wrong password"). Password costs are bounded when read (at most 1 GiB, 16 passes, 8 lanes), so a hostile header
cannot exhaust memory.

Performance and cost choices:

* **Measured** (build PC, release build, the `aes` crate in this process, one core, no pipelining): AES-256-XTS 751 MiB/s. The kernel's dm-crypt with AES-NI is several times faster; an SSD over USB 3 (about 1 GB/s at best) is the limit either way.
* **Measured**: Argon2id at the format's floor (64 MiB, 3 passes, 2 lanes) takes 0.09 s here, too cheap to be the real setting. The formatter must **calibrate** to about 1 s on the machine it runs on, with a memory floor of 256 MiB (64 MiB is only the minimum the format accepts) and never lower the cost to hide slowness. Slower machines (a Mac mini from a USB stick) will land lower in time and should be measured on the device.
* **Expected**: the Mac mini 2018, the Lenovo and the HP all have AES instructions; encryption overhead is small next to the T7 SSD. A CPU without them would be slow (about 100-200 MB/s) and Settings would say so.
* Discard/TRIM is **off** through dm-crypt (it reveals which blocks are free); an opt-in later.

## 6. Unattended tiers

Kiosk, cloud and k3s images never need option B's key at boot, so nothing changes for them. A signed-in person is the only unlock event. Cloud tenants'
disks are protected by the provider's volume encryption and the tenant vaults; the per-person homes still work there. If option A is ever offered it
is refused for images that boot unattended (the boot-mode flag already exists), with the message "This computer starts without anyone to type a password".

## 7. Update and rollback

Under B nothing changes: root images, `boot-next/`, `boot-prev/`, patches stay plain on the volume and stage1 reads them as today. A header change
(password, slot) is written to the unused of two header copies, read back and compared; a power cut leaves one whole header (tested at every cut point of the write).
The container is a file, so it is part of the volume's backups like any file (the header is inside the file; a copy of the file is a full backup).
Under A (later), `boot-prev/` and the "previous version" entry must move to the boot partition or be dropped for encrypted installs (section B8 of the older page).

## 8. Recovery

| Situation | Way back |
|---|---|
| Forgot the password | The recovery key (typed, checked for typos). Then set a new password (one slot rewritten) |
| Lost both | Gone. Said in one sentence when the key is shown. No escrow, no backdoor |
| One header copy damaged | Opens from the other; the next change repairs the copy (tested) |
| Both damaged | Message below; a header backup is a file copy of the first 8 KiB, kept only on a removable drive, never on the boot partition (it would let a thief with a password guess check offline against the boot partition) |
| Admin reset of someone's password | Home stays sealed unless an admin slot or the recovery key exists |
| Container file deleted | The person's home is gone; the system says so and offers a fresh empty one (backups: `docs/BACKUP.md`) |

## 9. Migrating an existing unencrypted install, in place and without losing data

Rule: **copy, verify, switch, keep the original until the person agrees**. Never convert in place.

1. Check: enough free space for the container (used size plus 10 percent) on the data volume or a chosen second drive; the person is signed in with their password; no app has files open in the home (they are stopped, as at sign-out).
2. Make the container (header, recovery key shown once with the "I saved it" step), open it.
3. Copy the home into it (files, modes, owners, symlinks, timestamps), counting files and bytes.
4. **Verify**: SHA-256 of every source file against the copy, read back through the decrypting path, plus the file/byte counts. Any mismatch aborts; nothing was switched.
5. Switch: write a marker `home.encrypted`, rename the plain home to `<name>.plain-before-encrypt` (still there, not deleted), mount the container at `/home/<name>`.
6. After the person has used it and confirms (or after 14 days with a reminder), delete the plain copy. **Honest limit:** deleting on flash storage does not erase the old blocks; the drive may keep readable remnants until it overwrites or trims them. For a stick or SSD that was already stolen once this protects nothing. Moving to a freshly formatted drive (copy, verify, then reformat the old one with a full overwrite or secure-erase) is the strong form, and the only form offered for option A.
7. Power cut at any step: before step 5 nothing changed (the half-built container is deleted at the next start); after it the marker makes the next sign-in mount the container and the plain copy is just an old folder.

## 10. Failure modes and the words shown

| Situation | Message (plain language) |
|---|---|
| Wrong password or recovery key | "The password or recovery key is wrong." After the usual throttle, never telling which slot or whether a key exists |
| Recovery key mistyped | "The recovery key is not written correctly: the check letters do not match; look for a mistyped character." (or too short, too long, a character not used) |
| Damaged header, one copy | Opens from the other copy; Settings shows "One copy of the header was damaged and has been repaired" |
| Damaged header, both | "The volume header is damaged (its checksum does not match)." plus the header-backup route |
| Not our format | "This is not an IYAKKI encrypted volume (the start of the drive does not match)." |
| Newer format | "This volume was made by a newer IYAKKI (format 2); update IYAKKI to open it." |
| Slot table changed by someone without the key | "The volume header was changed by someone without the key; do not trust this drive." Opening is refused |
| Hostile memory cost | "The drive asks for 2048 MiB of memory to check a password, more than this computer allows." |
| Last slot | "This is the last way to open the volume; it cannot be removed." |
| Eight slots used | "All key slots are in use; remove one first." |
| Drive error mid-write | "Reading or writing the drive failed: ..." and nothing is switched; the old header copy is whole |
| No random numbers | "The system could not supply random numbers." (the volume is not made) |
| Drive too small | "The drive is smaller than the volume says." |

## 11. What slice 1 built (verified) and what it did not

Crate `crates/iyakki-volcrypt` (a new workspace member; no new dependency, no unsafe, no device access):

| Module | Content | Known answers and hostile-input tests |
|---|---|---|
| `xts.rs` | AES-256-XTS (and AES-128 for the standard's own vectors), plain64 tweak | IEEE 1619 vector 1 and 2 (AES-128), AES-256 vectors made with OpenSSL, 4096-byte sector with the 512-unit tweak, bad lengths refused |
| `kw.rs` | AES key wrap, RFC 3394 | RFC 3394 vectors (128-bit and 256-bit key), every single flipped bit rejected, wrong key rejected, bad sizes refused |
| `mac.rs` | HMAC-SHA256, constant-time compare | RFC 4231 cases 1, 2, 6; Argon2id RFC 9106 vector against the `argon2` crate |
| `recovery.rs` | Recovery key text, parse, check characters | Round trip, typing habits (lower case, spaces, O/I/L), every single wrong character at every position rejected, too short/long/foreign |
| `header.rs` | Header, slots, unlock, add/change/remove, recovery slot, MAC | Right/wrong password, equal slow-hash work for right and wrong secrets (counted), every flipped byte of the header rejected, truncation at every length, edited-and-re-checksummed headers refused or caught by the MAC, hostile cost values, last-slot and full-table rules, pinned known-header digest |
| `container.rs` | Two header copies, sector read/write over any byte store (memory or file), safe header change | Write/read for 512 and 4096 sectors, no plaintext in the store, reopen, recovery key, 512-unit tweak check by decrypting the raw bytes, **a simulated power cut at every 61st byte of the header write** (plus the last bytes), one damaged copy survived, both damaged reported, real file store |

"Wrong-password timing": the tests prove the work done is the same (the slow hash runs for every password slot whichever one matches, counted by a hook); the final comparison
is a constant-time compare. The `aes` crate's software path is not constant-time on CPUs without AES instructions and no attempt is made to hide that here. A wall-clock timing test is not included because it would be flaky.

Not built, honestly: nothing touches a real disk, loop device, dm-crypt or stage1; the kernel has no `DM_CRYPT`/`XTS`/`BLK_DEV_DM` option in the fragments I searched
(`dist/buildroot/board/iyakki/*.fragment`; a kernel change belongs to whoever owns the kernel config, and loop-device support is to be confirmed); no sign-in hook; no migration; no UI; no chat tools; no
LUKS2 export. The recovery-key text is checked for typos, but nothing yet enforces that it was *saved*. The vault files inside the volume remain the only encrypted data today, which is why F-STORE-05 is `partial` at 15 percent, not more.

## 12. Next slices

1. **Kernel and the dm-crypt layer** (owner approval for the kernel options): `DM_CRYPT`, `XTS`, AES-NI, loop; a thin device-mapper ioctl wrapper in `iyakki-storage` (so the shared kernel-access gates only shrink in the shell), tested in QEMU with a file as disk. First real-volume test.
2. **Sign-in integration** (option B): open at sign-in with the password, mount over the home, close at sign-out, password-change re-wrap, minimal-session banner on failure. Fake-device tests first, then QEMU.
3. **Migration tool** (section 9): copy, SHA-256 verify, switch, keep the original; test with a fake home tree and a simulated power cut at each step.
4. **Settings page**: Encrypt my private folder, show the recovery key once, change recovery key, remove slot, status; admin-only slots page.
5. **Chat tools** (every feature reachable by chat, with approvals): status (free), enable and migrate, change password slot, make a new recovery key (hard ask, shown on the card and never to the model), remove slot.
6. Later: option A as a split volume for attended machines (stage1 unlock), TPM slot, LUKS2 export, header-backup export.

## 13. Questions for the owner

1. Approve option B (per-person homes unlocked at sign-in) as the first and default route, with option A only later for attended machines?
2. Own format `IYAKVOL1` (recommended) rather than an in-house LUKS2 writer. A LUKS2 export converter can come later; fine?
3. Admin slot on personal homes: none by default (recovery key only, strong privacy), or an optional admin slot the person can allow?
4. Same password as sign-in (recommended, one thing to remember), or a separate home password?
5. Kernel options (`DM_CRYPT`, `XTS`, AES-NI, loop, device mapper): approve for the next image build, given the boot-test-before-push rule?
6. Argon2id target: about 1 s on the machine that formats, memory floor 256 MiB. Acceptable on the Mac mini booted from the stick?
7. Is it acceptable that system data (logs, `users.json` hashes, k3s data, TLS/SSH host keys) stays readable under option B until option A exists?
