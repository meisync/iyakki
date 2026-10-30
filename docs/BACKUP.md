# Backup and restore

Settings -> Account & people -> Backup and restore. One file with your own data, to keep on a stick or another disk, and a way
back from such a file. Code: `crates/iyakki-agent/src/backup.rs` (format and rules), `crates/iyakki-shell/src/apps/settings_backup.rs`
(the page). Plan item P6 (`docs/development/IMPROVEMENTS-PLAN.md`, `docs/project/NEXT-PLAN.md` item 11). Rows F-BACKUP-01 to 03.

## What goes in

Only the places in a fixed table (`backup::sources`), for the signed-in person. Each can be switched off on the page.

| Area | Where it lives |
|---|---|
| Settings | the person's layout, text size, screen saver, displays, radio and browser choice files in the data folder |
| Browser | bookmarks, history, zoom (the browser data file) and the header rules file. Header **values** are in the vault and are not in the file |
| Dashboards | the `Dashboards` folder of the home folder, and the person's data-source file (it holds only the names of vault entries) |
| Flows | the `Flows` folder of the home folder |
| Skills | the person's skills folder |
| Notes | the `.iyakki` folder of the home folder (context and notes) and the person's calendar |
| Chats | the person's chat sessions |
| Recordings | the `Recordings` folder of the home folder |
| Vault | the person's vault file, copied byte for byte. It stays the encrypted file it is; nothing is decrypted into the backup |

Never in the file: VPN profiles and keys, Git and SSH keys, tokens, system-wide settings, other people's data, the store, Gitea,
models. A file under any area whose text holds a private-key marker (`PRIVATE KEY-----`, `PrivateKey`, `PresharedKey`, an OpenVPN
static key) is left out and listed on the page. Links (symbolic) are never followed or stored.

## The file

`IYAKKIBK`, a format number (now 1), a flag byte, then the body: a length, a manifest (account, release, time, and per file the
place, the path below it, the length and SHA-256), the file bytes in manifest order, and a SHA-256 of everything before it.
Without a passphrase the body is plain: the file is **not encrypted**, and the page says so before and after. With a passphrase
(8 or more characters) the body is cut into 64 KiB pieces, each sealed with XChaCha20-Poly1305 under an Argon2id key (the primitives
the vault already uses, nothing new). A dropped or changed piece, or a wrong passphrase, is detected.

A format number higher than this release knows is refused ("made by a newer IYAKKI ... nothing was changed"). Older formats: only
1 exists today; the manifest ignores fields it does not know and defaults the ones an older writer left out.

## Writing

You choose the folder or stick; nothing is pre-chosen and nothing is written elsewhere. The file is written as `….iybak.part` and
renamed when complete; an existing file is never replaced; free space is checked first. It is named
`iyakki-backup-<account>-<date>-<time>.iybak`.

## Restoring

1. Open the file. The whole file is read and checked (format, every file's SHA-256, the total checksum, no extra bytes). Only then
   is the content shown. Nothing has changed yet.
2. Tick the areas you want. None is ticked for you.
3. Type RESTORE. Files are first written next to their final place (`.iyakki-restore-…`) and checked again; then what is there now
   is renamed to `<name>.before-restore-<time>` and the restored data moves in. Nothing is deleted, nothing is overwritten. If a step
   fails, the earlier moves are undone.
4. A file made for another account asks for that account's name to be typed, and its vault is never restored. Where a file lands is
   decided by the table and the signed-in person, never by a name in the file; `..`, absolute, empty, backslash or control-character
   names and unknown places refuse the whole file.

A running session keeps its old data in memory until it is reopened. After restoring the vault, sign out and in.

## Not done

Schedule, a network share as destination, the `iyakki backup` command, system-wide data (VPN profiles, Git connections, Gitea,
the store), incremental backups, a progress bar and cancel (a big backup runs in the background but says only "Backing up").
Older-schema files are lifted by the loaders of each file on the next start, not by the restore.

## Trying it (for the owner, on real media)

1. Back up with a stick plugged in: Settings -> Backup and restore, Back up, choose the stick's folder with the folder button. Check
   the file appears with the right size and no `.part` is left.
2. With and without a passphrase. Try `strings` on the file: without a passphrase you will see names and text; with one, nothing.
3. Open it on the same machine, then change something (delete a dashboard, edit a flow), restore only that area, and check the old
   version sits next to it as `…before-restore-…`.
4. Unplug the stick during a backup: no file with the real name should exist.
5. Open the file as another account: it must ask for the account name.
6. Flip one byte of the file with a hex editor: opening must say it is damaged and change nothing.
