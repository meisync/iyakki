# Developer mode: install, debug and configure a device from your own computer

**For:** people who build tile apps, skills, MCP servers or extensions for IYAKKI and want to try them on a real device
without publishing to a store first, people who bring up IYAKKI on new hardware (a MacBook, a Mac mini), and the administrators who
allow that. **You will get:** setup, pairing, the **scopes** an administrator grants per computer, every `iyakki dev` command, the
system update, the IDE target, and the security model. In short: it is `adb` for IYAKKI (a table of equivalents is in section 4).

Code: `crates/iyakki-agent/src/devmode/` (`mod.rs` state, keys, pairing and logs; `auth.rs` the two proofs; `ops.rs` install and
remove through the STORE; `scopes.rs`, `grants.rs`, `sessions.rs` the permissions; `api.rs` every route beyond install, with its scope;
`debug.rs`, `files.rs`, `input.rs`, `config.rs`, `app.rs`, `shellexec.rs`, `tunnel.rs`, `update.rs`, `bootmount.rs` what the scopes do;
`client.rs`, `cli.rs`, `cli_adb.rs` the computer's side), `crates/iyakki-remote/src/devserver.rs` (the listener) and `devconfig.rs` (host
name, display size, firewall), `crates/iyakki-usb/src/bootpack.rs` (reads an install image's boot partition), `crates/iyakki-agent/src/directlink/` (one cable between two computers, section 10),
`crates/iyakki-shell/src/apps/settings_developer.rs` (Settings), `devinput.rs` (taps and keys into the shell), `apps/ide_tile.rs` (the IDE
target). Tests: `devmode/tests.rs`, `tests_adb.rs`, `tests_update.rs`, `crates/iyakki-remote/tests/devmode.rs` and `devmode_adb.rs`,
`settings_developer_tests.rs`, `shell_devadb_tests.rs`, `ide_tile_tests.rs`.

## 1. Switch it on (administrators, on the device)

Settings → **Developer mode** (System group; administrators only):

| Key | What it does |
|---|---|
| `e` | Switch on / off. Off by default. While off, nothing listens and every request is refused. |
| `h` | The automatic switch-off: 4 h (default), 1 h, 8 h, 24 h, 72 h, or never. Changing it while on restarts the countdown. |
| `p` | Pair a computer: shows a one-time code (10 characters, five minutes, five tries) and the device's certificate fingerprint. |
| `c` | Cancel the pairing code. |
| Up/Down, `x` | Choose a paired computer, revoke it (its next request is refused, and its open sessions end at once). |
| `s`, space | Move to the next scope and switch it on or off for the chosen computer (section 6). |
| `g` | While a pairing code is open: add or remove the scope under the cursor for the computer that is about to pair. |
| `a`, `o`, `d` | A computer is asking to run commands (or restart the device): approve for 10 minutes, approve for one session, refuse. |
| `k` | Disconnect the chosen computer: ends its shell access and every session it has open. |
| `l` | The activity log: every action, who did it (computer and account), when, and whether it was allowed. |

While developer mode is on the device listens on **TCP 7979** (TLS) on all its addresses; the page shows the address to type.
The listener starts within two seconds of switching on and stops within two seconds of switching off (by hand or by the timer).

For the IYAKKI menu and the top bar (another part of the shell): `iyakki_agent::devmode::set_enabled(data_dir, policy, user, on, hours)`
switches it (administrators only, logged), `devmode::is_on(data_dir)` says whether it is on, and `devmode::indicator(data_dir)` returns
`Some("DEV")` (or `"DEV · pairing"`) while it is on, for a sign in the top bar.

## 2. Pair your computer (once)

Install the `iyakki` program on your computer (the same binary as on the device: `cargo build --release -p iyakki-core`, or the
release tarball). Then, with a pairing code on the device's screen:

```text
$ iyakki dev pair 192.168.1.20
Device iyakki-box (IYAKKI 0.1.0-alpha.1) at 192.168.1.20:7979
Its certificate fingerprint: 3F:A2:...:91
Check that the device's screen shows the same fingerprint.
Pairing code shown on the device: K7QFM-2XPRT
Paired: this computer is "laptop" on 192.168.1.20 and acts as the account root. Saved as device "iyakki-box" in ~/.config/iyakki/dev.
```

* `--code CODE` gives the code without the question; `--name NAME` names this computer (default: its host name); `--as NAME` names the
  device on this computer; `--fingerprint FP` refuses to pair unless the device shows exactly that certificate.
* The computer's key is made the first time, in `~/.config/iyakki/dev/id.key` (mode 0600; `$IYAKKI_DEV_HOME` or `--home DIR` move it).
  `devices.json` beside it keeps each paired device: its address, port and the pinned certificate fingerprint.
* The new key acts as the **account of the administrator who opened the pairing**. (`DevMode::start_pairing_as` pairs it for another
  account; the Settings page uses the administrator's own account.)

## 3. Commands

Every command takes `--device NAME` (default: the first paired device, or the one set with `devices --default`) and `--home DIR`.

| Command | What it does |
|---|---|
| `iyakki dev devices [--default NAME \| --forget NAME]` | The paired devices, and this computer's public key. |
| `iyakki dev whoami` | The device, this computer's name there, the account and role it acts with, and whether developer mode is on. |
| `iyakki dev install <pkg.iyp \| folder> [--shared] [--open]` | Install, upgrade or reinstall. A folder is packed first: a project with `iyakki-app.json` (any kind `iyakki pkg pack` makes: tile, skill, MCP server, WASM tool, extension, template, theme, agent preset; there is no package kind for FLOWS workflows yet, so a workflow travels as an `extension` or `template` package that nothing reads automatically) or a bare tile app folder (`app.json` + `index.html`). Folder builds are signed with this computer's key. `--shared` installs for everyone (needs a role that may). |
| `iyakki dev uninstall <id>` | Remove something developer mode installed (`kind:name` or `name`). Its saved data is kept. Things installed from a store are not touched (remove them in the STORE). |
| `iyakki dev list [--all]` | What developer mode installed that this account can see; `--all` lists everything installed through the STORE that it can see. |
| `iyakki dev logs <id> [--follow]` | The app's log on the device: each install, reinstall and removal, and for a tile app the page's console (`console.log`, errors) while it runs on the screen. `--follow` keeps reading. |
| `iyakki dev run <id>` (or `open`) | Open the tile app on the device's screen (for the person signed in, if they have it; queued until someone signs in). |
| `iyakki dev push <folder> [--watch] [--open]` | Pack and install the folder; with `--watch`, again after every change (live reload: an open copy of the app reloads by itself on the device). A build that fails is reported and the watch goes on. |
| `iyakki dev screenshot [-o FILE.png] [--display N] [--record SECONDS [--fps N]]` | A PNG of whatever the device's screen shows; `--record` takes a numbered PNG sequence (at most 120 frames) into the folder given with `-o`. `--display N` is accepted, but the device draws on one screen (display 0) for now and refuses the others by saying so. |
| `iyakki dev info` | Device model (DMI vendor and product), image variant and version, kernel and its taint flags, loaded modules, uptime, memory, disks and mounts, network addresses, GPU drivers and connected outputs, sound cards, UEFI or BIOS, and the firmware files the kernel could not load. (`debug`) |
| `iyakki dev ps` | Processes (all for an administrator account, the account's own otherwise). (`debug`) |
| `iyakki dev dmesg [--follow] [--grep X] [-n N]` | The kernel log, from `/dev/kmsg`; credentials in a line are hidden. (`debug`, administrators) |
| `iyakki dev logs --system [--source shell\|boot\|audit] [-n N]` | The shell and agent lines (kernel log records tagged `[iyakki `), the boot log `/var/log/iyakki-boot.log` and the audit log tail. (`debug`, administrators) |
| `iyakki dev status` | A plain table of k3s, Lemonade, Ollama, firewall, shared folders, sound, sound server, graphics and the shell. (`debug`) |
| `iyakki dev files ls\|stat\|mkdir\|rm [-r]\|push LOCAL REMOTE [--force]\|pull REMOTE [LOCAL]` | Copy files to and from the device, in 4 MiB chunks, resumable, checked with SHA-256 at the end, folders too (at most 5000 files). Allowed places: the account's home, its workspaces and `/tmp/iyakki-dev`; `~` is the home. (`files`) |
| `iyakki dev input tap X Y\|key NAME\|text 'abc'\|swipe X1 Y1 X2 Y2 [MS]` | Taps, keys and text for the shell on the screen, as the shell's own events. Refused while a password field has focus unless the computer also holds `input:password`. (`input`) |
| `iyakki dev config list\|get KEY\|set KEY VALUE\|export [FILE]\|import FILE` | Settings through a whitelist with types and ranges (sound output and volume, time zone and clock, voice speed and engines, keyboard, cloud mode, chat and embedding models, limits, host name, display size, firewall on/off). `import` checks the whole profile before it applies anything. (`config`, administrator accounts for machine-wide keys) |
| `iyakki dev config provider add ID --kind K [--url U] [--key-file F\|--key-env V\|--key-stdin]` / `key ID ...` / `remove ID` | Model providers. The API key goes into the system vault and can never be read back: not by `list`, `export`, a log or the audit log. It is never taken on the command line. (`config`) |
| `iyakki dev config firewall list\|allow VALUE [--ports P] [--proto tcp\|udp\|both] [--minutes N]\|remove ID\|keep\|revert` | The firewall with the same 60-second keep-or-revert as Settings. After a change the command checks that the device still answers this computer and then keeps it; if it does not answer, the device reverts by itself. (`config`) |
| `iyakki dev app info\|permissions\|clear-data\|reset APP`, `app data APP [KEY]`, `app config APP get\|set\|rm KEY [VALUE\|--file F]`, `app logs\|console APP [--follow]` | The saved data (`iyakki.storage` keys) of a tile app that developer mode installed, so test data can be seeded; the page console is the app's log. Apps from a store are out of reach. `app grant\|revoke` says plainly that an app holds exactly what its manifest declares. (`config`) |
| `iyakki dev shell [--minutes N] [--root] [--reason TEXT] [-- CMD ARGS]` | Run commands on the device. Asks the administrator on the device's screen; with `-- CMD` runs one command and returns its exit status. Interactive otherwise: one command per line (no terminal: a line mode), the folder carries over, Ctrl+C sends SIGINT (twice: KILL). (`shell` + approval) |
| `iyakki dev forward LOCAL:REMOTE` | Forward `127.0.0.1:LOCAL` on this computer to port REMOTE of the device's own loopback (the Lemonade port 13305, the dev k3s API 6443...). (`forward`) |
| `iyakki dev reboot` / `poweroff` | Restart or switch off, after an approval on the device's screen every time. (`power`) |
| `iyakki dev update-pack IMAGE.img -o DIR [--version V] [--variant NAME]` | On the computer: read the install image's boot partition WITHOUT mounting it and write the update payload (kernel, initramfs, boot menu, theme, EFI loader, SHA-256 of each, `update.json`). |
| `iyakki dev update PAYLOAD\|IMAGE.img [--force] [--reboot]` / `update --status` | Replace the boot files with a newer IYAKKI (section 7). (`update`) |

Versions: the same version can be installed again (a rebuild) as often as you like; a higher version is an upgrade; a lower version is
refused, as in the STORE (the high-water rule). Raise the version in `iyakki-app.json`/`app.json` when you mean a new release.

**From the device itself.** The IDE's **Publish** dialog lists **This device (developer mode)** while developer mode is on, and
**Paired device NAME (developer mode)** for each device the signed-in account paired with. Both install through the same code as
`iyakki dev install` (the first directly, the second over the network), and a tile app opens straight away. The IDE keeps each account's
key and paired devices in `<data>/devmode/client/<account>/`; to pair from the device, run
`iyakki dev pair ADDRESS --home /var/lib/iyakki/devmode/client/<account>` in its terminal.

### The SDK on your computer (`iyakki sdk`)

`iyakki sdk init NAME [--kind tile|skill|mcp|service] [--dir D]` starts a project (the store's scaffolds, or a small service for k3s) with `CLAUDE.md` and `AGENTS.md`
for coding agents, `iyakki.d.ts` (typings of `window.iyakki`) and a `.gitignore` that keeps keys out; it refuses a folder that has files and works offline.
`iyakki sdk doctor` checks the program, the paired device and whether it answers, and says the next step (it never fails because no device is paired).
`iyakki sdk docs [topic]` lists the guides. Code: `crates/iyakki-agent/src/sdkcli.rs`, tests `sdkcli_tests.rs`. Files: `dist/sdk/README.md` (quickstart), `dist/sdk/iyakki.d.ts`,
`dist/sdk/templates/` (agent instructions; `.mcp.json` is for the future `iyakki dev mcp`, not built), `dist/sdk/EMULATOR.md` (a QEMU device for the loop: specified, NOT BUILT).

## 4. From `adb` to `iyakki dev`

| adb | iyakki dev | Scope |
|---|---|---|
| `adb devices` | `iyakki dev devices` | none |
| `adb install app.apk` | `iyakki dev install app.iyp` (or a folder; `push --watch` for live reload) | `install` |
| `adb uninstall` | `iyakki dev uninstall ID` | `install` |
| `adb logcat` | `iyakki dev logs APP --follow`, `iyakki dev dmesg --follow`, `iyakki dev logs --system` | `install` / `debug` |
| `adb shell getprop`, `dumpsys`, `ps` | `iyakki dev info`, `status`, `ps` | `debug` |
| `adb shell` | `iyakki dev shell` (asks on the device's screen each time) | `shell` + approval |
| `adb push` / `adb pull` | `iyakki dev files push` / `pull` | `files` |
| `adb forward tcp:8080 tcp:13305` | `iyakki dev forward 8080:13305` | `forward` |
| `adb shell input tap\|text\|keyevent` | `iyakki dev input tap\|text\|key` | `input` |
| `adb exec-out screencap`, `screenrecord` | `iyakki dev screenshot`, `--record SECONDS` (PNG sequence) | `install` |
| `adb shell settings put` | `iyakki dev config set KEY VALUE`, `export`/`import` | `config` |
| `adb shell pm clear`, `run-as` | `iyakki dev app reset`, `app config APP set` | `config` |
| `adb reboot`, `adb shell reboot -p` | `iyakki dev reboot`, `poweroff` | `power` |
| `adb sideload ota.zip` | `iyakki dev update IMAGE.img` (the previous version stays in the boot menu) | `update` |

## 5. What is installed, and how it is labelled

There is no second installer. Every package goes through the STORE's own review and install (`Store::review` then `Store::install`)
with the source `developer:<computer>`:

* The hash and any signature are checked, the permissions are worked out from the payload, the tile app's lint and the version rules
  apply, and the account's role decides: installing for yourself needs the STORE and `skills`, for everyone `manage_skills`, a tool (MCP)
  server an administrator and is always for everyone. Unsigned packages are accepted from a paired computer (the pairing is the
  administrator's consent), but they are never shown as anything other than Developer.
* The trust level is **Developer (local)**, whatever key signed it: an amber shield in the STORE, "Developer (local)" and a DEV ribbon on
  the launcher tile, "DEVELOPER (LOCAL)" in the app's bar and info panel. It is **never promoted**: the STORE's update check says "Installed
  from a paired developer computer: it is never updated from a store", and a store version of the same app is refused until the developer
  build is removed (the publisher rule: a different signer).
* **Refused:** driver packs and kernel modules (`extension` with `ext_kind: driver`; use the STORE's Drivers page, where the driver-pack
  rules apply), command-line tools (the CLI tile installs those) and `app` packages (not supported yet). A revoked package or key is
  refused as everywhere.

## 6. Scopes: what a paired computer may do, decided per computer on the device

Pairing proves who the computer is. **What it may do is a separate decision**, made by an administrator on the device itself
(Settings → Developer mode: a list of switches for every paired computer) and checked on every single request. A new computer holds
only `install`. The pairing card shows what the computer that pairs will get, and the administrator can change that before it pairs
(`g`). A request can never widen its own scopes: there is no route for it, and a scope that depends on another (`files:system` on
`files`, `input:password` on `input`) is refused without it. Taking a scope away, revoking the computer, `k` (Disconnect) or switching
developer mode off ends that computer's open sessions at once.

| Scope | Allows | Account | Notes |
|---|---|---|---|
| `install` | install, upgrade, remove, list, app logs, open an app, screenshot | any role the STORE allows | the default |
| `debug` | `info`, `ps`, `dmesg`, `logs --system`, `status` | `dmesg` and the system logs: administrators; `ps`: all processes for administrators, else the account's own | read only; credentials in log lines are hidden |
| `files` | `files ls/stat/push/pull/rm/mkdir` in the account's home, its workspaces and `/tmp/iyakki-dev` | any | links are resolved before the check and never written through; sizes and chunks are bounded |
| `files:system` | also READ `/etc`, `/lib/modules`, `/var/log` and the data folder | administrators | never write; the vault, keys, password files, the paired-computer list, TLS keys and wifi passwords are never readable |
| `config` | `config ...`, `app ...` | machine-wide settings: administrators | a whitelist with types and ranges; provider keys are write-only |
| `input` | `input tap/key/text/swipe` | any | into the shell's event queue, not below it |
| `input:password` | the same while a password field has focus | any | off by default even with `input` |
| `shell` | `shell`, and everything `power` allows | administrators | **also needs a grant** (below) |
| `power` | `reboot`, `poweroff` | administrators | needs an approval on the device's screen each time |
| `update` | `update` (the boot files) | administrators | section 7 |
| `forward` | `forward` to ports of the device's own loopback | administrators | never to another machine, never to the developer-mode port |

**The shell grant.** Holding `shell` only lets a computer *ask*. `iyakki dev shell` files a request; the device shows it
(Settings → Developer mode, and `SHELL REQUEST from <computer>` in the top bar); the administrator approves it for 10 minutes (`a`) or
for one session (`o`), or refuses it (`d`). A request is dropped after two minutes. While any grant is in force the top bar shows
**`SHELL ACCESS by <computer>`** in red. Commands run as the account's own system user with a clean environment; they run as root only
when the account is `root` **and** the grant says "as root" (`--root`); a process that is not root itself cannot change user and runs as
itself. Every command line goes to the activity log and the audit log (a credential in it is hidden) and its exit status is logged. The
grant ends by itself when its time is up, and a command still running is killed with its whole process group; the same when the
administrator presses `k`, the computer is revoked or developer mode switches off. An idle session closes after ten minutes. There is
no terminal (no pty): the CLI tile has none either, so the shell is a line mode; programs that need a terminal do not work in it.

## 7. System update: replace the boot files, keep the way back

`iyakki dev update-pack IMAGE.img -o payload` (on the computer) reads the install image's boot partition with the installer's own FAT
reader (long names understood) and writes `payload/boot/...`, `payload/EFI/...` and `update.json` (version, variant, Secure Boot state,
SHA-256 of every file). The image's `boot/iyakki-release` says which IYAKKI it is (the image build now writes it; for an older image give
`--version` and `--variant`). `iyakki dev update payload` then does this, and the device decides every step (the computer's opinion of
what is safe counts for nothing):

1. **Checks**, before a byte moves. Refused: another variant than the running one (macweb vs pc) unless `--force`; a version older than
   what is on the boot partition unless `--force`; a change of the Secure Boot signing state (**never forced**: the boot chain cannot be
   swapped in place, reinstall); a RAM-only data folder (the previous version could not be kept); a data partition without room for the
   copy; paths outside `boot/` and `EFI/`, duplicates, wrong checksums; and a boot partition that cannot hold the **growth** of the new
   files over the old ones (`new total - old total <= free`, in whole clusters; the message gives the numbers and says the PC image's
   512 MB boot volume has room that a 256 MB one does not). Only the files that differ are sent.
2. **Staging** on the data partition (`boot-next/`), chunked, resumable, every file checked against its SHA-256.
3. **The way back first.** The kernel and initramfs now on the boot partition are copied to the data partition
   (`boot-prev/<version>/`), read back and compared. Then the boot menu (the current `grub.cfg`) gets the entry **IYAKKI (previous
   version)**: the normal entry's command line, loading `($iyakki_dp)/boot-prev/<version>/bzImage` and `initrd` after
   `search --label IYAKKI-DATA`. It does not depend on any file of the boot partition. (GRUB's `ext2`, `search_label`, `part_gpt` and
   `part_msdos` modules are in both builds; `boot_menu.rs` checks it.)
4. **A marker.** `boot/UPDATING` on the boot partition and `boot-next/UPDATING.json` on the data partition. If the update is cut, the
   shell reads the second one at start and says so in the top bar (`UPDATE INTERRUPTED: choose "previous version" at boot`), in red.
5. **The replacement, in the safest order.** The initramfs first, then the kernel, then the rest, `grub.cfg` last. A file that fits
   beside the old one is written next to it and renamed over it (atomic); the initramfs, which does not, is overwritten in place
   (the 256 MB boot volume has no room for two 180 MB copies). Ordinary buffered writes, one fsync per file, never a synchronous mount
   (that ran at 47 KB/s on a USB 3 drive and took 15 minutes). Every file is read back and compared.
6. **Done**: the marker goes, the staged files are deleted, only the newest two saved versions are kept. `--reboot` restarts.

The data partition's contents are untouched: only `boot-next/` and `boot-prev/` are written there. The boot partition is reached by
finding the FAT32 volume labelled IYAKKI-BOOT **on the same disk as the data volume** and mounting it (or remounting it read-write if it
is mounted read-only), then putting the mount back. The installer's `fatwrite.rs` only ADDS files to a volume nobody has mounted and
never changes an existing one, so it cannot replace a file and was not used for this.

### 7b. Hot patch: fix a few files of the running system

`iyakki dev patch build DIR -o PACK`, `apply PACK [--live|--next-boot] [--shell]`, `list`, `remove ID`, `show PACK`: add, replace or delete
files (mode, owner and SHA-256 checked) and run a short fixed list of steps (restart a helper, remount, drop caches). Scope `update`, an
approval card on the device the first time per computer, audit lines, undo copies in `iyakki/patches/<id>/undo/`. A patch is live at once
(bind mounts; a symlink is swapped for the file) and is put under the root again at every start by the first-stage init; three starts that
never come up switch all patches off by themselves. The PID 1 binary, kernel modules and firmware only take effect at the next start.
Full design, path rules and what is not verified yet: [development/REMOTE-PATCH.md](development/REMOTE-PATCH.md).

## 8. Security model

| Question | Answer |
|---|---|
| Who can turn it on? | Administrators only (`manage_settings`), in Settings or through `devmode::set_enabled`. Off by default; optional automatic switch-off (default 4 h). Every request checks that it is on. |
| What listens? | One TLS listener (port 7979 by default), only while developer mode is on. The certificate is the web front end's (`<data>/tls`, self-signed, made once). No plain HTTP, no CORS, at most 16 connections, a body limit of 12 MiB + 64 KiB, two-minute request limit. |
| How does a computer get in? | Pairing: the administrator opens a code on the device; the computer sends its ed25519 public key and `HMAC-SHA256(argon2id(code, salt = the certificate fingerprint), "IYAKKI-DEV-PAIR v1" + fingerprint + key + name)`. The fingerprint is the one the computer saw in the TLS handshake, so a machine in the middle (which has to present its own certificate) produces a proof the device refuses; argon2id makes guessing the code from a captured proof slow; five wrong tries or five minutes close the code. The device answers with a second HMAC so the computer knows it reached the device that shows the code. The computer pins the certificate from then on. Both sides print the fingerprint so a person can compare. |
| How is each request authenticated? | An ed25519 signature (from `ed25519-dalek`, already used by the STORE) over `"IYAKKI-DEV-REQ v1"`, the method, path, sorted query, time, a random nonce, the SHA-256 of the body and the device's certificate fingerprint. The device checks the key is paired and not revoked, the time is within two minutes, the nonce is new (replays are refused) and the signature is valid (`verify_strict`). Refusals are logged and rate-limited per address. |
| What can a paired computer do? | Only what the scopes of section 6 give it, with the role of its account: by default install / upgrade / remove what developer mode installed, list, read the logs of the account's own apps, open its tile apps, take a screenshot. Every other power is a scope an administrator switched on for that computer on the device; the most dangerous (`shell`) also needs an approval on the screen each time. It can never reach the vault, a key, a password file, the paired-computer list, install drivers, or remove store installs. Deleting the account, revoking the key or switching developer mode off ends its access and its sessions at once. |
| What is recorded? | `<data>/devmode/activity.log` (time, computer, account, action, detail, allowed or refused; capped, rotated once), shown in Settings, plus the system audit log (`devmode.pair`, `devmode.install`, `devmode.uninstall`, `devmode.open`, `devmode.screenshot`, `devmode.on`, `devmode.off`, `devmode.revoke`, and for the scopes `devmode.scope.grant/revoke`, `devmode.files.put/rm/mkdir`, `devmode.input`, `devmode.config.set/import/provider`, `devmode.shell.exec` with the command line, `devmode.power.reboot`, `devmode.update.plan/apply`). No code, key or secret is ever written to either: typed text is logged by length, provider keys and file contents never, credentials in a command line or a log line are replaced. Requests are bounded: bodies 12 MiB, file chunks 8 MiB and files 2 GiB, commands 8 KiB, shell output 4 MiB kept, 16 sessions per computer, 4 commands per shell session, 16 forwarded connections per forward, an input queue of 256 events. |
| Cryptography | Nothing new: TLS from rustls (the web front end's), ed25519, SHA-256, HMAC-SHA256 (RFC 2104, checked against the RFC 4231 vectors) and argon2id, all crates already in the tree. |

Files on the device: `<data>/devmode/state.json` (on/off, by whom, until, port), `keys.json` (paired computers: name, public key, account,
when, by whom, last use; mode 0600), `pairing.json` (the open code, deleted when used, cancelled or expired), `activity.log`,
`logs/<app>.log` (256 KiB, rotated once).

## 9. Limits (alpha) and what was not tried

* **Discovery:** there is no LAN discovery for developer mode yet; type the device's address (shown on the Settings page). NET SCAN finds
  devices on the network if you need the address. Over ONE cable between two IYAKKI computers the peer is found by a beacon (section 10).
* **The desktop window** (`iyakki-desktop`) does not start the listener; developer mode works on the booted image, `iyakki shell`,
  `iyakki web` and `iyakki serve`. Only tested over loopback in the test suite: no real network, device or second machine was used.
* **Logs** of tile apps are the page's console as the device's shell sees it while the app is open; MCP servers and skills have only
  their install and removal lines.
* The pairing code is shown on the device's screen only; the IDE cannot pair to another device by itself yet (use the terminal command above).
* **Not tried on a device** (the owner tests these on the MacBookPro14,1 and the Mac mini): the real mount of the IYAKKI-BOOT partition and
  the remount (`bootmount.rs`: only the search for the partition is tested, against a made-up `/proc` and `/sys`); GRUB reading
  `boot-prev` on the ext4 data partition (the modules are in the build; the data volume's ext4 features were not checked against GRUB's
  driver); the update on a real 256 MB FAT32 volume and the speed of the in-place initramfs write; changing the user with `setuid` (the
  tests run as an ordinary user); the sound volume, time zone and display size taking effect in the running shell after `config set`
  (the settings are saved and the shell reads them, but nobody watched it change); `reboot`/`poweroff` reaching the real host;
  a second display (`--display N`); the pairing and every scope against a second machine on a real network.
* **Not done:** a terminal (pty) for `dev shell`; `app grant/revoke` (an app holds exactly what its manifest declares); shared folders,
  display layout and app permissions in `config` (the whitelist has what is listed by `config list`); the desktop window does not run
  the listener, so none of this works there; forwarded connections and shell output travel as separate HTTPS requests (fine on a LAN,
  slow for bulk transfers: use `files push` or `forward` with patience).
* The boot partition's `UPDATING` marker is mirrored on the data partition because the boot partition is not mounted at start; the
  shell only reads the mirror.

## 10. Connect two computers with one cable

The owner's wish: sit at a dev PC running IYAKKI, join it to a second IYAKKI PC with a single USB-C cable and check, pair, deploy and
debug over that cable, with no Wi-Fi and no Ethernet. This section says what hardware can do that and what IYAKKI does about it.
Everything below is written from the Linux kernel's documentation and the code in `crates/iyakki-agent/src/directlink/`; **it has been
tested with made-up machines only, never with two real computers and a real cable.**

### 10.1 The hardware facts

Two ordinary PCs **cannot** talk over a plain USB-C or USB 3 cable. USB needs one host and one device, and both PCs' ports are hosts: a PC's
USB controller has no device (gadget) mode, so there is nothing on the other end to talk to. A USB-C cable is only a cable; the port
decides. What does work:

| Cable and ports | Works? | What happens |
|---|---|---|
| **Thunderbolt 3, Thunderbolt 4 or USB4 cable** between two Thunderbolt 3/4 or USB4 ports | **Yes**, 10 to 20 Gbit/s | The kernel's `thunderbolt` driver sees the other computer (host to host) and `thunderbolt-net` makes a network interface `thunderbolt0` (`CONFIG_USB4`, `CONFIG_USB4_NET`). Linux to Linux is what IYAKKI uses. Intel Macs from 2016 on have Thunderbolt 3 (MacBookPro13,x to 16,x, the 2018 Mac mini). The cable must be a real Thunderbolt/USB4 cable (lightning symbol or "40 Gbps"); a USB 3 data cable in a Thunderbolt port makes it an ordinary USB port. |
| Charge-only USB-C cable | **No** | No data wires at all. |
| Plain USB-C, USB 3 or USB-A to USB-A cable between two PCs | **No** | Both ends are hosts. (A USB-A to USB-A cable can damage ports: do not try.) |
| **Host-to-host USB bridge cable** ("USB network link", "Easy Transfer" cable) with a chip Linux has a `usbnet` driver for: Prolific PL-2301/2302/25A1/27A1 (`plusb`), NetChip 1080 (`net1080`), the simple CDC links (`cdc_subset`) | **Yes**, at the chip's speed (USB 2 class for most; not measured) | The cable has a small chip in the middle that is a device to each PC; an interface `usb0` or `enx...` appears on both. Kernel options `CONFIG_USB_NET_PLUSB`, `CONFIG_USB_NET_NET1080`, `CONFIG_USB_NET_CDC_SUBSET`. Cables whose chip needs a Windows-only driver do not work. |
| A cable with a USB Ethernet chip at each end that shows up as an ordinary Ethernet adapter (ASIX AX88xxx, Realtek RTL8152, ...) | Maybe | Handled like a direct link only when no DHCP server answered for 20 seconds. |
| **USB gadget mode**: a computer with a USB device controller (ARM boards, phones, some mini PCs with an OTG port) as the device, the other computer as host, any USB-C or USB-A data cable | **Yes** where the board has the controller | The device side runs `iyakki dev usb gadget on` (configfs, CDC NCM); the other computer sees a USB network card. Needs a kernel with `CONFIG_USB_GADGET`, `CONFIG_USB_CONFIGFS_NCM` and the controller's own driver (board specific). No x86 PC and no Mac has such a controller. |

So the practical route between two PCs or Macs is **Thunderbolt/USB4 networking**. Check which ports you have before buying a cable.

### 10.2 What IYAKKI does when the cable is in

* **Modules.** `thunderbolt` and `thunderbolt_net` are loaded at boot (also found by their aliases; a `tbsvc:` device that appears when the
  other computer connects is loaded by the hotplug listener). The kernel options are in `linux.fragment`; a kernel rebuild is needed for
  images made before them (section 10.5).
* **The interface comes up by itself, without DHCP.** The link manager (started with the shell, as the first process only) watches
  `/sys/class/net`. For `thunderbolt0`, a bridge-cable interface (`plusb`, `net1080`, `cdc_subset`) and a gadget `usb0` it switches the link up,
  turns forwarding off on it (nothing is routed or translated between the cable and any other network), and takes an **IPv4 link-local
  address** (169.254.x.y, RFC 3927): a candidate worked out from the interface's MAC address, three ARP probes, another candidate when
  somebody answers, an announcement. The kernel adds the **IPv6 link-local address** (fe80::) itself. The boot-time network setup no longer
  starts a DHCP client on these interfaces. An ambiguous USB network adapter (cdc_ether, r8152, ...) gets DHCP first and the link-local address only
  after 20 seconds without one; when a real address arrives later it is let go.
* **Optional fixed private pair.** Off by default (`f` on the page, or `iyakki dev usb link --fixed-pair on`): the two ends also take
  10.77.0.1 and 10.77.0.2 (/30), the lower MAC address getting .1, worked out by each end alone from the MAC address in the other's beacon.
  It is optional because a LAN that already uses 10.77.0.0/24 would be shadowed by the cable's route.
* **The beacon.** Every 3 seconds each end sends one UDP broadcast on the cable only (port 7980): `IYAKKI-DEV 1`, host name, version, role,
  whether developer mode is on, whether a pairing code is open (never the code), the link's MAC address and a random id. No secret is in it.
  The parser refuses anything over 512 bytes, non-text, unknown magic, more than 14 lines or a bad field. A peer is forgotten 15 seconds after its last beacon.
* **Firewall.** Direct links are a zone of their own (`docs/FIREWALL.md`, "Direct links"): only developer mode's TCP port 7979, the beacon's UDP port and
  ICMP are let in, and everything else on the cable is dropped, including what the presets would allow for "the local network". The administrator switch
  **Allow more** (page key `m`) lets everything in on the cable.
* **Pairing is unchanged.** The page's Pair button and `iyakki dev usb pair` call the same pairing as `iyakki dev pair`: the code shown on the other computer's
  screen, the certificate fingerprint, the proofs both ways. Without someone at the other screen nobody can pair, cable or not.

### 10.3 The check list (Settings -> Developer mode -> Connect over USB-C, key `u`; or `iyakki dev usb`)

| Step | Green when | When it is red |
|---|---|---|
| 1 Cable and port can carry a network | a Thunderbolt/USB4 port, a bridge cable or a device controller is found | says why a plain USB-C cable cannot work and what to use; "driver not loaded" gives `modprobe thunderbolt` |
| 2 The other computer is connected | a host-to-host connection (or a link on a bridge cable) | what to plug where; if a Thunderbolt device waits for approval, the **Authorize** button |
| 3 Network interface is up with an address | `thunderbolt0` (or `usb0`, `enx...`) has an address and a link | missing interface: load `thunderbolt_net`; no address yet: wait a few seconds |
| 4 The other IYAKKI is found | a beacon was seen | the other computer must run IYAKKI with its link up |
| 5 Developer mode answers | TLS hello on port 7979 | developer mode off there (Settings -> Developer mode, `e`), or an older firewall |
| 6 Paired | this computer has the other as a paired device | on the other computer `p` shows a code; here **Pair**, type it, Enter |
| 7 Speed and delay | after **Speed**: 64 MiB sent through the developer-mode link | needs step 6 |

The page shows **Last result** (for example "5 of 7 steps passed. Stuck at: This computer is paired with it"), the link (name, address, speed, other
computer) and **Copy diagnostics** (interfaces, Thunderbolt tree, steps; no keys, no codes). Thunderbolt **security levels** (the `security` file of the
domain): `none` connects at once; `user` needs one approval per new device; `secure` needs approval once, then the device proves a key (IYAKKI makes the
key and keeps it in `<data>/directlink/tb-keys`, mode 0600); `dponly`, `usbonly` and `nopcie` never tunnel PCIe, so there is nothing to approve. Per the kernel
documentation a host-to-host connection (`thunderbolt-net`) needs no approval at all; the button is for docks and drives.

The same from a terminal (the dev PC, as root for anything that changes the machine):

```text
iyakki dev usb                          the seven steps and Last result
iyakki dev usb authorize                approve the waiting Thunderbolt device
iyakki dev usb pair [--code CODE]       pair with the other computer (its screen shows the code and the fingerprint)
iyakki dev usb speed                    64 MiB through the developer-mode link: Gbit/s and delay
iyakki dev usb link [--fixed-pair on|off] [--allow-more on|off]
iyakki dev usb gadget on|off            act as a USB network card (needs a USB device controller)
```

After step 6 every command of section 3 works over the cable with `--device NAME` (the paired device's address is the other computer's link-local address).

### 10.4 The speed check

`POST /v1/dev/speed` (any paired computer, no scope) counts the bytes of its body (at most 8 MiB per request) and throws them away. The check sends 16
requests of 4 MiB after a warm-up, so the figure includes TLS and request signing and is lower than the raw link speed; the delay is the best of five TCP
connects to port 7979.

### 10.5 Kernel options (a kernel rebuild is needed) and honest limits

Added to `dist/buildroot/board/iyakki/linux.fragment` (every x86-64 image, PC and Mac): `CONFIG_USB4=m`, `CONFIG_USB4_NET=m`, `CONFIG_USB_NET_PLUSB=m`,
`CONFIG_USB_NET_NET1080=m`, `CONFIG_USB_NET_CDC_SUBSET=m` (`CONFIG_USB_NET_AX88179_178A`, `CONFIG_USB_NET_AX8817X`, `CONFIG_USB_RTL8152` were already there;
`CONFIG_USB4_KUNIT_TEST` and `CONFIG_USB4_DEBUGFS_WRITE` stay off; `CONFIG_HOTPLUG_PCI_PCIE` was already on). Added to `board/iyakki-aarch64/linux.fragment`:
the same usbnet drivers and `USB4`/`USB4_NET`, plus `CONFIG_CONFIGFS_FS`, `CONFIG_USB_GADGET`, `CONFIG_USB_CONFIGFS` (module), `CONFIG_USB_CONFIGFS_NCM`,
`CONFIG_USB_CONFIGFS_ECM`; the controller driver of a board (dwc2, dwc3, musb, ...) is not set. The fragments were resolved with kconfig on a private copy of 6.12.112
(`verify-kernel-config.sh`): every new option applies. No image was built with them.

* **Never tried:** two real computers, a real Thunderbolt/USB4 cable, a bridge cable, a gadget board. The raw ARP socket, the interface ioctls, the UDP
  beacon socket and the configfs writes (`directlink/real.rs`, `gadget.rs`) have never run on a real machine; the engine is tested against fakes.
* **Macs:** see `docs/MAC-HARDWARE.md`, "Thunderbolt networking between two Macs". Whether Apple's firmware leaves the controller usable for host-to-host links on
  each model is not known.
* **IPv4 only:** the beacon is an IPv4 broadcast. The IPv6 link-local address exists and works for ping and developer mode once the address is known,
  but nothing announces it.
* Forwarding is switched off per interface (input side): the kernel does not forward packets that arrive on the cable. Nothing NATs.
* Linux to Linux only: macOS and Windows have their own Thunderbolt networking; IYAKKI's page does not manage them.
