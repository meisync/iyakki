# Shared folders that open the firewall with consent, and a package drop

Status: plan, 2026-11-05. Owner request: "share drive should open the firewall automatically with user consent. This is also a way for us to send a package."

## Today

* A shared folder (`sharedfolder.rs`, `sharedsite.rs`, Settings -> Shared folders, F-SHARE-01 to 06) listens on a fixed port per share, with access modes
  (secret link, password, anyone on the local network after a typed confirmation, this machine only) and optional uploads (off by default).
* Starting a share does **not** touch the firewall. The person has to open the port by hand (Settings -> Firewall), which most people will not find.
* The firewall (`iyakki-fw`) already has time-limited allow rules with an approval step (`devrules::plan`, `iyakki dev config firewall allow VALUE --ports P
  --minutes N`, `keep`, `revert`) and typed rule writers (`gaterule`, `webrule`).

## 1. Opening the port with consent

* Starting a share whose mode can be reached from the network (secret link, password, anyone on the local network) shows one card:
  "Open port N on the firewall for devices on your network while this share runs? Reach: this network only (not the internet). [Open] [Not now]".
  The person who presses Start is the one who consents; a share started by a flow or by the assistant needs the approval path of the approvals router instead.
* Yes: a firewall rule for exactly that port and protocol, source limited to the local network (private ranges, never a public address), written through the
  typed rule writer, with the share's id in the rule's name. No: the share starts and says "reachable from this computer only; open the port when you want others in".
* The rule lives as long as the share runs. It goes away when the share stops, expires (the share's own expiry), at sign-out, at "Stop all sharing", and at boot
  (shares do not survive a restart unless "keep running" is on; then the rule is written again only after the share starts).
* Audit lines: `share.firewall.open|close` with the share id, port, source range and who consented. Never a rule for a public interface or "any" source.
* Administrators can forbid it in policy (the same switch family as the tethering block): then the card says the administrator does not allow it.
* Tests (fakes, no real firewall): consent yes/no, rule exactly one port with a private source, removal on stop, expiry, sign-out and boot clean-up, policy
  block, refusal of public sources, a crashed share leaves no rule behind (clean-up scan at start).

## 2. A package drop (how we, or anyone, send a file to the machine)

A shared folder with uploads is already a way to send a file with `curl -T file http://HOST:PORT/name` or a browser. What is missing is a target the add-on installer
reads, because `/local/packages` is administrator-only and a normal share cannot be there.

* **"Receive a package" preset** (Settings -> Shared folders -> New share -> Receive a package; administrators only). It makes a share of
  `<data>/local/packages` with uploads on, password mode, this-network only, 10 minutes by default (the person may change it), and the consent card of
  section 1. The agent runs as root, so the file lands where the installer already looks (`/local/packages`), owned by root. Nothing needs a stick or a shell.
* **Only what the installer would accept**: the drop takes `ffmpeg-libs-*-linux-musl-<arch>.tar.gz` and its `.sha256`, never overwrites a file, caps the
  size (64 MB), refuses every other name and every folder. A drop that was not used is removed with the share.
* **Nothing is installed by the upload.** The Store item "Browser audio and video" shows the review (found file, FFmpeg version, NOTICE, checks) and the
  administrator confirms the licence and patent line, as for any package source.
* The same mechanism carries other packages later (an update pack, a tile app) with its own allow-list of names; each is checked by its own installer.
* For the owner's developer workflow the commands are: on the Mac start "Receive a package", press Open on the card, then on the PC
  `curl -T ffmpeg-libs-....tar.gz -u iyakki:PASSWORD http://MAC:PORT/` and press Install.

## 3. Order

1. Consent card and the firewall rule (section 1) for every share. Tests above.
2. The package-drop preset (section 2) on top of it.
3. FEATURES.md rows F-SHARE-08 (firewall with consent) and F-SHARE-09 (package drop), BOUNDARY.md if a new file calls the firewall writer.

## Open questions

* Should "anyone on the local network" shares always need the consent card, or may an administrator pre-approve a port range for shares?
* Should the drop also accept the package from a `.local` name without a password when the administrator is standing at the machine? (Recommendation: no, always a password.)
