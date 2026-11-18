# Host files (window mode)

In **window mode** (`iyakki-desktop` on a normal computer) the IYAKKI account has a home and workspaces of its own, while
your real files live in the computer's home folder. Host files is the one, small bridge between them. It is **off** in the OS
image and in the browser-streamed front end, where it says why (there is no host to reach, or the files are not on your
computer). The desktop turns it on with `iyakki_agent::hostfiles::set_enabled(true)`; `IYAKKI_HOST_HOME` names another
folder as the host home (tests, demos).

Nothing of the computer is reachable until you grant it, and the first use explains in plain words what is being allowed.

## Three things you can do

| | What happens | Where it ends up |
|---|---|---|
| **Import** (copy) | Files and folders are copied into your account. Nothing on the computer changes. | `<home>/Uploads` (a table opens straight away in DATA) |
| **Share a folder** (link) | One folder is read where it is, as `host:<name>`: read-only unless you turn writing on for it. Nothing is copied. | no copy; the media library, DATA, the editor and the assistant read it in place |
| **Save to the computer** (export) | An account file (a DATA view as CSV) is written to a folder you choose. | the host home or a writable shared folder; a name that exists is never replaced silently |

**Link or copy for a folder.** The dialog counts the folder (off the screen thread) and suggests: *Link* for big or media
folders (over 200 MB, over 500 files, or mostly music, video and pictures: no second copy on disk, it stays in step with the
computer), *Copy* for small ones (it keeps working if the folder moves or the drive is unplugged, and you can edit it freely).
Single files are copied. Your choice is remembered per folder. A shared folder can later be **converted to a copy** (copied
into your account, then unshared) or **stopped**, from Host files.

## How to bring files in

* **Drag and drop** files or folders onto the window. An overlay says "Drop to import into <where>"; a dropped table opens in
  DATA, anything else goes to Uploads. A dropped folder asks link or copy. (egui `dropped_files` and `hovered_files`.)
* **Host files…** in the DATA toolbar: a browser rooted at the computer's home folder with multi-select, quick links and the
  shared folders.

## The rules (all in `iyakki_agent::hostfiles`, tested)

* The host side is confined to the host `$HOME` (not `/` or `/root`): `..`, absolute paths elsewhere, links that lead out, and
  any hidden folder (`~/.ssh`, `~/.config`) are refused. Dropped items are the one exception to "inside the home": you
  chose them, so they can be copied (never linked), and folders inside them skip hidden entries and links.
* Nothing is overwritten without asking (keep both, replace, or cancel). A file may be at most 512 MB and one import 2 GB,
  with a plain message; copies run on a worker thread with a progress bar and are written under a temporary name first.
* A shared folder is stored per person (`host-mounts-<user>.json`) and **checked again on every access**: it must still exist
  and still be inside the host home. Removing it takes effect at once, including tables the assistant has open from it
  (`data_*` handles close with a message) and the table open in DATA (closed within a second).
* The assistant's `read_file`, `list_dir`, `write_file` and the `data_*` tools accept `host:<name>/…` paths, inside the
  folder's mode only; writes still ask first as usual.
* The audit log records `host.consent`, `host.mount.add`, `host.mount.remove`, `host.mount.mode`, `host.import`,
  `host.export` and `host.export.refused`. The window title reads "IYAKKI · Host access: n folders" while folders are shared.

## The media library

A shared folder is scanned and played in place, read-only, and listed on the Folders page as `host:<name>` with "host ·
read-only". If it is unplugged or moved it shows "not available" and the songs leave the list; when it returns the library
looks again by itself (about every 5 s). Nothing is ever written to the folder: no tags, no index files.

## Where things are

* **Settings → Host access** (everyone; in other modes it only explains why Host files is unavailable). Lists each shared folder
  with its mode, file count and size (counted in the background), and whether it is available or unplugged. Keys and buttons:
  `a` Add folder…, `o`/Enter Options… (change mode, browse and import from it, convert to a copy, stop sharing), `m` change
  mode, `c` convert to a copy, `u` unlink, `i` import files…, `l` the audit trail of every `host.*` event. The first-use
  explanation is shown on the page and again before the first grant.
* **Drops** go where the app on screen says (`App::on_drop` returns `DropAction`):

  | On | What happens |
  |---|---|
  | DATA | a table is copied into Uploads and opened; a folder asks link or copy |
  | AI IDE | into the file-tree folder under the pointer (else the workspace root), with the overwrite prompt; the first file then opens in the editor, a CSV or TSV in DATA |
  | CHAT (and the IDE's chat is not included) | text files (up to 32 KB) are quoted into the next message; pictures are saved in Uploads and named so the assistant can `show_image` them; other files are saved in Uploads and listed by name; folders are not attached; the audit log gets `chat.attach` |
  | MEDIA | into the first music folder the account may write to (a dropped folder can be linked read-only instead); the library looks again |
  | BROWSER | nothing: "drop files on DATA, the AI IDE or the chat" |
  | anything else | the import dialog for Uploads |

* **Save to the computer**: the editor (Ctrl+Shift+H, or "Save a copy on this computer…" in its right-click menu) and the chat
  (Ctrl+Shift+H saves the last answer: its table as `result.csv`, else its first code block, else `answer.md`). The folder chooser
  is rooted at the host home or a writable shared folder; a name that exists is refused, never replaced.
* **Media**: a linked folder is listed on the Folders page as `host:<name>` with "host · read-only"; `c` converts it to a copy and
  `u` stops sharing it (both open the same options dialog).
* **The AI IDE terminal and run/test commands** find the tools the CLI tile installed (kubectl, helm, claude, ...): that folder
  is put in front of `PATH` for them.

## For other tiles: saving and importing

Tiles that produce a file (the story and dash tiles' exports, anything new) do not need their own host code:

* **Save to the computer**: implement `App::take_host_save(&mut self) -> Option<(String, Vec<u8>)>` and return the suggested name
  and the bytes when the person asks (a key such as Ctrl+Shift+H and an entry in the context menu are the convention).
  The shell shows `HostDialog::save_bytes` (window mode only; otherwise it says why not). Nothing is written without the
  person choosing a folder and a name, and an existing file is never replaced.
* **Drops**: implement `App::on_drop` (`DropAction::Into { dest, label }`, `Taken`, `Ignore(message)` or the default `Uploads`)
  and `App::dropped_done(files, cx)` to react to what arrived.
* **Options of a shared folder**: `App::take_host_mount_menu` returns a mount name to open its options.
* Reading and writing shared folders from tools: use `iyakki_agent::scope::FileRoots` (`resolve`, `read_ok`, `write_ok`).

## What was not tested

Real drag-and-drop from a file manager cannot be exercised headless. The shell side (`Shell::set_drag_hover`,
`Shell::drop_files`, the overlay drawing, the dialog it opens) is tested by calling those functions; the egui wiring in
`crates/iyakki-gui/src/desktop.rs` (read `hovered_files` and `dropped_files` each frame, forward them, set the title) is
small and was only compiled. Try it by running `./run.sh` and dragging a CSV onto the window.

There is no native file dialog (a built-in browser is used; adding `rfd` or the desktop
portals would need a licence check first).
