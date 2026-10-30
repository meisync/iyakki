# DOCTOR and Share diagnostics

DOCTOR is one screen that checks this computer, lists what is wrong in plain words, and gives every finding ONE fix button.
**Share diagnostics** builds a support bundle with the secrets taken out, shows you exactly what it holds, and saves it to a file
you send yourself. Nothing is uploaded anywhere. (Backlog P3 and P4; matrix rows F-DOCTOR-01, F-DOCTOR-02, F-ENGLOG-01, F-DIAGSHARE-01, F-DIAGSHARE-02.)

## The tile

Devices & System group, for administrators, operators and users (not guests). Open it and the checks run at once; the cheap ones
are on screen immediately, the two slow ones (a connection to each model server on this computer or the local network, and the
audit log) come a moment later ("still checking...").

| Key | Does |
| --- | --- |
| Up / Down | choose a finding |
| Enter | the finding's fix button |
| `r` | check again |
| `s` | Share diagnostics |
| Esc | leave (or close the preview) |

Administrators also see a technical detail line under a finding (an address, a port, a log line); everyone else sees the plain
sentence only. Cut lines show the whole text in a tooltip.

## What it checks

| Finding | Seen when | Its fix button opens |
| --- | --- | --- |
| The dev cluster is stopped / stopped with an error | it is installed and failed, or stopped although autostart is on, or ended on its own in the last day | Settings, Dev cluster |
| The cluster's own network address is missing | the fixed node address is in use and no interface carries it | Settings, Dev cluster |
| A cluster service is open to the local network | the newest `devcluster.exposure.*` audit line is `devcluster.exposure.unexpected` (DOCTOR reads it; the scan itself is the dev cluster's) | Settings, Dev cluster |
| The firewall blocks the developer-mode port | developer mode is on and the firewall lets nothing in to its port (the Developer page's own rule) | Settings, Firewall |
| Developer mode switches off in N minutes | less than an hour left | extends it by 4 hours (administrators; same call and audit line as the Settings button), others get the Developer page |
| The vault is locked and a password waits | the pending-vault list (`vaultpending.rs`) has entries for you | Settings, Vault (you type the password there, never in DOCTOR) |
| The web engine stopped unexpectedly / trouble with the camera or microphone | the kept engine log (below) has such lines in the last day | Settings, Browser (camera) or Sound (microphone) |
| No sound card / the chosen output is not connected | no card listed after the first minute, or the chosen output is gone | Settings, Sound |
| The disk is getting full / almost full | under 10% or 2 GB free (warning), under 5% or 512 MB (problem) | Settings, Storage |
| The clock has not been set from the network | the network is up for 5 minutes and no time server answered, or the clock is ahead and waits for your yes | Settings, Time |
| A model server does not answer | an enabled provider whose address is on this computer or the local network refuses a connection (cloud services are never touched) | Settings, Models |
| No network / names do not resolve | slow check: no default route, or the time server's name does not resolve within 2 seconds (one lookup, nothing is sent to it) | Settings, Network |
| Developer mode is on but nothing answers on its port | slow check: a connection to the port on this computer (nothing is sent) | Settings, Developer |
| Memory is tight / almost out / swap nearly full | under 10% / 5% of memory available, or under 10% of swap free (`/proc/meminfo`) | Settings, Overview |
| No camera was found (note) | `/sys/class/video4linux` lists none | Settings, Browser |
| The firewall is switched off | the firewall engine's config has it off | Settings, Firewall |
| Your vault is locked (note) | locked and nothing waits for it | Settings, Vault |
| A system update did not finish | the update centre's interrupted-update sentence (same source as the Settings overview) | Settings, System update |
| The last session ended unexpectedly | the kept kernel log marks the previous session unclean and you have not pressed "I have seen it" | Settings, Session log |
| A VPN is switched on but not connected / down with the guard on | `Manager::summary()` light Open (warning) or Blocked (problem); the profile name is never shown | Settings, VPN |
| Any other service did not start | the startup registry has it as failed | its own page |

A fix **never** accepts a permission popup, a licence or a vault password for you, never changes the firewall and never restarts
the cluster: it opens the page where you do that. (Plan decision D4, whether a fix may restart k3s without a question, is still
open; until it is decided none does.) Each fix press is audited as `doctor.fix` with the finding's id.

The rules are one small function each in `crates/iyakki-agent/src/doctor.rs`, over a plain `Facts` struct, so every one is tested
with a made-up machine. `doctor_facts.rs` in the shell reads the real machine.

## The kept web-engine log (plan N4)

The browser engine's helper writes to its standard error (GStreamer and WebKit warnings, the sound doctor's notes, why it stopped).
Only the last twenty lines used to be kept, in memory. `crates/iyakki-agent/src/enginelog.rs` now keeps the last ~200 KB in two
rotating files, `<data folder>/log/engine.log` and `engine.log.1` (next to the crash log; mode 0600). The shell writes to it when
the helper reports a line or stops.

* **Kept:** the helper's own lines, each with the time.
* **Not kept:** page addresses (each becomes `[address]`), page console messages, anything that looks like a credential, control
  characters. A line is cut at 400 characters.
* **Read it:** `iyakki dev logs --system --source engine` (developer mode, scope `debug`, administrators), DOCTOR, and the bundle.

## Share diagnostics

From DOCTOR (`s`, or the button) or Settings, Overview, "Share diagnostics". A dialog shows the whole bundle, line for line;
the arrows and the wheel scroll it. Then:

* `s` or Enter, or the first button, saves it in your home folder as `iyakki-diagnostics-YYYYMMDD-HHMMSS.txt`.
* `1` to `5`, or the other buttons, save it on a USB stick or card that DISKS opened (the folders in `<home>/media`).
* After saving: Copy the file's path (`c`). Esc closes without writing anything. A file is never replaced.

The file is a single readable text file with sections: Version, Services, Doctor, Check-up (the newest CHECK-UP report, `docs/CHECKUP.md`;
"(nothing)" when none ran), Switches that are on, Hardware, Web engine log, Boot log (end). **What is shown is what is written**: the preview text and the file come from the same function and the test
compares them byte for byte. It is capped at 512 KB (the oldest log lines go first).

### Redaction

Every line goes through `redact_line` (`crates/iyakki-agent/src/diagshare.rs`), then the whole text through the chat's credential
detector once more. Removed, replaced by `***` or a bracketed word:

* `Cookie`, `Set-Cookie`, `Authorization`, `X-Api-Key` header values;
* the value of anything named like a secret (`password`, `token`, `api_key`, `secret`, `cookie`, ...) as `name=value`, `name: value`
  or in JSON, and a vault entry's name with its value (`vault:name=value`);
* the query string and fragment of an address, and `user:password@`;
* key formats the detector knows (`sk-...`, `ghp_...`, AWS ids, JWTs, private key blocks), `Bearer` and `Basic` tokens;
* hexadecimal strings of 32 characters or more (image checksums too);
* e-mail addresses, MAC addresses, IPv4 and IPv6 addresses (not loopback), and the account name in `/home/<name>` paths.

Address host names and paths stay, so a log can still be read. Redaction removes too much rather than too little.

## What is not built

* `iyakki doctor` and `iyakki dev doctor` on the command line (plan P3), and `iyakki diag bundle -o FILE` (plan P4).
* Sending the bundle (plan decision D5): file or stick only.
* A "pending update" check beyond an unfinished one (no "update available" is known to DOCTOR), and a "missing firmware" and a "camera busy" finding from the live camera state (only the kept log is read for the camera).
* Everything is tested with made-up machines and files; the tile has not been looked at on a real screen, with the real helper's
  log, or with a real USB stick.
