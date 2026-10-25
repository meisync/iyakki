# Linux apps: a Debian or Ubuntu sandbox

Status: phase 1 (terminal and command-line programs) is built and tested with fakes; one real sandbox run (a static BusyBox in
a namespaced root) was tried on a development PC. **Not yet tried on an IYAKKI device**, and `apt` inside a downloaded Debian or
Ubuntu has not been run for real (the build machine had no network to the distribution). Phase 2 (graphical programs in a window) is built in its minimal form and tested with fakes (section 11, [`LINUX-WINDOWS.md`](LINUX-WINDOWS.md)). Owner decisions of 2026-10-21 are in [`APP-MODE-AND-APPLE-SILICON.md`](APP-MODE-AND-APPLE-SILICON.md): Debian/Ubuntu
sandbox first, Wine later and on top of it, all app-store plans dropped (instead: image apps and building your own).

## 1. What it is, in plain words

IYAKKI itself is small (musl and BusyBox), so most Linux programs from the big distributions do not run on it directly. A **Linux
app sandbox** is a complete Debian or Ubuntu inside a folder of the data volume. You press a button, IYAKKI downloads the
distribution's own base system, checks it, unpacks it, and from then on you can open a terminal inside it, install packages with
`apt`, and run what you installed. The sandbox sees only its own files and the folders **you** choose to mount in. It cannot see the vault, other
people's files or IYAKKI's own programs. Nothing of Debian or Ubuntu is shipped in an IYAKKI image.

Where to find it:

* IYAKKI menu (click the name in the top bar, or F10) → **Linux apps** (key `l`): the page described below.
* Store → **Setup** → **Linux apps** (the IYAKKI menu's *Quick install* opens it): one click makes a first sandbox called `debian` (Debian 12, network on, no folders).
* The **LINUX WINDOW** tile (System section): a graphical program of a sandbox in an IYAKKI window (section 11).
* The **CLI** tile: the `linux` command, and `linux use <name>` for a terminal inside a sandbox (section 5).
* The **AI IDE**: a workspace can run its run/test commands and its terminal in a sandbox (section 6).
* **Image apps** (ready-made container images) are the third tab of the page (section 8).

All of it needs the **full command-line permission** for your account (administrators and operators have it), because running a
program of your choosing is what a shell is.

## 2. The page

Three views (keys `1`, `2`, `3`, or click the buttons):

* **Sandboxes**: your sandboxes (name, release, network state, folders, size). With one chosen:
  **Terminal** (opens the CLI tile inside it), **Install** and **Remove** (the packages typed in the first field; separated by spaces),
  **Upgrade**, the **network switch**, **Mount read-only** / **Mount read-write** / **Unmount** (the folder typed in the second field),
  **Delete** (asks for the sandbox's name to be typed; nothing is deleted otherwise).
* **New**: choose Debian 12, Debian 13, Ubuntu 22.04 LTS or Ubuntu 24.04 LTS, type a name (lower-case letters, digits, `-`), optionally a
  folder to mount, decide the network, press **Create sandbox**. Creating is a detached job: progress shows beside the clock in the
  top bar and the log is in the installer runs list. The licence and trademark notice is shown on the page and in the job's log.
* **Image apps**: the ready-made container-image apps (section 8).

The top line says how the sandboxes run on this kernel, or, if the kernel lacks something, exactly which options (section 10).

## 3. Where the base comes from, and what is checked

Downloaded for you at your click, never shipped, into `<data>/linuxapps/<you>/<name>/rootfs`.

| Release | Source | What is verified |
|---|---|---|
| Ubuntu 22.04 / 24.04 LTS | `cdimage.ubuntu.com/ubuntu-base/releases/<version>/release/`: the `ubuntu-base` root tarball made for `chroot` (about 30 MB, no cloud-init or snap). Its file name carries the point release, so it is read from that folder's `SHA256SUMS`. | The download must hash (SHA-256) to exactly the value in `SHA256SUMS`, which is fetched over HTTPS (certificate-checked) from the same host. |
| Debian 12 / 13 | The official `debian:<suite>-slim` image of Docker Hub's library (the Debian project's own build), pulled with the plain registry protocol by IYAKKI's own code (no Docker needed). | The tag is resolved to a manifest; the manifest's SHA-256 is computed here (and compared with a pin when the release table has one); every layer must hash to the digest the manifest names. A layer that differs is discarded. |

**What is not verified:** the distribution's GPG signatures (`SHA256SUMS.gpg`, image signatures). That would need a GPG verifier and the
archive keys, which IYAKKI does not carry. HTTPS to the distribution's host is the trust anchor, plus the hashes above. `apt` inside
the sandbox **does** verify the signed package lists with the distribution's own keys (they are in the base).
The release table has a `pin` field for a manifest digest; it is empty until the owner decides which Debian build to pin (a pinned
release refuses a tag that moved: tested). The page and the audit log record the digest the tag resolved to.

Licences: Debian and Ubuntu are free software; every package keeps its own licence (`/usr/share/doc/<package>/copyright` inside the
sandbox). "Ubuntu" and its logo are trademarks of Canonical Ltd; "Debian" is a registered trademark of Software in the Public Interest,
Inc. IYAKKI does not rebrand them and is not endorsed by either. Space needed: about 120 MB unpacked per base plus your packages; the
free space of the data volume is checked before the transfer (about five times the download) and again before installing packages (512 MB).

## 4. The security model, in plain words

What the sandbox cannot do, and how:

| Rule | How it is done |
|---|---|
| It sees only its own files as `/` | `pivot_root` into the sandbox's folder, then the old root is detached: the host's files are not reachable by any path. Links inside the sandbox can only lead inside it. |
| Only folders you mounted show up | A bind mount per folder, **read-only unless you chose read-write** (the mount is also `nosuid`, `nodev`). The folder must pass your file permissions, must not be the data volume (the vault, other people's files, sockets) or a system folder, and a folder replaced by a link after you mounted it is refused when the sandbox starts. Mount points are under `/mnt`, `/home`, `/srv`, `/opt` (never `/etc`, `/usr`, `/proc`...). |
| No access to IYAKKI's vault, sockets or `/var/lib/iyakki` | Not mounted; the data-folder rule above; nothing of IYAKKI is inside the sandbox. |
| Not root on the host | A new **user namespace**: ids inside map to ids on the host that have no rights there. Running as root on the host, each sandbox gets its own range of 65,536 ids; otherwise it maps just your own id. Inside, the sandbox user is `user` (id 1000, not root). Only `apt` runs as root *inside* (powerless on the host). |
| No new privileges | `no_new_privs`, all capabilities dropped for the sandbox user, the dangerous ones (mount, module loading, raw I/O, ptrace, time...) dropped even for root inside. |
| A system-call filter | **seccomp** deny-list: mount, ptrace, bpf, module loading, kexec, reboot, swap, new namespaces (`unshare`, `setns`), keyrings, `perf_event_open`, clock setting, `userfaultfd`, `process_vm_*`, and the new mount API fail with `EPERM`. A foreign architecture (32-bit programs on a 64-bit kernel) is refused. |
| Own processes, hostname, IPC | New pid, uts and ipc namespaces: it is pid 1 of its own tree and cannot see or signal other processes. `/proc` is the sandbox's own. |
| Minimal `/dev`, no `/sys` | A small memory file system with six harmless nodes bound from the host (`null`, `zero`, `full`, `random`, `urandom`, `tty`) and the sandbox's own `/dev/pts` instance. No `/sys`, no device passthrough (no GPU, no disks, no USB). `/tmp` and `/dev/shm` are memory file systems with a size limit. |
| Network is a switch | **Off**: an empty network namespace with only a loopback. **On**: the host's network, outbound; `/etc/resolv.conf` is copied from the host. See the limit below. |
| Resource limits | cgroup v2 (memory, cpu, process count) when the kernel has the controllers (section 10); a run is also stopped after its time limit. |
| Everything started with argument arrays | `iyakki linuxapp-init <options> -- <command> <args...>` through the `wsexec` runner: no shell string anywhere. Package names are checked (`[a-z0-9][a-z0-9+.-]*`, no trailing `-`), search words too, and `apt-get` gets only fixed options. |
| It is all written down | `<data>/linuxapps/audit.log` (create, mount, unmount, network, run, apt install/remove/upgrade, import, delete, refused deletes) and the signed-in audit log of the shell. A run is logged by program name and argument count, **not** the arguments (they can hold passwords). |

**Honest limits** (read these):

* **Network "on" shares the host's network namespace.** The sandbox can reach what the machine can reach, **including services that
  listen on the machine's loopback** (IYAKKI's web front end, the dev cluster's API). A sandbox with its network on is therefore treated as
  able to reach "any server": only an account allowed every host may turn it on, run in it or make one. Keep the network **off** for
  programs you do not trust and turn it on only to install packages. Planned: an outbound-only network (a virtual interface with
  address translation, or a user-space network stack), which would also allow a host allow-list.
* When IYAKKI runs as root (the normal case on a device) each sandbox gets its own range of 65,536 ids and `apt` can change owners inside. A copy of IYAKKI
  that does **not** run as root maps one id only: the sandbox works, but a package that must `chown` to another user or group fails to unpack there.
* Memory, cpu and process limits need root and cgroup v2 (a non-root run, such as a development PC, has no limits; the time limit of each run still applies).
* The sandbox is as strong as the Linux kernel's namespaces and seccomp. A kernel bug can break it; it is not a virtual machine.
* The base image comes from the distribution's server and is trusted as such (section 3).
* The seccomp filter is a deny-list (programs keep working); an allow-list profile per program is not built.
* `/dev/pts` (added for Linux windows, section 11): the sandbox has its own pseudo-terminal instance (`devpts` with `newinstance`, shared with nothing of the host), so `xterm` and programs that open a pseudo-terminal can run. Best effort: a kernel that refuses the mount leaves the sandbox without it. The `linux use` terminal and `linux run` still run one command to the end without a terminal; `sudo` is not useful (nobody is root on the host).

## 5. Terminal and the command line

CLI tile → `linux`:

```
linux list                              your sandboxes
linux releases                          what can be created
linux new <name> <release> [net]        make a sandbox (a job); net = network on
linux use <name>                        a terminal inside it: each line you type runs there; `exit` leaves
linux run <name> <command...>           one command, no shell
linux apt install|remove <name> <package...>     (jobs)    linux apt update|upgrade <name>
linux apt search <name> <word...>       linux apt list <name>
linux net <name> on|off                 linux mount <name> <folder> [mount-point] [rw]    linux unmount <name> <mount-point>
linux import <name> <tarball>           linux delete <name> <name again>      linux log
```

There is **no shell**: a line is split into words (quotes group, nothing is expanded, `;`, `|`, `&&` and `$HOME` are ordinary
words) and the words are the program's arguments. For a pipe or a loop, say so: `bash -c 'ls | wc -l'` (inside the sandbox, on purpose
and visible). There is no pseudo-terminal, so each line is one command run to the end (120 seconds at most; use a job for long work).

## 6. The AI IDE

In a workspace's terminal: `sandbox` shows where commands run; `sandbox dev` makes this workspace's **run, test and terminal
commands** run in the sandbox `dev`; `sandbox off` goes back to the machine. The choice is the `"sandbox"` field of
`.iyakki/project.json`. Only the workspace folder is mounted, read-write, at `/mnt/workspace`; the command runs in the same
sub-folder inside; the rest of the host is not visible; the sandbox's own record is not changed. If the sandbox was deleted, the command
stops with an explanation (it never silently runs on the machine instead).

## 7. Packages (`apt`)

**Install** and **Remove** are detached jobs (progress beside the clock, live log; **Stop** in the installer list stops one; a sandbox has no background service of its own, so "stop a sandbox" means stopping its jobs): `apt-get update`, then `apt-get install -y
--no-install-recommends <packages>` with `DEBIAN_FRONTEND=noninteractive`, as root inside, with the network on. Services are never started
inside the sandbox (`policy-rc.d`). **Upgrade** runs `update` and `upgrade -y`. `linux apt search`/`list` search the package lists and show what
is installed. **Storage** is shown per sandbox. **Delete** removes everything (typed name).

Using a program: `linux use dev`, then for example `python3 -c 'print(1)'`. Mount a project folder read-write at `/mnt/<name>` to work on
your files with the sandbox's tools; mount read-only to let it read without changing anything.

## 8. Image apps

A ready-made container-image app: a small JSON file with the name, description, **the image pinned by digest**, ports, environment, volumes
and licence. The files are in `image-apps/*.json` of the repository (three **examples**, marked as such) and `<data>/image-apps/*.json`
(your own). **Run** writes a Deployment and a Service for the namespace `iyakki-apps` and applies them with the dev cluster's managed
`kubectl` (the cluster pulls the image; nothing else touches the cluster). **Stop** removes them. Pods get no service-account token, no
privileges, all capabilities dropped, the runtime's default seccomp profile and resource limits; the Service is a NodePort, the page shows
`open http://<this machine>:<port>` when it runs.

* A reference without `@sha256:<64 hex>` is refused when the file is read (a tag can move).
* The three examples carry an **all-zero placeholder digest** because the digests could not be looked up offline when they were written. They are
  listed, but **cannot run** until `dist/release/pin-image-apps.sh` has filled in the real ones (a release step: it needs the network).
* Needs the dev cluster (Store → Setup → k3s). With the Podman backend the manifest file is not visible inside the container, so Run needs
  the native backend.
* Examples: BusyBox httpd (GPL-2.0), Python's `http.server` (PSF-2.0), `traefik/whoami` (Apache-2.0).
* `kind: "rootfs"` entries (a plain root tarball run in the sandbox runtime) parse and show but cannot run from the catalog yet; use
  **import** (below) instead.

Building your own: [`BUILD-YOUR-OWN-IMAGE-APP.md`](BUILD-YOUR-OWN-IMAGE-APP.md).

## 9. Files, jobs and commands (for developers)

| What | Where |
|---|---|
| Strict root-file-system reader (links, hard links, devices, set-id bits, size caps, layer whiteouts) | `crates/iyakki-agent/src/rootfs.rs` |
| Downloads and verification | `linuxapps_dist.rs` |
| Manager: records, mounts, audit, argument arrays, apt, probe, jobs, AI IDE choice | `linuxapps.rs` |
| The inside of the sandbox (`iyakki linuxapp-init`), seccomp program | `linuxapps_init.rs` (system calls listed in [`BOUNDARY.md`](../BOUNDARY.md)) |
| Image apps | `imageapps.rs`, `image-apps/*.json` |
| CLI `linux` command | `cli_linux.rs` |
| IYAKKI menu page and the shell's part | `crates/iyakki-shell/src/brandmenu_linux.rs`, `shell_linux.rs` |
| Linux windows (phase 2): install, launcher, window start/stop, cpu and memory | `linuxwin.rs`; the tile `crates/iyakki-shell/src/apps/linuxwin.rs` |
| RFB (VNC) client, no dependencies | `crates/iyakki-rfb` |
| Detached job program | `iyakki linux-apps --data D --user U create\|install\|remove\|upgrade\|update\|run\|import ...` |

The "runner" is one trait (`wsexec::Runner`), so tests use a fake that records the exact command. The sandbox command is
`iyakki linuxapp-init --root R --name N --uid U --gid G --idmap single|multi:BASE --net on|off [--bind-ro SRC DST]... [--bind-rw SRC DST]...
--tmp-mb N --cwd DIR --mem-mb N --cpu-pct N --pids N --seccomp on|off [--env K=V]... -- <command> <args...>`.

**Runtime choice.** (a) namespaced `pivot_root` as described (implemented). (b) A pod in the dev cluster from a rootfs imported as an image is the
documented fallback when the kernel lacks namespaces: the pod manifest (no service account, no privilege, capabilities dropped,
default seccomp) and the exec command are built and tested; **converting a rootfs into an image the cluster can import is not
built** (a layer tar, a config and a manifest in the docker-archive format, then `k3s ctr images import`). On the IYAKKI kernels (section 10) the
fallback is not needed.

**Import a tarball you assembled** (the "build your own" route): `linux import <name> <file.tar.gz|.tar.xz|.tar.zst|.tar>` unpacks it with the same
strict reader. Nothing is compared with a vendor list (the page says "none: imported from your file", with the SHA-256 computed).

## 10. Kernel and image needs

The sandbox needs: `CONFIG_NAMESPACES`, `CONFIG_USER_NS`, `CONFIG_PID_NS`, `CONFIG_NET_NS`, `CONFIG_UTS_NS`, `CONFIG_IPC_NS`, `CONFIG_SECCOMP_FILTER`,
`CONFIG_TMPFS`, `CONFIG_DEVTMPFS`, procfs, and the sysctl `user.max_user_namespaces` above 0. For limits: cgroup v2 mounted at `/sys/fs/cgroup`
(IYAKKI's PID 1 does this) with the `memory`, `cpu` and `pids` controllers (`CONFIG_MEMCG`, `CONFIG_CGROUP_SCHED` with `CONFIG_CFS_BANDWIDTH`,
`CONFIG_CGROUP_PIDS`).

**Checked in the kernel configurations of the libre x86-64, libre aarch64 and web builds (`target/buildroot-*/build/linux-6.12.112/.config`):
every one of these is `=y`. No kernel rebuild is needed for phase 1.** (The fragment `linux.fragment` sets `USER_NS`, `MEMCG` and `CFS_BANDWIDTH`
itself; the others come from the defaults and the k3s options.) BusyBox in the image has no `unshare` or `nsenter` applet (`CONFIG_UNSHARE`
is off), which is why the sandbox is built from system calls in IYAKKI's own code; `chroot` is not used (`pivot_root` is safer).
The page reads `/proc` and `/sys` when it opens and lists any missing option by name. To verify on a device: open the page (the top line), or
`iyakki` CLI `linux releases` then create a sandbox. Not present: a veth/NAT for outbound-only networking.

## 11. Phase 2: graphical programs (minimal version built)

Built, with the details in [`LINUX-WINDOWS.md`](LINUX-WINDOWS.md); tested with fakes, and tried for real only as far as that file's section 8 says.

1. **Display server inside the sandbox that draws into memory: TigerVNC's `Xvnc`** (an X server with RFB built in; GPL-2.0, a Debian package installed by `apt` in the sandbox at the person's click, never shipped). It was chosen over Xvfb plus a VNC server and over Weston headless with Xwayland because it is **one package** that already speaks RFB on a Unix socket and supports resizing (Extended Desktop Size). The licence is GPL-2.0, not a permissive one, but IYAKKI only runs it, inside the sandbox, as a separate program. `matchbox-window-manager` (GPL-2.0, one maximised window) and Mesa's software OpenGL (MIT) come with it.
2. **Pixels out, input in: RFB over a Unix socket** in a runtime folder IYAKKI makes under the sandbox's own folder and mounts into the sandbox (`/mnt/iyakki-display`), nothing else. IYAKKI's **own RFB client** (`crates/iyakki-rfb`: Raw, CopyRect, Hextile, ZRLE with its own inflate; key, pointer and clipboard-text messages; resize) turns the frames into pixels of the tile.
3. **A tile** (LINUX WINDOW) that shows the frames, takes keyboard, pointer and wheel, resizes the virtual screen, has an explicit-only clipboard bridge (typed text) and a status strip with the sandbox's CPU and memory.
4. **A launcher** that reads `usr/share/applications/*.desktop` of the sandbox (strict reader rules) and starts the program with `DISPLAY` set (by the session script, no shell on the host); one click installs a small test program (xterm, x11-apps or mousepad).
5. **Sandbox changes:** one more bind mount (`LinuxApps::exec_argv_with_run`: the runtime folder, read-write, only a folder of the sandbox itself); `/tmp/.X11-unix` is in the memory file system `/tmp`. No seccomp exception was needed for xterm in the tests that could run (see `LINUX-WINDOWS.md` section 8). Audio (a PulseAudio or PipeWire socket) and GPU (`/dev/dri` render node, opt-in) are **not** built.

**Not built:** audio, GPU, several windows per tile, an Alt key, drag and drop, a clipboard of pictures. Wine (Windows `.exe`) comes after this, on top of it, and is not part of this work. A web browser such as Brave (glibc, Chromium) is a possible later use of this window (it cannot run natively on IYAKKI's musl base); it was considered and **not** added.

## 12. What was tested, and what was not

Tested without a network or a real distribution (fake downloads, temporary folders, a fake runner): path traversal, links, hard links, devices and
size limits in the archive reader; checksum mismatch, redirect and registry handling; golden argument arrays (no shell, only the sandbox folder and the
mounted folders as host paths, read-only flags, network switch); mount rules; package-name validation; deletion needs the name; audit entries; kernel probe; the
IYAKKI menu page at four screen sizes; the CLI command; Quick install entry; image-app digest enforcement and manifests; the AI IDE runner choice; the
`linuxapp-init` argument round trip and the seccomp program.
**Tried for real** (the ignored test `linuxapps_tests::a_real_sandbox_runs_busybox_cannot_see_the_host_and_respects_the_switches`, run with `--ignored
--test-threads=1`) on a development PC as a normal user: a static BusyBox root, user/mount/pid/uts/ipc/net namespaces, `pivot_root`, `/proc`, `/dev`, mounts
read-only and read-write, host files invisible, an empty network (only `lo`), mounting refused.
**Not tried:** a real Debian or Ubuntu download and `apt` inside; the wide id range (running as root); cgroup limits; the seccomp filter against a real
Debian's programs (the test enables it when the kernel offers it); anything on an IYAKKI device or image; Docker Hub and the Ubuntu server's actual answers
(the format is as documented by the registry and Ubuntu; the checks fail closed); the job program run from the menu; image apps in a real cluster.
