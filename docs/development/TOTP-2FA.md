# Authenticator-app two-step sign-in (TOTP) for IYAKKI accounts

**Status: DESIGN FOR THE OWNER TO APPROVE (2026-10-24, owner request). Nothing here is built.** Tags used below:
**EXISTS** (read in code or docs, path given), **GAP** (missing today), **PROPOSED** (this design). Nothing in this file is a
commitment until the owner answers the open decisions at the end.

**Owner decision already taken:** two-step sign-in is **optional and off by default**. Each person turns it on for their own account in
Settings -> Account -> Two-step sign-in. It is never mandatory by default, for administrators either.

## 1. Threat model: what it protects, what it does not

| Question | Answer |
|---|---|
| What does a code add? | A second proof at the sign-in screen and at a short list of sensitive actions (section 7): a stolen or guessed password alone is no longer enough. |
| Attacks it stops | password guessing or reuse (the code changes every 30 s); shoulder-surfing a password; a password typed into a look-alike page; a leftover signed-in browser tab on the web sign-in (docs/USER-GUIDE.md, "Sign in ... https://<machine>:8443/", EXISTS) when the action asks again. |
| Attacks it does NOT stop | **A stolen disk or USB stick.** The data volume is not encrypted (SECURITY.md line 52, KNOWN-ISSUES.md line 14, EXISTS); whoever holds the drive can read `users.json`, copy vault files and attack the Argon2id slots offline, and no code is asked. Only **disk encryption** (GAP, not planned here) closes that. Also: malware already running as the signed-in person; a person standing at an already signed-in desktop (IYAKKI has no lock screen, docs/SCREEN-SAVER.md line 97, GAP); a phished code relayed live to the real sign-in. |
| Physical access to the machine | Out of the model today (SECURITY.md line 46). A person who can boot the data volume elsewhere or reach a shell can use the recovery path of section 6; that is deliberate so nobody is locked out, and it is exactly why a drive in the wrong hands is not protected. |
| Must be said in the UI | The enrolment page states in one sentence: "This protects signing in. It does not protect the files on the drive if someone takes the drive." |

## 2. How accounts and sign-in work today (EXISTS)

| Piece | Where | Fact |
|---|---|---|
| Accounts file | `crates/iyakki-shell/src/accounts.rs` (`UserStore`, `UserRecord`) | `users.json`, mode 0600, atomic write. A record is `{name, hash, role, overrides}`; `hash` is Argon2id PHC. |
| Sign-in | `UserStore::authenticate(name, pw)` | One Argon2 verify always runs (unknown names cost the same). Returns a `Session {user, role, policy}`. Password only; there is no second step. |
| Throttle | `throttle_left` | In memory, per account name: from the 3rd failure wait 1, 2, 4 ... capped at 60 s. Lost at restart. |
| Roles | `iyakki_sec::policy::Role` (Admin, Operator, User, Guest); `Policy.manage_users` | The last admin cannot be removed or demoted. |
| Own password | `set_password` | Needs the old password even for admins; admins reset others without it. |
| Audit log | `AuditLog::record(user, event, detail)` | Hash-chained JSON lines, mode 0600, control characters stripped, 300 characters. Events today include `login`, `user.create`, `app.denied`, `agent.login.*`. |
| Vault | `crates/iyakki-agent/src/vault.rs` | `LocalVault`: random 256-bit data key, XChaCha20-Poly1305; the key is wrapped per **key slot** with an Argon2id key from that slot's password. Files `vault/system.afv`, `vault/users/<user>.afv` (docs/development/IMPORT-FROM-DRIVE.md). A personal vault opens with the login password. |
| Password change vs vault | `LocalVault::change_password`; docs/ADMIN-GUIDE.md lines 56 and 389 | Own change re-wraps the slot, data is kept. An admin reset of another person **resets their personal vault** (contents lost). |
| Random source | `iyakki_sec::random_fill` (`crates/iyakki-sec/src/lib.rs`) | Used by the vault for keys and nonces. |
| Hashes available | `sha2` 0.10 in several crates | No SHA-1 and no HMAC in the workspace code. |
| Sign-in surfaces | shell login `Screen::Login` (`shell.rs`); web sign-in on port 8443 (`iyakki-remote`, docs/USER-GUIDE.md line 776) | Both end in `UserStore::authenticate`. |
| Lock screen | docs/SCREEN-SAVER.md line 97 | **GAP:** none. The saver wakes straight into the session. "Lock/saver unlock" therefore needs the lock state to be built first (phase 5). |
| Forgotten password | docs/ADMIN-GUIDE.md "Forgotten admin password" | Anyone at the machine's shell can run `iyakki user add rescue --role admin`; no network needed; no vault password needed. |
| Developer mode | docs/DEVELOPER-MODE.md | One-time pairing code (10 characters, 5 minutes, 5 tries); approval cards `a`/`o`/`d` on the device for `shell`, `reboot`, `poweroff`. |
| Store | docs/STORE.md | Publisher keys sign packages; licence tokens are device-bound. (A publish "token" is a signing key operation, not a login.) |
| Clock | `crates/iyakki-agent/src/clockfloor.rs`, `ntp.rs` | Floor from build time and saved time; SNTP status in `ntp::status()` (`last_sync`, `offset`, `message`, `pending_back`). `clockfloor::pending()` is true while the floor is being applied. |
| QR | `crates/iyakki-agent/src/qr.rs` (`qr::encode(&[u8]) -> Option<Qr>`, `get(x,y)`, `version()`) | Encoder only. The QR **decoder** for the Claude Code sign-in (`agent_login.rs`) is separate work in progress; this design needs only the encoder and must not touch the decoder. |

## 3. The TOTP implementation (PROPOSED, in-house)

New module `crates/iyakki-agent/src/totp.rs` (pure functions, no I/O, tests beside it). No new dependency: SHA-1 (about 60 lines),
HMAC (about 25 lines over SHA-1 and the existing `sha2`), base32 (RFC 4648, about 30 lines) are written in-house (matches the
"build it ourselves" rule; no third-party code).

| Item | Choice |
|---|---|
| Standards | HOTP RFC 4226, TOTP RFC 6238. |
| Algorithm | HMAC-SHA1 by default (what every authenticator app supports); HMAC-SHA256 as an option written into the URI (`algorithm=SHA256`), selectable under "Advanced" at enrolment. SHA-512 not offered. |
| Digits, step | 6 digits, 30 s. (8 digits and 60 s not offered: apps differ.) |
| Secret | 160 bits (20 bytes) from `random_fill`, shown as base32 (32 characters, no padding) in groups of 4. 256-bit secret for SHA-256. |
| URI | `otpauth://totp/IYAKKI:<account>@<machine name>?secret=<base32>&issuer=IYAKKI&algorithm=SHA1&digits=6&period=30`, percent-encoded; the machine name comes from `hostname.rs`. The QR is `qr::encode(uri)`. |
| Compare | Constant time over the 6 bytes; the same cost whether a code, a recovery code or garbage is given. |
| Replay | The account stores `last_step` (the counter of the last accepted code). A code is accepted only for a step **greater** than `last_step`. Second use of the same code inside its window fails. |
| Window | Default: accept steps `now-1, now, now+1` (+/- 30 s). Optional per-account "My phone's clock is a little off": +/- 2 steps (+/- 60 s), never more. A wider window is never silent; it is named in the audit line. |
| Clock state | The check runs only when the clock is trustworthy: `clockfloor::pending()` is false, `ntp::status().last_sync > 0` **or** the time is above the clock floor. When the clock is below the floor or still being set the screen says: "The machine's clock is not set yet, so codes cannot be checked. Connect to the network, or use a recovery code." The floor test alone is weak (a clock only 2 minutes wrong passes it), so a failed check **with** `last_sync == 0` shows the same message as a hint ("or the machine's clock is wrong"). A recovery code works regardless. |
| Drift help | If a code is valid for step +/- 3..20, the screen says "Your code is right but the clock differs by about N minutes" **without** accepting it, and names the machine clock vs the phone clock. |
| Offline | TOTP needs no network on the machine; only a correct local clock. The clock floor keeps it from the past after reboot (`clockfloor.rs`); a battery-less machine that started offline in a stale-but-above-floor state is the main failure and is covered by the message above and by recovery codes. |

### Where the secret lives

| Option | Verdict |
|---|---|
| In `users.json` in clear | Rejected: the file is readable to anyone with the drive; the code would be free for them. |
| **Sealed in the person's personal vault** as a secret named `system.totp` (a reserved name, hidden from the vault list) | **Proposed.** The vault already seals with XChaCha20-Poly1305 under a key wrapped by the login password. Value: JSON `{alg, digits, period, secret, last_step, window, recovery_hashes[]}`. |
| Problem with the vault option | The vault is locked until the password is typed, and sign-in must check the code **after** the password (so the vault is open) - fine: password first, then code. But a vault reset (below) destroys the secret, and the **second step must still exist** to refuse a password-only attacker. |
| Companion field in `users.json` | `UserRecord` gains `#[serde(default)] two_step: bool` (a flag only, no secret) so the sign-in screen knows to ask. Default false, so old files read unchanged. Anyone with the drive can flip it off: that is the section 1 limit, stated. |

**Password change.** Own change (`set_password` plus `change_password`): the slot is re-wrapped, the secret survives, no new
enrolment. The change itself is a step-up action (section 7). **Admin reset of another person** (existing behaviour: personal vault
reset): the sealed secret is lost with it. Proposed rule: the reset also sets `two_step = false`, writes audit `2fa.reset`, and the
person is told "Two-step sign-in was turned off by the reset; turn it on again in Settings." This is a **GAP** to close deliberately,
not a surprise: otherwise the person would be locked out by a secret nobody can read.

**Failed-attempt state.** `last_step` and the recovery-code hashes change after sign-in; they are written inside the vault (needs the
vault unlocked: it is, since the password just opened it). The wrong-code counter is kept in `users.json` as `{fails, locked_until}`
so a reboot does not reset the lockout (today's password throttle is in memory only: GAP).

## 4. Enrolment UI (Settings -> Account -> Two-step sign-in) (PROPOSED)

The Account tab exists (`Tab::Account` in `crates/iyakki-shell/src/apps/settings.rs`, category "Account & people"). A new card under it. The card is
shown to every role except Guest. The state line reads "Off" until the person changes it.

| Step | Screen |
|---|---|
| 0. Intro | One short paragraph and the limit sentence from section 1. A button **Turn on...**. Asks for the account password first (re-prompt, not the open session alone). |
| 1. Add to your app | QR of the otpauth URI (from `qr::encode`), the secret in text groups (`ABCD EFGH ...`) with **Copy**, and the account label as the app will show it (`IYAKKI: <account>@<machine>`). Plain advice: "Open your authenticator app, add an account, scan this." No third-party app is named as required. |
| 2. Confirm | The person types the current 6-digit code. Only a correct code enables it (nothing is saved as "on" before). Three wrong tries cancel the enrolment and discard the secret. |
| 3. Recovery codes | 10 codes shown **once**, 10 characters each in two groups (e.g. `K7QFM-2XPRT`, same alphabet as pairing codes; ~50 bits each). Buttons **Copy**, **Print**, and a required tick "I saved these". Never shown again; **Make new codes** needs a fresh code and invalidates the old set. |
| 4. Done | "Two-step sign-in is on." Audit `2fa.enable`. |
| Off again | **Turn off** needs password plus a fresh code (or a recovery code); audit `2fa.disable`. |
| Advanced (collapsed) | SHA-256, wider window, "show me the secret again" (needs a fresh code; the QR hides on screen capture is not possible here, so a note says "someone watching can copy this"). |

The secret and URI live only in memory during enrolment, are zeroized (`zeroize` is already used in `vault.rs`) and are never written to
the clipboard unless the person presses Copy. The terminal screenshot feature must skip the secret view (check `shell` screenshot hooks).

## 5. Sign-in flow (PROPOSED)

| Case | Behaviour |
|---|---|
| Password then code | `authenticate` returns a **half session** (name only, no policy, no vault open for the apps) when `two_step` is true; the second field asks "Code from your authenticator app" and also accepts a recovery code (typed in the same box; the format differs). Only a complete result produces a `Session`. The half session expires in 2 minutes. |
| Unknown names | Behave like known ones: the second step is **not** shown for unknown names; they fail at the password with the same text and cost as today (no account enumeration by seeing "code" prompts). |
| Web sign-in (8443) | Same two steps; the web page must carry the half session as a one-use server-side token, not a cookie that grants anything. |
| Lock/saver unlock | GAP: no lock state today (docs/SCREEN-SAVER.md line 97). When a lock is built (phase 5) it asks the password only; a code at every unlock is not required (the code was proven at sign-in). Step-up actions still ask. |
| Wrong clock / offline | See section 3: clear message, recovery code accepted, never a bypass. |
| Rate limit (numbers) | Wrong code: 3 free tries per half session, then the half session ends (password again). Per account, persistent: after 5 wrong codes in 15 minutes wait 30 s; each further failure doubles the wait up to 15 minutes; after 20 wrong codes in 24 h the account is **locked for the second step for 1 hour** (password still counts). The password throttle (`throttle_left`, 1 to 60 s) is unchanged and also applies. Recovery-code attempts count in the same counter. |
| Guests | Excluded: the guest account has no personal vault and no enrolment card; `two_step` is ignored for `Role::Guest`. |
| Auto-sign-in / kiosk | No auto-sign-in exists in the code today (grep found none; `cage`/`cog` in `webengine.rs` is a browser kiosk, not a login). If one is added later, an account with two-step on is **not** eligible for auto-sign-in. |
| Boot, installer, emergency | Never asks (section 7). |

## 6. Recovery (PROPOSED)

| Situation | Path |
|---|---|
| Lost phone, has recovery codes | Type a recovery code at the code prompt. Single use: stored as `sha256(salt || code)` with a per-set 16-byte salt (codes have ~50 bits and are rate-limited by the lockout; Argon2id is overkill and slow on a sign-in screen, but acceptable if the owner prefers: open decision). Used code is crossed off; audit `2fa.recovery.used` (not the code). After one use the screen says "Set up your app again" and fewer than 3 codes left shows a banner. |
| Lost phone, no codes | **Administrator reset for other users:** Users app, select person, new key `2` (PROPOSED, next to `p`): turns `two_step` off, keeps their password and vault **only if** the secret is not needed to open the vault (it is not: the vault opens by password). Needs the admin's own fresh code if the admin has two-step on. Audit `2fa.reset <user> by <admin>`. The admin never sees the secret. |
| First or only admin, lost phone and codes: **physically present reset** | Everything is done at the device, with no network. Order of preference: (a) **console command** `iyakki user 2fa-reset NAME` run in the machine's terminal or the SSH diagnostics shell (the same access the existing "Forgotten admin password" procedure already needs, docs/ADMIN-GUIDE.md line 381). To keep it from being a free bypass for someone who merely has a shell, the command **asks for the account's password** (it opens the personal vault with it, proving the password and being able to remove the sealed secret). It works offline. (b) **Boot-menu entry** "Account recovery" on the physical console (the boot screen, `iyakki-stage1`, would need a new text menu: S-M work), which asks the same password and turns two-step off. Entry (b) is optional; (a) is enough. |
| Forgotten password AND lost phone | Unchanged from today: the `rescue` administrator procedure (ADMIN-GUIDE line 387) creates a new admin with no two-step; the old admin's personal vault is lost on reset (existing behaviour); two-step on the old account is turned off by the reset. Say plainly: **anyone at the console can do this; two-step is not a barrier against a person with physical access.** |
| Machine never lets you out | The recovery console command and the `rescue` procedure do not need a code, a network or the clock. |

## 7. Step-up: actions that ask for a fresh code (PROPOSED)

A "fresh code" is a code for a step newer than `last_step`, i.e. never one already used at sign-in (the person waits up to 30 s). Only for
accounts with two-step on. A successful step-up opens a **5-minute window for that kind of action** in the same session (so approving
several cards does not mean five codes). The window is held in memory only.

| Action | Where (EXISTS) | Ask? |
|---|---|---|
| Pair a computer in developer mode | `p` in DEVELOPER-MODE (`devmode/mod.rs`) | Yes, when the pairing code is opened |
| Approve shell / root / reboot / poweroff | cards `a`/`o`/`d` | Yes for `--root` and for approve-for-session `o`; a 10-minute approve `a` also asks (it is the same power). Refuse never asks. |
| Publish to a store, sign a package | `store/publishflow.rs`, `store/keys.rs` | Yes |
| Export the vault, import from another drive | docs/development/IMPORT-FROM-DRIVE.md | Yes (export always; import when it brings secrets) |
| Turn off two-step, make new recovery codes, show secret | section 4 | Yes |
| Change own password | `set_password` | Yes, plus the old password it already needs |
| Admin: create/delete users, change roles, reset others | `UserStore::create_user` etc. | Yes for an admin with two-step on |
| Remote / SSH admin | diagnostics shell, web sign-in | Sign-in itself already has two steps; SSH diagnostics uses its own keys and is **out of scope** (GAP, listed in decisions) |
| Add a vault secret provider, firewall off, install/uninstall system apps | Settings | Not in the first version (list kept short on purpose) |
| **Must NOT ask** | boot, installer, update rollback, emergency power-off on critical battery (`shell_battery.rs`), the recovery command of section 6, reading time, shutting down | Never a code: they must work with the phone lost, the clock wrong and the network gone |

## 8. Policy and audit (PROPOSED)

| Policy | Setting |
|---|---|
| Default | **Off for every account.** A person turns it on or off in Settings. |
| Per user | `two_step` (a flag) in the user record; the person controls it with their own password and code. |
| "Administrators must use two-step" | A single switch in Settings -> Account & people, **off by default**; only an administrator may flip it later. When on: administrators without two-step see a banner for the **grace period** (default 14 days, an administrator may set 0 to 90), then are sent to the enrolment page at the next sign-in. The switch is never turned on by the image or by an upgrade. Its state is audited. |
| "Everyone must use it" | Not proposed. |

Audit lines (`AuditLog::record`, names only, **never a secret, a code or a recovery code, never the otpauth URI**):

| Event | Detail |
|---|---|
| `2fa.enable`, `2fa.disable` | algorithm, window setting |
| `2fa.login.ok`, `2fa.login.fail` | `via=code` or `via=recovery`; for failure the running count |
| `2fa.lockout` | wait or duration |
| `2fa.recovery.used`, `2fa.recovery.new` | codes left |
| `2fa.reset` | target and who did it |
| `2fa.stepup.ok`, `2fa.stepup.fail` | the action name |
| `2fa.clock` | "code refused: clock not trusted" (once per attempt) |

## 9. Assistants, agents, browser and external tools

| Topic | Rule |
|---|---|
| The model never types a code | Same as docs/BROWSER-AGENT.md rule 4 (line 60), EXISTS: one-time-code and 2FA fields are refused by the tool; the person types them. The assistant, flows and skills have **no tool** that reads the secret, a code or the recovery codes; `system.totp` is excluded from `uvault:` references (a settings reference to it fails). |
| Step-up from an agent action | An action that an assistant triggers (publish, pair) shows the same card and waits for the person; the code box is a shell control, not a tool argument. |
| Browser saved logins | `browser_login` (BROWSER-AGENT.md line 25) fills passwords only. Storing a site's own TOTP secret in the vault so the browser fills codes is a **different feature**: not planned (the model must still never see it). |
| External agents (Claude Code, Copilot) | Their sign-in (`agent_login.rs`) is unchanged; those agents get no IYAKKI code. A developer-mode `iyakki dev shell` on a paired computer triggers an approval card on the device, which is the step-up point. |
| Remote shell sessions (`iyakki dev`) | Pairing and approvals need the person at the device; no code is ever sent through the client. |

## 10. Tests plan

| Test | Content |
|---|---|
| RFC 4226 Appendix D | the 10 HOTP values for secret `12345678901234567890`. |
| RFC 6238 Appendix B | the SHA-1, SHA-256 (32-byte secret) vectors at times 59, 1111111109, 1111111111, 1234567890, 2000000000, 20000000000, as 8 digits (the RFC lists 8) and the 6-digit truncation. |
| SHA-1, HMAC-SHA1, base32 | FIPS 180 and RFC 2202 and RFC 4648 vectors. |
| Window | accepted at -1, 0, +1; refused at +/- 2 with default window; accepted at +/- 2 with the wide option; refused at 3 with a drift message. |
| Replay | the same code twice -> second refused; an older step after a newer one refused. |
| Clock | below the floor, `clockfloor::pending()` true, `last_sync == 0`: message and no acceptance; recovery code still works. |
| Lockout | 3 tries end the half session; 5 in 15 minutes wait 30 s; 20 in 24 h lock; lock persists over a reopen of `UserStore`; guest excluded. |
| Recovery | each code works once; a used code fails; "make new codes" kills the old set; salted hashes differ across sets. |
| Enrolment | no flag set until a valid code; three wrong cancel and zeroize; secret never in the audit file (grep the log for the secret and the URI). |
| Compat | an old `users.json` without `two_step` reads as off; `iyakki-remote` tests (`tests/shell.rs`) still sign in with a password only for accounts without it. |
| Password change | own change keeps the secret; admin reset turns two-step off. |

### Phases and effort

| Phase | Work | Effort | Risk |
|---|---|---|---|
| 1 | `totp.rs`: SHA-1, HMAC, base32, HOTP/TOTP, URI, vectors, window, replay logic | S | in-house crypto: vectors from the RFCs are the safeguard |
| 2 | Account model: `two_step` flag, vault secret `system.totp`, counters, recovery codes, audit events, `authenticate` split into two steps, CLI `iyakki user 2fa-reset` | M | breaking sign-in for everybody if the flag read is wrong: tests with old files |
| 3 | Sign-in screen second field (shell and web) and the Settings card with QR | M | QR size on small panels; the web token handling |
| 4 | Step-up windows on the actions of section 7 | M | missing a path; start with pairing, shell approval, publish, export, disable, password |
| 5 | Lock screen (separate piece) and unlock rules | L | not part of 2FA itself |
| 6 | Policy switch and grace banner, admin reset key in USERS | S | |
| 7 | Hardware check: real panel and phone with two apps and a wrong clock on a Mac | S | the clock-off-by-minutes case |

## 11. Later options (NOT PLANNED)

| Option | What it needs |
|---|---|
| FIDO2 / WebAuthn security keys, passkeys | A USB HID transport (`iyakki-usb` has no HID class driver for this today, GAP), CTAP2 over HID (CBOR encoder/decoder, PIN protocol, `authenticatorMakeCredential` and `GetAssertion`), an ES256 (P-256) verifier (a `p256` crate is permissive but "build in-house" applies), a credential store per account (in the vault), user-presence prompt in the shell, the browser's WebAuthn JS API for sites (separate work in `iyakki-webview`), a policy for losing a key, tests with a software authenticator. L, several months. |
| Phone-approval push ("approve on your phone") | A relay service or a LAN protocol with a paired phone app and its own key; needs a server or an app: against the offline-first stance. Number matching to resist push fatigue. L. |
| Security key for disk or vault unlocking | Needs the disk-encryption work first. |

## OPEN DECISIONS (owner)

1. Approve the design as a whole and the phase order (section 10)?
2. Secret in the personal vault slot (proposed) or in `users.json` encrypted with a per-machine key? (Vault option means an admin reset turns two-step off.)
3. Recovery-code hashing: salted SHA-256 (proposed) or Argon2id (slower, stronger, ~50-bit codes)?
4. Physically-present reset: console command only (proposed), or also a boot-menu entry?
5. Should the reset command ask for the account password (proposed), or work with no secret at all like the existing `rescue` admin procedure?
6. Step-up list: accept the section 7 list for the first version, and the 5-minute step-up window?
7. Lockout numbers (3 tries per half session, 30 s after 5, 1 h lock after 20 in 24 h): accept?
8. SHA-256 and the wider window as "Advanced" options, or SHA-1 and +/- 1 step only?
9. Build a lock screen (L, phase 5) so the saver can ask on waking, or leave unlock without a code?
10. Is SSH diagnostics access to be covered later by its own key policy?
11. Keep the "administrators must use two-step" switch with a 14-day default grace (off unless an administrator turns it on), or drop the switch?
12. Should the Account page offer to turn two-step on right after the first account is created (a suggestion, not a requirement)?
