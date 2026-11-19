# Layered root images and delta update packs (design)

Status: built and tested in QEMU and with fakes (2026-11-11); nothing has run on a real machine. Owner decision 2026-11-11: goes into alpha.3. Related: [`COMPONENT-UPDATES-PLAN.md`](COMPONENT-UPDATES-PLAN.md) (the larger plan this is the first slice of), [`../BOOT-SPEED.md`](../BOOT-SPEED.md) (the lean boot), [`CORE-COMPATIBILITY-DESIGN.md`](CORE-COMPATIBILITY-DESIGN.md) (additive rules, schema numbers).

## Problem

A lean install has one `root.sfs` (417 MB at zstd -19, 467 MB at -3). An update pack carries all of it, so every push moves 420 MB and writes a 420 MB file to the data volume, although a change usually touches one program. The 465 MB boot volume has about 12 MB free, so the seed root cannot be doubled there.

## Layers

| Layer | Holds | Changes | Size (measured, zstd -3) |
|---|---|---|---|
| `base` | libraries, BusyBox, web engine, kernel modules, firmware, empty mount-point folders | weeks | 398 MB |
| `core` | the `iyakki` program, its data (`usr/share/iyakki`), release stamp, notices, helper libraries | daily | 39 MB |
| `ui` | fonts, icons, samples, the web view helper, theme, and `usr/bin/iyakki-ui` + `usr/libexec/iyakki-ui/**` (rules reserved, tested by `test-ui-layer.sh`; the shell program moves here when it is its own file) | hourly | 8.5 MB |
| packs | optional layers (an add-on), `optional: true`: a missing one is left out of the stack | on demand | |

The split is **data**: `dist/buildroot/layers.txt` (rules: first matching glob wins, last rule `base **`). `layers.py` walks the root, gives every file to one layer and builds one squashfs per layer (zstd, `-all-time 0 -mkfs-time 0`, so the same files give the same bytes and an unchanged layer keeps its hash). A file lives in exactly one layer, so the stack needs no whiteouts. Folders are in every layer that has something under them; folders with nothing below them (mount points) are in `base`.

Each layer file is **content-addressed**: `NAME-<first 12 hex of SHA-256>.sfs`. A changed layer is a new file; the old one stays until nothing names it.

## The layer manifest (`layers.json`, schema 1)

```json
{ "schema": 1, "release": "20261111...", "layers": [
  { "name": "base", "volume": "boot", "path": "boot/layers/base-7d1d6fc52e39.sfs", "sha256": "...", "size": 417124352,
    "provides": {"base_abi": "1"}, "requires": {} },
  { "name": "core", ..., "provides": {"core_iface": "1"}, "requires": {"base_abi": "1..1"} },
  { "name": "ui", ..., "requires": {"core_iface": "1..1", "base_abi": "1.."} } ] }
```

Bottom layer first. `provides` are interface numbers (the surfaces of the compat charter; `base_abi` is the contract between the base libraries and the programs, `core_iface` between core and the shell). `requires` are ranges (`1..1`, `2..`, `..3`, `6.12..6.13`). A set is usable only when every `requires` of its layers is met by a `provides` of the same set (or by the running kernel release, key `kernel`). Rules follow the charter: unknown fields are ignored, a **newer schema is refused by name**, nothing is guessed. Code: `crates/iyakki-stage1/src/layers.rs` (one definition, used by the first stage and the update code).

## Where the layers live and which set starts

* **Seed set** (a fresh layered install): `boot/layers.json` + `boot/layers/*.sfs` on the FAT boot volume, instead of `boot/root.sfs` (both do not fit).
* **Updated sets**: `iyakki/layers/NAME-HASH.sfs` on the data volume and the manifest `iyakki/layers/current.json` (the set that starts) and `previous.json` (the set it replaced). A layer in an updated set that did not change is **referenced where it is** (`volume: boot` for the seed base), so a core-only update writes only the core file. The data volume has the room; the boot volume does not.
* **Order the first stage tries**: `current.json`, `previous.json`, `boot/layers.json`, then the existing single roots (`iyakki/root/current`, `previous`, `boot/root.sfs`), then the failure screen. The boot menu's "IYAKKI (previous version)" (`iyakki.root=previous`) skips `current.json`, so it starts `previous.json`. Every set is checked before use: each file exists with exactly the promised size (a half-written layer has another size), and the requires hold. A set that fails is skipped with a line in the boot log and the next one is tried.
* **Single roots keep working untouched**: an install with no `layers.json` boots exactly as before; a single-root update retires the layer pointers (`current.json`, `previous.json` renamed `.retired`) so the newer root is not shadowed.

## Stacking: overlayfs (chosen) or bind mounts (not needed)

The first stage already puts an overlay over the root (RAM-backed upper layer, plus hot-patch layers). Layers become more lower directories: each squashfs is attached to a loop device and mounted read-only at `/layers/NAME`, then `lowerdir=<patches>:/layers/ui:/layers/core:/layers/base`. `CONFIG_OVERLAY_FS=y` and `CONFIG_SQUASHFS_ZSTD=y` are already in `dist/buildroot/board/iyakki/linux.fragment` (and the lean boot already uses both), so **no kernel rebuild is needed**. A fallback without overlayfs (mount each layer and symlink) was considered and rejected: it breaks `/usr` as one tree, and the kernel has the feature. Cost: 24 ms to mount three layers in QEMU. Overlayfs allows at most 500 lower layers; the cap here is 12.

Hot patches (`iyakki dev patch`) still stack above the layers. A patch made for a specific root id is skipped on a layered root ("made for root X, not this one"); patches for any root (`*`) work. Patches for layered roots keyed to the set are future work.

## Update pack v2

`update.json` gets `format: 2` and a `layers` section (the target set plus, per layer, `keep`, `full` or `delta`). An old device reads the JSON and refuses with its existing plain message ("update format 2 is not understood (this device reads format 1)") before anything is staged: **never applied half**. Format 1 packs apply as before. The pack still carries the kernel, the small RAM disk and the menu (they are small and compared by hash as today); the layer payload files sit under `boot/layers/` in the pack and are staged to the data volume, never written to the boot volume:

* `keep`: the device must already have a layer of that name and hash; nothing is carried.
* `full`: the squashfs.
* `delta`: `NAME-NEW.from-OLD.delta`, made by `iyakki-bindelta` (our bsdiff-style delta, reused from the component work) against the old layer file; the device streams it against the layer it has (`apply_delta_files`, small buffers), checks the result's SHA-256 and size.

Device steps (plan, then apply; the plan already refuses before any byte is staged): check the manifest and requires against what the device will have after the update (names the layer that must go first); check the `keep`/`delta` bases exist; check data-volume room; stage (hash-checked chunks as today); then apply: (1) the previous kernel/RAM disk copy and menu entry as today, (2) for each changed layer write `NAME-HASH.sfs.new`, fsync, check size and SHA-256, rename to its final name, (3) write `previous.json` (the old effective set) by write-temp-fsync-rename, (4) write `current.json` the same way **last**, (5) remove layer files no manifest names, (6) kernel and menu as today. A power cut before (4) leaves the old set starting (new files are unreferenced extras, removed by the next update); after (4) the new set starts and is complete. Tested by stopping after every step.

## Fallback chain and room

1. `current.json` set, 2. `previous.json` set (also what the previous-version menu entry starts), 3. seed layers, 4. single roots, 5. GRUB's "IYAKKI (previous version)" and full-start entries. Layers never need to exist twice on the boot volume: updated layers go to the data volume one file at a time (copy-then-rename); the data volume holds at most the layers of `current` and `previous` plus the one being written.

## Measured pack sizes

See `../BOOT-SPEED.md` section "Layered root": nothing changed 0 bytes, a ui change 23 KB, a small core change 44 KB, a base library 1 MB, a rebuilt program 38 MB (15.5 MB if the core layer is stored uncompressed; not switched on). Before: 417 to 467 MB every time.

## Not done / decisions for the owner

* The shell is still inside the `iyakki` program, so a shell change is a core change today (one rule in `layers.txt` moves it when the program is split).
* `store` (uncompressed core layer) halves the rebuilt-program delta but needs 60 MB more on the data volume; not on by default.
* Patches made for one root id do not match a layered root (patches for `*` do).
* No real machine has run layers yet (Mac mini, Lenovo, HP, MacBook Pro): the QEMU run is the only boot proof.
