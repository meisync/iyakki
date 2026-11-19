# Remote patch, binary delta: ship the difference, not the 80 MB program

**For:** developers who change code and want it on a device in seconds. **You will get:** the delta format, the limits, what the device does
with it, and the exact commands. Parent documents: [REMOTE-PATCH.md](REMOTE-PATCH.md) (the pack, the paths, live and boot behaviour),
[COMPILE-SPEED-PLAN.md](COMPILE-SPEED-PLAN.md) (Phase 4, option 1). Code: `crates/iyakki-agent/src/bindelta.rs` (the delta),
`devmode/patch.rs` (`build_with`, `check_delta_old`, `write_delta_result`), `devmode/cli_patch.rs`. Tests: `bindelta_tests.rs`, the
`delta` tests at the end of `devmode/tests_patch.rs`.

## 1. Why

`/usr/bin/iyakki` is about 80 MB. A hot patch refuses a file over 64 MB (128 MB a patch), so a code change meant a 420 MB update pack. A
change touches a small part of the program, so the pack now carries a **delta**: how to make the new program from the old one that is already
on the device. The 64 MB limit applies to the delta, not to the program it makes.

## 2. The delta (`bindelta.rs`)

`make_delta(old, new) -> Vec<u8>` and `apply_delta(old, delta) -> Result<Vec<u8>, String>`. Our own code; the only crates are `sha2` and
`ruzstd`, which the agent already used (no new dependency).

Method: the bsdiff idea with a hash index. The old file is indexed at every 8th byte by a hash of 16 bytes (about 64 MB of table for an 80 MB
file; bsdiff's suffix array would need 640 MB). The new file is walked; where a run also exists in the old file it is found through the hash,
extended forward and backward across a few changed bytes (a relinked program differs in many addresses, not only in the changed function),
and written as pieces of three kinds: bytes **added** to the old file's bytes (the `diff` stream, mostly zeros), new bytes **copied** as they
are (`extra`), and a **move** in the old file (`ctrl`, varints). Each stream is zstd-compressed (`ruzstd`, "fastest" level).

Layout (all numbers big-endian): `IYBDELT1` | version `1` | old length | new length | SHA-256 of old | SHA-256 of new | three streams, each
`raw length, compressed length, zstd frame` (ctrl, diff, extra) | SHA-256 of everything before it.

`apply_delta` trusts nothing: the trailer first (cut, flipped byte, wrong magic and unknown version are named), then the old file's length and
SHA-256 ("not the version this delta was made against"), then each stream's announced size against what a file of that length can hold (no
allocation from a forged size), then every piece's bounds, then the new file's SHA-256 against the promise in the header. It returns the file
only if all of that holds. Largest file either side: 512 MB.

## 3. The pack entry

An op in `patch.json` (and in the pack's manifest) with a `delta` object is a delta file; the rest of the format is unchanged, so staging,
chunked upload and the routes need no change:

```json
{ "path": "/usr/bin/iyakki", "action": "replace", "mode": "0755",
  "size": 12358199, "sha256": "<of the delta>",
  "delta": { "old_sha256": "<of the file on the device now>", "new_sha256": "<of the file that results>", "new_size": 77852448 } }
```
`size` and `sha256` describe the **delta** (what is carried and checked in transit, 64 MB limit); `delta` describes the file before and after.
Only `replace` can be a delta (there is nothing to start from for `add`). The manifest check requires both hashes and `new_size` at most
512 MB. Free-space check on the device: for each delta, twice `new_size` (layer file and undo copy) plus the delta.

## 4. The device side

1. **plan** (before a byte is sent): the target must be a regular file, and its SHA-256 must equal `old_sha256`. If not: `<path> on this device
   is not the version this delta was made against ... nothing was changed. Build the delta against the program that runs on the device, or
   send the whole file.`
2. **apply**: the staged delta is checked against the manifest as any file is. Then the CURRENT file at the path is read again, its hash
   checked again (it may have changed since the plan), and the new file is made in memory (old + new at once, about 3 times the program, 330 MB
   measured for 78 MB), checked against `new_sha256` and `new_size`, and written into the patch **layer** beside its final name
   (`.<name>.delta-new`), flushed (`fsync`), renamed (atomic), and the folder flushed. Any failure removes the temporary patch folder: nothing
   on the running system was touched, and nothing is listed as active.
3. The layer's `layer.txt` lists the size of the file the delta made, which is what the first stage checks at boot.
4. Then it is an ordinary patch: a normal path is bind-mounted live (the original stays under it; `remove` unmounts it), and
   `/usr/bin/iyakki` keeps the existing **next start only** rule (needs the shell scope and approval; stored, never live; the boot overlay puts
   the layer's file in place at the next start, with the boot-loop guard of REMOTE-PATCH.md section 6). The running program file is never
   rewritten in place.

## 5. Commands

```
# 1. the source folder: patch.json + files/usr/bin/iyakki holding the NEW program
{ "format": 1, "id": "iyakki-dev", "title": "iyakki build 123",
  "ops": [ { "path": "/usr/bin/iyakki", "action": "replace", "mode": "0755", "delta": {} } ] }

# 2. build against the program that is on the device now (a file, a pack that carries it whole, or a folder laid out like the root)
iyakki dev patch build ./iyakki-dev -o iyakki-dev.ipatch --delta-from ./previous/iyakki
iyakki dev patch show iyakki-dev.ipatch      # replace delta N bytes -> file of M bytes
# 3. send it (shell scope: /usr/bin/iyakki is a next-start path)
iyakki dev patch apply iyakki-dev.ipatch --shell
iyakki dev reboot                            # or the service restart you use; the next start runs the new program
```
`--delta-from OLD` takes: a **file** (the old program; only when the patch changes exactly one file), a **pack** whose op carries that path
whole (a pack that is itself a delta cannot be an old version), or a **folder** with the old files at their root paths (or under `files/`).
An op marked `"delta": {}` must have an old version (otherwise the build stops). Without the mark, `--delta-from` turns every `replace` file
op into a delta where an old version is found and the delta is smaller than the file; the others travel whole. A delta op without
`--delta-from` is refused at build time. Keep the previous build's program (or its pack) around: the next delta is made against what the
device runs now.

## 6. Limits

| What | Limit |
|---|---|
| a delta carried in a pack | 64 MB (`MAX_FILE`), as for any patch file |
| files of a patch (deltas count by their own size) | 128 MB total, 200 files |
| the file a delta makes, and the old file | 512 MB (`MAX_TARGET`) |
| memory to make a delta | about old + new + 64 MB index + the diff stream |
| memory to apply | about old + new + the diff stream |
| inputs of 4 GiB or more | no index: the delta carries everything as new bytes (never reached under the 512 MB cap) |

## 7. Measured (2026-11-07, dev PC, release build of `bindelta.rs` alone, one thread)

Real builds of the main program that were on disk (no two consecutive builds with one small change were available, so these are several
days of changes each, a pessimistic case):

| Old | New | make | apply | Delta | Peak memory |
|---|---|---|---|---|---|
| 73.3 MB (3 Nov) | 77.9 MB (4 Nov) | 2.0 s | 0.21 s | 12.4 MB (15.9 %) | 316 MB (make + apply together) |
| 77.9 MB (4 Nov) | 80.6 MB (7 Nov) | 2.1 s | 0.22 s | 13.4 MB (16.6 %) | 329 MB |

For comparison `zstd -3` of the 77.9 MB program alone is 33.5 MB. Of the first delta: 4.8 MB of genuinely new bytes (3.4 MB compressed), the
byte differences of moved addresses 73 MB raw (8.5 MB compressed), moves 0.4 MB. So a delta of a few days of work is 12 to 13 MB; a
single small change should be smaller (the added code plus the moved addresses after it), but **that was not measured** on a real pair of
builds. The compressor is `ruzstd`'s fastest level; a better entropy coder on the diff stream is the obvious next saving.

## 8. What is verified, what is not

Verified by tests (no device): round trips (empty, tiny, random, truncated, extended, insertions, deletions, a moved block, a 5 % change,
a relinked-like program with moved addresses), size sanity, refusal of the wrong old file / flipped bytes / cut deltas / forged headers with a
fixed trailer / a forged huge size; the pack end to end with the fake system (live bind and undo, next-start for `/usr/bin/iyakki`, wrong old
file refused at plan and at apply with nothing changed, damaged staged delta, lying manifest, limits applying to the delta, the free-space
check, the builder with file, pack and folder sources). **Not verified:** a real device; a real consecutive-build delta of a small change;
the boot overlay using a layer file made this way (it is a normal layer file of the listed size); memory on a device with little RAM
(about 3 times the program is needed while applying).
