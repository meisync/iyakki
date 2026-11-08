# Remote hot patch: fix a file on a running device, without a new image

**For:** developers who find a bug on a device (a MacBook booted from a USB stick, soon its internal disk) and want to fix it now,
and the administrators who must be able to see and undo what was changed. **You will get:** the pack format, where a patch lives on the
device, how it takes effect live and at boot, the safety rules, and what is and is not verified.
Command: `iyakki dev patch` (scope `update`, see [DEVELOPER-MODE.md](../DEVELOPER-MODE.md)). Code: `crates/iyakki-agent/src/devmode/patch.rs`
(pack, rules, live apply, undo), `cli_patch.rs` (the computer's side), `api.rs` (`patch_route`), `grants.rs` (approval kind `patch`),
`crates/iyakki-stage1/src/lib.rs` + `main.rs` (the boot overlay). Tests: `devmode/tests_patch.rs`, the `patch layers` tests of `iyakki-stage1`.

## 1. Why

This week the K8S tile was fixed on the owner's MacBook by replacing the `kubectl` symlink with a wrapper that moves `-A` after the
verb, and an audio helper and `/etc/asound.conf` were bind-mounted by hand over SSH. Each fix was invisible afterwards (no list, no undo,
gone at the next restart) or needed an image rebuild and a drive swap. A patch makes that first-class: signed requests, checked files,
an approval on the device, an audit trail, a list, a way back, and persistence across restarts.

The motivating patch (`patch.json` in the source folder; `iyakki dev patch build` fills in the sizes and hashes):

```json
{
  "format": 1, "id": "k8s-verb-first", "title": "kubectl wrapper: -A after the verb",
  "base": "current", "obsolete_from": "0.1.0-alpha.3",
  "ops": [ { "path": "/usr/bin/kubectl", "action": "replace", "mode": "0755" } ],
  "post": [ { "do": "sync" } ]
}
```
with `files/usr/bin/kubectl` holding the wrapper (`exec /usr/bin/k3s kubectl "$@"` with the arguments reordered). Then:

```
iyakki dev patch build ./k8s-verb-first -o k8s-verb-first.ipatch
iyakki dev patch apply k8s-verb-first.ipatch          # live and kept after a restart
iyakki dev patch list
iyakki dev patch remove k8s-verb-first                # the symlink to k3s comes back
```

## 2. The pack

`IYPATCH1` container: 8 bytes magic, 4 bytes manifest length, the manifest (JSON), the file contents in manifest order, and a SHA-256 of
everything before it. A folder with `patch.json` and `files/<absolute path>` is accepted the same way (it is built and checked first).
Every file also has its own SHA-256 and size in the manifest. This is a checksum, not a signature: the authenticity is the paired
computer's signed, TLS-protected request plus the approval on the device. (A developer signature over the manifest is a possible
later step; it would not replace the approval.)

Manifest fields: `id` (1 to 64 of `A-Za-z0-9._-`, the same rule as root ids), `title`, `description`, `base` (`*` any root, `current` the
root that is current when applied, or a root id), `obsolete_from` (the IYAKKI version that fixes it for good), `ops`, `pre`, `post`.
An op: `path` (absolute), `action` (`add`, `replace`, `delete`), `kind` (`file`, or `symlink` with `link`), `mode` (octal text, default
`0644`), `uid`/`gid` (default 0), `size`, `sha256`.

**Steps** are a fixed vocabulary: `restart_service` (only helpers: sshd, wpa_supplicant, dhcpcd, udhcpc, dnsmasq, chronyd, pipewire),
`remount` (path, rw), `drop_caches`, `sync`, and `shell` (any command; only with the `shell` scope AND a valid shell approval). A
failing `pre` step stops the patch before anything changes; a failing `post` step is reported as a warning and the patch stays.

**Limits:** 200 files, 64 MB a file, 128 MB a patch, 16 active patches, 8 steps before and 8 after, the data volume must have twice the
patch plus 64 MB free.

## 3. Paths

| Class | Paths | Needs |
|---|---|---|
| normal | `/usr/bin /usr/sbin /usr/lib /usr/libexec /usr/share /usr/local /opt /etc/iyakki/ /etc/alsa/ /etc/asound.conf` | scope `update` |
| wide | the rest of the root (`/etc`, `/bin`, `/sbin`, `/lib`, `/var` outside the data volume, setuid or setgid modes, free `shell` steps) | scope `shell` and a valid shell approval |
| next start only | `/init`, `/sbin/init`, `/usr/bin/iyakki`, `/lib/modules`, `/lib/firmware` | wide, and stored only (never live) |
| never | `/boot`, `/.stage1`, `/var/lib/iyakki` (the data volume: vault, keys, settings), `/home`, `/root`, `/proc /sys /dev /run /tmp /mnt /media`, `/etc/passwd group shadow gshadow sudoers`, `/etc/ssl/private`, `ssh_host_*`, any `.ssh`, any secret-looking name (`*.key`, `*.pem`, `vault*`...), components that are not plain names (`..`, spaces, control characters) | nothing lifts it |

A `replace` or `delete` needs the target to exist, an `add` over an existing file says so, a folder is never a target.

## 4. Where a patch lives

On the DATA volume, never the boot partition (a failed patch must not be able to stop GRUB):

```
iyakki/patches/active           ids, one per line, the winner first (written last, atomically)
iyakki/patches/<id>/patch.json  the manifest as applied
iyakki/patches/<id>/layer/      the files as they appear in the root; deletions are overlay whiteouts
iyakki/patches/<id>/layer.txt   what the first stage checks: id, base (resolved root id or *), file SIZE PATH / link / del lines
iyakki/patches/<id>/undo/       undo.json + files/: what was there before (content, mode, owner, link target)
iyakki/patches/<id>/state.json  active | obsolete | removed, who applied it, when, which start it is live in, what was done live
iyakki/patches/boot-count, disabled, skipped, last-failure, booted    the boot side (section 6)
iyakki/patch-staging/<id>/      files being sent (named f0000...; never by target path); removed after apply
```
(`iyakki/` is the folder on the data volume that also holds `iyakki/root/`; the running system sees the volume at `/var/lib/iyakki`.)

## 5. Live

`apply` (default `--live`; `--next-boot` stores it and touches nothing running): plan on the device (paths, existence, room, `base`,
`obsolete_from`) -> files sent in chunks and checked -> the folder is built beside its name and renamed in -> `pre` steps -> each op is
made true -> `active` is written -> `post` steps. Per op:

* a regular file: a **bind mount** of the layer's file over the target (the original stays under it; a running binary keeps its inode);
  an `add` first creates an empty target to bind over;
* a **symlink** target (the kubectl case): bind mounting would land on the link's TARGET and replace `k3s` itself, so the link is removed
  and the file written in its place (in the writable layer); `remove` puts the link back from `undo/`;
* a deletion: the file is unlinked; `remove` restores it from `undo/`;
* a path that is next-start only is skipped live and reported (`needs_reboot`).

Any failure undoes what was done, deletes the patch folder and leaves `active` as it was. The root's writable layer is RAM, so none of
this survives a restart by itself; that is what section 6 is for. What cannot be live: the PID 1 `iyakki` binary and everything it has
mapped, kernel modules, firmware. They are stored, flagged `needs_reboot`, and take effect at the next start.
`remove`: live changes of this start are undone, the id leaves `active`; a layer the running system was started with is still held by
the kernel, so its folder is only marked `removed` and deleted by the first `boot_ok` of a later start.
`list` shows `live` (applied in this start), `active` (in this start's boot overlay), `next start`, `obsolete`, `removing`.

## 6. At boot (lean install)

`iyakki-stage1`, after choosing the root: `plan_patches` reads `iyakki/patches/active`, and for each id, in order, checks `layer.txt`
(parses, names this id, `base` is `*` or this root's id) and that every listed entry exists and every file has the listed size (stat only, no
hashing: the hashes were checked when it was applied). The overlay becomes
`lowerdir=<patch layers, winner first>:/lower,upperdir=...`. Fail-safe, in this order:

1. no `active` file, or nothing valid: the unchanged overlay (the existing code path);
2. a layer that does not verify is **skipped** with a log line (`stage1.log`) and an entry in `iyakki/patches/skipped`; the others are used;
3. if mounting the overlay with layers fails, it is mounted again without them, `iyakki/patches/last-failure` says why;
4. **boot-loop guard:** every start that uses layers adds one to `iyakki/patches/boot-count`; the running system removes the file after
   30 s of uptime (`patch::mark_boot_good`). At 3 counted starts that never got there, no patch is used, `disabled` is written, and
   `list` prints `GUARD`. Applying or removing a patch clears it. This also covers a broken PID 1 binary delivered by a patch.
   It is independent of the existing `iyakki-lean-failed` marker (which falls back to the full boot);
5. `iyakki/patches/booted` records which layers this start really used.

A new image or tree: a patch whose `base` names another root is skipped at boot (`obsolete`), and `refresh_obsolete` (on `list` and at
`boot_ok`) drops it from `active` and marks it so, as it does for a patch whose `obsolete_from` is reached. `Superseded` is the same id applied
again (the old one is removed first, the ORIGINAL file is what comes back on `remove`).
Patches are applied newest first; one patch overriding another's file wins by order.

## 7. Full boot (non-lean install) and QEMU

The full boot unpacks the RAM-disk root itself; there is no first stage. `patch::overlay_into(data, root)` is the same layer idea in the
init: after unpacking, copy the verified active layers (lowest priority first) over the root, using `layer.txt` for deletions, skipping a
layer that does not verify. It is implemented and tested on folders; it is NOT yet called from the full-boot init. Live apply is the same
code on both. The QEMU emulator: patches work like on hardware because the data volume is a disk; `dist/buildroot/qemu-patch-test.sh`
(an outline, not run) plants good, corrupt and counted-out layers on a private copy of a lean image.

## 8. Authority, approval, audit

**One grant per computer and kind (2026-11-07).** `grants.rs` keeps the approvals of a computer per kind (`shell`, `power`, `patch`); before,
a computer had one slot and approving the patch card replaced its shell grant, so a patch of a wide path (`/usr/bin/iyakki` on the Mac mini)
was refused with "outside the places a patch may change without the shell scope and its approval" even right after `--shell`. A grant of
one kind never stands in for another (`shell_status_kind`, `shell_grant_kind`). Test
`devmode::tests_adb::a_patch_approval_keeps_the_shell_grant_and_one_kind_never_stands_in_for_another`.

* every route needs the `update` scope (administrator accounts only, granted per computer on the device);
* **approval card, once per computer:** the first `apply` or `remove` is answered `not approved`; the CLI asks (`/v1/dev/grant/ask`, kind
  `patch`), the administrator approves on the device's screen (the top bar shows `PATCH REQUEST from <computer>`); the computer is then
  remembered (`devmode/patch-trust.json`) until it is revoked or developer mode is switched off;
* wide paths, free `shell` steps and setuid bits need the `shell` scope and a currently valid shell approval every time (`--shell` asks);
* audit lines: `devmode.patch.plan`, `.apply` (id, live or next start, file count, wide), `.remove`, `devmode.patch.failed`, and the
  activity log of developer mode; refusals are logged as refused;
* the secret-name and never-list rules cannot be lifted by any scope.

## 9. What is verified, what is not

Verified by unit tests on folders and a fake of the system effects: the pack, tamper detection, the path table, the manifest rules, the
kubectl link swap and undo, bind/create/delete/undo, rollback on a failure halfway and on step failures, next-start mode, obsolete and
superseded, limits, list status, `boot_ok`, the full-boot overlay, the route gates and audit, and the first stage's pure decisions (no
patches dir, valid layers, corrupt `layer.txt` skipped, wrong size, other root, guard after 3 starts).
**Not verified on any real boot:** the overlay mount with extra lower layers in `main.rs`, whiteouts (`mknod`) in a layer on ext4, bind
mounts on the device, `restart_service` (it sends SIGTERM to a helper by name and relies on whatever starts it again), the 30 s
`mark_boot_good`, `overlay_into` in the full-boot init (not wired), the Settings card for kind `patch` (it reuses the shell request card's
wording; check it before relying on it), the QEMU script.
