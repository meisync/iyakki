# Git and SSH keys

Alpha.2 adds git to the AI IDE: clone several repositories into one workspace, see where each one stands, fetch and pull them
all, and tell IYAKKI which SSH key or token each git site and each workspace uses. Rows F-GIT-01 to F-GIT-04 in
[FEATURES.md](../FEATURES.md). What is tried and what is not is said at the end.

To **look at** the repositories of your connections (files, commits, branches, issues) and of the clones on this computer without opening the IDE, use the
GIT tile: [`GIT-TILE.md`](GIT-TILE.md).

## What you get

* **git in the image.** The web and Mac images carry the `git` program (GPL-2.0, a separate program that IYAKKI starts as a
  process and never links) with HTTPS through libcurl and OpenSSL. The libre image leaves it out; the IDE says so plainly
  (see "When git is missing").
* **Clone repositories…** in the AI IDE (top bar ⋯ menu, the workspace switcher `c`, and "no: clone repositories into it next" in the
  new-workspace form). One address per line, `#branch` after an address, and a compact table with branch, depth and folder
  per repository; into the open workspace or a new one.
* **Repositories…** (⋯ menu, shown when the workspace has repositories): the state of every repository, progress and the last
  git message per clone, Fetch all, Pull all, retry, commands per repository, remove.
* **Settings → Git and SSH keys:** make or import ed25519 keys (kept in your vault), copy the public half or scan it, say which key
  each git host uses, set HTTPS tokens, forget a trusted host.
* **A key service in memory.** While a clone, fetch or pull runs, IYAKKI serves a tiny ssh-agent on a private unix socket and git's ssh
  asks it to sign. The private key is never written to a file, a workspace or `/tmp`.

## Clone repositories

```
https://git.example.org/team/app.git
git@github.com:octo/tool.git #dev
ssh://git@git.example.org:2222/team/api #release/1.2
```

* Addresses: `https://host/owner/repo(.git)`, `ssh://[user@]host[:port]/owner/repo`, or `[user@]host:owner/repo` (scp-like). Nothing
  else: `git://` (not encrypted), `file://`, local paths, `ext::` and anything starting with `-` are refused, so an address can never
  become an option or a program to run.
* **No password or token in an address.** `https://user:token@host/…` is refused with a pointer to Settings; the message never repeats the
  password. Tokens live in the vault and reach git through an askpass helper (below).
* Each repository goes to its own subfolder of the workspace: the name comes from the address (`app` for `…/app.git`), a number is added
  when the name is taken (`app-2`), and a name you type must be a plain name (letters, digits, `.` `_` `-`). It never leaves the
  workspace root and never lands on an existing folder that has files in it. A symbolic link is never used.
* **Jobs.** Each repository is a detached job of the jobs engine (`docs/JOBS.md`): the installer indicator in the top bar shows them, each
  has a live log, and they survive the chat. At most **two run at the same time**, the others wait. One failure never stops the others;
  a failed repository shows git's last message and a **Retry** button. A failed clone leaves no folder behind.
* **project.json** gets a `repos` list (url, folder, branch, key id, depth, state, optional `test` and `run`). The file tree, the built-in
  assistant and the external agents (Claude Code, Copilot) see every repository: the PROJECT STATE they start from lists them
  ("Repositories in this workspace: api/ <- https://… (branch main); test: cargo test"). No secret is in it.
* **Commands per repository:** in the Repositories panel, `e` sets a test and a run command for the selected repository. F5 on a file inside
  it runs that run command in the repository's folder, and "Run tests" for a file inside it runs that test command there; other files use the
  workspace's commands.
* **The platform's private Gitea.** Clone from a Gitea of the platform services (`docs/PLATFORM-SERVICES.md`) with its address like any
  other host: add a token (or a key) for its host name in Settings once. Gitea accepts any user name with a token.

### Status, fetch, pull, remove

* The file tree shows `main ↑1 ↓2 ✎3` next to each repository folder (branch, commits ahead and behind, changed files). It is read in the
  background every 30 seconds and after every operation; a failure of the read shows as `(message)` in the panel.
* **Fetch all** runs `git fetch --all --prune` everywhere. **Pull all** runs `git pull --ff-only --no-rebase`: it only moves forward, it never
  merges and never rebases. If some repositories have uncommitted changes a card names them and asks: pull the clean ones only, pull all
  (still fast-forward only; git itself stops when a changed file would be overwritten), or cancel.
* **Remove** (`x`) shows what would be lost (uncommitted files, commits not pushed, a branch without upstream), then asks you to type the
  folder name. Only a folder of the workspace is removed, never a link's target, and the `repos` entry goes with it.

## Connect Git or Gitea

One flow, two doors: **Settings → Git and SSH keys** (the *Connections* rows at the top) and the **AI IDE** (the top bar's *Connect Git / Gitea*
chip when the project has no remote, the `⋯` menu entry *Connect Git / Gitea…*, and the Repositories panel). Both use the same code
(`gitconn.rs`, the wizard in `apps/git_wizard.rs`), the same vault, the same key service and the same git jobs; there is no second git client and
no second token store.

**A connection** is a plain JSON record in `<data>/ssh/<user>/connections.json` (no secret in it): name, kind (GitHub, GitLab, Gitea / Forgejo,
Other), address, account, how it signs in, the commit name and email, whether it is the default, the last test and, for a self-signed server, the
fingerprint of the certificate the person trusted. The token is in the vault as `git.conn.<id>.token`; for the Gitea of the person's cluster the
connection simply names the entry the platform services already keep (`svc.<instance>.…token`) and nothing is copied. Removing a connection
deletes only its own `git.conn.…` entry. The git jobs find the credential of a host from the host table first and, when it has none, from the
connection for that host (read from where it is kept; the table is not changed), so a clone, fetch, pull or push over HTTPS just works after
connecting.

**Settings.** Rows show the name, kind, address, account, how it signs in, a status dot (green after a good test, red after a failed one, grey
when never tested), the last test's words and where the data goes (*on your cluster*, *on your local network*, *on the internet*). Keys: `a` add,
`T` test (the host's own API, in the background), `e` edit, `m` make default, `R` or Delete remove (type the name). The rows of keys, hosts and
known hosts below are unchanged. Anyone who has the AI IDE has this page; there is no extra permission.

**The wizard** (Settings `a`, or *Add connection…* in the IDE, in place): 1 kind; 2 where (for Gitea / Forgejo: *The Gitea running on my cluster*,
listing the instances installed through Platform services with their address, or *An existing Gitea / Forgejo server*; with none installed it says so
and has an *Open Platform services* button); 3 sign-in: a personal access token (masked; paste it or the text from the clipboard's Scan QR;
never shown again, only replaced) or an SSH key of the key service (pick one or make one; the public key is shown with Copy and the host's page
to add it: `/settings/keys` on GitHub, `/-/user_settings/ssh_keys` on GitLab, `/user/settings/keys` on Gitea); 4 test; 5 commit name and email
(prefilled from the account) and *use for new workspaces*. Everything works from the keyboard (Up/Down and 1-4, Tab/Enter, PgUp back, Esc cancel,
Ctrl+S next). **Nothing is stored until the last step's Save**: Cancel at any step leaves no connection, no token, no certificate file and deletes
a key the wizard made.

**The test** asks the host: Gitea / Forgejo `GET /api/v1/user` (and `/user/repos?limit=1` for reading), GitHub `GET /user` (scopes from
`X-OAuth-Scopes`), GitLab `GET /api/v4/user` and `/personal_access_tokens/self` (scopes). It shows the account, whether the token can read and write
repositories, whether the connection is encrypted, where the data goes, and says what it could not check (Gitea does not say what a token may write;
a GitHub fine-grained token has no scope list; *Other* hosts have no account API). Plain words: *The server did not answer*, *The token was
refused (401)*, *The token cannot write repositories (needs the repo scope)*. An account whose network policy forbids the host is told so and nothing
is sent. A server with a certificate the system does not trust shows its SHA256 fingerprint; **Trust this certificate** keeps that one certificate
for that one connection (curl gets it as its CA file, and git as `GIT_SSL_CAINFO` for that host); verification is never switched off.

**In the AI IDE**, *Connect Git / Gitea* offers the connections (and *Add connection…*), then:

* **Create a new repository** on that host: `POST /api/v1/user/repos` (Gitea / Forgejo), `POST /user/repos` (GitHub), `POST /api/v4/projects` (GitLab).
  The review shows the name (the project's, lowercased, editable), private by default, and every step in order: the request, `git init` when the
  folder is not a repository, the first commit when there is none (you are asked; a warning lists secret-looking files such as `.env` or `*.key`
  that `.gitignore` does not name), and `origin` set to the new repository's address (no credential in it; an existing `origin` is shown and replaced
  only because you confirmed). For the Gitea of the cluster this step is the platform services' own connect step (`platform/connect.rs`), not a copy.
  Nothing is pushed by it.
* **Push** has its own review: the exact remote and branch, how it signs in, where the data goes. It then runs as a normal git job (`Op::Push`:
  `git push --set-upstream --progress origin <branch>`), with the key service, the new-host fingerprint card and the token through askpass, and shows
  in the Repositories panel.
* **Connect to an existing repository**: pick it from the host's list (`GET …/user/repos`, GitHub `…/user/repos`, GitLab `…/projects?membership=true`)
  or type its address or `owner/repo`; a first Enter shows what will happen, a second sets `origin`. Nothing is pushed or fetched.
* The panel's header shows the connection, host, remote and branch with **Fetch**, **Pull**, **Push…**, **Change…** and **Disconnect**. Disconnecting
  (removing `origin`) is a separate confirmation card; the commits, the files, the server and the connection stay.

**Local Gitea (Quick install).** Store -> Setup -> *Gitea (versioning for your work)* installs a Gitea in the cluster and, once its password and
token are in your vault, adds the connection **Local Gitea** (kind Gitea, `http://127.0.0.1:3300`, signs in with the token the install made, placement
"on your cluster"; its token is the platform's own vault entry `svc.<instance>.token`, never copied). Running the install again changes nothing
(the connection is found by the instance it names; the address follows if it moved; a connection you named "Local Gitea" yourself is left alone and
this one is called "Local Gitea (IYAKKI)"). The GIT tile and the AI IDE's Git tab (Connections) list it like any other, and **Put this project under
version control on Local Gitea** in the AI IDE (see `AI-IDE.md`) uses it. Only this machine reaches it, over HTTP with the token; SSH is not
published. `iyakki-agent::quickinstall::gitea`, `docs/PLATFORM-SERVICES.md`.

**Audit** (host and repository names only, never a token): `git.connect.add`, `.edit`, `.test`, `.trust`, `.default`, `.remove`, `git.repo.create`,
`git.origin.set`, `git.push`, `git.disconnect`. Every error text passes through the token scrubber, and the token goes to curl in a private temp
config file, never in a command line.

## Keys, tokens and hosts

### Keys

* **Generate** makes an ed25519 key in memory (`ed25519-dalek`, BSD-3-Clause) and stores it, in the OpenSSH private-key format, in your vault as
  `ssh.<name>` (the same names the CLI tile's SSH client uses, so one key serves both). Only the public half, the fingerprint and the
  comment go to `<data>/ssh/<user>/git.json`.
* **Import** reads an OpenSSH private key you paste (a text field has no line breaks; the key is accepted on one line) or a file path the
  account may read. ed25519 only: an RSA or ECDSA key is refused with the instruction to make an ed25519 key. A key with a **passphrase**
  works when it was written with ssh-keygen's default (`bcrypt` + `aes256-ctr`); IYAKKI derives the key itself (bcrypt_pbkdf in `sshkey.rs`) and
  you choose whether the passphrase is **asked once per session** or **kept in the vault** (`ssh.<name>.passphrase`). A wrong passphrase is found at
  import time. Sign-out forgets the session's passphrases.
* **Public half:** the page shows it with the fingerprint, a short form to read out, a **Copy** button (`c`) and a QR code for a phone. Add it
  to GitHub (Settings → SSH and GPG keys), Gitea, GitLab or `authorized_keys`.
* **Delete** needs the key's name typed and clears the host rules that pointed at it.

### Which key for which site

The **host table** maps a git host to a key (`github.com` → key A, the private Gitea → key B) and, separately, says whether an HTTPS
token is stored for it. A **workspace** can override the key for a host (Repositories panel, `k`); the override is a plain `git_keys` map in
that workspace's `.iyakki/project.json` (host → key id), so it travels with the project and holds no secret. Resolution for an SSH address:
the workspace's choice, else the person's table, else none (the clone fails early with "no SSH key is set for <host>"). For an HTTPS address:
the host's token if there is one, else none (public repositories need none).

### Known hosts

Known hosts are kept per person (`<data>/ssh/<user>/home/.ssh/known_hosts`, the same file the CLI tile's SSH client uses). The first time
a clone needs an SSH host, IYAKKI asks the server for its host key and shows a card with the **SHA256 fingerprint**. Nothing is sent to the host
before you click **Trust this host**; compare the fingerprint with the one the site publishes. A **changed** host key is refused with an
explanation (recorded and presented fingerprints, "nothing was sent"); remove the old entry in Settings (Forget host) only when you know
the server was reinstalled.

### The key service

A `GitAgent` (`gitagent.rs`) lives while a batch runs. It holds only the keys and tokens that batch needs, in memory, and serves two sockets in a
private folder (`0700`) under `<data>/git-run/<user>/`:

* `agent.sock`: the ssh-agent protocol (request identities, sign request; everything else answers *failure*). git runs with
  `SSH_AUTH_SOCK` pointing at it, and its ssh is `GIT_SSH_COMMAND='/usr/libexec/iyakki/git-ssh -F /dev/null -o BatchMode=yes -o
  StrictHostKeyChecking=yes -o UserKnownHostsFile=<your known_hosts> -o IdentityAgent=<socket> …'`. OpenSSH's own `ssh-add -L` and
  `ssh-keygen -Y sign -U` are tested against it.
* `askpass.sock` and a link named `iyakki-git-askpass` to the running `iyakki` program (which, started under that name, answers git's
  `GIT_ASKPASS` prompt from the socket: the user name and token of that exact host, nothing for any other host). A token is never in an
  address, an argument, the environment, a log or the audit.

The folder, the sockets and the keys in memory are gone when the last job of the batch ends. A job outlives the shell, but its key service
does not: a clone that needs a key and outlives a restart of the shell fails and is retried.

### Audit

Every key use is written to the audit as `git.use clone <host> via key <id>` (or `via token`), every signature as `git.sign key <id> SHA256:…`,
trusted and refused hosts, key generation, import and deletion, and removals; never a key, a passphrase or a token. An **assistant** (not a
person's own click) must be approved once per key per session through an approval card before it can use a key (`Session::needs_approval`);
today every git operation is started by a person's click, and the assistant has no git tool that reaches the key service.

Related: [SSH tunnels](SSH-TUNNELS.md) use the same key list, the same known hosts file and the same host-key rules to put a web page behind an SSH host on `127.0.0.1`.

## Which ssh program (the choice)

git needs an `ssh` that speaks the **ssh-agent protocol**. Dropbear's `dbclient` (the image's `ssh`) takes `-i keyfile` only: using it
would mean writing the private key to a file. So the images carry **OpenSSH's `ssh` client alone** (BSD-3-Clause/ISC), built from the
upstream 10.2p1 source by the small package `dist/buildroot/package/iyakki-git-ssh` (about 1 MB) and installed as
`/usr/libexec/iyakki/git-ssh`. It is **not on the PATH**: the CLI tile keeps using Dropbear's `ssh`/`scp`, nothing about it changes, and
there is no sshd, no ssh-agent, no ssh-keygen in the image (key generation, the agent and the fingerprints are in-house). On a computer,
the system's OpenSSH `ssh` is used (if the only SSH client is Dropbear, SSH addresses say so and HTTPS with a token works).

## When git is missing

On the libre image (and any computer without git) the IDE says once, when it opens: "git is not installed on this IYAKKI image. The web and Mac
images include it …". The Clone dialog and the Repositories panel show the same message instead of the table and start nothing. No install
route exists on the libre image today (git is GPL-2.0 and the libre image leaves out GPL programs that are not needed to boot); on a
computer the message names the package manager command and says IYAKKI never runs sudo.

## Building git into the images

git, libcurl and the SSH helper are in the web fragment (`dist/buildroot/configs/iyakki_web.fragment`: `BR2_PACKAGE_GIT`,
`BR2_PACKAGE_LIBCURL` with OpenSSL, `BR2_PACKAGE_IYAKKI_GIT_SSH`), so a full web or macweb build has them. No perl, python, tcl, gitweb or
documentation is built. The quick way into an image that already exists (`docs/BUILD-INCREMENTAL.md`):

```
make -C target/buildroot-web git iyakki-git-ssh        # the packages only; JOBS=5 as for the other packages
./run.sh build-macweb-layered                          # repack.sh ... --graft git
```

`dist/buildroot/graft.sh` copies the files named in `dist/buildroot/graft/git.list` (git, the git-core helpers except the perl and python
ones, libcurl, the templates, the ssh helper) from the base build's `target/` into the unpacked root file system, keeps hard links, and fails
when a shared library of a copied program is missing from the image. `dist/buildroot/test-graft.sh` tests it on fake folders. It was also run for real on a copy: `repack.sh web <scratch dir> --base target/buildroot-web --graft git --fast-zstd` made an ISO and an install image in 16 s (nothing outside the scratch dir was written), and the git files taken back out of that copy's `rootfs.cpio` ran with the image's own musl loader and libraries (`git --version` 2.53.0, `git-ssh -V` OpenSSH_10.2p1, an HTTPS clone of a public repository, 156 git-core entries with the hard links kept and no perl helper). That copy was then deleted; the ISO was not booted.

**Size:** 163 files, about **14.3 MB** in the file system (git is one 4.4 MB program with 150 names; `git-remote-https` 2.6 MB; libcurl 0.7 MB;
the SSH helper 1.1 MB), about **3.4 MB** added to the compressed initrd/ISO (zstd -19, measured). The libraries git needs besides libcurl
(OpenSSL, zlib, expat, pcre2, nghttp2, libpsl) were already in the web image.

**Licences:** git is GPL-2.0 (some files LGPL-2.1+); OpenSSH's ssh is BSD/ISC; libcurl is curl's licence (MIT-style). `make legal-info`
puts git's source, and the others', into `legal-info/` automatically (the packages are in the build's manifest); `LICENSING.md` lists them.

## Tests

* `sshkey_tests.rs`: pi-derived Blowfish tables, key round trip, `ssh-keygen -l -f` and `-y` read our keys, we read `ssh-keygen`'s keys with
  and without a passphrase (bcrypt + aes256-ctr), RSA refused plainly.
* `gitagent_tests.rs`: the agent protocol with a test client, socket and folder modes, real `ssh-add -L` and `ssh-keygen -Y sign -U` against
  it, askpass answers (only the right host and user).
* `gitkeys_tests.rs`: vault storage (no private key in any file), CLI-tile keys listed, passphrase per session or in the vault, host → key
  with the workspace override, tokens, trust on first use, changed key refused.
* `gitsite_tests.rs`: https/scp/ssh:// parsing, credentials refused and not echoed, option and `ext::` injection, folder naming and escape
  attempts. `projectstate.rs`: repos listed for the assistant.
* `gitjobs_tests.rs`: jobs two at a time, partial failure and retry, host-key card flow, no key written to disk, no token anywhere, the
  `GIT_SSH_COMMAND`; **real git** with local bare repositories (clone with branch, status ahead/behind/changed, fetch, fast-forward pull,
  diverged pull fails, removal), skipped when git is absent.
* `gitconn_tests.rs`: the connection store (token only in the vault, default, edit, remove), the test for Gitea, GitHub and GitLab against a fake
  API (200, 401, 403, scopes, unreachable, self-signed fingerprint and trust), create and list repositories, the cluster's Gitea, the project's
  git through a fake runner. `gitjobs_tests.rs`: the push job. `git_wizard_tests.rs`, and the connection tests in `settings_git_tests.rs` and
  `ide_git_tests.rs`: the wizard's steps, back and cancel, the flow, the review, the push, disconnecting.
* `ide_git_tests.rs` (IDE) and `settings_git_tests.rs` (Settings): the dialogs, the panel, cards, tree badges, per-repository commands, the
  page and forms, all drawn at 1280x720, 1920x1080, 1024x600 and 640x360.
* `dist/buildroot/test-graft.sh`.

## Not tried, said plainly

* **Connect Git or Gitea** was tried only against fakes (a fake API client, a fake runner for the project's git, a fake git program for the push):
  no real GitHub, GitLab, Gitea or Forgejo was asked, no real repository was made and nothing was pushed; curl's header and certificate output
  (`include`, `--certinfo`) is read from sample text, not from a live server; the screens were drawn in tests, not looked at on a real screen.

* A clone from a real GitHub/Gitea over SSH, and over HTTPS with a token, on the Mac mini or a PC: the tests use a fake git, local
  repositories and a fake ssh. The `git` and `git-ssh` programs were built in `target/buildroot-web` and run on the build PC with the image's
  own musl loader (HTTPS clone of a public repository worked; `git-ssh -V` printed OpenSSH_10.2p1), but no new image was assembled or booted.
* The key service with the image's `git-ssh` against a real server (OpenSSH's own tools against the service are tested).
* Keys other than ed25519, and passphrase keys written with a cipher other than `aes256-ctr` (chacha20-poly1305, aes256-gcm): refused with the reason.
* `git://`, Git LFS, submodules over SSH with different keys, and signing commits are not handled.
* Terminal `git` typed by hand has no key service (it works for public HTTPS and for credentials git finds by itself); use the IDE's buttons for
  authenticated work.
