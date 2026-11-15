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

## Interactive sessions (F-CLI-08)

On the Connections tab, **L** (or the button **Log in (l)**) opens a real login in the tile: the SSH client runs on a pseudo-terminal and
its screen is drawn in the tile. It needs what Connect needs (a full terminal, a host the account may reach) and a host key that is
already trusted (press T first; the question about a new key is never asked in the middle of a login). The same rules apply as for every
other SSH call (allowed options, host key check, keys from the vault by `-i uvault:ssh.<name>`, written to a private folder that is
removed when the session ends). **Keys only**: `BatchMode=yes` stays on, so the client never asks for a password; a server that needs one
is not reachable this way yet. Every key goes to the other computer (Esc and Tab too); **F10** (or the Close button) hangs up; Ctrl+V
pastes (escape characters removed); the wheel and Shift+PgUp/PgDn scroll back, typing or new output returns to the live screen. When the
program ends the screen stays with its exit code until Enter or F10. The audit log has `ssh.login` and `ssh.logout` with the connection
name only. Linux and macOS (a pseudo-terminal; on other systems the tile says so and Connect still runs a command without a terminal). A login
asks the client for a terminal (`-t`, so a saved default command gets one too) and runs only that default command; it does **not** end in
`true` as Connect's quick check does (`Connection::login_args` against `Connection::args`).

**For the assistant.** `ssh_connections` lists the saved hosts (name, `user@host:port`, key name, whether the host key is trusted) and needs no
card. `ssh_login` (argument `connection`) asks to open one as a session in the tile: it is on the hard-ask list, so it asks every time (Auto
mode too), a session grant never covers it, and the card names the connection and says that the person types in the session. The tool
refuses before asking what the tile would refuse (no full terminal, a host the account may not reach, a host key that is not trusted yet,
no client); it can never trust a key. After the approval it queues the request (`sshsessiontools::request_login`); the shell brings the CLI
tile up and the tile runs the same checks as the L key again and opens the session. The assistant cannot type in, read or see the session
(commands whose output it needs go through `run_command`, which asks and redacts). The audit log has `ssh.login.request` with the
connection name. Code: `crates/iyakki-agent/src/sshsessiontools.rs`, tests `sshsessiontools_tests.rs`, `apps/cli_term_tests.rs`.

`crates/iyakki-vt` is the engine, in plain Rust with no dependency but `libc`: an escape-sequence parser (VT100 / ECMA-48 and the xterm
parts `top`, `vi`, `less`, `htop` and `tmux` use: cursor movement, erase, insert and delete, scroll regions, tab stops, SGR with 16, 256
and true colours, the alternate screen, line drawing, bracketed paste, answers to status requests, UTF-8 with wide characters), a screen
with scrollback, the key encoding, a `Terminal` over any byte `Link` and, on Linux and macOS, the pseudo-terminal (`posix_openpt`, `setsid`,
`TIOCSCTTY`, `TIOCSWINSZ`). Not done: mouse reporting, pictures (sixel), right-to-left text, combining marks (dropped), selecting and
copying text out of the session screen, a password prompt. The parser is fed random bytes in a test and always leaves a valid screen.
Tests: the engine with real shells on a pseudo-terminal (Linux and macOS), the tile's keyboard path, drawing at four sizes and the end of a
program with a fake link (everywhere), and one login through the tile, and one the assistant asked for, with a stand-in `ssh` program on a real pseudo-terminal (Linux, macOS).
Not tried: a real `ssh` to a real server, `vi`, `top` or `tmux` through it, a password-less login with a vault key.

## Checked and not checked

Checked: the rules, host keys, vault keys, saved hosts and the tab with stand-in programs at 1280x720, 1920x1080, 1024x600 and 800x480;
the real Dropbear client (the one in the image, run with its own musl loader) against a Dropbear server on 127.0.0.1: reading the key,
trusting it, and the refusal when the key changed (`loopback_dropbear_probe_trust_and_changed_key_end_to_end`, ignored by default; set
`IYAKKI_SSH_LOOPBACK=<port>:<folder with the links>`). Not checked: an image was **not** rebuilt or booted for this change (the existing
images already carry the client; the new self-test line appears in the next build), a login with a real key to a real server, OpenSSH
10 on macOS or Windows, passphrase keys with a real `ssh-keygen`.
