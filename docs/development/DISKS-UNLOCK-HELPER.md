# DISKS: unlocking LUKS and BitLocker volumes (F-DISKS-08, part 1)

## Decision: a person-installed helper program, not our own cipher code (for now)

| Route | Verdict |
|---|---|
| Own LUKS1 + LUKS2 unlock (header parse, PBKDF2 or Argon2id key slot derivation, anti-forensic merge, AES-XTS key-slot decrypt, then dm-crypt through the device-mapper ioctl) | Realistic: the agent already has `argon2`, `sha2` and `aes` (all permissive), the rest is a few hundred lines. **Not done now** because it cannot be proven right without a real LUKS volume (no `cryptsetup` and no root on the build PC), and a wrong key-slot implementation fails silently as "wrong password". Kept as the in-house follow-up. |
| Own BitLocker | Not realistic: the format is only reverse-engineered (FVE metadata, VMK, several protectors, Elephant diffuser on old volumes). |
| Sub-process helper: `cryptsetup` (LUKS), `dislocker` (BitLocker) | Chosen. Both are GPL: never shipped, never linked. They are CLI tools entries (`cryptsetup`, `dislocker`, source `Setting`, like OpenVPN): the person gives a download address and SHA-256, nothing is fetched or invented. A copy from the system's package manager is found too. |

## How the helper is run (`disks_helper.rs`)

* Fixed argument lists, no shell. Environment cleared (`PATH`, `LC_ALL` only).
* The password or recovery key goes to the helper's **standard input** and nowhere else: not an argument, not the environment, not a file. The type `Secret` has no `Clone`/`Display`, prints nothing and is overwritten when dropped; the masked box in the tile wipes its text as soon as Unlock is pressed and on every clone.
* LUKS: `cryptsetup open --type luks --key-file=- --batch-mode DEV iyakki-PART`; exit code 2 means a wrong key, and only that counts for the rate limit (waits of 2, 5, 15, 30, then 60 seconds per partition).
* BitLocker: `dislocker -V DEV -u|-p -r -- DIR` (FUSE, read-only), `losetup --find --show --read-only`, and a link `/dev/mapper/iyakki-PART`. Always read-only; the engine refuses "Allow writing" for BitLocker.
* The engine mounts the file system inside `/dev/mapper/iyakki-PART` **read-only**, `nosuid,nodev,noexec`. "Allow writing" is a separate button (unmount, mount read-write). Lock = unmount, then close. An unlocked drive cannot be ejected; the partition manager refuses it as in use. No format or write to the drive is ever done by this feature.
* Chat: `disks_status` (safe), `disks_unlock` and `disks_lock` (ask every time, hard ask, no standing allow). `disks_unlock` has one argument, the partition; it only leaves a request that the Disks tile turns into the masked box. The model never sees the password.

## Verified and not verified

Verified with made-up programs and drives (tests): header parsers on hostile and truncated input, argument lists, stdin-only secret, exit-code mapping, wrong-key limit, read-only first, locking, refusal rules, tile flow, chat tools.

**Needs the real tools and a real encrypted volume (owner test):**
* `cryptsetup` exit code 2 for a wrong key and `--key-file=-` reading the whole stdin without a trailing newline.
* `dislocker` reading the password from stdin when `-u`/`-p` have no value (if it insists on a terminal, the BitLocker route needs a pty or a key file in a 0600 tmpfs file), its wrong-key wording, and FUSE availability on the image.
* Kernel `dm-crypt` and the needed cipher modules in the IYAKKI kernel, and root for the device-mapper node.
* Unlocked volumes stay unlocked at sign-out (sign-out unmounts, it does not close the mapping): use Lock.
