# Remote files: SFTP and FTPS client (F-FILE-02, queue item Q2)

Status 2026-11-08: built and tested with fake and in-tree servers over loopback sockets. Not yet tried against a server that is not ours
(see "Steps for the owner").

## What it is

The REMOTE FILES tile (System group). Saved connections, a folder browser, upload and download with progress and resume, a transfer queue,
and chat tools for the assistant. Protocols: **SFTP** (over an in-house SSH client), **FTPS** (explicit TLS, `AUTH TLS`) and **plain FTP**
(only behind an explicit warning toggle on the connection).

## Where the code is

| Piece | Where | Notes |
| --- | --- | --- |
| The tile | `crates/iyakki-tile-remotefiles` | Depends on `iyakki-uikit` and `iyakki-tile-api` only; compiles and tests alone (about a second). |
| The facade | `crates/iyakki-tile-api/src/remote.rs` | Plain data, the `RemoteSource` trait (like `UsageSource`), `NoRemote`; `TileCx` gained one field, `remote`. |
| Sessions, pins, queue | `crates/iyakki-agent/src/remotefiles.rs` | One `Manager` per signed-in person, shared by the tile and the chat tools. |
| SFTP client | `rf_sftp.rs` | Pure codec (`parse_response`, `encode`) plus `SftpFs` over any stream. |
| SSH 2 client | `rf_ssh.rs` | curve25519-sha256, ssh-ed25519, aes256-ctr, hmac-sha2-256 (etm or not), strict key exchange, server re-keys answered. |
| FTP / FTPS client | `rf_ftp.rs` | rustls (already in the agent) for FTPS. |
| Chat tools | `remotefilestools.rs` | See below. |
| Shell side | `crates/iyakki-shell/src/apps/remotefiles.rs` | The `RemoteSource` adapter, the vault and file rules, the shell `App` wrapper. |
| Policy | `iyakki-sec` `App::Remote` (id `remote`) | On for administrators, operators and users by default. |

**The SFTP codec and the server.** The server agent's codec (`sftp_proto.rs`) parses requests and writes answers; the client needs the
opposite direction, so `rf_sftp.rs` is its mirror. They share only packet numbers by value; nothing in the server's files was touched.
Folding both into one module is a later cleanup. The in-tree SSH server (`ssh_server.rs`) is used as the test server for the client, so
the two implementations judge each other. The client's SSH packet layer repeats about 120 lines of the server's `Transport` (its fields
are private); same later cleanup.

## Decisions

* **In-house SSH client**, not the system `sftp`. The images carry Dropbear, which has no `sftp` client, and a program started for each
  action cannot show progress, resume, or pin host keys in the way the tile needs. The primitives (X25519, Ed25519, AES-CTR, SHA-256) are
  the ones the server already uses; no new dependency.
* **Host keys pinned on first use.** `Manager::open` ends before any password or key signature is sent when the identity is not the pinned one:
  `Unknown` (a card with the fingerprint, the person presses y) or `Changed` (refused, red, no accept button). `Manager::trust` accepts only
  the fingerprint the server showed last, and refuses when another is pinned; the person must first use "Forget the old identity" (a
  confirmation) and then it is a first contact again. For FTPS the pin is the SHA-256 of the server certificate (no CA store needed; a
  self-signed server works). **The assistant cannot trust or forget an identity**; there is no tool for it.
* **Secrets.** A password is stored in the person's vault as `rf.<name>`; the saved list (`<data>/remotefiles/<user>/connections.json`,
  0600) holds none. A key is a vault key made in the CLI tile (`ssh.<x>`, with an optional `ssh.<x>.passphrase`). Passwords are never
  logged, audited, put in an error text, or sent through a tool.
* **Plain FTP** needs `plain_ok` on the connection (a choice in the form with the warning text) and is refused by `Conn::validate` otherwise;
  the assistant cannot create one.
* **Transfers on their own connections**, at most two at a time, so browsing stays quick during a big download. Downloads write
  `name.part` and rename when the size matches; uploads write `name.part` on the server and rename there. A `.part` is continued if it is
  not longer than the file (resume); cancel keeps it. A final file or a remote name that exists is replaced only after an explicit
  "Replace". A link at the local target (or at the `.part`) is never written through.
* **Local paths** go through the person's file rules (`FileRoots`: read and write rules, granted host folders) at the moment the transfer is
  asked for and again when it runs.

## Hostile server input (tested)

SFTP: lengths checked before allocation, names with `/`, `..`, `.`, empty or NUL dropped from listings, data blocks longer than asked
refused, answers with the wrong id refused, packets over 256 KiB refused, every prefix of every answer parses to an error and never panics.
SSH: greeting line limits, packets over 35000 bytes refused before they are read, bad padding, wrong signature, weak keys, MAC failures,
stop switch and idle timeout. FTP: reply lines and reply sizes capped, listings capped, `PASV` addresses ignored (the control address is
used), port 0 refused, names with path parts dropped from `LIST` and `MLSD`, commands refuse paths with CR, LF or NUL (no command injection),
the data connection must show the control connection's certificate.

## Chat tools

`remote_connections` (safe), `remote_list` (safe, marked untrusted: names come from the server), `remote_transfers` (safe; cancel, clear),
`remote_close` (safe), `remote_connect` (asks), `remote_transfer` (asks, one file, background), `remote_change` (asks; mkdir, rename, delete;
delete asks every time and cannot be granted), `remote_save` (asks; no password, no plain FTP). Everything the tile does except trusting an
identity and typing a password is reachable by chat.

## Honest limits

* Verified only against our own SSH/SFTP server (loopback) and a fake FTP/FTPS server written for the tests. **Not verified against OpenSSH
  `sshd`, vsftpd, ProFTPD, FileZilla Server or a NAS.**
* SSH: Ed25519 host keys and Ed25519 keys only; the cipher is aes256-ctr with hmac-sha2-256. A server that offers only RSA or ECDSA host
  keys, or only chacha20/gcm, cannot be used (the error says so). RSA keys in the vault are not usable.
* SFTP protocol version 3 only, no symbolic links shown as such beyond the attribute bits, no `.`/`..` navigation tricks. Pipelined reads
  and writes (8 in flight) assume answers in order (OpenSSH does).
* FTPS: servers that require TLS session reuse on data connections may refuse; Windows IIS style listings are not read (servers with
  `MLSD` are fine); resume needs `REST` (download) and `APPE` (upload).
* A resumed download trusts that the remote file did not change since the `.part` was started (the size must still fit).
* Folders are not transferred whole; delete is for files and empty folders; no recursive operations.
* No UI for private key import in the tile: make or import the key in the CLI tile.

## Steps for the owner (real servers)

1. Start the tile (REMOTE FILES in System), press `n`, add your NAS or a Linux box with SFTP, enter the password, save.
2. Press Enter on it: compare the fingerprint card with `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` on the server; press `y`.
3. Browse, press `u` to upload a file, `d` to download one, `t` for the queue; cancel a big download with `c`, start it again with `r`.
4. Reinstall (or swap the host key of) the server and connect again: the tile must refuse and show both fingerprints.
5. Repeat with an FTPS server (vsftpd with `ssl_enable=YES`, or FileZilla Server). Report servers that fail with the error text.
6. In the chat: "list my remote connections", "connect to nas", "download /x to my Downloads" (each asks).
