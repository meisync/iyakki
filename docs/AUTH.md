# AUTH

AUTH is an authenticator: it makes the one-time codes (TOTP) that web sites ask for after your password. It sits in the Devices &
System group beside USERS, for administrators, operators and users (not guests: a guest account has the chat and the inbox only). In a
cloud tenant it is shown like the other personal tiles.

## Where the codes live

Everything, the secrets and the names alike, is one secret (`auth.codes`, JSON) in **your own vault** (`iyakki_agent::authcodes`).
Nothing is on disk in the clear. Your sign-in password opens the vault; when it is locked, AUTH says so and shows nothing: sign out and
sign in again with your password, or open Settings, Vault and press `u`. A stored book that cannot be read is reported and never
overwritten. A saved secret is never shown again and cannot be exported; to change one, delete the code and add it again.

An entry is: service (issuer), account, secret (base32), hash (SHA1 by default, SHA256, SHA512), digits (6 by default, 7, 8), time step
(30 s by default) and group. Up to 200 codes and 30 groups. The codes follow RFC 4226 and RFC 6238 and are worked out in-house
(`iyakki_agent::totp`) from the hashes already in the tree; secrets are read as RFC 4648 base32, leniently (spaces, dashes, lower case,
missing padding).

## Adding a code

* Type the service, the account and the secret key the site shows (the *Kind of code* box offers 8 digits, SHA256, SHA512 and 60 s).
* Paste an `otpauth://totp/...` link into the secret box, or press **Paste link** with the link on the clipboard. The label, issuer,
  secret, algorithm, digits and period are read; percent-escapes are decoded. Counter-based (HOTP) links are refused in words.
* Press **Scan QR** and hold the site's QR code up to the camera. This is the same shared camera card as the clipboard history's Scan QR:
  it asks first, shows CAMERA IS ON with a live picture, stops as soon as a code is read, on Esc, when you leave and after a minute.
  What was read opens the Add form to look at; nothing is saved before Save. Pictures are never saved or sent.

## Using the codes

The page lists your groups on the left and the codes of the chosen group on the right: service and account, the code in big grouped
digits (`123 456`), a bar for the seconds left and, in the last five seconds, the next code. **Copy** (or a tap on the code) puts the code
on the clipboard as a secret: it is not kept in the clipboard history, pastes once and is wiped after 60 seconds.

The codes need the right time. If the clock is earlier than the clock floor (`iyakki_agent::clockfloor::COMPILED_FLOOR`) no code is
shown and the page says why.

Groups are your own names. New, Rename and Delete group (asks first; the codes of a deleted group move to the first group; the only
group cannot be deleted). Move puts a code in another group. Edit changes the service and account names. Delete asks first.

| Key | Does |
| --- | --- |
| Up / Down | choose a group or a code |
| Right / Enter | from the groups to the codes; Enter on a code copies it |
| Left / Esc | back to the groups, then out |
| `c` | copy |
| `a` / `p` / `s` | add / paste a link / scan a QR code |
| `e` / `m` / Delete | edit / move / delete (delete asks: press `y`) |
| `n` / `r` | new group / rename the group |
| Tab | switch sides |

## Audit

The audit lines name only the action and the service, never a secret or a code: `auth.add issuer=...`, `auth.copy issuer=...`,
`auth.edit`, `auth.move`, `auth.delete`, `auth.group.add|rename|delete name=...`.
