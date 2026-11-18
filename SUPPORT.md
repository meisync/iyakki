# Getting help and reporting problems (alpha)

IYAKKI 0.1.0-alpha.1 is a preview built by a small team. There is no support contract; there is a good-faith effort to read what you send.

## Before you ask

1. [docs/QUICKSTART.md](docs/QUICKSTART.md) and [docs/INSTALL.md](docs/INSTALL.md) for the first steps, [docs/USER-GUIDE.md](docs/USER-GUIDE.md) for the tiles.
2. [KNOWN-ISSUES.md](KNOWN-ISSUES.md): is it already listed?
3. [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) and [docs/FAQ.md](docs/FAQ.md) when they exist in your copy.

## Reporting a problem

Open an issue on the project's GitHub repository (Issues for a bug, Discussions for a question or an idea). Please include:

* the version: `iyakki --version`, or SYSTEM, About (it shows the build date, git describe, variant and kernel),
* how you run it: ISO or install image on a PC (model, firmware BIOS/UEFI), QEMU or another VM, the window tarball, a container,
* what you did, what you expected, what happened (a photo of the screen is fine),
* for a boot or hardware problem: the output of `iyakki hwreport` (or the Boot hardware check) and the serial log if you have one,
* for a crash: `crash.log` in the data folder (window mode: `~/.local/share/iyakki-dev`).

Security problems go to the address in [SECURITY.md](SECURITY.md), not to a public issue.

## Testing hardware

The most useful thing an alpha tester can do is boot the image on a real machine and report. [docs/BOOT-TEST.md](docs/BOOT-TEST.md) is the
checklist; for a Mac mini see [docs/MAC-HARDWARE.md](docs/MAC-HARDWARE.md) section 10.
