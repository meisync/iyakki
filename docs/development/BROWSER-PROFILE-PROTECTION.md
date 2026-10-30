# Browser profile protection (cookies and web data)

**Status: first slice BUILT (2026-10-28), off by default.** Owner decision 2026-10-28: safeguard browser cookies and web data with the
person's credentials. Tags: **EXISTS** (read in code in this tree), **BUILT** (this slice), **PROPOSED** (not built), **UNVERIFIED** (no
real WebKit profile, no real Mac, no performance measurement yet). Backlog row: P17 (`docs/project/BACKLOG.md`).

## 1. What exists (verified against the tree)

| Fact | Where | Tag |
|---|---|---|
| The full browser (WPE WebKit through the helper) keeps cookies (`cookies.sqlite`), local storage and IndexedDB in plain files under `<data>/web-profile/<user>/data`; `config` and `cache` beside it; folders mode 0700 | `crates/iyakki-agent/src/webengine.rs` `prepare_profile`, `profile_dir`; `dist/webview/wpe-helper/helper.c` | EXISTS |
| Five callers ask for the profile folder before they start an engine | `webengine.rs` (`run`), `apps/browser.rs`, `apps/web_agent.rs`, `apps/media.rs`, `flows/browser.rs` | EXISTS |
| The reader browser keeps `browser-data-<user>.json` (bookmarks, history, zoom, downloads: addresses and titles only, no cookies) | `apps/browser_data.rs` | EXISTS (not covered here, see D9) |
| The vault: random 256-bit data key, XChaCha20-Poly1305, wrapped once per key slot with Argon2id of that slot's password; the person's slot is `user:<name>` and opens at sign-in with the typed password; changing the password re-wraps, never re-encrypts | `crates/iyakki-agent/src/vault.rs`, `services.rs` (`on_login`, `on_password_change`, `reset_user_vault`) | EXISTS |
| Sign-in holds the password only for the call to `Services::on_login`; a login is a password (Argon2id PHC in `users.json`). **There is no PIN and no lock screen today** | `crates/iyakki-shell/src/accounts.rs`, `docs/development/LOCK-AND-ENCRYPTION.md` section A (design only) | EXISTS / GAP |
| The data volume is not encrypted; whole-volume encryption (dm-crypt) is a separate design that needs kernel options the image does not have | `LOCK-AND-ENCRYPTION.md` section B, `SECURITY.md` | GAP |
| Sign-out stops flows, shares, services, the assistant's browser tab and shows the login screen; idle stages exist (Dim, Saver, Off) | `shell.rs` `sign_out`, `saver.rs` | EXISTS |
| The startup registry tracks services that start (clock, network, web engine, ...); this feature has no start step of its own | `crates/iyakki-agent/src/startup.rs` | EXISTS (no entry needed, D8) |

## 2. Threat model

Cookies are sign-in credentials: whoever has `cookies.sqlite` is the person on every site that still honours the session. Theft is an
account takeover without a password, usually past two-factor sign-in.

| Threat | Today | With protection on |
|---|---|---|
| Stolen or lost disk or stick, drive sent for repair, powered off | Readable by anybody with the drive (the data volume is plain) | Only the sealed file is on the drive: unreadable without the person's login password (Argon2id-wrapped key in the vault) |
| Another local account (a second person on the machine) | 0700 folders; the shell runs as one OS user, so the barrier is the shell's own rules, and a second person who gets a shell has the files | Each person has their own key; a second person's key opens nothing of theirs. The decrypted copy exists only while its owner is signed in |
| A process of another OS user | Blocked by 0700 | Same, and the files are in RAM only while open |
| Backups (copy of `/var/lib/iyakki`) | Contain the cookie jar in clear | Contain only the sealed file |
| A rogue app or tile run by the signed-in person | Can read the files | **Can still read the decrypted copy while the person is signed in.** Not solved here: it needs per-app isolation. Stated plainly |
| Malware running as the shell's user while signed in | Reads everything | Reads everything (the key is in memory) |
| Cold boot and DMA attacks on RAM | Out of scope | Out of scope |
| Someone watching the screen or the typed password | Out of scope | Out of scope |

What this does not defend: a thief who also has the password; a running signed-in session; an evil-maid boot partition (see
`LOCK-AND-ENCRYPTION.md` B1). It does not replace whole-volume encryption, it covers the one folder whose loss is an account takeover,
and it can ship now because it needs no kernel change.

## 3. Options compared

| Option | What | Costs | Verdict |
|---|---|---|---|
| **A1. fscrypt** | Kernel per-directory encryption on ext4 | Needs the ext4 `encrypt` feature on the volume (the formatter, `mke2fs` in `iyakki-storage`, does not set it) and `CONFIG_FS_ENCRYPTION`; keys go in the kernel keyring (`keyctl`, no userland tool in the image); per-directory policy is not removable; key per person from the vault is possible. The kernel fragments have no `FS_ENCRYPTION` line (grep over `dist/buildroot/board`) | Not available today: needs a kernel rebuild and a volume reformat, and old volumes cannot be converted. **Revisit when whole-volume encryption lands** (then it is redundant) |
| **A2. Encrypted loopback image** | One file holding an ext4 image opened with dm-crypt | Needs `DM_CRYPT`, loop and `cryptsetup` (absent, same kernel gap) | Not available today |
| **A3. In-house sealed bundle** | The profile folders as one authenticated-encrypted file (tar-like archive, XChaCha20-Poly1305 in chunks) | Whole-profile copy at sign-in and sign-out; needs RAM for the decrypted copy | Works on the current image and every volume, uses crates already in the tree. **Chosen** |
| **B. Decrypted view only while signed in and the browser runs** | The live folder on a RAM disk (`/run`), wiped on sign-out | RAM equal to the profile; combined with A3 it is how A3 is used | **Chosen together with A3** (A3 is the at-rest form, B is the in-use form) |
| **C. Per-cookie encryption in the helper** | WebKit would encrypt each cookie value | WebKit (WPE) stores cookies in SQLite through libsoup with no hook to transform values, and local storage and IndexedDB have no such hook either; patching WebKit is not realistic | **Not possible** without forking WebKit. Verified in the helper: it sets the data directory and nothing else for storage (`helper.c`); from memory about WebKit: no value-encryption hook exists |
| **D. Leave it plain** | Status quo | A stolen disk is every account | Remains the default until the person switches protection on (D1) |

**Recommendation: A3 + B**: a sealed file at rest, decrypted into RAM at sign-in, written back at sign-out. Later, when whole-volume
encryption exists, this is still useful for backups and for the per-person split (the volume key is one key for everybody; this one is per
person), and the bundle format is what cloud mode moves between machines (section 8).

Library: none new. The crates already in the vault (`chacha20poly1305` 0.10, `argon2` 0.5, `zeroize`, `sha2`) are used; the archive
and the chunked stream are about 500 lines of in-house code (`crates/iyakki-agent/src/profilestore.rs`). No tar crate, no zip crate.

## 4. Design

### 4.1 Layout (BUILT)

| Path | Contents |
|---|---|
| `<data>/web-profile/<user>.sealed` | the sealed profile (header, key check, authenticated chunks) |
| `<data>/web-profile/<user>.sealed.bak` | the generation before (a hard link, so a sealed file exists at every moment) |
| `<data>/web-profile/<user>.sealed.new` | a write in progress; removed at the next start |
| `<data>/web-profile/<user>.pre-protect` | the plain profile kept during the migration, removed at the first successful open |
| `/run/iyakki/profile-live/<user>` | the decrypted copy while open (RAM disk, mode 0700, files 0600), with `data`, `config`, `cache` and an `.open` marker |
| `<data>/web-profile/<user>` | **a link to the decrypted copy while open**, so every caller and the helper find the profile at the path they always used. Absent while sealed |

Outside the production data folder (`/var/lib/iyakki`) the decrypted copy lives in `<data>/web-profile-live` unless `IYAKKI_PROFILE_RUN`
names another folder: that is for development and tests and is not a RAM disk. Settings does not claim otherwise (see D2).

### 4.2 Format (BUILT)

Header: 8 magic bytes `IYKBP1\0\0` and a random 16-byte file id. Then a key check (a known text encrypted under the key: a wrong key is told
apart from damage). Then chunks of up to 1 MiB: `u32` length, XChaCha20-Poly1305 ciphertext. The nonce is the file id and a chunk counter; the
associated data holds the header, the counter and a "last chunk" flag, so a flipped byte, a reordered chunk, a dropped last chunk and a
file from another person all fail. The plain stream is a tar-like archive of `data` and `config` only: directories and regular files,
no links, no special files, relative paths that start in `data` or `config` and have no `..` (extraction refuses anything else). `cache`
is rebuilt, stays in RAM and is never sealed. Limits: 4 GiB of file bytes and 400 000 entries; a bigger profile is refused, never cut.

### 4.3 Key handling (BUILT)

* The key is 256 random bits kept as the vault secret `browser-profile` of the **person's own vault** (`uvault:browser-profile`). It is
  never on disk unwrapped: the vault file holds it inside the XChaCha20-Poly1305 data, which is wrapped by Argon2id of the login password.
  The alternative (a dedicated key slot in a separate file) was rejected: a second file to keep in step, no gain.
* **Password change**: the vault re-wraps its data key (existing `on_password_change`); the profile key and the sealed file do not change.
  Tested: the sealed file is byte-identical after the change and opens with the key read from the vault under the new password.
* **Forgotten password**: the profile is lost. Say it plainly at the switch (the confirmation card does), and again on the sign-in
  warning. An administrator reset of the password already starts an empty personal vault (admins cannot read user secrets), which takes the
  key with it. The person then sees "Your protected browser data could not be opened" and the page offers **Reset browser data** (key X): it
  deletes the sealed files without any key, forgets the key and turns protection off.
* **Key in memory**: held (zeroized on drop) from sign-in until sign-out, so write-back needs no password. Same lifetime as the vault's data key.
* Rotation (`rekey`) re-encrypts under a new key; it is not needed for a password change and is not in the UI yet.

### 4.4 Lifecycle (BUILT unless marked)

| Moment | What happens |
|---|---|
| Sign-in (`Services::on_login`) | if protection is on: read the key from the vault, decrypt into the RAM folder, link it in place. Failure keeps it sealed, audits `browser.data.locked`, toasts a hint; the engine refuses to start beside it (`prepare_profile` returns the reason; it never creates a fresh plain profile next to a sealed one) |
| Browser running | the engine uses the usual path; nothing else changes |
| Checkpoint (`on_checkpoint`) | writes back and keeps the copy open. BUILT as a function; **not called on a timer yet** (D6) |
| Sign-out (`Shell::sign_out`, after the browser tab is closed) | write back atomically, then overwrite and remove the RAM copy and the link. If the write-back fails the RAM copy is kept (nothing lost), `browser.data.seal.failed` is audited; the key is forgotten either way |
| Screen lock | **No lock screen exists** (`LOCK-AND-ENCRYPTION.md` A). Recommendation for when it exists: locking hides the screen; sign-out and the idle timeout seal. A setting `Lock browser data when I lock the screen` (default off) seals on lock and asks the password to open. PROPOSED |
| Idle timeout | PROPOSED: seal after N idle minutes (default off), only when no download or media is running |
| Crash, power loss | RAM is empty after a reboot. After a crash of the shell alone the decrypted copy is still in RAM: `Services::open` runs `wipe_stale` first thing and removes every decrypted copy, dangling link and half-written `.new` file. Changes since the last write-back are lost (D6) |
| Atomic write-back | write `.new`, fsync, read it back and compare a SHA-256 of the plain stream, hard-link the old sealed file to `.bak`, rename `.new` over the sealed file, fsync the folder. A stop at any point leaves the old sealed file whole (tested at each point) |

### 4.5 Migration of an existing plain profile (BUILT)

Settings -> Browser -> **Protect browser data** (key P) after a confirmation card. Steps, each safe to repeat:

1. make the key in the vault; write `.sealed.new` from the plain `data` and `config`, read it back (digest);
2. extract-compare: every file and folder of the sealed file is compared with the plain files, and the entry counts must match;
3. rename `.new` to `.sealed` (a stop here leaves both; status says "protection was interrupted", the engine is blocked, pressing Protect again finishes);
4. rename the plain folder to `<user>.pre-protect` (the rollback);
5. open the sealed file for real (decrypt into RAM, link in place): this is the "verified" point; on success the rollback copy is overwritten with zeros and removed. If the open fails the plain folder is restored and the sealed files removed.

The plain copy therefore exists on the drive only between steps 1 and 5, seconds for a normal profile. The removal overwrites with zeros once
and unlinks; on a journalling file system, SSD or SD card a copy of the old blocks can remain (wear levelling): **this is best effort and
the page says so.** Real protection from a drive that held the plain profile earlier needs whole-volume encryption or a fresh drive.
The browser must be closed first (the card says so); there is no cross-process check yet (UNVERIFIED).

### 4.6 Per-site options

* **Never persist cookies for a site** and **clear on exit**: PROPOSED, separate work. They need a hook the engine does not give today (the helper would have to delete a site's rows from `cookies.sqlite` and its storage folders at sign-out, before sealing; doable as a pass over the extracted tree before sealing because the SQLite file is plain inside RAM). Recommendation: implement as "clear on exit" first (a deny list in the person's browser config, applied by the sign-out path before the seal), "never persist" later with the helper's cookie accept policy.

### 4.7 Costs

| Item | Expected (UNVERIFIED: not measured) |
|---|---|
| RAM | the profile size, plus a few MB; typical profiles are tens of MB, IndexedDB-heavy sites can reach hundreds. 4 GiB hard limit in the format; the RAM disk is a tmpfs, so a very large profile competes with the browser for memory (the Mac mini and Pi targets matter most) |
| Sign-in and sign-out time | proportional to size: ChaCha20 is far faster than the disk, so it is disk-bound; the write-back also reads the sealed file back (twice the I/O of a plain copy) |
| SQLite on tmpfs | faster than on flash; WAL and journals live in RAM |
| Wear | one rewrite of the profile per sign-out instead of continuous small writes: less than before |
| Cache | `cache` is in RAM and wiped at sign-out; not sealed |

### 4.8 Cloud mode (PROPOSED)

Preferred shape (P17): the browser runs next to its profile (a pod with a persistent volume, shown through the web UI), so the profile
never moves. If a profile must move (a person switches machines), move **the sealed file only**, never plain cookies: the format already
authenticates the owner's key and can be copied as is. One active session per profile: a lock file next to the sealed file on the
volume with an owner id and a lease, taken at open and released at seal; a second open refuses and says where it is open. Per-site
allow-list when exporting is a filter over the archive before sealing. Nothing is silent: moving needs the person's confirmation card.

## 5. Decisions D1..D10

| # | Decision | Recommendation (built unless noted) |
|---|---|---|
| D1 | Default | **Off** for every existing and new profile; one button migrates. Revisit "on for new accounts" after a release of use |
| D2 | Where the decrypted copy lives | RAM disk `/run/iyakki/profile-live` on the production image; in the data folder only for development paths (not a protection there). Needs `/run` to be a tmpfs: from the base image (verify on the Mac, UNVERIFIED) |
| D3 | Key location | A vault secret `browser-profile` in the person's own vault; no new key slot |
| D4 | Forgotten password | Data is lost; **Reset browser data** offered; never an escrow or admin backdoor |
| D5 | Lock screen | When it exists: lock hides, sign-out and idle seal, option `Lock browser data when I lock the screen`. Not built (no lock screen) |
| D6 | Crash recovery | Wipe the decrypted copy at start; accept losing changes since the last write-back; add a timed `on_checkpoint` (every 10 minutes while the browser is open, and when the person leaves the browser tile) so the loss window is small. The checkpoint is not wired yet |
| D7 | Seal failure at sign-out | Keep the RAM copy, audit, forget the key; the copy is wiped at the next start. A person who signs in again before that sees the copy as it was; the next sign-out retries. Alternative (wipe anyway) loses data |
| D8 | Startup registry | No entry: there is no start step and nothing waits. Failure is shown by the audit line, the toast and the Settings status |
| D9 | The reader browser's `browser-data-<user>.json` (bookmarks, history, downloads) | Out of this slice: addresses and titles are less sensitive than cookies but are a privacy leak. Cheap follow-up: put the file inside the sealed profile folder |
| D10 | Per-site clear on exit and never-persist | After this slice, as a pass before sealing (4.6) |

## 6. What is built and tested (this slice)

Code: `crates/iyakki-agent/src/profilestore.rs` (`ProfileStore` open, seal, checkpoint, status, rekey, reset, protect, unprotect, rollback;
`wipe_stale`; the sign-in, sign-out and checkpoint hooks; `key_in_vault`); the hook in `webengine::prepare_profile`; `Services::on_login`
and `Services::open`; `Shell::sign_out`; `iyakki dev diag` shows one line (counts only, no names or keys); Settings -> Browser row and
buttons (`apps/settings_browserprotect.rs`). `iyakki dev state` shows the Settings page text, which carries the status text.

Tests (all with fake files in temp folders): round trip and mode bits, no plain name or content left after seal (scan), wrong key refused
and never falls back to the backup, password change re-wraps the vault and the sealed file stays byte-identical, a changed byte, a cut
file, a dropped last chunk and swapped chunks are detected and the backup is used, a crash leftover is wiped, migration with a stop and
repeat and rollback, two people cannot open each other's files, an interrupted seal at each of four points keeps the old copy, links are
not followed, path traversal in an archive is refused, hooks and the engine block, Settings cards, audit lines, forgotten password path,
hit rectangles at five sizes.

## 7. Not verified

No real WebKit profile (the sizes, file-locking and SQLite WAL behaviour of a live WPE profile); a sealed copy of a profile that is being
written (checkpoint while the browser runs) can capture a WAL file and its database at slightly different moments; SQLite recovers from
that normally but it is untested. No real Mac or tmpfs timing. `/run` being a tmpfs on the image. The browser being fully stopped before
the sign-out seal (the sign-out path closes the tab and the helper's drop waits for the child, read in `webview/client.rs`, but there is no
check that all engine processes are gone). The Settings text at real sizes on a device.

## 8. Recommended next steps

1. Wire the timed checkpoint and the "clear on exit" pass (D6, D10); a check that no engine process still uses the live folder before the seal.
2. A real-profile run on the Mac mini (size, time, WAL consistency) and on a Pi.
3. Lock screen (LOCK-AND-ENCRYPTION A) and the `Lock browser data when I lock the screen` option.
4. Put the reader browser's data file inside the sealed profile (D9).
5. Whole-volume encryption (LOCK-AND-ENCRYPTION B) for everything else; keep this per-person layer.
