# SFTP server (F-FILE-01, queue item Q1)

Status 2026-11-08: built and tested on this PC with a real OpenSSH `sftp` client over loopback. Not yet tried from another computer.

## Decision

**Route (a): an in-house SFTP subsystem (protocol version 3) over an in-house SSH 2 server transport.** Route (b), a separate program
(Dropbear or OpenSSH) with a thin supervisor, was rejected. The reasons, in order of weight:

1. **The existing SSH code cannot act as a server.** `ssh.rs` only wraps a client program (OpenSSH or Dropbear's `dbclient`). But everything a
   server needs is already in the tree and reviewed for other work: X25519 and Ed25519 (`x25519-dalek`, `ed25519-dalek`, `sshkey.rs`),
   AES-CTR (`aes`, `ctr`), SHA-256 and a short HMAC. No new dependency was added.
2. **Scope enforcement is only exact in-house.** A person's reach is the account's file rules (`FileRoots`: home, workspaces, readable
   scopes, granted host folders, with separate read and write rules). Neither Dropbear nor OpenSSH's `sftp-server` can apply those rules; the
   most they offer is a chroot (needs root and one fixed folder) or a start directory (not a jail). With in-house code the same rules the
   file tools use decide every path.
3. **Login through the account system.** A program such as `sshd` checks passwords in its own way (PAM, `/etc/shadow`). IYAKKI accounts live
   in `users.json` with Argon2id and throttling; only in-house code can ask them directly and never see the hash.
4. **Third-party tools are never shipped (owner rule).** Dropbear's multi-call program is in the images only for the diagnostics client and
   has no `sftp-server`; OpenSSH would be a person-clicked download of about 2 MB plus a supervisor, config files and a PAM story, and the
   result would still not follow the file rules. Not worth it for a worse result.

The price of route (a) is that we own the security of an SSH server. That is why the design is small and strict (below), the packet layers
are pure and tested with hostile input, and the real OpenSSH client is used as the judge of the transport.

## What was built

| Piece | File | Notes |
| --- | --- | --- |
| SFTP v3 codec (pure) | `crates/iyakki-agent/src/sftp_proto.rs` | INIT/VERSION, OPEN, CLOSE, READ, WRITE, LSTAT, FSTAT, SETSTAT, FSETSTAT, OPENDIR, READDIR, REMOVE, MKDIR, RMDIR, REALPATH, STAT, RENAME; STATUS, HANDLE, DATA, NAME, ATTRS. READLINK, SYMLINK and extensions answer "unsupported". |
| Scope-confined files | `sftp_fs.rs` | The `Jail`: virtual tree, path walk, read and write rules as closures. |
| Session | `sftp_session.rs` | Handles, listing pages, audit lines, ends on a broken stream. |
| SSH 2 server | `ssh_server.rs` | `curve25519-sha256`, `ssh-ed25519`, `aes256-ctr` with `hmac-sha2-256-etm@openssh.com` or `hmac-sha2-256`, no compression, strict key exchange, re-keying, public-key and password login, one `session` channel, subsystem `sftp` only. |
| Manager | `sftp.rs` | Settings, authorised keys, host key, listener, rate limit, firewall rules, expiry. |
| Chat tools | `sftptools.rs` | `sftp_status` (safe), `sftp_set` (asks every time). |
| Tests | `sftp_tests.rs` and the module tests in `ssh_server.rs` | |

### The virtual tree and the scope rule

`/` is the person's home folder. Every other folder their account may read (a workspace, a scope such as `/shared`, a granted host folder)
shows as `/+name`. Nothing else exists. A client path is split on `/`, `.` and empty parts are dropped, `..` removes a part and never goes
above `/` (as in a chroot). The rest is walked one part at a time from a real, canonical base; after every part the path is canonicalised
(links followed) and must still be inside what the account may read, otherwise the part is "not found" (so a link that leads out gives no hint
about what is out there). A listing hides links that lead out. Writing (create, write, mkdir, remove, rename, setstat) needs the account's
write rule on every real path involved; the extra roots are read-only unless the account's rules say otherwise. The system data folder is
never reachable unless it holds the home itself. Files open with `O_NOFOLLOW` and `O_NONBLOCK` and must be regular files after opening (a
FIFO or device is refused without blocking). `READLINK` and `SYMLINK` are not offered. Removing a link removes the link, never the target.
Rename never replaces an existing file (version 3 rule). A folder is made only when its parent exists. Permissions are limited to the lower
nine bits; the owner is never changed.

### Login and limits

* **Off by default.** Nothing listens until an administrator enables it. It does not come back after a restart: each start is deliberate.
  Enabling asks on a card (the chat tool asks every time; no standing grant is possible).
* **Who:** only accounts on the allow-list; empty means nobody. Enabling is refused when nobody could log in (no key and no password login).
* **Keys:** Ed25519 public keys, one file per account (`<data>/sftp/keys/<account>.pub`, mode 0600, at most 20). Public keys may pass through
  chat; a private key or a password never may (the tool refuses anything that looks like one).
* **Password login** is a separate switch, off by default. The password goes straight to the account code (`Authenticator`: Argon2id,
  one verification time for unknown names, throttle per account) and is dropped; it is never stored, logged or put in an audit line. Ten wrong
  passwords from one address in five minutes block that address for five minutes, and while it is blocked the account code is not asked at all
  (so a stranger cannot lock the person's account by guessing from outside the block). Four failed logins end a connection; 30 seconds to log
  in; 15 minutes idle; 8 connections at once. Known limit: the account throttle can still be tripped by someone on the local network who tries
  passwords until their address is blocked; keys avoid this.
* **Where from:** only the local network. Every connection is checked (private, loopback and link-local addresses; anything else is closed
  at once and logged), and the firewall opening is four rules for the private IPv4 ranges only, for that one TCP port, with an expiry. The
  rules are removed when the server is turned off, when it expires, and by `disable` even if nothing runs (a leftover after a crash).
  Opening is kept at once (it can only let more in); the 60-second keep step applies to closing as for any firewall change.
* **Expiry:** `1 hour`, `1 day` (default), `7 days` or `never`, per start.
* **Host key:** an Ed25519 key made on first use in `<data>/sftp/host_key` (0600). Its fingerprint is in the status so the person can check
  it at the first connection.
* **Audit** (event, no secrets): `sftp.enable`, `sftp.disable`, `sftp.login`, `sftp.login_failed`, `sftp.refused_address`,
  `sftp.refused_request`, `sftp.protocol_error`, and per session `sftp.open create|write|read <path>`, `mkdir`, `rmdir`, `remove`,
  `rename a -> b`, `setstat`. Paths are virtual and have control characters replaced; file contents, passwords and keys never appear.

### What the transport refuses

No shell, no command (`exec`), no pseudo-terminal, no port, agent or X11 forwarding, no second channel, no compression, no other ciphers or
key types. Clients need an Ed25519 host key, AES-256-CTR and HMAC-SHA-256 (every current OpenSSH, Dropbear 2020+, PuTTY 0.75+, WinSCP, FileZilla
and libssh2). Old scp over `exec` does not work; OpenSSH 9 `scp` uses SFTP and does. Packets above 35 000 bytes are refused before they are
read; the MAC is checked before decryption in the encrypt-then-MAC form; sequence numbers wrap; strict key exchange (the Terrapin fix) is on
when the client offers it; a client re-key is handled.

## Verified and unverified

Verified on this PC (unit tests, no hardware): the codec with every request type, every cut-short prefix, lying lengths, bad UTF-8, NUL,
oversize paths and handles; the jail against `..`, absolute paths, doubled and backslash separators, links out (absolute, relative,
dangling, to a file, to `/etc`), links in, creation through links, the read-only roots, regular-file-only opens (a FIFO), hostile names and
the 64-handle and paging limits; the session conversation and the audit trail (no contents); the transport (both MAC modes, every packet
size, tampering, hostile lengths, hostile beginnings, weak key, wrong key size, signature checks, RFC 4231 HMAC vector); the keys, the host
key, the limiter, the firewall rules (including that they only open), the manager, the gate and the chat tool.

Also verified: a **real OpenSSH `sftp`** client over loopback, with a throwaway Ed25519 key (see the test
`a_real_sftp_client_works_over_loopback_and_cannot_escape`): login by key, `ls`, `put` and `get` of a small and a 3 MB file (byte-identical),
`mkdir`, `rename`, `rm`, `rmdir`, `/+shared` read-only, every escape attempt refused, an unknown key refused, password login through the
account code, a wrong password refused, audit lines without secrets.

Not verified: any client on another computer or another operating system (WinSCP, FileZilla, Cyberduck, Dropbear, Android clients), the real
firewall engine on a device (the rules are checked as data and with the validator; the engine call itself is the same one the web service
uses), real accounts from `users.json` (a fake account store stands in; the shell adapter is the same one the web service uses), behaviour
under many simultaneous large transfers, and the Settings page on a real screen. A time-of-check/time-of-use race (another program swapping a
folder for a link between the check and the open) is not closed; that is a limit of any path-based server.

## Steps for the owner to test from another computer

1. On the IYAKKI computer, ask the assistant: "turn on the SFTP server for my account with this key" and give the public key line of the
   other computer (`cat ~/.ssh/id_ed25519.pub`; make one with `ssh-keygen -t ed25519` if needed). Approve the cards (allow user, add key,
   enable). `sftp_status` then shows the port, the host key fingerprint and the address.
2. From the other computer on the same network: `sftp -P 2222 ACCOUNT@ADDRESS`. Compare the host key fingerprint it prints with the one in
   `sftp_status` before answering yes.
3. Try `ls`, `put`, `get`, `mkdir`, `rm`; then `cd ..`, `ls /etc`, `get /etc/passwd`: nothing outside the account's folders must appear.
4. Try the same from a phone or a computer on another network (mobile data): the connection must fail.
5. Optional: ask the assistant to allow password login, connect with the account password, then switch it off again; try five wrong passwords
   and see `sftp.login_failed` in the audit view.
6. Ask the assistant to turn it off, or wait for the expiry; the port must stop answering and the firewall rule must be gone.

## Later

* Settings page text for the same status and switches (Remote access) with the shell's approval card; today the chat tool is the
  way in (the page row is listed in FEATURES.md).
* ECDSA / RSA client keys, more ciphers (ChaCha20-Poly1305, AES-GCM) for older or stricter clients, `posix-rename@openssh.com`, `statvfs`, `fsync`.
* A per-account quota and a transfer log with sizes.
* Restart after reboot, if the owner wants it (today deliberately not).
