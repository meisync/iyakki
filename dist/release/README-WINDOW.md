# IYAKKI in a window (Linux x86_64, alpha)

This tarball runs the IYAKKI shell as an ordinary program in a desktop window. It is the quickest way to look at IYAKKI; it is
not the operating system image (for that use the `.iso` or the install image from the same download page).

## Run it

    tar xzf iyakki-*-linux-x86_64-window.tar.gz
    cd iyakki-*-linux-x86_64-window
    ./run-window.sh

`./iyakki-desktop --version` prints the version. The first start asks you to create the administrator account.

## What you need

* Linux on x86_64 with a graphical session (Wayland or X11) and working OpenGL; glibc 2.31 or newer is a safe bet.
* About 1 GB of free RAM and 200 MB of disk for the program and its data.
* To use the assistant, a model: a local Ollama or a cloud key you add yourself (Settings).

## What it does and does not do

* It shows **virtual disks only**, never your real drives (start with `--real-disks` if you really want to see them).
* Data is kept in `~/.local/share/iyakki-dev`.
* It is a build for looking and testing: the checks in `KNOWN-ISSUES.md` apply, and some tiles that need the real image
  (hardware, boot, drivers) show what a window can show.
* macOS and Windows builds are not part of this alpha.

## Verify the download

Compare the SHA-256 with `SHA256SUMS` on the download page (`sha256sum -c SHA256SUMS --ignore-missing`), and check the
signature with `iyakki release verify . --pubkey <key from the page>`.

Licence: Apache-2.0 (`LICENSE`, `NOTICE`, `THIRD_PARTY_LICENSES.txt`).
