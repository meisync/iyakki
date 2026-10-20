# The SSH client in the CLI tile

`ssh`, `scp` and `sftp` are in the CLIs list as **SSH client (OpenSSH)**. They are not downloaded: the entry finds the client that
is already on the machine (`ssh -V`: path, version, which commands exist) and says how to get one when there is none. The
**Connections** tab (third tab, or `c` on the entry) keeps saved hosts. Code: `crates/iyakki-agent/src/ssh.rs` (rules, host keys,
keys, saved hosts), `exec.rs` (running), `cli.rs` (`ssh_call`, the terminal commands), `crates/iyakki-shell/src/apps/cli_conn.rs`
(the tab). Tests: `ssh_tests.rs`, `apps/cli_conn_tests.rs`, `agent.rs` (`ssh_through_run_command…`).

## Which client, and why

| Where | Client | Why |
|---|---|---|
| Linux computer | OpenSSH from the distribution | `sudo apt install openssh-client` (Debian/Ubuntu), `sudo dnf install openssh-clients`, `sudo pacman -S openssh`, `apk add openssh-client`. IYAKKI never runs sudo. |
| macOS | built in (`/usr/bin/ssh`) | nothing to install |
| Windows | the OpenSSH Client optional feature | Settings > Apps > Optional features, or `Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0` |
| **IYAKKI images** (libre, hw, mac, web, x86-64 and ARM64) | **Dropbear's client**: `dbclient`, `ssh` (a link to it), `scp`, `dropbearkey` | already in the image for the diagnostics server, so the client costs **0 KB**: one multi-call program (417,056 bytes on x86-64, 384,096 on ARM64) holds server and client. MIT / BSD-2 licence. OpenSSH (BSD/ISC) would add about 2 MB (ssh, scp, sftp, ssh-keygen; libcrypto is already in the image), which was judged too much for what it adds. `BR2_PACKAGE_DROPBEAR_CLIENT=y` is now written in both defconfigs, `post-build.sh` stops the build if `ssh`, `scp`, `dbclient` or `dropbearkey` is missing, and the boot self-test prints `IYAKKI-SELFTEST ssh Dropbear v…` (checked by `qemu-test.sh`). |

What Dropbear lacks, and the program says so: **no `sftp`** (use `scp`, or `ssh host command`), no passphrase-protected keys (make a
key without a passphrase with *Make key*), no `ssh-keyscan` (the host key is read with the client itself), and fewer `-o` options.
If sftp is wanted on the images, adding `BR2_PACKAGE_OPENSSH=y` (client + key utilities, no server) is the path; the code already
prefers `ssh-keyscan`, `ssh-keygen` and `sftp` when they exist.

## Safety rules (the same for the assistant, the terminal and the tab)

* **Hosts**: the account's network rules decide (`allows_host`). Loopback and private addresses are ordinary hosts: they are
  allowed exactly when the account's list allows them (or has `*`).
* **Approval**: the assistant reaches the client only through `run_command`, which asks the person every time, even in Auto
  (`hard_ask`). The person typing `ssh` in the terminal is the approval.
* **Allow-listed options**. Refused before anything starts: `-o ProxyCommand`, `LocalCommand`, `PermitLocalCommand`,
  `KnownHostsCommand`, `ProxyJump`, `Include`, `Match`, `RemoteCommand`, `IdentityAgent`, `UserKnownHostsFile`, any `-o` not in the list in
  `ssh.rs`; `-F`, `-J`, `-S`, `-s`, `-D`, `-E`, `-L`, `-R`, `-W`, `-w`, `-f`, `-N`, `-y`; `ssh://` and `scp://` links; options placed
  after the host (OpenSSH reads those too) are checked the same way. `-A` and `ForwardAgent=yes` are refused for the assistant; you may
  type them yourself. `StrictHostKeyChecking`, `BatchMode` and `PreferredAuthentications` can only be set to the safe value.
* **No passwords**: `BatchMode=yes` always; stdin is closed; nothing is ever asked. No agent socket is passed on unless you typed `-A`.
* **Host keys** (per person, `<data>/ssh/<user>/home/.ssh/known_hosts`, folder 0700, file 0600): a host that is not in it is refused,
  without starting the client. To trust one: *Trust key (t)* in Connections, or `sshtrust <host> [port]` in the terminal. Both fetch the
  key (`ssh-keyscan`, or the client in a throw-away home with no key to offer), show `SHA256:…` fingerprints, and record a key only after
  you confirm (`y`, or by typing the fingerprint in `sshtrust host SHA256:…`). The assistant can never trust a host. A key that differs
  from the recorded one is a **hard refusal** (`HOST KEY CHANGED …`); the tab and `sshtrust` then show a red "different key" warning,
  and *Forget key (f)* / `sshforget` is the person's step. `sshhosts` lists what is trusted. Dropbear keeps no port in `known_hosts`, so each
  run gets a private home holding only the keys trusted for that host and port.
* **Private keys** live in your vault as `ssh.<name>` (made with *Make key (g)* or stored in Settings > Vault). A command names one as
  `-i uvault:ssh.<name>`; the assistant may use nothing else (no key files in folders). A run writes it to a 0600 file in a 0700 folder
  under the temp folder and removes the folder when the run ends, also on errors. A key with a passphrase needs `ssh.<name>.passphrase` in
  the vault; it reaches `ssh-keygen -p` through an askpass helper, never an argument, and only OpenSSH can open such keys.
  *Make key* uses `ssh-keygen` (or `dropbearkey`, stored as `b64:…`) in a private temp folder; the private half goes straight into the vault.
* **Local files** of `scp` and `sftp -b` must be inside the account's read (sources) or write (destinations) scope; the real path is
  what is passed on. A batch file for the assistant may only hold remote commands and scoped `get`/`put`; `!`, `lcd`, `lls` … are refused.
  Copying between two remote machines is refused.
* **Output** is redacted (private keys, `password:`, tokens) before the model, the log or the screen sees it. The audit log has the
  command name, the connection name and `host:port` for trust and forget; never arguments, keys or output.

## Connections tab

A list of saved hosts (name, host, port, user, key name, optional default command) in `<data>/ssh/<user>/connections.json` (0600;
names only, no secret). Keys: `n` new, `e` edit, `x` remove, `c` or Enter connect, `t` trust host key, `f` forget it, `g` make a key.
The panel shows the client found, the host-key state, whether the key is in the vault, and the public half of a key made here.
**Connect** runs the default command (or `true` as a quick check) without a terminal and streams the output into the Terminal tab;
an unknown host asks for the fingerprint first. Everything on the tab needs a full terminal and a host the account may reach.

Terminal commands (full terminals): `ssh`, `scp`, `sftp`, `sshtrust`, `sshforget`, `sshhosts`. They run on a thread of their own,
up to two minutes, and stream into the log; the screen does not wait. (In a pipe, such as `ssh host cmd | grep x`, the line is run in one piece and the screen waits for it.)

## For the assistant

`list_clis` shows `ssh … AVAILABLE`; `run_command` takes `program` `ssh`, `scp` or `sftp`. The tool text tells the model to pass
`-i uvault:ssh.<name>`; there is no separate skill (the built-in skill list is shared with the store and left alone).
Long work: `run_command` allows up to 300 s; beyond that the person starts a job.

## Not built: an interactive session

The tile has no pseudo-terminal today (the IDE terminal is a command runner too). A real `ssh host` login, `top` or `vi` needs
`openpty`/`forkpty` with `setsid` and `TIOCSCTTY`, `TIOCSWINSZ` for the size and resizing, a VT100/ANSI emulator (cursor movement,
colours, alternate screen, scrolling regions, UTF-8 and wide characters) with scrollback, drawing it in the tile, and raw key
forwarding (arrows, Ctrl, function keys, paste). Estimated 1,800 to 2,500 lines plus tests, mostly the emulator, and BOUNDARY.md rows
for `openpty`, `setsid` and `ioctl`. It is listed as F-CLI-08 in `FEATURES.md` and under class C in `ALPHA-SCOPE.md`.

## Checked and not checked

Checked: the rules, host keys, vault keys, saved hosts and the tab with stand-in programs at 1280x720, 1920x1080, 1024x600 and 800x480;
the real Dropbear client (the one in the image, run with its own musl loader) against a Dropbear server on 127.0.0.1: reading the key,
trusting it, and the refusal when the key changed (`loopback_dropbear_probe_trust_and_changed_key_end_to_end`, ignored by default; set
`IYAKKI_SSH_LOOPBACK=<port>:<folder with the links>`). Not checked: an image was **not** rebuilt or booted for this change (the existing
images already carry the client; the new self-test line appears in the next build), a login with a real key to a real server, OpenSSH
10 on macOS or Windows, passphrase keys with a real `ssh-keygen`.
